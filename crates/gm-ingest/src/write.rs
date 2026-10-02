//! Writing a `.glb` on the standard rig: the template a creator starts from
//! (`gm-tools model template`), and the generated avatars the tests and the acceptance run
//! upload. The fields are public so a test can break exactly one thing before `build`.

use glam::Vec3;
use gm_model::mannequin::MeshData;
use gm_model::rig::{self, BONES};
use serde_json::{Value, json};

pub struct GlbBuilder {
    /// Model space. A joint index of `BONES + k` names `extra_joints[k]`.
    pub mesh: MeshData,
    /// The encoded base colour image.
    pub image: Vec<u8>,
    pub mime: &'static str,
    pub alpha_mode: &'static str,
    pub double_sided: bool,
    /// Node name per standard bone.
    pub bone_names: Vec<String>,
    /// Standard bones written as joints.
    pub include: u32,
    /// Joints that are not standard bones: `(name, parent bone, position in model space)`.
    pub extra_joints: Vec<(String, usize, Vec3)>,
    /// Emit `NORMAL`.
    pub normals: bool,
}

fn pad4(v: &mut Vec<u8>, fill: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(fill);
    }
}

/// Model space (+X forward, +Z up, units) to glTF (+Z forward, +Y up, metres).
fn to_gltf(p: Vec3) -> [f32; 3] {
    [p.y / 32.0, p.z / 32.0, p.x / 32.0]
}

fn to_gltf_dir(n: Vec3) -> [f32; 3] {
    [n.y, n.z, n.x]
}

impl GlbBuilder {
    pub fn new(mesh: MeshData, image: Vec<u8>) -> GlbBuilder {
        let include = mesh.bone_mask;
        GlbBuilder {
            mesh,
            image,
            mime: "image/png",
            alpha_mode: "OPAQUE",
            double_sided: false,
            bone_names: rig::NAMES.iter().map(|s| s.to_string()).collect(),
            include,
            extra_joints: Vec::new(),
            normals: true,
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let m = &self.mesh;
        let mut bin: Vec<u8> = Vec::new();
        let mut views: Vec<Value> = Vec::new();
        let mut accessors: Vec<Value> = Vec::new();
        let mut view = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| -> usize {
            pad4(bin, 0);
            let mut v = json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len() });
            if let Some(t) = target {
                v["target"] = json!(t);
            }
            bin.extend_from_slice(bytes);
            views.push(v);
            views.len() - 1
        };

        // Skin joints: the included standard bones, then the extras.
        let mut joint_of_bone = [usize::MAX; BONES];
        let mut joints: Vec<(String, Option<usize>, Vec3)> = Vec::new();
        for b in 0..BONES {
            if self.include & (1 << b) == 0 {
                continue;
            }
            // The nearest included ancestor.
            let mut parent = rig::parent(b);
            while let Some(p) = parent
                && self.include & (1 << p) == 0
            {
                parent = rig::parent(p);
            }
            joint_of_bone[b] = joints.len();
            joints.push((
                self.bone_names[b].clone(),
                parent.map(|p| joint_of_bone[p]),
                m.pivots[b],
            ));
        }
        let first_extra = joints.len();
        for (name, parent, at) in &self.extra_joints {
            joints.push((name.clone(), Some(joint_of_bone[*parent]), *at));
        }
        let skin_joint = |j: u8| -> u8 {
            let j = j as usize;
            if j >= BONES {
                (first_extra + j - BONES) as u8
            } else if joint_of_bone[j] != usize::MAX {
                joint_of_bone[j] as u8
            } else {
                0
            }
        };

