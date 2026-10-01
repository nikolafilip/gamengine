//! The zone process: tick loop over `gm_core::sim::Zone`, sessions, snapshots and reports.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use gm_core::build::{Build, ContentPack};
use gm_core::sim::{HitKind, Zone, ZoneEvent};
use gm_core::tick::TickRate;
use gm_core::vocab::EntityId;
use gm_net::control::{BuildChoice, Control};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tracing::{info, warn};

use crate::net::{ClientEvent, EVENT_CHANNEL, JoinInfo, NetConfig, accept_loop};
use crate::session::{PvsCache, Session};
use crate::tick::{TickMetrics, TickScheduler};
use crate::world::ZoneWorld;

pub struct ZoneConfig {
    pub rate: TickRate,
    pub open: bool,
    pub seed: u64,
    pub max_players: usize,
    pub report_every: Duration,
    /// Stop after this many ticks (tests and benchmarks).
    pub max_ticks: Option<u64>,
    /// Latest report, for tests and dashboards.
    pub report_tx: Option<watch::Sender<ZoneReport>>,
    /// Abilities and preset builds (MATRIX.md 10).
    pub content: ContentPack,
    /// Preset given to clients that ask for none.
    pub default_build: String,
}

impl Default for ZoneConfig {
    fn default() -> Self {
        ZoneConfig {
            rate: TickRate::COMBAT,
            open: true,
            seed: 1,
            max_players: 64,
            report_every: Duration::from_secs(5),
            max_ticks: None,
            report_tx: None,
            content: gm_core::sim::test_content::pack(TickRate::COMBAT),
            default_build: "blade".into(),
        }
    }
}

/// Counters over one report window plus running totals.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneReport {
    pub tick: u64,
    pub players: usize,
    pub window_secs: f64,
    /// UDP bytes per connected player per second, from QUIC's own counters.
    pub tx_bytes_per_player_s: f64,
    pub rx_bytes_per_player_s: f64,
    pub snapshot_payload_per_player_s: f64,
    pub tick_mean_us: f64,
    pub tick_p99_us: f64,
    pub tick_max_us: f64,
    pub late_max_us: f64,
    pub overruns: u64,
    // Totals since start.
    pub joins: u64,
    pub leaves: u64,
    pub executed_frames: u64,
    pub starved_ticks: u64,
    pub dropped_frames: u64,
    pub oversize_drops: u64,
    pub send_failures: u64,
    pub malformed: u64,
    pub hits_melee: u64,
    pub hits_projectile: u64,
    pub hits_area: u64,
    pub hits_dot: u64,
    pub parries: u64,
    pub guard_breaks: u64,
    pub staggers: u64,
    pub statuses_applied: u64,
    pub kills: u64,
    /// Kills per team (index 0 = team 0 / none, 1, 2).
    pub team_kills: [u64; 3],
    pub max_tx_bytes_per_player_s: f64,
    pub max_rx_bytes_per_player_s: f64,
    /// Counters of the players connected right now (not yet folded into the totals).
    pub live_executed_frames: u64,
    pub live_starved_ticks: u64,
}

