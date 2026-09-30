//! The shared zone simulation: players, abilities, projectiles, melee with lag compensation,
//! deaths and respawns, in the tick order of VOCABULARY.md 7 and the contract of PROTOCOL.md 7.
//!
//! [`step_mover`] is the part both sides run: the client predicts its own [`Mover`] with it and
//! the server runs it for everyone. [`Zone`] is the authoritative container the server drives;
//! it also runs in tests without any networking.

use std::collections::{BTreeMap, VecDeque};

use glam::Vec3;

use crate::collide::{Aabb, EntityWorld};
use crate::geom::{Capsule, sweep_sphere_capsule};
use crate::movement::{MoveInput, MoveVars, PlayerState, player_move, yaw_vectors};
use crate::rng::Rng;
use crate::tick::{Tick, TickRate};
use crate::trace::{CollisionWorld, Contents, Hull};
use crate::vocab::{
    Ability, AbilityId, ArchetypeFrame, Bounce, Bypass, Cooldown, Cost, DamagePacket, DamageType,
    EntityId, Interrupt, MeleeArc, MoveKind, MoveSelf, Origin, Projectile as ProjectileDef, Step,
    Timing, Verb,
};

/// Button bits (PROTOCOL.md 4). Movement direction is in the axes, not here.
pub mod buttons {
    pub const JUMP: u16 = 1 << 0;
    pub const CROUCH: u16 = 1 << 1;
    pub const PRIMARY: u16 = 1 << 2;
    pub const SECONDARY: u16 = 1 << 3;
    pub const GUARD: u16 = 1 << 4;
    pub const ABILITY1: u16 = 1 << 5;
    pub const ABILITY2: u16 = 1 << 6;
    pub const ABILITY3: u16 = 1 << 7;
    pub const ABILITY4: u16 = 1 << 8;
    pub const INTERACT: u16 = 1 << 9;
    pub const VIEWPORT: u16 = 1 << 10;
    /// Bits 11–15 must be zero on the wire.
    pub const RESERVED: u16 = 0xF800;
}

/// Animation states carried in snapshots (`anim`). Cosmetic; the client never simulates them.
pub mod anim {
    pub const IDLE: u8 = 0;
    pub const RUN: u8 = 1;
    pub const AIR: u8 = 2;
    pub const WINDUP: u8 = 3;
    pub const SWING: u8 = 4;
    pub const RECOVER: u8 = 5;
    pub const DASH: u8 = 6;
    pub const DEAD: u8 = 7;
}

/// One tick of input in simulation units (dequantized from the wire frame).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub buttons: u16,
    pub yaw: f32,
    pub pitch: f32,
    pub forward: f32,
    pub side: f32,
    /// Ability slot activated this tick (1-based), 0 = none.
    pub ability: u8,
}

pub const MAX_HEALTH: i32 = 100;
pub const MAX_STAMINA: f32 = 100.0;
pub const STAMINA_REGEN_PER_S: f32 = 15.0;
/// Lag compensation bound (PROTOCOL.md 7.4): 13 ticks ≈ 200 ms at 64 Hz.
pub const MAX_REWIND_TICKS: Tick = 13;
/// Interpolation delay a legitimate client adds on top of its one-way latency (PROTOCOL.md 4).
pub const REWIND_ALLOWANCE_TICKS: Tick = 8;
/// Rewind (13) + the longest windup and active window; PROTOCOL.md 7.4.
pub const HISTORY_TICKS: usize = 32;
/// Frame ledger (PROTOCOL.md 4).
pub const MAX_FRAMES_PER_TICK: u32 = 3;
pub const CREDIT_BURST: f32 = 8.0;
pub const MAX_QUEUED_FRAMES: usize = 32;
/// Frames held back so one tick of arrival jitter never starves the simulation (one tick of
/// added latency).
pub const RESERVE_FRAMES: usize = 1;
/// Queue depth from which two frames run per tick to drain a burst.
pub const DRAIN_DEPTH: usize = 4;
/// A projectile ignores its owner's capsule this long after spawning (VOCABULARY.md 5.2).
pub const PROJECTILE_OWNER_GRACE: Tick = 2;
pub const RESPAWN_MS: u32 = 3000;
pub const MAX_ABILITIES: usize = 8;

/// Ability slots of the Phase 2 kit (every player has the same kit until Phase 3 point-buy).
pub const SWORD: usize = 0;
pub const CROSSBOW: usize = 1;
pub const DASH: usize = 2;

/// The abilities in play plus their script durations.
#[derive(Clone, Debug)]
pub struct Kit {
    pub abilities: Vec<Ability>,
    /// Ticks a script occupies the actor (windup + active + recovery of its verbs).
    pub durations: Vec<Tick>,
}

