//! The entity vocabulary: the data mirror of `docs/VOCABULARY.md`.
//!
//! Every ability in every genre compiles down to six server verbs with parameters. This module
//! holds the parameter types and their validation ranges. Resolution (who gets hit, when) lives
//! in [`crate::sim`]; nothing here has behaviour beyond validation.
//!
//! Keep this file and the document in lockstep. When they disagree, the document wins and the
//! code is wrong. With the `bitcode` feature every type here is wire-encodable, so a zone can
//! send its loaded content to clients verbatim (MATRIX.md 10).

use crate::tick::{Tick, TickRate};

/// Zone-local entity identifier.
pub type EntityId = u32;

/// Damage types (MATRIX.md 4.1 and 5): three physical kinds and five elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[repr(u8)]
pub enum DamageType {
    Slash = 0,
    Pierce = 1,
    Blunt = 2,
    Fire = 3,
    Water = 4,
    Grass = 5,
    Electric = 6,
    Ground = 7,
    Air = 8,
}

impl DamageType {
    pub const ALL: [DamageType; 9] = [
        DamageType::Slash,
        DamageType::Pierce,
        DamageType::Blunt,
        DamageType::Fire,
        DamageType::Water,
        DamageType::Grass,
        DamageType::Electric,
        DamageType::Ground,
        DamageType::Air,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            DamageType::Slash => "slash",
            DamageType::Pierce => "pierce",
            DamageType::Blunt => "blunt",
            DamageType::Fire => "fire",
            DamageType::Water => "water",
            DamageType::Grass => "grass",
            DamageType::Electric => "electric",
            DamageType::Ground => "ground",
            DamageType::Air => "air",
        }
    }
}

/// The initial status set (VOCABULARY.md 5.4, semantics in MATRIX.md 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[repr(u8)]
pub enum Status {
    Slow = 0,
    Haste = 1,
    Root = 2,
    Bleed = 3,
    Burn = 4,
    Chill = 5,
    Shock = 6,
    Silence = 7,
    Stagger = 8,
    Fortify = 9,
    Weaken = 10,
    Expose = 11,
    Regen = 12,
    Stealth = 13,
    /// On the ground (MODES.md 4.5): no action, no guard, no movement; hits land in full.
    Knockdown = 14,
    /// Thrown into the air, then down: a knockdown with a lift at its start.
    Launched = 15,
    /// Taunted (MATRIX.md 8, 16): the body and its view are turned to the slot's `source`
    /// and held there while it lasts; a mind under it fights the source. A control.
    Taunt = 16,
}

impl Status {
    pub const ALL: [Status; 17] = [
        Status::Slow,
        Status::Haste,
        Status::Root,
        Status::Bleed,
        Status::Burn,
        Status::Chill,
        Status::Shock,
        Status::Silence,
        Status::Stagger,
        Status::Fortify,
        Status::Weaken,
        Status::Expose,
        Status::Regen,
        Status::Stealth,
        Status::Knockdown,
        Status::Launched,
        Status::Taunt,
    ];

    pub const fn from_index(i: u8) -> Option<Status> {
        if (i as usize) < Self::ALL.len() {
            Some(Self::ALL[i as usize])
        } else {
            None
        }
    }

    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn name(self) -> &'static str {
        match self {
            Status::Slow => "slow",
            Status::Haste => "haste",
            Status::Root => "root",
            Status::Bleed => "bleed",
            Status::Burn => "burn",
            Status::Chill => "chill",
            Status::Shock => "shock",
            Status::Silence => "silence",
            Status::Stagger => "stagger",
            Status::Fortify => "fortify",
            Status::Weaken => "weaken",
            Status::Expose => "expose",
            Status::Regen => "regen",
            Status::Stealth => "stealth",
            Status::Knockdown => "knockdown",
            Status::Launched => "launched",
            Status::Taunt => "taunt",
        }
    }

    /// Stagger and Shock durations are not scaled by the defender (MATRIX.md 8).
    pub const fn fixed_duration(self) -> bool {
        matches!(self, Status::Stagger | Status::Shock)
    }

    /// A control (MODES.md 4.5): the second within ten seconds lasts half, the third does
    /// nothing, and the body is then immune to this kind for ten seconds.
    pub const fn is_control(self) -> bool {
        matches!(
            self,
            Status::Knockdown | Status::Launched | Status::Root | Status::Taunt
        )
    }

    /// The body is on the ground: it cannot act, guard or move (MODES.md 4.5).
    pub const fn downs(self) -> bool {
        matches!(self, Status::Knockdown | Status::Launched)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct AbilityId(pub u16);

/// Archetype frames decide the hitbox capsule and the animation rig, never the mesh.
/// World collision uses [`crate::trace::Hull::Player`] for all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArchetypeFrame {
    Colossus,
    Striker,
    Caster,
    Infiltrator,
}

