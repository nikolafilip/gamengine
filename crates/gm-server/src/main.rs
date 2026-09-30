//! gm-server: the authoritative zone server (PLAN.md 2.1, 11.1).
//!
//! Phase 1 scope: the fixed-tick loop with timing metrics and nothing else. Ticks are scheduled on
//! an absolute timeline (`start + n * period`) so timing error never accumulates; if the process
//! falls more than a second behind it resynchronises instead of trying to catch up.
#![forbid(unsafe_code)]

mod tick;

use std::time::{Duration, Instant};

use gm_core::tick::TickRate;
use tracing::{info, warn};

use crate::tick::{TickMetrics, TickScheduler};

struct Args {
    hz: u32,
    report_secs: u64,
    ticks: Option<u64>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        hz: TickRate::COMBAT.hz(),
        report_secs: 5,
        ticks: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
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
            "-h" | "--help" => {
                println!("gm-server [--hz 64|20] [--report-secs N] [--ticks N]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if args.hz == 0 {
        return Err("--hz must be positive".into());
    }
    Ok(args)
}

/// The simulation step. Empty until Phase 2 wires gm-core and gm-net in.
fn simulate(_tick: u64) {}

#[tokio::main]
async fn main() {
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
    let rate = TickRate::new(args.hz);
    info!(
        hz = rate.hz(),
        period_us = rate.period().as_micros(),
        "zone server starting"
    );

    let mut scheduler = TickScheduler::new(rate, Instant::now());
    let mut metrics = TickMetrics::new(rate.period());
    let report_every = Duration::from_secs(args.report_secs.max(1));
    let mut last_report = Instant::now();
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    loop {
        let deadline = scheduler.next_deadline();
        tokio::select! {
            _ = &mut shutdown => {
                info!("ctrl-c: shutting down");
                break;
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
        }
        let started = Instant::now();
        if let Some(behind) = scheduler.resync_if_behind(started) {
            warn!(
                behind_ms = behind.as_millis(),
                "fell behind; resynchronising the tick timeline"
            );
        }
        simulate(scheduler.tick());
        metrics.record(started - deadline, started.elapsed());
        scheduler.advance();

        if last_report.elapsed() >= report_every {
            let s = metrics.summary();
            info!(
                tick = scheduler.tick(),
                ticks = s.count,
                tick_us_mean = format_args!("{:.1}", s.mean_us),
                tick_us_p99 = format_args!("{:.1}", s.p99_us),
                tick_us_max = format_args!("{:.1}", s.max_us),
                late_us_mean = format_args!("{:.1}", s.late_mean_us),
                late_us_max = format_args!("{:.1}", s.late_max_us),
                overruns = s.overruns,
                "tick report"
            );
            metrics.reset_window();
            last_report = Instant::now();
        }
        if args.ticks.is_some_and(|n| scheduler.tick() >= n) {
            break;
        }
    }
    let s = metrics.summary_total();
    info!(
        ticks = s.count,
        tick_us_mean = format_args!("{:.1}", s.mean_us),
        tick_us_max = format_args!("{:.1}", s.max_us),
        late_us_max = format_args!("{:.1}", s.late_max_us),
        overruns = s.overruns,
        "zone server stopped"
    );
}
