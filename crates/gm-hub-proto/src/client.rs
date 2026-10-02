//! A hub connection for zones, bots and the client: one request per bidirectional stream,
//! hub notices on unidirectional streams (HUB.md 3).

use std::net::SocketAddr;

use gm_net::control::{self, ControlError};
use gm_net::transport::hub_client_config;
use quinn::rustls::pki_types::CertificateDer;

use crate::player::{self, PlayerRequest, PlayerResponse};
use crate::protocol::{HUB_PREAMBLE, HUB_VERSION, HubError, HubNotice, HubRequest, HubResponse};

/// Server name the hub's certificate carries.
pub const HUB_SERVER_NAME: &str = "gamengine-hub";

#[derive(Debug, thiserror::Error)]
pub enum HubClientError {
    #[error("hub connection: {0}")]
    Connect(#[from] quinn::ConnectError),
    #[error("hub connection lost: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("hub stream: {0}")]
    Stream(#[from] ControlError),
    #[error("hub refused: {0}")]
    Refused(HubError),
    #[error("hub answered with the wrong message")]
    Unexpected,
    #[error(
        "the hub speaks version {0} of its protocol and this program version {ours}: they must be of one build",
        ours = crate::protocol::HUB_VERSION
    )]
    Version(u8),
    #[error("{0}")]
    Other(String),
}

#[derive(Clone)]
pub struct HubClient {
    conn: quinn::Connection,
}

impl HubClient {
    /// Connect through an existing endpoint that trusts the hub's certificate.
    pub async fn connect(
        endpoint: &quinn::Endpoint,
        addr: SocketAddr,
    ) -> Result<HubClient, HubClientError> {
        let conn = endpoint.connect(addr, HUB_SERVER_NAME)?.await?;
        Ok(HubClient { conn })
    }

