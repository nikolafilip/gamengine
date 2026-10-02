//! The model cache (MODELS.md 8): GPU → store → source. A model is asked for every frame its
//! wearer is drawn; until it is on the GPU the wearer is a mannequin. The store is a
//! directory of files named by model id natively and the Cache API in the browser (WEB.md
//! 4), with a byte cap that holds at all times; the GPU cache is a hard cap too, and nearest
//! wearers win.

use std::collections::{HashMap, VecDeque};

use gm_model::{Model, ModelId};
use web_time::{Duration, Instant};

use crate::characters::{Characters, model_gpu_bytes};
use crate::render::Gpu;

#[cfg(not(target_arch = "wasm32"))]
mod disk;
#[cfg(target_arch = "wasm32")]
pub use crate::web::store::Loader;
#[cfg(not(target_arch = "wasm32"))]
pub use disk::{DirSource, Loader, ModelSource, default_cache_dir};

/// Downloads and store reads running at once.
pub const MAX_IN_FLIGHT: usize = 4;
/// GPU uploads per frame: a 1024² atlas is 0.7 MB, two of them do not show in a frame time.
const UPLOADS_PER_FRAME: usize = 2;
/// A model nobody has asked for in this many frames is not worth loading any more.
const FORGET_AFTER_FRAMES: u64 = 240;
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The source will not serve this model: do not ask again this session.
    Refused(String),
    /// Try again later.
    Failed(String),
}

/// What a loader hands back.
pub enum Loaded {
    Ready {
        id: ModelId,
        model: Box<Model>,
        /// Bytes that came from the source (0: a disk hit).
        fetched: usize,
    },
    Refused(ModelId),
    Failed(ModelId),
}

enum Entry {
    /// Wanted; `distance` is the nearest wearer's when it was last asked for.
    Queued {
        distance: f32,
        wanted: u64,
    },
    /// With a loader thread, or loaded and waiting for the GPU. `retry` is the delay this
    /// attempt waited out, so that the next failure doubles it.
    Loading {
        retry: Option<Duration>,
    },
    Ready {
        slot: usize,
        used: u64,
        bytes: usize,
    },
    /// Failed; tried again after `until` while somebody still wears it (`wanted`).
    Backoff {
        until: Instant,
        delay: Duration,
        wanted: u64,
    },
    Refused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Models on the GPU now.
    pub ready: usize,
    pub gpu_bytes: usize,
    /// Loads that came from the source, and their bytes.
    pub fetched: u64,
    pub fetched_bytes: u64,
    pub disk_hits: u64,
    pub failed: u64,
    pub refused: u64,
    /// Models unloaded to stay under the GPU cap, and loads put off because nothing could go.
    pub gpu_evictions: u64,
    pub gpu_deferred: u64,
}

pub struct ModelCache {
    entries: HashMap<ModelId, Entry>,
    /// Where models are loaded from: the store, then the source.
    pub loader: Loader,
    in_flight: usize,
    frame: u64,
    vram_cap: usize,
    vram_used: usize,
    uploads: VecDeque<(ModelId, Box<Model>)>,
    pub stats: CacheStats,
}

