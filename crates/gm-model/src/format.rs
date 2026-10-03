//! The ingested model (`.gmm`, MODELS.md 5): what the hub stores, what clients fetch, hash and
//! cache. The reader trusts nothing: every length, count, index and joint is checked, so a
//! file that decodes can be handed to the GPU as it is.

use glam::Vec3;
use sha2::{Digest, Sha256};

use crate::rig::{ALL_BONES, BONES, REQUIRED};

/// SHA-256 of the whole `.gmm` file.
pub type ModelId = [u8; 32];

pub const MAGIC: [u8; 4] = *b"GMM1";
/// Texture format byte: BC1 with 1-bit alpha, sRGB.
pub const TEX_BC1_SRGB: u8 = 1;
pub const FLAG_CUTOUT: u16 = 1 << 0;
pub const FLAG_TWO_SIDED: u16 = 1 << 1;
/// A prop (CONTENT.md 4): a thing held, not a body. Its `frame` byte is `PROP_FRAME`, its
/// bone mask is bone 0 alone with zero pivots, and every vertex is on bone 0 with weight
/// 255: the character pipeline draws it with the hand's matrix at index 0 (LOOK.md 6.3).
pub const FLAG_PROP: u16 = 1 << 2;
/// The `frame` byte of a prop: no frame at all.
pub const PROP_FRAME: u8 = 255;

/// Hard limits of the format (PLAN.md 2.6; a test keeps them equal to `budgets.toml`).
pub mod limits {
    pub const MAX_TRIANGLES: usize = 3500;
    pub const MAX_VERTICES: usize = MAX_TRIANGLES * 3;
    pub const MIN_TEXTURE: u16 = 64;
    pub const MAX_TEXTURE: u16 = 1024;
    /// The whole file.
    pub const MAX_FILE_BYTES: usize = 1_572_864;
    /// A prop's triangles, texture side and file (CONTENT.md 4).
    pub const PROP_MAX_TRIANGLES: usize = 1000;
    pub const PROP_MAX_TEXTURE: u16 = 256;
    pub const PROP_MAX_FILE_BYTES: usize = 131_072;
    /// How far from its grip a prop may reach, in world units (3 m).
    pub const PROP_MAX_EXTENT: f32 = 96.0;
    /// The payload after inflation.
    pub const MAX_RAW_BYTES: usize = 2 * 1024 * 1024;
    /// Coordinates and pivots stay inside this cube (world units).
    pub const MAX_EXTENT: f32 = 256.0;
}

const HEADER_BYTES: usize = 12;
const FIXED_BYTES: usize = 12 + 4 + 4 + BONES * 12 + 16;

/// One vertex exactly as the GPU reads it: 24 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// `position = pos / 32767 × scale`; the fourth component is 0.
    pub pos: [i16; 4],
    /// Unit normal × 127; the fourth component is 0.
    pub normal: [i8; 4],
    /// Texture coordinates × 65535.
    pub uv: [u16; 2],
    /// Standard bone indices; unused influences are bone 0 with weight 0.
    pub joints: [u8; 4],
    /// Sum to 255.
    pub weights: [u8; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub flags: u16,
    /// Archetype frame index (`rig::frame_index`).
    pub frame: u8,
    pub scale: [f32; 3],
    /// Mean opaque texel, sRGB.
    pub average: [u8; 4],
    pub bone_mask: u32,
    pub pivots: [[f32; 3]; BONES],
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
    pub tex_w: u16,
    pub tex_h: u16,
    /// BC1 blocks, largest mip first.
    pub texture: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelError {
    #[error("not a .gmm file")]
    Magic,
    #[error("the file is {0} bytes; the limit is {1}")]
    TooLarge(usize, usize),
    #[error("truncated or padded: {0}")]
    Length(&'static str),
    #[error("the payload does not inflate to the declared length")]
    Inflate,
    #[error("out of range: {0}")]
    Range(&'static str),
    #[error("unsupported texture format {0}")]
    TextureFormat(u8),
}

pub fn model_id(file: &[u8]) -> ModelId {
    Sha256::digest(file).into()
}

pub fn id_hex(id: &ModelId) -> String {
    let mut s = String::with_capacity(64);
    for b in id {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 15) as u32, 16).unwrap());
    }
    s
}

