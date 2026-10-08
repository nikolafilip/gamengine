//! Phase 7 acceptance, step 3 (PLAN.md 11.8, COMPANIONS.md 14): a solo player clears the
//! tutorial dungeon with three **hired avatars**, through the real hub, a real database and
//! the real protocol on loopback. The hires are paid in the tavern, the zone turns them into
//! companions at the claim, the Warden's drop and coin arrive in the leader's inventory by
//! `ZoneEcon`, the leader's trial is recorded and opens a gated zone, and a hire ends when
//! the avatar's owner takes the character back.
//! Needs `GM_TEST_DATABASE_URL` (a Postgres the test may wipe); without it the test is skipped.
//! It runs in real time: about three minutes.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gm_bot::{Behaviour, BotConfig, HubFlowConfig, run_hub_flow};
use gm_core::tick::TickRate;
use gm_hub::economy::{Economy, HIRE_BURN_PER_CENT};
use gm_hub::protocol::{HubError, HubRequest, HubResponse};
use gm_hub::{Db, HubConfig, HubKey};
use gm_hub_proto::{HubClient, HubClientError};
use gm_net::transport::{Identity, hub_server_config, server_config};
use gm_server::{HubLink, HubLinkConfig, ZoneConfig, ZoneWorld};

const MAPS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/built");
const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "test-zone-secret";
const PASSWORD: &str = "correct horse battery";
const PRICE: i64 = 100;
const AVATARS: [(&str, &str); 3] = [
    ("Bulwark", "ironclad"),
    ("Salve", "mender"),
    ("Rime", "frostweaver"),
];

struct ZoneSpec {
    id: &'static str,
    map: &'static str,
    squads: bool,
    recruits: Vec<String>,
    requires: Vec<String>,
    seconds: u64,
}

