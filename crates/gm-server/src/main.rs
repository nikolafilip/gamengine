//! gm-server binary: bind a QUIC endpoint, write the zone certificate for clients, run the zone.
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gm_core::tick::TickRate;
use gm_net::transport::{Identity, server_config};
use gm_server::{ZoneConfig, ZoneWorld};
use tracing::info;

struct Args {
    map: PathBuf,
    content: PathBuf,
    default_build: String,
    listen: SocketAddr,
    hub: Option<SocketAddr>,
    hub_cert: PathBuf,
    zone_id: String,
    zone_secret: String,
    public_addr: Option<SocketAddr>,
    cert_out: PathBuf,
    hz: u32,
    report_secs: u64,
    ticks: Option<u64>,
    max_players: usize,
    seed: u64,
    wild: bool,
    arrive_at_entry: bool,
    squads: bool,
    recruits: Vec<String>,
    requires: Vec<String>,
}

const USAGE: &str = "gm-server [--map PATH] [--content DIR] [--default-build NAME] [--listen ADDR] \
[--cert-out PATH] [--hz 64|20] [--report-secs N] [--ticks N] [--max-players N] [--seed N] \
[--wild] [--arrive-at-entry] [--squads] [--recruits BUILD,BUILD,...] \
[--hub ADDR --hub-cert PATH --zone-id NAME --zone-secret S [--public-addr ADDR] \
[--requires TRIAL,TRIAL,...]]   (env: GM_ZONE_SECRET)";

