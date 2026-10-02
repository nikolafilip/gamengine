//! One connection type over QUIC and WebTransport (WEB.md 2.1). A zone and the hub accept
//! native clients on a quinn endpoint and browsers on a WebTransport one; past the accept
//! nothing knows the difference. A WebTransport stream is a QUIC stream after its header,
//! which `wtransport` reads and writes, so streams of either kind are used as the quinn
//! streams they are; only datagrams and the close differ.

use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::time::Duration;

use bytes::Bytes;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct LinkError(pub String);

fn err(e: impl std::fmt::Display) -> LinkError {
    LinkError(e.to_string())
}

/// A client's connection: QUIC, or a WebTransport session.
#[derive(Clone, Debug)]
pub enum Link {
    Quic(quinn::Connection),
    #[cfg(feature = "web")]
    Web(wtransport::Connection),
}

/// The sending half of a bidirectional or unidirectional stream.
pub enum SendHalf {
    Quic(quinn::SendStream),
    #[cfg(feature = "web")]
    Web(wtransport::SendStream),
}

/// The receiving half of a stream.
pub enum RecvHalf {
    Quic(quinn::RecvStream),
    #[cfg(feature = "web")]
    Web(wtransport::RecvStream),
}

impl Deref for SendHalf {
    type Target = quinn::SendStream;
    fn deref(&self) -> &quinn::SendStream {
        match self {
            SendHalf::Quic(s) => s,
            #[cfg(feature = "web")]
            SendHalf::Web(s) => s.quic_stream(),
        }
    }
}

impl DerefMut for SendHalf {
    fn deref_mut(&mut self) -> &mut quinn::SendStream {
        match self {
            SendHalf::Quic(s) => s,
            #[cfg(feature = "web")]
            SendHalf::Web(s) => s.quic_stream_mut(),
        }
    }
}

impl Deref for RecvHalf {
    type Target = quinn::RecvStream;
    fn deref(&self) -> &quinn::RecvStream {
        match self {
            RecvHalf::Quic(s) => s,
            #[cfg(feature = "web")]
            RecvHalf::Web(s) => s.quic_stream(),
        }
    }
}

impl DerefMut for RecvHalf {
    fn deref_mut(&mut self) -> &mut quinn::RecvStream {
        match self {
            RecvHalf::Quic(s) => s,
            #[cfg(feature = "web")]
            RecvHalf::Web(s) => s.quic_stream_mut(),
        }
    }
}

/// The QUIC error code a WebTransport application error code travels as
/// (draft-ietf-webtrans-http3: codes are spread over a reserved range, skipping the
/// GREASE values). A raw QUIC code on a WebTransport stream reaches the browser as a
/// protocol error instead of the stream error it is.
#[cfg(feature = "web")]
fn web_error_code(code: u32) -> quinn::VarInt {
    const FIRST: u64 = 0x52e4_a40f_a8db;
    let n = code as u64;
    quinn::VarInt::from_u64(FIRST + n + n / 0x1e).expect("in the varint range")
}

impl RecvHalf {
    /// Tell the peer to stop sending on this stream.
    pub fn stop(&mut self, code: u32) {
        let _ = match self {
            RecvHalf::Quic(s) => s.stop(code.into()),
            #[cfg(feature = "web")]
            RecvHalf::Web(s) => s.quic_stream_mut().stop(web_error_code(code)),
        };
    }
}

impl SendHalf {
    /// Abandon the stream: what was written and not yet delivered may be lost.
    pub fn reset(&mut self, code: u32) {
        let _ = match self {
            SendHalf::Quic(s) => s.reset(code.into()),
            #[cfg(feature = "web")]
            SendHalf::Web(s) => s.quic_stream_mut().reset(web_error_code(code)),
        };
    }
}

impl From<quinn::Connection> for Link {
    fn from(c: quinn::Connection) -> Link {
        Link::Quic(c)
    }
}

impl Link {
    /// The QUIC connection underneath (a WebTransport session runs on one).
    fn quic(&self) -> &quinn::Connection {
        match self {
            Link::Quic(c) => c,
            #[cfg(feature = "web")]
            Link::Web(c) => c.quic_connection(),
        }
    }

    pub fn is_web(&self) -> bool {
        !matches!(self, Link::Quic(_))
    }

    pub fn remote_address(&self) -> SocketAddr {
        self.quic().remote_address()
    }

    pub fn stable_id(&self) -> usize {
        self.quic().stable_id()
    }

