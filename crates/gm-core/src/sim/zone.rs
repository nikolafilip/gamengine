//! The authoritative simulation of one zone (server only): the frame ledger, melee with lag
//! compensation, projectiles, areas, statuses, guards, deaths, respawns and respecs.

use std::collections::{BTreeMap, VecDeque};

use glam::Vec3;

use crate::build::{Build, BuildError, ContentPack, Sheet};
use crate::collide::{Aabb, BodyGrid, EntityWorld};
use crate::geom::{Capsule, ray_capsule, sweep_sphere_capsule};
use crate::matrix::{
    AttackerStats, DOT_PULSES_PER_S, DefenderStats, STAGGER_DECAY_PER_S, STAGGER_IMMUNITY_MS,
    STAGGER_MS, resolve_damage,
};
use crate::movement::{MoveVars, yaw_vectors};
use crate::rng::Rng;
use crate::sim::mover::{
    Action, GuardState, Input, Mover, anim, capsule_at, melee_hit_point, step_mover,
};
use crate::sim::{
    CREDIT_BURST, DRAIN_DEPTH, HISTORY_TICKS, MAX_FRAMES_PER_TICK, MAX_QUEUED_FRAMES,
    MAX_REWIND_TICKS, PROJECTILE_OWNER_GRACE, RESERVE_FRAMES, RESPAWN_MS, REWIND_ALLOWANCE_TICKS,
    tick_delta,
};
use crate::tick::{Tick, TickRate};
use crate::trace::{CollisionWorld, Contents, Hull};
use crate::vocab::{
    ApplyStatus, ArchetypeFrame, AreaEffect, Bypass, DamagePacket, DamageType, EntityId, Falloff,
    Guard, Interrupt, MeleeArc, Origin, Projectile as ProjectileDef, Riposte, Shape, StackRule,
    Status, StatusTarget, Trigger, Verb,
};

/// Damage- and heal-over-time pulse every this many server ticks (MATRIX.md 8: 4 per second).
pub const DOT_INTERVAL_TICKS: Tick = 64 / DOT_PULSES_PER_S;
/// From this many living bodies on, a tick keeps a grid over them for its sweeps.
const GRID_FROM: usize = 24;

/// A spawn point: hull origin, facing, team (0 = any).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    pub origin: Vec3,
    pub yaw: f32,
    pub team: u8,
}

/// Who produces a body's frames (COMPANIONS.md 2.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Driver {
    /// A client, through the frame ledger (PROTOCOL.md 4).
    #[default]
    Client,
    /// A mind: exactly one frame per server tick, handed over with [`Zone::drive`].
    Mind,
}

/// A body as the server sees it: a player, a companion or a creature.
#[derive(Clone, Debug)]
pub struct Player {
    pub id: EntityId,
    pub sheet: Sheet,
    pub mover: Mover,
    pub health: i32,
    pub alive: bool,
    pub respawn_at: Tick,
    pub anim: u8,
    /// Clamped rewind target for melee (PROTOCOL.md 7.4).
    pub view_tick: Tick,
    /// `min(13, half_rtt_ticks + 8)`; the server sets it from the measured RTT.
    pub max_rewind: Tick,
    pub last_input_tick: u32,
    credits: f32,
    /// `(client tick, input, clamped view tick)` waiting to run, ascending by tick.
    queue: VecDeque<(u32, Input, Tick)>,
    pub starved_ticks: u64,
    pub executed_frames: u64,
    pub dropped_frames: u64,
    pub kills: u32,
    pub deaths: u32,
    /// Stagger build-up (MATRIX.md 7), server only.
    pub stagger: f32,
    /// Build applied at the next respawn.
    pub pending_build: Option<Build>,
    /// Server tick of the last hit taken (statistics, diagnostics).
    pub last_hit_tick: Tick,
    /// In transit to another zone (HUB.md 3.3): the body stays, visible and hittable, but no
    /// frames run.
    pub ghost: bool,
    pub driver: Driver,
    /// The party the body belongs to (COMPANIONS.md 3.1): its own id until it joins another,
    /// its commander's for a companion, 0 for a creature. Damage never reads it.
    pub party: u32,
    /// No timed respawn: the body stays down until [`Zone::revive`] (an encounter holds its
    /// dead, a creature belongs to its encounter).
    pub hold: bool,
    /// A mind's frame for the next tick.
    next: Option<Input>,
}

impl Player {
    pub fn frame(&self) -> ArchetypeFrame {
        self.sheet.build.frame
    }

    pub fn team(&self) -> u8 {
        self.sheet.team
    }

    pub fn capsule(&self) -> Capsule {
        self.mover.capsule(self.frame())
    }

    pub fn aabb(&self) -> Aabb {
        self.mover.aabb()
    }

    pub fn queued_frames(&self) -> usize {
        self.queue.len()
    }

    pub fn max_health(&self) -> i32 {
        self.sheet.derived.health
    }

    fn attacker_stats(&self) -> AttackerStats {
        AttackerStats::from_derived(
            &self.sheet.derived,
            self.mover.statuses.magnitude(Status::Weaken),
        )
    }
}

/// A swing in progress (server only).
#[derive(Clone, Debug)]
pub struct Swing {
    pub attacker: EntityId,
    pub ability: u8,
    pub arc: MeleeArc,
    pub stats: AttackerStats,
    pub active_from: Tick,
    pub active_until: Tick,
    pub view_tick: Tick,
    pub hit: Vec<EntityId>,
    /// `Hit`-targeted statuses of the steps that follow the arc, applied with each hit.
    pub on_hit: Vec<ApplyStatus>,
    /// A parry riposte: not tied to a running script.
    pub riposte: bool,
}

#[derive(Clone, Debug)]
pub struct Projectile {
    pub id: EntityId,
    pub owner: EntityId,
    pub ability: u8,
    pub input_tick: u32,
    pub def: ProjectileDef,
    pub stats: AttackerStats,
    pub pos: Vec3,
    pub vel: Vec3,
    pub spawned: Tick,
    pub dies: Tick,
    pub pierce_left: u8,
    pub bounces_left: u8,
    pub hit: Vec<EntityId>,
}

/// A pulsing volume (VOCABULARY.md 5.3), server only.
#[derive(Clone, Debug)]
pub struct Area {
    pub id: EntityId,
    pub owner: EntityId,
    pub ability: u8,
    pub def: AreaEffect,
    pub stats: AttackerStats,
    pub origin: Vec3,
    /// Facing for cones.
    pub dir: Vec3,
    pub next_pulse: Tick,
    pub ends: Tick,
    pub pulses: u32,
}

impl Area {
    /// Largest extent, for the wire and for drawing.
    pub fn radius(&self) -> f32 {
        match self.def.shape {
            Shape::Sphere { radius } | Shape::Cylinder { radius, .. } => radius,
            Shape::Cone { length, .. } => length,
            Shape::Box { half_extents } => half_extents.iter().cloned().fold(0.0, f32::max),
        }
    }
}

/// Recent origins per entity for melee rewinds.
#[derive(Clone, Debug, Default)]
pub struct History {
    frames: VecDeque<(Tick, Vec<(EntityId, Vec3)>)>,
}

impl History {
    pub fn record(&mut self, tick: Tick, origins: Vec<(EntityId, Vec3)>) {
        self.frames.push_back((tick, origins));
        while self.frames.len() > HISTORY_TICKS {
            self.frames.pop_front();
        }
    }

