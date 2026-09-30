//! Reliable control messages (PROTOCOL.md 8): `bitcode` payloads with a big-endian u16 length.

use bitcode::{Decode, Encode};

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum Control {
    // client → server
    Hello {
        version: u16,
        name: String,
        token: Vec<u8>,
    },
    Chat(String),
    Bye,
    // server → client
    Welcome {
        entity: u32,
        server_tick: u32,
        hz: u16,
        map: String,
        map_hash: u64,
    },
    Reject(String),
    PlayerInfo {
        id: u32,
        name: String,
    },
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

/// Length-prefixed bytes for one message.
pub fn encode_framed(msg: &Control) -> Result<Vec<u8>, ControlError> {
    let payload = bitcode::encode(msg);
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(ControlError::TooLarge);
    }
    let mut out = Vec::with_capacity(payload.len() + 2);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
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
    let bytes = encode_framed(msg)?;
    stream.write_all(&bytes).await?;
    Ok(())
}

/// Read one message from a QUIC stream; `Ok(None)` on a clean end of stream.
pub async fn recv(stream: &mut quinn::RecvStream) -> Result<Option<Control>, ControlError> {
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

    #[test]
    fn framing_round_trips_and_handles_partials() {
        let msgs = [
            Control::Hello {
                version: 1,
                name: "pezo".into(),
                token: vec![1, 2, 3],
            },
            Control::Welcome {
                entity: 7,
                server_tick: 1000,
                hz: 64,
                map: "test_room".into(),
                map_hash: 0xdead_beef,
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