/// Run the zone until `shutdown` resolves or `max_ticks` is reached. The endpoint must already
/// be bound; it is closed on exit.
pub async fn run(
    cfg: ZoneConfig,
    world: Arc<ZoneWorld>,
    endpoint: quinn::Endpoint,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<ZoneReport> {
    let rate = cfg.rate;
    let (tx, mut rx) = mpsc::channel::<ClientEvent>(EVENT_CHANNEL);
    let net_cfg = Arc::new(NetConfig {
        hz: rate.hz() as u16,
        map_name: world.name.clone(),
        map_hash: world.hash,
        open: cfg.open,
        content: Arc::new(cfg.content.clone()),
    });
    let acceptor = tokio::spawn(accept_loop(endpoint.clone(), tx.clone(), net_cfg));
    drop(tx);

    let mut zone = Zone::new(rate, cfg.seed, world.spawns.clone(), cfg.content.clone());
    let resolve = |zone: &Zone, choice: Option<&BuildChoice>| -> Result<Build, String> {
        match choice {
            None => zone
                .content
                .build(&cfg.default_build)
                .cloned()
                .ok_or_else(|| format!("no default build {:?}", cfg.default_build)),
            Some(BuildChoice::Preset(name)) => zone
                .content
                .build(name)
                .cloned()
                .ok_or_else(|| format!("unknown preset {name:?}")),
            Some(BuildChoice::Custom(b)) => Ok(b.clone()),
        }
    };
    let mut sessions: BTreeMap<EntityId, Session> = BTreeMap::new();
    let mut pvs = PvsCache::default();
    let mut scheduler = TickScheduler::new(rate, Instant::now());
    let mut metrics = TickMetrics::new(rate.period());
    let mut report = ZoneReport::default();
    let mut last_report = Instant::now();
    let shutdown = std::pin::pin!(shutdown);
    let mut shutdown = shutdown.fuse_once();

    info!(
        hz = rate.hz(),
        map = %world.name,
        spawns = world.spawns.len(),
        "zone running"
    );

    loop {
        let deadline = scheduler.next_deadline();
        tokio::select! {
            _ = &mut shutdown => {
                info!("shutdown requested");
                break;
            }
            _ = tokio::time::sleep_until(deadline) => {}
        }
        let started = Instant::now();
        if let Some(behind) = scheduler.resync_if_behind(started) {
            warn!(
                behind_ms = behind.as_millis(),
                "fell behind; resynchronising"
            );
        }

        // Network events since the last tick.
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ClientEvent::Join {
                    name,
                    build,
                    team,
                    conn,
                    control,
                    reply,
                } => {
                    if sessions.len() >= cfg.max_players {
                        let _ = reply.send(Err("zone full".into()));
                        continue;
                    }
                    let build = match resolve(&zone, build.as_ref()) {
                        Ok(b) => b,
                        Err(e) => {
                            let _ = reply.send(Err(e));
                            continue;
                        }
                    };
                    let team = if team == 0 || team > 2 {
                        zone.smallest_team()
                    } else {
                        team
                    };
                    let id = match zone.add_player(&world.bsp, build.clone(), team) {
                        Ok(id) => id,
                        Err(e) => {
                            let _ = reply.send(Err(format!("invalid build: {e}")));
                            continue;
                        }
                    };
                    let _ = reply.send(Ok(JoinInfo {
                        entity: id,
                        server_tick: zone.tick,
                        build,
                        team,
                    }));
                    for s in sessions.values() {
                        s.send_control(Control::PlayerInfo {
                            id,
                            name: name.clone(),
                            team,
                        });
                        let _ = control.try_send(Control::PlayerInfo {
                            id: s.id,
                            name: s.name.clone(),
                            team: zone.player(s.id).map_or(0, |p| p.team()),
                        });
                    }
                    let _ = control.try_send(Control::PlayerInfo {
                        id,
                        name: name.clone(),
                        team,
                    });
                    sessions.insert(id, Session::new(id, name, conn, control));
                    report.joins += 1;
                }
                ClientEvent::Respec { id, build } => {
                    let result = resolve(&zone, Some(&build))
                        .and_then(|b| zone.request_respec(id, b).map_err(|e| e.to_string()));
                    if let Some(s) = sessions.get(&id) {
                        s.send_control(Control::RespecResult(result));
                    }
                }
                ClientEvent::Input { id, datagram } => {
                    if let Some(s) = sessions.get_mut(&id) {
                        s.on_ack(datagram.ack_tick);
                        for (t, frame) in datagram.frames() {
                            zone.queue_input(id, t, frame.to_sim(), datagram.view_tick);
                        }
                    }
                }
                ClientEvent::Malformed { id } => {
                    if let Some(s) = sessions.get_mut(&id) {
                        s.malformed += 1;
                        report.malformed += 1;
                        if s.malformed > 100 {
                            s.conn.close(2u32.into(), b"malformed datagrams");
                        }
                    }
                }
                ClientEvent::Chat { id, text } => {
                    if sessions.contains_key(&id) {
                        for s in sessions.values() {
                            s.send_control(Control::ChatFrom {
                                from: id,
                                text: text.clone(),
                            });
                        }
                    }
                }
                ClientEvent::Leave { id } => {
                    if let Some(p) = zone.remove_player(id) {
                        info!(
                            entity = id,
                            executed = p.executed_frames,
                            starved = p.starved_ticks,
                            dropped = p.dropped_frames,
                            queued = p.queued_frames(),
                            kills = p.kills,
                            deaths = p.deaths,
                            "player stats at leave"
                        );
                        report.executed_frames += p.executed_frames;
                        report.starved_ticks += p.starved_ticks;
                        report.dropped_frames += p.dropped_frames;
                    }
                    if let Some(s) = sessions.remove(&id) {
                        report.oversize_drops += s.oversize_drops;
                        report.send_failures += s.send_failures;
                    }
                    for s in sessions.values() {
                        s.send_control(Control::PlayerLeft(id));
                    }
                    report.leaves += 1;
                }
            }
        }

        // Latency-bounded lag compensation (PROTOCOL.md 4), refreshed once a second.
        if scheduler.tick().is_multiple_of(rate.hz() as u64) {
            for s in sessions.values() {
                let half_rtt = s.conn.rtt() / 2;
                let ticks = half_rtt.as_secs_f64() / rate.period().as_secs_f64();
                zone.set_half_rtt_ticks(s.id, ticks.ceil() as u32);
            }
        }

        zone.step(&world.bsp);

        let events: Vec<ZoneEvent> = zone.events.drain(..).collect();
        for ev in events {
            match ev {
                ZoneEvent::Hit { kind, .. } => match kind {
                    HitKind::Melee => report.hits_melee += 1,
                    HitKind::Projectile => report.hits_projectile += 1,
                    HitKind::Area => report.hits_area += 1,
                    HitKind::Dot => report.hits_dot += 1,
                },
                ZoneEvent::Killed { victim, killer } => {
                    report.kills += 1;
                    let team = zone.player(killer).map_or(0, |p| p.team()) as usize;
                    report.team_kills[team.min(2)] += 1;
                    for s in sessions.values_mut() {
                        if !s.send_control(Control::Killed { victim, killer }) {
                            // Not draining its control stream: the connection is as good as dead.
                            s.conn.close(3u32.into(), b"control stream not read");
                        }
                    }
                }
                ZoneEvent::Respawned(id) => {
                    // The client's prediction switches to whatever build the respawn applied.
                    if let (Some(s), Some(p)) = (sessions.get(&id), zone.player(id)) {
                        s.send_control(Control::BuildApplied(p.sheet.build.clone()));
                    }
                }
                ZoneEvent::Parried { .. } => report.parries += 1,
                ZoneEvent::GuardBroken(_) => report.guard_breaks += 1,
                ZoneEvent::Staggered(_) => report.staggers += 1,
                ZoneEvent::StatusApplied { .. } => report.statuses_applied += 1,
                ZoneEvent::Healed { .. }
                | ZoneEvent::ProjectileSpawned { .. }
                | ZoneEvent::ProjectileRemoved(_)
                | ZoneEvent::AreaSpawned { .. }
                | ZoneEvent::AreaRemoved(_) => {}
            }
        }

        let tick = zone.tick;
        for s in sessions.values_mut() {
            if let Some(bytes) = s.build_snapshot(&zone, &world, &mut pvs, tick)
                && s.conn.send_datagram(Bytes::from(bytes)).is_err()
            {
                s.send_failures += 1;
            }
        }

        metrics.record(
            started.saturating_duration_since(deadline),
            started.elapsed(),
        );
        scheduler.advance();

        if last_report.elapsed() >= cfg.report_every {
            let secs = last_report.elapsed().as_secs_f64();
            fill_report(
                &mut report,
                &mut sessions,
                &zone,
                &metrics,
                secs,
                scheduler.tick(),
            );
            info!(
                tick = report.tick,
                players = report.players,
                tx_bps = format_args!("{:.0}", report.tx_bytes_per_player_s),
                rx_bps = format_args!("{:.0}", report.rx_bytes_per_player_s),
                snap_bps = format_args!("{:.0}", report.snapshot_payload_per_player_s),
                tick_us_mean = format_args!("{:.1}", report.tick_mean_us),
                tick_us_max = format_args!("{:.1}", report.tick_max_us),
                late_us_max = format_args!("{:.1}", report.late_max_us),
                overruns = report.overruns,
                starved = report.starved_ticks + report.live_starved_ticks,
                executed = report.executed_frames + report.live_executed_frames,
                hits = report.hits_melee + report.hits_projectile + report.hits_area,
                kills = report.kills,
                "zone report"
            );
            if let Some(w) = &cfg.report_tx {
                let _ = w.send(report.clone());
            }
            metrics.reset_window();
            last_report = Instant::now();
        }
        if cfg.max_ticks.is_some_and(|n| scheduler.tick() >= n) {
            break;
        }
    }

    let secs = last_report.elapsed().as_secs_f64().max(1e-3);
    fill_report(
        &mut report,
        &mut sessions,
        &zone,
        &metrics,
        secs,
        scheduler.tick(),
    );
    for s in sessions.values() {
        s.send_control(Control::Kick("zone stopped".into()));
        s.conn.close(0u32.into(), b"zone stopped");
    }
    // Fold the live sessions' counters into the totals.
    for p in zone.players() {
        report.executed_frames += p.executed_frames;
        report.starved_ticks += p.starved_ticks;
        report.dropped_frames += p.dropped_frames;
    }
    for s in sessions.values() {
        report.oversize_drops += s.oversize_drops;
        report.send_failures += s.send_failures;
    }
    if let Some(w) = &cfg.report_tx {
        let _ = w.send(report.clone());
    }
    endpoint.close(0u32.into(), b"zone stopped");
    acceptor.abort();
    info!(ticks = report.tick, "zone stopped");
    Ok(report)
}

