//! The native half of the model cache (MODELS.md 8): a directory with a byte cap, blocking
//! sources and four loader threads.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime};

use gm_model::{Model, ModelId, id_from_hex, id_hex, model_id};

use super::{FetchError, Loaded, MAX_IN_FLIGHT};

/// The smallest disk cap: a dozen models.
pub const MIN_DISK_CAP: u64 = 16 * 1024 * 1024;
/// Temporary files older than this are a crashed writer's.
const STALE_TEMP: Duration = Duration::from_secs(600);

/// Where model bytes come from when the disk does not have them.
pub trait ModelSource: Send + Sync {
    fn fetch(&self, id: &ModelId) -> Result<Vec<u8>, FetchError>;
}

/// A directory of ingested models indexed by their ids: the offline crowd's source.
pub struct DirSource {
    files: HashMap<ModelId, PathBuf>,
}

impl DirSource {
    /// Every `.gmm` under `dir`, with the ids in file-name order.
    pub fn scan(dir: &Path) -> std::io::Result<(DirSource, Vec<ModelId>)> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "gmm"))
            .collect();
        paths.sort();
        let mut files = HashMap::new();
        let mut ids = Vec::new();
        for p in paths {
            let id = model_id(&std::fs::read(&p)?);
            if files.insert(id, p).is_none() {
                ids.push(id);
            }
        }
        Ok((DirSource { files }, ids))
    }
}

impl ModelSource for DirSource {
    fn fetch(&self, id: &ModelId) -> Result<Vec<u8>, FetchError> {
        let path = self
            .files
            .get(id)
            .ok_or_else(|| FetchError::Refused("not in the directory".into()))?;
        std::fs::read(path).map_err(|e| FetchError::Failed(e.to_string()))
    }
}

struct DiskState {
    /// Bytes in the directory as far as this process knows.
    total: u64,
    /// The directory's modification time when `total` was last true: another client writing
    /// to the same directory moves it, and the next insertion counts again.
    seen: Option<SystemTime>,
    pinned: HashSet<ModelId>,
    /// Files whose modification time was already moved to "now" this session.
    touched: HashSet<ModelId>,
}

/// The disk cache: `<dir>/<id in hex>.gmm`, least recently used out first.
pub struct DiskCache {
    dir: PathBuf,
    cap: u64,
    state: Mutex<DiskState>,
    /// One insertion at a time: making room and writing are one step, or four loaders would
    /// each make room for one file and together overshoot the cap by three.
    inserting: Mutex<()>,
    temp: AtomicU64,
}

struct DiskFile {
    path: PathBuf,
    id: Option<ModelId>,
    size: u64,
    used: SystemTime,
}

impl DiskCache {
    /// Open (and create) the directory, clear stale temporary files and enforce the cap.
    pub fn open(dir: &Path, cap: u64) -> std::io::Result<DiskCache> {
        std::fs::create_dir_all(dir)?;
        let cache = DiskCache {
            dir: dir.to_path_buf(),
            cap: cap.max(MIN_DISK_CAP),
            state: Mutex::new(DiskState {
                total: 0,
                seen: None,
                pinned: HashSet::new(),
                touched: HashSet::new(),
            }),
            inserting: Mutex::new(()),
            temp: AtomicU64::new(0),
        };
        cache.enforce(cache.cap);
        Ok(cache)
    }

    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// Bytes on disk as of the last scan plus what this process added since.
    pub fn total(&self) -> u64 {
        self.state.lock().unwrap().total
    }

    fn path(&self, id: &ModelId) -> PathBuf {
        self.dir.join(format!("{}.gmm", id_hex(id)))
    }

    /// Never evict this model before the unpinned ones (the own avatar).
    pub fn pin(&self, id: &ModelId) {
        self.state.lock().unwrap().pinned.insert(*id);
    }

