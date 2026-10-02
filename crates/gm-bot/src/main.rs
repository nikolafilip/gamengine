//! gm-bot binary: a swarm of bots against a running zone, with a bandwidth and feel summary.
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gm_bot::{Behaviour, BotConfig, run_bot};
use gm_bsp::Bsp;
use gm_core::tick::TickRate;
use gm_net::transport::client_config;
use rustls_pki::CertificateDer;

mod rustls_pki {
    pub use quinn::rustls::pki_types::CertificateDer;
}

struct Args {
    connect: SocketAddr,
    cert: PathBuf,
    map: PathBuf,
    bots: usize,
    secs: u64,
    behaviour: Behaviour,
    seed: u64,
    /// Preset builds, cycled over the bots (empty: the zone's default).
    builds: Vec<String>,
    /// Teams, cycled over the bots (empty: let the zone balance).
    teams: Vec<u8>,
    counter_pick: bool,
    /// Play through the hub instead of connecting to a zone directly.
    hub: Option<SocketAddr>,
    hub_cert: PathBuf,
    user: String,
    password: String,
    register: bool,
    character: String,
    zone: String,
    travel_to: Option<String>,
    travel_after: u64,
    maps_dir: PathBuf,
    /// The first N bots of a hub swarm walk to the market and open a stall.
    stalls: usize,
}

