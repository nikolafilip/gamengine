//! The browser client's connection to a zone (WEB.md 2.1, 3.3): the same handshake and the
//! same bytes as the native one, on a `WebTransport` session driven by the page's event loop.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gm_net::PROTOCOL_VERSION;
use gm_net::control::{self, BuildChoice, FromClient, FromZone};
use wasm_bindgen_futures::spawn_local;

use super::timeout_ms;
use super::wt::{Reader, Session, Writer};
use crate::Error;
use crate::net::{NetEvent, ZoneAddr};

/// Snapshots waiting for a frame. Reader tasks run whether or not frames do (a hidden tab
/// has no frames): past this many the oldest goes, as it would in the browser's own queue.
const MAX_QUEUED_SNAPSHOTS: usize = 32;
/// Events of any kind waiting for a frame; past this the session is given up.
const MAX_QUEUED_EVENTS: usize = 4096;
/// The longest the connect and the handshake may each take, milliseconds.
const HANDSHAKE_MS: u32 = 10_000;
/// The longest a `Bye` may hold up the close.
const BYE_MS: u32 = 500;

struct Live {
    session: Session,
    control: Writer,
}

#[derive(Default)]
struct Shared {
    events: VecDeque<NetEvent>,
    queued_snapshots: usize,
    live: Option<Rc<Live>>,
    /// Reliable messages queued before the session was up.
    early: Vec<FromClient>,
    /// The client hung up itself: what follows is not a disconnect to report.
    closing: bool,
    /// Payload bytes in and out since the start (the zone counts the UDP bytes).
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl Shared {
    fn push(&mut self, ev: NetEvent) {
        if matches!(ev, NetEvent::Snapshot(_)) {
            if self.queued_snapshots >= MAX_QUEUED_SNAPSHOTS
                && let Some(i) = self
                    .events
                    .iter()
                    .position(|e| matches!(e, NetEvent::Snapshot(_)))
            {
                self.events.remove(i);
                self.queued_snapshots -= 1;
            }
            self.queued_snapshots += 1;
        }
        self.events.push_back(ev);
    }
}

pub struct NetClient {
    shared: Rc<RefCell<Shared>>,
}

impl NetClient {
    /// Connect in the background; events arrive through `poll`.
    pub fn connect(
        zone: ZoneAddr,
        name: String,
        build: Option<String>,
        team: u8,
        token: Vec<u8>,
    ) -> Result<NetClient, Error> {
        let web = zone.web.ok_or("this zone has no web listener")?;
        let shared = Rc::new(RefCell::new(Shared::default()));
        let task = shared.clone();
        spawn_local(async move {
            let hello = FromClient::Hello {
                version: PROTOCOL_VERSION as u16,
                name,
                token,
                build: build.map(BuildChoice::Preset),
                team,
            };
            let reason = match session(&web, hello, task.clone()).await {
                Ok(reason) => reason,
                Err(e) => e,
            };
            let mut s = task.borrow_mut();
            s.live = None;
            if !s.closing {
                s.push(NetEvent::Disconnected(reason));
            }
        });
        Ok(NetClient { shared })
    }

    pub fn send_input(&self, datagram: Vec<u8>) {
        let mut s = self.shared.borrow_mut();
        if let Some(live) = &s.live {
            live.session.send_datagram(&datagram);
            s.tx_bytes += datagram.len() as u64;
        }
    }

    /// Queue a reliable message (chat, respec).
    pub fn send_control(&self, msg: FromClient) {
        let mut s = self.shared.borrow_mut();
        match s.live.clone() {
            Some(live) => {
                if let Ok(bytes) = control::encode_framed(&msg) {
                    s.tx_bytes += bytes.len() as u64;
                    live.control.write_detached(&bytes);
                }
            }
            None => s.early.push(msg),
        }
    }

    /// Say goodbye and close. Nothing here may block: the `Bye` is written and the session
    /// closed behind it.
    pub fn close(&mut self) {
        let live = {
            let mut s = self.shared.borrow_mut();
            s.closing = true;
            s.live.take()
        };
        if let Some(live) = live {
            spawn_local(hang_up(live));
        }
    }

    /// Begin to say goodbye without waiting for it: here that is all `close` ever does.
    pub fn hang_up(&mut self) {
        self.close();
    }

    /// Whether the connection has ended: nothing here is waited for.
    pub fn gone(&self) -> bool {
        true
    }

    /// Everything that arrived since the last call.
    pub fn poll(&self) -> Vec<NetEvent> {
        let mut s = self.shared.borrow_mut();
        s.queued_snapshots = 0;
        s.events.drain(..).collect()
    }

