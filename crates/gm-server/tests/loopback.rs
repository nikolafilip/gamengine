//! The real socket path: a zone on 127.0.0.1 and two bots over actual UDP for two seconds.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gm_bot::{Behaviour, BotConfig, run_bot};
use gm_bsp::Bsp;
use gm_core::tick::TickRate;
use gm_net::transport::{Identity, client_config, server_config};
use gm_server::{ZoneConfig, ZoneWorld};

const MAP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/test_room.bsp"
);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zone_and_bots_over_real_udp() {
    let identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(MAP)).unwrap());
    let bsp = Arc::new(Bsp::load(Path::new(MAP)).unwrap());
    let endpoint = quinn::Endpoint::server(
        server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let cfg = ZoneConfig {
        max_ticks: Some(64 * 3),
        report_every: Duration::from_secs(1),
        ..ZoneConfig::default()
    };
    let server = tokio::spawn(gm_server::run(cfg, world, endpoint, std::future::pending()));

    let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client.set_default_client_config(client_config(std::slice::from_ref(&identity.cert)).unwrap());
    let mut bots = Vec::new();
    for (i, behaviour) in [Behaviour::Hunter, Behaviour::Hold].into_iter().enumerate() {
        let client = client.clone();
        let bsp = bsp.clone();
        bots.push(tokio::spawn(async move {
            run_bot(
                &client,
                addr,
                BotConfig {
                    name: format!("loop{i}"),
                    seed: 10 + i as u64,
                    behaviour,
                    rate: TickRate::COMBAT,
                    run_ticks: 128,
                    build: None,
                    team: 0,
                    counter_pick: false,
                    travel_to: None,
                    travel_after_ticks: 0,
                    stall_tile: None,
                    aim: Default::default(),
                    report_after_ticks: 0,
                    say: None,
                },
                bsp,
                std::future::pending(),
            )
            .await
        }));
    }
    for b in bots {
        let r = b.await.unwrap().unwrap();
        println!(
            "{}: snaps {} corrections {} rtt {:.2} ms tx {:.0} rx {:.0} B/s",
            r.name,
            r.client.snapshots,
            r.client.corrections,
            r.rtt_ms,
            r.tx_bytes_per_s(),
            r.rx_bytes_per_s()
        );
        assert!(
            r.client.snapshots > 64,
            "{}: {} snapshots",
            r.name,
            r.client.snapshots
        );
        assert_eq!(r.client.decode_errors, 0);
    }
    let report = server.await.unwrap().unwrap();
    println!("server: {report:?}");
    assert_eq!(report.joins, 2);
    assert!(report.executed_frames >= 200, "{}", report.executed_frames);
}

/// WEB.md 7: a bot on QUIC and a bot on WebTransport in one zone, over real UDP. Both get
/// their snapshots, both are counted as joins, and the zone cannot tell them apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quic_and_webtransport_bots_share_a_zone() {
    use gm_bot::run_bot_web;
    use gm_net::link::web_listener;
    use gm_net::transport::web_transport_config;

    let identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(MAP)).unwrap());
    let bsp = Arc::new(Bsp::load(Path::new(MAP)).unwrap());
    let endpoint = quinn::Endpoint::server(
        server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let (web_endpoint, web_addr) = web_listener(
        "127.0.0.1:0".parse().unwrap(),
        None,
        None,
        web_transport_config(),
    )
    .await
    .unwrap();
    let cfg = ZoneConfig {
        max_ticks: Some(64 * 3),
        report_every: Duration::from_secs(1),
        ..ZoneConfig::default()
    };
    let web = gm_server::WebListener {
        endpoint: web_endpoint,
        origins: Vec::new(),
    };
    let server = tokio::spawn(gm_server::run_with_web(
        cfg,
        world,
        endpoint,
        Some(web),
        std::future::pending(),
    ));
    let bot = |name: &str, seed: u64, behaviour| BotConfig {
        name: name.into(),
        seed,
        behaviour,
        rate: TickRate::COMBAT,
        run_ticks: 128,
        build: None,
        team: 0,
        counter_pick: false,
        travel_to: None,
        travel_after_ticks: 0,
        stall_tile: None,
        aim: Default::default(),
        report_after_ticks: 0,
        say: None,
    };

    let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client.set_default_client_config(client_config(std::slice::from_ref(&identity.cert)).unwrap());
    let native = {
        let bsp = bsp.clone();
        let cfg = bot("native", 10, Behaviour::Hunter);
        tokio::spawn(async move { run_bot(&client, addr, cfg, bsp, std::future::pending()).await })
    };
    let web = {
        let bsp = bsp.clone();
        let cfg = bot("web", 11, Behaviour::Hunter);
        tokio::spawn(async move {
            run_bot_web(
                &web_addr,
                cfg,
                Vec::new(),
                move |_| Ok(bsp.clone()),
                std::future::pending(),
            )
            .await
            .map(|(report, _, _)| report)
        })
    };
    for b in [native, web] {
        let r = b.await.unwrap().unwrap();
        println!(
            "{}: snaps {} corrections {} ({} unexplained) others {} rtt {:.2} ms tx {:.0} rx {:.0} B/s",
            r.name,
            r.client.snapshots,
            r.client.corrections,
            r.client.corrections_unexplained,
            r.others_seen_max,
            r.rtt_ms,
            r.tx_bytes_per_s(),
            r.rx_bytes_per_s()
        );
        assert!(
            r.client.snapshots > 64,
            "{}: {} snapshots",
            r.name,
            r.client.snapshots
        );
        assert_eq!(r.client.decode_errors, 0);
        assert_eq!(r.send_failures, 0, "{}", r.name);
        assert!(r.others_seen_max >= 1, "{} saw nobody", r.name);
    }
    let report = server.await.unwrap().unwrap();
    assert_eq!(report.joins, 2);
    assert!(report.executed_frames >= 200, "{}", report.executed_frames);
}