impl Kit {
    /// Sword (melee arc), crossbow (projectile) and dash (move self). Values are tuning
    /// placeholders; feel is decided in playtests.
    pub fn phase2(rate: TickRate) -> Kit {
        let sword = Ability {
            id: AbilityId(1),
            name: "Sword".into(),
            cost: Cost::default(),
            cooldown: Cooldown {
                ticks: rate.ms_to_ticks(300),
                group: None,
            },
            steps: vec![Step {
                at: 0,
                verb: Verb::MeleeArc(MeleeArc {
                    reach: 72.0,
                    arc_deg: 90.0,
                    half_height: 40.0,
                    timing: Timing {
                        windup: rate.ms_to_ticks(90),
                        active: rate.ms_to_ticks(45),
                        recovery: rate.ms_to_ticks(160),
                    },
                    damage: DamagePacket {
                        amount: 35,
                        dtype: DamageType(1),
                        bypass: Bypass::NONE,
                        knockback: 150.0,
                        stagger: 20,
                    },
                    max_targets: 3,
                    cleave_falloff: 0.7,
                    parryable: true,
                    hit_stop: rate.ms_to_ticks(30),
                }),
            }],
            move_scale: 0.6,
            interrupt: Interrupt::OnStagger,
        };
        let crossbow = Ability {
            id: AbilityId(2),
            name: "Crossbow".into(),
            cost: Cost::default(),
            cooldown: Cooldown {
                ticks: rate.ms_to_ticks(1500),
                group: None,
            },
            steps: vec![Step {
                at: rate.ms_to_ticks(125),
                verb: Verb::Projectile(ProjectileDef {
                    speed: 1800.0,
                    gravity_scale: 0.3,
                    radius: 2.0,
                    lifetime: rate.ms_to_ticks(3000),
                    damage: DamagePacket {
                        amount: 40,
                        dtype: DamageType(2),
                        bypass: Bypass::NONE,
                        knockback: 100.0,
                        stagger: 10,
                    },
                    pierce: 0,
                    bounce: Bounce::default(),
                    drag: 0.0,
                    spawn: Origin::Weapon {
                        offset: [16.0, 4.0, -2.0],
                    },
                    inherit_velocity: 0.0,
                    spread_deg: 0.3,
                    count: 1,
                    on_hit: vec![],
                    on_expire: vec![],
                }),
            }],
            move_scale: 0.7,
            interrupt: Interrupt::Never,
        };
        let dash = Ability {
            id: AbilityId(3),
            name: "Dash".into(),
            cost: Cost {
                stamina: 30,
                focus: 0,
            },
            cooldown: Cooldown {
                ticks: rate.ms_to_ticks(1000),
                group: None,
            },
            steps: vec![Step {
                at: 0,
                verb: Verb::MoveSelf(MoveSelf {
                    kind: MoveKind::Dash {
                        speed: 900.0,
                        duration: rate.ms_to_ticks(150),
                    },
                    cancelable: false,
                    keep_friction: false,
                    iframes: 0,
                }),
            }],
            move_scale: 1.0,
            interrupt: Interrupt::Never,
        };
        let abilities = vec![sword, crossbow, dash];
        for a in &abilities {
            a.validate(rate).expect("phase 2 kit validates");
        }
        let durations = abilities.iter().map(script_duration).collect();
        Kit {
            abilities,
            durations,
        }
    }
}

fn verb_duration(v: &Verb) -> Tick {
    match v {
        Verb::MeleeArc(m) => m.timing.total(),
        Verb::MoveSelf(MoveSelf {
            kind: MoveKind::Dash { duration, .. } | MoveKind::Charge { duration, .. },
            ..
        }) => *duration,
        _ => 0,
    }
}

fn script_duration(a: &Ability) -> Tick {
    a.steps
        .iter()
        .map(|s| s.at + verb_duration(&s.verb))
        .max()
        .unwrap_or(0)
        .max(1)
}

/// A running ability script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Script {
    pub ability: u8,
    pub started: Tick,
    pub next_step: u8,
    pub ends: Tick,
}

/// An active `MoveSelf::Dash`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dash {
    pub dir: Vec3,
    pub speed: f32,
    pub until: Tick,
}

/// Everything the client predicts for its own entity. Plain `Copy` data so a prediction ring
/// can store one per tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mover {
    pub mv: PlayerState,
    pub yaw: f32,
    pub pitch: f32,
    pub stamina: f32,
    pub buttons_prev: u16,
    pub script: Option<Script>,
    /// Tick at which each slot is ready again.
    pub cooldowns: [Tick; MAX_ABILITIES],
    pub dash: Option<Dash>,
}

impl Mover {
    pub fn new(origin: Vec3, yaw: f32) -> Mover {
        Mover {
            mv: PlayerState::new(origin),
            yaw,
            pitch: 0.0,
            stamina: MAX_STAMINA,
            buttons_prev: 0,
            script: None,
            cooldowns: [0; MAX_ABILITIES],
            dash: None,
        }
    }

    pub fn eye(&self) -> Vec3 {
        self.mv.eye_position()
    }

    /// Unit view direction from yaw and pitch (positive pitch looks down).
    pub fn view_dir(&self) -> Vec3 {
        view_dir(self.yaw, self.pitch)
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::around(self.mv.origin, self.mv.hull)
    }

    /// Hitbox capsule for `frame`, standing on the hull's feet.
    pub fn capsule(&self, frame: ArchetypeFrame) -> Capsule {
        capsule_at(self.mv.origin, self.mv.hull, frame)
    }
}

pub fn view_dir(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    Vec3::new(cp * cy, cp * sy, -sp)
}

pub fn capsule_at(origin: Vec3, hull: Hull, frame: ArchetypeFrame) -> Capsule {
    let (radius, height) = frame.capsule();
    Capsule::upright(origin + Vec3::new(0.0, 0.0, hull.mins().z), radius, height)
}

/// Wrapping tick difference `a - b`.
pub fn tick_delta(a: Tick, b: Tick) -> i32 {
    a.wrapping_sub(b) as i32
}

/// What a mover asked the authoritative side to do this tick. The client ignores these (or
/// spawns cosmetic previews); the server resolves them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Swing {
        ability: usize,
        active_from: Tick,
        active_until: Tick,
    },
    Fire {
        ability: usize,
    },
}

const DASH_VARS: MoveVars = MoveVars {
    friction: 0.0,
    edge_friction: 1.0,
    accelerate: 0.0,
    air_accelerate: 0.0,
    ..MoveVars::QUAKE
};

