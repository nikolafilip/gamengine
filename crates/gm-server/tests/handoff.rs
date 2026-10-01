//! Phase 4 acceptance (PLAN.md 11.8, HUB.md 5): login → zone → handoff → logout over the real
//! protocol on loopback, with the character's location checked in the database at every step.
//! Needs `GM_TEST_DATABASE_URL` (a Postgres the test may wipe); without it the test is skipped.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gm_bot::{Behaviour, BotConfig, HubFlowConfig, run_hub_flow};
use gm_core::tick::TickRate;
use gm_hub::protocol::LocationSummary;
use gm_hub::{Db, HubConfig, HubKey};
use gm_net::transport::{Identity, hub_server_config, server_config};
use gm_server::{HubLink, HubLinkConfig, ZoneConfig, ZoneWorld};

const ARENA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/arena.bsp"
);
const MAPS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/built");
const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const BUDGETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../budgets.toml");
const SECRET: &str = "test-zone-secret";

fn hub_budget(key: &str) -> f64 {
    let text = std::fs::read_to_string(BUDGETS).expect("budgets.toml");
    let mut in_hub = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_hub = line == "[hub]";
            continue;
        }
        if in_hub
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return v
                .split('#')
                .next()
                .unwrap()
                .trim()
                .parse()
                .expect("numeric budget");
        }
    }
    panic!("budgets.toml has no [hub] {key}");
}

