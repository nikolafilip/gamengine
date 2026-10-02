//! Phase 9 acceptance through the hub (ANTICHEAT.md 10.3): four accounts fight in a recorded
//! arena, two with a hand and two with an aim lock. Their numbers and the fight's replay
//! reach the database; the aim report flags the two cheats and only them; a report is
//! filed, its replay kept and fetched, upheld, and both reputations move; a ban ends the
//! account's play and names its reason; a zone that asks for a trust tier refuses a new
//! account and admits it once a moderator raised its tier; a team kill is a ledger row.
//! Needs `GM_TEST_DATABASE_URL` (a Postgres the test may wipe); without it the test is
//! skipped.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gm_bot::{AimModel, Behaviour, BotConfig, HubFlowConfig, run_hub_flow};
use gm_core::tick::TickRate;
use gm_hub::protocol::{AimStats, HubError, HubRequest, HubResponse, ModOp, SessionId, Verdict};
use gm_hub::{Db, HubClient, HubClientError, HubConfig, HubKey};
use gm_net::transport::{Identity, hub_server_config, server_config};
use gm_server::{HubLink, HubLinkConfig, ReplayConfig, ZoneConfig, ZoneWorld};

const ARENA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/arena.bsp"
);
const MAPS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/built");
const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "test-zone-secret";
const PLAY_SECS: u64 = 60;

async fn start_zone(
    id: &str,
    hub_addr: std::net::SocketAddr,
    hub_cert: Vec<u8>,
    content: gm_core::build::ContentPack,
    min_trust: i16,
    replays: Option<std::path::PathBuf>,
) -> tokio::task::JoinHandle<anyhow::Result<gm_server::ZoneReport>> {
    let identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(ARENA)).unwrap());
    let endpoint = quinn::Endpoint::server(
        server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let link = HubLink::connect(HubLinkConfig {
        addr: hub_addr,
        cert_der: hub_cert,
        zone: id.to_string(),
        secret: SECRET.into(),
        map: world.name.clone(),
        map_hash: world.hash,
        public_addr: endpoint.local_addr().unwrap(),
        zone_cert_der: identity.cert_der().to_vec(),
        web: None,
        min_trust,
        requires: Vec::new(),
    })
    .await
    .expect("zone registers");
    let cfg = ZoneConfig {
        max_ticks: Some(64 * (PLAY_SECS + 60)),
        report_every: Duration::from_secs(5),
        content,
        hub: Some(link),
        replay: replays.map(|dir| ReplayConfig {
            dir,
            bytes_per_hour: 1 << 30,
            zone: id.to_string(),
        }),
        ..ZoneConfig::default()
    };
    tokio::spawn(gm_server::run(cfg, world, endpoint, std::future::pending()))
}

