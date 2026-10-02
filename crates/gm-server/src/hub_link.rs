//! The zone's connection to the hub (HUB.md): registration, heartbeats, claims, saves,
//! handoffs and notices. Everything here is best effort from the tick loop's point of view:
//! requests run on their own tasks and come back as `ClientEvent`s.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gm_hub_proto::protocol::{
    CharacterId, CharacterState, EconReply, HiredAvatar, HubNotice, HubRequest, HubResponse,
    ModelRef, SessionToken, StallSummary, TokenPayload, ZoneEconOp, ZoneId, ZoneTicket, now_secs,
};
use gm_hub_proto::{HubClient, HubClientError, TokenError, TokenVerifier};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::net::ClientEvent;

/// A kill is reported this many times at most, a quarter of a second apart at first and
/// twice as long each time (eight seconds in all).
const KILL_REPORT_TRIES: u32 = 6;

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
    /// The zone's WebTransport listener (WEB.md 2.3).
    pub web: Option<gm_net::control::WebAddr>,
    /// The least trust tier the zone admits (ANTICHEAT.md 6); 0 = everybody.
    pub min_trust: i16,
    /// Trials that open this zone (COMPANIONS.md 11); empty = open to all.
    pub requires: Vec<String>,
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
    /// The character's active hires (COMPANIONS.md 3.3).
    pub squad: Vec<HiredAvatar>,
}

