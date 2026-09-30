//! Axis-aligned box sweeps and the composite collision world: the BSP plus the boxes of other
//! movers (PROTOCOL.md 7.5). Semantics match the BSP hull tracer so movement code cannot tell
//! a player apart from a wall.

use glam::Vec3;

use crate::trace::{CollisionWorld, Contents, Hull, Trace};

/// Hits stop this far short of the surface so the next trace starts in open space.
pub const DIST_EPSILON: f32 = 0.03125;

/// A solid axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub mins: Vec3,
    pub maxs: Vec3,
}

impl Aabb {
    pub fn new(mins: Vec3, maxs: Vec3) -> Aabb {
        Aabb { mins, maxs }
    }

    /// The box a hull occupies at `origin`.
    pub fn around(origin: Vec3, hull: Hull) -> Aabb {
        Aabb {
            mins: origin + hull.mins(),
            maxs: origin + hull.maxs(),
        }
    }

    pub fn overlaps(&self, other: &Aabb) -> bool {
        (0..3).all(|a| self.mins[a] < other.maxs[a] && self.maxs[a] > other.mins[a])
    }

    pub fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|a| p[a] > self.mins[a] && p[a] < self.maxs[a])
    }

    pub fn center(&self) -> Vec3 {
        (self.mins + self.maxs) * 0.5
    }
}

/// Sweep `hull` from `start` to `end` against one solid box (Minkowski expansion plus a slab
/// test). A start inside the expanded box reports `start_solid`; a hit stops `DIST_EPSILON`
/// short, like the BSP tracer.
pub fn sweep_box(hull: Hull, start: Vec3, end: Vec3, solid: &Aabb) -> Trace {
    let mut tr = Trace::clear(start, end);
    let emin = solid.mins - hull.maxs();
    let emax = solid.maxs - hull.mins();
    let expanded = Aabb::new(emin, emax);
    if expanded.contains(start) {
        tr.start_solid = true;
        tr.all_solid = expanded.contains(end);
        tr.fraction = 0.0;
        tr.end = start;
        tr.contents = Contents::Solid;
        return tr;
    }
    let delta = end - start;
    let mut tmin = 0.0f32;
    let mut tmax = 1.0f32;
    let mut hit: Option<Vec3> = None;
    for a in 0..3 {
        let (s, d) = (start[a], delta[a]);
        if d.abs() < 1e-9 {
            if s <= emin[a] || s >= emax[a] {
                return tr;
            }
            continue;
        }
        let inv = 1.0 / d;
        let (mut t0, mut t1) = ((emin[a] - s) * inv, (emax[a] - s) * inv);
        let mut normal = Vec3::ZERO;
        normal[a] = -1.0;
        if t0 > t1 {
            core::mem::swap(&mut t0, &mut t1);
            normal[a] = 1.0;
        }
        // `>=` so a hull resting on a surface and moving into it reports a fraction-0 hit with
        // that surface's normal.
        if t0 >= tmin {
            tmin = t0;
            hit = Some(normal);
        }
        tmax = tmax.min(t1);
        if tmin > tmax {
            return tr;
        }
    }
    let Some(normal) = hit else {
        return tr;
    };
    if tmin > 1.0 {
        return tr;
    }
    let len = delta.length();
    let frac = if len > 0.0 {
        ((tmin * len) - DIST_EPSILON).max(0.0) / len
    } else {
        0.0
    };
    tr.fraction = frac;
    tr.end = start + delta * frac;
    tr.plane_normal = normal;
    tr.plane_dist = normal.dot(tr.end);
    tr.contents = Contents::Empty;
    tr
}

/// Nearest hit among several boxes.
pub fn sweep_boxes<'a>(
    hull: Hull,
    start: Vec3,
    end: Vec3,
    boxes: impl IntoIterator<Item = &'a Aabb>,
) -> Trace {
    let mut best = Trace::clear(start, end);
    for b in boxes {
        let t = sweep_box(hull, start, end, b);
        best = nearer(best, t);
    }
    best
}

/// Merge two traces of the same sweep: solid starts win, then the shorter fraction.
pub fn nearer(a: Trace, b: Trace) -> Trace {
    if b.start_solid {
        return Trace {
            start_solid: true,
            all_solid: a.all_solid || b.all_solid,
            ..b
        };
    }
    if a.start_solid {
        return a;
    }
    if b.fraction < a.fraction { b } else { a }
}

/// A world plus the boxes of other movers. Movement against players slides and steps exactly as
/// against walls (PLAN.md 2.4, PROTOCOL.md 7.5).
pub struct Composite<'a, W: CollisionWorld + ?Sized> {
    pub world: &'a W,
    pub solids: &'a [Aabb],
}

