//! A small orthographic rasteriser: the coverage rule of MODELS.md 4 and the preview a
//! moderator looks at (MODELS.md 10). Triangles are drawn as the game draws them: back faces
//! are culled unless the model is two-sided, and a cutout texel is a hole.

use glam::Vec3;
use gm_model::{Model, bc1};

/// Where the camera stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// At +X looking at the model's face.
    Front,
    /// At −X.
    Back,
    /// At +Y, looking at its left side.
    Left,
    /// At −Y.
    Right,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Front, Dir::Back, Dir::Left, Dir::Right];

    /// `(screen right, towards the camera)` in model space; screen up is +Z.
    fn axes(self) -> (Vec3, Vec3) {
        match self {
            Dir::Front => (Vec3::Y, Vec3::X),
            Dir::Back => (Vec3::NEG_Y, Vec3::NEG_X),
            Dir::Left => (Vec3::NEG_X, Vec3::Y),
            Dir::Right => (Vec3::X, Vec3::NEG_Y),
        }
    }

    /// Half-width of the hitbox rectangle seen from here is always the capsule radius.
    pub fn is_side(self) -> bool {
        matches!(self, Dir::Left | Dir::Right)
    }
}

/// Triangles to draw.
pub struct Soup<'a> {
    pub positions: &'a [Vec3],
    pub normals: &'a [Vec3],
    pub uvs: &'a [[f32; 2]],
    pub indices: &'a [u32],
    pub two_sided: bool,
}

/// A window of the view: `x` across (screen right), `z` up, in world units.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub x0: f32,
    pub x1: f32,
    pub z0: f32,
    pub z1: f32,
    pub width: usize,
    pub height: usize,
}

pub struct Raster {
    pub width: usize,
    pub height: usize,
    /// Depth towards the camera; `f32::MIN` where nothing was drawn.
    pub depth: Vec<f32>,
    /// `(u, v, shade)` of the nearest fragment.
    pub frag: Vec<[f32; 3]>,
}

impl Raster {
    pub fn covered(&self) -> usize {
        self.depth.iter().filter(|d| **d > f32::MIN).count()
    }
}

/// Draw `soup` into `window`. `opaque(u, v)` is the cutout test.
pub fn draw(
    soup: &Soup<'_>,
    dir: Dir,
    window: Window,
    opaque: &dyn Fn(f32, f32) -> bool,
) -> Raster {
    let (right, toward) = dir.axes();
    let light = Vec3::new(0.5, 0.35, 0.8).normalize();
    draw_axes(soup, right, Vec3::Z, toward, light, window, opaque)
}

