//! Hub protocol (HUB.md 3): `bitcode` messages, one request per bidirectional stream, framed
//! like PROTOCOL.md 8.

use std::net::SocketAddr;

use bitcode::{Decode, Encode};
use gm_core::build::Build;
pub use gm_net::control::BuildChoice;

pub type AccountId = i64;
pub type CharacterId = i64;
pub type ZoneId = String;

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
    },
    Registered {
        public_key: [u8; 32],
    },
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

/// Unix seconds now.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
