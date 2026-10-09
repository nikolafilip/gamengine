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
    /// Write a built atlas as a PNG to look at.
    Atlas {
        #[arg(default_value = "assets/content")]
        dir: PathBuf,
        /// Texels per dot: 1 to 4.
        #[arg(long, default_value_t = 1)]
        density: u8,
        #[arg(long)]
        out: PathBuf,
    },
    /// The fitting room (CONTENT.md 9): the mannequin holding a prop in every stance of
    /// the shared animation set, from the front and from its right, as one PNG.
    Look {
        /// The prop's key (`sword`, `staff`, ...).
        key: String,
        #[arg(long, default_value = "assets/content")]
        dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// In the left hand (LOOK.md 6.5: a shield), with the right one empty.
        #[arg(long)]
        left: bool,
    },
    /// Write one of the tool's own flat-coloured props as a .glb (CONTENT.md 7: the
    /// procedural source): sword, greatsword, shield, hammer, musket, pistol.
    Synth {
        /// `sword`, `greatsword`, `shield`, `hammer`, `musket` or `pistol`.
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
        ContentCmd::Atlas { dir, density, out } => {
            let bundle = build(&dir)?;
            let Some(a) = bundle.atlases.iter().find(|a| a.density == density) else {
                bail!("no atlas at {density} texels a dot");
            };
            std::fs::write(
                &out,
                gm_ingest::write::png(a.w as u32, a.h as u32, &a.texels),
            )?;
            println!("content: atlas {} x {} -> {}", a.w, a.h, out.display());
            Ok(())
        }
        ContentCmd::Look {
            key,
            dir,
            out,
            left,
        } => {
            let bundle = build(&dir)?;
            let Some(model) = bundle.props.get(&key) else {
                bail!(
                    "no prop `{key}`: {}",
                    bundle.props.keys().cloned().collect::<Vec<_>>().join(", ")
                );
            };
            let (w, h, rgba) = fitting_room(model, left);
            std::fs::write(&out, gm_ingest::write::png(w, h, &rgba))
                .with_context(|| format!("writing {}", out.display()))?;
            println!("content: {key} in the hand, {w} x {h} -> {}", out.display());
            Ok(())
        }
        ContentCmd::Synth { what, out } => {
            let glb = match what.as_str() {
                "sword" => gm_ingest::synth::sword_prop(0.85),
                "greatsword" => gm_ingest::synth::greatsword_prop(),
                "shield" => gm_ingest::synth::shield_prop(),
                "hammer" => gm_ingest::synth::hammer_prop(),
                "musket" => gm_ingest::synth::musket_prop(),
                "pistol" => gm_ingest::synth::pistol_prop(),
                other => {
                    bail!(
                        "`{other}` is not a prop the tool makes: sword, greatsword, shield, hammer, musket, pistol"
                    )
                }
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
    /// One atlas per density, thinnest first (LOOK.md 2.2).
    pub atlases: Vec<Atlas>,
    /// What was baked rather than given, by icon key.
    pub baked: Vec<String>,
    /// The props as ingested, by key (the fitting room draws them).
    pub props: BTreeMap<String, Model>,
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
    /// The face's height in dots, from the top of its ascenders to the bottom of its
    /// descenders.
    size: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PieceToml {
    /// The picture at one texel a dot; `<stem>@2x.png` (and 3x, 4x) beside it are the
    /// same picture drawn finer, used for the denser atlases when they exist.
    file: String,
    /// Nine-slice insets in dots: left, top, right, bottom.
    #[serde(default)]
    inset: [u8; 4],
}

/// The densities the bundle's atlases are made at (LOOK.md 2.2): the UI scales.
pub const DENSITIES: [u8; 4] = [1, 2, 3, 4];

/// `<stem>@<d>x.<ext>` beside `path`: the same picture at `d` texels a dot.
fn dense_path(path: &Path, density: u8) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
    path.with_file_name(format!("{stem}@{density}x.{ext}"))
}

/// A picture `k` times larger, every texel a square of `k`: what a density without a
/// picture of its own is drawn from.
fn enlarge(pic: &Picture, k: usize) -> Picture {
    let (w, h) = (pic.w as usize, pic.h as usize);
    let mut rgba = vec![0u8; w * k * h * k * 4];
    for y in 0..h * k {
        for x in 0..w * k {
            let src = ((y / k) * w + x / k) * 4;
            let dst = (y * w * k + x) * 4;
            rgba[dst..dst + 4].copy_from_slice(&pic.rgba[src..src + 4]);
        }
    }
    Picture {
        w: (w * k) as u16,
        h: (h * k) as u16,
        rgba,
    }
}

/// The picture at `path` for a density: its own file when there is one (it must be
/// exactly `density` times the base picture), else the base enlarged.
fn dense_picture(path: &Path, base: &Picture, density: u8) -> Result<Picture> {
    if density <= 1 {
        return Ok(enlarge(base, 1));
    }
    let dense = dense_path(path, density);
    if !dense.is_file() {
        return Ok(enlarge(base, density as usize));
    }
    let img = image::load_from_memory(&read(&dense)?)
        .with_context(|| format!("decoding {}", dense.display()))?
        .to_rgba8();
    let want = (
        base.w as u32 * density as u32,
        base.h as u32 * density as u32,
    );
    if (img.width(), img.height()) != want {
        bail!(
            "{}: {} × {}, but {density} times {} is {} × {}",
            dense.display(),
            img.width(),
            img.height(),
            path.display(),
            want.0,
            want.1
        );
    }
    Ok(Picture {
        w: img.width() as u16,
        h: img.height() as u16,
        rgba: img.into_raw(),
    })
}

/// Where an icon's picture comes from, at any density.
enum IconSource {
    /// A file of 32 × 32 (with its denser `@2x` ... beside it, when drawn).
    File(PathBuf),
    /// Baked from a prop, by key.
    Prop(String),
    /// Baked from a frame's mannequin in an armour's tint.
    Portrait(gm_core::vocab::ArchetypeFrame, [f32; 3]),
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

/// A glyph as rasterised: its char, picture, bearing (texels) and advance (quarter dots).
type RasterGlyph = (char, Picture, [i16; 2], u8);
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
    // Where each icon's picture comes from, by icon key.
    let mut icons: BTreeMap<String, IconSource> = BTreeMap::new();
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
        icons: &mut BTreeMap<String, IconSource>,
        problems: &mut Vec<String>,
    ) -> Option<String> {
        if path.is_file() {
            match read_icon(&path) {
                Ok(_) => {
                    icons.insert(key.to_string(), IconSource::File(path));
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
            None => match t.model.as_ref().filter(|m| props.contains_key(*m)) {
                Some(model) => {
                    let key = format!("item/{}", t.key);
                    icons.insert(key.clone(), IconSource::Prop(model.clone()));
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
                icons.insert(key.clone(), IconSource::Portrait(frame, *tint));
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
    let mut pieces: BTreeMap<String, (PathBuf, Picture, [u8; 4])> = BTreeMap::new();
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
        if p.inset
            .iter()
            .any(|v| *v as u32 * atlas::MAX_DENSITY as u32 > 255)
        {
            problems.push(format!(
                "skin piece `{name}`: an inset over {} dots",
                255 / atlas::MAX_DENSITY
            ));
            continue;
        }
        pieces.insert(
            name.to_string(),
            (
                path,
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
    // The faces' files and sizes; rasterised once per density below.
    let mut fonts: Vec<(u8, &str, Vec<u8>, f32)> = Vec::new();
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
        match read(&path).and_then(|bytes| rasterise(&bytes, f.size, 1).map(|_| bytes)) {
            Ok(bytes) => fonts.push((*id, name, bytes, f.size)),
            Err(e) => problems.push(format!("font `{name}`: {e:#}")),
        }
    }

    if !problems.is_empty() {
        for p in &problems {
            eprintln!("content: {p}");
        }
        bail!("{} problem(s) in {}", problems.len(), dir.display());
    }

    // The atlases, one per density: everything drawn at that many texels a dot and packed.
    let portraits: BTreeMap<String, Model> = icons
        .iter()
        .filter_map(|(key, src)| match src {
            IconSource::Portrait(frame, tint) => {
                Some((key.clone(), mannequin_model(*frame, *tint)))
            }
            _ => None,
        })
        .collect();
    let mut atlases = Vec::new();
    for density in DENSITIES {
        let side = atlas::ICON as usize * density as usize;
        let mut dense_pieces: BTreeMap<String, (Picture, [u8; 4])> = BTreeMap::new();
        for (name, (path, base, inset)) in &pieces {
            let pic = dense_picture(path, base, density)
                .with_context(|| format!("skin piece `{name}`"))?;
            dense_pieces.insert(name.clone(), (pic, inset.map(|v| v * density)));
        }
        let mut faces: Vec<RasterFace> = Vec::new();
        for (id, name, bytes, size) in &fonts {
            let (line_height, ascent, glyphs) =
                rasterise(bytes, *size, density).with_context(|| format!("font `{name}`"))?;
            faces.push((*id, line_height, ascent, glyphs));
        }
        let mut dense_icons: BTreeMap<String, Picture> = BTreeMap::new();
        for (key, src) in &icons {
            let rgba = match src {
                IconSource::File(path) => {
                    let base = Picture {
                        w: atlas::ICON,
                        h: atlas::ICON,
                        rgba: read_icon(path)?,
                    };
                    dense_picture(path, &base, density)
                        .with_context(|| format!("icon `{key}`"))?
                        .rgba
                }
                IconSource::Prop(model) => gm_ingest::raster::icon(&props[model], side),
                IconSource::Portrait(..) => gm_ingest::raster::portrait(&portraits[key], side),
            };
            dense_icons.insert(
                key.clone(),
                Picture {
                    w: side as u16,
                    h: side as u16,
                    rgba,
                },
            );
        }
        let atlas = assemble(&dense_pieces, &faces, &dense_icons, density)
            .with_context(|| format!("the atlas at {density} texels a dot"))?;
        let gma = atlas.encode().map_err(|e| anyhow!("the atlas: {e}"))?;
        let file = atlas::file_name(density);
        manifest.atlases.push(gm_model::manifest::AtlasEntry {
            density,
            file: file.clone(),
            sha256: sha(&gma),
            bytes: gma.len() as u32,
        });
        files.insert(file, gma);
        atlases.push(atlas);
    }
    files.insert("manifest.gmc".into(), manifest.encode());
    Ok(Bundle {
        files,
        manifest,
        atlases,
        baked,
        props,
        facts,
    })
}

/// The stances of the fitting room: the snapshot's state, the moment it is drawn at, and
/// whether the body crouches (MODES.md 3.5: the last two columns, still and creeping).
const STANCES: [(u8, f32, f32, bool); 11] = [
    (gm_core::sim::anim::IDLE, 0.0, 0.0, false),
    (gm_core::sim::anim::RUN, 1.0, 1.2, false),
    (gm_core::sim::anim::WINDUP, 0.3, 0.0, false),
    (gm_core::sim::anim::SWING, 0.3, 0.0, false),
    (gm_core::sim::anim::RECOVER, 0.3, 0.0, false),
    (gm_core::sim::anim::GUARD, 0.3, 0.0, false),
    (gm_core::sim::anim::PARRY, 0.3, 0.0, false),
    (gm_core::sim::anim::CAST, 0.3, 0.0, false),
    (gm_core::sim::anim::DASH, 0.3, 0.0, false),
    (gm_core::sim::anim::IDLE, 0.0, 0.0, true),
    (gm_core::sim::anim::RUN, 1.0, 1.2, true),
];

/// The striker's mannequin holding `prop` in every stance, seen from the front (top row)
/// and from its right (bottom row): RGBA, its width and height.
fn fitting_room(prop: &Model, left: bool) -> (u32, u32, Vec<u8>) {
    use gm_ingest::raster::{self, Dir, ModelSoup, Soup, Window};
    let frame = gm_core::vocab::ArchetypeFrame::Striker;
    let mesh = mannequin::build(frame, &mannequin::Shape::MANNEQUIN);
    let held = ModelSoup::new(prop);
    let (cw, ch) = (220usize, 275usize);
    let window = Window {
        x0: -44.0,
        x1: 44.0,
        z0: -6.0,
        z1: 104.0,
        width: cw,
        height: ch,
    };
    let (w, h) = (cw * STANCES.len(), ch * 2);
    let mut out = vec![0u8; w * h * 4];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&[58, 62, 70, 255]);
    }
    let cut = prop.cutout();
    for (col, (state, t, cycle, crouched)) in STANCES.iter().enumerate() {
        let pose = gm_model::anim::pose(&gm_model::anim::AnimInput {
            state: *state,
            t: *t,
            cycle: *cycle,
            speed: gm_model::anim::FULL_SPEED,
            pitch: 0.0,
            hips_z: mesh.pivots[rig::bone::HIPS].z,
            weight: 0.0,
            crouched: *crouched,
        });
        let skin = gm_model::skin_matrices(&mesh.pivots, mesh.bone_mask, &pose);
        let body: Vec<glam::Vec3> = (0..mesh.positions.len())
            .map(|i| {
                let mut p = glam::Vec3::ZERO;
                for k in 0..4 {
                    p += skin[mesh.joints[i][k] as usize].transform_point3(mesh.positions[i])
                        * mesh.weights[i][k];
                }
                p
            })
            .collect();
        let body_normals: Vec<glam::Vec3> = (0..mesh.normals.len())
            .map(|i| skin[mesh.joints[i][0] as usize].transform_vector3(mesh.normals[i]))
            .collect();
        let attach = if left {
            gm_model::pose::prop_attach_left(&mesh.pivots, &skin)
        } else {
            gm_model::pose::prop_attach(&mesh.pivots, &skin)
        };
        let prop_at: Vec<glam::Vec3> = held
            .positions
            .iter()
            .map(|p| attach.transform_point3(*p))
            .collect();
        let prop_normals: Vec<glam::Vec3> = held
            .normals
            .iter()
            .map(|n| attach.transform_vector3(*n))
            .collect();
        for (row, dir) in [Dir::Front, Dir::Right].into_iter().enumerate() {
            // Lit from the camera's side and above, so neither view is in its own shade.
            let (right, toward) = match dir {
                Dir::Front => (glam::Vec3::Y, glam::Vec3::X),
                _ => (glam::Vec3::X, glam::Vec3::NEG_Y),
            };
            let light = (toward * 0.6 + glam::Vec3::Z * 0.7 - right * 0.3).normalize();
            let a = raster::draw_axes(
                &Soup {
                    positions: &body,
                    normals: &body_normals,
                    uvs: &mesh.uvs,
                    indices: &mesh.indices,
                    two_sided: false,
                },
                right,
                glam::Vec3::Z,
                toward,
                light,
                window,
                &|_, _| true,
            );
            let b = raster::draw_axes(
                &Soup {
                    positions: &prop_at,
                    normals: &prop_normals,
                    uvs: &held.uvs,
                    indices: &held.indices,
                    two_sided: true,
                },
                right,
                glam::Vec3::Z,
                toward,
                light,
                window,
                &|u, v| !cut || raster::sample(prop, u, v)[3] >= 128,
            );
            for y in 0..ch {
                for x in 0..cw {
                    let at = y * cw + x;
                    let o = ((row * ch + y) * w + col * cw + x) * 4;
                    let px = if b.depth[at] > f32::MIN && b.depth[at] >= a.depth[at] {
                        let [u, v, shade] = b.frag[at];
                        let c = raster::sample(prop, u, v);
                        [0, 1, 2].map(|k| (c[k] as f32 * shade) as u8)
                    } else if a.depth[at] > f32::MIN {
                        let shade = a.frag[at][2];
                        [150.0, 152.0, 160.0].map(|c: f32| (c * shade) as u8)
                    } else if (window.z1 - (y as f32 + 0.5) * (window.z1 - window.z0) / ch as f32)
                        < 0.0
                    {
                        [44, 46, 52]
                    } else {
                        continue;
                    };
                    out[o..o + 3].copy_from_slice(&px);
                }
            }
        }
    }
    (w as u32, h as u32, out)
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

/// Rasterise a face at `size` dots per em for an atlas of `density` texels a dot: the line
/// height and the ascent in dots, and every glyph of the charset as a picture with its
/// bearing in texels and its advance in quarter dots (LOOK.md 2.3).
///
/// The metrics are the same at every density, so a layout is: the ascent is the height of
/// the face's capitals (the baseline lies that far under the line's top; an accent stands
/// over it), the line is that and the descent, and an advance is the outline's own, to the
/// nearest quarter of a dot. Only the pictures are drawn finer.
fn rasterise(bytes: &[u8], size: f32, density: u8) -> Result<(u8, u8, Vec<RasterGlyph>)> {
    if !(4.0..=64.0).contains(&size) {
        bail!("a face is 4 to 64 dots, not {size}");
    }
    let font = ab_glyph::FontRef::try_from_slice(bytes).context("not a font file")?;
    let dots = font.as_scaled(ab_glyph::PxScale::from(size));
    let capitals = dots
        .outline_glyph(font.glyph_id('H').with_scale(size))
        .map(|o| -o.px_bounds().min.y)
        .filter(|h| *h >= 1.0)
        .unwrap_or(dots.ascent() * 0.72);
    let ascent = capitals.round().clamp(1.0, 255.0);
    let descent = (-dots.descent()).round().max(0.0);
    let line_height = (ascent + descent).clamp(1.0, 255.0);
    let k = density.max(1) as f32;
    let fine = font.as_scaled(ab_glyph::PxScale::from(size * k));
    let mut glyphs = Vec::new();
    for c in charset() {
        let id = font.glyph_id(c);
        let advance = (dots.h_advance(id) * atlas::ADVANCE_PARTS)
            .round()
            .clamp(0.0, 255.0) as u8;
        let glyph = id.with_scale_and_position(size * k, ab_glyph::point(0.0, ascent * k));
        let nothing = || {
            (
                Picture {
                    w: 0,
                    h: 0,
                    rgba: Vec::new(),
                },
                [0i16; 2],
            )
        };
        let (picture, bearing) = match fine.outline_glyph(glyph) {
            Some(outline) => {
                let b = outline.px_bounds();
                let (w, h) = (
                    (b.max.x - b.min.x).ceil() as usize,
                    (b.max.y - b.min.y).ceil() as usize,
                );
                if w > 255 || h > 255 {
                    bail!("{c:?} is {w} × {h} texels at density {density}: a smaller face");
                }
                let (w, h) = (w.max(1), h.max(1));
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
                    nothing()
                } else {
                    (
                        Picture {
                            w: w as u16,
                            h: h as u16,
                            rgba,
                        },
                        [b.min.x.round() as i16, b.min.y.round() as i16],
                    )
                }
            }
            None => nothing(),
        };
        glyphs.push((c, picture, bearing, advance));
    }
    Ok((line_height as u8, ascent as u8, glyphs))
}

/// Pack the pieces, the faces (face 0, the small font, first), and the icons into one
/// atlas of `density` texels a dot: shelves of pictures sorted by height, in a texture 512
/// wide (1024 or 2048 when that is not enough), each picture a texel of space from the
/// next.
fn assemble(
    pieces: &BTreeMap<String, (Picture, [u8; 4])>,
    faces: &[RasterFace],
    icons: &BTreeMap<String, Picture>,
    density: u8,
) -> Result<Atlas> {
    // Everything to place: a name, the picture, and what to do with its cell.
    enum Slot {
        Piece(String, [u8; 4]),
        Glyph(u8, char, [i16; 2], u8),
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
    // Face 0: the small font's cells, white with the dot as alpha, from the shared table;
    // a dot is a square of the density.
    let advance = (smallfont::ADVANCE * atlas::ADVANCE_PARTS) as u8;
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
        let dots = Picture {
            w: w as u16,
            h: h as u16,
            rgba,
        };
        small.push((*c, enlarge(&dots, density.max(1) as usize), [0, 0], advance));
    }
    small.push((
        ' ',
        Picture {
            w: 0,
            h: 0,
            rgba: Vec::new(),
        },
        [0, 0],
        advance,
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
    for (key, pic) in icons {
        items.push((
            Slot::Icon(key.clone()),
            Picture {
                w: pic.w,
                h: pic.h,
                rgba: pic.rgba.clone(),
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
    for w in [512u16, 1024, 2048] {
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
        density: density.max(1),
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
        "content: version {}, {} templates, {} materials, {} abilities, {} creatures, {} props, {} icons ({} baked), atlases {}, bundle {} files, {} bytes",
        b.manifest.content_version,
        b.manifest.templates.len(),
        b.manifest.materials.len(),
        b.manifest.abilities.len(),
        b.manifest.creatures.len(),
        b.manifest.props.len(),
        b.atlases[0].icons.len(),
        b.baked.len(),
        b.atlases
            .iter()
            .zip(&b.manifest.atlases)
            .map(|(a, e)| format!("{}x: {} x {} ({} bytes)", a.density, a.w, a.h, e.bytes))
            .collect::<Vec<_>>()
            .join(", "),
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
    let thin = &b.atlases[0];
    let mut faces: Vec<_> = thin.faces.iter().collect();
    faces.sort_by_key(|(id, _)| **id);
    for (id, f) in faces {
        let widest = f.glyphs.values().map(|g| g.cell.w).max().unwrap_or(0);
        let tallest = f.glyphs.values().map(|g| g.cell.h).max().unwrap_or(0);
        println!(
            "face {id}: line {} ascent {} glyphs {} (widest {widest}, tallest {tallest}, 'M' advance {})",
            f.line_height,
            f.ascent,
            f.glyphs.len(),
            f.glyphs
                .get(&'M')
                .map_or(0.0, |g| g.advance as f32 / atlas::ADVANCE_PARTS)
        );
    }
    println!(
        "atlas {} x {}: {} pieces, {} faces, {} icons; statuses with pictures: {}; portraits: {}",
        thin.w,
        thin.h,
        thin.pieces.len(),
        thin.faces.len(),
        thin.icons.len(),
        b.manifest.statuses.len(),
        b.manifest.portraits.len()
    );
    print_summary(b);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")
    }

    /// LOOK.md 2.2: an atlas per density, and nothing a layout counts with differs
    /// between them: the same lines, the same advances, the same things, each drawn
    /// with that many texels a dot.
    #[test]
    fn the_atlases_differ_in_texels_and_in_nothing_a_layout_counts() {
        let bundle = build(&sources()).expect("the content builds");
        assert_eq!(
            bundle.atlases.iter().map(|a| a.density).collect::<Vec<_>>(),
            DENSITIES
        );
        let thin = &bundle.atlases[0];
        for a in &bundle.atlases {
            let d = a.density as u16;
            assert_eq!(
                bundle.files[&atlas::file_name(a.density)],
                a.encode().unwrap()
            );
            assert_eq!(a.faces.len(), thin.faces.len());
            for (id, face) in &a.faces {
                let base = &thin.faces[id];
                assert_eq!(
                    (face.line_height, face.ascent, face.glyphs.len()),
                    (base.line_height, base.ascent, base.glyphs.len())
                );
                for (c, g) in &face.glyphs {
                    assert_eq!(g.advance, base.glyphs[c].advance, "face {id} {c:?}");
                }
            }
            // A capital of the text face is as tall as the face's ascent says, to a texel.
            let h = a.faces[&1].glyphs[&'H'];
            assert!(
                (h.cell.h as i32 - a.faces[&1].ascent as i32 * d as i32).abs() <= 1,
                "density {d}: H is {} texels, the ascent {} dots",
                h.cell.h,
                a.faces[&1].ascent
            );
            assert_eq!(a.icons.len(), thin.icons.len());
            for cell in a.icons.values() {
                assert_eq!((cell.w, cell.h), (atlas::ICON * d, atlas::ICON * d));
            }
            assert_eq!(a.pieces.len(), thin.pieces.len());
            for (key, piece) in &a.pieces {
                let base = &thin.pieces[key];
                assert_eq!(
                    (piece.cell.w, piece.cell.h),
                    (base.cell.w * d, base.cell.h * d)
                );
                assert_eq!(piece.inset, base.inset.map(|v| v * d as u8));
            }
        }
        // The manifest names each, and a client finds the one of its scale.
        assert_eq!(bundle.manifest.atlases.len(), DENSITIES.len());
        for scale in 1..=6u8 {
            let e = bundle
                .manifest
                .atlas(scale)
                .expect("an atlas for every scale");
            assert_eq!(e.density, scale.min(4));
            assert_eq!(e.sha256, sha(&bundle.files[&e.file]));
        }
    }

    /// A piece's finer picture is the piece's own size times the density, or refused; one
    /// that is missing is the first picture enlarged.
    #[test]
    fn a_denser_picture_must_be_the_same_picture_finer() {
        let base = Picture {
            w: 2,
            h: 1,
            rgba: vec![1, 2, 3, 255, 9, 8, 7, 255],
        };
        let big = enlarge(&base, 2);
        assert_eq!((big.w, big.h), (4, 2));
        assert_eq!(&big.rgba[..8], &[1, 2, 3, 255, 1, 2, 3, 255]);
        assert_eq!(&big.rgba[24..], &[9, 8, 7, 255, 9, 8, 7, 255]);
        let dir = sources().join("ui");
        let panel = dir.join("panel.png");
        assert_eq!(dense_path(&panel, 3), dir.join("panel@3x.png"));
        // The panel has finer pictures of its own; asked for against a base of another
        // size they are refused.
        assert!(dense_picture(&panel, &base, 2).is_err());
        let missing = dir.join("no_such_piece.png");
        assert_eq!(dense_picture(&missing, &base, 3).unwrap().w, 6);
    }

    /// LOOK.md 6.3: the fist holds a blade across the forearm, and what a row's fit lays
    /// along the arm points where the arm does.
    #[test]
    fn a_blade_stands_across_the_arm_and_a_crossbow_along_it() {
        use gm_core::sim::anim;
        let bundle = build(&sources()).expect("the content builds");
        let frame = gm_core::vocab::ArchetypeFrame::Striker;
        let pivots = rig::rest_pivots(frame);
        // The far end of a prop: its vertex furthest from the grip.
        let tip = |key: &str| {
            let m = &bundle.props[key];
            (0..m.vertices.len())
                .map(|i| m.position(i))
                .max_by(|a, b| a.length().partial_cmp(&b.length()).unwrap())
                .unwrap()
                .normalize()
        };
        let aimed = |key: &str, state: u8| {
            let pose = gm_model::anim::pose(&gm_model::anim::AnimInput {
                state,
                t: 0.0,
                hips_z: 29.0,
                ..Default::default()
            });
            let skin = gm_model::skin_matrices(&pivots, rig::ALL_BONES, &pose);
            let attach = gm_model::pose::prop_attach(&pivots, &skin);
            let arm = (skin[rig::bone::HAND_R].transform_point3(pivots[rig::bone::HAND_R])
                - skin[rig::bone::FOREARM_R].transform_point3(pivots[rig::bone::FOREARM_R]))
            .normalize();
            (attach.transform_vector3(tip(key)).normalize(), arm)
        };
        // Standing: the sword's blade points ahead, nowhere near the line of the arm.
        let (blade, arm) = aimed("sword", anim::IDLE);
        assert!(blade.x > 0.85, "the blade points ahead: {blade:?}");
        assert!(
            blade.dot(arm).abs() < 0.45,
            "and across the arm: {}",
            blade.dot(arm)
        );
        // Casting: the staff is raised, the crossbow levelled along the arm.
        let (staff, _) = aimed("staff", anim::CAST);
        assert!(staff.z > 0.7, "the staff stands up: {staff:?}");
        let (bow, arm) = aimed("crossbow", anim::CAST);
        assert!(
            bow.dot(arm) > 0.9 && bow.x > 0.9,
            "levelled: {bow:?} along {arm:?}"
        );
        let (bow, _) = aimed("crossbow", anim::IDLE);
        assert!(bow.z < -0.8, "carried muzzle down: {bow:?}");
    }
}