impl ArchetypeFrame {
    /// Hitbox capsule `(radius, height)` in world units, feet to top.
    pub const fn capsule(self) -> (f32, f32) {
        match self {
            ArchetypeFrame::Colossus => (18.0, 60.0),
            ArchetypeFrame::Striker => (14.0, 56.0),
            ArchetypeFrame::Caster => (13.0, 56.0),
            ArchetypeFrame::Infiltrator => (12.0, 52.0),
        }
    }
}

/// The game a character plays (MODES.md 2): chosen with the build, never switched in play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Mode {
    /// First person, a firearm in hand (MODES.md 3).
    Gun,
    /// Over the shoulder, free aim, chains and the dodge (MODES.md 4).
    #[default]
    Action,
    /// The orbit camera, a target, actions that walk into range (MODES.md 5).
    Rpg,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Gun, Mode::Action, Mode::Rpg];

    pub const fn name(self) -> &'static str {
        match self {
            Mode::Gun => "gun",
            Mode::Action => "action",
            Mode::Rpg => "rpg",
        }
    }

    pub fn from_name(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }

    /// Seen from behind the body (the action and RPG modes) or from its eyes (the gun).
    pub const fn third_person(self) -> bool {
        !matches!(self, Mode::Gun)
    }
}

/// Windup, active and recovery windows of a verb, in ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Timing {
    pub windup: Tick,
    pub active: Tick,
    pub recovery: Tick,
}