/// A comma-separated list of names.
fn names(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        map: PathBuf::from("assets/maps/built/test_room.bsp"),
        content: PathBuf::from("assets/content"),
        default_build: "blade".into(),
        listen: "127.0.0.1:4433".parse().unwrap(),
        hub: None,
        hub_cert: PathBuf::from("hub-cert.der"),
        zone_id: "arena".into(),
        zone_secret: std::env::var("GM_ZONE_SECRET").unwrap_or_default(),
        public_addr: None,
        cert_out: PathBuf::from("zone-cert.der"),
        hz: TickRate::COMBAT.hz(),
        report_secs: 5,
        ticks: None,
        max_players: 64,
        seed: 1,
        wild: false,
        arrive_at_entry: false,
        squads: false,
        recruits: Vec::new(),
        requires: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--map" => args.map = PathBuf::from(value("--map")?),
            "--content" => args.content = PathBuf::from(value("--content")?),
            "--default-build" => args.default_build = value("--default-build")?,
            "--hub" => args.hub = Some(value("--hub")?.parse().map_err(|e| format!("--hub: {e}"))?),
            "--hub-cert" => args.hub_cert = PathBuf::from(value("--hub-cert")?),
            "--zone-id" => args.zone_id = value("--zone-id")?,
            "--zone-secret" => args.zone_secret = value("--zone-secret")?,
            "--public-addr" => {
                args.public_addr = Some(
                    value("--public-addr")?
                        .parse()
                        .map_err(|e| format!("--public-addr: {e}"))?,
                )
            }
            "--listen" => {
                args.listen = value("--listen")?
                    .parse()
                    .map_err(|e| format!("--listen: {e}"))?
            }
            "--cert-out" => args.cert_out = PathBuf::from(value("--cert-out")?),
            "--hz" => args.hz = value("--hz")?.parse().map_err(|e| format!("--hz: {e}"))?,
            "--report-secs" => {
                args.report_secs = value("--report-secs")?
                    .parse()
                    .map_err(|e| format!("--report-secs: {e}"))?
            }
            "--ticks" => {
                args.ticks = Some(
                    value("--ticks")?
                        .parse()
                        .map_err(|e| format!("--ticks: {e}"))?,
                )
            }
            "--max-players" => {
                args.max_players = value("--max-players")?
                    .parse()
                    .map_err(|e| format!("--max-players: {e}"))?
            }
            "--seed" => {
                args.seed = value("--seed")?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            "--wild" => args.wild = true,
            "--arrive-at-entry" => args.arrive_at_entry = true,
            "--squads" => args.squads = true,
            "--recruits" => args.recruits = names(&value("--recruits")?),
            "--requires" => args.requires = names(&value("--requires")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if args.hz == 0 {
        return Err("--hz must be positive".into());
    }
    Ok(args)
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
            eprintln!("gm-server: {e}");
            std::process::exit(2);
        }
    };
    let world = Arc::new(ZoneWorld::load(&args.map)?);
    let rate = TickRate::new(args.hz);
    let content = gm_content::load_dir(&args.content, rate)?;
    if content.build(&args.default_build).is_none() {
        anyhow::bail!(
            "default build {:?} is not in {}",
            args.default_build,
            args.content.display()
        );
    }
    info!(
        abilities = content.abilities.len(),
        builds = content.builds.len(),
        creatures = content.creatures.len(),
        trials = content.trials.len(),
        dir = %args.content.display(),
        "content loaded"
    );
    // What the squads and the gate are made of has to exist before anybody joins.
    for name in &args.recruits {
        if content.build(name).is_none() {
            anyhow::bail!(
                "recruit build {name:?} is not in {}",
                args.content.display()
            );
        }
    }
    for key in &args.requires {
        if !content.trials.iter().any(|t| &t.key == key) {
            anyhow::bail!("trial {key:?} is not in {}", args.content.display());
        }
    }
    for post in &world.creature_posts {
        if content.creature(&post.creature).is_none() {
            anyhow::bail!(
                "the map posts a creature {:?} that is not in {}",
                post.creature,
                args.content.display()
            );
        }
    }
    if !args.recruits.is_empty() && !args.squads {
        anyhow::bail!("--recruits needs --squads");
    }
    if !args.requires.is_empty() && args.hub.is_none() {
        anyhow::bail!("--requires needs --hub: the hub is what remembers trials");
    }
    // A map that posts creatures is a wild zone (COMPANIONS.md 3.1).
    let wild = args.wild || !world.creature_posts.is_empty();
    let identity = Identity::generate(&["localhost"])?;
    std::fs::write(&args.cert_out, identity.cert_der())?;
    info!(cert = %args.cert_out.display(), "zone certificate written; clients pass it with --cert");
    let endpoint = quinn::Endpoint::server(server_config(&identity)?, args.listen)?;
    info!(listen = %endpoint.local_addr()?, map = %world.name, hash = format_args!("{:016x}", world.hash), "listening");
    let hub = match args.hub {
        Some(addr) => {
            if args.zone_secret.is_empty() {
                anyhow::bail!("--zone-secret (or GM_ZONE_SECRET) is required with --hub");
            }
            let cert_der = std::fs::read(&args.hub_cert)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", args.hub_cert.display()))?;
            Some(
                gm_server::HubLink::connect(gm_server::HubLinkConfig {
                    addr,
                    cert_der,
                    zone: args.zone_id.clone(),
                    secret: args.zone_secret.clone(),
                    map: world.name.clone(),
                    map_hash: world.hash,
                    public_addr: args.public_addr.unwrap_or(endpoint.local_addr()?),
                    zone_cert_der: identity.cert_der().to_vec(),
                    requires: args.requires.clone(),
                })
                .await?,
            )
        }
        None => None,
    };

    let cfg = ZoneConfig {
        rate,
        open: true,
        seed: args.seed,
        max_players: args.max_players,
        report_every: Duration::from_secs(args.report_secs.max(1)),
        max_ticks: args.ticks,
        report_tx: None,
        content,
        default_build: args.default_build,
        hub,
        wild,
        arrive_at_entry: args.arrive_at_entry,
        squads: args.squads,
        recruits: args.recruits,
    };
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let report = gm_server::run(cfg, world, endpoint, shutdown).await?;
    info!(
        ticks = report.tick,
        joins = report.joins,
        executed_frames = report.executed_frames,
        starved_ticks = report.starved_ticks,
        hits = report.hits_melee + report.hits_projectile + report.hits_area,
        kills = report.kills,
        team_kills = ?report.team_kills,
        max_tx_bps = format_args!("{:.0}", report.max_tx_bytes_per_player_s),
        max_rx_bps = format_args!("{:.0}", report.max_rx_bytes_per_player_s),
        encounters_cleared = report.encounters_cleared,
        encounters_reset = report.encounters_reset,
        trials_passed = report.trials_passed,
        "final report"
    );
    Ok(())
}
