//! Reliable control messages (PROTOCOL.md 8): `bitcode` payloads with a big-endian u16 length.

use bitcode::{Decode, Encode};
use gm_core::build::{Build, ContentPack};

/// How a client asks for a build: a preset by name (the zone resolves it against its content)
/// or a full allocation (validated by the zone against MATRIX.md 9).
/// How near the trainer a body must stand to wear a new build in the world (MATRIX.md
/// 9.1): the zone enforces it, the client offers the button by it.
pub const TRAINER_REACH: f32 = 160.0;

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum BuildChoice {
    Preset(String),
    Custom(Build),
}

/// Who drives a body (COMPANIONS.md 2.1), as a zone announces it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub enum BodyKind {
    #[default]
    Human,
    /// A companion of the player with this entity id.
    Companion { owner: u32 },
    /// An instance of creature definition `def` of the content pack.
    Creature { def: u16 },
}

/// What a body holds and wears, as indices into the pack's `props` list the zone sent
/// (LOOK.md 6.2; `NONE` for nothing). The indices are of that session's pack: a client
/// reads an index past the list as `NONE`. `worn` is the armour overlay of Phase 16 and
/// always `NONE` until then.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Look {
    pub held: u16,
    pub worn: u16,
}

impl Look {
    pub const NONE: u16 = u16::MAX;
    pub const EMPTY: Look = Look {
        held: Look::NONE,
        worn: Look::NONE,
    };
}

impl Default for Look {
    fn default() -> Look {
        Look::EMPTY
    }
}

/// One body as a zone announces it: a player, a companion or a creature.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct PlayerEntry {
    pub id: u32,
    pub name: String,
    pub team: u8,
    pub model: Option<[u8; 32]>,
    pub kind: BodyKind,
    /// v8: what it holds.
    pub look: Look,
}

/// A standing order (COMPANIONS.md 5.3).
#[derive(Clone, Copy, Debug, PartialEq, Encode, Decode)]
pub enum Order {
    Follow,
    Hold,
    MoveTo([f32; 3]),
    Attack(u32),
}

/// One companion as its commander is told about it (COMPANIONS.md 13).
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct SquadEntry {
    pub id: u32,
    pub name: String,
    /// `gm_ai::Role` index: 0 heal, 1 tank, 2 scout, 3 dps.
    pub role: u8,
    pub order: Order,
    pub max_health: u16,
    /// Lent by the zone rather than hired.
    pub recruit: bool,
}

/// What an encounter did (COMPANIONS.md 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum EncounterState {
    Engaged,
    Reset,
    Cleared { secs: u32 },
}

/// An open stall as a zone shows it (ECONOMY.md 7): where it stands and who keeps it. The
/// Why a client reports a body (ANTICHEAT.md 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ReportReason {
    /// Its aim is not a hand's.
    Aim,
    /// It plays to spoil the game for its own side.
    Griefing,
    Other,
}

impl ReportReason {
    pub fn name(self) -> &'static str {
        match self {
            ReportReason::Aim => "aim",
            ReportReason::Griefing => "griefing",
            ReportReason::Other => "other",
        }
    }
}

/// Where a WebTransport listener is and how a browser may trust it (WEB.md 2.3).
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct WebAddr {
    /// `https://host:port`.
    pub url: String,
    /// The pinned certificate's SHA-256; `None` means a publicly trusted certificate.
    pub cert_sha256: Option<[u8; 32]>,
}

/// keeper is drawn as a body that never moves; it costs no snapshot bytes.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct StallEntry {
    pub id: i64,
    /// Centre of the tile, on the ground.
    pub pos: [f32; 3],
    /// Yaw the keeper faces.
    pub yaw: f32,
    pub owner: String,
    /// The keeper's frame, armour class and avatar model, as in `PlayerInfo`.
    pub frame: u8,
    pub armour: u8,
    pub model: Option<[u8; 32]>,
}

/// How near a body must stand to a stall to buy at it (ITEMS.md 5): this far from the
/// centre of its tile along the ground (a tile is 128 wide, and a body in front of the
/// counter is about 80 from its middle; the next stall's middle is 160 away), its feet no
/// further above or below the tile than `STALL_REACH_UP`.
pub const STALL_REACH: f32 = 120.0;
pub const STALL_REACH_UP: f32 = 96.0;

