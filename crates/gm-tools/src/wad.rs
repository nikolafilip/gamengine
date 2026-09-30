//! Procedural palette and WAD2 texture generation.
//!
//! Quake BSPs store 8-bit palette-indexed textures and the palette lives outside the BSP. We
//! generate our own 256-colour palette (16 hue ramps x 16 shades) and a handful of tileable
//! 64x64 textures, deterministically, so nothing from id Software's data is needed and the
//! pipeline works from a fresh checkout.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub const TEX_SIZE: u32 = 64;
const MIPTEX_HEADER: u32 = 40;
const WAD_MIPTEX_TYPE: u8 = 0x44;

pub struct Palette(pub Vec<[u8; 3]>);

impl Palette {
    pub fn generate() -> Palette {
        const RAMPS: [[f32; 3]; 16] = [
            [0.93, 0.93, 0.93], // neutral gray
            [0.90, 0.85, 0.75], // warm plaster
            [0.75, 0.80, 0.90], // cool slate
            [0.60, 0.42, 0.25], // wood brown
            [0.72, 0.36, 0.28], // brick red
            [0.85, 0.65, 0.30], // ochre
            [0.45, 0.50, 0.25], // olive
            [0.30, 0.55, 0.35], // moss
            [0.25, 0.55, 0.55], // teal
            [0.35, 0.40, 0.70], // blue
            [0.55, 0.35, 0.65], // purple
            [0.80, 0.45, 0.55], // rose
            [0.90, 0.80, 0.55], // sand
            [0.95, 0.90, 0.80], // bone
            [1.00, 0.60, 0.20], // accent orange
            [0.30, 0.80, 1.00], // accent cyan
        ];
        let dark = [0.02f32, 0.02, 0.03];
        let mut colors = Vec::with_capacity(256);
        for ramp in RAMPS {
            for i in 0..16 {
                let shade = ((i as f32 + 1.0) / 16.0).powf(1.3);
                let c = |k: usize| ((dark[k] + (ramp[k] - dark[k]) * shade) * 255.0).round() as u8;
                colors.push([c(0), c(1), c(2)]);
            }
        }
        Palette(colors)
    }

    pub fn nearest(&self, rgb: [u8; 3]) -> u8 {
        let mut best = 0usize;
        let mut best_d = u32::MAX;
        for (i, c) in self.0.iter().enumerate() {
            let d = (0..3)
                .map(|k| (c[k] as i32 - rgb[k] as i32).pow(2) as u32)
                .sum::<u32>();
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best as u8
    }

    pub fn to_lmp(&self) -> Vec<u8> {
        self.0.iter().flatten().copied().collect()
    }
}

pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Palette indices for mip levels 0..4 (full, 1/2, 1/4, 1/8).
    pub mips: [Vec<u8>; 4],
}

/// The textures every map can rely on. Names are at most 15 characters (WAD limit).
/// `skip`, `clip` and `trigger` are tool textures qbsp looks up by name.
pub const BASE_TEXTURES: [&str; 10] = [
    "floor_stone",
    "wall_brick",
    "ceil_plaster",
    "ramp_wood",
    "trim_dark",
    "metal_panel",
    "light_panel",
    "skip",
    "clip",
    "trigger",
];

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Tileable white noise on a `period_x` by `period_y` torus.
fn noise(x: i32, y: i32, period_x: i32, period_y: i32, seed: u32) -> f32 {
    hash(x.rem_euclid(period_x), y.rem_euclid(period_y), seed)
}

