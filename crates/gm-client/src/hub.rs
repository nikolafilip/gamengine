//! The client's trip through the hub (HUB.md): log in, pick the character, get a ticket. Runs
//! on its own tokio runtime before the window opens; travel tickets later arrive through the
//! zone connection and are handled by the app.

use std::net::SocketAddr;

use std::sync::Arc;

use gm_hub_proto::protocol::{
    BuildChoice, HubError, HubRequest, HubResponse, MAX_MODEL_BYTES, SessionId, ZoneTicket,
};
use gm_hub_proto::{HubClient, HubClientError};
use gm_model::ModelId;

use crate::Error;
use crate::cache::{FetchError, ModelSource};

pub struct HubLogin {
    pub hub: SocketAddr,
    pub cert_der: Vec<u8>,
    pub email: String,
    pub password: String,
    pub register: bool,
    pub character: String,
    /// Preset for a character that does not exist yet.
    pub new_preset: Option<String>,
    pub zone: String,
}

pub struct HubSession {
    rt: tokio::runtime::Runtime,
    client: HubClient,
    pub session: SessionId,
    pub character: i64,
}

impl HubSession {
    /// Log in (or register), find or create the character, and get a ticket for the zone.
    pub fn enter(login: HubLogin) -> Result<(HubSession, ZoneTicket), Error> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("gm-hub")
            .build()?;
        let (client, session, character, ticket) = rt.block_on(async {
            let client = HubClient::connect_with_cert(login.hub, login.cert_der.clone()).await?;
            let mut session = None;
            if login.register {
                match client
                    .request(&HubRequest::Register {
                        email: login.email.clone(),
                        password: login.password.clone(),
                    })
                    .await
                {
                    Ok(HubResponse::Session { session: s, .. }) => session = Some(s),
                    Ok(other) => return Err(format!("unexpected answer {other:?}").into()),
                    Err(HubClientError::Refused(gm_hub_proto::protocol::HubError::Taken)) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            let session = match session {
                Some(s) => s,
                None => match client
                    .request(&HubRequest::Login {
                        email: login.email.clone(),
                        password: login.password.clone(),
                    })
                    .await?
                {
                    HubResponse::Session { session, .. } => session,
                    other => return Err(format!("unexpected answer {other:?}").into()),
                },
            };
            let characters = match client.request(&HubRequest::Characters { session }).await? {
                HubResponse::Characters(c) => c,
                other => return Err(format!("unexpected answer {other:?}").into()),
            };
            let character = match characters.iter().find(|c| c.name == login.character) {
                Some(c) => c.id,
                None => {
                    let preset = login
                        .new_preset
                        .clone()
                        .ok_or("no such character; pass --build PRESET to create it")?;
                    match client
                        .request(&HubRequest::CreateCharacter {
                            session,
                            name: login.character.clone(),
                            build: BuildChoice::Preset(preset),
                        })
                        .await?
                    {
                        HubResponse::Character(c) => c.id,
                        other => return Err(format!("unexpected answer {other:?}").into()),
                    }
                }
            };
            let ticket = match client
                .request(&HubRequest::Enter {
                    session,
                    character,
                    zone: login.zone.clone(),
                })
                .await?
            {
                HubResponse::Ticket(t) => t,
                other => return Err(format!("unexpected answer {other:?}").into()),
            };
            Ok::<_, Error>((client, session, character, ticket))
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
            Err(HubClientError::Refused(HubError::Busy)) => Err(FetchError::Failed("busy".into())),
            Err(HubClientError::Refused(e)) => Err(FetchError::Refused(e.to_string())),
            Err(e) => Err(FetchError::Failed(e.to_string())),
        }
    }
}
