//! The mannequin (MODELS.md 9): a jointed figure generated from the rig. The client draws it
//! for every entity whose model is not ready (PLAN.md 2.9); ingestion measures uploads against
//! its silhouette (MODELS.md 4); a finer `Shape` gives the template and the test avatars.

use glam::Vec3;
use gm_core::vocab::ArchetypeFrame;

use crate::rig::{self, ALL_BONES, BONES, bone};

/// A mesh in model space, before quantisation.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u8; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub pivots: [Vec3; BONES],
    pub bone_mask: u32,
}

impl MeshData {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Body parts: each owns a rectangle of the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Torso,
    Head,
    Neck,
    UpperArmL,
    ForearmL,
    UpperArmR,
    ForearmR,
    ThighL,
    ShinL,
    ThighR,
    ShinR,
    HandL,
    HandR,
    FootL,
    FootR,
}

impl Part {
    pub const ALL: [Part; 15] = [
        Part::Torso,
        Part::Head,
        Part::Neck,
        Part::UpperArmL,
        Part::ForearmL,
        Part::UpperArmR,
        Part::ForearmR,
        Part::ThighL,
        Part::ShinL,
        Part::ThighR,
        Part::ShinR,
        Part::HandL,
        Part::HandR,
        Part::FootL,
        Part::FootR,
    ];

    /// `(u0, v0, u1, v1)` of the part in the atlas.
    pub fn rect(self) -> [f32; 4] {
        match self {
            Part::Torso => [0.0, 0.0, 0.5, 0.5],
            Part::Head => [0.5, 0.0, 0.75, 0.25],
            Part::Neck => [0.75, 0.0, 0.875, 0.125],
            Part::HandL => [0.75, 0.125, 0.875, 0.25],
            Part::HandR => [0.875, 0.125, 1.0, 0.25],
            Part::UpperArmL => [0.5, 0.25, 0.75, 0.5],
            Part::ForearmL => [0.75, 0.25, 1.0, 0.5],
            Part::UpperArmR => [0.0, 0.5, 0.25, 0.75],
            Part::ForearmR => [0.25, 0.5, 0.5, 0.75],
            Part::ThighL => [0.5, 0.5, 0.75, 0.75],
            Part::ShinL => [0.75, 0.5, 1.0, 0.75],
            Part::ThighR => [0.0, 0.75, 0.25, 1.0],
            Part::ShinR => [0.25, 0.75, 0.5, 1.0],
            Part::FootL => [0.5, 0.75, 0.625, 0.875],
            Part::FootR => [0.625, 0.75, 0.75, 0.875],
        }
    }

    /// The part whose rectangle holds `(u, v)`.
    pub fn at(u: f32, v: f32) -> Option<Part> {
        Part::ALL.into_iter().find(|p| {
            let [u0, v0, u1, v1] = p.rect();
            u >= u0 && u < u1 && v >= v0 && v < v1
        })
    }
}

/// How the figure is built. `MANNEQUIN` is the client's stand-in; `detailed` fills the
/// triangle budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Sides of every limb and of the torso.
    pub sides: u32,
    /// Rings along each limb segment.
    pub rings: u32,
    /// Smooth weights at the joints instead of rigid segments.
    pub blend: bool,
    /// Multipliers on the reference proportions.
    pub height: f32,
    pub girth: f32,
    pub shoulders: f32,
    pub head: f32,
    pub belly: f32,
}

impl Shape {
    pub const MANNEQUIN: Shape = Shape {
        sides: 8,
        rings: 3,
        blend: false,
        height: 1.0,
        girth: 1.0,
        shoulders: 1.0,
        head: 1.0,
        belly: 1.0,
    };

    /// About 3,480 triangles: the ceiling of PLAN.md 2.6.
    pub const fn detailed() -> Shape {
        Shape {
            sides: 21,
            rings: 6,
            blend: true,
            ..Shape::MANNEQUIN
        }
    }
}

struct Builder {
    mesh: MeshData,
    blend: bool,
}