/// Advance one mover by one tick: ability activation, script steps, movement, stamina.
/// `now` is the **client tick of the frame** on both sides (the server passes the frame's
/// tick, never its own), so cooldowns, scripts and dashes elapse identically even when the
/// server runs two frames in one tick.
pub fn step_mover<W: CollisionWorld + ?Sized>(
    world: &W,
    kit: &Kit,
    m: &mut Mover,
    input: &Input,
    now: Tick,
    dt: f32,
    actions: &mut Vec<Action>,
) {
    let pressed = input.buttons & !m.buttons_prev;
    m.buttons_prev = input.buttons;
    m.yaw = input.yaw;
    m.pitch = input.pitch;

    let slot = if input.ability > 0 {
        Some(input.ability as usize - 1)
    } else if pressed & buttons::PRIMARY != 0 {
        Some(SWORD)
    } else if pressed & buttons::SECONDARY != 0 {
        Some(CROSSBOW)
    } else if pressed & buttons::ABILITY1 != 0 {
        Some(DASH)
    } else if pressed & buttons::ABILITY2 != 0 {
        Some(3)
    } else if pressed & buttons::ABILITY3 != 0 {
        Some(4)
    } else if pressed & buttons::ABILITY4 != 0 {
        Some(5)
    } else {
        None
    };
    if let Some(slot) = slot
        && slot < kit.abilities.len().min(MAX_ABILITIES)
    {
        try_activate(kit, m, slot, now);
    }

    if let Some(mut s) = m.script {
        let ab = &kit.abilities[s.ability as usize];
        let elapsed = tick_delta(now, s.started).max(0) as Tick;
        while (s.next_step as usize) < ab.steps.len()
            && ab.steps[s.next_step as usize].at <= elapsed
        {
            resolve_step(
                m,
                input,
                &ab.steps[s.next_step as usize].verb,
                s.ability as usize,
                now,
                actions,
            );
            s.next_step += 1;
        }
        m.script = if tick_delta(now, s.ends) >= 0 {
            None
        } else {
            Some(s)
        };
    }

    let scale = m
        .script
        .map_or(1.0, |s| kit.abilities[s.ability as usize].move_scale);
    let jump = input.buttons & buttons::JUMP != 0;
    match m.dash {
        Some(d) if tick_delta(d.until, now) > 0 => {
            m.mv.velocity.x = d.dir.x * d.speed;
            m.mv.velocity.y = d.dir.y * d.speed;
            let mi = MoveInput {
                yaw: m.yaw,
                forward: 0.0,
                side: 0.0,
                jump: false,
            };
            player_move(&world, &DASH_VARS, &mut m.mv, &mi, dt);
        }
        _ => {
            m.dash = None;
            let mi = MoveInput {
                yaw: m.yaw,
                forward: input.forward.clamp(-1.0, 1.0) * scale,
                side: input.side.clamp(-1.0, 1.0) * scale,
                jump,
            };
            player_move(&world, &MoveVars::QUAKE, &mut m.mv, &mi, dt);
        }
    }
    m.stamina = (m.stamina + STAMINA_REGEN_PER_S * dt).min(MAX_STAMINA);
}

fn try_activate(kit: &Kit, m: &mut Mover, slot: usize, now: Tick) -> bool {
    if m.script.is_some() {
        return false;
    }
    if tick_delta(now, m.cooldowns[slot]) < 0 {
        return false;
    }
    let ab = &kit.abilities[slot];
    if m.stamina < ab.cost.stamina as f32 {
        return false;
    }
    m.stamina -= ab.cost.stamina as f32;
    m.cooldowns[slot] = now.wrapping_add(ab.cooldown.ticks.max(1));
    m.script = Some(Script {
        ability: slot as u8,
        started: now,
        next_step: 0,
        ends: now.wrapping_add(kit.durations[slot]),
    });
    true
}

fn resolve_step(
    m: &mut Mover,
    input: &Input,
    verb: &Verb,
    ability: usize,
    now: Tick,
    actions: &mut Vec<Action>,
) {
    match verb {
        Verb::MeleeArc(arc) => actions.push(Action::Swing {
            ability,
            active_from: now.wrapping_add(arc.timing.windup),
            active_until: now.wrapping_add(arc.timing.windup + arc.timing.active),
        }),
        Verb::Projectile(_) => actions.push(Action::Fire { ability }),
        Verb::MoveSelf(ms) => {
            let (fwd, right) = yaw_vectors(m.yaw);
            let wish = fwd * input.forward + right * input.side;
            let dir = if wish.length_squared() > 1e-4 {
                wish.normalize()
            } else {
                fwd
            };
            match ms.kind {
                MoveKind::Dash { speed, duration }
                | MoveKind::Charge {
                    speed, duration, ..
                } => {
                    m.dash = Some(Dash {
                        dir,
                        speed,
                        until: now.wrapping_add(duration),
                    });
                }
                MoveKind::Leap { forward, up } => {
                    m.mv.velocity += dir * forward + Vec3::new(0.0, 0.0, up);
                    m.mv.on_ground = false;
                }
                MoveKind::Blink { .. } => {
                    // Needs a world trace; resolved in a later phase.
                }
            }
        }
        // Statuses, areas and guards arrive in Phase 3.
        Verb::AreaEffect(_) | Verb::ApplyStatus(_) | Verb::Guard(_) => {}
    }
}

/// Wedge test of VOCABULARY.md 5.1 against one capsule. Returns the point to trace line of
/// sight to when the capsule is inside the arc.
pub fn melee_hit_point(eye: Vec3, yaw: f32, arc: &MeleeArc, target: &Capsule) -> Option<Vec3> {
    let center = target.center();
    let to = center - eye;
    let horiz = to.truncate();
    let dist = horiz.length();
    if dist - target.radius > arc.reach {
        return None;
    }
    let half_height = (target.b.z - target.a.z) * 0.5 + target.radius;
    if to.z.abs() > arc.half_height + half_height {
        return None;
    }
    if dist > 1e-3 {
        let (fwd, _) = yaw_vectors(yaw);
        let cos = fwd.truncate().dot(horiz) / dist;
        let angle = cos.clamp(-1.0, 1.0).acos().to_degrees();
        let allowance = (target.radius / dist.max(target.radius))
            .min(1.0)
            .asin()
            .to_degrees();
        if angle - allowance > arc.arc_deg * 0.5 {
            return None;
        }
    }
    Some(target.closest_axis_point(eye))
}

