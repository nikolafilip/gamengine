//! Hull tracing: a port of Quake's `SV_RecursiveHullCheck` over the BSP clip trees, exposed
//! through [`gm_core::trace::CollisionWorld`].

use glam::Vec3;
use gm_core::trace::{CollisionWorld, Contents, Hull, Trace};

use crate::{Bsp, ClipNode, Plane};

const DIST_EPSILON: f32 = 0.03125;

struct HullRef<'a> {
    clipnodes: &'a [ClipNode],
    planes: &'a [Plane],
    first: i32,
}

struct State {
    fraction: f32,
    end: Vec3,
    normal: Vec3,
    dist: f32,
    start_solid: bool,
    all_solid: bool,
}

fn plane_dist(plane: &Plane, p: Vec3) -> f32 {
    match plane.kind {
        0 => p.x - plane.dist,
        1 => p.y - plane.dist,
        2 => p.z - plane.dist,
        _ => plane.normal.dot(p) - plane.dist,
    }
}

fn contents_of(num: i32) -> Contents {
    Contents::from_i32(num).unwrap_or(Contents::Solid)
}

fn point_contents(hull: &HullRef<'_>, mut num: i32, p: Vec3) -> i32 {
    while num >= 0 {
        let node = &hull.clipnodes[num as usize];
        let d = plane_dist(&hull.planes[node.plane as usize], p);
        num = node.children[if d < 0.0 { 1 } else { 0 }];
    }
    num
}

/// Returns `true` while the sweep is still in open space, `false` once it hit something.
fn recursive_check(
    hull: &HullRef<'_>,
    num: i32,
    p1f: f32,
    p2f: f32,
    p1: Vec3,
    p2: Vec3,
    st: &mut State,
) -> bool {
    if num < 0 {
        if num != Contents::Solid as i32 {
            st.all_solid = false;
        } else {
            st.start_solid = true;
        }
        return true;
    }
    let node = &hull.clipnodes[num as usize];
    let plane = &hull.planes[node.plane as usize];
    let t1 = plane_dist(plane, p1);
    let t2 = plane_dist(plane, p2);

    if t1 >= 0.0 && t2 >= 0.0 {
        return recursive_check(hull, node.children[0], p1f, p2f, p1, p2, st);
    }
    if t1 < 0.0 && t2 < 0.0 {
        return recursive_check(hull, node.children[1], p1f, p2f, p1, p2, st);
    }

    // Put the crossing point DIST_EPSILON on the near side.
    let mut frac = if t1 < 0.0 {
        (t1 + DIST_EPSILON) / (t1 - t2)
    } else {
        (t1 - DIST_EPSILON) / (t1 - t2)
    };
    frac = frac.clamp(0.0, 1.0);
    let mut midf = p1f + (p2f - p1f) * frac;
    let mut mid = p1 + (p2 - p1) * frac;
    let side = usize::from(t1 < 0.0);

    // Move up to the node.
    if !recursive_check(hull, node.children[side], p1f, midf, p1, mid, st) {
        return false;
    }
    // Go past the node when the far side is not solid.
    if point_contents(hull, node.children[side ^ 1], mid) != Contents::Solid as i32 {
        return recursive_check(hull, node.children[side ^ 1], midf, p2f, mid, p2, st);
    }
    if st.all_solid {
        return false; // never got out of the solid area
    }

    // The far side is solid: this is the impact point.
    if side == 0 {
        st.normal = plane.normal;
        st.dist = plane.dist;
    } else {
        st.normal = -plane.normal;
        st.dist = -plane.dist;
    }
    // Back up if we still ended inside solid (rare, but Quake does it too).
    while point_contents(hull, hull.first, mid) == Contents::Solid as i32 {
        frac -= 0.1;
        if frac < 0.0 {
            st.fraction = midf;
            st.end = mid;
            return false;
        }
        midf = p1f + (p2f - p1f) * frac;
        mid = p1 + (p2 - p1) * frac;
    }
    st.fraction = midf;
    st.end = mid;
    false
}

impl Bsp {
    fn hull_ref(&self, model: usize, hull: Hull) -> HullRef<'_> {
        let m = &self.models[model];
        match hull {
            Hull::Point => HullRef {
                clipnodes: &self.hull0,
                planes: &self.planes,
                first: m.head_nodes[0],
            },
            Hull::Player => HullRef {
                clipnodes: &self.clipnodes,
                planes: &self.planes,
                first: m.head_nodes[1],
            },
            Hull::Large => HullRef {
                clipnodes: &self.clipnodes,
                planes: &self.planes,
                first: m.head_nodes[2],
            },
        }
    }

    /// Contents at `point` for `hull` in `model`.
    pub fn hull_point_contents(&self, model: usize, hull: Hull, point: Vec3) -> Contents {
        let h = self.hull_ref(model, hull);
        contents_of(point_contents(
            &h,
            h.first,
            point - self.models[model].origin,
        ))
    }

    /// Sweep `hull` from `start` to `end` against `model`.
    pub fn trace_model(&self, model: usize, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        let h = self.hull_ref(model, hull);
        let origin = self.models[model].origin;
        let (s, e) = (start - origin, end - origin);
        let mut st = State {
            fraction: 1.0,
            end: e,
            normal: Vec3::ZERO,
            dist: 0.0,
            start_solid: false,
            all_solid: true,
        };
        recursive_check(&h, h.first, 0.0, 1.0, s, e, &mut st);
        if st.all_solid {
            st.start_solid = true;
        }
        let contents = contents_of(point_contents(&h, h.first, st.end));
        Trace {
            fraction: st.fraction,
            end: st.end + origin,
            plane_normal: st.normal,
            plane_dist: st.dist + st.normal.dot(origin),
            start_solid: st.start_solid,
            all_solid: st.all_solid,
            contents,
        }
    }
}

impl CollisionWorld for Bsp {
    fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        self.trace_model(0, hull, start, end)
    }

    fn point_contents(&self, hull: Hull, point: Vec3) -> Contents {
        self.hull_point_contents(0, hull, point)
    }
}
