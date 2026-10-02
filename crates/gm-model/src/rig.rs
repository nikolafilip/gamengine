//! The standard rig (MODELS.md 2): 24 bones with fixed indices and parents. A bone is its pivot
//! and nothing else; joint axes do not exist here.

use glam::Vec3;
use gm_core::vocab::ArchetypeFrame;

pub const BONES: usize = 24;
pub const NO_PARENT: u8 = 0xff;

/// Bone indices by name.
pub mod bone {
    pub const HIPS: usize = 0;
    pub const SPINE: usize = 1;
    pub const CHEST: usize = 2;
    pub const NECK: usize = 3;
    pub const HEAD: usize = 4;
    pub const CLAVICLE_L: usize = 5;
    pub const UPPER_ARM_L: usize = 6;
    pub const FOREARM_L: usize = 7;
    pub const HAND_L: usize = 8;
    pub const CLAVICLE_R: usize = 9;
    pub const UPPER_ARM_R: usize = 10;
    pub const FOREARM_R: usize = 11;
    pub const HAND_R: usize = 12;
    pub const THIGH_L: usize = 13;
    pub const SHIN_L: usize = 14;
    pub const FOOT_L: usize = 15;
    pub const TOE_L: usize = 16;
    pub const THIGH_R: usize = 17;
    pub const SHIN_R: usize = 18;
    pub const FOOT_R: usize = 19;
    pub const TOE_R: usize = 20;
    pub const PROP_R: usize = 21;
    pub const PROP_L: usize = 22;
    pub const PROP_BACK: usize = 23;
}

pub const NAMES: [&str; BONES] = [
    "hips",
    "spine",
    "chest",
    "neck",
    "head",
    "clavicle_l",
    "upper_arm_l",
    "forearm_l",
    "hand_l",
    "clavicle_r",
    "upper_arm_r",
    "forearm_r",
    "hand_r",
    "thigh_l",
    "shin_l",
    "foot_l",
    "toe_l",
    "thigh_r",
    "shin_r",
    "foot_r",
    "toe_r",
    "prop_r",
    "prop_l",
    "prop_back",
];

/// Parent of each bone; parents always have the lower index, so one forward pass composes.
pub const PARENTS: [u8; BONES] = [
    NO_PARENT, // hips
    0,         // spine
    1,         // chest
    2,         // neck
    3,         // head
    2,         // clavicle_l
    5,         // upper_arm_l
    6,         // forearm_l
    7,         // hand_l
    2,         // clavicle_r
    9,         // upper_arm_r
    10,        // forearm_r
    11,        // hand_r
    0,         // thigh_l
    13,        // shin_l
    14,        // foot_l
    15,        // toe_l
    0,         // thigh_r
    17,        // shin_r
    18,        // foot_r
    19,        // toe_r
    12,        // prop_r
    8,         // prop_l
    2,         // prop_back
];

/// Bones every model must have.
pub const REQUIRED: u32 = (1 << bone::HIPS)
    | (1 << bone::SPINE)
    | (1 << bone::CHEST)
    | (1 << bone::HEAD)
    | (1 << bone::UPPER_ARM_L)
    | (1 << bone::FOREARM_L)
    | (1 << bone::UPPER_ARM_R)
    | (1 << bone::FOREARM_R)
    | (1 << bone::THIGH_L)
    | (1 << bone::SHIN_L)
    | (1 << bone::THIGH_R)
    | (1 << bone::SHIN_R);

pub const ALL_BONES: u32 = (1 << BONES) - 1;

pub fn parent(bone: usize) -> Option<usize> {
    match PARENTS[bone] {
        NO_PARENT => None,
        p => Some(p as usize),
    }
}

/// Lowercase with every run of non-alphanumerics read as one `_` (MODELS.md 3), so Blender's
/// `upper_arm.L` and `Upper Arm L` both name `upper_arm_l`.
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    out
}

/// The standard bone a joint name refers to, if any.
pub fn bone_by_name(name: &str) -> Option<usize> {
    let n = normalize_name(name);
    NAMES.iter().position(|b| *b == n)
}

pub fn frame_index(frame: ArchetypeFrame) -> u8 {
    match frame {
        ArchetypeFrame::Colossus => 0,
        ArchetypeFrame::Striker => 1,
        ArchetypeFrame::Caster => 2,
        ArchetypeFrame::Infiltrator => 3,
    }
}

pub fn frame_from_index(i: u8) -> Option<ArchetypeFrame> {
    match i {
        0 => Some(ArchetypeFrame::Colossus),
        1 => Some(ArchetypeFrame::Striker),
        2 => Some(ArchetypeFrame::Caster),
        3 => Some(ArchetypeFrame::Infiltrator),
        _ => None,
    }
}

pub const FRAMES: [ArchetypeFrame; 4] = [
    ArchetypeFrame::Colossus,
    ArchetypeFrame::Striker,
    ArchetypeFrame::Caster,
    ArchetypeFrame::Infiltrator,
];

pub fn frame_name(frame: ArchetypeFrame) -> &'static str {
    match frame {
        ArchetypeFrame::Colossus => "colossus",
        ArchetypeFrame::Striker => "striker",
        ArchetypeFrame::Caster => "caster",
        ArchetypeFrame::Infiltrator => "infiltrator",
    }
}

pub fn frame_by_name(name: &str) -> Option<ArchetypeFrame> {
    FRAMES.into_iter().find(|f| frame_name(*f) == name)
}

