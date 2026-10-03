//! `gm-tools content` (CONTENT.md 5): the content directory checked, built into the bundle
//! the client loads, and reported on. Everything the client will draw for the content is
//! made here: props ingested from their `.glb`s, icons given or baked, the fonts rasterised,
//! the skin's pieces and all the pictures packed into one atlas, and the manifest that
//! names every one of them by key.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use ab_glyph::{Font, ScaleFont};
use anyhow::{Context, Result, anyhow, bail};
use clap::Subcommand;
use gm_content::looks::Looks;
use gm_core::tick::TickRate;
use gm_core::vocab::Status;
use gm_ingest::source::Fit;
use gm_model::atlas::{self, Atlas, Cell, Face, Glyph, Piece};
use gm_model::manifest::{
    AbilityEntry, BuildEntry, CreatureEntry, HELD_BACK, HELD_LEFT, HELD_RIGHT, IconEntry,
    MANIFEST_VERSION, Manifest, MaterialEntry, PropEntry, TemplateEntry,
};
use gm_model::{Model, mannequin, rig, smallfont};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Subcommand)]
pub enum ContentCmd {
    /// Every row, every link, every file, every budget; exit 1 on the first list of violations.
    Check {
        #[arg(default_value = "assets/content")]
        dir: PathBuf,
        /// Also rebuild into a temporary directory and compare with this committed bundle,
        /// byte for byte (CI's check, CONTENT.md 5.4).
        #[arg(long)]
        built: Option<PathBuf>,
    },
    /// Check, ingest, bake, assemble, write the manifest.
    Build {
        #[arg(default_value = "assets/content")]
        dir: PathBuf,
        #[arg(long, default_value = "assets/built/content")]
        out: PathBuf,
    },
    /// A table of everything: keys, files, triangles, bytes, icons baked or given.
    Report {
        #[arg(default_value = "assets/content")]
        dir: PathBuf,
    },
    /// Write the built atlas as a PNG to look at (and each prop's baked icon beside it).
    Atlas {
        #[arg(default_value = "assets/content")]
        dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Write one of the tool's own flat-coloured props as a .glb (CONTENT.md 7: the
    /// procedural source): sword, hammer, musket.
    Synth {
        /// `sword`, `hammer` or `musket`.
        what: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Bring a file in: a model (`.gltf` with its .bin and image beside it, or a `.glb`)
    /// packed as one `.glb` under models/<kind>/<key>.glb, or a picture as icons/<key>.png.
    Import {
        /// `prop`, `creature`, `avatar` or `icon`.
        kind: String,
        key: String,
        file: PathBuf,
        #[arg(long, default_value = "assets/content")]
        dir: PathBuf,
    },
}

pub fn run(cmd: ContentCmd) -> Result<()> {
    match cmd {
        ContentCmd::Check { dir, built } => {
            let bundle = build(&dir)?;
            print_summary(&bundle);
            if let Some(committed) = built {
                compare(&bundle, &committed)?;
                println!(
                    "content: the committed bundle in {} is what the sources build",
                    committed.display()
                );
            }
            Ok(())
        }
        ContentCmd::Build { dir, out } => {
            let bundle = build(&dir)?;
            write(&bundle, &out)?;
            print_summary(&bundle);
            println!("content: bundle written to {}", out.display());
            Ok(())
        }
        ContentCmd::Report { dir } => {
            let bundle = build(&dir)?;
            report(&bundle);
            Ok(())
        }
        ContentCmd::Import {
            kind,
            key,
            file,
            dir,
        } => import(&kind, &key, &file, &dir),
        ContentCmd::Atlas { dir, out } => {
            let bundle = build(&dir)?;
            let a = &bundle.atlas;
            std::fs::write(
                &out,
                gm_ingest::write::png(a.w as u32, a.h as u32, &a.texels),
            )?;
            println!("content: atlas {} x {} -> {}", a.w, a.h, out.display());
            Ok(())
        }
        ContentCmd::Synth { what, out } => {
            let glb = match what.as_str() {
                "sword" => gm_ingest::synth::sword_prop(0.85),
                "hammer" => gm_ingest::synth::hammer_prop(),
                "musket" => gm_ingest::synth::musket_prop(),
                other => bail!("`{other}` is not a prop the tool makes: sword, hammer, musket"),
            }
            .build();
            std::fs::write(&out, &glb).with_context(|| format!("writing {}", out.display()))?;
            println!("content: {what} -> {} ({} bytes)", out.display(), glb.len());
            Ok(())
        }
    }
}

/// `gm-tools content import KIND KEY FILE`: the file packed and put where the standard
/// looks for it (CONTENT.md 2), checked as its kind on the way. Nothing is written to a
/// table: the row that names the key is the author's.
fn import(kind: &str, key: &str, file: &Path, dir: &Path) -> Result<()> {
    if !gm_content::looks::valid_key(key) {
        bail!("`{key}` is not a key (lowercase letters, digits, `_`)");
    }
    let name = key.replace('/', "_");
    match kind {
        "prop" | "creature" | "avatar" => {
            let glb = crate::glb::pack(file)?;
            let (report, _) = if kind == "prop" {
                gm_ingest::ingest_prop(&glb, Fit::default())
            } else {
                // A body is checked for every frame; one must take it.
                let mut best: Option<gm_ingest::Report> = None;
                for frame in rig::FRAMES {
                    let (r, _) = gm_ingest::ingest(&glb, frame);
                    if r.ok
                        || best
                            .as_ref()
                            .is_none_or(|b| r.violations.len() < b.violations.len())
                    {
                        best = Some(r.clone());
                    }
                    if r.ok {
                        break;
                    }
                }
                (best.expect("four frames"), None)
            };
            for n in &report.notes {
                println!("content: note: {n}");
            }
            if !report.ok {
                for v in &report.violations {
                    eprintln!("content: {v}");
                }
                if kind == "prop" {
                    eprintln!(
                        "content: (a prop's fit is the row's; import takes the file as it is, and a reach past the hand is a fit to make)"
                    );
                }
                bail!("{} is not a {kind} the pipeline takes", file.display());
            }
            let out = dir.join(format!("models/{kind}s/{name}.glb"));
            std::fs::create_dir_all(out.parent().expect("a parent"))?;
            std::fs::write(&out, &glb).with_context(|| format!("writing {}", out.display()))?;
            println!(
                "content: {} -> {} ({} triangles, {} bytes); name it in LICENSES.md and in a row's `model`",
                file.display(),
                out.display(),
                report.facts.triangles,
                glb.len()
            );
        }
        "icon" => {
            let px = read_icon(file)?;
            let out = dir.join(format!("icons/{name}.png"));
            std::fs::create_dir_all(out.parent().expect("a parent"))?;
            std::fs::write(
                &out,
                gm_ingest::write::png(atlas::ICON as u32, atlas::ICON as u32, &px),
            )?;
            println!("content: {} -> {}", file.display(), out.display());
        }
        other => bail!("`{other}` is not a kind: prop, creature, avatar or icon"),
    }
    Ok(())
}

/// The bundle, in memory: every file by its path under the output directory.
pub struct Bundle {
    pub files: BTreeMap<String, Vec<u8>>,
    pub manifest: Manifest,
    pub atlas: Atlas,
    /// What was baked rather than given, by icon key.
    pub baked: Vec<String>,
    pub facts: Vec<(String, gm_ingest::Facts)>,
}

/// `skin.toml` (LOOK.md 2.2): the faces to rasterise and the pieces of the skin.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkinToml {
    #[serde(default)]
    font: BTreeMap<String, FontToml>,
    #[serde(default)]
    piece: BTreeMap<String, PieceToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FontToml {
    file: String,
    /// Dots per em.
    size: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PieceToml {
    file: String,
    /// Nine-slice insets: left, top, right, bottom.
    #[serde(default)]
    inset: [u8; 4],
}

/// The pieces the client's toolkit asks for (LOOK.md 2.2): every one must be in the skin.
pub const PIECES: &[&str] = &[
    "panel",
    "panel_title",
    "well",
    "button",
    "button_hot",
    "button_down",
    "button_off",
    "field",
    "field_focus",
    "slot",
    "slot_hot",
    "slot_picked",
    "slot_worn",
    "slot_off",
    "bar_frame",
    "bar_fill",
    "portrait_frame",
    "hotbar_cell",
    "hotbar_key",
    "tooltip",
    "check_off",
    "check_on",
    "slider_rail",
    "slider_knob",
    "scroll_rail",
    "scroll_knob",
    "cursor",
    "cursor_drag",
    "coin_gold",
    "coin_silver",
    "mark_new",
    "mark_taken",
    "mark_worn",
];

/// The faces (LOOK.md 2.3): their ids in the atlas.
pub const FACES: &[(&str, u8)] = &[("text", 1), ("title", 2)];

/// The characters a face is rasterised for: printable ASCII and the ten letters of
/// CLIENT.md 3.
fn charset() -> Vec<char> {
    (0x20u8..=0x7e)
        .map(char::from)
        .chain("čćđšžČĆĐŠŽ".chars())
        .collect()
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A 32 × 32 RGBA picture read from a PNG, or refused.
fn read_icon(path: &Path) -> Result<Vec<u8>> {
    let img = image::load_from_memory(&read(path)?)
        .with_context(|| format!("decoding {}", path.display()))?
        .to_rgba8();
    if img.width() != atlas::ICON as u32 || img.height() != atlas::ICON as u32 {
        bail!(
            "{}: an icon is {0} × {0} dots, this one is {} × {}",
            atlas::ICON,
            img.width(),
            img.height()
        );
    }
    Ok(img.into_raw())
}

/// A glyph as rasterised: its char, picture, bearing and advance.
type RasterGlyph = (char, Picture, [i8; 2], u8);
/// A face as rasterised: id, line height, ascent, glyphs.
type RasterFace = (u8, u8, u8, Vec<RasterGlyph>);

/// A picture to pack: its texels and size.
struct Picture {
    w: u16,
    h: u16,
    rgba: Vec<u8>,
}

/// Build the bundle from the sources in `dir`, checking everything on the way (CONTENT.md
/// 5.1 and 5.2). The first violation is an error; a list of them where a list is cheap.
pub fn build(dir: &Path) -> Result<Bundle> {
    // What things do, validated as the zones validate it.
    let pack = gm_content::load_dir(dir, TickRate::COMBAT)
        .map_err(|e| anyhow!("{e}"))
        .context("the tables")?;
    let looks = Looks::load_dir(dir).map_err(|e| anyhow!("{e}"))?;
    let content_version: u32 = std::fs::read_to_string(dir.join("VERSION"))
        .with_context(|| {
            format!(
                "{}: every content directory has a VERSION file with a number",
                dir.join("VERSION").display()
            )
        })?
        .trim()
        .parse()
        .context("VERSION is a number")?;
    let licenses = std::fs::read_to_string(dir.join("LICENSES.md")).unwrap_or_default();
    let mut problems: Vec<String> = Vec::new();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut facts: Vec<(String, gm_ingest::Facts)> = Vec::new();
    let mut baked: Vec<String> = Vec::new();
    // Pictures by icon key, to be packed.
    let mut icons: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // Models by prop key: ingested once however many rows name them.
    let mut props: BTreeMap<String, Model> = BTreeMap::new();

    let licensed = |file: &str| licenses.contains(file);

    // Props, from the templates (with their fits) and the abilities (bare, no fit).
    let mut prop_fits: BTreeMap<String, Fit> = BTreeMap::new();
    for t in &looks.templates {
        if let Some(model) = &t.model {
            let fit = Fit {
                mov: t.fit.mov,
                turn: t.fit.turn,
                scale: t.fit.scale,
            };
            if let Some(other) = prop_fits.get(model)
                && *other != fit
            {
                problems.push(format!(
                    "template {}: names model `{model}` with another fit than an earlier row; one model, one fit",
                    t.key
                ));
            }
            prop_fits.insert(model.clone(), fit);
        }
    }
    for a in &looks.abilities {
        if let Some(p) = &a.prop {
            prop_fits.entry(p.clone()).or_default();
        }
    }
    for (key, fit) in &prop_fits {
        let file = format!("models/props/{}.glb", key.replace('/', "_"));
        let path = dir.join(&file);
        if !path.is_file() {
            problems.push(format!("prop `{key}`: {file} does not exist"));
            continue;
        }
        if !licensed(&format!("{}.glb", key.replace('/', "_"))) {
            problems.push(format!("prop `{key}`: {file} is not named in LICENSES.md"));
        }
        let (report, ingested) = gm_ingest::ingest_prop(&read(&path)?, *fit);
        match ingested {
            Some(ing) if report.ok => {
                let out = format!("props/{}.gmm", key.replace('/', "_"));
                facts.push((key.clone(), report.facts.clone()));
                files.insert(out, ing.gmm);
                props.insert(key.clone(), ing.model);
            }
            _ => {
                for v in &report.violations {
                    problems.push(format!("prop `{key}` ({file}): {v}"));
                }
            }
        }
    }

    // Icons: given as files, or baked from models; the keys they go in the atlas under.
    let icon_file = |key: &str| dir.join(format!("icons/{}.png", key.replace('/', "_")));
    fn take_icon(
        path: PathBuf,
        key: &str,
        owner: &str,
        icons: &mut BTreeMap<String, Vec<u8>>,
        problems: &mut Vec<String>,
    ) -> Option<String> {
        if path.is_file() {
            match read_icon(&path) {
                Ok(px) => {
                    icons.insert(key.to_string(), px);
                    return Some(key.to_string());
                }
                Err(e) => problems.push(format!("{owner}: icon `{key}`: {e:#}")),
            }
        } else {
            problems.push(format!(
                "{owner}: icon `{key}` has no file {}",
                path.display()
            ));
        }
        None
    }

    let mut manifest = Manifest {
        format: MANIFEST_VERSION,
        content_version,
        ..Default::default()
    };
    for t in &looks.templates {
        let owner = format!("template {}", t.key);
        let icon = match &t.icon {
            Some(k) => take_icon(icon_file(k), k, &owner, &mut icons, &mut problems),
            None => match t.model.as_ref().and_then(|m| props.get(m)) {
                Some(model) => {
                    let key = format!("item/{}", t.key);
                    icons.insert(
                        key.clone(),
                        gm_ingest::raster::icon(model, atlas::ICON as usize),
                    );
                    baked.push(key.clone());
                    Some(key)
                }
                None => None,
            },
        };
        manifest.templates.push(TemplateEntry {
            key: t.key.clone(),
            kind: t.kind.clone(),
            model: t.model.clone(),
            icon,
            held: match t.held.as_str() {
                "left" => HELD_LEFT,
                "back" => HELD_BACK,
                _ => HELD_RIGHT,
            },
            fit_view: t.fit_view.map(|f| {
                [
                    f.mov[0], f.mov[1], f.mov[2], f.turn[0], f.turn[1], f.turn[2], f.scale,
                ]
            }),
            retired: t.retired,
        });
    }
    for m in &looks.materials {
        let owner = format!("material {}", m.key);
        let icon = m
            .icon
            .as_ref()
            .and_then(|k| take_icon(icon_file(k), k, &owner, &mut icons, &mut problems));
        manifest.materials.push(MaterialEntry {
            key: m.key.clone(),
            layer: m.layer.clone(),
            icon,
            tint: m.tint,
        });
    }
    for a in &looks.abilities {
        let owner = format!("ability {}", a.key);
        let icon = a
            .icon
            .as_ref()
            .and_then(|k| take_icon(icon_file(k), k, &owner, &mut icons, &mut problems));
        manifest.abilities.push(AbilityEntry {
            key: a.key.clone(),
            icon,
            prop: a.prop.clone(),
            sound: a.sound.clone(),
        });
    }
    for c in &looks.creatures {
        let owner = format!("creature {}", c.key);
        if let Some(model) = &c.model {
            let file = format!("models/creatures/{}.glb", model.replace('/', "_"));
            if !dir.join(&file).is_file() {
                problems.push(format!(
                    "{owner}: {file} does not exist (creature models are Phase 16's)"
                ));
            }
        }
        let icon = c
            .icon
            .as_ref()
            .and_then(|k| take_icon(icon_file(k), k, &owner, &mut icons, &mut problems));
        manifest.creatures.push(CreatureEntry {
            key: c.key.clone(),
            model: c.model.clone(),
            icon,
        });
    }
    for b in &looks.builds {
        let owner = format!("build {}", b.name);
        let icon = b
            .icon
            .as_ref()
            .and_then(|k| take_icon(icon_file(k), k, &owner, &mut icons, &mut problems));
        manifest.builds.push(BuildEntry {
            name: b.name.clone(),
            icon,
        });
    }
    for s in Status::ALL {
        let key = format!("status/{}", s.name());
        if icon_file(&key).is_file()
            && let Some(k) = take_icon(icon_file(&key), &key, "a status", &mut icons, &mut problems)
        {
            manifest.statuses.push(IconEntry {
                key: s.name().to_string(),
                icon: k,
            });
        }
    }
    // Portraits of every frame in every armour class, from the mannequin (CONTENT.md 5.3),
    // unless a file is given.
    for frame in rig::FRAMES {
        for (ai, tint) in mannequin::ARMOUR_TINTS.iter().enumerate() {
            let armour =
                gm_core::matrix::ArmourClass::from_index(ai as u8).map_or("cloth", |a| a.name());
            let key = format!("portrait/{}_{}", rig::frame_name(frame), armour);
            let icon = if icon_file(&key).is_file() {
                take_icon(
                    icon_file(&key),
                    &key,
                    "a portrait",
                    &mut icons,
                    &mut problems,
                )
            } else {
                let model = mannequin_model(frame, *tint);
                icons.insert(
                    key.clone(),
                    gm_ingest::raster::portrait(&model, atlas::ICON as usize),
                );
                baked.push(key.clone());
                Some(key.clone())
            };
            if let Some(icon) = icon {
                manifest.portraits.push(IconEntry {
                    key: format!("{}_{}", rig::frame_name(frame), armour),
                    icon,
                });
            }
        }
    }
    for (key, model) in &props {
        let file = format!("props/{}.gmm", key.replace('/', "_"));
        let bytes = &files[&file];
        manifest.props.push(PropEntry {
            key: key.clone(),
            file,
            sha256: sha(bytes),
            bytes: bytes.len() as u32,
            triangles: model.triangles() as u32,
        });
    }
    // Every ability of the pack has a row of looks (the same file): a prop named by an
    // ability must have been ingested above.
    for a in &pack.abilities {
        if looks.abilities.iter().all(|l| l.key != a.key) {
            problems.push(format!(
                "ability {}: no look row (the tables disagree)",
                a.key
            ));
        }
    }

    // The skin and the fonts.
    let skin_path = dir.join("ui/skin.toml");
    let skin: SkinToml = match std::fs::read_to_string(&skin_path) {
        Ok(text) => {
            toml::from_str(&text).with_context(|| format!("parsing {}", skin_path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            problems.push(format!("{} does not exist", skin_path.display()));
            SkinToml::default()
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", skin_path.display())),
    };
    let mut pieces: BTreeMap<String, (Picture, [u8; 4])> = BTreeMap::new();
    for name in PIECES {
        let Some(p) = skin.piece.get(*name) else {
            problems.push(format!("skin.toml: no piece `{name}`"));
            continue;
        };
        let path = dir.join("ui").join(&p.file);
        let img = match read(&path).and_then(|b| {
            image::load_from_memory(&b)
                .map(|i| i.to_rgba8())
                .with_context(|| format!("decoding {}", path.display()))
        }) {
            Ok(i) => i,
            Err(e) => {
                problems.push(format!("skin piece `{name}`: {e:#}"));
                continue;
            }
        };
        if img.width() > 256 || img.height() > 256 || img.width() == 0 || img.height() == 0 {
            problems.push(format!(
                "skin piece `{name}`: {} × {} is not within 256 × 256",
                img.width(),
                img.height()
            ));
            continue;
        }
        if p.inset[0] as u32 + p.inset[2] as u32 > img.width()
            || p.inset[1] as u32 + p.inset[3] as u32 > img.height()
        {
            problems.push(format!(
                "skin piece `{name}`: the insets are larger than the picture"
            ));
            continue;
        }
        pieces.insert(
            name.to_string(),
            (
                Picture {
                    w: img.width() as u16,
                    h: img.height() as u16,
                    rgba: img.into_raw(),
                },
                p.inset,
            ),
        );
    }
    for name in skin.piece.keys() {
        if !PIECES.contains(&name.as_str()) {
            problems.push(format!(
                "skin.toml: piece `{name}` is not one the toolkit asks for"
            ));
        }
    }
    let mut faces: Vec<RasterFace> = Vec::new();
    for (name, id) in FACES {
        let Some(f) = skin.font.get(*name) else {
            problems.push(format!("skin.toml: no font `{name}`"));
            continue;
        };
        let path = dir.join("ui").join(&f.file);
        if !licensed(
            Path::new(&f.file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&f.file),
        ) {
            problems.push(format!(
                "font `{name}`: {} is not named in LICENSES.md",
                f.file
            ));
        }
        match read(&path).and_then(|bytes| rasterise(&bytes, f.size)) {
            Ok((line_height, ascent, glyphs)) => faces.push((*id, line_height, ascent, glyphs)),
            Err(e) => problems.push(format!("font `{name}`: {e:#}")),
        }
    }

    if !problems.is_empty() {
        for p in &problems {
            eprintln!("content: {p}");
        }
        bail!("{} problem(s) in {}", problems.len(), dir.display());
    }

    // The atlas: pack everything.
    let atlas = assemble(&pieces, &faces, &icons)?;
    let gma = atlas.encode().map_err(|e| anyhow!("the atlas: {e}"))?;
    manifest.atlas_sha256 = sha(&gma);
    manifest.atlas_bytes = gma.len() as u32;
    files.insert("ui.gma".into(), gma);
    files.insert("manifest.gmc".into(), manifest.encode());
    Ok(Bundle {
        files,
        manifest,
        atlas,
        baked,
        facts,
    })
}

/// The mannequin of a frame, tinted, as a model the rasteriser draws (for its portrait).
fn mannequin_model(frame: gm_core::vocab::ArchetypeFrame, tint: [f32; 3]) -> Model {
    let mesh = mannequin::build(frame, &mannequin::Shape::MANNEQUIN);
    let n = mannequin::MANNEQUIN_TEXTURE_SIZE;
    let mut rgba = mannequin::mannequin_texture();
    for px in rgba.chunks_exact_mut(4) {
        for k in 0..3 {
            px[k] = (px[k] as f32 * tint[k]).clamp(0.0, 255.0) as u8;
        }
    }
    let img = image::RgbaImage::from_raw(n, n, rgba).expect("the mannequin's texture");
    let atlas = gm_ingest::texture::build(&img, [1.0; 4], None);
    let (scale, vertices) = gm_model::format::quantize_vertices(
        &mesh.positions,
        &mesh.normals,
        &mesh.uvs,
        &mesh.joints,
        &mesh.weights,
    );
    Model {
        flags: 0,
        frame: rig::frame_index(frame),
        scale,
        average: atlas.average,
        bone_mask: mesh.bone_mask,
        pivots: mesh.pivots.map(|p| p.to_array()),
        vertices,
        indices: mesh.indices.iter().map(|i| *i as u16).collect(),
        tex_w: atlas.w,
        tex_h: atlas.h,
        texture: atlas.bc1,
    }
}

/// Rasterise a face at `size` dots per em: the line height, the ascent, and every glyph of
/// the charset as a picture with its bearing and advance (LOOK.md 2.3).
fn rasterise(bytes: &[u8], size: f32) -> Result<(u8, u8, Vec<RasterGlyph>)> {
    if !(4.0..=64.0).contains(&size) {
        bail!("a face is 4 to 64 dots, not {size}");
    }
    let font = ab_glyph::FontRef::try_from_slice(bytes).context("not a font file")?;
    let scaled = font.as_scaled(ab_glyph::PxScale::from(size));
    let ascent = scaled.ascent().round();
    let descent = scaled.descent().round();
    let line_height = (ascent - descent + scaled.line_gap()).round().max(1.0);
    let mut glyphs = Vec::new();
    for c in charset() {
        let id = font.glyph_id(c);
        let advance = scaled.h_advance(id).round().clamp(0.0, 255.0) as u8;
        let glyph = id.with_scale_and_position(size, ab_glyph::point(0.0, ascent));
        let (picture, bearing) = match scaled.outline_glyph(glyph) {
            Some(outline) => {
                let b = outline.px_bounds();
                let (w, h) = (
                    (b.max.x - b.min.x).ceil() as usize,
                    (b.max.y - b.min.y).ceil() as usize,
                );
                let (w, h) = (w.clamp(1, 255), h.clamp(1, 255));
                let mut rgba = vec![0u8; w * h * 4];
                outline.draw(|x, y, cov| {
                    let (x, y) = (x as usize, y as usize);
                    if x < w && y < h {
                        let o = (y * w + x) * 4;
                        rgba[o] = 255;
                        rgba[o + 1] = 255;
                        rgba[o + 2] = 255;
                        rgba[o + 3] = (cov * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                    }
                });
                if rgba.chunks_exact(4).all(|p| p[3] == 0) {
                    (
                        Picture {
                            w: 0,
                            h: 0,
                            rgba: Vec::new(),
                        },
                        [0i8; 2],
                    )
                } else {
                    (
                        Picture {
                            w: w as u16,
                            h: h as u16,
                            rgba,
                        },
                        [
                            b.min.x.round().clamp(-128.0, 127.0) as i8,
                            b.min.y.round().clamp(-128.0, 127.0) as i8,
                        ],
                    )
                }
            }
            None => (
                Picture {
                    w: 0,
                    h: 0,
                    rgba: Vec::new(),
                },
                [0i8; 2],
            ),
        };
        glyphs.push((c, picture, bearing, advance));
    }
    Ok((line_height as u8, ascent as u8, glyphs))
}

/// Pack the pieces, the faces (face 0, the small font, first), and the icons into one
/// atlas: shelves of pictures sorted by height, in a texture 512 wide (1024 when 512 is
/// not enough), each picture a dot of space from the next.
fn assemble(
    pieces: &BTreeMap<String, (Picture, [u8; 4])>,
    faces: &[RasterFace],
    icons: &BTreeMap<String, Vec<u8>>,
) -> Result<Atlas> {
    // Everything to place: a name, the picture, and what to do with its cell.
    enum Slot {
        Piece(String, [u8; 4]),
        Glyph(u8, char, [i8; 2], u8),
        Icon(String),
    }
    let mut items: Vec<(Slot, Picture)> = Vec::new();
    for (name, (pic, inset)) in pieces {
        items.push((
            Slot::Piece(name.clone(), *inset),
            Picture {
                w: pic.w,
                h: pic.h,
                rgba: pic.rgba.clone(),
            },
        ));
    }
    // Face 0: the small font's cells, white with the dot as alpha, from the shared table.
    let mut small: Vec<RasterGlyph> = Vec::new();
    for (c, rows) in smallfont::GLYPHS {
        let (w, h) = (smallfont::GLYPH_W as usize, smallfont::GLYPH_ROWS);
        let mut rgba = vec![0u8; w * h * 4];
        for (y, row) in rows.iter().enumerate() {
            for x in 0..w {
                if row & (1 << (w - 1 - x)) != 0 {
                    let o = (y * w + x) * 4;
                    rgba[o..o + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        small.push((
            *c,
            Picture {
                w: w as u16,
                h: h as u16,
                rgba,
            },
            [0, 0],
            smallfont::ADVANCE as u8,
        ));
    }
    small.push((
        ' ',
        Picture {
            w: 0,
            h: 0,
            rgba: Vec::new(),
        },
        [0, 0],
        smallfont::ADVANCE as u8,
    ));
    let mut face_meta: Vec<(u8, u8, u8)> =
        vec![(0, smallfont::GLYPH_H as u8 + 2, smallfont::GLYPH_H as u8)];
    for (c, pic, bearing, advance) in small {
        items.push((Slot::Glyph(0, c, bearing, advance), pic));
    }
    for (id, line_height, ascent, glyphs) in faces {
        face_meta.push((*id, *line_height, *ascent));
        for (c, pic, bearing, advance) in glyphs {
            items.push((
                Slot::Glyph(*id, *c, *bearing, *advance),
                Picture {
                    w: pic.w,
                    h: pic.h,
                    rgba: pic.rgba.clone(),
                },
            ));
        }
    }
    for (key, rgba) in icons {
        items.push((
            Slot::Icon(key.clone()),
            Picture {
                w: atlas::ICON,
                h: atlas::ICON,
                rgba: rgba.clone(),
            },
        ));
    }
    // Shelves: tallest first; deterministic because the order is the sort's and the
    // sort is stable over a deterministic sequence.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse((items[*i].1.h, items[*i].1.w)));
    let place = |width: u16| -> (u16, Vec<(u16, u16)>) {
        let mut at: Vec<(u16, u16)> = vec![(0, 0); items.len()];
        let (mut x, mut y, mut shelf) = (1u16, 1u16, 0u16);
        for &i in &order {
            let p = &items[i].1;
            if p.w == 0 || p.h == 0 {
                continue;
            }
            if x + p.w + 1 > width {
                x = 1;
                y += shelf + 1;
                shelf = 0;
            }
            at[i] = (x, y);
            x += p.w + 1;
            shelf = shelf.max(p.h);
        }
        ((y + shelf + 1).div_ceil(4) * 4, at)
    };
    let (mut width, mut height, mut at) = (512u16, 0u16, Vec::new());
    for w in [512u16, 1024] {
        let (h, a) = place(w);
        width = w;
        height = h;
        at = a;
        if h <= atlas::MAX_SIDE {
            break;
        }
    }
    if height > atlas::MAX_SIDE {
        bail!(
            "the atlas does not fit in {0} × {0}: fewer or smaller pictures",
            atlas::MAX_SIDE
        );
    }
    let mut out = Atlas {
        w: width,
        h: height,
        texels: vec![0u8; width as usize * height as usize * 4],
        ..Default::default()
    };
    for (id, line_height, ascent) in face_meta {
        out.faces.insert(
            id,
            Face {
                line_height,
                ascent,
                glyphs: HashMap::new(),
            },
        );
    }
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut collide = |key: &str| -> Result<u32> {
        let h = atlas::hash(key);
        if let Some(other) = names.get(&h)
            && other != key
        {
            bail!("`{key}` and `{other}` hash alike; rename one");
        }
        names.insert(h, key.to_string());
        Ok(h)
    };
    for (i, (slot, pic)) in items.iter().enumerate() {
        let (x, y) = at[i];
        for row in 0..pic.h as usize {
            let src = &pic.rgba[row * pic.w as usize * 4..(row + 1) * pic.w as usize * 4];
            let o = ((y as usize + row) * width as usize + x as usize) * 4;
            out.texels[o..o + src.len()].copy_from_slice(src);
        }
        let cell = Cell {
            x,
            y,
            w: pic.w,
            h: pic.h,
        };
        match slot {
            Slot::Piece(name, inset) => {
                let h = collide(name)?;
                out.pieces.insert(
                    h,
                    Piece {
                        cell,
                        inset: *inset,
                    },
                );
            }
            Slot::Glyph(face, c, bearing, advance) => {
                out.faces
                    .get_mut(face)
                    .expect("the face was made")
                    .glyphs
                    .insert(
                        *c,
                        Glyph {
                            cell: if pic.w == 0 { Cell::default() } else { cell },
                            bearing: *bearing,
                            advance: *advance,
                        },
                    );
            }
            Slot::Icon(key) => {
                let h = collide(key)?;
                out.icons.insert(h, cell);
            }
        }
    }
    out.validate().map_err(|e| anyhow!("the atlas: {e}"))?;
    Ok(out)
}

pub fn write(bundle: &Bundle, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out.join("props"))?;
    // What was there and is no longer made is removed: the directory is the build's.
    for entry in walkdir::WalkDir::new(out).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(out).unwrap_or(entry.path());
            let rel = rel.to_string_lossy().replace('\\', "/");
            if !bundle.files.contains_key(&rel) {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    for (name, bytes) in &bundle.files {
        let path = out.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// The committed bundle against what the sources build: the same files, the same bytes.
fn compare(bundle: &Bundle, committed: &Path) -> Result<()> {
    let mut wrong: Vec<String> = Vec::new();
    for (name, bytes) in &bundle.files {
        match std::fs::read(committed.join(name)) {
            Ok(theirs) if theirs == *bytes => {}
            Ok(_) => wrong.push(format!("{name}: differs")),
            Err(_) => wrong.push(format!("{name}: missing")),
        }
    }
    for entry in walkdir::WalkDir::new(committed).into_iter().flatten() {
        if entry.file_type().is_file() {
            let rel = entry.path().strip_prefix(committed).unwrap_or(entry.path());
            let rel = rel.to_string_lossy().replace('\\', "/");
            if !bundle.files.contains_key(&rel) {
                wrong.push(format!("{rel}: not made by the sources"));
            }
        }
    }
    if !wrong.is_empty() {
        for w in &wrong {
            eprintln!("content: {w}");
        }
        bail!(
            "the committed bundle is not what the sources build ({} file(s)); run `gm-tools content build` and commit",
            wrong.len()
        );
    }
    Ok(())
}

fn print_summary(b: &Bundle) {
    let total: usize = b.files.values().map(Vec::len).sum();
    println!(
        "content: version {}, {} templates, {} materials, {} abilities, {} creatures, {} props, {} icons ({} baked), atlas {} x {} ({} bytes), bundle {} files, {} bytes",
        b.manifest.content_version,
        b.manifest.templates.len(),
        b.manifest.materials.len(),
        b.manifest.abilities.len(),
        b.manifest.creatures.len(),
        b.manifest.props.len(),
        b.atlas.icons.len(),
        b.baked.len(),
        b.atlas.w,
        b.atlas.h,
        b.manifest.atlas_bytes,
        b.files.len(),
        total
    );
}

fn report(b: &Bundle) {
    println!(
        "{:<20} {:<10} {:>9} {:>9} {:>8}  icon",
        "prop", "texture", "tris", "bytes", "reach"
    );
    for p in &b.manifest.props {
        let f = b.facts.iter().find(|(k, _)| k == &p.key).map(|(_, f)| f);
        println!(
            "{:<20} {:<10} {:>9} {:>9} {:>8.1}",
            p.key,
            f.map_or(String::new(), |f| format!(
                "{}x{}",
                f.texture[0], f.texture[1]
            )),
            p.triangles,
            p.bytes,
            f.map_or(0.0, |f| f.top)
        );
    }
    println!();
    println!(
        "{:<20} {:<8} {:<16} {:<16} held",
        "template", "kind", "model", "icon"
    );
    for t in &b.manifest.templates {
        println!(
            "{:<20} {:<8} {:<16} {:<16} {}{}",
            t.key,
            t.kind,
            t.model.as_deref().unwrap_or("-"),
            t.icon.as_deref().map_or("- (glyph)".to_string(), |i| {
                if b.baked.iter().any(|k| k == i) {
                    format!("{i} (baked)")
                } else {
                    i.to_string()
                }
            }),
            match t.held {
                HELD_LEFT => "left",
                HELD_BACK => "back",
                _ => "right",
            },
            if t.retired { "  retired" } else { "" }
        );
    }
    println!();
    println!("{:<20} {:<16} {:<16} sound", "ability", "icon", "prop");
    for a in &b.manifest.abilities {
        println!(
            "{:<20} {:<16} {:<16} {}",
            a.key,
            a.icon.as_deref().unwrap_or("- (glyph)"),
            a.prop.as_deref().unwrap_or("-"),
            a.sound.as_deref().unwrap_or("- (by verb)")
        );
    }
    println!();
    let mut faces: Vec<_> = b.atlas.faces.iter().collect();
    faces.sort_by_key(|(id, _)| **id);
    for (id, f) in faces {
        let widest = f.glyphs.values().map(|g| g.cell.w).max().unwrap_or(0);
        let tallest = f.glyphs.values().map(|g| g.cell.h).max().unwrap_or(0);
        println!(
            "face {id}: line {} ascent {} glyphs {} (widest {widest}, tallest {tallest}, 'M' advance {})",
            f.line_height,
            f.ascent,
            f.glyphs.len(),
            f.glyphs.get(&'M').map_or(0, |g| g.advance)
        );
    }
    println!(
        "atlas {} x {}: {} pieces, {} faces, {} icons; statuses with pictures: {}; portraits: {}",
        b.atlas.w,
        b.atlas.h,
        b.atlas.pieces.len(),
        b.atlas.faces.len(),
        b.atlas.icons.len(),
        b.manifest.statuses.len(),
        b.manifest.portraits.len()
    );
    print_summary(b);
}
