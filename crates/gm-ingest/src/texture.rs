//! The atlas (MODELS.md 3, 5): resample the uploaded image to a power-of-two size of at most
//! 1024, build the mip chain in linear light with the cutout's coverage carried along, and
//! encode every level as BC1.

use gm_model::format::{bc1_level_bytes, limits, mip_count};
use image::RgbaImage;

pub struct Atlas {
    pub w: u16,
    pub h: u16,
    /// BC1 blocks, largest mip first.
    pub bc1: Vec<u8>,
    /// Mean opaque texel, sRGB.
    pub average: [u8; 4],
}

/// The side an uploaded side of `src` texels is stored at: the largest power of two that is
/// neither larger than the source nor larger than the budget, and at least 64.
pub fn target_side(src: u32) -> u16 {
    let capped = src.clamp(limits::MIN_TEXTURE as u32, limits::MAX_TEXTURE as u32);
    1 << capped.ilog2()
}

fn srgb_to_linear_table() -> [f32; 256] {
    let mut t = [0f32; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    t
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// A level in linear light: premultiplied RGB and coverage per texel.
struct Level {
    w: usize,
    h: usize,
    /// `[r·a, g·a, b·a, a]` with `a` the fraction of the texel that is opaque.
    px: Vec<[f32; 4]>,
}

/// Overlap of `[a0, a1)` with the unit cell `[i, i + 1)`.
fn overlap(a0: f32, a1: f32, i: usize) -> f32 {
    (a1.min(i as f32 + 1.0) - a0.max(i as f32)).max(0.0)
}

fn resample(img: &RgbaImage, w: usize, h: usize, factor: [f32; 4], cutout: Option<f32>) -> Level {
    let lut = srgb_to_linear_table();
    let (sw, sh) = (img.width() as usize, img.height() as usize);
    let (sx, sy) = (sw as f32 / w as f32, sh as f32 / h as f32);
    let src = img.as_raw();
    let mut px = vec![[0f32; 4]; w * h];
    for y in 0..h {
        let (y0, y1) = (y as f32 * sy, (y as f32 + 1.0) * sy);
        let rows =
            (y0.floor() as usize)..((y1.ceil() as usize).min(sh).max(y0.floor() as usize + 1));
        for x in 0..w {
            let (x0, x1) = (x as f32 * sx, (x as f32 + 1.0) * sx);
            let cols =
                (x0.floor() as usize)..((x1.ceil() as usize).min(sw).max(x0.floor() as usize + 1));
            let mut acc = [0f32; 4];
            let mut plain = [0f32; 3];
            let mut total = 0f32;
            for yy in rows.clone() {
                let wy = overlap(y0, y1, yy).max(1e-6);
                for xx in cols.clone() {
                    let wgt = wy * overlap(x0, x1, xx).max(1e-6);
                    let o = (yy.min(sh - 1) * sw + xx.min(sw - 1)) * 4;
                    let c = [
                        lut[src[o] as usize] * factor[0],
                        lut[src[o + 1] as usize] * factor[1],
                        lut[src[o + 2] as usize] * factor[2],
                    ];
                    let alpha = src[o + 3] as f32 / 255.0 * factor[3];
                    let a = match cutout {
                        None => 1.0,
                        Some(cut) => (alpha >= cut) as u8 as f32,
                    };
                    for k in 0..3 {
                        acc[k] += wgt * a * c[k];
                        plain[k] += wgt * c[k];
                    }
                    acc[3] += wgt * a;
                    total += wgt;
                }
            }
            let out = &mut px[y * w + x];
            if acc[3] > 0.0 {
                // Colour of the opaque part only: a cut edge does not darken.
                let cover = acc[3] / total;
                for k in 0..3 {
                    out[k] = acc[k] / acc[3] * cover;
                }
                out[3] = cover;
            } else {
                // Fully cut: keep a trace of the colour underneath (`to_rgba` divides it back
                // out) so the block's endpoints stay sane; it weighs nothing in the mips.
                let k = 1e-6 / total;
                *out = [plain[0] * k, plain[1] * k, plain[2] * k, 0.0];
            }
        }
    }
    Level { w, h, px }
}

fn halve(l: &Level) -> Level {
    let (w, h) = (l.w / 2, l.h / 2);
    let mut px = vec![[0f32; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let s = l.px[(y * 2 + dy) * l.w + x * 2 + dx];
                for k in 0..4 {
                    acc[k] += s[k] * 0.25;
                }
            }
            px[y * w + x] = acc;
        }
    }
    Level { w, h, px }
}

/// sRGB bytes of a level: straight colour, alpha 255 or 0.
fn to_rgba(l: &Level, cutout: bool) -> Vec<u8> {
    let mut out = vec![0u8; l.w * l.h * 4];
    for (i, p) in l.px.iter().enumerate() {
        let a = p[3].max(1e-6);
        let o = i * 4;
        out[o] = linear_to_srgb(p[0] / a);
        out[o + 1] = linear_to_srgb(p[1] / a);
        out[o + 2] = linear_to_srgb(p[2] / a);
        out[o + 3] = if !cutout || p[3] >= 0.5 { 255 } else { 0 };
    }
    out
}

/// Build the atlas from the uploaded image.
pub fn build(img: &RgbaImage, factor: [f32; 4], cutout: Option<f32>) -> Atlas {
    let (w, h) = (target_side(img.width()), target_side(img.height()));
    let mut level = resample(img, w as usize, h as usize, factor, cutout);
    let params = texpresso::Params {
        algorithm: texpresso::Algorithm::ClusterFit,
        ..Default::default()
    };
    let mut bc1 = Vec::with_capacity(gm_model::format::texture_bytes(w, h));
    let mut average = [0u8; 4];
    for i in 0..mip_count(w, h) {
        if i > 0 {
            level = halve(&level);
        }
        if i == 0 {
            let mut sum = [0f64; 4];
            for p in &level.px {
                for k in 0..4 {
                    sum[k] += p[k] as f64;
                }
            }
            let a = sum[3].max(1e-9);
            average = [
                linear_to_srgb((sum[0] / a) as f32),
                linear_to_srgb((sum[1] / a) as f32),
                linear_to_srgb((sum[2] / a) as f32),
                255,
            ];
        }
        let rgba = to_rgba(&level, cutout.is_some());
        let start = bc1.len();
        bc1.resize(start + bc1_level_bytes(level.w, level.h), 0);
        texpresso::Format::Bc1.compress(&rgba, level.w, level.h, params, &mut bc1[start..]);
    }
    Atlas { w, h, bc1, average }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_model::bc1;

    #[test]
    fn sides_are_powers_of_two_within_the_budget() {
        assert_eq!(target_side(1024), 1024);
        assert_eq!(target_side(4096), 1024);
        assert_eq!(target_side(1000), 512);
        assert_eq!(target_side(513), 512);
        assert_eq!(target_side(64), 64);
        assert_eq!(target_side(17), 64);
        assert_eq!(target_side(1), 64);
    }

    #[test]
    fn a_flat_colour_survives_resampling_mips_and_bc1() {
        let img = RgbaImage::from_pixel(300, 200, image::Rgba([200, 120, 40, 255]));
        let atlas = build(&img, [1.0; 4], None);
        assert_eq!((atlas.w, atlas.h), (256, 128));
        assert_eq!(atlas.bc1.len(), gm_model::format::texture_bytes(256, 128));
        for c in 0..3 {
            let want: i32 = [200, 120, 40][c];
            assert!(
                (atlas.average[c] as i32 - want).abs() <= 2,
                "{:?}",
                atlas.average
            );
        }
        // Every mip decodes to the same colour within BC1's 5:6:5 endpoints.
        let mut offset = 0;
        for i in 0..mip_count(256, 128) {
            let (w, h) = (256usize >> i, 128usize >> i);
            let len = bc1_level_bytes(w, h);
            let px = bc1::decode(&atlas.bc1[offset..offset + len], w, h);
            offset += len;
            for p in px.chunks_exact(4) {
                assert!((p[0] as i32 - 200).abs() <= 8 && (p[1] as i32 - 120).abs() <= 6);
                assert_eq!(p[3], 255);
            }
        }
    }

    #[test]
    fn the_cutout_keeps_its_coverage_and_does_not_darken_edges() {
        // Left half opaque red, right half transparent black.
        let mut img = RgbaImage::from_pixel(128, 128, image::Rgba([0, 0, 0, 0]));
        for y in 0..128 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            }
        }
        let atlas = build(&img, [1.0; 4], Some(0.5));
        let top = bc1::decode(&atlas.bc1[..bc1_level_bytes(128, 128)], 128, 128);
        let opaque = top.chunks_exact(4).filter(|p| p[3] == 255).count();
        assert_eq!(opaque, 64 * 128);
        assert!(
            atlas.average[0] > 240 && atlas.average[1] < 12,
            "{:?}",
            atlas.average
        );
        // The opaque texels are still pure red at the cut edge.
        let edge = &top[(10 * 128 + 63) * 4..][..4];
        assert!(edge[0] > 240 && edge[1] < 12 && edge[3] == 255, "{edge:?}");
        // An opaque model ignores the alpha channel entirely.
        let solid = build(&img, [1.0; 4], None);
        let top = bc1::decode(&solid.bc1[..bc1_level_bytes(128, 128)], 128, 128);
        assert!(top.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn the_base_colour_factor_is_baked_in() {
        let img = RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
        let atlas = build(&img, [1.0, 0.0, 0.0, 1.0], None);
        assert!(atlas.average[0] > 250 && atlas.average[1] < 5 && atlas.average[2] < 5);
    }
}
