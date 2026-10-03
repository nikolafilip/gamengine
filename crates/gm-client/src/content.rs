//! The content bundle as the client has it (CONTENT.md 6): the manifest and the atlas read
//! at start from `<assets>/built/content/`, props loaded by key when first needed, from the
//! bundle and never from the hub. Everything is looked up by key; what the bundle lacks is
//! drawn as nothing (principle 4: never wait for art), and a bundle that is missing or
//! broken leaves the client drawing exactly as it did before Phase 14.

use std::collections::HashMap;

use gm_model::Model;
use gm_model::atlas::Atlas;
use gm_model::manifest::Manifest;

/// Where the bundle's files are, under the assets: `<assets>/built/content`.
pub const DIR: &str = "built/content";

/// A prop as the client has it.
#[derive(Debug)]
pub enum PropState {
    /// Asked for and not yet here (a fetch in flight in the browser).
    #[allow(dead_code)]
    Loading,
    /// On the GPU, in this slot of the character renderer.
    Loaded(usize),
    /// The bundle has no such file, or the file is not a prop: said once, never asked again.
    Missing,
}

pub struct Content {
    pub manifest: Option<Manifest>,
    /// The atlas, until the HUD takes it (`take_atlas`).
    atlas: Option<Atlas>,
    pub props: HashMap<String, PropState>,
    /// Where props are read from: a directory natively, a URL prefix in the browser.
    base: String,
    /// Files that arrived (the browser's fetches land here; natively a read is immediate).
    #[cfg(target_arch = "wasm32")]
    inbox: std::rc::Rc<std::cell::RefCell<Vec<(String, Option<Vec<u8>>)>>>,
    /// What was said about the bundle, for the report.
    pub note: String,
}

impl Content {
    /// A client without a bundle.
    pub fn none(why: &str) -> Content {
        Content {
            manifest: None,
            atlas: None,
            props: HashMap::new(),
            base: String::new(),
            #[cfg(target_arch = "wasm32")]
            inbox: Default::default(),
            note: why.to_string(),
        }
    }

    /// From the bundle's two files (read by whoever has them) and where its props are.
    pub fn from_bytes(base: String, manifest: Option<Vec<u8>>, atlas: Option<Vec<u8>>) -> Content {
        let mut c = Content::none("");
        c.base = base;
        let mut notes = Vec::new();
        match manifest.as_deref().map(Manifest::decode) {
            Some(Ok(m)) => {
                notes.push(format!(
                    "content version {} ({} templates, {} props, {} abilities)",
                    m.content_version,
                    m.templates.len(),
                    m.props.len(),
                    m.abilities.len()
                ));
                c.manifest = Some(m);
            }
            Some(Err(e)) => {
                log::error!("the content manifest: {e}");
                notes.push(format!("manifest refused: {e}"));
            }
            None => notes.push("no content bundle".into()),
        }
        match atlas.as_deref().map(Atlas::decode) {
            Some(Ok(a)) => {
                if let Some(m) = &c.manifest
                    && gm_model::model_id(atlas.as_deref().unwrap_or(&[])) != m.atlas_sha256
                {
                    log::warn!("the atlas is not the one the manifest names");
                }
                notes.push(format!(
                    "atlas {}x{} ({} pieces, {} faces, {} icons)",
                    a.w,
                    a.h,
                    a.pieces.len(),
                    a.faces.len(),
                    a.icons.len()
                ));
                c.atlas = Some(a);
            }
            Some(Err(e)) => {
                log::error!("the content atlas: {e}");
                notes.push(format!("atlas refused: {e}"));
            }
            None => {}
        }
        c.note = notes.join("; ");
        c
    }