/// One ring of a lathe: position along the axis (0..=1) and the two radii.
#[derive(Clone, Copy)]
struct Ring {
    t: f32,
    ru: f32,
    rv: f32,
}

fn ring(t: f32, ru: f32, rv: f32) -> Ring {
    Ring { t, ru, rv }
}

impl Builder {
    fn vertex(&mut self, p: Vec3, n: Vec3, uv: [f32; 2], bones: [(usize, f32); 2]) -> u32 {
        let m = &mut self.mesh;
        m.positions.push(p);
        m.normals.push(n.normalize_or(Vec3::Z));
        m.uvs.push(uv);
        m.joints.push([bones[0].0 as u8, bones[1].0 as u8, 0, 0]);
        m.weights.push([bones[0].1, bones[1].1, 0.0, 0.0]);
        (m.positions.len() - 1) as u32
    }

    /// A closed tube from `a` to `b` with elliptical rings (`ru` along `u_dir`, `rv` along
    /// `axis × u_dir`), capped at both ends. `weights(t)` gives the two bones of a ring.
    #[allow(clippy::too_many_arguments)]
    fn lathe(
        &mut self,
        part: Part,
        a: Vec3,
        b: Vec3,
        u_dir: Vec3,
        rings: &[Ring],
        sides: u32,
        weights: &dyn Fn(f32) -> [(usize, f32); 2],
    ) {
        let axis = (b - a).normalize();
        let e1 = (u_dir - axis * u_dir.dot(axis)).normalize();
        let e2 = axis.cross(e1);
        let [u0, v0, u1, v1] = part.rect();
        // A margin keeps the mips of one part out of its neighbours.
        let (mu, mv) = ((u1 - u0) * 0.04, (v1 - v0) * 0.04);
        let uv = |s: f32, t: f32| {
            [
                u0 + mu + (u1 - u0 - 2.0 * mu) * s,
                v0 + mv + (v1 - v0 - 2.0 * mv) * (0.1 + 0.8 * t),
            ]
        };
        let mut starts = Vec::with_capacity(rings.len());
        for r in rings {
            let centre = a + (b - a) * r.t;
            let w = weights(r.t);
            starts.push(self.mesh.positions.len() as u32);
            // The seam vertex is doubled so the texture wraps once. The seam is on the −u side
            // (the back of a body), so the middle of the part's rectangle is its front.
            for k in 0..=sides {
                let s = k as f32 / sides as f32;
                let (sin, cos) = ((s + 0.5) * std::f32::consts::TAU).sin_cos();
                let p = centre + e1 * (r.ru * cos) + e2 * (r.rv * sin);
                let n = e1 * (cos / r.ru.max(1e-3)) + e2 * (sin / r.rv.max(1e-3));
                self.vertex(p, n, uv(s, r.t), w);
            }
        }
        for pair in starts.windows(2) {
            for k in 0..sides {
                let (p0, p1) = (pair[0] + k, pair[0] + k + 1);
                let (q0, q1) = (pair[1] + k, pair[1] + k + 1);
                self.mesh
                    .indices
                    .extend_from_slice(&[p0, p1, q1, p0, q1, q0]);
            }
        }
        // Caps: a fan around the ring's centre, facing away from the tube.
        for (r, start, dir) in [
            (rings[0], starts[0], -axis),
            (rings[rings.len() - 1], starts[starts.len() - 1], axis),
        ] {
            let centre = a + (b - a) * r.t;
            let w = weights(r.t);
            let cap_uv = uv(0.5, if dir.dot(axis) < 0.0 { 0.0 } else { 1.0 });
            let c = self.vertex(centre, dir, cap_uv, w);
            let first = self.mesh.positions.len() as u32;
            for k in 0..sides {
                let s = k as f32 / sides as f32;
                let (sin, cos) = (s * std::f32::consts::TAU).sin_cos();
                let p = centre + e1 * (r.ru * cos) + e2 * (r.rv * sin);
                self.vertex(p, dir, cap_uv, w);
            }
            for k in 0..sides {
                let (p0, p1) = (first + k, first + (k + 1) % sides);
                if dir.dot(axis) < 0.0 {
                    self.mesh.indices.extend_from_slice(&[c, p1, p0]);
                } else {
                    self.mesh.indices.extend_from_slice(&[c, p0, p1]);
                }
            }
            let _ = start;
        }
    }

