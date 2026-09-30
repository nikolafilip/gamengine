//! Quake-style player movement (PLAN.md 2.4), a port of QuakeWorld's `pmove.c` onto the
//! [`CollisionWorld`] trait: ground friction with edge friction, ground and air acceleration
//! (with the 30 u/s air wish cap that makes strafe-jumping work), gravity, 18-unit step-up,
//! plane clipping with up to four bumps, and no pogo-sticking (jump must be released).
//!
//! `dt` is the tick length. Call once per tick with the input sampled for that tick. The same
//! function runs on the client for prediction and on the server for authority.

use glam::Vec3;

use crate::trace::{CollisionWorld, Hull, Trace};

/// Tunables. Defaults are Quake's. Statuses (Slow, Haste, Root) act by scaling these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveVars {
    /// u/s². Quake: 800 (2.5 g at 32 u/m; deliberate, it reads as snappy).
    pub gravity: f32,
    /// Below this speed friction acts as if at this speed, so stops are crisp.
    pub stop_speed: f32,
    /// Ground wish speed cap, u/s.
    pub max_speed: f32,
    /// Ground acceleration factor.
    pub accelerate: f32,
    /// Air acceleration factor.
    pub air_accelerate: f32,
    /// Air wish-speed cap, u/s. The Quake 30 keeps air control small but non-zero.
    pub air_wish_cap: f32,
    /// Ground friction.
    pub friction: f32,
    /// Friction multiplier when the leading edge is over a drop.
    pub edge_friction: f32,
    /// Maximum step height climbed without jumping.
    pub step_size: f32,
    /// Vertical velocity added by a jump.
    pub jump_velocity: f32,
    /// Absolute per-axis velocity clamp.
    pub max_velocity: f32,
}

impl MoveVars {
    pub const QUAKE: MoveVars = MoveVars {
        gravity: 800.0,
        stop_speed: 100.0,
        max_speed: 320.0,
        accelerate: 10.0,
        air_accelerate: 10.0,
        air_wish_cap: 30.0,
        friction: 4.0,
        edge_friction: 2.0,
        step_size: 18.0,
        jump_velocity: 270.0,
        max_velocity: 2000.0,
    };
}

impl Default for MoveVars {
    fn default() -> Self {
        Self::QUAKE
    }
}

/// Per-tick movement input. Already mapped from keys/sticks; movement axes are `-1..=1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveInput {
    /// View yaw in degrees, Quake convention (0 = +X, counter-clockwise from above).
    pub yaw: f32,
    /// Forward (+) / back (-).
    pub forward: f32,
    /// Right (+) / left (-).
    pub side: f32,
    pub jump: bool,
}

/// Everything the mover needs to carry between ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerState {
    /// Hull origin. Feet are at `origin + hull.mins().z`.
    pub origin: Vec3,
    pub velocity: Vec3,
    pub hull: Hull,
    pub on_ground: bool,
    pub ground_normal: Vec3,
    /// Jump was held last tick; a new jump needs a release first.
    pub jump_held: bool,
}

impl PlayerState {
    pub fn new(origin: Vec3) -> Self {
        PlayerState {
            origin,
            ..Default::default()
        }
    }

    pub fn eye_position(&self) -> Vec3 {
        self.origin + Vec3::new(0.0, 0.0, self.hull.eye_height())
    }

    /// Horizontal speed, u/s.
    pub fn ground_speed(&self) -> f32 {
        self.velocity.truncate().length()
    }
}

/// Forward and right unit vectors on the ground plane for a yaw in degrees.
pub fn yaw_vectors(yaw_deg: f32) -> (Vec3, Vec3) {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    (Vec3::new(c, s, 0.0), Vec3::new(s, -c, 0.0))
}

const STOP_EPSILON: f32 = 0.1;
const MAX_CLIP_PLANES: usize = 5;
const NUM_BUMPS: usize = 4;
/// Surfaces steeper than this are walls, not floors (cos 45.6°).
const GROUND_NORMAL_Z: f32 = 0.7;