/// How near two bodies must stand for one to ask the other for a trade (PARTY.md 6):
/// this far apart along the ground, and no further above or below than
/// `TRADE_REACH_UP`. A request lapses after `TRADE_ASK_SECS`.
pub const TRADE_REACH: f32 = 160.0;
pub const TRADE_REACH_UP: f32 = 96.0;
pub const TRADE_ASK_SECS: u64 = 30;

/// May two bodies whose feet are at `a` and `b` ask each other for a trade? The zone
/// decides with this, and a client offers the button with the same.
pub fn trade_in_reach(a: [f32; 3], b: [f32; 3]) -> bool {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy <= TRADE_REACH * TRADE_REACH && dz.abs() <= TRADE_REACH_UP
}

/// The channels of `Heard`.
pub const CHANNEL_PARTY: u8 = 1;
pub const CHANNEL_WHISPER: u8 = 2;
pub const CHANNEL_WHISPERED: u8 = 3;

/// May a body whose feet are at `feet` buy at the stall whose tile's centre is `stall`?
/// The zone decides with this, and a client offers the stall with the same.
pub fn stall_in_reach(stall: [f32; 3], feet: [f32; 3]) -> bool {
    let (dx, dy, dz) = (feet[0] - stall[0], feet[1] - stall[1], feet[2] - stall[2]);
    dx * dx + dy * dy <= STALL_REACH * STALL_REACH && dz.abs() <= STALL_REACH_UP
}

/// What a client says to its zone (PROTOCOL.md 8).
///
/// The two directions of the control stream are two types: each side carries the code to
/// write the one and to read the other (the browser build is a megabyte, and the code for
/// one enum of everything was a twelfth of it), and neither can send what is not its to
/// send. A client and a zone of different versions must still be able to say so, so three
/// messages keep the bytes they had when both directions were one enum (to v6): `Hello`
/// is number 0 here, and `Reject` and `Kick` are numbers 14 and 25 of [`FromZone`]. What a
/// version adds goes at the end of its enum.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum FromClient {
    Hello {
        version: u16,
        name: String,
        token: Vec<u8>,
        /// `None`: the zone's default preset.
        build: Option<BuildChoice>,
        /// Preferred team, 0 = let the zone balance.
        team: u8,
    },
    Chat(String),
    /// Applied at the next respawn (MATRIX.md 9); answered by `RespecResult`.
    Respec(BuildChoice),
    /// Ask the hub (through the zone) for a ticket to another zone (HUB.md 3.3).
    Travel(String),
    /// Open a stall on the market tile the player stands on; answered by `StallResult`.
    StallOpen,
    /// Close the own stall; answered by `StallResult`.
    StallClose,
    /// An order for the squad slots in `slots` (bit i = slot i), from the command stance
    /// (COMPANIONS.md 5.3). A refusal is answered by `OrderRefused`; an accepted order shows
    /// in the next `Squad`.
    Order {
        slots: u8,
        order: Order,
    },
    Bye,
    /// Report the body `target` (ANTICHEAT.md 5): one the client is being sent, or one
    /// that left within the last two minutes.
    Report {
        target: u32,
        reason: ReportReason,
    },
    /// Buy a listing of the stall the player stands at, at the price it was shown
    /// (ITEMS.md 5); answered by `BuyResult`, always.
    StallBuy {
        stall: i64,
        listing: i64,
        price: i64,
    },
    /// Put an item of the inventory on, or take one off (ITEMS.md 2); answered by
    /// `WearResult`, always. The zone is asked, because gear takes effect in the zone and
    /// only the zone knows whether the body is in a fight.
    Wear {
        item: i64,
    },
    TakeOff {
        item: i64,
    },
    // v7 (PARTY.md 4). The answers to these are lines of the zone (`ChatFrom` from
    // nobody) and, when the party changed, `Party`.
    /// Invite the character of that name, wherever in the game it is.
    PartyInvite {
        name: String,
    },
    /// Answer the invitation of the character of that name.
    PartyAnswer {
        from: String,
        join: bool,
    },
    PartyLeave,
    /// The leader takes a member out.
    PartyRemove {
        name: String,
    },
    /// A line to the party, and a line to one character anywhere in the game: chat, with
    /// chat's limits (PARTY.md 5).
    PartySay(String),
    Whisper {
        to: String,
        text: String,
    },
    /// Ask the body `with` for a trade, or answer its asking with one's own: when both
    /// have asked within half a minute, standing together, the zone has the hub open the
    /// trade and says `TradeOpened` to both (PARTY.md 6).
    TradeAsk {
        with: u32,
    },
    // v10 (GM.md 2).
    /// Something only a game master may ask (the zone says `Gm(Refused)` to anyone else).
    Gm(GmOp),
}

