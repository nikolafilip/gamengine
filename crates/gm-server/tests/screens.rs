//! What the client's screens lean on at the hub and in a zone (CLIENT.md 7): names that
//! every client can draw and nobody can mistake, the content before any zone, an entry
//! that finds a zone by itself, a full zone refused, a character that a zone would not
//! take put back where it came from, sessions that live while they are used, and a logout
//! that leaves the account's other client alone.
//! Needs `GM_TEST_DATABASE_URL` (a Postgres the test may wipe); without it the test is
//! skipped.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gm_bot::{Behaviour, BotConfig, HubFlowConfig, run_hub_flow};
use gm_core::tick::TickRate;
use gm_hub::player::{PlayerRequest, PlayerResponse};
use gm_hub::protocol::{
    BuildChoice, CharacterId, HubError, HubRequest, HubResponse, LocationSummary, SessionId,
};
use gm_hub::{Db, HubClient, HubClientError, HubConfig, HubKey};
use gm_net::transport::{Identity, hub_server_config, server_config};
use gm_server::{HubLink, HubLinkConfig, ZoneConfig, ZoneWorld};

const ARENA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/arena.bsp"
);
const MAPS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/built");
const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const SECRET: &str = "test-zone-secret";

/// A zone: what it tells the hub it takes, what it really takes, and what it asks for.
struct Spec {
    id: &'static str,
    told: u32,
    takes: usize,
    requires: Vec<String>,
}

async fn start_zone(
    spec: Spec,
    hub_addr: std::net::SocketAddr,
    hub_cert: Vec<u8>,
    content: gm_core::build::ContentPack,
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
        zone: spec.id.to_string(),
        secret: SECRET.into(),
        map: world.name.clone(),
        map_hash: world.hash,
        public_addr: endpoint.local_addr().unwrap(),
        zone_cert_der: identity.cert_der().to_vec(),
        web: None,
        min_trust: 0,
        requires: spec.requires,
        max_players: spec.told,
    })
    .await
    .expect("zone registers");
    let cfg = ZoneConfig {
        max_ticks: Some(64 * 90),
        max_players: spec.takes,
        report_every: Duration::from_secs(5),
        content,
        hub: Some(link),
        ..ZoneConfig::default()
    };
    tokio::spawn(gm_server::run(cfg, world, endpoint, std::future::pending()))
}