impl Timing {
    pub const fn total(self) -> Tick {
        self.windup + self.active + self.recovery
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Cost {
    pub stamina: u16,
    pub focus: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Cooldown {
    pub ticks: Tick,
    /// Abilities sharing a group share the cooldown.
    pub group: Option<u8>,
}

/// Defence layers a damage packet ignores (the RPS "true bypasses" of PLAN.md 3.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Bypass(pub u8);

impl Bypass {
    pub const NONE: Bypass = Bypass(0);
    pub const ARMOR: Bypass = Bypass(1);
    pub const MAGIC_SHIELD: Bypass = Bypass(2);
    pub const EVASION: Bypass = Bypass(4);

    pub const fn contains(self, other: Bypass) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Bypass) -> Bypass {
        Bypass(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct DamagePacket {
    pub amount: u16,
    pub dtype: DamageType,
    pub bypass: Bypass,
    /// Impulse applied along the hit direction, u/s.
    pub knockback: f32,
    /// Stagger build-up. Statuses decide the threshold.
    pub stagger: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Shape {
    Sphere { radius: f32 },
    Cylinder { radius: f32, height: f32 },
    Cone { length: f32, half_angle_deg: f32 },
    Box { half_extents: [f32; 3] },
}

/// Where a verb is anchored.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Origin {
    SelfFeet,
    SelfEyes,
    /// Offset in the actor's view frame (forward, right, up).
    Weapon {
        offset: [f32; 3],
    },
    Point([f32; 3]),
    /// Where the actor aims: the first body or world surface along the view ray within
    /// `range`, dropped to the ground (under a body: its feet). Nothing homes; an area placed
    /// here stays where it was put (VOCABULARY.md 4).
    Aim {
        range: f32,
    },
    /// The impact point of the projectile that triggered this effect.
    Impact,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Falloff {
    #[default]
    None,
    Linear,
    InverseSquare,
}

/// Verb 1: a swing that hits every capsule in a wedge. Resolved with melee lag compensation.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct MeleeArc {
    pub reach: f32,
    pub arc_deg: f32,
    pub half_height: f32,
    pub timing: Timing,
    pub damage: DamagePacket,
    pub max_targets: u8,
    /// Damage multiplier per additional target (1.0 = full cleave).
    pub cleave_falloff: f32,
    pub parryable: bool,
    /// Ticks the attacker's animation freezes on hit.
    pub hit_stop: Tick,
    /// The magnet (MODES.md 4.2): at the swing's start the body turns up to this many
    /// degrees toward the nearest enemy within `reach × 1.5` that it can see. 0: none.
    pub assist_deg: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Bounce {
    pub count: u8,
    pub restitution: f32,
}

/// Verb 2: everything ranged. Bolts, arrows, fireballs, thrown axes. Never hitscan, never homing.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Projectile {
    pub speed: f32,
    pub gravity_scale: f32,
    pub radius: f32,
    pub lifetime: Tick,
    pub damage: DamagePacket,
    /// Entities passed through before stopping.
    pub pierce: u8,
    pub bounce: Bounce,
    /// Velocity fraction lost per second.
    pub drag: f32,
    pub spawn: Origin,
    /// Fraction of the shooter's velocity added at spawn.
    pub inherit_velocity: f32,
    pub spread_deg: f32,
    /// Projectiles per activation.
    pub count: u8,
    pub on_hit: Vec<Trigger>,
    pub on_expire: Vec<Trigger>,
}

/// What a projectile hit or expiry triggers (VOCABULARY.md 5.2): a status on the entity hit
/// (`target: Hit`) or on the shooter (`Actor`), or an area at the impact point.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Trigger {
    Status(ApplyStatus),
    Area(AreaEffect),
}

/// Verb 3: a volume that pulses.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct AreaEffect {
    pub shape: Shape,
    pub origin: Origin,
    pub delay: Tick,
    /// 0 = a single instant pulse.
    pub duration: Tick,
    pub interval: Tick,
    pub damage: Option<DamagePacket>,
    /// Statuses put on every entity a pulse touches (`target: Area`) or on the caster
    /// (`Actor`).
    pub effects: Vec<ApplyStatus>,
    pub falloff: Falloff,
    pub max_targets: u8,
    pub requires_los: bool,
    /// The caster is never a target (shockwaves from the caster's own feet). Default false:
    /// a fireball at your feet burns you.
    pub exclude_actor: bool,
}

impl AreaEffect {
    /// It hurts who stands in it: it deals damage, or puts a status on them that is not a
    /// boon. What a circle on the floor shows (PROTOCOL.md 5) and what a mind steps out of.
    pub fn harmful(&self) -> bool {
        self.damage.is_some_and(|d| d.amount > 0)
            || self.effects.iter().any(|s| {
                s.target != StatusTarget::Actor
                    && !matches!(
                        s.status,
                        Status::Regen | Status::Haste | Status::Fortify | Status::Stealth
                    )
            })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum StackRule {
    #[default]
    Refresh,
    Extend,
    Independent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum StatusTarget {
    #[default]
    Actor,
    Hit,
    Area,
}

/// Verb 4: put a status on someone. `target` `Hit` means every entity the previous step of
/// the same ability hit (or the entity a projectile hit when nested in `on_hit`); `Area`
/// means every entity an enclosing `AreaEffect` pulse touched; `Actor` is the caster
/// (VOCABULARY.md 5.4).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct ApplyStatus {
    pub status: Status,
    pub duration: Tick,
    pub magnitude: f32,
    pub max_stacks: u8,
    pub stacking: StackRule,
    pub target: StatusTarget,
    pub dispellable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum MoveKind {
    Dash {
        speed: f32,
        duration: Tick,
    },
    Leap {
        forward: f32,
        up: f32,
    },
    Charge {
        speed: f32,
        duration: Tick,
        stop_on_hit: bool,
    },
    /// Line-of-sight and hull checked; never through walls.
    Blink {
        distance: f32,
    },
}

/// Verb 5: the actor moves itself. Predicted on the client.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct MoveSelf {
    pub kind: MoveKind,
    pub cancelable: bool,
    pub keep_friction: bool,
    /// Invulnerability ticks. Default 0: dodging is positional, not a free pass.
    pub iframes: Tick,
    /// May cut another ability's recovery short (MODES.md 4.4): pressed while a script is
    /// past its active window, it ends that script and starts.
    pub cancel_recovery: bool,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Block {
    pub arc_deg: f32,
    /// 0..1 damage removed.
    pub mitigation: f32,
    pub stamina_per_hit: u16,
    pub stops_projectiles: bool,
    pub move_speed_scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Parry {
    pub arc_deg: f32,
    pub window: Tick,
    pub whiff_recovery: Tick,
    pub on_success: Vec<Riposte>,
}

/// What a successful parry does to the attacker (VOCABULARY.md 5.6): a status, or a swing
/// from the defender.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Riposte {
    Status(ApplyStatus),
    Swing(MeleeArc),
}

/// Verb 6: block or parry.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Guard {
    Block(Block),
    Parry(Parry),
}

/// The six verbs. Effects triggered by another verb (a projectile hit, an area pulse, a parry)
/// are typed lists of the verbs allowed there ([`Trigger`], [`ApplyStatus`], [`Riposte`]), so
/// the vocabulary is composable, non-recursive and wire-encodable.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Verb {
    MeleeArc(MeleeArc),
    Projectile(Projectile),
    AreaEffect(AreaEffect),
    ApplyStatus(ApplyStatus),
    MoveSelf(MoveSelf),
    Guard(Guard),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum Interrupt {
    #[default]
    Never,
    OnDamage,
    OnStagger,
}

/// A verb scheduled at a tick offset from activation.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Step {
    pub at: Tick,
    pub verb: Verb,
}

/// The next stage of a chain (MODES.md 4.3): while this ability's recovery runs and for
/// `window` ticks after it ends, the same slot plays `next` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Chain {
    /// The pack's index of the next stage.
    pub next: u16,
    pub window: Tick,
}

/// How a firearm fires (MODES.md 3.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub enum FireMode {
    /// Held: a shot every cycle.
    Auto,
    /// A click a shot.
    #[default]
    Semi,
    /// A click a shot, the action worked for the cycle; no shot faster than a walk.
    Bolt,
}

/// The cone a firearm's bolt is rolled in, in degrees (MODES.md 3.4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Cone {
    pub stand: f32,
    pub crouch: f32,
    /// The stance's base while the scope is up (MODES.md 3.4): content gives it, or a
    /// quarter of `stand` (`sim::SCOPED_CONE`).
    pub scoped: f32,
    /// The move's share past a walk (MODES.md 3.4): nothing up to half the body's speed,
    /// all of it from four fifths.
    pub moving: f32,
    pub air: f32,
    /// The spray's share: this much on the fourth shot within `recover` of the last,
    /// growing with the square of the count to four times it by the eighth.
    pub shot: f32,
    /// The spray forgets itself this long after a shot.
    pub recover: Tick,
}

/// A firearm (MODES.md 3.2): the magazine, the reload, the cycle, the recoil pattern and
/// the cone of the primary that carries it. Its steps launch bolts as any ranged ability's.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Firearm {
    pub magazine: u8,
    /// The stack the reserve is drawn from (MODES.md 11.2): the key of a `stack` template
    /// of items.toml. The rounds carried are that stack's quantity, read by the zone from
    /// the inventory; empty: no stack, the magazine is issued full once and that is all.
    pub ammo: String,
    pub reload: Tick,
    /// Ticks between two shots; the ability's cooldown is this.
    pub cycle: Tick,
    pub fire: FireMode,
    /// The multiplier for a bolt that enters the head band (MODES.md 3.5).
    pub headshot: f32,
    /// 0: no scope; else the zoom (2 or 4).
    pub scope: u8,
    /// The kick after the n-th shot of a spray, (yaw, pitch) in degrees, cumulative; past
    /// the end the last repeats; the index resets after `cone.recover` without a shot.
    pub recoil: Vec<(f32, f32)>,
    pub cone: Cone,
}

