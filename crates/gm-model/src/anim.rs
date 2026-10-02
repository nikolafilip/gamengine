//! The shared animation set (MODELS.md 9): a function from the snapshot's `anim` state and time
//! to a pose, the same for every model and every mannequin. Procedural placeholder art; the
//! contract is that it is shared, so nobody's avatar can hide a windup.

use glam::{Quat, Vec3};
use gm_core::sim::anim;

use crate::pose::Pose;
use crate::rig::bone::*;

/// Cross-fade between two states.
pub const FADE_SECS: f32 = 0.12;
/// Distance covered by one full run cycle (two steps).
pub const RUN_CYCLE_UNITS: f32 = 160.0;
/// The speed at which the run cycle is at full amplitude.
pub const FULL_SPEED: f32 = 300.0;
/// Time a body takes to fall when it dies.
const FALL_SECS: f32 = 0.45;

#[derive(Clone, Copy, Debug, Default)]
pub struct AnimInput {
    /// `gm_core::sim::anim` state.
    pub state: u8,
    /// Seconds since the state began.
    pub t: f32,
    /// Run phase in radians.
    pub cycle: f32,
    /// Horizontal speed in u/s.
    pub speed: f32,
    /// View pitch in degrees, positive looking down.
    pub pitch: f32,
    /// Height of the model's hips pivot (to lay a body on the ground).
    pub hips_z: f32,
    /// Armour weight, 0 (cloth) to 1 (plate): armour class is in the gait (MODELS.md 9).
    pub weight: f32,
}

/// Armour weight of an armour-class index (cloth, leather, mail, plate).
pub fn armour_weight(class: u8) -> f32 {
    class.min(3) as f32 / 3.0
}

/// A heavier body takes longer strides.
fn cycle_units(weight: f32) -> f32 {
    RUN_CYCLE_UNITS * (1.0 + 0.25 * weight.clamp(0.0, 1.0))
}

fn rx(deg: f32) -> Quat {
    Quat::from_rotation_x(deg.to_radians())
}

fn ry(deg: f32) -> Quat {
    Quat::from_rotation_y(deg.to_radians())
}

