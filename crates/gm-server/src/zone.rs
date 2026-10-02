//! The zone process: tick loop over `gm_core::sim::Zone`, sessions, snapshots and reports.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use gm_ai::director::{CompanionSpec, CreatureSpawn, Director, DirectorEvent, EncounterState};
use gm_core::build::{Build, ContentPack};
use gm_core::sim::{HitKind, Zone, ZoneEvent};
use gm_core::tick::TickRate;
use gm_core::trace::{CollisionWorld, Contents, Hull};
use gm_core::vocab::EntityId;
use gm_net::control::{self, BodyKind, BuildChoice, Control, PlayerEntry, SquadEntry, StallEntry};
use rayon::prelude::*;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tracing::{info, warn};

use crate::hub_link::HubLink;
use crate::net::{ClientEvent, EVENT_CHANNEL, JoinInfo, NetConfig, accept_loop};
use crate::session::{PvsCache, Session, TickTable};
use gm_hub_proto::protocol::{
    CharacterId, CharacterState, ModelId, ModelRef, StallSummary, now_secs,
};

/// Zones save every character this often (HUB.md 3.2).
pub const SAVE_EVERY: Duration = Duration::from_secs(30);
/// A ghost waits this long for the other zone's claim (HUB.md 3.3).
pub const GHOST_TIMEOUT: Duration = Duration::from_secs(10);
/// A revoked model is remembered this long, so a claim that raced the takedown cannot bring
/// it back (MODELS.md 7).
pub const REVOKED_MEMORY: Duration = Duration::from_secs(60);
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
    /// The hub this zone runs under (HUB.md); `None` = open development zone.
    pub hub: Option<Arc<HubLink>>,
    /// A wild zone (COMPANIONS.md 3.1): every human is team 1 and the map's creatures stand
    /// on their posts. Otherwise a team zone, as the arena.
    pub wild: bool,
    /// Companions come along with their commanders (COMPANIONS.md 3.2).
    pub squads: bool,
    /// Preset builds the zone lends to fill squad slots hires left empty (tutorial zones).
    pub recruits: Vec<String>,
    /// Every arrival starts at the map's spawns; a saved position is not resumed. For
    /// dungeons: nobody logs out past the gate and comes back at the boss's feet.
    pub arrive_at_entry: bool,
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
            hub: None,
            wild: false,
            squads: false,
            recruits: Vec::new(),
            arrive_at_entry: false,
        }
    }
}

fn wire_kind(kind: gm_ai::BodyKind) -> BodyKind {
    match kind {
        gm_ai::BodyKind::Human => BodyKind::Human,
        gm_ai::BodyKind::Companion { owner } => BodyKind::Companion { owner },
        gm_ai::BodyKind::Creature { def } => BodyKind::Creature { def },
    }
}

fn wire_order(order: gm_ai::Order) -> control::Order {
    match order {
        gm_ai::Order::Follow => control::Order::Follow,
        gm_ai::Order::Hold => control::Order::Hold,
        gm_ai::Order::MoveTo(p) => control::Order::MoveTo(p.into()),
        gm_ai::Order::Attack(id) => control::Order::Attack(id),
    }
}

fn ai_order(order: control::Order) -> gm_ai::Order {
    match order {
        control::Order::Follow => gm_ai::Order::Follow,
        control::Order::Hold => gm_ai::Order::Hold,
        control::Order::MoveTo(p) => gm_ai::Order::MoveTo(p.into()),
        control::Order::Attack(id) => gm_ai::Order::Attack(id),
    }
}

/// A commander's squad as its client is told.
fn squad_entries(director: &Director, zone: &Zone, commander: EntityId) -> Vec<SquadEntry> {
    director
        .squad(commander)
        .iter()
        .map(|m| SquadEntry {
            id: m.id,
            name: m.name.clone(),
            role: m.role() as u8,
            order: wire_order(m.mind.order()),
            max_health: zone
                .player(m.id)
                .map_or(0, |p| p.max_health().clamp(0, u16::MAX as i32) as u16),
            recruit: m.recruit,
        })
        .collect()
}

/// A recruit's name: the preset's, capitalised.
fn recruit_name(preset: &str) -> String {
    let mut c = preset.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => "Recruit".into(),
    }
}

/// Hub bookkeeping of one session.
struct HubSlot {
    character: CharacterId,
    joined: Instant,
    play_seconds_before: u32,
    last_save: Instant,
    /// Set when a travel ticket went out; the body is a ghost until the claim or the timeout.
    ghost_since: Option<Instant>,
    /// Another zone claimed the character: no save on leave.
    claimed_elsewhere: bool,
}

