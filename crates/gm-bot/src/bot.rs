//! One bot: connect, handshake, receive the content, then a fixed-step loop of predicted
//! inputs and snapshots.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use gm_bsp::Bsp;
use gm_core::build::Sheet;
use gm_core::tick::TickRate;
use gm_core::vocab::EntityId;
use gm_net::PROTOCOL_VERSION;
use gm_net::client::{ClientState, ClientStats};
use gm_net::control::{self, BuildChoice, Control};
use gm_net::transport::SERVER_NAME;
use tokio::time::Instant;
use tracing::{debug, info};

use crate::brain::{Behaviour, Brain, View, counter_pick, dominant_enemy_aspects};

/// Counter-picking bots look at the enemy every this many seconds (after the first look).
pub const COUNTER_PICK_PERIOD_S: u32 = 10;

#[derive(Clone, Debug)]
pub struct BotConfig {
    pub name: String,
    pub seed: u64,
    pub behaviour: Behaviour,
    pub rate: TickRate,
    /// Stop after this many client ticks (0 = run until `shutdown`).
    pub run_ticks: u32,
    /// Preset build to ask for (`None`: the zone's default).
    pub build: Option<String>,
    /// Team to ask for (0 = let the zone balance).
    pub team: u8,
    /// Re-spec to the preset that counters the enemy's aspects (MATRIX.md 11).
    pub counter_pick: bool,
    /// Ask to travel to this zone after `travel_after_ticks` (hub flows only).
    pub travel_to: Option<String>,
    pub travel_after_ticks: u32,
}

/// Why the bot loop ended.
#[derive(Clone, Debug)]
pub enum BotExit {
    Done,
    /// The zone handed over a ticket for another zone.
    Travel(Box<gm_hub_proto::protocol::ZoneTicket>),
}

#[derive(Clone, Debug, Default)]
pub struct BotReport {
    pub name: String,
    pub entity: EntityId,
    pub team: u8,
    pub ticks: u32,
    pub secs: f64,
    pub client: ClientStats,
    pub udp_tx_bytes: u64,
    pub udp_rx_bytes: u64,
    pub rtt_ms: f64,
    pub kills_seen: u32,
    pub deaths_seen: u32,
    pub own_kills: u32,
    pub own_deaths: u32,
    pub final_health: i32,
    pub send_failures: u64,
    pub others_seen_max: usize,
    /// Re-specs requested and the build the bot ended with.
    pub respecs: u32,
    pub final_build: String,
}

impl BotReport {
    pub fn tx_bytes_per_s(&self) -> f64 {
        self.udp_tx_bytes as f64 / self.secs.max(1e-3)
    }

    pub fn rx_bytes_per_s(&self) -> f64 {
        self.udp_rx_bytes as f64 / self.secs.max(1e-3)
    }
}

