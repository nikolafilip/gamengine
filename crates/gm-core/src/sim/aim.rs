//! Where to aim at a moving body: the lead a mind takes (COMPANIONS.md 5) and the one a
//! target-action is given (MODES.md 5.3), the same arithmetic on both.

use glam::Vec3;

/// Where to aim a bolt of `speed` and `gravity` scale, launched from `from`, to meet a
/// body whose centre is `centre` moving at `velocity`: two rounds of the flight time, then
/// the drop over it added back.
pub fn lead(from: Vec3, centre: Vec3, velocity: Vec3, speed: f32, gravity: f32) -> Vec3 {
    let mut p = centre;
    let mut t = 0.0;
    for _ in 0..2 {
        t = (p - from).length() / speed.max(1.0);
        p = centre + velocity * t;
    }
    p.z += 0.5 * crate::movement::MoveVars::QUAKE.gravity * gravity * t * t;
    p
}
