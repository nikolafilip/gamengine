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
}

const USAGE: &str = "gm-bot --connect ADDR --cert PATH [--map PATH] [--bots N] [--secs N] \
[--behaviour wander|hunter|hold] [--seed N]";

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        connect: "127.0.0.1:4433".parse().unwrap(),
        cert: PathBuf::from("zone-cert.der"),
        map: PathBuf::from("assets/maps/built/test_room.bsp"),
        bots: 16,
        secs: 30,
        behaviour: Behaviour::Wander,
        seed: 1,
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
                    other => return Err(format!("--behaviour: unknown {other}")),
                }
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
    Ok(())
}
