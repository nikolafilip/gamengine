//! The frame envelope (MODELS.md 3, 4): the pose a model must stand in, the box it must stay
//! inside and the silhouette it must fill. The reference is the frame's own mannequin.

use std::sync::OnceLock;

use glam::Vec3;
use gm_core::vocab::ArchetypeFrame;
use gm_model::Model;
use gm_model::mannequin::{self, Shape};
use gm_model::rig::{self, bone};

use crate::raster::{self, Dir, ModelSoup, Soup};

/// Limbs may point this far from their T-pose direction.
pub const LIMB_TOLERANCE_DEG: f32 = 15.0;
/// The spine and the neck may lean this far from upright.
pub const UPRIGHT_TOLERANCE_DEG: f32 = 30.0;
/// The head pivot's height, relative to the mannequin's.
pub const BODY_HEIGHT: (f32, f32) = (0.92, 1.08);
/// The highest vertex, in frame heights.
pub const TOP: (f32, f32) = (0.90, 1.15);
/// The box, in capsule radii (depth) and frame heights (span, floor).
pub const BOX_DEPTH: f32 = 2.5;
pub const BOX_SPAN: f32 = 0.70;
pub const BOX_FLOOR: f32 = -0.05;
/// Coverage of the hitbox rectangle, relative to the mannequin's.
pub const COVERAGE: (f32, f32) = (0.50, 1.50);

/// What a frame's mannequin measures.
#[derive(Clone, Copy, Debug)]
pub struct Reference {
    pub r: f32,
    pub h: f32,
    pub head_z: f32,
    /// Fraction of the hitbox rectangle covered from the front and from the side.
    pub front: f32,
    pub side: f32,
}

pub fn reference(frame: ArchetypeFrame) -> Reference {
    static CACHE: OnceLock<[Reference; 4]> = OnceLock::new();
    CACHE.get_or_init(|| {
        rig::FRAMES.map(|f| {
            let (r, h) = f.capsule();
            let m = mannequin::build(f, &Shape::MANNEQUIN);
            let soup = Soup {
                positions: &m.positions,
                normals: &m.normals,
                uvs: &m.uvs,
                indices: &m.indices,
                two_sided: false,
            };
            let all = |_: f32, _: f32| true;
            Reference {
                r,
                h,
                head_z: m.pivots[bone::HEAD].z,
                front: raster::coverage(&soup, Dir::Front, r, h, &all),
                side: raster::coverage(&soup, Dir::Left, r, h, &all),
            }
        })
    })[rig::frame_index(frame) as usize]
}

