//! Asset budgets (PLAN.md 2.6): parse `budgets.toml`, lint models and maps, and the self-test
//! that proves the gate rejects an over-budget asset.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Budgets {
    pub character: CharacterBudget,
    pub map: MapBudget,
    #[allow(dead_code)]
    pub client: ClientBudget,
}

#[derive(Debug, Deserialize)]
pub struct CharacterBudget {
    pub max_triangles: u64,
    pub max_bones: u64,
    pub max_texture_size: u32,
    pub max_textures: u64,
    pub max_payload_bytes: u64,
}

#[derive(Debug, Deserialize)]
pub struct MapBudget {
    pub max_bsp_bytes: u64,
    pub max_lightmap_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ClientBudget {
    pub max_binary_bytes: u64,
    pub max_regression_bytes: u64,
    pub max_rss_bytes: u64,
    pub min_fps: u32,
}

pub fn load_budgets(path: &Path) -> Result<Budgets> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[derive(Debug)]
pub struct Report {
    pub path: PathBuf,
    pub summary: String,
    pub violations: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Facts the checker extracts from a model.
#[derive(Debug, Default)]
pub struct ModelFacts {
    pub triangles: u64,
    pub bones: u64,
    pub textures: Vec<(u32, u32)>,
    pub payload_bytes: u64,
}

fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn image_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    Ok(reader.into_dimensions()?)
}

/// Resolve a glTF URI to bytes: `data:` URIs inline, everything else relative to `base`.
fn resolve_uri(uri: &str, base: &Path) -> Result<Vec<u8>> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, payload) = rest.split_once(',').context("malformed data URI")?;
        return decode_base64(payload).context("malformed base64 in data URI");
    }
    let path = base.join(uri);
    fs::read(&path).with_context(|| format!("reading external glTF resource {}", path.display()))
}

pub fn model_facts(path: &Path) -> Result<ModelFacts> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let gltf::Gltf { document, blob } =
        gltf::Gltf::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;

    let mut facts = ModelFacts {
        payload_bytes: bytes.len() as u64,
        ..Default::default()
    };

    // Buffers: the GLB blob or external files. External bytes count toward the payload.
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for buffer in document.buffers() {
        match buffer.source() {
            gltf::buffer::Source::Bin => buffers.push(blob.clone().unwrap_or_default()),
            gltf::buffer::Source::Uri(uri) => {
                let data = resolve_uri(uri, base)?;
                if !uri.starts_with("data:") {
                    facts.payload_bytes += data.len() as u64;
                }
                buffers.push(data);
            }
        }
    }

    for mesh in document.meshes() {
        for prim in mesh.primitives() {
            let count = prim
                .indices()
                .map(|a| a.count())
                .or_else(|| prim.get(&gltf::Semantic::Positions).map(|a| a.count()))
                .unwrap_or(0) as u64;
            facts.triangles += match prim.mode() {
                gltf::mesh::Mode::Triangles => count / 3,
                gltf::mesh::Mode::TriangleStrip | gltf::mesh::Mode::TriangleFan => {
                    count.saturating_sub(2)
                }
                _ => 0,
            };
        }
    }

    facts.bones = document
        .skins()
        .map(|s| s.joints().count() as u64)
        .max()
        .unwrap_or(0);

    for image in document.images() {
        let data: Vec<u8> = match image.source() {
            gltf::image::Source::View { view, .. } => {
                let buf = buffers
                    .get(view.buffer().index())
                    .context("image buffer view out of range")?;
                buf.get(view.offset()..view.offset() + view.length())
                    .context("image buffer view out of range")?
                    .to_vec()
            }
            gltf::image::Source::Uri { uri, .. } => {
                let data = resolve_uri(uri, base)?;
                if !uri.starts_with("data:") {
                    facts.payload_bytes += data.len() as u64;
                }
                data
            }
        };
        facts.textures.push(
            image_dimensions(&data)
                .with_context(|| format!("decoding image header in {}", path.display()))?,
        );
    }
    Ok(facts)
}