pub fn id_from_hex(s: &str) -> Option<ModelId> {
    let s = s.as_bytes();
    if s.len() != 64 {
        return None;
    }
    let mut id = [0u8; 32];
    for (i, pair) in s.chunks_exact(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        id[i] = (hi * 16 + lo) as u8;
    }
    Some(id)
}

/// Mip levels of a `w × h` texture: down to the level whose smaller side is 4.
pub fn mip_count(w: u16, h: u16) -> u8 {
    (w.min(h).max(4).ilog2() - 1) as u8
}

/// Bytes of BC1 for one level.
pub fn bc1_level_bytes(w: usize, h: usize) -> usize {
    w.div_ceil(4) * h.div_ceil(4) * 8
}

/// Bytes of the whole BC1 mip chain.
pub fn texture_bytes(w: u16, h: u16) -> usize {
    (0..mip_count(w, h))
        .map(|i| bc1_level_bytes((w >> i) as usize, (h >> i) as usize))
        .sum()
}

fn valid_texture_side(v: u16) -> bool {
    v.is_power_of_two() && (limits::MIN_TEXTURE..=limits::MAX_TEXTURE).contains(&v)
}

/// Weights as bytes summing to exactly 255 (largest remainder), heaviest first, with zero
/// weights on bone 0.
pub fn quantize_weights(joints: [u8; 4], weights: [f32; 4]) -> ([u8; 4], [u8; 4]) {
    let mut pairs: Vec<(u8, f32)> = joints
        .into_iter()
        .zip(weights)
        .filter(|(_, w)| w.is_finite() && *w > 0.0)
        .collect();
    // The same bone twice is one influence.
    pairs.sort_by_key(|(j, _)| *j);
    pairs.dedup_by(|b, a| {
        if a.0 == b.0 {
            a.1 += b.1;
            true
        } else {
            false
        }
    });
    let total: f32 = pairs.iter().map(|(_, w)| w).sum();
    if pairs.is_empty() || total <= 0.0 {
        return ([joints[0], 0, 0, 0], [255, 0, 0, 0]);
    }
    pairs.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let exact: Vec<f32> = pairs.iter().map(|(_, w)| w / total * 255.0).collect();
    let mut q: Vec<u8> = exact.iter().map(|e| e.floor() as u8).collect();
    let mut left = 255 - q.iter().map(|v| *v as i32).sum::<i32>();
    let mut order: Vec<usize> = (0..q.len()).collect();
    order.sort_by(|&a, &b| {
        (exact[b] - exact[b].floor())
            .total_cmp(&(exact[a] - exact[a].floor()))
            .then(a.cmp(&b))
    });
    for &i in order.iter().cycle() {
        if left <= 0 {
            break;
        }
        q[i] += 1;
        left -= 1;
    }
    let mut out_j = [0u8; 4];
    let mut out_w = [0u8; 4];
    let mut n = 0;
    for (i, (j, _)) in pairs.iter().enumerate() {
        if q[i] > 0 {
            out_j[n] = *j;
            out_w[n] = q[i];
            n += 1;
        }
    }
    (out_j, out_w)
}

