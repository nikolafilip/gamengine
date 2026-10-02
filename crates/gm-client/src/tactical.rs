//! The tactical viewport (COMPANIONS.md 6): a camera above the commander, a free cursor,
//! picking and the squad selection. Everything here is arithmetic over what the client is
//! shown; what it may be shown is the zone's business (squad sight, section 5.2).

use glam::{Mat4, Vec3};
use gm_core::sim::view_dir;
use gm_core::trace::{CollisionWorld, Contents, Hull};

/// The camera looks down at this angle.
pub const PITCH: f32 = 60.0;
pub const ZOOM_MIN: f32 = 420.0;
pub const ZOOM_MAX: f32 = 1100.0;
pub const ZOOM_DEFAULT: f32 = 760.0;
/// How far from the commander the view may be panned.
pub const PAN_MAX: f32 = 1536.0;
const PAN_SPEED: f32 = 900.0;
const TURN_SPEED: f32 = 100.0;
const ZOOM_STEP: f32 = 60.0;
/// A cursor ray picks a body it passes within this distance of.
const PICK_RADIUS: f32 = 26.0;
/// How far a cursor ray is followed.
const REACH: f32 = 4096.0;

pub struct Tactical {
    pub active: bool,
    /// The focus of the view, relative to the commander, on the ground plane.
    pub pan: Vec3,
    pub zoom: f32,
    pub yaw: f32,
    /// Squad slots selected (bit i = slot i).
    pub selected: u8,
    /// The cursor, in pixels from the top left.
    pub cursor: (f32, f32),
}

impl Tactical {
    pub fn new() -> Tactical {
        Tactical {
            active: false,
            pan: Vec3::ZERO,
            zoom: ZOOM_DEFAULT,
            yaw: 0.0,
            selected: 0b1_1111,
            cursor: (0.0, 0.0),
        }
    }

    /// Enter the view looking the way the body looks, centred on it.
    pub fn enter(&mut self, yaw: f32) {
        self.active = true;
        self.pan = Vec3::ZERO;
        self.yaw = yaw;
    }

