//! gm-ingest: model ingestion (`docs/MODELS.md` 3–5). A glTF upload goes in; a report comes out,
//! and, when every check passes, the `.gmm` the hub will serve and the preview a moderator will
//! look at.
//!
//! - [`source`]: the upload, bounds-checked, merged onto the standard rig in model space.
//! - [`texture`]: the atlas: resampled, mipped in linear light, encoded as BC1.
//! - [`envelope`]: the pose, the box and the coverage a frame demands.
//! - [`raster`]: the orthographic rasteriser behind the coverage rule and the preview.
//! - [`write`], [`synth`]: writing `.glb` files: the creator's template and generated avatars.
//!
//! The hub never calls [`ingest`] in its own process: uploads are parsed in a worker
//! (`gm-hub ingest-worker`, [`worker`]) so that a hostile file costs one upload and no more.
#![forbid(unsafe_code)]

pub mod envelope;
pub mod raster;
pub mod source;
pub mod synth;
pub mod texture;
pub mod write;

use std::path::Path;

use gm_core::vocab::ArchetypeFrame;
use gm_model::format::{self, FLAG_CUTOUT, FLAG_TWO_SIDED, limits};
use gm_model::{Model, ModelId, rig};
use serde::{Deserialize, Serialize};

/// The largest upload the pipeline looks at (MODELS.md 3).
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
/// UVs may leave the unit square by this much before they are a violation.
const UV_SLACK: f32 = 0.01;

/// What was measured, accepted or not.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    pub triangles: u32,
    pub vertices: u32,
    pub source_joints: u32,
    pub bones: u32,
    pub source_texture: [u32; 2],
    pub texture: [u32; 2],
    /// Size of the `.gmm`.
    pub bytes: u32,
    pub cutout: bool,
    pub two_sided: bool,
    /// Highest vertex, world units.
    pub top: f32,
    /// Coverage relative to the frame's mannequin.
    pub coverage_front: f32,
    pub coverage_side: f32,
}

/// The answer to an upload (MODELS.md 4: every violation at once).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub ok: bool,
    /// The model id in hex when `ok`.
    pub id: String,
    pub frame: String,
    pub facts: Facts,
    pub violations: Vec<String>,
    pub notes: Vec<String>,
}

pub struct Ingested {
    pub id: ModelId,
    /// The `.gmm` file.
    pub gmm: Vec<u8>,
    /// The moderation preview, PNG.
    pub preview: Vec<u8>,
    pub model: Model,
}

/// Validate an upload for `frame` and re-encode it.
pub fn ingest(upload: &[u8], frame: ArchetypeFrame) -> (Report, Option<Ingested>) {
    let mut report = Report {
        frame: rig::frame_name(frame).to_string(),
        ..Default::default()
    };
    if upload.len() > MAX_UPLOAD_BYTES {
        report.violations.push(format!(
            "the upload is {} bytes; the limit is {MAX_UPLOAD_BYTES}",
            upload.len()
        ));
        return (report, None);
    }
    let src = match source::read(upload) {
        Ok(s) => s,
        Err(violations) => {
            report.violations = violations;
            return (report, None);
        }
    };
    report.notes = src.notes.clone();
    let facts = &mut report.facts;
    facts.triangles = src.mesh.triangles() as u32;
    facts.vertices = src.mesh.positions.len() as u32;
    facts.source_joints = src.source_joints as u32;
    facts.bones = src.mesh.bone_mask.count_ones();
    facts.source_texture = [src.texture.width(), src.texture.height()];
    facts.cutout = src.cutout.is_some();
    facts.two_sided = src.two_sided;
    let dropped = src.source_triangles - src.mesh.triangles();
    if dropped > 0 {
        report
            .notes
            .push(format!("{dropped} degenerate triangles were dropped"));
    }

    let v = &mut report.violations;
    if src.mesh.triangles() > limits::MAX_TRIANGLES {
        v.push(format!(
            "{} triangles; the budget is {}",
            src.mesh.triangles(),
            limits::MAX_TRIANGLES
        ));
    }
    let stray = src
        .mesh
        .uvs
        .iter()
        .filter(|uv| {
            uv.iter()
                .any(|c| !c.is_finite() || *c < -UV_SLACK || *c > 1.0 + UV_SLACK)
        })
        .count();
    if stray > 0 {
        v.push(format!(
            "{stray} vertices have texture coordinates outside [0, 1]; the atlas does not tile"
        ));
    }
    envelope::check_pose(&src.mesh.pivots, src.mesh.bone_mask, frame, v);
    if src.mesh.triangles() > limits::MAX_TRIANGLES || src.mesh.triangles() == 0 {
        return (report, None);
    }

    let atlas = texture::build(&src.texture, src.base_color, src.cutout);
    let (scale, vertices) = format::quantize_vertices(
        &src.mesh.positions,
        &src.mesh.normals,
        &src.mesh.uvs,
        &src.mesh.joints,
        &src.mesh.weights,
    );
    let model = Model {
        flags: if src.cutout.is_some() { FLAG_CUTOUT } else { 0 }
            | if src.two_sided { FLAG_TWO_SIDED } else { 0 },
        frame: rig::frame_index(frame),
        scale,
        average: atlas.average,
        bone_mask: src.mesh.bone_mask,
        pivots: src.mesh.pivots.map(|p| p.to_array()),
        vertices,
        indices: src.mesh.indices.iter().map(|i| *i as u16).collect(),
        tex_w: atlas.w,
        tex_h: atlas.h,
        texture: atlas.bc1,
    };
    report.facts.texture = [atlas.w as u32, atlas.h as u32];
    let gmm = match model.encode() {
        Ok(bytes) => bytes,
        Err(e) => {
            // Far out of the box (the scale check) or over the size ceiling.
            report
                .violations
                .push(format!("the model cannot be stored: {e}"));
            return (report, None);
        }
    };
    report.facts.bytes = gmm.len() as u32;
    let measured = envelope::check_model(&model, frame, &mut report.violations);
    report.facts.top = measured.top;
    report.facts.coverage_front = measured.front;
    report.facts.coverage_side = measured.side;
    if !report.violations.is_empty() {
        return (report, None);
    }
    // The last word is the check the hub repeats on the stored bytes: an upload is accepted
    // here exactly when its `.gmm` is accepted there.
    let Verified { id, model, preview } = match verify(&gmm, frame) {
        Ok(v) => v,
        Err(violations) => {
            report.violations = violations;
            return (report, None);
        }
    };
    report.ok = true;
    report.id = format::id_hex(&id);
    (
        report,
        Some(Ingested {
            id,
            gmm,
            preview,
            model,
        }),
    )
}

