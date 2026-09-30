//! `gm-tools map build`: the ericw-tools wrapper (qbsp -> vis -> light), and `map info`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub struct Tools {
    pub qbsp: PathBuf,
    pub vis: PathBuf,
    pub light: PathBuf,
}

fn exe(dir: &Path, name: &str) -> Option<PathBuf> {
    [dir.join(name), dir.join(format!("{name}.exe"))]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

fn tools_in(dir: &Path) -> Option<Tools> {
    Some(Tools {
        qbsp: exe(dir, "qbsp")?,
        vis: exe(dir, "vis")?,
        light: exe(dir, "light")?,
    })
}

/// Locate qbsp/vis/light: explicit dir, `$ERICW_TOOLS_DIR`, `tools/ericw-tools` (relative to the
/// working directory and to the workspace root), then `PATH`.
pub fn find_tools(explicit: Option<&Path>) -> Result<Tools> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = explicit {
        dirs.push(d.to_owned());
    }
    if let Ok(d) = std::env::var("ERICW_TOOLS_DIR") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("tools/ericw-tools"));
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/ericw-tools"));
    if let Ok(path) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    for d in &dirs {
        if let Some(t) = tools_in(d) {
            return Ok(t);
        }
    }
    bail!(
        "ericw-tools (qbsp, vis, light) not found. Run scripts/fetch-ericw-tools.sh, set \
         ERICW_TOOLS_DIR, or pass --tools-dir."
    )
}

pub struct BuildOptions {
    pub out_dir: PathBuf,
    pub wad_dir: PathBuf,
    pub bsp2: bool,
    pub fast: bool,
    pub no_light: bool,
}

fn run(exe: &Path, args: &[&str]) -> Result<()> {
    println!("+ {} {}", exe.display(), args.join(" "));
    let status = Command::new(exe)
        .args(args)
        .status()
        .with_context(|| format!("running {}", exe.display()))?;
    if !status.success() {
        bail!("{} failed with {status}", exe.display());
    }
    Ok(())
}

/// Compile `map` into `<out_dir>/<stem>.bsp` (+ `.lit`). Returns the BSP path.
pub fn build(map: &Path, opts: &BuildOptions, tools: &Tools) -> Result<PathBuf> {
    if !map.is_file() {
        bail!("map source {} does not exist", map.display());
    }
    fs::create_dir_all(&opts.out_dir)?;
    let stem = map.file_stem().context("map path has no file name")?;
    let bsp = opts.out_dir.join(stem).with_extension("bsp");
    let bsp_s = bsp.to_str().context("non-UTF-8 output path")?;
    let map_s = map.to_str().context("non-UTF-8 map path")?;
    let wad_s = opts.wad_dir.to_str().context("non-UTF-8 wad dir")?;

    let mut qbsp = vec!["-log", "0", "-nopercent", "-leaktest", "-wadpath", wad_s];
    if opts.bsp2 {
        qbsp.push("-bsp2");
    }
    qbsp.extend([map_s, bsp_s]);
    run(&tools.qbsp, &qbsp)?;

    let mut vis = vec!["-log", "0", "-nopercent"];
    if opts.fast {
        vis.push("-fast");
    }
    vis.push(bsp_s);
    run(&tools.vis, &vis)?;

    if !opts.no_light {
        run(
            &tools.light,
            &["-log", "0", "-nopercent", "-extra", "-lit", bsp_s],
        )?;
    }
    let size = fs::metadata(&bsp)?.len();
    let lit = bsp.with_extension("lit");
    let lit_size = fs::metadata(&lit).map(|m| m.len()).unwrap_or(0);
    println!(
        "map build: {} ({size} bytes), {} ({lit_size} bytes)",
        bsp.display(),
        lit.display()
    );
    Ok(bsp)
}

/// Print what the loader sees in a compiled map.
pub fn info(bsp_path: &Path) -> Result<()> {
    let bsp =
        gm_bsp::Bsp::load(bsp_path).with_context(|| format!("loading {}", bsp_path.display()))?;
    let world = bsp.models.first().context("BSP has no models")?;
    println!("{}: {:?}", bsp_path.display(), bsp.version);
    println!(
        "  planes {}  vertices {}  edges {}  faces {}  nodes {}  leaves {}  clipnodes {}  models {}",
        bsp.planes.len(),
        bsp.vertices.len(),
        bsp.edges.len(),
        bsp.faces.len(),
        bsp.nodes.len(),
        bsp.leaves.len(),
        bsp.clipnodes.len(),
        bsp.models.len()
    );
    println!("  world bounds {:?} .. {:?}", world.mins, world.maxs);
    println!(
        "  lighting {} bytes, lit {}, visdata {} bytes, entities {}",
        bsp.lighting.len(),
        if bsp.lit.is_some() { "yes" } else { "no" },
        bsp.visdata.len(),
        bsp.entities.len()
    );
    let lm = bsp.lightmap_stats();
    println!(
        "  lightmapped faces {} of {}, {} luxels, largest {}x{}",
        lm.lit_faces,
        bsp.faces.len(),
        lm.luxels,
        lm.max_width,
        lm.max_height
    );
    for t in &bsp.textures {
        println!(
            "  texture {:<16} {}x{} {}",
            t.name,
            t.width,
            t.height,
            if t.pixels.is_some() {
                "embedded"
            } else {
                "no data"
            }
        );
    }
    let mut classes: Vec<&str> = bsp.entities.iter().map(|e| e.classname()).collect();
    classes.sort_unstable();
    classes.dedup();
    for c in classes {
        let n = bsp.entities.iter().filter(|e| e.classname() == c).count();
        println!("  entity {c} x{n}");
    }
    Ok(())
}
