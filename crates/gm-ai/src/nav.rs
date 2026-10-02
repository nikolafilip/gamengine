//! Navigation (COMPANIONS.md 7): a grid of standable cells found by walking the player hull
//! over the map from its spawns, A* over the links, and the steering a mind does along a path.
//! Built from collision alone: any map that compiles is navigable, with no authoring.

use std::collections::{BinaryHeap, HashMap, VecDeque};

use glam::{Vec2, Vec3};
use gm_core::tick::Tick;
use gm_core::trace::{CollisionWorld, Contents, Hull};

/// Cell size: the width of the player hull.
pub const CELL: f32 = 32.0;
/// What movement steps up without jumping (`gm_core::movement`).
pub const STEP: f32 = 18.0;
/// The tallest drop a path may take (one way).
pub const MAX_DROP: f32 = 256.0;
/// Two floors over one cell are two nodes when they are at least this far apart.
const LAYER: f32 = 28.0;
const MAX_NODES: usize = 400_000;
const NONE: u32 = u32::MAX;
/// Surfaces steeper than this are not floors (`gm_core::movement`).
const GROUND_NORMAL_Z: f32 = 0.7;

/// The eight neighbours, east first, counter-clockwise.
const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

pub type NodeId = u32;

#[derive(Clone, Copy, Debug)]
struct Node {
    /// Hull origin of a body standing on the cell's centre.
    pos: Vec3,
    links: [NodeId; 8],
    /// Bit `i`: link `i` is a drop (one way down).
    drops: u8,
}

#[derive(Clone, Debug, Default)]
pub struct NavGrid {
    nodes: Vec<Node>,
    cells: HashMap<(i32, i32), Vec<NodeId>>,
    /// The strongly connected component of each node: within one, every node reaches every
    /// other. Components are numbered so that a component only leads to lower numbers.
    comp: Vec<u32>,
    /// Per component, the set of components it leads to (itself included), a bit each; empty
    /// when the map has more components than are worth a table.
    reach: Vec<Vec<u64>>,
}

/// More components than this are not tabulated; `path` then searches, up to its cap.
const MAX_TABLED_COMPONENTS: usize = 2048;
/// Nodes one path search may expand: no order and no stuck mind costs more than this.
const MAX_EXPANSIONS: usize = 200_000;

fn cell_of(p: Vec3) -> (i32, i32) {
    ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
}

fn centre(cell: (i32, i32)) -> Vec2 {
    Vec2::new((cell.0 as f32 + 0.5) * CELL, (cell.1 as f32 + 0.5) * CELL)
}

/// Where the hull comes to rest when let down from `from`: `None` in solid, over a pit
/// deeper than `reach`, or on a slope too steep to stand on.
fn settle(world: &dyn CollisionWorld, from: Vec3, reach: f32) -> Option<Vec3> {
    let tr = world.trace(Hull::Player, from, from - Vec3::Z * reach);
    if tr.start_solid || tr.fraction >= 1.0 || tr.plane_normal.z < GROUND_NORMAL_Z {
        return None;
    }
    Some(tr.end)
}

