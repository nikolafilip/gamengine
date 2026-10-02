//! The hub process (HUB.md): accounts, sessions, the zone registry, tickets, claims, saves and
//! handoffs. One QUIC endpoint for clients and zones; one task per connection, one task per
//! request stream; the database does the invariants.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use gm_core::build::{Build, ContentPack};
use gm_core::tick::TickRate;
use gm_net::control::{self, WebAddr, valid_name};
use gm_net::link::{Link, RecvHalf, SendHalf, WebEndpoint, web_accept};
use gm_net::transport::fnv1a64;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::db::Db;
use gm_hub_proto::protocol::{
    AccountId, BuildChoice, CharacterId, ContractOutcome, EconOp, EconReply, HASH_PERMITS,
    HiredAvatar, HubError, HubNotice, HubRequest, HubResponse, ItemSummary, LocationSummary, ModOp,
    ModelRef, SessionId, StallSummary, TavernEntry, TradeOffer, ZoneEconOp, ZoneId, ZoneSummary,
    ZoneTicket, now_secs,
};

use crate::economy::{EconError, Economy, Outcome, TradeStatus};
use crate::models::{IngestMode, Models};

/// The body of an upload must arrive within ten seconds plus its length at 64 KiB/s
/// (MODELS.md 6.2): 15 s for 300 KB, 138 s for the 8 MiB limit. A slow sender holds one of
/// the upload slots for that long at most, never a worker.
fn upload_body_timeout(len: u32) -> Duration {
    Duration::from_secs(10) + Duration::from_secs_f64(len as f64 / 65_536.0)
}

/// A zone whose last heartbeat is older than this gets no new players (HUB.md 2).
const ZONE_STALE: Duration = Duration::from_secs(15);
use gm_hub_proto::token::{HubKey, TokenVerifier};

pub struct HubConfig {
    pub zone_secret: String,
    pub content: ContentPack,
    pub key: HubKey,
    /// Idle sessions expire after this.
    pub session_secs: u64,
    /// `Register`/`Login` attempts per minute per source address.
    pub auth_per_minute: f64,
    /// Item templates a craft may name (`assets/content/items.toml`); empty = any.
    pub templates: Vec<String>,
    /// The largest coin drop a zone may report in one grant, in copper (ECONOMY.md 9).
    pub max_coin_grant: i64,
    /// Where ingested models, their previews and the uploads live (MODELS.md 6.1).
    pub models_dir: std::path::PathBuf,
    /// How uploads are parsed: a worker process in production.
    pub ingest: IngestMode,
    /// Wall clock of one ingestion (`models::INGEST_TIMEOUT` outside tests).
    pub ingest_timeout: Duration,
}

struct Session {
    account: AccountId,
    created: Instant,
}

struct ZoneEntry {
    addr: SocketAddr,
    cert_der: Vec<u8>,
    web: Option<WebAddr>,
    map: String,
    #[allow(dead_code)]
    map_hash: u64,
    players: u32,
    #[allow(dead_code)]
    tick_mean_us: f32,
    since: Instant,
    last_heartbeat: Instant,
    conn: Link,
    /// Trials that open the zone; empty = open to all (COMPANIONS.md 11).
    requires: Vec<String>,
}

/// The most trials a zone may name as its key.
const MAX_ZONE_REQUIRES: usize = 16;

struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
struct State {
    sessions: HashMap<SessionId, Session>,
    zones: HashMap<ZoneId, ZoneEntry>,
    buckets: HashMap<IpAddr, Bucket>,
    last_sweep: Option<Instant>,
}

struct Hub {
    cfg: HubConfig,
    db: Db,
    econ: Economy,
    models: Models,
    state: Mutex<State>,
    hashing: Semaphore,
    /// Verifies our own tokens on `Claim` (defence in depth; the zone verified too).
    verifier: Mutex<HashMap<ZoneId, TokenVerifier>>,
}

/// What a connection has proven about itself.
#[derive(Default)]
struct ConnAuth {
    zone: Mutex<Option<ZoneId>>,
}

pub async fn run(
    cfg: HubConfig,
    db: Db,
    endpoint: quinn::Endpoint,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    run_with_web(cfg, db, endpoint, None, shutdown).await
}

/// `run`, with a WebTransport listener for browsers beside the QUIC endpoint (WEB.md 2.1):
/// the endpoint and the `Origin`s a session may come from (empty = any).
pub async fn run_with_web(
    cfg: HubConfig,
    db: Db,
    endpoint: quinn::Endpoint,
    web: Option<(WebEndpoint, Vec<String>)>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let models = Models::new(
        db.pool().clone(),
        &cfg.models_dir,
        cfg.ingest.clone(),
        cfg.ingest_timeout,
    )?;
    let hub = Arc::new(Hub {
        hashing: Semaphore::new(HASH_PERMITS),
        verifier: Mutex::new(HashMap::new()),
        models,
        cfg,
        econ: Economy::new(db.pool().clone()),
        db,
        state: Mutex::new(State::default()),
    });
    info!(listen = %endpoint.local_addr()?, "hub listening");
    // Stalls past their 48 h close and stalled contracts refund, once a minute.
    let sweeper = {
        let hub = hub.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            loop {
                every.tick().await;
                match hub.econ.stalls_expired().await {
                    Ok(owners) => {
                        for owner in owners {
                            match hub.econ.stall_close(owner).await {
                                Ok((stall, zone)) => {
                                    hub.notify(&zone, HubNotice::StallClosed { stall }).await;
                                }
                                Err(e) => warn!(owner, "closing an expired stall: {e}"),
                            }
                        }
                    }
                    Err(e) => warn!("stall sweep: {e}"),
                }
                if let Err(e) = hub.econ.contracts_expire().await {
                    warn!("contract sweep: {e}");
                }
            }
        })
    };
    let accept = {
        let hub = hub.clone();
        let endpoint = endpoint.clone();
        async move {
            while let Some(incoming) = endpoint.accept().await {
                let hub = hub.clone();
                tokio::spawn(async move {
                    let remote = incoming.remote_address();
                    match incoming.await {
                        Ok(conn) => handle_connection(hub, Link::Quic(conn)).await,
                        Err(e) => debug!(%remote, "handshake failed: {e}"),
                    }
                });
            }
        }
    };
    // Browsers: the same requests on the streams of a WebTransport session.
    let accept_web = {
        let hub = hub.clone();
        async move {
            let Some((endpoint, origins)) = web else {
                return std::future::pending::<()>().await;
            };
            loop {
                let incoming = web_accept(&endpoint, &origins).await;
                let hub = hub.clone();
                tokio::spawn(async move {
                    let remote = incoming.remote_address();
                    match tokio::time::timeout(Duration::from_secs(5), incoming.accept()).await {
                        Ok(Ok(conn)) => handle_connection(hub, conn).await,
                        Ok(Err(e)) => debug!(%remote, "web session refused: {e}"),
                        Err(_) => debug!(%remote, "web session: no request in time"),
                    }
                });
            }
        }
    };
    tokio::select! {
        _ = accept => {}
        _ = accept_web => {}
        _ = shutdown => {}
    }
    sweeper.abort();
    endpoint.close(0u32.into(), b"hub stopped");
    info!("hub stopped");
    Ok(())
}