    /// Origin of `id` at `tick`, or at the nearest later recorded tick.
    pub fn origin_at(&self, tick: Tick, id: EntityId) -> Option<Vec3> {
        self.frames
            .iter()
            .filter(|(t, _)| tick_delta(*t, tick) >= 0)
            .min_by_key(|(t, _)| tick_delta(*t, tick))
            .and_then(|(_, origins)| origins.iter().find(|(e, _)| *e == id).map(|(_, o)| *o))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    Melee,
    Projectile,
    Area,
    /// Damage over time (Bleed, Burn).
    Dot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoneEvent {
    Hit {
        attacker: EntityId,
        target: EntityId,
        amount: i32,
        kind: HitKind,
        /// What the target's block took off the hit (0 when it was not blocked).
        absorbed: i32,
    },
    Healed {
        target: EntityId,
        /// Who put the Regen there (0 = nobody).
        source: EntityId,
        /// Health actually restored: healing a full body is not healing.
        amount: i32,
    },
    Killed {
        victim: EntityId,
        /// 0 = the world.
        killer: EntityId,
    },
    Respawned(EntityId),
    ProjectileSpawned {
        id: EntityId,
        owner: EntityId,
        input_tick: u32,
    },
    ProjectileRemoved(EntityId),
    AreaSpawned {
        id: EntityId,
        owner: EntityId,
    },
    AreaRemoved(EntityId),
    Parried {
        defender: EntityId,
        attacker: EntityId,
    },
    GuardBroken(EntityId),
    Staggered(EntityId),
    StatusApplied {
        target: EntityId,
        status: Status,
        source: EntityId,
    },
}

/// The authoritative simulation of one zone.
pub struct Zone {
    pub rate: TickRate,
    pub tick: Tick,
    pub content: ContentPack,
    players: BTreeMap<EntityId, Player>,
    projectiles: Vec<Projectile>,
    areas: Vec<Area>,
    swings: Vec<Swing>,
    history: History,
    next_id: EntityId,
    rng: Rng,
    spawns: Vec<Spawn>,
    respawn_ticks: Tick,
    pub events: Vec<ZoneEvent>,
}

impl Zone {
    /// `spawns` are hull-origin spawn points; an empty list spawns at the origin.
    pub fn new(rate: TickRate, seed: u64, spawns: Vec<Spawn>, content: ContentPack) -> Zone {
        Zone {
            rate,
            tick: 0,
            content,
            players: BTreeMap::new(),
            projectiles: Vec::new(),
            areas: Vec::new(),
            swings: Vec::new(),
            history: History::default(),
            next_id: 1,
            rng: Rng::new(seed),
            spawns: if spawns.is_empty() {
                vec![Spawn {
                    origin: Vec3::new(0.0, 0.0, 24.0),
                    yaw: 0.0,
                    team: 0,
                }]
            } else {
                spawns
            },
            respawn_ticks: rate.ms_to_ticks(RESPAWN_MS),
            events: Vec::new(),
        }
    }

    pub fn players(&self) -> impl Iterator<Item = &Player> {
        self.players.values()
    }

    pub fn player(&self, id: EntityId) -> Option<&Player> {
        self.players.get(&id)
    }

    pub fn player_mut(&mut self, id: EntityId) -> Option<&mut Player> {
        self.players.get_mut(&id)
    }

    pub fn projectiles(&self) -> &[Projectile] {
        &self.projectiles
    }

    pub fn areas(&self) -> &[Area] {
        &self.areas
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn spawns(&self) -> &[Spawn] {
        &self.spawns
    }

    /// The team with fewer living or dead players (1 or 2); for auto-assignment.
    pub fn smallest_team(&self) -> u8 {
        let count = |t: u8| self.players.values().filter(|p| p.team() == t).count();
        if count(2) < count(1) { 2 } else { 1 }
    }

    fn alloc_id(&mut self) -> EntityId {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("entity ids exhausted");
        id
    }

    /// Add a player at a free spawn point of its team. Ids are monotonic and never reused.
    /// A build that fails MATRIX.md 9 is refused.
    pub fn add_player(
        &mut self,
        world: &dyn CollisionWorld,
        build: Build,
        team: u8,
    ) -> Result<EntityId, BuildError> {
        build.validate(&self.content)?;
        let (origin, yaw) = self.free_spawn(world, Hull::Player, team);
        Ok(self.add_player_at(build, team, origin, yaw))
    }

    /// Add a player at an exact hull origin (zone handoff arrivals, tests). The build is
    /// trusted (validate it first).
    pub fn add_player_at(&mut self, build: Build, team: u8, origin: Vec3, yaw: f32) -> EntityId {
        let sheet = Sheet::new(build, &self.content, team);
        self.add_body(sheet, origin, yaw, Driver::Client)
    }

    /// Add a body with a ready sheet (a player's, a companion's or a creature's) at an exact
    /// hull origin. Its party is itself; see [`Zone::set_party`].
    pub fn add_body(&mut self, sheet: Sheet, origin: Vec3, yaw: f32, driver: Driver) -> EntityId {
        let id = self.alloc_id();
        let mover = Mover::spawn(origin, yaw, &sheet);
        let health = sheet.derived.health;
        let p = Player {
            id,
            sheet,
            mover,
            health,
            alive: true,
            respawn_at: 0,
            anim: anim::IDLE,
            view_tick: self.tick,
            max_rewind: MAX_REWIND_TICKS,
            last_input_tick: 0,
            credits: CREDIT_BURST,
            queue: VecDeque::new(),
            starved_ticks: 0,
            executed_frames: 0,
            dropped_frames: 0,
            kills: 0,
            deaths: 0,
            stagger: 0.0,
            pending_build: None,
            last_hit_tick: 0,
            ghost: false,
            driver,
            party: id,
            hold: false,
            next: None,
        };
        self.players.insert(id, p);
        id
    }

    pub fn set_party(&mut self, id: EntityId, party: u32) {
        if let Some(p) = self.players.get_mut(&id) {
            p.party = party;
        }
    }

    /// Hold a body's respawn (or release it): a held body stays down until [`Zone::revive`].
    /// Releasing a dead body lets the timed respawn run from now.
    pub fn set_hold(&mut self, id: EntityId, hold: bool) {
        let now = self.tick;
        let respawn_ticks = self.respawn_ticks;
        if let Some(p) = self.players.get_mut(&id) {
            if p.hold && !hold && !p.alive {
                p.respawn_at = now.wrapping_add(respawn_ticks);
            }
            p.hold = hold;
        }
    }

    /// A mind's frame for the next tick (COMPANIONS.md 2.1). Ignored for client-driven bodies.
    pub fn drive(&mut self, id: EntityId, input: Input) {
        if let Some(p) = self.players.get_mut(&id)
            && p.driver == Driver::Mind
        {
            p.next = Some(input);
        }
    }

    /// Put a body back on its feet at `origin` with full pools and nothing running: a held
    /// body's respawn, or a creature restored by its encounter. Cooldowns are cleared too
    /// when `fresh` (a reset creature starts over; a respawning player keeps them).
    pub fn revive(&mut self, id: EntityId, origin: Vec3, yaw: f32, fresh: bool) {
        self.swings.retain(|s| s.attacker != id);
        let content = &self.content;
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let was_dead = !p.alive;
        if let Some(build) = p.pending_build.take() {
            p.sheet = Sheet::new(build, content, p.team());
        }
        let cooldowns = p.mover.cooldowns;
        p.mover = Mover::spawn(origin, yaw, &p.sheet);
        if !fresh {
            p.mover.cooldowns = cooldowns;
        } else {
            // Ready at once in the body's own frame clock.
            p.mover.cooldowns = [p.last_input_tick; crate::sim::MAX_ABILITIES];
        }
        p.health = p.sheet.derived.health;
        p.alive = true;
        p.stagger = 0.0;
        p.next = None;
        if was_dead {
            self.events.push(ZoneEvent::Respawned(id));
        }
    }

    /// A free place to stand near `near`: the point itself, then rings around it, where the
    /// hull is in open space, has ground under it and overlaps no living body. Falls back to
    /// `near`.
    pub fn spot_near(&self, world: &dyn CollisionWorld, near: Vec3, hull: Hull) -> Vec3 {
        let free = |origin: Vec3| -> Option<Vec3> {
            if world.point_contents(hull, origin) != Contents::Empty {
                return None;
            }
            // Reachable in a straight line from the point (not through a wall).
            if world.trace(Hull::Point, near, origin).fraction < 1.0 {
                return None;
            }
            let down = world.trace(hull, origin, origin - Vec3::Z * 96.0);
            if down.start_solid || down.fraction >= 1.0 {
                return None;
            }
            let bb = Aabb::around(down.end, hull);
            (!self
                .players
                .values()
                .any(|p| p.alive && p.aabb().overlaps(&bb)))
            .then_some(down.end)
        };
        for radius in [0.0f32, 48.0, 80.0, 112.0, 160.0] {
            let steps = if radius == 0.0 { 1 } else { 8 };
            for i in 0..steps {
                let a = i as f32 * core::f32::consts::TAU / steps as f32;
                let p = near + Vec3::new(a.cos(), a.sin(), 0.0) * radius;
                if let Some(spot) = free(p) {
                    return spot;
                }
            }
        }
        near
    }

    /// Mark a player as a ghost (or back); a ghost's frames are consumed but never run.
    pub fn set_ghost(&mut self, id: EntityId, ghost: bool) {
        if let Some(p) = self.players.get_mut(&id) {
            p.ghost = ghost;
            if ghost {
                p.mover.reset_actions();
            }
        }
    }

    pub fn remove_player(&mut self, id: EntityId) -> Option<Player> {
        self.swings.retain(|s| s.attacker != id);
        self.players.remove(&id)
    }

    /// Validate a new build and apply it at the player's next respawn (MATRIX.md 9).
    pub fn request_respec(&mut self, id: EntityId, build: Build) -> Result<(), BuildError> {
        build.validate(&self.content)?;
        if let Some(p) = self.players.get_mut(&id) {
            p.pending_build = Some(build);
        }
        Ok(())
    }

    /// Queue a frame for execution (PROTOCOL.md 4). `view_tick` 0 means "no rewind". Frames
    /// already executed or already queued are ignored; a late datagram's frames still slot in
    /// ahead of newer ones (UDP reorders), so the redundancy is never wasted.
    pub fn queue_input(&mut self, id: EntityId, input_tick: u32, input: Input, view_tick: Tick) {
        let now = self.tick;
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if (p.executed_frames > 0 || !p.queue.is_empty())
            && tick_delta(input_tick, p.last_input_tick) <= 0
        {
            return;
        }
        let max_rewind = p.max_rewind.min(MAX_REWIND_TICKS);
        let clamped_view = if view_tick == 0 {
            now
        } else {
            let lag = tick_delta(now, view_tick);
            if lag < 0 {
                now
            } else {
                now.wrapping_sub((lag as Tick).min(max_rewind))
            }
        };
        let pos = p
            .queue
            .partition_point(|(t, _, _)| tick_delta(*t, input_tick) < 0);
        if p.queue.get(pos).is_some_and(|(t, _, _)| *t == input_tick) {
            return;
        }
        p.queue.insert(pos, (input_tick, input, clamped_view));
        p.view_tick = clamped_view;
        while p.queue.len() > MAX_QUEUED_FRAMES {
            p.queue.pop_front();
            p.dropped_frames += 1;
        }
    }

    /// Tell the zone a client's one-way latency so `view_tick` clamps honestly.
    pub fn set_half_rtt_ticks(&mut self, id: EntityId, half_rtt_ticks: Tick) {
        if let Some(p) = self.players.get_mut(&id) {
            p.max_rewind = (half_rtt_ticks + REWIND_ALLOWANCE_TICKS).min(MAX_REWIND_TICKS);
        }
    }

    /// One server tick in the order of VOCABULARY.md 7 (steps 1–7; snapshots are the caller's).
    pub fn step(&mut self, world: &dyn CollisionWorld) {
        self.tick = self.tick.wrapping_add(1);
        let now = self.tick;
        let dt = self.rate.dt();
        let ids: Vec<EntityId> = self.players.keys().copied().collect();

        // 1 + 2: inputs and movement, players blocking each other. One shared box list per
        // tick; each mover ignores its own entry.
        let mut fires: Vec<(EntityId, u8, u8, u32, Tick)> = Vec::new();
        let mut area_spawns: Vec<(EntityId, u8, u8)> = Vec::new();
        let mut solids: Vec<(EntityId, Aabb)> = self
            .players
            .values()
            .filter(|o| o.alive)
            .map(|o| (o.id, o.aabb()))
            .collect();
        // With a crowd, a grid over the boxes: each sweep looks at its neighbours only.
        let mut grid = (solids.len() >= GRID_FROM && solids.len() <= u16::MAX as usize)
            .then(|| BodyGrid::build(&solids));
        for &id in &ids {
            let p = self.players.get_mut(&id).expect("id from keys");
            p.credits = (p.credits + 1.0).min(CREDIT_BURST);
            p.stagger = (p.stagger - STAGGER_DECAY_PER_S * dt).max(0.0);
            let depth = p.queue.len();
            let mut allowed: u32 = if depth >= DRAIN_DEPTH {
                2
            } else if depth > RESERVE_FRAMES {
                1
            } else {
                0
            };
            let mut executed = 0;
            let mut actions: Vec<(u32, Tick, Action)> = Vec::new();
            let mut sink = Vec::new();
            if p.driver == Driver::Mind {
                // A mind's body runs exactly one frame per tick, in its own frame clock, and
                // its swings are not rewound: it has no latency to compensate.
                allowed = 0;
                if let Some(input) = p.next.take() {
                    let t = p.last_input_tick.wrapping_add(1);
                    p.last_input_tick = t;
                    p.executed_frames += 1;
                    p.view_tick = now;
                    executed = 1;
                    if p.alive {
                        let sheet = &p.sheet;
                        let composite = EntityWorld {
                            world,
                            solids: &solids,
                            ignore: id,
                            own: Some(p.mover.aabb()),
                            grid: grid.as_ref(),
                        };
                        step_mover(&composite, sheet, &mut p.mover, &input, t, dt, &mut sink);
                        actions.extend(sink.drain(..).map(|a| (t, now, a)));
                    }
                }
            }
            while allowed > 0 && executed < MAX_FRAMES_PER_TICK && p.credits >= 1.0 {
                let Some((t, input, view)) = p.queue.pop_front() else {
                    break;
                };
                allowed -= 1;
                p.credits -= 1.0;
                executed += 1;
                p.last_input_tick = t;
                p.executed_frames += 1;
                if p.alive && !p.ghost {
                    let sheet = &p.sheet;
                    let composite = EntityWorld {
                        world,
                        solids: &solids,
                        ignore: id,
                        own: Some(p.mover.aabb()),
                        grid: grid.as_ref(),
                    };
                    step_mover(&composite, sheet, &mut p.mover, &input, t, dt, &mut sink);
                    actions.extend(sink.drain(..).map(|a| (t, view, a)));
                } else {
                    p.mover.yaw = input.yaw;
                    p.mover.pitch = input.pitch;
                    p.mover.buttons_prev = input.buttons;
                }
            }
            if executed == 0 && p.driver == Driver::Client {
                p.starved_ticks += 1;
            }
            if p.alive
                && let Ok(i) = solids.binary_search_by_key(&id, |(e, _)| *e)
            {
                let (old, new) = (solids[i].1, p.aabb());
                solids[i].1 = new;
                if let Some(g) = grid.as_mut() {
                    g.moved(i as u16, &old, &new);
                }
            }
            let stats = p.attacker_stats();
            for (t, view_tick, a) in actions {
                match a {
                    // Script times are in the frame's tick space; re-anchor them to server ticks.
                    Action::Swing {
                        ability,
                        step,
                        active_from,
                        active_until,
                    } => {
                        let ab = &p.sheet.kit.abilities[ability as usize];
                        let Verb::MeleeArc(arc) = &ab.steps[step as usize].verb else {
                            continue;
                        };
                        let on_hit = hit_statuses(&ab.steps[step as usize + 1..]);
                        self.swings.push(Swing {
                            attacker: id,
                            ability,
                            arc: arc.clone(),
                            stats,
                            active_from: now
                                .wrapping_add(tick_delta(active_from, t).max(0) as Tick),
                            active_until: now
                                .wrapping_add(tick_delta(active_until, t).max(0) as Tick),
                            view_tick,
                            hit: Vec::new(),
                            on_hit,
                            riposte: false,
                        });
                    }
                    Action::Fire { ability, step } => {
                        fires.push((id, ability, step, t, view_tick));
                    }
                    Action::Area { ability, step } => area_spawns.push((id, ability, step)),
                    Action::ParryOpened => {}
                }
            }
        }
        self.history.record(
            now,
            self.players
                .values()
                .filter(|p| p.alive)
                .map(|p| (p.id, p.mover.mv.origin))
                .collect(),
        );

        // 3: melee resolution with lag compensation, then projectile and area spawns.
        self.resolve_swings(world);
        for (owner, ability, step, input_tick, view_tick) in fires {
            self.fire(world, owner, ability, step, input_tick, view_tick);
        }
        for (owner, ability, step) in area_spawns {
            self.spawn_area_from_step(world, owner, ability, step);
        }

        // 4: projectiles.
        self.step_projectiles(world);

        // 5: areas pulse.
        self.pulse_areas(world);

        // 6: statuses tick (damage and healing over time).
        if now.is_multiple_of(DOT_INTERVAL_TICKS) {
            self.pulse_dots();
        }

        // 7: respawns (a held body waits for whoever holds it).
        for &id in &ids {
            let due = self
                .players
                .get(&id)
                .is_some_and(|p| !p.alive && !p.hold && tick_delta(now, p.respawn_at) >= 0);
            if due {
                self.respawn(world, id);
            }
        }
        for p in self.players.values_mut() {
            p.anim = compute_anim(p);
        }
    }

    fn resolve_swings(&mut self, world: &dyn CollisionWorld) {
        let now = self.tick;
        // Everything a landed hit needs, captured before any damage is applied: applying
        // damage can remove swings (a death) or add them (a riposte), so indices do not hold.
        struct Landed {
            target: EntityId,
            attacker: EntityId,
            packet: DamagePacket,
            stats: AttackerStats,
            parryable: bool,
            on_hit: Vec<ApplyStatus>,
            dir: Vec3,
        }
        let mut hits: Vec<Landed> = Vec::new();
        for s in self.swings.iter_mut() {
            if tick_delta(now, s.active_from) < 0 || tick_delta(now, s.active_until) >= 0 {
                continue;
            }
            let Some(attacker) = self.players.get(&s.attacker) else {
                continue;
            };
            if !attacker.alive {
                continue;
            }
            // A stagger or shock cleared the script: the swing never lands.
            if !s.riposte
                && !attacker
                    .mover
                    .script
                    .is_some_and(|sc| sc.ability == s.ability)
            {
                continue;
            }
            let eye = attacker.mover.eye();
            let yaw = attacker.mover.yaw;
            for target in self.players.values() {
                if target.id == s.attacker || !target.alive || s.hit.contains(&target.id) {
                    continue;
                }
                if s.hit.len() >= s.arc.max_targets as usize {
                    break;
                }
                let origin = self
                    .history
                    .origin_at(s.view_tick, target.id)
                    .unwrap_or(target.mover.mv.origin);
                let cap = capsule_at(origin, target.mover.mv.hull, target.frame());
                let Some(point) = melee_hit_point(eye, yaw, &s.arc, &cap) else {
                    continue;
                };
                if world.trace(Hull::Point, eye, point).fraction < 1.0 {
                    continue;
                }
                let dir = (origin - attacker.mover.mv.origin)
                    .truncate()
                    .normalize_or_zero()
                    .extend(0.0);
                let scale = s.arc.cleave_falloff.powi(s.hit.len() as i32);
                s.hit.push(target.id);
                let mut packet = s.arc.damage;
                packet.amount = ((packet.amount as f32) * scale).round() as u16;
                hits.push(Landed {
                    target: target.id,
                    attacker: s.attacker,
                    packet,
                    stats: s.stats,
                    parryable: s.arc.parryable,
                    on_hit: s.on_hit.clone(),
                    dir,
                });
            }
        }
        for h in hits {
            let landed = self.apply_damage(
                h.target,
                h.attacker,
                &h.packet,
                h.stats,
                h.dir,
                HitKind::Melee,
                Some(h.parryable),
            );
            if landed {
                for st in &h.on_hit {
                    self.apply_status(h.target, h.attacker, st);
                }
            }
        }
        // Keep finished swings around briefly so late diagnostics can see them.
        self.swings.retain(|s| {
            tick_delta(now, s.active_until) < 8 && self.players.contains_key(&s.attacker)
        });
    }

    fn fire(
        &mut self,
        world: &dyn CollisionWorld,
        owner: EntityId,
        ability: u8,
        step: u8,
        input_tick: u32,
        view_tick: Tick,
    ) {
        let Some(p) = self.players.get(&owner) else {
            return;
        };
        if !p.alive {
            return;
        }
        let Verb::Projectile(def) =
            &p.sheet.kit.abilities[ability as usize].steps[step as usize].verb
        else {
            return;
        };
        let def = def.clone();
        let stats = p.attacker_stats();
        let eye = p.mover.eye();
        let dir = p.mover.view_dir();
        let origin = resolve_origin(p, def.spawn, None);
        // Never spawn inside a wall: fall back to the eye when the muzzle is blocked.
        let origin = if world.trace(Hull::Point, eye, origin).fraction < 1.0 {
            eye
        } else {
            origin
        };
        let owner_vel = p.mover.mv.velocity;
        let lag = tick_delta(self.tick, view_tick).clamp(0, p.max_rewind as i32) as Tick;
        for _ in 0..def.count.max(1) {
            let shot_dir = if def.spread_deg > 0.0 {
                let a = self.rng.range_f32(0.0, core::f32::consts::TAU);
                let r = self.rng.next_f32().sqrt() * def.spread_deg.to_radians();
                let side = dir.cross(Vec3::Z).normalize_or_zero();
                let side = if side.length_squared() < 0.5 {
                    Vec3::X
                } else {
                    side
                };
                let up2 = side.cross(dir).normalize_or_zero();
                (dir + (side * a.cos() + up2 * a.sin()) * r.tan()).normalize()
            } else {
                dir
            };
            let id = self.alloc_id();
            let mut proj = Projectile {
                id,
                owner,
                ability,
                input_tick,
                stats,
                pos: origin,
                vel: shot_dir * def.speed + owner_vel * def.inherit_velocity,
                spawned: self.tick,
                // The forward step below consumes `lag` ticks of the lifetime up front.
                dies: self
                    .tick
                    .wrapping_add(def.lifetime.saturating_sub(lag).max(1)),
                pierce_left: def.pierce,
                bounces_left: def.bounce.count,
                hit: Vec::new(),
                def: def.clone(),
            };
            self.events.push(ZoneEvent::ProjectileSpawned {
                id,
                owner,
                input_tick,
            });
            // Forward step (PROTOCOL.md 7.4): the projectile exists at the time the attacker saw,
            // and each caught-up tick is swept against where the targets *were* at that tick, so
            // faking latency buys nothing but stale targets.
            let mut alive = true;
            for i in 0..lag {
                let at = self.tick.wrapping_sub(lag - i);
                if !self.step_projectile(world, &mut proj, Some(at)) {
                    alive = false;
                    break;
                }
            }
            if alive {
                self.projectiles.push(proj);
            } else {
                self.events.push(ZoneEvent::ProjectileRemoved(id));
            }
        }
    }

    fn step_projectiles(&mut self, world: &dyn CollisionWorld) {
        let mut projectiles = std::mem::take(&mut self.projectiles);
        let mut keep = Vec::with_capacity(projectiles.len());
        for mut proj in projectiles.drain(..) {
            if self.step_projectile(world, &mut proj, None) {
                keep.push(proj);
            } else {
                self.events.push(ZoneEvent::ProjectileRemoved(proj.id));
            }
        }
        // Projectiles spawned by expiries during the loop (none in v1) would go here.
        self.projectiles = keep;
    }

    /// One tick of one projectile. Returns `false` when it is gone. With `rewind_to`, target
    /// capsules come from the position history at that tick (forward steps at spawn).
    fn step_projectile(
        &mut self,
        world: &dyn CollisionWorld,
        proj: &mut Projectile,
        rewind_to: Option<Tick>,
    ) -> bool {
        let now = self.tick;
        if tick_delta(now, proj.dies) >= 0 {
            let at = proj.pos;
            let def = proj.def.clone();
            self.run_triggered(&def.on_expire, proj.owner, proj.stats, at, None);
            return false;
        }
        let dt = self.rate.dt();
        proj.vel.z -= MoveVars::QUAKE.gravity * proj.def.gravity_scale * dt;
        if proj.def.drag > 0.0 {
            proj.vel *= (1.0 - proj.def.drag * dt).max(0.0);
        }
        let start = proj.pos;
        let end = start + proj.vel * dt;
        let world_hit = world.trace(Hull::Point, start, end);
        let mut best: Option<(f32, EntityId)> = None;
        let grace = tick_delta(now, proj.spawned) < PROJECTILE_OWNER_GRACE as i32;
        for target in self.players.values() {
            if !target.alive || proj.hit.contains(&target.id) {
                continue;
            }
            if target.id == proj.owner && grace {
                continue;
            }
            let cap = match rewind_to.and_then(|at| self.history.origin_at(at, target.id)) {
                Some(origin) => capsule_at(origin, target.mover.mv.hull, target.frame()),
                None => target.capsule(),
            };
            if let Some(t) = sweep_sphere_capsule(start, end, proj.def.radius, &cap)
                && t <= world_hit.fraction
                && best.is_none_or(|(bt, _)| t < bt)
            {
                best = Some((t, target.id));
            }
        }
        match best {
            Some((t, target)) => {
                proj.pos = start + (end - start) * t;
                proj.hit.push(target);
                let dir = proj.vel.normalize_or_zero();
                let owner = proj.owner;
                let stats = proj.stats;
                let def = proj.def.clone();
                let landed = self.apply_damage(
                    target,
                    owner,
                    &def.damage,
                    stats,
                    dir,
                    HitKind::Projectile,
                    None,
                );
                if landed {
                    self.run_triggered(&def.on_hit, owner, stats, proj.pos, Some(target));
                }
                if proj.pierce_left == 0 {
                    return false;
                }
                proj.pierce_left -= 1;
                proj.pos = end;
                true
            }
            None => {
                if world_hit.start_solid {
                    return false;
                }
                if world_hit.fraction < 1.0 {
                    if proj.bounces_left > 0 && proj.def.bounce.restitution > 0.0 {
                        proj.bounces_left -= 1;
                        proj.pos = world_hit.end;
                        let n = world_hit.plane_normal;
                        proj.vel =
                            (proj.vel - n * (2.0 * proj.vel.dot(n))) * proj.def.bounce.restitution;
                        return true;
                    }
                    let at = world_hit.end;
                    let def = proj.def.clone();
                    self.run_triggered(&def.on_hit, proj.owner, proj.stats, at, None);
                    return false;
                }
                proj.pos = end;
                true
            }
        }
    }

    /// `on_hit` / `on_expire` triggers: statuses on the hit entity or the actor, areas at
    /// the impact point.
    fn run_triggered(
        &mut self,
        triggers: &[Trigger],
        owner: EntityId,
        stats: AttackerStats,
        impact: Vec3,
        hit: Option<EntityId>,
    ) {
        for t in triggers {
            match t {
                Trigger::Status(s) => match s.target {
                    StatusTarget::Actor => self.apply_status(owner, owner, s),
                    StatusTarget::Hit | StatusTarget::Area => {
                        if let Some(h) = hit {
                            self.apply_status(h, owner, s);
                        }
                    }
                },
                Trigger::Area(ae) => {
                    let (origin, dir) = match self.players.get(&owner) {
                        Some(p) => (
                            resolve_origin(p, ae.origin, Some(impact)),
                            p.mover.view_dir(),
                        ),
                        None => (impact, Vec3::X),
                    };
                    self.spawn_area(owner, 0, ae.clone(), stats, origin, dir);
                }
            }
        }
    }

    fn spawn_area_from_step(
        &mut self,
        world: &dyn CollisionWorld,
        owner: EntityId,
        ability: u8,
        step: u8,
    ) {
        let Some(p) = self.players.get(&owner) else {
            return;
        };
        if !p.alive {
            return;
        }
        let Verb::AreaEffect(ae) =
            &p.sheet.kit.abilities[ability as usize].steps[step as usize].verb
        else {
            return;
        };
        let ae = ae.clone();
        let stats = p.attacker_stats();
        let origin = match ae.origin {
            Origin::Aim { range } => self.aim_point(world, p, range),
            other => resolve_origin(p, other, None),
        };
        let dir = p.mover.view_dir();
        self.spawn_area(owner, ability, ae, stats, origin, dir);
    }

    /// `Origin::Aim` (VOCABULARY.md 4): the first body or world surface along the actor's view
    /// ray within `range`, dropped to the ground; under a body, its feet. Current positions:
    /// what is placed is a spot on the floor, and it does not follow anyone.
    pub fn aim_point(&self, world: &dyn CollisionWorld, p: &Player, range: f32) -> Vec3 {
        let eye = p.mover.eye();
        let dir = p.mover.view_dir();
        let tr = world.trace(Hull::Point, eye, eye + dir * range);
        let mut reach = range * tr.fraction;
        let mut body: Option<Vec3> = None;
        for o in self.players.values() {
            if o.id == p.id || !o.alive {
                continue;
            }
            if let Some(t) = ray_capsule(eye, dir, reach, &o.capsule()) {
                reach = t;
                body = Some(o.mover.mv.origin + Vec3::new(0.0, 0.0, o.mover.mv.hull.mins().z));
            }
        }
        // From just above the feet, or from a little short of the surface the ray met.
        let from = match body {
            Some(feet) => feet + Vec3::Z * 8.0,
            None => eye + dir * (reach - 4.0).max(0.0),
        };
        let down = world.trace(Hull::Point, from, from - Vec3::Z * 1024.0);
        if down.start_solid { from } else { down.end }
    }

    fn spawn_area(
        &mut self,
        owner: EntityId,
        ability: u8,
        def: AreaEffect,
        stats: AttackerStats,
        origin: Vec3,
        dir: Vec3,
    ) {
        let id = self.alloc_id();
        let now = self.tick;
        let ends = now.wrapping_add(def.delay).wrapping_add(def.duration);
        self.areas.push(Area {
            id,
            owner,
            ability,
            next_pulse: now.wrapping_add(def.delay),
            ends,
            pulses: 0,
            def,
            stats,
            origin,
            dir,
        });
        self.events.push(ZoneEvent::AreaSpawned { id, owner });
    }

    fn pulse_areas(&mut self, world: &dyn CollisionWorld) {
        let now = self.tick;
        let mut areas = std::mem::take(&mut self.areas);
        let mut keep = Vec::with_capacity(areas.len());
        for mut area in areas.drain(..) {
            if tick_delta(now, area.next_pulse) >= 0 {
                self.pulse_area(world, &area);
                area.pulses += 1;
                area.next_pulse = area.next_pulse.wrapping_add(area.def.interval.max(1));
            }
            let done = area.def.duration == 0 && area.pulses > 0
                || area.def.duration > 0 && tick_delta(area.next_pulse, area.ends) > 0;
            if done {
                self.events.push(ZoneEvent::AreaRemoved(area.id));
            } else {
                keep.push(area);
            }
        }
        self.areas = keep;
    }

    fn pulse_area(&mut self, world: &dyn CollisionWorld, area: &Area) {
        // Targets: capsules overlapping the shape, nearest first, bounded by max_targets.
        let mut targets: Vec<(f32, EntityId)> = Vec::new();
        for p in self.players.values() {
            if !p.alive || (area.def.exclude_actor && p.id == area.owner) {
                continue;
            }
            let cap = p.capsule();
            let Some(norm) = shape_overlap(&area.def.shape, area.origin, area.dir, &cap) else {
                continue;
            };
            if area.def.requires_los
                && world.trace(Hull::Point, area.origin, cap.center()).fraction < 1.0
            {
                continue;
            }
            targets.push((norm, p.id));
        }
        targets.sort_by(|a, b| a.0.total_cmp(&b.0));
        if area.def.max_targets > 0 {
            targets.truncate(area.def.max_targets as usize);
        }
        for (norm, id) in targets {
            if let Some(packet) = area.def.damage {
                let falloff = match area.def.falloff {
                    Falloff::None => 1.0,
                    Falloff::Linear => (1.0 - norm).clamp(0.0, 1.0),
                    Falloff::InverseSquare => (1.0 - norm).clamp(0.0, 1.0).powi(2),
                };
                let mut packet = packet;
                packet.amount = ((packet.amount as f32) * falloff).round() as u16;
                if packet.amount > 0 {
                    let dir = (self.players[&id].mover.mv.origin - area.origin)
                        .truncate()
                        .normalize_or_zero()
                        .extend(0.0);
                    self.apply_damage(
                        id,
                        area.owner,
                        &packet,
                        area.stats,
                        dir,
                        HitKind::Area,
                        None,
                    );
                }
            }
            for s in &area.def.effects {
                match s.target {
                    StatusTarget::Actor => self.apply_status(area.owner, area.owner, s),
                    StatusTarget::Hit | StatusTarget::Area => self.apply_status(id, area.owner, s),
                }
            }
        }
    }

    /// Bleed, Burn and Regen pulses (MATRIX.md 8), four per second.
    fn pulse_dots(&mut self) {
        let ids: Vec<EntityId> = self.players.keys().copied().collect();
        for id in ids {
            let Some(p) = self.players.get(&id) else {
                continue;
            };
            if !p.alive {
                continue;
            }
            let slots: Vec<_> = p.mover.statuses.active().copied().collect();
            // Fractional magnitudes accumulate across the four pulses of a second so that a
            // magnitude of m deals exactly floor(m) per second, never 0 and never rounded up.
            let pulse = (self.tick / DOT_INTERVAL_TICKS) % DOT_PULSES_PER_S;
            for slot in slots {
                let per = slot.magnitude / DOT_PULSES_PER_S as f32;
                let amount =
                    (per * (pulse + 1) as f32).floor() as i32 - (per * pulse as f32).floor() as i32;
                let source = slot.source;
                let stats = self
                    .players
                    .get(&source)
                    .map_or(AttackerStats::NEUTRAL, |s| s.attacker_stats());
                match slot.status {
                    Some(Status::Bleed) | Some(Status::Burn) if amount > 0 => {
                        let dtype = if slot.status == Some(Status::Bleed) {
                            DamageType::Pierce
                        } else {
                            DamageType::Flame
                        };
                        let packet = DamagePacket {
                            amount: amount as u16,
                            dtype,
                            bypass: if dtype == DamageType::Pierce {
                                Bypass::ARMOR
                            } else {
                                Bypass::NONE
                            },
                            knockback: 0.0,
                            stagger: 0,
                        };
                        self.apply_damage(
                            id,
                            source,
                            &packet,
                            stats,
                            Vec3::ZERO,
                            HitKind::Dot,
                            None,
                        );
                    }
                    Some(Status::Regen) if amount > 0 => {
                        if let Some(p) = self.players.get_mut(&id) {
                            let before = p.health;
                            p.health = (p.health + amount).min(p.max_health());
                            let healed = p.health - before;
                            if healed > 0 {
                                self.events.push(ZoneEvent::Healed {
                                    target: id,
                                    source,
                                    amount: healed,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Put a status on `target` (MATRIX.md 8), scaling the duration by the target's factor.
    pub fn apply_status(&mut self, target: EntityId, source: EntityId, s: &ApplyStatus) {
        let Some(t) = self.players.get_mut(&target) else {
            return;
        };
        if !t.alive {
            return;
        }
        let factor = if s.status.fixed_duration() {
            1.0
        } else {
            t.sheet.derived.status_duration
        };
        let duration = ((s.duration as f32) * factor).round().max(1.0) as Tick;
        let now = t.last_input_tick;
        let outcome = t.mover.statuses.apply(s, duration, now, source);
        if matches!(
            outcome,
            crate::status::Applied::Immune | crate::status::Applied::NoRoom
        ) {
            return;
        }
        if s.status == Status::Stagger || s.status == Status::Shock {
            t.mover.script = None;
            t.mover.dash = None;
            t.mover.guard = GuardState::None;
            self.swings.retain(|sw| sw.attacker != target || sw.riposte);
        } else if t.mover.script.is_some_and(|sc| {
            t.sheet.kit.abilities[sc.ability as usize].interrupt == Interrupt::OnStagger
        }) && s.status == Status::Stagger
        {
            t.mover.script = None;
        }
        self.events.push(ZoneEvent::StatusApplied {
            target,
            status: s.status,
            source,
        });
    }

    /// The damage pipeline of MATRIX.md 7 for one packet. `parryable` is `Some` for melee
    /// (whether the swing can be parried); projectiles pass `None` (never parried, blocked only
    /// by shields); areas and DoTs ignore guards. Returns whether the packet landed.
    #[allow(clippy::too_many_arguments)]
    fn apply_damage(
        &mut self,
        target: EntityId,
        attacker: EntityId,
        packet: &DamagePacket,
        stats: AttackerStats,
        dir: Vec3,
        kind: HitKind,
        parryable: Option<bool>,
    ) -> bool {
        let now = self.tick;
        let respawn_ticks = self.respawn_ticks;
        let attacker_origin = self.players.get(&attacker).map(|a| a.mover.mv.origin);
        let Some(t) = self.players.get_mut(&target) else {
            return false;
        };
        if !t.alive {
            return false;
        }
        let frame_now = t.last_input_tick;
        if t.mover.invulnerable(frame_now) {
            return false;
        }
        // A packet of amount 0 is not an attack (MATRIX.md 7): it lands for its triggers and
        // does nothing else. Nothing guards against it and it interrupts nothing.
        if packet.amount == 0 {
            return true;
        }
        // Guard (VOCABULARY.md 5.6): facing test against the attacker for melee, against
        // where the shot came from for projectiles (the shooter may have moved since).
        let guard = t.sheet.kit.guard_verb().cloned();
        let guardable = matches!(kind, HitKind::Melee | HitKind::Projectile);
        let toward = match (kind, attacker_origin) {
            (HitKind::Melee, Some(from)) => (from - t.mover.mv.origin).truncate(),
            _ => -dir.truncate(),
        };
        let (fwd, _) = yaw_vectors(t.mover.yaw);
        let facing_deg = if toward.length_squared() > 1e-6 {
            fwd.truncate()
                .dot(toward.normalize())
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
        } else {
            0.0
        };
        let mut block = None;
        let mut guard_broke = false;
        if guardable {
            match (t.mover.guard, &guard) {
                (GuardState::Block, Some(Guard::Block(b)))
                    if facing_deg <= b.arc_deg * 0.5
                        && (kind == HitKind::Melee || b.stops_projectiles) =>
                {
                    let cost = b.stamina_per_hit as f32;
                    if t.mover.stamina >= cost {
                        t.mover.stamina -= cost;
                        t.mover.regen_pause_until = frame_now
                            .wrapping_add(self.rate.ms_to_ticks(crate::sim::REGEN_PAUSE_MS));
                        block = Some(b.mitigation);
                    } else {
                        t.mover.stamina = 0.0;
                        t.mover.regen_pause_until = frame_now
                            .wrapping_add(self.rate.ms_to_ticks(crate::sim::REGEN_PAUSE_MS));
                        t.mover.guard = GuardState::None;
                        guard_broke = true;
                    }
                }
                (GuardState::Parry { until }, Some(Guard::Parry(p)))
                    if tick_delta(frame_now, until) < 0
                        && facing_deg <= p.arc_deg * 0.5
                        && parryable == Some(true) =>
                {
                    t.mover.guard = GuardState::None;
                    let on_success = p.on_success.clone();
                    let defender_stats = t.attacker_stats();
                    self.events.push(ZoneEvent::Parried {
                        defender: target,
                        attacker,
                    });
                    // Riposte and statuses land on the attacker.
                    for r in on_success {
                        match r {
                            Riposte::Status(s) => self.apply_status(attacker, target, &s),
                            Riposte::Swing(arc) => {
                                self.swings.push(Swing {
                                    attacker: target,
                                    ability: 0,
                                    active_from: now.wrapping_add(arc.timing.windup),
                                    active_until: now
                                        .wrapping_add(arc.timing.windup + arc.timing.active),
                                    arc,
                                    stats: defender_stats,
                                    view_tick: now,
                                    hit: Vec::new(),
                                    on_hit: Vec::new(),
                                    riposte: true,
                                });
                            }
                        }
                    }
                    return false;
                }
                _ => {}
            }
        }
        let t = self.players.get_mut(&target).expect("still there");
        let d = &t.sheet.derived;
        let defender = DefenderStats {
            armour_class: t.sheet.build.armour,
            aspects: t.sheet.build.aspects,
            armour: d.armour,
            ward: d.ward,
            evasion: d.evasion,
            fortify: t.mover.statuses.magnitude(Status::Fortify),
            evading: t.mover.evading(frame_now),
            exposed: t.mover.statuses.has(Status::Expose),
            block,
        };
        let amount = resolve_damage(packet, &stats, &defender);
        let absorbed = if block.is_some() {
            let unblocked = DefenderStats {
                block: None,
                ..defender
            };
            (resolve_damage(packet, &stats, &unblocked) - amount).max(0)
        } else {
            0
        };
        t.health -= amount;
        t.last_hit_tick = now;
        let knock = dir * packet.knockback * stats.knockback_dealt * d.knockback_taken;
        t.mover.mv.velocity += knock;
        if knock.z > 0.0 {
            t.mover.mv.on_ground = false;
        }
        if t.mover.script.is_some_and(|sc| {
            t.sheet.kit.abilities[sc.ability as usize].interrupt == Interrupt::OnDamage
        }) {
            t.mover.script = None;
        }
        self.events.push(ZoneEvent::Hit {
            attacker,
            target,
            amount,
            kind,
            absorbed,
        });
        // Stagger build-up (guard mitigation does not reduce it), immunity window.
        let mut stagger = guard_broke;
        if packet.stagger > 0 && tick_delta(frame_now, t.mover.statuses.stagger_immune_until) >= 0 {
            t.stagger += packet.stagger as f32;
            if t.stagger >= d.stagger_threshold {
                t.stagger = 0.0;
                stagger = true;
            }
        }
        if guard_broke {
            self.events.push(ZoneEvent::GuardBroken(target));
        }
        if t.health <= 0 {
            t.health = 0;
            t.alive = false;
            t.deaths += 1;
            t.respawn_at = now.wrapping_add(respawn_ticks);
            t.mover.reset_actions();
            t.mover.mv.velocity = Vec3::ZERO;
            t.stagger = 0.0;
            self.swings.retain(|s| s.attacker != target);
            if attacker != target
                && let Some(a) = self.players.get_mut(&attacker)
            {
                a.kills += 1;
            }
            self.events.push(ZoneEvent::Killed {
                victim: target,
                killer: attacker,
            });
            return true;
        }
        if stagger {
            let rate = self.rate;
            let verb = ApplyStatus {
                status: Status::Stagger,
                duration: rate.ms_to_ticks(STAGGER_MS),
                magnitude: 1.0,
                max_stacks: 1,
                stacking: StackRule::Refresh,
                target: StatusTarget::Hit,
                dispellable: false,
            };
            self.apply_status(target, attacker, &verb);
            if let Some(t) = self.players.get_mut(&target) {
                t.mover.statuses.stagger_immune_until = frame_now
                    .wrapping_add(verb.duration)
                    .wrapping_add(rate.ms_to_ticks(STAGGER_IMMUNITY_MS));
                self.events.push(ZoneEvent::Staggered(target));
            }
        }
        true
    }

    fn respawn(&mut self, world: &dyn CollisionWorld, id: EntityId) {
        let team = self.players.get(&id).map_or(0, |p| p.team());
        let (origin, yaw) = self.free_spawn(world, Hull::Player, team);
        let content = &self.content;
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if let Some(build) = p.pending_build.take() {
            p.sheet = Sheet::new(build, content, p.team());
        }
        let cooldowns = p.mover.cooldowns;
        p.mover = Mover::spawn(origin, yaw, &p.sheet);
        p.mover.cooldowns = cooldowns;
        p.health = p.sheet.derived.health;
        p.alive = true;
        p.stagger = 0.0;
        self.events.push(ZoneEvent::Respawned(id));
    }

    /// A spawn point of `team` (0 = any; falls back to any team's points), with small offsets
    /// when occupied, where the hull is in open space and overlaps no living player.
    fn free_spawn(&mut self, world: &dyn CollisionWorld, hull: Hull, team: u8) -> (Vec3, f32) {
        let mut candidates: Vec<Spawn> = self
            .spawns
            .iter()
            .filter(|s| team == 0 || s.team == team || s.team == 0)
            .copied()
            .collect();
        if candidates.is_empty() {
            candidates = self.spawns.clone();
        }
        let n = candidates.len();
        let start = self.rng.below(n as u32) as usize;
        // The point itself, the eight places round it, then the sixteen round those: a
        // crowd arriving at once (squads come four bodies to a player) still finds room.
        let mut offsets: Vec<Vec3> = vec![
            Vec3::ZERO,
            Vec3::new(40.0, 0.0, 0.0),
            Vec3::new(-40.0, 0.0, 0.0),
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(0.0, -40.0, 0.0),
            Vec3::new(40.0, 40.0, 0.0),
            Vec3::new(-40.0, -40.0, 0.0),
            Vec3::new(40.0, -40.0, 0.0),
            Vec3::new(-40.0, 40.0, 0.0),
        ];
        for a in -2..=2i32 {
            for b in -2..=2i32 {
                if a.abs() == 2 || b.abs() == 2 {
                    offsets.push(Vec3::new(a as f32 * 40.0, b as f32 * 40.0, 0.0));
                }
            }
        }
        for &off in &offsets {
            for i in 0..n {
                let s = candidates[(start + i) % n];
                let origin = s.origin + off;
                if world.point_contents(hull, origin) != Contents::Empty {
                    continue;
                }
                let bb = Aabb::around(origin, hull);
                if self
                    .players
                    .values()
                    .any(|p| p.alive && p.aabb().overlaps(&bb))
                {
                    continue;
                }
                return (origin, s.yaw);
            }
        }
        let s = candidates[start];
        (s.origin, s.yaw)
    }
}

/// `Hit`-targeted statuses among the steps that follow a verb, up to the next hitting verb.
fn hit_statuses(following: &[crate::vocab::Step]) -> Vec<ApplyStatus> {
    let mut out = Vec::new();
    for step in following {
        match &step.verb {
            Verb::ApplyStatus(s) if s.target == StatusTarget::Hit => out.push(*s),
            Verb::MeleeArc(_) | Verb::Projectile(_) | Verb::AreaEffect(_) => break,
            _ => {}
        }
    }
    out
}

/// Where a verb anchors, for the actor `p` (VOCABULARY.md 4).
fn resolve_origin(p: &Player, origin: Origin, impact: Option<Vec3>) -> Vec3 {
    let eye = p.mover.eye();
    match origin {
        Origin::SelfFeet => p.mover.mv.origin + Vec3::new(0.0, 0.0, p.mover.mv.hull.mins().z),
        Origin::SelfEyes => eye,
        Origin::Weapon { offset } => {
            let (fwd, right) = yaw_vectors(p.mover.yaw);
            eye + fwd * offset[0] + right * offset[1] + Vec3::Z * offset[2]
        }
        Origin::Point(pt) => Vec3::from(pt),
        // An aim is resolved against the world and the bodies (`Zone::aim_point`); where that
        // is not possible (a trigger), the impact or the eyes stand in.
        Origin::Aim { .. } | Origin::Impact => impact.unwrap_or(eye),
    }
}

/// Whether a capsule overlaps a shape at `origin` facing `dir`; returns the normalised
/// distance (0 at the centre, 1 at the edge) for falloff.
fn shape_overlap(shape: &Shape, origin: Vec3, dir: Vec3, cap: &Capsule) -> Option<f32> {
    let center = cap.center();
    match *shape {
        Shape::Sphere { radius } => {
            let d = (cap.closest_axis_point(origin) - origin).length() - cap.radius;
            (d <= radius).then(|| ((center - origin).length() / radius.max(1e-3)).min(1.0))
        }
        Shape::Cylinder { radius, height } => {
            let horiz = (center - origin).truncate().length() - cap.radius;
            let z_lo = cap.a.z - cap.radius;
            let z_hi = cap.b.z + cap.radius;
            let overlap = horiz <= radius && z_hi >= origin.z && z_lo <= origin.z + height;
            overlap.then(|| ((center - origin).truncate().length() / radius.max(1e-3)).min(1.0))
        }
        Shape::Cone {
            length,
            half_angle_deg,
        } => {
            let to = center - origin;
            let dist = to.length();
            if dist - cap.radius > length || dist < 1e-3 {
                return (dist < 1e-3).then_some(0.0);
            }
            let angle = dir
                .normalize_or_zero()
                .dot(to / dist)
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            let allowance = (cap.radius / dist).min(1.0).asin().to_degrees();
            (angle - allowance <= half_angle_deg).then(|| (dist / length.max(1e-3)).min(1.0))
        }
        Shape::Box { half_extents } => {
            let he = Vec3::from(half_extents);
            let bb = Aabb::new(origin - he, origin + he);
            let cb = Aabb::new(
                cap.a - Vec3::splat(cap.radius),
                cap.b + Vec3::splat(cap.radius),
            );
            bb.overlaps(&cb)
                .then(|| ((center - origin).length() / he.length().max(1e-3)).min(1.0))
        }
    }
}

fn compute_anim(p: &Player) -> u8 {
    if !p.alive {
        return anim::DEAD;
    }
    if p.mover.statuses.staggered() {
        return anim::STAGGER;
    }
    if p.mover.commanding(p.last_input_tick) {
        return anim::COMMAND;
    }
    if p.mover.dash.is_some() {
        return anim::DASH;
    }
    match p.mover.guard {
        GuardState::Block => return anim::GUARD,
        GuardState::Parry { .. } | GuardState::Whiff { .. } => return anim::PARRY,
        GuardState::None => {}
    }
    if let Some(s) = p.mover.script {
        let ab = &p.sheet.kit.abilities[s.ability as usize];
        // Script times live in the client's tick space; the last executed frame is "now".
        let elapsed = tick_delta(p.last_input_tick, s.started).max(0) as Tick;
        if let Some(crate::vocab::Step {
            verb: Verb::MeleeArc(arc),
            at,
        }) = ab.steps.first()
        {
            let windup_end = at + arc.timing.windup;
            let active_end = windup_end + arc.timing.active;
            return if elapsed < windup_end {
                anim::WINDUP
            } else if elapsed < active_end {
                anim::SWING
            } else {
                anim::RECOVER
            };
        }
        return anim::CAST;
    }
    if !p.mover.mv.on_ground {
        anim::AIR
    } else if p.mover.ground_speed() > 10.0 {
        anim::RUN
    } else {
        anim::IDLE
    }
}