impl NavGrid {
    /// Flood the map from `seeds` (spawns, posts, anything known to be a place to stand).
    pub fn build(world: &dyn CollisionWorld, seeds: &[Vec3]) -> NavGrid {
        let mut grid = NavGrid::default();
        let mut queue: VecDeque<NodeId> = VecDeque::new();
        for &seed in seeds {
            // The seed's own cell centre, or a neighbouring one when the seed hugs a wall.
            let c = cell_of(seed);
            let mut cells = vec![c];
            cells.extend(DIRS.iter().map(|d| (c.0 + d.0, c.1 + d.1)));
            for cell in cells {
                let xy = centre(cell);
                let from = Vec3::new(xy.x, xy.y, seed.z + STEP);
                if world.point_contents(Hull::Player, from) != Contents::Empty {
                    continue;
                }
                if world.trace(Hull::Point, seed, from).fraction < 1.0 {
                    continue;
                }
                if let Some(pos) = settle(world, from, STEP + MAX_DROP) {
                    if grid.find(cell, pos.z).is_none() {
                        queue.push_back(grid.add(cell, pos));
                    }
                    break;
                }
            }
        }
        while let Some(id) = queue.pop_front() {
            if grid.nodes.len() >= MAX_NODES {
                break;
            }
            let a = grid.nodes[id as usize].pos;
            let cell = cell_of(a);
            // Lift a step once; every neighbour is tried from up there, as movement does.
            let lifted = world.trace(Hull::Player, a, a + Vec3::Z * STEP).end;
            for (i, d) in DIRS.iter().enumerate() {
                let to = (cell.0 + d.0, cell.1 + d.1);
                let xy = centre(to);
                let across = Vec3::new(xy.x, xy.y, lifted.z);
                if world.trace(Hull::Player, lifted, across).fraction < 1.0 {
                    continue;
                }
                let Some(b) = settle(world, across, (lifted.z - a.z) + STEP + MAX_DROP) else {
                    continue;
                };
                let drop = a.z - b.z > STEP + 0.5;
                let other = match grid.find(to, b.z) {
                    Some(n) => n,
                    None => {
                        let n = grid.add(to, b);
                        queue.push_back(n);
                        n
                    }
                };
                let node = &mut grid.nodes[id as usize];
                node.links[i] = other;
                if drop {
                    node.drops |= 1 << i;
                }
            }
        }
        grid.label();
        grid
    }

