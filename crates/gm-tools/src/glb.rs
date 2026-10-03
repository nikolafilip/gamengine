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

/// Pack a `.gltf` whose buffers and images are separate files (the way packs ship) into
/// one self-contained `.glb` (CONTENT.md 3: the upload format). Buffers are concatenated
/// into the BIN chunk with every view's offset moved; images become buffer views. A
/// `data:` URI is decoded; anything else is read relative to the file.
pub fn pack(path: &std::path::Path) -> Result<Vec<u8>> {
    use anyhow::Context;
    let text = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let mut doc: serde_json::Value = if text.starts_with(b"glTF") {
        // Already a .glb: pass it through.
        return Ok(text);
    } else {
        serde_json::from_slice(&text)
            .with_context(|| format!("{} is not glTF JSON", path.display()))?
    };
    let read_uri = |uri: &str| -> Result<Vec<u8>> {
        if let Some(rest) = uri.strip_prefix("data:") {
            let (_, b64) = rest
                .split_once(";base64,")
                .ok_or_else(|| anyhow::anyhow!("a data URI that is not base64"))?;
            base64_decode(b64)
        } else {
            let p = dir.join(percent_decode(uri));
            std::fs::read(&p).with_context(|| format!("reading {}", p.display()))
        }
    };
    let mut bin: Vec<u8> = Vec::new();
    // Every buffer in order; where each one starts in the BIN chunk.
    let mut buffer_starts: Vec<usize> = Vec::new();
    let buffers = doc
        .get("buffers")
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();
    for b in &buffers {
        let uri = b
            .get("uri")
            .and_then(|u| u.as_str())
            .ok_or_else(|| anyhow::anyhow!("a buffer without a uri (a .glb's own chunk?)"))?;
        let bytes = read_uri(uri)?;
        pad4(&mut bin, 0);
        buffer_starts.push(bin.len());
        bin.extend_from_slice(&bytes);
    }
    if let Some(views) = doc.get_mut("bufferViews").and_then(|v| v.as_array_mut()) {
        for v in views.iter_mut() {
            let buffer = v.get("buffer").and_then(|b| b.as_u64()).unwrap_or(0) as usize;
            let offset = v.get("byteOffset").and_then(|b| b.as_u64()).unwrap_or(0) as usize;
            let start = *buffer_starts
                .get(buffer)
                .ok_or_else(|| anyhow::anyhow!("a buffer view names a buffer that is not there"))?;
            v["buffer"] = json!(0);
            v["byteOffset"] = json!(start + offset);
        }
    }
    // Images by uri become views of the one buffer.
    let mut new_views: Vec<serde_json::Value> = Vec::new();
    let view_count = doc
        .get("bufferViews")
        .and_then(|v| v.as_array())
        .map_or(0, |v| v.len());
    if let Some(images) = doc.get_mut("images").and_then(|v| v.as_array_mut()) {
        for img in images.iter_mut() {
            if let Some(uri) = img.get("uri").and_then(|u| u.as_str()).map(str::to_string) {
                let bytes = read_uri(&uri)?;
                let mime = if bytes.starts_with(b"\x89PNG") {
                    "image/png"
                } else {
                    "image/jpeg"
                };
                pad4(&mut bin, 0);
                new_views.push(
                    json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len() }),
                );
                bin.extend_from_slice(&bytes);
                let obj = img.as_object_mut().expect("an image is an object");
                obj.remove("uri");
                obj.insert("bufferView".into(), json!(view_count + new_views.len() - 1));
                obj.insert("mimeType".into(), json!(mime));
            }
        }
    }
    if !new_views.is_empty() {
        if !doc["bufferViews"].is_array() {
            doc["bufferViews"] = json!([]);
        }
        let views = doc["bufferViews"]
            .as_array_mut()
            .expect("an array was just made");
        views.extend(new_views);
    }
    pad4(&mut bin, 0);
    doc["buffers"] = json!([{ "byteLength": bin.len() }]);
    Ok(gm_ingest::write::container(&doc, &bin))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => anyhow::bail!("not base64"),
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
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
