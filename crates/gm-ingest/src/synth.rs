//! Generated avatars: the template a creator starts from and the distinct, budget-filling
//! models the tests and the acceptance run upload (MODELS.md 11). Everything is a pure
//! function of the seed, so a run can be repeated bit for bit.

use gm_core::rng::Rng;
use gm_core::vocab::ArchetypeFrame;
use gm_model::mannequin::{self, MeshData, Part, Shape};
use image::RgbaImage;

use crate::write::{GlbBuilder, png};

/// A body within the envelope, different for every seed.
pub fn shape(seed: u64) -> Shape {
    let mut r = Rng::new(seed ^ 0x5eed_b0d7);
    Shape {
        height: r.range_f32(0.97, 1.05),
        girth: r.range_f32(0.90, 1.18),
        shoulders: r.range_f32(0.94, 1.12),
        head: r.range_f32(0.95, 1.12),
        belly: r.range_f32(0.92, 1.22),
        ..Shape::detailed()
    }
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let h = |i: i32, j: i32| {
        hash((i as u32).wrapping_mul(73_856_093) ^ (j as u32).wrapping_mul(19_349_663) ^ seed)
            as f32
            / u32::MAX as f32
    };
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (a, b, c, d) = (h(xi, yi), h(xi + 1, yi), h(xi, yi + 1), h(xi + 1, yi + 1));
    let (u, v) = (s(fx), s(fy));
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

/// Brush-like noise: a few octaves, mean about 0.5.
fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let (mut total, mut amp, mut freq) = (0.0, 0.5, 1.0);
    for octave in 0..4 {
        total += amp * value_noise(x * freq, y * freq, seed.wrapping_add(octave));
        amp *= 0.5;
        freq *= 2.0;
    }
    total / 0.9375
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + v - c, g + v - c, b + v - c]
}

struct Look {
    skin: [f32; 3],
    hair: [f32; 3],
    tunic: [f32; 3],
    trousers: [f32; 3],
    leather: [f32; 3],
    trim: [f32; 3],
    /// 0 plain, 1 vertical stripes, 2 checks, 3 hoops.
    pattern: u32,
    sleeves: bool,
    gloves: bool,
    seed: u32,
}

fn look(seed: u64) -> Look {
    let mut r = Rng::new(seed ^ 0xa11_c0de);
    let skins = [
        [0.96, 0.80, 0.69],
        [0.87, 0.67, 0.53],
        [0.71, 0.50, 0.36],
        [0.50, 0.33, 0.23],
        [0.36, 0.24, 0.17],
    ];
    let hue = r.next_f32();
    Look {
        skin: skins[r.below(5) as usize],
        hair: hsv(
            r.range_f32(0.02, 0.14),
            r.range_f32(0.3, 0.8),
            r.range_f32(0.08, 0.75),
        ),
        tunic: hsv(hue, r.range_f32(0.45, 0.85), r.range_f32(0.45, 0.85)),
        trousers: hsv(
            hue + r.range_f32(0.3, 0.7),
            r.range_f32(0.2, 0.6),
            r.range_f32(0.2, 0.55),
        ),
        leather: hsv(
            r.range_f32(0.04, 0.10),
            r.range_f32(0.4, 0.7),
            r.range_f32(0.18, 0.4),
        ),
        trim: hsv(hue + 0.5, r.range_f32(0.5, 0.9), r.range_f32(0.75, 0.98)),
        pattern: r.below(4),
        sleeves: r.below(3) != 0,
        gloves: r.below(2) == 0,
        seed: r.next_u32(),
    }
}