    /// Number the strongly connected components (Tarjan, without recursion) and tabulate
    /// which leads to which. A drop is one way, so "is there a way from a to b" is not
    /// "are they on the same floor"; with the table the answer costs two lookups, and no
    /// order to a place without a way and no mind that wants to go there starts a search.
    fn label(&mut self) {
        let n = self.nodes.len();
        const UNSEEN: u32 = u32::MAX;
        let mut index = vec![UNSEEN; n];
        let mut low = vec![0u32; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<u32> = Vec::new();
        let mut comp = vec![UNSEEN; n];
        let mut next_index = 0u32;
        let mut comps = 0u32;
        // (node, the next link to look at).
        let mut work: Vec<(u32, u8)> = Vec::new();
        for root in 0..n as u32 {
            if index[root as usize] != UNSEEN {
                continue;
            }
            work.push((root, 0));
            while let Some(&(v, i)) = work.last() {
                let vi = v as usize;
                if i == 0 {
                    index[vi] = next_index;
                    low[vi] = next_index;
                    next_index += 1;
                    stack.push(v);
                    on_stack[vi] = true;
                }
                if (i as usize) < 8 {
                    work.last_mut().expect("just read").1 += 1;
                    let w = self.nodes[vi].links[i as usize];
                    if w == NONE {
                        continue;
                    }
                    let wi = w as usize;
                    if index[wi] == UNSEEN {
                        work.push((w, 0));
                    } else if on_stack[wi] {
                        low[vi] = low[vi].min(index[wi]);
                    }
                    continue;
                }
                // Every link looked at: close the component if v is its root, and hand
                // the low link up.
                work.pop();
                if low[vi] == index[vi] {
                    while let Some(w) = stack.pop() {
                        on_stack[w as usize] = false;
                        comp[w as usize] = comps;
                        if w == v {
                            break;
                        }
                    }
                    comps += 1;
                }
                if let Some(&(parent, _)) = work.last() {
                    let pi = parent as usize;
                    low[pi] = low[pi].min(low[vi]);
                }
            }
        }
        self.comp = comp;
        self.reach.clear();
        let comps = comps as usize;
        if comps > MAX_TABLED_COMPONENTS {
            return;
        }
        // A component is closed after everything it leads to, so it only leads to lower
        // numbers: one pass in order fills the table.
        let words = comps.div_ceil(64);
        let mut edges: Vec<Vec<u32>> = vec![Vec::new(); comps];
        for (v, node) in self.nodes.iter().enumerate() {
            let c = self.comp[v];
            for &w in node.links.iter().filter(|&&w| w != NONE) {
                let d = self.comp[w as usize];
                if d != c && !edges[c as usize].contains(&d) {
                    edges[c as usize].push(d);
                }
            }
        }
        let mut reach: Vec<Vec<u64>> = Vec::with_capacity(comps);
        for (c, out) in edges.iter().enumerate() {
            let mut row = vec![0u64; words];
            row[c / 64] |= 1 << (c % 64);
            for &d in out {
                debug_assert!((d as usize) < c, "components are closed sinks first");
                for (a, b) in row.iter_mut().zip(&reach[d as usize]) {
                    *a |= *b;
                }
            }
            reach.push(row);
        }
        self.reach = reach;
    }

    /// Whether node `a` leads to node `b`: `None` when the map's components are not
    /// tabulated (then only a search can tell).
    pub fn leads(&self, a: NodeId, b: NodeId) -> Option<bool> {
        let (ca, cb) = (self.comp[a as usize], self.comp[b as usize]);
        if ca == cb {
            return Some(true);
        }
        let row = self.reach.get(ca as usize)?;
        Some(row[cb as usize / 64] & (1 << (cb % 64)) != 0)
    }

    fn add(&mut self, cell: (i32, i32), pos: Vec3) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            pos,
            links: [NONE; 8],
            drops: 0,
        });
        self.cells.entry(cell).or_default().push(id);
        id
    }

    /// The node of `cell` whose floor is within a layer of `z`.
    fn find(&self, cell: (i32, i32), z: f32) -> Option<NodeId> {
        self.cells.get(&cell)?.iter().copied().find(|&n| {
            let dz = self.nodes[n as usize].pos.z - z;
            dz.abs() < LAYER
        })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn pos(&self, node: NodeId) -> Vec3 {
        self.nodes[node as usize].pos
    }

    /// The node nearest to `p` among its cell and the cells around it (two rings), on a floor
    /// not more than a body's height away.
    pub fn nearest(&self, p: Vec3) -> Option<NodeId> {
        let c = cell_of(p);
        let mut best: Option<(f32, NodeId)> = None;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let Some(list) = self.cells.get(&(c.0 + dx, c.1 + dy)) else {
                    continue;
                };
                for &n in list {
                    let q = self.nodes[n as usize].pos;
                    if (q.z - p.z).abs() > 56.0 {
                        continue;
                    }
                    let d = (q - p).length_squared();
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, n));
                    }
                }
            }
        }
        best.map(|(_, n)| n)
    }

    /// Whether a body may walk the straight line from `a` to `b`: every cell under the line
    /// is standable at about the line's height, neighbours on it are linked without a drop,
    /// and the hull sweeps it without touching the world.
    pub fn walkable_line(&self, world: &dyn CollisionWorld, a: Vec3, b: Vec3) -> bool {
        let flat = (b - a).truncate();
        let len = flat.length();
        if len < 1.0 {
            return (a.z - b.z).abs() < LAYER;
        }
        let steps = (len / (CELL * 0.5)).ceil() as usize;
        let mut prev: Option<NodeId> = None;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = a.lerp(b, t);
            let Some(n) = self.find(cell_of(p), p.z) else {
                return false;
            };
            if let Some(q) = prev
                && q != n
            {
                let from = &self.nodes[q as usize];
                match from.links.iter().position(|&l| l == n) {
                    Some(i) if from.drops & (1 << i) == 0 => {}
                    _ => return false,
                }
            }
            prev = Some(n);
        }
        let lift = Vec3::Z;
        world.trace(Hull::Player, a + lift, b + lift).fraction >= 1.0
    }

    /// The shortest path from `from` to `to` as hull origins, both ends included; `None` when
    /// there is no way.
    pub fn path(&self, from: NodeId, to: NodeId) -> Option<Vec<Vec3>> {
        #[derive(PartialEq)]
        struct Open(f32, NodeId);
        impl Eq for Open {}
        impl Ord for Open {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                // The heap is a max-heap: the smallest estimate first.
                other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
            }
        }
        impl PartialOrd for Open {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }
        if from == to {
            return Some(vec![self.pos(from)]);
        }
        if self.leads(from, to) == Some(false) {
            return None;
        }
        let goal = self.pos(to);
        let mut expanded = 0usize;
        let h = |n: NodeId| {
            let d = (self.nodes[n as usize].pos - goal).abs();
            let (lo, hi) = (d.x.min(d.y), d.x.max(d.y));
            hi + lo * 0.41421354 + d.z
        };
        let mut cost: HashMap<NodeId, (f32, NodeId)> = HashMap::new();
        let mut open = BinaryHeap::new();
        cost.insert(from, (0.0, NONE));
        open.push(Open(h(from), from));
        while let Some(Open(estimate, n)) = open.pop() {
            if n == to {
                let mut out = vec![goal];
                let mut at = to;
                while let Some(&(_, parent)) = cost.get(&at) {
                    if parent == NONE {
                        break;
                    }
                    out.push(self.pos(parent));
                    at = parent;
                }
                out.reverse();
                return Some(out);
            }
            let g = cost[&n].0;
            if estimate > g + h(n) + 1e-3 {
                continue; // a stale entry
            }
            expanded += 1;
            if expanded > MAX_EXPANSIONS {
                return None;
            }
            let node = &self.nodes[n as usize];
            for (i, &next) in node.links.iter().enumerate() {
                if next == NONE {
                    continue;
                }
                let mut step = (self.nodes[next as usize].pos - node.pos).length();
                if node.drops & (1 << i) != 0 {
                    // A drop is a commitment: take it only when it saves a real detour.
                    step += 96.0;
                }
                let ng = g + step;
                if cost.get(&next).is_none_or(|&(old, _)| ng < old - 1e-3) {
                    cost.insert(next, (ng, n));
                    open.push(Open(ng + h(next), next));
                }
            }
        }
        None
    }

    /// The path between two points (snapped to their nearest nodes), ending at `to` itself.
    pub fn path_between(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let (a, b) = (self.nearest(from)?, self.nearest(to)?);
        let mut path = self.path(a, b)?;
        path.push(to);
        Some(path)
    }

    /// Whether `to` can be walked to from `from` at all (orders, COMPANIONS.md 5.3).
    pub fn reachable(&self, from: Vec3, to: Vec3) -> bool {
        match (self.nearest(from), self.nearest(to)) {
            (Some(a), Some(b)) => self.reachable_node(a, b),
            _ => false,
        }
    }

    /// Whether node `to` can be walked to from node `from`: two lookups where the map's
    /// components are tabulated, a (capped) search otherwise.
    pub fn reachable_node(&self, from: NodeId, to: NodeId) -> bool {
        match self.leads(from, to) {
            Some(known) => known,
            None => self.path(from, to).is_some(),
        }
    }
}