async fn login(hub: &HubClient, email: &str, password: &str) -> Result<SessionId, HubClientError> {
    match hub
        .request(&HubRequest::Login {
            email: email.into(),
            password: password.into(),
        })
        .await?
    {
        HubResponse::Session { session, .. } => Ok(session),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn statistics_replays_reports_reputation_and_bans() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");
    let scratch = std::env::temp_dir().join(format!("gm-anticheat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);

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
            auth_per_minute: 1000.0,
            templates: Vec::new(),
            max_coin_grant: 10_000,
            models_dir: scratch.join("models"),
            ingest: gm_hub::IngestMode::InProcess,
            ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
        },
        db.clone(),
        endpoint,
        std::future::pending(),
    ));
    // A recorded arena, and a vault that asks for trust tier 1.
    let arena = start_zone(
        "arena",
        hub_addr,
        hub_cert.clone(),
        content.clone(),
        0,
        Some(scratch.join("zone-replays")),
    )
    .await;
    let vault = start_zone(
        "vault",
        hub_addr,
        hub_cert.clone(),
        content.clone(),
        1,
        None,
    )
    .await;
    // And a zone that records nothing, to travel to: what the hub knows of a traveller's
    // aim can only have come from the arena it left.
    let annex = start_zone(
        "annex",
        hub_addr,
        hub_cert.clone(),
        content.clone(),
        0,
        None,
    )
    .await;

    // Five players: two hands, two aim locks, and a third lock that travels on after half
    // a minute. The first hand reports whoever it sees after ten seconds.
    let players = [
        (
            "hand-a@example.com",
            "HandA",
            AimModel::Hand,
            "blade",
            10 * 64,
            false,
        ),
        (
            "hand-b@example.com",
            "HandB",
            AimModel::Sharp,
            "frostweaver",
            0,
            false,
        ),
        (
            "lock-a@example.com",
            "LockA",
            AimModel::Lock,
            "shade",
            0,
            false,
        ),
        (
            "lock-b@example.com",
            "LockB",
            AimModel::Lock,
            "frostweaver",
            0,
            false,
        ),
        (
            "traveller@example.com",
            "Traveller",
            AimModel::Lock,
            "shade",
            0,
            true,
        ),
    ];
    let mut flows = Vec::new();
    for (i, (email, name, aim, preset, report_after, travels)) in players.into_iter().enumerate() {
        flows.push(tokio::spawn(run_hub_flow(HubFlowConfig {
            hub: hub_addr,
            hub_cert_der: hub_cert.clone(),
            email: email.into(),
            password: "a long enough password".into(),
            register: true,
            character: name.into(),
            preset: preset.into(),
            zone: "arena".into(),
            travel_after: travels.then_some(Duration::from_secs(30)),
            travel_to: travels.then(|| "annex".to_string()),
            maps_dir: MAPS_DIR.into(),
            bot: BotConfig {
                name: String::new(),
                seed: 40 + i as u64,
                behaviour: Behaviour::Duelist,
                rate: TickRate::COMBAT,
                run_ticks: 0,
                build: None,
                team: 0,
                counter_pick: false,
                travel_to: None,
                travel_after_ticks: 0,
                stall_tile: None,
                aim,
                report_after_ticks: report_after,
            },
            play: Duration::from_secs(PLAY_SECS),
            list_for_hire: None,
            hire: 0,
        })));
    }
    let mut reported = false;
    for f in flows {
        let report = f.await.unwrap().expect("hub flow");
        reported |= report.reports.iter().any(|r| r.report_accepted);
    }
    assert!(reported, "the zone took the hand's report");

    // A moderator.
    let hub = HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();
    hub.request(&HubRequest::Register {
        email: "mod@example.com".into(),
        password: "the moderator's password".into(),
    })
    .await
    .unwrap();
    assert!(
        gm_hub::models::grant_moderator(db.pool(), "mod@example.com")
            .await
            .unwrap()
    );
    let moderator = login(&hub, "mod@example.com", "the moderator's password")
        .await
        .unwrap();
    let ask = async |op: ModOp| {
        hub.request(&HubRequest::Mod {
            session: moderator,
            op,
        })
        .await
    };

    // The fight closes ten seconds after its last hit and its replay is uploaded; the
    // report's replay ten seconds after the report. Wait for both.
    let deadline = Instant::now() + Duration::from_secs(40);
    let replays = loop {
        let HubResponse::Replays(rows) = ask(ModOp::Replays {
            email: None,
            reported: false,
            flagged: false,
            limit: 50,
        })
        .await
        .unwrap() else {
            panic!("replays")
        };
        if rows.iter().any(|r| r.reported) && rows.iter().any(|r| !r.reported) {
            break rows;
        }
        assert!(Instant::now() < deadline, "replays never arrived: {rows:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    println!("replays at the hub: {replays:?}");
    let fight = replays.iter().find(|r| !r.reported).unwrap();
    assert!(fight.seconds > 20.0, "{fight:?}");
    assert!(fight.participants.len() >= 4, "{fight:?}");

    // The aim report: both locks flagged, neither hand.
    let HubResponse::AimReport(rows) = ask(ModOp::AimReport {
        weeks: 1,
        min_shots: 10,
    })
    .await
    .unwrap() else {
        panic!("aim report")
    };
    for r in &rows {
        println!("{}: rules {:?}: {}", r.email, r.rules, r.stats.line());
    }
    // A program is in the report and breaks the rule made for it; a hand breaks none (and
    // one that barely shot, a blade, is not in the report at all).
    let rules_of = |email: &str| {
        rows.iter()
            .find(|r| r.email == email)
            .map(|r| r.rules.clone())
            .unwrap_or_default()
    };
    assert!(rules_of("lock-a@example.com").contains(&"lock".to_string()));
    assert!(rules_of("lock-b@example.com").contains(&"lock".to_string()));
    assert!(rules_of("hand-a@example.com").is_empty());
    assert!(rules_of("hand-b@example.com").is_empty());
    // The traveller's numbers left the arena with it: the zone it went to keeps none, so
    // these are the arena's, reported when the other zone claimed the character.
    let HubResponse::AimReport(everyone) = ask(ModOp::AimReport {
        weeks: 1,
        min_shots: 1,
    })
    .await
    .unwrap() else {
        panic!("aim report")
    };
    let traveller = everyone
        .iter()
        .find(|r| r.email == "traveller@example.com")
        .expect("the traveller's aim was reported at the handoff");
    assert!(traveller.stats.shots >= 5, "{}", traveller.stats.line());

    // The fight's replay comes back as the bytes the zone wrote, and analysing it gives
    // what the hub was told about each participant in it.
    let bytes = hub
        .download(
            &HubRequest::Mod {
                session: moderator,
                op: ModOp::ReplayGet { id: fight.id },
            },
            gm_hub::protocol::MAX_REPLAY_BYTES as usize,
        )
        .await
        .expect("the replay's bytes");
    assert_eq!(bytes.len(), fight.bytes as usize);
    let replay = gm_replay::Replay::read(&bytes).expect("a replay");
    let (stats, _) = gm_replay::aim::analyse(&replay);
    for who in replay.everyone().iter().filter(|r| r.human()) {
        let rules: Vec<&str> = stats
            .get(&who.id)
            .map(|s| s.rules().iter().map(|r| r.name()).collect())
            .unwrap_or_default();
        let told = &fight
            .participants
            .iter()
            .find(|(name, _)| name == &who.name)
            .unwrap_or_else(|| panic!("{} is not listed", who.name))
            .1;
        assert_eq!(&rules.join(","), told, "{}", who.name);
        assert!(who.character != 0, "the roster names the hub's character");
    }
    // A flagged account's standing lists the flag; a clean one's does not.
    let HubResponse::Standing(lock) = ask(ModOp::Reputation {
        email: "lock-a@example.com".into(),
    })
    .await
    .unwrap() else {
        panic!("standing")
    };
    assert!(!lock.flags.is_empty(), "{lock:?}");

    // The report: open, with its replay; upheld once and only once.
    let HubResponse::Reports(reports) = ask(ModOp::Reports { open_only: true }).await.unwrap()
    else {
        panic!("reports")
    };
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = &reports[0];
    assert_eq!(report.reporter, "hand-a@example.com");
    assert_eq!(report.reason, "aim");
    assert!(report.replay.is_some(), "{report:?}");
    let target = report.target.clone();
    ask(ModOp::ReportVerdict {
        id: report.id,
        verdict: Verdict::Upheld,
        note: "watched replay".into(),
    })
    .await
    .expect("verdict");
    let again = ask(ModOp::ReportVerdict {
        id: report.id,
        verdict: Verdict::Abusive,
        note: String::new(),
    })
    .await;
    assert!(matches!(
        again,
        Err(HubClientError::Refused(HubError::NotFound))
    ));
    let standing = async |email: &str| match ask(ModOp::Reputation {
        email: email.into(),
    })
    .await
    .unwrap()
    {
        HubResponse::Standing(s) => s,
        other => panic!("{other:?}"),
    };
    let t = standing(&target).await;
    assert!(
        t.ledger
            .iter()
            .any(|r| r.kind == "report_upheld" && r.delta == -10),
        "{t:?}"
    );
    let r = standing("hand-a@example.com").await;
    assert!(
        r.ledger
            .iter()
            .any(|row| row.kind == "report_helpful" && row.delta == 1),
        "{r:?}"
    );

    // A ban: the login is refused with the reason, the standing shows it and the
    // confirmed cheat; lifted, the login works again.
    ask(ModOp::Ban {
        email: "lock-a@example.com".into(),
        days: 7,
        reason: "aim assistance, replay 1".into(),
        cheat: true,
    })
    .await
    .expect("ban");
    match login(&hub, "lock-a@example.com", "a long enough password").await {
        Err(HubClientError::Refused(HubError::Banned { reason, until })) => {
            assert_eq!(reason, "aim assistance, replay 1");
            assert!(until > 0);
        }
        other => panic!("a banned account logged in: {other:?}"),
    }
    let banned = standing("lock-a@example.com").await;
    assert!(banned.ban.is_some());
    assert!(
        banned
            .ledger
            .iter()
            .any(|r| r.kind == "cheat_confirmed" && r.delta == -100)
    );
    // A ban with no reason is refused; nobody bans themselves.
    assert!(
        ask(ModOp::Ban {
            email: "lock-b@example.com".into(),
            days: 1,
            reason: " ".into(),
            cheat: false,
        })
        .await
        .is_err()
    );
    ask(ModOp::Unban {
        email: "lock-a@example.com".into(),
        note: "appeal".into(),
    })
    .await
    .expect("unban");
    login(&hub, "lock-a@example.com", "a long enough password")
        .await
        .expect("an unbanned account logs in");

    // A zone that asks for trust tier 1: a new account is refused, and admitted once a
    // moderator raised its tier.
    let session = login(&hub, "hand-b@example.com", "a long enough password")
        .await
        .unwrap();
    let HubResponse::Characters(chars) = hub
        .request(&HubRequest::Characters { session })
        .await
        .unwrap()
    else {
        panic!("characters")
    };
    let enter = HubRequest::Enter {
        session,
        character: chars[0].id,
        zone: "vault".into(),
    };
    match hub.request(&enter).await {
        Err(HubClientError::Refused(HubError::Locked(what))) => assert!(what.contains("trust")),
        other => panic!("the vault let a new account in: {other:?}"),
    }
    ask(ModOp::SetTrust {
        email: "hand-b@example.com".into(),
        tier: 1,
    })
    .await
    .expect("trust");
    assert!(matches!(
        hub.request(&enter).await,
        Ok(HubResponse::Ticket(_))
    ));

    // A zone reports team kills with a client's numbers: reputation, five a day at most,
    // and a repeated report counts once.
    let zone = HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();
    zone.request(&HubRequest::ZoneHello {
        secret: SECRET.into(),
        zone: "pit".into(),
        map: "arena".into(),
        map_hash: 0,
        addr: "127.0.0.1:9".parse().unwrap(),
        cert_der: Vec::new(),
        web: None,
        min_trust: 0,
        requires: Vec::new(),
    })
    .await
    .expect("a zone registers");
    let hand_b = chars[0].id;
    let griefing = HubRequest::ZoneAim {
        nonce: 77,
        character: hand_b,
        stats: AimStats {
            team_kills: 7,
            ..AimStats::default()
        },
    };
    zone.request(&griefing).await.expect("aim numbers");
    zone.request(&griefing).await.expect("the same again");
    let b = standing("hand-b@example.com").await;
    let team_kills: Vec<_> = b.ledger.iter().filter(|r| r.kind == "team_kill").collect();
    assert_eq!(
        team_kills.len(),
        5,
        "five a day count, whoever reports them: {b:?}"
    );
    // The account's reputation is the sum of its ledger, whatever is in it (this account
    // may also have been the target of the report upheld above).
    assert_eq!(
        b.reputation,
        b.ledger.iter().map(|r| r.delta).sum::<i32>(),
        "{b:?}"
    );
    assert!(b.reputation <= -10);

    // Everything a moderator did and looked at is on record.
    let logged = gm_hub::conduct::mod_log_rows(db.pool()).await.unwrap();
    assert!(logged >= 10, "{logged} rows in mod_log");

    hub.close();
    zone.close();
    hub_task.abort();
    arena.abort();
    vault.abort();
    annex.abort();
    let _ = std::fs::remove_dir_all(&scratch);
}