/// Quantise a mesh into wire vertices; returns the scale positions were divided by.
pub fn quantize_vertices(
    positions: &[Vec3],
    normals: &[Vec3],
    uvs: &[[f32; 2]],
    joints: &[[u8; 4]],
    weights: &[[f32; 4]],
) -> ([f32; 3], Vec<Vertex>) {
    let mut scale = Vec3::splat(1e-3);
    for p in positions {
        scale = scale.max(p.abs());
    }
    let q = |v: f32, s: f32| (v / s * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
    let vertices = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let n = normals[i].normalize_or(Vec3::Z);
            let (j, w) = quantize_weights(joints[i], weights[i]);
            let uv = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
            Vertex {
                pos: [q(p.x, scale.x), q(p.y, scale.y), q(p.z, scale.z), 0],
                normal: [
                    (n.x * 127.0).round() as i8,
                    (n.y * 127.0).round() as i8,
                    (n.z * 127.0).round() as i8,
                    0,
                ],
                uv: [uv(uvs[i][0]), uv(uvs[i][1])],
                joints: j,
                weights: w,
            }
        })
        .collect();
    (scale.to_array(), vertices)
}

impl Model {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn cutout(&self) -> bool {
        self.flags & FLAG_CUTOUT != 0
    }

    pub fn two_sided(&self) -> bool {
        self.flags & FLAG_TWO_SIDED != 0
    }

    /// A thing held (CONTENT.md 4), not a body: no frame, no rig.
    pub fn is_prop(&self) -> bool {
        self.flags & FLAG_PROP != 0
    }

    pub fn position(&self, i: usize) -> Vec3 {
        let v = &self.vertices[i];
        Vec3::new(
            v.pos[0] as f32 / 32767.0 * self.scale[0],
            v.pos[1] as f32 / 32767.0 * self.scale[1],
            v.pos[2] as f32 / 32767.0 * self.scale[2],
        )
    }

