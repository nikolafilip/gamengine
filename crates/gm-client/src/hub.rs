//! The client's line to the hub (HUB.md): one request, one answer, sent behind the frame's
//! back and picked up by a later frame (CLIENT.md 1.4). What carries a request is the
//! platform's (a quinn connection natively, a `WebTransport` session in the browser, WEB.md
//! 2.1); the connection is opened by the first request that needs it and opened again by
//! the next one after it broke. Travel tickets later arrive through the zone connection
//! and are handled by the app.

use gm_hub_proto::player::{PlayerRequest, PlayerResponse};
use gm_hub_proto::protocol::{HubError, SessionId, ZoneTicket};

use crate::net::ZoneAddr;

#[cfg(target_arch = "wasm32")]
pub use crate::web::hub::Hub;
#[cfg(not(target_arch = "wasm32"))]
pub use native::Hub;

/// Why a request failed: the hub said no, or it never got an answer.
#[derive(Clone, Debug, PartialEq)]
pub enum RpcError {
    Refused(HubError),
    Other(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Refused(e) => write!(f, "{e}"),
            RpcError::Other(e) => write!(f, "{e}"),
        }
    }
}

pub type Answer = Result<PlayerResponse, RpcError>;

#[cfg(not(target_arch = "wasm32"))]
type Cell<T> = std::sync::Arc<std::sync::Mutex<Option<T>>>;
#[cfg(target_arch = "wasm32")]
type Cell<T> = std::rc::Rc<std::cell::RefCell<Option<T>>>;

/// An answer that will be there in some later frame.
pub struct Pending<T>(Cell<T>);

impl<T> Pending<T> {
    /// An empty one, and the other end that fills it.
    pub fn new() -> (Pending<T>, Filler<T>) {
        let cell: Cell<T> = Default::default();
        (Pending(cell.clone()), Filler(cell))
    }

    /// One that is there already.
    #[cfg(test)]
    pub fn ready(value: T) -> Pending<T> {
        let (pending, filler) = Pending::new();
        filler.fill(value);
        pending
    }

    /// The answer, once, when it has come.
    pub fn take(&self) -> Option<T> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.lock().unwrap().take()
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.0.borrow_mut().take()
        }
    }
}

/// The end of a `Pending` the answer is put into.
pub struct Filler<T>(Cell<T>);

impl<T> Filler<T> {
    pub fn fill(self, value: T) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            *self.0.lock().unwrap() = Some(value);
        }
        #[cfg(target_arch = "wasm32")]
        {
            *self.0.borrow_mut() = Some(value);
        }
    }
}

/// What answers the screens' requests: the hub, or in a test something that plays it.
pub trait HubApi {
    fn call(&self, req: PlayerRequest) -> Pending<Answer>;
}

impl HubApi for Hub {
    fn call(&self, req: PlayerRequest) -> Pending<Answer> {
        Hub::call(self, req)
    }
}

/// Where the ticket says the zone is, for either transport.
pub fn ticket_addr(ticket: &ZoneTicket) -> ZoneAddr {
    ZoneAddr {
        addr: Some(ticket.addr),
        cert_der: ticket.cert_der.clone(),
        web: ticket.web.clone(),
    }
}

