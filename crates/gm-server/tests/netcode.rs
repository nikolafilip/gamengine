//! Phase 2 acceptance (PLAN.md 11.8, PROTOCOL.md 9): 16 bots on one zone under turmoil with
//! 150 ms round trip (75 ± 10 ms per hop) and 3% independent datagram loss per direction.
//! Everything here runs in simulated time; a 30-second match takes a few wall-clock seconds.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gm_bot::{Behaviour, BotConfig, BotReport, run_bot};
use gm_bsp::Bsp;
use gm_core::tick::TickRate;
use gm_net::sim::TurmoilSocket;
use gm_net::transport::{Identity, client_config, server_config};
use gm_server::{ZoneConfig, ZoneReport, ZoneWorld};
use quinn::{Endpoint, EndpointConfig, TokioRuntime};

const MAP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/test_room.bsp"
);
const BUDGETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../budgets.toml");
const PORT: u16 = 4433;

/// `budgets.toml [net]` values without a TOML dependency: `key = value` lines after `[net]`.
fn net_budget(key: &str) -> f64 {
    let text = std::fs::read_to_string(BUDGETS).expect("budgets.toml");
    let mut in_net = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_net = line == "[net]";
            continue;
        }
        if in_net
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            let v = v.split('#').next().unwrap().trim();
            return v.parse().expect("numeric budget");
        }
    }
    panic!("budgets.toml has no [net] {key}");
}

struct Match {
    bots: usize,
    seconds: u64,
    loss: f64,
    one_way_ms: (u64, u64),
    behaviours: Vec<Behaviour>,
    seed: u64,
}

struct Outcome {
    server: ZoneReport,
    bots: Vec<BotReport>,
}

fn play(m: Match) -> Outcome {
    // `GM_TRACE=gm_net::client=debug` prints every unexplained correction.
    if let Ok(filter) = std::env::var("GM_TRACE") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    }
    let identity = Identity::generate(&["server"]).unwrap();
    let cert = identity.cert.clone();
    let world_bsp = Arc::new(Bsp::load(Path::new(MAP)).expect("test map built"));
    let world_bytes = std::fs::read(MAP).unwrap();
    let zone_world = Arc::new(ZoneWorld::from_bsp(
        (*world_bsp).clone(),
        "test_room",
        gm_net::transport::fnv1a64(&world_bytes),
    ));
    let server_report: Arc<Mutex<Option<ZoneReport>>> = Arc::new(Mutex::new(None));
    let bot_reports: Arc<Mutex<Vec<BotReport>>> = Arc::new(Mutex::new(Vec::new()));

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(m.seconds + 30))
        .min_message_latency(Duration::from_millis(m.one_way_ms.0))
        .max_message_latency(Duration::from_millis(m.one_way_ms.1))
        .tick_duration(Duration::from_millis(1))
        .udp_capacity(1024)
        .rng_seed(m.seed)
        .build();

    let ticks = m.seconds * TickRate::COMBAT.hz() as u64 + TickRate::COMBAT.hz() as u64 * 2;
    {
        let identity = identity.clone();
        let zone_world = zone_world.clone();
        let server_report = server_report.clone();
        let loss = m.loss;
        let seed = m.seed;
        sim.host("server", move || {
            let identity = identity.clone();
            let zone_world = zone_world.clone();
            let server_report = server_report.clone();
            async move {
                let socket = TurmoilSocket::bind(("0.0.0.0", PORT), loss, seed ^ 0x5eed).await?;
                let endpoint = Endpoint::new_with_abstract_socket(
                    EndpointConfig::default(),
                    Some(server_config(&identity)?),
                    socket,
                    Arc::new(TokioRuntime),
                )?;
                let cfg = ZoneConfig {
                    rate: TickRate::COMBAT,
                    open: true,
                    seed,
                    max_players: 64,
                    report_every: Duration::from_secs(5),
                    max_ticks: Some(ticks),
                    report_tx: None,
                };
                let report =
                    gm_server::run(cfg, zone_world, endpoint, std::future::pending()).await?;
                *server_report.lock().unwrap() = Some(report);
                Ok(())
            }
        });
    }

    let server_ip: IpAddr = sim.lookup("server");
    let server_addr = SocketAddr::new(server_ip, PORT);
    for i in 0..m.bots {
        let name = format!("bot{i:02}");
        let cert = cert.clone();
        let world = world_bsp.clone();
        let reports = bot_reports.clone();
        let behaviour = m.behaviours[i % m.behaviours.len()];
        let loss = m.loss;
        let seed = m.seed * 1000 + i as u64;
        let secs = m.seconds;
        sim.client(name.clone(), async move {
            // Stagger joins over the first second so the zone sees a realistic trickle.
            tokio::time::sleep(Duration::from_millis(50 * i as u64)).await;
            let socket = TurmoilSocket::bind(("0.0.0.0", 0), loss, seed ^ 0xb07).await?;
            let mut endpoint = Endpoint::new_with_abstract_socket(
                EndpointConfig::default(),
                None,
                socket,
                Arc::new(TokioRuntime),
            )?;
            endpoint.set_default_client_config(client_config(&[cert])?);
            let cfg = BotConfig {
                name,
                seed,
                behaviour,
                rate: TickRate::COMBAT,
                run_ticks: 0,
            };
            let report = run_bot(
                &endpoint,
                server_addr,
                cfg,
                world,
                tokio::time::sleep(Duration::from_secs(secs)),
            )
            .await?;
            reports.lock().unwrap().push(report);
            Ok(())
        });
    }

    sim.run().expect("simulation completes");
    // Let the server host finish its last ticks.
    let server = loop {
        if let Some(r) = server_report.lock().unwrap().clone() {
            break r;
        }
        sim.step().expect("server finishes");
    };
    let bots = std::mem::take(&mut *bot_reports.lock().unwrap());
    Outcome { server, bots }
}