/// What a mind does with a path: where to walk right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Steer {
    pub toward: Vec3,
    /// Within `arrive` of the goal.
    pub arrived: bool,
    /// The grid knows no way there.
    pub blocked: bool,
}

/// One mind's route to its current goal.
#[derive(Clone, Debug, Default)]
pub struct Navigator {
    route: Vec<Vec3>,
    next: usize,
    goal: Vec3,
    direct: bool,
    planned: bool,
    blocked: bool,
    /// When the route was last looked at for a shortcut, and when it was planned.
    looked_at: Tick,
    /// Progress watch: the distance to the goal at a tick, to notice being stuck.
    watch: (Tick, f32),
    sidestep_until: Tick,
    sidestep: f32,
}

/// The goal moved this far: plan again.
const REPLAN_DIST: f32 = 64.0;
/// Nodes ahead a mind may cut straight to.
const LOOKAHEAD: usize = 6;

impl Navigator {
    pub fn clear(&mut self) {
        *self = Navigator::default();
    }

    /// Sideways wish (−1, 0, 1) while a sidestep around an obstacle runs.
    pub fn sidestep(&self, tick: Tick) -> f32 {
        if gm_core::sim::tick_delta(tick, self.sidestep_until) < 0 {
            self.sidestep
        } else {
            0.0
        }
    }

