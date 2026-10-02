//! gm-bsp: Quake BSP loader for gamengine.
//!
//! Loads BSP version 29 (vanilla Quake) and BSP2 (ericw-tools `-bsp2`) as compiled by
//! `gm-tools map build`: geometry, embedded textures, lightmaps (grayscale in the BSP plus RGB
//! from a `.lit` file), the potentially visible set, entities, and the collision hulls used by
//! [`gm_core::trace::CollisionWorld`].
//!
//! Used by the client (rendering, prediction) and the server (authority, PVS culling), so
//! nothing here touches the GPU or the network.
#![forbid(unsafe_code)]

pub mod entity;
pub mod format;
mod lightmap;
mod pvs;
mod stalls;
mod trace;

use std::path::Path;

use glam::Vec3;

pub use entity::Entity;
pub use format::{
    BspError, Header, LUMP_CLIPNODES, LUMP_EDGES, LUMP_ENTITIES, LUMP_FACES, LUMP_LEAVES,
    LUMP_LIGHTING, LUMP_MARKSURFACES, LUMP_MODELS, LUMP_NODES, LUMP_PLANES, LUMP_SURFEDGES,
    LUMP_TEXINFO, LUMP_TEXTURES, LUMP_VERTICES, LUMP_VISIBILITY, Lump, NUM_LUMPS, Version,
};
pub use lightmap::{FaceLightmap, LightmapStats};
pub use stalls::StallGrid;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
    /// 0..=2 axial (x, y, z), 3..=5 dominant axis. Axial planes take a fast path in tracing.
    pub kind: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MipTex {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Palette indices of mip level 0, `width * height` bytes, when embedded.
    pub pixels: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub plane: u32,
    /// `>= 0`: child node index. `< 0`: leaf index `-1 - child`.
    pub children: [i32; 2],
    pub mins: Vec3,
    pub maxs: Vec3,
    pub first_face: u32,
    pub num_faces: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexInfo {
    /// `s = dot(v, s.xyz) + s.w`, in texels.
    pub s: [f32; 4],
    pub t: [f32; 4],
    pub miptex: u32,
    pub flags: u32,
}

impl TexInfo {
    /// Sky and liquid surfaces: no lightmap, no subdivision.
    pub const SPECIAL: u32 = 1;

    pub fn is_special(&self) -> bool {
        self.flags & Self::SPECIAL != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub plane: u32,
    /// The face's normal is the plane's, flipped when `side` is set.
    pub side: bool,
    pub first_edge: u32,
    pub num_edges: u32,
    pub texinfo: u32,
    /// Light styles; 255 marks an unused slot. Style 0 is the static lightmap.
    pub styles: [u8; 4],
    /// Byte offset into the lighting lump, or -1.
    pub light_ofs: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipNode {
    pub plane: u32,
    /// `>= 0`: child clipnode. `< 0`: contents.
    pub children: [i32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leaf {
    pub contents: i32,
    /// Offset into the visibility lump, or -1 for "everything visible".
    pub vis_ofs: i32,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub first_marksurface: u32,
    pub num_marksurfaces: u32,
    pub ambient: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    /// Root of the node tree (hull 0) and of the clipnode trees for hulls 1..3.
    pub head_nodes: [i32; 4],
    pub vis_leafs: i32,
    pub first_face: u32,
    pub num_faces: u32,
}

/// A loaded map.
#[derive(Clone, Debug)]
pub struct Bsp {
    pub version: Version,
    pub entities: Vec<Entity>,
    pub planes: Vec<Plane>,
    pub textures: Vec<MipTex>,
    pub vertices: Vec<Vec3>,
    pub visdata: Vec<u8>,
    pub nodes: Vec<Node>,
    pub texinfo: Vec<TexInfo>,
    pub faces: Vec<Face>,
    /// Grayscale lightmap samples, one byte per luxel.
    pub lighting: Vec<u8>,
    /// RGB lightmap samples from the `.lit` file, three bytes per luxel, same offsets x3.
    pub lit: Option<Vec<u8>>,
    pub clipnodes: Vec<ClipNode>,
    pub leaves: Vec<Leaf>,
    pub marksurfaces: Vec<u32>,
    pub edges: Vec<[u32; 2]>,
    pub surfedges: Vec<i32>,
    pub models: Vec<Model>,
    /// Hull 0 (point) clip tree, derived from `nodes` with leaf contents folded in.
    hull0: Vec<ClipNode>,
}

impl Bsp {
    /// Read `path` and, when present, `path` with the extension `.lit`.
    pub fn load(path: &Path) -> Result<Bsp, BspError> {
        let bytes = std::fs::read(path)?;
        let mut bsp = Bsp::parse(&bytes)?;
        let lit_path = path.with_extension("lit");
        if lit_path.is_file() {
            let lit = std::fs::read(&lit_path)?;
            bsp.attach_lit(&lit)?;
        }
        Ok(bsp)
    }

    /// Parse the header only (cheap; used by the budget checker).
    pub fn parse_header(bytes: &[u8]) -> Result<Header, BspError> {
        format::parse_header(bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Bsp, BspError> {
        format::parse(bytes)
    }

    /// Attach RGB lightmaps from a `.lit` file (`QLIT`, version 1).
    pub fn attach_lit(&mut self, lit: &[u8]) -> Result<(), BspError> {
        if lit.len() < 8 || &lit[0..4] != b"QLIT" {
            return Err(BspError::Invalid("lit file has no QLIT header".into()));
        }
        let version = i32::from_le_bytes([lit[4], lit[5], lit[6], lit[7]]);
        if version != 1 {
            return Err(BspError::Invalid(format!(
                "unsupported lit version {version}"
            )));
        }
        let data = &lit[8..];
        if data.len() != self.lighting.len() * 3 {
            return Err(BspError::Invalid(format!(
                "lit has {} samples, bsp lighting has {}",
                data.len() / 3,
                self.lighting.len()
            )));
        }
        self.lit = Some(data.to_vec());
        Ok(())
    }

    /// The world model.
    pub fn world(&self) -> &Model {
        &self.models[0]
    }

    /// The vertices of a face in winding order.
    pub fn face_vertices(&self, face: usize) -> impl Iterator<Item = Vec3> + '_ {
        let f = &self.faces[face];
        (f.first_edge..f.first_edge + f.num_edges).map(move |i| {
            let e = self.surfedges[i as usize];
            if e >= 0 {
                self.vertices[self.edges[e as usize][0] as usize]
            } else {
                self.vertices[self.edges[(-e) as usize][1] as usize]
            }
        })
    }

    /// Outward normal of a face (plane normal, flipped for back-facing sides).
    pub fn face_normal(&self, face: usize) -> Vec3 {
        let f = &self.faces[face];
        let n = self.planes[f.plane as usize].normal;
        if f.side { -n } else { n }
    }

    /// Point entity origin of the first `info_player_start`, if any.
    pub fn player_start(&self) -> Option<(Vec3, f32)> {
        self.entities
            .iter()
            .find(|e| e.classname() == "info_player_start")
            .and_then(|e| Some((e.origin()?, e.f32("angle").unwrap_or(0.0))))
    }
}
