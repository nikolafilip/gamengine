//! Reading an upload (MODELS.md 3): a glTF 2.0 binary evaluated in the pose it was exported
//! in, merged into one mesh in model space on the standard rig. Nothing here trusts the file:
//! every accessor is bounds-checked before a byte is read, and the caller runs in a worker
//! process, so a file that still manages to blow up costs one upload.

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use gltf::accessor::{DataType, Dimensions};
use gltf::mesh::{Mode, Semantic};
use gm_model::mannequin::MeshData;
use gm_model::rig::{self, BONES, bone};

pub const MAX_NODES: usize = 4096;
pub const MAX_SOURCE_JOINTS: usize = 128;
pub const MAX_SOURCE_VERTICES: usize = 200_000;
/// Triangle indices over the whole upload. A mesh may be instanced by every node, so what a
/// small file asks for is bounded here, not by its size.
pub const MAX_SOURCE_INDICES: usize = MAX_SOURCE_VERTICES * 3;
/// Problems listed before the reader stops reading geometry.
const MAX_LISTED: usize = 64;
pub const MAX_SOURCE_TEXTURE: u32 = 4096;
/// Decoded image bytes the worker will allocate at most.
const MAX_IMAGE_ALLOC: u64 = 160 * 1024 * 1024;
/// glTF metres to world units (VOCABULARY.md 2).
const UNITS_PER_METRE: f32 = 32.0;

/// What the upload says, before any budget is applied.
pub struct Source {
    /// Model space, one mesh, joints are standard bones, unused vertices dropped.
    pub mesh: MeshData,
    pub texture: image::RgbaImage,
    pub base_color: [f32; 4],
    /// Alpha cutoff when any material is `MASK` or `BLEND`.
    pub cutout: Option<f32>,
    pub two_sided: bool,
    pub source_joints: usize,
    /// Triangles before degenerate ones were dropped.
    pub source_triangles: usize,
    pub notes: Vec<String>,
}

/// How a prop's file is moved into hand space before anything else (CONTENT.md 3.1): in
/// glTF's own conventions, metres and degrees, applied in this order: scaled, turned about
/// X, then Y, then Z, then moved. The grip ends up at the origin, the business end along
/// +Z (glTF's forward), the edge along +Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub mov: [f32; 3],
    pub turn: [f32; 3],
    pub scale: f32,
}

impl Default for Fit {
    fn default() -> Fit {
        Fit {
            mov: [0.0; 3],
            turn: [0.0; 3],
            scale: 1.0,
        }
    }
}

impl Fit {
    pub fn matrix(&self) -> Mat4 {
        let r = Mat4::from_rotation_z(self.turn[2].to_radians())
            * Mat4::from_rotation_y(self.turn[1].to_radians())
            * Mat4::from_rotation_x(self.turn[0].to_radians());
        Mat4::from_translation(Vec3::from(self.mov)) * r * Mat4::from_scale(Vec3::splat(self.scale))
    }
}

/// What an upload is read as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A body on the standard rig (MODELS.md 3).
    Avatar,
    /// A thing held (CONTENT.md 3.1): rigid, fitted, on bone 0; a file without a texture
    /// gets one made of swatches of its colours.
    Prop(Fit),
}

/// Swatch cells of a texture made from flat colours: this many texels a side, in a grid
/// of `SWATCH_COLS` columns.
const SWATCH_SIDE: u32 = 8;
const SWATCH_COLS: u32 = 16;
const MAX_SWATCHES: usize = 256;

/// glTF (+Y up, facing +Z, metres) to model space (+Z up, facing +X, units).
fn to_model(p: Vec3) -> Vec3 {
    Vec3::new(p.z, p.x, p.y) * UNITS_PER_METRE
}

fn to_model_dir(n: Vec3) -> Vec3 {
    Vec3::new(n.z, n.x, n.y)
}

/// A bounds-checked view of one accessor.
struct Acc<'a> {
    data: &'a [u8],
    stride: usize,
    count: usize,
    ty: DataType,
    normalized: bool,
}

