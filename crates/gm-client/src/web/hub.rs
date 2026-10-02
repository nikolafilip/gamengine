//! The hub from a browser (WEB.md 2.1): the same requests as natively, one per bidirectional
//! stream of a `WebTransport` session.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use gm_hub_proto::protocol::{
    HubError, HubRequest, HubResponse, MAX_MODEL_BYTES, SessionId, ZoneTicket,
};
use gm_model::ModelId;
use gm_net::control::encode_framed_any;
use wasm_bindgen_futures::spawn_local;

use super::store::WebSource;
use super::wt::Session;
use crate::cache::FetchError;
use crate::hub::{HubLogin, Rpc, RpcError, enter_flow};

/// The longest one model may take to arrive; then the fetch backs off.
const FETCH_TIMEOUT_MS: u32 = 120_000;
/// The longest the hub may take to answer a request (a login hashes a password).
const REQUEST_TIMEOUT_MS: u32 = 15_000;

pub struct HubSession {
    wt: Rc<Session>,
    pub session: SessionId,
    pub character: i64,
}

/// One request on its own stream; the response, and the reader for what follows it.
async fn exchange(
    wt: &Session,
    req: &HubRequest,
) -> Result<(HubResponse, super::wt::Reader), RpcError> {
    let other = RpcError::Other;
    let (writer, mut reader) = wt.open_bi().await.map_err(other)?;
    let bytes = encode_framed_any(req).map_err(|e| RpcError::Other(e.to_string()))?;
    writer.write(&bytes).await.map_err(other)?;
    writer.finish();
    let payload = reader
        .frame()
        .await
        .map_err(other)?
        .ok_or_else(|| RpcError::Other("the hub closed the stream without an answer".into()))?;
    let resp: HubResponse = bitcode::decode(&payload)
        .map_err(|e| RpcError::Other(format!("the hub's answer did not decode: {e}")))?;
    match resp {
        HubResponse::Err(e) => Err(RpcError::Refused(e)),
        resp => Ok((resp, reader)),
    }
}

struct Web<'a>(&'a Session);

impl Rpc for Web<'_> {
    async fn request(&self, req: &HubRequest) -> Result<HubResponse, RpcError> {
        match super::timeout_ms(REQUEST_TIMEOUT_MS, exchange(self.0, req)).await {
            Some(result) => result.map(|(resp, _)| resp),
            None => Err(RpcError::Other("the hub did not answer".into())),
        }
    }
}

impl HubSession {
    pub async fn enter(login: HubLogin) -> Result<(HubSession, ZoneTicket), String> {
        let web = login
            .hub
            .web
            .clone()
            .ok_or("the page is not configured with a hub (config.json)")?;
        let wt = super::timeout_ms(REQUEST_TIMEOUT_MS, Session::connect(&web))
            .await
            .ok_or("the hub did not answer")??;
        let (session, character, ticket) = match enter_flow(&Web(&wt), &login).await {
            Ok(entered) => entered,
            Err(e) => {
                wt.close();
                return Err(e);
            }
        };
        Ok((
            HubSession {
                wt: Rc::new(wt),
                session,
                character,
            },
            ticket,
        ))
    }

    /// Models are fetched from the hub on this session (MODELS.md 6.2).
    pub fn model_source(&self) -> Rc<dyn WebSource> {
        Rc::new(HubSource {
            wt: self.wt.clone(),
            session: self.session,
        })
    }

    /// Log out and hang up, behind the frame's back: nothing in a page may block.
    pub fn logout(&self) {
        let (wt, session) = (self.wt.clone(), self.session);
        spawn_local(async move {
            let _ = super::timeout_ms(2_000, exchange(&wt, &HubRequest::Logout { session })).await;
            wt.close();
        });
    }
}

struct HubSource {
    wt: Rc<Session>,
    session: SessionId,
}

async fn download(wt: &Session, req: &HubRequest, max: usize) -> Result<Vec<u8>, FetchError> {
    let (resp, mut reader) = match exchange(wt, req).await {
        Ok(r) => r,
        // Over the rate limit, or the hub is busy: later.
        Err(RpcError::Refused(HubError::Busy)) => return Err(FetchError::Failed("busy".into())),
        Err(RpcError::Refused(e)) => return Err(FetchError::Refused(e.to_string())),
        Err(RpcError::Other(e)) => return Err(FetchError::Failed(e)),
    };
    let HubResponse::Blob { len } = resp else {
        return Err(FetchError::Failed(
            "the hub answered with the wrong message".into(),
        ));
    };
    let len = len as usize;
    if len > max {
        return Err(FetchError::Failed(format!(
            "the hub announced {len} bytes; at most {max} were expected"
        )));
    }
    match reader.read_exact(len).await {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) if len == 0 => Ok(Vec::new()),
        Ok(None) => Err(FetchError::Failed("download interrupted".into())),
        Err(e) => Err(FetchError::Failed(format!("download interrupted: {e}"))),
    }
}

impl WebSource for HubSource {
    fn fetch(&self, id: ModelId) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, FetchError>>>> {
        let (wt, session) = (self.wt.clone(), self.session);
        Box::pin(async move {
            let req = HubRequest::ModelGet { session, model: id };
            // A load slot must come back: a stalled stream would otherwise hold one of the
            // four for good.
            match super::timeout_ms(
                FETCH_TIMEOUT_MS,
                download(&wt, &req, MAX_MODEL_BYTES as usize),
            )
            .await
            {
                Some(result) => result,
                None => Err(FetchError::Failed("timed out".into())),
            }
        })
    }
}
