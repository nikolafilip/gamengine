//! gm-net: the wire protocol of gamengine, implementing `docs/PROTOCOL.md` exactly.
//!
//! - [`bits`]: MSB-first bit writer/reader with `uvar`/`svar` (section 2).
//! - [`quant`]: quantization of positions, velocities, angles and move axes (section 2).
//! - [`input`]: input datagrams (section 4).
//! - [`snapshot`]: delta-compressed snapshots (section 5).
//! - [`client`]: prediction, reconciliation and interpolation shared by client and bots (7).
//! - [`control`]: reliable `Control` messages with u16 framing (section 8).
//! - [`transport`]: quinn configuration, certificates, map hashing (section 1).
//! - [`sim`] (feature `turmoil`): quinn over turmoil's simulated UDP with loss injection.
#![forbid(unsafe_code)]

pub mod bits;
pub mod client;
pub mod control;
pub mod input;
pub mod quant;
pub mod snapshot;
pub mod transport;

#[cfg(feature = "turmoil")]
pub mod sim;

/// Protocol version byte (PROTOCOL.md header). Bumped on any wire change.
pub const PROTOCOL_VERSION: u8 = 3;

/// Largest datagram payload we ever send (PROTOCOL.md 1): well under the 1,200-byte initial
/// QUIC MTU minus framing, so nothing depends on MTU discovery.
pub const MAX_DATAGRAM_PAYLOAD: usize = 1100;

/// Datagram kinds (PROTOCOL.md 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Input = 0,
    Snapshot = 1,
    Ping = 2,
    Pong = 3,
}

impl Kind {
    pub fn from_u8(v: u8) -> Option<Kind> {
        match v {
            0 => Some(Kind::Input),
            1 => Some(Kind::Snapshot),
            2 => Some(Kind::Ping),
            3 => Some(Kind::Pong),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetError {
    #[error("read past the end of the datagram")]
    Overrun,
    #[error("malformed datagram: {0}")]
    Malformed(&'static str),
    #[error("unsupported protocol version {0}")]
    Version(u8),
    #[error("unknown datagram kind {0}")]
    Kind(u8),
    #[error("unknown snapshot baseline tick {0}")]
    UnknownBaseline(u32),
}

/// Write the two-byte datagram header (PROTOCOL.md 3).
pub fn write_header(w: &mut bits::BitWriter, kind: Kind) {
    w.write_bits(PROTOCOL_VERSION as u64, 8);
    w.write_bits(kind as u64, 8);
}

/// Read and validate the datagram header, returning the kind.
pub fn read_header(r: &mut bits::BitReader<'_>) -> Result<Kind, NetError> {
    let version = r.read_bits(8)? as u8;
    if version != PROTOCOL_VERSION {
        return Err(NetError::Version(version));
    }
    let kind = r.read_bits(8)? as u8;
    Kind::from_u8(kind).ok_or(NetError::Kind(kind))
}

/// Peek at the kind of a datagram without decoding it.
pub fn peek_kind(bytes: &[u8]) -> Result<Kind, NetError> {
    let mut r = bits::BitReader::new(bytes);
    read_header(&mut r)
}