/// What a game master may do to a running zone (GM.md 2): tune its timings, make a body
/// whole, wear another build at once. Every change of the content is told to every client
/// as a new `Content`.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum GmOp {
    /// Every script's windups, windows and cast times take this many times as long.
    Tempo(f32),
    /// Numbers set outright on one ability (its key in the pack); all `None` forgets it.
    Ability(gm_core::tuning::AbilityTuning),
    /// Back to the content as loaded.
    ResetTuning,
    /// Full health, stamina and focus, every cooldown ready: the own body, or everyone.
    Heal { everyone: bool },
    /// This build now, not at the next respawn (validated as a respec is).
    Respec(Build),
}

/// What the zone answers a game master (GM.md 2).
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum GmNews {
    /// Sent after `Content` to a client whose character may do these things.
    Granted,
    /// The zone's tuning as it stands (after `Content` when it is not the default, and
    /// after every change).
    Tuning(gm_core::tuning::Tuning),
    /// Why the last `Gm` was not done.
    Refused(String),
}

/// What a zone says to a client (PROTOCOL.md 8). `Reject` is number 14 and `Kick` number
/// 25, as they were to v6 (see [`FromClient`]): the eight messages before `Welcome` stand
/// where the client's eight stood then.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum FromZone {
    /// To a commander: its squad, in slot order, whenever a member or an order changes.
    Squad(Vec<SquadEntry>),
    OrderRefused(String),
    /// An encounter the client takes part in, or stands near, changed state.
    Encounter {
        name: String,
        state: EncounterState,
    },
    /// What a boss kill gave this client (COMPANIONS.md 10).
    Loot {
        encounter: String,
        items: Vec<String>,
        coin: u32,
    },
    /// The answer to `Report`: the report was taken and the fight around it is kept, or
    /// why not.
    ReportResult(Result<(), String>),
    /// A trial's verdict on this client (COMPANIONS.md 11): passed, or why not.
    Trial {
        key: String,
        name: String,
        passed: bool,
        detail: String,
        secs: u32,
    },
    /// The answer to `StallBuy`, naming the listing it was about; a refusal is in words
    /// for the buyer.
    BuyResult {
        listing: i64,
        result: Result<(), String>,
    },
    /// The answer to `Wear` or `TakeOff`, naming the item it was about.
    WearResult {
        item: i64,
        result: Result<(), String>,
    },
    Welcome {
        entity: u32,
        server_tick: u32,
        hz: u16,
        map: String,
        map_hash: u64,
    },
    /// Sent right after `Welcome`: the zone's content and the client's own build and team.
    /// Clients run exactly these numbers (MATRIX.md 10).
    Content {
        pack: ContentPack,
        own: Build,
        team: u8,
        /// v8: the keys of every prop the content names, in the order a `Look` indexes
        /// (LOOK.md 6.2). The client finds the files; the zone never reads one.
        props: Vec<String>,
    },
    /// The requested build was accepted (`Ok`) or refused with the reason.
    RespecResult(Result<(), String>),
    /// The pending build took effect (at the respawn); the client's prediction switches now.
    BuildApplied(Build),
    /// The hub issued a ticket for another zone: say `Bye`, connect there with `token`
    /// (HUB.md 3.3). The body stays as a ghost here until the other zone claims it.
    TravelTicket {
        zone: String,
        addr: String,
        cert_der: Vec<u8>,
        token: Vec<u8>,
        /// The zone's WebTransport listener, if it has one (WEB.md 2.3).
        web: Option<WebAddr>,
    },
    /// The travel request failed.
    TravelRefused(String),
    /// Number 14, whatever the version.
    Reject(String),
    /// To a joiner: everyone already in the zone, itself included, in one message.
    Roster(Vec<PlayerEntry>),
    /// A player joined, or what it wears changed. `model` is the id of its avatar model
    /// (MODELS.md 7), absent for the frame's mannequin.
    PlayerInfo {
        id: u32,
        name: String,
        team: u8,
        model: Option<[u8; 32]>,
        kind: BodyKind,
        /// v8: what it holds.
        look: Look,
    },
    /// A model was taken down: forget it, delete it (MODELS.md 8).
    ModelRevoked([u8; 32]),
    /// To a joiner: every open stall of the zone.
    Stalls(Vec<StallEntry>),
    StallOpened(StallEntry),
    StallClosed(i64),
    /// The answer to `StallOpen` or `StallClose`.
    StallResult(Result<(), String>),
    PlayerLeft(u32),
    Killed {
        victim: u32,
        /// 0 = the world.
        killer: u32,
    },
    ChatFrom {
        from: u32,
        text: String,
    },
    /// Number 25, whatever the version.
    Kick(String),
    // v7 (PARTY.md 4).
    /// The hub's word on the client's party: its members by name, the leader first
    /// (nobody: the client is in none).
    Party(Vec<String>),
    /// Somebody invites the client to a party.
    Invited {
        from: String,
    },
    /// A line that came through the hub: `channel` 1, a line of the party's; 2, a
    /// whisper to the client; 3, the client's own whisper as it went out (`from` is the
    /// name of whom it went to).
    Heard {
        channel: u8,
        from: String,
        text: String,
    },
    /// The body `from` asks for a trade.
    TradeAsked {
        from: u32,
    },
    /// The hub opened a trade between the client and the character of that name: the
    /// window is the hub's from here (ECONOMY.md 6).
    TradeOpened {
        trade: i64,
        with: String,
    },
    // v8 (LOOK.md 6.2).
    /// A body's look changed: what it holds, by the pack's prop list.
    Look {
        id: u32,
        look: Look,
    },
    // v10 (GM.md 2).
    Gm(GmNews),
    /// v10 (LOOK.md 13.8): the own hand landed a blow. `amount` came off the target's
    /// health after its block took `absorbed`; the number the client floats over it.
    /// The own hurts are read from the own health, which the snapshot carries.
    Hit {
        target: u32,
        amount: u16,
        absorbed: u16,
    },
    /// v10 (LOOK.md 13.8): a Regen the own hand put on another body gave it `amount`
    /// health back this pulse. The own healings are read from the own health.
    Healed {
        target: u32,
        amount: u16,
    },
}