fn component_bytes(ty: DataType) -> usize {
    match ty {
        DataType::I8 | DataType::U8 => 1,
        DataType::I16 | DataType::U16 => 2,
        DataType::U32 | DataType::F32 => 4,
    }
}

fn accessor<'a>(
    blob: &'a [u8],
    a: &gltf::Accessor<'_>,
    dims: Dimensions,
    types: &[DataType],
    what: &str,
) -> Result<Acc<'a>, String> {
    if a.sparse().is_some() {
        return Err(format!("{what}: sparse accessors are not supported"));
    }
    if a.dimensions() != dims {
        return Err(format!(
            "{what}: expected {dims:?}, found {:?}",
            a.dimensions()
        ));
    }
    let ty = a.data_type();
    if !types.contains(&ty) {
        return Err(format!("{what}: component type {ty:?} is not allowed"));
    }
    let view = a
        .view()
        .ok_or_else(|| format!("{what}: accessor without a buffer view"))?;
    if !matches!(view.buffer().source(), gltf::buffer::Source::Bin) {
        return Err(format!("{what}: data must live in the .glb's BIN chunk"));
    }
    let element = component_bytes(ty) * dims.multiplicity();
    let stride = view.stride().unwrap_or(element);
    if stride < element {
        return Err(format!("{what}: stride smaller than the element"));
    }
    let count = a.count();
    let start = view
        .offset()
        .checked_add(a.offset())
        .ok_or_else(|| format!("{what}: offset overflow"))?;
    let view_end = view
        .offset()
        .checked_add(view.length())
        .ok_or_else(|| format!("{what}: view overflow"))?;
    let end = if count == 0 {
        start
    } else {
        (count - 1)
            .checked_mul(stride)
            .and_then(|v| v.checked_add(start))
            .and_then(|v| v.checked_add(element))
            .ok_or_else(|| format!("{what}: length overflow"))?
    };
    if end > view_end || view_end > blob.len() {
        return Err(format!("{what}: accessor reaches past its buffer"));
    }
    Ok(Acc {
        data: &blob[start..end],
        stride,
        count,
        ty,
        normalized: a.normalized(),
    })
}

impl Acc<'_> {
    fn raw(&self, i: usize, c: usize) -> f64 {
        let at = i * self.stride + c * component_bytes(self.ty);
        let d = &self.data[at..];
        match self.ty {
            DataType::I8 => d[0] as i8 as f64,
            DataType::U8 => d[0] as f64,
            DataType::I16 => i16::from_le_bytes([d[0], d[1]]) as f64,
            DataType::U16 => u16::from_le_bytes([d[0], d[1]]) as f64,
            DataType::U32 => u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64,
            DataType::F32 => f32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64,
        }
    }

    /// The value as a float, with integer types normalised when the accessor says so.
    fn f32(&self, i: usize, c: usize) -> f32 {
        let v = self.raw(i, c);
        let v = if self.normalized {
            match self.ty {
                DataType::I8 => (v / 127.0).max(-1.0),
                DataType::U8 => v / 255.0,
                DataType::I16 => (v / 32767.0).max(-1.0),
                DataType::U16 => v / 65535.0,
                _ => v,
            }
        } else {
            v
        };
        v as f32
    }

    fn u32(&self, i: usize, c: usize) -> u32 {
        self.raw(i, c) as u32
    }

    fn vec3(&self, i: usize) -> Vec3 {
        Vec3::new(self.f32(i, 0), self.f32(i, 1), self.f32(i, 2))
    }
}

struct Graph {
    parent: Vec<Option<usize>>,
    world: Vec<Mat4>,
    /// Nodes that carry a mesh, in traversal order.
    meshes: Vec<usize>,
}