    /// Where to walk at `tick` to get from `pos` to `goal`. Plans when the goal moved, cuts
    /// corners where the line is walkable, and sidesteps when no progress is made.
    #[allow(clippy::too_many_arguments)]
    pub fn steer(
        &mut self,
        nav: &NavGrid,
        world: &dyn CollisionWorld,
        pos: Vec3,
        goal: Vec3,
        arrive: f32,
        tick: Tick,
        hz: u32,
    ) -> Steer {
        let dist = (goal - pos).truncate().length();
        if dist <= arrive && (goal.z - pos.z).abs() < 56.0 {
            self.watch = (tick, dist);
            return Steer {
                toward: goal,
                arrived: true,
                blocked: false,
            };
        }
        // Stuck: no closer than three quarters of a second ago. Step aside and plan again.
        if self.planned {
            let since = gm_core::sim::tick_delta(tick, self.watch.0);
            if since < 0 || since as u32 >= hz * 3 / 4 {
                if since >= 0 && self.watch.1 - dist < 8.0 {
                    self.sidestep_until = tick.wrapping_add(hz / 3);
                    self.sidestep = if (tick / hz.max(1)) & 1 == 0 {
                        1.0
                    } else {
                        -1.0
                    };
                    self.planned = false;
                }
                self.watch = (tick, dist);
            }
        }
        if !self.planned || (goal - self.goal).length() > REPLAN_DIST {
            self.planned = true;
            self.watch = (tick, dist);
            self.goal = goal;
            self.next = 0;
            self.looked_at = tick;
            self.direct = nav.walkable_line(world, pos, goal);
            self.blocked = false;
            if self.direct {
                self.route.clear();
            } else {
                match nav.path_between(pos, goal) {
                    Some(path) => self.route = path,
                    None => {
                        self.route.clear();
                        self.blocked = true;
                    }
                }
            }
        }
        if self.direct || self.blocked {
            return Steer {
                toward: goal,
                arrived: false,
                blocked: self.blocked,
            };
        }
        // Pass the nodes already reached.
        while self.next + 1 < self.route.len()
            && (self.route[self.next] - pos).truncate().length() < CELL * 0.6
        {
            self.next += 1;
        }
        // A few times a second, look for the farthest node ahead that can be walked to
        // straight; the last point of the route is the goal itself.
        if gm_core::sim::tick_delta(tick, self.looked_at) >= (hz / 8).max(1) as i32 {
            self.looked_at = tick;
            let last = (self.next + LOOKAHEAD).min(self.route.len() - 1);
            for k in (self.next + 1..=last).rev() {
                if nav.walkable_line(world, pos, self.route[k]) {
                    self.next = k;
                    break;
                }
            }
        }
        Steer {
            toward: self.route[self.next.min(self.route.len() - 1)],
            arrived: false,
            blocked: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::collide::BoxWorld;

    #[test]
    fn a_ledge_one_can_only_drop_from_is_reached_by_nobody_below() {
        // A floor, and a block 100 high with no step up to it: from its top a body can drop
        // (a one-way link), from below there is no way up. A second block stands apart,
        // out of the flood's reach from the floor, with its own seed.
        let mut w = BoxWorld::floor();
        w.push(
            Vec3::new(-640.0, -640.0, 0.0),
            Vec3::new(-624.0, 640.0, 200.0),
        );
        w.push(
            Vec3::new(624.0, -640.0, 0.0),
            Vec3::new(640.0, 640.0, 200.0),
        );
        w.push(
            Vec3::new(-640.0, -640.0, 0.0),
            Vec3::new(640.0, -624.0, 200.0),
        );
        w.push(
            Vec3::new(-640.0, 624.0, 0.0),
            Vec3::new(640.0, 640.0, 200.0),
        );
        w.push(
            Vec3::new(128.0, -128.0, 0.0),
            Vec3::new(384.0, 128.0, 100.0),
        );
        let floor = Vec3::new(-300.0, 0.0, 24.0);
        let ledge = Vec3::new(256.0, 0.0, 124.0);
        let grid = NavGrid::build(&w, &[floor, ledge]);
        let (a, b) = (grid.nearest(floor).unwrap(), grid.nearest(ledge).unwrap());
        assert!((grid.pos(b).z - 124.0).abs() < 2.0);
        // Down is a way, up is not; the table says so without a search, and the search
        // agrees.
        assert_eq!(grid.leads(b, a), Some(true));
        assert_eq!(grid.leads(a, b), Some(false));
        assert!(grid.path(b, a).is_some());
        assert!(grid.path(a, b).is_none());
        assert!(grid.reachable(ledge, floor) && !grid.reachable(floor, ledge));
        // On the floor everything leads everywhere, round the block.
        let far = grid.nearest(Vec3::new(500.0, 0.0, 24.0)).unwrap();
        assert_eq!(grid.leads(a, far), Some(true));
        assert_eq!(grid.leads(far, a), Some(true));
        assert!(grid.path(a, far).is_some());
        // Every pair the table rules out has no path, and every pair it allows has one.
        let n = grid.len() as NodeId;
        for (x, y) in [
            (0, n - 1),
            (n - 1, 0),
            (n / 2, n / 3),
            (n / 3, n / 2),
            (a, b),
            (b, far),
        ] {
            assert_eq!(
                grid.leads(x, y),
                Some(grid.path(x, y).is_some()),
                "{x} to {y}"
            );
        }
    }

    /// A floor with a wall across it that has one gap, and a raised platform with a ramp of
    /// steps on one side.
    fn world() -> BoxWorld {
        let mut w = BoxWorld::floor();
        // A wall along x = 0 from y = -1024 to y = 1024, 200 high, with a doorway at y in 96..160.
        w.push(Vec3::new(-8.0, -1024.0, 0.0), Vec3::new(8.0, 96.0, 200.0));
        w.push(Vec3::new(-8.0, 160.0, 0.0), Vec3::new(8.0, 1024.0, 200.0));
        // Bounds, so the flood ends.
        w.push(
            Vec3::new(-1040.0, -1040.0, 0.0),
            Vec3::new(-1024.0, 1040.0, 200.0),
        );
        w.push(
            Vec3::new(1024.0, -1040.0, 0.0),
            Vec3::new(1040.0, 1040.0, 200.0),
        );
        w.push(
            Vec3::new(-1040.0, -1040.0, 0.0),
            Vec3::new(1040.0, -1024.0, 200.0),
        );
        w.push(
            Vec3::new(-1040.0, 1024.0, 0.0),
            Vec3::new(1040.0, 1040.0, 200.0),
        );
        // A platform 96 high in the east, reached by six steps of 16 from the south.
        w.push(
            Vec3::new(512.0, 512.0, 0.0),
            Vec3::new(1024.0, 1024.0, 96.0),
        );
        for i in 0..6 {
            let top = 16.0 * (i + 1) as f32;
            let y0 = 512.0 - 48.0 * (6 - i) as f32;
            w.push(Vec3::new(640.0, y0, 0.0), Vec3::new(768.0, 512.0, top));
        }
        w
    }

    #[test]
    fn the_flood_finds_both_rooms_the_platform_and_the_way_through_the_door() {
        let w = world();
        let start = Vec3::new(-500.0, -500.0, 24.0);
        let t0 = std::time::Instant::now();
        let grid = NavGrid::build(&w, &[start]);
        println!("{} nodes in {:?}", grid.len(), t0.elapsed());
        // About (2048 / 32)^2 cells, less the wall, plus the platform's second floor.
        assert!((3600..4400).contains(&grid.len()), "{} nodes", grid.len());
        let behind = Vec3::new(500.0, -500.0, 24.0);
        let path = grid
            .path_between(start, behind)
            .expect("a way through the door");
        // The path crosses x = 0 inside the doorway.
        let crossing = path
            .windows(2)
            .find(|w| w[0].x < 0.0 && w[1].x >= 0.0)
            .expect("crosses the wall line");
        assert!(
            (96.0..=160.0).contains(&crossing[0].y) && (96.0..=160.0).contains(&crossing[1].y),
            "{crossing:?}"
        );
        let length: f32 = path.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        // Straight would be 1000; through the door it is about 780 + 780.
        assert!((1400.0..1800.0).contains(&length), "{length}");
        // Up the steps onto the platform, and a drop link straight off its edge on the way
        // back (shorter than the stairs).
        let top = Vec3::new(900.0, 900.0, 120.0);
        let up = grid.path_between(behind, top).expect("the stairs");
        assert!(up.last().unwrap().z > 100.0);
        assert!(
            up.iter()
                .any(|p| (640.0..768.0).contains(&p.x) && p.y < 512.0 && p.z > 40.0),
            "goes by the steps"
        );
        let down = grid.path_between(top, behind).expect("down again");
        let down_len: f32 = down.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let up_len: f32 = up.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        assert!(
            down_len < up_len,
            "jumping off beats the stairs: {down_len} vs {up_len}"
        );
        // No way into the solid of the wall or out of the map.
        assert!(!grid.reachable(start, Vec3::new(3000.0, 0.0, 24.0)));
    }

    #[test]
    fn straight_lines_are_walkable_only_where_a_body_fits_and_the_floor_holds() {
        let w = world();
        let grid = NavGrid::build(&w, &[Vec3::new(-500.0, -500.0, 24.0)]);
        let a = Vec3::new(-500.0, -500.0, 24.0);
        assert!(grid.walkable_line(&w, a, Vec3::new(-100.0, 300.0, 24.0)));
        // Through the wall: no.
        assert!(!grid.walkable_line(&w, a, Vec3::new(500.0, -500.0, 24.0)));
        // Through the doorway, squarely: yes.
        assert!(grid.walkable_line(
            &w,
            Vec3::new(-200.0, 128.0, 24.0),
            Vec3::new(200.0, 128.0, 24.0)
        ));
        // Off the platform's edge: the line is clear but the floor is not there.
        assert!(!grid.walkable_line(
            &w,
            Vec3::new(700.0, 700.0, 120.0),
            Vec3::new(300.0, 700.0, 120.0)
        ));
    }

    #[test]
    fn a_navigator_walks_a_body_through_the_door() {
        use gm_core::movement::{MoveInput, MoveVars, PlayerState, player_move};
        let w = world();
        let start = Vec3::new(-500.0, -500.0, 24.0);
        let goal = Vec3::new(500.0, -500.0, 24.0);
        let grid = NavGrid::build(&w, &[start]);
        let mut nav = Navigator::default();
        let mut st = PlayerState::new(start);
        let mut ticks = 0u32;
        loop {
            ticks += 1;
            let s = nav.steer(&grid, &w, st.origin, goal, 24.0, ticks, 64);
            assert!(!s.blocked);
            if s.arrived {
                break;
            }
            let to = s.toward - st.origin;
            let input = MoveInput {
                yaw: to.y.atan2(to.x).to_degrees(),
                forward: 1.0,
                side: 0.0,
                jump: false,
            };
            player_move(&w, &MoveVars::QUAKE, &mut st, &input, 1.0 / 64.0);
            assert!(ticks < 64 * 12, "still walking at {:?}", st.origin);
        }
        // About 1,600 u at 320 u/s: five to six seconds, corners cut.
        println!("arrived after {ticks} ticks");
        assert!(ticks < 64 * 7, "{ticks} ticks");
        // A goal with no way to it says so.
        let mut lost = Navigator::default();
        let s = lost.steer(&grid, &w, start, Vec3::new(3000.0, 0.0, 24.0), 24.0, 1, 64);
        assert!(s.blocked);
    }
}