    /// Payload bytes received and sent so far.
    pub fn bytes(&self) -> (u64, u64) {
        let s = self.shared.borrow();
        (s.rx_bytes, s.tx_bytes)
    }
}

impl Drop for NetClient {
    fn drop(&mut self) {
        self.close();
    }
}

async fn read_control(reader: &mut Reader) -> Result<Option<FromZone>, String> {
    match reader.frame().await? {
        Some(payload) => bitcode::decode(&payload)
            .map(Some)
            .map_err(|e| format!("control message did not decode: {e}")),
        None => Ok(None),
    }
}

/// The handshake and the life of the session; returns why it ended. Whatever the way out,
/// the session is closed: dropping the wrapper of a `WebTransport` does not close it, and a
/// zone that has admitted the player would keep the body standing.
async fn session(
    web: &gm_net::control::WebAddr,
    hello: FromClient,
    shared: Rc<RefCell<Shared>>,
) -> Result<String, String> {
    let session = match timeout_ms(HANDSHAKE_MS, Session::connect(web)).await {
        Some(s) => s?,
        None => return Err(format!("connecting to {}: no answer", web.url)),
    };
    let closing = || shared.borrow().closing;
    if closing() {
        session.close();
        return Ok("closed".into());
    }
    let handshake = async {
        let (control, mut reader) = session.open_bi().await?;
        control
            .write(&control::encode_framed(&hello).map_err(|e| e.to_string())?)
            .await?;
        let first = read_control(&mut reader).await?;
        Ok::<_, String>((control, reader, first))
    };
    let (control, mut reader, first) = match timeout_ms(HANDSHAKE_MS, handshake).await {
        Some(Ok(h)) => h,
        Some(Err(e)) => {
            session.close();
            return Err(e);
        }
        None => {
            session.close();
            return Err("the zone did not answer the handshake".into());
        }
    };
    let welcome = match first {
        Some(FromZone::Welcome {
            entity,
            hz,
            map,
            map_hash,
            ..
        }) => NetEvent::Welcome {
            entity,
            hz,
            map,
            map_hash,
        },
        Some(FromZone::Reject(reason)) => {
            session.close();
            return Err(format!("rejected: {reason}"));
        }
        _ => {
            session.close();
            return Err("the zone's first message was not a welcome".into());
        }
    };
    let datagrams = match session.datagram_reader() {
        Ok(d) => d,
        Err(e) => {
            session.close();
            return Err(e);
        }
    };
    let live = Rc::new(Live { session, control });
    if closing() {
        // `close()` came while the zone was admitting us: leave properly.
        hang_up(live).await;
        return Ok("closed".into());
    }
    {
        let mut s = shared.borrow_mut();
        s.push(welcome);
        for msg in std::mem::take(&mut s.early) {
            if let Ok(bytes) = control::encode_framed(&msg) {
                s.tx_bytes += bytes.len() as u64;
                live.control.write_detached(&bytes);
            }
        }
        s.live = Some(live.clone());
    }
    // Datagrams: snapshots, one per read.
    {
        let shared = shared.clone();
        let mut datagrams = datagrams;
        spawn_local(async move {
            while let Ok(Some(bytes)) = datagrams.chunk().await {
                let mut s = shared.borrow_mut();
                s.rx_bytes += bytes.len() as u64;
                s.push(NetEvent::Snapshot(bytes));
            }
        });
    }
    // The control stream, until it ends, a kick arrives or the session goes.
    let controls = async {
        loop {
            match read_control(&mut reader).await {
                Ok(Some(FromZone::Kick(reason))) => return format!("kicked: {reason}"),
                Ok(Some(msg)) => {
                    let mut s = shared.borrow_mut();
                    // Nobody drains the queue (a hidden tab): a zone may not fill memory
                    // with reliable messages either.
                    if s.events.len() >= MAX_QUEUED_EVENTS {
                        return "the page stopped reading the zone's messages".to_string();
                    }
                    s.push(NetEvent::Control(msg));
                }
                Ok(None) => return "connection closed".to_string(),
                Err(e) => return e,
            }
        }
    };
    let reason = first_of(controls, live.session.closed()).await;
    live.session.close();
    Ok(reason)
}

/// Say `Bye` and close; a stream that never takes the `Bye` does not hold the close up.
async fn hang_up(live: Rc<Live>) {
    if let Ok(bytes) = control::encode_framed(&FromClient::Bye) {
        let _ = timeout_ms(BYE_MS, live.control.write(&bytes)).await;
    }
    live.session.close();
}

/// The output of whichever future finishes first (the other is dropped).
async fn first_of<A, B>(a: A, b: B) -> String
where
    A: Future<Output = String>,
    B: Future<Output = String>,
{
    use std::pin::pin;
    use std::task::Poll;
    let mut a = pin!(a);
    let mut b = pin!(b);
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) {
            return Poll::Ready(v);
        }
        if let Poll::Ready(v) = b.as_mut().poll(cx) {
            return Poll::Ready(v);
        }
        Poll::Pending
    })
    .await
}
