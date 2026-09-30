//! On-disk layout of BSP29 and BSP2 and the parser that turns bytes into [`Bsp`].

use glam::Vec3;
use thiserror::Error;

use crate::{Bsp, ClipNode, Face, Leaf, MipTex, Model, Node, Plane, TexInfo, entity};

#[derive(Debug, Error)]
pub enum BspError {
    #[error("not a Quake BSP (magic {0:#010x}; expected 29 or \"BSP2\")")]
    BadMagic(u32),
    #[error("truncated data while reading {0}")]
    Truncated(&'static str),
    #[error("lump {0} lies outside the file")]
    BadLump(usize),
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub const LUMP_ENTITIES: usize = 0;
pub const LUMP_PLANES: usize = 1;
pub const LUMP_TEXTURES: usize = 2;
pub const LUMP_VERTICES: usize = 3;
pub const LUMP_VISIBILITY: usize = 4;
pub const LUMP_NODES: usize = 5;
pub const LUMP_TEXINFO: usize = 6;
pub const LUMP_FACES: usize = 7;
pub const LUMP_LIGHTING: usize = 8;
pub const LUMP_CLIPNODES: usize = 9;
pub const LUMP_LEAVES: usize = 10;
pub const LUMP_MARKSURFACES: usize = 11;
pub const LUMP_EDGES: usize = 12;
pub const LUMP_SURFEDGES: usize = 13;
pub const LUMP_MODELS: usize = 14;
pub const NUM_LUMPS: usize = 15;

const BSP29: u32 = 29;
const BSP2: u32 = u32::from_le_bytes(*b"BSP2");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Bsp29,
    Bsp2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lump {
    pub ofs: u32,
    pub len: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub version: Version,
    pub lumps: [Lump; NUM_LUMPS],
}

pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8], what: &'static str) -> Self {
        Reader { data, pos: 0, what }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], BspError> {
        let s = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or(BspError::Truncated(self.what))?;
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, BspError> {
        Ok(self.bytes(1)?[0])
    }

    pub(crate) fn i16(&mut self) -> Result<i16, BspError> {
        Ok(i16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub(crate) fn u16(&mut self) -> Result<u16, BspError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, BspError> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, BspError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, BspError> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub(crate) fn vec3(&mut self) -> Result<Vec3, BspError> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    pub(crate) fn vec3_i16(&mut self) -> Result<Vec3, BspError> {
        Ok(Vec3::new(
            self.i16()? as f32,
            self.i16()? as f32,
            self.i16()? as f32,
        ))
    }
}

pub fn parse_header(bytes: &[u8]) -> Result<Header, BspError> {
    let mut r = Reader::new(bytes, "header");
    let magic = r.u32()?;
    let version = match magic {
        BSP29 => Version::Bsp29,
        BSP2 => Version::Bsp2,
        other => return Err(BspError::BadMagic(other)),
    };
    let mut lumps = [Lump::default(); NUM_LUMPS];
    for lump in &mut lumps {
        let ofs = r.i32()?;
        let len = r.i32()?;
        if ofs < 0 || len < 0 {
            return Err(BspError::Invalid("negative lump offset or length".into()));
        }
        *lump = Lump {
            ofs: ofs as u32,
            len: len as u32,
        };
    }
    Ok(Header { version, lumps })
}

fn lump<'a>(bytes: &'a [u8], header: &Header, index: usize) -> Result<&'a [u8], BspError> {
    let l = header.lumps[index];
    bytes
        .get(l.ofs as usize..(l.ofs + l.len) as usize)
        .ok_or(BspError::BadLump(index))
}

