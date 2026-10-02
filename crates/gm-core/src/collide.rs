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

/// Whether two bodies are in each other, by more than a unit on every axis (bodies in
/// contact are not).
pub fn entangled(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|k| a.mins[k] + 1.0 < b.maxs[k] && a.maxs[k] - 1.0 > b.mins[k])
}

/// Nearest hit among the boxes of other bodies, for a mover whose own box is `own` as its
/// step begins. A body the mover is already inside does not hold it: two bodies that ended
/// up in each other (a spawn point that overflowed, a blink into a crowd) can walk apart
/// instead of being stuck for good. The test costs nothing until a sweep starts in a box.
pub fn sweep_bodies<'a>(
    hull: Hull,
    start: Vec3,
    end: Vec3,
    boxes: impl IntoIterator<Item = &'a Aabb>,
    own: Option<Aabb>,
) -> Trace {
    let mut best = Trace::clear(start, end);
    for b in boxes {
        let t = sweep_box(hull, start, end, b);
        if t.start_solid && own.is_some_and(|own| entangled(&own, b)) {
            continue;
        }
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
    /// The mover's own box as the step begins ([`sweep_bodies`]).
    pub own: Option<Aabb>,
}

impl<W: CollisionWorld + ?Sized> CollisionWorld for Composite<'_, W> {
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        let world = self.world.trace(hull, start, end);
        if self.solids.is_empty() {
            return world;
        }
        let boxes = sweep_bodies(hull, start, end, self.solids, self.own);
        nearer(world, boxes)
    }
}

/// Side of a [`BodyGrid`] cell: a few bodies wide.
const GRID_CELL: f32 = 128.0;
/// A sweep whose bounds cover more cells than this walks the whole list instead.
const GRID_MAX_CELLS: i32 = 36;
/// Candidates one sweep can collect before it walks the whole list instead.
const GRID_MAX_CANDIDATES: usize = 96;

/// A coarse grid over the bodies of a tick, on the ground plane: which boxes a sweep can
/// possibly touch. With two hundred bodies in a hall, a mover's sweeps meet a handful of
/// them, not all; the answer is the same as walking the whole list, in the same order.
#[derive(Clone, Debug, Default)]
pub struct BodyGrid {
    cells:
        std::collections::HashMap<(i32, i32), Vec<u16>, std::hash::BuildHasherDefault<CellHasher>>,
}

/// Cell coordinates are small integers: mixing them is all the hashing they need.
#[derive(Clone, Copy, Debug, Default)]
pub struct CellHasher(u64);

impl std::hash::Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn write_i32(&mut self, v: i32) {
        self.0 = (self.0 ^ v as u32 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

impl BodyGrid {
    fn span(mins: Vec3, maxs: Vec3) -> (i32, i32, i32, i32) {
        (
            (mins.x / GRID_CELL).floor() as i32,
            (mins.y / GRID_CELL).floor() as i32,
            (maxs.x / GRID_CELL).floor() as i32,
            (maxs.y / GRID_CELL).floor() as i32,
        )
    }

    /// Every box in every cell it reaches into. At most 65,535 boxes.
    pub fn build(solids: &[(u32, Aabb)]) -> BodyGrid {
        let mut grid = BodyGrid::default();
        for (i, (_, b)) in solids.iter().enumerate() {
            grid.insert(i as u16, b);
        }
        grid
    }

    fn insert(&mut self, index: u16, b: &Aabb) {
        let (x0, y0, x1, y1) = BodyGrid::span(b.mins, b.maxs);
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(index);
            }
        }
    }