    pub fn rtt(&self) -> Duration {
        self.quic().rtt()
    }

    /// UDP bytes and datagrams as QUIC counts them (the budget gates read these).
    pub fn stats(&self) -> quinn::ConnectionStats {
        self.quic().stats()
    }

    /// The largest datagram payload the peer accepts right now, after the transport's own
    /// framing; `None` when it accepts none.
    pub fn max_datagram_size(&self) -> Option<usize> {
        match self {
            Link::Quic(c) => c.max_datagram_size(),
            #[cfg(feature = "web")]
            Link::Web(c) => c.max_datagram_size(),
        }
    }

    pub fn send_datagram(&self, bytes: Bytes) -> Result<(), LinkError> {
        match self {
            Link::Quic(c) => c.send_datagram(bytes).map_err(err),
            #[cfg(feature = "web")]
            Link::Web(c) => c.send_datagram(bytes).map_err(err),
        }
    }

    pub async fn read_datagram(&self) -> Result<Bytes, LinkError> {
        match self {
            Link::Quic(c) => c.read_datagram().await.map_err(err),
            #[cfg(feature = "web")]
            Link::Web(c) => c.receive_datagram().await.map(|d| d.payload()).map_err(err),
        }
    }

    pub async fn accept_bi(&self) -> Result<(SendHalf, RecvHalf), LinkError> {
        match self {
            Link::Quic(c) => {
                let (s, r) = c.accept_bi().await.map_err(err)?;
                Ok((SendHalf::Quic(s), RecvHalf::Quic(r)))
            }
            #[cfg(feature = "web")]
            Link::Web(c) => {
                let (s, r) = c.accept_bi().await.map_err(err)?;
                Ok((SendHalf::Web(s), RecvHalf::Web(r)))
            }
        }
    }

    pub async fn open_bi(&self) -> Result<(SendHalf, RecvHalf), LinkError> {
        match self {
            Link::Quic(c) => {
                let (s, r) = c.open_bi().await.map_err(err)?;
                Ok((SendHalf::Quic(s), RecvHalf::Quic(r)))
            }
            #[cfg(feature = "web")]
            Link::Web(c) => {
                let (s, r) = c.open_bi().await.map_err(err)?.await.map_err(err)?;
                Ok((SendHalf::Web(s), RecvHalf::Web(r)))
            }
        }
    }

    pub async fn open_uni(&self) -> Result<SendHalf, LinkError> {
        match self {
            Link::Quic(c) => Ok(SendHalf::Quic(c.open_uni().await.map_err(err)?)),
            #[cfg(feature = "web")]
            Link::Web(c) => Ok(SendHalf::Web(
                c.open_uni().await.map_err(err)?.await.map_err(err)?,
            )),
        }
    }

    pub async fn accept_uni(&self) -> Result<RecvHalf, LinkError> {
        match self {
            Link::Quic(c) => Ok(RecvHalf::Quic(c.accept_uni().await.map_err(err)?)),
            #[cfg(feature = "web")]
            Link::Web(c) => Ok(RecvHalf::Web(c.accept_uni().await.map_err(err)?)),
        }
    }

    /// Resolves when the connection is gone, whoever closed it.
    pub async fn closed(&self) {
        self.quic().closed().await;
    }

    pub fn close(&self, code: u32, reason: &[u8]) {
        match self {
            Link::Quic(c) => c.close(code.into(), reason),
            #[cfg(feature = "web")]
            Link::Web(c) => c.close(wtransport::VarInt::from_u32(code), reason),
        }
    }
}

#[cfg(feature = "web")]
pub use web::*;

#[cfg(feature = "web")]
mod web {
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::Arc;

    use wtransport::endpoint::endpoint_side::{Client, Server};
    use wtransport::tls::Sha256Digest;
    use wtransport::tls::self_signed::time::{Duration as TimeDuration, OffsetDateTime};

    use super::{Link, LinkError, err};

    /// How long a self-signed WebTransport certificate lives (WEB.md 2.2): a browser pins one
    /// by hash only if its whole validity is at most 14 days.
    pub const PINNED_CERT_DAYS: i64 = 13;

    /// A WebTransport listener's TLS identity (WEB.md 2.2).
    pub struct WebIdentity {
        identity: wtransport::Identity,
        /// SHA-256 of the leaf, for `serverCertificateHashes`; `None` for a certificate a
        /// browser verifies by its chain.
        pub cert_sha256: Option<[u8; 32]>,
    }