/// The session a logged-in client holds (HUB.md 3.1), and who it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub session: SessionId,
    pub email: String,
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use gm_hub_proto::player::PlayerRequest;
    use gm_hub_proto::protocol::{HubError, MAX_MODEL_BYTES, SessionId};
    use gm_hub_proto::{HubClient, HubClientError};
    use gm_model::ModelId;

    use super::{Answer, Pending, RpcError};
    use crate::Error;
    use crate::cache::{FetchError, ModelSource};
    use crate::net::ZoneAddr;

    /// The longest the hub may take to answer a request (a login hashes a password).
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
    /// The longest one model may take to arrive (1.5 MiB at 13 KB/s); then the fetch backs off.
    const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
    /// How long the program waits for the hub to take its goodbye.
    const LOGOUT_TIMEOUT: Duration = Duration::from_secs(2);

    /// What the tasks on the runtime share. (The runtime itself is not in here: a task
    /// must never be the one to drop it.)
    struct Line {
        addr: SocketAddr,
        cert_der: Vec<u8>,
        conn: tokio::sync::Mutex<Option<HubClient>>,
        session: Mutex<Option<SessionId>>,
    }

    impl Line {
        /// The connection, opened if there is none.
        async fn client(&self) -> Result<HubClient, RpcError> {
            let mut conn = self.conn.lock().await;
            if let Some(c) = &*conn
                && c.connection().close_reason().is_none()
            {
                return Ok(c.clone());
            }
            let fresh = tokio::time::timeout(
                REQUEST_TIMEOUT,
                HubClient::connect_with_cert(self.addr, self.cert_der.clone()),
            )
            .await
            .map_err(|_| RpcError::Other("the hub does not answer".into()))?
            .map_err(|e| RpcError::Other(format!("the hub cannot be reached: {e}")))?;
            *conn = Some(fresh.clone());
            Ok(fresh)
        }

        async fn request(&self, req: &PlayerRequest) -> Answer {
            let client = self.client().await?;
            match tokio::time::timeout(REQUEST_TIMEOUT, client.player(req)).await {
                Ok(Ok(resp)) => Ok(resp),
                Ok(Err(HubClientError::Refused(e))) => Err(RpcError::Refused(e)),
                Ok(Err(e)) => {
                    // A broken line is opened again by the next request.
                    *self.conn.lock().await = None;
                    Err(RpcError::Other(format!("the hub: {e}")))
                }
                Err(_) => Err(RpcError::Other("the hub does not answer".into())),
            }
        }
    }

    /// The hub, as the client sees it: cheap to clone, never blocks a frame.
    #[derive(Clone)]
    pub struct Hub {
        rt: Arc<tokio::runtime::Runtime>,
        line: Arc<Line>,
    }

    impl Hub {
        /// Nothing is sent yet: the first request opens the connection.
        pub fn new(hub: &ZoneAddr) -> Result<Hub, Error> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("gm-hub")
                .build()?;
            Ok(Hub {
                rt: Arc::new(rt),
                line: Arc::new(Line {
                    addr: hub.addr.ok_or("the hub has no QUIC address")?,
                    cert_der: hub.cert_der.clone(),
                    conn: tokio::sync::Mutex::new(None),
                    session: Mutex::new(None),
                }),
            })
        }

        pub fn call(&self, req: PlayerRequest) -> Pending<Answer> {
            let (pending, filler) = Pending::new();
            let line = self.line.clone();
            self.rt
                .spawn(async move { filler.fill(line.request(&req).await) });
            pending
        }

        /// The session models are fetched on (MODELS.md 6.2): set at login, cleared at logout.
        pub fn set_session(&self, session: Option<SessionId>) {
            *self.line.session.lock().unwrap() = session;
        }

        pub fn model_source(&self) -> Arc<dyn ModelSource> {
            Arc::new(HubSource {
                handle: self.rt.handle().clone(),
                line: self.line.clone(),
            })
        }

        /// Say goodbye and hang up. The program is ending: it waits a moment for the hub
        /// to take it (the hub then takes the account's characters out of their zones).
        pub fn logout(&self) {
            let session = self.line.session.lock().unwrap().take();
            let line = self.line.clone();
            // All of it inside the time: a connection still being opened (to a hub that
            // does not answer) holds the line, and the program does not wait for that.
            self.rt.block_on(async move {
                let goodbye = async {
                    let Some(client) = line.conn.lock().await.take() else {
                        return;
                    };
                    if let Some(session) = session {
                        let bye = PlayerRequest::Logout { session };
                        let _ = client.player(&bye).await;
                    }
                    client.close();
                };
                let _ = tokio::time::timeout(LOGOUT_TIMEOUT, goodbye).await;
            });
        }
    }

    /// The hub as a model source: one request per model on its own stream.
    struct HubSource {
        handle: tokio::runtime::Handle,
        line: Arc<Line>,
    }

    impl ModelSource for HubSource {
        fn fetch(&self, id: &ModelId) -> Result<Vec<u8>, FetchError> {
            let Some(session) = *self.line.session.lock().unwrap() else {
                return Err(FetchError::Failed("not logged in".into()));
            };
            let req = PlayerRequest::ModelGet {
                session,
                model: *id,
            };
            // A loader thread must come back: a stalled stream on a live connection would
            // otherwise hold one of the four for good.
            let download = self.handle.block_on(async {
                let client = self.line.client().await.map_err(|e| e.to_string())?;
                tokio::time::timeout(
                    FETCH_TIMEOUT,
                    client.player_download(&req, MAX_MODEL_BYTES as usize),
                )
                .await
                .map_err(|_| "timed out".to_string())
            });
            match download {
                Ok(Ok(bytes)) => Ok(bytes),
                // Over the rate limit, or the hub is busy: later. A session that ended is
                // not the model's fault either: the next login fetches it.
                Ok(Err(HubClientError::Refused(HubError::Busy | HubError::Unauthorized))) => {
                    Err(FetchError::Failed("busy".into()))
                }
                Ok(Err(HubClientError::Refused(e))) => Err(FetchError::Refused(e.to_string())),
                Ok(Err(e)) => Err(FetchError::Failed(e.to_string())),
                Err(e) => Err(FetchError::Failed(e)),
            }
        }
    }
}
