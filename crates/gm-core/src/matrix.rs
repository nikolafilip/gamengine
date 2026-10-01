//! The character matrix: the data mirror of `docs/MATRIX.md`. Elements, physical kinds,
//! armour classes, frame modifiers, derived stats and the damage pipeline (MATRIX.md 7).
//!
//! Keep this file and the document in lockstep. When they disagree, the document wins and the
//! code is wrong.

use crate::vocab::{ArchetypeFrame, Bypass, DamagePacket, DamageType};

/// The five elements, in pentagram order (MATRIX.md 5): element `i` beats `i + 1` and
/// `i + 3` (mod 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[repr(u8)]
pub enum Element {
    Flame = 0,
    Shadow = 1,
    Storm = 2,
    Frost = 3,
    Stone = 4,
}

impl Element {
    pub const ALL: [Element; 5] = [
        Element::Flame,
        Element::Shadow,
        Element::Storm,
        Element::Frost,
        Element::Stone,
    ];

    pub const fn from_index(i: u8) -> Option<Element> {
        match i {
            0 => Some(Element::Flame),
            1 => Some(Element::Shadow),
            2 => Some(Element::Storm),
            3 => Some(Element::Frost),
            4 => Some(Element::Stone),
            _ => None,
        }
    }

    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn name(self) -> &'static str {
        match self {
            Element::Flame => "flame",
            Element::Shadow => "shadow",
            Element::Storm => "storm",
            Element::Frost => "frost",
            Element::Stone => "stone",
        }
    }
}

/// Damage multiplier of an `attack` element into one defending `aspect` (MATRIX.md 5).
pub fn element_mult(attack: Element, aspect: Element) -> f32 {
    let a = attack as u8;
    let d = aspect as u8;
    if (a + 1) % 5 == d || (a + 3) % 5 == d {
        2.0
    } else if a == d || (d + 1) % 5 == a || (d + 3) % 5 == a {
        0.5
    } else {
        // Five elements, each beats two, loses to two and resists itself: no neutral pairs.
        unreachable!("pentagram covers every pair")
    }
}

/// A set of 1..=2 aspects as a bitmask over `Element` indices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Aspects(pub u8);

impl Aspects {
    pub const NONE: Aspects = Aspects(0);

    pub fn one(e: Element) -> Aspects {
        Aspects(1 << e as u8)
    }

    pub fn two(a: Element, b: Element) -> Aspects {
        Aspects((1 << a as u8) | (1 << b as u8))
    }

    pub fn contains(self, e: Element) -> bool {
        self.0 & (1 << e as u8) != 0
    }

    pub fn count(self) -> u32 {
        (self.0 & 0x1f).count_ones()
    }

    pub fn iter(self) -> impl Iterator<Item = Element> {
        Element::ALL.into_iter().filter(move |e| self.contains(*e))
    }

    /// Product of the element multipliers over the aspects; 1 when there are none.
    pub fn mult_against(self, attack: Element) -> f32 {
        self.iter()
            .map(|asp| element_mult(attack, asp))
            .product::<f32>()
    }
}

/// Physical damage kinds (MATRIX.md 4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[repr(u8)]
pub enum Kind {
    Slash = 0,
    Pierce = 1,
    Blunt = 2,
}

/// Armour classes (MATRIX.md 4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
#[repr(u8)]
pub enum ArmourClass {
    #[default]
    Cloth = 0,
    Leather = 1,
    Mail = 2,
    Plate = 3,
}

impl ArmourClass {
    pub const ALL: [ArmourClass; 4] = [
        ArmourClass::Cloth,
        ArmourClass::Leather,
        ArmourClass::Mail,
        ArmourClass::Plate,
    ];

    pub const fn from_index(i: u8) -> Option<ArmourClass> {
        match i {
            0 => Some(ArmourClass::Cloth),
            1 => Some(ArmourClass::Leather),
            2 => Some(ArmourClass::Mail),
            3 => Some(ArmourClass::Plate),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            ArmourClass::Cloth => "cloth",
            ArmourClass::Leather => "leather",
            ArmourClass::Mail => "mail",
            ArmourClass::Plate => "plate",
        }
    }

    /// Build points (MATRIX.md 4).
    pub const fn cost(self) -> u32 {
        match self {
            ArmourClass::Cloth => 0,
            ArmourClass::Leather => 4,
            ArmourClass::Mail => 8,
            ArmourClass::Plate => 12,
        }
    }