pub fn check_model(path: &Path, b: &CharacterBudget) -> Result<Report> {
    let f = model_facts(path)?;
    let mut v = Vec::new();
    if f.triangles > b.max_triangles {
        v.push(format!("{} triangles > {}", f.triangles, b.max_triangles));
    }
    if f.bones > b.max_bones {
        v.push(format!("{} bones > {}", f.bones, b.max_bones));
    }
    if f.textures.len() as u64 > b.max_textures {
        v.push(format!(
            "{} textures > {}",
            f.textures.len(),
            b.max_textures
        ));
    }
    for (w, h) in &f.textures {
        if *w > b.max_texture_size || *h > b.max_texture_size {
            v.push(format!(
                "texture {w}x{h} exceeds {0}x{0}",
                b.max_texture_size
            ));
        }
    }
    if f.payload_bytes > b.max_payload_bytes {
        v.push(format!(
            "{} payload bytes > {}",
            f.payload_bytes, b.max_payload_bytes
        ));
    }
    let tex = f
        .textures
        .iter()
        .map(|(w, h)| format!("{w}x{h}"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(Report {
        path: path.to_owned(),
        summary: format!(
            "tris={} bones={} textures=[{tex}] bytes={}",
            f.triangles, f.bones, f.payload_bytes
        ),
        violations: v,
    })
}

pub fn check_bsp(path: &Path, b: &MapBudget) -> Result<Report> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let header =
        gm_bsp::Bsp::parse_header(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    let lighting = header.lumps[gm_bsp::LUMP_LIGHTING].len as u64;
    let lit = fs::metadata(path.with_extension("lit"))
        .map(|m| m.len().saturating_sub(8))
        .unwrap_or(0);
    let lightmap_bytes = lighting + lit;
    let mut v = Vec::new();
    if bytes.len() as u64 > b.max_bsp_bytes {
        v.push(format!("{} bsp bytes > {}", bytes.len(), b.max_bsp_bytes));
    }
    if lightmap_bytes > b.max_lightmap_bytes {
        v.push(format!(
            "{lightmap_bytes} lightmap bytes (bsp + lit) > {}",
            b.max_lightmap_bytes
        ));
    }
    Ok(Report {
        path: path.to_owned(),
        summary: format!(
            "{:?} bytes={} lightmap_bytes={lightmap_bytes}",
            header.version,
            bytes.len()
        ),
        violations: v,
    })
}

pub fn check_paths(paths: &[PathBuf], budgets: &Budgets) -> Result<Vec<Report>> {
    let mut reports = Vec::new();
    for root in paths {
        if !root.exists() {
            bail!("{} does not exist", root.display());
        }
        let walker = walkdir::WalkDir::new(root).sort_by_file_name();
        for entry in walker {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            match ext.as_deref() {
                Some("glb") | Some("gltf") => reports.push(check_model(p, &budgets.character)?),
                Some("bsp") => reports.push(check_bsp(p, &budgets.map)?),
                _ => {}
            }
        }
    }
    Ok(reports)
}

pub fn check_cli(paths: &[PathBuf], budgets_path: &Path) -> Result<()> {
    let budgets = load_budgets(budgets_path)?;
    let reports = check_paths(paths, &budgets)?;
    let mut failed = 0;
    for r in &reports {
        if r.ok() {
            println!("OK    {} ({})", r.path.display(), r.summary);
        } else {
            failed += 1;
            println!("FAIL  {} ({})", r.path.display(), r.summary);
            for v in &r.violations {
                println!("      - {v}");
            }
        }
    }
    println!(
        "budget check: {} assets, {failed} over budget",
        reports.len()
    );
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Generate an over-budget and an in-budget model, run the checker on both, and fail unless the
/// verdicts are exactly "rejected" and "accepted".
pub fn self_test(budgets_path: &Path) -> Result<()> {
    let budgets = load_budgets(budgets_path)?;
    let b = &budgets.character;
    let dir = std::env::temp_dir().join(format!("gm-tools-selftest-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let result = (|| -> Result<()> {
        let over = dir.join("over_budget.glb");
        fs::write(
            &over,
            crate::glb::synthetic(&crate::glb::Spec {
                triangles: (b.max_triangles + 1500) as u32,
                bones: (b.max_bones + 8) as u32,
                textures: vec![(b.max_texture_size * 2, b.max_texture_size * 2), (64, 64)],
            })?,
        )?;
        let within = dir.join("in_budget.glb");
        fs::write(
            &within,
            crate::glb::synthetic(&crate::glb::Spec {
                triangles: (b.max_triangles / 3) as u32,
                bones: (b.max_bones - 8) as u32,
                textures: vec![(b.max_texture_size / 2, b.max_texture_size / 2)],
            })?,
        )?;
        let over_report = check_model(&over, b)?;
        let within_report = check_model(&within, b)?;
        println!(
            "over-budget model:  {} -> {} violations",
            over_report.summary,
            over_report.violations.len()
        );
        for v in &over_report.violations {
            println!("  - {v}");
        }
        println!(
            "in-budget model:    {} -> {} violations",
            within_report.summary,
            within_report.violations.len()
        );
        if over_report.violations.len() < 4 {
            bail!(
                "self-test FAIL: expected the over-budget model to violate triangles, bones, texture count and texture size"
            );
        }
        if !within_report.ok() {
            bail!(
                "self-test FAIL: the in-budget model was rejected: {:?}",
                within_report.violations
            );
        }
        println!(
            "self-test OK: the over-budget asset fails the gate and the in-budget asset passes"
        );
        Ok(())
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}