impl<W: CollisionWorld + ?Sized> CollisionWorld for Composite<'_, W> {
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        let world = self.world.trace(hull, start, end);
        if self.solids.is_empty() {
            return world;
        }
        let boxes = sweep_boxes(hull, start, end, self.solids);
        nearer(world, boxes)
    }
}

/// A world plus the boxes of every mover, tagged by entity id, with one id ignored (the mover
/// itself). The server builds the list once per tick and shares it across all players.
pub struct EntityWorld<'a, W: CollisionWorld + ?Sized> {
    pub world: &'a W,
    pub solids: &'a [(u32, Aabb)],
    pub ignore: u32,
}

impl<W: CollisionWorld + ?Sized> CollisionWorld for EntityWorld<'_, W> {
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        let world = self.world.trace(hull, start, end);
        let boxes = sweep_boxes(
            hull,
            start,
            end,
            self.solids
                .iter()
                .filter(|(id, _)| *id != self.ignore)
                .map(|(_, b)| b),
        );
        nearer(world, boxes)
    }
}

/// A world made only of boxes. Used by tests in several crates.
#[derive(Clone, Debug, Default)]
pub struct BoxWorld {
    pub solids: Vec<Aabb>,
}

impl BoxWorld {
    /// A flat floor at z = 0 extending 4096 units in every direction.
    pub fn floor() -> BoxWorld {
        BoxWorld {
            solids: vec![Aabb::new(
                Vec3::new(-4096.0, -4096.0, -64.0),
                Vec3::new(4096.0, 4096.0, 0.0),
            )],
        }
    }

    pub fn push(&mut self, mins: Vec3, maxs: Vec3) {
        self.solids.push(Aabb::new(mins, maxs));
    }
}

impl CollisionWorld for BoxWorld {
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        sweep_boxes(hull, start, end, &self.solids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_sweep_hits_the_near_face() {
        let solid = Aabb::new(Vec3::new(10.0, -10.0, -10.0), Vec3::new(20.0, 10.0, 10.0));
        let t = sweep_box(Hull::Point, Vec3::ZERO, Vec3::new(40.0, 0.0, 0.0), &solid);
        assert!(t.hit());
        assert!(
            (t.end.x - (10.0 - DIST_EPSILON)).abs() < 1e-4,
            "{:?}",
            t.end
        );
        assert_eq!(t.plane_normal, Vec3::new(-1.0, 0.0, 0.0));
        let miss = sweep_box(Hull::Point, Vec3::ZERO, Vec3::new(0.0, 40.0, 0.0), &solid);
        assert!(!miss.hit());
    }

    #[test]
    fn player_hull_is_expanded() {
        let solid = Aabb::new(Vec3::new(100.0, -10.0, 0.0), Vec3::new(120.0, 10.0, 100.0));
        let t = sweep_box(
            Hull::Player,
            Vec3::new(0.0, 0.0, 24.0),
            Vec3::new(200.0, 0.0, 24.0),
            &solid,
        );
        // Stops when the hull's +x face (origin + 16) reaches x = 100.
        assert!(
            (t.end.x - (84.0 - DIST_EPSILON)).abs() < 1e-3,
            "{:?}",
            t.end
        );
    }

    #[test]
    fn starting_inside_is_solid() {
        let solid = Aabb::new(Vec3::splat(-10.0), Vec3::splat(10.0));
        let t = sweep_box(Hull::Point, Vec3::ZERO, Vec3::new(50.0, 0.0, 0.0), &solid);
        assert!(t.start_solid && !t.all_solid);
        assert_eq!(t.fraction, 0.0);
        let t = sweep_box(Hull::Point, Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), &solid);
        assert!(t.all_solid);
    }

    #[test]
    fn composite_reports_the_nearest_of_world_and_boxes() {
        let world = BoxWorld::floor();
        let other = [Aabb::around(Vec3::new(64.0, 0.0, 24.0), Hull::Player)];
        let c = Composite {
            world: &world,
            solids: &other,
        };
        let t = c.trace(
            Hull::Player,
            Vec3::new(0.0, 0.0, 24.0),
            Vec3::new(200.0, 0.0, 24.0),
        );
        // Two 16-unit half-widths: the moving hull stops at x = 64 - 32 = 32.
        assert!(
            (t.end.x - (32.0 - DIST_EPSILON)).abs() < 1e-3,
            "{:?}",
            t.end
        );
        let down = c.trace(
            Hull::Player,
            Vec3::new(0.0, 0.0, 24.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        assert_eq!(down.plane_normal, Vec3::Z);
    }
}
