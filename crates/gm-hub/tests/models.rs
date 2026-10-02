//! Models over the hub protocol (MODELS.md 6, 10, 11): upload, moderation, wearing, takedown
//! and the limits, against a real Postgres and the real ingestion worker (this crate's own
//! binary). Needs `GM_TEST_DATABASE_URL`; without it the test is skipped.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gm_core::tick::TickRate;
use gm_core::vocab::ArchetypeFrame;
use gm_hub::protocol::{
    BuildChoice, CharacterId, HubError, HubNotice, HubRequest, HubResponse, MAX_MODEL_BYTES,
    MAX_MODEL_UPLOAD_BYTES, ModOp, ModelId, ModelRef, ModelStatus, ModelSummary, ReasonCode,
    SessionId, TOS_VERSION,
};
use gm_hub::{Db, HubClient, HubClientError, HubConfig, HubKey, IngestMode};
use gm_ingest::synth;
use gm_net::transport::{Identity, hub_server_config};
use sqlx::Row;

/// The tests share one database, and with it one moderation queue: one at a time, each on
/// a wiped database.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The test database, migrated and wiped. Call with `SERIAL` held.
async fn database() -> Option<Db> {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return None;
    };
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    Some(db)
}

const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "models-test-secret";
const STRIKER: u8 = 1;
const CASTER: u8 = 2;

