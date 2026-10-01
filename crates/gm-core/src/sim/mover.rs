//! The predicted part of a player: abilities, guard, statuses and movement for one tick.
//! Runs identically on the client (prediction) and the server (authority).

use glam::Vec3;

use crate::build::Sheet;
use crate::collide::Aabb;
use crate::geom::Capsule;
use crate::matrix::EVADING_GRACE_TICKS;
use crate::movement::{MoveInput, MoveVars, PlayerState, player_move, yaw_vectors};
use crate::sim::{MAX_ABILITIES, REGEN_PAUSE_MS, tick_delta};
use crate::status::Statuses;
use crate::tick::{Tick, TickRate};
use crate::trace::{CollisionWorld, Hull};
use crate::vocab::{ArchetypeFrame, Guard, MeleeArc, MoveKind, StatusTarget, Verb};

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
    pub const GUARD: u8 = 8;
    pub const PARRY: u8 = 9;
    pub const CAST: u8 = 10;
    pub const STAGGER: u8 = 11;
}

/// Marker for "no cast animation" lookups.
pub const CAST_ANIM_NONE: u8 = 0xff;

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

/// A running ability script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Script {
    pub ability: u8,
    pub started: Tick,
    pub next_step: u8,
    pub ends: Tick,
}

/// An active `MoveSelf::Dash` or `Charge`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dash {
    pub dir: Vec3,
    pub speed: f32,
    pub until: Tick,
    /// Charge: ends when the way is blocked.
    pub stop_on_hit: bool,
}

/// Guard state (VOCABULARY.md 5.6). Times are frame ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuardState {
    #[default]
    None,
    /// Block held.
    Block,
    /// Parry window open until the tick.
    Parry { until: Tick },
    /// Missed parry: recovering until the tick, no actions.
    Whiff { until: Tick },
}

/// Everything the client predicts for its own entity. Plain `Copy` data so a prediction ring
/// can store one per tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mover {
    pub mv: PlayerState,
    pub yaw: f32,
    pub pitch: f32,
    pub stamina: f32,
    pub focus: f32,
    pub buttons_prev: u16,
    pub script: Option<Script>,
    /// Tick at which each slot is ready again.
    pub cooldowns: [Tick; MAX_ABILITIES],
    pub dash: Option<Dash>,
    pub guard: GuardState,
    pub statuses: Statuses,
    /// Evading (MATRIX.md 6) until this frame tick.
    pub evade_until: Tick,
    /// Invulnerable until this frame tick (`MoveSelf.iframes`).
    pub iframes_until: Tick,
    /// Stamina regeneration resumes at this frame tick.
    pub regen_pause_until: Tick,
}

impl Mover {
    pub fn new(origin: Vec3, yaw: f32) -> Mover {
        Mover {
            mv: PlayerState::new(origin),
            yaw,
            pitch: 0.0,
            stamina: 100.0,
            focus: 100.0,
            buttons_prev: 0,
            script: None,
            cooldowns: [0; MAX_ABILITIES],
            dash: None,
            guard: GuardState::None,
            statuses: Statuses::default(),
            evade_until: 0,
            iframes_until: 0,
            regen_pause_until: 0,
        }
    }

