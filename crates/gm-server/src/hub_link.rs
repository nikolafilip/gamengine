//! The zone's connection to the hub (HUB.md): registration, heartbeats, claims, saves,
//! handoffs and notices. Everything here is best effort from the tick loop's point of view:
//! requests run on their own tasks and come back as `ClientEvent`s.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gm_hub_proto::protocol::{
    CharacterId, CharacterState, EconReply, GearReading, HiredAvatar, HubError, HubNotice,
    HubRequest, HubResponse, ModelRef, PartyReading, PartyReply, SessionToken, StallSummary,
    TokenPayload, ZoneEconOp, ZoneId, ZonePartyOp, ZoneTicket, now_secs,
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
    /// How many clients the zone takes: the hub sends nobody to a full one.
    pub max_players: u32,
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
    /// What its worn items do to damage (ITEMS.md 3.3).
    pub gear: GearReading,
    /// The party it is in (PARTY.md 3).
    pub party: PartyReading,
    /// The character is a game master here (GM.md 1).
    pub gm: bool,
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
                max_players: cfg.max_players,
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
                gear,
                party,
                gm,
            }) => Ok(Claimed {
                character,
                name,
                state,
                team,
                model,
                squad,
                gear,
                party,
                gm,
            }),
            Ok(other) => Err(format!("unexpected hub answer {other:?}")),
            Err(HubClientError::Refused(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Save a character. `Err`: the hub says the character is not this zone's to save
    /// (it was taken out of here, or never arrived as far as the hub knows). `Ok`: the
    /// number of the hub's present reading of its party (PARTY.md 3.2), 0 when the hub
    /// did not say (a last save, a hub that could not be asked).
    pub async fn save(
        &self,
        character: CharacterId,
        state: CharacterState,
        leaving: bool,
    ) -> Result<u64, ()> {
        let req = HubRequest::Save {
            character,
            state,
            leaving,
        };
        match self.client.request(&req).await {
            Ok(HubResponse::Saved { party }) => Ok(party),
            Ok(_) => Ok(0),
            Err(e) => {
                warn!(character, leaving, "save refused: {e}");
                match e {
                    HubClientError::Refused(HubError::NotFound) => Err(()),
                    _ => Ok(0),
                }
            }
        }
    }

    /// A save whose failure is told in words, for the player to read: the one that follows
    /// an accepted respec (MATRIX.md 9.1). `Ok`: as `save`'s.
    pub async fn save_told(
        &self,
        character: CharacterId,
        state: CharacterState,
    ) -> Result<u64, String> {
        let req = HubRequest::Save {
            character,
            state,
            leaving: false,
        };
        match self.client.request(&req).await {
            Ok(HubResponse::Saved { party }) => Ok(party),
            Ok(_) => Ok(0),
            Err(HubClientError::Refused(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// The zone has no body for a character it claimed: the hub takes it back, offline
    /// as it came (HUB.md 3.8).
    pub async fn release(&self, character: CharacterId) {
        if let Err(e) = self.client.ok(&HubRequest::Release { character }).await {
            warn!(character, "release refused: {e}");
        }
    }

    pub async fn handoff(
        &self,
        character: CharacterId,
        state: CharacterState,
        to_zone: ZoneId,
        web: bool,
    ) -> Result<ZoneTicket, String> {
        match self
            .client
            .request(&HubRequest::Handoff {
                character,
                state,
                to_zone,
                web,
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

    /// Buy a listing for a character the zone saw standing at that stall, at the price it
    /// was shown (ITEMS.md 5). The words of a refusal are for the buyer.
    pub async fn stall_buy(
        &self,
        character: CharacterId,
        stall: i64,
        listing: i64,
        price: i64,
    ) -> Result<GearReading, String> {
        let op = ZoneEconOp::StallBuy {
            character,
            stall,
            listing,
            price,
        };
        match self.client.request(&HubRequest::ZoneEcon(op)).await {
            Ok(HubResponse::Econ(EconReply::Gear(reading))) => Ok(reading),
            Err(HubClientError::Refused(e)) => Err(match e {
                HubError::NotFound => "it is no longer for sale here".to_string(),
                HubError::Insufficient => "not enough coin".to_string(),
                HubError::Full => "the inventory is full".to_string(),
                HubError::Invalid(why) => why,
                other => Self::words(&other, "buy"),
            }),
            other => Err(Self::trouble("buy", &format!("{other:?}"))),
        }
    }

    /// What a player is told when the hub refused for a reason that is not about the
    /// thing asked for; the reason itself goes to the zone's log, not to the player.
    fn words(e: &HubError, what: &str) -> String {
        match e {
            // The hub does not have the character in this zone: it is on its way
            // somewhere, or its ghost has only just come back.
            HubError::Unauthorized => "you are between zones: try again in a moment".to_string(),
            HubError::Busy => "the hub is busy: try again".to_string(),
            other => Self::trouble(what, &other.to_string()),
        }
    }

    fn trouble(what: &str, detail: &str) -> String {
        warn!("the hub on a {what}: {detail}");
        "the hub could not be asked: try again".to_string()
    }

    /// Put an item on (`on`) or take it off for a character playing here (ITEMS.md 2):
    /// what its worn items do from now on. The words of a refusal are for the player.
    pub async fn wear(
        &self,
        character: CharacterId,
        item: i64,
        on: bool,
    ) -> Result<GearReading, String> {
        let op = if on {
            ZoneEconOp::Wear { character, item }
        } else {
            ZoneEconOp::TakeOff { character, item }
        };
        match self.client.request(&HubRequest::ZoneEcon(op)).await {
            Ok(HubResponse::Econ(EconReply::Gear(reading))) => Ok(reading),
            Err(HubClientError::Refused(e)) => Err(match e {
                HubError::NotFound => "there is no such item".to_string(),
                HubError::Invalid(why) => why,
                other => Self::words(&other, "wear"),
            }),
            other => Err(Self::trouble("wear", &format!("{other:?}"))),
        }
    }

    /// A character playing here spent `quantity` of a stack (MODES.md 11.2); the hub's
    /// books follow, and its reading after is what the body carries.
    pub async fn consume(
        &self,
        character: CharacterId,
        item: i64,
        quantity: u32,
    ) -> Result<GearReading, String> {
        let op = ZoneEconOp::Consume {
            character,
            item,
            quantity,
        };
        match self.client.request(&HubRequest::ZoneEcon(op)).await {
            Ok(HubResponse::Econ(EconReply::Gear(reading))) => Ok(reading),
            Err(HubClientError::Refused(e)) => Err(Self::words(&e, "consume")),
            other => Err(Self::trouble("consume", &format!("{other:?}"))),
        }
    }

    /// Something about the party of a character playing here (PARTY.md 3.2). The words
    /// of a refusal are for the player.
    pub async fn party(&self, op: ZonePartyOp) -> Result<PartyReply, String> {
        match self.client.request(&HubRequest::ZoneParty(op)).await {
            Ok(HubResponse::Party(reply)) => Ok(reply),
            Err(HubClientError::Refused(e)) => Err(match e {
                HubError::Invalid(why) => why,
                other => Self::words(&other, "party"),
            }),
            other => Err(Self::trouble("party", &format!("{other:?}"))),
        }
    }

    /// Open a trade between two characters the zone saw stand together and both ask
    /// (PARTY.md 6): the hub's id of it.
    pub async fn trade_open(&self, a: CharacterId, b: CharacterId) -> Result<i64, String> {
        let op = ZoneEconOp::TradeOpen { a, b };
        match self.client.request(&HubRequest::ZoneEcon(op)).await {
            Ok(HubResponse::Econ(EconReply::Id(trade))) => Ok(trade),
            Err(HubClientError::Refused(e)) => Err(match e {
                HubError::Invalid(why) => why,
                other => Self::words(&other, "trade"),
            }),
            other => Err(Self::trouble("trade", &format!("{other:?}"))),
        }
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
                    // What a character carries changed without this zone's hand (MODES.md
                    // 11.2): taken as any reading of its gear is, nobody waiting for it.
                    HubNotice::Gear { character, reading } => ClientEvent::Worn {
                        id: 0,
                        character,
                        item: 0,
                        result: Ok(reading),
                        tell: false,
                    },
                    HubNotice::HireEnded { hirer, hire } => {
                        ClientEvent::HubHireEnded { hirer, hire }
                    }
                    HubNotice::Party(news) => ClientEvent::HubParty(news),
                    HubNotice::Invited { to, from } => ClientEvent::HubInvited { to, from },
                    HubNotice::Declined { to, by } => ClientEvent::HubDeclined { to, by },
                    HubNotice::Heard {
                        to,
                        channel,
                        from,
                        text,
                    } => ClientEvent::HubHeard {
                        to,
                        channel,
                        from,
                        text,
                    },
                };
                if tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
    }
}