    impl WebIdentity {
        /// A fresh ECDSA P-256 certificate a browser accepts by its hash for
        /// `PINNED_CERT_DAYS`. It starts an hour ago, for clients whose clock runs behind.
        pub fn self_signed(names: &[&str]) -> Result<WebIdentity, LinkError> {
            let now = OffsetDateTime::now_utc();
            let identity = wtransport::Identity::self_signed_builder()
                .subject_alt_names(names)
                .validity_period(
                    now - TimeDuration::hours(1),
                    now + TimeDuration::days(PINNED_CERT_DAYS) - TimeDuration::hours(1),
                )
                .build()
                .map_err(err)?;
            let hash = *identity.certificate_chain().as_slice()[0].hash().as_ref();
            Ok(WebIdentity {
                identity,
                cert_sha256: Some(hash),
            })
        }

        /// A certificate chain and key from PEM files: one that chains to a public root.
        pub async fn from_pem(cert: &Path, key: &Path) -> Result<WebIdentity, LinkError> {
            let identity = wtransport::Identity::load_pemfiles(cert, key)
                .await
                .map_err(err)?;
            Ok(WebIdentity {
                identity,
                cert_sha256: None,
            })
        }
    }

    pub use crate::control::WebAddr;

    pub type WebEndpoint = wtransport::Endpoint<Server>;

    /// A WebTransport listener with our QUIC transport parameters.
    pub fn web_server(
        listen: SocketAddr,
        identity: &WebIdentity,
        transport: quinn::TransportConfig,
    ) -> Result<WebEndpoint, LinkError> {
        let config = wtransport::ServerConfig::builder()
            .with_bind_address(listen)
            .with_custom_transport(identity.identity.clone_identity(), transport)
            .build();
        wtransport::Endpoint::server(config).map_err(err)
    }

    /// Open a WebTransport listener (WEB.md 2.1, 2.2): with a certificate chain from PEM
    /// files, or with a fresh self-signed certificate that browsers pin by its hash. Returns
    /// the endpoint and what clients are told about it.
    pub async fn web_listener(
        listen: SocketAddr,
        pem: Option<(&Path, &Path)>,
        url: Option<String>,
        transport: quinn::TransportConfig,
    ) -> Result<(WebEndpoint, WebAddr), LinkError> {
        let identity = match pem {
            Some((cert, key)) => WebIdentity::from_pem(cert, key).await?,
            None => WebIdentity::self_signed(&["localhost", "127.0.0.1", "::1"])?,
        };
        let endpoint = web_server(listen, &identity, transport)?;
        let local = endpoint.local_addr().map_err(err)?;
        let addr = WebAddr {
            url: url.unwrap_or_else(|| format!("https://{local}")),
            cert_sha256: identity.cert_sha256,
        };
        if identity.cert_sha256.is_some() {
            tracing::warn!(
                listen = %local,
                "web listener with a self-signed certificate: browsers pin its hash, it is \
                 valid for {PINNED_CERT_DAYS} days and this process must be restarted before then"
            );
        }
        tracing::info!(listen = %local, url = %addr.url, "web listener (WebTransport)");
        Ok((endpoint, addr))
    }

    /// What a page needs to reach a web listener, as JSON:
    /// `{"url": ..., "cert_sha256": hex | null}`.
    pub fn web_info_json(addr: &WebAddr) -> String {
        let hash = match addr.cert_sha256 {
            Some(h) => format!(
                "\"{}\"",
                h.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ),
            None => "null".into(),
        };
        format!("{{\"url\": \"{}\", \"cert_sha256\": {hash}}}\n", addr.url)
    }

    /// Accept the next WebTransport session whose `Origin` is allowed (`origins` empty: any).
    /// `None` when the handshake or the request failed or was refused; the caller loops.
    pub async fn web_accept(endpoint: &WebEndpoint, origins: &[String]) -> WebIncoming {
        WebIncoming {
            incoming: endpoint.accept().await,
            origins: origins.to_vec(),
        }
    }

    /// A session that has arrived and is not yet established: finish it off the accept loop.
    pub struct WebIncoming {
        incoming: wtransport::endpoint::IncomingSession,
        origins: Vec<String>,
    }

    impl WebIncoming {
        pub fn remote_address(&self) -> SocketAddr {
            self.incoming.remote_address()
        }