/// How long a stopping zone waits for the hub to take one last thing.
const LAST_WORDS: Duration = Duration::from_secs(5);

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
                web: cfg.web,
                min_trust: cfg.min_trust,
                requires: cfg.requires,
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
                squad,
            }) => Ok(Claimed {
                character,
                name,
                state,
                team,
                model,
                squad,
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

    /// Everything one kill gives, once (ECONOMY.md 9): the report is repeated until the
    /// hub answers, and the hub pays a `reference` only once. `Ok(false)`: paid before.
    pub async fn grant_kill(
        &self,
        reference: i64,
        components: Vec<(CharacterId, String)>,
        coin: Vec<(CharacterId, i64)>,
    ) -> Result<bool, String> {
        let op = ZoneEconOp::GrantKill {
            reference,
            components,
            coin,
        };
        let mut wait = Duration::from_millis(250);
        let mut last = String::new();
        for _ in 0..KILL_REPORT_TRIES {
            match self.client.request(&HubRequest::ZoneEcon(op.clone())).await {
                Ok(HubResponse::Econ(EconReply::Ids(_))) => return Ok(true),
                Ok(HubResponse::Econ(EconReply::Done)) => return Ok(false),
                Ok(other) => return Err(format!("unexpected hub answer {other:?}")),
                // Busy is the database asking for a repeat; a lost answer is why the report
                // may be repeated at all. Any other refusal will not change by asking again.
                Err(HubClientError::Refused(e)) if e != gm_hub_proto::protocol::HubError::Busy => {
                    return Err(e.to_string());
                }
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(wait).await;
            wait *= 2;
        }
        Err(last)
    }

    /// A character of this zone passed a trial (COMPANIONS.md 11).
    pub async fn trial(
        &self,
        character: CharacterId,
        trial: String,
        secs: u32,
    ) -> Result<(), String> {
        self.client
            .ok(&HubRequest::Trial {
                character,
                trial,
                secs,
            })
            .await
            .map_err(|e| e.to_string())
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

    /// A client's aim numbers since the last report (ANTICHEAT.md 4.3). The nonce makes a
    /// repeated report count once; three tries, then the numbers of this stretch are lost.
    pub async fn aim(&self, character: CharacterId, stats: gm_hub_proto::protocol::AimStats) {
        self.aim_tries(character, stats, 3).await
    }

    /// The same with one try and a short patience: for a zone that is stopping.
    pub async fn aim_once(&self, character: CharacterId, stats: gm_hub_proto::protocol::AimStats) {
        if tokio::time::timeout(LAST_WORDS, self.aim_tries(character, stats, 1))
            .await
            .is_err()
        {
            warn!(
                character,
                "aim numbers not reported: the hub did not answer"
            );
        }
    }

    async fn aim_tries(
        &self,
        character: CharacterId,
        stats: gm_hub_proto::protocol::AimStats,
        tries: u32,
    ) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
            ^ (character as u64).rotate_left(40);
        let req = HubRequest::ZoneAim {
            nonce,
            character,
            stats,
        };
        for attempt in 0..tries {
            match self.client.request(&req).await {
                Ok(_) => return,
                Err(HubClientError::Refused(e)) => {
                    warn!(character, "the hub refused aim numbers: {e}");
                    return;
                }
                Err(e) if attempt + 1 == tries => {
                    warn!(character, "aim numbers not reported: {e}")
                }
                Err(_) => tokio::time::sleep(Duration::from_secs(2 << attempt)).await,
            }
        }
    }

    /// Upload a replay (ANTICHEAT.md 3.3): once a minute for an hour, then the file stays
    /// where the zone wrote it (the next zone started on that directory sends it).
    pub async fn replay(&self, w: crate::recorder::Written) {
        self.replay_tries(w, 60).await
    }

    /// The same with one try and a short patience: for a zone that is stopping.
    pub async fn replay_once(&self, w: crate::recorder::Written) {
        let file = w.path.clone();
        if tokio::time::timeout(LAST_WORDS, self.replay_tries(w, 1))
            .await
            .is_err()
        {
            warn!(file = %file.display(), "replay not uploaded, it stays on disk: the hub did not answer");
        }
    }

    async fn replay_tries(&self, w: crate::recorder::Written, tries: u32) {
        let summary = gm_hub_proto::protocol::ReplaySummary {
            started_unix: w.header.started_unix,
            seconds: w.seconds,
            reported: w.header.reason == gm_replay::Reason::Report,
            reports: w.header.reports.clone(),
            kills: w.kills,
            damage: w.damage,
            participants: w
                .participants
                .iter()
                .filter(|(who, _)| who.character != 0)
                .map(|(who, aim)| (who.character, aim.clone()))
                .collect(),
        };
        let req = HubRequest::ZoneReplay {
            summary,
            len: w.bytes.len() as u32,
        };
        for attempt in 0..tries {
            match self.client.upload(&req, &w.bytes).await {
                Ok(HubResponse::ReplayStored { id }) => {
                    info!(replay = id, file = %w.path.display(), "replay stored at the hub");
                    // The hub has it: the zone's copy has done its work.
                    let _ = std::fs::remove_file(&w.path);
                    return;
                }
                Ok(other) => {
                    warn!("unexpected hub answer to a replay: {other:?}");
                    return;
                }
                Err(HubClientError::Refused(e)) => {
                    // Set aside under another name: the next zone started on this
                    // directory does not offer it again.
                    warn!(file = %w.path.display(), "the hub refused a replay: {e}");
                    let _ = std::fs::rename(&w.path, w.path.with_extension("refused"));
                    return;
                }
                Err(e) if attempt + 1 == tries => {
                    warn!(file = %w.path.display(), "replay not uploaded, it stays on disk: {e}")
                }
                Err(_) => tokio::time::sleep(Duration::from_secs(60)).await,
            }
        }
    }

    /// Open a player's report at the hub (ANTICHEAT.md 5): its id, within the reporter's
    /// limits.
    pub async fn report(
        &self,
        reporter: CharacterId,
        target: CharacterId,
        reason: gm_net::control::ReportReason,
    ) -> Result<i64, String> {
        match self
            .client
            .request(&HubRequest::ZoneReport {
                reporter,
                target,
                reason,
            })
            .await
        {
            Ok(HubResponse::ReportOpened { id }) => Ok(id),
            Ok(other) => Err(format!("unexpected hub answer {other:?}")),
            Err(HubClientError::Refused(gm_hub_proto::protocol::HubError::Taken)) => {
                Err("you have already reported this player".into())
            }
            Err(HubClientError::Refused(gm_hub_proto::protocol::HubError::Busy)) => {
                Err("you have too many open reports".into())
            }
            Err(HubClientError::Refused(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
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
                    HubNotice::HireEnded { hirer, hire } => {
                        ClientEvent::HubHireEnded { hirer, hire }
                    }
                };
                if tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
    }
}
