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
    listen: SocketAddr,
    cert_out: PathBuf,
    hz: u32,
    report_secs: u64,
    ticks: Option<u64>,
    max_players: usize,
    seed: u64,
}

const USAGE: &str = "gm-server [--map PATH] [--listen ADDR] [--cert-out PATH] [--hz 64|20] \
[--report-secs N] [--ticks N] [--max-players N] [--seed N]";

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        map: PathBuf::from("assets/maps/built/test_room.bsp"),
        listen: "127.0.0.1:4433".parse().unwrap(),
        cert_out: PathBuf::from("zone-cert.der"),
        hz: TickRate::COMBAT.hz(),
        report_secs: 5,
        ticks: None,
        max_players: 64,
        seed: 1,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--map" => args.map = PathBuf::from(value("--map")?),
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
    let identity = Identity::generate(&["localhost"])?;
    std::fs::write(&args.cert_out, identity.cert_der())?;
    info!(cert = %args.cert_out.display(), "zone certificate written; clients pass it with --cert");
    let endpoint = quinn::Endpoint::server(server_config(&identity)?, args.listen)?;
    info!(listen = %endpoint.local_addr()?, map = %world.name, hash = format_args!("{:016x}", world.hash), "listening");

    let cfg = ZoneConfig {
        rate: TickRate::new(args.hz),
        open: true,
        seed: args.seed,
        max_players: args.max_players,
        report_every: Duration::from_secs(args.report_secs.max(1)),
        max_ticks: args.ticks,
        report_tx: None,
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
        hits = report.hits_melee + report.hits_projectile,
        kills = report.kills,
        max_tx_bps = format_args!("{:.0}", report.max_tx_bytes_per_player_s),
        max_rx_bps = format_args!("{:.0}", report.max_rx_bytes_per_player_s),
        "final report"
    );
    Ok(())
}