fn graph(doc: &gltf::Document) -> Result<Graph, String> {
    let n = doc.nodes().count();
    if n > MAX_NODES {
        return Err(format!("{n} nodes; the limit is {MAX_NODES}"));
    }
    let mut g = Graph {
        parent: vec![None; n],
        world: vec![Mat4::IDENTITY; n],
        meshes: Vec::new(),
    };
    let mut seen = vec![false; n];
    let mut stack: Vec<(gltf::Node<'_>, Option<usize>)> = Vec::new();
    let scenes: Vec<gltf::Scene<'_>> = match doc.default_scene() {
        Some(s) => vec![s],
        None => doc.scenes().collect(),
    };
    for scene in scenes {
        for node in scene.nodes() {
            stack.push((node, None));
        }
    }
    while let Some((node, parent)) = stack.pop() {
        let i = node.index();
        if std::mem::replace(&mut seen[i], true) {
            return Err("a node is reachable twice (a cycle or an instanced subtree)".into());
        }
        let local = Mat4::from_cols_array_2d(&node.transform().matrix());
        if !local.is_finite() {
            return Err("a node transform is not finite".into());
        }
        g.parent[i] = parent;
        g.world[i] = parent.map_or(local, |p| g.world[p] * local);
        if node.mesh().is_some() {
            g.meshes.push(i);
        }
        for child in node.children() {
            stack.push((child, Some(i)));
        }
    }
    Ok(g)
}

/// Read and merge the upload as an avatar. `Err` lists every reason it cannot be used.
pub fn read(upload: &[u8]) -> Result<Source, Vec<String>> {
    read_kind(upload, Kind::Avatar)
}

/// Read a prop: rigid geometry under `fit`, on bone 0, no rig (CONTENT.md 3.1).
pub fn read_prop(upload: &[u8], fit: Fit) -> Result<Source, Vec<String>> {
    read_kind(upload, Kind::Prop(fit))
}

