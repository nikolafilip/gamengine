//! Turns a loaded BSP into GPU-ready data: one vertex/index buffer for the world with per-face
//! index ranges (so PVS culling is an index-buffer rebuild), a lightmap atlas, and the palette
//! textures expanded to an RGBA texture array.

use glam::Vec3;
use gm_bsp::Bsp;

/// Every embedded texture is resampled to this size so a single `texture_2d_array` holds them.
pub const TEXTURE_SIZE: u32 = 64;
const LUXEL: f32 = 16.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub lm_uv: [f32; 2],
    pub layer: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FaceRange {
    pub first_index: u32,
    pub index_count: u32,
}

pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct WorldMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Indexed by BSP face; `index_count == 0` for faces that are not drawn.
    pub face_ranges: Vec<FaceRange>,
    pub lightmap: Atlas,
    /// RGBA `TEXTURE_SIZE`² layers, one per BSP texture.
    pub texture_layers: Vec<Vec<u8>>,
}

pub type Palette = [[u8; 3]; 256];

/// Load a 768-byte palette; a neutral ramp when the file is missing.
pub fn load_palette(path: &std::path::Path) -> Palette {
    let mut pal = [[0u8; 3]; 256];
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() >= 768 => {
            for (i, c) in pal.iter_mut().enumerate() {
                c.copy_from_slice(&bytes[i * 3..i * 3 + 3]);
            }
        }
        _ => {
            log::warn!(
                "palette {} missing or short; using a gray ramp",
                path.display()
            );
            for (i, c) in pal.iter_mut().enumerate() {
                *c = [i as u8; 3];
            }
        }
    }
    pal
}

/// Shelf packer: returns `(atlas_w, atlas_h, positions)` for rectangles `(w, h)`.
fn pack(rects: &[(u32, u32)]) -> (u32, u32, Vec<(u32, u32)>) {
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(rects[i].1));
    for width in [256u32, 512, 1024, 2048] {
        let mut pos = vec![(0u32, 0u32); rects.len()];
        let (mut x, mut y, mut shelf_h) = (0u32, 0u32, 0u32);
        let mut ok = true;
        for &i in &order {
            let (w, h) = rects[i];
            if w > width {
                ok = false;
                break;
            }
            if x + w > width {
                x = 0;
                y += shelf_h;
                shelf_h = 0;
            }
            pos[i] = (x, y);
            x += w;
            shelf_h = shelf_h.max(h);
        }
        let height = (y + shelf_h).next_power_of_two().max(1);
        if ok && height <= 2048 {
            return (width, height, pos);
        }
    }
    panic!("lightmaps do not fit a 2048x2048 atlas");
}

fn newell_normal(points: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        n += Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    n
}

fn texture_layer(tex: &gm_bsp::MipTex, pal: &Palette) -> Vec<u8> {
    let n = (TEXTURE_SIZE * TEXTURE_SIZE) as usize;
    let mut out = vec![0u8; n * 4];
    match &tex.pixels {
        Some(px) if tex.width > 0 && tex.height > 0 => {
            for y in 0..TEXTURE_SIZE {
                for x in 0..TEXTURE_SIZE {
                    let sx = (x * tex.width / TEXTURE_SIZE) as usize;
                    let sy = (y * tex.height / TEXTURE_SIZE) as usize;
                    let idx = px.get(sy * tex.width as usize + sx).copied().unwrap_or(0) as usize;
                    let c = pal[idx];
                    let o = ((y * TEXTURE_SIZE + x) * 4) as usize;
                    out[o..o + 3].copy_from_slice(&c);
                    out[o + 3] = 255;
                }
            }
        }
        _ => {
            // Missing texture: magenta/black checkerboard, impossible to miss.
            for y in 0..TEXTURE_SIZE {
                for x in 0..TEXTURE_SIZE {
                    let on = ((x / 8) + (y / 8)) % 2 == 0;
                    let o = ((y * TEXTURE_SIZE + x) * 4) as usize;
                    out[o..o + 4].copy_from_slice(if on {
                        &[255, 0, 255, 255]
                    } else {
                        &[0, 0, 0, 255]
                    });
                }
            }
        }
    }
    out
}