async fn handle_connection(hub: Arc<Hub>, conn: Link) {
    let remote = conn.remote_address();
    let auth = Arc::new(ConnAuth::default());
    debug!(%remote, "connection");
    loop {
        let (send, recv) = match conn.accept_bi().await {
            Ok(s) => s,
            Err(e) => {
                debug!(%remote, "connection ended: {e}");
                break;
            }
        };
        let hub = hub.clone();
        let auth = auth.clone();
        let conn = conn.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_stream(hub, auth, conn, remote, send, recv).await {
                debug!(%remote, "request failed: {e}");
            }
        });
    }
    // A zone that disconnects takes its players with it: they go offline.
    let zone = auth.zone.lock().unwrap().clone();
    if let Some(zone) = zone {
        let removed = {
            let mut st = hub.state.lock().unwrap();
            match st.zones.get(&zone) {
                Some(entry) if entry.conn.stable_id() == conn.stable_id() => {
                    st.zones.remove(&zone);
                    true
                }
                _ => false,
            }
        };
        if removed {
            match hub.db.offline_zone(&zone).await {
                Ok(n) => info!(%zone, players = n, "zone disconnected; its players are offline"),
                Err(e) => warn!(%zone, "offline_zone failed: {e}"),
            }
            hub.db.log(&zone, "disconnected", "").await;
        }
    }
}

async fn handle_stream(
    hub: Arc<Hub>,
    auth: Arc<ConnAuth>,
    conn: Link,
    remote: SocketAddr,
    mut send: SendHalf,
    mut recv: RecvHalf,
) -> anyhow::Result<()> {
    let req: HubRequest = match control::recv_any(&mut recv).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    // Requests that carry or are answered with raw bytes on the stream (MODELS.md 6.2).
    let resp = match req {
        HubRequest::ModelUpload {
            session,
            frame,
            tos_version,
            len,
        } => {
            let r = upload(&hub, session, frame, tos_version, len, &mut recv).await;
            if r.is_err() {
                // Refused, perhaps before the body was read: tell the sender to stop.
                recv.stop(0);
            }
            r.unwrap_or_else(HubResponse::Err)
        }
        HubRequest::ModelGet { session, model } => {
            let blob = async {
                let account = hub.session_account(session)?;
                hub.models.get(session, account, &model).await
            }
            .await;
            return send_blob(&mut send, blob).await;
        }
        HubRequest::Mod {
            session,
            op: ModOp::Preview { model },
        } => {
            let blob = async {
                hub.moderator(session).await?;
                hub.models.preview(&model)
            }
            .await;
            return send_blob(&mut send, blob).await;
        }
        req => match handle(&hub, &auth, &conn, remote, req).await {
            Ok(r) => r,
            Err(e) => HubResponse::Err(e),
        },
    };
    control::send_any(&mut send, &resp).await?;
    let _ = send.finish();
    Ok(())
}

/// Answer with `Blob { len }` and the bytes, or with the error.
async fn send_blob(
    send: &mut quinn::SendStream,
    blob: Result<Vec<u8>, HubError>,
) -> anyhow::Result<()> {
    match blob {
        Ok(bytes) => {
            control::send_any(
                send,
                &HubResponse::Blob {
                    len: bytes.len() as u32,
                },
            )
            .await?;
            send.write_all(&bytes).await?;
        }
        Err(e) => control::send_any(send, &HubResponse::Err(e)).await?,
    }
    let _ = send.finish();
    Ok(())
}

/// `ModelUpload`: everything that can be refused is refused before the body is read. One
/// upload per account at a time and at most `UPLOAD_SLOTS` bodies in memory; a worker is taken
/// only once the body is complete, so a slow sender never keeps one idle.
async fn upload(
    hub: &Hub,
    session: SessionId,
    frame: u8,
    tos_version: u16,
    len: u32,
    recv: &mut quinn::RecvStream,
) -> Result<HubResponse, HubError> {
    let account = hub.session_account(session)?;
    let _turn = hub.models.begin_upload(account)?;
    hub.models
        .precheck(account, frame, tos_version, len)
        .await?;
    let mut body = vec![0u8; len as usize];
    match tokio::time::timeout(upload_body_timeout(len), recv.read_exact(&mut body)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(HubError::Invalid(format!("the upload was cut short: {e}"))),
        Err(_) => return Err(HubError::Invalid("the upload took too long".into())),
    }
    let (model, status) = hub.models.upload(account, frame, body).await?;
    Ok(HubResponse::ModelAccepted { model, status })
}