struct TestHub {
    addr: std::net::SocketAddr,
    cert: Vec<u8>,
    dir: PathBuf,
    _task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

async fn start_hub(db: &Db, ingest: IngestMode, timeout: Duration, tag: &str) -> TestHub {
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");
    let identity = Identity::generate(&[gm_hub::HUB_SERVER_NAME, "localhost"]).unwrap();
    let cert = identity.cert_der().to_vec();
    let endpoint = quinn::Endpoint::server(
        hub_server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let dir = std::env::temp_dir().join(format!("gm-hub-models-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let task = tokio::spawn(gm_hub::run(
        HubConfig {
            zone_secret: SECRET.into(),
            content,
            key: HubKey::generate(),
            session_secs: 3600,
            auth_per_minute: 1000.0,
            templates: Vec::new(),
            max_coin_grant: 10_000,
            models_dir: dir.clone(),
            ingest,
            ingest_timeout: timeout,
        },
        db.clone(),
        endpoint,
        std::future::pending(),
    ));
    TestHub {
        addr,
        cert,
        dir,
        _task: task,
    }
}

struct Account {
    client: HubClient,
    session: SessionId,
    email: String,
}

async fn register(hub: &TestHub, name: &str) -> Account {
    let client = HubClient::connect_with_cert(hub.addr, hub.cert.clone())
        .await
        .unwrap();
    let email = format!("{name}@example.test");
    let HubResponse::Session { session, .. } = client
        .request(&HubRequest::Register {
            email: email.clone(),
            password: "correct horse battery".into(),
        })
        .await
        .unwrap()
    else {
        panic!("register")
    };
    Account {
        client,
        session,
        email,
    }
}

fn refused<T: std::fmt::Debug>(r: Result<T, HubClientError>) -> HubError {
    match r {
        Err(HubClientError::Refused(e)) => e,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn upload_as(
    client: HubClient,
    session: SessionId,
    frame: u8,
    bytes: Vec<u8>,
) -> Result<(ModelId, ModelStatus), HubClientError> {
    let req = HubRequest::ModelUpload {
        session,
        frame,
        tos_version: TOS_VERSION,
        len: bytes.len() as u32,
    };
    match client.upload(&req, &bytes).await? {
        HubResponse::ModelAccepted { model, status } => Ok((model, status)),
        other => panic!("unexpected {other:?}"),
    }
}

impl Account {
    async fn upload(
        &self,
        frame: u8,
        bytes: &[u8],
    ) -> Result<(ModelId, ModelStatus), HubClientError> {
        upload_as(self.client.clone(), self.session, frame, bytes.to_vec()).await
    }

    async fn list(&self) -> Vec<ModelSummary> {
        match self
            .client
            .request(&HubRequest::ModelList {
                session: self.session,
            })
            .await
            .unwrap()
        {
            HubResponse::Models(m) => m,
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn get(&self, model: ModelId) -> Result<Vec<u8>, HubClientError> {
        self.client
            .download(
                &HubRequest::ModelGet {
                    session: self.session,
                    model,
                },
                MAX_MODEL_BYTES as usize,
            )
            .await
    }

    async fn moderate(&self, op: ModOp) -> Result<HubResponse, HubClientError> {
        self.client
            .request(&HubRequest::Mod {
                session: self.session,
                op,
            })
            .await
    }

    async fn character(&self, name: &str, preset: &str) -> CharacterId {
        match self
            .client
            .request(&HubRequest::CreateCharacter {
                session: self.session,
                name: name.into(),
                build: BuildChoice::Preset(preset.into()),
            })
            .await
            .unwrap()
        {
            HubResponse::Character(c) => c.id,
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn wear(
        &self,
        character: CharacterId,
        model: Option<ModelId>,
    ) -> Result<(), HubClientError> {
        self.client
            .ok(&HubRequest::SetModel {
                session: self.session,
                character,
                model,
            })
            .await
    }

    /// What the account's character wears according to the hub.
    async fn worn(&self, character: CharacterId) -> Option<ModelId> {
        match self
            .client
            .request(&HubRequest::Characters {
                session: self.session,
            })
            .await
            .unwrap()
        {
            HubResponse::Characters(list) => list.iter().find(|c| c.id == character).unwrap().model,
            other => panic!("unexpected {other:?}"),
        }
    }
}

async fn strikes(db: &Db, email: &str) -> i16 {
    sqlx::query("select upload_strikes from accounts where email = $1")
        .bind(email)
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap()
}

/// A distinct, small, valid upload for `frame`.
fn avatar(seed: u64, frame: ArchetypeFrame) -> Vec<u8> {
    synth::avatar(seed, frame, 64).build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn models_from_upload_to_takedown() {
    let _serial = SERIAL.lock().await;
    let Some(db) = database().await else {
        return;
    };
    let worker = IngestMode::Worker(PathBuf::from(env!("CARGO_BIN_EXE_gm-hub")));
    let hub = start_hub(&db, worker, gm_hub::models::INGEST_TIMEOUT, "main").await;

    // A zone, to claim characters and to hear notices.
    let zone = HubClient::connect_with_cert(hub.addr, hub.cert.clone())
        .await
        .unwrap();
    zone.request(&HubRequest::ZoneHello {
        secret: SECRET.into(),
        zone: "town".into(),
        map: "town".into(),
        map_hash: 0,
        addr: "127.0.0.1:9".parse().unwrap(),
        cert_der: vec![1, 2, 3],
        requires: Vec::new(),
    })
    .await
    .unwrap();
    let (notice_tx, mut notices) = tokio::sync::mpsc::unbounded_channel::<HubNotice>();
    {
        let zone = zone.clone();
        tokio::spawn(async move {
            while let Some(n) = zone.notice().await {
                let _ = notice_tx.send(n);
            }
        });
    }

    let creator = register(&hub, "creator").await;
    let stranger = register(&hub, "stranger").await;
    let moderator = register(&hub, "moderator").await;
    assert!(
        gm_hub::models::grant_moderator(db.pool(), &moderator.email)
            .await
            .unwrap()
    );
    let upload = avatar(1, ArchetypeFrame::Striker);

    // --- Who may upload, and what is refused before the body is read.
    assert_eq!(
        refused(creator.upload(STRIKER, &upload).await),
        HubError::Unauthorized
    );
    assert_eq!(
        refused(
            creator
                .moderate(ModOp::SetUpload {
                    email: creator.email.clone(),
                    allow: true
                })
                .await
        ),
        HubError::Unauthorized,
        "only a moderator grants uploads"
    );
    for who in [&creator, &stranger] {
        moderator
            .moderate(ModOp::SetUpload {
                email: who.email.clone(),
                allow: true,
            })
            .await
            .unwrap();
    }
    let wrong_terms = creator
        .client
        .upload(
            &HubRequest::ModelUpload {
                session: creator.session,
                frame: STRIKER,
                tos_version: TOS_VERSION + 1,
                len: upload.len() as u32,
            },
            &upload,
        )
        .await;
    assert!(matches!(refused(wrong_terms), HubError::Invalid(m) if m.contains("upload terms")));
    let too_big = creator
        .client
        .upload(
            &HubRequest::ModelUpload {
                session: creator.session,
                frame: STRIKER,
                tos_version: TOS_VERSION,
                len: MAX_MODEL_UPLOAD_BYTES + 1,
            },
            &[0u8; 16],
        )
        .await;
    assert!(matches!(refused(too_big), HubError::Invalid(_)));
    assert!(matches!(
        refused(creator.upload(9, &upload).await),
        HubError::Invalid(m) if m.contains("frame")
    ));

    // --- An upload is ingested by the worker and waits for a moderator.
    let (id, status) = creator.upload(STRIKER, &upload).await.unwrap();
    assert_eq!(status, ModelStatus::Pending);
    let (local, _) = gm_ingest::ingest(&upload, ArchetypeFrame::Striker);
    assert_eq!(
        gm_model::id_hex(&id),
        local.id,
        "the worker and the library agree on the bytes"
    );
    let listed = creator.list().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id, listed[0].status, listed[0].frame),
        (id, ModelStatus::Pending, STRIKER)
    );
    assert_eq!(listed[0].triangles, local.facts.triangles);
    // The same file again: the same model, no second row.
    assert_eq!(
        creator.upload(STRIKER, &upload).await.unwrap(),
        (id, ModelStatus::Pending)
    );
    assert_eq!(creator.list().await.len(), 1);

    // Pending: its holder and a moderator can fetch it, a stranger cannot, nobody wears it.
    let bytes = creator.get(id).await.unwrap();
    assert_eq!(gm_model::model_id(&bytes), id);
    assert!(gm_model::Model::decode(&bytes).is_ok());
    assert_eq!(moderator.get(id).await.unwrap(), bytes);
    assert_eq!(refused(stranger.get(id).await), HubError::NotFound);
    let hero = creator.character("Hero", "blade").await; // a striker
    let tank = creator.character("Tank", "ironclad").await; // a colossus
    assert!(matches!(
        refused(creator.wear(hero, Some(id)).await),
        HubError::Invalid(m) if m.contains("pending")
    ));

    // --- The queue, the preview, the decision.
    assert_eq!(
        refused(stranger.moderate(ModOp::Queue { limit: 10 }).await),
        HubError::Unauthorized
    );
    let HubResponse::ModQueue(queue) = moderator
        .moderate(ModOp::Queue { limit: 10 })
        .await
        .unwrap()
    else {
        panic!("queue")
    };
    assert_eq!(queue.len(), 1);
    assert_eq!(
        (
            queue[0].model.id,
            queue[0].uploader.as_str(),
            queue[0].holders
        ),
        (id, creator.email.as_str(), 1)
    );
    let preview = moderator
        .client
        .download(
            &HubRequest::Mod {
                session: moderator.session,
                op: ModOp::Preview { model: id },
            },
            MAX_MODEL_BYTES as usize,
        )
        .await
        .unwrap();
    assert_eq!(&preview[1..4], b"PNG");
    moderator
        .moderate(ModOp::Decide {
            model: id,
            approve: true,
            code: ReasonCode::None,
            reason: String::new(),
        })
        .await
        .unwrap();
    assert_eq!(creator.list().await[0].status, ModelStatus::Active);
    assert!(matches!(
        refused(
            moderator
                .moderate(ModOp::Decide {
                    model: id,
                    approve: false,
                    code: ReasonCode::Other,
                    reason: "twice".into()
                })
                .await
        ),
        HubError::Invalid(m) if m.contains("not pending")
    ));

    // --- Wearing: the right frame only, a holder only, and the zone hears of it at the claim.
    assert!(matches!(
        refused(creator.wear(tank, Some(id)).await),
        HubError::Invalid(m) if m.contains("striker") && m.contains("colossus")
    ));
    creator.wear(hero, Some(id)).await.unwrap();
    assert_eq!(creator.worn(hero).await, Some(id));
    let stray = stranger.character("Stray", "blade").await;
    assert_eq!(
        refused(stranger.wear(stray, Some(id)).await),
        HubError::NotFound,
        "an active model is not everyone's to wear"
    );
    assert_eq!(
        stranger.get(id).await.unwrap(),
        bytes,
        "but everyone may fetch it"
    );
    let HubResponse::Ticket(ticket) = creator
        .client
        .request(&HubRequest::Enter {
            session: creator.session,
            character: hero,
            zone: "town".into(),
        })
        .await
        .unwrap()
    else {
        panic!("enter")
    };
    let HubResponse::Claimed { model, state, .. } = zone
        .request(&HubRequest::Claim {
            token: ticket.token,
        })
        .await
        .unwrap()
    else {
        panic!("claim")
    };
    assert_eq!(model, Some(ModelRef { id, frame: STRIKER }));
    assert!(
        matches!(refused(creator.wear(hero, None).await), HubError::NotFound),
        "a character in a zone does not change clothes"
    );
    // Nor by dropping the model: everyone in the zone would keep seeing what the hub says
    // nobody wears.
    assert!(matches!(
        refused(
            creator
                .client
                .ok(&HubRequest::ModelDrop {
                    session: creator.session,
                    model: id,
                })
                .await
        ),
        HubError::Invalid(m) if m.contains("in the world")
    ));
    assert_eq!(creator.worn(hero).await, Some(id));

    // --- Takedown: one transaction, every zone told, nobody wears it, strangers get `Gone`.
    assert!(matches!(
        refused(
            stranger
                .moderate(ModOp::Takedown {
                    model: id,
                    code: ReasonCode::Copyright,
                    reason: String::new(),
                    reference: String::new()
                })
                .await
        ),
        HubError::Unauthorized
    ));
    moderator
        .moderate(ModOp::Takedown {
            model: id,
            code: ReasonCode::Copyright,
            reason: "a notice from the rights holder".into(),
            reference: "DSA-2026-0001".into(),
        })
        .await
        .unwrap();
    let notice = tokio::time::timeout(Duration::from_secs(5), notices.recv())
        .await
        .expect("the zone is told")
        .unwrap();
    assert_eq!(notice, HubNotice::ModelRevoked { model: id });
    assert_eq!(creator.worn(hero).await, None);
    assert_eq!(refused(stranger.get(id).await), HubError::Gone);
    assert_eq!(refused(creator.get(id).await), HubError::Gone);
    assert_eq!(
        moderator.get(id).await.unwrap(),
        bytes,
        "kept for the appeal, served to nobody else"
    );
    let mine = &creator.list().await[0];
    assert_eq!(
        (mine.status, mine.code),
        (ModelStatus::Takedown, ReasonCode::Copyright)
    );
    assert_eq!(mine.reason, "a notice from the rights holder");
    assert_eq!(strikes(&db, &creator.email).await, 1);
    // Never again: the same file is refused at once, for this frame and for any other.
    assert!(matches!(
        refused(creator.upload(STRIKER, &upload).await),
        HubError::Invalid(m) if m.contains("refused (copyright)")
    ));
    assert!(matches!(
        refused(stranger.upload(CASTER, &upload).await),
        HubError::Invalid(m) if m.contains("refused (copyright)")
    ));
    // The zone lets the character go; it is offline again.
    zone.ok(&HubRequest::Save {
        character: hero,
        state,
        leaving: true,
    })
    .await
    .unwrap();
    assert!(matches!(
        refused(creator.wear(hero, Some(id)).await),
        HubError::Invalid(m) if m.contains("takedown")
    ));

    // --- A counter-notice: back to active, the strike returned, nobody wearing it yet.
    moderator
        .moderate(ModOp::Reinstate {
            model: id,
            reason: "the uploader showed a licence".into(),
        })
        .await
        .unwrap();
    assert_eq!(strikes(&db, &creator.email).await, 0);
    assert_eq!(creator.list().await[0].status, ModelStatus::Active);
    assert_eq!(creator.worn(hero).await, None);
    creator.wear(hero, Some(id)).await.unwrap();

    // --- A second holder of the same file, another frame of it, and dropping.
    assert_eq!(
        stranger.upload(STRIKER, &upload).await.unwrap(),
        (id, ModelStatus::Active)
    );
    stranger.wear(stray, Some(id)).await.unwrap();
    let (caster_id, caster_status) = stranger.upload(CASTER, &upload).await.unwrap();
    assert_ne!(caster_id, id, "the frame is part of the bytes");
    assert_eq!(
        caster_status,
        ModelStatus::Active,
        "the upload was already approved"
    );
    creator
        .client
        .ok(&HubRequest::ModelDrop {
            session: creator.session,
            model: id,
        })
        .await
        .unwrap();
    assert_eq!(
        creator.worn(hero).await,
        None,
        "dropping a model takes it off"
    );
    assert!(creator.list().await.is_empty());
    assert_eq!(
        stranger.worn(stray).await,
        Some(id),
        "the other holder keeps wearing it"
    );
    assert_eq!(
        refused(
            creator
                .client
                .ok(&HubRequest::ModelDrop {
                    session: creator.session,
                    model: id
                })
                .await
        ),
        HubError::NotFound
    );
    // A takedown takes every frame of the upload with it.
    moderator
        .moderate(ModOp::Takedown {
            model: caster_id,
            code: ReasonCode::Other,
            reason: "both frames".into(),
            reference: String::new(),
        })
        .await
        .unwrap();
    let mut revoked = Vec::new();
    for _ in 0..2 {
        match tokio::time::timeout(Duration::from_secs(5), notices.recv()).await {
            Ok(Some(HubNotice::ModelRevoked { model })) => revoked.push(model),
            other => panic!("expected two revocations, got {other:?}"),
        }
    }
    revoked.sort();
    let mut both = vec![id, caster_id];
    both.sort();
    assert_eq!(revoked, both);
    assert_eq!(stranger.worn(stray).await, None);
    assert_eq!(
        strikes(&db, &stranger.email).await,
        0,
        "`other` is not a strike"
    );

    // --- What ingestion refuses comes back with its reasons, and leaves no model behind.
    let fat = gm_ingest::write::GlbBuilder::new(
        gm_model::mannequin::build(
            ArchetypeFrame::Striker,
            &gm_model::mannequin::Shape {
                sides: 30,
                ..gm_model::mannequin::Shape::detailed()
            },
        ),
        gm_ingest::write::png(64, 64, synth::paint(1, 64).as_raw()),
    )
    .build();
    assert!(matches!(
        refused(creator.upload(STRIKER, &fat).await),
        HubError::Invalid(m) if m.contains("triangles; the budget is 3500")
    ));
    let thin = gm_ingest::write::GlbBuilder::new(
        gm_model::mannequin::build(
            ArchetypeFrame::Striker,
            &gm_model::mannequin::Shape {
                girth: 0.3,
                shoulders: 0.4,
                ..gm_model::mannequin::Shape::detailed()
            },
        ),
        gm_ingest::write::png(64, 64, synth::paint(1, 64).as_raw()),
    )
    .build();
    assert!(matches!(
        refused(creator.upload(STRIKER, &thin).await),
        HubError::Invalid(m) if m.contains("visible where it can be hit")
    ));
    assert!(matches!(
        refused(creator.upload(STRIKER, b"this is not a model").await),
        HubError::Invalid(m) if m.contains("glTF 2.0 binary")
    ));
    assert!(creator.list().await.is_empty());

    // --- Rejection is a strike, and three strikes end uploading.
    let artist = register(&hub, "artist").await;
    moderator
        .moderate(ModOp::SetUpload {
            email: artist.email.clone(),
            allow: true,
        })
        .await
        .unwrap();
    for n in 0..3u64 {
        let bad = avatar(100 + n, ArchetypeFrame::Striker);
        let (bad_id, status) = artist.upload(STRIKER, &bad).await.unwrap();
        assert_eq!(status, ModelStatus::Pending);
        assert!(
            matches!(
                refused(
                    moderator
                        .moderate(ModOp::Decide {
                            model: bad_id,
                            approve: false,
                            code: ReasonCode::None,
                            reason: String::new()
                        })
                        .await
                ),
                HubError::Invalid(_)
            ),
            "a rejection states its reason"
        );
        moderator
            .moderate(ModOp::Decide {
                model: bad_id,
                approve: false,
                code: ReasonCode::Hateful,
                reason: "a hate symbol on the chest".into(),
            })
            .await
            .unwrap();
        assert_eq!(strikes(&db, &artist.email).await, n as i16 + 1);
        // The rejected upload cannot come back.
        if n == 0 {
            assert!(matches!(
                refused(artist.upload(STRIKER, &bad).await),
                HubError::Invalid(m) if m.contains("refused (hateful): a hate symbol")
            ));
        }
    }
    assert!(matches!(
        refused(artist.upload(STRIKER, &avatar(200, ArchetypeFrame::Striker)).await),
        HubError::Invalid(m) if m.contains("3 strikes")
    ));
    moderator
        .moderate(ModOp::ClearStrikes {
            email: artist.email.clone(),
        })
        .await
        .unwrap();

    // --- The pending cap, the trusted tier and the slots.
    let keen = register(&hub, "keen").await;
    moderator
        .moderate(ModOp::SetUpload {
            email: keen.email.clone(),
            allow: true,
        })
        .await
        .unwrap();
    let mut pending = Vec::new();
    for n in 0..3u64 {
        pending.push(
            keen.upload(STRIKER, &avatar(300 + n, ArchetypeFrame::Striker))
                .await
                .unwrap()
                .0,
        );
    }
    assert!(matches!(
        refused(keen.upload(STRIKER, &avatar(310, ArchetypeFrame::Striker)).await),
        HubError::Invalid(m) if m.contains("waiting for a moderator")
    ));
    // A pending model nobody holds any more leaves the queue and the store.
    let sources_before = std::fs::read_dir(hub.dir.join("src")).unwrap().count();
    keen.client
        .ok(&HubRequest::ModelDrop {
            session: keen.session,
            model: pending[0],
        })
        .await
        .unwrap();
    let HubResponse::ModQueue(queue) = moderator
        .moderate(ModOp::Queue { limit: 50 })
        .await
        .unwrap()
    else {
        panic!("queue")
    };
    assert!(queue.iter().all(|e| e.model.id != pending[0]));
    assert!(
        !hub.dir
            .join(format!("{}.gmm", gm_model::id_hex(&pending[0])))
            .exists()
    );
    // And so does the upload it came from: of the three, two remain.
    let sources = |dir: &std::path::Path| std::fs::read_dir(dir.join("src")).unwrap().count();
    assert_eq!(sources(&hub.dir), sources_before - 1);
    moderator
        .moderate(ModOp::SetTrust {
            email: keen.email.clone(),
            tier: 2,
        })
        .await
        .unwrap();
    for n in 0..2u64 {
        let (_, status) = keen
            .upload(STRIKER, &avatar(320 + n, ArchetypeFrame::Striker))
            .await
            .unwrap();
        assert_eq!(
            status,
            ModelStatus::Active,
            "a trusted creator skips the queue"
        );
    }
    assert_eq!(keen.list().await.len(), 4);
    assert_eq!(
        refused(
            keen.upload(STRIKER, &avatar(330, ArchetypeFrame::Striker))
                .await
        ),
        HubError::Full,
        "four slots"
    );

    // --- Two accounts, one file, at the same moment: one model, two holders.
    let (a, b) = (
        register(&hub, "twin-a").await,
        register(&hub, "twin-b").await,
    );
    for who in [&a, &b] {
        moderator
            .moderate(ModOp::SetUpload {
                email: who.email.clone(),
                allow: true,
            })
            .await
            .unwrap();
    }
    let same = avatar(400, ArchetypeFrame::Striker);
    let (ra, rb) = tokio::join!(a.upload(STRIKER, &same), b.upload(STRIKER, &same));
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    assert_eq!(ra.0, rb.0);
    let holders: i64 = sqlx::query("select count(*) from model_holders where hash = $1")
        .bind(ra.0.as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(holders, 2);

    // --- Two workers: five uploads at once queue for them and are all taken.
    let crowd: Vec<Account> = {
        let mut v = Vec::new();
        for i in 0..5 {
            let acc = register(&hub, &format!("crowd{i}")).await;
            moderator
                .moderate(ModOp::SetUpload {
                    email: acc.email.clone(),
                    allow: true,
                })
                .await
                .unwrap();
            v.push(acc);
        }
        v
    };
    let mut tasks = tokio::task::JoinSet::new();
    for (i, acc) in crowd.iter().enumerate() {
        let upload = avatar(500 + i as u64, ArchetypeFrame::Striker);
        tasks.spawn(upload_as(acc.client.clone(), acc.session, STRIKER, upload));
    }
    let mut results = Vec::new();
    while let Some(r) = tasks.join_next().await {
        results.push(r.unwrap());
    }
    assert!(results.iter().all(|r| r.is_ok()), "{results:?}");
    // One upload per account at a time: the second is told to come back.
    let twice = &crowd[0];
    let (one, two) = (
        avatar(520, ArchetypeFrame::Striker),
        avatar(521, ArchetypeFrame::Striker),
    );
    let (ra, rb) = tokio::join!(twice.upload(STRIKER, &one), twice.upload(STRIKER, &two));
    let busy = [&ra, &rb]
        .iter()
        .filter(|r| matches!(r, Err(HubClientError::Refused(HubError::Busy))))
        .count();
    assert_eq!(
        (busy, ra.is_ok() as u8 + rb.is_ok() as u8),
        (1, 1),
        "{ra:?} {rb:?}"
    );

    // --- Ten uploads an hour per account.
    let spammer = register(&hub, "spammer").await;
    moderator
        .moderate(ModOp::SetUpload {
            email: spammer.email.clone(),
            allow: true,
        })
        .await
        .unwrap();
    let mut limited = false;
    for i in 0..12 {
        match spammer
            .upload(STRIKER, format!("junk {i}").as_bytes())
            .await
        {
            Err(HubClientError::Refused(HubError::Invalid(_))) => {}
            Err(HubClientError::Refused(HubError::Busy)) => {
                assert!(i >= 10, "limited after {i} uploads");
                limited = true;
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(limited, "the hourly limit never answered");

    // --- The audit trail has every step.
    let events: Vec<String> =
        sqlx::query("select event from model_events where model = $1 order by id")
            .bind(id.as_slice())
            .fetch_all(db.pool())
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get("event").unwrap())
            .collect();
    assert_eq!(
        events,
        [
            "uploaded",
            "approved",
            "takedown",
            "reinstated",
            "held",
            "dropped",
            "takedown"
        ]
    );
    let _ = std::fs::remove_dir_all(&hub.dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_that_crashes_or_hangs_costs_one_upload() {
    let _serial = SERIAL.lock().await;
    let Some(db) = database().await else {
        return;
    };
    let scripts = std::env::temp_dir().join(format!("gm-hub-workers-{}", std::process::id()));
    std::fs::create_dir_all(&scripts).unwrap();
    let script = |name: &str, body: &str| -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = scripts.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let upload = avatar(900, ArchetypeFrame::Striker);
    // Workers that were subverted by the file they parsed (MODELS.md 6.2): each says "accepted"
    // and hands back something the checks would have refused. The hub believes the bytes, not
    // the report.
    let said_yes = scripts.join("yes.json");
    let yes = gm_ingest::Report {
        ok: true,
        ..Default::default()
    };
    std::fs::write(&said_yes, serde_json::to_vec(&yes).unwrap()).unwrap();
    let lie = |name: &str, model: &[u8]| -> PathBuf {
        let file = scripts.join(format!("{name}.gmm"));
        std::fs::write(&file, model).unwrap();
        script(
            &format!("{name}.sh"),
            &format!(
                "cp '{}' \"$7/model.gmm\" && cp '{}' \"$7/report.json\"",
                file.display(),
                said_yes.display()
            ),
        )
    };
    let honest = |frame| {
        gm_ingest::ingest(&avatar(901, frame), frame)
            .1
            .expect("accepted")
    };
    let mut tiny = honest(ArchetypeFrame::Striker).model;
    for s in &mut tiny.scale {
        *s *= 0.3;
    }
    for p in tiny.pivots.iter_mut().flatten() {
        *p *= 0.3;
    }
    // The control: the same kind of worker handing back a model that is what it says. The
    // hub takes it, under the id of the bytes, with a preview the hub drew itself (the worker
    // wrote none).
    {
        let truth = honest(ArchetypeFrame::Striker);
        let hub = start_hub(
            &db,
            IngestMode::Worker(lie("truth", &truth.gmm)),
            Duration::from_secs(30),
            "truth",
        )
        .await;
        let acc = register(&hub, &format!("control-{}", std::process::id())).await;
        sqlx::query("update accounts set upload_privileges = true where email = $1")
            .bind(&acc.email)
            .execute(db.pool())
            .await
            .unwrap();
        let (id, status) = acc
            .upload(STRIKER, &avatar(902, ArchetypeFrame::Striker))
            .await
            .expect("accepted");
        assert_eq!((id, status), (truth.id, ModelStatus::Pending));
        let files = walk(&hub.dir);
        assert!(
            files
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "gmm"))
        );
        let preview = files
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "png"))
            .expect("the hub drew a preview");
        assert_eq!(std::fs::read(preview).unwrap(), truth.preview);
        // Withdrawn again: the queue is the other test's too.
        acc.client
            .ok(&HubRequest::ModelDrop {
                session: acc.session,
                model: id,
            })
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&hub.dir);
    }
    for (tag, exe, timeout) in [
        (
            "lies-garbage",
            lie("garbage", b"GMM1 and then nothing a reader accepts"),
            Duration::from_secs(30),
        ),
        (
            "lies-frame",
            lie("frame", &honest(ArchetypeFrame::Colossus).gmm),
            Duration::from_secs(30),
        ),
        (
            "lies-envelope",
            lie("envelope", &tiny.encode().unwrap()),
            Duration::from_secs(30),
        ),
        (
            "lies-link",
            script(
                "link.sh",
                &format!(
                    "ln -s /etc/passwd \"$7/model.gmm\" && cp '{}' \"$7/report.json\"",
                    said_yes.display()
                ),
            ),
            Duration::from_secs(30),
        ),
        (
            "crash",
            script("crash.sh", "kill -SEGV $$"),
            Duration::from_secs(30),
        ),
        (
            "silent",
            script("silent.sh", "exit 0"),
            Duration::from_secs(30),
        ),
        (
            "hang",
            script("hang.sh", "sleep 600"),
            Duration::from_millis(400),
        ),
        (
            "missing",
            scripts.join("no-such-worker"),
            Duration::from_secs(30),
        ),
    ] {
        let hub = start_hub(&db, IngestMode::Worker(exe), timeout, tag).await;
        let acc = register(&hub, &format!("victim-{tag}-{}", std::process::id())).await;
        sqlx::query("update accounts set upload_privileges = true where email = $1")
            .bind(&acc.email)
            .execute(db.pool())
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let r = acc.upload(STRIKER, &upload).await;
        match (tag, refused(r)) {
            ("missing", HubError::Internal) => {}
            (_, HubError::Invalid(m)) if m.contains("could not be processed") => {}
            (tag, e) => panic!("{tag}: {e:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{tag} took {:?}",
            started.elapsed()
        );
        // The hub is still there, and nothing was stored: no row, no file.
        assert!(acc.list().await.is_empty());
        let stored = walk(&hub.dir)
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "gmm" || e == "png"))
            .count();
        assert_eq!(stored, 0, "{tag}: the hub kept what the worker wrote");
        let _ = std::fs::remove_dir_all(&hub.dir);
    }
    let _ = std::fs::remove_dir_all(&scripts);
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// One upload per account at a time, and a bounded number of bodies in memory.
#[tokio::test]
async fn upload_turns_are_bounded() {
    let _serial = SERIAL.lock().await;
    let Some(db) = database().await else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("gm-hub-turns-{}", std::process::id()));
    let models = gm_hub::models::Models::new(
        db.pool().clone(),
        &dir,
        IngestMode::InProcess,
        Duration::from_secs(30),
    )
    .unwrap();
    let first = models
        .begin_upload(1)
        .expect("a free account and a free slot");
    assert!(matches!(models.begin_upload(1), Err(HubError::Busy)));
    // Other accounts fill the remaining slots; the next one waits its turn.
    let others: Vec<_> = (2..=gm_hub::models::UPLOAD_SLOTS as i64)
        .map(|a| models.begin_upload(a).expect("a slot"))
        .collect();
    assert!(matches!(models.begin_upload(100), Err(HubError::Busy)));
    // A refused turn must not have taken the account's turn or a slot with it.
    drop(first);
    assert!(models.begin_upload(100).is_ok());
    assert!(models.begin_upload(1).is_ok());
    drop(others);
    let _ = std::fs::remove_dir_all(&dir);
}

/// MODELS.md 6.1: status belongs to the upload. Two frames of one file arriving at the same
/// moment from a trusted and an untrusted account, or a frame arriving while a moderator
/// decides on its sibling, must not end up with different statuses. This checks the outcome
/// under concurrency; the exact interleaving the upload's lock closes (a row inserted after a
/// decision's `select ... for update` began) cannot be forced from outside the hub.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_upload_has_one_status_whoever_races() {
    let _serial = SERIAL.lock().await;
    let Some(db) = database().await else {
        return;
    };
    let hub = start_hub(
        &db,
        IngestMode::InProcess,
        gm_hub::models::INGEST_TIMEOUT,
        "race",
    )
    .await;
    let tag = std::process::id();
    let moderator = register(&hub, &format!("race-moderator-{tag}")).await;
    assert!(
        gm_hub::models::grant_moderator(db.pool(), &moderator.email)
            .await
            .unwrap()
    );
    let statuses = |file: Vec<u8>| {
        let pool = db.pool().clone();
        async move {
            let source = gm_model::model_id(&file);
            let rows = sqlx::query("select status from models where source_hash = $1")
                .bind(source.as_slice())
                .fetch_all(&pool)
                .await
                .unwrap();
            rows.iter()
                .map(|r| r.try_get::<String, _>("status").unwrap())
                .collect::<Vec<_>>()
        }
    };
    for round in 0..6u64 {
        // Fresh accounts every round: slots and the pending cap stay out of the way.
        let mut accounts = Vec::new();
        for who in ["plain", "trusted", "second"] {
            let acc = register(&hub, &format!("race-{who}-{round}-{tag}")).await;
            moderator
                .moderate(ModOp::SetUpload {
                    email: acc.email.clone(),
                    allow: true,
                })
                .await
                .unwrap();
            accounts.push(acc);
        }
        let (plain, trusted, second) = (&accounts[0], &accounts[1], &accounts[2]);
        moderator
            .moderate(ModOp::SetTrust {
                email: trusted.email.clone(),
                tier: 2,
            })
            .await
            .unwrap();

        // A trusted and an untrusted account, one file, two frames, one moment.
        let file = avatar(7000 + round, ArchetypeFrame::Striker);
        let (a, b) = tokio::join!(plain.upload(STRIKER, &file), trusted.upload(CASTER, &file));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.1, b.1, "round {round}: both were told the same status");
        let seen = statuses(file).await;
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], seen[1], "round {round}: {seen:?}");

        // A second frame arriving while the moderator approves the first.
        let file = avatar(7100 + round, ArchetypeFrame::Striker);
        let (first, status) = plain.upload(STRIKER, &file).await.unwrap();
        assert_eq!(status, ModelStatus::Pending);
        let (decision, late) = tokio::join!(
            moderator.moderate(ModOp::Decide {
                model: first,
                approve: true,
                code: ReasonCode::None,
                reason: String::new(),
            }),
            second.upload(CASTER, &file)
        );
        decision.unwrap();
        late.unwrap();
        let seen = statuses(file).await;
        assert_eq!(seen, ["active", "active"], "round {round}");
    }
    let _ = std::fs::remove_dir_all(&hub.dir);
}
