//! Content authoring format (MATRIX.md 10): abilities and preset builds written in TOML with
//! milliseconds and names, compiled against a tick rate into a validated
//! [`gm_core::build::ContentPack`]. Only zones and tools load content; clients receive the
//! compiled pack over the wire.
#![forbid(unsafe_code)]

pub mod items;

use std::path::Path;

use gm_core::build::{AbilityDef, Build, ContentPack, NamedBuild, Slot};
use gm_core::matrix::{ArmourClass, Aspects, Attributes, Element};
use gm_core::tick::{Tick, TickRate};
use gm_core::vocab::{
    Ability, AbilityId, ApplyStatus, ArchetypeFrame, AreaEffect, Block, Bounce, Bypass, Cooldown,
    Cost, DamagePacket, DamageType, Falloff, Guard, Interrupt, MeleeArc, MoveKind, MoveSelf,
    Origin, Parry, Projectile, Riposte, Shape, StackRule, Status, StatusTarget, Step, Timing,
    Trigger, Verb,
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

// ---------- authoring types (TOML) ----------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbilitiesFile {
    #[serde(default)]
    ability: Vec<AbilityToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildsFile {
    #[serde(default)]
    build: Vec<BuildToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AbilityToml {
    key: String,
    name: String,
    slot: String,
    cost: u8,
    aspect: Option<String>,
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
    #[serde(default)]
    step: Vec<StepToml>,
}

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
    Weapon { weapon: [f32; 3] },
    Point { point: [f32; 3] },
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
struct BuildToml {
    name: String,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttributesToml {
    str: u8,
    agi: u8,
    con: u8,
    int: u8,
    spr: u8,
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
        Ok(MoveSelf {
            kind,
            cancelable: m.cancelable,
            keep_friction: m.keep_friction,
            iframes: self.ticks(m.iframes_ms),
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
        other => return ctx.err(format!("unknown slot {other:?}")),
    };
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
                ticks: ctx.ticks(a.cooldown_ms),
                group: a.cooldown_group,
            },
            steps,
            move_scale: a.move_scale,
            interrupt,
        },
        slot,
        cost: a.cost,
        aspect,
    })
}

fn compile_build(b: &BuildToml, pack: &ContentPack) -> Result<NamedBuild, ContentError> {
    let err = |msg: String| ContentError::Invalid(format!("build {}: {msg}", b.name));
    let find = |key: &str| {
        pack.find(key)
            .ok_or_else(|| err(format!("unknown ability {key:?}")))
    };
    let frame = match b.frame.as_str() {
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
    for a in &b.aspects {
        let e = Element::ALL
            .into_iter()
            .find(|e| e.name() == a.as_str())
            .ok_or_else(|| err(format!("unknown aspect {a:?}")))?;
        aspects.0 |= Aspects::one(e).0;
    }
    Ok(NamedBuild {
        name: b.name.clone(),
        build: Build {
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
            primary: find(&b.primary)?,
            secondary: find(&b.secondary)?,
            guard: b.guard.as_deref().map(find).transpose()?,
            actives: b
                .actives
                .iter()
                .map(|k| find(k))
                .collect::<Result<_, _>>()?,
        },
    })
}

/// Compile the two TOML documents into a validated pack.
pub fn load_str(
    abilities: &str,
    builds: &str,
    rate: TickRate,
) -> Result<ContentPack, ContentError> {
    let af: AbilitiesFile = toml::from_str(abilities).map_err(|e| ContentError::Toml {
        path: "abilities.toml".into(),
        source: e,
    })?;
    let bf: BuildsFile = toml::from_str(builds).map_err(|e| ContentError::Toml {
        path: "builds.toml".into(),
        source: e,
    })?;
    let mut pack = ContentPack::default();
    for (i, a) in af.ability.iter().enumerate() {
        pack.abilities.push(compile_ability(a, i, rate)?);
    }
    for b in &bf.build {
        let nb = compile_build(b, &pack)?;
        pack.builds.push(nb);
    }
    pack.validate(rate)?;
    Ok(pack)
}

/// Load `<dir>/abilities.toml` and `<dir>/builds.toml`.
pub fn load_dir(dir: &Path, rate: TickRate) -> Result<ContentPack, ContentError> {
    let read = |name: &str| {
        let path = dir.join(name);
        std::fs::read_to_string(&path).map_err(|e| ContentError::Io {
            path: path.display().to_string(),
            source: e,
        })
    };
    load_str(&read("abilities.toml")?, &read("builds.toml")?, rate)
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
            assert_eq!(a.ability.steps, b.ability.steps, "{}", a.key);
            assert_eq!(a.ability.cost, b.ability.cost, "{}", a.key);
            assert_eq!(a.ability.cooldown, b.ability.cooldown, "{}", a.key);
            assert_eq!(a.ability.move_scale, b.ability.move_scale, "{}", a.key);
            assert_eq!(a.ability.interrupt, b.ability.interrupt, "{}", a.key);
        }
        assert_eq!(pack.builds, fixture.builds);
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
