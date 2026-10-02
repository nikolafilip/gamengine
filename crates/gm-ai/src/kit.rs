//! Reading a kit by the shape of its abilities (COMPANIONS.md 4): a mind uses whatever it was
//! given by what each verb does, never by an ability's name.

use gm_core::build::{Kit, MAX_ACTIVES, Sheet};
use gm_core::matrix::ArmourClass;
use gm_core::tick::Tick;
use gm_core::vocab::{
    Ability, ApplyStatus, ArchetypeFrame, AreaEffect, Guard, MoveKind, Origin, Shape, Status,
    StatusTarget, Trigger, Verb,
};

/// What an ability is good for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Use {
    /// A status on the actor itself.
    Buff,
    /// An area at the actor's feet.
    Burst {
        radius: f32,
        harmful: bool,
    },
    /// An area a fixed way ahead.
    Placed {
        ahead: f32,
        radius: f32,
        harmful: bool,
    },
    /// An area where the actor aims.
    Aimed {
        range: f32,
        radius: f32,
        harmful: bool,
    },
    /// A projectile. `mends`: it carries Regen and no damage.
    Shot {
        speed: f32,
        gravity: f32,
        mends: bool,
        /// Ticks from activation to release.
        release: Tick,
    },
    Swing {
        reach: f32,
        arc: f32,
        windup: Tick,
    },
    /// A dash, a charge or a leap: a gap closer or a way out.
    Closer,
    Blink {
        distance: f32,
    },
}

/// The guard slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GuardPlan {
    None,
    Block {
        arc: f32,
        projectiles: bool,
        cost: f32,
    },
    Parry {
        arc: f32,
        window: Tick,
        slot: u8,
    },
}

/// A companion's role (COMPANIONS.md 4), read from its build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Role {
    Heal = 0,
    Tank = 1,
    Scout = 2,
    Dps = 3,
}

impl Role {
    pub const fn name(self) -> &'static str {
        match self {
            Role::Heal => "heal",
            Role::Tank => "tank",
            Role::Scout => "scout",
            Role::Dps => "dps",
        }
    }

    pub const fn from_index(i: u8) -> Role {
        match i {
            0 => Role::Heal,
            1 => Role::Tank,
            2 => Role::Scout,
            _ => Role::Dps,
        }
    }

    /// The first match from the top decides: heal, tank, scout, dps.
    pub fn of(sheet: &Sheet) -> Role {
        let plan = KitPlan::of(&sheet.kit);
        if plan.mends() {
            return Role::Heal;
        }
        let heavy = sheet.build.frame == ArchetypeFrame::Colossus
            && matches!(sheet.build.armour, ArmourClass::Mail | ArmourClass::Plate);
        if heavy
            || matches!(
                plan.guard,
                GuardPlan::Block {
                    projectiles: true,
                    ..
                }
            )
        {
            return Role::Tank;
        }
        if sheet.build.frame == ArchetypeFrame::Infiltrator || plan.stealth {
            return Role::Scout;
        }
        Role::Dps
    }
}

/// A kit as a mind reads it: kit slot and use of every ability.
#[derive(Clone, Debug, PartialEq)]
pub struct KitPlan {
    pub primary: Option<(u8, Use)>,
    pub secondary: Option<(u8, Use)>,
    /// By active slot (the `Input::ability` number is the index plus one).
    pub actives: [Option<(u8, Use)>; MAX_ACTIVES],
    pub guard: GuardPlan,
    /// A self-buff of the kit is Stealth.
    pub stealth: bool,
}

fn radius_of(shape: &Shape) -> f32 {
    match *shape {
        Shape::Sphere { radius } | Shape::Cylinder { radius, .. } => radius,
        Shape::Cone { length, .. } => length,
        Shape::Box { half_extents } => half_extents[0].max(half_extents[1]),
    }
}

/// Statuses nobody wants on themselves.
fn bane(s: &ApplyStatus) -> bool {
    !matches!(
        s.status,
        Status::Regen | Status::Haste | Status::Fortify | Status::Stealth
    )
}

fn area_harmful(a: &AreaEffect) -> bool {
    a.damage.is_some_and(|d| d.amount > 0)
        || a.effects
            .iter()
            .any(|s| s.target != StatusTarget::Actor && bane(s))
}

fn classify(ab: &Ability) -> Option<Use> {
    let step = ab.steps.first()?;
    Some(match &step.verb {
        Verb::ApplyStatus(s) if s.target == StatusTarget::Actor => Use::Buff,
        Verb::ApplyStatus(_) => return None,
        Verb::AreaEffect(a) => {
            let (radius, harmful) = (radius_of(&a.shape), area_harmful(a));
            match a.origin {
                Origin::Weapon { offset } => Use::Placed {
                    ahead: offset[0],
                    radius,
                    harmful,
                },
                Origin::Aim { range } => Use::Aimed {
                    range,
                    radius,
                    harmful,
                },
                _ => Use::Burst { radius, harmful },
            }
        }
        Verb::Projectile(p) => Use::Shot {
            speed: p.speed,
            gravity: p.gravity_scale,
            mends: p.damage.amount == 0
                && p.on_hit.iter().any(
                    |t| matches!(t, Trigger::Status(s) if s.status == Status::Regen && s.target == StatusTarget::Hit),
                ),
            release: step.at,
        },
        Verb::MeleeArc(m) => Use::Swing {
            reach: m.reach,
            arc: m.arc_deg,
            windup: step.at + m.timing.windup,
        },
        Verb::MoveSelf(m) => match m.kind {
            MoveKind::Blink { distance } => Use::Blink { distance },
            _ => Use::Closer,
        },
        Verb::Guard(_) => return None,
    })
}

