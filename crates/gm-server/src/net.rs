//! Connection handling for QUIC and WebTransport clients alike (PROTOCOL.md 1, 8; WEB.md 2): handshake on the control stream, datagram
//! receive loop, control message writer. Everything the tick loop needs arrives as
//! [`ClientEvent`]s on one channel.

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use gm_core::build::{Build, ContentPack};
use gm_core::vocab::EntityId;
use gm_hub_proto::protocol::{
    CharacterId, GearReading, HiredAvatar, ModelId, ModelRef, PartyNews, PartyReading, PartyReply,
    SayTo, StallSummary,
};
use gm_net::PROTOCOL_VERSION;
use gm_net::control::{self, BuildChoice, FromClient, FromZone, GmOp};
use gm_net::input::InputDatagram;
use gm_net::link::{Link, WebEndpoint, web_accept};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// What the tick loop hands back to a joining connection.
#[derive(Clone, Debug)]
pub struct JoinInfo {
    pub entity: EntityId,
    pub server_tick: u32,
    pub build: Build,
    pub team: u8,
    /// The zone's content as it stands (a game master may have tuned it, GM.md 3).
    pub pack: Arc<ContentPack>,
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
    /// The character's active hires: its companions here (COMPANIONS.md 3.3).
    pub squad: Vec<HiredAvatar>,
    /// What its worn items do, as the hub read it at the claim (ITEMS.md 3.3).
    pub gear: GearReading,
    /// The party it is in, as the hub read it at the claim (PARTY.md 3).
    pub party: PartyReading,
    /// The hub says the character is a game master here (GM.md 1).
    pub gm: bool,
}

/// What a client asks about its party (PARTY.md 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartyAsk {
    Invite(String),
    Answer { from: String, join: bool },
    Leave,
    Remove(String),
}

/// A person asks a screen's question a second at most, and the zone answers every one it
/// is handed: so that a program sending thousands costs the tick loop nothing, a
/// connection hands on four a second with eight in hand and drops the rest unread.
struct AskBucket {
    /// (Double precision, as the hub's bucket: a flood of asks nanoseconds apart adds
    /// what a single-precision sum would lose, and the bucket refills under it.)
    tokens: f64,
    last: Instant,
}

impl AskBucket {
    const PER_SEC: f64 = 4.0;
    const FULL: f64 = 8.0;