fn angle_deg(a: Vec3, b: Vec3) -> f32 {
    a.normalize_or_zero()
        .dot(b.normalize_or_zero())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// The pose rules. They read the pivots and nothing else, so they hold for the upload's mesh
/// and for a stored model alike.
pub fn check_pose(
    p: &[Vec3; rig::BONES],
    bone_mask: u32,
    frame: ArchetypeFrame,
    out: &mut Vec<String>,
) {
    let has = |b: usize| bone_mask & (1 << b) != 0;
    let reference = reference(frame);
    for (b, child, dir) in rig::LIMB_DIRECTIONS {
        if !has(b) || !has(child) {
            continue;
        }
        let off = angle_deg(p[child] - p[b], dir);
        if off.is_nan() || off > LIMB_TOLERANCE_DEG {
            out.push(format!(
                "`{}` points {off:.0}° away from the T-pose (the limit is {LIMB_TOLERANCE_DEG:.0}°); export the model in T-pose: arms straight out, legs straight down",
                rig::NAMES[b]
            ));
        }
    }
    for (a, b, what) in [
        (bone::HIPS, bone::CHEST, "the spine"),
        (bone::CHEST, bone::HEAD, "the neck"),
    ] {
        let off = angle_deg(p[b] - p[a], Vec3::Z);
        if off.is_nan() || off > UPRIGHT_TOLERANCE_DEG {
            out.push(format!(
                "{what} leans {off:.0}° from upright (the limit is {UPRIGHT_TOLERANCE_DEG:.0}°); the model must stand, +Y up and facing +Z in glTF"
            ));
        }
    }
    for (l, r) in [
        (bone::UPPER_ARM_L, bone::UPPER_ARM_R),
        (bone::THIGH_L, bone::THIGH_R),
    ] {
        if p[l].y <= p[r].y {
            out.push(format!(
                "`{}` is on the right of `{}`: the model is mirrored or faces backward",
                rig::NAMES[l],
                rig::NAMES[r]
            ));
        }
    }
    let head = p[bone::HEAD].z / reference.head_z;
    if !(BODY_HEIGHT.0..=BODY_HEIGHT.1).contains(&head) {
        out.push(format!(
            "the `head` pivot is at {:.1} u; a {} body has it between {:.1} and {:.1} (glTF is in metres, 32 u each)",
            p[bone::HEAD].z,
            rig::frame_name(frame),
            reference.head_z * BODY_HEIGHT.0,
            reference.head_z * BODY_HEIGHT.1
        ));
    }
}

/// What the ingested model measures.
#[derive(Clone, Copy, Debug, Default)]
pub struct Measured {
    pub top: f32,
    /// Coverage relative to the mannequin's: the least of front and back, of left and right.
    pub front: f32,
    pub side: f32,
}

/// The box and the coverage rule, on the model as it will be stored.
pub fn check_model(model: &Model, frame: ArchetypeFrame, out: &mut Vec<String>) -> Measured {
    let reference = reference(frame);
    let (r, h) = (reference.r, reference.h);
    let soup = ModelSoup::new(model);
    let mut top = f32::MIN;
    let mut outside = 0usize;
    for p in &soup.positions {
        top = top.max(p.z);
        if p.x.abs() > BOX_DEPTH * r + 0.01
            || p.y.abs() > BOX_SPAN * h + 0.01
            || p.z < BOX_FLOOR * h - 0.01
        {
            outside += 1;
        }
    }
    if !(TOP.0 * h..=TOP.1 * h).contains(&top) {
        out.push(format!(
            "the highest vertex is at {top:.1} u; a {} model reaches between {:.1} and {:.1}",
            rig::frame_name(frame),
            TOP.0 * h,
            TOP.1 * h
        ));
    }
    if outside > 0 {
        out.push(format!(
            "{outside} vertices are outside the box of a {}: {:.0} u front and back, {:.0} u to each side, {:.0} u below the ground",
            rig::frame_name(frame),
            BOX_DEPTH * r,
            BOX_SPAN * h,
            -BOX_FLOOR * h
        ));
    }
    let cut = model.cutout();
    let opaque = |u: f32, v: f32| !cut || raster::sample(model, u, v)[3] >= 128;
    let cover = |dir: Dir| raster::coverage(&soup.soup(), dir, r, h, &opaque);
    let front = cover(Dir::Front).min(cover(Dir::Back)) / reference.front;
    let side = cover(Dir::Left).min(cover(Dir::Right)) / reference.side;
    for (what, c) in [("front", front), ("side", side)] {
        if c < COVERAGE.0 {
            out.push(format!(
                "from the {what} the model covers {:.0}% of what the {} mannequin covers inside the hitbox; at least {:.0}% is required (an avatar must be visible where it can be hit)",
                c * 100.0,
                rig::frame_name(frame),
                COVERAGE.0 * 100.0
            ));
        } else if c > COVERAGE.1 {
            out.push(format!(
                "from the {what} the model covers {:.0}% of what the {} mannequin covers inside the hitbox; at most {:.0}% is allowed (bulk is how a frame reads)",
                c * 100.0,
                rig::frame_name(frame),
                COVERAGE.1 * 100.0
            ));
        }
    }
    Measured { top, front, side }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mannequins_define_the_reference() {
        for frame in rig::FRAMES {
            let r = reference(frame);
            // A humanoid fills between a quarter and three quarters of its hitbox rectangle.
            assert!(
                (0.25..0.75).contains(&r.front),
                "{frame:?} front {}",
                r.front
            );
            assert!((0.15..0.60).contains(&r.side), "{frame:?} side {}", r.side);
            assert!(r.front > r.side, "a body is wider than it is deep");
        }
        // A colossus is bulkier than an infiltrator in absolute terms.
        let area = |f: ArchetypeFrame| {
            let r = reference(f);
            r.front * 2.0 * r.r * r.h
        };
        assert!(area(ArchetypeFrame::Colossus) > 1.4 * area(ArchetypeFrame::Infiltrator));
    }

    #[test]
    fn the_mannequin_stands_in_the_required_pose() {
        for frame in rig::FRAMES {
            let m = mannequin::build(frame, &Shape::MANNEQUIN);
            let mut v = Vec::new();
            check_pose(&m.pivots, m.bone_mask, frame, &mut v);
            assert!(v.is_empty(), "{frame:?}: {v:?}");
        }
    }

    #[test]
    fn a_wrong_pose_is_named() {
        let frame = ArchetypeFrame::Striker;
        let mut m = mannequin::build(frame, &Shape::MANNEQUIN);
        // An A-pose: the left forearm joint 45° below the shoulder line.
        let len = (m.pivots[bone::FOREARM_L] - m.pivots[bone::UPPER_ARM_L]).length();
        m.pivots[bone::FOREARM_L] =
            m.pivots[bone::UPPER_ARM_L] + Vec3::new(0.0, 0.707, -0.707) * len;
        // Lying face down: the head ahead of the chest instead of above it.
        m.pivots[bone::HEAD] = m.pivots[bone::CHEST] + Vec3::new(10.0, 0.0, 1.0);
        // Mirrored legs.
        m.pivots.swap(bone::THIGH_L, bone::THIGH_R);
        let mut v = Vec::new();
        check_pose(&m.pivots, m.bone_mask, frame, &mut v);
        let text = v.join("\n");
        assert!(text.contains("`upper_arm_l` points 45°"), "{text}");
        assert!(text.contains("the neck leans"), "{text}");
        assert!(text.contains("`thigh_l` is on the right"), "{text}");
        assert!(text.contains("the `head` pivot is at"), "{text}");
    }
}