    pub const fn mods(self) -> ArmourMods {
        match self {
            ArmourClass::Cloth => ArmourMods {
                speed: 1.0,
                regen: 1.0,
                ward: 0.05,
                evasion: 0.0,
            },
            ArmourClass::Leather => ArmourMods {
                speed: 1.0,
                regen: 1.0,
                ward: 0.0,
                evasion: 0.05,
            },
            ArmourClass::Mail => ArmourMods {
                speed: 0.95,
                regen: 0.90,
                ward: 0.0,
                evasion: 0.0,
            },
            ArmourClass::Plate => ArmourMods {
                speed: 0.90,
                regen: 0.75,
                ward: 0.0,
                evasion: 0.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmourMods {
    pub speed: f32,
    pub regen: f32,
    pub ward: f32,
    pub evasion: f32,
}

/// Physical kind × armour class (MATRIX.md 4.1).
pub fn kind_mult(kind: Kind, class: ArmourClass) -> f32 {
    const TABLE: [[f32; 4]; 3] = [
        // Cloth, Leather, Mail, Plate
        [1.25, 1.00, 0.75, 0.50], // Slash
        [1.00, 1.00, 1.00, 0.75], // Pierce
        [0.75, 0.75, 1.00, 1.25], // Blunt
    ];
    TABLE[kind as usize][class as usize]
}

/// Frame modifiers (MATRIX.md 3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameMods {
    pub mass: f32,
    pub mobility: f32,
    pub armour: f32,
    pub ward: f32,
    pub evasion: f32,
    pub focus: i32,
}

impl ArchetypeFrame {
    pub const fn mods(self) -> FrameMods {
        match self {
            ArchetypeFrame::Colossus => FrameMods {
                mass: 1.5,
                mobility: 0.92,
                armour: 0.10,
                ward: 0.0,
                evasion: 0.0,
                focus: 0,
            },
            ArchetypeFrame::Striker => FrameMods {
                mass: 1.0,
                mobility: 1.0,
                armour: 0.05,
                ward: 0.0,
                evasion: 0.0,
                focus: 0,
            },
            ArchetypeFrame::Caster => FrameMods {
                mass: 0.9,
                mobility: 0.96,
                armour: 0.0,
                ward: 0.10,
                evasion: 0.0,
                focus: 20,
            },
            ArchetypeFrame::Infiltrator => FrameMods {
                mass: 0.8,
                mobility: 1.06,
                armour: 0.0,
                ward: 0.0,
                evasion: 0.05,
                focus: 0,
            },
        }
    }
}

/// The five attributes (MATRIX.md 2), each `MIN..=MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bitcode", derive(bitcode::Encode, bitcode::Decode))]
pub struct Attributes {
    pub str_: u8,
    pub agi: u8,
    pub con: u8,
    pub int: u8,
    pub spr: u8,
}

impl Attributes {
    pub const MIN: u8 = 5;
    pub const MAX: u8 = 20;

    pub const fn flat(v: u8) -> Attributes {
        Attributes {
            str_: v,
            agi: v,
            con: v,
            int: v,
            spr: v,
        }
    }

    pub const fn new(str_: u8, agi: u8, con: u8, int: u8, spr: u8) -> Attributes {
        Attributes {
            str_,
            agi,
            con,
            int,
            spr,
        }
    }

    pub fn as_array(self) -> [u8; 5] {
        [self.str_, self.agi, self.con, self.int, self.spr]
    }

    /// Build points spent: every point above the floor costs one.
    pub fn cost(self) -> u32 {
        self.as_array()
            .iter()
            .map(|&v| v.saturating_sub(Self::MIN) as u32)
            .sum()
    }

    pub fn in_range(self) -> bool {
        self.as_array()
            .iter()
            .all(|v| (Self::MIN..=Self::MAX).contains(v))
    }
}

impl Default for Attributes {
    fn default() -> Self {
        Attributes::flat(12)
    }
}

/// Everything the simulation reads per character (MATRIX.md 6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Derived {
    pub health: i32,
    pub stamina: f32,
    pub stamina_regen: f32,
    pub focus: f32,
    pub focus_regen: f32,
    pub max_speed: f32,
    pub physical_mult: f32,
    pub elemental_mult: f32,
    pub armour: f32,
    pub ward: f32,
    pub evasion: f32,
    pub knockback_taken: f32,
    pub knockback_dealt: f32,
    pub status_duration: f32,
    pub stagger_threshold: f32,
}

pub const MAX_ARMOUR: f32 = 0.35;
pub const MAX_WARD: f32 = 0.50;
pub const MAX_EVASION: f32 = 0.40;
/// Evasion lingers this many ticks after a `MoveSelf` ends (MATRIX.md 6).
pub const EVADING_GRACE_TICKS: u32 = 2;
/// Stagger build-up decays this many points per second (MATRIX.md 7).
pub const STAGGER_DECAY_PER_S: f32 = 20.0;
/// Stagger applied when the threshold is crossed, milliseconds.
pub const STAGGER_MS: u32 = 400;
/// Build-up is discarded this long after a stagger ends (MATRIX.md 7).
pub const STAGGER_IMMUNITY_MS: u32 = 1000;
/// Freeze: Root this long, then Chill immunity (MATRIX.md 8).
pub const FREEZE_MS: u32 = 1000;
pub const CHILL_IMMUNITY_MS: u32 = 2000;
/// Chill slows this much per stack.
pub const CHILL_PER_STACK: f32 = 0.15;
/// Damage- and heal-over-time pulses per second.
pub const DOT_PULSES_PER_S: u32 = 4;

impl Derived {
    pub fn compute(attrs: Attributes, frame: ArchetypeFrame, armour: ArmourClass) -> Derived {
        let f = frame.mods();
        let a = armour.mods();
        let (str_, agi, con, int, spr) = (
            attrs.str_ as f32,
            attrs.agi as f32,
            attrs.con as f32,
            attrs.int as f32,
            attrs.spr as f32,
        );
        Derived {
            health: 80 + 3 * attrs.con as i32,
            stamina: 60.0 + 2.0 * (con + agi),
            stamina_regen: (10.0 + 0.5 * agi) * a.regen,
            focus: 40.0 + 4.0 * int + f.focus as f32,
            focus_regen: 5.0 + 0.5 * spr,
            max_speed: (290.0 + 2.0 * agi) * f.mobility * a.speed,
            physical_mult: 0.80 + 0.02 * str_,
            elemental_mult: 0.80 + 0.02 * int,
            armour: (0.01 * con + f.armour).min(MAX_ARMOUR),
            ward: (0.015 * spr + f.ward + a.ward).min(MAX_WARD),
            evasion: (0.01 * agi + f.evasion + a.evasion).min(MAX_EVASION),
            knockback_taken: 1.0 / f.mass,
            knockback_dealt: 0.80 + 0.02 * str_,
            status_duration: 1.20 - 0.02 * spr,
            stagger_threshold: 40.0 + 2.0 * con,
        }
    }
}

/// The attacker's side of the pipeline, captured when the packet is created so a dead or
/// departed owner (area pulses, projectiles in flight) still resolves consistently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackerStats {
    pub physical_mult: f32,
    pub elemental_mult: f32,
    pub knockback_dealt: f32,
    /// `Weaken` magnitude, 0 when absent.
    pub weaken: f32,
}

impl AttackerStats {
    pub const NEUTRAL: AttackerStats = AttackerStats {
        physical_mult: 1.0,
        elemental_mult: 1.0,
        knockback_dealt: 1.0,
        weaken: 0.0,
    };

