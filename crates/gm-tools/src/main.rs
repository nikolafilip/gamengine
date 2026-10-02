//! gm-tools: the asset pipeline CLI (PLAN.md 11.5) and the CI budget gates (2.6, 2.7).
#![forbid(unsafe_code)]

mod arena;
mod budget;
mod dungeon;
mod glb;
mod hubcli;
mod mapbuild;
mod mapgen;
mod model;
mod town;
mod wad;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "gm-tools",
    version,
    about = "gamengine asset, map and budget tooling"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Texture WADs and palettes.
    Wad {
        #[command(subcommand)]
        cmd: WadCmd,
    },
    /// Map compilation (ericw-tools wrapper) and inspection.
    Map {
        #[command(subcommand)]
        cmd: MapCmd,
    },
    /// Asset budgets (budgets.toml).
    Budget {
        #[command(subcommand)]
        cmd: BudgetCmd,
    },
    /// Avatar models (docs/MODELS.md): check, template, generate, upload, wear.
    Model {
        #[command(subcommand)]
        cmd: ModelCmd,
    },
    /// Moderation (docs/MODELS.md 10): the queue, decisions, takedowns, privileges.
    Mod {
        #[command(subcommand)]
        cmd: hubcli::ModCmd,
    },
    /// Hub accounts and test setup.
    Hub {
        #[command(subcommand)]
        cmd: hubcli::HubCmd,
    },
}

#[derive(Subcommand)]
enum ModelCmd {
    /// Run the hub's ingestion on a local .glb: every violation, or the .gmm and its preview.
    Ingest {
        file: PathBuf,
        /// The archetype frame the model is for: colossus, striker, caster, infiltrator.
        #[arg(long)]
        frame: String,
        /// Where to write <name>.gmm and <name>.preview.png (default: next to the file).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Write the frame's mannequin on the standard rig as a .glb to start from.
    Template {
        #[arg(long)]
        frame: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Generate distinct avatars at the budget ceiling (tests, benchmarks, the acceptance run).
    Synth {
        #[arg(long, default_value_t = 100)]
        count: u32,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Side of the painted atlas.
        #[arg(long, default_value_t = 1024)]
        side: u32,
        /// Frames to cycle through (default: all four).
        #[arg(long, value_delimiter = ',')]
        frames: Vec<String>,
        /// Also write each avatar's ingested .gmm.
        #[arg(long)]
        ingest: bool,
    },
    #[command(flatten)]
    Hub(hubcli::HubModelCmd),
}

#[derive(Subcommand)]
enum WadCmd {
    /// Generate the palette and the base texture WAD procedurally (deterministic output).
    Make {
        #[arg(long, default_value = "assets/textures/base.wad")]
        out: PathBuf,
        #[arg(long, default_value = "assets/textures/palette.lmp")]
        palette: PathBuf,
    },
    /// List the textures in a WAD.
    List { wad: PathBuf },
}

#[derive(Subcommand)]
enum MapCmd {
    /// Compile a .map with qbsp, vis and light into <out>/<name>.bsp plus <name>.lit.
    Build {
        map: PathBuf,
        #[arg(long, default_value = "assets/maps/built")]
        out: PathBuf,
        #[arg(long, default_value = "assets/textures")]
        wad_dir: PathBuf,
        /// Directory holding qbsp, vis and light. Default: $ERICW_TOOLS_DIR, then
        /// tools/ericw-tools, then PATH.
        #[arg(long)]
        tools_dir: Option<PathBuf>,
        /// Emit BSP2 (32-bit indices) instead of BSP29.
        #[arg(long)]
        bsp2: bool,
        /// Fast vis, for iteration only.
        #[arg(long)]
        fast: bool,
        /// Skip the light stage.
        #[arg(long)]
        no_light: bool,
    },
    /// Write the generated 8v8 arena source map.
    GenArena {
        #[arg(long, default_value = "assets/maps/src/arena.map")]
        out: PathBuf,
    },
    /// Write the generated town source map: the market square of Phase 6.
    GenTown {
        #[arg(long, default_value = "assets/maps/src/town.map")]
        out: PathBuf,
    },
    /// Write the generated tutorial dungeon source map (Phase 7).
    GenDungeon {
        #[arg(long, default_value = "assets/maps/src/dungeon.map")]
        out: PathBuf,
    },
    /// Print statistics about a compiled .bsp.
    Info { bsp: PathBuf },
}

#[derive(Subcommand)]
enum BudgetCmd {
    /// Check every asset under the given paths; exit 1 on any violation.
    Check {
        #[arg(default_value = "assets")]
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "budgets.toml")]
        budgets: PathBuf,
    },
    /// Prove the checker rejects an over-budget model and accepts an in-budget one.
    SelfTest {
        #[arg(long, default_value = "budgets.toml")]
        budgets: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Wad {
            cmd: WadCmd::Make { out, palette },
        } => wad::make(&out, &palette),
        Cmd::Wad {
            cmd: WadCmd::List { wad },
        } => wad::list(&wad),
        Cmd::Map {
            cmd:
                MapCmd::Build {
                    map,
                    out,
                    wad_dir,
                    tools_dir,
                    bsp2,
                    fast,
                    no_light,
                },
        } => {
            let tools = mapbuild::find_tools(tools_dir.as_deref())?;
            let opts = mapbuild::BuildOptions {
                out_dir: out,
                wad_dir,
                bsp2,
                fast,
                no_light,
            };
            let bsp = mapbuild::build(&map, &opts, &tools)?;
            mapbuild::info(&bsp)
        }
        Cmd::Map {
            cmd: MapCmd::Info { bsp },
        } => mapbuild::info(&bsp),
        Cmd::Map {
            cmd: MapCmd::GenArena { out },
        } => {
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = arena::generate();
            std::fs::write(&out, &text)?;
            println!("gen-arena: {} bytes -> {}", text.len(), out.display());
            Ok(())
        }
        Cmd::Map {
            cmd: MapCmd::GenTown { out },
        } => {
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = town::generate();
            std::fs::write(&out, &text)?;
            println!("gen-town: {} bytes -> {}", text.len(), out.display());
            Ok(())
        }
        Cmd::Map {
            cmd: MapCmd::GenDungeon { out },
        } => {
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = dungeon::generate();
            std::fs::write(&out, &text)?;
            println!("gen-dungeon: {} bytes -> {}", text.len(), out.display());
            Ok(())
        }
        Cmd::Budget {
            cmd: BudgetCmd::Check { paths, budgets },
        } => budget::check_cli(&paths, &budgets),
        Cmd::Budget {
            cmd: BudgetCmd::SelfTest { budgets },
        } => budget::self_test(&budgets),
        Cmd::Model {
            cmd: ModelCmd::Ingest { file, frame, out },
        } => model::ingest(&file, &frame, out.as_deref()),
        Cmd::Model {
            cmd: ModelCmd::Template { frame, out },
        } => model::template(&frame, out.as_deref()),
        Cmd::Model {
            cmd:
                ModelCmd::Synth {
                    count,
                    out,
                    seed,
                    side,
                    frames,
                    ingest,
                },
        } => model::synth_avatars(count, &out, seed, side, &frames, ingest),
        Cmd::Model {
            cmd: ModelCmd::Hub(cmd),
        } => hubcli::model(cmd),
        Cmd::Mod { cmd } => hubcli::moderate(cmd),
        Cmd::Hub { cmd } => hubcli::hub(cmd),
    }
}