/// Draw `soup` seen along any axes: `right` and `up` span the window (`x` along `right`,
/// `z` along `up`), `toward` points at the camera, `light` is where the light comes from.
#[allow(clippy::too_many_arguments)]
pub fn draw_axes(
    soup: &Soup<'_>,
    right: Vec3,
    up: Vec3,
    toward: Vec3,
    light: Vec3,
    window: Window,
    opaque: &dyn Fn(f32, f32) -> bool,
) -> Raster {
    let mut out = Raster {
        width: window.width,
        height: window.height,
        depth: vec![f32::MIN; window.width * window.height],
        frag: vec![[0.0; 3]; window.width * window.height],
    };
    let sx = window.width as f32 / (window.x1 - window.x0);
    let sz = window.height as f32 / (window.z1 - window.z0);
    for t in soup.indices.chunks_exact(3) {
        let idx = [t[0] as usize, t[1] as usize, t[2] as usize];
        let p = idx.map(|i| soup.positions[i]);
        // Screen space: x right, y down, d towards the camera.
        let s = p.map(|v| {
            [
                (v.dot(right) - window.x0) * sx,
                (window.z1 - v.dot(up)) * sz,
                v.dot(toward),
            ]
        });
        let area =
            (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[2][0] - s[0][0]) * (s[1][1] - s[0][1]);
        if area.abs() < 1e-9 {
            continue;
        }
        // With y down, a triangle that is counter-clockwise in the world faces the camera
        // when its screen area is negative.
        if !soup.two_sided && area > 0.0 {
            continue;
        }
        // Pixel bounds of the triangle, clamped to the window (a triangle wholly outside
        // gives an empty range).
        let bound = |axis: usize, limit: usize| -> (usize, usize) {
            let lo = s.iter().map(|v| v[axis]).fold(f32::MAX, f32::min).floor();
            let hi = s.iter().map(|v| v[axis]).fold(f32::MIN, f32::max).ceil();
            (
                lo.clamp(0.0, limit as f32) as usize,
                hi.clamp(0.0, limit as f32) as usize,
            )
        };
        let (min_x, max_x) = bound(0, window.width);
        let (min_y, max_y) = bound(1, window.height);
        for y in min_y..max_y {
            for x in min_x..max_x {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((s[1][0] - px) * (s[2][1] - py) - (s[2][0] - px) * (s[1][1] - py)) / area;
                let w1 = ((s[2][0] - px) * (s[0][1] - py) - (s[0][0] - px) * (s[2][1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let d = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                let at = y * window.width + x;
                if d <= out.depth[at] {
                    continue;
                }
                let uv = |k: usize| {
                    w0 * soup.uvs[idx[0]][k] + w1 * soup.uvs[idx[1]][k] + w2 * soup.uvs[idx[2]][k]
                };
                let (u, v) = (uv(0), uv(1));
                if !opaque(u, v) {
                    continue;
                }
                let n = (soup.normals[idx[0]] * w0
                    + soup.normals[idx[1]] * w1
                    + soup.normals[idx[2]] * w2)
                    .normalize_or(toward);
                out.depth[at] = d;
                out.frag[at] = [u, v, 0.5 + 0.5 * n.dot(light)];
            }
        }
    }
    out
}

/// Fraction of the hitbox rectangle (`2r × h`, feet on the ground) the soup covers from `dir`,
/// at two pixels per unit.
pub fn coverage(
    soup: &Soup<'_>,
    dir: Dir,
    r: f32,
    h: f32,
    opaque: &dyn Fn(f32, f32) -> bool,
) -> f32 {
    let window = Window {
        x0: -r,
        x1: r,
        z0: 0.0,
        z1: h,
        width: (4.0 * r).round() as usize,
        height: (2.0 * h).round() as usize,
    };
    let raster = draw(soup, dir, window, opaque);
    raster.covered() as f32 / (window.width * window.height) as f32
}

/// The triangles of an ingested model, dequantised.
pub struct ModelSoup {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub two_sided: bool,
}

impl ModelSoup {
    pub fn new(model: &Model) -> ModelSoup {
        let n = model.vertices.len();
        ModelSoup {
            positions: (0..n).map(|i| model.position(i)).collect(),
            normals: (0..n).map(|i| model.normal(i)).collect(),
            uvs: (0..n).map(|i| model.uv(i)).collect(),
            indices: model.indices.iter().map(|i| *i as u32).collect(),
            two_sided: model.two_sided(),
        }
    }

    pub fn soup(&self) -> Soup<'_> {
        Soup {
            positions: &self.positions,
            normals: &self.normals,
            uvs: &self.uvs,
            indices: &self.indices,
            two_sided: self.two_sided,
        }
    }
}

/// One texel of the model's largest mip at `(u, v)`.
pub fn sample(model: &Model, u: f32, v: f32) -> [u8; 4] {
    let (w, h) = (model.tex_w as usize, model.tex_h as usize);
    let blocks = &model.texture[..gm_model::format::bc1_level_bytes(w, h)];
    let x = (u.clamp(0.0, 1.0) * w as f32) as usize;
    let y = (v.clamp(0.0, 1.0) * h as f32) as usize;
    bc1::texel(blocks, w, h, x, y)
}

pub const PREVIEW_SIDE: usize = 256;

/// Supersampling of a baked icon: drawn this many times larger, then averaged down.
const ICON_OVER: usize = 4;
/// Of the icon's side, this much is picture; the rest is margin.
const ICON_FILL: f32 = 28.0 / 32.0;

/// A model's icon (CONTENT.md 5.3): RGBA8, `side × side`, straight alpha, transparent where
/// nothing is drawn. The model is seen along its thinnest axis with its longest axis
/// running from the bottom left to the top right, lit from the top left, filling 28 of 32
/// dots. Deterministic: IEEE basics, integer texel fetches and integer averaging only.
pub fn icon(model: &Model, side: usize) -> Vec<u8> {
    let soup = ModelSoup::new(model);
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in &soup.positions {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let extent = (hi - lo).max(Vec3::splat(1e-3));
    let e = extent.to_array();
    // The longest axis, the thinnest one (looked along), and the one between.
    let mut order = [0usize, 1, 2];
    order.sort_by(|a, b| {
        e[*b]
            .partial_cmp(&e[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let axis = |i: usize| match i {
        0 => Vec3::X,
        1 => Vec3::Y,
        _ => Vec3::Z,
    };
    let longest = axis(order[0]);
    let middle = axis(order[1]);
    let mut toward = axis(order[2]);
    // A right-handed frame: (longest, middle, toward) as (x, y, z).
    if longest.cross(middle).dot(toward) < 0.0 {
        toward = -toward;
    }
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let right = (longest - middle) * s;
    let up = (longest + middle) * s;
    let light = (-0.45 * right + 0.6 * up + 0.65 * toward).normalize();
    let centre = (lo + hi) * 0.5;
    // The picture's half-extent along the diagonal frame, with the margin.
    let half = {
        let mut m = 0f32;
        for p in &soup.positions {
            let d = *p - centre;
            m = m.max(d.dot(right).abs()).max(d.dot(up).abs());
        }
        m.max(1e-3) / ICON_FILL
    };
    let big = side * ICON_OVER;
    let window = Window {
        x0: centre.dot(right) - half,
        x1: centre.dot(right) + half,
        z0: centre.dot(up) - half,
        z1: centre.dot(up) + half,
        width: big,
        height: big,
    };
    let cut = model.cutout();
    let opaque = |u: f32, v: f32| !cut || sample(model, u, v)[3] >= 128;
    let raster = draw_axes(&soup.soup(), right, up, toward, light, window, &opaque);
    // Average down: colour over the covered samples, alpha as the coverage.
    let mut out = vec![0u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let mut sum = [0u32; 3];
            let mut covered = 0u32;
            for yy in 0..ICON_OVER {
                for xx in 0..ICON_OVER {
                    let at = (y * ICON_OVER + yy) * big + x * ICON_OVER + xx;
                    if raster.depth[at] > f32::MIN {
                        let [u, v, shade] = raster.frag[at];
                        let c = sample(model, u, v);
                        // The shade in 1/256ths, as an integer, so that the sum is exact.
                        let sh = (shade * 256.0) as u32;
                        for k in 0..3 {
                            sum[k] += c[k] as u32 * sh / 256;
                        }
                        covered += 1;
                    }
                }
            }
            let o = (y * side + x) * 4;
            if let Some(c) = (covered > 0).then_some(covered) {
                for k in 0..3 {
                    out[o + k] = (sum[k] / c) as u8;
                }
                out[o + 3] = (c * 255 / (ICON_OVER * ICON_OVER) as u32) as u8;
            }
        }
    }
    out
}

/// A body's portrait (CONTENT.md 5.3): the head and shoulders from the front, `side` square.
pub fn portrait(model: &Model, side: usize) -> Vec<u8> {
    let head = model.pivot(gm_model::rig::bone::HEAD);
    let chest = model.pivot(gm_model::rig::bone::CHEST);
    // From the chest to a little over the head, as wide as it is tall.
    let z1 = head.z + (head.z - chest.z) * 0.9;
    let z0 = chest.z - (head.z - chest.z) * 0.2;
    let half = (z1 - z0) * 0.5;
    let big = side * ICON_OVER;
    let window = Window {
        x0: -half,
        x1: half,
        z0,
        z1,
        width: big,
        height: big,
    };
    let soup = ModelSoup::new(model);
    let cut = model.cutout();
    let opaque = |u: f32, v: f32| !cut || sample(model, u, v)[3] >= 128;
    let (right, toward) = Dir::Front.axes();
    let light = Vec3::new(0.5, 0.35, 0.8).normalize();
    let raster = draw_axes(&soup.soup(), right, Vec3::Z, toward, light, window, &opaque);
    let mut out = vec![0u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let mut sum = [0u32; 3];
            let mut covered = 0u32;
            for yy in 0..ICON_OVER {
                for xx in 0..ICON_OVER {
                    let at = (y * ICON_OVER + yy) * big + x * ICON_OVER + xx;
                    if raster.depth[at] > f32::MIN {
                        let [u, v, shade] = raster.frag[at];
                        let c = sample(model, u, v);
                        let sh = (shade * 256.0) as u32;
                        for k in 0..3 {
                            sum[k] += c[k] as u32 * sh / 256;
                        }
                        covered += 1;
                    }
                }
            }
            let o = (y * side + x) * 4;
            if let Some(c) = (covered > 0).then_some(covered) {
                for k in 0..3 {
                    out[o + k] = (sum[k] / c) as u8;
                }
                out[o + 3] = (c * 255 / (ICON_OVER * ICON_OVER) as u32) as u8;
            }
        }
    }
    out
}

/// The moderation preview: the front and the left side, textured and shaded, side by side.
/// RGBA, `2 × PREVIEW_SIDE` wide and `PREVIEW_SIDE` high.
pub fn preview(model: &Model, h: f32) -> Vec<u8> {
    let soup = ModelSoup::new(model);
    let window = Window {
        x0: -0.75 * h,
        x1: 0.75 * h,
        z0: -0.15 * h,
        z1: 1.35 * h,
        width: PREVIEW_SIDE,
        height: PREVIEW_SIDE,
    };
    let cut = model.cutout();
    let opaque = |u: f32, v: f32| !cut || sample(model, u, v)[3] >= 128;
    let mut out = vec![0u8; PREVIEW_SIDE * 2 * PREVIEW_SIDE * 4];
    for (k, dir) in [Dir::Front, Dir::Left].into_iter().enumerate() {
        let raster = draw(&soup.soup(), dir, window, &opaque);
        for y in 0..PREVIEW_SIDE {
            for x in 0..PREVIEW_SIDE {
                let at = y * PREVIEW_SIDE + x;
                let o = (y * PREVIEW_SIDE * 2 + k * PREVIEW_SIDE + x) * 4;
                let px = if raster.depth[at] > f32::MIN {
                    let [u, v, shade] = raster.frag[at];
                    let c = sample(model, u, v);
                    [
                        (c[0] as f32 * shade) as u8,
                        (c[1] as f32 * shade) as u8,
                        (c[2] as f32 * shade) as u8,
                        255,
                    ]
                } else {
                    // A checker floor line at z = 0 and a plain backdrop.
                    let z = window.z1
                        - (y as f32 + 0.5) / PREVIEW_SIDE as f32 * (window.z1 - window.z0);
                    if z < 0.0 {
                        [44, 44, 52, 255]
                    } else {
                        [70, 72, 84, 255]
                    }
                };
                out[o..o + 4].copy_from_slice(&px);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 × 20 rectangle in the YZ plane facing +X, centred on the axis.
    fn card(facing_front: bool) -> (Vec<Vec3>, Vec<Vec3>, Vec<[f32; 2]>, Vec<u32>) {
        let p = vec![
            Vec3::new(0.0, -5.0, 0.0),
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, 5.0, 20.0),
            Vec3::new(0.0, -5.0, 20.0),
        ];
        let n = vec![Vec3::X; 4];
        let uv = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        // Counter-clockwise seen from +X.
        let idx = if facing_front {
            vec![0, 1, 2, 0, 2, 3]
        } else {
            vec![0, 2, 1, 0, 3, 2]
        };
        (p, n, uv, idx)
    }

    #[test]
    fn coverage_counts_what_the_game_would_draw() {
        let (p, n, uv, idx) = card(true);
        let soup = Soup {
            positions: &p,
            normals: &n,
            uvs: &uv,
            indices: &idx,
            two_sided: false,
        };
        let all = |_: f32, _: f32| true;
        // The card fills half the width of a 20 × 20 rectangle (r = 10, h = 20).
        let front = coverage(&soup, Dir::Front, 10.0, 20.0, &all);
        assert!((front - 0.5).abs() < 0.03, "front {front}");
        // From behind a one-sided card is culled; from the side it is edge-on.
        assert_eq!(coverage(&soup, Dir::Back, 10.0, 20.0, &all), 0.0);
        assert_eq!(coverage(&soup, Dir::Left, 10.0, 20.0, &all), 0.0);
        // Two-sided, it shows from behind too.
        let both = Soup {
            two_sided: true,
            ..soup
        };
        let back = coverage(&both, Dir::Back, 10.0, 20.0, &all);
        assert!((back - 0.5).abs() < 0.03, "back {back}");
        // Wound the other way it faces away from the front camera.
        let (p, n, uv, idx) = card(false);
        let away = Soup {
            positions: &p,
            normals: &n,
            uvs: &uv,
            indices: &idx,
            two_sided: false,
        };
        assert_eq!(coverage(&away, Dir::Front, 10.0, 20.0, &all), 0.0);
        assert!(coverage(&away, Dir::Back, 10.0, 20.0, &all) > 0.45);
    }

    #[test]
    fn a_cutout_is_a_hole() {
        let (p, n, uv, idx) = card(true);
        let soup = Soup {
            positions: &p,
            normals: &n,
            uvs: &uv,
            indices: &idx,
            two_sided: false,
        };
        // The left half of the texture is cut away.
        let half = |u: f32, _: f32| u >= 0.5;
        let c = coverage(&soup, Dir::Front, 10.0, 20.0, &half);
        assert!((c - 0.25).abs() < 0.03, "{c}");
    }

    #[test]
    fn only_the_hitbox_rectangle_counts() {
        // A card far wider than the rectangle covers all of it and no more.
        let p = vec![
            Vec3::new(0.0, -50.0, -10.0),
            Vec3::new(0.0, 50.0, -10.0),
            Vec3::new(0.0, 50.0, 90.0),
            Vec3::new(0.0, -50.0, 90.0),
        ];
        let n = vec![Vec3::X; 4];
        let uv = vec![[0.0; 2]; 4];
        let idx = vec![0, 1, 2, 0, 2, 3];
        let soup = Soup {
            positions: &p,
            normals: &n,
            uvs: &uv,
            indices: &idx,
            two_sided: false,
        };
        assert_eq!(coverage(&soup, Dir::Front, 10.0, 20.0, &|_, _| true), 1.0);
    }
}