pub const MAX_MESSAGE_BYTES: usize = u16::MAX as usize;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("control message exceeds 65535 bytes")]
    TooLarge,
    #[error("control message did not decode: {0}")]
    Decode(#[from] bitcode::Error),
    #[cfg(not(target_arch = "wasm32"))]
    #[error("stream read failed: {0}")]
    Read(#[from] quinn::ReadExactError),
    #[cfg(not(target_arch = "wasm32"))]
    #[error("stream write failed: {0}")]
    Write(#[from] quinn::WriteError),
    /// The browser's stream failed (WEB.md 2.1).
    #[error("stream failed: {0}")]
    Stream(String),
}

/// Length-prefixed bytes for one message of any `bitcode` type (the hub protocol uses the
/// same framing, HUB.md 3).
pub fn encode_framed_any<T: Encode>(msg: &T) -> Result<Vec<u8>, ControlError> {
    let payload = bitcode::encode(msg);
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(ControlError::TooLarge);
    }
    let mut out = Vec::with_capacity(payload.len() + 2);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Length-prefixed bytes for one message.
pub fn encode_framed<T: Encode>(msg: &T) -> Result<Vec<u8>, ControlError> {
    encode_framed_any(msg)
}

/// Write one message of any `bitcode` type to a QUIC stream.
#[cfg(not(target_arch = "wasm32"))]
pub async fn send_any<T: Encode>(
    stream: &mut quinn::SendStream,
    msg: &T,
) -> Result<(), ControlError> {
    let bytes = encode_framed_any(msg)?;
    stream.write_all(&bytes).await?;
    Ok(())
}

/// Read one message of any `bitcode` type; `Ok(None)` on a clean end of stream.
#[cfg(not(target_arch = "wasm32"))]
pub async fn recv_any<T: for<'a> Decode<'a>>(
    stream: &mut quinn::RecvStream,
) -> Result<Option<T>, ControlError> {
    match recv_frame(stream).await? {
        Some(payload) => Ok(Some(bitcode::decode(&payload)?)),
        None => Ok(None),
    }
}

/// Read one frame's payload, whatever it encodes; `Ok(None)` on a clean end of stream.
#[cfg(not(target_arch = "wasm32"))]
pub async fn recv_frame(stream: &mut quinn::RecvStream) -> Result<Option<Vec<u8>>, ControlError> {
    let mut len = [0u8; 2];
    match stream.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u16::from_be_bytes(len) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

/// Decode one framed message from `buf`, returning it and the bytes consumed; `None` when the
/// buffer does not yet hold a whole message.
pub fn decode_framed<T: for<'a> Decode<'a>>(
    buf: &[u8],
) -> Result<Option<(T, usize)>, ControlError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if buf.len() < 2 + len {
        return Ok(None);
    }
    let msg: T = bitcode::decode(&buf[2..2 + len])?;
    Ok(Some((msg, 2 + len)))
}

/// Write one message to a QUIC stream.
#[cfg(not(target_arch = "wasm32"))]
pub async fn send<T: Encode>(stream: &mut quinn::SendStream, msg: &T) -> Result<(), ControlError> {
    send_any(stream, msg).await
}

/// Read one message from a QUIC stream; `Ok(None)` on a clean end of stream.
#[cfg(not(target_arch = "wasm32"))]
pub async fn recv<T: for<'a> Decode<'a>>(
    stream: &mut quinn::RecvStream,
) -> Result<Option<T>, ControlError> {
    recv_any(stream).await
}

/// The longest chat line, in characters (PROTOCOL.md 8, CLIENT.md 5).
pub const MAX_CHAT_CHARS: usize = 200;
/// A client's chat bucket: this many lines at once, one more every `CHAT_REFILL_SECS`.
pub const CHAT_BURST: u32 = 5;
pub const CHAT_REFILL_SECS: f32 = 2.0;

/// A character that cannot be seen: a control character, one that takes no room, or one
/// that changes the order of those around it. A line or a name that carries one is not
/// what it looks like.
pub fn unseen(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{061C}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{E0000}'..='\u{E007F}'
        )
}

/// Validate a chat line: 1..=`MAX_CHAT_CHARS` characters after trimming, none of them one
/// that cannot be seen (a line break in a line would draw as somebody else's line).
pub fn valid_chat(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_CHAT_CHARS {
        return None;
    }
    if trimmed.chars().any(unseen) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Validate a `Hello.name` (PROTOCOL.md 8): 1..=24 bytes of printable UTF-8 after trimming.
pub fn valid_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 24 {
        return None;
    }
    if trimmed.chars().any(unseen) {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::sim::test_content;
    use gm_core::tick::TickRate;

    /// A client and a zone of different versions must still be able to say so: the
    /// messages of the handshake keep the bytes they had in v5 (taken from that build),
    /// when both directions were one enum, and what a version adds goes at the end of
    /// its enum.
    #[test]
    fn the_handshake_s_messages_keep_their_bytes_across_versions() {
        fn hex<T: Encode>(m: &T) -> String {
            bitcode::encode(m)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }
        let unhex = |h: &str| -> Vec<u8> {
            (0..h.len() / 2)
                .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
                .collect()
        };
        let reject = FromZone::Reject("protocol version mismatch".into());
        let kick = FromZone::Kick("bye".into());
        let hello = FromClient::Hello {
            version: 5,
            name: "a".into(),
            token: vec![1, 2],
            build: None,
            team: 0,
        };
        for (now, then) in [
            (
                hex(&reject),
                "0e1970726f746f636f6c2076657273696f6e206d69736d61746368",
            ),
            (hex(&kick), "1903627965"),
            (hex(&hello), "00050001610201020000"),
        ] {
            assert_eq!(now, then);
        }
        // And read back from those bytes, as a newer build reads an older one's.
        assert_eq!(
            bitcode::decode::<FromZone>(&unhex("1903627965")).unwrap(),
            kick
        );
        assert_eq!(
            bitcode::decode::<FromClient>(&unhex("00050001610201020000")).unwrap(),
            hello
        );
        // Both enums have more than sixteen messages, as the one enum had: `bitcode`
        // then writes a message's number as a plain byte (with sixteen or fewer it packs
        // it), which is what the bytes above are.
        let last_of_the_client = FromClient::TradeAsk { with: 1 };
        assert!(bitcode::encode(&last_of_the_client)[0] > 16);
        // What the other side cannot say is not read as something else: a zone's
        // `Welcome` is no message of a client's.
        let welcome = FromZone::Welcome {
            entity: 7,
            server_tick: 1,
            hz: 64,
            map: "m".into(),
            map_hash: 1,
        };
        assert!(bitcode::decode::<FromClient>(&bitcode::encode(&welcome)).is_err());
    }

    #[test]
    fn a_stall_is_in_reach_along_the_ground_and_not_from_a_roof() {
        let stall = [272.0, -320.0, 0.0];
        let at = |dx: f32, dy: f32, dz: f32| stall_in_reach(stall, [272.0 + dx, -320.0 + dy, dz]);
        // In front of the counter, at the edge of reach, and a step past it.
        assert!(at(-72.0, 0.0, 0.0) && at(0.0, 0.0, 0.0));
        assert!(at(STALL_REACH, 0.0, 0.0) && at(0.0, -STALL_REACH, 0.0));
        assert!(!at(STALL_REACH + 0.5, 0.0, 0.0));
        assert!(
            at(84.0, 84.0, 0.0) && !at(86.0, 86.0, 0.0),
            "a circle, not a square"
        );
        // The middle of the next stall is 160 away: standing on it is not standing here.
        assert!(!at(160.0, 0.0, 0.0));
        // Above and below: a floor up is out, a step up is in.
        assert!(at(0.0, 0.0, STALL_REACH_UP) && at(0.0, 0.0, -STALL_REACH_UP));
        assert!(!at(0.0, 0.0, STALL_REACH_UP + 0.5) && !at(0.0, 0.0, -STALL_REACH_UP - 0.5));
        // Two bodies and a trade: a little further than a stall's reach, and a circle too.
        let near = |dx: f32, dy: f32, dz: f32| {
            trade_in_reach([10.0, 20.0, 0.0], [10.0 + dx, 20.0 + dy, dz])
        };
        assert!(near(TRADE_REACH, 0.0, 0.0) && !near(TRADE_REACH + 0.5, 0.0, 0.0));
        assert!(near(113.0, 113.0, 0.0) && !near(114.0, 114.0, 0.0));
        assert!(near(0.0, 0.0, -TRADE_REACH_UP) && !near(0.0, 0.0, TRADE_REACH_UP + 0.5));
        assert!(!near(f32::NAN, 0.0, 0.0));
        assert!(
            !at(f32::NAN, 0.0, 0.0),
            "a place that is no number is nowhere"
        );
    }

    #[test]
    fn framing_round_trips_and_handles_partials() {
        let pack = test_content::pack(TickRate::COMBAT);
        let hello = FromClient::Hello {
            version: 2,
            name: "pezo".into(),
            token: vec![1, 2, 3],
            build: Some(BuildChoice::Preset("blade".into())),
            team: 0,
        };
        let framed = encode_framed(&hello).unwrap();
        assert!(decode_framed::<FromClient>(&framed[..1]).unwrap().is_none());
        assert!(decode_framed::<FromClient>(&framed[..5]).unwrap().is_none());
        assert_eq!(
            decode_framed::<FromClient>(&framed).unwrap(),
            Some((hello, framed.len()))
        );
        let msgs = [
            FromZone::Welcome {
                entity: 7,
                server_tick: 1000,
                hz: 64,
                map: "test_room".into(),
                map_hash: 0xdead_beef,
            },
            FromZone::Content {
                own: pack.build("blade").unwrap().clone(),
                team: 1,
                pack: pack.clone(),
                props: vec!["sword".into()],
            },
            FromZone::Killed {
                victim: 3,
                killer: 0,
            },
            FromZone::Hit {
                target: 3,
                amount: 40,
                absorbed: 5,
            },
            FromZone::Healed {
                target: 3,
                amount: 25,
            },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            buf.extend(encode_framed(m).unwrap());
        }
        assert!(decode_framed::<FromZone>(&buf[..1]).unwrap().is_none());
        assert!(decode_framed::<FromZone>(&buf[..5]).unwrap().is_none());
        let mut pos = 0;
        let mut out = Vec::new();
        while let Some((m, n)) = decode_framed::<FromZone>(&buf[pos..]).unwrap() {
            out.push(m);
            pos += n;
        }
        assert_eq!(out, msgs);
        assert_eq!(pos, buf.len());
        // The whole content pack fits one message with room to spare.
        let content = encode_framed(&msgs[1]).unwrap();
        assert!(
            content.len() < 16 * 1024,
            "content is {} bytes",
            content.len()
        );
        println!("content pack on the wire: {} bytes", content.len());
    }

    #[test]
    fn a_full_market_fits_one_message() {
        // gm_bsp::stalls::MAX_STALL_TILES stalls, every keeper with the longest name and a
        // model.
        let market = FromZone::Stalls(
            (0..512)
                .map(|i| StallEntry {
                    id: i64::MAX - i,
                    pos: [1234.5, -2345.25, 96.0],
                    yaw: 180.0,
                    owner: "Ž".repeat(12),
                    frame: 3,
                    armour: 3,
                    model: Some([i as u8; 32]),
                })
                .collect(),
        );
        let bytes = encode_framed(&market).unwrap();
        println!("a market of 512 stalls is {} bytes", bytes.len());
        assert!(bytes.len() < MAX_MESSAGE_BYTES, "{} bytes", bytes.len());
    }

    #[test]
    fn a_roster_of_a_full_town_fits_one_message() {
        let roster = FromZone::Roster(
            (0..400)
                .map(|i| PlayerEntry {
                    id: 1000 + i,
                    name: format!("Žanamarija Škrinjarić{i:03}"),
                    team: (i % 3) as u8,
                    model: Some([i as u8; 32]),
                    kind: BodyKind::Companion { owner: i },
                    look: Look {
                        held: (i % 7) as u16,
                        worn: Look::NONE,
                    },
                })
                .collect(),
        );
        let bytes = encode_framed(&roster).unwrap();
        assert!(bytes.len() < MAX_MESSAGE_BYTES, "{} bytes", bytes.len());
        let (back, n) = decode_framed::<FromZone>(&bytes).unwrap().unwrap();
        assert_eq!(n, bytes.len());
        assert_eq!(back, roster);
    }

    #[test]
    fn oversized_messages_are_refused() {
        let msg = FromClient::Chat("x".repeat(70_000));
        assert!(matches!(encode_framed(&msg), Err(ControlError::TooLarge)));
    }

    #[test]
    fn names_are_validated() {
        assert_eq!(valid_name("  Marko "), Some("Marko".into()));
        assert_eq!(valid_name(""), None);
        assert_eq!(valid_name("   "), None);
        assert_eq!(valid_name("a\u{7}b"), None);
        assert_eq!(valid_name(&"x".repeat(25)), None);
        assert!(valid_name("Žanamarija Škrinjarić").is_some());
        // What cannot be seen is in no name and in no line: a zero-width space, a mark
        // that turns the letters around, a line break.
        assert_eq!(valid_name("Mar\u{200b}ko"), None);
        assert_eq!(valid_chat("hello"), Some("hello".into()));
        assert_eq!(valid_chat("  hello  there "), Some("hello  there".into()));
        assert_eq!(valid_chat("   "), None);
        assert_eq!(
            valid_chat(&"x".repeat(MAX_CHAT_CHARS)).map(|l| l.len()),
            Some(200)
        );
        assert_eq!(valid_chat(&"x".repeat(MAX_CHAT_CHARS + 1)), None);
        assert_eq!(
            valid_chat(&"š".repeat(MAX_CHAT_CHARS)).map(|l| l.chars().count()),
            Some(200)
        );
        for unseen in [
            "a\nZone: you win",
            "a\u{202e}b",
            "a\u{200d}b",
            "a\u{feff}",
            "x\u{2028}x",
        ] {
            assert_eq!(valid_chat(unseen), None, "{unseen:?}");
        }
    }
}
