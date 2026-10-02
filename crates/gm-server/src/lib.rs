//! gm-server: the authoritative zone server (PLAN.md 2.1, 11.1). The library holds everything
//! the binary and the turmoil tests share: world loading, per-client sessions with snapshot
//! history, the QUIC connection handling and the tick loop that drives `gm_core::sim::Zone`.
#![forbid(unsafe_code)]

pub mod hub_link;
pub mod net;
pub mod recorder;
pub mod session;
pub mod sight;
pub mod tick;
pub mod world;
pub mod zone;

pub use hub_link::{HubLink, HubLinkConfig};
pub use net::WebListener;
pub use world::ZoneWorld;
pub use zone::{GEAR_AFTER_FIGHT, ReplayConfig, ZoneConfig, ZoneReport, run, run_with_web};
