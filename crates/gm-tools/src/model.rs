//! Model commands (MODELS.md): what a creator runs before uploading. `ingest` is the exact
//! pipeline the hub runs, so a file that passes here passes there.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use gm_core::vocab::ArchetypeFrame;
use gm_ingest::{Report, synth};
use gm_model::rig;

pub fn parse_frame(name: &str) -> Result<ArchetypeFrame> {
    rig::frame_by_name(name)
        .with_context(|| format!("unknown frame {name:?} (colossus, striker, caster, infiltrator)"))
}

pub fn print_report(r: &Report) {
    let f = &r.facts;
    println!(
        "{} for a {}: {} triangles, {} vertices, {} bones (from {} joints), texture {}x{} (from {}x{}), {} bytes{}{}",
        if r.ok { "ACCEPTED" } else { "REFUSED" },
        r.frame,
        f.triangles,
        f.vertices,
        f.bones,
        f.source_joints,
        f.texture[0],
        f.texture[1],
        f.source_texture[0],
        f.source_texture[1],
        f.bytes,
        if f.cutout { ", cutout" } else { "" },
        if f.two_sided { ", two-sided" } else { "" },
    );
    if f.bytes > 0 {
        println!(
            "  highest vertex {:.1} u, coverage {:.0}% front / {:.0}% side of the mannequin's",
            f.top,
            f.coverage_front * 100.0,
            f.coverage_side * 100.0
        );
    }
    for v in &r.violations {
        println!("  - {v}");
    }
    for n in &r.notes {
        println!("  note: {n}");
    }
    if r.ok {
        println!("  id {}", r.id);
    }
}

/// Run the ingestion on a local file; exit 1 when it is refused.
pub fn ingest(file: &Path, frame: &str, out: Option<&Path>) -> Result<()> {
    let frame = parse_frame(frame)?;
    let upload = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let (report, ingested) = gm_ingest::ingest(&upload, frame);
    print_report(&report);
    let Some(i) = ingested else {
        std::process::exit(1);
    };
    let dir = out.map_or_else(
        || file.parent().unwrap_or(Path::new(".")).to_path_buf(),
        Path::to_path_buf,
    );
    std::fs::create_dir_all(&dir)?;
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
    let gmm = dir.join(format!("{stem}.gmm"));
    let preview = dir.join(format!("{stem}.preview.png"));
    std::fs::write(&gmm, &i.gmm)?;
    std::fs::write(&preview, &i.preview)?;
    println!("  wrote {} and {}", gmm.display(), preview.display());
    Ok(())
}

/// Write the frame's template: the mannequin on the standard rig, in T-pose.
pub fn template(frame: &str, out: Option<&Path>) -> Result<()> {
    let f = parse_frame(frame)?;
    let path = out.map_or_else(
        || PathBuf::from(format!("{frame}_template.glb")),
        Path::to_path_buf,
    );
    let glb = synth::template(f).build();
    std::fs::write(&path, &glb).with_context(|| format!("writing {}", path.display()))?;
    println!(
        "template for a {frame}: {} bytes -> {} (the 24 bones of docs/MODELS.md 2, T-pose, +Y up, facing +Z, metres)",
        glb.len(),
        path.display()
    );
    Ok(())
}

/// Generate `count` distinct avatars at the budget ceiling; with `ingest`, also their `.gmm`.
pub fn synth_avatars(
    count: u32,
    out: &Path,
    seed: u64,
    side: u32,
    frames: &[String],
    ingest: bool,
) -> Result<()> {
    if !(64..=4096).contains(&side) {
        bail!("--side must be 64..=4096");
    }
    let frames: Vec<ArchetypeFrame> = if frames.is_empty() {
        rig::FRAMES.to_vec()
    } else {
        frames
            .iter()
            .map(|f| parse_frame(f))
            .collect::<Result<_>>()?
    };
    std::fs::create_dir_all(out)?;
    let started = std::time::Instant::now();
    let (mut glb_bytes, mut gmm_bytes) = (0u64, 0u64);
    for i in 0..count {
        let frame = frames[i as usize % frames.len()];
        let name = format!("avatar-{i:03}-{}", rig::frame_name(frame));
        let glb = synth::avatar(seed.wrapping_add(i as u64), frame, side).build();
        glb_bytes += glb.len() as u64;
        std::fs::write(out.join(format!("{name}.glb")), &glb)?;
        if ingest {
            let (report, ingested) = gm_ingest::ingest(&glb, frame);
            let Some(ing) = ingested else {
                print_report(&report);
                bail!("generated avatar {name} was refused");
            };
            gmm_bytes += ing.gmm.len() as u64;
            std::fs::write(out.join(format!("{name}.gmm")), &ing.gmm)?;
        }
    }
    println!(
        "synth: {count} avatars -> {} ({:.1} MB of .glb{}) in {:.1} s",
        out.display(),
        glb_bytes as f64 / 1e6,
        if ingest {
            format!(
                ", {:.1} MB of .gmm, {:.0} KB each",
                gmm_bytes as f64 / 1e6,
                gmm_bytes as f64 / 1e3 / count.max(1) as f64
            )
        } else {
            String::new()
        },
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
