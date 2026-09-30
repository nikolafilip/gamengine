//! gm-tools: the asset pipeline CLI (PLAN.md 11.5) and the CI budget gates (2.6, 2.7).
#![forbid(unsafe_code)]

mod budget;
mod glb;
mod mapbuild;
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
        Cmd::Budget {
            cmd: BudgetCmd::Check { paths, budgets },
        } => budget::check_cli(&paths, &budgets),
        Cmd::Budget {
            cmd: BudgetCmd::SelfTest { budgets },
        } => budget::self_test(&budgets),
    }
}
