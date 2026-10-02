//! The client's connection to a zone. Natively a small tokio runtime on its own thread drives
//! quinn; in the browser the page's event loop drives a `WebTransport` session (WEB.md 3.3).
//! Either way the frame talks to it through queues and never blocks on the network.

use gm_net::control::{FromZone, WebAddr};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
pub use crate::web::net::NetClient;
#[cfg(not(target_arch = "wasm32"))]
pub use native::NetClient;

pub enum NetEvent {
    Welcome {
        entity: u32,
        hz: u16,
        map: String,
        map_hash: u64,
    },
    Snapshot(Vec<u8>),
    Control(FromZone),
    Disconnected(String),
}

/// Where a zone is, for either transport: a ticket carries both (WEB.md 2.3).
#[derive(Clone, Debug, Default)]
pub struct ZoneAddr {
    /// The QUIC endpoint and the certificate to trust (native clients).
    pub addr: Option<std::net::SocketAddr>,
    pub cert_der: Vec<u8>,
    /// The WebTransport listener (browsers).
    pub web: Option<WebAddr>,
}