    /// Move the view for `dt` seconds: `forward` and `side` pan in the camera's frame, `turn`
    /// rotates (positive = clockwise seen from above), `wheel` zooms (positive = in).
    pub fn steer(&mut self, forward: f32, side: f32, turn: f32, wheel: f32, dt: f32) {
        self.yaw = (self.yaw - turn * TURN_SPEED * dt).rem_euclid(360.0);
        let (s, c) = self.yaw.to_radians().sin_cos();
        let ahead = Vec3::new(c, s, 0.0);
        let right = Vec3::new(s, -c, 0.0);
        // Panning is by sight: the farther out, the faster.
        let speed = PAN_SPEED * self.zoom / ZOOM_DEFAULT;
        self.pan += (ahead * forward + right * side) * speed * dt;
        if self.pan.length() > PAN_MAX {
            self.pan = self.pan.normalize() * PAN_MAX;
        }
        self.zoom = (self.zoom - wheel * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
    }

    /// The point the camera looks at, for a commander standing at `centre`.
    pub fn focus(&self, centre: Vec3) -> Vec3 {
        centre + self.pan
    }

    /// Where the camera is: `zoom` back along its line of sight from the focus.
    pub fn camera(&self, centre: Vec3) -> Vec3 {
        self.focus(centre) - view_dir(self.yaw, PITCH) * self.zoom
    }

    /// The ray under the cursor: where it starts and its unit direction.
    pub fn ray(&self, view_proj: Mat4, size: (f32, f32)) -> (Vec3, Vec3) {
        let x = self.cursor.0 / size.0.max(1.0) * 2.0 - 1.0;
        let y = 1.0 - self.cursor.1 / size.1.max(1.0) * 2.0;
        let inv = view_proj.inverse();
        let near = inv.project_point3(Vec3::new(x, y, 0.0));
        let far = inv.project_point3(Vec3::new(x, y, 1.0));
        (near, (far - near).normalize_or_zero())
    }
}

/// Where a world point is on a screen of `size` pixels; `None` behind the camera.
pub fn project(view_proj: Mat4, size: (f32, f32), point: Vec3) -> Option<(f32, f32)> {
    let clip = view_proj * point.extend(1.0);
    if clip.w <= 1.0 {
        return None;
    }
    let (x, y) = (clip.x / clip.w, clip.y / clip.w);
    Some(((x + 1.0) * 0.5 * size.0, (1.0 - y) * 0.5 * size.1))
}

/// The body the ray passes through first. `bodies` are `(id, hull origin)`; a body is its
/// hull's upright axis, 56 units tall, with `PICK_RADIUS` round it.
pub fn pick_body(origin: Vec3, dir: Vec3, bodies: &[(u32, Vec3)]) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for &(id, pos) in bodies {
        let (a, b) = (pos - Vec3::Z * 24.0, pos + Vec3::Z * 32.0);
        // Closest approach between the ray and the segment a..b.
        let axis = b - a;
        let w = origin - a;
        let (dd, da, aa) = (dir.dot(dir), dir.dot(axis), axis.dot(axis));
        let (dw, aw) = (dir.dot(w), axis.dot(w));
        let denom = dd * aa - da * da;
        let mut t = if denom.abs() > 1e-6 {
            ((dd * aw - da * dw) / denom).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut s = (da * t - dw) / dd.max(1e-6);
        if s < 0.0 {
            s = 0.0;
            t = (aw / aa.max(1e-6)).clamp(0.0, 1.0);
        }
        let gap = (origin + dir * s - (a + axis * t)).length();
        if gap <= PICK_RADIUS && s <= REACH && best.is_none_or(|(d, _)| s < d) {
            best = Some((s, id));
        }
    }
    best.map(|(_, id)| id)
}

/// Where the ray meets the map's floor. The camera hangs above the ceiling, inside rock, so
/// the ray is walked to the first open point and traced from there; what it hits is dropped
/// to the floor below it. `None` when the ray never enters the map.
pub fn pick_ground(world: &dyn CollisionWorld, origin: Vec3, dir: Vec3) -> Option<Vec3> {
    let mut at = 0.0;
    let open = loop {
        let p = origin + dir * at;
        if world.point_contents(Hull::Point, p) == Contents::Empty {
            break p;
        }
        at += 12.0;
        if at > REACH {
            return None;
        }
    };
    let hit = world.trace(Hull::Point, open, open + dir * REACH);
    if hit.fraction >= 1.0 {
        return None;
    }
    // A wall or a pillar side: the floor at its foot.
    let above = hit.end + hit.plane_normal * 4.0;
    let down = world.trace(Hull::Point, above, above - Vec3::Z * 1024.0);
    (down.fraction < 1.0 && !down.start_solid).then_some(down.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::view_proj;
    use gm_core::collide::BoxWorld;

    #[test]
    fn the_camera_hangs_above_and_behind_the_focus() {
        let mut t = Tactical::new();
        t.enter(0.0);
        let centre = Vec3::new(100.0, 50.0, 24.0);
        let cam = t.camera(centre);
        // 60 degrees down from 760 away: 658 up, 380 back.
        assert!((cam.z - centre.z - 658.2).abs() < 0.5, "{cam:?}");
        assert!((centre.x - cam.x - 380.0).abs() < 0.5, "{cam:?}");
        assert!((cam.y - centre.y).abs() < 1e-3);
        // Panning is clamped to the leash, zooming to its range.
        t.steer(1.0, 0.0, 0.0, 0.0, 10.0);
        assert!((t.pan.length() - PAN_MAX).abs() < 1e-2);
        t.steer(0.0, 0.0, 0.0, 100.0, 0.0);
        assert_eq!(t.zoom, ZOOM_MIN);
        t.steer(0.0, 0.0, 0.0, -100.0, 0.0);
        assert_eq!(t.zoom, ZOOM_MAX);
        // Turning clockwise lowers the yaw (yaw runs counter-clockwise).
        t.yaw = 10.0;
        t.steer(0.0, 0.0, 1.0, 0.0, 0.2);
        assert!((t.yaw - 350.0).abs() < 1e-3, "{}", t.yaw);
    }

    #[test]
    fn the_cursor_ray_picks_the_ground_and_the_bodies_under_it() {
        let world = BoxWorld::floor();
        let mut t = Tactical::new();
        t.enter(90.0);
        let centre = Vec3::new(0.0, 0.0, 24.0);
        let size = (1280.0, 720.0);
        let vp = view_proj(t.camera(centre), t.yaw, PITCH, size.0 / size.1);
        // The middle of the screen is the focus.
        t.cursor = (640.0, 360.0);
        let (origin, dir) = t.ray(vp, size);
        let ground = pick_ground(&world, origin, dir).expect("the floor");
        assert!(ground.truncate().length() < 30.0, "{ground:?}");
        assert!(ground.z.abs() < 1.0);
        // A point projects to where a ray through that pixel comes back to.
        let point = Vec3::new(140.0, 220.0, 0.0);
        let px = project(vp, size, point).expect("in front");
        t.cursor = px;
        let (origin, dir) = t.ray(vp, size);
        let back = pick_ground(&world, origin, dir).unwrap();
        assert!((back - point).length() < 3.0, "{back:?}");
        // A body standing there is picked; one elsewhere is not.
        let here = point + Vec3::Z * 24.0;
        let bodies = [(7, Vec3::new(-300.0, 0.0, 24.0)), (9, here)];
        assert_eq!(pick_body(origin, dir, &bodies), Some(9));
        assert_eq!(pick_body(origin, dir, &bodies[..1]), None);
        // Of two on the ray, the nearer.
        let behind = here + dir * 120.0;
        assert_eq!(pick_body(origin, dir, &[(3, behind), (9, here)]), Some(9));
        // Behind the camera nothing projects.
        assert_eq!(project(vp, size, t.camera(centre) + Vec3::Z * 500.0), None);
    }

    #[test]
    fn a_ray_from_inside_rock_finds_the_room_below() {
        // A room with a ceiling slab; the camera is above the slab.
        let mut world = BoxWorld::floor();
        world.push(
            Vec3::new(-512.0, -512.0, 200.0),
            Vec3::new(512.0, 512.0, 2000.0),
        );
        let origin = Vec3::new(0.0, -300.0, 700.0);
        let dir = (Vec3::new(40.0, 60.0, 0.0) - origin).normalize();
        let ground = pick_ground(&world, origin, dir).expect("through the slab to the floor");
        assert!(
            (ground - Vec3::new(40.0, 60.0, 0.0)).length() < 4.0,
            "{ground:?}"
        );
        // A ray that never leaves the slab hits nothing.
        assert_eq!(pick_ground(&world, origin, Vec3::X), None);
    }
}
