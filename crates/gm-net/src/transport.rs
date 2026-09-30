//! quinn configuration (PROTOCOL.md 1): certificates, transport parameters, map hashing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use quinn::congestion::{Controller, ControllerFactory};
use quinn::{ClientConfig, ServerConfig, TransportConfig, VarInt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// TLS server name clients present; certificates carry it as a SAN.
pub const SERVER_NAME: &str = "gamengine-zone";
/// Idle timeout (PROTOCOL.md 1).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// QUIC keep-alive (PROTOCOL.md 1).
pub const KEEP_ALIVE: Duration = Duration::from_secs(2);

/// A zone's TLS identity: a self-signed certificate and its key.
#[derive(Debug)]
pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
}

impl Clone for Identity {
    fn clone(&self) -> Self {
        Identity {
            cert: self.cert.clone(),
            key: self.key.clone_key(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("certificate generation failed: {0}")]
    Rcgen(#[from] rcgen::Error),
    #[error("TLS configuration failed: {0}")]
    Tls(#[from] rustls::Error),
    #[error("root store failed: {0}")]
    Verifier(#[from] rustls::client::VerifierBuilderError),
}

impl Identity {
    /// Generate a fresh self-signed identity for `SERVER_NAME` plus any extra names.
    pub fn generate(extra_names: &[&str]) -> Result<Identity, TransportError> {
        let mut names = vec![SERVER_NAME.to_string()];
        names.extend(extra_names.iter().map(|s| s.to_string()));
        let ck = rcgen::generate_simple_self_signed(names)?;
        Ok(Identity {
            cert: ck.cert.der().clone(),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der())),
        })
    }

    pub fn from_der(cert: Vec<u8>, pkcs8_key: Vec<u8>) -> Identity {
        Identity {
            cert: CertificateDer::from(cert),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_key)),
        }
    }

    /// The certificate bytes clients need to trust this zone.
    pub fn cert_der(&self) -> &[u8] {
        self.cert.as_ref()
    }
}

/// Congestion window that never shrinks (PROTOCOL.md 1). Game traffic is a fixed, small,
/// application-limited rate; loss-based controllers would throttle 64 Hz datagrams under the
/// random loss we must survive (3% halves NewReno's window every few seconds), and queued
/// snapshots are worse than dropped ones. The budget gate keeps the rate honest instead.
#[derive(Clone, Debug)]
pub struct FixedWindow {
    pub window: u64,
}

impl FixedWindow {
    /// 64 KiB: 30 KB/s (the per-player budget) at a 2 s round trip, with room for bursts.
    pub const DEFAULT_WINDOW: u64 = 64 * 1024;
}

impl Default for FixedWindow {
    fn default() -> Self {
        FixedWindow {
            window: Self::DEFAULT_WINDOW,
        }
    }
}

impl Controller for FixedWindow {
    fn on_congestion_event(
        &mut self,
        _now: Instant,
        _sent: Instant,
        _persistent: bool,
        _lost_bytes: u64,
    ) {
    }

    fn on_mtu_update(&mut self, _new_mtu: u16) {}

    fn window(&self) -> u64 {
        self.window
    }

    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(self.clone())
    }

    fn initial_window(&self) -> u64 {
        self.window
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

impl ControllerFactory for FixedWindow {
    fn build(self: Arc<Self>, _now: Instant, _current_mtu: u16) -> Box<dyn Controller> {
        Box::new((*self).clone())
    }
}

/// Transport parameters shared by both ends: short idle timeout, keep-alives, a fixed
/// congestion window, a small datagram send buffer (a stale snapshot is worse than a lost one:
/// quinn drops the oldest queued datagram when the buffer is full), a handful of streams.
/// Snapshots and inputs never use streams, so stream flow control cannot stall them.
pub fn transport_config() -> TransportConfig {
    let mut t = TransportConfig::default();
    t.max_idle_timeout(Some(IDLE_TIMEOUT.try_into().expect("idle timeout fits")));
    t.keep_alive_interval(Some(KEEP_ALIVE));
    t.max_concurrent_bidi_streams(VarInt::from_u32(4));
    t.max_concurrent_uni_streams(VarInt::from_u32(0));
    t.datagram_receive_buffer_size(Some(256 * 1024));
    t.datagram_send_buffer_size(4 * 1024);
    t.congestion_controller_factory(Arc::new(FixedWindow::default()));
    t.initial_rtt(Duration::from_millis(100));
    t
}

pub fn server_config(identity: &Identity) -> Result<ServerConfig, TransportError> {
    let mut cfg =
        ServerConfig::with_single_cert(vec![identity.cert.clone()], identity.key.clone_key())?;
    cfg.transport_config(Arc::new(transport_config()));
    Ok(cfg)
}

/// A client configuration trusting exactly the given zone certificates.
pub fn client_config(trusted: &[CertificateDer<'static>]) -> Result<ClientConfig, TransportError> {
    let mut roots = rustls::RootCertStore::empty();
    for c in trusted {
        roots.add(c.clone())?;
    }
    let mut cfg = ClientConfig::with_root_certificates(Arc::new(roots))?;
    cfg.transport_config(Arc::new(transport_config()));
    Ok(cfg)
}

/// FNV-1a 64 of a byte string; the map hash in `Welcome` (PROTOCOL.md 8).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_configs_build() {
        let id = Identity::generate(&["localhost"]).unwrap();
        assert!(!id.cert_der().is_empty());
        server_config(&id).unwrap();
        client_config(std::slice::from_ref(&id.cert)).unwrap();
    }

    #[test]
    fn fnv_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }
}
