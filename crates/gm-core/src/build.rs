//! Point-buy builds and content packs (MATRIX.md 9 and 10): the budget, the slots, validation,
//! and the runtime [`Kit`] a build compiles to.

use crate::matrix::{ArmourClass, Aspects, Attributes, Derived, Element};
use crate::tick::{Tick, TickRate};
use crate::vocab::{
    Ability, ArchetypeFrame, DamageType, Guard, MoveKind, MoveSelf, Riposte, Trigger, Verb,
};

/// Every build spends exactly this many points (MATRIX.md 9).
pub const BUDGET: u32 = 100;
/// Second aspect cost (MATRIX.md 5).
pub const SECOND_ASPECT_COST: u32 = 10;
pub const MAX_ACTIVES: usize = 4;

/// What slot an ability may be put in (MATRIX.md 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Slot {
    Primary = 0,
    Secondary = 1,
    Guard = 2,
    Active = 3,
}

/// An ability as content ships it: the verb script plus its price and gating.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct AbilityDef {
    /// Stable content key (`"hammer"`), unique within a pack.
    pub key: String,
    pub ability: Ability,
    pub slot: Slot,
    pub cost: u8,
    /// Required aspect; also the element of every elemental packet in the script.
    pub aspect: Option<Element>,
}

/// A character build (MATRIX.md 9). Ability references are indices into the content pack.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Build {
    pub frame: ArchetypeFrame,
    pub attributes: Attributes,
    pub armour: ArmourClass,
    pub aspects: Aspects,
    pub primary: u16,
    pub secondary: u16,
    pub guard: Option<u16>,
    pub actives: Vec<u16>,
}

/// A build with the name content gave it.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NamedBuild {
    pub name: String,
    pub build: Build,
}

/// Everything a zone loaded from `assets/content` and sends to its clients (MATRIX.md 10).
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct ContentPack {
    pub abilities: Vec<AbilityDef>,
    pub builds: Vec<NamedBuild>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    AttributeOutOfRange,
    NoAspect,
    TooManyAspects,
    UnknownAbility(u16),
    WrongSlot { index: u16, expected: Slot },
    DuplicateAbility(u16),
    MissingAspect { index: u16, needs: Element },
    SharedCooldownGroup(u8),
    TooManyActives,
    Budget { spent: u32 },
}

impl core::fmt::Display for BuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BuildError::AttributeOutOfRange => write!(
                f,
                "attributes must be {}..={}",
                Attributes::MIN,
                Attributes::MAX
            ),
            BuildError::NoAspect => write!(f, "a build needs at least one aspect"),
            BuildError::TooManyAspects => write!(f, "at most two aspects"),
            BuildError::UnknownAbility(i) => write!(f, "unknown ability {i}"),
            BuildError::WrongSlot { index, expected } => {
                write!(f, "ability {index} does not fit the {expected:?} slot")
            }
            BuildError::DuplicateAbility(i) => write!(f, "ability {i} slotted twice"),
            BuildError::MissingAspect { index, needs } => {
                write!(f, "ability {index} needs the {} aspect", needs.name())
            }
            BuildError::SharedCooldownGroup(g) => {
                write!(f, "two abilities share cooldown group {g}")
            }
            BuildError::TooManyActives => write!(f, "at most {MAX_ACTIVES} actives"),
            BuildError::Budget { spent } => write!(f, "build spends {spent} of {BUDGET} points"),
        }
    }
}

impl core::error::Error for BuildError {}

#[derive(Debug, PartialEq, Eq)]
pub struct ContentError {
    pub key: String,
    pub reason: String,
}

impl core::fmt::Display for ContentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "content {}: {}", self.key, self.reason)
    }
}

impl core::error::Error for ContentError {}

impl Build {
    /// Points spent (MATRIX.md 9).
    pub fn cost(&self, pack: &ContentPack) -> u32 {
        let ability = |i: u16| pack.abilities.get(i as usize).map_or(0, |a| a.cost as u32);
        self.attributes.cost()
            + self.armour.cost()
            + if self.aspects.count() >= 2 {
                SECOND_ASPECT_COST
            } else {
                0
            }
            + ability(self.primary)
            + ability(self.secondary)
            + self.guard.map_or(0, ability)
            + self.actives.iter().map(|&i| ability(i)).sum::<u32>()
    }

    /// Every slotted ability index in kit order: primary, secondary, actives, guard.
    pub fn slots(&self) -> Vec<(u16, Slot)> {
        let mut out = vec![
            (self.primary, Slot::Primary),
            (self.secondary, Slot::Secondary),
        ];
        out.extend(self.actives.iter().map(|&i| (i, Slot::Active)));
        if let Some(g) = self.guard {
            out.push((g, Slot::Guard));
        }
        out
    }

