//! gm-bot: a headless client. Runs the same prediction and interpolation code as gm-client
//! (`gm_net::client`) with a scripted behaviour instead of a human, and reports what it saw.
//! The brain is pure (`brain::Brain::think` over a view of the world), so the arena tests
//! drive it against `gm_core::sim::Zone` directly, without a network.
#![forbid(unsafe_code)]

pub mod bot;
pub mod brain;

pub use bot::{BotConfig, BotReport, run_bot};
pub use brain::{Behaviour, Brain, View};