pub fn build(bsp: &Bsp, pal: &Palette) -> WorldMesh {
    // Lightmap atlas: one rectangle per lit face, plus a 2x2 white block for unlit faces at (0,0).
    let world = bsp.world();
    let face_ids: Vec<usize> = (world.first_face..world.first_face + world.num_faces)
        .map(|f| f as usize)
        .collect();
    let lightmaps: Vec<Option<gm_bsp::FaceLightmap<'_>>> =
        face_ids.iter().map(|&f| bsp.face_lightmap(f)).collect();
    let mut rects = vec![(2u32, 2u32)];
    rects.extend(lightmaps.iter().flatten().map(|lm| (lm.width, lm.height)));
    let (aw, ah, pos) = pack(&rects);
    let mut rgba = vec![0u8; (aw * ah * 4) as usize];
    for y in 0..2 {
        for x in 0..2 {
            let o = ((y * aw + x) * 4) as usize;
            rgba[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    let mut rect_of_face = vec![None; lightmaps.len()];
    let mut next_rect = 1;
    for (i, lm) in lightmaps.iter().enumerate() {
        let Some(lm) = lm else { continue };
        let (ax, ay) = pos[next_rect];
        rect_of_face[i] = Some((ax, ay));
        next_rect += 1;
        for ly in 0..lm.height {
            for lx in 0..lm.width {
                let s = (ly * lm.width + lx) as usize;
                let o = (((ay + ly) * aw + ax + lx) * 4) as usize;
                let c = match lm.rgb {
                    Some(rgb) => [rgb[s * 3], rgb[s * 3 + 1], rgb[s * 3 + 2]],
                    None => [lm.gray[s]; 3],
                };
                rgba[o..o + 3].copy_from_slice(&c);
                rgba[o + 3] = 255;
            }
        }
    }

    let texture_layers: Vec<Vec<u8>> = bsp.textures.iter().map(|t| texture_layer(t, pal)).collect();

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut face_ranges = vec![FaceRange::default(); bsp.faces.len()];
    let mut points: Vec<Vec3> = Vec::new();
    for (i, &f) in face_ids.iter().enumerate() {
        let face = &bsp.faces[f];
        let ti = &bsp.texinfo[face.texinfo as usize];
        let tex = &bsp.textures[ti.miptex as usize];
        if tex.name.starts_with("skip")
            || tex.name.starts_with("clip")
            || tex.name.starts_with("trigger")
        {
            continue;
        }
        points.clear();
        points.extend(bsp.face_vertices(f));
        if points.len() < 3 {
            continue;
        }
        if newell_normal(&points).dot(bsp.face_normal(f)) < 0.0 {
            points.reverse();
        }
        let (tw, th) = (tex.width.max(1) as f32, tex.height.max(1) as f32);
        let lm = lightmaps[i];
        let base = vertices.len() as u32;
        for &p in &points {
            let (s, t) = Bsp::texcoord(ti, p);
            let lm_uv = match (lm, rect_of_face[i]) {
                (Some(lm), Some((ax, ay))) => [
                    (ax as f32 + (s - lm.mins[0] as f32) / LUXEL + 0.5) / aw as f32,
                    (ay as f32 + (t - lm.mins[1] as f32) / LUXEL + 0.5) / ah as f32,
                ],
                _ => [1.0 / aw as f32, 1.0 / ah as f32],
            };
            vertices.push(Vertex {
                pos: p.to_array(),
                uv: [s / tw, t / th],
                lm_uv,
                layer: ti.miptex,
            });
        }
        let first_index = indices.len() as u32;
        for k in 1..points.len() as u32 - 1 {
            indices.extend_from_slice(&[base, base + k, base + k + 1]);
        }
        face_ranges[f] = FaceRange {
            first_index,
            index_count: indices.len() as u32 - first_index,
        };
    }
    log::info!(
        "world mesh: {} vertices, {} triangles, lightmap atlas {aw}x{ah}, {} textures",
        vertices.len(),
        indices.len() / 3,
        texture_layers.len()
    );
    WorldMesh {
        vertices,
        indices,
        face_ranges,
        lightmap: Atlas {
            width: aw,
            height: ah,
            rgba,
        },
        texture_layers,
    }
}