impl Hub {
    fn session_account(&self, session: SessionId) -> Result<AccountId, HubError> {
        let mut st = self.state.lock().unwrap();
        let ttl = Duration::from_secs(self.cfg.session_secs);
        st.sessions.retain(|_, s| s.created.elapsed() < ttl);
        st.sessions
            .get(&session)
            .map(|s| s.account)
            .ok_or(HubError::Unauthorized)
    }

    fn new_session(&self, account: AccountId) -> SessionId {
        let mut id = [0u8; 16];
        rand::fill(&mut id);
        let id = SessionId(id);
        self.state.lock().unwrap().sessions.insert(
            id,
            Session {
                account,
                created: Instant::now(),
            },
        );
        id
    }

    /// Token bucket per source address for `Register` and `Login`.
    fn auth_allowed(&self, ip: IpAddr) -> bool {
        let mut st = self.state.lock().unwrap();
        let per_min = self.cfg.auth_per_minute.max(1.0);
        let now = Instant::now();
        if st
            .last_sweep
            .is_none_or(|t| now.duration_since(t) > Duration::from_secs(60))
        {
            st.buckets
                .retain(|_, b| now.duration_since(b.last) < Duration::from_secs(600));
            st.last_sweep = Some(now);
        }
        let b = st.buckets.entry(ip).or_insert(Bucket {
            tokens: per_min,
            last: now,
        });
        b.tokens =
            (b.tokens + now.duration_since(b.last).as_secs_f64() * per_min / 60.0).min(per_min);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// A preset name or a full build, validated against the hub's content (MATRIX.md 9).
    fn resolve_build(&self, choice: BuildChoice) -> Result<Build, HubError> {
        let build = match choice {
            BuildChoice::Preset(name) => self
                .cfg
                .content
                .build(&name)
                .cloned()
                .ok_or_else(|| HubError::Invalid(format!("unknown preset {name:?}")))?,
            BuildChoice::Custom(b) => b,
        };
        build
            .validate(&self.cfg.content)
            .map_err(|e| HubError::Invalid(e.to_string()))?;
        Ok(build)
    }

    fn zone_live(&self, zone: &ZoneId) -> bool {
        self.state
            .lock()
            .unwrap()
            .zones
            .get(zone)
            .is_some_and(|z| z.last_heartbeat.elapsed() < ZONE_STALE)
    }

    /// The gate of a zone (COMPANIONS.md 11): a character that passed none of the trials the
    /// zone names is not let in, by `Enter` or by a handoff.
    async fn gate(&self, zone: &ZoneId, character: CharacterId) -> Result<(), HubError> {
        let requires = self
            .state
            .lock()
            .unwrap()
            .zones
            .get(zone)
            .map(|z| z.requires.clone())
            .unwrap_or_default();
        if requires.is_empty() {
            return Ok(());
        }
        let passed = self.db.trials_of(character).await?;
        if passed.iter().any(|(t, _)| requires.contains(t)) {
            Ok(())
        } else {
            Err(HubError::Locked(requires.join(", ")))
        }
    }

    /// The active hires of `hirer` as its zone and its client are told (COMPANIONS.md 3.3):
    /// oldest first, at most `capacity`. A stored build that no longer validates against
    /// the content is left out rather than sent into a zone.
    async fn hired(
        &self,
        hirer: CharacterId,
        capacity: usize,
    ) -> Result<Vec<HiredAvatar>, HubError> {
        let mut out = Vec::new();
        for h in self.econ.squad(hirer).await.map_err(econ_err)? {
            if out.len() >= capacity {
                break;
            }
            let Ok(build) = serde_json::from_value::<Build>(h.build) else {
                continue;
            };
            if build.validate(&self.cfg.content).is_err() {
                continue;
            }
            out.push(HiredAvatar {
                hire: h.id,
                character: h.avatar,
                name: h.name,
                model: self
                    .models
                    .worn(h.avatar)
                    .await?
                    .filter(|m| m.frame == gm_model::rig::frame_index(build.frame)),
                build,
                expires_at: h.expires_unix.max(0) as u64,
            });
        }
        Ok(out)
    }

    fn zone_of_conn(&self, auth: &ConnAuth) -> Result<ZoneId, HubError> {
        auth.zone
            .lock()
            .unwrap()
            .clone()
            .ok_or(HubError::Unauthorized)
    }

    fn ticket_for(
        &self,
        zone: &ZoneId,
        account: AccountId,
        character: CharacterId,
    ) -> Result<ZoneTicket, HubError> {
        let st = self.state.lock().unwrap();
        let entry = st.zones.get(zone).ok_or(HubError::NotFound)?;
        Ok(ZoneTicket {
            zone: zone.clone(),
            addr: entry.addr,
            cert_der: entry.cert_der.clone(),
            web: entry.web.clone(),
            token: self.cfg.key.issue(account, character, zone),
        })
    }

    /// The account of a session that is a moderator's.
    async fn moderator(&self, session: SessionId) -> Result<AccountId, HubError> {
        let account = self.session_account(session)?;
        if self.models.is_moderator(account).await? {
            Ok(account)
        } else {
            Err(HubError::Unauthorized)
        }
    }

    /// Send a notice to every connected zone.
    async fn notify_all(&self, notice: HubNotice) {
        let zones: Vec<ZoneId> = self.state.lock().unwrap().zones.keys().cloned().collect();
        for zone in zones {
            self.notify(&zone, notice.clone()).await;
        }
    }

    /// Send a notice to a zone, if it is connected.
    async fn notify(&self, zone: &ZoneId, notice: HubNotice) {
        let conn = self
            .state
            .lock()
            .unwrap()
            .zones
            .get(zone)
            .map(|z| z.conn.clone());
        if let Some(conn) = conn
            && let Ok(mut uni) = conn.open_uni().await
        {
            let _ = control::send_any(&mut uni, &notice).await;
            let _ = uni.finish();
        }
    }
}

fn normalize_email(email: &str) -> Result<String, HubError> {
    let e = email.trim().to_lowercase();
    if e.len() < 3
        || e.len() > 254
        || !e.contains('@')
        || e.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(HubError::Invalid("email".into()));
    }
    Ok(e)
}

