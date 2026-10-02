//! Hub protocol (HUB.md 3): `bitcode` messages, one request per bidirectional stream, framed
//! like PROTOCOL.md 8.

use std::net::SocketAddr;

use bitcode::{Decode, Encode};
use gm_core::build::Build;
pub use gm_net::control::BuildChoice;

pub type AccountId = i64;
pub type CharacterId = i64;
pub type ZoneId = String;
/// SHA-256 of an ingested model file (MODELS.md 5).
pub type ModelId = [u8; 32];

/// Sixteen random bytes; lives in hub memory for 24 h or until `Logout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Encode, Decode)]
pub struct SessionId(pub [u8; 16]);

/// What outlives a zone (HUB.md 3.2).
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct CharacterState {
    pub build: Build,
    /// Where the position belongs; `None` = spawn at the next zone.
    pub zone: Option<ZoneId>,
    pub position: [f32; 3],
    pub yaw: f32,
    pub viewport: u8,
    pub play_seconds: u32,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct CharacterSummary {
    pub id: CharacterId,
    pub name: String,
    pub build: Build,
    pub location: LocationSummary,
    pub play_seconds: u32,
    /// The model the character wears (MODELS.md 6), whatever its status.
    pub model: Option<ModelId>,
}

/// What a zone is told about a character's avatar: the id clients fetch by and the frame the
/// model was ingested for (MODELS.md 6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct ModelRef {
    pub id: ModelId,
    pub frame: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ModelStatus {
    Pending,
    Active,
    Rejected,
    Takedown,
}

impl ModelStatus {
    pub const fn name(self) -> &'static str {
        match self {
            ModelStatus::Pending => "pending",
            ModelStatus::Active => "active",
            ModelStatus::Rejected => "rejected",
            ModelStatus::Takedown => "takedown",
        }
    }

    pub fn from_name(s: &str) -> Option<ModelStatus> {
        [
            ModelStatus::Pending,
            ModelStatus::Active,
            ModelStatus::Rejected,
            ModelStatus::Takedown,
        ]
        .into_iter()
        .find(|m| m.name() == s)
    }
}

/// Why a model was refused or removed (MODELS.md 10). Every code but `Other` is a strike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ReasonCode {
    None,
    Copyright,
    Likeness,
    Sexual,
    Hateful,
    Other,
}

impl ReasonCode {
    pub const fn name(self) -> &'static str {
        match self {
            ReasonCode::None => "",
            ReasonCode::Copyright => "copyright",
            ReasonCode::Likeness => "likeness",
            ReasonCode::Sexual => "sexual",
            ReasonCode::Hateful => "hateful",
            ReasonCode::Other => "other",
        }
    }

    pub fn from_name(s: &str) -> Option<ReasonCode> {
        [
            ReasonCode::None,
            ReasonCode::Copyright,
            ReasonCode::Likeness,
            ReasonCode::Sexual,
            ReasonCode::Hateful,
            ReasonCode::Other,
        ]
        .into_iter()
        .find(|c| c.name() == s)
    }

    pub const fn is_strike(self) -> bool {
        !matches!(self, ReasonCode::None | ReasonCode::Other)
    }
}

/// A model as its holder (or a moderator) sees it.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct ModelSummary {
    pub id: ModelId,
    pub frame: u8,
    pub status: ModelStatus,
    pub bytes: u32,
    pub triangles: u32,
    pub texture: [u16; 2],
    /// The statement of reasons for a rejection or a takedown.
    pub code: ReasonCode,
    pub reason: String,
}

/// One entry of the moderation queue.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct ModEntry {
    pub model: ModelSummary,
    pub uploader: String,
    pub holders: u32,
    pub waiting_secs: u64,
}

