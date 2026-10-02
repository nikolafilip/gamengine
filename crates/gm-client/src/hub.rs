//! The client's trip through the hub (HUB.md): log in, pick the character, get a ticket. The
//! steps are the same on every platform; what carries a request is the platform's (a quinn
//! connection natively, a `WebTransport` session in the browser, WEB.md 2.1). Travel tickets
//! later arrive through the zone connection and are handled by the app.

use gm_hub_proto::protocol::{
    BuildChoice, HubError, HubRequest, HubResponse, SessionId, ZoneTicket,
};

use crate::net::ZoneAddr;

#[cfg(target_arch = "wasm32")]
pub use crate::web::hub::HubSession;
#[cfg(not(target_arch = "wasm32"))]
pub use native::HubSession;

pub struct HubLogin {
    /// Where the hub is: its QUIC address and certificate, or its web listener.
    pub hub: ZoneAddr,
    pub email: String,
    pub password: String,
    pub register: bool,
    pub character: String,
    /// Preset for a character that does not exist yet.
    pub new_preset: Option<String>,
    pub zone: String,
}

/// Why a request failed: the hub said no, or it never got an answer.
#[derive(Debug)]
pub enum RpcError {
    Refused(HubError),
    Other(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Refused(e) => write!(f, "hub refused: {e}"),
            RpcError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// One request, one response, on whatever connects this client to the hub.
pub trait Rpc {
    fn request(&self, req: &HubRequest) -> impl Future<Output = Result<HubResponse, RpcError>>;
}

/// Where the ticket says the zone is, for either transport.
pub fn ticket_addr(ticket: &ZoneTicket) -> ZoneAddr {
    ZoneAddr {
        addr: Some(ticket.addr),
        cert_der: ticket.cert_der.clone(),
        web: ticket.web.clone(),
    }
}

/// Log in (or register), find or create the character, and get a ticket for the zone.
pub async fn enter_flow<R: Rpc>(
    rpc: &R,
    login: &HubLogin,
) -> Result<(SessionId, i64, ZoneTicket), String> {
    let unexpected = |other: HubResponse| format!("unexpected answer {other:?}");
    let mut session = None;
    if login.register {
        match rpc
            .request(&HubRequest::Register {
                email: login.email.clone(),
                password: login.password.clone(),
            })
            .await
        {
            Ok(HubResponse::Session { session: s, .. }) => session = Some(s),
            Ok(other) => return Err(unexpected(other)),
            // The account exists: log in to it.
            Err(RpcError::Refused(HubError::Taken)) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let session = match session {
        Some(s) => s,
        None => match rpc
            .request(&HubRequest::Login {
                email: login.email.clone(),
                password: login.password.clone(),
            })
            .await
            .map_err(|e| e.to_string())?
        {
            HubResponse::Session { session, .. } => session,
            other => return Err(unexpected(other)),
        },
    };
    let characters = match rpc
        .request(&HubRequest::Characters { session })
        .await
        .map_err(|e| e.to_string())?
    {
        HubResponse::Characters(c) => c,
        other => return Err(unexpected(other)),
    };
    let character = match characters.iter().find(|c| c.name == login.character) {
        Some(c) => c.id,
        None => {
            let preset = login
                .new_preset
                .clone()
                .ok_or("no such character; name a build to create it")?;
            match rpc
                .request(&HubRequest::CreateCharacter {
                    session,
                    name: login.character.clone(),
                    build: BuildChoice::Preset(preset),
                })
                .await
                .map_err(|e| e.to_string())?
            {
                HubResponse::Character(c) => c.id,
                other => return Err(unexpected(other)),
            }
        }
    };
    let ticket = match rpc
        .request(&HubRequest::Enter {
            session,
            character,
            zone: login.zone.clone(),
        })
        .await
        .map_err(|e| e.to_string())?
    {
        HubResponse::Ticket(t) => t,
        other => return Err(unexpected(other)),
    };
    Ok((session, character, ticket))
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::Arc;

    use gm_hub_proto::protocol::{
        HubError, HubRequest, HubResponse, MAX_MODEL_BYTES, SessionId, ZoneTicket,
    };
    use gm_hub_proto::{HubClient, HubClientError};
    use gm_model::ModelId;

    use super::{HubLogin, Rpc, RpcError, enter_flow};
    use crate::Error;
    use crate::cache::{FetchError, ModelSource};

    pub struct HubSession {
        rt: tokio::runtime::Runtime,
        client: HubClient,
        pub session: SessionId,
        pub character: i64,
    }

    struct Quic<'a>(&'a HubClient);

    impl Rpc for Quic<'_> {
        async fn request(&self, req: &HubRequest) -> Result<HubResponse, RpcError> {
            match self.0.request(req).await {
                Ok(r) => Ok(r),
                Err(HubClientError::Refused(e)) => Err(RpcError::Refused(e)),
                Err(e) => Err(RpcError::Other(e.to_string())),
            }
        }
    }

    impl HubSession {
        /// Runs on its own tokio runtime, before the window opens.
        pub fn enter(login: HubLogin) -> Result<(HubSession, ZoneTicket), Error> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("gm-hub")
                .build()?;
            let addr = login.hub.addr.ok_or("the hub has no QUIC address")?;
            let (client, session, character, ticket) = rt.block_on(async {
                let client = HubClient::connect_with_cert(addr, login.hub.cert_der.clone())
                    .await
                    .map_err(|e| e.to_string())?;
                let (session, character, ticket) = enter_flow(&Quic(&client), &login).await?;
                Ok::<_, String>((client, session, character, ticket))
            })?;
            Ok((
                HubSession {
                    rt,
                    client,
                    session,
                    character,
                },
                ticket,
            ))
        }

        /// Models are fetched from the hub on this session (MODELS.md 6.2).
        pub fn model_source(&self) -> Arc<dyn ModelSource> {
            Arc::new(HubSource {
                handle: self.rt.handle().clone(),
                client: self.client.clone(),
                session: self.session,
            })
        }

        pub fn logout(&self) {
            let session = self.session;
            let _ = self
                .rt
                .block_on(self.client.ok(&HubRequest::Logout { session }));
            self.client.close();
        }
    }

    /// The longest one model may take to arrive (1.5 MiB at 13 KB/s); then the fetch backs off.
    const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    /// The hub as a model source: one request per model on its own stream.
    struct HubSource {
        handle: tokio::runtime::Handle,
        client: HubClient,
        session: SessionId,
    }

    impl ModelSource for HubSource {
        fn fetch(&self, id: &ModelId) -> Result<Vec<u8>, FetchError> {
            let req = HubRequest::ModelGet {
                session: self.session,
                model: *id,
            };
            // A loader thread must come back: a stalled stream on a live connection would
            // otherwise hold one of the four for good.
            let download = self.handle.block_on(async {
                tokio::time::timeout(
                    FETCH_TIMEOUT,
                    self.client.download(&req, MAX_MODEL_BYTES as usize),
                )
                .await
            });
            let Ok(result) = download else {
                return Err(FetchError::Failed("timed out".into()));
            };
            match result {
                Ok(bytes) => Ok(bytes),
                // Over the rate limit, or the hub is busy: later.
                Err(HubClientError::Refused(HubError::Busy)) => {
                    Err(FetchError::Failed("busy".into()))
                }
                Err(HubClientError::Refused(e)) => Err(FetchError::Refused(e.to_string())),
                Err(e) => Err(FetchError::Failed(e.to_string())),
            }
        }
    }
}
