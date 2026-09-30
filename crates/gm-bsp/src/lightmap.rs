//! Face lightmap extents and sample lookup. Quake lightmaps use 16 texels per luxel, and a face's
//! lightmap covers `floor(min/16)..=ceil(max/16)` of its texture-space bounds.

use glam::Vec3;

use crate::{Bsp, TexInfo};

pub const LUXEL_SIZE: f32 = 16.0;

/// One face's static lightmap (style 0).
#[derive(Clone, Copy, Debug)]
pub struct FaceLightmap<'a> {
    pub width: u32,
    pub height: u32,
    /// Texture-space origin of luxel (0, 0), in texels (`texturemins`).
    pub mins: [i32; 2],
    /// Grayscale samples, `width * height` bytes.
    pub gray: &'a [u8],
    /// RGB samples when a `.lit` is attached, `3 * width * height` bytes.
    pub rgb: Option<&'a [u8]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightmapStats {
    pub lit_faces: usize,
    pub luxels: u64,
    pub max_width: u32,
    pub max_height: u32,
}

impl Bsp {
    /// Texture coordinates of a point on a face, in texels.
    pub fn texcoord(ti: &TexInfo, v: Vec3) -> (f32, f32) {
        let s = v.x * ti.s[0] + v.y * ti.s[1] + v.z * ti.s[2] + ti.s[3];
        let t = v.x * ti.t[0] + v.y * ti.t[1] + v.z * ti.t[2] + ti.t[3];
        (s, t)
    }

    /// `(texturemins, extents)` of a face in texels, or `None` for special (sky/liquid) faces.
    pub fn face_extents(&self, face: usize) -> Option<([i32; 2], [u32; 2])> {
        let f = &self.faces[face];
        let ti = &self.texinfo[f.texinfo as usize];
        if ti.is_special() {
            return None;
        }
        let mut mins = [f32::MAX; 2];
        let mut maxs = [f32::MIN; 2];
        for v in self.face_vertices(face) {
            let (s, t) = Self::texcoord(ti, v);
            mins[0] = mins[0].min(s);
            maxs[0] = maxs[0].max(s);
            mins[1] = mins[1].min(t);
            maxs[1] = maxs[1].max(t);
        }
        let mut tmins = [0i32; 2];
        let mut ext = [0u32; 2];
        for i in 0..2 {
            let bmin = (mins[i] / LUXEL_SIZE).floor() as i32;
            let bmax = (maxs[i] / LUXEL_SIZE).ceil() as i32;
            tmins[i] = bmin * LUXEL_SIZE as i32;
            ext[i] = ((bmax - bmin) * LUXEL_SIZE as i32).max(0) as u32;
        }
        Some((tmins, ext))
    }

    /// Lightmap of `face` (style 0), or `None` when the face has none.
    pub fn face_lightmap(&self, face: usize) -> Option<FaceLightmap<'_>> {
        let f = &self.faces[face];
        if f.light_ofs < 0 {
            return None;
        }
        let (mins, ext) = self.face_extents(face)?;
        let width = ext[0] / LUXEL_SIZE as u32 + 1;
        let height = ext[1] / LUXEL_SIZE as u32 + 1;
        let n = (width * height) as usize;
        let ofs = f.light_ofs as usize;
        let gray = self.lighting.get(ofs..ofs + n)?;
        let rgb = self
            .lit
            .as_ref()
            .and_then(|lit| lit.get(ofs * 3..(ofs + n) * 3));
        Some(FaceLightmap {
            width,
            height,
            mins,
            gray,
            rgb,
        })
    }

    pub fn lightmap_stats(&self) -> LightmapStats {
        let mut s = LightmapStats::default();
        for i in 0..self.faces.len() {
            if let Some(lm) = self.face_lightmap(i) {
                s.lit_faces += 1;
                s.luxels += (lm.width * lm.height) as u64;
                s.max_width = s.max_width.max(lm.width);
                s.max_height = s.max_height.max(lm.height);
            }
        }
        s
    }
}