    /// MATRIX.md 9: ranges, aspects, slots, gating, cooldown groups, the exact budget.
    pub fn validate(&self, pack: &ContentPack) -> Result<(), BuildError> {
        if !self.attributes.in_range() {
            return Err(BuildError::AttributeOutOfRange);
        }
        if self.aspects.0 & !0x1f != 0 {
            return Err(BuildError::TooManyAspects);
        }
        match self.aspects.count() {
            0 => return Err(BuildError::NoAspect),
            1 | 2 => {}
            _ => return Err(BuildError::TooManyAspects),
        }
        if self.actives.len() > MAX_ACTIVES {
            return Err(BuildError::TooManyActives);
        }
        let mut seen: Vec<u16> = Vec::new();
        let mut groups: Vec<u8> = Vec::new();
        for (index, expected) in self.slots() {
            let def = pack
                .abilities
                .get(index as usize)
                .ok_or(BuildError::UnknownAbility(index))?;
            if def.slot != expected {
                return Err(BuildError::WrongSlot { index, expected });
            }
            if seen.contains(&index) {
                return Err(BuildError::DuplicateAbility(index));
            }
            seen.push(index);
            if let Some(needs) = def.aspect
                && !self.aspects.contains(needs)
            {
                return Err(BuildError::MissingAspect { index, needs });
            }
            if let Some(g) = def.ability.cooldown.group {
                if groups.contains(&g) {
                    return Err(BuildError::SharedCooldownGroup(g));
                }
                groups.push(g);
            }
        }
        let spent = self.cost(pack);
        if spent != BUDGET {
            return Err(BuildError::Budget { spent });
        }
        Ok(())
    }

    pub fn derived(&self) -> Derived {
        Derived::compute(self.attributes, self.frame, self.armour)
    }
}

impl ContentPack {
    pub fn find(&self, key: &str) -> Option<u16> {
        self.abilities
            .iter()
            .position(|a| a.key == key)
            .map(|i| i as u16)
    }

    pub fn build(&self, name: &str) -> Option<&Build> {
        self.builds
            .iter()
            .find(|b| b.name == name)
            .map(|b| &b.build)
    }

    /// Every ability validates against VOCABULARY.md 11, keys are unique, elemental packets
    /// match the ability's aspect, guard abilities are exactly one `Guard` step, and every
    /// preset build validates.
    pub fn validate(&self, rate: TickRate) -> Result<(), ContentError> {
        for (i, def) in self.abilities.iter().enumerate() {
            let err = |reason: String| ContentError {
                key: def.key.clone(),
                reason,
            };
            if def.key.is_empty() || self.abilities[..i].iter().any(|o| o.key == def.key) {
                return Err(err("duplicate or empty key".into()));
            }
            def.ability
                .validate(rate)
                .map_err(|e| err(e.reason.to_string()))?;
            let is_guard =
                def.ability.steps.len() == 1 && matches!(def.ability.steps[0].verb, Verb::Guard(_));
            if (def.slot == Slot::Guard) != is_guard {
                return Err(err(
                    "guard slot abilities are exactly one Guard step, and only those".into(),
                ));
            }
            let mut types = Vec::new();
            collect_types(&def.ability, &mut types);
            for t in types {
                match (t.element(), def.aspect) {
                    (Some(e), Some(a)) if e != a => {
                        return Err(err(format!(
                            "{} packet in a {} ability",
                            t.name(),
                            a.name()
                        )));
                    }
                    (Some(e), None) => {
                        return Err(err(format!(
                            "{} packet in an ability without an aspect",
                            e.name()
                        )));
                    }
                    _ => {}
                }
            }
        }
        for nb in &self.builds {
            nb.build.validate(self).map_err(|e| ContentError {
                key: nb.name.clone(),
                reason: e.to_string(),
            })?;
        }
        Ok(())
    }
}

fn collect_types(a: &Ability, out: &mut Vec<DamageType>) {
    let area = |ae: &crate::vocab::AreaEffect, out: &mut Vec<DamageType>| {
        if let Some(d) = ae.damage {
            out.push(d.dtype);
        }
    };
    for s in &a.steps {
        match &s.verb {
            Verb::MeleeArc(m) => out.push(m.damage.dtype),
            Verb::Projectile(p) => {
                out.push(p.damage.dtype);
                for t in p.on_hit.iter().chain(&p.on_expire) {
                    if let Trigger::Area(ae) = t {
                        area(ae, out);
                    }
                }
            }
            Verb::AreaEffect(ae) => area(ae, out),
            Verb::Guard(Guard::Parry(p)) => {
                for r in &p.on_success {
                    if let Riposte::Swing(arc) = r {
                        out.push(arc.damage.dtype);
                    }
                }
            }
            Verb::ApplyStatus(_) | Verb::MoveSelf(_) | Verb::Guard(Guard::Block(_)) => {}
        }
    }
}