    /// Read the bundle from `<assets>/built/content` on the desktop.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(assets: &std::path::Path) -> Content {
        let dir = assets.join(DIR);
        let read = |name: &str| std::fs::read(dir.join(name)).ok();
        let c = Content::from_bytes(
            dir.to_string_lossy().into_owned(),
            read("manifest.gmc"),
            read("ui.gma"),
        );
        log::info!("content: {}", c.note);
        c
    }

    /// Fetch the bundle from `<assets>/built/content` on the page; a site without one
    /// (404) is a client without a bundle, not an error.
    #[cfg(target_arch = "wasm32")]
    pub async fn fetch(assets: &str) -> Content {
        let base = format!("{assets}/{DIR}");
        let get = |name: &str| {
            let url = format!("{base}/{name}");
            async move {
                match crate::web::fetch_bytes(&url).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        log::warn!("{e}");
                        None
                    }
                }
            }
        };
        let manifest = get("manifest.gmc").await;
        let atlas = get("ui.gma").await;
        let c = Content::from_bytes(base, manifest, atlas);
        log::info!("content: {}", c.note);
        c
    }

    /// The atlas, once: the HUD uploads it.
    pub fn take_atlas(&mut self) -> Option<Atlas> {
        self.atlas.take()
    }

    /// Ask for a prop by key: `Some(slot)` when it is on the GPU. The first ask starts the
    /// load; a prop the bundle has not is `None` for good. `load` puts a decoded model on
    /// the GPU and returns its slot.
    pub fn prop(
        &mut self,
        key: &str,
        mut load: impl FnMut(&Model) -> Option<usize>,
    ) -> Option<usize> {
        if let Some(state) = self.props.get(key) {
            return match state {
                PropState::Loaded(slot) => Some(*slot),
                _ => None,
            };
        }
        let Some(entry) = self.manifest.as_ref().and_then(|m| m.prop(key)) else {
            self.props.insert(key.to_string(), PropState::Missing);
            return None;
        };
        let (file, sha) = (entry.file.clone(), entry.sha256);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = std::path::Path::new(&self.base).join(&file);
            let state = match std::fs::read(&path) {
                Ok(bytes) => Self::decoded(key, &bytes, sha, &mut load),
                Err(e) => {
                    log::warn!("prop `{key}`: {}: {e}", path.display());
                    PropState::Missing
                }
            };
            let out = match &state {
                PropState::Loaded(slot) => Some(*slot),
                _ => None,
            };
            self.props.insert(key.to_string(), state);
            out
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = sha;
            let url = format!("{}/{file}", self.base);
            let inbox = self.inbox.clone();
            let key_owned = key.to_string();
            wasm_bindgen_futures::spawn_local(async move {
                let bytes = match crate::web::fetch_bytes(&url).await {
                    Ok(b) => b,
                    Err(e) => {
                        log::warn!("{e}");
                        None
                    }
                };
                inbox.borrow_mut().push((key_owned, bytes));
            });
            self.props.insert(key.to_string(), PropState::Loading);
            None
        }
    }

    /// Fetches that landed since the last frame, decoded and put on the GPU (one a frame,
    /// like avatars, WEB.md 4).
    #[cfg(target_arch = "wasm32")]
    pub fn poll(&mut self, mut load: impl FnMut(&Model) -> Option<usize>) {
        let arrived = {
            let mut inbox = self.inbox.borrow_mut();
            if inbox.is_empty() {
                return;
            }
            inbox.remove(0)
        };
        let (key, bytes) = arrived;
        let sha = self
            .manifest
            .as_ref()
            .and_then(|m| m.prop(&key))
            .map(|p| p.sha256)
            .unwrap_or([0; 32]);
        let state = match bytes {
            Some(bytes) => Self::decoded(&key, &bytes, sha, &mut load),
            None => {
                log::warn!("prop `{key}`: not on the site");
                PropState::Missing
            }
        };
        self.props.insert(key, state);
    }

    fn decoded(
        key: &str,
        bytes: &[u8],
        sha: [u8; 32],
        load: &mut impl FnMut(&Model) -> Option<usize>,
    ) -> PropState {
        if gm_model::model_id(bytes) != sha {
            log::warn!("prop `{key}`: the file is not the one the manifest names");
            return PropState::Missing;
        }
        match Model::decode(bytes) {
            Ok(m) if m.is_prop() => match load(&m) {
                Some(slot) => PropState::Loaded(slot),
                None => PropState::Missing,
            },
            Ok(_) => {
                log::warn!("prop `{key}`: the file is a body, not a prop");
                PropState::Missing
            }
            Err(e) => {
                log::warn!("prop `{key}`: {e}");
                PropState::Missing
            }
        }
    }
}
