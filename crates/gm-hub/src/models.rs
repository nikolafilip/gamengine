//! Custom models at the hub (MODELS.md 6, 10): uploads ingested in a worker process, stored by
//! hash, held by the accounts that uploaded them, moderated and taken down as one upload
//! across frames. Every state change is one transaction and one `model_events` row.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gm_hub_proto::protocol::{
    AccountId, CharacterId, DEFAULT_MODEL_SLOTS, HubError, INGEST_PERMITS, MAX_MODEL_UPLOAD_BYTES,
    MAX_PENDING_MODELS, ModEntry, ModelId, ModelRef, ModelStatus, ModelSummary, ReasonCode,
    SessionId, TOS_VERSION, TRUSTED_TIER, UPLOAD_STRIKES,
};
use gm_ingest::Report;
use gm_model::{id_hex, model_id, rig};
use sqlx::postgres::PgPool;
use sqlx::{Postgres, Row, Transaction};
use tokio::sync::Semaphore;
use tracing::{info, warn};

/// Wall clock of one ingestion (MODELS.md 6.2).
pub const INGEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Uploads whose bytes are in the hub's memory at once (received, or waiting for a worker).
pub const UPLOAD_SLOTS: usize = 8;
/// How long a received upload waits for one of the `INGEST_PERMITS` workers before `Busy`.
pub const INGEST_QUEUE_WAIT: Duration = Duration::from_secs(30);
/// Uploads an account may make per hour.
pub const UPLOADS_PER_HOUR: f64 = 10.0;
/// Download budget per session: bytes per second and burst (MODELS.md 6.2).
pub const GET_BYTES_PER_SEC: f64 = 8.0 * 1024.0 * 1024.0;
pub const GET_BURST_BYTES: f64 = 128.0 * 1024.0 * 1024.0;

type Tx<'a> = Transaction<'a, Postgres>;

fn internal(e: sqlx::Error) -> HubError {
    tracing::error!("database: {e}");
    HubError::Internal
}

fn io(e: std::io::Error) -> HubError {
    tracing::error!("model store: {e}");
    HubError::Internal
}

/// How uploads are parsed.
#[derive(Clone, Debug)]
pub enum IngestMode {
    /// `<exe> ingest-worker --frame N --in FILE --out DIR` in a child process with resource
    /// limits: what production runs.
    Worker(PathBuf),
    /// In the hub's own process: for embedding the hub in tests of other crates.
    InProcess,
}

/// The content-addressed store: `<id>.gmm`, `<id>.png`, `src/<sha256>.glb`.
pub struct ModelStore {
    dir: PathBuf,
    temp: AtomicU64,
}

impl ModelStore {
    pub fn open(dir: &Path) -> std::io::Result<ModelStore> {
        std::fs::create_dir_all(dir.join("src"))?;
        Ok(ModelStore {
            dir: dir.to_path_buf(),
            temp: AtomicU64::new(0),
        })
    }

    fn put(&self, path: PathBuf, bytes: &[u8]) -> std::io::Result<()> {
        if path.exists() {
            return Ok(());
        }
        let tmp = self.dir.join(format!(
            ".{}.{}.tmp",
            std::process::id(),
            self.temp.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)
    }

    fn model_path(&self, id: &ModelId) -> PathBuf {
        self.dir.join(format!("{}.gmm", id_hex(id)))
    }

    fn preview_path(&self, id: &ModelId) -> PathBuf {
        self.dir.join(format!("{}.png", id_hex(id)))
    }

    pub fn put_model(&self, id: &ModelId, gmm: &[u8], preview: &[u8]) -> std::io::Result<()> {
        self.put(self.model_path(id), gmm)?;
        self.put(self.preview_path(id), preview)
    }

    pub fn put_source(&self, hash: &ModelId, upload: &[u8]) -> std::io::Result<()> {
        self.put(self.source_path(hash), upload)
    }

    pub fn model(&self, id: &ModelId) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.model_path(id))
    }

    pub fn preview(&self, id: &ModelId) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.preview_path(id))
    }

    fn source_path(&self, hash: &ModelId) -> PathBuf {
        self.dir.join("src").join(format!("{}.glb", id_hex(hash)))
    }

    fn remove(&self, id: &ModelId) {
        let _ = std::fs::remove_file(self.model_path(id));
        let _ = std::fs::remove_file(self.preview_path(id));
    }

    fn remove_source(&self, hash: &ModelId) {
        let _ = std::fs::remove_file(self.source_path(hash));
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    /// Take `cost` from a bucket of `burst` refilled at `rate` per second.
    fn take(&mut self, rate: f64, burst: f64, cost: f64) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * rate).min(burst);
        self.last = now;
        if self.tokens >= cost {
            self.tokens -= cost;
            true
        } else {
            false
        }
    }

    /// Give `cost` back: what it paid for did not happen, and not by the payer's doing.
    fn refund(&mut self, burst: f64, cost: f64) {
        self.tokens = (self.tokens + cost).min(burst);
    }
}

pub struct Models {
    pool: PgPool,
    store: ModelStore,
    ingest: IngestMode,
    timeout: Duration,
    /// Workers running (`INGEST_PERMITS`), and uploads in memory (`UPLOAD_SLOTS`).
    permits: Semaphore,
    bodies: Semaphore,
    queue_wait: Duration,
    /// Accounts with an upload in flight: one at a time each.
    uploading: Mutex<HashSet<AccountId>>,
    /// Files and the rows that name them change together: held while an upload is stored and
    /// while a withdrawn model is deleted, so the one never removes what the other just wrote.
    files: tokio::sync::Mutex<()>,
    uploads: Mutex<HashMap<AccountId, Bucket>>,
    gets: Mutex<HashMap<SessionId, Bucket>>,
    jobs: AtomicU64,
}