    /// A fresh client endpoint trusting exactly `cert_der`, connected to the hub.
    pub async fn connect_with_cert(
        addr: SocketAddr,
        cert_der: Vec<u8>,
    ) -> Result<HubClient, HubClientError> {
        let bind: SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse().unwrap()
        } else {
            "[::]:0".parse().unwrap()
        };
        let mut endpoint =
            quinn::Endpoint::client(bind).map_err(|e| HubClientError::Other(e.to_string()))?;
        endpoint.set_default_client_config(
            hub_client_config(&[CertificateDer::from(cert_der)])
                .map_err(|e| HubClientError::Other(e.to_string()))?,
        );
        HubClient::connect(&endpoint, addr).await
    }

    pub fn connection(&self) -> &quinn::Connection {
        &self.conn
    }

    /// A stream that speaks the hub's messages, with the version and `req` written to
    /// it: the caller writes what else it has, finishes, and reads with `hub_answer`.
    async fn hub_stream(
        &self,
        req: &HubRequest,
    ) -> Result<(quinn::SendStream, quinn::RecvStream), HubClientError> {
        let (mut send, recv) = self.conn.open_bi().await?;
        let mut bytes = HUB_PREAMBLE.to_vec();
        bytes.extend(control::encode_framed_any(req)?);
        send.write_all(&bytes)
            .await
            .map_err(|_| HubClientError::Unexpected)?;
        Ok((send, recv))
    }

    /// The hub's version, which comes first, and then its answer; `None` when the stream
    /// ended without one.
    async fn hub_answer(
        recv: &mut quinn::RecvStream,
    ) -> Result<Option<HubResponse>, HubClientError> {
        match control::recv_frame(recv).await?.as_deref() {
            Some([hub]) if *hub == HUB_VERSION => {}
            Some([hub]) => return Err(HubClientError::Version(*hub)),
            None => return Ok(None),
            _ => return Err(HubClientError::Unexpected),
        }
        Ok(control::recv_any(recv).await?)
    }

    /// One request, one response.
    pub async fn request(&self, req: &HubRequest) -> Result<HubResponse, HubClientError> {
        let (mut send, mut recv) = self.hub_stream(req).await?;
        send.finish().map_err(|_| HubClientError::Unexpected)?;
        let resp = Self::hub_answer(&mut recv)
            .await?
            .ok_or(HubClientError::Unexpected)?;
        match resp {
            HubResponse::Err(e) => Err(HubClientError::Refused(e)),
            other => Ok(other),
        }
    }

    /// A stream that speaks the players' messages (`player.rs`), with `req` sent on it
    /// and the hub's version read: what comes next on it is the answer.
    async fn player_stream(
        &self,
        req: &PlayerRequest,
    ) -> Result<quinn::RecvStream, HubClientError> {
        let (mut send, mut recv) = self.conn.open_bi().await?;
        let mut bytes = player::PREAMBLE.to_vec();
        bytes.extend(control::encode_framed_any(req)?);
        send.write_all(&bytes)
            .await
            .map_err(|_| HubClientError::Unexpected)?;
        send.finish().map_err(|_| HubClientError::Unexpected)?;
        match control::recv_frame(&mut recv).await?.as_deref() {
            Some([hub]) if *hub == player::PLAYER_VERSION => Ok(recv),
            Some([hub]) => Err(HubClientError::Other(player::version_words(*hub))),
            _ => Err(HubClientError::Unexpected),
        }
    }

    /// One request of a player's client, in the players' encoding.
    pub async fn player(&self, req: &PlayerRequest) -> Result<PlayerResponse, HubClientError> {
        let mut recv = self.player_stream(req).await?;
        let resp: PlayerResponse = control::recv_any(&mut recv)
            .await?
            .ok_or(HubClientError::Unexpected)?;
        match resp {
            PlayerResponse::Err(e) => Err(HubClientError::Refused(e)),
            other => Ok(other),
        }
    }

    /// A player's request that is answered with bytes: `Blob { len }`, then `len` raw
    /// bytes, at most `max`.
    pub async fn player_download(
        &self,
        req: &PlayerRequest,
        max: usize,
    ) -> Result<Vec<u8>, HubClientError> {
        let mut recv = self.player_stream(req).await?;
        let resp: PlayerResponse = control::recv_any(&mut recv)
            .await?
            .ok_or(HubClientError::Unexpected)?;
        let len = match resp {
            PlayerResponse::Blob { len } => len as usize,
            PlayerResponse::Err(e) => return Err(HubClientError::Refused(e)),
            _ => return Err(HubClientError::Unexpected),
        };
        if len > max {
            return Err(HubClientError::Other(format!(
                "the hub announced {len} bytes; at most {max} were expected"
            )));
        }
        let mut body = vec![0u8; len];
        recv.read_exact(&mut body)
            .await
            .map_err(|e| HubClientError::Other(format!("download interrupted: {e}")))?;
        Ok(body)
    }

    /// A request that carries bytes: the framed message, then `body` raw on the same stream
    /// (MODELS.md 6.2).
    pub async fn upload(
        &self,
        req: &HubRequest,
        body: &[u8],
    ) -> Result<HubResponse, HubClientError> {
        let (mut send, mut recv) = self.hub_stream(req).await?;
        // The hub may refuse before reading the body and stop the stream: that is its
        // answer, not our error.
        let sent = send.write_all(body).await;
        let _ = send.finish();
        let resp = Self::hub_answer(&mut recv).await?;
        match resp {
            Some(HubResponse::Err(e)) => Err(HubClientError::Refused(e)),
            Some(other) => Ok(other),
            None => Err(match sent {
                Err(e) => HubClientError::Other(format!("upload interrupted: {e}")),
                Ok(()) => HubClientError::Unexpected,
            }),
        }
    }

    /// A request answered with bytes: `Blob { len }`, then `len` raw bytes, at most `max`.
    pub async fn download(&self, req: &HubRequest, max: usize) -> Result<Vec<u8>, HubClientError> {
        let (mut send, mut recv) = self.hub_stream(req).await?;
        send.finish().map_err(|_| HubClientError::Unexpected)?;
        let resp = Self::hub_answer(&mut recv)
            .await?
            .ok_or(HubClientError::Unexpected)?;
        let len = match resp {
            HubResponse::Blob { len } => len as usize,
            HubResponse::Err(e) => return Err(HubClientError::Refused(e)),
            _ => return Err(HubClientError::Unexpected),
        };
        if len > max {
            return Err(HubClientError::Other(format!(
                "the hub announced {len} bytes; at most {max} were expected"
            )));
        }
        let mut body = vec![0u8; len];
        recv.read_exact(&mut body)
            .await
            .map_err(|e| HubClientError::Other(format!("download interrupted: {e}")))?;
        Ok(body)
    }

    /// Expect `Ok`.
    pub async fn ok(&self, req: &HubRequest) -> Result<(), HubClientError> {
        match self.request(req).await? {
            HubResponse::Ok => Ok(()),
            _ => Err(HubClientError::Unexpected),
        }
    }

    /// The next hub notice (zones only); `None` when the connection is gone. A stream
    /// that ends early, or carries what this build cannot read, is skipped: one bad
    /// notice does not make a zone deaf to the rest (a party's news and a line of chat
    /// travel this way, PARTY.md 3.2).
    pub async fn notice(&self) -> Option<HubNotice> {
        loop {
            let mut recv = self.conn.accept_uni().await.ok()?;
            if let Ok(Some(notice)) = control::recv_any(&mut recv).await {
                return Some(notice);
            }
        }
    }

    pub fn close(&self) {
        self.conn.close(0u32.into(), b"bye");
    }
}
