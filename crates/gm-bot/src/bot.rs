//! One bot: connect, handshake, then a fixed-step loop of predicted inputs and snapshots.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use glam::Vec3;
use gm_bsp::Bsp;
use gm_core::rng::Rng;
use gm_core::sim::{Input, buttons};
use gm_core::tick::TickRate;
use gm_core::vocab::EntityId;
use gm_net::PROTOCOL_VERSION;
use gm_net::client::{ClientState, ClientStats, RenderEntity};
use gm_net::control::{self, Control};
use gm_net::snapshot::EntityKind;
use gm_net::transport::SERVER_NAME;
use tokio::time::Instant;
use tracing::{debug, info};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Stand still, face the nearest player, swing when it is in reach.
    Hold,
    /// Run around, jump now and then, shoot and swing at whoever is nearest.
    Wander,
    /// Chase the nearest player: sword in reach, crossbow otherwise.
    Hunter,
}

#[derive(Clone, Debug)]
pub struct BotConfig {
    pub name: String,
    pub seed: u64,
    pub behaviour: Behaviour,
    pub rate: TickRate,
    /// Stop after this many client ticks (0 = run until `shutdown`).
    pub run_ticks: u32,
}

#[derive(Clone, Debug, Default)]
pub struct BotReport {
    pub name: String,
    pub entity: EntityId,
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
    let conn = endpoint.connect(server, SERVER_NAME)?.await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    control::send(
        &mut send,
        &Control::Hello {
            version: PROTOCOL_VERSION as u16,
            name: cfg.name.clone(),
            token: Vec::new(),
        },
    )
    .await?;
    let welcome = control::recv(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("server closed before Welcome"))?;
    let (entity, hz) = match welcome {
        Control::Welcome { entity, hz, .. } => (entity, hz),
        Control::Reject(reason) => anyhow::bail!("rejected: {reason}"),
        other => anyhow::bail!("unexpected handshake message {other:?}"),
    };
    let rate = TickRate::new(hz as u32);
    info!(name = %cfg.name, entity, hz, "bot joined");

    let mut client = ClientState::new(entity, rate);
    let mut brain = Brain::new(cfg.seed, cfg.behaviour);
    let mut report = BotReport {
        name: cfg.name.clone(),
        entity,
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
                report.others_seen_max = report.others_seen_max.max(others.len());
                let input = brain.think(&client, &others, ticks);
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
    let _ = control::send(&mut send, &Control::Bye).await;
    conn.close(0u32.into(), b"done");
    Ok(report)
}

/// The scripted player.
struct Brain {
    rng: Rng,
    behaviour: Behaviour,
    yaw: f32,
    pitch: f32,
    next_turn: u32,
    next_shot: u32,
    jump_until: u32,
}

impl Brain {
    fn new(seed: u64, behaviour: Behaviour) -> Brain {
        let mut rng = Rng::new(seed);
        let yaw = rng.range_f32(0.0, 360.0);
        Brain {
            rng,
            behaviour,
            yaw,
            pitch: 0.0,
            next_turn: 64,
            next_shot: 90,
            jump_until: 0,
        }
    }

    fn nearest<'a>(&self, me: Vec3, others: &'a [RenderEntity]) -> Option<(&'a RenderEntity, f32)> {
        others
            .iter()
            .filter(|e| e.kind == EntityKind::Player && e.alive())
            .map(|e| (e, (e.pos - me).length()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    fn face(&mut self, me_eye: Vec3, target: Vec3) {
        let to = target + Vec3::new(0.0, 0.0, 28.0) - me_eye;
        let horiz = to.truncate().length();
        self.yaw = to.y.atan2(to.x).to_degrees().rem_euclid(360.0);
        self.pitch = (-to.z).atan2(horiz).to_degrees().clamp(-89.0, 89.0);
    }

    fn think(&mut self, client: &ClientState, others: &[RenderEntity], tick: u32) -> Input {
        let me = client.mover.mv.origin;
        let eye = client.mover.eye();
        let mut buttons = 0u16;
        let mut forward = 0.0f32;
        let nearest = self.nearest(me, others);
        match self.behaviour {
            Behaviour::Hold => {
                if let Some((e, d)) = nearest {
                    self.face(eye, e.pos);
                    if d < 70.0 && tick.is_multiple_of(24) {
                        buttons |= buttons::PRIMARY;
                    }
                }
            }
            Behaviour::Wander => {
                if tick >= self.next_turn {
                    self.yaw = self.rng.range_f32(0.0, 360.0);
                    self.pitch = 0.0;
                    self.next_turn = tick + 64 + self.rng.below(128);
                }
                forward = 1.0;
                if self.rng.next_f32() < 0.02 {
                    self.jump_until = tick + 2;
                }
                if tick < self.jump_until {
                    buttons |= buttons::JUMP;
                }
                if let Some((e, d)) = nearest {
                    if d < 70.0 && tick.is_multiple_of(24) {
                        self.face(eye, e.pos);
                        buttons |= buttons::PRIMARY;
                    } else if tick >= self.next_shot {
                        self.face(eye, e.pos);
                        buttons |= buttons::SECONDARY;
                        self.next_shot = tick + 96 + self.rng.below(64);
                    }
                }
            }
            Behaviour::Hunter => {
                if let Some((e, d)) = nearest {
                    self.face(eye, e.pos);
                    if d > 50.0 {
                        forward = 1.0;
                    }
                    if d < 70.0 {
                        if tick.is_multiple_of(20) {
                            buttons |= buttons::PRIMARY;
                        }
                    } else if d > 150.0 && tick >= self.next_shot {
                        buttons |= buttons::SECONDARY;
                        self.next_shot = tick + 96;
                    }
                } else {
                    forward = 1.0;
                    if tick >= self.next_turn {
                        self.yaw = self.rng.range_f32(0.0, 360.0);
                        self.next_turn = tick + 96;
                    }
                }
            }
        }
        Input {
            buttons,
            yaw: self.yaw,
            pitch: self.pitch,
            forward,
            side: 0.0,
            ability: 0,
        }
    }
}