/// The abilities in play for one character, in kit order: primary, secondary, actives,
/// guard. This is what [`crate::sim::step_mover`] runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Kit {
    pub abilities: Vec<Ability>,
    /// Ticks a script occupies the actor (windup + active + recovery of its verbs).
    pub durations: Vec<Tick>,
    /// Elemental abilities are refused under `Silence`.
    pub elemental: Vec<bool>,
    pub primary: Option<u8>,
    pub secondary: Option<u8>,
    pub actives: [Option<u8>; MAX_ACTIVES],
    pub guard: Option<u8>,
}

impl Kit {
    /// Compile a validated build against its pack.
    pub fn from_build(build: &Build, pack: &ContentPack) -> Kit {
        let mut kit = Kit::empty();
        for (index, slot) in build.slots() {
            let def = &pack.abilities[index as usize];
            let i = kit.push(def.ability.clone(), def.aspect.is_some());
            match slot {
                Slot::Primary => kit.primary = Some(i),
                Slot::Secondary => kit.secondary = Some(i),
                Slot::Guard => kit.guard = Some(i),
                Slot::Active => {
                    if let Some(free) = kit.actives.iter_mut().find(|a| a.is_none()) {
                        *free = Some(i);
                    }
                }
            }
        }
        kit
    }

    pub fn empty() -> Kit {
        Kit {
            abilities: Vec::new(),
            durations: Vec::new(),
            elemental: Vec::new(),
            primary: None,
            secondary: None,
            actives: [None; MAX_ACTIVES],
            guard: None,
        }
    }

    /// Append an ability and return its index.
    pub fn push(&mut self, ability: Ability, elemental: bool) -> u8 {
        self.durations.push(script_duration(&ability));
        self.elemental.push(elemental);
        self.abilities.push(ability);
        (self.abilities.len() - 1) as u8
    }

    /// The guard verb, if the kit has one.
    pub fn guard_verb(&self) -> Option<&Guard> {
        let i = self.guard? as usize;
        match &self.abilities[i].steps[0].verb {
            Verb::Guard(g) => Some(g),
            _ => None,
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

pub fn script_duration(a: &Ability) -> Tick {
    a.steps
        .iter()
        .map(|s| s.at + verb_duration(&s.verb))
        .max()
        .unwrap_or(0)
        .max(1)
}

/// Everything the simulation needs per character.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub build: Build,
    pub kit: Kit,
    pub derived: Derived,
    /// Build points left after the kit; informational.
    pub team: u8,
}

impl Sheet {
    pub fn new(build: Build, pack: &ContentPack, team: u8) -> Sheet {
        Sheet {
            kit: Kit::from_build(&build, pack),
            derived: build.derived(),
            build,
            team,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::test_content;

    #[test]
    fn presets_validate_and_spend_exactly_the_budget() {
        let pack = test_content::pack(TickRate::COMBAT);
        pack.validate(TickRate::COMBAT).unwrap();
        for nb in &pack.builds {
            assert_eq!(nb.build.cost(&pack), BUDGET, "{}", nb.name);
            let kit = Kit::from_build(&nb.build, &pack);
            assert!(
                kit.primary.is_some() && kit.secondary.is_some(),
                "{}",
                nb.name
            );
        }
    }

    #[test]
    fn validation_catches_every_rule() {
        let pack = test_content::pack(TickRate::COMBAT);
        let base = pack.build("blade").unwrap().clone();
        let mut b = base.clone();
        b.attributes.str_ = 21;
        assert_eq!(b.validate(&pack), Err(BuildError::AttributeOutOfRange));
        let mut b = base.clone();
        b.aspects = Aspects::NONE;
        assert_eq!(b.validate(&pack), Err(BuildError::NoAspect));
        let mut b = base.clone();
        b.primary = 999;
        assert_eq!(b.validate(&pack), Err(BuildError::UnknownAbility(999)));
        let mut b = base.clone();
        b.primary = b.secondary;
        assert!(matches!(
            b.validate(&pack),
            Err(BuildError::WrongSlot { .. })
        ));
        let mut b = base.clone();
        b.actives.push(b.actives[0]);
        assert!(matches!(
            b.validate(&pack),
            Err(BuildError::DuplicateAbility(_))
        ));
        let mut b = base.clone();
        b.attributes.str_ -= 1;
        assert!(matches!(b.validate(&pack), Err(BuildError::Budget { .. })));
        // Frost content needs the Frost aspect.
        let mut b = base.clone();
        b.secondary = pack.find("ice_shard").unwrap();
        assert!(matches!(
            b.validate(&pack),
            Err(BuildError::MissingAspect { .. })
        ));
    }
}
