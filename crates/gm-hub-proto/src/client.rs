//! A hub connection for zones, bots and the client: one request per bidirectional stream,
//! hub notices on unidirectional streams (HUB.md 3).

use std::net::SocketAddr;

use gm_net::control::{self, ControlError};
use gm_net::transport::hub_client_config;
use quinn::rustls::pki_types::CertificateDer;

use crate::protocol::{HubError, HubNotice, HubRequest, HubResponse};

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

    /// One request, one response.
    pub async fn request(&self, req: &HubRequest) -> Result<HubResponse, HubClientError> {
        let (mut send, mut recv) = self.conn.open_bi().await?;
        control::send_any(&mut send, req).await?;
        send.finish().map_err(|_| HubClientError::Unexpected)?;
        let resp: HubResponse = control::recv_any(&mut recv)
            .await?
            .ok_or(HubClientError::Unexpected)?;
        match resp {
            HubResponse::Err(e) => Err(HubClientError::Refused(e)),
            other => Ok(other),
        }
    }

    /// Expect `Ok`.
    pub async fn ok(&self, req: &HubRequest) -> Result<(), HubClientError> {
        match self.request(req).await? {
            HubResponse::Ok => Ok(()),
            _ => Err(HubClientError::Unexpected),
        }
    }

    /// The next hub notice (zones only); `None` when the connection is gone.
    pub async fn notice(&self) -> Option<HubNotice> {
        let mut recv = self.conn.accept_uni().await.ok()?;
        control::recv_any(&mut recv).await.ok().flatten()
    }

    pub fn close(&self) {
        self.conn.close(0u32.into(), b"bye");
    }
}