impl Firearm {
    /// The pattern's pair after the n-th shot (0-based) of a spray: the view's punch.
    pub fn kick(&self, index: u8) -> (f32, f32) {
        match self.recoil.last() {
            None => (0.0, 0.0),
            Some(last) => *self.recoil.get(index as usize).unwrap_or(last),
        }
    }

    /// Where the pattern has turned the gun before the n-th shot (MODES.md 3.3): nothing
    /// for the first shot of a spray, which flies where the crosshair is; the pair after
    /// the shot before for the rest.
    pub fn turn(&self, index: u8) -> (f32, f32) {
        match index {
            0 => (0.0, 0.0),
            n => self.kick(n - 1),
        }
    }
}

/// An ability is a timed script of verbs. There is no ability-specific server code.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Ability {
    pub id: AbilityId,
    pub name: String,
    pub cost: Cost,
    pub cooldown: Cooldown,
    pub steps: Vec<Step>,
    /// Movement speed multiplier while the script runs.
    pub move_scale: f32,
    pub interrupt: Interrupt,
    /// The next stage (MODES.md 4.3), if the ability is the start or a middle of a chain.
    pub chain: Option<Chain>,
    /// A firearm's numbers (MODES.md 3.2), on a gun.
    pub firearm: Option<Firearm>,
    /// How far a target-action reaches (MODES.md 5.3): a melee arc's reach, a bolt's
    /// range as content gives it, an aimed area's; 0 when nothing of this ability aims.
    pub range: f32,
}