async fn handle(
    hub: &Arc<Hub>,
    auth: &ConnAuth,
    conn: &Link,
    remote: SocketAddr,
    req: HubRequest,
) -> Result<HubResponse, HubError> {
    match req {
        HubRequest::Register { email, password } => {
            if !hub.auth_allowed(remote.ip()) {
                return Err(HubError::Busy);
            }
            let email = normalize_email(&email)?;
            if password.len() < 8 || password.len() > 256 {
                return Err(HubError::Invalid("password must be 8..=256 bytes".into()));
            }
            let _permit = hub.hashing.try_acquire().map_err(|_| HubError::Busy)?;
            let hash = tokio::task::spawn_blocking(move || {
                let mut salt = [0u8; 16];
                rand::fill(&mut salt);
                Argon2::default()
                    .hash_password_with_salt(password.as_bytes(), &salt)
                    .map(|h: PasswordHash| h.to_string())
            })
            .await
            .map_err(|_| HubError::Internal)?
            .map_err(|_| HubError::Internal)?;
            let account = hub.db.create_account(&email, &hash).await?;
            let session = hub.new_session(account);
            info!(%email, account, "registered");
            Ok(HubResponse::Session { session, account })
        }
        HubRequest::Login { email, password } => {
            if !hub.auth_allowed(remote.ip()) {
                return Err(HubError::Busy);
            }
            let email = normalize_email(&email)?;
            let Some((account, hash)) = hub.db.account_by_email(&email).await? else {
                return Err(HubError::Credentials);
            };
            let _permit = hub.hashing.try_acquire().map_err(|_| HubError::Busy)?;
            let ok = tokio::task::spawn_blocking(move || {
                PasswordHash::new(&hash)
                    .map(|parsed| {
                        Argon2::default()
                            .verify_password(password.as_bytes(), &parsed)
                            .is_ok()
                    })
                    .unwrap_or(false)
            })
            .await
            .map_err(|_| HubError::Internal)?;
            if !ok {
                return Err(HubError::Credentials);
            }
            let session = hub.new_session(account);
            Ok(HubResponse::Session { session, account })
        }
        HubRequest::Characters { session } => {
            let account = hub.session_account(session)?;
            let rows = hub.db.characters_of(account).await?;
            Ok(HubResponse::Characters(
                rows.iter().map(|r| r.summary()).collect(),
            ))
        }
        HubRequest::CreateCharacter {
            session,
            name,
            build,
        } => {
            let account = hub.session_account(session)?;
            let name = valid_name(&name).ok_or_else(|| HubError::Invalid("name".into()))?;
            let build = hub.resolve_build(build)?;
            let row = hub.db.create_character(account, &name, &build).await?;
            Ok(HubResponse::Character(row.summary()))
        }
        HubRequest::SetBuild {
            session,
            character,
            build,
        } => {
            let account = hub.session_account(session)?;
            let build = hub.resolve_build(build)?;
            hub.db.set_build(account, character, &build).await?;
            Ok(HubResponse::Ok)
        }
        HubRequest::ListZones { session } => {
            hub.session_account(session)?;
            let st = hub.state.lock().unwrap();
            let mut zones: Vec<ZoneSummary> = st
                .zones
                .iter()
                .filter(|(_, z)| z.last_heartbeat.elapsed() < ZONE_STALE)
                .map(|(id, z)| ZoneSummary {
                    id: id.clone(),
                    map: z.map.clone(),
                    players: z.players,
                    addr: z.addr,
                    cert_hash: fnv1a64(&z.cert_der),
                    up_secs: z.since.elapsed().as_secs(),
                })
                .collect();
            zones.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(HubResponse::Zones(zones))
        }
        HubRequest::Enter {
            session,
            character,
            zone,
        } => {
            let account = hub.session_account(session)?;
            if !hub.zone_live(&zone) {
                return Err(HubError::NotFound);
            }
            if hub
                .db
                .character(character)
                .await?
                .is_none_or(|r| r.account_id != account)
            {
                return Err(HubError::NotFound);
            }
            hub.gate(&zone, character).await?;
            hub.db.begin_enter(account, character, &zone).await?;
            Ok(HubResponse::Ticket(
                hub.ticket_for(&zone, account, character)?,
            ))
        }
        HubRequest::Logout { session } => {
            let account = hub.session_account(session)?;
            hub.state.lock().unwrap().sessions.remove(&session);
            // Characters still in a zone are kicked there and go offline here.
            let rows = hub.db.characters_of(account).await?;
            let in_zone: Vec<(CharacterId, ZoneId)> = rows
                .iter()
                .filter(|r| r.location_kind == "zone")
                .filter_map(|r| r.location_zone.clone().map(|z| (r.id, z)))
                .collect();
            // Characters in a live zone are saved and taken offline by that zone when it
            // handles the kick; everything else (transits, orphans) goes offline here.
            hub.db.offline_not_in_zone(account).await?;
            for (character, zone) in in_zone {
                hub.notify(
                    &zone,
                    HubNotice::Kick {
                        character,
                        reason: "logged out".into(),
                    },
                )
                .await;
            }
            Ok(HubResponse::Ok)
        }
        HubRequest::Trials { session, character } => {
            let account = hub.session_account(session)?;
            hub.db
                .character(character)
                .await?
                .filter(|r| r.account_id == account)
                .ok_or(HubError::NotFound)?;
            Ok(HubResponse::Trials(hub.db.trials_of(character).await?))
        }
        HubRequest::Trial {
            character,
            trial,
            secs,
        } => {
            // A zone speaks only for characters playing in it, and only about the trials
            // of its own map.
            let zone = hub.zone_of_conn(auth)?;
            if hub.db.zone_of(character).await?.as_ref() != Some(&zone) {
                return Err(HubError::Unauthorized);
            }
            let map = hub
                .state
                .lock()
                .unwrap()
                .zones
                .get(&zone)
                .map(|z| z.map.clone())
                .ok_or(HubError::Unauthorized)?;
            if !hub
                .cfg
                .content
                .trials
                .iter()
                .any(|t| t.key == trial && t.map == map)
            {
                return Err(HubError::Invalid(format!(
                    "{trial:?} is not a trial of the map {map:?}"
                )));
            }
            hub.db.trial_pass(character, &trial, &zone, secs).await?;
            hub.db
                .log(&zone, "trial", &format!("{character} {trial} {secs}s"))
                .await;
            Ok(HubResponse::Ok)
        }
        HubRequest::ZoneHello {
            secret,
            zone,
            map,
            map_hash,
            addr,
            cert_der,
            web,
            requires,
        } => {
            if secret != hub.cfg.zone_secret || hub.cfg.zone_secret.is_empty() {
                warn!(%remote, %zone, "zone hello with a wrong secret");
                return Err(HubError::Unauthorized);
            }
            if valid_name(&zone).is_none() {
                return Err(HubError::Invalid("zone id".into()));
            }
            if requires.len() > MAX_ZONE_REQUIRES
                || requires
                    .iter()
                    .any(|r| !hub.cfg.content.trials.iter().any(|t| &t.key == r))
            {
                return Err(HubError::Invalid(
                    "the zone requires an unknown trial".into(),
                ));
            }
            // A restarted zone has lost its players: they go offline and re-enter.
            let orphaned = hub.db.offline_zone(&zone).await?;
            let previous = {
                let mut st = hub.state.lock().unwrap();
                let previous = st.zones.remove(&zone).map(|z| z.conn);
                st.zones.insert(
                    zone.clone(),
                    ZoneEntry {
                        addr,
                        cert_der,
                        web,
                        map: map.clone(),
                        map_hash,
                        players: 0,
                        tick_mean_us: 0.0,
                        since: Instant::now(),
                        last_heartbeat: Instant::now(),
                        conn: conn.clone(),
                        requires,
                    },
                );
                previous
            };
            if let Some(old) = previous
                && old.stable_id() != conn.stable_id()
            {
                old.close(1, b"replaced by a new zone process");
            }
            hub.verifier.lock().unwrap().insert(
                zone.clone(),
                TokenVerifier::new(hub.cfg.key.public_key(), &zone)
                    .map_err(|_| HubError::Internal)?,
            );
            *auth.zone.lock().unwrap() = Some(zone.clone());
            hub.db
                .log(
                    &zone,
                    "hello",
                    &format!("{map} {map_hash:016x} {addr} orphaned={orphaned}"),
                )
                .await;
            info!(%zone, %map, %addr, orphaned, "zone registered");
            Ok(HubResponse::Registered {
                public_key: hub.cfg.key.public_key(),
            })
        }
        HubRequest::Heartbeat {
            players,
            tick_mean_us,
        } => {
            let zone = hub.zone_of_conn(auth)?;
            let mut st = hub.state.lock().unwrap();
            if let Some(z) = st.zones.get_mut(&zone) {
                z.players = players;
                z.tick_mean_us = tick_mean_us;
                z.last_heartbeat = Instant::now();
            }
            Ok(HubResponse::Ok)
        }
        HubRequest::Claim { token } => {
            let zone = hub.zone_of_conn(auth)?;
            let payload = {
                let mut v = hub.verifier.lock().unwrap();
                let verifier = v.get_mut(&zone).ok_or(HubError::Unauthorized)?;
                verifier
                    .accept(&token, now_secs())
                    .map_err(|e| HubError::Invalid(e.to_string()))?
            };
            let row = hub.db.claim(payload.character, &zone).await?;
            // A handoff: the origin zone drops its ghost.
            if let Some(from) = &row.location_zone
                && from != &zone
            {
                hub.notify(from, HubNotice::Claimed { character: row.id })
                    .await;
            }
            let mut state = row.state();
            if state.zone.as_deref() != Some(zone.as_str()) {
                // The saved position is on another zone's map, or there is none: spawn here.
                state.zone = None;
            }
            // The owner is playing this character now: every hire of it as an avatar ends
            // (ECONOMY.md 11), and the zones its hirers play in are told.
            for (hire, hirer) in hub.econ.end_hires_of(row.id).await.map_err(econ_err)? {
                if let Some(z) = hub.db.zone_of(hirer).await? {
                    hub.notify(&z, HubNotice::HireEnded { hirer, hire }).await;
                }
            }
            let squad = hub
                .hired(row.id, row.build.squad_capacity(&hub.cfg.content))
                .await?;
            Ok(HubResponse::Claimed {
                character: row.id,
                name: row.name.clone(),
                state,
                team: 0,
                model: hub.models.worn(row.id).await?,
                squad,
            })
        }
        HubRequest::Save {
            character,
            state,
            leaving,
        } => {
            let zone = hub.zone_of_conn(auth)?;
            state
                .build
                .validate(&hub.cfg.content)
                .map_err(|e| HubError::Invalid(e.to_string()))?;
            hub.db.save(character, &zone, &state, leaving).await?;
            Ok(HubResponse::Ok)
        }
        HubRequest::Handoff {
            character,
            state,
            to_zone,
        } => {
            let zone = hub.zone_of_conn(auth)?;
            if to_zone == zone {
                return Err(HubError::Invalid("already there".into()));
            }
            if !hub.zone_live(&to_zone) {
                return Err(HubError::NotFound);
            }
            state
                .build
                .validate(&hub.cfg.content)
                .map_err(|e| HubError::Invalid(e.to_string()))?;
            hub.gate(&to_zone, character).await?;
            hub.db
                .begin_handoff(character, &zone, &to_zone, &state)
                .await?;
            let row = hub
                .db
                .character(character)
                .await?
                .ok_or(HubError::NotFound)?;
            Ok(HubResponse::Ticket(hub.ticket_for(
                &to_zone,
                row.account_id,
                character,
            )?))
        }
        HubRequest::Econ {
            session,
            character,
            op,
        } => {
            let account = hub.session_account(session)?;
            let row = hub
                .db
                .character(character)
                .await?
                .filter(|r| r.account_id == account)
                .ok_or(HubError::NotFound)?;
            let zone = match row.summary().location {
                LocationSummary::Zone(z) => Some(z),
                _ => None,
            };
            Ok(HubResponse::Econ(econ_op(hub, character, zone, op).await?))
        }
        HubRequest::ZoneEcon(op) => {
            let zone = hub.zone_of_conn(auth)?;
            Ok(HubResponse::Econ(zone_econ_op(hub, &zone, op).await?))
        }
        // Answered on the stream itself (`handle_stream`): they carry or return raw bytes.
        HubRequest::ModelUpload { .. } | HubRequest::ModelGet { .. } => Err(HubError::Internal),
        HubRequest::ModelList { session } => {
            let account = hub.session_account(session)?;
            Ok(HubResponse::Models(hub.models.list(account).await?))
        }
        HubRequest::ModelDrop { session, model } => {
            let account = hub.session_account(session)?;
            hub.models.drop_model(account, &model).await?;
            Ok(HubResponse::Ok)
        }
        HubRequest::SetModel {
            session,
            character,
            model,
        } => {
            let account = hub.session_account(session)?;
            let row = hub
                .db
                .character(character)
                .await?
                .filter(|r| r.account_id == account)
                .ok_or(HubError::NotFound)?;
            let frame = gm_model::rig::frame_index(row.build.frame);
            hub.models
                .set_model(account, character, frame, model.as_ref())
                .await?;
            Ok(HubResponse::Ok)
        }
        HubRequest::Mod { session, op } => {
            let moderator = hub.moderator(session).await?;
            match op {
                ModOp::Queue { limit } => Ok(HubResponse::ModQueue(hub.models.queue(limit).await?)),
                ModOp::Preview { .. } => Err(HubError::Internal),
                ModOp::Decide {
                    model,
                    approve,
                    code,
                    reason,
                } => {
                    hub.models
                        .decide(moderator, &model, approve, code, &reason)
                        .await?;
                    Ok(HubResponse::Ok)
                }
                ModOp::Takedown {
                    model,
                    code,
                    reason,
                    reference,
                } => {
                    let revoked = hub
                        .models
                        .takedown(moderator, &model, code, &reason, &reference)
                        .await?;
                    // The database already says so; now the zones and their clients.
                    for model in revoked {
                        hub.notify_all(HubNotice::ModelRevoked { model }).await;
                    }
                    Ok(HubResponse::Ok)
                }
                ModOp::Reinstate { model, reason } => {
                    hub.models.reinstate(moderator, &model, &reason).await?;
                    Ok(HubResponse::Ok)
                }
                ModOp::SetUpload { email, allow } => {
                    hub.models.set_upload(moderator, &email, allow).await?;
                    Ok(HubResponse::Ok)
                }
                ModOp::SetTrust { email, tier } => {
                    hub.models.set_trust(moderator, &email, tier).await?;
                    Ok(HubResponse::Ok)
                }
                ModOp::ClearStrikes { email } => {
                    hub.models.clear_strikes(moderator, &email).await?;
                    Ok(HubResponse::Ok)
                }
            }
        }
    }
}