    /// A limb segment bound to `bone`, blending into `before` at its start and `after` at its
    /// end when the shape asks for smooth joints.
    #[allow(clippy::too_many_arguments)]
    fn limb(
        &mut self,
        part: Part,
        a: Vec3,
        b: Vec3,
        u_dir: Vec3,
        radii: [(f32, f32); 2],
        shape: &Shape,
        bone: usize,
        before: Option<usize>,
        after: Option<usize>,
    ) {
        let n = shape.rings.max(2);
        let rings: Vec<Ring> = (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                // A slight swell in the middle: muscle, not pipe.
                let swell = 1.0 + 0.10 * (t * std::f32::consts::PI).sin();
                ring(
                    t,
                    (radii[0].0 + (radii[1].0 - radii[0].0) * t) * swell,
                    (radii[0].1 + (radii[1].1 - radii[0].1) * t) * swell,
                )
            })
            .collect();
        let blend = self.blend;
        let weights = move |t: f32| -> [(usize, f32); 2] {
            if !blend {
                return [(bone, 1.0), (0, 0.0)];
            }
            let band = 0.3;
            if let Some(prev) = before
                && t < band
            {
                let w = 0.5 * (1.0 - t / band);
                return [(bone, 1.0 - w), (prev, w)];
            }
            if let Some(next) = after
                && t > 1.0 - band
            {
                let w = 0.5 * (t - (1.0 - band)) / band;
                return [(bone, 1.0 - w), (next, w)];
            }
            [(bone, 1.0), (0, 0.0)]
        };
        self.lathe(part, a, b, u_dir, &rings, shape.sides, &weights);
    }
}