/// Validation limits (VOCABULARY.md section 11). Content outside these ranges fails to load.
pub mod limits {
    pub const MAX_MELEE_REACH: f32 = 160.0;
    /// A bullet (MODES.md 3.6) is a bolt at 20,000 u/s: swept per tick as every bolt is.
    pub const MAX_PROJECTILE_SPEED: f32 = 20000.0;
    pub const MAX_PROJECTILE_LIFETIME_S: f32 = 20.0;
    pub const MAX_AREA_RADIUS: f32 = 512.0;
    pub const MAX_DASH_SPEED: f32 = 1600.0;
    pub const MAX_BLINK_DISTANCE: f32 = 384.0;
    pub const MAX_SCRIPT_S: f32 = 10.0;
    pub const MAX_AIM_RANGE: f32 = 1024.0;
    /// A target-action's reach (MODES.md 5.3).
    pub const MAX_RANGE: f32 = 4096.0;
    pub const MAX_ASSIST_DEG: f32 = 90.0;
    pub const MAX_CHAIN_WINDOW_S: f32 = 2.0;
    pub const MAX_MAGAZINE: u8 = 100;
    /// A content key named by another (a firearm's `ammo`): the hub's item templates
    /// are at most this long too.
    pub const MAX_KEY_LEN: usize = 48;
    pub const MAX_RELOAD_S: f32 = 6.0;
    pub const MIN_CYCLE_S: f32 = 0.05;
    pub const MAX_CYCLE_S: f32 = 3.0;
    pub const MAX_HEADSHOT: f32 = 5.0;
    pub const MAX_RECOIL_PAIRS: usize = 32;
    pub const MAX_KICK_DEG: f32 = 6.0;
    pub const MAX_CONE_DEG: f32 = 15.0;
    pub const MIN_RECOVER_S: f32 = 0.1;
    pub const MAX_RECOVER_S: f32 = 2.0;
}