/// An account's turn to upload (`Models::begin_upload`): released when dropped.
pub struct UploadTurn<'a> {
    models: &'a Models,
    account: AccountId,
    _slot: tokio::sync::SemaphorePermit<'a>,
}

impl Drop for UploadTurn<'_> {
    fn drop(&mut self) {
        self.models.uploading.lock().unwrap().remove(&self.account);
    }
}

/// What an account may do, read with its row locked.
struct Uploader {
    privileged: bool,
    strikes: i16,
    trust_tier: i16,
    slots: i16,
}

fn id_of(bytes: Vec<u8>) -> Result<ModelId, HubError> {
    bytes.try_into().map_err(|_| HubError::Internal)
}

fn summary(r: &sqlx::postgres::PgRow) -> Result<ModelSummary, HubError> {
    let status: String = r.try_get("status").map_err(internal)?;
    let code: String = r.try_get("reason_code").map_err(internal)?;
    Ok(ModelSummary {
        id: id_of(r.try_get("hash").map_err(internal)?)?,
        frame: r.try_get::<i16, _>("frame").map_err(internal)? as u8,
        status: ModelStatus::from_name(&status).ok_or(HubError::Internal)?,
        bytes: r.try_get::<i32, _>("bytes").map_err(internal)? as u32,
        triangles: r.try_get::<i32, _>("triangles").map_err(internal)? as u32,
        texture: [
            r.try_get::<i16, _>("tex_w").map_err(internal)? as u16,
            r.try_get::<i16, _>("tex_h").map_err(internal)? as u16,
        ],
        code: ReasonCode::from_name(&code).unwrap_or(ReasonCode::Other),
        reason: r.try_get("reason").map_err(internal)?,
    })
}

const SUMMARY_COLUMNS: &str =
    "m.hash, m.frame, m.status, m.bytes, m.triangles, m.tex_w, m.tex_h, m.reason_code, m.reason";

async fn event(
    tx: &mut Tx<'_>,
    model: Option<&ModelId>,
    actor: AccountId,
    what: &str,
    detail: &str,
) -> Result<(), HubError> {
    sqlx::query("insert into model_events (model, actor, event, detail) values ($1, $2, $3, $4)")
        .bind(model.map(|m| m.as_slice()))
        .bind(actor)
        .bind(what)
        .bind(detail)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    Ok(())
}

impl Models {
    pub fn new(
        pool: PgPool,
        dir: &Path,
        ingest: IngestMode,
        timeout: Duration,
    ) -> std::io::Result<Models> {
        Ok(Models {
            pool,
            store: ModelStore::open(dir)?,
            ingest,
            timeout,
            permits: Semaphore::new(INGEST_PERMITS),
            bodies: Semaphore::new(UPLOAD_SLOTS),
            queue_wait: INGEST_QUEUE_WAIT,
            uploading: Mutex::new(HashSet::new()),
            files: tokio::sync::Mutex::new(()),
            uploads: Mutex::new(HashMap::new()),
            gets: Mutex::new(HashMap::new()),
            jobs: AtomicU64::new(0),
        })
    }

    /// How long a received upload waits for a worker (tests shorten it).
    pub fn set_queue_wait(&mut self, wait: Duration) {
        self.queue_wait = wait;
    }