    pub fn from_derived(d: &Derived, weaken: f32) -> AttackerStats {
        AttackerStats {
            physical_mult: d.physical_mult,
            elemental_mult: d.elemental_mult,
            knockback_dealt: d.knockback_dealt,
            weaken,
        }
    }
}

/// The defender's side of the pipeline at the moment of the hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefenderStats {
    pub armour_class: ArmourClass,
    pub aspects: Aspects,
    pub armour: f32,
    pub ward: f32,
    pub evasion: f32,
    /// `Fortify` magnitude, 0 when absent.
    pub fortify: f32,
    pub evading: bool,
    /// `Expose` on the defender: attackers bypass armour.
    pub exposed: bool,
    /// Block mitigation when the defender guards toward the attacker and the guard applies
    /// to this packet (melee, or a projectile against a shield), else `None`.
    pub block: Option<f32>,
}

/// MATRIX.md 7 without the gear factor (Phase 5). Never returns less than 1.
pub fn resolve_damage(packet: &DamagePacket, a: &AttackerStats, d: &DefenderStats) -> i32 {
    let bypass = if d.exposed {
        packet.bypass.union(Bypass::ARMOR)
    } else {
        packet.bypass
    };
    let mut x = packet.amount as f32 * (1.0 - a.weaken).max(0.0);
    match packet.dtype.kind() {
        Some(kind) => {
            x *= a.physical_mult * kind_mult(kind, d.armour_class);
            if !bypass.contains(Bypass::ARMOR) {
                x *= 1.0 - d.armour;
            }
        }
        None => {
            let element = packet
                .dtype
                .element()
                .expect("non-physical types are elements");
            x *= a.elemental_mult * d.aspects.mult_against(element);
            if !bypass.contains(Bypass::MAGIC_SHIELD) {
                x *= 1.0 - d.ward;
            }
        }
    }
    if d.fortify > 0.0 && !bypass.contains(Bypass::MAGIC_SHIELD) {
        x *= 1.0 - d.fortify;
    }
    if d.evading && !bypass.contains(Bypass::EVASION) {
        x *= 1.0 - d.evasion;
    }
    if let Some(m) = d.block {
        x *= 1.0 - m;
    }
    (x + 0.5).floor().max(1.0) as i32
}

