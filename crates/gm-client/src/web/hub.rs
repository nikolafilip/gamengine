//! The hub from a browser (WEB.md 2.1): the same requests as natively, one per bidirectional
//! stream of a `WebTransport` session. The session is opened by the first request that needs
//! it and opened again by the next one after it broke.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use gm_hub_proto::player::{
    PLAYER_VERSION, PREAMBLE, PlayerRequest, PlayerResponse, version_words,
};
use gm_hub_proto::protocol::{HubError, MAX_MODEL_BYTES, SessionId};
use gm_model::ModelId;
use gm_net::control::{WebAddr, encode_framed_any};
use wasm_bindgen_futures::spawn_local;

use super::store::WebSource;
use super::wt::Session;
use crate::cache::FetchError;
use crate::hub::{Answer, Pending, RpcError};
use crate::net::ZoneAddr;

/// The longest one model may take to arrive; then the fetch backs off.
const FETCH_TIMEOUT_MS: u32 = 120_000;
/// The longest the hub may take to answer a request (a login hashes a password).
const REQUEST_TIMEOUT_MS: u32 = 15_000;

struct Line {
    web: WebAddr,
    conn: Rc<RefCell<Option<Rc<Session>>>>,
    /// Somebody is opening the session: the others wait for it.
    connecting: Cell<bool>,
    session: Cell<Option<SessionId>>,
}

impl Line {
    /// The session, opened if there is none.
    async fn wt(&self) -> Result<Rc<Session>, RpcError> {
        loop {
            if let Some(wt) = self.conn.borrow().clone() {
                return Ok(wt);
            }
            if self.connecting.replace(true) {
                super::sleep_ms(40).await;
                continue;
            }
            let opened = super::timeout_ms(REQUEST_TIMEOUT_MS, Session::connect(&self.web)).await;
            self.connecting.set(false);
            return match opened {
                Some(Ok(wt)) => {
                    let wt = Rc::new(wt);
                    *self.conn.borrow_mut() = Some(wt.clone());
                    // A session that ends by itself (the machine slept, the network
                    // changed, the hub restarted) is not handed to the next request.
                    let (conn, watched) = (self.conn.clone(), wt.clone());
                    spawn_local(async move {
                        watched.closed().await;
                        let mut conn = conn.borrow_mut();
                        if conn.as_ref().is_some_and(|c| Rc::ptr_eq(c, &watched)) {
                            *conn = None;
                        }
                    });
                    Ok(wt)
                }
                Some(Err(e)) => Err(RpcError::Other(format!("the hub cannot be reached: {e}"))),
                None => Err(RpcError::Other("the hub does not answer".into())),
            };
        }
    }

    /// A session that failed is not used again.
    fn broken(&self, wt: &Rc<Session>) {
        let mut conn = self.conn.borrow_mut();
        if conn.as_ref().is_some_and(|c| Rc::ptr_eq(c, wt)) {
            wt.close();
            *conn = None;
        }
    }

    async fn request(&self, req: &PlayerRequest) -> Answer {
        let wt = self.wt().await?;
        match super::timeout_ms(REQUEST_TIMEOUT_MS, exchange(&wt, req)).await {
            Some(Ok((resp, _))) => Ok(resp),
            Some(Err(RpcError::Refused(e))) => Err(RpcError::Refused(e)),
            Some(Err(e)) => {
                self.broken(&wt);
                Err(e)
            }
            None => {
                self.broken(&wt);
                Err(RpcError::Other("the hub does not answer".into()))
            }
        }
    }
}

/// One request on its own stream, in the players' encoding (the stream begins with the
/// frame that says so); the response, and the reader for what follows it.
async fn exchange(
    wt: &Session,
    req: &PlayerRequest,
) -> Result<(PlayerResponse, super::wt::Reader), RpcError> {
    let other = RpcError::Other;
    let (writer, mut reader) = wt.open_bi().await.map_err(other)?;
    let mut bytes = PREAMBLE.to_vec();
    bytes.extend(encode_framed_any(req).map_err(|e| RpcError::Other(e.to_string()))?);
    writer.write(&bytes).await.map_err(other)?;
    writer.finish();
    // The hub's version first: a client of another build is told so, not garbled at.
    match reader.frame().await.map_err(other)?.as_deref() {
        Some([hub]) if *hub == PLAYER_VERSION => {}
        Some([hub]) => return Err(RpcError::Other(version_words(*hub))),
        _ => {
            return Err(RpcError::Other(
                "the hub closed the stream without an answer".into(),
            ));
        }
    }
    let payload = reader
        .frame()
        .await
        .map_err(other)?
        .ok_or_else(|| RpcError::Other("the hub closed the stream without an answer".into()))?;
    let resp: PlayerResponse = bitcode::decode(&payload)
        .map_err(|e| RpcError::Other(format!("the hub's answer did not decode: {e}")))?;
    match resp {
        PlayerResponse::Err(e) => Err(RpcError::Refused(e)),
        resp => Ok((resp, reader)),
    }
}