/// A stall as clients see it: the tile's centre, the keeper's looks. `None` when the tile is
/// not on this map (a stall left over from another build of it).
fn stall_entry(
    world: &ZoneWorld,
    s: &StallSummary,
    revoked: &[(ModelId, Instant)],
) -> Option<StallEntry> {
    let (grid, centre) = world
        .stall_grids
        .iter()
        .find_map(|g| Some((g, g.centre(s.tile_x, s.tile_y)?)))?;
    Some(StallEntry {
        id: s.id,
        pos: centre.into(),
        yaw: grid.yaw,
        owner: s.owner_name.clone(),
        frame: s.frame,
        armour: s.armour,
        model: s
            .model
            .filter(|m| m.frame == s.frame && !revoked.iter().any(|(id, _)| *id == m.id))
            .map(|m| m.id),
    })
}

fn character_state(
    zone: &Zone,
    link: &HubLink,
    id: EntityId,
    slot: &HubSlot,
) -> Option<CharacterState> {
    let p = zone.player(id)?;
    Some(CharacterState {
        build: p.sheet.build.clone(),
        zone: Some(link.zone.clone()),
        position: p.mover.mv.origin.into(),
        yaw: p.mover.yaw,
        viewport: 0,
        play_seconds: slot.play_seconds_before + slot.joined.elapsed().as_secs() as u32,
    })
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
    /// Where the tick went, mean microseconds over the window: network events, the
    /// simulation step, snapshot building, datagram sends.
    pub events_us_mean: f64,
    pub sim_us_mean: f64,
    pub snapshot_us_mean: f64,
    pub send_us_mean: f64,
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
    /// Minds the zone runs (companions and creatures) and what they cost, mean microseconds
    /// per tick over the window (COMPANIONS.md 14).
    pub minds: usize,
    pub minds_us_mean: f64,
    /// Encounters, totals since start.
    pub encounters_engaged: u64,
    pub encounters_reset: u64,
    pub encounters_cleared: u64,
    /// The time the last cleared encounter took, seconds.
    pub last_clear_secs: u32,
    pub loot_items: u64,
    pub trials_passed: u64,
    pub orders: u64,
    pub orders_refused: u64,
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
        hub: cfg.hub.clone(),
    });
    let acceptor = tokio::spawn(accept_loop(endpoint.clone(), tx.clone(), net_cfg));
    let hub_stats: Arc<std::sync::Mutex<(u32, f32)>> = Arc::new(std::sync::Mutex::new((0, 0.0)));
    if let Some(link) = &cfg.hub {
        link.spawn_heartbeat(hub_stats.clone());
        link.spawn_notice_reader(tx.clone());
    }
    let event_tx = tx.clone();
    drop(tx);
    let mut hub_slots: BTreeMap<EntityId, HubSlot> = BTreeMap::new();
    let mut revoked: Vec<(ModelId, Instant)> = Vec::new();
    // The market: open stalls by id (the hub is their owner; this is its mirror).
    let mut stalls: BTreeMap<i64, StallSummary> = BTreeMap::new();
    if let Some(link) = cfg.hub.clone().filter(|_| !world.stall_grids.is_empty()) {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            match link.stalls().await {
                Ok(list) => {
                    let _ = tx.send(ClientEvent::StallsLoaded(list)).await;
                }
                Err(e) => warn!("loading the zone's stalls: {e}"),
            }
        });
    }

    let mut zone = Zone::new(rate, cfg.seed, world.spawns.clone(), cfg.content.clone());
    // Minds (COMPANIONS.md): the nav grid, the creatures on their posts, the squads.
    let seeds: Vec<glam::Vec3> = world.spawns.iter().map(|s| s.origin).collect();
    let posts: Vec<CreatureSpawn> = if cfg.wild {
        world
            .creature_posts
            .iter()
            .map(|p| CreatureSpawn {
                creature: p.creature.clone(),
                encounter: p.encounter.clone(),
                origin: p.origin,
                yaw: p.yaw,
            })
            .collect()
    } else {
        Vec::new()
    };
    let minded = cfg.squads || !posts.is_empty();
    let nav_started = std::time::Instant::now();
    let mut director = Director::new(
        &mut zone,
        &world.bsp,
        &world.name,
        if minded { &seeds } else { &[] },
        &posts,
        cfg.seed,
    );
    if minded {
        info!(
            nav_nodes = director.nav.len(),
            nav_ms = format_args!("{:.1}", nav_started.elapsed().as_secs_f64() * 1000.0),
            creatures = director.minds(),
            "minds ready"
        );
    }
    director.events.clear();
    // The avatar models hired companions wear, and when their hires run out.
    let mut companion_models: BTreeMap<EntityId, ModelRef> = BTreeMap::new();
    let mut hire_expiry: BTreeMap<EntityId, u64> = BTreeMap::new();
    let mut squads_dirty: Vec<EntityId> = Vec::new();
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
    let started_unix = now_secs();
    let mut sessions: BTreeMap<EntityId, Session> = BTreeMap::new();
    let mut pvs = PvsCache::default();
    let mut table = TickTable::default();
    let mut scheduler = TickScheduler::new(rate, Instant::now());
    let mut metrics = TickMetrics::new(rate.period());
    let mut phases = Phases::default();
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
        let t_events = Instant::now();
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ClientEvent::Join {
                    name,
                    build,
                    team,
                    hub,
                    conn,
                    control,
                    reply,
                } => {
                    if sessions.len() >= cfg.max_players {
                        let _ = reply.send(Err("zone full".into()));
                        continue;
                    }
                    // The same character again (a reconnect the hub allowed): drop the old body.
                    if let Some(h) = &hub
                        && let Some(old) = hub_slots
                            .iter()
                            .find(|(_, s)| s.character == h.character)
                            .map(|(id, _)| *id)
                    {
                        zone.remove_player(old);
                        if let Some(s) = sessions.remove(&old) {
                            s.conn.close(4u32.into(), b"character joined again");
                        }
                        hub_slots.remove(&old);
                    }
                    let build = match resolve(&zone, build.as_ref()) {
                        Ok(b) => b,
                        Err(e) => {
                            let _ = reply.send(Err(e));
                            continue;
                        }
                    };
                    let team = if cfg.wild {
                        1
                    } else if team == 0 || team > 2 {
                        zone.smallest_team()
                    } else {
                        team
                    };
                    // A saved position is used when it is still a place to stand (the map
                    // may have been rebuilt since); otherwise the player spawns.
                    let resume = hub.as_ref().and_then(|h| h.origin).filter(|(origin, _)| {
                        !cfg.arrive_at_entry
                            && origin.is_finite()
                            && world.bsp.point_contents(Hull::Player, *origin) == Contents::Empty
                    });
                    let id = match resume {
                        Some((origin, yaw)) => {
                            if let Err(e) = build.validate(&zone.content) {
                                let _ = reply.send(Err(format!("invalid build: {e}")));
                                continue;
                            }
                            zone.add_player_at(build.clone(), team, origin, yaw)
                        }
                        None => match zone.add_player(&world.bsp, build.clone(), team) {
                            Ok(id) => id,
                            Err(e) => {
                                let _ = reply.send(Err(format!("invalid build: {e}")));
                                continue;
                            }
                        },
                    };
                    revoked.retain(|(_, at)| at.elapsed() < REVOKED_MEMORY);
                    let model = hub
                        .as_ref()
                        .and_then(|h| h.model)
                        .filter(|m| !revoked.iter().any(|(id, _)| *id == m.id));
                    let hired = hub.as_ref().map(|h| h.squad.clone()).unwrap_or_default();
                    if let Some(h) = hub {
                        hub_slots.insert(
                            id,
                            HubSlot {
                                character: h.character,
                                joined: Instant::now(),
                                play_seconds_before: h.play_seconds,
                                // Staggered so a zone full of players never saves at once.
                                last_save: Instant::now()
                                    - Duration::from_secs(
                                        (id % SAVE_EVERY.as_secs() as u32) as u64,
                                    ),
                                ghost_since: None,
                                claimed_elsewhere: false,
                            },
                        );
                    }
                    let _ = reply.send(Ok(JoinInfo {
                        entity: id,
                        server_tick: zone.tick,
                        build,
                        team,
                    }));
                    let mut session = Session::new(id, name.clone(), conn, control);
                    session.model = model;
                    session.announced = zone.player(id).and_then(|p| session.wears(p.frame()));
                    for s in sessions.values() {
                        s.send_control(Control::PlayerInfo {
                            id,
                            name: name.clone(),
                            team,
                            model: session.announced,
                            kind: BodyKind::Human,
                        });
                    }
                    sessions.insert(id, session);
                    // The squad (COMPANIONS.md 3.2): hired avatars first, then the zone's
                    // recruits for the slots left.
                    if cfg.squads {
                        for h in hired {
                            let spec = CompanionSpec {
                                name: h.name.clone(),
                                build: h.build.clone(),
                                recruit: false,
                                hire: Some(h.hire),
                            };
                            if let Some(c) = director.add_companion(&mut zone, &world.bsp, id, spec)
                            {
                                hire_expiry.insert(c, h.expires_at);
                                if let Some(m) = h.model.filter(|m| {
                                    m.frame == crate::session::frame_index(h.build.frame)
                                        && !revoked.iter().any(|(r, _)| *r == m.id)
                                }) {
                                    companion_models.insert(c, m);
                                }
                            }
                        }
                        for preset in &cfg.recruits {
                            let Some(build) = zone.content.build(preset).cloned() else {
                                continue;
                            };
                            let spec = CompanionSpec {
                                name: recruit_name(preset),
                                build,
                                recruit: true,
                                hire: None,
                            };
                            director.add_companion(&mut zone, &world.bsp, id, spec);
                        }
                    }
                    // The joiner learns everyone, itself included, in one message: a town
                    // of hundreds must not overflow its control queue.
                    let mut roster: Vec<PlayerEntry> = sessions
                        .values()
                        .map(|s| PlayerEntry {
                            id: s.id,
                            name: s.name.clone(),
                            team: zone.player(s.id).map_or(0, |p| p.team()),
                            model: s.announced,
                            kind: BodyKind::Human,
                        })
                        .collect();
                    roster.extend(director.driven().into_iter().map(|(body, name, kind)| {
                        PlayerEntry {
                            id: body,
                            name,
                            team: zone.player(body).map_or(0, |p| p.team()),
                            model: companion_models.get(&body).map(|m| m.id),
                            kind: wire_kind(kind),
                        }
                    }));
                    sessions[&id].send_control(Control::Roster(roster));
                    if !stalls.is_empty() {
                        sessions[&id].send_control(Control::Stalls(
                            stalls
                                .values()
                                .filter_map(|s| stall_entry(&world, s, &revoked))
                                .collect(),
                        ));
                    }
                    report.joins += 1;
                }
                ClientEvent::Order { id, slots, order } => {
                    let Some(session) = sessions.get(&id) else {
                        continue;
                    };
                    let visible = |target: EntityId| session.sees(target);
                    match director.order(&zone, id, slots, ai_order(order), &visible) {
                        Ok(()) => report.orders += 1,
                        Err(why) => {
                            report.orders_refused += 1;
                            session.send_control(Control::OrderRefused(why));
                        }
                    }
                }
                ClientEvent::HubHireEnded { hirer, hire } => {
                    // The companion leaves now, or when its encounter ends (COMPANIONS.md 3.3).
                    if let Some(commander) = hub_slots
                        .iter()
                        .find(|(_, s)| s.character == hirer)
                        .map(|(id, _)| *id)
                        && let Some(m) = director
                            .squad(commander)
                            .iter()
                            .find(|m| m.hire == Some(hire))
                    {
                        hire_expiry.insert(m.id, 0);
                    }
                }
                ClientEvent::Respec { id, build } => {
                    let result = resolve(&zone, Some(&build))
                        .and_then(|b| zone.request_respec(id, b).map_err(|e| e.to_string()));
                    if let Some(s) = sessions.get(&id) {
                        s.send_control(Control::RespecResult(result));
                    }
                }
                ClientEvent::Travel { id, zone: to_zone } => {
                    let Some(link) = cfg.hub.clone() else {
                        if let Some(s) = sessions.get(&id) {
                            s.send_control(Control::TravelRefused("no hub".into()));
                        }
                        continue;
                    };
                    let Some(slot) = hub_slots.get(&id) else {
                        continue;
                    };
                    if slot.ghost_since.is_some() {
                        continue;
                    }
                    let Some(state) = character_state(&zone, &link, id, slot) else {
                        continue;
                    };
                    // One handoff request per player at the hub, and no more than one a
                    // second: the rest of a flood is dropped here.
                    if !sessions
                        .get_mut(&id)
                        .is_some_and(|s| s.begin_travel_request())
                    {
                        continue;
                    }
                    let character = slot.character;
                    let tx = event_tx.clone();
                    tokio::spawn(async move {
                        let result = link.handoff(character, state, to_zone).await;
                        let _ = tx.send(ClientEvent::TravelResult { id, result }).await;
                    });
                }
                ClientEvent::TravelResult { id, result } => {
                    let Some(s) = sessions.get_mut(&id) else {
                        continue;
                    };
                    s.end_travel_request();
                    match result {
                        Ok(ticket) => {
                            s.send_control(Control::TravelTicket {
                                zone: ticket.zone,
                                addr: ticket.addr.to_string(),
                                cert_der: ticket.cert_der,
                                token: bitcode::encode(&ticket.token),
                            });
                            zone.set_ghost(id, true);
                            director.human_left(&mut zone, id);
                            if let Some(slot) = hub_slots.get_mut(&id) {
                                slot.ghost_since = Some(Instant::now());
                            }
                        }
                        Err(e) => {
                            s.send_control(Control::TravelRefused(e));
                        }
                    }
                }
                ClientEvent::HubClaimed { character } => {
                    if let Some(id) = hub_slots
                        .iter()
                        .find(|(_, s)| s.character == character)
                        .map(|(id, _)| *id)
                    {
                        if let Some(slot) = hub_slots.get_mut(&id) {
                            slot.claimed_elsewhere = true;
                        }
                        director.human_left(&mut zone, id);
                        zone.remove_player(id);
                        if let Some(s) = sessions.remove(&id) {
                            s.send_control(Control::Kick("claimed by another zone".into()));
                            s.conn.close(0u32.into(), b"travelled");
                        }
                        hub_slots.remove(&id);
                        // The body leaves here: said once, counted once (the `Leave` that
                        // follows the closed connection finds nothing left to announce).
                        for s in sessions.values() {
                            s.send_control(Control::PlayerLeft(id));
                        }
                        report.leaves += 1;
                    }
                }
                ClientEvent::HubKick { character, reason } => {
                    if let Some(id) = hub_slots
                        .iter()
                        .find(|(_, s)| s.character == character)
                        .map(|(id, _)| *id)
                        && let Some(s) = sessions.get(&id)
                    {
                        s.send_control(Control::Kick(reason));
                        s.conn.close(3u32.into(), b"kicked by the hub");
                    }
                }
                ClientEvent::HubModelRevoked { model } => {
                    revoked.retain(|(_, at)| at.elapsed() < REVOKED_MEMORY);
                    revoked.push((model, Instant::now()));
                    for s in sessions.values_mut() {
                        if s.model.is_some_and(|m| m.id == model) {
                            s.model = None;
                            s.announced = None;
                        }
                    }
                    for stall in stalls.values_mut() {
                        if stall.model.is_some_and(|m| m.id == model) {
                            stall.model = None;
                        }
                    }
                    // Everyone forgets it, whoever wore it.
                    for s in sessions.values() {
                        s.send_control(Control::ModelRevoked(model));
                    }
                }
                ClientEvent::StallsLoaded(list) => {
                    stalls = list.into_iter().map(|s| (s.id, s)).collect();
                    let entries: Vec<StallEntry> = stalls
                        .values()
                        .filter_map(|s| stall_entry(&world, s, &revoked))
                        .collect();
                    info!(stalls = entries.len(), "market loaded");
                    for s in sessions.values() {
                        s.send_control(Control::Stalls(entries.clone()));
                    }
                }
                ClientEvent::StallOpen { id } => {
                    let Some(session) = sessions.get_mut(&id) else {
                        continue;
                    };
                    if !session.begin_stall_request() {
                        continue;
                    }
                    // Only the zone knows where the player stands: it must be alive, on a
                    // tile of the market, and the tile must be free (ECONOMY.md 7).
                    let request = (|| {
                        let link = cfg.hub.clone().ok_or("this zone has no market")?;
                        let slot = hub_slots.get(&id).ok_or("this zone has no market")?;
                        let p = zone
                            .player(id)
                            .filter(|p| p.alive && !p.ghost)
                            .ok_or("you cannot open a stall now")?;
                        let (x, y) = world
                            .stall_grids
                            .iter()
                            .find_map(|g| g.tile_at(p.mover.mv.origin))
                            .ok_or("stand on a market tile to open a stall")?;
                        if stalls.values().any(|s| (s.tile_x, s.tile_y) == (x, y)) {
                            return Err("that tile is taken");
                        }
                        Ok((link, slot.character, x, y))
                    })();
                    match request {
                        Ok((link, character, x, y)) => {
                            let tx = event_tx.clone();
                            tokio::spawn(async move {
                                let result = link.stall_open(character, x, y).await;
                                let _ = tx.send(ClientEvent::StallOpened { id, result }).await;
                            });
                        }
                        Err(why) => {
                            session.end_stall_request();
                            session.send_control(Control::StallResult(Err(why.to_string())));
                        }
                    }
                }
                ClientEvent::StallOpened { id, result } => {
                    let answer = match result {
                        Ok(stall) => {
                            if let Some(entry) = stall_entry(&world, &stall, &revoked) {
                                for s in sessions.values() {
                                    s.send_control(Control::StallOpened(entry.clone()));
                                }
                            }
                            stalls.insert(stall.id, stall);
                            Ok(())
                        }
                        Err(e) => Err(e),
                    };
                    if let Some(s) = sessions.get_mut(&id) {
                        s.end_stall_request();
                        s.send_control(Control::StallResult(answer));
                    }
                }
                ClientEvent::StallClose { id } => {
                    let Some(session) = sessions.get_mut(&id) else {
                        continue;
                    };
                    if !session.begin_stall_request() {
                        continue;
                    }
                    match (cfg.hub.clone(), hub_slots.get(&id)) {
                        (Some(link), Some(slot)) => {
                            let character = slot.character;
                            let tx = event_tx.clone();
                            tokio::spawn(async move {
                                let result = link.stall_close(character).await;
                                let _ = tx.send(ClientEvent::StallCloseResult { id, result }).await;
                            });
                        }
                        _ => {
                            session.end_stall_request();
                            session.send_control(Control::StallResult(Err(
                                "this zone has no market".into(),
                            )));
                        }
                    }
                }
                ClientEvent::StallCloseResult { id, result } => {
                    // The stall itself goes when the hub's notice arrives.
                    if let Some(s) = sessions.get_mut(&id) {
                        s.end_stall_request();
                        s.send_control(Control::StallResult(result));
                    }
                }
                ClientEvent::HubStallClosed { stall } => {
                    if stalls.remove(&stall).is_some() {
                        for s in sessions.values() {
                            s.send_control(Control::StallClosed(stall));
                        }
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
                    // A ghost's connection ends with its `Bye` on the way to another zone: the
                    // body and the hub's transit stay until the claim or the timeout (HUB.md 3.3).
                    if hub_slots.get(&id).is_some_and(|s| s.ghost_since.is_some()) {
                        if let Some(s) = sessions.remove(&id) {
                            report.oversize_drops += s.oversize_drops;
                            report.send_failures += s.send_failures;
                        }
                        // The body is still here: it leaves at the claim or the timeout.
                        continue;
                    }
                    if let (Some(link), Some(slot)) = (&cfg.hub, hub_slots.remove(&id))
                        && !slot.claimed_elsewhere
                        && let Some(state) = character_state(&zone, link, id, &slot)
                    {
                        let link = link.clone();
                        tokio::spawn(async move {
                            link.save(slot.character, state, true).await;
                        });
                    }
                    director.human_left(&mut zone, id);
                    let body = zone.remove_player(id);
                    if let Some(p) = &body {
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
                    let session = sessions.remove(&id);
                    if let Some(s) = &session {
                        report.oversize_drops += s.oversize_drops;
                        report.send_failures += s.send_failures;
                    }
                    // Claimed by another zone a moment ago: that said and counted it.
                    if body.is_none() && session.is_none() {
                        continue;
                    }
                    for s in sessions.values() {
                        s.send_control(Control::PlayerLeft(id));
                    }
                    report.leaves += 1;
                }
            }
        }

        // Hub bookkeeping once a second: periodic saves (staggered), ghost timeouts.
        if let Some(link) = &cfg.hub
            && scheduler.tick().is_multiple_of(rate.hz() as u64)
        {
            let now = Instant::now();
            let mut expired_ghosts: Vec<EntityId> = Vec::new();
            for (&id, slot) in hub_slots.iter_mut() {
                if let Some(since) = slot.ghost_since
                    && now.duration_since(since) >= GHOST_TIMEOUT
                {
                    slot.ghost_since = None;
                    match sessions.get(&id) {
                        Some(s) => {
                            // The other zone never claimed: the player plays on here.
                            zone.set_ghost(id, false);
                            s.send_control(Control::TravelRefused(
                                "the other zone never claimed you".into(),
                            ));
                        }
                        // The client is gone too: the body leaves and the character goes
                        // offline (the hub accepts the origin's leaving save of a transit).
                        None => expired_ghosts.push(id),
                    }
                }
                if slot.ghost_since.is_none()
                    && now.duration_since(slot.last_save) >= SAVE_EVERY
                    && let Some(state) = character_state(&zone, link, id, slot)
                {
                    slot.last_save = now;
                    let link = link.clone();
                    let character = slot.character;
                    tokio::spawn(async move {
                        link.save(character, state, false).await;
                    });
                }
            }
            for id in expired_ghosts {
                if let Some(slot) = hub_slots.remove(&id)
                    && let Some(state) = character_state(&zone, link, id, &slot)
                {
                    let link = link.clone();
                    tokio::spawn(async move {
                        link.save(slot.character, state, true).await;
                    });
                }
                director.human_left(&mut zone, id);
                zone.remove_player(id);
                for s in sessions.values() {
                    s.send_control(Control::PlayerLeft(id));
                }
                report.leaves += 1;
            }
        }

        // Hires that ran out, or ended early: the companion leaves once it is not in the
        // middle of an encounter (COMPANIONS.md 3.3). Looked at once a second.
        if scheduler.tick().is_multiple_of(rate.hz() as u64) && !hire_expiry.is_empty() {
            let now = now_secs();
            let due: Vec<EntityId> = hire_expiry
                .iter()
                .filter(|(id, at)| **at <= now && !director.in_encounter(**id))
                .map(|(id, _)| *id)
                .collect();
            for id in due {
                hire_expiry.remove(&id);
                director.remove_companion(&mut zone, id);
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

        phases.events += t_events.elapsed();
        let t_minds = Instant::now();
        director.pre_step(&mut zone, &world.bsp);
        phases.minds += t_minds.elapsed();
        let t_sim = Instant::now();
        zone.step(&world.bsp);
        phases.sim += t_sim.elapsed();

        let events: Vec<ZoneEvent> = zone.events.drain(..).collect();
        let t_minds = Instant::now();
        director.post_step(&mut zone, &world.bsp, &events);
        phases.minds += t_minds.elapsed();
        // What the director did: rosters, squads, encounters, loot, trials.
        for ev in std::mem::take(&mut director.events) {
            match ev {
                DirectorEvent::Spawned { id, name, kind } => {
                    let team = zone.player(id).map_or(0, |p| p.team());
                    let model = companion_models.get(&id).map(|m| m.id);
                    for s in sessions.values() {
                        s.send_control(Control::PlayerInfo {
                            id,
                            name: name.clone(),
                            team,
                            model,
                            kind: wire_kind(kind),
                        });
                    }
                }
                DirectorEvent::Removed { id } => {
                    companion_models.remove(&id);
                    hire_expiry.remove(&id);
                    for s in sessions.values() {
                        s.send_control(Control::PlayerLeft(id));
                    }
                }
                DirectorEvent::Squad { commander } => {
                    if !squads_dirty.contains(&commander) {
                        squads_dirty.push(commander);
                    }
                }
                DirectorEvent::Encounter { name, state, tell } => {
                    let wire = match state {
                        EncounterState::Engaged => {
                            report.encounters_engaged += 1;
                            control::EncounterState::Engaged
                        }
                        EncounterState::Reset => {
                            report.encounters_reset += 1;
                            control::EncounterState::Reset
                        }
                        EncounterState::Cleared { secs } => {
                            report.encounters_cleared += 1;
                            report.last_clear_secs = secs;
                            control::EncounterState::Cleared { secs }
                        }
                    };
                    info!(encounter = %name, ?state, "encounter");
                    for id in tell {
                        if let Some(s) = sessions.get(&id) {
                            s.send_control(Control::Encounter {
                                name: name.clone(),
                                state: wire,
                            });
                        }
                    }
                }
                DirectorEvent::Loot {
                    encounter,
                    kill,
                    grants,
                } => {
                    // The kill's reference: unique across restarts of this zone process.
                    let reference = ((started_unix & 0xffff_ffff) << 24) as i64 | kill as i64;
                    let mut items: Vec<(CharacterId, String)> = Vec::new();
                    let mut coins: Vec<(CharacterId, i64)> = Vec::new();
                    for g in &grants {
                        report.loot_items += g.items.len() as u64;
                        info!(encounter = %encounter, human = g.human, items = ?g.items, coin = g.coin, "loot");
                        if let Some(s) = sessions.get(&g.human) {
                            s.send_control(Control::Loot {
                                encounter: encounter.clone(),
                                items: g.items.clone(),
                                coin: g.coin,
                            });
                        }
                        if let Some(slot) = hub_slots.get(&g.human) {
                            items.extend(g.items.iter().map(|m| (slot.character, m.clone())));
                            if g.coin > 0 {
                                coins.push((slot.character, g.coin as i64));
                            }
                        }
                    }
                    // One report per kill, repeated until the hub answers; the hub pays a
                    // reference once (ECONOMY.md 9).
                    if let Some(link) = cfg.hub.clone()
                        && (!items.is_empty() || !coins.is_empty())
                    {
                        tokio::spawn(async move {
                            match link.grant_kill(reference, items, coins).await {
                                Ok(true) => {}
                                Ok(false) => info!(reference, "the kill was already paid"),
                                Err(e) => warn!(reference, "the drop of a kill is lost: {e}"),
                            }
                        });
                    }
                }
                DirectorEvent::Trial {
                    human,
                    key,
                    name,
                    verdict,
                    standing,
                    secs,
                } => {
                    let passed = verdict.is_ok();
                    info!(human, trial = %key, passed, %standing, "trial");
                    if passed {
                        report.trials_passed += 1;
                    }
                    if let Some(s) = sessions.get(&human) {
                        s.send_control(Control::Trial {
                            key: key.clone(),
                            name,
                            passed,
                            // Why not; or, for a pass, what the ledger said.
                            detail: verdict
                                .err()
                                .map_or_else(|| standing.to_string(), |f| f.to_string()),
                            secs,
                        });
                    }
                    if passed
                        && let (Some(link), Some(slot)) = (cfg.hub.clone(), hub_slots.get(&human))
                    {
                        let character = slot.character;
                        tokio::spawn(async move {
                            if let Err(e) = link.trial(character, key, secs).await {
                                warn!(character, "recording a trial: {e}");
                            }
                        });
                    }
                }
            }
        }
        // Squads that changed: their commanders are told, once, what the squad is now.
        for commander in squads_dirty.drain(..) {
            let entries = squad_entries(&director, &zone, commander);
            if let Some(s) = sessions.get_mut(&commander)
                && s.squad_told != entries
            {
                s.squad_told = entries.clone();
                s.send_control(Control::Squad(entries));
            }
        }
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
                    for s in sessions.values() {
                        s.send_control(Control::Killed { victim, killer });
                    }
                }
                ZoneEvent::Respawned(id) => {
                    // The client's prediction switches to whatever build the respawn applied.
                    let mut wears = None;
                    if let (Some(s), Some(p)) = (sessions.get_mut(&id), zone.player(id)) {
                        s.send_control(Control::BuildApplied(p.sheet.build.clone()));
                        // A respec to another frame shows the mannequin; back, the model.
                        let now = s.wears(p.frame());
                        if now != s.announced {
                            s.announced = now;
                            wears = Some((s.name.clone(), p.team(), now));
                        }
                    }
                    if let Some((name, team, model)) = wears {
                        for s in sessions.values() {
                            s.send_control(Control::PlayerInfo {
                                id,
                                name: name.clone(),
                                team,
                                model,
                                kind: BodyKind::Human,
                            });
                        }
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
        let t_snap = Instant::now();
        table.rebuild(&zone, &world, &|id| wire_kind(director.kind_of(id)));
        for s in sessions.values() {
            if let Some(leaf) = s.eye_leaf(&zone, &world) {
                pvs.prepare(&world.bsp, leaf);
            }
            // Squad sight: the rows of a commanding client's companions (COMPANIONS.md 5.2).
            if zone.player(s.id).is_some_and(crate::session::commanding) {
                for m in director.squad(s.id) {
                    if let Some(p) = zone.player(m.id).filter(|p| p.alive) {
                        pvs.prepare(&world.bsp, world.bsp.leaf_for_point(p.mover.eye()));
                    }
                }
            }
        }
        // Each session's snapshot only reads the zone, the table and the PVS rows; the
        // per-session history is its own. Spread them over the pool (rayon).
        let outgoing: Vec<(EntityId, Vec<u8>)> = {
            let zone = &zone;
            let world = &*world;
            let table = &table;
            let pvs = &pvs;
            let mut slots: Vec<&mut Session> = sessions.values_mut().collect();
            slots
                .par_iter_mut()
                .filter_map(|s| {
                    s.build_snapshot(zone, world, table, pvs, tick)
                        .map(|bytes| (s.id, bytes))
                })
                .collect()
        };
        phases.snapshot += t_snap.elapsed();
        let t_send = Instant::now();
        for (id, bytes) in outgoing {
            if let Some(s) = sessions.get_mut(&id)
                && s.conn.send_datagram(Bytes::from(bytes)).is_err()
            {
                s.send_failures += 1;
            }
        }
        phases.send += t_send.elapsed();
        phases.ticks += 1;

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
            phases.fill(&mut report);
            report.minds = director.minds();
            phases = Phases::default();
            *hub_stats.lock().unwrap() = (report.players as u32, report.tick_mean_us as f32);
            info!(
                tick = report.tick,
                players = report.players,
                tx_bps = format_args!("{:.0}", report.tx_bytes_per_player_s),
                rx_bps = format_args!("{:.0}", report.rx_bytes_per_player_s),
                snap_bps = format_args!("{:.0}", report.snapshot_payload_per_player_s),
                tick_us_mean = format_args!("{:.1}", report.tick_mean_us),
                tick_us_p99 = format_args!("{:.1}", report.tick_p99_us),
                tick_us_max = format_args!("{:.1}", report.tick_max_us),
                late_us_max = format_args!("{:.1}", report.late_max_us),
                overruns = report.overruns,
                events_us = format_args!("{:.0}", report.events_us_mean),
                sim_us = format_args!("{:.0}", report.sim_us_mean),
                snapshot_us = format_args!("{:.0}", report.snapshot_us_mean),
                send_us = format_args!("{:.0}", report.send_us_mean),
                minds = report.minds,
                minds_us = format_args!("{:.0}", report.minds_us_mean),
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
    phases.fill(&mut report);
    report.minds = director.minds();
    // Save everyone before the lights go out (HUB.md 3.2).
    if let Some(link) = &cfg.hub {
        for (&id, slot) in &hub_slots {
            if !slot.claimed_elsewhere
                && let Some(state) = character_state(&zone, link, id, slot)
            {
                link.save(slot.character, state, true).await;
            }
        }
    }
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

/// Where the ticks of a report window went.
#[derive(Default)]
struct Phases {
    ticks: u64,
    events: Duration,
    minds: Duration,
    sim: Duration,
    snapshot: Duration,
    send: Duration,
}

impl Phases {
    fn fill(&self, report: &mut ZoneReport) {
        let n = self.ticks.max(1) as f64;
        report.events_us_mean = self.events.as_secs_f64() * 1e6 / n;
        report.sim_us_mean = self.sim.as_secs_f64() * 1e6 / n;
        report.snapshot_us_mean = self.snapshot.as_secs_f64() * 1e6 / n;
        report.send_us_mean = self.send.as_secs_f64() * 1e6 / n;
        report.minds_us_mean = self.minds.as_secs_f64() * 1e6 / n;
    }
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