/// Advance one player by one tick.
pub fn player_move<W: CollisionWorld>(
    world: &W,
    vars: &MoveVars,
    st: &mut PlayerState,
    input: &MoveInput,
    dt: f32,
) {
    categorize_position(world, st);
    check_jump(vars, st, input.jump);
    if st.on_ground {
        friction(world, vars, st, dt);
    }

    let (forward, right) = yaw_vectors(input.yaw);
    let mut wishvel =
        forward * (input.forward * vars.max_speed) + right * (input.side * vars.max_speed);
    wishvel.z = 0.0;
    let mut wishspeed = wishvel.length();
    let wishdir = if wishspeed > 1e-6 {
        wishvel / wishspeed
    } else {
        Vec3::ZERO
    };
    if wishspeed > vars.max_speed {
        wishspeed = vars.max_speed;
    }

    if st.on_ground {
        st.velocity.z = 0.0;
        accelerate(st, wishdir, wishspeed, vars.accelerate, dt);
        ground_move(world, vars, st, dt);
    } else {
        air_accelerate(st, wishdir, wishspeed, vars, dt);
        st.velocity.z -= vars.gravity * dt;
        fly_move(world, st, dt);
    }

    categorize_position(world, st);
    st.velocity = st.velocity.clamp(
        Vec3::splat(-vars.max_velocity),
        Vec3::splat(vars.max_velocity),
    );
}

fn categorize_position<W: CollisionWorld>(world: &W, st: &mut PlayerState) {
    if st.velocity.z > 180.0 {
        st.on_ground = false;
        st.ground_normal = Vec3::ZERO;
        return;
    }
    let point = st.origin - Vec3::new(0.0, 0.0, 1.0);
    let tr = world.trace(st.hull, st.origin, point);
    if tr.plane_normal.z < GROUND_NORMAL_Z {
        st.on_ground = false;
        st.ground_normal = Vec3::ZERO;
    } else {
        st.on_ground = true;
        st.ground_normal = tr.plane_normal;
        if !tr.start_solid && !tr.all_solid {
            st.origin = tr.end;
        }
    }
}

fn check_jump(vars: &MoveVars, st: &mut PlayerState, jump: bool) {
    if !jump {
        st.jump_held = false;
        return;
    }
    if !st.on_ground || st.jump_held {
        return;
    }
    st.on_ground = false;
    st.velocity.z += vars.jump_velocity;
    st.jump_held = true;
}

fn friction<W: CollisionWorld>(world: &W, vars: &MoveVars, st: &mut PlayerState, dt: f32) {
    let speed = st.velocity.length();
    if speed < 1.0 {
        st.velocity.x = 0.0;
        st.velocity.y = 0.0;
        return;
    }
    let mut friction = vars.friction;
    // Leading edge over a drop: more friction so players do not slide off ledges.
    let lead = st.origin + st.velocity / speed * 16.0;
    let start = Vec3::new(lead.x, lead.y, st.origin.z + st.hull.mins().z);
    let stop = start - Vec3::new(0.0, 0.0, 34.0);
    if world.trace(st.hull, start, stop).fraction == 1.0 {
        friction *= vars.edge_friction;
    }
    let control = speed.max(vars.stop_speed);
    let drop = control * friction * dt;
    let newspeed = (speed - drop).max(0.0) / speed;
    st.velocity *= newspeed;
}

fn accelerate(st: &mut PlayerState, wishdir: Vec3, wishspeed: f32, accel: f32, dt: f32) {
    let currentspeed = st.velocity.dot(wishdir);
    let addspeed = wishspeed - currentspeed;
    if addspeed <= 0.0 {
        return;
    }
    let accelspeed = (accel * dt * wishspeed).min(addspeed);
    st.velocity += wishdir * accelspeed;
}

fn air_accelerate(st: &mut PlayerState, wishdir: Vec3, wishspeed: f32, vars: &MoveVars, dt: f32) {
    let wishspd = wishspeed.min(vars.air_wish_cap);
    let currentspeed = st.velocity.dot(wishdir);
    let addspeed = wishspd - currentspeed;
    if addspeed <= 0.0 {
        return;
    }
    let accelspeed = (vars.air_accelerate * wishspeed * dt).min(addspeed);
    st.velocity += wishdir * accelspeed;
}