fn rz(deg: f32) -> Quat {
    Quat::from_rotation_z(deg.to_radians())
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Arms hanging `down` degrees from the T-pose, swung about the shoulders (negative =
/// forward), elbows flexed forward.
fn arms(p: &mut Pose, down: f32, swing_l: f32, swing_r: f32, flex_l: f32, flex_r: f32) {
    p.rot[UPPER_ARM_L] = ry(swing_l) * rx(-down);
    p.rot[UPPER_ARM_R] = ry(swing_r) * rx(down);
    p.rot[FOREARM_L] = rz(-flex_l);
    p.rot[FOREARM_R] = rz(flex_r);
}

/// Thighs swung about the hips (negative = forward), knees flexed backward.
fn legs(p: &mut Pose, thigh_l: f32, knee_l: f32, thigh_r: f32, knee_r: f32) {
    p.rot[THIGH_L] = ry(thigh_l);
    p.rot[SHIN_L] = ry(knee_l);
    p.rot[THIGH_R] = ry(thigh_r);
    p.rot[SHIN_R] = ry(knee_r);
}

/// Lean of the torso: positive forward, `twist` positive to the left.
fn torso(p: &mut Pose, lean: f32, twist: f32) {
    p.rot[SPINE] = ry(lean * 0.6) * rz(twist * 0.4);
    p.rot[CHEST] = ry(lean * 0.4) * rz(twist * 0.6);
}

/// The pose of one state at one moment.
pub fn pose(i: &AnimInput) -> Pose {
    let mut p = Pose::REST;
    let t = i.t;
    match i.state {
        anim::RUN => {
            let s = (i.speed / FULL_SPEED).clamp(0.25, 1.15);
            let w = i.weight.clamp(0.0, 1.0);
            let (sin, cos) = i.cycle.sin_cos();
            let stride = 38.0 * s * (1.0 + 0.10 * w);
            legs(
                &mut p,
                -stride * sin,
                8.0 + 60.0 * s * cos.max(0.0),
                stride * sin,
                8.0 + 60.0 * s * (-cos).max(0.0),
            );
            // Light bodies pump their arms; plate carries them.
            let swing = 30.0 * s * (1.0 - 0.45 * w);
            let flex = 15.0 + 55.0 * s * (1.0 - 0.5 * w);
            arms(
                &mut p,
                80.0 - 6.0 * w,
                swing * sin,
                -swing * sin,
                flex,
                flex,
            );
            torso(&mut p, (10.0 + 4.0 * w) * s, 8.0 * s * sin);
            // A heavy body rolls from side to side and drops into every step.
            p.rot[HIPS] = rx(3.5 * w * s * sin) * rz(-5.0 * s * sin);
            p.offset.z = 1.2 * s * (1.0 + 0.9 * w) * (cos.abs() - 0.7);
        }
        anim::AIR => {
            legs(&mut p, -22.0, 40.0, 12.0, 35.0);
            arms(&mut p, 55.0, 10.0, 10.0, 30.0, 30.0);
            torso(&mut p, 5.0, 0.0);
        }
        anim::WINDUP => {
            arms(&mut p, 78.0, 0.0, 0.0, 25.0, 0.0);
            // The weapon arm goes up and back, the body coils to the right.
            p.rot[UPPER_ARM_R] = rz(-35.0) * rx(-55.0);
            p.rot[FOREARM_R] = rz(70.0);
            torso(&mut p, -4.0, -28.0);
            legs(&mut p, -14.0, 14.0, 12.0, 10.0);
        }
        anim::SWING => {
            arms(&mut p, 78.0, 15.0, 0.0, 30.0, 0.0);
            // The arm cuts across to the front left, the body uncoils.
            p.rot[UPPER_ARM_R] = rz(65.0) * rx(20.0);
            p.rot[FOREARM_R] = rz(12.0);
            torso(&mut p, 12.0, 32.0);
            legs(&mut p, -24.0, 22.0, 16.0, 6.0);
        }
        anim::RECOVER => {
            arms(&mut p, 72.0, 8.0, -28.0, 22.0, 35.0);
            torso(&mut p, 7.0, 12.0);
            legs(&mut p, -12.0, 14.0, 8.0, 8.0);
        }
        anim::DASH => {
            arms(&mut p, 75.0, 50.0, 50.0, 20.0, 20.0);
            torso(&mut p, 30.0, 0.0);
            legs(&mut p, -36.0, 26.0, 30.0, 24.0);
            p.offset.z = -2.0;
        }
        anim::DEAD => {
            let e = ease(t / FALL_SECS);
            // Falls backward and lies on its back, arms out.
            p.rot[HIPS] = ry(-90.0 * e);
            p.offset.z = -(i.hips_z - 5.0).max(0.0) * e;
            arms(&mut p, 78.0 - 55.0 * e, 0.0, 0.0, 10.0, 10.0);
            legs(&mut p, -6.0 * e, 10.0 * e, 4.0 * e, 6.0 * e);
            return p;
        }
        anim::GUARD => {
            arms(&mut p, 75.0, 0.0, -10.0, 0.0, 40.0);
            // The shield arm comes up in front.
            p.rot[UPPER_ARM_L] = ry(-50.0) * rx(-60.0);
            p.rot[FOREARM_L] = rz(-85.0);
            torso(&mut p, 8.0, -10.0);
            legs(&mut p, -14.0, 20.0, 10.0, 18.0);
            p.offset.z = -1.5;
        }
        anim::PARRY => {
            arms(&mut p, 76.0, 0.0, 0.0, 25.0, 0.0);
            p.rot[UPPER_ARM_R] = rz(70.0) * rx(35.0);
            p.rot[FOREARM_R] = rz(55.0);
            torso(&mut p, 4.0, 18.0);
            legs(&mut p, -10.0, 14.0, 8.0, 12.0);
        }
        anim::CAST => {
            // Both hands forward and a little up, pulsing.
            let pulse = 4.0 * (t * 9.0).sin();
            p.rot[UPPER_ARM_L] = ry(-15.0 + pulse) * rz(-80.0);
            p.rot[UPPER_ARM_R] = ry(-15.0 + pulse) * rz(80.0);
            p.rot[FOREARM_L] = rz(-15.0);
            p.rot[FOREARM_R] = rz(15.0);
            torso(&mut p, -5.0, 0.0);
            legs(&mut p, -8.0, 10.0, 8.0, 10.0);
        }
        anim::COMMAND => {
            // Down on one knee, one arm stretched out ahead: giving orders, and plainly
            // not fighting (COMPANIONS.md 5.1).
            legs(&mut p, -85.0, 90.0, 5.0, 95.0);
            arms(&mut p, 80.0, 0.0, -78.0, 18.0, 6.0);
            torso(&mut p, 8.0, 0.0);
            p.rot[HEAD] = ry(-6.0);
            p.offset.z = -i.hips_z * 0.44;
        }
        anim::STAGGER => {
            let wobble = 5.0 * (t * 14.0).sin();
            arms(&mut p, 50.0, -15.0, -20.0, 30.0, 25.0);
            torso(&mut p, -22.0, wobble);
            p.rot[HEAD] = ry(-12.0);
            legs(&mut p, 8.0, 22.0, -6.0, 20.0);
            p.offset.z = -1.0;
        }
        // IDLE and anything unknown: stand and breathe.
        _ => {
            let breath = (t * 1.6).sin();
            arms(&mut p, 78.0 + breath, 0.0, 0.0, 12.0, 12.0);
            torso(&mut p, 1.5 * breath, 0.0);
            p.rot[HIPS] = rx(0.8 * (t * 0.8).sin());
        }
    }
    // Where the player looks: the chest and the head follow the pitch.
    let pitch = i.pitch.clamp(-60.0, 60.0);
    p.rot[CHEST] = ry(pitch * 0.2) * p.rot[CHEST];
    p.rot[HEAD] = ry(pitch * 0.45) * p.rot[HEAD];
    p
}

/// Per-entity animation state: the running clock, the run phase and the cross-fade.
#[derive(Clone, Copy, Debug)]
pub struct Animator {
    state: u8,
    t: f32,
    cycle: f32,
    from: Pose,
    fade: f32,
    current: Pose,
}

impl Default for Animator {
    fn default() -> Self {
        Animator::new(anim::IDLE)
    }
}

impl Animator {
    pub fn new(state: u8) -> Animator {
        Animator {
            state,
            t: 0.0,
            cycle: 0.0,
            from: Pose::REST,
            // No fade on first sight: an entity appears in its pose.
            fade: 1.0,
            current: Pose::REST,
        }
    }

    pub fn state(&self) -> u8 {
        self.state
    }

    /// Advance by `dt` seconds in which the entity moved `moved` (a vector in world space) and
    /// return the pose to draw.
    pub fn advance(
        &mut self,
        state: u8,
        dt: f32,
        moved: Vec3,
        pitch: f32,
        hips_z: f32,
        weight: f32,
    ) -> Pose {
        if state != self.state {
            self.from = self.current;
            self.fade = 0.0;
            self.t = 0.0;
            self.state = state;
        }
        let dt = dt.clamp(0.0, 0.25);
        let distance = moved.truncate().length();
        self.t += dt;
        self.cycle = (self.cycle + distance / cycle_units(weight) * std::f32::consts::TAU)
            .rem_euclid(std::f32::consts::TAU);
        self.fade = (self.fade + dt / FADE_SECS).min(1.0);
        let target = pose(&AnimInput {
            state,
            t: self.t,
            cycle: self.cycle,
            speed: if dt > 0.0 { distance / dt } else { 0.0 },
            pitch,
            hips_z,
            weight,
        });
        self.current = if self.fade >= 1.0 {
            target
        } else {
            self.from.blend(&target, ease(self.fade))
        };
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::skin_matrices;
    use crate::rig::{self, ALL_BONES};
    use gm_core::vocab::ArchetypeFrame;

    const STATES: [u8; 13] = [
        anim::IDLE,
        anim::RUN,
        anim::AIR,
        anim::WINDUP,
        anim::SWING,
        anim::RECOVER,
        anim::DASH,
        anim::DEAD,
        anim::GUARD,
        anim::PARRY,
        anim::CAST,
        anim::STAGGER,
        anim::COMMAND,
    ];

    fn at(state: u8, t: f32, cycle: f32) -> Pose {
        pose(&AnimInput {
            state,
            t,
            cycle,
            speed: FULL_SPEED,
            pitch: 0.0,
            hips_z: 29.0,
            weight: 0.0,
        })
    }

    #[test]
    fn every_state_gives_unit_rotations_and_keeps_the_body_near_the_ground() {
        let pivots = rig::rest_pivots(ArchetypeFrame::Striker);
        for state in STATES {
            for step in 0..40 {
                let p = at(state, step as f32 * 0.05, step as f32 * 0.4);
                for q in p.rot {
                    assert!(q.is_normalized(), "state {state}");
                }
                let m = skin_matrices(&pivots, ALL_BONES, &p);
                for b in 0..rig::BONES {
                    let at = m[b].transform_point3(pivots[b]);
                    assert!(at.is_finite());
                    assert!(
                        at.z > -8.0 && at.z < 75.0 && at.truncate().length() < 70.0,
                        "state {state} puts {} at {at:?}",
                        rig::NAMES[b]
                    );
                }
            }
        }
    }

    #[test]
    fn idle_hangs_the_arms_and_the_run_alternates_the_legs() {
        let pivots = rig::rest_pivots(ArchetypeFrame::Striker);
        let m = skin_matrices(&pivots, ALL_BONES, &at(anim::IDLE, 0.0, 0.0));
        for (hand, shoulder) in [(HAND_L, UPPER_ARM_L), (HAND_R, UPPER_ARM_R)] {
            let h = m[hand].transform_point3(pivots[hand]);
            assert!(
                h.z < pivots[shoulder].z - 10.0,
                "hands hang below the shoulders: {h:?}"
            );
            assert!(h.x > -1.0, "elbows bend forward, not backward: {h:?}");
        }
        // A quarter into the cycle the left foot is ahead, half a cycle later the right.
        let foot = |cycle: f32, b: usize| {
            let m = skin_matrices(&pivots, ALL_BONES, &at(anim::RUN, 1.0, cycle));
            m[b].transform_point3(pivots[b]).x
        };
        let q = std::f32::consts::FRAC_PI_2;
        assert!(foot(q, FOOT_L) > 5.0 && foot(q, FOOT_R) < -2.0);
        assert!(foot(3.0 * q, FOOT_R) > 5.0 && foot(3.0 * q, FOOT_L) < -2.0);
    }

    #[test]
    fn a_dead_body_lies_on_the_ground() {
        let pivots = rig::rest_pivots(ArchetypeFrame::Striker);
        let m = skin_matrices(&pivots, ALL_BONES, &at(anim::DEAD, 2.0, 0.0));
        let head = m[HEAD].transform_point3(pivots[HEAD]);
        let hips = m[HIPS].transform_point3(pivots[HIPS]);
        assert!(
            head.z < 10.0 && hips.z < 10.0,
            "head {head:?} hips {hips:?}"
        );
        assert!(head.x < -10.0, "it fell backward: {head:?}");
    }

    #[test]
    fn a_swing_reads_differently_from_its_windup() {
        let pivots = rig::rest_pivots(ArchetypeFrame::Striker);
        let hand = |state: u8| {
            let m = skin_matrices(&pivots, ALL_BONES, &at(state, 0.2, 0.0));
            m[HAND_R].transform_point3(pivots[HAND_R])
        };
        let (windup, swing) = (hand(anim::WINDUP), hand(anim::SWING));
        assert!(
            windup.z > pivots[CHEST].z,
            "the weapon hand is raised: {windup:?}"
        );
        assert!(
            swing.x > windup.x + 8.0,
            "the swing comes forward: {swing:?}"
        );
        assert!((windup - swing).length() > 15.0);
    }

    #[test]
    fn the_animator_cross_fades_and_keeps_its_phase() {
        let mut a = Animator::new(anim::IDLE);
        let idle = a.advance(anim::IDLE, 0.016, Vec3::ZERO, 0.0, 29.0, 0.0);
        // The first frame of a new state is still the old pose; it arrives within the fade.
        let first = a.advance(anim::GUARD, 0.001, Vec3::ZERO, 0.0, 29.0, 0.0);
        assert!(first.rot[FOREARM_L].angle_between(idle.rot[FOREARM_L]) < 0.05);
        let mut last = first;
        for _ in 0..12 {
            last = a.advance(anim::GUARD, 0.016, Vec3::ZERO, 0.0, 29.0, 0.0);
        }
        let guard = at(anim::GUARD, 0.0, 0.0);
        assert!(last.rot[FOREARM_L].angle_between(guard.rot[FOREARM_L]) < 0.01);
        // Running advances the cycle with the distance covered, not with time.
        let mut r = Animator::new(anim::RUN);
        let quarter = Vec3::new(RUN_CYCLE_UNITS / 4.0, 0.0, 0.0);
        r.advance(anim::RUN, 0.016, quarter, 0.0, 29.0, 0.0);
        assert!((r.cycle - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
        r.advance(anim::RUN, 0.016, Vec3::new(0.0, 0.0, 50.0), 0.0, 29.0, 0.0);
        assert!(
            (r.cycle - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "falling is not running"
        );
    }

    #[test]
    fn armour_class_is_in_the_gait() {
        // Plate takes longer strides than cloth over the same ground...
        let (mut cloth, mut plate) = (Animator::new(anim::RUN), Animator::new(anim::RUN));
        let step = Vec3::new(40.0, 0.0, 0.0);
        cloth.advance(anim::RUN, 0.125, step, 0.0, 29.0, armour_weight(0));
        plate.advance(anim::RUN, 0.125, step, 0.0, 29.0, armour_weight(3));
        assert!(
            plate.cycle < cloth.cycle * 0.85,
            "{} vs {}",
            plate.cycle,
            cloth.cycle
        );
        // ...and swings its arms less at the same point of the cycle.
        let swing = |weight: f32| {
            let p = pose(&AnimInput {
                state: anim::RUN,
                t: 1.0,
                cycle: std::f32::consts::FRAC_PI_2,
                speed: FULL_SPEED,
                pitch: 0.0,
                hips_z: 29.0,
                weight,
            });
            let hang = pose(&AnimInput {
                state: anim::RUN,
                t: 1.0,
                cycle: 0.0,
                speed: FULL_SPEED,
                pitch: 0.0,
                hips_z: 29.0,
                weight,
            });
            p.rot[UPPER_ARM_L].angle_between(hang.rot[UPPER_ARM_L])
        };
        assert!(swing(1.0) < swing(0.0) * 0.7);
        assert_eq!(armour_weight(0), 0.0);
        assert_eq!(armour_weight(3), 1.0);
        assert_eq!(armour_weight(9), 1.0);
    }
}