/// Read and merge the upload. `Err` lists every reason it cannot be used.
pub fn read_kind(upload: &[u8], kind: Kind) -> Result<Source, Vec<String>> {
    if !upload.starts_with(b"glTF") {
        return Err(vec!["the upload must be a glTF 2.0 binary (.glb)".into()]);
    }
    let gltf = gltf::Gltf::from_slice(upload)
        .map_err(|e| vec![format!("not a valid glTF 2.0 binary: {e}")])?;
    let blob: &[u8] = gltf.blob.as_deref().unwrap_or(&[]);
    let doc = &gltf.document;
    let fatal = |s: String| vec![s];
    let g = graph(doc).map_err(fatal)?;

    // A prop has no skin to speak of: whatever the file carries is read as rigid geometry
    // in the pose it was exported in, and the fit moves all of it into hand space.
    let prop_fit = match kind {
        Kind::Avatar => None,
        Kind::Prop(fit) => Some(fit.matrix()),
    };
    let mut skins = doc.skins();
    let skin = match (skins.next(), skins.next(), prop_fit.is_some()) {
        (_, _, true) => None,
        (Some(skin), None, false) => Some(skin),
        _ => {
            return Err(vec![format!(
                "{} skins; a model has exactly one, on the standard rig",
                doc.skins().count()
            )]);
        }
    };
    let joint_nodes: Vec<gltf::Node<'_>> = skin.iter().flat_map(|s| s.joints()).collect();
    if joint_nodes.len() > MAX_SOURCE_JOINTS {
        return Err(vec![format!(
            "{} joints in the skin; the limit is {MAX_SOURCE_JOINTS}",
            joint_nodes.len()
        )]);
    }
    let mut violations: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // Standard bones among the joints, by name.
    let mut bone_of_node: HashMap<usize, usize> = HashMap::new();
    let mut node_of_bone: [Option<usize>; BONES] = [None; BONES];
    for j in &joint_nodes {
        if let Some(b) = j.name().and_then(rig::bone_by_name) {
            if node_of_bone[b].is_some() {
                violations.push(format!("two joints are named `{}`", rig::NAMES[b]));
            }
            node_of_bone[b] = Some(j.index());
            bone_of_node.insert(j.index(), b);
        }
    }
    let missing: Vec<&str> = (0..BONES)
        .filter(|b| rig::REQUIRED & (1 << b) != 0 && node_of_bone[*b].is_none())
        .map(|b| rig::NAMES[b])
        .collect();
    if !missing.is_empty() && prop_fit.is_none() {
        violations.push(format!(
            "required bones are missing: {} (joints are matched by name; `gm-tools model template` writes a rig to start from)",
            missing.join(", ")
        ));
        return Err(violations);
    }
    // The nearest standard bone at or above a node; the hips when there is none.
    let fold = |mut node: usize, include_self: bool| -> Option<usize> {
        if include_self && let Some(b) = bone_of_node.get(&node) {
            return Some(*b);
        }
        let mut steps = 0;
        while let Some(p) = g.parent[node] {
            if let Some(b) = bone_of_node.get(&p) {
                return Some(*b);
            }
            node = p;
            steps += 1;
            if steps > MAX_NODES {
                break;
            }
        }
        None
    };
    let folded = joint_nodes
        .iter()
        .filter(|j| !bone_of_node.contains_key(&j.index()))
        .count();
    if folded > 0 {
        notes.push(format!(
            "{folded} joints are not standard bones; their weights went to their nearest standard ancestor"
        ));
    }
    // The file's hierarchy must be the rig's.
    let mut mask = 0u32;
    for b in 0..BONES {
        let Some(node) = node_of_bone[b] else {
            continue;
        };
        mask |= 1 << b;
        let mut expected = rig::parent(b);
        while let Some(e) = expected
            && node_of_bone[e].is_none()
        {
            expected = rig::parent(e);
        }
        let actual = fold(node, false);
        if actual != expected {
            let name = |x: Option<usize>| x.map_or("nothing", |i| rig::NAMES[i]);
            violations.push(format!(
                "`{}` must descend from `{}`; it sits under `{}`",
                rig::NAMES[b],
                name(expected),
                name(actual)
            ));
        }
    }

    // Skin matrices in the exported pose: joint world × inverse bind.
    let ibms: Vec<Mat4> = match skin.as_ref().and_then(|s| s.inverse_bind_matrices()) {
        Some(a) => {
            let acc = accessor(
                blob,
                &a,
                Dimensions::Mat4,
                &[DataType::F32],
                "inverse bind matrices",
            )
            .map_err(fatal)?;
            if acc.count != joint_nodes.len() {
                return Err(vec![
                    "the inverse bind matrices do not match the joints".into(),
                ]);
            }
            (0..acc.count)
                .map(|i| {
                    let mut m = [0f32; 16];
                    for (c, v) in m.iter_mut().enumerate() {
                        *v = acc.f32(i, c);
                    }
                    Mat4::from_cols_array(&m)
                })
                .collect()
        }
        None => vec![Mat4::IDENTITY; joint_nodes.len()],
    };
    let skin_mats: Vec<Mat4> = joint_nodes
        .iter()
        .zip(&ibms)
        .map(|(j, ibm)| g.world[j.index()] * *ibm)
        .collect();
    if skin_mats.iter().any(|m| !m.is_finite()) {
        return Err(vec!["the skin has non-finite matrices".into()]);
    }
    let joint_bone: Vec<usize> = joint_nodes
        .iter()
        .map(|j| fold(j.index(), true).unwrap_or(bone::HIPS))
        .collect();

    // Geometry.
    let mut mesh = MeshData::default();
    let mut images: Vec<usize> = Vec::new();
    let mut untextured = 0usize;
    let mut cutout: Option<f32> = None;
    let mut two_sided = false;
    let mut base_color: Option<[f32; 4]> = None;
    let mut source_triangles = 0usize;
    let nodes: Vec<gltf::Node<'_>> = doc.nodes().collect();
    // A prop without a texture: every flat colour (the material's factor times the vertex
    // colour, when the file has them) becomes a swatch, and the vertex points at it.
    let mut swatches: Vec<[u8; 3]> = Vec::new();
    let mut swatch_uvs: Vec<usize> = Vec::new();
    let mut textured_prims = 0usize;
    // Over the totals: nothing more is read (every further instance would cost the same again).
    let mut over = false;
    'nodes: for &ni in &g.meshes {
        let node = &nodes[ni];
        let Some(m) = node.mesh() else { continue };
        let skinned = node.skin().is_some() && prop_fit.is_none();
        let rigid_bone = fold(ni, true).unwrap_or(bone::HIPS);
        for prim in m.primitives() {
            if over || violations.len() >= MAX_LISTED {
                violations.push("the rest of the geometry was not read".into());
                break 'nodes;
            }
            let what = format!("mesh {} primitive {}", m.index(), prim.index());
            if !matches!(
                prim.mode(),
                Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan
            ) {
                violations.push(format!("{what}: only triangles are drawn"));
                continue;
            }
            let material = prim.material();
            let pbr = material.pbr_metallic_roughness();
            let uv_set = match pbr.base_color_texture() {
                Some(info) => {
                    let image = info.texture().source().index();
                    if !images.contains(&image) {
                        images.push(image);
                    }
                    textured_prims += 1;
                    info.tex_coord()
                }
                None => {
                    untextured += 1;
                    0
                }
            };
            let swatched = prop_fit.is_some() && pbr.base_color_texture().is_none();
            match material.alpha_mode() {
                gltf::material::AlphaMode::Opaque => {}
                gltf::material::AlphaMode::Mask => {
                    cutout.get_or_insert(material.alpha_cutoff().unwrap_or(0.5));
                }
                gltf::material::AlphaMode::Blend => {
                    cutout.get_or_insert(0.5);
                }
            }
            two_sided |= material.double_sided();
            let factor = pbr.base_color_factor();
            const DISAGREE: &str =
                "materials disagree on the base colour factor; the first one is used";
            if *base_color.get_or_insert(factor) != factor && !notes.iter().any(|n| n == DISAGREE) {
                notes.push(DISAGREE.into());
            }

            let read = (|| -> Result<(), String> {
                let pos = prim
                    .get(&Semantic::Positions)
                    .ok_or_else(|| format!("{what}: no POSITION"))?;
                let pos = accessor(blob, &pos, Dimensions::Vec3, &[DataType::F32], &what)?;
                let nrm = match prim.get(&Semantic::Normals) {
                    Some(a) => Some(accessor(
                        blob,
                        &a,
                        Dimensions::Vec3,
                        &[DataType::F32],
                        &what,
                    )?),
                    None => None,
                };
                // A swatched primitive needs no texture coordinates: it gets them.
                let uv = match prim.get(&Semantic::TexCoords(uv_set)) {
                    Some(a) => Some(accessor(
                        blob,
                        &a,
                        Dimensions::Vec2,
                        &[DataType::F32, DataType::U8, DataType::U16],
                        &what,
                    )?),
                    None if swatched => None,
                    None => return Err(format!("{what}: no TEXCOORD_{uv_set}")),
                };
                let colours = match (swatched, prim.get(&Semantic::Colors(0))) {
                    (true, Some(a)) => {
                        let dims = a.dimensions();
                        Some(accessor(
                            blob,
                            &a,
                            dims,
                            &[DataType::F32, DataType::U8, DataType::U16],
                            &what,
                        )?)
                    }
                    _ => None,
                };
                let skin_attrs = if skinned {
                    let j = prim
                        .get(&Semantic::Joints(0))
                        .ok_or_else(|| format!("{what}: no JOINTS_0"))?;
                    let w = prim
                        .get(&Semantic::Weights(0))
                        .ok_or_else(|| format!("{what}: no WEIGHTS_0"))?;
                    Some((
                        accessor(
                            blob,
                            &j,
                            Dimensions::Vec4,
                            &[DataType::U8, DataType::U16],
                            &what,
                        )?,
                        accessor(
                            blob,
                            &w,
                            Dimensions::Vec4,
                            &[DataType::F32, DataType::U8, DataType::U16],
                            &what,
                        )?,
                    ))
                } else {
                    None
                };
                let n = pos.count;
                if nrm.as_ref().is_some_and(|a| a.count != n)
                    || uv.as_ref().is_some_and(|a| a.count != n)
                    || colours.as_ref().is_some_and(|a| a.count != n)
                    || skin_attrs
                        .as_ref()
                        .is_some_and(|(j, w)| j.count != n || w.count != n)
                {
                    return Err(format!("{what}: attributes differ in length"));
                }
                if mesh.positions.len() + n > MAX_SOURCE_VERTICES {
                    over = true;
                    return Err(format!(
                        "more than {MAX_SOURCE_VERTICES} vertices in the upload"
                    ));
                }
                let base = mesh.positions.len() as u32;
                let rigid = match prop_fit {
                    Some(fit) => fit * g.world[ni],
                    None => g.world[ni],
                };
                for i in 0..n {
                    let p = pos.vec3(i);
                    let nn = nrm.as_ref().map_or(Vec3::ZERO, |a| a.vec3(i));
                    let (m, joints, weights) = match &skin_attrs {
                        Some((ja, wa)) => {
                            let mut m = Mat4::ZERO;
                            let mut total = 0.0;
                            let mut bones: Vec<(usize, f32)> = Vec::with_capacity(4);
                            for c in 0..4 {
                                let w = wa.f32(i, c);
                                // Skips zero, negative and NaN weights alike.
                                if w.is_nan() || w <= 0.0 {
                                    continue;
                                }
                                let k = ja.u32(i, c) as usize;
                                if k >= skin_mats.len() {
                                    return Err(format!("{what}: a joint index is past the skin"));
                                }
                                m += skin_mats[k] * w;
                                total += w;
                                match bones.iter_mut().find(|(b, _)| *b == joint_bone[k]) {
                                    Some(e) => e.1 += w,
                                    None => bones.push((joint_bone[k], w)),
                                }
                            }
                            if total <= 0.0 {
                                // An unweighted vertex follows the hips.
                                (rigid, [bone::HIPS as u8, 0, 0, 0], [1.0, 0.0, 0.0, 0.0])
                            } else {
                                let mut j = [0u8; 4];
                                let mut w = [0f32; 4];
                                for (k, (b, bw)) in bones.iter().enumerate() {
                                    j[k] = *b as u8;
                                    w[k] = bw / total;
                                }
                                (m * (1.0 / total), j, w)
                            }
                        }
                        None => (rigid, [rigid_bone as u8, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
                    };
                    let world = m.transform_point3(p);
                    if !world.is_finite() {
                        return Err(format!("{what}: a vertex is not finite"));
                    }
                    mesh.positions.push(to_model(world));
                    mesh.normals
                        .push(to_model_dir(m.transform_vector3(nn)).normalize_or_zero());
                    if swatched {
                        // The vertex's flat colour, linear as glTF gives it, quantised to a
                        // swatch (sRGB when the texture is made, below).
                        let mut c = [factor[0], factor[1], factor[2]];
                        if let Some(ca) = &colours {
                            for (k, v) in c.iter_mut().enumerate() {
                                *v *= ca.f32(i, k);
                            }
                        }
                        let q = c.map(|v| (linear_to_srgb(v) * 255.0 + 0.5) as u8);
                        let at = match swatches.iter().position(|s| *s == q) {
                            Some(at) => at,
                            None if swatches.len() < MAX_SWATCHES => {
                                swatches.push(q);
                                swatches.len() - 1
                            }
                            None => {
                                return Err(format!(
                                    "{what}: more than {MAX_SWATCHES} flat colours"
                                ));
                            }
                        };
                        swatch_uvs.push(at);
                        mesh.uvs.push([0.0, 0.0]);
                    } else {
                        let uv = uv.as_ref().expect("a textured primitive has coordinates");
                        mesh.uvs.push([uv.f32(i, 0), uv.f32(i, 1)]);
                    }
                    mesh.joints.push(joints);
                    mesh.weights.push(weights);
                }
                // Triangles, whatever the topology.
                let index: Vec<u32> = match prim.indices() {
                    Some(a) => {
                        let acc = accessor(
                            blob,
                            &a,
                            Dimensions::Scalar,
                            &[DataType::U8, DataType::U16, DataType::U32],
                            &what,
                        )?;
                        if acc.count > MAX_SOURCE_VERTICES * 3 {
                            return Err(format!("{what}: too many indices"));
                        }
                        let v: Vec<u32> = (0..acc.count).map(|i| acc.u32(i, 0)).collect();
                        if v.iter().any(|i| *i as usize >= n) {
                            return Err(format!("{what}: an index is past the vertices"));
                        }
                        v
                    }
                    None => (0..n as u32).collect(),
                };
                // A strip or a fan makes up to three indices of one.
                if mesh.indices.len() + index.len() * 3 > MAX_SOURCE_INDICES {
                    over = true;
                    return Err(format!(
                        "more than {} triangles in the upload",
                        MAX_SOURCE_INDICES / 3
                    ));
                }
                let mut tri = |a: u32, b: u32, c: u32| {
                    mesh.indices
                        .extend_from_slice(&[base + a, base + b, base + c]);
                };
                match prim.mode() {
                    Mode::Triangles => {
                        for t in index.chunks_exact(3) {
                            tri(t[0], t[1], t[2]);
                        }
                    }
                    Mode::TriangleStrip => {
                        for (k, t) in index.windows(3).enumerate() {
                            if k % 2 == 0 {
                                tri(t[0], t[1], t[2]);
                            } else {
                                tri(t[1], t[0], t[2]);
                            }
                        }
                    }
                    _ => {
                        for t in index.windows(2).skip(1) {
                            tri(index[0], t[0], t[1]);
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = read {
                violations.push(e);
            }
        }
    }
    if mesh.indices.is_empty() && violations.is_empty() {
        violations.push("the upload has no triangles".into());
    }
    let swatch_texture = match (prop_fit.is_some(), untextured, textured_prims) {
        // An avatar: every surface comes from the one atlas.
        (false, u, _) if u > 0 => {
            violations.push(format!(
                "{untextured} primitives have no base colour texture; every surface comes from the one atlas"
            ));
            false
        }
        // A prop of flat colours: its texture is made here.
        (true, u, 0) if u > 0 => true,
        // A prop of both: the file must choose.
        (true, u, t) if u > 0 && t > 0 => {
            violations.push(format!(
                "{u} primitives have no base colour texture and {t} have one; a prop is textured or flat, not both"
            ));
            false
        }
        _ => false,
    };
    if images.len() > 1 {
        violations.push(format!(
            "{} base colour textures; the budget is one atlas",
            images.len()
        ));
    }
    if !violations.is_empty() {
        return Err(violations);
    }

    // The atlas.
    let texture = if swatch_texture {
        // Swatches in a grid, and every swatched vertex pointed at the middle of its cell.
        let rows = (swatches.len() as u32).div_ceil(SWATCH_COLS).max(1);
        let (w, h) = (SWATCH_COLS * SWATCH_SIDE, rows * SWATCH_SIDE);
        let mut img = image::RgbaImage::new(w, h);
        for (i, c) in swatches.iter().enumerate() {
            let (cx, cy) = (
                (i as u32 % SWATCH_COLS) * SWATCH_SIDE,
                (i as u32 / SWATCH_COLS) * SWATCH_SIDE,
            );
            for y in cy..cy + SWATCH_SIDE {
                for x in cx..cx + SWATCH_SIDE {
                    img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
        }
        for (uv, at) in mesh.uvs.iter_mut().zip(&swatch_uvs) {
            let (cx, cy) = (*at as u32 % SWATCH_COLS, *at as u32 / SWATCH_COLS);
            *uv = [
                (cx as f32 + 0.5) * SWATCH_SIDE as f32 / w as f32,
                (cy as f32 + 0.5) * SWATCH_SIDE as f32 / h as f32,
            ];
        }
        // The factor is in the swatches already.
        base_color = Some([1.0; 4]);
        notes.push(format!(
            "no texture: {} flat colours were made into swatches",
            swatches.len()
        ));
        img
    } else {
        let image = images
            .first()
            .and_then(|i| doc.images().nth(*i))
            .ok_or_else(|| vec!["the base colour texture names no image".to_string()])?;
        let bytes = match image.source() {
            gltf::image::Source::View { view, .. } => {
                let end = view.offset().checked_add(view.length());
                match end {
                    Some(end)
                        if end <= blob.len()
                            && matches!(view.buffer().source(), gltf::buffer::Source::Bin) =>
                    {
                        &blob[view.offset()..end]
                    }
                    _ => return Err(vec!["the texture reaches past the .glb's BIN chunk".into()]),
                }
            }
            gltf::image::Source::Uri { .. } => {
                return Err(vec![
                    "the texture must be embedded in the .glb, not referenced by URI".into(),
                ]);
            }
        };
        decode_image(bytes).map_err(fatal)?
    };

    // Pivots in model space; an absent bone holds its parent's. A prop has none: bone 0
    // at the origin, where the hand closes.
    let mut pivots = [Vec3::ZERO; BONES];
    if prop_fit.is_none() {
        for b in 0..BONES {
            pivots[b] = match node_of_bone[b] {
                Some(n) => to_model(g.world[n].w_axis.truncate()),
                None => rig::parent(b).map_or(Vec3::ZERO, |p| pivots[p]),
            };
        }
    }
    let mask = if prop_fit.is_some() { 1 } else { mask };
    mesh.pivots = pivots;
    mesh.bone_mask = mask;

    // Drop degenerate triangles and the vertices nothing uses.
    source_triangles += mesh.indices.len() / 3;
    let mut kept: Vec<u32> = Vec::with_capacity(mesh.indices.len());
    for t in mesh.indices.chunks_exact(3) {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let area = (mesh.positions[b] - mesh.positions[a])
            .cross(mesh.positions[c] - mesh.positions[a])
            .length();
        if a != b && b != c && a != c && area > 1e-9 {
            kept.extend_from_slice(t);
        }
    }
    // Vertices keep the order they were uploaded in.
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    for i in &kept {
        remap[*i as usize] = 0;
    }
    let mut out = MeshData {
        pivots,
        bone_mask: mask,
        ..Default::default()
    };
    for (old, slot) in remap.iter_mut().enumerate() {
        if *slot == u32::MAX {
            continue;
        }
        *slot = out.positions.len() as u32;
        out.positions.push(mesh.positions[old]);
        out.normals.push(mesh.normals[old]);
        out.uvs.push(mesh.uvs[old]);
        out.joints.push(mesh.joints[old]);
        out.weights.push(mesh.weights[old]);
    }
    for i in &mut kept {
        *i = remap[*i as usize];
    }
    out.indices = kept;
    if out.normals.contains(&Vec3::ZERO) {
        compute_normals(&mut out);
        notes.push("normals were missing and have been computed".into());
    }

    Ok(Source {
        mesh: out,
        texture,
        base_color: base_color.unwrap_or([1.0; 4]),
        cutout,
        two_sided,
        source_joints: joint_nodes.len(),
        source_triangles,
        notes,
    })
}

/// Linear light to the sRGB curve, 0..1 (a swatch is stored as the texture is).
fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * gm_model::det::pow(c, 1.0 / 2.4) - 0.055
    }
}

/// Area-weighted smooth normals for vertices that came without one.
fn compute_normals(mesh: &mut MeshData) {
    let mut sum = vec![Vec3::ZERO; mesh.positions.len()];
    for t in mesh.indices.chunks_exact(3) {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let n =
            (mesh.positions[b] - mesh.positions[a]).cross(mesh.positions[c] - mesh.positions[a]);
        sum[a] += n;
        sum[b] += n;
        sum[c] += n;
    }
    for (n, s) in mesh.normals.iter_mut().zip(sum) {
        if *n == Vec3::ZERO {
            *n = s.normalize_or(Vec3::Z);
        }
    }
}

/// Decode a PNG or JPEG with hard limits on its size and on what decoding may allocate.
pub fn decode_image(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("the texture cannot be read: {e}"))?;
    match reader.format() {
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
        _ => return Err("the texture must be a PNG or a JPEG".into()),
    }
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| format!("the texture header is invalid: {e}"))?;
    if w == 0 || h == 0 || w > MAX_SOURCE_TEXTURE || h > MAX_SOURCE_TEXTURE {
        return Err(format!(
            "the texture is {w} × {h}; the limit for an upload is {MAX_SOURCE_TEXTURE} × {MAX_SOURCE_TEXTURE}"
        ));
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("the texture cannot be read: {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_TEXTURE);
    limits.max_image_height = Some(MAX_SOURCE_TEXTURE);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| format!("the texture does not decode: {e}"))?;
    Ok(img.to_rgba8())
}