#[derive(Debug, PartialEq, Eq)]
pub struct VocabError {
    pub ability: AbilityId,
    pub reason: &'static str,
}

impl core::fmt::Display for VocabError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ability {}: {}", self.ability.0, self.reason)
    }
}

impl core::error::Error for VocabError {}

impl Ability {
    /// Check every parameter against `limits` for the given zone tick rate.
    pub fn validate(&self, rate: TickRate) -> Result<(), VocabError> {
        let err = |reason| VocabError {
            ability: self.id,
            reason,
        };
        let max_ticks = |seconds: f32| (seconds * rate.hz() as f32) as Tick;
        if self.steps.is_empty() {
            return Err(err("ability has no steps"));
        }
        if self
            .steps
            .iter()
            .any(|s| s.at > max_ticks(limits::MAX_SCRIPT_S))
        {
            return Err(err("step scheduled past the script limit"));
        }
        if !(0.0..=1.0).contains(&self.move_scale) {
            return Err(err("move_scale outside 0..=1"));
        }
        if !(0.0..=limits::MAX_RANGE).contains(&self.range) {
            return Err(err("range outside 0..=4096"));
        }
        if let Some(c) = self.chain
            && c.window > max_ticks(limits::MAX_CHAIN_WINDOW_S)
        {
            return Err(err("chain window past 2 s"));
        }
        if let Some(f) = &self.firearm {
            if !(1..=limits::MAX_MAGAZINE).contains(&f.magazine) {
                return Err(err("magazine outside 1..=100"));
            }
            if f.ammo.len() > limits::MAX_KEY_LEN {
                return Err(err("ammo key past 48 characters"));
            }
            if f.reload == 0 || f.reload > max_ticks(limits::MAX_RELOAD_S) {
                return Err(err("reload outside a tick..=6 s"));
            }
            if f.cycle < max_ticks(limits::MIN_CYCLE_S).max(1)
                || f.cycle > max_ticks(limits::MAX_CYCLE_S)
            {
                return Err(err("cycle outside 50 ms..=3 s"));
            }
            if !(1.0..=limits::MAX_HEADSHOT).contains(&f.headshot) {
                return Err(err("headshot outside 1..=5"));
            }
            if !matches!(f.scope, 0 | 2 | 4) {
                return Err(err("scope is 0, 2 or 4"));
            }
            if f.recoil.len() > limits::MAX_RECOIL_PAIRS
                || f.recoil
                    .iter()
                    .any(|(y, p)| y.abs() > limits::MAX_KICK_DEG || p.abs() > limits::MAX_KICK_DEG)
            {
                return Err(err("recoil: at most 32 pairs within 6 degrees"));
            }
            let c = &f.cone;
            if [c.stand, c.crouch, c.scoped, c.moving, c.air, c.shot]
                .iter()
                .any(|d| !(0.0..=limits::MAX_CONE_DEG).contains(d))
            {
                return Err(err("cone outside 0..=15 degrees"));
            }
            if c.recover < max_ticks(limits::MIN_RECOVER_S).max(1)
                || c.recover > max_ticks(limits::MAX_RECOVER_S)
            {
                return Err(err("cone recover outside 100 ms..=2 s"));
            }
            if !self
                .steps
                .iter()
                .any(|s| matches!(s.verb, Verb::Projectile(_)))
            {
                return Err(err("a firearm's steps launch a bolt"));
            }
        }
        for step in &self.steps {
            validate_verb(&step.verb, rate, &err)?;
        }
        Ok(())
    }
}