    async fn begin(&self) -> Result<Tx<'_>, HubError> {
        self.pool.begin().await.map_err(internal)
    }

    /// The account's turn to upload, or `Busy`: one upload per account at a time, and at
    /// most `UPLOAD_SLOTS` bodies in memory. Taken before a byte of the body is read and held
    /// until the answer; it is not a worker (a slow sender must not keep a worker idle).
    pub fn begin_upload(&self, account: AccountId) -> Result<UploadTurn<'_>, HubError> {
        let slot = self.bodies.try_acquire().map_err(|_| HubError::Busy)?;
        if !self.uploading.lock().unwrap().insert(account) {
            return Err(HubError::Busy);
        }
        Ok(UploadTurn {
            models: self,
            account,
            _slot: slot,
        })
    }

    /// Give back the hourly token `precheck` took: the hub was busy, the upload did not run.
    fn refund_upload(&self, account: AccountId) {
        if let Some(b) = self.uploads.lock().unwrap().get_mut(&account) {
            b.refund(UPLOADS_PER_HOUR, 1.0);
        }
    }

    pub async fn is_moderator(&self, account: AccountId) -> Result<bool, HubError> {
        Ok(sqlx::query("select moderator from accounts where id = $1")
            .bind(account)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?
            .map(|r| r.try_get::<bool, _>("moderator"))
            .transpose()
            .map_err(internal)?
            .unwrap_or(false))
    }

    async fn uploader(tx: &mut Tx<'_>, account: AccountId) -> Result<Uploader, HubError> {
        let r = sqlx::query(
            "select upload_privileges, upload_strikes, trust_tier, model_slots from accounts where id = $1 for update",
        )
        .bind(account)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?
        .ok_or(HubError::NotFound)?;
        Ok(Uploader {
            privileged: r.try_get("upload_privileges").map_err(internal)?,
            strikes: r.try_get("upload_strikes").map_err(internal)?,
            trust_tier: r.try_get("trust_tier").map_err(internal)?,
            slots: r.try_get("model_slots").map_err(internal)?,
        })
    }

    /// Models the account holds that still occupy a slot, and how many of them are pending.
    async fn held(tx: &mut Tx<'_>, account: AccountId) -> Result<(i64, i64), HubError> {
        let r = sqlx::query(
            "select count(*) filter (where m.status in ('pending', 'active')) as held, \
             count(*) filter (where m.status = 'pending') as pending \
             from model_holders h join models m on m.hash = h.hash where h.account_id = $1",
        )
        .bind(account)
        .fetch_one(&mut **tx)
        .await
        .map_err(internal)?;
        Ok((
            r.try_get("held").map_err(internal)?,
            r.try_get("pending").map_err(internal)?,
        ))
    }

    /// Everything that can be refused before a byte of the body is read (MODELS.md 6.2).
    pub async fn precheck(
        &self,
        account: AccountId,
        frame: u8,
        tos_version: u16,
        len: u32,
    ) -> Result<(), HubError> {
        if rig::frame_from_index(frame).is_none() {
            return Err(HubError::Invalid("unknown frame".into()));
        }
        if tos_version != TOS_VERSION {
            return Err(HubError::Invalid(format!(
                "the upload terms are at version {TOS_VERSION}; certify them to upload"
            )));
        }
        if len == 0 || len > MAX_MODEL_UPLOAD_BYTES {
            return Err(HubError::Invalid(format!(
                "an upload is 1..={MAX_MODEL_UPLOAD_BYTES} bytes"
            )));
        }
        let mut tx = self.begin().await?;
        let u = Self::uploader(&mut tx, account).await?;
        if !u.privileged {
            return Err(HubError::Unauthorized);
        }
        if u.strikes >= UPLOAD_STRIKES {
            return Err(HubError::Invalid(format!(
                "{} strikes: this account cannot upload",
                u.strikes
            )));
        }
        let (held, pending) = Self::held(&mut tx, account).await?;
        if held >= u.slots as i64 {
            return Err(HubError::Full);
        }
        if pending >= MAX_PENDING_MODELS {
            return Err(HubError::Invalid(format!(
                "{pending} models are waiting for a moderator; wait for a decision"
            )));
        }
        drop(tx);
        let mut buckets = self.uploads.lock().unwrap();
        let b = buckets.entry(account).or_insert(Bucket {
            tokens: UPLOADS_PER_HOUR,
            last: Instant::now(),
        });
        if !b.take(UPLOADS_PER_HOUR / 3600.0, UPLOADS_PER_HOUR, 1.0) {
            return Err(HubError::Busy);
        }
        Ok(())
    }

    /// Run the ingestion on `upload`, out of process unless configured otherwise, and check
    /// what comes back. The worker has read an untrusted file: its report is text for the
    /// uploader, and its model is believed only as far as `gm_ingest::verify` confirms it
    /// from the bytes themselves (MODELS.md 6.2).
    async fn run_ingest(
        &self,
        upload: Vec<u8>,
        frame: u8,
    ) -> Result<(Report, Option<Accepted>), HubError> {
        let frame = rig::frame_from_index(frame).ok_or(HubError::Internal)?;
        let mode = self.ingest.clone();
        let timeout = self.timeout;
        let job = self.jobs.fetch_add(1, Ordering::Relaxed);
        tokio::task::spawn_blocking(move || {
            let (report, gmm) = match mode {
                IngestMode::InProcess => {
                    let (report, ingested) = gm_ingest::ingest(&upload, frame);
                    (report, ingested.map(|i| i.gmm))
                }
                IngestMode::Worker(exe) => worker(&exe, &upload, frame, job, timeout)?,
            };
            let Some(gmm) = gmm.filter(|_| report.ok) else {
                return Ok((report, None));
            };
            match gm_ingest::verify(&gmm, frame) {
                Ok(v) => Ok((
                    report,
                    Some(Accepted {
                        id: v.id,
                        triangles: v.model.triangles() as i32,
                        vertices: v.model.vertices.len() as i32,
                        texture: [v.model.tex_w as i16, v.model.tex_h as i16],
                        gmm,
                        preview: v.preview,
                    }),
                )),
                Err(why) => {
                    warn!("the ingestion worker returned a model the hub refuses: {why:?}");
                    Err(HubError::Invalid("the file could not be processed".into()))
                }
            }
        })
        .await
        .map_err(|_| HubError::Internal)?
    }

    /// Ingest an upload for `account` (MODELS.md 6.2). `begin_upload` and `precheck` come
    /// first; the body is complete.
    pub async fn upload(
        &self,
        account: AccountId,
        frame: u8,
        upload: Vec<u8>,
    ) -> Result<(ModelId, ModelStatus), HubError> {
        let source_hash = model_id(&upload);
        let source_bytes = upload.len() as i32;
        // Known already: no second look (MODELS.md 10), and no second ingestion.
        if let Some(answer) = self.known(account, &source_hash, frame).await? {
            return answer;
        }
        // A worker, or a short wait for one. The wait costs nobody a worker, and `Busy` is
        // not the uploader's doing: the hourly token goes back.
        let (report, accepted) = {
            let _worker = match tokio::time::timeout(self.queue_wait, self.permits.acquire()).await
            {
                Ok(Ok(permit)) => permit,
                _ => {
                    self.refund_upload(account);
                    return Err(HubError::Busy);
                }
            };
            self.run_ingest(upload.clone(), frame).await?
        };
        let Some(a) = accepted else {
            let mut text = report.violations.join("\n");
            if text.is_empty() {
                text = "the file could not be processed".into();
            }
            return Err(HubError::Invalid(text));
        };
        let id = a.id;
        // Files first: a row never names bytes that are not there. And files that no row
        // came to name (the upload was refused at the last step) do not stay.
        let _files = self.files.lock().await;
        self.store.put_model(&id, &a.gmm, &a.preview).map_err(io)?;
        self.store.put_source(&source_hash, &upload).map_err(io)?;
        let recorded = self
            .record(account, frame, &a, &source_hash, source_bytes)
            .await;
        if recorded.is_err() {
            self.sweep(&id, &source_hash).await;
        }
        recorded
    }

    /// Delete the files of `id` and of the upload `source` that no row names. The caller
    /// holds `files`. When the database cannot be asked, the files stay: unnamed bytes are
    /// litter, a row without its bytes is a broken model.
    async fn sweep(&self, id: &ModelId, source: &ModelId) {
        let named = sqlx::query(
            "select exists (select 1 from models where hash = $1) as model, \
             exists (select 1 from models where source_hash = $2) as source",
        )
        .bind(id.as_slice())
        .bind(source.as_slice())
        .fetch_one(&self.pool)
        .await
        .and_then(|r| {
            Ok((
                r.try_get::<bool, _>("model")?,
                r.try_get::<bool, _>("source")?,
            ))
        });
        match named {
            Ok((model, upload)) => {
                if !model {
                    self.store.remove(id);
                }
                if !upload {
                    self.store.remove_source(source);
                }
            }
            Err(e) => warn!("model store: could not sweep {}: {e}", id_hex(id)),
        }
    }

    /// The rows of an ingested upload: the model, its holder, the event.
    async fn record(
        &self,
        account: AccountId,
        frame: u8,
        a: &Accepted,
        source_hash: &ModelId,
        source_bytes: i32,
    ) -> Result<(ModelId, ModelStatus), HubError> {
        let id = a.id;
        // One lock order everywhere: the upload's model rows, then the account (a moderator's
        // decision takes the rows and then strikes the holders).
        let mut tx = self.begin().await?;
        // Uploads of one file are serialised, whatever their frames: while no row exists
        // there is nothing for `for update` to lock, and two of them would each decide the
        // status alone (MODELS.md 6.1: one upload, one status).
        Self::lock_upload(&mut tx, source_hash).await?;
        // Another frame of the same upload decides the status; otherwise trust does.
        let sibling: Option<String> =
            sqlx::query("select status from models where source_hash = $1 limit 1 for update")
                .bind(source_hash.as_slice())
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal)?
                .map(|r| r.try_get("status"))
                .transpose()
                .map_err(internal)?;
        let u = Self::uploader(&mut tx, account).await?;
        let (held, _) = Self::held(&mut tx, account).await?;
        if held >= u.slots as i64 {
            return Err(HubError::Full);
        }
        let status = match sibling.as_deref().and_then(ModelStatus::from_name) {
            Some(ModelStatus::Rejected | ModelStatus::Takedown) => {
                return Err(HubError::Invalid("this upload was refused before".into()));
            }
            Some(s) => s,
            None if u.trust_tier >= TRUSTED_TIER => ModelStatus::Active,
            None => ModelStatus::Pending,
        };
        sqlx::query(
            "insert into models (hash, frame, status, bytes, triangles, vertices, tex_w, tex_h, source_hash, source_bytes, uploaded_by) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) on conflict do nothing",
        )
        .bind(id.as_slice())
        .bind(frame as i16)
        .bind(status.name())
        .bind(a.gmm.len() as i32)
        .bind(a.triangles)
        .bind(a.vertices)
        .bind(a.texture[0])
        .bind(a.texture[1])
        .bind(source_hash.as_slice())
        .bind(source_bytes)
        .bind(account)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        // Two uploads of the same file at once: the second finds the first's row.
        let status: String = sqlx::query("select status from models where hash = $1")
            .bind(id.as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?
            .try_get("status")
            .map_err(internal)?;
        let status = ModelStatus::from_name(&status).ok_or(HubError::Internal)?;
        sqlx::query(
            "insert into model_holders (hash, account_id, tos_version) values ($1, $2, $3) on conflict do nothing",
        )
        .bind(id.as_slice())
        .bind(account)
        .bind(TOS_VERSION as i16)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        event(
            &mut tx,
            Some(&id),
            account,
            "uploaded",
            &format!(
                "frame {frame} status {} source {}",
                status.name(),
                id_hex(source_hash)
            ),
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        info!(account, model = %id_hex(&id), status = status.name(), "model uploaded");
        Ok((id, status))
    }

    /// An upload the hub has seen before: hold the existing model, or say why not.
    #[allow(clippy::type_complexity)]
    async fn known(
        &self,
        account: AccountId,
        source_hash: &ModelId,
        frame: u8,
    ) -> Result<Option<Result<(ModelId, ModelStatus), HubError>>, HubError> {
        let mut tx = self.begin().await?;
        let rows = sqlx::query(
            "select hash, frame, status, reason_code, reason from models where source_hash = $1 order by hash for update",
        )
        .bind(source_hash.as_slice())
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        let u = Self::uploader(&mut tx, account).await?;
        let mut same_frame = None;
        for r in &rows {
            let status: String = r.try_get("status").map_err(internal)?;
            let status = ModelStatus::from_name(&status).ok_or(HubError::Internal)?;
            if matches!(status, ModelStatus::Rejected | ModelStatus::Takedown) {
                let code: String = r.try_get("reason_code").map_err(internal)?;
                let reason: String = r.try_get("reason").map_err(internal)?;
                return Ok(Some(Err(HubError::Invalid(format!(
                    "this upload was refused ({code}): {reason}"
                )))));
            }
            if r.try_get::<i16, _>("frame").map_err(internal)? as u8 == frame {
                same_frame = Some((id_of(r.try_get("hash").map_err(internal)?)?, status));
            }
        }
        let Some((id, status)) = same_frame else {
            return Ok(None);
        };
        let holds: bool =
            sqlx::query("select 1 from model_holders where hash = $1 and account_id = $2")
                .bind(id.as_slice())
                .bind(account)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal)?
                .is_some();
        if !holds {
            let (held, _) = Self::held(&mut tx, account).await?;
            if held >= u.slots as i64 {
                return Ok(Some(Err(HubError::Full)));
            }
            sqlx::query(
                "insert into model_holders (hash, account_id, tos_version) values ($1, $2, $3)",
            )
            .bind(id.as_slice())
            .bind(account)
            .bind(TOS_VERSION as i16)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            event(
                &mut tx,
                Some(&id),
                account,
                "held",
                "an upload the hub already had",
            )
            .await?;
        }
        tx.commit().await.map_err(internal)?;
        Ok(Some(Ok((id, status))))
    }

    pub async fn list(&self, account: AccountId) -> Result<Vec<ModelSummary>, HubError> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "select {SUMMARY_COLUMNS} from model_holders h join models m on m.hash = h.hash \
             where h.account_id = $1 order by h.added"
        )))
        .bind(account)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter().map(summary).collect()
    }

    /// Stop holding a model: the account's characters stop wearing it; a model nobody holds
    /// and nobody has reviewed is deleted.
    pub async fn drop_model(&self, account: AccountId, model: &ModelId) -> Result<(), HubError> {
        let _files = self.files.lock().await;
        let mut tx = self.begin().await?;
        let row = sqlx::query("select status, source_hash from models where hash = $1 for update")
            .bind(model.as_slice())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?;
        let (status, source) = match &row {
            Some(r) => (
                Some(r.try_get::<String, _>("status").map_err(internal)?),
                Some(id_of(r.try_get("source_hash").map_err(internal)?)?),
            ),
            None => (None, None),
        };
        // What is worn does not change in the world (MODELS.md 12): the account's characters
        // wearing it are locked, and none of them may be in a zone, where everybody would
        // keep seeing a model the hub says nobody wears.
        let wearers = sqlx::query(
            "select location_kind from characters where model = $1 and account_id = $2 for update",
        )
        .bind(model.as_slice())
        .bind(account)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal)?;
        for w in &wearers {
            if w.try_get::<String, _>("location_kind").map_err(internal)? != "offline" {
                return Err(HubError::Invalid(
                    "a character wearing this model is in the world; log it out first".into(),
                ));
            }
        }
        let n = sqlx::query("delete from model_holders where hash = $1 and account_id = $2")
            .bind(model.as_slice())
            .bind(account)
            .execute(&mut *tx)
            .await
            .map_err(internal)?
            .rows_affected();
        if n == 0 {
            return Err(HubError::NotFound);
        }
        sqlx::query("update characters set model = null where model = $1 and account_id = $2")
            .bind(model.as_slice())
            .bind(account)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
        event(&mut tx, Some(model), account, "dropped", "").await?;
        let left: i64 = sqlx::query("select count(*) from model_holders where hash = $1")
            .bind(model.as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?
            .try_get(0)
            .map_err(internal)?;
        let orphan = left == 0 && status.as_deref() == Some("pending");
        if orphan {
            sqlx::query("delete from models where hash = $1")
                .bind(model.as_slice())
                .execute(&mut *tx)
                .await
                .map_err(internal)?;
        }
        tx.commit().await.map_err(internal)?;
        if let (true, Some(source)) = (orphan, source) {
            // The model's files, and the upload itself unless another frame of it remains.
            self.sweep(model, &source).await;
        }
        Ok(())
    }

    /// What an offline character wears. The model must be active, held by the account and
    /// ingested for the character's frame.
    pub async fn set_model(
        &self,
        account: AccountId,
        character: CharacterId,
        frame: u8,
        model: Option<&ModelId>,
    ) -> Result<(), HubError> {
        let mut tx = self.begin().await?;
        if let Some(id) = model {
            // A shared lock on the model row: a takedown waits for us, or we see it.
            let r = sqlx::query(
                "select m.status, m.frame from models m join model_holders h on h.hash = m.hash \
                 where m.hash = $1 and h.account_id = $2 for share of m",
            )
            .bind(id.as_slice())
            .bind(account)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?
            .ok_or(HubError::NotFound)?;
            let status: String = r.try_get("status").map_err(internal)?;
            if status != "active" {
                return Err(HubError::Invalid(format!("the model is {status}")));
            }
            let model_frame = r.try_get::<i16, _>("frame").map_err(internal)? as u8;
            if model_frame != frame {
                return Err(HubError::Invalid(format!(
                    "the model is for a {}; the character is a {}",
                    rig::frame_from_index(model_frame).map_or("?", rig::frame_name),
                    rig::frame_from_index(frame).map_or("?", rig::frame_name),
                )));
            }
        }
        let n = sqlx::query(
            "update characters set model = $1, updated = now() where id = $2 and account_id = $3 and location_kind = 'offline'",
        )
        .bind(model.map(|m| m.as_slice()))
        .bind(character)
        .bind(account)
        .execute(&mut *tx)
        .await
        .map_err(internal)?
        .rows_affected();
        if n == 0 {
            return Err(HubError::NotFound);
        }
        tx.commit().await.map_err(internal)
    }

    /// The model a character wears, while it is active: what a claiming zone is told.
    pub async fn worn(&self, character: CharacterId) -> Result<Option<ModelRef>, HubError> {
        let r = sqlx::query(
            "select m.hash, m.frame from characters c join models m on m.hash = c.model \
             where c.id = $1 and m.status = 'active'",
        )
        .bind(character)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        r.map(|r| {
            Ok(ModelRef {
                id: id_of(r.try_get("hash").map_err(internal)?)?,
                frame: r.try_get::<i16, _>("frame").map_err(internal)? as u8,
            })
        })
        .transpose()
    }

    /// The model's bytes for a session (MODELS.md 6.2): active to anyone, pending and rejected
    /// to holders and moderators, taken down to moderators only.
    pub async fn get(
        &self,
        session: SessionId,
        account: AccountId,
        model: &ModelId,
    ) -> Result<Vec<u8>, HubError> {
        let r = sqlx::query(
            "select m.status, m.bytes, exists (select 1 from model_holders h where h.hash = m.hash and h.account_id = $2) as holds \
             from models m where m.hash = $1",
        )
        .bind(model.as_slice())
        .bind(account)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or(HubError::NotFound)?;
        let status: String = r.try_get("status").map_err(internal)?;
        let holds: bool = r.try_get("holds").map_err(internal)?;
        let bytes: i32 = r.try_get("bytes").map_err(internal)?;
        let allowed = match status.as_str() {
            "active" => true,
            "pending" | "rejected" => holds || self.is_moderator(account).await?,
            _ => self.is_moderator(account).await?,
        };
        if !allowed {
            return Err(if status == "takedown" {
                HubError::Gone
            } else {
                HubError::NotFound
            });
        }
        {
            let mut buckets = self.gets.lock().unwrap();
            let now = Instant::now();
            if buckets.len() > 4096 {
                buckets.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(600));
            }
            let b = buckets.entry(session).or_insert(Bucket {
                tokens: GET_BURST_BYTES,
                last: now,
            });
            if !b.take(GET_BYTES_PER_SEC, GET_BURST_BYTES, bytes as f64) {
                return Err(HubError::Busy);
            }
        }
        let id = *model;
        let path = self.store.model_path(&id);
        tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .map_err(|_| HubError::Internal)?
            .map_err(io)
    }

    pub fn preview(&self, model: &ModelId) -> Result<Vec<u8>, HubError> {
        self.store.preview(model).map_err(|_| HubError::NotFound)
    }

    /// Pending models, oldest first.
    pub async fn queue(&self, limit: u32) -> Result<Vec<ModEntry>, HubError> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "select {SUMMARY_COLUMNS}, a.email, \
             (select count(*) from model_holders h where h.hash = m.hash) as holders, \
             extract(epoch from (now() - m.uploaded))::float8 as waiting \
             from models m join accounts a on a.id = m.uploaded_by \
             where m.status = 'pending' order by m.uploaded, m.hash limit $1"
        )))
        .bind(limit.clamp(1, 200) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(|r| {
                Ok(ModEntry {
                    model: summary(r)?,
                    uploader: r.try_get("email").map_err(internal)?,
                    holders: r.try_get::<i64, _>("holders").map_err(internal)? as u32,
                    waiting_secs: r.try_get::<f64, _>("waiting").map_err(internal)?.max(0.0) as u64,
                })
            })
            .collect()
    }

    /// The upload's lock, held to the end of the transaction: taken before its model rows by
    /// everything that decides a status for the upload or adds a frame to it.
    async fn lock_upload(tx: &mut Tx<'_>, source_hash: &ModelId) -> Result<(), HubError> {
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(i64::from_le_bytes(
                source_hash[..8].try_into().expect("eight bytes"),
            ))
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        Ok(())
    }

    /// Every model ingested from the same upload as `model`, locked: `(hash, status, code)`.
    /// The upload's lock comes first: a frame being added at this moment is either in the
    /// list or waits for the decision (a row lock alone would not show a row inserted after
    /// the statement began).
    async fn siblings(
        tx: &mut Tx<'_>,
        model: &ModelId,
    ) -> Result<Vec<(ModelId, ModelStatus, ReasonCode)>, HubError> {
        let source = sqlx::query("select source_hash from models where hash = $1")
            .bind(model.as_slice())
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?
            .ok_or(HubError::NotFound)?;
        let source = id_of(source.try_get("source_hash").map_err(internal)?)?;
        Self::lock_upload(tx, &source).await?;
        let rows = sqlx::query(
            "select hash, status, reason_code from models where source_hash = $1 \
             order by hash for update",
        )
        .bind(source.as_slice())
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
        if rows.is_empty() {
            return Err(HubError::NotFound);
        }
        rows.iter()
            .map(|r| {
                let status: String = r.try_get("status").map_err(internal)?;
                let code: String = r.try_get("reason_code").map_err(internal)?;
                Ok((
                    id_of(r.try_get("hash").map_err(internal)?)?,
                    ModelStatus::from_name(&status).ok_or(HubError::Internal)?,
                    ReasonCode::from_name(&code).unwrap_or(ReasonCode::Other),
                ))
            })
            .collect()
    }

    /// Move the upload's models to `to` and record who decided and why.
    async fn set_status(
        tx: &mut Tx<'_>,
        ids: &[ModelId],
        to: ModelStatus,
        moderator: AccountId,
        code: ReasonCode,
        reason: &str,
    ) -> Result<(), HubError> {
        for id in ids {
            sqlx::query(
                "update models set status = $2, decided_by = $3, decided = now(), reason_code = $4, reason = $5 where hash = $1",
            )
            .bind(id.as_slice())
            .bind(to.name())
            .bind(moderator)
            .bind(code.name())
            .bind(reason)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        }
        Ok(())
    }

    /// A strike (or its return) for every account holding any of `ids`.
    async fn strike(tx: &mut Tx<'_>, ids: &[ModelId], delta: i16) -> Result<(), HubError> {
        let hashes: Vec<&[u8]> = ids.iter().map(|i| i.as_slice()).collect();
        sqlx::query(
            "update accounts set upload_strikes = greatest(0, upload_strikes + $2) where id in \
             (select distinct account_id from model_holders where hash = any($1))",
        )
        .bind(&hashes)
        .bind(delta)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        // A strike is reputation too (ANTICHEAT.md 6), and its return gives it back: a row
        // each way, the sum moved in the same transaction.
        let (kind, points) = if delta > 0 {
            ("model_strike", crate::conduct::MODEL_STRIKE * delta as i32)
        } else {
            (
                "model_strike_returned",
                -crate::conduct::MODEL_STRIKE * (-delta) as i32,
            )
        };
        sqlx::query(
            "with holders as (select distinct account_id from model_holders where hash = any($1)),                   rows as (insert into reputation (account_id, kind, delta, reference)                            select account_id, $2, $3, 'model' from holders returning account_id)              update accounts set reputation = reputation + $3 where id in (select account_id from rows)",
        )
        .bind(&hashes)
        .bind(kind)
        .bind(points)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        Ok(())
    }

    /// `pending → active | rejected` for the whole upload.
    pub async fn decide(
        &self,
        moderator: AccountId,
        model: &ModelId,
        approve: bool,
        code: ReasonCode,
        reason: &str,
    ) -> Result<(), HubError> {
        if !approve && code == ReasonCode::None {
            return Err(HubError::Invalid("a rejection needs a reason code".into()));
        }
        let mut tx = self.begin().await?;
        let siblings = Self::siblings(&mut tx, model).await?;
        if siblings.iter().any(|(_, s, _)| *s != ModelStatus::Pending) {
            return Err(HubError::Invalid("the model is not pending".into()));
        }
        let ids: Vec<ModelId> = siblings.iter().map(|s| s.0).collect();
        let (to, code) = if approve {
            (ModelStatus::Active, ReasonCode::None)
        } else {
            (ModelStatus::Rejected, code)
        };
        Self::set_status(&mut tx, &ids, to, moderator, code, reason).await?;
        if code.is_strike() {
            Self::strike(&mut tx, &ids, 1).await?;
        }
        for id in &ids {
            event(
                &mut tx,
                Some(id),
                moderator,
                if approve { "approved" } else { "rejected" },
                &format!("{} {reason}", code.name()),
            )
            .await?;
        }
        tx.commit().await.map_err(internal)
    }

    /// `active → takedown` for the whole upload; nobody wears it afterwards. Returns the ids
    /// the zones must be told about.
    pub async fn takedown(
        &self,
        moderator: AccountId,
        model: &ModelId,
        code: ReasonCode,
        reason: &str,
        reference: &str,
    ) -> Result<Vec<ModelId>, HubError> {
        if code == ReasonCode::None {
            return Err(HubError::Invalid("a takedown needs a reason code".into()));
        }
        let mut tx = self.begin().await?;
        let siblings = Self::siblings(&mut tx, model).await?;
        if siblings.iter().any(|(_, s, _)| *s != ModelStatus::Active) {
            return Err(HubError::Invalid("the model is not active".into()));
        }
        let ids: Vec<ModelId> = siblings.iter().map(|s| s.0).collect();
        Self::set_status(
            &mut tx,
            &ids,
            ModelStatus::Takedown,
            moderator,
            code,
            reason,
        )
        .await?;
        let hashes: Vec<&[u8]> = ids.iter().map(|i| i.as_slice()).collect();
        let worn = sqlx::query("update characters set model = null where model = any($1)")
            .bind(hashes)
            .execute(&mut *tx)
            .await
            .map_err(internal)?
            .rows_affected();
        if code.is_strike() {
            Self::strike(&mut tx, &ids, 1).await?;
        }
        for id in &ids {
            event(
                &mut tx,
                Some(id),
                moderator,
                "takedown",
                &format!(
                    "{} {reason} [notice: {reference}] worn by {worn}",
                    code.name()
                ),
            )
            .await?;
        }
        tx.commit().await.map_err(internal)?;
        info!(moderator, model = %id_hex(model), worn, "model taken down");
        Ok(ids)
    }

    /// `takedown → active` (a counter-notice): the strike goes back; nobody wears the model
    /// until they choose to.
    pub async fn reinstate(
        &self,
        moderator: AccountId,
        model: &ModelId,
        reason: &str,
    ) -> Result<(), HubError> {
        let mut tx = self.begin().await?;
        let siblings = Self::siblings(&mut tx, model).await?;
        if siblings.iter().any(|(_, s, _)| *s != ModelStatus::Takedown) {
            return Err(HubError::Invalid("the model is not taken down".into()));
        }
        let ids: Vec<ModelId> = siblings.iter().map(|s| s.0).collect();
        let was_strike = siblings.iter().any(|(_, _, c)| c.is_strike());
        Self::set_status(
            &mut tx,
            &ids,
            ModelStatus::Active,
            moderator,
            ReasonCode::None,
            "",
        )
        .await?;
        if was_strike {
            Self::strike(&mut tx, &ids, -1).await?;
        }
        for id in &ids {
            event(&mut tx, Some(id), moderator, "reinstated", reason).await?;
        }
        tx.commit().await.map_err(internal)
    }

    /// An account-level moderation change by email; `sql` sets the column from `$2`.
    async fn account_change<T>(
        &self,
        moderator: AccountId,
        email: &str,
        sql: &'static str,
        value: T,
        what: &str,
    ) -> Result<(), HubError>
    where
        T: for<'q> sqlx::Encode<'q, Postgres>
            + sqlx::Type<Postgres>
            + Send
            + std::fmt::Display
            + Copy
            + 'static,
    {
        let mut tx = self.begin().await?;
        let n = sqlx::query(sql)
            .bind(email.trim().to_lowercase())
            .bind(value)
            .execute(&mut *tx)
            .await
            .map_err(internal)?
            .rows_affected();
        if n == 0 {
            return Err(HubError::NotFound);
        }
        event(
            &mut tx,
            None,
            moderator,
            what,
            &format!("{email} = {value}"),
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    pub async fn set_upload(
        &self,
        moderator: AccountId,
        email: &str,
        allow: bool,
    ) -> Result<(), HubError> {
        self.account_change(
            moderator,
            email,
            "update accounts set upload_privileges = $2 where email = $1",
            allow,
            "upload_privileges",
        )
        .await
    }

    pub async fn set_trust(
        &self,
        moderator: AccountId,
        email: &str,
        tier: u8,
    ) -> Result<(), HubError> {
        self.account_change(
            moderator,
            email,
            "update accounts set trust_tier = $2 where email = $1",
            tier.min(9) as i16,
            "trust_tier",
        )
        .await
    }

    pub async fn clear_strikes(&self, moderator: AccountId, email: &str) -> Result<(), HubError> {
        self.account_change(
            moderator,
            email,
            "update accounts set upload_strikes = $2 where email = $1",
            0i16,
            "upload_strikes",
        )
        .await
    }
}

/// Make `email` a moderator (the bootstrap: `gm-hub --grant-moderator EMAIL`).
pub async fn grant_moderator(pool: &PgPool, email: &str) -> anyhow::Result<bool> {
    let n = sqlx::query("update accounts set moderator = true where email = $1")
        .bind(email.trim().to_lowercase())
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n > 0)
}