fn econ_err(e: EconError) -> HubError {
    match e {
        EconError::NotFound => HubError::NotFound,
        EconError::Forbidden => HubError::Unauthorized,
        EconError::Insufficient => HubError::Insufficient,
        EconError::Full => HubError::Full,
        EconError::Cooldown => HubError::Cooldown,
        EconError::State(s) | EconError::Invalid(s) => HubError::Invalid(s),
        EconError::Busy => HubError::Busy,
        EconError::Internal => HubError::Internal,
    }
}

fn item_summary(i: crate::economy::Item) -> ItemSummary {
    ItemSummary {
        id: i.id,
        template: i.template,
        components: i
            .components
            .into_iter()
            .map(|c| (c.layer, c.material))
            .collect(),
    }
}

fn trade_offer((coin, accepted, items): (i64, bool, Vec<crate::economy::Item>)) -> TradeOffer {
    TradeOffer {
        coin,
        accepted,
        items: items.into_iter().map(item_summary).collect(),
    }
}

fn holder_reply((coin, items): (i64, Vec<crate::economy::Item>)) -> EconReply {
    EconReply::Holder {
        coin,
        items: items.into_iter().map(item_summary).collect(),
    }
}

/// One economy request of a character its session owns. `zone` is where the character is
/// playing; the ops that happen in the world (ground, stalls, trades) need one.
async fn econ_op(
    hub: &Hub,
    me: CharacterId,
    zone: Option<ZoneId>,
    op: EconOp,
) -> Result<EconReply, HubError> {
    let e = &hub.econ;
    let here = || {
        zone.clone()
            .ok_or_else(|| HubError::Invalid("the character is not in a zone".into()))
    };
    let done = |r: Result<(), EconError>| r.map(|()| EconReply::Done).map_err(econ_err);
    let id = |r: Result<i64, EconError>| r.map(EconReply::Id).map_err(econ_err);
    match op {
        EconOp::Inventory => e.inventory(me).await.map(holder_reply).map_err(econ_err),
        EconOp::Storage => e.storage(me).await.map(holder_reply).map_err(econ_err),
        EconOp::StorageDeposit { item } => done(e.storage_deposit(me, item).await),
        EconOp::StorageWithdraw { item } => done(e.storage_withdraw(me, item).await),
        EconOp::Craft {
            template,
            components,
        } => {
            if !hub.cfg.templates.is_empty() && !hub.cfg.templates.contains(&template) {
                return Err(HubError::Invalid(format!("unknown template {template:?}")));
            }
            id(e.craft(me, &template, &components).await)
        }
        EconOp::Decompose { item } => e
            .decompose(me, item)
            .await
            .map(EconReply::Ids)
            .map_err(econ_err),
        EconOp::TradeOpen { with } => {
            // A trade window is between two characters standing in the same zone.
            let mine = here()?;
            if hub.db.zone_of(with).await?.as_ref() != Some(&mine) {
                return Err(HubError::Invalid("the other character is not here".into()));
            }
            id(e.trade_open(me, with).await)
        }
        EconOp::TradeOfferItem { trade, item } => done(e.trade_offer_item(trade, me, item).await),
        EconOp::TradeRetractItem { trade, item } => {
            done(e.trade_retract_item(trade, me, item).await)
        }
        EconOp::TradeSetCoin { trade, coin } => done(e.trade_set_coin(trade, me, coin).await),
        EconOp::TradeAccept { trade, version } => e
            .trade_accept(trade, me, version)
            .await
            .map(|s| EconReply::Trade {
                committed: s == TradeStatus::Committed,
            })
            .map_err(econ_err),
        EconOp::TradeCancel { trade } => done(e.trade_cancel(trade, me).await),
        EconOp::TradeView { trade } => e
            .trade_view(trade, me)
            .await
            .map(|(version, mine, theirs)| EconReply::TradeView {
                version,
                mine: trade_offer(mine),
                theirs: trade_offer(theirs),
            })
            .map_err(econ_err),
        EconOp::StallList { item, price } => id(e.stall_list(me, item, price).await),
        EconOp::StallBuy { listing, price } => done(e.stall_buy(me, listing, price).await),
        EconOp::StallClose => {
            // The owner may close from anywhere; the zone the stall stands in is told.
            let (stall, stall_zone) = e.stall_close(me).await.map_err(econ_err)?;
            hub.notify(&stall_zone, HubNotice::StallClosed { stall })
                .await;
            Ok(EconReply::Done)
        }
        EconOp::BuyOrderPost {
            material,
            price,
            quantity,
        } => id(e.buy_order_post(me, &material, price, quantity).await),
        EconOp::BuyOrderFill { order, item } => done(e.buy_order_fill(me, order, item).await),
        EconOp::BuyOrderCancel { order } => done(e.buy_order_cancel(me, order).await),
        EconOp::ContractPost {
            instance,
            price,
            collateral,
        } => id(e.contract_post(me, &instance, price, collateral).await),
        EconOp::ContractCancel { contract } => done(e.contract_cancel(me, contract).await),
        EconOp::ContractAccept { contract, sellers } => {
            done(e.contract_accept(me, contract, &sellers).await)
        }
        EconOp::ChestDeposit { chest, item } => done(e.chest_deposit(me, chest, item).await),
        EconOp::ChestWithdraw { chest, item } => done(e.chest_withdraw(me, chest, item).await),
        EconOp::HireList { price } => done(e.hire_list(me, price).await),
        EconOp::Hire { avatar } => e
            .hire(me, avatar, capacity(hub, me).await?)
            .await
            .map(|(hire, _burned)| EconReply::Id(hire))
            .map_err(econ_err),
        EconOp::Tavern => Ok(EconReply::Tavern(
            e.tavern()
                .await
                .map_err(econ_err)?
                .into_iter()
                // A listed character whose stored build no longer parses is not for hire.
                .filter_map(|t| {
                    Some(TavernEntry {
                        character: t.character,
                        name: t.name,
                        price: t.price,
                        hires: t.hires,
                        build: serde_json::from_value(t.build).ok()?,
                    })
                })
                .collect(),
        )),
        EconOp::Squad => Ok(EconReply::Squad(
            hub.hired(me, capacity(hub, me).await?).await?,
        )),
        EconOp::Dismiss { hire } => {
            e.dismiss(me, hire).await.map_err(econ_err)?;
            if let Some(z) = &zone {
                hub.notify(z, HubNotice::HireEnded { hirer: me, hire })
                    .await;
            }
            Ok(EconReply::Done)
        }
    }
}