/// Remove the component of `v` going into the plane. Returns the clipped velocity.
pub fn clip_velocity(v: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = v.dot(normal) * overbounce;
    let mut out = v - normal * backoff;
    for i in 0..3 {
        if out[i] > -STOP_EPSILON && out[i] < STOP_EPSILON {
            out[i] = 0.0;
        }
    }
    out
}

fn player_trace<W: CollisionWorld>(world: &W, hull: Hull, start: Vec3, end: Vec3) -> Trace {
    let mut tr = world.trace(hull, start, end);
    if tr.all_solid {
        tr.start_solid = true;
    }
    if tr.start_solid {
        tr.fraction = 0.0;
        tr.end = start;
    }
    tr
}

/// Slide along up to four planes. Returns a bitmask: 1 = hit a floor, 2 = hit a wall.
fn fly_move<W: CollisionWorld>(world: &W, st: &mut PlayerState, dt: f32) -> u8 {
    let mut blocked = 0u8;
    let original_velocity = st.velocity;
    let primal_velocity = st.velocity;
    let mut planes: [Vec3; MAX_CLIP_PLANES] = [Vec3::ZERO; MAX_CLIP_PLANES];
    let mut numplanes = 0usize;
    let mut time_left = dt;

    for _ in 0..NUM_BUMPS {
        let end = st.origin + st.velocity * time_left;
        let tr = player_trace(world, st.hull, st.origin, end);
        if tr.start_solid || tr.all_solid {
            st.velocity = Vec3::ZERO;
            return 3;
        }
        if tr.fraction > 0.0 {
            st.origin = tr.end;
            numplanes = 0;
        }
        if tr.fraction == 1.0 {
            break;
        }
        if tr.plane_normal.z > GROUND_NORMAL_Z {
            blocked |= 1;
        }
        if tr.plane_normal.z == 0.0 {
            blocked |= 2;
        }
        time_left -= time_left * tr.fraction;

        if numplanes >= MAX_CLIP_PLANES {
            st.velocity = Vec3::ZERO;
            break;
        }
        planes[numplanes] = tr.plane_normal;
        numplanes += 1;

        // Find a plane whose clipped velocity does not go into any other plane.
        let mut found = false;
        for i in 0..numplanes {
            let v = clip_velocity(original_velocity, planes[i], 1.0);
            let ok = (0..numplanes).all(|j| j == i || v.dot(planes[j]) >= 0.0);
            if ok {
                st.velocity = v;
                found = true;
                break;
            }
        }
        if !found {
            if numplanes != 2 {
                st.velocity = Vec3::ZERO;
                break;
            }
            // Slide along the crease between two planes.
            let dir = planes[0].cross(planes[1]);
            st.velocity = dir * dir.dot(st.velocity);
        }
        // Stop dead instead of oscillating in a sloped corner.
        if st.velocity.dot(primal_velocity) <= 0.0 {
            st.velocity = Vec3::ZERO;
            break;
        }
    }
    blocked
}

