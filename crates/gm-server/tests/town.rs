//! Phase 6 over the real protocol (MODELS.md 7, ECONOMY.md 7): a hub, the town zone, bots that
//! wear uploaded models and open stalls, a takedown in the middle and a late arrival. Needs
//! `GM_TEST_DATABASE_URL` (a Postgres the test may wipe); without it the test is skipped.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gm_bot::{Behaviour, BotConfig, HubFlowConfig, HubFlowReport, run_hub_flow};
use gm_core::tick::TickRate;
use gm_hub::protocol::{
    BuildChoice, HubRequest, HubResponse, ModOp, ModelId, ModelStatus, ReasonCode, TOS_VERSION,
};
use gm_hub::{Db, HubClient, HubConfig, HubKey, IngestMode};
use gm_model::rig;
use gm_net::transport::{Identity, hub_server_config, server_config};
use gm_server::{HubLink, HubLinkConfig, ZoneConfig, ZoneWorld};

const TOWN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/town.bsp"
);
const MAPS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/built");
const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "town-test-secret";
const PASSWORD: &str = "correct horse battery";
const PRESETS: [&str; 4] = ["ironclad", "blade", "frostweaver", "shade"];

async fn session(hub: &HubClient, email: &str) -> gm_hub::protocol::SessionId {
    match hub
        .request(&HubRequest::Register {
            email: email.into(),
            password: PASSWORD.into(),
        })
        .await
        .unwrap()
    {
        HubResponse::Session { session, .. } => session,
        other => panic!("{other:?}"),
    }
}