impl DamageType {
    /// The physical kind, or `None` for elements.
    pub const fn kind(self) -> Option<Kind> {
        match self {
            DamageType::Slash => Some(Kind::Slash),
            DamageType::Pierce => Some(Kind::Pierce),
            DamageType::Blunt => Some(Kind::Blunt),
            _ => None,
        }
    }

    pub const fn element(self) -> Option<Element> {
        match self {
            DamageType::Flame => Some(Element::Flame),
            DamageType::Shadow => Some(Element::Shadow),
            DamageType::Storm => Some(Element::Storm),
            DamageType::Frost => Some(Element::Frost),
            DamageType::Stone => Some(Element::Stone),
            _ => None,
        }
    }

    pub const fn is_physical(self) -> bool {
        self.kind().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pentagram_is_a_balanced_tournament() {
        for a in Element::ALL {
            let beats = Element::ALL
                .iter()
                .filter(|&&d| element_mult(a, d) == 2.0)
                .count();
            let loses = Element::ALL
                .iter()
                .filter(|&&d| d != a && element_mult(a, d) == 0.5)
                .count();
            assert_eq!((beats, loses), (2, 2), "{a:?}");
            assert_eq!(element_mult(a, a), 0.5);
            // Antisymmetric: if a beats d then d loses to a.
            for d in Element::ALL {
                if d != a {
                    assert_eq!(element_mult(a, d) * element_mult(d, a), 1.0, "{a:?} {d:?}");
                }
            }
        }
        // The readings of MATRIX.md 5.
        assert_eq!(element_mult(Element::Flame, Element::Shadow), 2.0);
        assert_eq!(element_mult(Element::Flame, Element::Frost), 2.0);
        assert_eq!(element_mult(Element::Stone, Element::Flame), 2.0);
        assert_eq!(element_mult(Element::Stone, Element::Storm), 2.0);
        assert_eq!(element_mult(Element::Frost, Element::Stone), 2.0);
        assert_eq!(element_mult(Element::Frost, Element::Shadow), 2.0);
    }

    #[test]
    fn dual_aspects_stack_to_four_and_a_quarter() {
        let shadow_frost = Aspects::two(Element::Shadow, Element::Frost);
        assert_eq!(shadow_frost.mult_against(Element::Flame), 4.0);
        let storm_stone = Aspects::two(Element::Storm, Element::Stone);
        assert_eq!(storm_stone.mult_against(Element::Flame), 0.25);
        // MATRIX.md 5: pairs two apart have one 4x hole and two 0.25x walls; adjacent pairs
        // have no hole and one wall.
        for a in Element::ALL {
            for b in Element::ALL {
                if a < b {
                    let pair = Aspects::two(a, b);
                    let holes = Element::ALL
                        .iter()
                        .filter(|&&e| pair.mult_against(e) == 4.0)
                        .count();
                    let walls = Element::ALL
                        .iter()
                        .filter(|&&e| pair.mult_against(e) == 0.25)
                        .count();
                    let dist = (b as u8 - a as u8).min(5 - (b as u8 - a as u8));
                    let expect = if dist == 2 { (1, 2) } else { (0, 1) };
                    assert_eq!((holes, walls), expect, "{a:?}+{b:?}");
                }
            }
        }
        assert_eq!(Aspects::NONE.mult_against(Element::Flame), 1.0);
        assert_eq!(Aspects::two(Element::Flame, Element::Flame).count(), 1);
    }

    #[test]
    fn derived_bands_match_the_document() {
        let lo = Derived::compute(
            Attributes::flat(5),
            ArchetypeFrame::Striker,
            ArmourClass::Cloth,
        );
        let hi = Derived::compute(
            Attributes::flat(20),
            ArchetypeFrame::Striker,
            ArmourClass::Cloth,
        );
        assert_eq!((lo.health, hi.health), (95, 140));
        assert_eq!((lo.stamina, hi.stamina), (80.0, 140.0));
        assert!((lo.max_speed - 300.0).abs() < 1e-3 && (hi.max_speed - 330.0).abs() < 1e-3);
        assert!((lo.physical_mult - 0.9).abs() < 1e-6 && (hi.physical_mult - 1.2).abs() < 1e-6);
        assert!((lo.armour - 0.10).abs() < 1e-6 && (hi.armour - 0.25).abs() < 1e-6);
        let plate = Derived::compute(
            Attributes::flat(20),
            ArchetypeFrame::Colossus,
            ArmourClass::Plate,
        );
        assert!((plate.max_speed - 330.0 * 0.92 * 0.90).abs() < 1e-3);
        assert!((plate.armour - 0.30).abs() < 1e-6);
        assert!((plate.knockback_taken - 1.0 / 1.5).abs() < 1e-6);
        assert_eq!(Attributes::new(20, 20, 10, 5, 5).cost(), 35);
        assert!(!Attributes::new(4, 20, 10, 5, 5).in_range());
    }

    fn packet(amount: u16, dtype: DamageType, bypass: Bypass) -> DamagePacket {
        DamagePacket {
            amount,
            dtype,
            bypass,
            knockback: 0.0,
            stagger: 0,
        }
    }

    fn defender(class: ArmourClass, aspects: Aspects) -> DefenderStats {
        DefenderStats {
            armour_class: class,
            aspects,
            armour: 0.2,
            ward: 0.2,
            evasion: 0.2,
            fortify: 0.0,
            evading: false,
            exposed: false,
            block: None,
        }
    }

    #[test]
    fn pipeline_follows_the_document() {
        let a = AttackerStats {
            physical_mult: 1.1,
            elemental_mult: 1.2,
            knockback_dealt: 1.0,
            weaken: 0.0,
        };
        // Slash into plate: 100 * 1.1 * 0.5 * (1 - 0.2) = 44.
        let d = defender(ArmourClass::Plate, Aspects::one(Element::Stone));
        assert_eq!(
            resolve_damage(&packet(100, DamageType::Slash, Bypass::NONE), &a, &d),
            44
        );
        // Armour bypass skips the mitigation: 55.
        assert_eq!(
            resolve_damage(&packet(100, DamageType::Slash, Bypass::ARMOR), &a, &d),
            55
        );
        // Frost into Stone (2x) ignores the armour class and armour, pays the ward:
        // 100 * 1.2 * 2 * 0.8 = 192.
        assert_eq!(
            resolve_damage(&packet(100, DamageType::Frost, Bypass::NONE), &a, &d),
            192
        );
        // Flame into Stone is 0.5: 48.
        assert_eq!(
            resolve_damage(&packet(100, DamageType::Flame, Bypass::NONE), &a, &d),
            48
        );
        // Fortify and evasion stack multiplicatively, blunt ignores the bubble.
        let mut d2 = d;
        d2.fortify = 0.5;
        d2.evading = true;
        assert_eq!(
            resolve_damage(&packet(100, DamageType::Frost, Bypass::NONE), &a, &d2),
            77
        );
        assert_eq!(
            resolve_damage(
                &packet(100, DamageType::Blunt, Bypass::MAGIC_SHIELD),
                &a,
                &d2
            ),
            88
        );
        // Expose grants armour bypass; a block removes its mitigation; never below 1.
        let mut d3 = d;
        d3.exposed = true;
        d3.block = Some(0.8);
        assert_eq!(
            resolve_damage(&packet(100, DamageType::Slash, Bypass::NONE), &a, &d3),
            11
        );
        assert_eq!(
            resolve_damage(&packet(1, DamageType::Slash, Bypass::NONE), &a, &d3),
            1
        );
        // Weaken on the attacker.
        let weak = AttackerStats { weaken: 0.3, ..a };
        assert_eq!(
            resolve_damage(&packet(100, DamageType::Slash, Bypass::NONE), &weak, &d),
            31
        );
    }
}