    pub fn normal(&self, i: usize) -> Vec3 {
        let n = self.vertices[i].normal;
        Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) / 127.0
    }

    pub fn uv(&self, i: usize) -> [f32; 2] {
        let v = self.vertices[i].uv;
        [v[0] as f32 / 65535.0, v[1] as f32 / 65535.0]
    }

    pub fn pivot(&self, bone: usize) -> Vec3 {
        Vec3::from(self.pivots[bone])
    }

    /// `(width, height, blocks)` per mip level, largest first.
    pub fn mips(&self) -> impl Iterator<Item = (u32, u32, &[u8])> {
        let mut offset = 0;
        (0..mip_count(self.tex_w, self.tex_h)).map(move |i| {
            let (w, h) = ((self.tex_w >> i) as usize, (self.tex_h >> i) as usize);
            let len = bc1_level_bytes(w, h);
            let blocks = &self.texture[offset..offset + len];
            offset += len;
            (w as u32, h as u32, blocks)
        })
    }

    /// Bytes this model occupies on the GPU.
    pub fn gpu_bytes(&self) -> usize {
        self.vertices.len() * std::mem::size_of::<Vertex>()
            + self.indices.len() * 2
            + self.texture.len()
    }

    /// Everything the reader will check, checked on the writer's side too.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.flags & !(FLAG_CUTOUT | FLAG_TWO_SIDED | FLAG_PROP) != 0 {
            return Err(ModelError::Range("flags"));
        }
        let prop = self.is_prop();
        // A prop is a prop in every way at once, or it is not one: the frame byte, the
        // flag and the rig agree, so that nothing reads a held thing as a body.
        if prop {
            if self.frame != PROP_FRAME {
                return Err(ModelError::Range("a prop has no frame"));
            }
            if self.bone_mask != 1 || self.pivots.iter().flatten().any(|v| *v != 0.0) {
                return Err(ModelError::Range("a prop has no rig"));
            }
            if self.indices.len() > limits::PROP_MAX_TRIANGLES * 3 {
                return Err(ModelError::Range("a prop's triangles"));
            }
            if self.tex_w > limits::PROP_MAX_TEXTURE || self.tex_h > limits::PROP_MAX_TEXTURE {
                return Err(ModelError::Range("a prop's texture"));
            }
            if !self
                .scale
                .iter()
                .all(|s| s.is_finite() && *s > 0.0 && *s <= limits::PROP_MAX_EXTENT)
            {
                return Err(ModelError::Range("a prop's extent"));
            }
            if self
                .vertices
                .iter()
                .any(|v| v.joints != [0; 4] || v.weights != [255, 0, 0, 0])
            {
                return Err(ModelError::Range("a prop's vertices are all on bone 0"));
            }
        } else if self.frame > 3 {
            return Err(ModelError::Range("frame"));
        }
        if !self
            .scale
            .iter()
            .all(|s| s.is_finite() && *s > 0.0 && *s <= limits::MAX_EXTENT)
        {
            return Err(ModelError::Range("scale"));
        }
        if !prop && (self.bone_mask & !ALL_BONES != 0 || self.bone_mask & REQUIRED != REQUIRED) {
            return Err(ModelError::Range("bone mask"));
        }
        if !self
            .pivots
            .iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= limits::MAX_EXTENT)
        {
            return Err(ModelError::Range("pivots"));
        }
        if self.vertices.len() < 3 || self.vertices.len() > limits::MAX_VERTICES {
            return Err(ModelError::Range("vertex count"));
        }
        if self.indices.len() < 3
            || !self.indices.len().is_multiple_of(3)
            || self.indices.len() > limits::MAX_TRIANGLES * 3
        {
            return Err(ModelError::Range("index count"));
        }
        if !valid_texture_side(self.tex_w) || !valid_texture_side(self.tex_h) {
            return Err(ModelError::Range("texture size"));
        }
        if self.texture.len() != texture_bytes(self.tex_w, self.tex_h) {
            return Err(ModelError::Length("texture"));
        }
        let n = self.vertices.len();
        if self.indices.iter().any(|&i| i as usize >= n) {
            return Err(ModelError::Range("index past the vertices"));
        }
        for v in &self.vertices {
            if v.pos[3] != 0 || v.normal[3] != 0 {
                return Err(ModelError::Range("vertex padding"));
            }
            if v.pos.contains(&i16::MIN) || v.normal.contains(&i8::MIN) {
                return Err(ModelError::Range("vertex component"));
            }
            let mut sum = 0u32;
            for k in 0..4 {
                let (j, w) = (v.joints[k] as usize, v.weights[k]);
                sum += w as u32;
                if w == 0 {
                    if k > 0 && j != 0 {
                        return Err(ModelError::Range("unused joint"));
                    }
                } else if j >= BONES || self.bone_mask & (1 << j) == 0 {
                    return Err(ModelError::Range("joint not in the model"));
                }
            }
            if sum != 255 {
                return Err(ModelError::Range("weights"));
            }
            // A zero-weight first joint is still read by the shader: it must exist.
            if v.joints[0] as usize >= BONES {
                return Err(ModelError::Range("joint"));
            }
        }
        Ok(())
    }

    /// The `.gmm` bytes. Deterministic: the same model always encodes to the same file.
    pub fn encode(&self) -> Result<Vec<u8>, ModelError> {
        self.validate()?;
        let mut raw = Vec::with_capacity(
            FIXED_BYTES + self.vertices.len() * 24 + self.indices.len() * 2 + self.texture.len(),
        );
        for s in self.scale {
            raw.extend_from_slice(&s.to_le_bytes());
        }
        raw.extend_from_slice(&self.average);
        raw.extend_from_slice(&self.bone_mask.to_le_bytes());
        for p in self.pivots.iter().flatten() {
            raw.extend_from_slice(&p.to_le_bytes());
        }
        raw.extend_from_slice(&(self.vertices.len() as u32).to_le_bytes());
        raw.extend_from_slice(&(self.indices.len() as u32).to_le_bytes());
        raw.extend_from_slice(&self.tex_w.to_le_bytes());
        raw.extend_from_slice(&self.tex_h.to_le_bytes());
        raw.push(mip_count(self.tex_w, self.tex_h));
        raw.extend_from_slice(&[0, 0, 0]);
        raw.extend_from_slice(bytemuck::cast_slice(&self.vertices));
        raw.extend_from_slice(bytemuck::cast_slice(&self.indices));
        raw.extend_from_slice(&self.texture);
        if raw.len() > limits::MAX_RAW_BYTES {
            return Err(ModelError::TooLarge(raw.len(), limits::MAX_RAW_BYTES));
        }
        let mut out = Vec::with_capacity(HEADER_BYTES + raw.len() / 2);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.push(self.frame);
        out.push(TEX_BC1_SRGB);
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
        let most = if self.is_prop() {
            limits::PROP_MAX_FILE_BYTES
        } else {
            limits::MAX_FILE_BYTES
        };
        if out.len() > most {
            return Err(ModelError::TooLarge(out.len(), most));
        }
        Ok(out)
    }

    pub fn decode(file: &[u8]) -> Result<Model, ModelError> {
        if file.len() > limits::MAX_FILE_BYTES {
            return Err(ModelError::TooLarge(file.len(), limits::MAX_FILE_BYTES));
        }
        if file.len() < HEADER_BYTES || file[..4] != MAGIC {
            return Err(ModelError::Magic);
        }
        let flags = u16::from_le_bytes([file[4], file[5]]);
        let frame = file[6];
        if flags & FLAG_PROP != 0 && file.len() > limits::PROP_MAX_FILE_BYTES {
            return Err(ModelError::TooLarge(
                file.len(),
                limits::PROP_MAX_FILE_BYTES,
            ));
        }
        if file[7] != TEX_BC1_SRGB {
            return Err(ModelError::TextureFormat(file[7]));
        }
        let raw_len = u32::from_le_bytes([file[8], file[9], file[10], file[11]]) as usize;
        if !(FIXED_BYTES..=limits::MAX_RAW_BYTES).contains(&raw_len) {
            return Err(ModelError::Range("payload length"));
        }
        let raw =
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&file[HEADER_BYTES..], raw_len)
                .map_err(|_| ModelError::Inflate)?;
        if raw.len() != raw_len {
            return Err(ModelError::Inflate);
        }
        let mut r = Reader { buf: &raw, at: 0 };
        let scale = [r.f32()?, r.f32()?, r.f32()?];
        let average = r.take::<4>()?;
        let bone_mask = r.u32()?;
        let mut pivots = [[0f32; 3]; BONES];
        for p in pivots.iter_mut().flatten() {
            *p = r.f32()?;
        }
        let vertex_count = r.u32()? as usize;
        let index_count = r.u32()? as usize;
        let tex_w = r.u16()?;
        let tex_h = r.u16()?;
        let mips = r.take::<4>()?;
        if vertex_count > limits::MAX_VERTICES || index_count > limits::MAX_TRIANGLES * 3 {
            return Err(ModelError::Range("counts"));
        }
        if !valid_texture_side(tex_w) || !valid_texture_side(tex_h) {
            return Err(ModelError::Range("texture size"));
        }
        if mips != [mip_count(tex_w, tex_h), 0, 0, 0] {
            return Err(ModelError::Range("mip count"));
        }
        let vertices: Vec<Vertex> = r
            .bytes(vertex_count * 24)?
            .chunks_exact(24)
            .map(bytemuck::pod_read_unaligned)
            .collect();
        let indices: Vec<u16> = r
            .bytes(index_count * 2)?
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let texture = r.bytes(texture_bytes(tex_w, tex_h))?.to_vec();
        if r.at != raw.len() {
            return Err(ModelError::Length("trailing bytes"));
        }
        let model = Model {
            flags,
            frame,
            scale,
            average,
            bone_mask,
            pivots,
            vertices,
            indices,
            tex_w,
            tex_h,
            texture,
        };
        model.validate()?;
        Ok(model)
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ModelError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or(ModelError::Length("payload"))?;
        let out = &self.buf[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], ModelError> {
        Ok(self.bytes(N)?.try_into().expect("length checked"))
    }

    fn u16(&mut self) -> Result<u16, ModelError> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32, ModelError> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn f32(&mut self) -> Result<f32, ModelError> {
        Ok(f32::from_le_bytes(self.take()?))
    }
}