fn summarize(o: &Outcome) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "server: ticks {} joins {} executed {} starved {} dropped {} oversize {} sendfail {} hits melee {} proj {} kills {} tx/player/s {:.0} rx/player/s {:.0} (max {:.0}/{:.0})\n",
        o.server.tick, o.server.joins, o.server.executed_frames, o.server.starved_ticks,
        o.server.dropped_frames, o.server.oversize_drops, o.server.send_failures,
        o.server.hits_melee, o.server.hits_projectile, o.server.kills,
        o.server.tx_bytes_per_player_s, o.server.rx_bytes_per_player_s,
        o.server.max_tx_bytes_per_player_s, o.server.max_rx_bytes_per_player_s
    ));
    for b in &o.bots {
        s.push_str(&format!(
            "{}: ticks {} secs {:.1} snaps {} gaps {} maxgap {} corrections {} (unexplained {}) maxcorr {:.1} unknown {} decode_err {} tx {:.0} B/s rx {:.0} B/s rtt {:.0} ms hp {} kills {} deaths {} others_max {}\n",
            b.name, b.ticks, b.secs, b.client.snapshots, b.client.gaps, b.client.max_gap,
            b.client.corrections, b.client.corrections_unexplained, b.client.max_correction, b.client.unknown_baseline,
            b.client.decode_errors, b.tx_bytes_per_s(), b.rx_bytes_per_s(), b.rtt_ms,
            b.final_health, b.own_kills, b.own_deaths, b.others_seen_max
        ));
    }
    s
}