/// Build the figure for `frame`.
pub fn build(frame: ArchetypeFrame, shape: &Shape) -> MeshData {
    let (r, h0) = frame.capsule();
    let h = h0 * shape.height;
    let g = shape.girth;
    let mut pivots = rig::rest_pivots(frame);
    for p in &mut pivots {
        *p *= shape.height;
        p.y *= if p.y.abs() > 0.3 * r {
            shape.shoulders.max(0.5)
        } else {
            1.0
        };
    }
    // Keep the arms attached to the (possibly wider) shoulders, and the legs where they were.
    for b in [
        bone::THIGH_L,
        bone::SHIN_L,
        bone::FOOT_L,
        bone::TOE_L,
        bone::THIGH_R,
        bone::SHIN_R,
        bone::FOOT_R,
        bone::TOE_R,
    ] {
        pivots[b].y = rig::rest_pivots(frame)[b].y * shape.height;
    }
    let mut b = Builder {
        mesh: MeshData {
            pivots,
            bone_mask: ALL_BONES,
            ..Default::default()
        },
        blend: shape.blend,
    };
    let sides = shape.sides.max(3);

    // Torso: one tube from the crotch to the base of the neck, widest at the chest.
    {
        let (z0, z1) = (0.455 * h, 0.845 * h);
        let at = |z: f32| (z * h - z0) / (z1 - z0);
        let wide = 0.58 * r * shape.shoulders;
        let deep = 0.36 * r * g;
        let n = (shape.rings + 3).max(5);
        let profile = [
            (0.455, 0.50 * r * g, 0.30 * r * g),
            (0.52, 0.52 * r * g, deep * 0.95 * shape.belly),
            (0.61, 0.44 * r * g * shape.belly, deep * 0.90 * shape.belly),
            (0.71, wide * 0.92, deep),
            (0.80, wide, deep * 0.92),
            (0.845, wide * 0.55, deep * 0.55),
        ];
        let sample = |z: f32| -> (f32, f32) {
            let mut i = 0;
            while i + 2 < profile.len() && z > profile[i + 1].0 {
                i += 1;
            }
            let (za, wa, da) = profile[i];
            let (zb, wb, db) = profile[i + 1];
            let t = ((z - za) / (zb - za)).clamp(0.0, 1.0);
            (wa + (wb - wa) * t, da + (db - da) * t)
        };
        let rings: Vec<Ring> = (0..n)
            .map(|i| {
                let z = 0.455 + (0.845 - 0.455) * i as f32 / (n - 1) as f32;
                let (w, d) = sample(z);
                // `ru` runs along X (depth), `rv` along Y (width).
                ring(at(z), d, w)
            })
            .collect();
        let (spine_z, chest_z) = (pivots[bone::SPINE].z, pivots[bone::CHEST].z);
        let blend = shape.blend;
        let weights = move |t: f32| -> [(usize, f32); 2] {
            let z = z0 + (z1 - z0) * t;
            if !blend {
                let bone = if z < spine_z {
                    bone::HIPS
                } else if z < chest_z {
                    bone::SPINE
                } else {
                    bone::CHEST
                };
                return [(bone, 1.0), (0, 0.0)];
            }
            // Hips at the bottom, the spine around its pivot, the chest from its pivot up.
            let mid = (spine_z + chest_z) * 0.5;
            if z < mid {
                let w = ((z - (spine_z - 0.04 * h)) / (mid - (spine_z - 0.04 * h))).clamp(0.0, 1.0);
                [(bone::HIPS, 1.0 - w), (bone::SPINE, w)]
            } else {
                let w = ((z - mid) / (chest_z + 0.03 * h - mid)).clamp(0.0, 1.0);
                [(bone::SPINE, 1.0 - w), (bone::CHEST, w)]
            }
        };
        b.lathe(
            Part::Torso,
            Vec3::new(0.0, 0.0, z0),
            Vec3::new(0.0, 0.0, z1),
            Vec3::X,
            &rings,
            sides,
            &weights,
        );
    }

    // Neck and head.
    let neck_r = 0.17 * r * g;
    b.limb(
        Part::Neck,
        Vec3::new(0.0, 0.0, 0.83 * h),
        Vec3::new(0.0, 0.0, 0.895 * h),
        Vec3::X,
        [(neck_r, neck_r), (neck_r * 0.9, neck_r * 0.9)],
        &Shape { rings: 2, ..*shape },
        bone::NECK,
        Some(bone::CHEST),
        Some(bone::HEAD),
    );
    {
        let (z0, z1) = (0.875 * h, h);
        let head_r = 0.5 * (z1 - z0) * shape.head;
        let centre = 0.5 * (z0 + z1);
        let n = (shape.rings + 2).max(5);
        let rings: Vec<Ring> = (0..n)
            .map(|i| {
                // Rings of an ellipsoid; the poles are small flat caps.
                let a = (0.12 + 0.76 * i as f32 / (n - 1) as f32) * std::f32::consts::PI;
                let t = 0.5 - 0.5 * a.cos();
                ring(t, head_r * 0.92 * a.sin(), head_r * 0.80 * a.sin())
            })
            .collect();
        b.lathe(
            Part::Head,
            Vec3::new(0.0, 0.0, centre - head_r),
            Vec3::new(0.0, 0.0, centre + head_r),
            Vec3::X,
            &rings,
            sides,
            &|_| [(bone::HEAD, 1.0), (0, 0.0)],
        );
    }

    for (side, upper, fore, hand, thigh, shin, foot, toe, parts) in [
        (
            1.0f32,
            bone::UPPER_ARM_L,
            bone::FOREARM_L,
            bone::HAND_L,
            bone::THIGH_L,
            bone::SHIN_L,
            bone::FOOT_L,
            bone::TOE_L,
            [
                Part::UpperArmL,
                Part::ForearmL,
                Part::HandL,
                Part::ThighL,
                Part::ShinL,
                Part::FootL,
            ],
        ),
        (
            -1.0,
            bone::UPPER_ARM_R,
            bone::FOREARM_R,
            bone::HAND_R,
            bone::THIGH_R,
            bone::SHIN_R,
            bone::FOOT_R,
            bone::TOE_R,
            [
                Part::UpperArmR,
                Part::ForearmR,
                Part::HandR,
                Part::ThighR,
                Part::ShinR,
                Part::FootR,
            ],
        ),
    ] {
        let (shoulder, elbow, wrist) = (pivots[upper], pivots[fore], pivots[hand]);
        let arm = [0.20 * r * g, 0.17 * r * g, 0.14 * r * g];
        b.limb(
            parts[0],
            shoulder - Vec3::Y * side * 0.12 * r,
            elbow,
            Vec3::X,
            [(arm[0], arm[0]), (arm[1], arm[1])],
            shape,
            upper,
            None,
            Some(fore),
        );
        b.limb(
            parts[1],
            elbow,
            wrist,
            Vec3::X,
            [(arm[1], arm[1]), (arm[2], arm[2])],
            shape,
            fore,
            Some(upper),
            Some(hand),
        );
        // The hand: a flat paddle, palm down.
        b.limb(
            parts[2],
            wrist,
            wrist + Vec3::Y * side * 0.09 * h,
            Vec3::X,
            [(0.17 * r * g, 0.07 * r * g), (0.13 * r * g, 0.05 * r * g)],
            &Shape {
                rings: shape.rings.clamp(2, 4),
                ..*shape
            },
            hand,
            Some(fore),
            None,
        );
        let (hip, knee, ankle) = (pivots[thigh], pivots[shin], pivots[foot]);
        let leg = [0.28 * r * g, 0.21 * r * g, 0.15 * r * g];
        b.limb(
            parts[3],
            hip + Vec3::Z * 0.03 * h,
            knee,
            Vec3::X,
            [(leg[0], leg[0]), (leg[1], leg[1])],
            shape,
            thigh,
            Some(bone::HIPS),
            Some(shin),
        );
        b.limb(
            parts[4],
            knee,
            ankle,
            Vec3::X,
            [(leg[1], leg[1]), (leg[2], leg[2])],
            shape,
            shin,
            Some(thigh),
            Some(foot),
        );
        // The foot: from the heel to the toes, flat on the ground.
        let heel = Vec3::new(-0.045 * h, ankle.y, 0.032 * h);
        let tip = Vec3::new(0.15 * h, ankle.y, 0.022 * h);
        let blend = shape.blend;
        let n = shape.rings.clamp(2, 4);
        let rings: Vec<Ring> = (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                ring(t, 0.20 * r * g * (1.0 - 0.15 * t), (0.032 - 0.012 * t) * h)
            })
            .collect();
        b.lathe(parts[5], heel, tip, Vec3::Y, &rings, sides, &move |t| {
            if t > 0.6 {
                if blend {
                    [(toe, 0.7), (foot, 0.3)]
                } else {
                    [(toe, 1.0), (0, 0.0)]
                }
            } else {
                [(foot, 1.0), (0, 0.0)]
            }
        });
    }
    b.mesh
}