    /// Box `index` moved from `old` to `new`.
    pub fn moved(&mut self, index: u16, old: &Aabb, new: &Aabb) {
        if BodyGrid::span(old.mins, old.maxs) == BodyGrid::span(new.mins, new.maxs) {
            return;
        }
        let (x0, y0, x1, y1) = BodyGrid::span(old.mins, old.maxs);
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(cell) = self.cells.get_mut(&(x, y)) {
                    cell.retain(|i| *i != index);
                }
            }
        }
        self.insert(index, new);
    }

    /// The boxes that reach into the region `mins..maxs`, ascending, into `out`; `false`
    /// when the region is too large or too crowded for the grid to help (walk them all).
    fn near(&self, mins: Vec3, maxs: Vec3, out: &mut Vec<u16>) -> bool {
        out.clear();
        let (x0, y0, x1, y1) = BodyGrid::span(mins, maxs);
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > GRID_MAX_CELLS {
            return false;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(cell) = self.cells.get(&(x, y)) {
                    out.extend_from_slice(cell);
                    if out.len() > GRID_MAX_CANDIDATES {
                        return false;
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        true
    }
}

/// A world plus the boxes of every mover, tagged by entity id, with one id ignored (the mover
/// itself). The server builds the list once per tick and shares it across all players.
pub struct EntityWorld<'a, W: CollisionWorld + ?Sized> {
    pub world: &'a W,
    pub solids: &'a [(u32, Aabb)],
    pub ignore: u32,
    /// As [`Composite::own`]: the mover's box as the step begins.
    pub own: Option<Aabb>,
    /// The grid over `solids`, when the caller keeps one.
    pub grid: Option<&'a BodyGrid>,
}

impl<W: CollisionWorld + ?Sized> CollisionWorld for EntityWorld<'_, W> {
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        let world = self.world.trace(hull, start, end);
        // Only the boxes that reach into the sweep's own bounds can meet it.
        let mut near = Vec::new();
        let margin = Vec3::splat(1.0);
        let listed = self.grid.is_some_and(|g| {
            g.near(
                start.min(end) + hull.mins() - margin,
                start.max(end) + hull.maxs() + margin,
                &mut near,
            )
        });
        let boxes = if listed {
            sweep_bodies(
                hull,
                start,
                end,
                near.iter()
                    .map(|&i| &self.solids[i as usize])
                    .filter(|(id, _)| *id != self.ignore)
                    .map(|(_, b)| b),
                self.own,
            )
        } else {
            sweep_bodies(
                hull,
                start,
                end,
                self.solids
                    .iter()
                    .filter(|(id, _)| *id != self.ignore)
                    .map(|(_, b)| b),
                self.own,
            )
        };
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
    fn a_body_inside_another_can_walk_out_but_not_through_a_third() {
        let floor = BoxWorld::floor();
        let at = Vec3::new(0.0, 0.0, 24.0);
        // One body exactly where the mover stands, another 100 units ahead.
        let solids = [
            Aabb::around(at, Hull::Player),
            Aabb::around(at + Vec3::X * 100.0, Hull::Player),
        ];
        // Without the mover's own box the old rule holds: it is stuck.
        let stuck = Composite {
            world: &floor,
            solids: &solids,
            own: None,
        };
        assert!(
            stuck
                .trace(Hull::Player, at, at + Vec3::X * 200.0)
                .start_solid
        );
        let mine = Aabb::around(at, Hull::Player);
        let own = Some(mine);
        let world = Composite {
            world: &floor,
            solids: &solids,
            own,
        };
        let t = world.trace(Hull::Player, at, at + Vec3::X * 200.0);
        assert!(!t.start_solid, "held by the body it stands in");
        // Out of the first, stopped by the second: 100 minus one hull width.
        assert!((t.end.x - 68.0).abs() < 0.1, "{:?}", t.end);
        // The same through the server's wrapper.
        let tagged = [(1, solids[0]), (2, solids[1]), (3, solids[0])];
        let world = EntityWorld {
            world: &floor,
            solids: &tagged,
            ignore: 3,
            own,
            grid: None,
        };
        // Bodies that only touch are not in each other.
        let beside = Aabb::around(at + Vec3::X * 32.0, Hull::Player);
        assert!(!entangled(&mine, &beside));
        assert!(entangled(&mine, &solids[0]));
        let t = world.trace(Hull::Player, at, at + Vec3::X * 200.0);
        assert!(
            !t.start_solid && (t.end.x - 68.0).abs() < 0.1,
            "{:?}",
            t.end
        );
        // Walls still hold: a start inside the world is solid.
        let t = world.trace(Hull::Player, at - Vec3::Z * 40.0, at);
        assert!(t.start_solid);
    }

    #[test]
    fn the_grid_answers_like_the_whole_list() {
        // A crowd of boxes on a floor; sweeps of every kind give the same trace with the
        // grid as without, bit for bit, also after boxes move.
        let floor = BoxWorld::floor();
        let mut rng = crate::rng::Rng::new(77);
        let mut solids: Vec<(u32, Aabb)> = (0..220u32)
            .map(|i| {
                let at = Vec3::new(
                    rng.range_f32(-900.0, 900.0),
                    rng.range_f32(-600.0, 600.0),
                    24.0 + rng.range_f32(0.0, 40.0),
                );
                (i + 1, Aabb::around(at, Hull::Player))
            })
            .collect();
        let mut grid = BodyGrid::build(&solids);
        let mut compared = 0;
        let mut hits = 0;
        for round in 0..6 {
            for _ in 0..400 {
                let i = rng.below(solids.len() as u32) as usize;
                let from = solids[i].1.center();
                // Mostly short moves, some long, some straight down, some of no length.
                let reach = match rng.below(8) {
                    0 => 700.0,
                    1 => 0.0,
                    _ => 30.0,
                };
                let to = from
                    + Vec3::new(
                        rng.range_f32(-reach, reach),
                        rng.range_f32(-reach, reach),
                        if rng.below(5) == 0 { -34.0 } else { 0.0 },
                    );
                for own in [None, Some(solids[i].1)] {
                    for hull in [Hull::Player, Hull::Point] {
                        let whole = EntityWorld {
                            world: &floor,
                            solids: &solids,
                            ignore: solids[i].0,
                            own,
                            grid: None,
                        };
                        let gridded = EntityWorld {
                            grid: Some(&grid),
                            ..whole
                        };
                        let (a, b) = (whole.trace(hull, from, to), gridded.trace(hull, from, to));
                        assert_eq!(a, b, "round {round}: {from:?} to {to:?}");
                        compared += 1;
                        hits += (a.fraction < 1.0) as u32;
                    }
                }
            }
            // Everybody shuffles along; the grid follows.
            for (i, (_, b)) in solids.iter_mut().enumerate() {
                let step = Vec3::new(rng.range_f32(-90.0, 90.0), rng.range_f32(-90.0, 90.0), 0.0);
                let old = *b;
                *b = Aabb::new(old.mins + step, old.maxs + step);
                grid.moved(i as u16, &old, b);
            }
        }
        assert!(
            compared == 9600 && hits > 1000,
            "{compared} sweeps, {hits} hits"
        );
    }

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
            own: None,
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