impl KitPlan {
    pub fn of(kit: &Kit) -> KitPlan {
        let slot = |i: Option<u8>| {
            let i = i?;
            Some((i, classify(kit.abilities.get(i as usize)?)?))
        };
        let guard = match (kit.guard, kit.guard_verb()) {
            (Some(_), Some(Guard::Block(b))) => GuardPlan::Block {
                arc: b.arc_deg,
                projectiles: b.stops_projectiles,
                cost: b.stamina_per_hit as f32,
            },
            (Some(i), Some(Guard::Parry(p))) => GuardPlan::Parry {
                arc: p.arc_deg,
                window: p.window,
                slot: i,
            },
            _ => GuardPlan::None,
        };
        let mut actives = [None; MAX_ACTIVES];
        for (out, a) in actives.iter_mut().zip(kit.actives) {
            *out = slot(a);
        }
        let stealth = kit.abilities.iter().any(|ab| {
            ab.steps.iter().any(|s| {
                matches!(&s.verb, Verb::ApplyStatus(st) if st.status == Status::Stealth && st.target == StatusTarget::Actor)
            })
        });
        KitPlan {
            primary: slot(kit.primary),
            secondary: slot(kit.secondary),
            actives,
            guard,
            stealth,
        }
    }

    /// Reach of the primary when it is a swing.
    pub fn reach(&self) -> f32 {
        match self.primary {
            Some((_, Use::Swing { reach, .. })) => reach,
            _ => 0.0,
        }
    }

    /// A harmful shot in the secondary slot: the kit fights at range.
    pub fn shoots(&self) -> bool {
        matches!(self.secondary, Some((_, Use::Shot { mends: false, .. })))
    }

    /// Any ability that heals somebody else.
    pub fn mends(&self) -> bool {
        let heals = |u: &Use| {
            matches!(
                u,
                Use::Shot { mends: true, .. }
                    | Use::Aimed { harmful: false, .. }
                    | Use::Placed { harmful: false, .. }
            )
        };
        self.secondary.iter().any(|(_, u)| heals(u))
            || self.actives.iter().flatten().any(|(_, u)| heals(u))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::sim::test_content;
    use gm_core::tick::TickRate;

    #[test]
    fn roles_are_read_from_the_presets() {
        let pack = test_content::pack(TickRate::COMBAT);
        let role = |name: &str| Role::of(&Sheet::new(pack.build(name).unwrap().clone(), &pack, 0));
        assert_eq!(role("ironclad"), Role::Tank);
        assert_eq!(role("mender"), Role::Heal);
        assert_eq!(role("frostweaver"), Role::Dps);
        assert_eq!(role("blade"), Role::Dps);
        assert_eq!(role("captain"), Role::Dps);
        assert_eq!(role("shade"), Role::Scout);
        for r in [Role::Heal, Role::Tank, Role::Scout, Role::Dps] {
            assert_eq!(Role::from_index(r as u8), r);
        }
    }

    #[test]
    fn kits_are_read_by_shape() {
        let pack = test_content::pack(TickRate::COMBAT);
        let plan =
            |name: &str| KitPlan::of(&Sheet::new(pack.build(name).unwrap().clone(), &pack, 0).kit);
        let mender = plan("mender");
        assert!(matches!(
            mender.secondary,
            Some((_, Use::Shot { mends: true, .. }))
        ));
        assert!(matches!(
            mender.actives[0],
            Some((
                _,
                Use::Aimed {
                    harmful: false,
                    range,
                    ..
                }
            )) if range == 500.0
        ));
        assert!(mender.mends() && !mender.shoots());
        let ironclad = plan("ironclad");
        assert_eq!(ironclad.reach(), 76.0);
        assert!(matches!(
            ironclad.guard,
            GuardPlan::Block {
                projectiles: true,
                ..
            }
        ));
        assert!(matches!(
            ironclad.actives[0],
            Some((_, Use::Burst { harmful: true, .. }))
        ));
        assert!(matches!(ironclad.actives[1], Some((_, Use::Buff))));
        let shade = plan("shade");
        assert!(shade.stealth);
        assert!(matches!(shade.actives[0], Some((_, Use::Blink { .. }))));
        assert!(matches!(
            shade.actives[2],
            Some((_, Use::Placed { harmful: true, .. }))
        ));
        // The Warden: a frontal swing, a thrown stone, a telegraphed circle, a stomp.
        let (_, def) = pack.creature("warden").unwrap();
        let warden = KitPlan::of(&Sheet::creature(def, &pack, 3).kit);
        assert!(matches!(warden.primary, Some((_, Use::Swing { arc, .. })) if arc == 120.0));
        assert!(warden.shoots());
        assert!(matches!(
            warden.actives[0],
            Some((_, Use::Aimed { harmful: true, .. }))
        ));
        assert_eq!(warden.guard, GuardPlan::None);
    }
}