/// The squad capacity of a character's stored build (COMPANIONS.md 3.2).
async fn capacity(hub: &Hub, character: CharacterId) -> Result<usize, HubError> {
    Ok(hub
        .db
        .character(character)
        .await?
        .ok_or(HubError::NotFound)?
        .build
        .squad_capacity(&hub.cfg.content))
}

/// What a zone reports. A zone speaks only for itself: it grants to characters playing in
/// it and decides contracts whose instance it is.
async fn zone_econ_op(hub: &Hub, zone: &ZoneId, op: ZoneEconOp) -> Result<EconReply, HubError> {
    let e = &hub.econ;
    match op {
        ZoneEconOp::GrantComponents { grants, reference } => {
            if grants.len() > 256 {
                return Err(HubError::Invalid("too many grants".into()));
            }
            for (character, _) in &grants {
                if hub.db.zone_of(*character).await?.as_ref() != Some(zone) {
                    return Err(HubError::Unauthorized);
                }
            }
            e.grant_components(zone, &grants, reference)
                .await
                .map(EconReply::Ids)
                .map_err(econ_err)
        }
        ZoneEconOp::GrantCoin {
            character,
            amount,
            reference,
        } => {
            if hub.db.zone_of(character).await?.as_ref() != Some(zone) {
                return Err(HubError::Unauthorized);
            }
            if amount > hub.cfg.max_coin_grant {
                return Err(HubError::Invalid("coin drops are tiny".into()));
            }
            e.grant_coin(character, amount, reference)
                .await
                .map(|()| EconReply::Done)
                .map_err(econ_err)
        }
        ZoneEconOp::GrantKill {
            reference,
            components,
            coin,
        } => {
            if components.len() > 256 || coin.len() > 64 {
                return Err(HubError::Invalid("too many grants".into()));
            }
            if coin
                .iter()
                .any(|(_, amount)| *amount > hub.cfg.max_coin_grant)
            {
                return Err(HubError::Invalid("coin drops are tiny".into()));
            }
            // A zone speaks only for characters playing in it. One who left between the
            // kill and this report forfeits: its coin is not made, its components lie on
            // the ground where the boss died.
            let mut here: Vec<(CharacterId, bool)> = Vec::new();
            for character in components
                .iter()
                .map(|(c, _)| *c)
                .chain(coin.iter().map(|(c, _)| *c))
            {
                if !here.iter().any(|(c, _)| *c == character) {
                    let playing = hub.db.zone_of(character).await?.as_ref() == Some(zone);
                    here.push((character, playing));
                }
            }
            let is_here = |c: CharacterId| here.iter().any(|(h, playing)| *h == c && *playing);
            let components: Vec<(Option<i64>, String)> = components
                .into_iter()
                .map(|(c, material)| (is_here(c).then_some(c), material))
                .collect();
            let coin: Vec<(i64, i64)> = coin.into_iter().filter(|(c, _)| is_here(*c)).collect();
            match e
                .grant_kill(zone, reference, &components, &coin)
                .await
                .map_err(econ_err)?
            {
                Some(ids) => Ok(EconReply::Ids(ids)),
                None => Ok(EconReply::Done),
            }
        }
        ZoneEconOp::StallOpen {
            character,
            tile_x,
            tile_y,
        } => {
            if hub.db.zone_of(character).await?.as_ref() != Some(zone) {
                return Err(HubError::Unauthorized);
            }
            let id = match e.stall_open(character, zone, tile_x, tile_y).await {
                Ok(id) => id,
                // The unique constraints: the tile is taken, or the character has a stall.
                Err(EconError::State(_)) => return Err(HubError::Taken),
                Err(other) => return Err(econ_err(other)),
            };
            let mut stalls = stall_summaries(hub, zone, Some(id)).await?;
            stalls.pop().map(EconReply::Stall).ok_or(HubError::Internal)
        }
        ZoneEconOp::StallClose { character } => {
            if hub.db.zone_of(character).await?.as_ref() != Some(zone) {
                return Err(HubError::Unauthorized);
            }
            let (stall, stall_zone) = e.stall_close(character).await.map_err(econ_err)?;
            hub.notify(&stall_zone, HubNotice::StallClosed { stall })
                .await;
            Ok(EconReply::Done)
        }
        ZoneEconOp::Stalls => Ok(EconReply::Stalls(stall_summaries(hub, zone, None).await?)),
        ZoneEconOp::Drop { character, item } | ZoneEconOp::Pickup { character, item }
            if hub.db.zone_of(character).await?.as_ref() != Some(zone) =>
        {
            let _ = item;
            Err(HubError::Unauthorized)
        }
        ZoneEconOp::Drop { character, item } => e
            .drop_item(character, item, zone)
            .await
            .map(|()| EconReply::Done)
            .map_err(econ_err),
        ZoneEconOp::Pickup { character, item } => e
            .pickup(character, item, zone)
            .await
            .map(|()| EconReply::Done)
            .map_err(econ_err),
        ZoneEconOp::ContractReport { contract, outcome } => {
            if e.contract_instance(contract).await.map_err(econ_err)? != *zone {
                return Err(HubError::Unauthorized);
            }
            let outcome = match outcome {
                ContractOutcome::Completed => Outcome::Completed,
                ContractOutcome::Wipe => Outcome::Wipe,
                ContractOutcome::Abandon => Outcome::Abandon,
            };
            e.contract_report(contract, outcome)
                .await
                .map(EconReply::Decided)
                .map_err(econ_err)
        }
    }
}

/// The stalls of a zone as its zone shows them.
async fn stall_summaries(
    hub: &Hub,
    zone: &ZoneId,
    only: Option<i64>,
) -> Result<Vec<StallSummary>, HubError> {
    let rows = hub.econ.stalls_in(zone, only).await.map_err(econ_err)?;
    rows.into_iter()
        .map(|(id, tile_x, tile_y, owner, owner_name, build, model)| {
            let build: Build = serde_json::from_value(build).map_err(|_| HubError::Internal)?;
            Ok(StallSummary {
                id,
                tile_x,
                tile_y,
                owner,
                owner_name,
                frame: gm_model::rig::frame_index(build.frame),
                armour: build.armour as u8,
                model: model.and_then(|(hash, frame)| {
                    Some(ModelRef {
                        id: hash.try_into().ok()?,
                        frame: frame as u8,
                    })
                }),
            })
        })
        .collect()
}

/// The tick rate the hub validates content against (builds do not depend on it, but the pack
/// does).
pub fn content_rate() -> TickRate {
    TickRate::COMBAT
}
