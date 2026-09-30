//! The client's connection to a zone: a small tokio runtime on its own thread drives quinn;
//! the render thread talks to it through channels and never blocks on the network.

use std::net::SocketAddr;
use std::sync::mpsc as std_mpsc;

use bytes::Bytes;
use gm_net::PROTOCOL_VERSION;
use gm_net::control::{self, Control};
use gm_net::transport::{SERVER_NAME, client_config};
use quinn::rustls::pki_types::CertificateDer;
use tokio::sync::mpsc;

use crate::Error;

pub enum NetEvent {
    Welcome {
        entity: u32,
        hz: u16,
        map: String,
        map_hash: u64,
    },
    Snapshot(Bytes),
    Control(Control),
    Disconnected(String),
}

pub struct NetClient {
    _rt: tokio::runtime::Runtime,
    input_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    events: std_mpsc::Receiver<NetEvent>,
}

impl NetClient {
    /// Connect in the background; events arrive through `poll`.
    pub fn connect(addr: SocketAddr, cert_der: Vec<u8>, name: String) -> Result<NetClient, Error> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("gm-net")
            .build()?;
        let (input_tx, input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (event_tx, events) = std_mpsc::channel::<NetEvent>();
        let cfg = client_config(&[CertificateDer::from(cert_der)])?;
        rt.spawn(async move {
            let result = session(addr, cfg, name, input_rx, event_tx.clone()).await;
            let reason = match result {
                Ok(()) => "connection closed".to_string(),
                Err(e) => format!("{e}"),
            };
            let _ = event_tx.send(NetEvent::Disconnected(reason));
        });
        Ok(NetClient {
            _rt: rt,
            input_tx: Some(input_tx),
            events,
        })
    }

    pub fn send_input(&self, datagram: Vec<u8>) {
        if let Some(tx) = &self.input_tx {
            let _ = tx.send(datagram);
        }
    }

    /// Say goodbye and close the connection, waiting briefly so the zone frees the slot now
    /// instead of after its idle timeout.
    pub fn close(&mut self) {
        self.input_tx = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            match self
                .events
                .recv_timeout(std::time::Duration::from_millis(50))
            {
                Ok(NetEvent::Disconnected(_)) | Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
                Ok(_) => {}
                Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// Everything that arrived since the last call.
    pub fn poll(&self) -> Vec<NetEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            out.push(ev);
        }
        out
    }
}

async fn session(
    addr: SocketAddr,
    cfg: quinn::ClientConfig,
    name: String,
    mut input_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    events: std_mpsc::Sender<NetEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind: SocketAddr = if addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(cfg);
    let conn = endpoint.connect(addr, SERVER_NAME)?.await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    control::send(
        &mut send,
        &Control::Hello {
            version: PROTOCOL_VERSION as u16,
            name,
            token: Vec::new(),
        },
    )
    .await?;
    match control::recv(&mut recv).await? {
        Some(Control::Welcome {
            entity,
            hz,
            map,
            map_hash,
            ..
        }) => {
            let _ = events.send(NetEvent::Welcome {
                entity,
                hz,
                map,
                map_hash,
            });
        }
        Some(Control::Reject(reason)) => return Err(format!("rejected: {reason}").into()),
        other => return Err(format!("unexpected handshake message {other:?}").into()),
    }
    loop {
        tokio::select! {
            input = input_rx.recv() => {
                match input {
                    Some(bytes) => {
                        let _ = conn.send_datagram(Bytes::from(bytes));
                    }
                    None => break,
                }
            }
            dg = conn.read_datagram() => {
                match dg {
                    Ok(bytes) => {
                        if events.send(NetEvent::Snapshot(bytes)).is_err() {
                            break;
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            msg = control::recv(&mut recv) => {
                match msg? {
                    Some(Control::Kick(reason)) => return Err(format!("kicked: {reason}").into()),
                    Some(msg) => {
                        let _ = events.send(NetEvent::Control(msg));
                    }
                    None => break,
                }
            }
        }
    }
    let _ = control::send(&mut send, &Control::Bye).await;
    conn.close(0u32.into(), b"bye");
    endpoint.wait_idle().await;
    Ok(())
}
