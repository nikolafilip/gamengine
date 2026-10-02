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
    /// Connect through the zone's WebTransport listener instead (WEB.md 7): its URL and the
    /// SHA-256 of its certificate in hex (none: a publicly trusted certificate).
    web: Option<String>,
    web_cert: Option<String>,
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
    /// How each bot's view moves, cycled like the builds (ANTICHEAT.md 10).
    aims: Vec<gm_bot::AimModel>,
    /// The first bot reports the first enemy it sees after this many seconds (0 = never).
    report_after: u64,
    /// The first bot says this line in the zone's chat, every `say_every` seconds.
    say: Option<String>,
    say_every: u64,
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
    list_for_hire: Option<i64>,
    hire: usize,
    /// Keepers of a stall put what they carry up for sale at this price.
    sell_at: Option<i64>,
    /// The first bot invites the character of this name into a party, and asks again
    /// until it is in one with it (PARTY.md 9).
    invite: Option<String>,
    /// Join whoever invites, trade with whoever asks, answer a party's line and a
    /// whisper.
    sociable: bool,
    /// In a trade: offer one thing it carries and accept when the other offers at least
    /// this much coin.
    trade_for: Option<i64>,
}

const USAGE: &str = "gm-bot (--connect ADDR --cert PATH | --web https://HOST:PORT [--web-cert SHA256HEX]) [--map PATH] [--bots N] [--secs N] \
[--behaviour wander|hunter|hold|duelist|stroll|raid] [--seed N] [--builds a,b,...] [--teams 1,2,...] [--counter-pick] [--aim brain|hand|lock|flick,...] [--report-after SECS] [--say TEXT [--say-every SECS]]\n\
       gm-bot --hub ADDR --hub-cert PATH --user EMAIL --password PW [--register] --character NAME --zone ID \
[--travel-to ZONE --travel-after SECS] [--maps-dir DIR] [--secs N] [--behaviour ...] \
[--bots N: one account each, {i} in --user and --character is the bot's number] [--stalls N: the first N open a stall] \
[--list-for-hire COPPER: list the character in the tavern; with --secs 0 it then stays offline] \
[--hire N: hire up to N avatars from the tavern before entering] \
[--sell-at COPPER: a bot that keeps a stall lists whatever it carries that can be worn, at this price] \
[--invite NAME: the first bot asks that character into a party] [--sociable: join whoever invites, trade with whoever asks, answer lines] \
[--trade-for COPPER: in a trade, offer one thing carried and accept for that much coin]";

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        connect: "127.0.0.1:4433".parse().unwrap(),
        cert: PathBuf::from("zone-cert.der"),
        web: None,
        web_cert: None,
        map: PathBuf::from("assets/maps/built/test_room.bsp"),
        bots: 16,
        secs: 30,
        behaviour: Behaviour::Wander,
        seed: 1,
        builds: Vec::new(),
        teams: Vec::new(),
        counter_pick: false,
        aims: Vec::new(),
        report_after: 0,
        say: None,
        say_every: 5,
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
        list_for_hire: None,
        hire: 0,
        sell_at: None,
        invite: None,
        sociable: false,
        trade_for: None,
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
            "--web" => a.web = Some(value("--web")?),
            "--web-cert" => a.web_cert = Some(value("--web-cert")?),
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
                    "raid" => Behaviour::Raid,
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
            "--aim" => {
                a.aims = value("--aim")?
                    .split(',')
                    .map(|m| {
                        gm_bot::AimModel::parse(m.trim())
                            .ok_or_else(|| format!("--aim: {m} is not brain, hand, lock or flick"))
                    })
                    .collect::<Result<_, _>>()?
            }
            "--say" => a.say = Some(value("--say")?),
            "--say-every" => {
                a.say_every = value("--say-every")?
                    .parse()
                    .map_err(|e| format!("--say-every: {e}"))?
            }
            "--report-after" => {
                a.report_after = value("--report-after")?
                    .parse()
                    .map_err(|e| format!("--report-after: {e}"))?
            }
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
            "--list-for-hire" => {
                a.list_for_hire = Some(
                    value("--list-for-hire")?
                        .parse()
                        .map_err(|e| format!("--list-for-hire: {e}"))?,
                )
            }
            "--hire" => {
                a.hire = value("--hire")?
                    .parse()
                    .map_err(|e| format!("--hire: {e}"))?
            }
            "--sell-at" => {
                let price: i64 = value("--sell-at")?
                    .parse()
                    .map_err(|e| format!("--sell-at: {e}"))?;
                if price <= 0 {
                    return Err("--sell-at: a price is more than nothing".into());
                }
                a.sell_at = Some(price);
            }
            "--invite" => a.invite = Some(value("--invite")?),
            "--sociable" => a.sociable = true,
            "--trade-for" => {
                a.trade_for = Some(
                    value("--trade-for")?
                        .parse()
                        .map_err(|e| format!("--trade-for: {e}"))?,
                )
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

/// A raid leader's run in one line (`scripts/check-dungeon.sh` reads it).
fn raid_line(r: &gm_bot::BotReport) -> String {
    let passed: Vec<&str> = r
        .trials
        .iter()
        .filter(|t| t.1)
        .map(|t| t.0.as_str())
        .collect();
    let cleared: Vec<String> = r.cleared.iter().map(|(n, s)| format!("{n}:{s}s")).collect();
    format!(
        "raid {}: done={} secs={:.1} squad={} hired={} orders={} refused={} cleared=[{}] resets={} deaths={} loot=[{}] coin={} trials_passed=[{}] creatures={} creature_health_seen={} rx_bytes_per_s={:.0} tx_bytes_per_s={:.0} corrections={} unexplained={}",
        r.name,
        r.raid_done,
        r.secs,
        r.squad_max,
        r.squad_hired,
        r.orders,
        r.orders_refused,
        cleared.join(","),
        r.resets,
        r.own_deaths,
        r.loot.join(","),
        r.coin,
        passed.join(","),
        r.creatures_announced,
        r.creature_health_seen,
        r.rx_bytes_per_s(),
        r.tx_bytes_per_s(),
        r.client.corrections,
        r.client.corrections_unexplained,
    )
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
                    aim: aim_of(&args.aims, i),
                    report_after_ticks: if i == 0 {
                        (args.report_after * 64) as u32
                    } else {
                        0
                    },
                    say: say_of(&args, i),
                    social: gm_bot::bot::Social {
                        invite: args.invite.clone().filter(|_| i == 0),
                        sociable: args.sociable,
                        trades: None,
                    },
                },
                play: Duration::from_secs(args.secs),
                list_for_hire: args.list_for_hire,
                sell_at: args.sell_at,
                trade_for: args.trade_for,
                hire: args.hire,
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
        print_heard(&reports);
        for rep in reports
            .iter()
            .filter(|r| r.squad_max > 0 || !r.cleared.is_empty())
        {
            println!("{}", raid_line(rep));
        }
        for f in flows.iter().filter(|_| !swarm || args.hire > 0) {
            println!(
                "character {}: squad=[{}] coin={} items=[{}] trials=[{}]",
                f.character,
                f.squad.join(","),
                f.coin,
                f.items.join(","),
                f.trials.join(",")
            );
        }
        if reports.is_empty() {
            // Nobody entered a zone (--secs 0): the listing or the hiring was the trip.
            println!("hub bots={} completed={}", count, flows.len());
            return Ok(());
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
    let web = match &args.web {
        Some(url) => {
            let cert_sha256 = match &args.web_cert {
                Some(hex) => Some(
                    gm_model_hex(hex)
                        .ok_or_else(|| anyhow::anyhow!("--web-cert: 64 hex digits"))?,
                ),
                None => None,
            };
            Some(gm_net::control::WebAddr {
                url: url.clone(),
                cert_sha256,
            })
        }
        None => None,
    };
    // Bots on the web listener need no QUIC certificate.
    let cert = match &web {
        Some(_) => None,
        None => Some(CertificateDer::from(std::fs::read(&args.cert)?)),
    };
    let world = Arc::new(Bsp::load(&args.map)?);
    let bind: SocketAddr = if args.connect.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    if let Some(cert) = cert {
        endpoint.set_default_client_config(client_config(&[cert])?);
    }

    let mut set = tokio::task::JoinSet::new();
    for i in 0..args.bots {
        let endpoint = endpoint.clone();
        let world = world.clone();
        let cfg = BotConfig {
            name: match aim_of(&args.aims, i) {
                gm_bot::AimModel::Brain => format!("bot{i:02}"),
                model => format!("{model:?}{i:02}").to_lowercase(),
            },
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
            aim: aim_of(&args.aims, i),
            report_after_ticks: if i == 0 {
                (args.report_after * 64) as u32
            } else {
                0
            },
            say: say_of(&args, i),
            social: Default::default(),
        };
        let secs = args.secs;
        let web = web.clone();
        set.spawn(async move {
            let stop = tokio::time::sleep(Duration::from_secs(secs));
            match web {
                Some(web) => {
                    gm_bot::run_bot_web(&web, cfg, Vec::new(), move |_| Ok(world.clone()), stop)
                        .await
                        .map(|(report, _, _)| report)
                }
                None => run_bot(&endpoint, args.connect, cfg, world, stop).await,
            }
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
    print_heard(&reports.iter().collect::<Vec<_>>());
    for r in reports
        .iter()
        .filter(|r| r.squad_max > 0 || !r.cleared.is_empty())
    {
        println!("{}", raid_line(r));
    }
    let respecs: u32 = reports.iter().map(|r| r.respecs).sum();
    if respecs > 0 {
        let mut finals: Vec<&str> = reports.iter().map(|r| r.final_build.as_str()).collect();
        finals.sort_unstable();
        println!("respecs {respecs}; final builds {finals:?}");
    }
    Ok(())
}

/// What bot `i` says in the chat, and every how many seconds: the first bot only.
fn say_of(args: &Args, i: usize) -> Option<(String, f32)> {
    match (&args.say, i) {
        (Some(line), 0) => Some((line.clone(), args.say_every.max(1) as f32)),
        _ => None,
    }
}

/// What the bots heard in the chat: one line each, for whoever scripted the run.
fn print_heard(reports: &[&gm_bot::BotReport]) {
    for r in reports {
        for (from, text) in &r.heard {
            println!("heard: {} from {from}: {text}", r.name);
        }
    }
}

/// The aim model of bot `i`: the list cycled, the brain's own view without one.
fn aim_of(aims: &[gm_bot::AimModel], i: usize) -> gm_bot::AimModel {
    if aims.is_empty() {
        gm_bot::AimModel::Brain
    } else {
        aims[i % aims.len()]
    }
}

/// 64 hex digits as 32 bytes.
fn gm_model_hex(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in hex.chunks_exact(2).enumerate() {
        let s = std::str::from_utf8(pair).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}