fn parse_array<T>(
    data: &[u8],
    what: &'static str,
    stride: usize,
    mut f: impl FnMut(&mut Reader<'_>) -> Result<T, BspError>,
) -> Result<Vec<T>, BspError> {
    if stride == 0 || !data.len().is_multiple_of(stride) {
        return Err(BspError::Invalid(format!(
            "{what} lump length {} is not a multiple of {stride}",
            data.len()
        )));
    }
    let mut r = Reader::new(data, what);
    let mut out = Vec::with_capacity(data.len() / stride);
    while r.remaining() > 0 {
        out.push(f(&mut r)?);
    }
    Ok(out)
}

fn parse_textures(data: &[u8]) -> Result<Vec<MipTex>, BspError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(data, "textures");
    let count = r.i32()?;
    if count < 0 {
        return Err(BspError::Invalid("negative texture count".into()));
    }
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let ofs = r.i32()?;
        if ofs < 0 {
            out.push(MipTex {
                name: String::new(),
                width: 0,
                height: 0,
                pixels: None,
            });
            continue;
        }
        let start = ofs as usize;
        let mut m = Reader::new(
            data.get(start..).ok_or(BspError::Truncated("miptex"))?,
            "miptex",
        );
        let name_bytes = m.bytes(16)?;
        let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
        let name = String::from_utf8_lossy(&name_bytes[..name_end]).into_owned();
        let width = m.u32()?;
        let height = m.u32()?;
        let mut offsets = [0u32; 4];
        for o in &mut offsets {
            *o = m.u32()?;
        }
        let pixels = if offsets[0] != 0 && width > 0 && height > 0 {
            let n = (width as usize) * (height as usize);
            let p = data.get(start + offsets[0] as usize..start + offsets[0] as usize + n);
            p.map(|p| p.to_vec())
        } else {
            None
        };
        out.push(MipTex {
            name,
            width,
            height,
            pixels,
        });
    }
    Ok(out)
}

fn check_index(value: i64, len: usize, what: &str) -> Result<(), BspError> {
    if value < 0 || value as usize >= len {
        return Err(BspError::Invalid(format!(
            "{what} index {value} out of range (len {len})"
        )));
    }
    Ok(())
}