    fn new(now: Instant) -> AskBucket {
        AskBucket {
            tokens: Self::FULL,
            last: now,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let gained = now.duration_since(self.last).as_secs_f64() * Self::PER_SEC;
        self.tokens = (self.tokens + gained).min(Self::FULL);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub enum ClientEvent {
    Join {
        name: String,
        build: Option<BuildChoice>,
        team: u8,
        hub: Option<HubJoin>,
        conn: Link,
        control: mpsc::Sender<FromZone>,
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
    /// A periodic save of this character was refused: the hub does not have it here.
    SaveRefused {
        character: CharacterId,
    },
    /// The save a character left with has been answered.
    LeftSaved {
        character: CharacterId,
    },
    /// The save that followed an accepted respec has been answered (MATRIX.md 9.1): the
    /// build the player chose is the hub's now, or it is not and the player is told why.
    RespecSaved {
        id: EntityId,
        result: Result<(), String>,
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
    /// The client buys a listing of a stall it says it stands at (ITEMS.md 5).
    StallBuy {
        id: EntityId,
        stall: i64,
        listing: i64,
        price: i64,
    },
    StallBought {
        id: EntityId,
        listing: i64,
        result: Result<(), String>,
    },
    /// The client puts an item on (`on`) or takes it off (ITEMS.md 2), and the hub's
    /// answer: what the character's worn items do from now on.
    Wear {
        id: EntityId,
        item: i64,
        on: bool,
    },
    /// `tell`: the client is waiting for this answer. (An answer that came after the
    /// zone stopped waiting is still what the character wears, and is applied.)
    Worn {
        id: EntityId,
        character: CharacterId,
        item: i64,
        result: Result<GearReading, String>,
        tell: bool,
    },
    /// The client asks something about its party, and the hub's answer (PARTY.md 4).
    Party {
        id: EntityId,
        ask: PartyAsk,
    },
    /// `tell`: the client is waiting for this answer (one that came after the zone
    /// stopped waiting is still what the hub did, and is applied).
    PartyAnswered {
        id: EntityId,
        ask: PartyAsk,
        result: Result<PartyReply, String>,
        tell: bool,
    },
    /// A line for the party or for one character anywhere, already checked as chat
    /// (PARTY.md 5), and the hub's answer to it.
    Say {
        id: EntityId,
        to: SayTo,
        text: String,
    },
    /// `result`: for a whisper, the name of whom it went to.
    Said {
        id: EntityId,
        whisper: bool,
        text: String,
        result: Result<String, String>,
    },
    /// The client asks the body `with` for a trade, or answers its asking (PARTY.md 6);
    /// and the hub's answer when both had asked.
    TradeAsk {
        id: EntityId,
        with: EntityId,
    },
    TradeOpened {
        a: EntityId,
        b: EntityId,
        /// Their names as they were when both asked: one of them may be gone by the time
        /// the hub answers, and the other is still told whose trade it holds.
        names: (String, String),
        result: Result<i64, String>,
        /// The first answer to `b`'s asking (the one its gate waits for); a trade the hub
        /// opened after the zone stopped waiting comes with `false`.
        tell: bool,
    },
    /// The hub's word on parties and lines that concern characters here.
    HubParty(PartyNews),
    /// The number of the hub's reading of a character's party, from a save's answer; and
    /// the reading itself, asked for because the zone's was behind (PARTY.md 3.2).
    PartySeq {
        character: CharacterId,
        seq: u64,
    },
    PartyRead {
        character: CharacterId,
        reading: PartyReading,
    },
    HubInvited {
        to: CharacterId,
        from: String,
    },
    HubDeclined {
        to: CharacterId,
        by: String,
    },
    HubHeard {
        to: Vec<CharacterId>,
        channel: u8,
        from: String,
        text: String,
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
    /// A game master's act (GM.md 2); the zone refuses anyone else's.
    Gm {
        id: EntityId,
        op: GmOp,
    },
    StallsLoaded(Vec<StallSummary>),
    HubStallClosed {
        stall: i64,
    },
    Respec {
        id: EntityId,
        build: BuildChoice,
    },
    /// The client reports a body (ANTICHEAT.md 5).
    Report {
        id: EntityId,
        target: EntityId,
        reason: control::ReportReason,
    },
    /// The hub opened the report (its id), or refused it.
    ReportOpened {
        id: EntityId,
        result: Result<i64, String>,
    },
    /// An order for the client's squad (COMPANIONS.md 5.3).
    Order {
        id: EntityId,
        slots: u8,
        order: control::Order,
    },
    /// The hub says a hire ended early: its companion leaves (COMPANIONS.md 3.3).
    HubHireEnded {
        hirer: CharacterId,
        hire: i64,
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
    /// The prop keys a `Look` indexes, sent with the content (LOOK.md 6.2).
    pub props: Arc<Vec<String>>,
    /// The hub, when this zone runs under one: tokens are then mandatory.
    pub hub: Option<Arc<crate::hub_link::HubLink>>,
    /// What each account that plays here may still say (PROTOCOL.md 8): its characters
    /// share one bucket, and a connection that comes back finds it as it left it.
    pub chat: ChatBuckets,
}

/// Chat buckets by account, kept a minute past the last connection that used them.
#[derive(Default)]
pub struct ChatBuckets(
    std::sync::Mutex<std::collections::HashMap<i64, Arc<std::sync::Mutex<ChatBucket>>>>,
);

impl ChatBuckets {
    /// How long an account's bucket is kept after its last connection went.
    const KEPT: Duration = Duration::from_secs(60);

    /// The bucket of `account`: the one it has, or a full one.
    fn of(&self, account: i64, now: Instant) -> Arc<std::sync::Mutex<ChatBucket>> {
        let mut all = self.0.lock().unwrap();
        all.retain(|_, b| {
            Arc::strong_count(b) > 1 || now.duration_since(b.lock().unwrap().at) < Self::KEPT
        });
        all.entry(account)
            .or_insert_with(|| Arc::new(std::sync::Mutex::new(ChatBucket::new(now))))
            .clone()
    }
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
            let conn = match incoming.await {
                Ok(c) => Link::Quic(c),
                Err(e) => {
                    debug!(%remote, "handshake failed: {e}");
                    return;
                }
            };
            if let Err(e) = handle_connection(conn, tx, cfg).await {
                debug!(%remote, "connection ended: {e:#}");
            }
        });
    }
}

/// A WebTransport listener beside the QUIC one (WEB.md 2.1).
pub struct WebListener {
    pub endpoint: WebEndpoint,
    /// `Origin`s a session may come from; empty = any.
    pub origins: Vec<String>,
}

/// Accept WebTransport sessions for as long as the zone runs: browsers, and bots with
/// `--web`. Past the accept they are clients like any other.
pub async fn accept_loop_web(
    listener: WebListener,
    tx: mpsc::Sender<ClientEvent>,
    cfg: Arc<NetConfig>,
) {
    loop {
        let incoming = web_accept(&listener.endpoint, &listener.origins).await;
        if tx.is_closed() {
            break;
        }
        let tx = tx.clone();
        let cfg = cfg.clone();
        tokio::spawn(async move {
            let remote = incoming.remote_address();
            let conn = match tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming.accept()).await {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    debug!(%remote, "web session refused: {e}");
                    return;
                }
                Err(_) => {
                    debug!(%remote, "web session: no request within the handshake timeout");
                    return;
                }
            };
            // A session that cannot carry a full snapshot datagram is refused now rather
            // than starved later (WEB.md 2.1).
            if conn.max_datagram_size().unwrap_or(0) < gm_net::MAX_DATAGRAM_PAYLOAD {
                debug!(%remote, "web session: datagrams too small or unsupported");
                conn.close(1, b"datagrams unsupported");
                return;
            }
            if let Err(e) = handle_connection(conn, tx, cfg).await {
                debug!(%remote, "web connection ended: {e:#}");
            }
        });
    }
}

/// Send a `Reject` and close, in that order as the client sees it: the stream is finished
/// and the client is given a moment to read and hang up first, because a connection closed
/// under an unread message can take the message with it (WEB.md 2.1).
/// Lines refused, not yet forgotten, at which a client is taken for a flood and let go;
/// one is forgotten every `CHAT_REFUSAL_FORGOTTEN_SECS` (somebody who overruns a line
/// now and then is never let go for it).
const CHAT_REFUSALS_BEFORE_KICK: f32 = 30.0;
const CHAT_REFUSAL_FORGOTTEN_SECS: f32 = 10.0;

/// An account's chat lines (PROTOCOL.md 8): `CHAT_BURST` at once, one more every
/// `CHAT_REFILL_SECS`.
pub struct ChatBucket {
    lines: f32,
    at: Instant,
    /// Lines refused and not yet forgotten.
    refused: f32,
}

impl ChatBucket {
    fn new(now: Instant) -> ChatBucket {
        ChatBucket {
            lines: control::CHAT_BURST as f32,
            at: now,
            refused: 0.0,
        }
    }

    /// Time passes: lines come back, refusals are forgotten.
    fn tick(&mut self, now: Instant) {
        let secs = now.duration_since(self.at).as_secs_f32();
        self.lines =
            (self.lines + secs / control::CHAT_REFILL_SECS).min(control::CHAT_BURST as f32);
        self.refused = (self.refused - secs / CHAT_REFUSAL_FORGOTTEN_SECS).max(0.0);
        self.at = now;
    }

    /// Whether a line may be said now.
    fn allow(&mut self, now: Instant) -> bool {
        self.tick(now);
        if self.lines < 1.0 {
            return false;
        }
        self.lines -= 1.0;
        true
    }

    /// A line was refused (over the rate, or not a line at all). `true`: that was one
    /// too many, the connection goes.
    fn refuse(&mut self, now: Instant) -> bool {
        self.tick(now);
        self.refused += 1.0;
        self.refused >= CHAT_REFUSALS_BEFORE_KICK
    }
}

async fn refuse(conn: &Link, send: &mut gm_net::link::SendHalf, msg: &FromZone) {
    let _ = control::send(send, msg).await;
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(1), conn.closed()).await;
    conn.close(1, b"rejected");
}

async fn handle_connection(
    conn: Link,
    tx: mpsc::Sender<ClientEvent>,
    cfg: Arc<NetConfig>,
) -> anyhow::Result<()> {
    let remote = conn.remote_address();
    let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.accept_bi())
        .await
        .map_err(|_| anyhow::anyhow!("no control stream within the handshake timeout"))??;
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, control::recv(&mut recv))
        .await
        .map_err(|_| anyhow::anyhow!("no Hello within the handshake timeout"))??;
    let Some(FromClient::Hello {
        version,
        name,
        token,
        build,
        team,
    }) = hello
    else {
        anyhow::bail!("first control message was not Hello");
    };
    let reject = |reason: &str| FromZone::Reject(reason.to_string());
    let rejection = if version != PROTOCOL_VERSION as u16 {
        // An old page or client: say what to do, since the ticket it holds is spent.
        Some(reject(&format!(
            "protocol version mismatch (the zone speaks {PROTOCOL_VERSION}, this client {version}): \
             the game was updated; reload the page or get the new client"
        )))
    } else if token.is_empty() && (!cfg.open || cfg.hub.is_some()) {
        Some(reject("session token required"))
    } else {
        None
    };
    if let Some(r) = rejection {
        refuse(&conn, &mut send, &r).await;
        return Ok(());
    }
    // Under a hub the token names the character; `Hello.name` is ignored (HUB.md 3.1).
    // What was claimed is given back if the zone then does not take the body.
    let mut claimed_here = None;
    let mut account = None;
    let (name, build, hub_join) = match &cfg.hub {
        Some(hub) if !token.is_empty() => {
            match hub.verify(&token) {
                Ok(payload) => account = Some(payload.account),
                Err(e) => {
                    refuse(&conn, &mut send, &reject(&e)).await;
                    return Ok(());
                }
            }
            match hub.claim(&token).await {
                Ok(claimed) => {
                    claimed_here = Some((hub.clone(), claimed.character));
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
                            squad: claimed.squad,
                            gear: claimed.gear,
                            party: claimed.party,
                            gm: claimed.gm,
                        }),
                    )
                }
                Err(e) => {
                    refuse(&conn, &mut send, &reject(&format!("claim failed: {e}"))).await;
                    return Ok(());
                }
            }
        }
        _ => match control::valid_name(&name) {
            Some(n) => (n, build, None),
            None => {
                refuse(&conn, &mut send, &reject("invalid name")).await;
                return Ok(());
            }
        },
    };

    let (control_tx, mut control_rx) = mpsc::channel(CONTROL_CHANNEL);
    // The connection's own way to say something to its client (a chat line refused).
    let to_client = control_tx.clone();
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
            // The hub has the character in this zone since the claim, and the zone has no
            // body for it (it is full, the build is not valid here): the character goes
            // back offline as it came, or it could enter nowhere until this zone restarts.
            // (Not a save: it never stood here, and a save would say it did.)
            if let Some((hub, character)) = claimed_here {
                hub.release(character).await;
            }
            refuse(&conn, &mut send, &FromZone::Reject(reason)).await;
            return Ok(());
        }
        Err(_) => anyhow::bail!("zone stopped during join"),
    };
    let id = info.entity;
    // From here the zone has a body for this connection: every way out says `Leave`.
    let welcome = FromZone::Welcome {
        entity: id,
        server_tick: info.server_tick,
        hz: cfg.hz,
        map: cfg.map_name.clone(),
        map_hash: cfg.map_hash,
    };
    let content = FromZone::Content {
        pack: (*info.pack).clone(),
        own: info.build.clone(),
        team: info.team,
        props: (*cfg.props).clone(),
    };
    let greeted = async {
        control::send(&mut send, &welcome).await?;
        control::send(&mut send, &content).await
    }
    .await;
    if let Err(e) = greeted {
        let _ = tx.send(ClientEvent::Leave { id }).await;
        anyhow::bail!("{remote}: the client went before it was welcomed: {e}");
    }
    info!(%remote, entity = info.entity, %name, team = info.team, web = conn.is_web(), "player joined");
    // An account's bucket is its own wherever it speaks from; without a hub there are no
    // accounts, and a connection has one of its own.
    let chat = match account {
        Some(account) => cfg.chat.of(account, Instant::now()),
        None => Arc::new(std::sync::Mutex::new(ChatBucket::new(Instant::now()))),
    };
    let mut flooding = false;
    // What a screen asks of the zone (a buy, a change of what is worn): a few a second
    // reach the tick loop, the rest are dropped here (PROTOCOL.md 18).
    let mut asks = AskBucket::new(Instant::now());

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
                    Ok(Some(
                        msg @ (FromClient::Chat(_)
                        | FromClient::PartySay(_)
                        | FromClient::Whisper { .. }),
                    )) => {
                        // A line to the zone, to the party or to one character: all
                        // three are chat (PARTY.md 5), and are checked here, before the
                        // queue everybody's frames share (PROTOCOL.md 8): a line out of
                        // bounds is dropped, a line too many is refused to its sender
                        // alone, and a client that keeps at it is let go.
                        let (text, to) = match msg {
                            FromClient::Chat(text) => (text, None),
                            FromClient::PartySay(text) => (text, Some(SayTo::Party)),
                            FromClient::Whisper { to, text } => (text, Some(SayTo::Whisper(to))),
                            _ => unreachable!("matched above"),
                        };
                        let now = Instant::now();
                        // A whisper to a name nobody can have goes nowhere, and is said
                        // so: it is a mistake of the hand, not a line too many.
                        if let Some(SayTo::Whisper(name)) = &to
                            && gm_hub_proto::names::character_name(name).is_err()
                        {
                            let _ = to_client.try_send(FromZone::ChatFrom {
                                from: 0,
                                text: "nobody can be called that".into(),
                            });
                            continue;
                        }
                        let text = control::valid_chat(&text);
                        let (said, flood) = {
                            let mut chat = chat.lock().unwrap();
                            match text {
                                Some(text) if chat.allow(now) => (Some(text), false),
                                // Not a line at all is no better than a line too many.
                                _ => (None, chat.refuse(now)),
                            }
                        };
                        if flood {
                            if !flooding {
                                flooding = true;
                                // Told why, then closed a moment later: a connection
                                // closed in the same instant can take the `Kick` with it.
                                let why = "flooding the chat";
                                let _ = to_client.try_send(FromZone::Kick(why.into()));
                                let conn = conn.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(crate::session::KICK_GRACE).await;
                                    conn.close(6, why.as_bytes());
                                });
                            }
                            continue;
                        }
                        let Some(text) = said else {
                            let _ = to_client.try_send(FromZone::ChatFrom {
                                from: 0,
                                text: "too many lines; wait a moment".into(),
                            });
                            continue;
                        };
                        let said = match to {
                            None => ClientEvent::Chat { id, text },
                            Some(to) => ClientEvent::Say { id, to, text },
                        };
                        if tx.send(said).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(
                        msg @ (FromClient::PartyInvite { .. }
                        | FromClient::PartyAnswer { .. }
                        | FromClient::PartyLeave
                        | FromClient::PartyRemove { .. }
                        | FromClient::TradeAsk { .. }),
                    )) => {
                        if !asks.take(Instant::now()) {
                            continue;
                        }
                        let party = |ask| ClientEvent::Party { id, ask };
                        let ev = match msg {
                            FromClient::PartyInvite { name } => party(PartyAsk::Invite(name)),
                            FromClient::PartyAnswer { from, join } => {
                                party(PartyAsk::Answer { from, join })
                            }
                            FromClient::PartyLeave => party(PartyAsk::Leave),
                            FromClient::PartyRemove { name } => party(PartyAsk::Remove(name)),
                            FromClient::TradeAsk { with } => ClientEvent::TradeAsk { id, with },
                            _ => unreachable!("matched above"),
                        };
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::Respec(build))) => {
                        if tx.send(ClientEvent::Respec { id, build }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::Gm(op))) => {
                        if tx.send(ClientEvent::Gm { id, op }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::Travel(zone))) => {
                        if tx.send(ClientEvent::Travel { id, zone }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::StallOpen)) => {
                        if tx.send(ClientEvent::StallOpen { id }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::StallClose)) => {
                        if tx.send(ClientEvent::StallClose { id }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::StallBuy {
                        stall,
                        listing,
                        price,
                    })) => {
                        if !asks.take(Instant::now()) {
                            continue;
                        }
                        let ev = ClientEvent::StallBuy {
                            id,
                            stall,
                            listing,
                            price,
                        };
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(msg @ (FromClient::Wear { .. } | FromClient::TakeOff { .. }))) => {
                        if !asks.take(Instant::now()) {
                            continue;
                        }
                        let ev = match msg {
                            FromClient::Wear { item } => ClientEvent::Wear { id, item, on: true },
                            FromClient::TakeOff { item } => ClientEvent::Wear { id, item, on: false },
                            _ => unreachable!("matched above"),
                        };
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::Report { target, reason })) => {
                        if tx.send(ClientEvent::Report { id, target, reason }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::Order { slots, order })) => {
                        if tx.send(ClientEvent::Order { id, slots, order }).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(FromClient::Bye)) | Ok(None) => break,
                    // (A second Hello; the one message this arm can see. Not quoted: it
                    // may be as long as a frame.)
                    Ok(Some(_)) => warn!(entity = id, "unexpected control message after the handshake"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_of_screen_requests_is_dropped_before_the_tick_loop() {
        let t0 = Instant::now();
        let at = |secs: f32| t0 + Duration::from_secs_f32(secs);
        let mut asks = AskBucket::new(t0);
        // Eight in hand, then four a second: ten thousand in a second hand on twelve.
        let passed = (0..10_000)
            .filter(|i| asks.take(at(*i as f32 / 10_000.0)))
            .count();
        assert!((11..=12).contains(&passed), "{passed}");
        // A person's pace is never stopped: one a second for a minute.
        let mut asks = AskBucket::new(t0);
        assert!((0..60).all(|i| asks.take(at(i as f32))));
    }

    #[test]
    fn a_chat_bucket_gives_five_lines_and_forgets_refusals_slowly() {
        let t0 = Instant::now();
        let at = |secs: f32| t0 + Duration::from_secs_f32(secs);
        let mut b = ChatBucket::new(t0);
        // Five at once, the sixth not; one more every two seconds.
        assert!((0..5).all(|_| b.allow(t0)));
        assert!(!b.allow(t0));
        assert!(!b.allow(at(1.9)));
        assert!(b.allow(at(2.1)));
        // Somebody who overruns a line now and then is never let go for it: thirty
        // refusals a quarter of a minute apart are thirty forgotten ones.
        let mut b = ChatBucket::new(t0);
        assert!((0..30).all(|i| !b.refuse(at(15.0 * i as f32))));
        // A flood is: thirty refusals in a breath.
        let mut b = ChatBucket::new(t0);
        assert!((0..29).all(|_| !b.refuse(t0)));
        assert!(b.refuse(t0));
        // And it stays one for whoever comes back at once on another connection: the
        // account's bucket is the same.
        let buckets = ChatBuckets::default();
        let first = buckets.of(7, t0);
        for _ in 0..30 {
            first.lock().unwrap().refuse(t0);
        }
        drop(first);
        let again = buckets.of(7, at(5.0));
        assert!(again.lock().unwrap().refuse(at(5.0)), "still a flood");
        // After a minute without a connection the bucket is forgotten.
        drop(again);
        let later = buckets.of(7, at(120.0));
        assert!(!later.lock().unwrap().refuse(at(120.0)));
        assert!(buckets.of(8, at(120.0)).lock().unwrap().allow(at(120.0)));
    }
}
