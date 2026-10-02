//! The browser half of the model cache (WEB.md 4): the Cache API in place of a directory,
//! async fetches in place of loader threads. The cap is ours and holds at all times; the
//! hash is checked before anything parses; a model is parsed on the main thread, one a frame.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use gm_model::{Model, ModelId, id_from_hex, id_hex, model_id};
use js_sys::{Array, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{JsFuture, spawn_local};

use super::wt::js_text;
use crate::cache::{FetchError, Loaded};

/// The cache's name; a format change takes a new one and the old is deleted by hand.
const CACHE_NAME: &str = "gm-models-v1";
/// Entries live under made-up URLs: `<base><hex id>/<bytes>`. The size is in the key because
/// enumerating a cache yields requests, not responses.
const KEY_BASE: &str = "https://gm-cache.invalid/";
/// The smallest cap: a dozen models.
pub const MIN_STORE_CAP: u64 = 16 * 1024 * 1024;
/// The largest: PLAN.md 2.9's desktop cache.
pub const MAX_STORE_CAP: u64 = 2048 * 1024 * 1024;
/// Models parsed per frame (inflate and the strict reader run on the main thread).
const DECODES_PER_FRAME: usize = 1;

/// Where model bytes come from when the store does not have them.
pub trait WebSource {
    fn fetch(&self, id: ModelId) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, FetchError>>>>;
}

struct StoreEntry {
    bytes: u64,
    /// The store's clock at the last use; entries found at start begin at zero.
    used: u64,
    /// Reserved for a write still in flight: not to be evicted from under it.
    writing: bool,
}

/// What is in the browser's cache, as far as this page knows.
#[derive(Default)]
struct StoreState {
    cache: Option<web_sys::Cache>,
    /// Opening failed or a write was refused (quota, a private window): the session runs
    /// without a store.
    memory_only: bool,
    opened: bool,
    index: HashMap<ModelId, StoreEntry>,
    total: u64,
    cap: u64,
    clock: u64,
    pinned: HashSet<ModelId>,
    /// Taken down this session: never stored again.
    revoked: HashSet<ModelId>,
}

#[derive(Clone)]
pub struct Store {
    state: Rc<RefCell<StoreState>>,
}

fn key(id: &ModelId, bytes: u64) -> String {
    format!("{KEY_BASE}{}/{bytes}", id_hex(id))
}

fn parse_key(url: &str) -> Option<(ModelId, u64)> {
    let rest = url.strip_prefix(KEY_BASE)?;
    let (hex, bytes) = rest.split_once('/')?;
    Some((id_from_hex(hex)?, bytes.parse().ok()?))
}

async fn open_cache() -> Result<(web_sys::Cache, Vec<(ModelId, u64)>), String> {
    let window = web_sys::window().ok_or("no window")?;
    let storage = window.caches().map_err(|e| js_text(&e))?;
    let cache: web_sys::Cache = JsFuture::from(storage.open(CACHE_NAME))
        .await
        .map_err(|e| js_text(&e))?
        .unchecked_into();
    let keys: Array = JsFuture::from(cache.keys())
        .await
        .map_err(|e| js_text(&e))?
        .unchecked_into();
    let mut found = Vec::new();
    for k in keys.iter() {
        let request: web_sys::Request = k.unchecked_into();
        match parse_key(&request.url()) {
            Some(entry) => found.push(entry),
            // Not ours, or an older layout: it goes.
            None => {
                let _ = JsFuture::from(cache.delete_with_str(&request.url())).await;
            }
        }
    }
    Ok((cache, found))
}

impl Store {
    /// Open the cache in the background; loads wait for it.
    pub fn open(cap: u64) -> Store {
        let store = Store {
            state: Rc::new(RefCell::new(StoreState {
                cap: cap.clamp(MIN_STORE_CAP, MAX_STORE_CAP),
                ..Default::default()
            })),
        };
        let task = store.clone();
        spawn_local(async move {
            let opened = open_cache().await;
            {
                let mut st = task.state.borrow_mut();
                match opened {
                    Ok((cache, found)) => {
                        for (id, bytes) in found {
                            st.total += bytes;
                            st.index.insert(
                                id,
                                StoreEntry {
                                    bytes,
                                    used: 0,
                                    writing: false,
                                },
                            );
                        }
                        st.cache = Some(cache);
                        log::info!(
                            "model cache: {:.1} of {} MiB in the browser",
                            st.total as f64 / 1048576.0,
                            st.cap / 1048576
                        );
                    }
                    Err(e) => {
                        st.memory_only = true;
                        log::warn!(
                            "no model cache in this browser ({e}): models are kept in memory only"
                        );
                    }
                }
                st.opened = true;
            }
            // A cap smaller than what an earlier session left behind holds from the start.
            task.make_room(0).await;
        });
        store
    }

    pub fn total(&self) -> u64 {
        self.state.borrow().total
    }

    pub fn cap(&self) -> u64 {
        self.state.borrow().cap
    }

    /// Keep this model before all others (the own avatar).
    pub fn pin(&self, id: &ModelId) {
        self.state.borrow_mut().pinned.insert(*id);
    }

    async fn wait_open(&self) {
        while !self.state.borrow().opened {
            super::sleep_ms(10).await;
        }
    }

    fn cache(&self) -> Option<web_sys::Cache> {
        self.state.borrow().cache.clone()
    }

    /// The stored bytes of a model, verified; a damaged entry is deleted.
    pub async fn read(&self, id: &ModelId) -> Option<Vec<u8>> {
        self.wait_open().await;
        let bytes = self.state.borrow().index.get(id).map(|e| e.bytes)?;
        let cache = self.cache()?;
        let found = JsFuture::from(cache.match_with_str(&key(id, bytes)))
            .await
            .ok()?;
        let body = match found.dyn_into::<web_sys::Response>() {
            Ok(response) => match response.array_buffer() {
                Ok(p) => JsFuture::from(p).await.ok(),
                Err(_) => None,
            },
            // The browser evicted it behind our back.
            Err(_) => None,
        };
        let data = body.map(|b| Uint8Array::new(&b).to_vec());
        match data {
            Some(data) if model_id(&data) == *id => {
                let mut st = self.state.borrow_mut();
                st.clock += 1;
                let now = st.clock;
                if let Some(e) = st.index.get_mut(id) {
                    e.used = now;
                }
                Some(data)
            }
            _ => {
                self.remove(id).await;
                None
            }
        }
    }

    /// Delete least recently used entries until `need` more bytes fit under the cap: pinned
    /// ones last. `false` when even that is not enough.
    async fn make_room(&self, need: u64) -> bool {
        loop {
            let victim = {
                let st = self.state.borrow();
                if st.total + need <= st.cap {
                    return true;
                }
                st.index
                    .iter()
                    .filter(|(_, e)| !e.writing)
                    .map(|(id, e)| (st.pinned.contains(id), e.used, *id))
                    .min()
                    .map(|(_, _, id)| id)
            };
            match victim {
                Some(id) => self.remove(&id).await,
                None => return false,
            }
        }
    }

    /// Store a verified model. Room is made before the write, never after.
    pub async fn insert(&self, id: &ModelId, data: &[u8]) {
        self.wait_open().await;
        let bytes = data.len() as u64;
        {
            let st = self.state.borrow();
            if st.memory_only
                || st.revoked.contains(id)
                || st.index.contains_key(id)
                || bytes > st.cap
            {
                return;
            }
        }
        // The size is reserved first: two writers must not both find room for one file.
        {
            let mut st = self.state.borrow_mut();
            st.clock += 1;
            let used = st.clock;
            st.total += bytes;
            st.index.insert(
                *id,
                StoreEntry {
                    bytes,
                    used,
                    writing: true,
                },
            );
        }
        let forget = |store: &Store| {
            let mut st = store.state.borrow_mut();
            if let Some(e) = st.index.remove(id) {
                st.total -= e.bytes;
            }
        };
        if !self.make_room(0).await {
            forget(self);
            return;
        }
        let Some(cache) = self.cache() else {
            forget(self);
            return;
        };
        let written = async {
            let body = Uint8Array::from(data);
            let response = web_sys::Response::new_with_opt_buffer_source(Some(&body))
                .map_err(|e| js_text(&e))?;
            JsFuture::from(cache.put_with_str(&key(id, bytes), &response))
                .await
                .map_err(|e| js_text(&e))
        }
        .await;
        match written {
            Ok(_) => {
                // Taken down while it was being written, or the entry is gone from the
                // index for any other reason: what the write left is deleted whatever the
                // index says, or it would sit in the cache uncounted and come back next time.
                let keep = {
                    let mut st = self.state.borrow_mut();
                    let revoked = st.revoked.contains(id);
                    match st.index.get_mut(id) {
                        Some(e) if !revoked => {
                            e.writing = false;
                            true
                        }
                        _ => false,
                    }
                };
                if !keep {
                    forget(self);
                    let _ = JsFuture::from(cache.delete_with_str(&key(id, bytes))).await;
                }
            }
            Err(e) => {
                forget(self);
                let mut st = self.state.borrow_mut();
                if !st.memory_only {
                    st.memory_only = true;
                    log::warn!(
                        "the browser refused a cache write ({e}): models are kept in memory only from here on"
                    );
                }
            }
        }
    }

    pub async fn remove(&self, id: &ModelId) {
        let entry = {
            let mut st = self.state.borrow_mut();
            let e = st.index.remove(id);
            if let Some(e) = &e {
                st.total -= e.bytes;
            }
            e
        };
        if let (Some(e), Some(cache)) = (entry, self.cache()) {
            let _ = JsFuture::from(cache.delete_with_str(&key(id, e.bytes))).await;
        }
    }

    /// A takedown (MODELS.md 8): out of the store and never back in this session.
    pub fn revoke(&self, id: &ModelId) {
        self.state.borrow_mut().revoked.insert(*id);
        let (store, id) = (self.clone(), *id);
        spawn_local(async move {
            store.wait_open().await;
            store.remove(&id).await;
        });
    }
}

/// What a load task leaves for the frame: the verified bytes, not yet parsed.
enum Arrived {
    Bytes {
        id: ModelId,
        bytes: Vec<u8>,
        fetched: usize,
    },
    Refused(ModelId),
    Failed(ModelId),
}

/// Store, then source; the hash before anything else.
async fn load(id: ModelId, store: Store, source: Option<Rc<dyn WebSource>>) -> Arrived {
    if let Some(bytes) = store.read(&id).await {
        return Arrived::Bytes {
            id,
            bytes,
            fetched: 0,
        };
    }
    let Some(source) = source else {
        return Arrived::Refused(id);
    };
    match source.fetch(id).await {
        Ok(bytes) if model_id(&bytes) == id => {
            store.insert(&id, &bytes).await;
            let fetched = bytes.len();
            Arrived::Bytes { id, bytes, fetched }
        }
        Ok(_) => {
            log::warn!("model {} arrived with the wrong hash", id_hex(&id));
            Arrived::Failed(id)
        }
        Err(FetchError::Refused(why)) => {
            log::debug!("model {} refused: {why}", id_hex(&id));
            Arrived::Refused(id)
        }
        Err(FetchError::Failed(why)) => {
            log::debug!("model {} failed: {why}", id_hex(&id));
            Arrived::Failed(id)
        }
    }
}

/// The load tasks and the store they share: what the loader threads and the disk are natively.
pub struct Loader {
    pub store: Store,
    source: Option<Rc<dyn WebSource>>,
    arrived: Rc<RefCell<VecDeque<Arrived>>>,
    decodes_left: usize,
}

impl Loader {
    pub fn new(cap: u64, source: Option<Rc<dyn WebSource>>) -> Loader {
        Loader {
            store: Store::open(cap),
            source,
            arrived: Rc::new(RefCell::new(VecDeque::new())),
            decodes_left: DECODES_PER_FRAME,
        }
    }

    pub fn start(&self, id: ModelId) -> bool {
        let (store, source, arrived) = (
            self.store.clone(),
            self.source.clone(),
            self.arrived.clone(),
        );
        spawn_local(async move {
            let result = load(id, store, source).await;
            arrived.borrow_mut().push_back(result);
        });
        true
    }

    pub fn begin_frame(&mut self) {
        self.decodes_left = DECODES_PER_FRAME;
    }

    /// The next finished load. Parsing happens here, on the frame, and is rationed.
    pub fn try_recv(&mut self) -> Option<Loaded> {
        let next = {
            let mut q = self.arrived.borrow_mut();
            match q.front() {
                Some(Arrived::Bytes { .. }) if self.decodes_left == 0 => return None,
                _ => q.pop_front()?,
            }
        };
        Some(match next {
            Arrived::Refused(id) => Loaded::Refused(id),
            Arrived::Failed(id) => Loaded::Failed(id),
            Arrived::Bytes { id, bytes, fetched } => {
                self.decodes_left -= 1;
                match Model::decode(&bytes) {
                    Ok(model) => Loaded::Ready {
                        id,
                        model: Box::new(model),
                        fetched,
                    },
                    Err(e) => {
                        // The hash matched and it still does not parse: asking again will
                        // not help.
                        log::warn!("model {} does not decode: {e}", id_hex(&id));
                        self.store.revoke(&id);
                        Loaded::Refused(id)
                    }
                }
            }
        })
    }

    pub fn remove(&self, id: &ModelId) {
        self.store.revoke(id);
    }
}
