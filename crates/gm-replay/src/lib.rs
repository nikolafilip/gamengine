//! gm-replay: what a zone records of a fight and what is computed from it (ANTICHEAT.md).
//!
//! - this module: the `.gmr` file (3.2): a header, then frames of one full-zone snapshot and
//!   the tick's events each, delta-encoded with the snapshot codec and deflated.
//! - [`aim`]: the aim statistics (4), computed from frames. The zone feeds the analyser the
//!   frames it records as it records them; `gm-tools replay aim` feeds it a file's. The
//!   numbers are the same because the code and the input are.
//! - [`play`]: bodies interpolated between two frames, for the viewer (3.4).
#![forbid(unsafe_code)]

pub mod aim;
pub mod play;

use bitcode::{Decode, Encode};
use gm_net::control::BodyKind;
use gm_net::snapshot::Snapshot;

pub const MAGIC: &[u8; 4] = b"GMR1";
/// Version of the header and frame layout.
/// 2 since 2026-10-07: the roster carries each body's mode (MODES.md).
pub const REPLAY_VERSION: u16 = 2;
/// Version of the snapshot records inside the frames (PROTOCOL.md 2 and 5). It is the
/// replay's own number and not the protocol's: a new control message bumps the protocol and
/// leaves every recorded fight readable.
pub const SNAPSHOT_CODEC: u8 = 1;
/// Every this many frames a frame is a full snapshot: a file can begin there.
pub const KEYFRAME_EVERY: u32 = 128;
/// The largest file a reader takes, and the largest it inflates to.
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_INFLATED_BYTES: usize = 512 * 1024 * 1024;
/// Frames a reader accepts: ten minutes at 64 Hz (a zone cuts its files at two).
pub const MAX_FRAMES: usize = 64 * 600;
/// Entity records over all frames a reader accepts (about 2 GiB of tables).
pub const MAX_ENTITY_RECORDS: usize = 24_000_000;
/// Entities a recorded frame holds at most: what the snapshot codec decodes.
pub const MAX_FRAME_ENTITIES: usize = 4096;

/// Why a replay was written (ANTICHEAT.md 3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Reason {
    Fight,
    Report,
}

/// One body of the roster: who it is, who drives it, whom it fights for.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct RosterEntry {
    pub id: u32,
    pub name: String,
    pub team: u8,
    pub kind: BodyKind,
    /// The party the body fights for: its own id, its commander's, 0 for a creature.
    pub party: u32,
    /// The preset's name, or "custom".
    pub build: String,
    /// The hub's character id; 0 in a zone without a hub.
    pub character: i64,
    /// The game the body plays (MODES.md 2), as `gm_core::vocab::Mode` is numbered: 0 the
    /// gun, 1 the action, 2 the RPG, whose aim is the zone's and nobody's business.
    pub mode: u8,
}

impl RosterEntry {
    /// Driven by a client: the only bodies whose aim is anybody's business.
    pub fn human(&self) -> bool {
        self.kind == BodyKind::Human
    }

    /// Its bolts are aimed by the zone at its target (MODES.md 5.3): the aim statistics
    /// of ANTICHEAT.md 4 have nothing to read in them.
    pub fn aim_is_the_zones(&self) -> bool {
        self.mode == 2
    }
}

/// How a hit landed (`gm_core::sim::HitKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Hit {
    Melee,
    Projectile,
    Area,
    Dot,
}