    /// The model's bytes if the cache holds them intact. A file that does not hash to its
    /// name is deleted.
    pub fn read(&self, id: &ModelId) -> Option<Vec<u8>> {
        let path = self.path(id);
        let bytes = std::fs::read(&path).ok()?;
        if model_id(&bytes) != *id {
            log::warn!("cached model {} is corrupt; deleting it", id_hex(id));
            if std::fs::remove_file(&path).is_ok() {
                let mut st = self.state.lock().unwrap();
                st.total = st.total.saturating_sub(bytes.len() as u64);
            }
            return None;
        }
        // Last use is the file's modification time; once per session is enough.
        if self.state.lock().unwrap().touched.insert(*id)
            && let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path)
        {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    /// Store a model. Room is made first, so the directory never exceeds the cap.
    pub fn insert(&self, id: &ModelId, bytes: &[u8]) -> std::io::Result<()> {
        let len = bytes.len() as u64;
        if len > self.cap {
            return Err(std::io::Error::other("model larger than the cache"));
        }
        let _one_at_a_time = self.inserting.lock().unwrap();
        let path = self.path(id);
        if path.exists() {
            return Ok(());
        }
        // Make room first, counting afresh when somebody else changed the directory. Two
        // clients sharing it can still overshoot by the one file each is writing right now;
        // within one client the cap is exact.
        let (total, seen) = {
            let st = self.state.lock().unwrap();
            (st.total, st.seen)
        };
        if seen != self.dir_time() || total + len > self.cap {
            self.enforce(self.cap - len);
        }
        // A name of our own (two clients may share the directory), then an atomic rename.
        let tmp = self.dir.join(format!(
            ".{}.{}.{}.tmp",
            id_hex(id),
            std::process::id(),
            self.temp.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, bytes)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        let seen = self.dir_time();
        let mut st = self.state.lock().unwrap();
        st.total += len;
        st.seen = seen;
        st.touched.insert(*id);
        Ok(())
    }

    fn dir_time(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.dir).and_then(|m| m.modified()).ok()
    }

    pub fn remove(&self, id: &ModelId) {
        let path = self.path(id);
        if let Ok(meta) = std::fs::metadata(&path)
            && std::fs::remove_file(&path).is_ok()
        {
            let mut st = self.state.lock().unwrap();
            st.total = st.total.saturating_sub(meta.len());
        }
    }

    fn scan(&self) -> Vec<DiskFile> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let now = SystemTime::now();
        let mut files = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".tmp") {
                if now.duration_since(used).is_ok_and(|age| age > STALE_TEMP) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                // Somebody is writing it: it counts, and it is not ours to delete.
                files.push(DiskFile {
                    path,
                    id: None,
                    size: meta.len(),
                    used: now,
                });
                continue;
            }
            let id = name.strip_suffix(".gmm").and_then(id_from_hex);
            if id.is_none() {
                continue; // not ours
            }
            files.push(DiskFile {
                path,
                id,
                size: meta.len(),
                used,
            });
        }
        files
    }

    /// Delete least recently used files until the directory holds at most `target` bytes.
    /// Works from a fresh scan, so it sees what other clients wrote.
    fn enforce(&self, target: u64) {
        let mut files = self.scan();
        let mut total: u64 = files.iter().map(|f| f.size).sum();
        let pinned = self.state.lock().unwrap().pinned.clone();
        // Pins are honoured newest first up to half the cap (MODELS.md 8).
        files.sort_by_key(|f| std::cmp::Reverse(f.used));
        let mut pinned_bytes = 0u64;
        let mut keep: HashSet<PathBuf> = HashSet::new();
        for f in &files {
            if f.id.is_some_and(|id| pinned.contains(&id)) && pinned_bytes + f.size <= self.cap / 2
            {
                pinned_bytes += f.size;
                keep.insert(f.path.clone());
            }
        }
        // Oldest first; the pinned set last.
        files.sort_by(|a, b| {
            keep.contains(&a.path)
                .cmp(&keep.contains(&b.path))
                .then(a.used.cmp(&b.used))
        });
        let mut removed = Vec::new();
        for f in &files {
            if total <= target {
                break;
            }
            if f.id.is_none() {
                continue;
            }
            if std::fs::remove_file(&f.path).is_ok() {
                total -= f.size;
                removed.push(f.id);
            }
        }
        let seen = self.dir_time();
        let mut st = self.state.lock().unwrap();
        st.total = total;
        st.seen = seen;
        for id in removed.into_iter().flatten() {
            st.touched.remove(&id);
        }
    }
}