fn refused(r: Result<HubResponse, HubClientError>) -> HubError {
    match r {
        Err(HubClientError::Refused(e)) => e,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn session(hub: &HubClient, email: &str, register: bool) -> SessionId {
    let (email, password) = (email.to_string(), "a long enough password".to_string());
    let req = if register {
        HubRequest::Register { email, password }
    } else {
        HubRequest::Login { email, password }
    };
    match hub.request(&req).await.expect("a session") {
        HubResponse::Session { session, .. } => session,
        other => panic!("{other:?}"),
    }
}

async fn create(hub: &HubClient, session: SessionId, name: &str) -> Result<CharacterId, HubError> {
    match hub
        .request(&HubRequest::CreateCharacter {
            session,
            name: name.into(),
            build: BuildChoice::Preset("blade".into()),
        })
        .await
    {
        Ok(HubResponse::Character(c)) => Ok(c.id),
        Ok(other) => panic!("{other:?}"),
        Err(HubClientError::Refused(e)) => Err(e),
        Err(e) => panic!("{e}"),
    }
}

async fn enter(
    hub: &HubClient,
    session: SessionId,
    character: CharacterId,
    zone: &str,
) -> Result<String, HubError> {
    match hub
        .request(&HubRequest::Enter {
            session,
            character,
            zone: zone.into(),
        })
        .await
    {
        Ok(HubResponse::Ticket(t)) => Ok(t.zone),
        Ok(other) => panic!("{other:?}"),
        Err(HubClientError::Refused(e)) => Err(e),
        Err(e) => panic!("{e}"),
    }
}

/// A bot of `email` that plays `character` in `zone` for `secs`.
fn play(
    hub: std::net::SocketAddr,
    cert: &[u8],
    email: &str,
    character: &str,
    zone: &str,
    secs: u64,
) -> tokio::task::JoinHandle<anyhow::Result<gm_bot::HubFlowReport>> {
    tokio::spawn(run_hub_flow(HubFlowConfig {
        hub,
        hub_cert_der: cert.to_vec(),
        email: email.into(),
        password: "a long enough password".into(),
        register: true,
        character: character.into(),
        preset: "blade".into(),
        zone: zone.into(),
        travel_after: None,
        travel_to: None,
        maps_dir: MAPS_DIR.into(),
        bot: BotConfig {
            name: String::new(),
            seed: 7,
            behaviour: Behaviour::Hold,
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
        play: Duration::from_secs(secs),
        list_for_hire: None,
        sell_at: None,
        trade_for: None,
        hire: 0,
    }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_the_screens_lean_on() {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this test may wipe");
        return;
    };
    let db = Db::connect(&url).await.expect("database");
    db.migrate().await.expect("migrations");
    db.wipe().await.expect("wipe");
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");
    let blurbs = gm_content::load_blurbs(Path::new(CONTENT)).expect("blurbs");
    let scratch = std::env::temp_dir().join(format!("gm-screens-{}", std::process::id()));

    // A hub whose sessions end after fifteen idle seconds, and four zones: the start zone,
    // one that takes a single client and says so, one that takes a single client and
    // tells the hub it takes eight, and one behind a trial.
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
            session_secs: 15,
            auth_per_minute: 1000.0,
            econ_per_second: 1000.0,
            party_sweep: std::time::Duration::from_millis(300),
            party_away: std::time::Duration::from_secs(2),
            items: Default::default(),
            max_coin_grant: 10_000,
            models_dir: scratch.join("models"),
            ingest: gm_hub::IngestMode::InProcess,
            ingest_timeout: gm_hub::models::INGEST_TIMEOUT,
            start_zone: Some("square".into()),
            blurbs: blurbs.clone(),
        },
        db.clone(),
        endpoint,
        std::future::pending(),
    ));
    let mut zones = Vec::new();
    for spec in [
        Spec {
            id: "square",
            told: 64,
            takes: 64,
            requires: Vec::new(),
        },
        Spec {
            id: "tiny",
            told: 1,
            takes: 1,
            requires: Vec::new(),
        },
        Spec {
            id: "cramped",
            told: 8,
            takes: 1,
            requires: Vec::new(),
        },
        Spec {
            id: "keep",
            told: 64,
            takes: 64,
            requires: vec!["warden_leader".into()],
        },
    ] {
        zones.push(start_zone(spec, hub_addr, hub_cert.clone(), content.clone()).await);
    }
    let hub = HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();

    // Names: what every client can draw, and no name another could be taken for.
    let s = session(&hub, "one@example.com", true).await;
    let aldric = create(&hub, s, "Aldric").await.expect("a plain name");
    assert_eq!(
        create(&hub, s, "AIdric").await,
        Err(HubError::Taken),
        "a capital I for an l"
    );
    assert_eq!(create(&hub, s, "aldric").await, Err(HubError::Taken));
    assert_eq!(create(&hub, s, "A1dric").await, Err(HubError::Taken));
    for (name, why) in [
        ("Zone", "that name is the game's own"),
        ("Adm1n", "that name is the game's own"),
        ("Zone 2", "that name is the game's own"),
        ("GM Bob", "that name is the game's own"),
        ("Moderator7", "that name is the game's own"),
        ("x", "a name is two letters or more"),
        ("č", "a name is two letters or more"),
        (
            "Aldric!",
            "a name is made of letters, digits, spaces, hyphens and apostrophes",
        ),
        ("9lives", "a name begins with a letter"),
    ] {
        assert_eq!(
            create(&hub, s, name).await,
            Err(HubError::Invalid(why.into())),
            "{name}"
        );
    }
    let zeljko = create(&hub, s, "Željko").await.expect("a marked letter");

    // The content, before any zone: the pack and a line or two for every archetype.
    match hub
        .request(&HubRequest::Content { session: s })
        .await
        .unwrap()
    {
        HubResponse::Content { pack, blurbs: got } => {
            assert_eq!(pack, content);
            assert_eq!(got, blurbs);
            assert_eq!(got.len(), pack.builds.len());
            assert!(got.iter().all(|b| !b.is_empty() && b.len() <= 160));
        }
        other => panic!("{other:?}"),
    }
    match hub
        .request(&HubRequest::Characters { session: s })
        .await
        .unwrap()
    {
        HubResponse::Characters(list) => {
            assert_eq!(list.len(), 2);
            assert!(
                list.iter().all(|c| c.last_zone.is_none()),
                "nobody has been anywhere"
            );
        }
        other => panic!("{other:?}"),
    }

    // The players' encoding (HUB.md 3.8): the same requests behind an empty frame,
    // answered in kind, a refusal as a refusal, and bytes as bytes.
    match hub
        .player(&PlayerRequest::Content { session: s })
        .await
        .unwrap()
    {
        PlayerResponse::Content { pack, blurbs: got } => {
            assert_eq!(pack, content);
            assert_eq!(got, blurbs);
        }
        other => panic!("{other:?}"),
    }
    match hub
        .player(&PlayerRequest::Characters { session: s })
        .await
        .unwrap()
    {
        PlayerResponse::Characters(list) => {
            let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, ["Aldric", "Željko"]);
        }
        other => panic!("{other:?}"),
    }
    let nobody = SessionId([0; 16]);
    assert!(matches!(
        hub.player(&PlayerRequest::ListZones { session: nobody })
            .await,
        Err(HubClientError::Refused(HubError::Unauthorized))
    ));
    assert!(matches!(
        hub.player_download(
            &PlayerRequest::ModelGet {
                session: s,
                model: [7; 32]
            },
            1 << 20
        )
        .await,
        Err(HubClientError::Refused(HubError::NotFound))
    ));

    // A client of another build is told the hub's version and nothing else: whatever its
    // messages have become, that much it can read. The same for a zone or a tool of
    // another build; one from before there were versions is told nothing.
    for (sent, answered) in [
        (
            &[0u8, 0, 0, 1, 99, 0, 3, 1, 2, 3][..],
            &gm_hub::player::HUB_PREAMBLE[..],
        ),
        (
            &[0, 1, 99, 0, 3, 1, 2, 3][..],
            &gm_hub::protocol::HUB_PREAMBLE[..],
        ),
    ] {
        let (mut send, mut recv) = hub.connection().open_bi().await.unwrap();
        send.write_all(sent).await.unwrap();
        send.finish().unwrap();
        let said = recv.read_to_end(64).await.unwrap();
        assert_eq!(said, answered, "its version, then the end");
    }
    assert!(matches!(
        HubClientError::Version(9).to_string().as_str(),
        text if text.contains("version 9") && text.contains("one build")
    ));

    // The names made before the skeleton was kept are keyed by the migration as the hub
    // would key them: the same expression, on names the old rule allowed.
    let migration = include_str!("../../gm-hub/migrations/0007_names.sql");
    let expression = migration
        .lines()
        .find(|l| l.contains("translate("))
        .expect("the backfill")
        .trim()
        .trim_end_matches(';')
        .replace("lower(name ", "lower($1::text ");
    for name in [
        "Aldric",
        "AIdric",
        "A1dric",
        "ALDRIC",
        "Bob",
        "B0b",
        "Čedo",
        "ČEDO",
        "Ćiro",
        "De Vil",
        "O'Neil",
        "Ana-Marija",
        "Đuro Šž",
        "LOL 101",
        "Ili",
    ] {
        let keyed: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("select {expression}")))
            .bind(name)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(keyed, gm_hub::names::skeleton(name), "{name}");
    }

    // The migration itself, on names from before it (a table of this transaction's own
    // stands in for the real one): two that are alike both keep their owners, the oldest
    // with the skeleton, and the index is made.
    {
        let mut tx = db.pool().begin().await.unwrap();
        sqlx::query(
            "create temp table characters (id bigserial primary key, name text not null) on commit drop",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        for name in ["Aldric", "AIdric", "aldric", "Bob", "De Vil", "Devil"] {
            sqlx::query("insert into characters (name) values ($1)")
                .bind(name)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let statements: String = migration
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        for statement in statements.split(';').filter(|s| !s.trim().is_empty()) {
            sqlx::query(sqlx::AssertSqlSafe(statement.to_string()))
                .execute(&mut *tx)
                .await
                .unwrap_or_else(|e| panic!("{statement}: {e}"));
        }
        let keys: Vec<(i64, String, String)> =
            sqlx::query_as("select id, name, name_key from characters order by id")
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        let key = |name: &str| keys.iter().find(|k| k.1 == name).unwrap().2.clone();
        assert_eq!(key("Aldric"), "aldrlc");
        assert_eq!(key("Bob"), "bob");
        assert_eq!(key("De Vil"), "devll");
        for (id, name, got) in &keys {
            if ["AIdric", "aldric", "Devil"].contains(&name.as_str()) {
                let skeleton = gm_hub::names::skeleton(name);
                assert_eq!(got, &format!("{skeleton}\u{1}{id}"), "{name}");
            }
        }
        tx.rollback().await.unwrap();
    }

    // An entry that names no zone goes to the start zone. The ticket is left unused: asked
    // again at once the character is on its way; the sweeper puts it offline again when
    // the ticket has run out (here: at once, by hand), and nothing of it is left.
    assert_eq!(enter(&hub, s, aldric, "").await.as_deref(), Ok("square"));
    assert_eq!(
        enter(&hub, s, aldric, "").await,
        Err(HubError::Busy),
        "it is on its way"
    );
    assert_eq!(db.sweep_transits(0).await.unwrap(), 1);
    match hub
        .request(&HubRequest::Characters { session: s })
        .await
        .unwrap()
    {
        HubResponse::Characters(list) => {
            assert_eq!(list[0].location, LocationSummary::Offline);
            assert_eq!(list[0].last_zone, None, "it never stood anywhere");
        }
        other => panic!("{other:?}"),
    }
    // A ticket nobody uses holds no seat: the zone that takes one client gives a second
    // ticket while the first is out (its own door will have the last word).
    assert_eq!(enter(&hub, s, aldric, "tiny").await.as_deref(), Ok("tiny"));
    // A zone behind a trial is not where an entry that names none ends up, even when the
    // character's saved position is there.
    sqlx::query("update characters set pos_zone = 'keep' where id = $1")
        .bind(zeljko)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(enter(&hub, s, zeljko, "").await.as_deref(), Ok("square"));
    let s2 = session(&hub, "two@example.com", true).await;
    let brena = create(&hub, s2, "Brena").await.unwrap();
    assert!(
        matches!(
            enter(&hub, s2, brena, "keep").await,
            Err(HubError::Locked(_))
        ),
        "named outright, the keep says what it asks for"
    );

    // A full zone: somebody plays in the one that takes one client, and the hub sends
    // nobody else there.
    let first = play(hub_addr, &hub_cert, "tiny@example.com", "Tenant", "tiny", 7);
    let lodger = play(
        hub_addr,
        &hub_cert,
        "cramped@example.com",
        "Lodger",
        "cramped",
        7,
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(enter(&hub, s2, brena, "tiny").await, Err(HubError::Full));
    // An entry that names no zone passes a last zone that is full, to the start zone.
    sqlx::query("update characters set pos_zone = 'tiny' where id = $1")
        .bind(brena)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(enter(&hub, s2, brena, "").await.as_deref(), Ok("square"));

    // Logging out of one client leaves the account's other client playing: the tenant's
    // account logs in a second time and out again while the tenant plays.
    let extra = HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();
    let again = session(&extra, "tiny@example.com", false).await;
    assert!(matches!(
        extra.request(&HubRequest::Logout { session: again }).await,
        Ok(HubResponse::Ok)
    ));

    // A zone that tells the hub it has room and has none: the hub gives the ticket, the
    // zone claims the character and then has no body for it. The character goes back
    // offline as it came, and can enter somewhere else at once.
    let squeezed = play(
        hub_addr,
        &hub_cert,
        "late@example.com",
        "Latecomer",
        "cramped",
        3,
    )
    .await
    .unwrap();
    let why = format!("{:#}", squeezed.expect_err("the zone is full"));
    assert!(why.contains("zone full"), "{why}");
    let late = HubClient::connect_with_cert(hub_addr, hub_cert.clone())
        .await
        .unwrap();
    let s3 = session(&late, "late@example.com", false).await;
    let latecomer = match late
        .request(&HubRequest::Characters { session: s3 })
        .await
        .unwrap()
    {
        HubResponse::Characters(list) => {
            assert_eq!(
                list[0].location,
                LocationSummary::Offline,
                "put back where it came from"
            );
            assert_eq!(
                list[0].last_zone, None,
                "it never stood in the zone that refused it"
            );
            list[0].id
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        enter(&late, s3, latecomer, "square").await.as_deref(),
        Ok("square")
    );

    // A session lives while it is used (fifteen idle seconds end it here): eighteen
    // seconds of asking every six, and later seventeen of silence.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(
            hub.request(&HubRequest::ListZones { session: s2 })
                .await
                .is_ok()
        );
    }
    match hub
        .request(&HubRequest::ListZones { session: s2 })
        .await
        .unwrap()
    {
        HubResponse::Zones(list) => {
            let tiny = list
                .iter()
                .find(|z| z.id == "tiny")
                .expect("tiny is listed");
            assert_eq!((tiny.max_players, tiny.web), (1, false));
            let keep = list.iter().find(|z| z.id == "keep").unwrap();
            assert_eq!(keep.requires, ["warden_leader"]);
        }
        other => panic!("{other:?}"),
    }

    let tenant = first.await.unwrap().expect("the tenant played to the end");
    assert!(
        tenant.reports.iter().all(|r| r.kicked.is_none()),
        "{:?}",
        tenant.reports[0].kicked
    );
    lodger.await.unwrap().expect("the lodger played to the end");

    // Where a character stood last is where an entry that names no zone takes it back.
    let s4 = session(&extra, "tiny@example.com", false).await;
    let tenant = match extra
        .request(&HubRequest::Characters { session: s4 })
        .await
        .unwrap()
    {
        HubResponse::Characters(list) => {
            assert_eq!(list[0].location, LocationSummary::Offline);
            assert_eq!(list[0].last_zone.as_deref(), Some("tiny"));
            list[0].id
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(enter(&extra, s4, tenant, "").await.as_deref(), Ok("tiny"));

    // An account has eight sessions at once: the ninth login ends the one used longest ago.
    let mut sessions = Vec::new();
    for _ in 0..9 {
        sessions.push(session(&extra, "late@example.com", false).await);
    }
    // (`s3` was this account's first, and eight newer ones are alive.)
    assert_eq!(
        refused(late.request(&HubRequest::Characters { session: s3 }).await),
        HubError::Unauthorized
    );
    assert!(
        late.request(&HubRequest::Characters {
            session: sessions[8]
        })
        .await
        .is_ok()
    );

    tokio::time::sleep(Duration::from_secs(17)).await;
    assert_eq!(
        refused(hub.request(&HubRequest::ListZones { session: s2 }).await),
        HubError::Unauthorized,
        "seventeen idle seconds end a session of fifteen"
    );

    for z in zones {
        z.abort();
    }
    hub_task.abort();
    let _ = std::fs::remove_dir_all(scratch);
}
