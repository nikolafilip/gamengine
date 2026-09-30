//! The collision contract between movers (gm-core) and worlds (gm-bsp, test worlds).
//!
//! Semantics follow Quake's `SV_Move`/`PM_PlayerMove`: a hull is swept from `start` to `end`;
//! the result says how far it got and what it hit. A start inside solid yields `fraction == 0`
//! and `end == start`.

use glam::Vec3;

/// Collision hulls. Discriminants are the Quake BSP hull indices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Hull {
    /// Zero-size hull: projectiles, view traces, line of sight.
    Point = 0,
    /// Player hull, 32 x 32 x 56 units. Every player archetype uses this for world collision
    /// (hitbox capsules differ, see `vocab::ArchetypeFrame`).
    #[default]
    Player = 1,
    /// Large hull, 64 x 64 x 88 units: big monsters and hired colossi.
    Large = 2,
}

impl Hull {
    pub const fn mins(self) -> Vec3 {
        match self {
            Hull::Point => Vec3::ZERO,
            Hull::Player => Vec3::new(-16.0, -16.0, -24.0),
            Hull::Large => Vec3::new(-32.0, -32.0, -24.0),
        }
    }

    pub const fn maxs(self) -> Vec3 {
        match self {
            Hull::Point => Vec3::ZERO,
            Hull::Player => Vec3::new(16.0, 16.0, 32.0),
            Hull::Large => Vec3::new(32.0, 32.0, 64.0),
        }
    }

    /// Eye height above the hull origin (Quake's `view_ofs`).
    pub const fn eye_height(self) -> f32 {
        match self {
            Hull::Point => 0.0,
            Hull::Player => 22.0,
            Hull::Large => 44.0,
        }
    }
}

/// Leaf contents, Quake numbering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Contents {
    #[default]
    Empty = -1,
    Solid = -2,
    Water = -3,
    Slime = -4,
    Lava = -5,
    Sky = -6,
}

impl Contents {
    pub const fn from_i32(v: i32) -> Option<Contents> {
        match v {
            -1 => Some(Contents::Empty),
            -2 => Some(Contents::Solid),
            -3 => Some(Contents::Water),
            -4 => Some(Contents::Slime),
            -5 => Some(Contents::Lava),
            -6 => Some(Contents::Sky),
            _ => None,
        }
    }

    pub const fn is_liquid(self) -> bool {
        matches!(self, Contents::Water | Contents::Slime | Contents::Lava)
    }
}

/// Result of sweeping a hull from `start` to `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trace {
    /// Fraction of the requested movement completed before the hit, in `0.0..=1.0`.
    pub fraction: f32,
    /// Where the hull ended up. Pulled back by a small epsilon from the hit plane so the next
    /// trace starts in open space.
    pub end: Vec3,
    /// Unit normal of the plane that stopped the move; zero when nothing was hit.
    pub plane_normal: Vec3,
    /// Plane distance (`dot(normal, p) == dist` on the plane).
    pub plane_dist: f32,
    /// The start position was inside solid.
    pub start_solid: bool,
    /// The whole sweep was inside solid.
    pub all_solid: bool,
    /// Contents at the end position.
    pub contents: Contents,
}

impl Trace {
    /// A sweep that met nothing.
    pub fn clear(_start: Vec3, end: Vec3) -> Trace {
        Trace {
            fraction: 1.0,
            end,
            plane_normal: Vec3::ZERO,
            plane_dist: 0.0,
            start_solid: false,
            all_solid: false,
            contents: Contents::Empty,
        }
    }

    pub fn hit(&self) -> bool {
        self.fraction < 1.0
    }
}

/// Anything a hull can be swept through: the BSP world, a test world, later a world plus
/// brush entities.
pub trait CollisionWorld {
    /// Sweep `hull` from `start` to `end`.
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace;

    /// Contents of the leaf containing `point` for the given hull.
    fn point_contents(&self, hull: Hull, point: Vec3) -> Contents {
        let t = self.trace(hull, point, point);
        if t.start_solid {
            Contents::Solid
        } else {
            t.contents
        }
    }
}

impl<W: CollisionWorld + ?Sized> CollisionWorld for &W {
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        (**self).trace(hull, start, end)
    }
}