fn ground_move<W: CollisionWorld>(world: &W, vars: &MoveVars, st: &mut PlayerState, dt: f32) {
    st.velocity.z = 0.0;
    if st.velocity == Vec3::ZERO {
        return;
    }
    // Straight move first.
    let dest = Vec3::new(
        st.origin.x + st.velocity.x * dt,
        st.origin.y + st.velocity.y * dt,
        st.origin.z,
    );
    let tr = player_trace(world, st.hull, st.origin, dest);
    if tr.fraction == 1.0 {
        st.origin = tr.end;
        return;
    }

    // Blocked: try sliding on the ground, and sliding after stepping up. Keep whichever went farther.
    let original = st.origin;
    let original_vel = st.velocity;

    fly_move(world, st, dt);
    let down = st.origin;
    let down_vel = st.velocity;

    st.origin = original;
    st.velocity = original_vel;
    let up_dest = st.origin + Vec3::new(0.0, 0.0, vars.step_size);
    let tr = player_trace(world, st.hull, st.origin, up_dest);
    if !tr.start_solid && !tr.all_solid {
        st.origin = tr.end;
    }
    fly_move(world, st, dt);
    let down_dest = st.origin - Vec3::new(0.0, 0.0, vars.step_size);
    let tr = player_trace(world, st.hull, st.origin, down_dest);
    let use_down = if tr.plane_normal.z < GROUND_NORMAL_Z {
        true
    } else {
        if !tr.start_solid && !tr.all_solid {
            st.origin = tr.end;
        }
        let up = st.origin;
        let downdist = (down - original).truncate().length_squared();
        let updist = (up - original).truncate().length_squared();
        downdist > updist
    };
    if use_down {
        st.origin = down;
        st.velocity = down_vel;
    } else {
        st.velocity.z = down_vel.z;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::Contents;

    /// Axis-aligned solid boxes; hull sweeps via Minkowski expansion and a slab test. This is a
    /// stand-in for the BSP hull tracer with identical trace semantics.
    struct BoxWorld {
        solids: Vec<(Vec3, Vec3)>,
    }

    const DIST_EPSILON: f32 = 0.03125;

    impl CollisionWorld for BoxWorld {
        fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
            let mut best = Trace::clear(start, end);
            let delta = end - start;
            for &(bmin, bmax) in &self.solids {
                let emin = bmin - hull.maxs();
                let emax = bmax - hull.mins();
                let inside = |p: Vec3| (0..3).all(|a| p[a] > emin[a] && p[a] < emax[a]);
                if inside(start) {
                    best.start_solid = true;
                    best.all_solid = inside(end);
                    best.fraction = 0.0;
                    best.end = start;
                    best.contents = Contents::Solid;
                    continue;
                }
                let mut tmin = 0.0f32;
                let mut tmax = 1.0f32;
                let mut hit: Option<Vec3> = None;
                let mut miss = false;
                for a in 0..3 {
                    let (s, d) = (start[a], delta[a]);
                    if d.abs() < 1e-9 {
                        if s <= emin[a] || s >= emax[a] {
                            miss = true;
                            break;
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
                    // `>=` so a hull resting exactly on a surface and moving into it reports a
                    // fraction-0 hit with the surface normal, as Quake's hull tracer does.
                    if t0 >= tmin {
                        tmin = t0;
                        hit = Some(normal);
                    }
                    tmax = tmax.min(t1);
                    if tmin > tmax {
                        miss = true;
                        break;
                    }
                }
                if miss {
                    continue;
                }
                if let Some(normal) = hit
                    && tmin < best.fraction
                {
                    let len = delta.length();
                    let frac = if len > 0.0 {
                        ((tmin * len) - DIST_EPSILON).max(0.0) / len
                    } else {
                        0.0
                    };
                    best.fraction = frac;
                    best.end = start + delta * frac;
                    best.plane_normal = normal;
                    best.plane_dist = normal.dot(best.end);
                    best.contents = Contents::Empty;
                }
            }
            best
        }
    }

    fn floor_world() -> BoxWorld {
        BoxWorld {
            solids: vec![(
                Vec3::new(-4096.0, -4096.0, -64.0),
                Vec3::new(4096.0, 4096.0, 0.0),
            )],
        }
    }

    const DT: f32 = 1.0 / 64.0;
    /// Hull origin when the player hull rests on z = 0.
    const REST_Z: f32 = 24.0;

    fn run(world: &BoxWorld, st: &mut PlayerState, input: MoveInput, ticks: usize) {
        for _ in 0..ticks {
            player_move(world, &MoveVars::QUAKE, st, &input, DT);
        }
    }

    #[test]
    fn falls_and_rests_on_the_floor() {
        let world = floor_world();
        let mut st = PlayerState::new(Vec3::new(0.0, 0.0, 200.0));
        run(&world, &mut st, MoveInput::default(), 128);
        assert!(st.on_ground);
        assert!(
            (st.origin.z - REST_Z).abs() < 0.1,
            "origin.z = {}",
            st.origin.z
        );
        assert_eq!(st.velocity, Vec3::ZERO);
    }

    #[test]
    fn ground_speed_caps_at_max_speed() {
        let world = floor_world();
        let mut st = PlayerState::new(Vec3::new(0.0, 0.0, REST_Z));
        let input = MoveInput {
            forward: 1.0,
            ..Default::default()
        };
        run(&world, &mut st, input, 64);
        let speed = st.ground_speed();
        assert!(speed > 300.0 && speed <= 320.0 + 1e-3, "speed = {speed}");
        // Yaw 0 means +X.
        assert!(st.velocity.x > 300.0 && st.velocity.y.abs() < 1e-3);
    }

    #[test]
    fn friction_stops_the_player() {
        let world = floor_world();
        let mut st = PlayerState::new(Vec3::new(0.0, 0.0, REST_Z));
        run(
            &world,
            &mut st,
            MoveInput {
                forward: 1.0,
                ..Default::default()
            },
            64,
        );
        run(&world, &mut st, MoveInput::default(), 64);
        assert_eq!(st.ground_speed(), 0.0);
    }

    #[test]
    fn jump_reaches_quake_height_and_does_not_pogo() {
        let world = floor_world();
        let mut st = PlayerState::new(Vec3::new(0.0, 0.0, REST_Z));
        let held = MoveInput {
            jump: true,
            ..Default::default()
        };
        let mut peak = st.origin.z;
        let mut jumps = 0;
        let mut was_ground = true;
        for _ in 0..192 {
            player_move(&world, &MoveVars::QUAKE, &mut st, &held, DT);
            peak = peak.max(st.origin.z);
            if was_ground && !st.on_ground {
                jumps += 1;
            }
            was_ground = st.on_ground;
        }
        // v²/2g = 270² / 1600 = 45.56 u, minus discretisation.
        let apex = peak - REST_Z;
        assert!(apex > 40.0 && apex < 46.0, "apex = {apex}");
        assert_eq!(jumps, 1, "holding jump must not pogo-stick");
        assert!(st.on_ground);
    }

    #[test]
    fn slides_along_walls() {
        let mut world = floor_world();
        world.solids.push((
            Vec3::new(64.0, -4096.0, 0.0),
            Vec3::new(128.0, 4096.0, 256.0),
        ));
        let mut st = PlayerState::new(Vec3::new(0.0, 0.0, REST_Z));
        // Move diagonally into the wall at x = 64.
        let input = MoveInput {
            yaw: 45.0,
            forward: 1.0,
            ..Default::default()
        };
        run(&world, &mut st, input, 128);
        assert!(
            st.origin.x <= 64.0 - 16.0 + 1e-3,
            "went through the wall: x = {}",
            st.origin.x
        );
        assert!(
            st.origin.x > 40.0,
            "never reached the wall: x = {}",
            st.origin.x
        );
        assert!(
            st.origin.y > 200.0,
            "did not slide along the wall: y = {}",
            st.origin.y
        );
    }

    #[test]
    fn climbs_a_step_but_not_a_ledge() {
        for (height, expect_climb) in [(16.0f32, true), (24.0f32, false)] {
            let mut world = floor_world();
            world.solids.push((
                Vec3::new(64.0, -256.0, 0.0),
                Vec3::new(4096.0, 256.0, height),
            ));
            let mut st = PlayerState::new(Vec3::new(0.0, 0.0, REST_Z));
            run(
                &world,
                &mut st,
                MoveInput {
                    forward: 1.0,
                    ..Default::default()
                },
                128,
            );
            let climbed = st.origin.z > REST_Z + height - 1.0;
            assert_eq!(
                climbed, expect_climb,
                "step {height}: origin = {:?}",
                st.origin
            );
            if expect_climb {
                assert!(
                    st.origin.x > 200.0,
                    "did not keep walking after the step: {:?}",
                    st.origin
                );
            } else {
                assert!(
                    st.origin.x <= 64.0 - 16.0 + 1e-3,
                    "climbed a ledge: {:?}",
                    st.origin
                );
            }
        }
    }

    #[test]
    fn clip_velocity_removes_the_normal_component() {
        let v = clip_velocity(Vec3::new(100.0, 0.0, -300.0), Vec3::Z, 1.0);
        assert_eq!(v, Vec3::new(100.0, 0.0, 0.0));
        let v = clip_velocity(Vec3::new(0.05, 200.0, 0.0), Vec3::X, 1.0);
        assert_eq!(v, Vec3::new(0.0, 200.0, 0.0));
    }
}