/// Tileable value noise with `cell`-sized lattice cells (`period % cell == 0`).
fn smooth_noise(x: i32, y: i32, period: i32, cell: i32, seed: u32) -> f32 {
    let (cx, cy) = (x.div_euclid(cell), y.div_euclid(cell));
    let (fx, fy) = (
        (x.rem_euclid(cell)) as f32 / cell as f32,
        (y.rem_euclid(cell)) as f32 / cell as f32,
    );
    let cells = period / cell;
    let n = |i: i32, j: i32| noise(i, j, cells, cells, seed);
    let (a, b, c, d) = (n(cx, cy), n(cx + 1, cy), n(cx, cy + 1), n(cx + 1, cy + 1));
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale(c: [f32; 3], s: f32) -> [f32; 3] {
    [c[0] * s, c[1] * s, c[2] * s]
}

/// Colour of texel (x, y) of `name`, in 0..1 RGB. Every pattern tiles at TEX_SIZE.
fn generate_rgb(name: &str, x: i32, y: i32) -> [f32; 3] {
    let p = TEX_SIZE as i32;
    match name {
        "floor_stone" => {
            let tile = 32;
            let (tx, ty) = (x.div_euclid(tile), y.div_euclid(tile));
            let base = [0.43, 0.41, 0.37];
            let per_tile = 0.85 + 0.3 * noise(tx, ty, p / tile, p / tile, 11);
            let grain = 1.0
                + 0.12 * (smooth_noise(x, y, p, 4, 12) - 0.5)
                + 0.05 * (noise(x, y, p, p, 13) - 0.5);
            let grout = x.rem_euclid(tile) < 2 || y.rem_euclid(tile) < 2;
            if grout {
                scale([0.24, 0.23, 0.21], 0.9 + 0.2 * noise(x, y, p, p, 14))
            } else {
                scale(base, per_tile * grain)
            }
        }
        "wall_brick" => {
            let (bw, bh) = (32, 16);
            let row = y.div_euclid(bh);
            let xo = if row % 2 == 0 { 0 } else { bw / 2 };
            let bx = (x + xo).div_euclid(bw);
            let mortar = (x + xo).rem_euclid(bw) < 2 || y.rem_euclid(bh) < 2;
            if mortar {
                scale([0.58, 0.55, 0.50], 0.9 + 0.2 * noise(x, y, p, p, 21))
            } else {
                let base = mix(
                    [0.60, 0.30, 0.22],
                    [0.70, 0.40, 0.30],
                    noise(bx, row, p / bw, p / bh, 22),
                );
                scale(
                    base,
                    0.9 + 0.2 * smooth_noise(x, y, p, 8, 23) + 0.06 * (noise(x, y, p, p, 24) - 0.5),
                )
            }
        }
        "ceil_plaster" => {
            let base = [0.66, 0.64, 0.58];
            scale(
                base,
                0.92 + 0.12 * smooth_noise(x, y, p, 16, 31) + 0.05 * (noise(x, y, p, p, 32) - 0.5),
            )
        }
        "ramp_wood" => {
            let plank = 16;
            let py = y.div_euclid(plank);
            let gap = y.rem_euclid(plank) == 0;
            let base = mix(
                [0.45, 0.30, 0.16],
                [0.55, 0.40, 0.24],
                noise(py, 0, p / plank, 1, 41),
            );
            let wave = (x as f32 * (core::f32::consts::TAU * 4.0 / p as f32) + py as f32 * 2.1)
                .sin()
                * 0.5
                + 0.5;
            let grain = 0.9 + 0.1 * wave + 0.06 * (smooth_noise(x, y, p, 8, 42) - 0.5);
            if gap {
                scale(base, 0.5)
            } else {
                scale(base, grain)
            }
        }
        "trim_dark" => {
            let stripe = (y.rem_euclid(p) / 8) % 2 == 0;
            let base = if stripe {
                [0.20, 0.20, 0.23]
            } else {
                [0.16, 0.16, 0.19]
            };
            scale(base, 0.9 + 0.2 * smooth_noise(x, y, p, 8, 51))
        }
        "metal_panel" => {
            let edge = x.rem_euclid(32) < 1 || y.rem_euclid(32) < 1;
            let rivet = {
                let (rx, ry) = (x.rem_euclid(32), y.rem_euclid(32));
                let near = |v: i32| (v - 4).abs() <= 1 || (v - 27).abs() <= 1;
                near(rx) && near(ry)
            };
            let base = [0.36, 0.38, 0.42];
            if edge {
                scale(base, 0.55)
            } else if rivet {
                scale(base, 1.35)
            } else {
                scale(base, 0.9 + 0.2 * smooth_noise(x, y, p, 16, 61))
            }
        }
        "light_panel" => {
            let (xx, yy) = (x.rem_euclid(p), y.rem_euclid(p));
            let border = xx < 4 || yy < 4 || xx >= p - 4 || yy >= p - 4;
            if border {
                [0.30, 0.30, 0.32]
            } else {
                scale([0.95, 0.90, 0.75], 0.95 + 0.05 * noise(x, y, p, p, 71))
            }
        }
        // Tool textures: flat, distinct colours so they are obvious in the editor.
        "skip" => [0.55, 0.55, 0.20],
        "clip" => [0.55, 0.20, 0.55],
        "trigger" => [0.20, 0.55, 0.55],
        other => panic!("no generator for texture {other}"),
    }
}

fn to_u8(c: [f32; 3]) -> [u8; 3] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [q(c[0]), q(c[1]), q(c[2])]
}