/// The default cache directory of the platform.
pub fn default_cache_dir() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("gamengine")
        .join("models")
}

/// One loader thread: disk, then the source; hash before anything parses.
pub(super) fn load(id: ModelId, disk: &DiskCache, source: Option<&dyn ModelSource>) -> Loaded {
    let (bytes, fetched) = match disk.read(&id) {
        Some(b) => (b, 0),
        None => {
            let Some(source) = source else {
                return Loaded::Refused(id);
            };
            match source.fetch(&id) {
                Ok(b) if model_id(&b) == id => {
                    if let Err(e) = disk.insert(&id, &b) {
                        log::warn!("caching model {}: {e}", id_hex(&id));
                    }
                    let n = b.len();
                    (b, n)
                }
                Ok(_) => {
                    log::warn!("model {} arrived with the wrong hash", id_hex(&id));
                    return Loaded::Failed(id);
                }
                Err(FetchError::Refused(why)) => {
                    log::debug!("model {} refused: {why}", id_hex(&id));
                    return Loaded::Refused(id);
                }
                Err(FetchError::Failed(why)) => {
                    log::debug!("model {} failed: {why}", id_hex(&id));
                    return Loaded::Failed(id);
                }
            }
        }
    };
    match Model::decode(&bytes) {
        Ok(model) => Loaded::Ready {
            id,
            model: Box::new(model),
            fetched,
        },
        Err(e) => {
            // The hash matched and it still does not parse: whoever made it is at fault,
            // and asking again will not help.
            log::warn!("model {} does not decode: {e}", id_hex(&id));
            disk.remove(&id);
            Loaded::Refused(id)
        }
    }
}

/// The loader threads and the disk they share.
pub struct Loader {
    pub disk: Arc<DiskCache>,
    jobs: mpsc::Sender<ModelId>,
    done: mpsc::Receiver<Loaded>,
}

impl Loader {
    pub fn new(
        dir: &Path,
        disk_cap: u64,
        source: Option<Arc<dyn ModelSource>>,
    ) -> std::io::Result<Loader> {
        let disk = Arc::new(DiskCache::open(dir, disk_cap)?);
        let (jobs, job_rx) = mpsc::channel::<ModelId>();
        let (done_tx, done) = mpsc::channel::<Loaded>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for i in 0..MAX_IN_FLIGHT {
            let (job_rx, done_tx) = (job_rx.clone(), done_tx.clone());
            let (disk, source) = (disk.clone(), source.clone());
            std::thread::Builder::new()
                .name(format!("gm-model-{i}"))
                .spawn(move || {
                    loop {
                        // The lock is held only while waiting for a job, not while loading.
                        let job = job_rx.lock().unwrap().recv();
                        let Ok(id) = job else { break };
                        if done_tx.send(load(id, &disk, source.as_deref())).is_err() {
                            break;
                        }
                    }
                })?;
        }
        Ok(Loader { disk, jobs, done })
    }

    /// Hand a model to a loader thread; `false` when the threads are gone.
    pub fn start(&self, id: ModelId) -> bool {
        self.jobs.send(id).is_ok()
    }

    pub fn begin_frame(&mut self) {}

    /// The next finished load, if any.
    pub fn try_recv(&mut self) -> Option<Loaded> {
        self.done.try_recv().ok()
    }

    /// Forget a stored model (a takedown).
    pub fn remove(&self, id: &ModelId) {
        self.disk.remove(id);
    }
}