/// What a moderator may ask (MODELS.md 10).
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum ModOp {
    /// Pending models, oldest first.
    Queue {
        limit: u32,
    },
    /// The preview image of a model (answered with a blob).
    Preview {
        model: ModelId,
    },
    /// `pending → active | rejected`.
    Decide {
        model: ModelId,
        approve: bool,
        code: ReasonCode,
        reason: String,
    },
    /// `active → takedown`; `reference` names the notice.
    Takedown {
        model: ModelId,
        code: ReasonCode,
        reason: String,
        reference: String,
    },
    /// `takedown → active` (a counter-notice).
    Reinstate {
        model: ModelId,
        reason: String,
    },
    SetUpload {
        email: String,
        allow: bool,
    },
    SetTrust {
        email: String,
        tier: u8,
    },
    ClearStrikes {
        email: String,
    },
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum LocationSummary {
    Offline,
    Zone(ZoneId),
    Transit { from: ZoneId, to: ZoneId },
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct ZoneSummary {
    pub id: ZoneId,
    pub map: String,
    pub players: u32,
    pub addr: SocketAddr,
    /// FNV-1a 64 of the zone's certificate DER.
    pub cert_hash: u64,
    pub up_secs: u64,
}

/// Entry to a zone (HUB.md 3.1).
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct TokenPayload {
    pub account: AccountId,
    pub character: CharacterId,
    pub zone: ZoneId,
    /// Unix seconds.
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: [u8; 16],
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct SessionToken {
    pub payload: TokenPayload,
    /// ed25519 over the bitcode-encoded payload.
    pub signature: [u8; 64],
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct ZoneTicket {
    pub zone: ZoneId,
    pub addr: SocketAddr,
    pub cert_der: Vec<u8>,
    pub token: SessionToken,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum HubRequest {
    // anyone
    Register {
        email: String,
        password: String,
    },
    Login {
        email: String,
        password: String,
    },
    // accounts
    Characters {
        session: SessionId,
    },
    CreateCharacter {
        session: SessionId,
        name: String,
        /// A preset of the hub's content, or a full build (validated either way).
        build: BuildChoice,
    },
    SetBuild {
        session: SessionId,
        character: CharacterId,
        build: BuildChoice,
    },
    ListZones {
        session: SessionId,
    },
    Enter {
        session: SessionId,
        character: CharacterId,
        zone: ZoneId,
    },
    Logout {
        session: SessionId,
    },
    // zones (the secret authenticates the connection once; later requests ride on it)
    ZoneHello {
        secret: String,
        zone: ZoneId,
        map: String,
        map_hash: u64,
        addr: SocketAddr,
        cert_der: Vec<u8>,
    },
    Heartbeat {
        players: u32,
        tick_mean_us: f32,
    },
    Claim {
        token: SessionToken,
    },
    Save {
        character: CharacterId,
        state: CharacterState,
        leaving: bool,
    },
    Handoff {
        character: CharacterId,
        state: CharacterState,
        to_zone: ZoneId,
    },
    // a session, about one of its characters (ECONOMY.md)
    Econ {
        session: SessionId,
        character: CharacterId,
        op: EconOp,
    },
    // a registered zone connection (ECONOMY.md 8, 9)
    ZoneEcon(ZoneEconOp),
    // models (MODELS.md 6.2). `ModelUpload` is followed on the same stream by `len` raw bytes.
    ModelUpload {
        session: SessionId,
        /// Archetype frame index the model is for.
        frame: u8,
        /// The upload terms the account certifies (MODELS.md 10).
        tos_version: u16,
        len: u32,
    },
    ModelList {
        session: SessionId,
    },
    /// Give up holding a model; the account's characters stop wearing it.
    ModelDrop {
        session: SessionId,
        model: ModelId,
    },
    /// What an offline character wears.
    SetModel {
        session: SessionId,
        character: CharacterId,
        model: Option<ModelId>,
    },
    /// Answered with `Blob { len }` followed by `len` raw bytes.
    ModelGet {
        session: SessionId,
        model: ModelId,
    },
    Mod {
        session: SessionId,
        op: ModOp,
    },
}

pub type ItemId = i64;

/// What a character may ask of the economy (ECONOMY.md). Every op is one transaction.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum EconOp {
    Inventory,
    Storage,
    StorageDeposit {
        item: ItemId,
    },
    StorageWithdraw {
        item: ItemId,
    },
    Craft {
        template: String,
        components: Vec<ItemId>,
    },
    Decompose {
        item: ItemId,
    },
    TradeOpen {
        with: CharacterId,
    },
    TradeOfferItem {
        trade: i64,
        item: ItemId,
    },
    TradeRetractItem {
        trade: i64,
        item: ItemId,
    },
    TradeSetCoin {
        trade: i64,
        coin: i64,
    },
    /// `version` is the offer version the client is showing.
    TradeAccept {
        trade: i64,
        version: i32,
    },
    TradeCancel {
        trade: i64,
    },
    /// The offers as the hub holds them, with the version an accept must name.
    TradeView {
        trade: i64,
    },
    StallList {
        item: ItemId,
        price: i64,
    },
    /// `price` is the price the buyer was shown.
    StallBuy {
        listing: i64,
        price: i64,
    },
    StallClose,
    BuyOrderPost {
        material: String,
        price: i64,
        quantity: i32,
    },
    BuyOrderFill {
        order: i64,
        item: ItemId,
    },
    BuyOrderCancel {
        order: i64,
    },
    ContractPost {
        instance: String,
        price: i64,
        collateral: i64,
    },
    ContractCancel {
        contract: i64,
    },
    /// By the party leader; `sellers` includes the leader.
    ContractAccept {
        contract: i64,
        sellers: Vec<CharacterId>,
    },
    ChestDeposit {
        chest: i64,
        item: ItemId,
    },
    ChestWithdraw {
        chest: i64,
        item: ItemId,
    },
    HireList {
        price: i64,
    },
    Hire {
        avatar: CharacterId,
    },
    Tavern,
}

/// What a zone reports (ECONOMY.md 8, 9): only a registered zone connection may send these.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum ZoneEconOp {
    /// The boss split's result: one component per entry.
    GrantComponents {
        grants: Vec<(CharacterId, String)>,
        reference: i64,
    },
    GrantCoin {
        character: CharacterId,
        amount: i64,
        reference: i64,
    },
    ContractReport {
        contract: i64,
        outcome: ContractOutcome,
    },
    /// A character drops an item onto this zone's ground, or picks one up from it. The zone
    /// asks, because only the zone knows where the character stands.
    Drop {
        character: CharacterId,
        item: ItemId,
    },
    Pickup {
        character: CharacterId,
        item: ItemId,
    },
    /// A character opens a stall on a tile of this zone's market. The zone asks, because only
    /// the zone knows that the character stands on that tile and that the tile exists
    /// (ECONOMY.md 7).
    StallOpen {
        character: CharacterId,
        tile_x: i32,
        tile_y: i32,
    },
    /// The owner, standing in this zone, closes the stall.
    StallClose { character: CharacterId },
    /// Every open stall of this zone (asked once, when the zone starts).
    Stalls,
}

/// An open stall as its zone shows it: where it is and who keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct StallSummary {
    pub id: i64,
    pub tile_x: i32,
    pub tile_y: i32,
    pub owner: CharacterId,
    pub owner_name: String,
    /// The keeper's frame and armour class, and its avatar model while that is active.
    pub frame: u8,
    pub armour: u8,
    pub model: Option<ModelRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ContractOutcome {
    Completed,
    Wipe,
    Abandon,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct ItemSummary {
    pub id: ItemId,
    pub template: String,
    /// `(layer, material)` in layer order.
    pub components: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct TradeOffer {
    pub coin: i64,
    pub accepted: bool,
    pub items: Vec<ItemSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum EconReply {
    Done,
    Id(i64),
    Ids(Vec<i64>),
    Holder {
        coin: i64,
        items: Vec<ItemSummary>,
    },
    TradeView {
        version: i32,
        mine: TradeOffer,
        theirs: TradeOffer,
    },
    /// `committed` is false while the other side has yet to accept.
    Trade {
        committed: bool,
    },
    /// The report decided the contract (false: it was already decided).
    Decided(bool),
    /// `(character, price, hires in the last 12 h)`.
    Tavern(Vec<(CharacterId, i64, i64)>),
    Stall(StallSummary),
    Stalls(Vec<StallSummary>),
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum HubError {
    Credentials,
    Taken,
    NotFound,
    Busy,
    Unauthorized,
    Invalid(String),
    Internal,
    /// Not enough coin.
    Insufficient,
    /// No room in the inventory, the storage or the stall.
    Full,
    /// Too soon after the last change of a trade.
    Cooldown,
    /// The content was removed and is not served (MODELS.md 6.2).
    Gone,
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HubError::Credentials => write!(f, "wrong email or password"),
            HubError::Taken => write!(f, "already taken"),
            HubError::NotFound => write!(f, "not found"),
            HubError::Busy => write!(f, "busy, try again"),
            HubError::Unauthorized => write!(f, "unauthorized"),
            HubError::Invalid(s) => write!(f, "invalid: {s}"),
            HubError::Internal => write!(f, "internal error"),
            HubError::Insufficient => write!(f, "not enough coin"),
            HubError::Full => write!(f, "no room"),
            HubError::Cooldown => write!(f, "too soon after the last change"),
            HubError::Gone => write!(f, "removed"),
        }
    }
}

impl std::error::Error for HubError {}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum HubResponse {
    Ok,
    Err(HubError),
    Session {
        session: SessionId,
        account: AccountId,
    },
    Characters(Vec<CharacterSummary>),
    Character(CharacterSummary),
    Zones(Vec<ZoneSummary>),
    Ticket(ZoneTicket),
    Claimed {
        character: CharacterId,
        name: String,
        state: CharacterState,
        team: u8,
        /// The avatar model, present only while it is active (MODELS.md 6.3).
        model: Option<ModelRef>,
    },
    Registered {
        public_key: [u8; 32],
    },
    Econ(EconReply),
    Models(Vec<ModelSummary>),
    /// The upload was ingested (or already known): its id and where it stands.
    ModelAccepted {
        model: ModelId,
        status: ModelStatus,
    },
    /// `len` raw bytes follow on the stream.
    Blob {
        len: u32,
    },
    ModQueue(Vec<ModEntry>),
}

/// Hub → zone, on unidirectional streams (HUB.md 3.4).
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum HubNotice {
    Claimed {
        character: CharacterId,
    },
    Kick {
        character: CharacterId,
        reason: String,
    },
    /// A model left `active`: nobody wears it any more (MODELS.md 6.3).
    ModelRevoked {
        model: ModelId,
    },
    /// A stall of this zone closed: its owner closed it, or its 48 h ran out (ECONOMY.md 7).
    StallClosed {
        stall: i64,
    },
}

/// Limits (HUB.md 3).
pub const MAX_CHARACTERS_PER_ACCOUNT: i64 = 10;
pub const TOKEN_VALID_SECS: u64 = 60;
pub const CLOCK_SKEW_SECS: u64 = 5;
pub const TRANSIT_ABANDON_SECS: u64 = 15;
pub const SESSION_SECS: u64 = 24 * 3600;
pub const HUB_BIDI_STREAMS: u32 = 1024;
/// Password hashes running at once; more answer `Busy`.
pub const HASH_PERMITS: usize = 8;
/// Models (MODELS.md 3, 5, 6.2).
pub const MAX_MODEL_UPLOAD_BYTES: u32 = 8 * 1024 * 1024;
pub const MAX_MODEL_BYTES: u32 = 1_572_864;
/// The largest blob a hub answers with (a model or its preview).
pub const MAX_BLOB_BYTES: u32 = MAX_MODEL_BYTES;
/// The upload terms an account must certify (MODELS.md 10).
pub const TOS_VERSION: u16 = 1;
pub const DEFAULT_MODEL_SLOTS: i16 = 4;
pub const MAX_PENDING_MODELS: i64 = 3;
pub const UPLOAD_STRIKES: i16 = 3;
/// Accounts at this trust tier or above skip the moderation queue.
pub const TRUSTED_TIER: i16 = 2;
/// Ingestion workers running at once; more answer `Busy`.
pub const INGEST_PERMITS: usize = 2;

/// Unix seconds now.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
