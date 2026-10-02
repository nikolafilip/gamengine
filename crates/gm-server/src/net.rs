//! QUIC connection handling (PROTOCOL.md 1, 8): handshake on the control stream, datagram
//! receive loop, control message writer. Everything the tick loop needs arrives as
//! [`ClientEvent`]s on one channel.

use std::sync::Arc;
use std::time::Duration;

use glam::Vec3;
use gm_core::build::{Build, ContentPack};
use gm_core::vocab::EntityId;
use gm_hub_proto::protocol::{CharacterId, ModelId, ModelRef, StallSummary};
use gm_net::PROTOCOL_VERSION;
use gm_net::control::{self, BuildChoice, Control};
use gm_net::input::InputDatagram;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// What the tick loop hands back to a joining connection.
#[derive(Clone, Debug)]
pub struct JoinInfo {
    pub entity: EntityId,
    pub server_tick: u32,
    pub build: Build,
    pub team: u8,
}

/// A character the hub claimed for this zone (HUB.md 3.1).
#[derive(Clone, Debug)]
pub struct HubJoin {
    pub character: CharacterId,
    /// Where it was in this zone, if the saved position belongs here.
    pub origin: Option<(Vec3, f32)>,
    pub play_seconds: u32,
    /// The avatar model the hub says the character wears (MODELS.md 6.3).
    pub model: Option<ModelRef>,
}

pub enum ClientEvent {
    Join {
        name: String,
        build: Option<BuildChoice>,
        team: u8,
        hub: Option<HubJoin>,
        conn: quinn::Connection,
        control: mpsc::Sender<Control>,
        reply: oneshot::Sender<Result<JoinInfo, String>>,
    },
    /// The client asks to move to another zone (HUB.md 3.3).
    Travel {
        id: EntityId,
        zone: String,
    },
    /// The hub answered a handoff with a ticket (or refused).
    TravelResult {
        id: EntityId,
        result: Result<gm_hub_proto::protocol::ZoneTicket, String>,
    },
    /// The hub says another zone claimed this character: drop the ghost.
    HubClaimed {
        character: CharacterId,
    },
    /// The hub says this character must leave (logout, operator).
    HubKick {
        character: CharacterId,
        reason: String,
    },
    /// The hub says a model was taken down: nobody wears it any more (MODELS.md 7).
    HubModelRevoked {
        model: ModelId,
    },
    /// The client wants a stall on the tile it stands on, or its stall closed (ECONOMY.md 7).
    StallOpen {
        id: EntityId,
    },
    StallClose {
        id: EntityId,
    },
    /// The hub's answers, and its word that a stall of this zone closed.
    StallOpened {
        id: EntityId,
        result: Result<StallSummary, String>,
    },
    StallCloseResult {
        id: EntityId,
        result: Result<(), String>,
    },
    StallsLoaded(Vec<StallSummary>),
    HubStallClosed {
        stall: i64,
    },
    Respec {
        id: EntityId,
        build: BuildChoice,
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
    /// Sent to every client after `Welcome` (MATRIX.md 10).
    pub content: Arc<ContentPack>,
    /// The hub, when this zone runs under one: tokens are then mandatory.
    pub hub: Option<Arc<crate::hub_link::HubLink>>,
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
        build,
        team,
    }) = hello
    else {
        anyhow::bail!("first control message was not Hello");
    };
    let reject = |reason: &str| Control::Reject(reason.to_string());
    let rejection = if version != PROTOCOL_VERSION as u16 {
        Some(reject("protocol version mismatch"))
    } else if token.is_empty() && (!cfg.open || cfg.hub.is_some()) {
        Some(reject("session token required"))
    } else {
        None
    };
    if let Some(r) = rejection {
        control::send(&mut send, &r).await?;
        conn.close(1u32.into(), b"rejected");
        return Ok(());
    }
    // Under a hub the token names the character; `Hello.name` is ignored (HUB.md 3.1).
    let (name, build, hub_join) = match &cfg.hub {
        Some(hub) if !token.is_empty() => {
            if let Err(e) = hub.verify(&token) {
                control::send(&mut send, &reject(&e)).await?;
                conn.close(1u32.into(), b"rejected");
                return Ok(());
            }
            match hub.claim(&token).await {
                Ok(claimed) => {
                    let origin = (claimed.state.zone.as_deref() == Some(hub.zone.as_str()))
                        .then(|| (Vec3::from(claimed.state.position), claimed.state.yaw));
                    (
                        claimed.name.clone(),
                        Some(BuildChoice::Custom(claimed.state.build.clone())),
                        Some(HubJoin {
                            character: claimed.character,
                            origin,
                            play_seconds: claimed.state.play_seconds,
                            model: claimed.model,
                        }),
                    )
                }
                Err(e) => {
                    control::send(&mut send, &reject(&format!("claim failed: {e}"))).await?;
                    conn.close(1u32.into(), b"rejected");
                    return Ok(());
                }
            }
        }
        _ => match control::valid_name(&name) {
            Some(n) => (n, build, None),
            None => {
                control::send(&mut send, &reject("invalid name")).await?;
                conn.close(1u32.into(), b"rejected");
                return Ok(());
            }
        },
    };

    let (control_tx, mut control_rx) = mpsc::channel(CONTROL_CHANNEL);
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(ClientEvent::Join {
        name: name.clone(),
        build,
        team,
        hub: hub_join,
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
    control::send(
        &mut send,
        &Control::Content {
            pack: (*cfg.content).clone(),
            own: info.build.clone(),
            team: info.team,
        },
    )
    .await?;
    info!(%remote, entity = info.entity, %name, team = info.team, "player joined");
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
                    Ok(Some(Control::Respec(build))) => {
                        if tx.send(ClientEvent::Respec { id, build }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(Control::Travel(zone))) => {
                        if tx.send(ClientEvent::Travel { id, zone }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(Control::StallOpen)) => {
                        if tx.send(ClientEvent::StallOpen { id }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(Control::StallClose)) => {
                        if tx.send(ClientEvent::StallClose { id }).await.is_err() {
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