/// The hub, as the client sees it: cheap to clone, never blocks a frame.
#[derive(Clone)]
pub struct Hub {
    line: Rc<Line>,
}

impl Hub {
    /// Nothing is sent yet: the first request opens the session.
    pub fn new(hub: &ZoneAddr) -> Result<Hub, crate::Error> {
        let web = hub
            .web
            .clone()
            .ok_or("the page is not configured with a hub (config.json)")?;
        Ok(Hub {
            line: Rc::new(Line {
                web,
                conn: Rc::new(RefCell::new(None)),
                connecting: Cell::new(false),
                session: Cell::new(None),
            }),
        })
    }

    pub fn call(&self, req: PlayerRequest) -> Pending<Answer> {
        let (pending, filler) = Pending::new();
        let line = self.line.clone();
        spawn_local(async move { filler.fill(line.request(&req).await) });
        pending
    }

    /// The session models are fetched on (MODELS.md 6.2): set at login, cleared at logout.
    pub fn set_session(&self, session: Option<SessionId>) {
        self.line.session.set(session);
    }

    pub fn model_source(&self) -> Rc<dyn WebSource> {
        Rc::new(HubSource {
            line: self.line.clone(),
        })
    }

    /// Log out and hang up, behind the frame's back: nothing in a page may block.
    pub fn logout(&self) {
        let session = self.line.session.take();
        let Some(wt) = self.line.conn.borrow_mut().take() else {
            return;
        };
        spawn_local(async move {
            if let Some(session) = session {
                let bye = PlayerRequest::Logout { session };
                let _ = super::timeout_ms(2_000, exchange(&wt, &bye)).await;
            }
            wt.close();
        });
    }
}

struct HubSource {
    line: Rc<Line>,
}

async fn download(line: &Line, req: &PlayerRequest, max: usize) -> Result<Vec<u8>, FetchError> {
    let wt = line
        .wt()
        .await
        .map_err(|e| FetchError::Failed(e.to_string()))?;
    let (resp, mut reader) = match exchange(&wt, req).await {
        Ok(r) => r,
        // Over the rate limit, or the hub is busy: later. A session that ended is not the
        // model's fault either: the next login fetches it.
        Err(RpcError::Refused(HubError::Busy | HubError::Unauthorized)) => {
            return Err(FetchError::Failed("busy".into()));
        }
        Err(RpcError::Refused(e)) => return Err(FetchError::Refused(e.to_string())),
        Err(RpcError::Other(e)) => return Err(FetchError::Failed(e)),
    };
    let PlayerResponse::Blob { len } = resp else {
        return Err(FetchError::Failed(
            "the hub answered with the wrong message".into(),
        ));
    };
    let len = len as usize;
    if len > max {
        return Err(FetchError::Failed(format!(
            "the hub announced {len} bytes; at most {max} were expected"
        )));
    }
    match reader.read_exact(len).await {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) if len == 0 => Ok(Vec::new()),
        Ok(None) => Err(FetchError::Failed("download interrupted".into())),
        Err(e) => Err(FetchError::Failed(format!("download interrupted: {e}"))),
    }
}

impl WebSource for HubSource {
    fn fetch(&self, id: ModelId) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, FetchError>>>> {
        let line = self.line.clone();
        Box::pin(async move {
            let Some(session) = line.session.get() else {
                return Err(FetchError::Failed("not logged in".into()));
            };
            let req = PlayerRequest::ModelGet { session, model: id };
            // A load slot must come back: a stalled stream would otherwise hold one of the
            // four for good.
            match super::timeout_ms(
                FETCH_TIMEOUT_MS,
                download(&line, &req, MAX_MODEL_BYTES as usize),
            )
            .await
            {
                Some(result) => result,
                None => Err(FetchError::Failed("timed out".into())),
            }
        })
    }
}
