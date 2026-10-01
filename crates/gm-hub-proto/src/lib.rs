//! The hub protocol (`docs/HUB.md` 3): messages, entry tokens and the connection zones, bots
//! and the client use. Small on purpose: the client binary links this, never the database.
#![forbid(unsafe_code)]

pub mod client;
pub mod protocol;
pub mod token;

pub use client::{HUB_SERVER_NAME, HubClient, HubClientError};
pub use token::{HubKey, TokenError, TokenVerifier};
