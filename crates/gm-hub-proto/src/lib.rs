//! The hub protocol (`docs/HUB.md` 3): messages, entry tokens and the connection zones, bots
//! and the client use. Small on purpose: the client binary links this, never the database.
#![forbid(unsafe_code)]

//! The browser client (WEB.md 3) links only the messages: its connection is the browser's.
#[cfg(not(target_arch = "wasm32"))]
pub mod client;
pub mod names;
pub mod player;
pub mod protocol;
#[cfg(not(target_arch = "wasm32"))]
pub mod token;

#[cfg(not(target_arch = "wasm32"))]
pub use client::{HUB_SERVER_NAME, HubClient, HubClientError};
#[cfg(not(target_arch = "wasm32"))]
pub use token::{HubKey, TokenError, TokenVerifier};