pub fn build_texture(name: &str, pal: &Palette) -> Texture {
    let w = TEX_SIZE;
    let mut level: Vec<[f32; 3]> = (0..w * w)
        .map(|i| generate_rgb(name, (i % w) as i32, (i / w) as i32))
        .collect();
    let mut mips: [Vec<u8>; 4] = Default::default();
    let mut cw = w;
    for (i, mip) in mips.iter_mut().enumerate() {
        *mip = level.iter().map(|c| pal.nearest(to_u8(*c))).collect();
        if i < 3 {
            let nw = cw / 2;
            let mut next = vec![[0f32; 3]; (nw * nw) as usize];
            for y in 0..nw {
                for x in 0..nw {
                    let mut acc = [0f32; 3];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let c = level[((y * 2 + dy) * cw + x * 2 + dx) as usize];
                        acc = [acc[0] + c[0], acc[1] + c[1], acc[2] + c[2]];
                    }
                    next[(y * nw + x) as usize] = scale(acc, 0.25);
                }
            }
            level = next;
            cw = nw;
        }
    }
    Texture {
        name: name.to_owned(),
        width: w,
        height: w,
        mips,
    }
}

fn name16(name: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    let bytes = name.as_bytes();
    assert!(
        bytes.len() <= 15,
        "texture name {name} longer than 15 bytes"
    );
    out[..bytes.len()].copy_from_slice(bytes);
    out
}

/// Serialize textures into a WAD2 archive.
pub fn write_wad(textures: &[Texture]) -> Vec<u8> {
    let mut out = vec![0u8; 12];
    let mut entries: Vec<(u32, u32, [u8; 16])> = Vec::new();
    for t in textures {
        let filepos = out.len() as u32;
        let (w, h) = (t.width, t.height);
        let mut mip = Vec::new();
        mip.extend_from_slice(&name16(&t.name));
        mip.extend_from_slice(&w.to_le_bytes());
        mip.extend_from_slice(&h.to_le_bytes());
        let mut ofs = MIPTEX_HEADER;
        for level in 0..4u32 {
            mip.extend_from_slice(&ofs.to_le_bytes());
            ofs += (w >> level) * (h >> level);
        }
        for level in &t.mips {
            mip.extend_from_slice(level);
        }
        out.extend_from_slice(&mip);
        entries.push((filepos, mip.len() as u32, name16(&t.name)));
    }
    let infotableofs = out.len() as u32;
    for (filepos, size, name) in &entries {
        out.extend_from_slice(&(*filepos as i32).to_le_bytes());
        out.extend_from_slice(&(*size as i32).to_le_bytes());
        out.extend_from_slice(&(*size as i32).to_le_bytes());
        out.push(WAD_MIPTEX_TYPE);
        out.push(0);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
    }
    out[0..4].copy_from_slice(b"WAD2");
    out[4..8].copy_from_slice(&(entries.len() as i32).to_le_bytes());
    out[8..12].copy_from_slice(&(infotableofs as i32).to_le_bytes());
    out
}

