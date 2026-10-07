//! Content authoring format (MATRIX.md 10): abilities and preset builds written in TOML with
//! milliseconds and names, compiled against a tick rate into a validated
//! [`gm_core::build::ContentPack`]. Only zones and tools load content; clients receive the
//! compiled pack over the wire.
#![forbid(unsafe_code)]

pub mod items;
pub mod looks;
pub mod tuning;

use std::path::Path;

use gm_core::build::{AbilityDef, Build, ContentPack, CreatureDef, Loot, NamedBuild, Slot};
use gm_core::matrix::{ArmourClass, Aspects, Attributes, Element};
use gm_core::tick::{Tick, TickRate};
use gm_core::trial::{Lens, TrialDef};
use gm_core::vocab::{
    Ability, AbilityId, ApplyStatus, ArchetypeFrame, AreaEffect, Block, Bounce, Bypass, Chain,
    Cone, Cooldown, Cost, DamagePacket, DamageType, Falloff, FireMode, Firearm, Guard, Interrupt,
    MeleeArc, Mode, MoveKind, MoveSelf, Origin, Parry, Projectile, Riposte, Shape, StackRule,
    Status, StatusTarget, Step, Timing, Trigger, Verb,
};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Toml {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Validation(#[from] gm_core::build::ContentError),
}

/// The longest a preset's blurb may be, in characters (CLIENT.md 4.3).
pub const MAX_BLURB_CHARS: usize = 160;

/// What each preset build is, in a line or two of plain words, in the order of the pack's
/// builds: shown to a person choosing an archetype (CLIENT.md 4.3). Not part of the pack a
/// zone sends: the hub hands the blurbs out where a character is made.
pub fn load_blurbs(dir: &Path) -> Result<Vec<String>, ContentError> {
    let path = dir.join("builds.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| ContentError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let file: BuildsFile = toml::from_str(&text).map_err(|source| ContentError::Toml {
        path: path.display().to_string(),
        source,
    })?;
    file.build
        .iter()
        .map(|b| {
            // Short, and nothing the client's font cannot draw.
            let blurb = b.blurb.trim();
            if blurb.is_empty()
                || blurb.chars().count() > MAX_BLURB_CHARS
                || !blurb.chars().all(|c| c == ' ' || c.is_ascii_graphic())
            {
                return Err(ContentError::Invalid(format!(
                    "build {:?}: its blurb must be 1 to {MAX_BLURB_CHARS} characters of ASCII",
                    b.name
                )));
            }
            Ok(blurb.to_string())
        })
        .collect()
}