#[cfg(target_endian = "big")]
compile_error!("the .gmm vertex layout is little-endian; a big-endian target needs a byte swap");

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::rig;

    /// A small valid model: one triangle per required bone, a 64 × 64 texture.
    pub(crate) fn sample() -> Model {
        let pivots = rig::rest_pivots(gm_core::vocab::ArchetypeFrame::Striker);
        let mut positions = Vec::new();
        let mut joints = Vec::new();
        for (b, &c) in pivots.iter().enumerate() {
            if REQUIRED & (1 << b) == 0 {
                continue;
            }
            positions.extend([c, c + Vec3::X * 2.0, c + Vec3::Z * 2.0]);
            joints.extend([[b as u8, 0, 0, 0]; 3]);
        }
        let n = positions.len();
        let normals = vec![Vec3::NEG_Y; n];
        let uvs: Vec<[f32; 2]> = (0..n).map(|i| [i as f32 / n as f32, 0.5]).collect();
        let weights = vec![[1.0, 0.0, 0.0, 0.0]; n];
        let (scale, vertices) = quantize_vertices(&positions, &normals, &uvs, &joints, &weights);
        let texture: Vec<u8> = (0..texture_bytes(64, 64)).map(|i| (i * 7) as u8).collect();
        Model {
            flags: FLAG_CUTOUT,
            frame: 1,
            scale,
            average: [120, 90, 60, 255],
            bone_mask: REQUIRED,
            pivots: pivots.map(|p| p.to_array()),
            vertices,
            indices: (0..n as u16).collect(),
            tex_w: 64,
            tex_h: 64,
            texture,
        }
    }

    #[test]
    fn round_trip_is_exact_and_deterministic() {
        let m = sample();
        let file = m.encode().unwrap();
        assert_eq!(file, m.encode().unwrap());
        let back = Model::decode(&file).unwrap();
        assert_eq!(back, m);
        assert_eq!(model_id(&file), model_id(&back.encode().unwrap()));
        assert_eq!(
            id_from_hex(&id_hex(&model_id(&file))),
            Some(model_id(&file))
        );
        assert_eq!(id_from_hex("xyz"), None);
    }

    /// A small valid prop: a blade of two triangles along +X, a 64 × 64 texture.
    pub(crate) fn sample_prop() -> Model {
        let positions = vec![
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(40.0, -1.0, 0.0),
            Vec3::new(40.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let normals = vec![Vec3::Z; 4];
        let uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let joints = vec![[0u8; 4]; 4];
        let weights = vec![[1.0, 0.0, 0.0, 0.0]; 4];
        let (scale, vertices) = quantize_vertices(&positions, &normals, &uvs, &joints, &weights);
        let texture: Vec<u8> = (0..texture_bytes(64, 64)).map(|i| (i * 3) as u8).collect();
        Model {
            flags: FLAG_PROP | FLAG_TWO_SIDED,
            frame: PROP_FRAME,
            scale,
            average: [180, 180, 190, 255],
            bone_mask: 1,
            pivots: [[0.0; 3]; BONES],
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3],
            tex_w: 64,
            tex_h: 64,
            texture,
        }
    }

    #[test]
    fn a_prop_round_trips_and_is_a_prop_in_every_way_or_not_at_all() {
        let p = sample_prop();
        let file = p.encode().unwrap();
        let back = Model::decode(&file).unwrap();
        assert_eq!(back, p);
        assert!(back.is_prop() && back.frame == PROP_FRAME);
        // The flag without the frame byte, the frame byte without the flag, a rig, a
        // vertex on another bone, too far a reach: each is refused.
        let mut m = sample_prop();
        m.frame = 1;
        assert!(m.encode().is_err());
        let mut m = sample();
        m.frame = PROP_FRAME;
        assert!(m.encode().is_err());
        let mut m = sample_prop();
        m.bone_mask = REQUIRED | 1;
        assert!(m.encode().is_err());
        let mut m = sample_prop();
        m.vertices[0].joints = [1, 0, 0, 0];
        m.bone_mask = 0b11;
        assert!(m.encode().is_err());
        let mut m = sample_prop();
        m.scale = [limits::PROP_MAX_EXTENT + 1.0, 1.0, 1.0];
        assert!(m.encode().is_err());
        // And an avatar's reader refuses it where a frame is expected: the flag is read.
        let mut m = sample_prop();
        m.tex_w = 512;
        m.tex_h = 512;
        m.texture = (0..texture_bytes(512, 512)).map(|i| i as u8).collect();
        assert!(matches!(m.encode(), Err(ModelError::Range(_))));
    }

    #[test]
    fn positions_survive_quantisation() {
        let m = sample();
        let pivots = rig::rest_pivots(gm_core::vocab::ArchetypeFrame::Striker);
        // The first vertex of the first triangle sits on the hips pivot.
        assert!((m.position(0) - pivots[0]).length() < 0.01);
        assert!((m.normal(0) - Vec3::NEG_Y).length() < 0.01);
    }

    #[test]
    fn mip_chain_sizes() {
        assert_eq!(mip_count(1024, 1024), 9);
        assert_eq!(mip_count(64, 64), 5);
        assert_eq!(mip_count(1024, 256), 7);
        assert_eq!(texture_bytes(1024, 1024), 699_048);
        assert_eq!(texture_bytes(64, 64), (256 + 64 + 16 + 4 + 1) * 8);
        let m = sample();
        let levels: Vec<(u32, u32, usize)> = m.mips().map(|(w, h, b)| (w, h, b.len())).collect();
        assert_eq!(levels.len(), 5);
        assert_eq!(levels[0], (64, 64, 2048));
        assert_eq!(levels[4], (4, 4, 8));
    }

    #[test]
    fn weights_always_sum_to_255() {
        for w in [
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.0, 0.0],
            [0.3333, 0.3333, 0.3334, 0.0],
            [0.25, 0.25, 0.25, 0.25],
            [0.001, 0.999, 0.0, 0.0],
            [3.0, 1.0, 0.0, 0.0],
        ] {
            let (j, q) = quantize_weights([4, 5, 6, 7], w);
            assert_eq!(q.iter().map(|v| *v as u32).sum::<u32>(), 255, "{w:?}");
            assert!(q[0] >= q[1], "heaviest first: {q:?}");
            for k in 0..4 {
                if q[k] == 0 {
                    assert_eq!(j[k], 0);
                }
            }
        }
        // The same bone twice merges; nothing at all falls back to the first joint.
        assert_eq!(
            quantize_weights([2, 2, 0, 0], [0.5, 0.5, 0.0, 0.0]),
            ([2, 0, 0, 0], [255, 0, 0, 0])
        );
        assert_eq!(
            quantize_weights([9, 1, 2, 3], [0.0; 4]),
            ([9, 0, 0, 0], [255, 0, 0, 0])
        );
        assert_eq!(
            quantize_weights([9, 1, 2, 3], [f32::NAN, 0.0, 0.0, 0.0]),
            ([9, 0, 0, 0], [255, 0, 0, 0])
        );
    }

    #[test]
    fn hostile_files_are_refused() {
        let m = sample();
        let file = m.encode().unwrap();
        assert_eq!(Model::decode(b"nope"), Err(ModelError::Magic));
        assert_eq!(Model::decode(&[]), Err(ModelError::Magic));
        let mut bad = file.clone();
        bad[7] = 9;
        assert_eq!(Model::decode(&bad), Err(ModelError::TextureFormat(9)));
        // A declared length that does not match the stream.
        let mut bad = file.clone();
        bad[8] = bad[8].wrapping_add(1);
        assert_eq!(Model::decode(&bad), Err(ModelError::Inflate));
        // A bomb: the header claims more than the cap.
        let mut bad = file.clone();
        bad[8..12].copy_from_slice(&(64u32 << 20).to_le_bytes());
        assert_eq!(
            Model::decode(&bad),
            Err(ModelError::Range("payload length"))
        );
        // Truncation anywhere never panics.
        for cut in [0, 5, 11, 12, 20, file.len() / 2, file.len() - 1] {
            assert!(Model::decode(&file[..cut]).is_err(), "cut at {cut}");
        }
        let huge = vec![0u8; limits::MAX_FILE_BYTES + 1];
        assert!(matches!(
            Model::decode(&huge),
            Err(ModelError::TooLarge(..))
        ));
    }

    #[test]
    fn invalid_contents_are_refused_on_both_sides() {
        let reject = |f: &dyn Fn(&mut Model), what: &str| {
            let mut m = sample();
            f(&mut m);
            assert!(m.validate().is_err(), "{what} validated");
            assert!(m.encode().is_err(), "{what} encoded");
        };
        reject(&|m| m.indices[0] = 9999, "an index past the vertices");
        reject(&|m| m.vertices[0].joints[0] = 30, "a joint past the rig");
        reject(
            &|m| m.vertices[0].joints[0] = rig::bone::TOE_L as u8,
            "a joint not in the mask",
        );
        reject(
            &|m| m.vertices[0].weights = [200, 0, 0, 0],
            "weights not summing to 255",
        );
        reject(&|m| m.bone_mask &= !1, "a missing required bone");
        reject(&|m| m.bone_mask |= 1 << 30, "a bone past the rig");
        reject(&|m| m.scale[1] = f32::NAN, "a NaN scale");
        reject(&|m| m.pivots[3][2] = f32::INFINITY, "an infinite pivot");
        reject(&|m| m.tex_w = 100, "a texture that is not a power of two");
        reject(&|m| m.tex_h = 2048, "a texture past the budget");
        reject(&|m| m.texture.push(0), "a texture of the wrong length");
        reject(&|m| m.indices.push(0), "indices not a multiple of three");
        reject(&|m| m.flags = 0x80, "an unknown flag");
        reject(&|m| m.frame = 4, "an unknown frame");
        reject(&|m| m.vertices[1].pos[3] = 1, "vertex padding");
        reject(
            &|m| m.vertices = vec![m.vertices[0]; limits::MAX_VERTICES + 1],
            "too many vertices",
        );
    }

    #[test]
    fn limits_match_budgets_toml() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../budgets.toml"))
                .expect("budgets.toml");
        let value = |key: &str| -> usize {
            let mut in_character = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_character = line == "[character]";
                } else if in_character
                    && let Some((k, v)) = line.split_once('=')
                    && k.trim() == key
                {
                    return v.split('#').next().unwrap().trim().parse().unwrap();
                }
            }
            panic!("budgets.toml has no [character] {key}");
        };
        assert_eq!(limits::MAX_TRIANGLES, value("max_triangles"));
        assert_eq!(limits::MAX_TEXTURE as usize, value("max_texture_size"));
        assert_eq!(limits::MAX_FILE_BYTES, value("max_payload_bytes"));
        assert!(BONES <= value("max_bones"));
        // And a prop's (CONTENT.md 4), under [content].
        let content = |key: &str| -> usize {
            let mut in_section = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_section = line == "[content]";
                } else if in_section
                    && let Some((k, v)) = line.split_once('=')
                    && k.trim() == key
                {
                    return v.split('#').next().unwrap().trim().parse().unwrap();
                }
            }
            panic!("budgets.toml has no [content] {key}");
        };
        assert_eq!(limits::PROP_MAX_TRIANGLES, content("max_prop_triangles"));
        assert_eq!(
            limits::PROP_MAX_TEXTURE as usize,
            content("max_prop_texture")
        );
        assert_eq!(limits::PROP_MAX_FILE_BYTES, content("max_prop_gmm_bytes"));
    }
}
