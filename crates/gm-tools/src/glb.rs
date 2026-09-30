//! Synthetic glTF binary models for the budget self-test and unit tests.

use std::io::Cursor;

use anyhow::Result;
use serde_json::json;

pub struct Spec {
    pub triangles: u32,
    pub bones: u32,
    /// `(width, height)` of each embedded PNG texture.
    pub textures: Vec<(u32, u32)>,
}

fn pad4(v: &mut Vec<u8>, fill: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(fill);
    }
}

fn png(w: u32, h: u32) -> Result<Vec<u8>> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 120, 80, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// A valid .glb: one mesh of `triangles` disjoint triangles, an optional skin with `bones`
/// joints, and one embedded PNG per texture.
pub fn synthetic(spec: &Spec) -> Result<Vec<u8>> {
    let mut bin: Vec<u8> = Vec::new();
    let vcount = spec.triangles * 3;
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for i in 0..spec.triangles {
        let a = i as f32 * 0.01;
        for p in [[a, 0.0, 0.0], [a + 0.005, 0.01, 0.0], [a, 0.01, 0.005]] {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
            for v in p {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    if spec.triangles == 0 {
        mn = [0.0; 3];
        mx = [0.0; 3];
    }
    let pos_len = bin.len();
    let idx_ofs = bin.len();
    for i in 0..vcount {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let idx_len = bin.len() - idx_ofs;

    let mut buffer_views = vec![
        json!({ "buffer": 0, "byteOffset": 0, "byteLength": pos_len, "target": 34962 }),
        json!({ "buffer": 0, "byteOffset": idx_ofs, "byteLength": idx_len, "target": 34963 }),
    ];
    let mut images = Vec::new();
    for (w, h) in &spec.textures {
        pad4(&mut bin, 0);
        let data = png(*w, *h)?;
        let ofs = bin.len();
        bin.extend_from_slice(&data);
        images.push(json!({ "bufferView": buffer_views.len(), "mimeType": "image/png" }));
        buffer_views.push(json!({ "buffer": 0, "byteOffset": ofs, "byteLength": data.len() }));
    }
    pad4(&mut bin, 0);

    let mut nodes = vec![json!({ "mesh": 0 })];
    let mut skins = Vec::new();
    if spec.bones > 0 {
        let joints: Vec<usize> = (1..=spec.bones as usize).collect();
        for _ in 0..spec.bones {
            nodes.push(json!({ "translation": [0.0, 0.1, 0.0] }));
        }
        skins.push(json!({ "joints": joints }));
        nodes[0] = json!({ "mesh": 0, "skin": 0 });
    }

    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "gm-tools synthetic" },
        "buffers": [{ "byteLength": bin.len() }],
        "bufferViews": buffer_views,
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": vcount, "type": "VEC3", "min": mn, "max": mx },
            { "bufferView": 1, "componentType": 5125, "count": vcount, "type": "SCALAR" }
        ],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1, "mode": 4 }] }],
        "nodes": nodes,
        "scenes": [{ "nodes": [0] }],
        "scene": 0
    });
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(
            (0..images.len())
                .map(|i| json!({ "source": i }))
                .collect::<Vec<_>>()
        );
    }
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
    }

    let mut json_bytes = serde_json::to_vec(&doc)?;
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
    out.extend_from_slice(&bin);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_models_report_their_facts() {
        let dir = std::env::temp_dir().join(format!("gm-tools-glb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("m.glb");
        std::fs::write(
            &p,
            synthetic(&Spec {
                triangles: 123,
                bones: 7,
                textures: vec![(32, 16)],
            })
            .unwrap(),
        )
        .unwrap();
        let f = crate::budget::model_facts(&p).unwrap();
        assert_eq!(f.triangles, 123);
        assert_eq!(f.bones, 7);
        assert_eq!(f.textures, vec![(32, 16)]);
        assert!(f.payload_bytes > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
