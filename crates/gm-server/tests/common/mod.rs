//! Shared turmoil harness: one zone host and N bot clients on a simulated network with
//! per-hop latency and independent datagram loss, everything in simulated time.
#![allow(dead_code)]

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

pub const TEST_ROOM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/test_room.bsp"
);
pub const ARENA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/maps/built/arena.bsp"
);
pub const BUDGETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../budgets.toml");
pub const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
const PORT: u16 = 4433;

/// `budgets.toml [net]` values without a TOML dependency: `key = value` lines after `[net]`.
pub fn net_budget(key: &str) -> f64 {
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

pub struct Match {
    pub map: &'static str,
    pub bots: usize,
    pub seconds: u64,
    pub loss: f64,
    pub one_way_ms: (u64, u64),
    pub behaviours: Vec<Behaviour>,
    pub seed: u64,
    /// Preset per bot (cycled); empty = the zone's default.
    pub builds: Vec<Option<String>>,
    /// Team per bot (cycled); empty = let the zone balance.
    pub teams: Vec<u8>,
    /// Teams whose bots counter-pick (empty: nobody).
    pub counter_pick_teams: Vec<u8>,
    pub report_every: Duration,
}

impl Match {
    pub fn simple(
        bots: usize,
        seconds: u64,
        loss: f64,
        one_way_ms: (u64, u64),
        seed: u64,
    ) -> Match {
        Match {
            map: TEST_ROOM,
            bots,
            seconds,
            loss,
            one_way_ms,
            behaviours: vec![Behaviour::Wander],
            seed,
            builds: Vec::new(),
            teams: Vec::new(),
            counter_pick_teams: Vec::new(),
            report_every: Duration::from_secs(5),
        }
    }
}

pub struct Outcome {
    pub server: ZoneReport,
    pub bots: Vec<BotReport>,
    /// One report per report window, in order.
    pub windows: Vec<ZoneReport>,
}

pub fn play(m: Match) -> Outcome {
    // `GM_TRACE=gm_net::client=debug` prints every unexplained correction.
    if let Ok(filter) = std::env::var("GM_TRACE") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    }
    let identity = Identity::generate(&["server"]).unwrap();
    let cert = identity.cert.clone();
    let world_bsp = Arc::new(Bsp::load(Path::new(m.map)).expect("map built"));
    let world_bytes = std::fs::read(m.map).unwrap();
    let name = Path::new(m.map)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let zone_world = Arc::new(ZoneWorld::from_bsp(
        (*world_bsp).clone(),
        &name,
        gm_net::transport::fnv1a64(&world_bytes),
    ));
    let content = gm_content::load_dir(Path::new(CONTENT), TickRate::COMBAT).expect("content");
    let server_report: Arc<Mutex<Option<ZoneReport>>> = Arc::new(Mutex::new(None));
    let windows: Arc<Mutex<Vec<ZoneReport>>> = Arc::new(Mutex::new(Vec::new()));
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
        let windows = windows.clone();
        let loss = m.loss;
        let seed = m.seed;
        let content = content.clone();
        let report_every = m.report_every;
        sim.host("server", move || {
            let identity = identity.clone();
            let zone_world = zone_world.clone();
            let server_report = server_report.clone();
            let windows = windows.clone();
            let content = content.clone();
            async move {
                let socket = TurmoilSocket::bind(("0.0.0.0", PORT), loss, seed ^ 0x5eed).await?;
                let endpoint = Endpoint::new_with_abstract_socket(
                    EndpointConfig::default(),
                    Some(server_config(&identity)?),
                    socket,
                    Arc::new(TokioRuntime),
                )?;
                let (tx, mut rx) = tokio::sync::watch::channel(ZoneReport::default());
                let recorder = {
                    let windows = windows.clone();
                    tokio::spawn(async move {
                        while rx.changed().await.is_ok() {
                            windows.lock().unwrap().push(rx.borrow().clone());
                        }
                    })
                };
                let cfg = ZoneConfig {
                    rate: TickRate::COMBAT,
                    open: true,
                    seed,
                    max_players: 64,
                    report_every,
                    max_ticks: Some(ticks),
                    report_tx: Some(tx),
                    content,
                    default_build: "blade".into(),
                    hub: None,
                };
                let report =
                    gm_server::run(cfg, zone_world, endpoint, std::future::pending()).await?;
                recorder.abort();
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
        let build = if m.builds.is_empty() {
            None
        } else {
            m.builds[i % m.builds.len()].clone()
        };
        let team = if m.teams.is_empty() {
            0
        } else {
            m.teams[i % m.teams.len()]
        };
        let counter_pick = m.counter_pick_teams.contains(&team);
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
                build,
                team,
                counter_pick,
                travel_to: None,
                travel_after_ticks: 0,
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
    let windows = std::mem::take(&mut *windows.lock().unwrap());
    Outcome {
        server,
        bots,
        windows,
    }
}

pub fn summarize(o: &Outcome) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "server: ticks {} joins {} executed {} starved {} dropped {} oversize {} sendfail {} hits melee {} proj {} area {} dot {} parries {} kills {} team_kills {:?} tx/player/s {:.0} rx/player/s {:.0} (max {:.0}/{:.0})\n",
        o.server.tick, o.server.joins, o.server.executed_frames, o.server.starved_ticks,
        o.server.dropped_frames, o.server.oversize_drops, o.server.send_failures,
        o.server.hits_melee, o.server.hits_projectile, o.server.hits_area, o.server.hits_dot,
        o.server.parries, o.server.kills, o.server.team_kills,
        o.server.tx_bytes_per_player_s, o.server.rx_bytes_per_player_s,
        o.server.max_tx_bytes_per_player_s, o.server.max_rx_bytes_per_player_s
    ));
    for b in &o.bots {
        s.push_str(&format!(
            "{}: team {} build {} ticks {} secs {:.1} snaps {} gaps {} maxgap {} corrections {} (unexplained {}) maxcorr {:.1} unknown {} decode_err {} tx {:.0} B/s rx {:.0} B/s rtt {:.0} ms hp {} kills {} deaths {} respecs {} others_max {}\n",
            b.name, b.team, b.final_build, b.ticks, b.secs, b.client.snapshots, b.client.gaps, b.client.max_gap,
            b.client.corrections, b.client.corrections_unexplained, b.client.max_correction, b.client.unknown_baseline,
            b.client.decode_errors, b.tx_bytes_per_s(), b.rx_bytes_per_s(), b.rtt_ms,
            b.final_health, b.own_kills, b.own_deaths, b.respecs, b.others_seen_max
        ));
    }
    s
}