    /// A fresh mover with the sheet's pools full.
    pub fn spawn(origin: Vec3, yaw: f32, sheet: &Sheet) -> Mover {
        let mut m = Mover::new(origin, yaw);
        m.stamina = sheet.derived.stamina;
        m.focus = sheet.derived.focus;
        m
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

    pub fn ground_speed(&self) -> f32 {
        self.mv.ground_speed()
    }

    pub fn blocking(&self) -> bool {
        self.guard == GuardState::Block
    }

    /// Evading at frame tick `now` (MATRIX.md 6).
    pub fn evading(&self, now: Tick) -> bool {
        self.dash.is_some() || tick_delta(now, self.evade_until) < 0
    }

    pub fn invulnerable(&self, now: Tick) -> bool {
        tick_delta(now, self.iframes_until) < 0
    }

    /// Forget every running thing (death, respawn); cooldowns survive.
    pub fn reset_actions(&mut self) {
        self.script = None;
        self.dash = None;
        self.guard = GuardState::None;
        self.statuses.clear();
        self.evade_until = 0;
        self.iframes_until = 0;
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

/// What a mover asked the authoritative side to do this tick. The client ignores these (or
/// spawns cosmetic previews); the server resolves them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Swing {
        ability: u8,
        step: u8,
        active_from: Tick,
        active_until: Tick,
    },
    Fire {
        ability: u8,
        step: u8,
    },
    Area {
        ability: u8,
        step: u8,
    },
    /// A parry window opened (the server resolves hits against it).
    ParryOpened,
}

const DASH_VARS: MoveVars = MoveVars {
    friction: 0.0,
    edge_friction: 1.0,
    accelerate: 0.0,
    air_accelerate: 0.0,
    ..MoveVars::QUAKE
};

/// Advance one mover by one tick: statuses, guard, ability activation, script steps, movement,
/// regeneration. `now` is the **client tick of the frame** on both sides (the server passes the
/// frame's tick, never its own), so cooldowns, scripts, statuses and dashes elapse identically
/// even when the server runs two frames in one tick.
pub fn step_mover<W: CollisionWorld + ?Sized>(
    world: &W,
    sheet: &Sheet,
    m: &mut Mover,
    input: &Input,
    now: Tick,
    dt: f32,
    actions: &mut Vec<Action>,
) {
    let kit = &sheet.kit;
    let d = &sheet.derived;
    let pressed = input.buttons & !m.buttons_prev;
    m.buttons_prev = input.buttons;
    m.yaw = input.yaw;
    m.pitch = input.pitch;
    m.statuses.expire(now);
    let staggered = m.statuses.staggered();
    if staggered {
        // Stagger interrupts everything (MATRIX.md 8); the server already cleared the script
        // when it applied the status, the client follows at reconciliation.
        m.script = None;
        m.guard = GuardState::None;
    }

    guard_step(sheet, m, input, pressed, now, staggered, actions);

    let slot = if input.ability > 0 && (input.ability as usize) <= kit.actives.len() {
        kit.actives[input.ability as usize - 1]
    } else if pressed & buttons::PRIMARY != 0 {
        kit.primary
    } else if pressed & buttons::SECONDARY != 0 {
        kit.secondary
    } else if pressed & buttons::ABILITY1 != 0 {
        kit.actives[0]
    } else if pressed & buttons::ABILITY2 != 0 {
        kit.actives[1]
    } else if pressed & buttons::ABILITY3 != 0 {
        kit.actives[2]
    } else if pressed & buttons::ABILITY4 != 0 {
        kit.actives[3]
    } else {
        None
    };
    if let Some(slot) = slot
        && !staggered
    {
        try_activate(sheet, m, slot as usize, now);
    }

    if let Some(mut s) = m.script {
        let ab = &kit.abilities[s.ability as usize];
        let elapsed = tick_delta(now, s.started).max(0) as Tick;
        while (s.next_step as usize) < ab.steps.len()
            && ab.steps[s.next_step as usize].at <= elapsed
        {
            resolve_step(
                world,
                sheet,
                m,
                input,
                &ab.steps[s.next_step as usize].verb,
                s.ability,
                s.next_step,
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

    // Movement scale: the script, the guard, then statuses on top of the sheet's speed.
    let mut scale = m
        .script
        .map_or(1.0, |s| kit.abilities[s.ability as usize].move_scale);
    match (m.guard, kit.guard_verb()) {
        (GuardState::Block, Some(Guard::Block(b))) => scale *= b.move_speed_scale,
        (GuardState::Parry { .. } | GuardState::Whiff { .. }, _) => scale *= 0.5,
        _ => {}
    }
    let vars = MoveVars {
        max_speed: d.max_speed * m.statuses.speed_scale(),
        ..MoveVars::QUAKE
    };
    let jump = input.buttons & buttons::JUMP != 0 && !m.statuses.staggered();
    match m.dash {
        Some(dsh)
            if tick_delta(dsh.until, now) > 0 && !m.statuses.has(crate::vocab::Status::Root) =>
        {
            m.mv.velocity.x = dsh.dir.x * dsh.speed;
            m.mv.velocity.y = dsh.dir.y * dsh.speed;
            let mi = MoveInput {
                yaw: m.yaw,
                forward: 0.0,
                side: 0.0,
                jump: false,
            };
            player_move(&world, &DASH_VARS, &mut m.mv, &mi, dt);
            if dsh.stop_on_hit && m.mv.velocity.truncate().length() < dsh.speed * 0.5 {
                m.dash = None;
            }
        }
        _ => {
            m.dash = None;
            let mi = MoveInput {
                yaw: m.yaw,
                forward: input.forward.clamp(-1.0, 1.0) * scale,
                side: input.side.clamp(-1.0, 1.0) * scale,
                jump,
            };
            player_move(&world, &vars, &mut m.mv, &mi, dt);
        }
    }

    if tick_delta(now, m.regen_pause_until) >= 0 {
        m.stamina = (m.stamina + d.stamina_regen * dt).min(d.stamina);
    }
    m.focus = (m.focus + d.focus_regen * dt).min(d.focus);
}

/// The block/parry state machine (VOCABULARY.md 5.6).
#[allow(clippy::too_many_arguments)]
fn guard_step(
    sheet: &Sheet,
    m: &mut Mover,
    input: &Input,
    pressed: u16,
    now: Tick,
    staggered: bool,
    actions: &mut Vec<Action>,
) {
    let kit = &sheet.kit;
    let Some(gi) = kit.guard else {
        m.guard = GuardState::None;
        return;
    };
    match kit.guard_verb() {
        Some(Guard::Block(_)) => {
            let want = input.buttons & buttons::GUARD != 0
                && !staggered
                && m.script.is_none()
                && m.stamina > 0.0;
            m.guard = if want {
                GuardState::Block
            } else {
                GuardState::None
            };
        }
        Some(Guard::Parry(p)) => match m.guard {
            GuardState::Parry { until } if tick_delta(now, until) >= 0 => {
                m.guard = GuardState::Whiff {
                    until: now.wrapping_add(p.whiff_recovery),
                };
            }
            GuardState::Whiff { until } if tick_delta(now, until) >= 0 => {
                m.guard = GuardState::None;
            }
            GuardState::None | GuardState::Block => {
                let ab = &kit.abilities[gi as usize];
                let ready = tick_delta(now, m.cooldowns[gi as usize]) >= 0;
                if pressed & buttons::GUARD != 0
                    && !staggered
                    && m.script.is_none()
                    && ready
                    && m.stamina >= ab.cost.stamina as f32
                {
                    m.stamina -= ab.cost.stamina as f32;
                    if ab.cost.stamina > 0 {
                        m.regen_pause_until = now.wrapping_add(regen_pause_ticks());
                    }
                    let ready_at = now.wrapping_add(ab.cooldown.ticks.max(1));
                    m.cooldowns[gi as usize] = ready_at;
                    if let Some(g) = ab.cooldown.group {
                        for (i, other) in kit.abilities.iter().enumerate() {
                            if other.cooldown.group == Some(g) {
                                m.cooldowns[i] = ready_at;
                            }
                        }
                    }
                    m.guard = GuardState::Parry {
                        until: now.wrapping_add(p.window),
                    };
                    actions.push(Action::ParryOpened);
                } else if m.guard == GuardState::Block {
                    m.guard = GuardState::None;
                }
            }
            _ => {}
        },
        None => m.guard = GuardState::None,
    }
}

fn regen_pause_ticks() -> Tick {
    TickRate::COMBAT.ms_to_ticks(REGEN_PAUSE_MS)
}

fn try_activate(sheet: &Sheet, m: &mut Mover, slot: usize, now: Tick) -> bool {
    let kit = &sheet.kit;
    if slot >= kit.abilities.len().min(MAX_ABILITIES) || m.script.is_some() {
        return false;
    }
    if matches!(m.guard, GuardState::Parry { .. } | GuardState::Whiff { .. }) {
        return false;
    }
    if tick_delta(now, m.cooldowns[slot]) < 0 {
        return false;
    }
    if kit.elemental[slot] && m.statuses.silenced() {
        return false;
    }
    let ab = &kit.abilities[slot];
    if m.stamina < ab.cost.stamina as f32 || m.focus < ab.cost.focus as f32 {
        return false;
    }
    m.stamina -= ab.cost.stamina as f32;
    m.focus -= ab.cost.focus as f32;
    if ab.cost.stamina > 0 {
        m.regen_pause_until = now.wrapping_add(regen_pause_ticks());
    }
    // Attacking drops a held block.
    if m.guard == GuardState::Block {
        m.guard = GuardState::None;
    }
    let ready_at = now.wrapping_add(ab.cooldown.ticks.max(1));
    m.cooldowns[slot] = ready_at;
    if let Some(g) = ab.cooldown.group {
        for (i, other) in kit.abilities.iter().enumerate() {
            if other.cooldown.group == Some(g) {
                m.cooldowns[i] = ready_at;
            }
        }
    }
    m.script = Some(Script {
        ability: slot as u8,
        started: now,
        next_step: 0,
        ends: now.wrapping_add(kit.durations[slot]),
    });
    true
}

#[allow(clippy::too_many_arguments)]
fn resolve_step<W: CollisionWorld + ?Sized>(
    world: &W,
    sheet: &Sheet,
    m: &mut Mover,
    input: &Input,
    verb: &Verb,
    ability: u8,
    step: u8,
    now: Tick,
    actions: &mut Vec<Action>,
) {
    match verb {
        Verb::MeleeArc(arc) => actions.push(Action::Swing {
            ability,
            step,
            active_from: now.wrapping_add(arc.timing.windup),
            active_until: now.wrapping_add(arc.timing.windup + arc.timing.active),
        }),
        Verb::Projectile(_) => actions.push(Action::Fire { ability, step }),
        Verb::AreaEffect(_) => actions.push(Action::Area { ability, step }),
        Verb::ApplyStatus(s) => {
            // Self-targeted statuses are predicted; hit/area targets are the server's.
            if s.target == StatusTarget::Actor {
                m.statuses.apply(s, s.duration, now, 0);
            }
        }
        Verb::MoveSelf(ms) => {
            let (fwd, right) = yaw_vectors(m.yaw);
            let wish = fwd * input.forward + right * input.side;
            let dir = if wish.length_squared() > 1e-4 {
                wish.normalize()
            } else {
                fwd
            };
            let duration = match ms.kind {
                MoveKind::Dash { speed, duration } => {
                    m.dash = Some(Dash {
                        dir,
                        speed,
                        until: now.wrapping_add(duration),
                        stop_on_hit: false,
                    });
                    duration
                }
                MoveKind::Charge {
                    speed,
                    duration,
                    stop_on_hit,
                } => {
                    m.dash = Some(Dash {
                        dir: fwd,
                        speed,
                        until: now.wrapping_add(duration),
                        stop_on_hit,
                    });
                    duration
                }
                MoveKind::Leap { forward, up } => {
                    m.mv.velocity += dir * forward + Vec3::new(0.0, 0.0, up);
                    m.mv.on_ground = false;
                    TickRate::COMBAT.ms_to_ticks(250)
                }
                MoveKind::Blink { distance } => {
                    // Hull-traced along the facing; never through geometry (VOCABULARY.md 5.5).
                    let start = m.mv.origin;
                    let end = start + fwd * distance;
                    let tr = world.trace(m.mv.hull, start, end);
                    if !tr.start_solid && !tr.all_solid {
                        m.mv.origin = crate::movement::nudge_position(world, m.mv.hull, tr.end);
                    }
                    m.mv.velocity = Vec3::ZERO;
                    0
                }
            };
            m.evade_until = now.wrapping_add(duration).wrapping_add(EVADING_GRACE_TICKS);
            if ms.iframes > 0 {
                m.iframes_until = now.wrapping_add(ms.iframes);
            }
            let _ = sheet;
        }
        // Guard verbs live in the guard slot, never in a script.
        Verb::Guard(_) => {}
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