/// A `.gmm` the hub may store.
#[derive(Debug)]
pub struct Verified {
    pub id: ModelId,
    pub model: Model,
    /// The moderation preview, PNG, drawn from the bytes that are stored.
    pub preview: Vec<u8>,
}

/// Check a `.gmm` for `frame` from its own bytes alone (MODELS.md 6.2). The ingestion worker
/// parses an untrusted file and may be subverted by it, so the hub believes nothing the
/// worker says: what it returns is read with the strict reader, must be the canonical encoding
/// of what it decodes to (one model, one id), must be for the frame and pass the pose and
/// envelope rules, and the preview a moderator judges it by is drawn here, from these bytes.
/// Nothing in this function reads an upload.
pub fn verify(gmm: &[u8], frame: ArchetypeFrame) -> Result<Verified, Vec<String>> {
    let model = Model::decode(gmm).map_err(|e| vec![format!("the model cannot be stored: {e}")])?;
    let mut v = Vec::new();
    if model.frame != rig::frame_index(frame) {
        v.push(format!(
            "the model is for another frame than {}",
            rig::frame_name(frame)
        ));
    }
    if model.encode().ok().as_deref() != Some(gmm) {
        v.push("the model is not in its canonical encoding".to_string());
    }
    let pivots: [glam::Vec3; rig::BONES] = std::array::from_fn(|b| model.pivot(b));
    envelope::check_pose(&pivots, model.bone_mask, frame, &mut v);
    envelope::check_model(&model, frame, &mut v);
    if !v.is_empty() {
        return Err(v);
    }
    let (_, h) = frame.capsule();
    let preview = write::png(
        (raster::PREVIEW_SIDE * 2) as u32,
        raster::PREVIEW_SIDE as u32,
        &raster::preview(&model, h),
    );
    Ok(Verified {
        id: format::model_id(gmm),
        model,
        preview,
    })
}

/// File names the worker writes into its output directory.
pub const WORKER_REPORT: &str = "report.json";
pub const WORKER_MODEL: &str = "model.gmm";

/// The body of the ingestion worker process: read the upload, ingest it, write the report and
/// (when accepted) the model and its preview into `out_dir`. The caller applies the resource
/// limits and the wall clock (MODELS.md 6.2).
pub fn worker(input: &Path, frame: ArchetypeFrame, out_dir: &Path) -> std::io::Result<Report> {
    let upload = std::fs::read(input)?;
    let (report, ingested) = ingest(&upload, frame);
    if let Some(i) = &ingested {
        std::fs::write(out_dir.join(WORKER_MODEL), &i.gmm)?;
    }
    // The report goes last: its presence says the worker finished.
    let json = serde_json::to_vec_pretty(&report).map_err(std::io::Error::other)?;
    std::fs::write(out_dir.join(WORKER_REPORT), json)?;
    Ok(report)
}
