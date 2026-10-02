//! Poses and skinning matrices (MODELS.md 2). A pose is a model-space rotation per bone about
//! its pivot, relative to the T-pose, plus an offset of the whole body. Because every bone's
//! rest frame is the model's own axes, the same pose drives any skeleton that has the standard
//! bones, whatever its proportions and whatever tool exported it.

use glam::{Mat4, Quat, Vec3};

use crate::rig::{BONES, NO_PARENT, PARENTS};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Translation of the whole body (bob, crouch, lying down).
    pub offset: Vec3,
    pub rot: [Quat; BONES],
}

impl Default for Pose {
    fn default() -> Self {
        Pose::REST
    }
}

impl Pose {
    /// The T-pose.
    pub const REST: Pose = Pose {
        offset: Vec3::ZERO,
        rot: [Quat::IDENTITY; BONES],
    };

    /// Shortest-path blend: `self` at 0, `other` at 1.
    pub fn blend(&self, other: &Pose, t: f32) -> Pose {
        let t = t.clamp(0.0, 1.0);
        let mut out = *self;
        out.offset = self.offset.lerp(other.offset, t);
        for (o, b) in out.rot.iter_mut().zip(&other.rot) {
            *o = o.lerp(*b, t);
        }
        out
    }
}

/// The skinning matrix of every bone: `D(bone) = D(parent) · T(pivot) · R · T(−pivot)`, with
/// the body offset applied at the root. A bone missing from `mask` behaves as its parent.
pub fn skin_matrices(pivots: &[Vec3; BONES], mask: u32, pose: &Pose) -> [Mat4; BONES] {
    let mut out = [Mat4::IDENTITY; BONES];
    for b in 0..BONES {
        let parent = match PARENTS[b] {
            NO_PARENT => Mat4::from_translation(pose.offset),
            p => out[p as usize],
        };
        out[b] = if mask & (1 << b) == 0 {
            parent
        } else {
            let (q, p) = (pose.rot[b], pivots[b]);
            parent * Mat4::from_rotation_translation(q, p - q * p)
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{self, ALL_BONES, bone};
    use gm_core::vocab::ArchetypeFrame;

    #[test]
    fn the_rest_pose_moves_nothing() {
        let pivots = rig::rest_pivots(ArchetypeFrame::Striker);
        for m in skin_matrices(&pivots, ALL_BONES, &Pose::REST) {
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-6));
        }
    }

    #[test]
    fn a_bone_rotates_about_its_pivot_and_carries_its_children() {
        let pivots = rig::rest_pivots(ArchetypeFrame::Striker);
        let mut pose = Pose::REST;
        // Lower the left arm: +Y swings to −Z about the shoulder.
        pose.rot[bone::UPPER_ARM_L] = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let m = skin_matrices(&pivots, ALL_BONES, &pose);
        let shoulder = pivots[bone::UPPER_ARM_L];
        let elbow = pivots[bone::FOREARM_L];
        let len = (elbow - shoulder).length();
        // The shoulder stays put; the elbow hangs straight below it; the hand follows.
        assert!(
            m[bone::UPPER_ARM_L]
                .transform_point3(shoulder)
                .abs_diff_eq(shoulder, 1e-4)
        );
        let moved = m[bone::FOREARM_L].transform_point3(elbow);
        assert!(
            moved.abs_diff_eq(shoulder - Vec3::Z * len, 1e-3),
            "{moved:?}"
        );
        let hand = m[bone::HAND_L].transform_point3(pivots[bone::HAND_L]);
        assert!(hand.z < moved.z && (hand.y - shoulder.y).abs() < 1e-3);
        // The other arm and the legs are untouched.
        assert!(m[bone::UPPER_ARM_R].abs_diff_eq(Mat4::IDENTITY, 1e-6));
        assert!(m[bone::THIGH_L].abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }

    #[test]
    fn an_absent_bone_follows_its_parent() {
        let pivots = rig::rest_pivots(ArchetypeFrame::Caster);
        let mut pose = Pose::REST;
        pose.rot[bone::CLAVICLE_L] = Quat::from_rotation_z(0.5);
        pose.rot[bone::CHEST] = Quat::from_rotation_z(0.2);
        let mask = ALL_BONES & !(1 << bone::CLAVICLE_L);
        let m = skin_matrices(&pivots, mask, &pose);
        assert!(m[bone::CLAVICLE_L].abs_diff_eq(m[bone::CHEST], 1e-6));
        assert!(m[bone::UPPER_ARM_L].abs_diff_eq(m[bone::CHEST], 1e-6));
    }

    #[test]
    fn blending_takes_the_short_way_and_moves_the_offset() {
        let mut a = Pose::REST;
        let mut b = Pose::REST;
        a.rot[0] = Quat::from_rotation_z(0.1);
        b.rot[0] = -Quat::from_rotation_z(0.3); // the same rotation, negated
        b.offset = Vec3::new(0.0, 0.0, -10.0);
        let mid = a.blend(&b, 0.5);
        let expect = Quat::from_rotation_z(0.2);
        assert!(mid.rot[0].abs_diff_eq(expect, 1e-3) || mid.rot[0].abs_diff_eq(-expect, 1e-3));
        assert_eq!(mid.offset.z, -5.0);
        assert_eq!(a.blend(&b, 0.0), a);
    }
}