/// What happened in a tick besides bodies moving.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum Event {
    Hit {
        attacker: u32,
        target: u32,
        amount: i32,
        kind: Hit,
        /// What the target's guard took off the hit.
        absorbed: i32,
    },
    Killed {
        victim: u32,
        /// 0 = the world.
        killer: u32,
    },
    Parried {
        defender: u32,
        attacker: u32,
    },
    GuardBroken(u32),
    Respawned(u32),
    /// A projectile left its owner: what the aim analysis needs of it. `origin` is where
    /// it left from (the muzzle, not the eye); `lag` is how many ticks behind the present
    /// the shooter's frame said its view of the world was, `honoured` how many the zone
    /// resolved its hits against (PROTOCOL.md 7.4). The shooter aimed at the world as it
    /// was one of those ago: the analysis takes whichever its aim fits better.
    Shot {
        projectile: u32,
        owner: u32,
        origin: [f32; 3],
        speed: f32,
        gravity: f32,
        /// Ticks it flies at most.
        lifetime: u32,
        lag: u32,
        honoured: u32,
    },
    /// From this tick on a client's frames say they look at the world `lag` ticks behind
    /// the present (until its next such event; every client's is repeated at a keyframe).
    /// The view angles a frame records and this belong together: the frames a zone runs
    /// in a tick are not the frames the client drew in it, and the aim analysis judges a
    /// view against the world that view was of (ANTICHEAT.md 4.1).
    View {
        id: u32,
        lag: u8,
    },
    /// `viewer` turned onto `body` after it came into its line of sight (ANTICHEAT.md 4.2):
    /// the time between the two in the viewer's own view of the world, milliseconds. The
    /// zone measures it (it has the map); the analysis only counts it.
    Reaction {
        viewer: u32,
        body: u32,
        ms: i16,
    },
    Joined(RosterEntry),
    Left(u32),
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Header {
    pub version: u16,
    /// `SNAPSHOT_CODEC` of the recorder.
    pub codec: u8,
    /// FNV-1a of the zone's content pack as it sends it to clients: which abilities the
    /// ability indices mean.
    pub content_hash: u64,
    /// The zone has teams (an arena); without them only parties tell friend from foe.
    pub teams: bool,
    pub zone: String,
    pub map: String,
    pub map_hash: u64,
    pub hz: u16,
    pub first_tick: u32,
    /// Wall clock at the first frame, unix seconds.
    pub started_unix: u64,
    pub reason: Reason,
    /// The hub's reports this file was written for.
    pub reports: Vec<i64>,
    /// Everybody present at the first frame.
    pub roster: Vec<RosterEntry>,
}

/// One tick as recorded: the snapshot's bytes (a delta against the frame before, or a full
/// snapshot at a keyframe) and the events.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct RawFrame {
    pub tick: u32,
    pub snapshot: Vec<u8>,
    pub events: Vec<Event>,
}

/// One tick, decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub tick: u32,
    pub snapshot: Snapshot,
    pub events: Vec<Event>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("not a replay file")]
    Magic,
    #[error("replay version {0} is not supported")]
    Version(u16),
    #[error("recorded with snapshot codec {0}; this build reads {1}")]
    Codec(u8, u8),
    #[error("the file is truncated or damaged: {0}")]
    Damaged(&'static str),
    #[error("the file is too large")]
    TooLarge,
    #[error("frame {0} does not decode: {1}")]
    Frame(usize, String),
    #[error("the first frame is not a full snapshot")]
    NoKeyframe,
}

/// The bytes of a replay file. `frames[0]` must be a keyframe.
pub fn write(header: &Header, frames: &[RawFrame]) -> Vec<u8> {
    let head = bitcode::encode(header);
    let body = bitcode::encode(frames);
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&body, 6);
    let mut out = Vec::with_capacity(8 + head.len() + packed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(&packed);
    out
}

/// A replay read into memory: every frame's full entity table.
#[derive(Clone, Debug)]
pub struct Replay {
    pub header: Header,
    pub frames: Vec<Frame>,
}