/// Default number of slots, for documentation and tests.
pub const MODEL_SLOTS: i16 = DEFAULT_MODEL_SLOTS;

/// A verified model, with the facts the database keeps about it (all read from the bytes).
struct Accepted {
    id: ModelId,
    gmm: Vec<u8>,
    preview: Vec<u8>,
    triangles: i32,
    vertices: i32,
    texture: [i16; 2],
}

/// A worker's answer: its report and, when it accepted the upload, the `.gmm` it wrote.
type WorkerAnswer = (Report, Option<Vec<u8>>);

/// Run one ingestion in a child process: a scratch directory, the wall clock, and whatever
/// the child wrote. A child that crashes, hangs or leaves no report refuses the upload.
fn worker(
    exe: &Path,
    upload: &[u8],
    frame: gm_core::vocab::ArchetypeFrame,
    job: u64,
    timeout: Duration,
) -> Result<WorkerAnswer, HubError> {
    let dir = std::env::temp_dir().join(format!("gm-hub-ingest-{}-{job}", std::process::id()));
    let result = (|| -> std::io::Result<Option<WorkerAnswer>> {
        std::fs::create_dir_all(&dir)?;
        let input = dir.join("upload.glb");
        std::fs::write(&input, upload)?;
        let mut child = std::process::Command::new(exe)
            .arg("ingest-worker")
            .arg("--frame")
            .arg(rig::frame_name(frame))
            .arg("--in")
            .arg(&input)
            .arg("--out")
            .arg(&dir)
            // Few threads and few malloc arenas: the address-space limit is on virtual memory.
            .env("RAYON_NUM_THREADS", "4")
            .env("MALLOC_ARENA_MAX", "2")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                warn!("ingestion worker timed out after {timeout:?}");
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        if !status.success() {
            warn!("ingestion worker ended with {status}");
            return Ok(None);
        }
        let Ok(json) = std::fs::read(dir.join(gm_ingest::WORKER_REPORT)) else {
            return Ok(None);
        };
        let Ok(report) = serde_json::from_slice::<Report>(&json) else {
            return Ok(None);
        };
        // Never more than a model may be: the file is the worker's, the limit is ours.
        let gmm = if report.ok {
            let path = dir.join(gm_ingest::WORKER_MODEL);
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file() && m.len() <= gm_model::limits::MAX_FILE_BYTES as u64)
            {
                Some(std::fs::read(path)?)
            } else {
                return Ok(None);
            }
        } else {
            None
        };
        Ok(Some((report, gmm)))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Ok(Some(r)) => Ok(r),
        Ok(None) => Err(HubError::Invalid("the file could not be processed".into())),
        Err(e) => Err(io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refund_gives_back_one_upload_and_never_more_than_the_hour_holds() {
        let (rate, burst) = (UPLOADS_PER_HOUR / 3600.0, UPLOADS_PER_HOUR);
        let mut b = Bucket {
            tokens: burst,
            last: Instant::now(),
        };
        for _ in 0..10 {
            assert!(b.take(rate, burst, 1.0));
        }
        assert!(!b.take(rate, burst, 1.0), "ten an hour");
        // The hub was busy: the upload did not run and does not count.
        b.refund(burst, 1.0);
        assert!(b.take(rate, burst, 1.0));
        assert!(!b.take(rate, burst, 1.0));
        for _ in 0..100 {
            b.refund(burst, 1.0);
        }
        assert!(b.tokens <= burst);
    }
}