/// Connect to `server` through `endpoint` (which must trust the zone's certificate) and play.
pub async fn run_bot(
    endpoint: &quinn::Endpoint,
    server: SocketAddr,
    cfg: BotConfig,
    world: Arc<Bsp>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<BotReport> {
    let (report, _, _) = run_bot_with_token(
        endpoint,
        server,
        cfg,
        Vec::new(),
        move |_| Ok(world.clone()),
        shutdown,
    )
    .await?;
    Ok(report)
}

/// Like [`run_bot`] with a hub token; the map is loaded by name from `Welcome` through
/// `load_map`. Returns the report, why the loop ended and the map name.
pub async fn run_bot_with_token(
    endpoint: &quinn::Endpoint,
    server: SocketAddr,
    cfg: BotConfig,
    token: Vec<u8>,
    load_map: impl Fn(&str) -> anyhow::Result<Arc<Bsp>>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<(BotReport, BotExit, String)> {
    let conn = endpoint.connect(server, SERVER_NAME)?.await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    control::send(
        &mut send,
        &Control::Hello {
            version: PROTOCOL_VERSION as u16,
            name: cfg.name.clone(),
            token,
            build: cfg.build.clone().map(BuildChoice::Preset),
            team: cfg.team,
        },
    )
    .await?;
    let welcome = control::recv(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("server closed before Welcome"))?;
    let (entity, hz, map_name) = match welcome {
        Control::Welcome {
            entity, hz, map, ..
        } => (entity, hz, map),
        Control::Reject(reason) => anyhow::bail!("rejected: {reason}"),
        other => anyhow::bail!("unexpected handshake message {other:?}"),
    };
    let world = load_map(&map_name)?;
    let mut exit = BotExit::Done;
    let content = control::recv(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("server closed before Content"))?;
    let (pack, own, team) = match content {
        Control::Content { pack, own, team } => (pack, own, team),
        other => anyhow::bail!("expected Content, got {other:?}"),
    };
    let rate = TickRate::new(hz as u32);
    info!(name = %cfg.name, entity, hz, team, "bot joined");

    let build_name = |b: &gm_core::build::Build| -> String {
        pack.builds
            .iter()
            .find(|nb| &nb.build == b)
            .map_or_else(|| "custom".to_string(), |nb| nb.name.clone())
    };
    let mut current_build = build_name(&own);
    let mut client = ClientState::new(entity, rate, Sheet::new(own, &pack, team));
    let mut brain = Brain::new(cfg.seed, cfg.behaviour);
    let mut next_pick = COUNTER_PICK_PERIOD_S * rate.hz();
    let mut report = BotReport {
        name: cfg.name.clone(),
        entity,
        team,
        ..Default::default()
    };
    let start = Instant::now();
    let period = rate.period();
    let mut next = start;
    let mut ticks: u32 = 0;
    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        let sleep = tokio::time::sleep_until(next);
        tokio::select! {
            _ = &mut shutdown => break,
            _ = sleep => {
                ticks += 1;
                let t = client.render_tick(0.0);
                let others = client.others_at(t);
                report.others_seen_max = report.others_seen_max.max(
                    others.iter().filter(|e| e.kind == gm_net::snapshot::EntityKind::Player).count(),
                );
                let input = brain.think(&View {
                    me: &client.mover,
                    kit: &client.sheet.kit,
                    frame: client.sheet.build.frame,
                    team,
                    alive: client.own_alive,
                    others: &others,
                    tick: client.tick.wrapping_add(1),
                });
                if let Some(to) = &cfg.travel_to
                    && cfg.travel_after_ticks > 0
                    && ticks == cfg.travel_after_ticks
                {
                    info!(name = %cfg.name, %to, "asking to travel");
                    let _ = control::send(&mut send, &Control::Travel(to.clone())).await;
                }
                if cfg.counter_pick && ticks >= next_pick {
                    next_pick = ticks + COUNTER_PICK_PERIOD_S * rate.hz();
                    let enemy = dominant_enemy_aspects(team, &others);
                    if let Some(name) = counter_pick(&pack, &current_build, enemy) {
                        info!(name = %cfg.name, from = %current_build, to = %name, "counter-pick");
                        let _ = control::send(&mut send, &Control::Respec(BuildChoice::Preset(name)))
                            .await;
                        report.respecs += 1;
                    }
                }
                let datagram = client.local_tick(world.as_ref(), input);
                if conn.send_datagram(Bytes::from(datagram.encode())).is_err() {
                    report.send_failures += 1;
                }
                client.prune(t);
                next += period;
                // Never try to catch up more than a second: reset the timeline instead.
                let now = Instant::now();
                if now.saturating_duration_since(next) > std::time::Duration::from_secs(1) {
                    next = now;
                }
                if cfg.run_ticks != 0 && ticks >= cfg.run_ticks {
                    break;
                }
            }
            dg = conn.read_datagram() => {
                match dg {
                    Ok(bytes) => {
                        if let Err(e) = client.on_snapshot(world.as_ref(), &bytes) {
                            debug!(name = %cfg.name, "snapshot dropped: {e}");
                        }
                    }
                    Err(e) => {
                        info!(name = %cfg.name, "connection closed: {e}");
                        break;
                    }
                }
            }
            msg = control::recv(&mut recv) => {
                match msg {
                    Ok(Some(Control::Killed { victim, killer })) => {
                        report.kills_seen += 1;
                        if victim == entity {
                            report.own_deaths += 1;
                        }
                        if killer == entity {
                            report.own_kills += 1;
                        }
                    }
                    Ok(Some(Control::BuildApplied(build))) => {
                        if build != client.sheet.build {
                            current_build = build_name(&build);
                            client.set_sheet(Sheet::new(build, &pack, team));
                        }
                    }
                    Ok(Some(Control::TravelTicket { zone, addr, cert_der, token })) => {
                        let addr: SocketAddr = match addr.parse() {
                            Ok(a) => a,
                            Err(e) => {
                                info!(name = %cfg.name, "bad travel address {addr}: {e}");
                                continue;
                            }
                        };
                        let Ok(token) = bitcode::decode(&token) else {
                            info!(name = %cfg.name, "bad travel token");
                            continue;
                        };
                        info!(name = %cfg.name, %zone, %addr, "travel ticket");
                        exit = BotExit::Travel(Box::new(gm_hub_proto::protocol::ZoneTicket {
                            zone,
                            addr,
                            cert_der,
                            token,
                        }));
                        break;
                    }
                    Ok(Some(Control::TravelRefused(reason))) => {
                        info!(name = %cfg.name, "travel refused: {reason}");
                    }
                    Ok(Some(Control::Kick(reason))) => {
                        info!(name = %cfg.name, "kicked: {reason}");
                        break;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }
    report.deaths_seen = report.kills_seen;
    let stats = conn.stats();
    report.ticks = ticks;
    report.secs = start.elapsed().as_secs_f64();
    report.client = client.stats;
    report.udp_tx_bytes = stats.udp_tx.bytes;
    report.udp_rx_bytes = stats.udp_rx.bytes;
    report.rtt_ms = conn.rtt().as_secs_f64() * 1000.0;
    report.final_health = client.own_health;
    report.final_build = current_build;
    let _ = control::send(&mut send, &Control::Bye).await;
    conn.close(0u32.into(), b"done");
    // Give the Bye a moment to leave before the endpoint is dropped.
    tokio::time::sleep(Duration::from_millis(20)).await;
    Ok((report, exit, map_name))
}