#[test]
fn sixteen_players_at_150ms_and_3_percent_loss() {
    let secs = 30;
    let o = play(Match {
        bots: 16,
        seconds: secs,
        loss: net_budget("test_loss"),
        one_way_ms: (65, 85),
        behaviours: vec![Behaviour::Wander, Behaviour::Wander, Behaviour::Hunter],
        seed: 7,
    });
    let text = summarize(&o);
    println!("{text}");
    assert_eq!(o.bots.len(), 16, "every bot finished");
    assert_eq!(o.server.joins, 16);

    // 5. Bandwidth, both directions, QUIC's own UDP byte counters.
    let budget = net_budget("max_bytes_per_player_s");
    for b in &o.bots {
        assert!(
            b.rx_bytes_per_s() < budget,
            "{}: {} B/s down",
            b.name,
            b.rx_bytes_per_s()
        );
        assert!(
            b.tx_bytes_per_s() < budget,
            "{}: {} B/s up",
            b.name,
            b.tx_bytes_per_s()
        );
    }
    assert!(o.server.max_tx_bytes_per_player_s < budget, "{text}");
    assert!(o.server.max_rx_bytes_per_player_s < budget, "{text}");

    // Basic liveness: snapshots flowed at roughly the tick rate, inputs ran.
    for b in &o.bots {
        let expected = (b.secs * 64.0) as u64;
        assert!(
            b.client.snapshots as f64 > expected as f64 * 0.9,
            "{}: {} snapshots in {:.1} s",
            b.name,
            b.client.snapshots,
            b.secs
        );
        assert_eq!(b.client.decode_errors, 0, "{}", b.name);
        assert_eq!(b.client.unknown_baseline, 0, "{}", b.name);
        assert!(
            b.others_seen_max >= 10,
            "{}: saw only {} others",
            b.name,
            b.others_seen_max
        );
    }
    let total_ticks: f64 = o.bots.iter().map(|b| b.ticks as f64).sum();
    assert!(
        o.server.executed_frames as f64 > total_ticks * 0.99,
        "frames executed {} of {} sent",
        o.server.executed_frames,
        total_ticks
    );

    // 2. Input starvation under 1% of ticks per bot on average.
    let server_ticks_with_players = o.server.tick as f64 * 16.0;
    assert!(
        (o.server.starved_ticks as f64) < server_ticks_with_players * 0.01 + 16.0 * 128.0,
        "starved {} of {}",
        o.server.starved_ticks,
        server_ticks_with_players
    );

    // 3. Snapshot gaps: never more than 4 consecutive ticks missing.
    for b in &o.bots {
        assert!(
            b.client.max_gap <= 4,
            "{}: max gap {}",
            b.name,
            b.client.max_gap
        );
    }

    // 1. Unexplained corrections (not a hit, death, respawn or body-block): fewer than one per
    // 10 s of movement per bot.
    for b in &o.bots {
        let allowed = (b.secs / 10.0).ceil() as u64;
        assert!(
            b.client.corrections_unexplained <= allowed,
            "{}: {} unexplained of {} corrections (max {:.1} u) in {:.0} s",
            b.name,
            b.client.corrections_unexplained,
            b.client.corrections,
            b.client.max_correction,
            b.secs
        );
    }

    // 4. Combat registers under latency: melee with lag compensation and projectiles both hit.
    assert!(o.server.hits_melee > 0, "no melee hits\n{text}");
    assert!(o.server.hits_projectile > 0, "no projectile hits\n{text}");
    assert_eq!(
        o.server.oversize_drops, 0,
        "snapshots exceeded the datagram budget"
    );
}

#[test]
fn two_players_perfect_network_agree_exactly() {
    let o = play(Match {
        bots: 2,
        seconds: 8,
        loss: 0.0,
        one_way_ms: (5, 5),
        behaviours: vec![Behaviour::Wander],
        seed: 3,
    });
    let text = summarize(&o);
    println!("{text}");
    assert_eq!(o.bots.len(), 2);
    for b in &o.bots {
        assert_eq!(b.client.gaps, 0, "{}", b.name);
        assert_eq!(
            b.client.corrections_unexplained, 0,
            "{}: {} unexplained of {} corrections",
            b.name, b.client.corrections_unexplained, b.client.corrections
        );
        assert!(b.client.snapshots > 8 * 64 - 64, "{}", b.name);
    }
}