impl ModelCache {
    /// A cache over a directory and a blocking source, with loader threads.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(
        dir: &std::path::Path,
        disk_cap: u64,
        vram_cap: usize,
        source: Option<std::sync::Arc<dyn ModelSource>>,
    ) -> std::io::Result<ModelCache> {
        Ok(ModelCache::with_loader(
            Loader::new(dir, disk_cap, source)?,
            vram_cap,
        ))
    }

    pub fn with_loader(loader: Loader, vram_cap: usize) -> ModelCache {
        ModelCache {
            entries: HashMap::new(),
            loader,
            in_flight: 0,
            frame: 0,
            vram_cap,
            vram_used: 0,
            uploads: VecDeque::new(),
            stats: CacheStats::default(),
        }
    }

    /// Ask for a model whose nearest wearer is `distance` away. The slot when it is on the GPU.
    pub fn want(&mut self, id: &ModelId, distance: f32) -> Option<usize> {
        let frame = self.frame;
        match self.entries.get_mut(id) {
            Some(Entry::Ready { slot, used, .. }) => {
                *used = frame;
                Some(*slot)
            }
            Some(Entry::Queued {
                distance: d,
                wanted,
            }) => {
                *d = if *wanted == frame {
                    d.min(distance)
                } else {
                    distance
                };
                *wanted = frame;
                None
            }
            Some(Entry::Backoff { wanted, .. }) => {
                *wanted = frame;
                None
            }
            Some(_) => None,
            None => {
                self.entries.insert(
                    *id,
                    Entry::Queued {
                        distance,
                        wanted: frame,
                    },
                );
                None
            }
        }
    }

    /// Once per frame: take finished loads, upload a few, start the nearest waiting ones.
    pub fn pump(&mut self, gpu: &Gpu, characters: &mut Characters) {
        let now = Instant::now();
        self.frame += 1;
        self.loader.begin_frame();
        self.collect(now);
        self.upload(gpu, characters, now);
        self.schedule(now);
        self.stats.ready = self
            .entries
            .values()
            .filter(|e| matches!(e, Entry::Ready { .. }))
            .count();
        self.stats.gpu_bytes = self.vram_used;
    }

    /// Take what the loader threads finished.
    fn collect(&mut self, now: Instant) {
        while let Some(loaded) = self.loader.try_recv() {
            self.in_flight -= 1;
            match loaded {
                Loaded::Ready { id, model, fetched } => {
                    if fetched > 0 {
                        self.stats.fetched += 1;
                        self.stats.fetched_bytes += fetched as u64;
                    } else {
                        self.stats.disk_hits += 1;
                    }
                    match self.entries.get(&id) {
                        Some(Entry::Loading { .. }) => self.uploads.push_back((id, model)),
                        // Revoked while it was loading: the loader has just written the
                        // file that `revoke` could not find. It goes too.
                        Some(Entry::Refused) => self.loader.remove(&id),
                        _ => {}
                    }
                }
                Loaded::Refused(id) => {
                    self.stats.refused += 1;
                    self.entries.insert(id, Entry::Refused);
                }
                Loaded::Failed(id) => {
                    self.stats.failed += 1;
                    // Only what is still loading is tried again: a model revoked meanwhile
                    // stays refused.
                    if matches!(self.entries.get(&id), Some(Entry::Loading { .. })) {
                        self.back_off(id, now);
                    }
                }
            }
        }
    }

    /// Put a few loaded models on the GPU, within its cap.
    fn upload(&mut self, gpu: &Gpu, characters: &mut Characters, now: Instant) {
        for _ in 0..UPLOADS_PER_FRAME {
            let Some((id, model)) = self.uploads.pop_front() else {
                break;
            };
            let need = model_gpu_bytes(&model, gpu.bc);
            if !self.make_room(need, characters) {
                // Everything loaded is in view: the nearest wearers keep their models and
                // this one stays a mannequin for now.
                self.stats.gpu_deferred += 1;
                self.back_off(id, now);
                continue;
            }
            let slot = characters.add_model(gpu, &model);
            let bytes = characters.info(slot).map_or(need, |i| i.gpu_bytes);
            self.vram_used += bytes;
            self.entries.insert(
                id,
                Entry::Ready {
                    slot,
                    used: self.frame,
                    bytes,
                },
            );
        }
    }

    /// Start loads, the nearest wanted first, and forget what nobody wants any more.
    fn schedule(&mut self, now: Instant) {
        let frame = self.frame;
        self.entries.retain(|_, e| match e {
            Entry::Queued { wanted, .. } | Entry::Backoff { wanted, .. } => {
                frame - *wanted < FORGET_AFTER_FRAMES
            }
            _ => true,
        });
        while self.in_flight < MAX_IN_FLIGHT {
            let next = self
                .entries
                .iter()
                .filter_map(|(id, e)| match e {
                    Entry::Queued { distance, .. } => Some((*id, *distance, None)),
                    Entry::Backoff { until, delay, .. } if *until <= now => {
                        Some((*id, f32::MAX, Some(*delay)))
                    }
                    _ => None,
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some((id, _, retry)) = next else { break };
            if !self.loader.start(id) {
                break;
            }
            self.entries.insert(id, Entry::Loading { retry });
            self.in_flight += 1;
        }
    }

    /// The attempt failed (the source, or no room on the GPU): wait twice as long as before
    /// this attempt, from 2 s up to 60 s, and only while somebody still wears the model.
    fn back_off(&mut self, id: ModelId, now: Instant) {
        let delay = match self.entries.get(&id) {
            Some(Entry::Loading { retry: Some(d) }) => (*d * 2).min(MAX_BACKOFF),
            _ => FIRST_BACKOFF,
        };
        self.entries.insert(
            id,
            Entry::Backoff {
                until: now + delay,
                delay,
                wanted: self.frame,
            },
        );
    }

    /// Unload models that are not in view until `need` more bytes fit under the cap.
    fn make_room(&mut self, need: usize, characters: &mut Characters) -> bool {
        while self.vram_used + need > self.vram_cap {
            // In view means drawn this frame or the last one.
            let victim = self
                .entries
                .iter()
                .filter_map(|(id, e)| match e {
                    Entry::Ready { used, .. } if *used + 1 < self.frame => Some((*id, *used)),
                    _ => None,
                })
                .min_by_key(|(_, used)| *used);
            let Some((id, _)) = victim else {
                return false;
            };
            if let Some(Entry::Ready { slot, bytes, .. }) = self.entries.remove(&id) {
                characters.remove(slot);
                self.vram_used -= bytes;
                self.stats.gpu_evictions += 1;
            }
        }
        true
    }

    /// The model was taken down (MODELS.md 8): off the GPU, off the disk, not asked for again.
    pub fn revoke(&mut self, id: &ModelId, characters: &mut Characters) {
        if let Some(slot) = self.refuse(id) {
            characters.remove(slot);
        }
    }

    /// Everything of `revoke` but the GPU: the slot to free, if the model was loaded.
    fn refuse(&mut self, id: &ModelId) -> Option<usize> {
        let slot = match self.entries.insert(*id, Entry::Refused) {
            Some(Entry::Ready { slot, bytes, .. }) => {
                self.vram_used -= bytes;
                Some(slot)
            }
            _ => None,
        };
        self.uploads.retain(|(i, _)| i != id);
        self.loader.remove(id);
        slot
    }

    /// Models waiting for the disk, the source or the GPU.
    pub fn pending(&self) -> usize {
        self.uploads.len()
            + self
                .entries
                .values()
                .filter(|e| matches!(e, Entry::Queued { .. } | Entry::Loading { .. }))
                .count()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::SystemTime;

    use gm_model::{id_hex, model_id};

    use super::disk::{DiskCache, MIN_DISK_CAP, load};
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gm-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Distinct bytes of a given size, and their id.
    fn blob(seed: u8, len: usize) -> (ModelId, Vec<u8>) {
        let bytes: Vec<u8> = (0..len)
            .map(|i| (i as u8).wrapping_mul(31) ^ seed)
            .collect();
        (model_id(&bytes), bytes)
    }

    fn dir_bytes(dir: &Path) -> u64 {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .sum()
    }

    fn age(dir: &Path, id: &ModelId, secs: u64) {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join(format!("{}.gmm", id_hex(id))))
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn the_cap_holds_after_every_insert_and_the_oldest_go_first() {
        let dir = temp_dir("cap");
        let cache = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        let size = 1024 * 1024;
        let mut ids = Vec::new();
        for i in 0..40u8 {
            let (id, bytes) = blob(i, size);
            cache.insert(&id, &bytes).unwrap();
            // Make insertion order the age order, whatever the clock's resolution.
            age(&dir, &id, 1000 - i as u64 * 10);
            ids.push(id);
            assert!(
                dir_bytes(&dir) <= MIN_DISK_CAP,
                "{} bytes after insert {i}",
                dir_bytes(&dir)
            );
            assert_eq!(cache.total(), dir_bytes(&dir));
        }
        // 16 MiB holds sixteen 1 MiB files: the newest sixteen.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 16);
        assert!(cache.read(&ids[0]).is_none());
        assert!(cache.read(&ids[23]).is_none());
        assert!(cache.read(&ids[24]).is_some());
        assert!(cache.read(&ids[39]).is_some());
        // Nothing but models lives there: no temporary file is left behind.
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            assert!(e.file_name().to_string_lossy().ends_with(".gmm"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn use_refreshes_a_file_and_pins_go_last() {
        let dir = temp_dir("lru");
        let cache = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        let size = 2 * 1024 * 1024;
        let files: Vec<(ModelId, Vec<u8>)> = (0..8u8).map(|i| blob(i, size)).collect();
        for (i, (id, bytes)) in files.iter().enumerate() {
            cache.insert(id, bytes).unwrap();
            age(&dir, id, 5000 - i as u64 * 100);
        }
        // The cache is full (8 × 2 MiB). The oldest is pinned; the second oldest is read.
        cache.pin(&files[0].0);
        let reopened = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        reopened.pin(&files[0].0);
        assert!(reopened.read(&files[1].0).is_some());
        // Two more arrive: the two oldest that are neither pinned nor just used must go.
        for i in 100..102u8 {
            let (id, bytes) = blob(i, size);
            reopened.insert(&id, &bytes).unwrap();
        }
        assert!(dir_bytes(&dir) <= MIN_DISK_CAP);
        assert!(
            reopened.read(&files[0].0).is_some(),
            "the pinned model stays"
        );
        assert!(
            reopened.read(&files[1].0).is_some(),
            "the model just used stays"
        );
        assert!(reopened.read(&files[2].0).is_none());
        assert!(reopened.read(&files[3].0).is_none());
        assert!(reopened.read(&files[4].0).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_deleted_and_stale_temporaries_are_cleared() {
        let dir = temp_dir("corrupt");
        let cache = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        let (id, bytes) = blob(1, 4096);
        cache.insert(&id, &bytes).unwrap();
        assert_eq!(cache.read(&id), Some(bytes.clone()));
        // Flip a byte on disk: the name no longer matches the content.
        let path = dir.join(format!("{}.gmm", id_hex(&id)));
        let mut damaged = bytes.clone();
        damaged[100] ^= 1;
        std::fs::write(&path, &damaged).unwrap();
        assert_eq!(cache.read(&id), None);
        assert!(!path.exists());
        assert_eq!(cache.total(), 0);
        // A crashed writer's temporary file, and a file that is not ours.
        let stale = dir.join(".dead.1.0.tmp");
        std::fs::write(&stale, [0u8; 100]).unwrap();
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        std::fs::write(dir.join("notes.txt"), b"keep me").unwrap();
        let reopened = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        assert!(!stale.exists());
        assert!(dir.join("notes.txt").exists());
        assert_eq!(reopened.total(), 0);
        // Inserting what is already there is a no-op; a model larger than the cache is refused.
        reopened.insert(&id, &bytes).unwrap();
        reopened.insert(&id, &bytes).unwrap();
        assert_eq!(reopened.total(), 4096);
        let (big_id, _) = blob(2, 16);
        assert!(
            reopened
                .insert(&big_id, &vec![0u8; MIN_DISK_CAP as usize + 1])
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_caches_on_one_directory_keep_the_cap() {
        let dir = temp_dir("shared");
        let a = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        let b = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        for i in 0..40u8 {
            let (id, bytes) = blob(i, 1024 * 1024);
            if i % 2 == 0 { &a } else { &b }
                .insert(&id, &bytes)
                .unwrap();
            // Each cache sees the other's writes by the directory's modification time; two
            // writes inside one clock tick can hide one file for one insertion.
            assert!(
                dir_bytes(&dir) <= MIN_DISK_CAP + 1024 * 1024,
                "{} bytes after insert {i}",
                dir_bytes(&dir)
            );
        }
        assert!(dir_bytes(&dir) <= MIN_DISK_CAP + 1024 * 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_loaders_never_overshoot_the_cap() {
        let dir = temp_dir("threads");
        let cache = Arc::new(DiskCache::open(&dir, MIN_DISK_CAP).unwrap());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // A watcher samples the directory while eight threads fill it three times over.
        let watcher = {
            let (dir, stop) = (dir.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut worst = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let bytes: u64 = std::fs::read_dir(&dir)
                        .map(|d| {
                            d.flatten()
                                .filter_map(|e| e.metadata().ok())
                                .map(|m| m.len())
                                .sum()
                        })
                        .unwrap_or(0);
                    worst = worst.max(bytes);
                }
                worst
            })
        };
        let writers: Vec<_> = (0..8u8)
            .map(|t| {
                let cache = cache.clone();
                std::thread::spawn(move || {
                    for i in 0..6u8 {
                        let (id, bytes) = blob(t * 16 + i, 1024 * 1024);
                        cache.insert(&id, &bytes).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        let worst = watcher.join().unwrap();
        assert!(worst <= MIN_DISK_CAP, "the directory reached {worst} bytes");
        assert_eq!(
            cache.total(),
            dir_bytes(&dir),
            "the count matches the directory"
        );
        assert!(
            dir_bytes(&dir) > MIN_DISK_CAP - 2 * 1024 * 1024,
            "and the cache is used"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct Down(AtomicU64);

    impl ModelSource for Down {
        fn fetch(&self, _: &ModelId) -> Result<Vec<u8>, FetchError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(FetchError::Failed("the hub is down".into()))
        }
    }

    #[test]
    fn a_failing_fetch_waits_twice_as_long_each_time_and_stops_when_nobody_wears_the_model() {
        let dir = temp_dir("backoff");
        let source = Arc::new(Down(AtomicU64::new(0)));
        let mut cache = ModelCache::new(&dir, MIN_DISK_CAP, 1 << 20, Some(source.clone())).unwrap();
        let id = [5u8; 32];
        let mut now = Instant::now();
        let mut delays = Vec::new();
        for _ in 0..7 {
            cache.frame += 1;
            assert_eq!(cache.want(&id, 10.0), None);
            cache.schedule(now);
            assert!(matches!(cache.entries[&id], Entry::Loading { .. }));
            // The loader thread answers.
            let waited = Instant::now();
            while !matches!(cache.entries[&id], Entry::Backoff { .. }) {
                assert!(waited.elapsed() < Duration::from_secs(5), "no answer");
                std::thread::sleep(Duration::from_millis(1));
                cache.collect(now);
            }
            let Entry::Backoff { delay, .. } = cache.entries[&id] else {
                unreachable!()
            };
            delays.push(delay.as_secs());
            // Not a moment before its time, however often it is wanted.
            cache.frame += 1;
            cache.want(&id, 10.0);
            cache.schedule(now + delay - Duration::from_millis(1));
            assert!(matches!(cache.entries[&id], Entry::Backoff { .. }));
            now += delay;
        }
        assert_eq!(delays, [2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(source.0.load(Ordering::Relaxed), 7);
        assert_eq!(cache.stats.failed, 7);
        // The wearer left while the retry was waiting: the model is forgotten, and when the
        // wait is over nothing is asked for.
        now -= Duration::from_secs(30);
        for _ in 0..=FORGET_AFTER_FRAMES {
            cache.frame += 1;
            cache.schedule(now);
        }
        assert!(!cache.entries.contains_key(&id));
        cache.frame += 1;
        cache.schedule(now + Duration::from_secs(3600));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(source.0.load(Ordering::Relaxed), 7);
        assert_eq!(cache.pending(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A small valid model: the striker mannequin with a blank 64 × 64 atlas.
    fn tiny_model() -> (ModelId, Vec<u8>, Model) {
        use gm_core::vocab::ArchetypeFrame;
        use gm_model::mannequin::{self, Shape};
        let mesh = mannequin::build(ArchetypeFrame::Striker, &Shape::MANNEQUIN);
        let (scale, vertices) = gm_model::format::quantize_vertices(
            &mesh.positions,
            &mesh.normals,
            &mesh.uvs,
            &mesh.joints,
            &mesh.weights,
        );
        let model = Model {
            flags: 0,
            frame: gm_model::rig::frame_index(ArchetypeFrame::Striker),
            scale,
            average: [128, 128, 128, 255],
            bone_mask: mesh.bone_mask,
            pivots: mesh.pivots.map(|p| p.to_array()),
            vertices,
            indices: mesh.indices.iter().map(|i| *i as u16).collect(),
            tex_w: 64,
            tex_h: 64,
            texture: vec![0; gm_model::format::texture_bytes(64, 64)],
        };
        let bytes = model.encode().unwrap();
        (model_id(&bytes), bytes, model)
    }

    /// A source that answers when the test lets it.
    struct Gated {
        go: Mutex<mpsc::Receiver<()>>,
        answer: Result<Vec<u8>, ()>,
    }

    impl ModelSource for Gated {
        fn fetch(&self, _: &ModelId) -> Result<Vec<u8>, FetchError> {
            let _ = self.go.lock().unwrap().recv();
            self.answer
                .clone()
                .map_err(|()| FetchError::Failed("down".into()))
        }
    }

    /// MODELS.md 8: a model revoked while its download is under way is not kept, not drawn
    /// and not asked for again, whichever way the download ends.
    #[test]
    fn a_revocation_wins_against_a_download_in_flight() {
        let (id, bytes, _) = tiny_model();
        for answer in [Ok(bytes), Err(())] {
            let arrives = answer.is_ok();
            let dir = temp_dir(if arrives { "revoke-ok" } else { "revoke-err" });
            let (go, gate) = mpsc::channel();
            let source = Arc::new(Gated {
                go: Mutex::new(gate),
                answer,
            });
            let mut cache = ModelCache::new(&dir, MIN_DISK_CAP, 1 << 24, Some(source)).unwrap();
            let now = Instant::now();
            cache.frame += 1;
            cache.want(&id, 1.0);
            cache.schedule(now);
            assert!(matches!(cache.entries[&id], Entry::Loading { .. }));
            // The takedown arrives first; then the download ends.
            assert_eq!(cache.refuse(&id), None);
            go.send(()).unwrap();
            let waited = Instant::now();
            while cache.in_flight > 0 {
                assert!(waited.elapsed() < Duration::from_secs(5), "no answer");
                std::thread::sleep(Duration::from_millis(1));
                cache.collect(now);
            }
            assert!(matches!(cache.entries[&id], Entry::Refused));
            assert!(cache.uploads.is_empty(), "nothing goes to the GPU");
            assert_eq!(dir_bytes(&dir), 0, "nothing stays on the disk");
            assert_eq!(cache.loader.disk.total(), 0);
            // Wanted again and again: never asked for again.
            for _ in 0..5 {
                cache.frame += 1;
                assert_eq!(cache.want(&id, 1.0), None);
                cache.schedule(now + Duration::from_secs(3600));
            }
            assert_eq!(cache.in_flight, 0);
            assert_eq!(cache.stats.fetched, u64::from(arrives));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn the_gpu_budget_counts_what_this_gpu_will_hold() {
        let (_, _, model) = tiny_model();
        let geometry = model.vertices.len() * 24 + model.indices.len() * 2;
        // With BC: the blocks as stored.
        assert_eq!(model_gpu_bytes(&model, true), model.gpu_bytes());
        assert_eq!(
            model_gpu_bytes(&model, true),
            geometry + model.texture.len()
        );
        // Without: RGBA8 of every level but the largest (32, 16, 8 and 4 texels a side).
        assert_eq!(
            model_gpu_bytes(&model, false),
            geometry + (32 * 32 + 16 * 16 + 8 * 8 + 4 * 4) * 4
        );
    }

    struct MapSource(HashMap<ModelId, Vec<u8>>);

    impl ModelSource for MapSource {
        fn fetch(&self, id: &ModelId) -> Result<Vec<u8>, FetchError> {
            self.0
                .get(id)
                .cloned()
                .ok_or_else(|| FetchError::Refused("unknown".into()))
        }
    }

    #[test]
    fn a_load_verifies_the_hash_before_parsing_and_fills_the_disk() {
        let dir = temp_dir("load");
        let disk = DiskCache::open(&dir, MIN_DISK_CAP).unwrap();
        let (id, garbage) = blob(7, 2000);
        // The source answers with other bytes than were asked for.
        let mut lying = HashMap::new();
        lying.insert(id, vec![1u8, 2, 3]);
        assert!(matches!(
            load(id, &disk, Some(&MapSource(lying))),
            Loaded::Failed(_)
        ));
        assert_eq!(disk.total(), 0, "nothing unverified reaches the disk");
        // The right bytes that are not a model: refused for good, and not kept.
        let mut honest = HashMap::new();
        honest.insert(id, garbage);
        assert!(matches!(
            load(id, &disk, Some(&MapSource(honest))),
            Loaded::Refused(_)
        ));
        assert_eq!(disk.total(), 0);
        // Unknown to the source, and no source at all.
        assert!(matches!(
            load([9; 32], &disk, Some(&MapSource(HashMap::new()))),
            Loaded::Refused(_)
        ));
        assert!(matches!(load([9; 32], &disk, None), Loaded::Refused(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
