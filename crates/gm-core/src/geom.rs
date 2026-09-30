//! Capsule and segment geometry for hit detection (VOCABULARY.md 3, 5.1, 5.2).

use glam::Vec3;

/// A capsule: the set of points within `radius` of the segment `a..b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
}

impl Capsule {
    /// Upright capsule standing on `feet`, `height` tall in total.
    pub fn upright(feet: Vec3, radius: f32, height: f32) -> Capsule {
        let r = radius.min(height * 0.5);
        Capsule {
            a: feet + Vec3::new(0.0, 0.0, r),
            b: feet + Vec3::new(0.0, 0.0, (height - r).max(r)),
            radius: r,
        }
    }

    pub fn center(&self) -> Vec3 {
        (self.a + self.b) * 0.5
    }

    /// Closest point on the axis segment to `p`.
    pub fn closest_axis_point(&self, p: Vec3) -> Vec3 {
        closest_point_on_segment(self.a, self.b, p)
    }

    pub fn distance_sq(&self, p: Vec3) -> f32 {
        (p - self.closest_axis_point(p)).length_squared()
    }

    pub fn contains(&self, p: Vec3) -> bool {
        self.distance_sq(p) <= self.radius * self.radius
    }

    pub fn intersects(&self, other: &Capsule) -> bool {
        let r = self.radius + other.radius;
        segment_segment_distance_sq(self.a, self.b, other.a, other.b) <= r * r
    }
}