fn flow(
    hub: std::net::SocketAddr,
    cert: &[u8],
    i: usize,
    stall: Option<u32>,
    secs: u64,
) -> HubFlowConfig {
    HubFlowConfig {
        hub,
        hub_cert_der: cert.to_vec(),
        email: format!("avatar-{i}@bots.test"),
        password: PASSWORD.into(),
        register: true,
        character: format!("Avatar{i}"),
        preset: PRESETS[i % 4].into(),
        zone: "town".into(),
        travel_after: None,
        travel_to: None,
        maps_dir: MAPS_DIR.into(),
        bot: BotConfig {
            name: String::new(),
            seed: 100 + i as u64,
            behaviour: Behaviour::Stroll,
            rate: TickRate::COMBAT,
            run_ticks: 0,
            build: None,
            team: 0,
            counter_pick: false,
            travel_to: None,
            travel_after_ticks: 0,
            stall_tile: stall,
        },
        play: Duration::from_secs(secs),
        list_for_hire: None,
        hire: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn avatars_and_stalls_in_the_town() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
    const BOTS: usize = 6;
    // `GM_TRACE=info` shows what the hub, the zone and the bots log.
    if let Ok(filter) = std::env::var("GM_TRACE") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    }
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");

    // The hub.
    let identity = Identity::generate(&[gm_hub::HUB_SERVER_NAME, "localhost"]).unwrap();
    let hub_cert = identity.cert_der().to_vec();
    let endpoint = quinn::Endpoint::server(
        hub_server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let hub_addr = endpoint.local_addr().unwrap();
    let models_dir = std::env::temp_dir().join(format!("gm-town-models-{}", std::process::id()));
    let hub_task = tokio::spawn(gm_hub::run(
        HubConfig {
            zone_secret: SECRET.into(),
            content: content.clone(),
            key: HubKey::generate(),
            session_secs: 3600,
            auth_per_minute: 1000.0,
            templates: Vec::new(),
            max_coin_grant: 10_000,
            models_dir: models_dir.clone(),
            ingest: IngestMode::InProcess,
            ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
        },
        db.clone(),
        endpoint,
        std::future::pending(),
    ));

    // The town zone.
    let zone_identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(TOWN)).expect("town.bsp is built"));
    assert_eq!(world.stall_grids.len(), 1, "the town has a market");
    let zone_endpoint = quinn::Endpoint::server(
        server_config(&zone_identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let link = HubLink::connect(HubLinkConfig {
        addr: hub_addr,
        cert_der: hub_cert.clone(),
        zone: "town".into(),
        secret: SECRET.into(),
        map: world.name.clone(),
        map_hash: world.hash,
        public_addr: zone_endpoint.local_addr().unwrap(),
        zone_cert_der: zone_identity.cert_der().to_vec(),
        requires: Vec::new(),
    })
    .await
    .expect("the zone registers");
    let zone_task = tokio::spawn(gm_server::run(
        ZoneConfig {
            max_ticks: Some(64 * 60),
            content,
            hub: Some(link),
            ..ZoneConfig::default()
        },
        world,
        zone_endpoint,
        std::future::pending(),
    ));

    // A moderator, and six accounts each with a character wearing its own approved model.
    let hub = HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();
    let moderator = session(&hub, "moderator@example.test").await;
    assert!(
        gm_hub::models::grant_moderator(db.pool(), "moderator@example.test")
            .await
            .unwrap()
    );
    let mut models: Vec<ModelId> = Vec::new();
    for i in 0..BOTS {
        let email = format!("avatar-{i}@bots.test");
        let s = session(&hub, &email).await;
        hub.ok(&HubRequest::Mod {
            session: moderator,
            op: ModOp::SetUpload { email, allow: true },
        })
        .await
        .unwrap();
        let frame = rig::FRAMES[i % 4];
        let HubResponse::Character(c) = hub
            .request(&HubRequest::CreateCharacter {
                session: s,
                name: format!("Avatar{i}"),
                build: BuildChoice::Preset(PRESETS[i % 4].into()),
            })
            .await
            .unwrap()
        else {
            panic!("create")
        };
        let upload = gm_ingest::synth::avatar(i as u64, frame, 64).build();
        let HubResponse::ModelAccepted { model, status } = hub
            .upload(
                &HubRequest::ModelUpload {
                    session: s,
                    frame: rig::frame_index(frame),
                    tos_version: TOS_VERSION,
                    len: upload.len() as u32,
                },
                &upload,
            )
            .await
            .unwrap()
        else {
            panic!("upload")
        };
        assert_eq!(status, ModelStatus::Pending);
        hub.ok(&HubRequest::Mod {
            session: moderator,
            op: ModOp::Decide {
                model,
                approve: true,
                code: ReasonCode::None,
                reason: String::new(),
            },
        })
        .await
        .unwrap();
        hub.ok(&HubRequest::SetModel {
            session: s,
            character: c.id,
            model: Some(model),
        })
        .await
        .unwrap();
        models.push(model);
    }

    // Everyone walks into the town. Bots 0 and 1 both head for the first market tile: one
    // gets it, the other takes the next.
    let mut flows: Vec<tokio::task::JoinHandle<anyhow::Result<HubFlowReport>>> = Vec::new();
    for i in 0..BOTS {
        let stall = (i < 2).then_some(0);
        flows.push(tokio::spawn(run_hub_flow(flow(
            hub_addr, &hub_cert, i, stall, 14,
        ))));
    }
    // In the middle of it, the last bot's model is taken down.
    tokio::time::sleep(Duration::from_secs(5)).await;
    hub.ok(&HubRequest::Mod {
        session: moderator,
        op: ModOp::Takedown {
            model: models[BOTS - 1],
            code: ReasonCode::Copyright,
            reason: "a notice".into(),
            reference: "TEST-1".into(),
        },
    })
    .await
    .unwrap();
    // And somebody arrives late, wearing nothing.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let late = run_hub_flow(flow(hub_addr, &hub_cert, 99, None, 5))
        .await
        .expect("the late arrival plays");
    let late = &late.reports[0];
    assert_eq!(
        late.roster,
        BOTS + 1,
        "the roster lists everyone, in one message"
    );
    assert_eq!(
        late.models_seen,
        BOTS - 1,
        "the revoked model is not announced any more"
    );
    assert_eq!(late.revocations, 0);
    assert_eq!(late.own_model, None);
    assert_eq!(late.stalls_seen, 2, "a joiner is told about the market");

    let mut reports = Vec::new();
    for f in flows {
        reports.push(f.await.unwrap().expect("hub flow").reports.remove(0));
    }
    for (i, r) in reports.iter().enumerate() {
        println!(
            "{}: model {} models_seen {} revocations {} stall {} stalls_seen {} snapshots {} corrections {}",
            r.name,
            r.own_model.is_some(),
            r.models_seen,
            r.revocations,
            r.stall_opened,
            r.stalls_seen,
            r.client.snapshots,
            r.client.corrections
        );
        assert!(
            r.client.snapshots > 300,
            "{}: {} snapshots",
            r.name,
            r.client.snapshots
        );
        assert!(r.travelled > 100.0, "{} moved {:.0} u", r.name, r.travelled);
        assert_eq!(r.client.decode_errors, 0);
        // Everyone saw every model, heard the takedown, and nobody was announced wearing
        // the revoked one afterwards.
        assert_eq!(r.revocations, 1, "{}", r.name);
        assert_eq!(r.models_seen, BOTS, "{}", r.name);
        assert!(!r.revoked_still_worn, "{}", r.name);
        assert_eq!(r.roster, BOTS + 1, "{}", r.name);
        assert_eq!(
            r.own_model,
            (i != BOTS - 1).then_some(models[i]),
            "{}",
            r.name
        );
        assert_eq!(r.stalls_seen, 2, "{}", r.name);
        assert_eq!(r.stall_opened, i < 2, "{}", r.name);
    }

    // The stalls outlive their keepers (48 h), on two different tiles; the taken-down model
    // is on nobody.
    let econ = gm_hub::economy::Economy::new(db.pool().clone());
    let stalls = econ.stalls_in("town", None).await.unwrap();
    assert_eq!(stalls.len(), 2);
    assert_ne!((stalls[0].1, stalls[0].2), (stalls[1].1, stalls[1].2));
    assert!(
        stalls.iter().all(|s| s.6.is_some()),
        "keepers wear their models: {stalls:?}"
    );
    for (i, model) in models.iter().enumerate() {
        let HubResponse::Session { session: s, .. } = hub
            .request(&HubRequest::Login {
                email: format!("avatar-{i}@bots.test"),
                password: PASSWORD.into(),
            })
            .await
            .unwrap()
        else {
            panic!("login")
        };
        let HubResponse::Characters(list) = hub
            .request(&HubRequest::Characters { session: s })
            .await
            .unwrap()
        else {
            panic!("characters")
        };
        assert_eq!(list[0].model, (i != BOTS - 1).then_some(*model));
    }
    hub.close();
    hub_task.abort();
    zone_task.abort();
    let _ = std::fs::remove_dir_all(&models_dir);
}