        // Vertex data.
        let n = m.positions.len();
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        let mut pos = Vec::with_capacity(n * 12);
        for p in &m.positions {
            let g = to_gltf(*p);
            for k in 0..3 {
                mn[k] = mn[k].min(g[k]);
                mx[k] = mx[k].max(g[k]);
                pos.extend_from_slice(&g[k].to_le_bytes());
            }
        }
        let v = view(&mut bin, &pos, Some(34962));
        accessors.push(json!({ "bufferView": v, "componentType": 5126, "count": n, "type": "VEC3", "min": mn, "max": mx }));
        let mut attributes = json!({ "POSITION": 0 });
        if self.normals {
            let mut data = Vec::with_capacity(n * 12);
            for nn in &m.normals {
                for c in to_gltf_dir(*nn) {
                    data.extend_from_slice(&c.to_le_bytes());
                }
            }
            let v = view(&mut bin, &data, Some(34962));
            attributes["NORMAL"] = json!(accessors.len());
            accessors.push(
                json!({ "bufferView": v, "componentType": 5126, "count": n, "type": "VEC3" }),
            );
        }
        let mut data = Vec::with_capacity(n * 8);
        for uv in &m.uvs {
            data.extend_from_slice(&uv[0].to_le_bytes());
            data.extend_from_slice(&uv[1].to_le_bytes());
        }
        let v = view(&mut bin, &data, Some(34962));
        attributes["TEXCOORD_0"] = json!(accessors.len());
        accessors
            .push(json!({ "bufferView": v, "componentType": 5126, "count": n, "type": "VEC2" }));
        let data: Vec<u8> = m
            .joints
            .iter()
            .zip(&m.weights)
            .flat_map(|(j, w)| {
                // A zero-weight influence names joint 0, as the specification asks.
                let mut out = [0u8; 4];
                for k in 0..4 {
                    if w[k] > 0.0 {
                        out[k] = skin_joint(j[k]);
                    }
                }
                out
            })
            .collect();
        let v = view(&mut bin, &data, Some(34962));
        attributes["JOINTS_0"] = json!(accessors.len());
        accessors
            .push(json!({ "bufferView": v, "componentType": 5121, "count": n, "type": "VEC4" }));
        let mut data = Vec::with_capacity(n * 16);
        for w in &m.weights {
            for c in w {
                data.extend_from_slice(&c.to_le_bytes());
            }
        }
        let v = view(&mut bin, &data, Some(34962));
        attributes["WEIGHTS_0"] = json!(accessors.len());
        accessors
            .push(json!({ "bufferView": v, "componentType": 5126, "count": n, "type": "VEC4" }));
        let indices = accessors.len();
        if n <= u16::MAX as usize {
            let data: Vec<u8> = m
                .indices
                .iter()
                .flat_map(|i| (*i as u16).to_le_bytes())
                .collect();
            let v = view(&mut bin, &data, Some(34963));
            accessors.push(json!({ "bufferView": v, "componentType": 5123, "count": m.indices.len(), "type": "SCALAR" }));
        } else {
            let data: Vec<u8> = m.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let v = view(&mut bin, &data, Some(34963));
            accessors.push(json!({ "bufferView": v, "componentType": 5125, "count": m.indices.len(), "type": "SCALAR" }));
        }
        // Inverse bind matrices: joints sit at their pivots, unrotated.
        let mut data = Vec::with_capacity(joints.len() * 64);
        for (_, _, at) in &joints {
            let g = to_gltf(*at);
            let ibm: [f32; 16] = [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -g[0], -g[1], -g[2],
                1.0,
            ];
            for c in ibm {
                data.extend_from_slice(&c.to_le_bytes());
            }
        }
        let v = view(&mut bin, &data, None);
        let ibm = accessors.len();
        accessors.push(json!({ "bufferView": v, "componentType": 5126, "count": joints.len(), "type": "MAT4" }));
        let image_view = view(&mut bin, &self.image, None);
        pad4(&mut bin, 0);

        // Nodes: the mesh, then one per joint.
        let mut nodes: Vec<Value> = vec![json!({ "name": "avatar", "mesh": 0, "skin": 0 })];
        let mut roots: Vec<usize> = vec![0];
        for (i, (name, parent, at)) in joints.iter().enumerate() {
            let local = match parent {
                Some(p) => *at - joints[*p].2,
                None => *at,
            };
            nodes.push(json!({ "name": name, "translation": to_gltf(local) }));
            if parent.is_none() {
                roots.push(i + 1);
            }
        }
        for (i, (_, parent, _)) in joints.iter().enumerate() {
            if let Some(p) = parent {
                let node = &mut nodes[p + 1];
                match node.get_mut("children").and_then(|c| c.as_array_mut()) {
                    Some(children) => children.push(json!(i + 1)),
                    None => node["children"] = json!([i + 1]),
                }
            }
        }
        let mut material = json!({
            "name": "atlas",
            "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 }, "metallicFactor": 0.0 },
            "doubleSided": self.double_sided,
        });
        if self.alpha_mode != "OPAQUE" {
            material["alphaMode"] = json!(self.alpha_mode);
        }
        let doc = json!({
            "asset": { "version": "2.0", "generator": "gamengine gm-ingest" },
            "buffers": [{ "byteLength": bin.len() }],
            "bufferViews": views,
            "accessors": accessors,
            "images": [{ "bufferView": image_view, "mimeType": self.mime }],
            "samplers": [{}],
            "textures": [{ "source": 0, "sampler": 0 }],
            "materials": [material],
            "meshes": [{ "primitives": [{ "attributes": attributes, "indices": indices, "material": 0, "mode": 4 }] }],
            "skins": [{ "joints": (1..=joints.len()).collect::<Vec<usize>>(), "inverseBindMatrices": ibm }],
            "nodes": nodes,
            "scenes": [{ "nodes": roots }],
            "scene": 0
        });
        container(&doc, &bin)
    }
}

/// Wrap a JSON document and a BIN chunk as a `.glb`.
pub fn container(doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(doc).expect("a JSON value serialises");
    pad4(&mut json_bytes, b' ');
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out
}

/// Encode RGBA as PNG.
pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    image::write_buffer_with_format(
        &mut std::io::Cursor::new(&mut out),
        rgba,
        w,
        h,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .expect("PNG encoding to memory does not fail");
    out
}