/// A player as the server sees it.
#[derive(Clone, Debug)]
pub struct Player {
    pub id: EntityId,
    pub frame: ArchetypeFrame,
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
}

impl Player {
    pub fn capsule(&self) -> Capsule {
        self.mover.capsule(self.frame)
    }

    pub fn aabb(&self) -> Aabb {
        self.mover.aabb()
    }

    pub fn queued_frames(&self) -> usize {
        self.queue.len()
    }
}

/// A swing in progress (server only).
#[derive(Clone, Debug)]
pub struct Swing {
    pub attacker: EntityId,
    pub ability: usize,
    pub active_from: Tick,
    pub active_until: Tick,
    pub view_tick: Tick,
    pub hit: Vec<EntityId>,
}

#[derive(Clone, Debug)]
pub struct Projectile {
    pub id: EntityId,
    pub owner: EntityId,
    pub ability: usize,
    pub input_tick: u32,
    pub pos: Vec3,
    pub vel: Vec3,
    pub radius: f32,
    pub spawned: Tick,
    pub dies: Tick,
    pub pierce_left: u8,
    pub hit: Vec<EntityId>,
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoneEvent {
    Hit {
        attacker: EntityId,
        target: EntityId,
        amount: i32,
        kind: HitKind,
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
}

/// The authoritative simulation of one zone.
pub struct Zone {
    pub rate: TickRate,
    pub tick: Tick,
    pub kit: Kit,
    players: BTreeMap<EntityId, Player>,
    projectiles: Vec<Projectile>,
    swings: Vec<Swing>,
    history: History,
    next_id: EntityId,
    rng: Rng,
    spawns: Vec<(Vec3, f32)>,
    respawn_ticks: Tick,
    pub events: Vec<ZoneEvent>,
}

impl Zone {
    /// `spawns` are `(hull origin, yaw)` pairs; an empty list spawns at the origin.
    pub fn new(rate: TickRate, seed: u64, spawns: Vec<(Vec3, f32)>) -> Zone {
        Zone {
            rate,
            tick: 0,
            kit: Kit::phase2(rate),
            players: BTreeMap::new(),
            projectiles: Vec::new(),
            swings: Vec::new(),
            history: History::default(),
            next_id: 1,
            rng: Rng::new(seed),
            spawns: if spawns.is_empty() {
                vec![(Vec3::new(0.0, 0.0, 24.0), 0.0)]
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

    pub fn history(&self) -> &History {
        &self.history
    }

    fn alloc_id(&mut self) -> EntityId {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("entity ids exhausted");
        id
    }

    /// Add a player at a free spawn point. Ids are monotonic and never reused.
    pub fn add_player(&mut self, world: &dyn CollisionWorld, frame: ArchetypeFrame) -> EntityId {
        let (origin, yaw) = self.free_spawn(world, Hull::Player);
        self.add_player_at(frame, origin, yaw)
    }

    /// Add a player at an exact hull origin (zone handoff arrivals, tests).
    pub fn add_player_at(&mut self, frame: ArchetypeFrame, origin: Vec3, yaw: f32) -> EntityId {
        let id = self.alloc_id();
        let p = Player {
            id,
            frame,
            mover: Mover::new(origin, yaw),
            health: MAX_HEALTH,
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
        };
        self.players.insert(id, p);
        id
    }

    pub fn remove_player(&mut self, id: EntityId) -> Option<Player> {
        self.swings.retain(|s| s.attacker != id);
        self.players.remove(&id)
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
        // Insert in tick order; drop exact duplicates.
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
        let mut fires: Vec<(EntityId, usize, u32, Tick)> = Vec::new();
        // Ascending by id (map order); a mover's own entry is refreshed after it moves so later
        // movers see it where it is now, as with Quake's sequential entity moves.
        let mut solids: Vec<(EntityId, Aabb)> = self
            .players
            .values()
            .filter(|o| o.alive)
            .map(|o| (o.id, o.aabb()))
            .collect();
        for &id in &ids {
            let composite = EntityWorld {
                world,
                solids: &solids,
                ignore: id,
            };
            let kit = &self.kit;
            let p = self.players.get_mut(&id).expect("id from keys");
            p.credits = (p.credits + 1.0).min(CREDIT_BURST);
            // Dejitter: keep one frame in reserve, drain bursts two at a time.
            let depth = p.queue.len();
            let mut allowed: u32 = if depth >= DRAIN_DEPTH {
                2
            } else if depth > RESERVE_FRAMES {
                1
            } else {
                0
            };
            let mut executed = 0;
            // (frame tick, view tick the frame was sent with, action)
            let mut actions: Vec<(u32, Tick, Action)> = Vec::new();
            let mut sink = Vec::new();
            while allowed > 0 && executed < MAX_FRAMES_PER_TICK && p.credits >= 1.0 {
                let Some((t, input, view)) = p.queue.pop_front() else {
                    break;
                };
                allowed -= 1;
                p.credits -= 1.0;
                executed += 1;
                p.last_input_tick = t;
                p.executed_frames += 1;
                if p.alive {
                    step_mover(&composite, kit, &mut p.mover, &input, t, dt, &mut sink);
                    actions.extend(sink.drain(..).map(|a| (t, view, a)));
                } else {
                    p.mover.yaw = input.yaw;
                    p.mover.pitch = input.pitch;
                    p.mover.buttons_prev = input.buttons;
                }
            }
            if executed == 0 {
                p.starved_ticks += 1;
            }
            if p.alive
                && let Ok(i) = solids.binary_search_by_key(&id, |(e, _)| *e)
            {
                solids[i].1 = p.aabb();
            }
            for (t, view_tick, a) in actions {
                match a {
                    // Script times are in the frame's tick space; re-anchor them to server ticks.
                    Action::Swing {
                        ability,
                        active_from,
                        active_until,
                    } => self.swings.push(Swing {
                        attacker: id,
                        ability,
                        active_from: now.wrapping_add(tick_delta(active_from, t).max(0) as Tick),
                        active_until: now.wrapping_add(tick_delta(active_until, t).max(0) as Tick),
                        view_tick,
                        hit: Vec::new(),
                    }),
                    Action::Fire { ability } => fires.push((id, ability, t, view_tick)),
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

        // 3: melee resolution with lag compensation, then projectile spawns.
        self.resolve_swings(world);
        for (owner, ability, input_tick, view_tick) in fires {
            self.fire(world, owner, ability, input_tick, view_tick);
        }

        // 4: projectiles.
        self.step_projectiles(world);

        // 6 + 7: respawns (statuses arrive in Phase 3).
        for &id in &ids {
            let due = self
                .players
                .get(&id)
                .is_some_and(|p| !p.alive && tick_delta(now, p.respawn_at) >= 0);
            if due {
                self.respawn(world, id);
            }
        }
        for p in self.players.values_mut() {
            p.anim = compute_anim(p, &self.kit);
        }
    }

    fn resolve_swings(&mut self, world: &dyn CollisionWorld) {
        let now = self.tick;
        let mut damage: Vec<(EntityId, EntityId, u16, Vec3)> = Vec::new();
        for s in &mut self.swings {
            if tick_delta(now, s.active_from) < 0 || tick_delta(now, s.active_until) >= 0 {
                continue;
            }
            let Some(attacker) = self.players.get(&s.attacker) else {
                continue;
            };
            if !attacker.alive {
                continue;
            }
            let Verb::MeleeArc(arc) = &self.kit.abilities[s.ability].steps[0].verb else {
                continue;
            };
            let eye = attacker.mover.eye();
            let yaw = attacker.mover.yaw;
            for target in self.players.values() {
                if target.id == s.attacker || !target.alive || s.hit.contains(&target.id) {
                    continue;
                }
                if s.hit.len() >= arc.max_targets as usize {
                    break;
                }
                let origin = self
                    .history
                    .origin_at(s.view_tick, target.id)
                    .unwrap_or(target.mover.mv.origin);
                let cap = capsule_at(origin, target.mover.mv.hull, target.frame);
                let Some(point) = melee_hit_point(eye, yaw, arc, &cap) else {
                    continue;
                };
                if world.trace(Hull::Point, eye, point).fraction < 1.0 {
                    continue;
                }
                let scale = arc.cleave_falloff.powi(s.hit.len() as i32);
                let amount = ((arc.damage.amount as f32) * scale).round() as u16;
                let dir = (origin - attacker.mover.mv.origin)
                    .truncate()
                    .normalize_or_zero()
                    .extend(0.0);
                s.hit.push(target.id);
                damage.push((s.attacker, target.id, amount, dir * arc.damage.knockback));
            }
        }
        self.swings.retain(|s| {
            tick_delta(now, s.active_until) < 0 && self.players.contains_key(&s.attacker)
        });
        for (attacker, target, amount, knock) in damage {
            self.apply_damage(target, attacker, amount as i32, knock, HitKind::Melee);
        }
    }

    fn fire(
        &mut self,
        world: &dyn CollisionWorld,
        owner: EntityId,
        ability: usize,
        input_tick: u32,
        view_tick: Tick,
    ) {
        let Some(p) = self.players.get(&owner) else {
            return;
        };
        if !p.alive {
            return;
        }
        let Verb::Projectile(def) = &self.kit.abilities[ability].steps[0].verb else {
            return;
        };
        let def = def.clone();
        let eye = p.mover.eye();
        let dir = p.mover.view_dir();
        let (fwd, right) = yaw_vectors(p.mover.yaw);
        let up = Vec3::Z;
        let origin = match def.spawn {
            Origin::Weapon { offset } => eye + fwd * offset[0] + right * offset[1] + up * offset[2],
            Origin::SelfFeet => p.mover.mv.origin + Vec3::new(0.0, 0.0, p.mover.mv.hull.mins().z),
            _ => eye,
        };
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
                pos: origin,
                vel: shot_dir * def.speed + owner_vel * def.inherit_velocity,
                radius: def.radius,
                spawned: self.tick,
                // The forward step below consumes `lag` ticks of the lifetime up front.
                dies: self
                    .tick
                    .wrapping_add(def.lifetime.saturating_sub(lag).max(1)),
                pierce_left: def.pierce,
                hit: Vec::new(),
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
                if !self.step_projectile(world, &mut proj, &def, Some(at)) {
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
            let Verb::Projectile(def) = &self.kit.abilities[proj.ability].steps[0].verb else {
                continue;
            };
            let def = def.clone();
            if self.step_projectile(world, &mut proj, &def, None) {
                keep.push(proj);
            } else {
                self.events.push(ZoneEvent::ProjectileRemoved(proj.id));
            }
        }
        self.projectiles = keep;
    }

    /// One tick of one projectile. Returns `false` when it is gone. With `rewind_to`, target
    /// capsules come from the position history at that tick (forward steps at spawn).
    fn step_projectile(
        &mut self,
        world: &dyn CollisionWorld,
        proj: &mut Projectile,
        def: &ProjectileDef,
        rewind_to: Option<Tick>,
    ) -> bool {
        let now = self.tick;
        if tick_delta(now, proj.dies) >= 0 {
            return false;
        }
        let dt = self.rate.dt();
        proj.vel.z -= MoveVars::QUAKE.gravity * def.gravity_scale * dt;
        if def.drag > 0.0 {
            proj.vel *= (1.0 - def.drag * dt).max(0.0);
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
                Some(origin) => capsule_at(origin, target.mover.mv.hull, target.frame),
                None => target.capsule(),
            };
            if let Some(t) = sweep_sphere_capsule(start, end, proj.radius, &cap)
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
                let knock = proj.vel.normalize_or_zero() * def.damage.knockback;
                let owner = proj.owner;
                let amount = def.damage.amount as i32;
                self.apply_damage(target, owner, amount, knock, HitKind::Projectile);
                if proj.pierce_left == 0 {
                    return false;
                }
                proj.pierce_left -= 1;
                proj.pos = end;
                true
            }
            None => {
                if world_hit.fraction < 1.0 || world_hit.start_solid {
                    return false;
                }
                proj.pos = end;
                true
            }
        }
    }

    fn apply_damage(
        &mut self,
        target: EntityId,
        attacker: EntityId,
        amount: i32,
        knockback: Vec3,
        kind: HitKind,
    ) {
        let now = self.tick;
        let respawn_ticks = self.respawn_ticks;
        let Some(t) = self.players.get_mut(&target) else {
            return;
        };
        if !t.alive {
            return;
        }
        t.health -= amount;
        t.mover.mv.velocity += knockback;
        if knockback.z > 0.0 {
            t.mover.mv.on_ground = false;
        }
        self.events.push(ZoneEvent::Hit {
            attacker,
            target,
            amount,
            kind,
        });
        if t.health <= 0 {
            t.health = 0;
            t.alive = false;
            t.deaths += 1;
            t.respawn_at = now.wrapping_add(respawn_ticks);
            t.mover.script = None;
            t.mover.dash = None;
            t.mover.mv.velocity = Vec3::ZERO;
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
        }
    }

    fn respawn(&mut self, world: &dyn CollisionWorld, id: EntityId) {
        let (origin, yaw) = self.free_spawn(world, Hull::Player);
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let cooldowns = p.mover.cooldowns;
        p.mover = Mover::new(origin, yaw);
        p.mover.cooldowns = cooldowns;
        p.health = MAX_HEALTH;
        p.alive = true;
        self.events.push(ZoneEvent::Respawned(id));
    }

    /// A spawn point (with small offsets when occupied) where the hull is in open space and
    /// overlaps no living player.
    fn free_spawn(&mut self, world: &dyn CollisionWorld, hull: Hull) -> (Vec3, f32) {
        let start = self.rng.below(self.spawns.len() as u32) as usize;
        let n = self.spawns.len();
        let offsets: [Vec3; 9] = [
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
        for &off in &offsets {
            for i in 0..n {
                let (base, yaw) = self.spawns[(start + i) % n];
                let origin = base + off;
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
                return (origin, yaw);
            }
        }
        self.spawns[start]
    }
}

fn compute_anim(p: &Player, kit: &Kit) -> u8 {
    if !p.alive {
        return anim::DEAD;
    }
    if p.mover.dash.is_some() {
        return anim::DASH;
    }
    if let Some(s) = p.mover.script {
        let ab = &kit.abilities[s.ability as usize];
        // Script times live in the client's tick space; the last executed frame is "now".
        let elapsed = tick_delta(p.last_input_tick, s.started).max(0) as Tick;
        if let Some(Step {
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
        return anim::WINDUP;
    }
    if !p.mover.mv.on_ground {
        anim::AIR
    } else if p.mover.ground_speed() > 10.0 {
        anim::RUN
    } else {
        anim::IDLE
    }
}

impl Mover {
    pub fn ground_speed(&self) -> f32 {
        self.mv.ground_speed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collide::BoxWorld;

    const REST_Z: f32 = 24.0;

    fn zone_with(world: &BoxWorld, spawns: Vec<(Vec3, f32)>) -> Zone {
        let _ = world;
        Zone::new(TickRate::COMBAT, 7, spawns)
    }

    /// Players placed exactly at the listed `(origin, yaw)` pairs, in order.
    fn arena(placements: &[(Vec3, f32)]) -> (BoxWorld, Zone, Vec<EntityId>) {
        let world = BoxWorld::floor();
        let mut zone = zone_with(&world, placements.to_vec());
        let ids = placements
            .iter()
            .map(|&(o, y)| zone.add_player_at(ArchetypeFrame::Striker, o, y))
            .collect();
        (world, zone, ids)
    }

    fn input(yaw: f32, forward: f32, buttons: u16) -> Input {
        Input {
            buttons,
            yaw,
            pitch: 0.0,
            forward,
            side: 0.0,
            ability: 0,
        }
    }

    /// Feed every player one new frame per tick and step. The frame tick continues after the
    /// frames still queued (the server keeps one in reserve, so execution runs a tick behind).
    fn tick(zone: &mut Zone, world: &BoxWorld, inputs: &[(EntityId, Input)], view: Tick) {
        for (id, inp) in inputs {
            let t = zone
                .player(*id)
                .map_or(0, |p| p.last_input_tick + p.queued_frames() as u32)
                + 1;
            zone.queue_input(*id, t, *inp, view);
        }
        zone.step(world);
    }

    #[test]
    fn kit_validates_and_has_durations() {
        let kit = Kit::phase2(TickRate::COMBAT);
        assert_eq!(kit.abilities.len(), 3);
        assert_eq!(kit.durations[SWORD], 6 + 3 + 11);
        assert_eq!(kit.durations[DASH], 10);
        assert_eq!(kit.durations[CROSSBOW], 8);
    }

    #[test]
    fn sword_hits_in_front_and_not_behind() {
        for (attacker_yaw, expect_hit) in [(0.0f32, true), (180.0f32, false)] {
            let (world, mut zone, ids) = arena(&[
                (Vec3::new(0.0, 0.0, REST_Z), 0.0),
                (Vec3::new(48.0, 0.0, REST_Z), 180.0),
            ]);
            let (a, b) = (ids[0], ids[1]);
            tick(
                &mut zone,
                &world,
                &[
                    (a, input(attacker_yaw, 0.0, buttons::PRIMARY)),
                    (b, input(180.0, 0.0, 0)),
                ],
                0,
            );
            for _ in 0..12 {
                tick(
                    &mut zone,
                    &world,
                    &[
                        (a, input(attacker_yaw, 0.0, buttons::PRIMARY)),
                        (b, input(180.0, 0.0, 0)),
                    ],
                    0,
                );
            }
            let hp = zone.player(b).unwrap().health;
            if expect_hit {
                assert_eq!(hp, 65, "yaw {attacker_yaw}");
                assert!(zone.events.iter().any(|e| matches!(
                    e,
                    ZoneEvent::Hit { attacker, target, amount: 35, kind: HitKind::Melee } if *attacker == a && *target == b
                )));
            } else {
                assert_eq!(hp, 100, "yaw {attacker_yaw}");
            }
            // Holding the button does not re-trigger; one swing per press.
            assert_eq!(
                zone.events
                    .iter()
                    .filter(|e| matches!(e, ZoneEvent::Hit { .. }))
                    .count(),
                expect_hit as usize
            );
        }
    }

    #[test]
    fn crossbow_bolt_hits_the_first_body_in_line_even_an_ally() {
        let (world, mut zone, ids) = arena(&[
            (Vec3::new(0.0, 0.0, REST_Z), 0.0),
            (Vec3::new(120.0, 0.0, REST_Z), 0.0),
            (Vec3::new(240.0, 0.0, REST_Z), 0.0),
        ]);
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let idle = input(0.0, 0.0, 0);
        tick(
            &mut zone,
            &world,
            &[
                (a, input(0.0, 0.0, buttons::SECONDARY)),
                (b, idle),
                (c, idle),
            ],
            0,
        );
        for _ in 0..40 {
            tick(&mut zone, &world, &[(a, idle), (b, idle), (c, idle)], 0);
        }
        assert_eq!(
            zone.player(b).unwrap().health,
            60,
            "the body in between takes the bolt"
        );
        assert_eq!(zone.player(c).unwrap().health, 100);
        assert!(
            zone.events
                .iter()
                .any(|e| matches!(e, ZoneEvent::ProjectileSpawned { owner, .. } if *owner == a))
        );
        assert!(
            zone.events
                .iter()
                .any(|e| matches!(e, ZoneEvent::ProjectileRemoved(_)))
        );
        assert!(zone.projectiles().is_empty());
    }

    #[test]
    fn dash_moves_fast_and_costs_stamina() {
        let (world, mut zone, ids) = arena(&[(Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
        let a = ids[0];
        tick(
            &mut zone,
            &world,
            &[(a, input(0.0, 1.0, buttons::ABILITY1))],
            0,
        );
        // The press runs one tick late (reserve); ten dash frames follow.
        for _ in 0..10 {
            tick(&mut zone, &world, &[(a, input(0.0, 1.0, 0))], 0);
        }
        let p = zone.player(a).unwrap();
        // 900 u/s for 10 ticks = 140.6 u, minus the epsilon pull-backs.
        assert!(
            p.mover.mv.origin.x > 130.0 && p.mover.mv.origin.x < 142.0,
            "{:?}",
            p.mover.mv.origin
        );
        assert!(
            (p.mover.stamina - (70.0 + 10.0 * STAMINA_REGEN_PER_S / 64.0)).abs() < 0.5,
            "{}",
            p.mover.stamina
        );
        assert_eq!(p.anim, anim::DASH);
        tick(&mut zone, &world, &[(a, input(0.0, 1.0, 0))], 0);
        tick(&mut zone, &world, &[(a, input(0.0, 1.0, 0))], 0);
        assert!(zone.player(a).unwrap().mover.dash.is_none());
    }

    #[test]
    fn lag_compensation_rewinds_the_target() {
        // The target runs straight away from the attacker. Swinging at where the attacker saw it
        // (13 ticks ago) hits only if the server rewinds; without a rewind it is out of reach.
        for (rewind, expect_hit) in [(true, true), (false, false)] {
            let (world, mut zone, ids) = arena(&[
                (Vec3::new(0.0, 0.0, REST_Z), 0.0),
                (Vec3::new(40.0, 0.0, REST_Z), 0.0),
            ]);
            let (a, b) = (ids[0], ids[1]);
            for _ in 0..10 {
                tick(
                    &mut zone,
                    &world,
                    &[(a, input(0.0, 0.0, 0)), (b, input(0.0, 1.0, 0))],
                    0,
                );
            }
            let then = zone.tick;
            let old_x = zone.player(b).unwrap().mover.mv.origin.x;
            for _ in 0..MAX_REWIND_TICKS {
                tick(
                    &mut zone,
                    &world,
                    &[(a, input(0.0, 0.0, 0)), (b, input(0.0, 1.0, 0))],
                    0,
                );
            }
            let now_x = zone.player(b).unwrap().mover.mv.origin.x;
            assert!(old_x - 14.0 <= 72.0, "old position in reach: {old_x}");
            assert!(
                now_x - 14.0 > 72.0 + 20.0,
                "new position out of reach: {now_x}"
            );
            let view = if rewind { then } else { 0 };
            for _ in 0..12 {
                tick(
                    &mut zone,
                    &world,
                    &[
                        (a, input(0.0, 0.0, buttons::PRIMARY)),
                        (b, input(0.0, 1.0, 0)),
                    ],
                    view,
                );
            }
            let hit = zone.player(b).unwrap().health < 100;
            assert_eq!(hit, expect_hit, "rewind {rewind}: old {old_x} now {now_x}");
        }
    }

    #[test]
    fn frames_run_exactly_once_and_are_rate_limited() {
        let (world, mut zone, ids) = arena(&[(Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
        let a = ids[0];
        for t in 1..=20 {
            zone.queue_input(a, t, input(0.0, 1.0, 0), 0);
        }
        // Duplicate and stale frames are ignored.
        zone.queue_input(a, 20, input(0.0, 1.0, 0), 0);
        zone.queue_input(a, 3, input(0.0, 1.0, 0), 0);
        assert_eq!(zone.player(a).unwrap().queued_frames(), 20);
        let mut per_tick = Vec::new();
        for _ in 0..8 {
            let before = zone.player(a).unwrap().executed_frames;
            zone.step(&world);
            per_tick.push(zone.player(a).unwrap().executed_frames - before);
        }
        // Burst drain at two per tick until the 8 credits run out, then one per tick.
        assert_eq!(per_tick, [2, 2, 2, 2, 2, 2, 2, 1]);
        assert_eq!(zone.player(a).unwrap().last_input_tick, 15);
        for _ in 0..8 {
            zone.step(&world);
        }
        let p = zone.player(a).unwrap();
        // One frame stays in reserve; it runs when the next one arrives.
        assert_eq!(p.executed_frames, 19);
        assert_eq!(p.queued_frames(), 1);
        assert_eq!(p.starved_ticks, 4, "reserve held for the last ticks");
        zone.queue_input(a, 21, input(0.0, 1.0, 0), 0);
        zone.step(&world);
        let p = zone.player(a).unwrap();
        assert_eq!(p.executed_frames, 20);
        assert_eq!(p.last_input_tick, 20);
        zone.step(&world);
        assert_eq!(
            zone.player(a).unwrap().executed_frames,
            20,
            "nothing invented"
        );
    }

    #[test]
    fn reordered_datagrams_keep_every_frame() {
        let (world, mut zone, ids) = arena(&[(Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
        let a = ids[0];
        // The newer datagram (frames 4..=7) arrives before the older one (1..=4).
        for t in 4..=7 {
            zone.queue_input(a, t, input(0.0, 1.0, 0), 0);
        }
        for t in 1..=4 {
            zone.queue_input(a, t, input(0.0, 1.0, 0), 0);
        }
        assert_eq!(
            zone.player(a).unwrap().queued_frames(),
            7,
            "no frame lost, no duplicate"
        );
        for _ in 0..8 {
            zone.step(&world);
        }
        let p = zone.player(a).unwrap();
        assert_eq!(p.executed_frames, 6, "one in reserve");
        assert_eq!(p.last_input_tick, 6);
        // A frame older than the last executed one is ignored even if it arrives late.
        zone.queue_input(a, 3, input(0.0, 1.0, 0), 0);
        assert_eq!(zone.player(a).unwrap().queued_frames(), 1);
    }

    #[test]
    fn players_block_each_other() {
        let (world, mut zone, ids) = arena(&[
            (Vec3::new(-100.0, 0.0, REST_Z), 0.0),
            (Vec3::new(100.0, 0.0, REST_Z), 180.0),
        ]);
        let (a, b) = (ids[0], ids[1]);
        for _ in 0..128 {
            tick(
                &mut zone,
                &world,
                &[(a, input(0.0, 1.0, 0)), (b, input(180.0, 1.0, 0))],
                0,
            );
        }
        let ax = zone.player(a).unwrap().mover.mv.origin.x;
        let bx = zone.player(b).unwrap().mover.mv.origin.x;
        assert!(ax < bx, "players passed through each other: {ax} {bx}");
        assert!(bx - ax >= 32.0 - 0.1, "hulls overlap: {ax} {bx}");
        assert!(bx - ax < 34.0, "players did not meet: {ax} {bx}");
    }

    #[test]
    fn death_and_respawn() {
        let (world, mut zone, ids) = arena(&[
            (Vec3::new(0.0, 0.0, REST_Z), 0.0),
            (Vec3::new(48.0, 0.0, REST_Z), 180.0),
        ]);
        let (a, b) = (ids[0], ids[1]);
        let mut swings = 0;
        let mut killed_at = None;
        for t in 0..400 {
            let press = t % 24 == 0;
            if press {
                swings += 1;
            }
            let btn = if press { buttons::PRIMARY } else { 0 };
            // The attacker keeps walking into the target so knockback cannot carry it out of reach.
            tick(
                &mut zone,
                &world,
                &[(a, input(0.0, 1.0, btn)), (b, input(180.0, 0.0, 0))],
                0,
            );
            if killed_at.is_none()
                && zone.events.iter().any(|e| matches!(e, ZoneEvent::Killed { victim, killer } if *victim == b && *killer == a))
            {
                killed_at = Some(zone.tick);
                assert!(!zone.player(b).unwrap().alive);
                assert_eq!(zone.player(b).unwrap().anim, anim::DEAD);
                assert_eq!(zone.player(a).unwrap().kills, 1);
                break;
            }
        }
        let killed_at = killed_at.expect("three swings kill");
        assert!(swings >= 3);
        for _ in 0..zone.respawn_ticks {
            tick(
                &mut zone,
                &world,
                &[(a, input(0.0, 0.0, 0)), (b, input(180.0, 0.0, 0))],
                0,
            );
        }
        let p = zone.player(b).unwrap();
        assert!(
            p.alive,
            "respawned {} ticks after {killed_at}",
            zone.tick - killed_at
        );
        assert_eq!(p.health, MAX_HEALTH);
        assert!(
            zone.events
                .iter()
                .any(|e| matches!(e, ZoneEvent::Respawned(id) if *id == b))
        );
    }

    #[test]
    fn history_rewinds_to_the_nearest_recorded_tick() {
        let mut h = History::default();
        for t in 1..=40u32 {
            h.record(t, vec![(1, Vec3::new(t as f32, 0.0, 0.0))]);
        }
        assert_eq!(h.origin_at(10, 1), Some(Vec3::new(10.0, 0.0, 0.0)));
        // Older than the ring: the oldest kept sample (tick 9) is the nearest later one.
        assert_eq!(h.origin_at(2, 1), Some(Vec3::new(9.0, 0.0, 0.0)));
        assert_eq!(h.origin_at(41, 1), None);
        assert_eq!(h.origin_at(10, 2), None);
    }
}
