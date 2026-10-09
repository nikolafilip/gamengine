//! What a mind is shown (COMPANIONS.md 2.2): bodies, areas, the map, and its own state. The
//! zone fills these from its simulation, a headless client from its snapshots; a mind cannot
//! tell which, and gets nothing a client in its place would not have.

use glam::Vec3;
use gm_core::build::{ContentPack, Sheet};
use gm_core::matrix::ArmourClass;
use gm_core::sim::{Mover, anim};
use gm_core::tick::Tick;
use gm_core::trace::{CollisionWorld, Hull};
use gm_core::vocab::{ArchetypeFrame, EntityId, Status};

use crate::nav::NavGrid;

/// Another body, as seen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub id: EntityId,
    /// Hull origin.
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub frame: ArchetypeFrame,
    /// Armour class: with the frame, what a body can take (both are on the wire).
    pub armour: ArmourClass,
    pub team: u8,
    pub party: u32,
    pub alive: bool,
    /// `gm_core::sim::anim` state: a windup is visible.
    pub anim: u8,
    /// Crouched (MODES.md 3.5): the body and its hitbox are `CROUCH_DROP` lower.
    pub crouched: bool,
    /// Status mask, a bit per `Status` index (the aura).
    pub status: u32,
    /// Health in per mille of its maximum, for bodies whose health the wire shows (the own
    /// party and creatures).
    pub health: Option<u16>,
    /// The creature definition it is an instance of.
    pub creature: Option<u16>,
    /// Stealth radius; infinite when not stealthed.
    pub stealth: f32,
}

impl Body {
    /// The middle of the hitbox, give or take: what a mind looks at and shoots at.
    pub fn centre(&self) -> Vec3 {
        let drop = if self.crouched {
            gm_core::sim::CROUCH_DROP * 0.5
        } else {
            0.0
        };
        self.pos + Vec3::Z * (4.0 - drop)
    }

    pub fn feet(&self) -> Vec3 {
        self.pos + Vec3::Z * Hull::Player.mins().z
    }

    pub fn radius(&self) -> f32 {
        self.frame.capsule().0
    }

    /// The body's health as a member of `party` may know it: its own party's and a
    /// creature's, which is what the wire shows; nobody else's.
    pub fn health_for(&self, party: u32) -> Option<u16> {
        if self.creature.is_some() || (party != 0 && self.party == party) {
            self.health
        } else {
            None
        }
    }

    pub fn has(&self, s: Status) -> bool {
        self.status & (1 << s.index()) != 0
    }

    /// A colossus in mail or plate: the body a squad sends in first.
    pub fn heavy(&self) -> bool {
        self.frame == ArchetypeFrame::Colossus
            && matches!(self.armour, ArmourClass::Mail | ArmourClass::Plate)
    }

    /// Winding up or swinging: the part of an attack that can be seen coming.
    pub fn attacking(&self) -> bool {
        matches!(self.anim, anim::WINDUP | anim::SWING | anim::CAST)
    }

    pub fn guarding(&self) -> bool {
        self.anim == anim::GUARD
    }

    /// Unit facing on the ground plane.
    pub fn facing(&self) -> Vec3 {
        let (s, c) = self.yaw.to_radians().sin_cos();
        Vec3::new(c, s, 0.0)
    }
}

/// An area effect, as seen: a circle on the floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaSight {
    pub id: EntityId,
    pub pos: Vec3,
    pub radius: f32,
    pub owner: EntityId,
    /// It will hurt who stands in it (a mind that only has the wire assumes so of a
    /// stranger's area).
    pub harmful: bool,
    /// It never touches its owner.
    pub spares_owner: bool,
}

/// Everything one mind is shown for one frame.
pub struct Senses<'a> {
    /// The frame tick this frame will run at (the body's own clock).
    pub tick: Tick,
    pub hz: u32,
    pub id: EntityId,
    pub me: &'a Mover,
    pub sheet: &'a Sheet,
    pub health: i32,
    pub team: u8,
    pub party: u32,
    /// Every other body the mind may know about; it still has to see them (`sees`).
    pub bodies: &'a [Body],
    pub areas: &'a [AreaSight],
    pub world: &'a dyn CollisionWorld,
    pub nav: &'a NavGrid,
    /// Public knowledge: the zone's content (a creature's kit is no secret).
    pub pack: &'a ContentPack,
}

impl Senses<'_> {
    pub fn pos(&self) -> Vec3 {
        self.me.mv.origin
    }

    pub fn eye(&self) -> Vec3 {
        self.me.eye()
    }

    pub fn dt(&self) -> f32 {
        1.0 / self.hz as f32
    }

    /// Ticks in `ms` at this zone's rate.
    pub fn ticks(&self, ms: u32) -> Tick {
        (ms * self.hz).div_ceil(1000)
    }

    pub fn body(&self, id: EntityId) -> Option<&Body> {
        self.bodies.iter().find(|b| b.id == id)
    }

    /// Ground distance to a body.
    pub fn dist(&self, b: &Body) -> f32 {
        (b.pos - self.pos()).truncate().length()
    }

    /// Line of sight to a body: within its stealth radius and nothing solid between the
    /// mind's eyes and the middle of the body. One point trace.
    pub fn sees(&self, b: &Body) -> bool {
        if (b.pos - self.pos()).length() > b.stealth {
            return false;
        }
        self.world
            .trace(Hull::Point, self.eye(), b.centre())
            .fraction
            >= 1.0
    }

    pub fn health_frac(&self) -> f32 {
        self.health as f32 / self.sheet.derived.health.max(1) as f32
    }

    /// Who taunted this mind's body (MATRIX.md 8), while it lasts and the taunter is
    /// alive and known: the body it must fight.
    pub fn taunted_by(&self) -> Option<EntityId> {
        self.me
            .statuses
            .taunted_by()
            .filter(|&id| self.body(id).is_some_and(|b| b.alive))
    }

    /// The ability in kit slot `slot` is off cooldown and affordable.
    pub fn ready(&self, slot: u8) -> bool {
        let i = slot as usize;
        let Some(ab) = self.sheet.kit.abilities.get(i) else {
            return false;
        };
        gm_core::sim::tick_delta(self.tick, self.me.cooldowns[i]) >= 0
            && self.me.stamina >= ab.cost.stamina as f32
            && self.me.focus >= ab.cost.focus as f32
            && !(self.sheet.kit.elemental[i] && self.me.statuses.silenced())
    }
}