        pub async fn accept(self) -> Result<Link, LinkError> {
            let request = self.incoming.await.map_err(err)?;
            // The allow-list is about browsers: a page's `Origin` must be on it. A session
            // without the header is a native program, which could have sent any.
            if let Some(origin) = request.origin()
                && !self.origins.is_empty()
            {
                let origin = normal_origin(origin);
                if !self.origins.iter().any(|o| normal_origin(o) == origin) {
                    request.forbidden().await;
                    return Err(LinkError(format!("origin {origin:?} is not allowed")));
                }
            }
            Ok(Link::Web(request.accept().await.map_err(err)?))
        }
    }

    /// An origin as browsers send it: scheme and host in lower case, no trailing slash.
    pub fn normal_origin(origin: &str) -> String {
        origin.trim().trim_end_matches('/').to_ascii_lowercase()
    }

    /// A native WebTransport client (bots and tests, WEB.md 7): trusts exactly the pinned
    /// hash, or the system's roots when there is none.
    pub async fn web_connect(
        addr: &WebAddr,
        transport: quinn::TransportConfig,
    ) -> Result<(wtransport::Endpoint<Client>, Link), LinkError> {
        let builder = wtransport::ClientConfig::builder().with_bind_default();
        let mut config = match addr.cert_sha256 {
            Some(hash) => builder
                .with_server_certificate_hashes([Sha256Digest::new(hash)])
                .build(),
            None => builder.with_native_certs().build(),
        };
        config
            .quic_config_mut()
            .transport_config(Arc::new(transport));
        let endpoint = wtransport::Endpoint::client(config).map_err(err)?;
        let conn = endpoint.connect(&addr.url).await.map_err(err)?;
        Ok((endpoint, Link::Web(conn)))
    }
}

#[cfg(all(test, feature = "web"))]
mod tests {
    use super::*;
    use crate::control::{self, FromClient, FromZone};
    use crate::transport::web_transport_config as transport_config;

    /// A WebTransport session carries what a QUIC connection carries: a control stream with
    /// framed messages both ways and datagrams both ways.
    #[tokio::test]
    async fn web_session_carries_streams_and_datagrams() {
        let identity = WebIdentity::self_signed(&["localhost", "127.0.0.1"]).unwrap();
        let server = web_server(
            "127.0.0.1:0".parse().unwrap(),
            &identity,
            transport_config(),
        )
        .unwrap();
        let port = server.local_addr().unwrap().port();
        let served = tokio::spawn(async move {
            let link = web_accept(&server, &[]).await.accept().await.unwrap();
            assert!(link.is_web());
            assert!(link.max_datagram_size().unwrap() >= crate::MAX_DATAGRAM_PAYLOAD);
            let (mut send, mut recv) = link.accept_bi().await.unwrap();
            let msg = control::recv(&mut recv).await.unwrap();
            assert_eq!(msg, Some(FromClient::Chat("hello".into())));
            control::send(&mut send, &FromZone::Reject("no".into()))
                .await
                .unwrap();
            let dg = link.read_datagram().await.unwrap();
            assert_eq!(&dg[..], b"ping");
            link.send_datagram(Bytes::from_static(b"pong")).unwrap();
            // Stay until the client has read everything and closed.
            let _ = link.read_datagram().await;
        });
        let addr = WebAddr {
            url: format!("https://127.0.0.1:{port}"),
            cert_sha256: identity.cert_sha256,
        };
        let (_endpoint, link) = web_connect(&addr, transport_config()).await.unwrap();
        let (mut send, mut recv) = link.open_bi().await.unwrap();
        control::send(&mut send, &FromClient::Chat("hello".into()))
            .await
            .unwrap();
        let reply = control::recv(&mut recv).await.unwrap();
        assert_eq!(reply, Some(FromZone::Reject("no".into())));
        link.send_datagram(Bytes::from_static(b"ping")).unwrap();
        let dg = link.read_datagram().await.unwrap();
        assert_eq!(&dg[..], b"pong");
        assert!(link.stats().udp_tx.bytes > 0);
        link.close(0, b"bye");
        served.await.unwrap();
    }

    #[tokio::test]
    async fn a_wrong_hash_is_refused() {
        let identity = WebIdentity::self_signed(&["127.0.0.1"]).unwrap();
        let server = web_server(
            "127.0.0.1:0".parse().unwrap(),
            &identity,
            transport_config(),
        )
        .unwrap();
        let port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = web_accept(&server, &[]).await.accept().await;
        });
        let addr = WebAddr {
            url: format!("https://127.0.0.1:{port}"),
            cert_sha256: Some([7; 32]),
        };
        assert!(web_connect(&addr, transport_config()).await.is_err());
    }
}
