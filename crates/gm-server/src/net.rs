//! QUIC connection handling (PROTOCOL.md 1, 8): handshake on the control stream, datagram
//! receive loop, control message writer. Everything the tick loop needs arrives as
//! [`ClientEvent`]s on one channel.

use std::sync::Arc;
use std::time::Duration;

use gm_core::vocab::EntityId;
use gm_net::PROTOCOL_VERSION;
use gm_net::control::{self, Control};
use gm_net::input::InputDatagram;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// What the tick loop hands back to a joining connection.
#[derive(Clone, Copy, Debug)]
pub struct JoinInfo {
    pub entity: EntityId,
    pub server_tick: u32,
}

pub enum ClientEvent {
    Join {
        name: String,
        conn: quinn::Connection,
        control: mpsc::Sender<Control>,
        reply: oneshot::Sender<Result<JoinInfo, String>>,
    },
    Input {
        id: EntityId,
        datagram: InputDatagram,
    },
    Malformed {
        id: EntityId,
    },
    Chat {
        id: EntityId,
        text: String,
    },
    Leave {
        id: EntityId,
    },
}

pub struct NetConfig {
    pub hz: u16,
    pub map_name: String,
    pub map_hash: u64,
    /// Accept empty session tokens (Phase 2 development and tests).
    pub open: bool,
}

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const EVENT_CHANNEL: usize = 4096;
/// Reliable messages queued per client before it counts as not reading.
pub const CONTROL_CHANNEL: usize = 256;

/// Accept connections until the endpoint closes.
pub async fn accept_loop(
    endpoint: quinn::Endpoint,
    tx: mpsc::Sender<ClientEvent>,
    cfg: Arc<NetConfig>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let tx = tx.clone();
        let cfg = cfg.clone();
        tokio::spawn(async move {
            let remote = incoming.remote_address();
            if let Err(e) = handle_connection(incoming, tx, cfg).await {
                debug!(%remote, "connection ended: {e:#}");
            }
        });
    }
}

async fn handle_connection(
    incoming: quinn::Incoming,
    tx: mpsc::Sender<ClientEvent>,
    cfg: Arc<NetConfig>,
) -> anyhow::Result<()> {
    let conn = incoming.await?;
    let remote = conn.remote_address();
    let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.accept_bi())
        .await
        .map_err(|_| anyhow::anyhow!("no control stream within the handshake timeout"))??;
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, control::recv(&mut recv))
        .await
        .map_err(|_| anyhow::anyhow!("no Hello within the handshake timeout"))??;
    let Some(Control::Hello {
        version,
        name,
        token,
    }) = hello
    else {
        anyhow::bail!("first control message was not Hello");
    };
    let reject = |reason: &str| Control::Reject(reason.to_string());
    let rejection = if version != PROTOCOL_VERSION as u16 {
        Some(reject("protocol version mismatch"))
    } else if token.is_empty() && !cfg.open {
        Some(reject("session token required"))
    } else {
        None
    };
    let name = match (rejection, control::valid_name(&name)) {
        (Some(r), _) => {
            control::send(&mut send, &r).await?;
            conn.close(1u32.into(), b"rejected");
            return Ok(());
        }
        (None, None) => {
            control::send(&mut send, &reject("invalid name")).await?;
            conn.close(1u32.into(), b"rejected");
            return Ok(());
        }
        (None, Some(n)) => n,
    };

    let (control_tx, mut control_rx) = mpsc::channel(CONTROL_CHANNEL);
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(ClientEvent::Join {
        name: name.clone(),
        conn: conn.clone(),
        control: control_tx,
        reply: reply_tx,
    })
    .await
    .map_err(|_| anyhow::anyhow!("zone stopped"))?;
    let info = match reply_rx.await {
        Ok(Ok(info)) => info,
        Ok(Err(reason)) => {
            control::send(&mut send, &Control::Reject(reason)).await?;
            conn.close(1u32.into(), b"rejected");
            return Ok(());
        }
        Err(_) => anyhow::bail!("zone stopped during join"),
    };
    control::send(
        &mut send,
        &Control::Welcome {
            entity: info.entity,
            server_tick: info.server_tick,
            hz: cfg.hz,
            map: cfg.map_name.clone(),
            map_hash: cfg.map_hash,
        },
    )
    .await?;
    info!(%remote, entity = info.entity, %name, "player joined");
    let id = info.entity;

    let writer = tokio::spawn(async move {
        while let Some(msg) = control_rx.recv().await {
            if control::send(&mut send, &msg).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });

    loop {
        tokio::select! {
            dg = conn.read_datagram() => {
                match dg {
                    Ok(bytes) => match InputDatagram::decode(&bytes) {
                        Ok(datagram) => {
                            if tx.send(ClientEvent::Input { id, datagram }).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            debug!(entity = id, "malformed input datagram: {e}");
                            if tx.send(ClientEvent::Malformed { id }).await.is_err() {
                                break;
                            }
                        }
                    },
                    Err(e) => {
                        debug!(entity = id, "connection closed: {e}");
                        break;
                    }
                }
            }
            msg = control::recv(&mut recv) => {
                match msg {
                    Ok(Some(Control::Chat(text))) => {
                        if tx.send(ClientEvent::Chat { id, text }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(Control::Bye)) | Ok(None) => break,
                    Ok(Some(other)) => warn!(entity = id, "unexpected control message {other:?}"),
                    Err(e) => {
                        debug!(entity = id, "control stream ended: {e}");
                        break;
                    }
                }
            }
        }
    }
    let _ = tx.send(ClientEvent::Leave { id }).await;
    writer.abort();
    info!(%remote, entity = id, "player left");
    Ok(())
}
