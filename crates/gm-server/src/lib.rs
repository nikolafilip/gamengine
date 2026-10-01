//! gm-server: the authoritative zone server (PLAN.md 2.1, 11.1). The library holds everything
//! the binary and the turmoil tests share: world loading, per-client sessions with snapshot
//! history, the QUIC connection handling and the tick loop that drives `gm_core::sim::Zone`.
#![forbid(unsafe_code)]

pub mod hub_link;
pub mod net;
pub mod session;
pub mod tick;
pub mod world;
pub mod zone;

pub use hub_link::{HubLink, HubLinkConfig};
pub use world::ZoneWorld;
pub use zone::{ZoneConfig, ZoneReport, run};