/// Chat is checked and limited before it is relayed (PROTOCOL.md 8): a line with a line
/// break in it goes nowhere, a client may say five lines at once and one more every two
/// seconds, the line too many is refused to its sender alone, and a flood ends the
/// connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_is_checked_and_limited() {
    let identity = Identity::generate(&["localhost"]).unwrap();
    let world = Arc::new(ZoneWorld::load(Path::new(MAP)).unwrap());
    let bsp = Arc::new(Bsp::load(Path::new(MAP)).unwrap());
    let endpoint = quinn::Endpoint::server(
        server_config(&identity).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let cfg = ZoneConfig {
        max_ticks: Some(64 * 9),
        report_every: Duration::from_secs(1),
        ..ZoneConfig::default()
    };
    let server = tokio::spawn(gm_server::run(cfg, world, endpoint, std::future::pending()));

    let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client.set_default_client_config(client_config(std::slice::from_ref(&identity.cert)).unwrap());
    // A flood (a line every tick), a polite talker (every two seconds), and one whose
    // line has a line break in it.
    let talkers = [
        ("flood", 1.0 / 64.0),
        ("hello there", 2.0),
        ("two\nlines", 1.0),
    ];
    let mut bots = Vec::new();
    for (i, (line, every)) in talkers.into_iter().enumerate() {
        let client = client.clone();
        let bsp = bsp.clone();
        bots.push(tokio::spawn(async move {
            run_bot(
                &client,
                addr,
                BotConfig {
                    name: format!("talker{i}"),
                    seed: 20 + i as u64,
                    behaviour: Behaviour::Hold,
                    rate: TickRate::COMBAT,
                    run_ticks: 64 * 6,
                    build: None,
                    team: 0,
                    counter_pick: false,
                    travel_to: None,
                    travel_after_ticks: 0,
                    stall_tile: None,
                    aim: Default::default(),
                    report_after_ticks: 0,
                    say: Some((line.to_string(), every)),
                },
                bsp,
                std::future::pending(),
            )
            .await
        }));
    }
    let mut reports = Vec::new();
    for b in bots {
        reports.push(b.await.unwrap().unwrap());
    }
    let (flood, polite, broken) = (&reports[0], &reports[1], &reports[2]);
    println!(
        "flood heard {:?}\npolite heard {:?}",
        flood.heard, polite.heard
    );
    // The flood: its first five lines went out, the next were refused to it alone, and
    // after thirty refusals the zone let it go.
    assert_eq!(flood.kicked.as_deref(), Some("flooding the chat"));
    assert!(
        flood
            .heard
            .iter()
            .any(|(from, text)| *from == 0 && text == "too many lines; wait a moment"),
        "{:?}",
        flood.heard
    );
    let floods = polite.heard.iter().filter(|(_, t)| t == "flood").count();
    assert!(
        (4..=7).contains(&floods),
        "{floods} lines of the flood were relayed"
    );
    assert!(
        polite.heard.iter().all(|(from, _)| *from != 0),
        "a refusal is its sender's alone: {:?}",
        polite.heard
    );
    // The polite talker was heard, every time; a line with a line break in it never.
    let hellos = broken
        .heard
        .iter()
        .filter(|(_, t)| t == "hello there")
        .count();
    assert!((2..=3).contains(&hellos), "{:?}", broken.heard);
    assert!(polite.kicked.is_none() && broken.kicked.is_none());
    for r in &reports {
        assert!(
            r.heard.iter().all(|(_, t)| !t.contains('\n')),
            "{:?}",
            r.heard
        );
    }
    let report = server.await.unwrap().unwrap();
    assert_eq!(report.chat_dropped, 0);
    assert!(
        (6..=10).contains(&report.chat_lines),
        "{}",
        report.chat_lines
    );
}