/// T-pose directions of the limbs the ingestion aligns (MODELS.md 3): `(bone, child, direction)`.
pub const LIMB_DIRECTIONS: [(usize, usize, Vec3); 8] = [
    (bone::UPPER_ARM_L, bone::FOREARM_L, Vec3::Y),
    (bone::FOREARM_L, bone::HAND_L, Vec3::Y),
    (bone::UPPER_ARM_R, bone::FOREARM_R, Vec3::NEG_Y),
    (bone::FOREARM_R, bone::HAND_R, Vec3::NEG_Y),
    (bone::THIGH_L, bone::SHIN_L, Vec3::NEG_Z),
    (bone::SHIN_L, bone::FOOT_L, Vec3::NEG_Z),
    (bone::THIGH_R, bone::SHIN_R, Vec3::NEG_Z),
    (bone::SHIN_R, bone::FOOT_R, Vec3::NEG_Z),
];

/// The mannequin's skeleton for a frame: pivots in model space (feet on the ground at z = 0,
/// facing +X, +Y left), proportioned from the hitbox capsule `(r, h)`.
pub fn rest_pivots(frame: ArchetypeFrame) -> [Vec3; BONES] {
    let (r, h) = frame.capsule();
    let mut p = [Vec3::ZERO; BONES];
    p[bone::HIPS] = Vec3::new(0.0, 0.0, 0.52 * h);
    p[bone::SPINE] = Vec3::new(0.0, 0.0, 0.60 * h);
    p[bone::CHEST] = Vec3::new(0.0, 0.0, 0.70 * h);
    p[bone::NECK] = Vec3::new(0.0, 0.0, 0.845 * h);
    p[bone::HEAD] = Vec3::new(0.0, 0.0, 0.885 * h);
    p[bone::PROP_BACK] = Vec3::new(-0.40 * r, 0.0, 0.75 * h);
    for (side, clav, upper, fore, hand, prop, thigh, shin, foot, toe) in [
        (
            1.0f32,
            bone::CLAVICLE_L,
            bone::UPPER_ARM_L,
            bone::FOREARM_L,
            bone::HAND_L,
            bone::PROP_L,
            bone::THIGH_L,
            bone::SHIN_L,
            bone::FOOT_L,
            bone::TOE_L,
        ),
        (
            -1.0,
            bone::CLAVICLE_R,
            bone::UPPER_ARM_R,
            bone::FOREARM_R,
            bone::HAND_R,
            bone::PROP_R,
            bone::THIGH_R,
            bone::SHIN_R,
            bone::FOOT_R,
            bone::TOE_R,
        ),
    ] {
        let shoulder = 0.62 * r;
        let arm_z = 0.815 * h;
        p[clav] = Vec3::new(0.0, side * 0.12 * r, arm_z);
        p[upper] = Vec3::new(0.0, side * shoulder, arm_z);
        p[fore] = Vec3::new(0.0, side * (shoulder + 0.155 * h), arm_z);
        p[hand] = Vec3::new(0.0, side * (shoulder + 0.300 * h), arm_z);
        p[prop] = Vec3::new(0.0, side * (shoulder + 0.350 * h), arm_z);
        let leg = side * 0.30 * r;
        p[thigh] = Vec3::new(0.0, leg, 0.50 * h);
        p[shin] = Vec3::new(0.0, leg, 0.275 * h);
        p[foot] = Vec3::new(0.0, leg, 0.05 * h);
        p[toe] = Vec3::new(0.09 * h, leg, 0.015 * h);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parents_precede_children_and_names_are_unique() {
        for (i, &p) in PARENTS.iter().enumerate() {
            if p != NO_PARENT {
                assert!((p as usize) < i, "{} has parent {}", NAMES[i], p);
            }
        }
        assert_eq!(PARENTS.iter().filter(|p| **p == NO_PARENT).count(), 1);
        for (i, a) in NAMES.iter().enumerate() {
            assert_eq!(bone_by_name(a), Some(i));
            assert_eq!(NAMES.iter().filter(|b| *b == a).count(), 1);
        }
    }

    #[test]
    fn names_match_common_spellings() {
        assert_eq!(bone_by_name("upper_arm.L"), Some(bone::UPPER_ARM_L));
        assert_eq!(bone_by_name("Upper Arm L"), Some(bone::UPPER_ARM_L));
        assert_eq!(bone_by_name("HIPS"), Some(bone::HIPS));
        assert_eq!(bone_by_name("thigh-r"), Some(bone::THIGH_R));
        assert_eq!(bone_by_name("finger_01_l"), None);
        assert_eq!(bone_by_name(""), None);
    }

    #[test]
    fn rest_pivots_are_symmetric_and_inside_the_frame() {
        for frame in FRAMES {
            let (r, h) = frame.capsule();
            let p = rest_pivots(frame);
            assert_eq!(p[bone::UPPER_ARM_L].y, -p[bone::UPPER_ARM_R].y);
            assert!(p[bone::HAND_L].y > p[bone::FOREARM_L].y);
            assert!(p[bone::HAND_L].y < 0.70 * h, "arm span inside the envelope");
            assert!(p[bone::HEAD].z < h);
            assert!(p[bone::THIGH_L].y < r);
            for (b, child, dir) in LIMB_DIRECTIONS {
                let d = (p[child] - p[b]).normalize();
                assert!(d.dot(dir) > 0.999, "{} is not along the T-pose", NAMES[b]);
            }
        }
    }

    #[test]
    fn frames_round_trip() {
        for f in FRAMES {
            assert_eq!(frame_from_index(frame_index(f)), Some(f));
            assert_eq!(frame_by_name(frame_name(f)), Some(f));
        }
        assert_eq!(frame_from_index(4), None);
    }
}
