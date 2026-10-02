//! What a player's client says to the hub and hears from it (HUB.md 3.8): the handful of
//! `HubRequest`s a client makes and the answers to them, as enums of their own. They are
//! the same requests and the hub handles them as such; what is separate is the encoding. A
//! client that speaks `HubRequest` carries the codecs of everything a zone and a moderator
//! can say as well, and in a browser that is paid for by everybody who downloads the page.
//!
//! On the wire a stream that speaks these begins with an empty frame (two zero bytes: no
//! `HubRequest` is empty, so the hub tells the two apart by that) and a frame of one byte,
//! the version of these messages. The hub answers with its own version in a frame of one
//! byte and, when the two are the same, with the response. A client of another build is
//! told so in a way no change of the messages can garble.

use bitcode::{Decode, Encode};
use gm_core::build::ContentPack;

use crate::protocol::{
    BuildChoice, CharacterId, CharacterSummary, HubError, HubRequest, HubResponse, ModelId,
    SessionId, ZoneId, ZoneSummary, ZoneTicket,
};

/// The version of the players' messages; any change to them, or to a type they carry, is
/// a new one.
pub const PLAYER_VERSION: u8 = 1;

/// What a stream that speaks the players' messages begins with: the empty frame, then
/// the version in a frame of its own.
pub const PREAMBLE: [u8; 5] = [0, 0, 0, 1, PLAYER_VERSION];

/// The hub's side of it: its version, in a frame of its own, before any answer.
pub const HUB_PREAMBLE: [u8; 3] = [0, 1, PLAYER_VERSION];

/// What to tell a person whose client and hub are of different builds.
pub fn version_words(hub: u8) -> String {
    if hub > PLAYER_VERSION {
        "this client is older than the hub it talks to: it needs an update".into()
    } else {
        "this client is newer than the hub it talks to".into()
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum PlayerRequest {
    Register {
        email: String,
        password: String,
    },
    Login {
        email: String,
        password: String,
    },
    Characters {
        session: SessionId,
    },
    CreateCharacter {
        session: SessionId,
        name: String,
        build: BuildChoice,
    },
    ListZones {
        session: SessionId,
    },
    Content {
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
    /// Answered with `Blob { len }` followed by `len` raw bytes.
    ModelGet {
        session: SessionId,
        model: ModelId,
    },
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum PlayerResponse {
    Ok,
    Err(HubError),
    Session {
        session: SessionId,
        account: i64,
    },
    Characters(Vec<CharacterSummary>),
    Character(CharacterSummary),
    Zones(Vec<ZoneSummary>),
    Content {
        pack: ContentPack,
        blurbs: Vec<String>,
    },
    Ticket(ZoneTicket),
    /// `len` raw bytes follow on the stream.
    Blob {
        len: u32,
    },
}

impl From<PlayerRequest> for HubRequest {
    fn from(req: PlayerRequest) -> HubRequest {
        match req {
            PlayerRequest::Register { email, password } => HubRequest::Register { email, password },
            PlayerRequest::Login { email, password } => HubRequest::Login { email, password },
            PlayerRequest::Characters { session } => HubRequest::Characters { session },
            PlayerRequest::CreateCharacter {
                session,
                name,
                build,
            } => HubRequest::CreateCharacter {
                session,
                name,
                build,
            },
            PlayerRequest::ListZones { session } => HubRequest::ListZones { session },
            PlayerRequest::Content { session } => HubRequest::Content { session },
            PlayerRequest::Enter {
                session,
                character,
                zone,
            } => HubRequest::Enter {
                session,
                character,
                zone,
            },
            PlayerRequest::Logout { session } => HubRequest::Logout { session },
            PlayerRequest::ModelGet { session, model } => HubRequest::ModelGet { session, model },
        }
    }
}

impl TryFrom<HubResponse> for PlayerResponse {
    /// An answer no player's request has: the hub's mistake, not the client's.
    type Error = ();

    fn try_from(resp: HubResponse) -> Result<PlayerResponse, ()> {
        Ok(match resp {
            HubResponse::Ok => PlayerResponse::Ok,
            HubResponse::Err(e) => PlayerResponse::Err(e),
            HubResponse::Session { session, account } => {
                PlayerResponse::Session { session, account }
            }
            HubResponse::Characters(list) => PlayerResponse::Characters(list),
            HubResponse::Character(c) => PlayerResponse::Character(c),
            HubResponse::Zones(list) => PlayerResponse::Zones(list),
            HubResponse::Content { pack, blurbs } => PlayerResponse::Content { pack, blurbs },
            HubResponse::Ticket(t) => PlayerResponse::Ticket(t),
            HubResponse::Blob { len } => PlayerResponse::Blob { len },
            _ => return Err(()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_s_request_is_the_hub_s_request_and_no_hub_request_is_an_empty_frame() {
        let session = SessionId([3; 16]);
        let req = PlayerRequest::Enter {
            session,
            character: 9,
            zone: "town".into(),
        };
        assert_eq!(
            HubRequest::from(req),
            HubRequest::Enter {
                session,
                character: 9,
                zone: "town".into()
            }
        );
        // The marker of a player's stream is an empty frame: nothing the hub's own
        // messages can encode to.
        for req in [
            HubRequest::Logout { session },
            HubRequest::Characters { session },
            HubRequest::Register {
                email: String::new(),
                password: String::new(),
            },
        ] {
            assert!(!bitcode::encode(&req).is_empty());
        }
        assert_eq!(
            PlayerResponse::try_from(HubResponse::Blob { len: 7 }),
            Ok(PlayerResponse::Blob { len: 7 })
        );
        // The preambles are frames as the hub reads them: a length, then that many bytes.
        assert_eq!(PREAMBLE, [0, 0, 0, 1, PLAYER_VERSION]);
        assert_eq!(HUB_PREAMBLE, [0, 1, PLAYER_VERSION]);
        assert!(version_words(PLAYER_VERSION + 1).contains("older"));
        assert!(version_words(PLAYER_VERSION - 1).contains("newer"));
        // What only a zone or a moderator is ever told has no place here.
        assert_eq!(
            PlayerResponse::try_from(HubResponse::ReplayStored { id: 1 }),
            Err(())
        );
    }
}
