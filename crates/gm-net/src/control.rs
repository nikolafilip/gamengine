//! Reliable control messages (PROTOCOL.md 8): `bitcode` payloads with a big-endian u16 length.

use bitcode::{Decode, Encode};
use gm_core::build::{Build, ContentPack};

/// How a client asks for a build: a preset by name (the zone resolves it against its content)
/// or a full allocation (validated by the zone against MATRIX.md 9).
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

/// One body as a zone announces it: a player, a companion or a creature.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct PlayerEntry {
    pub id: u32,
    pub name: String,
    pub team: u8,
    pub model: Option<[u8; 32]>,
    pub kind: BodyKind,
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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum Control {
    // client → server
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
    // server → client
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
    },
    /// The travel request failed.
    TravelRefused(String),
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
    Kick(String),
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
    /// A trial's verdict on this client (COMPANIONS.md 11): passed, or why not.
    Trial {
        key: String,
        name: String,
        passed: bool,
        detail: String,
        secs: u32,
    },
}

pub const MAX_MESSAGE_BYTES: usize = u16::MAX as usize;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("control message exceeds 65535 bytes")]
    TooLarge,
    #[error("control message did not decode: {0}")]
    Decode(#[from] bitcode::Error),
    #[error("stream read failed: {0}")]
    Read(#[from] quinn::ReadExactError),
    #[error("stream write failed: {0}")]
    Write(#[from] quinn::WriteError),
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
pub fn encode_framed(msg: &Control) -> Result<Vec<u8>, ControlError> {
    encode_framed_any(msg)
}

/// Write one message of any `bitcode` type to a QUIC stream.
pub async fn send_any<T: Encode>(
    stream: &mut quinn::SendStream,
    msg: &T,
) -> Result<(), ControlError> {
    let bytes = encode_framed_any(msg)?;
    stream.write_all(&bytes).await?;
    Ok(())
}

/// Read one message of any `bitcode` type; `Ok(None)` on a clean end of stream.
pub async fn recv_any<T: for<'a> Decode<'a>>(
    stream: &mut quinn::RecvStream,
) -> Result<Option<T>, ControlError> {
    let mut len = [0u8; 2];
    match stream.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u16::from_be_bytes(len) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    Ok(Some(bitcode::decode(&payload)?))
}

/// Decode one framed message from `buf`, returning it and the bytes consumed; `None` when the
/// buffer does not yet hold a whole message.
pub fn decode_framed(buf: &[u8]) -> Result<Option<(Control, usize)>, ControlError> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if buf.len() < 2 + len {
        return Ok(None);
    }
    let msg: Control = bitcode::decode(&buf[2..2 + len])?;
    Ok(Some((msg, 2 + len)))
}

/// Write one message to a QUIC stream.
pub async fn send(stream: &mut quinn::SendStream, msg: &Control) -> Result<(), ControlError> {
    send_any(stream, msg).await
}

/// Read one message from a QUIC stream; `Ok(None)` on a clean end of stream.
pub async fn recv(stream: &mut quinn::RecvStream) -> Result<Option<Control>, ControlError> {
    recv_any(stream).await
}

/// Validate a `Hello.name` (PROTOCOL.md 8): 1..=24 bytes of printable UTF-8 after trimming.
pub fn valid_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 24 {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::sim::test_content;
    use gm_core::tick::TickRate;

    #[test]
    fn framing_round_trips_and_handles_partials() {
        let pack = test_content::pack(TickRate::COMBAT);
        let msgs = [
            Control::Hello {
                version: 2,
                name: "pezo".into(),
                token: vec![1, 2, 3],
                build: Some(BuildChoice::Preset("blade".into())),
                team: 0,
            },
            Control::Welcome {
                entity: 7,
                server_tick: 1000,
                hz: 64,
                map: "test_room".into(),
                map_hash: 0xdead_beef,
            },
            Control::Content {
                own: pack.build("blade").unwrap().clone(),
                team: 1,
                pack: pack.clone(),
            },
            Control::Killed {
                victim: 3,
                killer: 0,
            },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            buf.extend(encode_framed(m).unwrap());
        }
        assert!(decode_framed(&buf[..1]).unwrap().is_none());
        assert!(decode_framed(&buf[..5]).unwrap().is_none());
        let mut pos = 0;
        let mut out = Vec::new();
        while let Some((m, n)) = decode_framed(&buf[pos..]).unwrap() {
            out.push(m);
            pos += n;
        }
        assert_eq!(out, msgs);
        assert_eq!(pos, buf.len());
        // The whole content pack fits one message with room to spare.
        let content = encode_framed(&msgs[2]).unwrap();
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
        let market = Control::Stalls(
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
        let roster = Control::Roster(
            (0..400)
                .map(|i| PlayerEntry {
                    id: 1000 + i,
                    name: format!("Žanamarija Škrinjarić{i:03}"),
                    team: (i % 3) as u8,
                    model: Some([i as u8; 32]),
                    kind: BodyKind::Companion { owner: i },
                })
                .collect(),
        );
        let bytes = encode_framed(&roster).unwrap();
        assert!(bytes.len() < MAX_MESSAGE_BYTES, "{} bytes", bytes.len());
        let (back, n) = decode_framed(&bytes).unwrap().unwrap();
        assert_eq!(n, bytes.len());
        assert_eq!(back, roster);
    }

    #[test]
    fn oversized_messages_are_refused() {
        let msg = Control::Chat("x".repeat(70_000));
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
    }
}