const USAGE: &str = "gm-bot --connect ADDR --cert PATH [--map PATH] [--bots N] [--secs N] \
[--behaviour wander|hunter|hold|duelist|stroll] [--seed N] [--builds a,b,...] [--teams 1,2,...] [--counter-pick]\n\
       gm-bot --hub ADDR --hub-cert PATH --user EMAIL --password PW [--register] --character NAME --zone ID \
[--travel-to ZONE --travel-after SECS] [--maps-dir DIR] [--secs N] [--behaviour ...] \
[--bots N: one account each, {i} in --user and --character is the bot's number] [--stalls N: the first N open a stall]";

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        connect: "127.0.0.1:4433".parse().unwrap(),
        cert: PathBuf::from("zone-cert.der"),
        map: PathBuf::from("assets/maps/built/test_room.bsp"),
        bots: 16,
        secs: 30,
        behaviour: Behaviour::Wander,
        seed: 1,
        builds: Vec::new(),
        teams: Vec::new(),
        counter_pick: false,
        hub: None,
        hub_cert: PathBuf::from("hub-cert.der"),
        user: String::new(),
        password: String::new(),
        register: false,
        character: String::new(),
        zone: "arena".into(),
        travel_to: None,
        travel_after: 10,
        maps_dir: PathBuf::from("assets/maps/built"),
        stalls: 0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--connect" => {
                a.connect = value("--connect")?
                    .parse()
                    .map_err(|e| format!("--connect: {e}"))?
            }
            "--cert" => a.cert = PathBuf::from(value("--cert")?),
            "--map" => a.map = PathBuf::from(value("--map")?),
            "--bots" => {
                a.bots = value("--bots")?
                    .parse()
                    .map_err(|e| format!("--bots: {e}"))?
            }
            "--secs" => {
                a.secs = value("--secs")?
                    .parse()
                    .map_err(|e| format!("--secs: {e}"))?
            }
            "--seed" => {
                a.seed = value("--seed")?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            "--behaviour" => {
                a.behaviour = match value("--behaviour")?.as_str() {
                    "wander" => Behaviour::Wander,
                    "hunter" => Behaviour::Hunter,
                    "hold" => Behaviour::Hold,
                    "duelist" => Behaviour::Duelist,
                    "stroll" => Behaviour::Stroll,
                    other => return Err(format!("--behaviour: unknown {other}")),
                }
            }
            "--builds" => {
                a.builds = value("--builds")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            "--counter-pick" => a.counter_pick = true,
            "--hub" => a.hub = Some(value("--hub")?.parse().map_err(|e| format!("--hub: {e}"))?),
            "--hub-cert" => a.hub_cert = PathBuf::from(value("--hub-cert")?),
            "--user" => a.user = value("--user")?,
            "--password" => a.password = value("--password")?,
            "--register" => a.register = true,
            "--character" => a.character = value("--character")?,
            "--zone" => a.zone = value("--zone")?,
            "--travel-to" => a.travel_to = Some(value("--travel-to")?),
            "--travel-after" => {
                a.travel_after = value("--travel-after")?
                    .parse()
                    .map_err(|e| format!("--travel-after: {e}"))?
            }
            "--maps-dir" => a.maps_dir = PathBuf::from(value("--maps-dir")?),
            "--stalls" => {
                a.stalls = value("--stalls")?
                    .parse()
                    .map_err(|e| format!("--stalls: {e}"))?
            }
            "--teams" => {
                a.teams = value("--teams")?
                    .split(',')
                    .map(|s| s.trim().parse().map_err(|e| format!("--teams: {e}")))
                    .collect::<Result<_, _>>()?
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(a)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("gm-bot: {e}");
            std::process::exit(2);
        }
    };
    if let Some(hub) = args.hub {
        if args.user.is_empty() || args.password.is_empty() || args.character.is_empty() {
            anyhow::bail!("--hub needs --user, --password and --character");
        }
        // One bot, or a swarm with an account each: `{i}` in the user and the character.
        let swarm = args.user.contains("{i}");
        let count = if swarm { args.bots } else { 1 };
        let hub_cert_der = std::fs::read(&args.hub_cert)?;
        let mut set = tokio::task::JoinSet::new();
        for i in 0..count {
            let number = format!("{i:03}");
            let cfg = gm_bot::HubFlowConfig {
                hub,
                hub_cert_der: hub_cert_der.clone(),
                email: args.user.replace("{i}", &number),
                password: args.password.clone(),
                register: args.register,
                character: args.character.replace("{i}", &number),
                preset: if args.builds.is_empty() {
                    "blade".into()
                } else {
                    args.builds[i % args.builds.len()].clone()
                },
                zone: args.zone.clone(),
                travel_after: args
                    .travel_to
                    .as_ref()
                    .map(|_| Duration::from_secs(args.travel_after)),
                travel_to: args.travel_to.clone(),
                maps_dir: args.maps_dir.clone(),
                bot: BotConfig {
                    name: String::new(),
                    seed: args.seed * 1000 + i as u64,
                    behaviour: args.behaviour,
                    rate: TickRate::COMBAT,
                    run_ticks: 0,
                    build: None,
                    team: 0,
                    counter_pick: args.counter_pick,
                    travel_to: None,
                    travel_after_ticks: 0,
                    // Every other tile, so the market fills evenly.
                    stall_tile: (i < args.stalls).then_some(i as u32 * 2),
                },
                play: Duration::from_secs(args.secs),
            };
            set.spawn(async move {
                // Arrive over a few seconds, like people do.
                tokio::time::sleep(Duration::from_millis(i as u64 * 25)).await;
                gm_bot::run_hub_flow(cfg).await
            });
        }
        let mut flows = Vec::new();
        while let Some(r) = set.join_next().await {
            match r? {
                Ok(flow) => flows.push(flow),
                Err(e) => eprintln!("bot failed: {e:#}"),
            }
        }
        if flows.is_empty() {
            anyhow::bail!("no bot completed");
        }
        if !swarm {
            let r = &flows[0];
            println!(
                "hub flow: character {} zones {:?} login {:.0} ms enter {:.0} ms travel {:.0} ms logout {:.0} ms",
                r.character, r.zones, r.login_ms, r.enter_ms, r.travel_ms, r.logout_ms
            );
        }
        let reports: Vec<&gm_bot::BotReport> = flows.iter().flat_map(|f| &f.reports).collect();
        for rep in reports.iter().take(if swarm { 0 } else { usize::MAX }) {
            println!(
                "  {}: {} snapshots, {} corrections, kills {} deaths {}",
                rep.name,
                rep.client.snapshots,
                rep.client.corrections,
                rep.own_kills,
                rep.own_deaths
            );
        }
        let n = reports.len().max(1) as f64;
        println!(
            "hub bots={} completed={} stalls_opened={} stalls_seen_max={} wearing_a_model={} models_seen_max={} roster_max={} revocations_max={} rx_bytes_per_s_avg={:.0} tx_bytes_per_s_avg={:.0} corrections_avg={:.2} unexplained_max={}",
            count,
            flows.len(),
            reports.iter().filter(|r| r.stall_opened).count(),
            reports.iter().map(|r| r.stalls_seen).max().unwrap_or(0),
            reports.iter().filter(|r| r.own_model.is_some()).count(),
            reports.iter().map(|r| r.models_seen).max().unwrap_or(0),
            reports.iter().map(|r| r.roster).max().unwrap_or(0),
            reports.iter().map(|r| r.revocations).max().unwrap_or(0),
            reports.iter().map(|r| r.rx_bytes_per_s()).sum::<f64>() / n,
            reports.iter().map(|r| r.tx_bytes_per_s()).sum::<f64>() / n,
            reports
                .iter()
                .map(|r| r.client.corrections as f64)
                .sum::<f64>()
                / n,
            reports
                .iter()
                .map(|r| r.client.corrections_unexplained)
                .max()
                .unwrap_or(0),
        );
        return Ok(());
    }
    let cert = CertificateDer::from(std::fs::read(&args.cert)?);
    let world = Arc::new(Bsp::load(&args.map)?);
    let bind: SocketAddr = if args.connect.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(client_config(&[cert])?);

    let mut set = tokio::task::JoinSet::new();
    for i in 0..args.bots {
        let endpoint = endpoint.clone();
        let world = world.clone();
        let cfg = BotConfig {
            name: format!("bot{i:02}"),
            seed: args.seed * 1000 + i as u64,
            behaviour: args.behaviour,
            rate: TickRate::COMBAT,
            run_ticks: 0,
            build: (!args.builds.is_empty()).then(|| args.builds[i % args.builds.len()].clone()),
            team: if args.teams.is_empty() {
                0
            } else {
                args.teams[i % args.teams.len()]
            },
            counter_pick: args.counter_pick,
            travel_to: None,
            travel_after_ticks: 0,
            stall_tile: None,
        };
        let secs = args.secs;
        set.spawn(async move {
            run_bot(
                &endpoint,
                args.connect,
                cfg,
                world,
                tokio::time::sleep(Duration::from_secs(secs)),
            )
            .await
        });
    }
    let mut reports = Vec::new();
    while let Some(r) = set.join_next().await {
        match r? {
            Ok(rep) => reports.push(rep),
            Err(e) => eprintln!("bot failed: {e:#}"),
        }
    }
    endpoint.wait_idle().await;

    if reports.is_empty() {
        anyhow::bail!("no bot completed");
    }
    let n = reports.len() as f64;
    let avg = |f: &dyn Fn(&gm_bot::BotReport) -> f64| reports.iter().map(f).sum::<f64>() / n;
    let max = |f: &dyn Fn(&gm_bot::BotReport) -> f64| reports.iter().map(f).fold(0.0, f64::max);
    println!("bots={} secs={:.1}", reports.len(), avg(&|r| r.secs));
    println!(
        "bytes/s per bot: tx avg {:.0} max {:.0} | rx avg {:.0} max {:.0}",
        avg(&|r| r.tx_bytes_per_s()),
        max(&|r| r.tx_bytes_per_s()),
        avg(&|r| r.rx_bytes_per_s()),
        max(&|r| r.rx_bytes_per_s())
    );
    println!(
        "rtt ms avg {:.1} | snapshots avg {:.0} | gaps avg {:.1} max_gap {} | corrections avg {:.2} max_corr {:.1} u",
        avg(&|r| r.rtt_ms),
        avg(&|r| r.client.snapshots as f64),
        avg(&|r| r.client.gaps as f64),
        reports.iter().map(|r| r.client.max_gap).max().unwrap_or(0),
        avg(&|r| r.client.corrections as f64),
        max(&|r| r.client.max_correction as f64)
    );
    println!(
        "unknown_baseline {} decode_errors {} send_failures {} kills_seen {} own_kills {} own_deaths {}",
        reports
            .iter()
            .map(|r| r.client.unknown_baseline)
            .sum::<u64>(),
        reports.iter().map(|r| r.client.decode_errors).sum::<u64>(),
        reports.iter().map(|r| r.send_failures).sum::<u64>(),
        reports.iter().map(|r| r.kills_seen).max().unwrap_or(0),
        reports.iter().map(|r| r.own_kills).sum::<u32>(),
        reports.iter().map(|r| r.own_deaths).sum::<u32>()
    );
    let respecs: u32 = reports.iter().map(|r| r.respecs).sum();
    if respecs > 0 {
        let mut finals: Vec<&str> = reports.iter().map(|r| r.final_build.as_str()).collect();
        finals.sort_unstable();
        println!("respecs {respecs}; final builds {finals:?}");
    }
    Ok(())
}