pub fn parse(bytes: &[u8]) -> Result<Bsp, BspError> {
    let header = parse_header(bytes)?;
    let bsp2 = header.version == Version::Bsp2;
    let l = |i| lump(bytes, &header, i);

    let ent_bytes = l(LUMP_ENTITIES)?;
    let ent_end = ent_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(ent_bytes.len());
    let entities = entity::parse_entities(&String::from_utf8_lossy(&ent_bytes[..ent_end]))?;

    let planes = parse_array(l(LUMP_PLANES)?, "planes", 20, |r| {
        Ok(Plane {
            normal: r.vec3()?,
            dist: r.f32()?,
            kind: r.i32()? as u32,
        })
    })?;
    let textures = parse_textures(l(LUMP_TEXTURES)?)?;
    let vertices = parse_array(l(LUMP_VERTICES)?, "vertices", 12, |r| r.vec3())?;
    let visdata = l(LUMP_VISIBILITY)?.to_vec();

    let nodes = if bsp2 {
        parse_array(l(LUMP_NODES)?, "nodes", 44, |r| {
            Ok(Node {
                plane: r.i32()? as u32,
                children: [r.i32()?, r.i32()?],
                mins: r.vec3()?,
                maxs: r.vec3()?,
                first_face: r.u32()?,
                num_faces: r.u32()?,
            })
        })?
    } else {
        parse_array(l(LUMP_NODES)?, "nodes", 24, |r| {
            Ok(Node {
                plane: r.i32()? as u32,
                children: [r.i16()? as i32, r.i16()? as i32],
                mins: r.vec3_i16()?,
                maxs: r.vec3_i16()?,
                first_face: r.u16()? as u32,
                num_faces: r.u16()? as u32,
            })
        })?
    };

    let texinfo = parse_array(l(LUMP_TEXINFO)?, "texinfo", 40, |r| {
        Ok(TexInfo {
            s: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
            t: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
            miptex: r.i32()? as u32,
            flags: r.i32()? as u32,
        })
    })?;

    let faces = if bsp2 {
        parse_array(l(LUMP_FACES)?, "faces", 28, |r| {
            Ok(Face {
                plane: r.i32()? as u32,
                side: r.i32()? != 0,
                first_edge: r.i32()? as u32,
                num_edges: r.i32()? as u32,
                texinfo: r.i32()? as u32,
                styles: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
                light_ofs: r.i32()?,
            })
        })?
    } else {
        parse_array(l(LUMP_FACES)?, "faces", 20, |r| {
            Ok(Face {
                plane: r.i16()? as u32,
                side: r.i16()? != 0,
                first_edge: r.i32()? as u32,
                num_edges: r.i16()? as u32,
                texinfo: r.i16()? as u32,
                styles: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
                light_ofs: r.i32()?,
            })
        })?
    };

    let lighting = l(LUMP_LIGHTING)?.to_vec();

    let clipnodes = if bsp2 {
        parse_array(l(LUMP_CLIPNODES)?, "clipnodes", 12, |r| {
            Ok(ClipNode {
                plane: r.i32()? as u32,
                children: [r.i32()?, r.i32()?],
            })
        })?
    } else {
        parse_array(l(LUMP_CLIPNODES)?, "clipnodes", 8, |r| {
            Ok(ClipNode {
                plane: r.i32()? as u32,
                children: [r.i16()? as i32, r.i16()? as i32],
            })
        })?
    };

    let leaves = if bsp2 {
        parse_array(l(LUMP_LEAVES)?, "leaves", 44, |r| {
            Ok(Leaf {
                contents: r.i32()?,
                vis_ofs: r.i32()?,
                mins: r.vec3()?,
                maxs: r.vec3()?,
                first_marksurface: r.u32()?,
                num_marksurfaces: r.u32()?,
                ambient: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
            })
        })?
    } else {
        parse_array(l(LUMP_LEAVES)?, "leaves", 28, |r| {
            Ok(Leaf {
                contents: r.i32()?,
                vis_ofs: r.i32()?,
                mins: r.vec3_i16()?,
                maxs: r.vec3_i16()?,
                first_marksurface: r.u16()? as u32,
                num_marksurfaces: r.u16()? as u32,
                ambient: [r.u8()?, r.u8()?, r.u8()?, r.u8()?],
            })
        })?
    };

    let marksurfaces = if bsp2 {
        parse_array(l(LUMP_MARKSURFACES)?, "marksurfaces", 4, |r| r.u32())?
    } else {
        parse_array(l(LUMP_MARKSURFACES)?, "marksurfaces", 2, |r| {
            Ok(r.u16()? as u32)
        })?
    };
    let edges = if bsp2 {
        parse_array(l(LUMP_EDGES)?, "edges", 8, |r| Ok([r.u32()?, r.u32()?]))?
    } else {
        parse_array(l(LUMP_EDGES)?, "edges", 4, |r| {
            Ok([r.u16()? as u32, r.u16()? as u32])
        })?
    };
    let surfedges = parse_array(l(LUMP_SURFEDGES)?, "surfedges", 4, |r| r.i32())?;
    let models = parse_array(l(LUMP_MODELS)?, "models", 64, |r| {
        Ok(Model {
            mins: r.vec3()?,
            maxs: r.vec3()?,
            origin: r.vec3()?,
            head_nodes: [r.i32()?, r.i32()?, r.i32()?, r.i32()?],
            vis_leafs: r.i32()?,
            first_face: r.i32()? as u32,
            num_faces: r.i32()? as u32,
        })
    })?;

    // Validate every cross-reference once, so the rest of the crate can index without checks.
    if models.is_empty() {
        return Err(BspError::Invalid("no models (world missing)".into()));
    }
    if leaves.is_empty() || nodes.is_empty() {
        return Err(BspError::Invalid("no leaves or nodes".into()));
    }
    for e in &edges {
        check_index(e[0] as i64, vertices.len(), "edge vertex")?;
        check_index(e[1] as i64, vertices.len(), "edge vertex")?;
    }
    for &se in &surfedges {
        check_index(se.unsigned_abs() as i64, edges.len(), "surfedge")?;
    }
    for t in &texinfo {
        check_index(t.miptex as i64, textures.len(), "texinfo miptex")?;
    }
    for f in &faces {
        check_index(f.plane as i64, planes.len(), "face plane")?;
        check_index(f.texinfo as i64, texinfo.len(), "face texinfo")?;
        if f.num_edges < 3 || (f.first_edge as u64 + f.num_edges as u64) > surfedges.len() as u64 {
            return Err(BspError::Invalid("face edge range out of bounds".into()));
        }
        if f.light_ofs >= 0 && f.light_ofs as usize > lighting.len() {
            return Err(BspError::Invalid("face light offset out of bounds".into()));
        }
    }
    for n in &nodes {
        check_index(n.plane as i64, planes.len(), "node plane")?;
        for &c in &n.children {
            if c >= 0 {
                check_index(c as i64, nodes.len(), "node child")?;
            } else {
                check_index((-1 - c) as i64, leaves.len(), "node leaf child")?;
            }
        }
    }
    for c in &clipnodes {
        check_index(c.plane as i64, planes.len(), "clipnode plane")?;
        for &ch in &c.children {
            if ch >= 0 {
                check_index(ch as i64, clipnodes.len(), "clipnode child")?;
            }
        }
    }
    for leaf in &leaves {
        if (leaf.first_marksurface as u64 + leaf.num_marksurfaces as u64)
            > marksurfaces.len() as u64
        {
            return Err(BspError::Invalid(
                "leaf marksurface range out of bounds".into(),
            ));
        }
    }
    for &m in &marksurfaces {
        check_index(m as i64, faces.len(), "marksurface")?;
    }
    for m in &models {
        check_index(m.head_nodes[0] as i64, nodes.len(), "model head node")?;
        for &h in &m.head_nodes[1..] {
            if h >= 0 && !clipnodes.is_empty() {
                check_index(h as i64, clipnodes.len(), "model clip head node")?;
            }
        }
        if (m.first_face as u64 + m.num_faces as u64) > faces.len() as u64 {
            return Err(BspError::Invalid("model face range out of bounds".into()));
        }
    }

    // Hull 0: the node tree with leaf children replaced by their contents.
    let hull0 = nodes
        .iter()
        .map(|n| ClipNode {
            plane: n.plane,
            children: n.children.map(|c| {
                if c >= 0 {
                    c
                } else {
                    leaves[(-1 - c) as usize].contents
                }
            }),
        })
        .collect();

    Ok(Bsp {
        version: header.version,
        entities,
        planes,
        textures,
        vertices,
        visdata,
        nodes,
        texinfo,
        faces,
        lighting,
        lit: None,
        clipnodes,
        leaves,
        marksurfaces,
        edges,
        surfedges,
        models,
        hull0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_rejects_garbage_and_reads_lumps() {
        assert!(matches!(
            parse_header(b"nop").err(),
            Some(BspError::Truncated(_))
        ));
        let mut bytes = vec![0u8; 4 + NUM_LUMPS * 8];
        bytes[0..4].copy_from_slice(&7u32.to_le_bytes());
        assert!(matches!(
            parse_header(&bytes).err(),
            Some(BspError::BadMagic(7))
        ));
        bytes[0..4].copy_from_slice(&29u32.to_le_bytes());
        bytes[4 + LUMP_LIGHTING * 8..8 + LUMP_LIGHTING * 8].copy_from_slice(&100i32.to_le_bytes());
        bytes[8 + LUMP_LIGHTING * 8..12 + LUMP_LIGHTING * 8].copy_from_slice(&42i32.to_le_bytes());
        let h = parse_header(&bytes).unwrap();
        assert_eq!(h.version, Version::Bsp29);
        assert_eq!(h.lumps[LUMP_LIGHTING], Lump { ofs: 100, len: 42 });
        bytes[0..4].copy_from_slice(b"BSP2");
        assert_eq!(parse_header(&bytes).unwrap().version, Version::Bsp2);
    }
}