pub fn make(out: &Path, palette_path: &Path) -> Result<()> {
    let pal = Palette::generate();
    let textures: Vec<Texture> = BASE_TEXTURES
        .iter()
        .map(|n| build_texture(n, &pal))
        .collect();
    let wad = write_wad(&textures);
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir)?;
    }
    if let Some(dir) = palette_path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(palette_path, pal.to_lmp())
        .with_context(|| format!("writing {}", palette_path.display()))?;
    fs::write(out, &wad).with_context(|| format!("writing {}", out.display()))?;
    println!(
        "wad make: {} textures ({}x{}), {} bytes -> {}; palette -> {}",
        textures.len(),
        TEX_SIZE,
        TEX_SIZE,
        wad.len(),
        out.display(),
        palette_path.display()
    );
    Ok(())
}

fn le_i32(b: &[u8], at: usize) -> Result<i32> {
    let s: [u8; 4] = b.get(at..at + 4).context("truncated WAD")?.try_into()?;
    Ok(i32::from_le_bytes(s))
}

pub fn list(path: &Path) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"WAD2" {
        bail!("{} is not a WAD2 file", path.display());
    }
    let n = le_i32(&bytes, 4)? as usize;
    let table = le_i32(&bytes, 8)? as usize;
    for i in 0..n {
        let e = table + i * 32;
        let filepos = le_i32(&bytes, e)? as usize;
        let size = le_i32(&bytes, e + 4)?;
        let name_bytes = bytes
            .get(e + 16..e + 32)
            .context("truncated WAD info table")?;
        let name = String::from_utf8_lossy(name_bytes)
            .trim_end_matches('\0')
            .to_owned();
        let w = le_i32(&bytes, filepos + 16)?;
        let h = le_i32(&bytes, filepos + 20)?;
        println!("{name:<16} {w:>4}x{h:<4} {size} bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_has_256_distinct_entries_and_is_deterministic() {
        let a = Palette::generate();
        let b = Palette::generate();
        assert_eq!(a.0.len(), 256);
        assert_eq!(a.to_lmp(), b.to_lmp());
        let mut sorted = a.0.clone();
        sorted.sort();
        sorted.dedup();
        assert!(
            sorted.len() > 240,
            "palette has too many duplicate colours: {}",
            sorted.len()
        );
    }

    #[test]
    fn textures_tile_and_wad_round_trips() {
        let pal = Palette::generate();
        let textures: Vec<Texture> = BASE_TEXTURES
            .iter()
            .map(|n| build_texture(n, &pal))
            .collect();
        for t in &textures {
            assert_eq!(t.mips[0].len(), 64 * 64);
            assert_eq!(t.mips[3].len(), 8 * 8);
        }
        let size = TEX_SIZE as i32;
        for name in BASE_TEXTURES {
            for i in 0..size {
                assert_eq!(
                    generate_rgb(name, 0, i),
                    generate_rgb(name, size, i),
                    "{name} does not tile in x"
                );
                assert_eq!(
                    generate_rgb(name, i, 0),
                    generate_rgb(name, i, size),
                    "{name} does not tile in y"
                );
            }
        }
        let wad = write_wad(&textures);
        assert_eq!(&wad[0..4], b"WAD2");
        let n = i32::from_le_bytes(wad[4..8].try_into().unwrap());
        assert_eq!(n as usize, textures.len());
        let table = i32::from_le_bytes(wad[8..12].try_into().unwrap()) as usize;
        assert_eq!(table + 32 * textures.len(), wad.len());
    }
}