pub fn closest_point_on_segment(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq <= 1e-12 {
        return a;
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    a + ab * t
}

/// Squared distance between segments `p1..q1` and `p2..q2` (Ericson, Real-Time Collision
/// Detection 5.1.9).
pub fn segment_segment_distance_sq(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> f32 {
    const EPS: f32 = 1e-9;
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let (s, t);
    if a <= EPS && e <= EPS {
        return r.length_squared();
    }
    if a <= EPS {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= EPS {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom != 0.0 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    let c1 = p1 + d1 * s;
    let c2 = p2 + d2 * t;
    (c1 - c2).length_squared()
}

/// First `t` in `0..=1` where the point `start + (end - start) * t` is within `radius` of the
/// segment `a..b` (a sphere of `radius` swept along `start..end` against the capsule's axis;
/// use `radius = sphere + capsule radius`). `Some(0.0)` when already inside.
fn sweep_point_capsule(start: Vec3, end: Vec3, radius: f32, a: Vec3, b: Vec3) -> Option<f32> {
    let r2 = radius * radius;
    if (start - closest_point_on_segment(a, b, start)).length_squared() <= r2 {
        return Some(0.0);
    }
    let d = b - a;
    let n = end - start;
    let m = start - a;
    let dd = d.dot(d);
    let nn = n.dot(n);
    if nn <= 1e-12 {
        return None;
    }
    let sphere = |c: Vec3| -> Option<f32> {
        let mc = start - c;
        let bq = mc.dot(n);
        let cq = mc.dot(mc) - r2;
        let discr = bq * bq - nn * cq;
        if discr < 0.0 {
            return None;
        }
        let t = (-bq - discr.sqrt()) / nn;
        (0.0..=1.0).contains(&t).then_some(t)
    };
    if dd <= 1e-12 {
        return sphere(a);
    }
    let nd = n.dot(d);
    let md = m.dot(d);
    let mn = m.dot(n);
    let aq = dd * nn - nd * nd;
    let k = m.dot(m) - r2;
    let cq = dd * k - md * md;
    let cylinder_t = if aq.abs() < 1e-9 {
        // Moving parallel to the axis: only the end caps can be hit.
        None
    } else {
        let bq = dd * mn - nd * md;
        let discr = bq * bq - aq * cq;
        if discr < 0.0 {
            return None;
        }
        let t = (-bq - discr.sqrt()) / aq;
        if !(0.0..=1.0).contains(&t) {
            None
        } else {
            let axial = md + t * nd;
            if axial < 0.0 || axial > dd {
                None
            } else {
                Some(t)
            }
        }
    };
    let cap_a = sphere(a);
    let cap_b = sphere(b);
    [cylinder_t, cap_a, cap_b]
        .into_iter()
        .flatten()
        .fold(None, |best: Option<f32>, t| {
            Some(best.map_or(t, |b| b.min(t)))
        })
}

/// Sweep a sphere of `radius` from `start` to `end` against a capsule.
pub fn sweep_sphere_capsule(start: Vec3, end: Vec3, radius: f32, cap: &Capsule) -> Option<f32> {
    sweep_point_capsule(start, end, radius + cap.radius, cap.a, cap.b)
}

/// Ray of length `len` along unit `dir` against a capsule; returns the distance to the hit.
pub fn ray_capsule(origin: Vec3, dir: Vec3, len: f32, cap: &Capsule) -> Option<f32> {
    sweep_point_capsule(origin, origin + dir * len, cap.radius, cap.a, cap.b).map(|t| t * len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player_cap() -> Capsule {
        Capsule::upright(Vec3::new(100.0, 0.0, 0.0), 14.0, 56.0)
    }

    #[test]
    fn upright_capsule_geometry() {
        let c = player_cap();
        assert_eq!(c.a, Vec3::new(100.0, 0.0, 14.0));
        assert_eq!(c.b, Vec3::new(100.0, 0.0, 42.0));
        assert!(c.contains(Vec3::new(100.0, 0.0, 28.0)));
        assert!(c.contains(Vec3::new(100.0, 0.0, 55.9)));
        assert!(!c.contains(Vec3::new(100.0, 0.0, 56.1)));
        assert!(c.contains(Vec3::new(113.9, 0.0, 28.0)));
        assert!(!c.contains(Vec3::new(114.1, 0.0, 28.0)));
    }

    #[test]
    fn ray_hits_the_cylinder_and_the_caps() {
        let c = player_cap();
        // Straight at the middle from the origin side.
        let t = ray_capsule(Vec3::new(0.0, 0.0, 28.0), Vec3::X, 200.0, &c).unwrap();
        assert!((t - 86.0).abs() < 1e-3, "t = {t}");
        // Down onto the top cap.
        let t = ray_capsule(Vec3::new(100.0, 0.0, 200.0), -Vec3::Z, 400.0, &c).unwrap();
        assert!((t - 144.0).abs() < 1e-3, "t = {t}");
        // Miss to the side.
        assert!(ray_capsule(Vec3::new(0.0, 30.0, 28.0), Vec3::X, 200.0, &c).is_none());
        // Too short.
        assert!(ray_capsule(Vec3::new(0.0, 0.0, 28.0), Vec3::X, 50.0, &c).is_none());
        // Start inside.
        assert_eq!(
            ray_capsule(Vec3::new(100.0, 0.0, 28.0), Vec3::X, 50.0, &c),
            Some(0.0)
        );
    }

    #[test]
    fn swept_sphere_accounts_for_both_radii() {
        let c = player_cap();
        let t = sweep_sphere_capsule(
            Vec3::new(0.0, 0.0, 28.0),
            Vec3::new(200.0, 0.0, 28.0),
            6.0,
            &c,
        )
        .unwrap();
        // Contact when the sphere centre is 14 + 6 = 20 from the axis: x = 80 → t = 0.4.
        assert!((t - 0.4).abs() < 1e-4, "t = {t}");
    }

    #[test]
    fn segment_distance_and_capsule_overlap() {
        let d = segment_segment_distance_sq(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(3.0, 4.0, -5.0),
            Vec3::new(3.0, 4.0, 20.0),
        );
        assert!((d - 25.0).abs() < 1e-4);
        let a = Capsule::upright(Vec3::ZERO, 14.0, 56.0);
        let b = Capsule::upright(Vec3::new(27.0, 0.0, 0.0), 14.0, 56.0);
        assert!(a.intersects(&b));
        let c = Capsule::upright(Vec3::new(29.0, 0.0, 0.0), 14.0, 56.0);
        assert!(!a.intersects(&c));
    }
}