fn validate_verb(
    verb: &Verb,
    rate: TickRate,
    err: &dyn Fn(&'static str) -> VocabError,
) -> Result<(), VocabError> {
    let max_ticks = |seconds: f32| (seconds * rate.hz() as f32) as Tick;
    match verb {
        Verb::MeleeArc(m) => {
            if !(0.0..=limits::MAX_MELEE_REACH).contains(&m.reach) {
                return Err(err("melee reach out of range"));
            }
            if !(0.0..=limits::MAX_ASSIST_DEG).contains(&m.assist_deg) {
                return Err(err("assist outside 0..=90 degrees"));
            }
            if !(0.0..=360.0).contains(&m.arc_deg) {
                return Err(err("melee arc out of range"));
            }
            if m.timing.active == 0 {
                return Err(err("melee arc needs an active window"));
            }
            if m.max_targets == 0 {
                return Err(err("melee arc needs max_targets >= 1"));
            }
        }
        Verb::Projectile(p) => {
            if !(1.0..=limits::MAX_PROJECTILE_SPEED).contains(&p.speed) {
                return Err(err("projectile speed out of range"));
            }
            if p.lifetime == 0 || p.lifetime > max_ticks(limits::MAX_PROJECTILE_LIFETIME_S) {
                return Err(err("projectile lifetime out of range"));
            }
            if p.count == 0 {
                return Err(err("projectile count must be >= 1"));
            }
            if p.radius < 0.0 {
                return Err(err("projectile radius negative"));
            }
            if matches!(p.spawn, Origin::Aim { .. } | Origin::Impact) {
                return Err(err(
                    "a projectile spawns at the actor, not at an aim or impact",
                ));
            }
            for t in p.on_hit.iter().chain(&p.on_expire) {
                match t {
                    Trigger::Status(st) => validate_verb(&Verb::ApplyStatus(*st), rate, err)?,
                    Trigger::Area(a) => validate_verb(&Verb::AreaEffect(a.clone()), rate, err)?,
                }
            }
        }
        Verb::AreaEffect(a) => {
            let radius = match a.shape {
                Shape::Sphere { radius } | Shape::Cylinder { radius, .. } => radius,
                Shape::Cone { length, .. } => length,
                Shape::Box { half_extents } => half_extents.iter().cloned().fold(0.0, f32::max),
            };
            if !(0.0..=limits::MAX_AREA_RADIUS).contains(&radius) {
                return Err(err("area radius out of range"));
            }
            if a.duration > 0 && a.interval == 0 {
                return Err(err("persistent area needs an interval"));
            }
            if let Origin::Aim { range } = a.origin
                && !(1.0..=limits::MAX_AIM_RANGE).contains(&range)
            {
                return Err(err("aim range out of range"));
            }
            if a.damage.is_none() && a.effects.is_empty() {
                return Err(err("area effect does nothing"));
            }
            if a.delay > max_ticks(limits::MAX_SCRIPT_S)
                || a.duration > max_ticks(limits::MAX_SCRIPT_S)
            {
                return Err(err("area effect delay or duration past the script limit"));
            }
            for st in &a.effects {
                validate_verb(&Verb::ApplyStatus(*st), rate, err)?;
            }
        }
        Verb::ApplyStatus(s) => {
            if s.duration == 0 {
                return Err(err("status duration must be >= 1 tick"));
            }
            if s.max_stacks == 0 {
                return Err(err("status max_stacks must be >= 1"));
            }
        }
        Verb::MoveSelf(m) => match m.kind {
            MoveKind::Dash { speed, duration }
            | MoveKind::Charge {
                speed, duration, ..
            } => {
                if !(0.0..=limits::MAX_DASH_SPEED).contains(&speed) || duration == 0 {
                    return Err(err("dash/charge speed or duration out of range"));
                }
            }
            MoveKind::Leap { forward, up } => {
                if forward.abs() > limits::MAX_DASH_SPEED || up.abs() > limits::MAX_DASH_SPEED {
                    return Err(err("leap impulse out of range"));
                }
            }
            MoveKind::Blink { distance } => {
                if !(0.0..=limits::MAX_BLINK_DISTANCE).contains(&distance) {
                    return Err(err("blink distance out of range"));
                }
            }
        },
        Verb::Guard(Guard::Block(b)) => {
            if !(0.0..=1.0).contains(&b.mitigation) || !(0.0..=360.0).contains(&b.arc_deg) {
                return Err(err("block mitigation or arc out of range"));
            }
        }
        Verb::Guard(Guard::Parry(p)) => {
            if p.window == 0 || !(0.0..=360.0).contains(&p.arc_deg) {
                return Err(err("parry window or arc out of range"));
            }
            for r in &p.on_success {
                match r {
                    Riposte::Status(st) => validate_verb(&Verb::ApplyStatus(*st), rate, err)?,
                    Riposte::Swing(arc) => validate_verb(&Verb::MeleeArc(arc.clone()), rate, err)?,
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bolt() -> Verb {
        Verb::Projectile(Projectile {
            speed: 2400.0,
            gravity_scale: 0.4,
            radius: 1.0,
            lifetime: TickRate::COMBAT.ms_to_ticks(3000),
            damage: DamagePacket {
                amount: 45,
                dtype: DamageType::Pierce,
                bypass: Bypass::NONE,
                knockback: 80.0,
                stagger: 10,
            },
            pierce: 0,
            bounce: Bounce::default(),
            drag: 0.0,
            spawn: Origin::Weapon {
                offset: [16.0, 4.0, -2.0],
            },
            inherit_velocity: 0.0,
            spread_deg: 0.25,
            count: 1,
            on_hit: vec![],
            on_expire: vec![],
        })
    }

    #[test]
    fn crossbow_shot_validates() {
        let rate = TickRate::COMBAT;
        let ability = Ability {
            id: AbilityId(1),
            name: "Crossbow shot".into(),
            cost: Cost {
                stamina: 0,
                focus: 0,
            },
            cooldown: Cooldown {
                ticks: rate.ms_to_ticks(1800),
                group: None,
            },
            steps: vec![Step {
                at: rate.ms_to_ticks(120),
                verb: bolt(),
            }],
            move_scale: 0.6,
            interrupt: Interrupt::OnStagger,
            chain: None,
            firearm: None,
            range: 0.0,
        };
        assert_eq!(ability.validate(rate), Ok(()));
    }

    #[test]
    fn hitscan_speeds_are_rejected() {
        let rate = TickRate::COMBAT;
        let Verb::Projectile(mut p) = bolt() else {
            unreachable!()
        };
        p.speed = 1.0e6;
        let ability = Ability {
            id: AbilityId(2),
            name: "Laser".into(),
            cost: Cost::default(),
            cooldown: Cooldown::default(),
            steps: vec![Step {
                at: 0,
                verb: Verb::Projectile(p),
            }],
            move_scale: 1.0,
            interrupt: Interrupt::Never,
            chain: None,
            firearm: None,
            range: 0.0,
        };
        assert_eq!(
            ability.validate(rate),
            Err(VocabError {
                ability: AbilityId(2),
                reason: "projectile speed out of range"
            })
        );
    }

    #[test]
    fn nested_effects_are_validated() {
        let rate = TickRate::COMBAT;
        let bad_status = Trigger::Status(ApplyStatus {
            status: Status::Bleed,
            duration: 0,
            magnitude: 0.3,
            max_stacks: 1,
            stacking: StackRule::Refresh,
            target: StatusTarget::Hit,
            dispellable: true,
        });
        let Verb::Projectile(mut p) = bolt() else {
            unreachable!()
        };
        p.on_hit.push(bad_status);
        let ability = Ability {
            id: AbilityId(3),
            name: "Poison bolt".into(),
            cost: Cost::default(),
            cooldown: Cooldown::default(),
            steps: vec![Step {
                at: 0,
                verb: Verb::Projectile(p),
            }],
            move_scale: 1.0,
            interrupt: Interrupt::Never,
            chain: None,
            firearm: None,
            range: 0.0,
        };
        assert!(ability.validate(rate).is_err());
    }

    #[test]
    fn bypass_flags_compose() {
        let b = Bypass::ARMOR.union(Bypass::EVASION);
        assert!(b.contains(Bypass::ARMOR));
        assert!(!b.contains(Bypass::MAGIC_SHIELD));
    }
}