fn fill_report(
    report: &mut ZoneReport,
    sessions: &mut BTreeMap<EntityId, Session>,
    zone: &Zone,
    metrics: &TickMetrics,
    secs: f64,
    tick: u64,
) {
    let mut tx = 0u64;
    let mut rx = 0u64;
    let mut payload = 0u64;
    for s in sessions.values_mut() {
        let st = s.conn.stats();
        tx += st.udp_tx.bytes.saturating_sub(s.last_udp_tx);
        rx += st.udp_rx.bytes.saturating_sub(s.last_udp_rx);
        s.last_udp_tx = st.udp_tx.bytes;
        s.last_udp_rx = st.udp_rx.bytes;
        payload += s.snapshot_bytes;
        s.snapshot_bytes = 0;
    }
    let n = sessions.len().max(1) as f64;
    let secs = secs.max(1e-3);
    let sm = metrics.summary();
    report.tick = tick;
    report.players = sessions.len();
    report.window_secs = secs;
    report.tx_bytes_per_player_s = tx as f64 / n / secs;
    report.rx_bytes_per_player_s = rx as f64 / n / secs;
    report.snapshot_payload_per_player_s = payload as f64 / n / secs;
    if !sessions.is_empty() {
        report.max_tx_bytes_per_player_s = report
            .max_tx_bytes_per_player_s
            .max(report.tx_bytes_per_player_s);
        report.max_rx_bytes_per_player_s = report
            .max_rx_bytes_per_player_s
            .max(report.rx_bytes_per_player_s);
    }
    report.tick_mean_us = sm.mean_us;
    report.tick_p99_us = sm.p99_us;
    report.tick_max_us = sm.max_us;
    report.late_max_us = sm.late_max_us;
    report.overruns = sm.overruns;
    // Live players' counters (folded into the totals when they leave).
    report.live_executed_frames = zone.players().map(|p| p.executed_frames).sum();
    report.live_starved_ticks = zone.players().map(|p| p.starved_ticks).sum();
}

/// A future that stays pending after it first resolved, so `select!` can poll it repeatedly.
trait FuseOnce: Future<Output = ()> + Sized {
    fn fuse_once(self) -> Fused<Self> {
        Fused { inner: Some(self) }
    }
}

impl<F: Future<Output = ()>> FuseOnce for F {}

struct Fused<F> {
    inner: Option<F>,
}

impl<F: Future<Output = ()> + Unpin> Future for Fused<F> {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        match &mut self.inner {
            Some(f) => match std::pin::Pin::new(f).poll(cx) {
                std::task::Poll::Ready(()) => {
                    self.inner = None;
                    std::task::Poll::Ready(())
                }
                std::task::Poll::Pending => std::task::Poll::Pending,
            },
            None => std::task::Poll::Pending,
        }
    }
}