// ---------- authoring types (TOML) ----------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AbilitiesFile {
    #[serde(default)]
    pub(crate) ability: Vec<AbilityToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuildsFile {
    #[serde(default)]
    pub(crate) build: Vec<BuildToml>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreaturesFile {
    #[serde(default)]
    pub(crate) creature: Vec<CreatureToml>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrialsFile {
    #[serde(default)]
    trial: Vec<TrialToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AbilityToml {
    pub(crate) key: String,
    name: String,
    slot: String,
    cost: u8,
    aspect: Option<String>,
    /// The look fields (CONTENT.md 3): a picture, the prop held while this is the primary
    /// and nothing is worn, the patch its cues use. Read by `looks`, not compiled.
    #[serde(default)]
    pub(crate) icon: Option<String>,
    #[serde(default)]
    pub(crate) prop: Option<String>,
    #[serde(default)]
    pub(crate) sound: Option<String>,
    /// Squad slots the ability adds while slotted (COMPANIONS.md 3.2).
    #[serde(default)]
    squad: u8,
    /// Only creatures may slot it (COMPANIONS.md 8.1).
    #[serde(default)]
    creature: bool,
    #[serde(default)]
    cooldown_ms: u32,
    cooldown_group: Option<u8>,
    #[serde(default)]
    stamina: u16,
    #[serde(default)]
    focus: u16,
    #[serde(default = "one")]
    move_scale: f32,
    #[serde(default = "never")]
    interrupt: String,
    /// The next stage of a chain (MODES.md 4.3).
    chain: Option<ChainToml>,
    /// A firearm's numbers (MODES.md 3.2).
    firearm: Option<FirearmToml>,
    /// How far a target-action reaches (MODES.md 5.3); without it, a melee arc's reach,
    /// an aimed area's range, or 600 for a bolt.
    range: Option<f32>,
    #[serde(default)]
    step: Vec<StepToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChainToml {
    next: String,
    window_ms: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FirearmToml {
    magazine: u8,
    reserve: u16,
    reload_ms: u32,
    cycle_ms: u32,
    #[serde(default = "semi")]
    fire: String,
    #[serde(default = "one")]
    headshot: f32,
    #[serde(default)]
    scope: u8,
    #[serde(default)]
    recoil: Vec<[f32; 2]>,
    cone: FireConeToml,
}

fn semi() -> String {
    "semi".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FireConeToml {
    stand: f32,
    crouch: f32,
    #[serde(rename = "move")]
    moving: f32,
    air: f32,
    shot: f32,
    recover_ms: u32,
}

/// The bolt's reach when content names none (MODES.md 5.3).
const DEFAULT_BOLT_RANGE: f32 = 600.0;

fn one() -> f32 {
    1.0
}

fn never() -> String {
    "never".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepToml {
    #[serde(default)]
    at_ms: u32,
    melee_arc: Option<MeleeToml>,
    projectile: Option<ProjectileToml>,
    area_effect: Option<AreaToml>,
    apply_status: Option<StatusToml>,
    move_self: Option<MoveToml>,
    guard: Option<GuardToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DamageToml {
    amount: u16,
    #[serde(rename = "type")]
    dtype: String,
    #[serde(default)]
    bypass: Vec<String>,
    #[serde(default)]
    knockback: f32,
    #[serde(default)]
    stagger: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeleeToml {
    reach: f32,
    arc: f32,
    #[serde(default = "forty")]
    half_height: f32,
    windup_ms: u32,
    active_ms: u32,
    recovery_ms: u32,
    damage: DamageToml,
    #[serde(default = "one_u8")]
    max_targets: u8,
    #[serde(default = "one")]
    cleave_falloff: f32,
    #[serde(default = "yes")]
    parryable: bool,
    #[serde(default)]
    hit_stop_ms: u32,
    /// The magnet's turn in degrees (MODES.md 4.2).
    #[serde(default)]
    assist: f32,
}

fn forty() -> f32 {
    40.0
}

fn one_u8() -> u8 {
    1
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BounceToml {
    count: u8,
    restitution: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectileToml {
    speed: f32,
    #[serde(default)]
    gravity: f32,
    radius: f32,
    lifetime_ms: u32,
    damage: DamageToml,
    #[serde(default)]
    pierce: u8,
    bounce: Option<BounceToml>,
    #[serde(default)]
    drag: f32,
    spawn: OriginToml,
    #[serde(default)]
    inherit_velocity: f32,
    #[serde(default)]
    spread: f32,
    #[serde(default = "one_u8")]
    count: u8,
    #[serde(default)]
    on_hit: Vec<TriggerToml>,
    #[serde(default)]
    on_expire: Vec<TriggerToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerToml {
    status: Option<StatusToml>,
    area: Option<AreaToml>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum OriginToml {
    Named(String),
    Weapon {
        weapon: [f32; 3],
    },
    Point {
        point: [f32; 3],
    },
    /// Where the actor aims, within this range (VOCABULARY.md 4).
    Aim {
        aim: f32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapeToml {
    sphere: Option<SphereToml>,
    cylinder: Option<CylinderToml>,
    cone: Option<ConeToml>,
    #[serde(rename = "box")]
    box_: Option<BoxToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SphereToml {
    radius: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CylinderToml {
    radius: f32,
    height: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConeToml {
    length: f32,
    half_angle: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoxToml {
    half_extents: [f32; 3],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AreaToml {
    shape: ShapeToml,
    origin: OriginToml,
    #[serde(default)]
    delay_ms: u32,
    #[serde(default)]
    duration_ms: u32,
    #[serde(default)]
    interval_ms: u32,
    damage: Option<DamageToml>,
    #[serde(default)]
    effects: Vec<StatusToml>,
    #[serde(default = "none_s")]
    falloff: String,
    #[serde(default)]
    max_targets: u8,
    #[serde(default)]
    requires_los: bool,
    #[serde(default)]
    exclude_actor: bool,
}

fn none_s() -> String {
    "none".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusToml {
    status: String,
    duration_ms: u32,
    #[serde(default)]
    magnitude: f32,
    #[serde(default = "one_u8")]
    max_stacks: u8,
    #[serde(default = "refresh")]
    stacking: String,
    target: String,
    #[serde(default = "yes")]
    dispellable: bool,
}

fn refresh() -> String {
    "refresh".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveToml {
    dash: Option<DashToml>,
    leap: Option<LeapToml>,
    charge: Option<ChargeToml>,
    blink: Option<BlinkToml>,
    #[serde(default)]
    cancelable: bool,
    #[serde(default)]
    keep_friction: bool,
    #[serde(default)]
    iframes_ms: u32,
    /// `"recovery"`: may cut another ability's recovery short (MODES.md 4.4).
    cancel: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DashToml {
    speed: f32,
    duration_ms: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeapToml {
    forward: f32,
    up: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChargeToml {
    speed: f32,
    duration_ms: u32,
    #[serde(default = "yes")]
    stop_on_hit: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlinkToml {
    distance: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GuardToml {
    block: Option<BlockToml>,
    parry: Option<ParryToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockToml {
    arc: f32,
    mitigation: f32,
    stamina_per_hit: u16,
    #[serde(default)]
    stops_projectiles: bool,
    #[serde(default = "one")]
    move_speed_scale: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParryToml {
    arc: f32,
    window_ms: u32,
    whiff_recovery_ms: u32,
    #[serde(default)]
    on_success: Vec<RiposteToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RiposteToml {
    status: Option<StatusToml>,
    swing: Option<MeleeToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuildToml {
    pub(crate) name: String,
    blurb: String,
    /// The archetype's picture (CONTENT.md 3).
    #[serde(default)]
    pub(crate) icon: Option<String>,
    /// The game the archetype plays (MODES.md 2): `gun`, `action` or `rpg`.
    #[serde(default = "action")]
    pub(crate) mode: String,
    frame: String,
    attributes: AttributesToml,
    armour: String,
    aspects: Vec<String>,
    primary: String,
    secondary: String,
    guard: Option<String>,
    #[serde(default)]
    actives: Vec<String>,
}

fn action() -> String {
    "action".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttributesToml {
    str: u8,
    agi: u8,
    con: u8,
    int: u8,
    spr: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatureToml {
    pub(crate) key: String,
    name: String,
    /// The look fields (CONTENT.md 3): its model on the standard rig, its portrait.
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) icon: Option<String>,
    frame: String,
    armour: String,
    aspects: Vec<String>,
    attributes: AttributesToml,
    health: u16,
    #[serde(default)]
    stagger_threshold: u16,
    primary: String,
    secondary: String,
    guard: Option<String>,
    #[serde(default)]
    actives: Vec<String>,
    sight: f32,
    leash: f32,
    #[serde(default)]
    boss: bool,
    #[serde(default)]
    respawn_s: u16,
    loot: Option<LootToml>,
    #[serde(default)]
    still: bool,
    #[serde(default)]
    npc: bool,
    #[serde(default = "one")]
    might: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LootToml {
    components: u8,
    standard: Vec<String>,
    top: Vec<String>,
    #[serde(default)]
    coin: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrialToml {
    key: String,
    name: String,
    map: String,
    encounter: String,
    #[serde(default)]
    time_limit_s: u32,
    max_party_deaths: Option<u8>,
    #[serde(default)]
    max_humans: u8,
    #[serde(default)]
    role: RoleToml,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleToml {
    #[serde(default)]
    damage: u16,
    #[serde(default)]
    tank: u16,
    #[serde(default)]
    healing: u16,
    #[serde(default)]
    command: u16,
}

// ---------- compilation ----------

struct Ctx {
    rate: TickRate,
    key: String,
}

impl Ctx {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, ContentError> {
        Err(ContentError::Invalid(format!(
            "{}: {}",
            self.key,
            msg.into()
        )))
    }

    fn ticks(&self, ms: u32) -> Tick {
        self.rate.ms_to_ticks(ms)
    }

    fn damage_type(&self, s: &str) -> Result<DamageType, ContentError> {
        DamageType::ALL
            .into_iter()
            .find(|t| t.name() == s)
            .map_or_else(|| self.err(format!("unknown damage type {s:?}")), Ok)
    }

    fn element(&self, s: &str) -> Result<Element, ContentError> {
        Element::ALL
            .into_iter()
            .find(|e| e.name() == s)
            .map_or_else(|| self.err(format!("unknown element {s:?}")), Ok)
    }

    fn status_name(&self, s: &str) -> Result<Status, ContentError> {
        Status::ALL
            .into_iter()
            .find(|st| st.name() == s)
            .map_or_else(|| self.err(format!("unknown status {s:?}")), Ok)
    }

    fn damage(&self, d: &DamageToml) -> Result<DamagePacket, ContentError> {
        let mut bypass = Bypass::NONE;
        for b in &d.bypass {
            bypass = bypass.union(match b.as_str() {
                "armor" | "armour" => Bypass::ARMOR,
                "magic_shield" => Bypass::MAGIC_SHIELD,
                "evasion" => Bypass::EVASION,
                other => return self.err(format!("unknown bypass {other:?}")),
            });
        }
        Ok(DamagePacket {
            amount: d.amount,
            dtype: self.damage_type(&d.dtype)?,
            bypass,
            knockback: d.knockback,
            stagger: d.stagger,
        })
    }

    fn melee(&self, m: &MeleeToml) -> Result<MeleeArc, ContentError> {
        Ok(MeleeArc {
            reach: m.reach,
            arc_deg: m.arc,
            half_height: m.half_height,
            timing: Timing {
                windup: self.ticks(m.windup_ms),
                active: self.ticks(m.active_ms),
                recovery: self.ticks(m.recovery_ms),
            },
            damage: self.damage(&m.damage)?,
            max_targets: m.max_targets,
            cleave_falloff: m.cleave_falloff,
            parryable: m.parryable,
            hit_stop: self.ticks(m.hit_stop_ms),
            assist_deg: m.assist,
        })
    }

    fn origin(&self, o: &OriginToml) -> Result<Origin, ContentError> {
        Ok(match o {
            OriginToml::Named(s) => match s.as_str() {
                "self_feet" => Origin::SelfFeet,
                "self_eyes" => Origin::SelfEyes,
                "impact" => Origin::Impact,
                other => return self.err(format!("unknown origin {other:?}")),
            },
            OriginToml::Weapon { weapon } => Origin::Weapon { offset: *weapon },
            OriginToml::Point { point } => Origin::Point(*point),
            OriginToml::Aim { aim } => Origin::Aim { range: *aim },
        })
    }

    fn shape(&self, s: &ShapeToml) -> Result<Shape, ContentError> {
        let n = s.sphere.is_some() as u8
            + s.cylinder.is_some() as u8
            + s.cone.is_some() as u8
            + s.box_.is_some() as u8;
        if n != 1 {
            return self.err("a shape is exactly one of sphere, cylinder, cone, box");
        }
        Ok(if let Some(x) = &s.sphere {
            Shape::Sphere { radius: x.radius }
        } else if let Some(x) = &s.cylinder {
            Shape::Cylinder {
                radius: x.radius,
                height: x.height,
            }
        } else if let Some(x) = &s.cone {
            Shape::Cone {
                length: x.length,
                half_angle_deg: x.half_angle,
            }
        } else {
            let b = s.box_.as_ref().expect("counted");
            Shape::Box {
                half_extents: b.half_extents,
            }
        })
    }

    fn status(&self, s: &StatusToml) -> Result<ApplyStatus, ContentError> {
        Ok(ApplyStatus {
            status: self.status_name(&s.status)?,
            duration: self.ticks(s.duration_ms),
            magnitude: s.magnitude,
            max_stacks: s.max_stacks,
            stacking: match s.stacking.as_str() {
                "refresh" => StackRule::Refresh,
                "extend" => StackRule::Extend,
                "independent" => StackRule::Independent,
                other => return self.err(format!("unknown stacking {other:?}")),
            },
            target: match s.target.as_str() {
                "actor" => StatusTarget::Actor,
                "hit" => StatusTarget::Hit,
                "area" => StatusTarget::Area,
                other => return self.err(format!("unknown status target {other:?}")),
            },
            dispellable: s.dispellable,
        })
    }

    fn area(&self, a: &AreaToml) -> Result<AreaEffect, ContentError> {
        Ok(AreaEffect {
            shape: self.shape(&a.shape)?,
            origin: self.origin(&a.origin)?,
            delay: self.ticks(a.delay_ms),
            duration: self.ticks(a.duration_ms),
            interval: self.ticks(a.interval_ms),
            damage: a.damage.as_ref().map(|d| self.damage(d)).transpose()?,
            effects: a
                .effects
                .iter()
                .map(|s| self.status(s))
                .collect::<Result<_, _>>()?,
            falloff: match a.falloff.as_str() {
                "none" => Falloff::None,
                "linear" => Falloff::Linear,
                "inverse_square" => Falloff::InverseSquare,
                other => return self.err(format!("unknown falloff {other:?}")),
            },
            max_targets: a.max_targets,
            requires_los: a.requires_los,
            exclude_actor: a.exclude_actor,
        })
    }

    fn trigger(&self, t: &TriggerToml) -> Result<Trigger, ContentError> {
        match (&t.status, &t.area) {
            (Some(s), None) => Ok(Trigger::Status(self.status(s)?)),
            (None, Some(a)) => Ok(Trigger::Area(self.area(a)?)),
            _ => self.err("a trigger is exactly one of status, area"),
        }
    }

    fn projectile(&self, p: &ProjectileToml) -> Result<Projectile, ContentError> {
        Ok(Projectile {
            speed: p.speed,
            gravity_scale: p.gravity,
            radius: p.radius,
            lifetime: self.ticks(p.lifetime_ms),
            damage: self.damage(&p.damage)?,
            pierce: p.pierce,
            bounce: p.bounce.as_ref().map_or(Bounce::default(), |b| Bounce {
                count: b.count,
                restitution: b.restitution,
            }),
            drag: p.drag,
            spawn: self.origin(&p.spawn)?,
            inherit_velocity: p.inherit_velocity,
            spread_deg: p.spread,
            count: p.count,
            on_hit: p
                .on_hit
                .iter()
                .map(|t| self.trigger(t))
                .collect::<Result<_, _>>()?,
            on_expire: p
                .on_expire
                .iter()
                .map(|t| self.trigger(t))
                .collect::<Result<_, _>>()?,
        })
    }

    fn move_self(&self, m: &MoveToml) -> Result<MoveSelf, ContentError> {
        let n = m.dash.is_some() as u8
            + m.leap.is_some() as u8
            + m.charge.is_some() as u8
            + m.blink.is_some() as u8;
        if n != 1 {
            return self.err("move_self is exactly one of dash, leap, charge, blink");
        }
        let kind = if let Some(d) = &m.dash {
            MoveKind::Dash {
                speed: d.speed,
                duration: self.ticks(d.duration_ms),
            }
        } else if let Some(l) = &m.leap {
            MoveKind::Leap {
                forward: l.forward,
                up: l.up,
            }
        } else if let Some(c) = &m.charge {
            MoveKind::Charge {
                speed: c.speed,
                duration: self.ticks(c.duration_ms),
                stop_on_hit: c.stop_on_hit,
            }
        } else {
            MoveKind::Blink {
                distance: m.blink.as_ref().expect("counted").distance,
            }
        };
        let cancel_recovery = match m.cancel.as_deref() {
            None => false,
            Some("recovery") => true,
            Some(other) => return self.err(format!("unknown cancel {other:?}")),
        };
        Ok(MoveSelf {
            kind,
            cancelable: m.cancelable,
            keep_friction: m.keep_friction,
            iframes: self.ticks(m.iframes_ms),
            cancel_recovery,
        })
    }

    fn guard(&self, g: &GuardToml) -> Result<Guard, ContentError> {
        match (&g.block, &g.parry) {
            (Some(b), None) => Ok(Guard::Block(Block {
                arc_deg: b.arc,
                mitigation: b.mitigation,
                stamina_per_hit: b.stamina_per_hit,
                stops_projectiles: b.stops_projectiles,
                move_speed_scale: b.move_speed_scale,
            })),
            (None, Some(p)) => Ok(Guard::Parry(Parry {
                arc_deg: p.arc,
                window: self.ticks(p.window_ms),
                whiff_recovery: self.ticks(p.whiff_recovery_ms),
                on_success: p
                    .on_success
                    .iter()
                    .map(|r| match (&r.status, &r.swing) {
                        (Some(s), None) => Ok(Riposte::Status(self.status(s)?)),
                        (None, Some(m)) => Ok(Riposte::Swing(self.melee(m)?)),
                        _ => self.err("a riposte is exactly one of status, swing"),
                    })
                    .collect::<Result<_, _>>()?,
            })),
            _ => self.err("guard is exactly one of block, parry"),
        }
    }

    fn step(&self, s: &StepToml) -> Result<Step, ContentError> {
        let n = s.melee_arc.is_some() as u8
            + s.projectile.is_some() as u8
            + s.area_effect.is_some() as u8
            + s.apply_status.is_some() as u8
            + s.move_self.is_some() as u8
            + s.guard.is_some() as u8;
        if n != 1 {
            return self.err("a step is exactly one verb");
        }
        let verb = if let Some(m) = &s.melee_arc {
            Verb::MeleeArc(self.melee(m)?)
        } else if let Some(p) = &s.projectile {
            Verb::Projectile(self.projectile(p)?)
        } else if let Some(a) = &s.area_effect {
            Verb::AreaEffect(self.area(a)?)
        } else if let Some(st) = &s.apply_status {
            Verb::ApplyStatus(self.status(st)?)
        } else if let Some(m) = &s.move_self {
            Verb::MoveSelf(self.move_self(m)?)
        } else {
            Verb::Guard(self.guard(s.guard.as_ref().expect("counted"))?)
        };
        Ok(Step {
            at: self.ticks(s.at_ms),
            verb,
        })
    }
}

fn compile_ability(
    a: &AbilityToml,
    index: usize,
    keys: &[String],
    rate: TickRate,
) -> Result<AbilityDef, ContentError> {
    let ctx = Ctx {
        rate,
        key: a.key.clone(),
    };
    let slot = match a.slot.as_str() {
        "primary" => Slot::Primary,
        "secondary" => Slot::Secondary,
        "guard" => Slot::Guard,
        "active" => Slot::Active,
        "extra" => Slot::Extra,
        other => return ctx.err(format!("unknown slot {other:?}")),
    };
    let chain = a
        .chain
        .as_ref()
        .map(|c| {
            let next = keys
                .iter()
                .position(|k| *k == c.next)
                .ok_or_else(|| {
                    ContentError::Invalid(format!("{}: chain names unknown ability {:?}", a.key, c.next))
                })?;
            Ok::<_, ContentError>(Chain {
                next: next as u16,
                window: ctx.ticks(c.window_ms),
            })
        })
        .transpose()?;
    let firearm = a
        .firearm
        .as_ref()
        .map(|f| {
            let fire = match f.fire.as_str() {
                "auto" => FireMode::Auto,
                "semi" => FireMode::Semi,
                "bolt" => FireMode::Bolt,
                other => return ctx.err(format!("unknown fire {other:?}")),
            };
            Ok(Firearm {
                magazine: f.magazine,
                reserve: f.reserve,
                reload: ctx.ticks(f.reload_ms),
                cycle: ctx.ticks(f.cycle_ms),
                fire,
                headshot: f.headshot,
                scope: f.scope,
                recoil: f.recoil.iter().map(|p| (p[0], p[1])).collect(),
                cone: Cone {
                    stand: f.cone.stand,
                    crouch: f.cone.crouch,
                    moving: f.cone.moving,
                    air: f.cone.air,
                    shot: f.cone.shot,
                    recover: ctx.ticks(f.cone.recover_ms),
                },
            })
        })
        .transpose()?;
    let interrupt = match a.interrupt.as_str() {
        "never" => Interrupt::Never,
        "on_damage" => Interrupt::OnDamage,
        "on_stagger" => Interrupt::OnStagger,
        other => return ctx.err(format!("unknown interrupt {other:?}")),
    };
    let aspect = a.aspect.as_deref().map(|s| ctx.element(s)).transpose()?;
    let steps = a
        .step
        .iter()
        .map(|s| ctx.step(s))
        .collect::<Result<Vec<_>, _>>()?;
    // The reach of a target-action (MODES.md 5.3), when content leaves it to the verbs.
    let range = a.range.unwrap_or_else(|| {
        steps
            .iter()
            .find_map(|s| match &s.verb {
                Verb::MeleeArc(m) => Some(m.reach),
                Verb::Projectile(_) => Some(DEFAULT_BOLT_RANGE),
                Verb::AreaEffect(ae) => match ae.origin {
                    Origin::Aim { range } => Some(range),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or(0.0)
    });
    // A firearm's cycle is its cooldown (MODES.md 3.2).
    let cooldown_ticks = match &firearm {
        Some(f) => f.cycle,
        None => ctx.ticks(a.cooldown_ms),
    };
    Ok(AbilityDef {
        key: a.key.clone(),
        ability: Ability {
            id: AbilityId(index as u16 + 1),
            name: a.name.clone(),
            cost: Cost {
                stamina: a.stamina,
                focus: a.focus,
            },
            cooldown: Cooldown {
                ticks: cooldown_ticks,
                group: a.cooldown_group,
            },
            steps,
            move_scale: a.move_scale,
            interrupt,
            chain,
            firearm,
            range,
        },
        slot,
        cost: a.cost,
        aspect,
        squad: a.squad,
        creature: a.creature,
    })
}

/// What a build and a creature share: the frame, the attributes, the armour class, the
/// aspects and the slotted abilities by key.
struct BodyToml<'a> {
    what: &'a str,
    name: &'a str,
    /// The mode (MODES.md 2); a creature plays none.
    mode: Option<&'a str>,
    frame: &'a str,
    armour: &'a str,
    aspects: &'a [String],
    attributes: &'a AttributesToml,
    primary: &'a str,
    secondary: &'a str,
    guard: Option<&'a str>,
    actives: &'a [String],
}

fn compile_body(b: &BodyToml<'_>, pack: &ContentPack) -> Result<Build, ContentError> {
    let err = |msg: String| ContentError::Invalid(format!("{} {}: {msg}", b.what, b.name));
    let find = |key: &str| {
        pack.find(key)
            .ok_or_else(|| err(format!("unknown ability {key:?}")))
    };
    let frame = match b.frame {
        "colossus" => ArchetypeFrame::Colossus,
        "striker" => ArchetypeFrame::Striker,
        "caster" => ArchetypeFrame::Caster,
        "infiltrator" => ArchetypeFrame::Infiltrator,
        other => return Err(err(format!("unknown frame {other:?}"))),
    };
    let armour = ArmourClass::ALL
        .into_iter()
        .find(|c| c.name() == b.armour)
        .ok_or_else(|| err(format!("unknown armour class {:?}", b.armour)))?;
    let mut aspects = Aspects::NONE;
    for a in b.aspects {
        let e = Element::ALL
            .into_iter()
            .find(|e| e.name() == a.as_str())
            .ok_or_else(|| err(format!("unknown aspect {a:?}")))?;
        aspects.0 |= Aspects::one(e).0;
    }
    let mode = match b.mode {
        None => Mode::Action,
        Some(m) => Mode::from_name(m).ok_or_else(|| err(format!("unknown mode {m:?}")))?,
    };
    Ok(Build {
        mode,
        frame,
        attributes: Attributes::new(
            b.attributes.str,
            b.attributes.agi,
            b.attributes.con,
            b.attributes.int,
            b.attributes.spr,
        ),
        armour,
        aspects,
        primary: find(b.primary)?,
        secondary: find(b.secondary)?,
        guard: b.guard.map(find).transpose()?,
        actives: b
            .actives
            .iter()
            .map(|k| find(k))
            .collect::<Result<_, _>>()?,
    })
}

fn compile_build(b: &BuildToml, pack: &ContentPack) -> Result<NamedBuild, ContentError> {
    Ok(NamedBuild {
        name: b.name.clone(),
        build: compile_body(
            &BodyToml {
                what: "build",
                name: &b.name,
                mode: Some(&b.mode),
                frame: &b.frame,
                armour: &b.armour,
                aspects: &b.aspects,
                attributes: &b.attributes,
                primary: &b.primary,
                secondary: &b.secondary,
                guard: b.guard.as_deref(),
                actives: &b.actives,
            },
            pack,
        )?,
    })
}

fn compile_creature(c: &CreatureToml, pack: &ContentPack) -> Result<CreatureDef, ContentError> {
    Ok(CreatureDef {
        key: c.key.clone(),
        name: c.name.clone(),
        build: compile_body(
            &BodyToml {
                what: "creature",
                name: &c.key,
                mode: None,
                frame: &c.frame,
                armour: &c.armour,
                aspects: &c.aspects,
                attributes: &c.attributes,
                primary: &c.primary,
                secondary: &c.secondary,
                guard: c.guard.as_deref(),
                actives: &c.actives,
            },
            pack,
        )?,
        health: c.health,
        stagger_threshold: c.stagger_threshold,
        sight: c.sight,
        leash: c.leash,
        boss: c.boss,
        respawn_s: c.respawn_s,
        still: c.still,
        npc: c.npc,
        might: c.might,
        loot: c.loot.as_ref().map(|l| Loot {
            components: l.components,
            standard: l.standard.clone(),
            top: l.top.clone(),
            coin: l.coin,
        }),
    })
}

fn compile_trial(t: &TrialToml) -> TrialDef {
    TrialDef {
        key: t.key.clone(),
        name: t.name.clone(),
        map: t.map.clone(),
        encounter: t.encounter.clone(),
        time_limit_s: t.time_limit_s,
        max_party_deaths: t.max_party_deaths,
        max_humans: t.max_humans,
        lens: Lens {
            damage: t.role.damage,
            tank: t.role.tank,
            healing: t.role.healing,
            command: t.role.command,
        },
    }
}

/// Compile abilities and builds into a validated pack (no creatures, no trials).
pub fn load_str(
    abilities: &str,
    builds: &str,
    rate: TickRate,
) -> Result<ContentPack, ContentError> {
    load_all(abilities, builds, "", "", rate)
}

/// Compile the four TOML documents into a validated pack.
pub fn load_all(
    abilities: &str,
    builds: &str,
    creatures: &str,
    trials: &str,
    rate: TickRate,
) -> Result<ContentPack, ContentError> {
    fn parse<T: serde::de::DeserializeOwned>(text: &str, path: &str) -> Result<T, ContentError> {
        toml::from_str(text).map_err(|e| ContentError::Toml {
            path: path.into(),
            source: e,
        })
    }
    let af: AbilitiesFile = parse(abilities, "abilities.toml")?;
    let bf: BuildsFile = parse(builds, "builds.toml")?;
    let cf: CreaturesFile = parse(creatures, "creatures.toml")?;
    let tf: TrialsFile = parse(trials, "trials.toml")?;
    let mut pack = ContentPack::default();
    let keys: Vec<String> = af.ability.iter().map(|a| a.key.clone()).collect();
    for (i, a) in af.ability.iter().enumerate() {
        pack.abilities.push(compile_ability(a, i, &keys, rate)?);
    }
    for b in &bf.build {
        let nb = compile_build(b, &pack)?;
        pack.builds.push(nb);
    }
    for c in &cf.creature {
        let def = compile_creature(c, &pack)?;
        pack.creatures.push(def);
    }
    pack.trials = tf.trial.iter().map(compile_trial).collect();
    pack.validate(rate)?;
    Ok(pack)
}

/// Load `<dir>/abilities.toml`, `builds.toml` and, when they exist, `creatures.toml` and
/// `trials.toml`. With an `items.toml` beside them, every material a creature drops must be
/// one it defines (ECONOMY.md 4).
pub fn load_dir(dir: &Path, rate: TickRate) -> Result<ContentPack, ContentError> {
    let read = |name: &str, optional: bool| {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(ContentError::Io {
                path: path.display().to_string(),
                source: e,
            }),
        }
    };
    let pack = load_all(
        &read("abilities.toml", false)?,
        &read("builds.toml", false)?,
        &read("creatures.toml", true)?,
        &read("trials.toml", true)?,
        rate,
    )?;
    if dir.join("items.toml").exists() {
        let items = items::load_items(dir)?;
        for c in &pack.creatures {
            for m in c.loot.iter().flat_map(|l| l.standard.iter().chain(&l.top)) {
                if !items.materials.iter().any(|known| &known.id == m) {
                    return Err(ContentError::Invalid(format!(
                        "creature {}: drops {m:?}, which items.toml does not define",
                        c.key
                    )));
                }
            }
        }
    }
    Ok(pack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::sim::test_content;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");

    #[test]
    fn shipped_content_loads_and_mirrors_the_fixture_pack() {
        let pack = load_dir(Path::new(DIR), TickRate::COMBAT).expect("content loads");
        let fixture = test_content::pack(TickRate::COMBAT);
        assert_eq!(pack.abilities.len(), fixture.abilities.len());
        for (a, b) in pack.abilities.iter().zip(&fixture.abilities) {
            assert_eq!(a.key, b.key);
            assert_eq!(a.slot, b.slot, "{}", a.key);
            assert_eq!(a.cost, b.cost, "{}", a.key);
            assert_eq!(a.aspect, b.aspect, "{}", a.key);
            assert_eq!(a.squad, b.squad, "{}", a.key);
            assert_eq!(a.creature, b.creature, "{}", a.key);
            assert_eq!(a.ability.steps, b.ability.steps, "{}", a.key);
            assert_eq!(a.ability.cost, b.ability.cost, "{}", a.key);
            assert_eq!(a.ability.cooldown, b.ability.cooldown, "{}", a.key);
            assert_eq!(a.ability.move_scale, b.ability.move_scale, "{}", a.key);
            assert_eq!(a.ability.interrupt, b.ability.interrupt, "{}", a.key);
            assert_eq!(a.ability.chain, b.ability.chain, "{}", a.key);
            assert_eq!(a.ability.firearm, b.ability.firearm, "{}", a.key);
            assert_eq!(a.ability.range, b.ability.range, "{}", a.key);
        }
        assert_eq!(pack.builds, fixture.builds);
        assert_eq!(pack.creatures, fixture.creatures);
        assert_eq!(pack.trials, fixture.trials);
    }

    #[test]
    fn creatures_follow_the_kit_rules_and_players_cannot_slot_their_abilities() {
        let pack = load_dir(Path::new(DIR), TickRate::COMBAT).expect("content loads");
        let (_, warden) = pack.creature("warden").expect("the Warden ships");
        assert!(warden.boss && warden.loot.as_ref().is_some_and(|l| l.components == 3));
        let sheet = gm_core::build::Sheet::creature(warden, &pack, 3);
        assert_eq!(sheet.derived.health, warden.health as i32);
        assert_eq!(sheet.derived.stagger_threshold, 400.0);
        // A player build with the Warden's maul is refused, whatever it costs.
        let mut stolen = pack.build("ironclad").unwrap().clone();
        stolen.primary = pack.find("maul").unwrap();
        assert!(matches!(
            stolen.validate(&pack),
            Err(gm_core::build::BuildError::CreatureOnly(_))
        ));
        // The leadership ability is what takes a squad from three to five.
        assert_eq!(pack.build("blade").unwrap().squad_capacity(&pack), 3);
        assert_eq!(pack.build("captain").unwrap().squad_capacity(&pack), 5);
        // A creature that drops what no item file defines is refused.
        let bad = std::fs::read_to_string(Path::new(DIR).join("creatures.toml"))
            .unwrap()
            .replace("core/iron", "core/unobtainium");
        let dir = std::env::temp_dir().join(format!("gm-content-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["abilities.toml", "builds.toml", "trials.toml", "items.toml"] {
            std::fs::copy(Path::new(DIR).join(f), dir.join(f)).unwrap();
        }
        std::fs::write(dir.join("creatures.toml"), bad).unwrap();
        let err = load_dir(&dir, TickRate::COMBAT).unwrap_err().to_string();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("unobtainium"), "{err}");
    }

    #[test]
    fn errors_name_the_ability() {
        let bad = r#"
[[ability]]
key = "x"
name = "X"
slot = "primary"
cost = 0
[[ability.step]]
melee_arc = { reach = 9999, arc = 90, windup_ms = 1, active_ms = 1, recovery_ms = 1, damage = { amount = 1, type = "slash" } }
"#;
        let e = load_str(bad, "", TickRate::COMBAT).unwrap_err();
        assert!(e.to_string().contains("x"), "{e}");
        let unknown = r#"
[[ability]]
key = "y"
name = "Y"
slot = "primary"
cost = 0
[[ability.step]]
melee_arc = { reach = 10, arc = 90, windup_ms = 1, active_ms = 1, recovery_ms = 1, damage = { amount = 1, type = "plasma" } }
"#;
        assert!(load_str(unknown, "", TickRate::COMBAT).is_err());
        let typo = r#"
[[ability]]
key = "z"
name = "Z"
slot = "primary"
cost = 0
cooldwon_ms = 5
"#;
        assert!(matches!(
            load_str(typo, "", TickRate::COMBAT),
            Err(ContentError::Toml { .. })
        ));
    }
}
