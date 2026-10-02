//! The zone's connection to the hub (HUB.md): registration, heartbeats, claims, saves,
//! handoffs and notices. Everything here is best effort from the tick loop's point of view:
//! requests run on their own tasks and come back as `ClientEvent`s.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gm_hub_proto::protocol::{
    CharacterId, CharacterState, EconReply, HubNotice, HubRequest, HubResponse, ModelRef,
    SessionToken, StallSummary, TokenPayload, ZoneEconOp, ZoneId, ZoneTicket, now_secs,
};
use gm_hub_proto::{HubClient, HubClientError, TokenError, TokenVerifier};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::net::ClientEvent;

pub struct HubLinkConfig {
    pub addr: SocketAddr,
    pub cert_der: Vec<u8>,
    pub zone: ZoneId,
    pub secret: String,
    pub map: String,
    pub map_hash: u64,
    /// What clients connect to.
    pub public_addr: SocketAddr,
    pub zone_cert_der: Vec<u8>,
}

pub struct HubLink {
    pub client: HubClient,
    pub zone: ZoneId,
    verifier: Mutex<TokenVerifier>,
}

/// What a verified token says about the joiner.
#[derive(Clone, Debug)]
pub struct Claimed {
    pub character: CharacterId,
    pub name: String,
    pub state: CharacterState,
    pub team: u8,
    /// The avatar model, while it is active (MODELS.md 6.3).
    pub model: Option<ModelRef>,
}

impl HubLink {
    /// Connect and register; fails if the hub refuses.
    pub async fn connect(cfg: HubLinkConfig) -> anyhow::Result<Arc<HubLink>> {
        let client = HubClient::connect_with_cert(cfg.addr, cfg.cert_der).await?;
        let resp = client
            .request(&HubRequest::ZoneHello {
                secret: cfg.secret,
                zone: cfg.zone.clone(),
                map: cfg.map,
                map_hash: cfg.map_hash,
                addr: cfg.public_addr,
                cert_der: cfg.zone_cert_der,
            })
            .await?;
        let HubResponse::Registered { public_key } = resp else {
            anyhow::bail!("hub did not register the zone: {resp:?}");
        };
        info!(hub = %cfg.addr, zone = %cfg.zone, "registered with the hub");
        Ok(Arc::new(HubLink {
            client,
            zone: cfg.zone.clone(),
            verifier: Mutex::new(TokenVerifier::new(public_key, &cfg.zone)?),
        }))
    }

    /// Verify a `Hello` token offline (HUB.md 3.1).
    pub fn verify(&self, token_bytes: &[u8]) -> Result<TokenPayload, String> {
        let token: SessionToken =
            bitcode::decode(token_bytes).map_err(|_| "malformed token".to_string())?;
        self.verifier
            .lock()
            .unwrap()
            .accept(&token, now_secs())
            .map_err(|e: TokenError| e.to_string())
    }

    /// Claim the character the token names.
    pub async fn claim(&self, token_bytes: &[u8]) -> Result<Claimed, String> {
        let token: SessionToken =
            bitcode::decode(token_bytes).map_err(|_| "malformed token".to_string())?;
        match self.client.request(&HubRequest::Claim { token }).await {
            Ok(HubResponse::Claimed {
                character,
                name,
                state,
                team,
                model,
            }) => Ok(Claimed {
                character,
                name,
                state,
                team,
                model,
            }),
            Ok(other) => Err(format!("unexpected hub answer {other:?}")),
            Err(HubClientError::Refused(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub async fn save(&self, character: CharacterId, state: CharacterState, leaving: bool) {
        if let Err(e) = self
            .client
            .ok(&HubRequest::Save {
                character,
                state,
                leaving,
            })
            .await
        {
            warn!(character, leaving, "save refused: {e}");
        }
    }

    pub async fn handoff(
        &self,
        character: CharacterId,
        state: CharacterState,
        to_zone: ZoneId,
    ) -> Result<ZoneTicket, String> {
        match self
            .client
            .request(&HubRequest::Handoff {
                character,
                state,
                to_zone,
            })
            .await
        {
            Ok(HubResponse::Ticket(t)) => Ok(t),
            Ok(other) => Err(format!("unexpected hub answer {other:?}")),
            Err(HubClientError::Refused(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// One economy request of this zone (ECONOMY.md); the error is what the player is told.
    async fn econ(&self, op: ZoneEconOp) -> Result<EconReply, String> {
        match self.client.request(&HubRequest::ZoneEcon(op)).await {
            Ok(HubResponse::Econ(r)) => Ok(r),
            Ok(other) => Err(format!("unexpected hub answer {other:?}")),
            Err(HubClientError::Refused(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Open a stall for a character standing on tile `(tile_x, tile_y)`.
    pub async fn stall_open(
        &self,
        character: CharacterId,
        tile_x: i32,
        tile_y: i32,
    ) -> Result<StallSummary, String> {
        match self
            .econ(ZoneEconOp::StallOpen {
                character,
                tile_x,
                tile_y,
            })
            .await?
        {
            EconReply::Stall(s) => Ok(s),
            other => Err(format!("unexpected hub answer {other:?}")),
        }
    }

    pub async fn stall_close(&self, character: CharacterId) -> Result<(), String> {
        self.econ(ZoneEconOp::StallClose { character })
            .await
            .map(|_| ())
    }

    /// Every open stall of this zone.
    pub async fn stalls(&self) -> Result<Vec<StallSummary>, String> {
        match self.econ(ZoneEconOp::Stalls).await? {
            EconReply::Stalls(s) => Ok(s),
            other => Err(format!("unexpected hub answer {other:?}")),
        }
    }

    /// Heartbeat every 5 s with the latest player count and tick time.
    pub fn spawn_heartbeat(self: &Arc<Self>, stats: Arc<Mutex<(u32, f32)>>) {
        let link = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let (players, tick_mean_us) = *stats.lock().unwrap();
                if link
                    .client
                    .ok(&HubRequest::Heartbeat {
                        players,
                        tick_mean_us,
                    })
                    .await
                    .is_err()
                {
                    warn!("hub heartbeat failed; the hub connection is gone");
                    break;
                }
            }
        });
    }

    /// Forward hub notices to the tick loop.
    pub fn spawn_notice_reader(self: &Arc<Self>, tx: mpsc::Sender<ClientEvent>) {
        let link = self.clone();
        tokio::spawn(async move {
            while let Some(notice) = link.client.notice().await {
                let ev = match notice {
                    HubNotice::Claimed { character } => ClientEvent::HubClaimed { character },
                    HubNotice::Kick { character, reason } => {
                        ClientEvent::HubKick { character, reason }
                    }
                    HubNotice::ModelRevoked { model } => ClientEvent::HubModelRevoked { model },
                    HubNotice::StallClosed { stall } => ClientEvent::HubStallClosed { stall },
                };
                if tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
    }
}
