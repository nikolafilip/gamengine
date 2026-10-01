//! gm-hub: accounts, characters, the zone registry, entry tokens, handoff and persistence
//! (`docs/HUB.md`). Zones never touch the database; clients never talk to zones without a
//! ticket from here. The protocol, tokens and the connection live in `gm-hub-proto`.
#![forbid(unsafe_code)]

pub mod db;
pub mod hub;

pub use db::Db;
pub use gm_hub_proto::{
    HUB_SERVER_NAME, HubClient, HubClientError, HubKey, TokenError, TokenVerifier, protocol, token,
};
pub use hub::{HubConfig, run};