/// Armour-class tints of the mannequin (MODELS.md 9): cloth, leather, mail, plate.
pub const ARMOUR_TINTS: [[f32; 3]; 4] = [
    [0.84, 0.80, 0.70],
    [0.55, 0.38, 0.24],
    [0.62, 0.66, 0.72],
    [0.30, 0.32, 0.36],
];

pub const MANNEQUIN_TEXTURE_SIZE: u32 = 64;

/// The mannequin's atlas as RGBA: a grey per part with a soft gradient, to be tinted.
pub fn mannequin_texture() -> Vec<u8> {
    let n = MANNEQUIN_TEXTURE_SIZE as usize;
    let mut out = vec![255u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (u, v) = ((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
            let shade = match Part::at(u, v) {
                Some(Part::Torso) => {
                    let [_, v0, _, v1] = Part::Torso.rect();
                    0.78 + 0.22 * ((v - v0) / (v1 - v0))
                }
                Some(Part::Head) => 1.0,
                Some(Part::Neck) => 0.85,
                Some(Part::HandL | Part::HandR | Part::FootL | Part::FootR) => 0.55,
                Some(Part::ForearmL | Part::ForearmR | Part::ShinL | Part::ShinR) => 0.70,
                Some(_) => 0.82,
                None => 0.6,
            };
            let c = (shade * 255.0) as u8;
            out[(y * n + x) * 4..][..3].copy_from_slice(&[c, c, c]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mannequin_is_small_closed_and_faces_outward() {
        for frame in rig::FRAMES {
            let m = build(frame, &Shape::MANNEQUIN);
            assert!(
                (600..=900).contains(&m.triangles()),
                "{} triangles",
                m.triangles()
            );
            assert_eq!(m.positions.len(), m.normals.len());
            assert_eq!(m.positions.len(), m.uvs.len());
            assert_eq!(m.positions.len(), m.joints.len());
            let n = m.positions.len() as u32;
            assert!(m.indices.iter().all(|i| *i < n));
            // Every triangle's geometric normal agrees with its vertices' normals.
            let mut flipped = 0;
            for t in m.indices.chunks_exact(3) {
                let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
                let geo = (m.positions[b] - m.positions[a]).cross(m.positions[c] - m.positions[a]);
                if geo.length() < 1e-6 {
                    continue;
                }
                let avg = m.normals[a] + m.normals[b] + m.normals[c];
                if geo.dot(avg) <= 0.0 {
                    flipped += 1;
                }
            }
            assert_eq!(flipped, 0, "{frame:?} has inward-facing triangles");
        }
    }

    #[test]
    fn the_figure_fills_its_frame() {
        for frame in rig::FRAMES {
            let (r, h) = frame.capsule();
            let m = build(frame, &Shape::MANNEQUIN);
            let top = m.positions.iter().map(|p| p.z).fold(f32::MIN, f32::max);
            let bottom = m.positions.iter().map(|p| p.z).fold(f32::MAX, f32::min);
            let span = m.positions.iter().map(|p| p.y.abs()).fold(0.0, f32::max);
            let depth = m.positions.iter().map(|p| p.x.abs()).fold(0.0, f32::max);
            assert!((top - h).abs() < 0.02 * h, "{frame:?} top {top} of {h}");
            assert!((-0.5..=1.5).contains(&bottom), "{frame:?} bottom {bottom}");
            assert!(span <= 0.70 * h, "{frame:?} half-span {span}");
            assert!(depth <= 2.5 * r, "{frame:?} depth {depth}");
            // UVs stay in the atlas and weights are normalised.
            assert!(m.uvs.iter().flatten().all(|c| (0.0..=1.0).contains(c)));
            for w in &m.weights {
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn the_detailed_shape_sits_at_the_triangle_ceiling() {
        let m = build(ArchetypeFrame::Striker, &Shape::detailed());
        assert!(
            (3400..=crate::limits::MAX_TRIANGLES).contains(&m.triangles()),
            "{} triangles",
            m.triangles()
        );
        assert!(m.positions.len() <= crate::limits::MAX_VERTICES);
        // Smooth joints: some vertices carry two bones.
        assert!(m.weights.iter().any(|w| w[0] > 0.0 && w[1] > 0.0));
    }

    #[test]
    fn every_part_owns_its_rectangle() {
        for p in Part::ALL {
            let [u0, v0, u1, v1] = p.rect();
            assert_eq!(Part::at((u0 + u1) / 2.0, (v0 + v1) / 2.0), Some(p));
        }
        let tex = mannequin_texture();
        assert_eq!(tex.len(), 64 * 64 * 4);
        assert!(tex.chunks_exact(4).all(|p| p[3] == 255));
    }
}