async fn start_zone(
    id: &str,
    hub_addr: std::net::SocketAddr,
    hub_cert: Vec<u8>,
    content: gm_core::build::ContentPack,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<gm_server::ZoneReport>>,
) {
    let identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(ARENA)).unwrap());
    let endpoint = quinn::Endpoint::server(
        server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let link = HubLink::connect(HubLinkConfig {
        addr: hub_addr,
        cert_der: hub_cert,
        zone: id.to_string(),
        secret: SECRET.into(),
        map: world.name.clone(),
        map_hash: world.hash,
        public_addr: addr,
        zone_cert_der: identity.cert_der().to_vec(),
    })
    .await
    .expect("zone registers");
    let cfg = ZoneConfig {
        max_ticks: Some(64 * 40),
        report_every: Duration::from_secs(5),
        content,
        hub: Some(link),
        ..ZoneConfig::default()
    };
    let task = tokio::spawn(gm_server::run(cfg, world, endpoint, std::future::pending()));
    (addr, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_zone_handoff_logout_round_trip() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
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
    let hub_cfg = HubConfig {
        zone_secret: SECRET.into(),
        content: content.clone(),
        key: HubKey::generate(),
        session_secs: 3600,
        auth_per_minute: 100.0,
    };
    let hub_task = tokio::spawn(gm_hub::run(
        hub_cfg,
        db.clone(),
        endpoint,
        std::future::pending(),
    ));

    // Two zones on the same map.
    let (_a, zone_a) = start_zone("arena-a", hub_addr, hub_cert.clone(), content.clone()).await;
    let (_b, zone_b) = start_zone("arena-b", hub_addr, hub_cert.clone(), content.clone()).await;

    // The trip: register, create a character, enter A, travel to B after 3 s, play 3 s more,
    // log out. Meanwhile the character's location is checked at the hub.
    let started = Instant::now();
    let flow = tokio::spawn(run_hub_flow(HubFlowConfig {
        hub: hub_addr,
        hub_cert_der: hub_cert.clone(),
        email: "pezo@example.com".into(),
        password: "correct horse battery".into(),
        register: true,
        character: "Marko".into(),
        preset: "blade".into(),
        zone: "arena-a".into(),
        travel_after: Some(Duration::from_secs(3)),
        travel_to: Some("arena-b".into()),
        maps_dir: MAPS_DIR.into(),
        bot: BotConfig {
            name: String::new(),
            seed: 5,
            behaviour: Behaviour::Wander,
            rate: TickRate::COMBAT,
            run_ticks: 0,
            build: None,
            team: 0,
            counter_pick: false,
            travel_to: None,
            travel_after_ticks: 0,
        },
        play: Duration::from_secs(7),
    }));

    // Poll the database for the expected locations (event-driven on the hub's side; the test
    // only reads).
    async fn wait_for(db: &Db, name: &str, want: &dyn Fn(&LocationSummary) -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let rows = db.characters_of(1).await.unwrap_or_default();
            if let Some(r) = rows.iter().find(|r| r.name == name)
                && want(&r.summary().location)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {rows:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    wait_for(
        &db,
        "Marko",
        &|l| matches!(l, LocationSummary::Zone(z) if z == "arena-a"),
        "claim in A",
    )
    .await;
    let t_in_a = started.elapsed();
    wait_for(
        &db,
        "Marko",
        &|l| matches!(l, LocationSummary::Zone(z) if z == "arena-b"),
        "claim in B",
    )
    .await;
    let t_in_b = started.elapsed();
    wait_for(
        &db,
        "Marko",
        &|l| matches!(l, LocationSummary::Offline),
        "logout",
    )
    .await;
    let t_offline = started.elapsed();

    let report = flow.await.unwrap().expect("hub flow");
    println!(
        "round trip: in A after {:.0} ms, in B after {:.0} ms, offline after {:.0} ms; login {:.0} ms enter {:.0} ms travel {:.0} ms logout {:.0} ms; zones {:?}",
        t_in_a.as_secs_f64() * 1000.0,
        t_in_b.as_secs_f64() * 1000.0,
        t_offline.as_secs_f64() * 1000.0,
        report.login_ms,
        report.enter_ms,
        report.travel_ms,
        report.logout_ms,
        report.zones
    );
    assert_eq!(
        report.zones.len(),
        2,
        "played in two zones: {:?}",
        report.zones
    );
    assert!(report.zones[0].starts_with("arena-a:"));
    assert!(report.zones[1].starts_with("arena-b:"));
    for r in &report.reports {
        assert!(
            r.client.snapshots > 60,
            "{}: {} snapshots",
            r.name,
            r.client.snapshots
        );
        assert_eq!(r.client.decode_errors, 0);
    }
    // The position persisted in B is where the bot was, not the spawn's origin.
    let rows = db.characters_of(1).await.unwrap();
    let marko = rows.iter().find(|r| r.name == "Marko").unwrap();
    assert_eq!(marko.summary().location, LocationSummary::Offline);
    assert!(
        marko.play_seconds >= 5,
        "play seconds {}",
        marko.play_seconds
    );
    // Budgets (HUB.md 5): the hub's part of the trip, excluding the seconds spent playing.
    let budget = hub_budget("round_trip_ms");
    let hub_ms = report.login_ms + report.enter_ms + report.travel_ms + report.logout_ms;
    assert!(
        hub_ms < budget,
        "hub round trip {hub_ms:.0} ms over {budget} ms"
    );

    // A second login with the right password works, the wrong one does not, and a stolen
    // ticket cannot be used twice (the zone remembers the nonce).
    let hub = gm_hub::HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();
    let wrong = hub
        .request(&gm_hub::protocol::HubRequest::Login {
            email: "pezo@example.com".into(),
            password: "wrong".into(),
        })
        .await;
    assert!(matches!(
        wrong,
        Err(gm_hub::HubClientError::Refused(
            gm_hub::protocol::HubError::Credentials
        ))
    ));
    let session = match hub
        .request(&gm_hub::protocol::HubRequest::Login {
            email: "PEZO@example.com ".into(),
            password: "correct horse battery".into(),
        })
        .await
        .unwrap()
    {
        gm_hub::protocol::HubResponse::Session { session, .. } => session,
        other => panic!("{other:?}"),
    };
    let ticket = match hub
        .request(&gm_hub::protocol::HubRequest::Enter {
            session,
            character: report.character,
            zone: "arena-a".into(),
        })
        .await
        .unwrap()
    {
        gm_hub::protocol::HubResponse::Ticket(t) => t,
        other => panic!("{other:?}"),
    };
    // Entering twice while the first ticket is unclaimed is refused (the character is in
    // transit), and the transit is recoverable after the abandon age.
    let again = hub
        .request(&gm_hub::protocol::HubRequest::Enter {
            session,
            character: report.character,
            zone: "arena-b".into(),
        })
        .await;
    assert!(matches!(
        again,
        Err(gm_hub::HubClientError::Refused(
            gm_hub::protocol::HubError::Busy
        ))
    ));
    assert_eq!(ticket.zone, "arena-a");
    hub.ok(&gm_hub::protocol::HubRequest::Logout { session })
        .await
        .unwrap();
    hub.close();

    hub_task.abort();
    zone_a.abort();
    zone_b.abort();
}