/// Only the header of a file (cheap: nothing is inflated).
pub fn read_header(bytes: &[u8]) -> Result<(Header, &[u8]), ReplayError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(ReplayError::TooLarge);
    }
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err(ReplayError::Magic);
    }
    let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let rest = &bytes[8..];
    if len > rest.len() {
        return Err(ReplayError::Damaged("header"));
    }
    let header: Header =
        bitcode::decode(&rest[..len]).map_err(|_| ReplayError::Damaged("header"))?;
    if header.version != REPLAY_VERSION {
        return Err(ReplayError::Version(header.version));
    }
    if header.codec != SNAPSHOT_CODEC {
        return Err(ReplayError::Codec(header.codec, SNAPSHOT_CODEC));
    }
    Ok((header, &rest[len..]))
}

impl Replay {
    pub fn read(bytes: &[u8]) -> Result<Replay, ReplayError> {
        let (header, packed) = read_header(bytes)?;
        let body =
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, MAX_INFLATED_BYTES)
                .map_err(|_| ReplayError::Damaged("frames"))?;
        let raw: Vec<RawFrame> =
            bitcode::decode(&body).map_err(|_| ReplayError::Damaged("frames"))?;
        // A reader holds every frame's whole table: a file of millions of empty deltas
        // behind one crowded keyframe is small on disk and enormous here.
        if raw.len() > MAX_FRAMES {
            return Err(ReplayError::TooLarge);
        }
        let mut records = 0usize;
        let mut frames: Vec<Frame> = Vec::with_capacity(raw.len());
        for (i, f) in raw.into_iter().enumerate() {
            let previous = frames.last().map(|f: &Frame| &f.snapshot);
            let snapshot = Snapshot::decode(&with_datagram_header(&f.snapshot), |tick| {
                previous.filter(|p| p.server_tick == tick)
            })
            .map_err(|e| ReplayError::Frame(i, e.to_string()))?;
            if i == 0 && snapshot.baseline_tick != 0 {
                return Err(ReplayError::NoKeyframe);
            }
            records += snapshot.entities.len();
            if records > MAX_ENTITY_RECORDS {
                return Err(ReplayError::TooLarge);
            }
            frames.push(Frame {
                tick: f.tick,
                snapshot,
                events: f.events,
            });
        }
        Ok(Replay { header, frames })
    }

    /// Seconds from the first frame to the last.
    pub fn seconds(&self) -> f32 {
        match (self.frames.first(), self.frames.last()) {
            (Some(a), Some(b)) => b.tick.wrapping_sub(a.tick) as f32 / self.header.hz.max(1) as f32,
            _ => 0.0,
        }
    }

    /// The roster as it stands after frame `upto` (joins and leaves applied).
    pub fn roster_at(&self, upto: usize) -> Vec<RosterEntry> {
        let mut roster = self.header.roster.clone();
        for f in self.frames.iter().take(upto + 1) {
            apply_roster(&mut roster, &f.events);
        }
        roster
    }

    /// Everybody who was in the file at any time.
    pub fn everyone(&self) -> Vec<RosterEntry> {
        let mut all = self.header.roster.clone();
        for f in &self.frames {
            for e in &f.events {
                if let Event::Joined(entry) = e
                    && !all.iter().any(|r| r.id == entry.id)
                {
                    all.push(entry.clone());
                }
            }
        }
        all
    }
}

/// A snapshot as a frame keeps it: without the datagram header, whose protocol version
/// would make a recorded fight unreadable at the next protocol change.
pub fn strip_datagram_header(datagram: &[u8]) -> Vec<u8> {
    datagram[2.min(datagram.len())..].to_vec()
}

fn with_datagram_header(stored: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(stored.len() + 2);
    out.push(gm_net::PROTOCOL_VERSION);
    out.push(gm_net::Kind::Snapshot as u8);
    out.extend_from_slice(stored);
    out
}

/// Apply a tick's joins and leaves to a roster.
pub fn apply_roster(roster: &mut Vec<RosterEntry>, events: &[Event]) {
    for e in events {
        match e {
            Event::Joined(entry) => {
                roster.retain(|r| r.id != entry.id);
                roster.push(entry.clone());
            }
            Event::Left(id) => roster.retain(|r| r.id != *id),
            _ => {}
        }
    }
}
