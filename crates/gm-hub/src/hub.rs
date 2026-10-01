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
use gm_net::control::{self, valid_name};
use gm_net::transport::fnv1a64;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::db::Db;
use gm_hub_proto::protocol::{
    AccountId, BuildChoice, CharacterId, HASH_PERMITS, HubError, HubNotice, HubRequest,
    HubResponse, SessionId, ZoneId, ZoneSummary, ZoneTicket, now_secs,
};

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
}

struct Session {
    account: AccountId,
    created: Instant,
}

struct ZoneEntry {
    addr: SocketAddr,
    cert_der: Vec<u8>,
    map: String,
    #[allow(dead_code)]
    map_hash: u64,
    players: u32,
    #[allow(dead_code)]
    tick_mean_us: f32,
    since: Instant,
    last_heartbeat: Instant,
    conn: quinn::Connection,
}

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
    let hub = Arc::new(Hub {
        hashing: Semaphore::new(HASH_PERMITS),
        verifier: Mutex::new(HashMap::new()),
        cfg,
        db,
        state: Mutex::new(State::default()),
    });
    info!(listen = %endpoint.local_addr()?, "hub listening");
    let accept = {
        let hub = hub.clone();
        let endpoint = endpoint.clone();
        async move {
            while let Some(incoming) = endpoint.accept().await {
                let hub = hub.clone();
                tokio::spawn(async move {
                    let remote = incoming.remote_address();
                    match incoming.await {
                        Ok(conn) => handle_connection(hub, conn).await,
                        Err(e) => debug!(%remote, "handshake failed: {e}"),
                    }
                });
            }
        }
    };
    tokio::select! {
        _ = accept => {}
        _ = shutdown => {}
    }
    endpoint.close(0u32.into(), b"hub stopped");
    info!("hub stopped");
    Ok(())
}

async fn handle_connection(hub: Arc<Hub>, conn: quinn::Connection) {
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
    conn: quinn::Connection,
    remote: SocketAddr,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
) -> anyhow::Result<()> {
    let req: HubRequest = match control::recv_any(&mut recv).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    let resp = match handle(&hub, &auth, &conn, remote, req).await {
        Ok(r) => r,
        Err(e) => HubResponse::Err(e),
    };
    control::send_any(&mut send, &resp).await?;
    let _ = send.finish();
    Ok(())
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
            token: self.cfg.key.issue(account, character, zone),
        })
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
    conn: &quinn::Connection,
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
        HubRequest::ZoneHello {
            secret,
            zone,
            map,
            map_hash,
            addr,
            cert_der,
        } => {
            if secret != hub.cfg.zone_secret || hub.cfg.zone_secret.is_empty() {
                warn!(%remote, %zone, "zone hello with a wrong secret");
                return Err(HubError::Unauthorized);
            }
            if valid_name(&zone).is_none() {
                return Err(HubError::Invalid("zone id".into()));
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
                        map: map.clone(),
                        map_hash,
                        players: 0,
                        tick_mean_us: 0.0,
                        since: Instant::now(),
                        last_heartbeat: Instant::now(),
                        conn: conn.clone(),
                    },
                );
                previous
            };
            if let Some(old) = previous
                && old.stable_id() != conn.stable_id()
            {
                old.close(1u32.into(), b"replaced by a new zone process");
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
            if row.location_zone.as_deref() != Some(zone.as_str()) {
                // The saved position belongs to another zone: spawn here.
                state.zone = None;
            }
            Ok(HubResponse::Claimed {
                character: row.id,
                name: row.name.clone(),
                state,
                team: 0,
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
    }
}

/// The tick rate the hub validates content against (builds do not depend on it, but the pack
/// does).
pub fn content_rate() -> TickRate {
    TickRate::COMBAT
}
