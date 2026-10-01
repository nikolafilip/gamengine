//! The client's trip through the hub (HUB.md): log in, pick the character, get a ticket. Runs
//! on its own tokio runtime before the window opens; travel tickets later arrive through the
//! zone connection and are handled by the app.

use std::net::SocketAddr;

use gm_hub_proto::protocol::{BuildChoice, HubRequest, HubResponse, SessionId, ZoneTicket};
use gm_hub_proto::{HubClient, HubClientError};

use crate::Error;

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

    pub fn logout(&self) {
        let session = self.session;
        let _ = self
            .rt
            .block_on(self.client.ok(&HubRequest::Logout { session }));
        self.client.close();
    }
}