/// The colour of one texel: `part` and its local coordinates (`s` around, front at 0.5; `t`
/// along, from the start of the part).
fn texel(look: &Look, part: Part, s: f32, t: f32) -> [f32; 3] {
    let front = (s - 0.5).abs();
    let cloth = |base: [f32; 3]| -> [f32; 3] {
        let k = match look.pattern {
            1 if (s * 14.0).fract() < 0.35 => 0.72,
            2 if ((s * 10.0).floor() + (t * 8.0).floor()) % 2.0 == 0.0 => 0.78,
            3 if (t * 7.0).fract() < 0.3 => 0.7,
            _ => 1.0,
        };
        [base[0] * k, base[1] * k, base[2] * k]
    };
    match part {
        Part::Torso => {
            if (0.26..0.34).contains(&t) {
                // A belt with a buckle at the front.
                if front < 0.035 {
                    look.trim
                } else {
                    look.leather
                }
            } else if t < 0.26 {
                cloth(look.trousers)
            } else if t > 0.93 || (front < 0.03 && t > 0.55) {
                look.trim
            } else if ((s - 0.5) * 2.2).hypot(t - 0.66) < 0.09 {
                // The emblem on the chest.
                look.trim
            } else {
                cloth(look.tunic)
            }
        }
        Part::Head => {
            let eye = ((front - 0.075) * 3.0).hypot(t - 0.52) < 0.045;
            let mouth = front < 0.06 && (0.30..0.33).contains(&t);
            if t > 0.68 || (front > 0.27 && t > 0.3) {
                look.hair
            } else if eye {
                [0.08, 0.07, 0.07]
            } else if mouth {
                [look.skin[0] * 0.6, look.skin[1] * 0.4, look.skin[2] * 0.4]
            } else {
                look.skin
            }
        }
        Part::Neck => look.skin,
        Part::UpperArmL | Part::UpperArmR => {
            if t > 0.9 {
                look.trim
            } else {
                cloth(look.tunic)
            }
        }
        Part::ForearmL | Part::ForearmR => {
            if t > 0.7 && look.gloves {
                look.leather
            } else if look.sleeves {
                cloth(look.tunic)
            } else {
                look.skin
            }
        }
        Part::HandL | Part::HandR => {
            if look.gloves {
                look.leather
            } else {
                look.skin
            }
        }
        Part::ThighL | Part::ThighR => cloth(look.trousers),
        Part::ShinL | Part::ShinR => {
            if t > 0.45 {
                look.leather
            } else {
                cloth(look.trousers)
            }
        }
        Part::FootL | Part::FootR => look.leather,
    }
}

/// A painted atlas for the mannequin's UV layout, `side × side`.
pub fn paint(seed: u64, side: u32) -> RgbaImage {
    let look = look(seed);
    let mut img = RgbaImage::new(side, side);
    let n = side as f32;
    for y in 0..side {
        for x in 0..side {
            let (u, v) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);
            let base = match Part::at(u, v) {
                Some(part) => {
                    let [u0, v0, u1, v1] = part.rect();
                    let s = (u - u0) / (u1 - u0);
                    // The mesh uses the middle 80% of the part's height (`mannequin::lathe`).
                    let t = (((v - v0) / (v1 - v0) - 0.1) / 0.8).clamp(0.0, 1.0);
                    let c = texel(&look, part, s, t);
                    // Volume: darker towards the back and the ends.
                    let shade = 0.80 + 0.20 * (1.0 - (s - 0.5).abs() * 2.0).powf(0.6);
                    [c[0] * shade, c[1] * shade, c[2] * shade]
                }
                None => [0.25, 0.25, 0.25],
            };
            // Brush strokes: one broad layer and one fine.
            let broad = fbm(u * 22.0, v * 22.0, look.seed);
            let fine = fbm(u * 160.0, v * 160.0, look.seed ^ 0x9e37);
            let k = 0.78 + 0.34 * broad + 0.12 * (fine - 0.5);
            let px = base.map(|c| ((c * k).clamp(0.0, 1.0) * 255.0) as u8);
            img.put_pixel(x, y, image::Rgba([px[0], px[1], px[2], 255]));
        }
    }
    img
}

/// A generated avatar as an upload: about 3,480 triangles, a painted `side × side` atlas.
pub fn avatar(seed: u64, frame: ArchetypeFrame, side: u32) -> GlbBuilder {
    let mesh = mannequin::build(frame, &shape(seed));
    let texture = paint(seed, side);
    GlbBuilder::new(mesh, png(side, side, texture.as_raw()))
}

/// The mannequin on the rig with a plain atlas: what `gm-tools model template` writes.
pub fn template(frame: ArchetypeFrame) -> GlbBuilder {
    let mesh: MeshData = mannequin::build(frame, &Shape::detailed());
    let side = 256;
    let mut img = RgbaImage::new(side, side);
    for y in 0..side {
        for x in 0..side {
            let (u, v) = (
                (x as f32 + 0.5) / side as f32,
                (y as f32 + 0.5) / side as f32,
            );
            // A different grey per part and a white stripe down each part's front, so the
            // layout can be read off the texture.
            let c = match Part::at(u, v) {
                Some(part) => {
                    let [u0, _, u1, _] = part.rect();
                    let s = (u - u0) / (u1 - u0);
                    let index = Part::ALL.iter().position(|p| *p == part).unwrap_or(0);
                    if (s - 0.5).abs() < 0.02 {
                        250
                    } else {
                        110 + (index as u8 % 5) * 25
                    }
                }
                None => 60,
            };
            img.put_pixel(x, y, image::Rgba([c, c, c, 255]));
        }
    }
    GlbBuilder::new(mesh, png(side, side, img.as_raw()))
}