async fn start_zone(
    spec: ZoneSpec,
    hub_addr: std::net::SocketAddr,
    hub_cert: Vec<u8>,
    content: gm_core::build::ContentPack,
) -> tokio::task::JoinHandle<anyhow::Result<gm_server::ZoneReport>> {
    let identity = Identity::generate(&["localhost"]).unwrap();
    let path = Path::new(MAPS_DIR).join(format!("{}.bsp", spec.map));
    let world = Arc::new(ZoneWorld::load(&path).expect("map built"));
    let endpoint = quinn::Endpoint::server(
        server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let link = HubLink::connect(HubLinkConfig {
        addr: hub_addr,
        cert_der: hub_cert,
        zone: spec.id.to_string(),
        secret: SECRET.into(),
        map: world.name.clone(),
        map_hash: world.hash,
        public_addr: endpoint.local_addr().unwrap(),
        zone_cert_der: identity.cert_der().to_vec(),
        web: None,
        min_trust: 0,
        requires: spec.requires,
        max_players: 64,
    })
    .await
    .expect("zone registers");
    let cfg = ZoneConfig {
        max_ticks: Some(64 * spec.seconds),
        report_every: Duration::from_secs(5),
        content,
        hub: Some(link),
        wild: !world.creature_posts.is_empty(),
        // A dungeon: arrivals start at its entry, wherever they logged out.
        arrive_at_entry: !world.creature_posts.is_empty(),
        squads: spec.squads,
        recruits: spec.recruits,
        ..ZoneConfig::default()
    };
    tokio::spawn(gm_server::run(cfg, world, endpoint, std::future::pending()))
}

fn flow(
    hub: std::net::SocketAddr,
    cert: &[u8],
    email: &str,
    character: &str,
    preset: &str,
) -> HubFlowConfig {
    HubFlowConfig {
        hub,
        hub_cert_der: cert.to_vec(),
        email: email.into(),
        password: PASSWORD.into(),
        register: true,
        character: character.into(),
        preset: preset.into(),
        zone: "dungeon".into(),
        travel_after: None,
        travel_to: None,
        maps_dir: MAPS_DIR.into(),
        bot: BotConfig {
            name: String::new(),
            seed: 11,
            behaviour: Behaviour::Raid,
            rate: TickRate::COMBAT,
            run_ticks: 0,
            build: None,
            team: 0,
            counter_pick: false,
            travel_to: None,
            travel_after_ticks: 0,
            stall_tile: None,
            aim: Default::default(),
            report_after_ticks: 0,
            say: None,
            social: Default::default(),
        },
        play: Duration::ZERO,
        list_for_hire: None,
        sell_at: None,
        trade_for: None,
        hire: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_solo_player_clears_the_dungeon_with_three_hired_avatars() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    let econ = Economy::new(db.pool().clone());
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
    let hub_task = tokio::spawn(gm_hub::run(
        HubConfig {
            zone_secret: SECRET.into(),
            content: content.clone(),
            key: HubKey::generate(),
            session_secs: 3600,
            auth_per_minute: 100.0,
            econ_per_second: 1000.0,
            party_sweep: std::time::Duration::from_millis(300),
            party_away: std::time::Duration::from_secs(2),
            items: Default::default(),
            looks: gm_content::looks::Looks::load_dir(Path::new(CONTENT)).expect("looks"),
            max_coin_grant: 500,
            models_dir: std::env::temp_dir()
                .join(format!("gm-hub-models-companions-{}", std::process::id())),
            ingest: gm_hub::IngestMode::InProcess,
            ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
            start_zone: None,
            blurbs: Vec::new(),
        },
        db.clone(),
        endpoint,
        std::future::pending(),
    ));

    // The dungeon lends recruits, but a full squad of hires leaves them no slot; the keep
    // opens only to who passed the leader's trial.
    let dungeon = start_zone(
        ZoneSpec {
            id: "dungeon",
            map: "dungeon",
            squads: true,
            recruits: vec!["ironclad".into(), "mender".into(), "frostweaver".into()],
            requires: Vec::new(),
            seconds: 400,
        },
        hub_addr,
        hub_cert.clone(),
        content.clone(),
    )
    .await;
    let keep = start_zone(
        ZoneSpec {
            id: "keep",
            map: "arena",
            squads: false,
            recruits: Vec::new(),
            requires: vec!["warden_leader".into()],
            seconds: 400,
        },
        hub_addr,
        hub_cert.clone(),
        content.clone(),
    )
    .await;

    // Three owners make a character each, list it in the tavern and stay offline.
    let mut avatars = Vec::new();
    for (i, (name, preset)) in AVATARS.iter().enumerate() {
        let mut cfg = flow(
            hub_addr,
            &hub_cert,
            &format!("owner-{i}@example.test"),
            name,
            preset,
        );
        cfg.list_for_hire = Some(PRICE);
        let report = run_hub_flow(cfg).await.expect("an owner lists an avatar");
        avatars.push(report.character);
    }

    // The leader: a character with the coin for three hires (a drop the test stands in for).
    let leader_flow = flow(hub_addr, &hub_cert, "leader@example.test", "Marko", "blade");
    let leader = run_hub_flow(leader_flow.clone())
        .await
        .expect("the leader registers")
        .character;
    econ.grant_coin(leader, 3 * PRICE, 0).await.unwrap();

    // The keep is locked to it, and says which trial opens it.
    let client = HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();
    let session = match client
        .request(&HubRequest::Login {
            email: "leader@example.test".into(),
            password: PASSWORD.into(),
        })
        .await
        .unwrap()
    {
        HubResponse::Session { session, .. } => session,
        other => panic!("login: {other:?}"),
    };
    let enter_keep = HubRequest::Enter {
        session,
        character: leader,
        zone: "keep".into(),
    };
    match client.request(&enter_keep).await {
        Err(HubClientError::Refused(HubError::Locked(trials))) => {
            assert_eq!(trials, "warden_leader")
        }
        other => panic!("the keep should be locked: {other:?}"),
    }
    assert_eq!(
        client
            .request(&HubRequest::Trials {
                session,
                character: leader
            })
            .await
            .unwrap(),
        HubResponse::Trials(Vec::new())
    );

    // The run: hire the three, enter, lead them through the gate and the Warden.
    let mut cfg = leader_flow.clone();
    cfg.register = false;
    cfg.hire = 3;
    cfg.play = Duration::from_secs(330);
    let started = std::time::Instant::now();
    let report = run_hub_flow(cfg).await.expect("the leader's run");
    let b = &report.reports[0];
    println!(
        "run: {:.1} s (wall {:.1} s), squad {:?}, hired {}, orders {} (refused {}), cleared {:?}, resets {}, deaths {}, loot {:?}, coin {}, trials {:?}; left with {} silver, items {:?}, trials {:?}; rx {:.0} B/s tx {:.0} B/s",
        b.secs,
        started.elapsed().as_secs_f64(),
        report.squad,
        b.squad_hired,
        b.orders,
        b.orders_refused,
        b.cleared,
        b.resets,
        b.own_deaths,
        b.loot,
        b.coin,
        b.trials,
        report.coin,
        report.items,
        report.trials,
        b.rx_bytes_per_s(),
        b.tx_bytes_per_s()
    );

    // The squad was the three hired avatars, by name, and no recruit.
    let mut names = report.squad.clone();
    names.sort();
    assert_eq!(names, ["Bulwark", "Rime", "Salve"]);
    assert_eq!(b.squad_max, 3);
    assert_eq!(b.squad_hired, 3, "hires fill the squad before recruits");

    // The clear.
    assert!(b.raid_done, "the raid was not finished: {:?}", b.cleared);
    let warden = b
        .cleared
        .iter()
        .find(|c| c.0 == "warden")
        .expect("the Warden fell")
        .1;
    assert!((45..=300).contains(&warden), "the Warden took {warden} s");
    assert_eq!(b.loot, ["core/iron", "frame/ash", "catalyst/basalt"]);
    assert_eq!(b.coin, 30);
    assert!(b.trials.iter().any(|t| t.0 == "warden_leader" && t.1));

    // The database agrees: the drop is in the leader's inventory (first use of drops through
    // `ZoneEcon`), the hires were paid and burned, the avatars got flat coin and no loot,
    // the trial is on the character, and the ledger is sound.
    let mut items = report.items.clone();
    items.sort();
    assert_eq!(items, ["catalyst/basalt", "core/iron", "frame/ash"]);
    assert_eq!(
        report.coin, 30,
        "300 granted, 300 paid for hires, 30 dropped"
    );
    assert_eq!(report.trials, ["warden_leader"]);
    let (coin, held) = econ.inventory(leader).await.unwrap();
    assert_eq!((coin, held.len()), (30, 3));
    for &avatar in &avatars {
        let (coin, held) = econ.inventory(avatar).await.unwrap();
        assert_eq!(coin, PRICE * (100 - HIRE_BURN_PER_CENT) / 100);
        assert!(held.is_empty(), "a hired avatar never receives loot");
    }
    let supply = econ.supply().await.unwrap();
    assert_eq!(supply.burned, 3 * PRICE * HIRE_BURN_PER_CENT / 100);
    assert_eq!(supply.created, 3 * PRICE + 30);
    assert_eq!(econ.audit().await.unwrap(), 0, "the ledger is sound");
    assert_eq!(
        db.trials_of(leader).await.unwrap(),
        vec![("warden_leader".to_string(), warden)]
    );

    // The trial opens the keep.
    match client.request(&enter_keep).await {
        Ok(HubResponse::Ticket(t)) => assert_eq!(t.zone, "keep"),
        other => panic!("the keep should be open now: {other:?}"),
    }
    client.ok(&HubRequest::Logout { session }).await.unwrap();

    // The hires are still running (12 h) until an owner takes its character back: entering
    // a zone with it ends that hire, with no refund.
    assert_eq!(econ.squad(leader).await.unwrap().len(), 3);
    let mut owner = flow(
        hub_addr,
        &hub_cert,
        "owner-0@example.test",
        AVATARS[0].0,
        AVATARS[0].1,
    );
    owner.register = false;
    owner.bot.behaviour = Behaviour::Hold;
    owner.play = Duration::from_secs(2);
    run_hub_flow(owner)
        .await
        .expect("an owner plays its avatar");
    let left: Vec<String> = econ
        .squad(leader)
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.name)
        .collect();
    assert_eq!(left, ["Salve", "Rime"]);
    let (coin, _) = econ.inventory(leader).await.unwrap();
    assert_eq!(coin, 30, "no refund");

    dungeon.abort();
    keep.abort();
    hub_task.abort();
}
