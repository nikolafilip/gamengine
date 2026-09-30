//! gm-bot: a headless client. Runs the same prediction and interpolation code as gm-client
//! (`gm_net::client`) with a scripted behaviour instead of a human, and reports what it saw.
#![forbid(unsafe_code)]

pub mod bot;

pub use bot::{Behaviour, BotConfig, BotReport, run_bot};
