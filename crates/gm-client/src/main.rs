//! gm-client: the gamengine client (PLAN.md 11.1).
//!
//! Phase 1: load a compiled map, render it with lightmaps through a wgpu forward renderer, and
//! walk around with Quake movement predicted locally at 64 Hz with interpolated rendering.
#![forbid(unsafe_code)]

mod app;
mod headless;
mod render;
mod stats;
mod world;

use std::path::PathBuf;

pub type Error = Box<dyn std::error::Error>;

pub struct Options {
    pub map: PathBuf,
    pub palette: PathBuf,
    pub bench_frames: Option<u32>,
    pub vsync: bool,
    /// Explicit present mode (overrides `vsync`): fifo, relaxed, mailbox, immediate.
    pub present: Option<wgpu::PresentMode>,
    /// CPU-side frame cap in normal play (0 = uncapped). Benchmarks are always uncapped.
    pub max_fps: u32,
    pub headless: bool,
    pub software: bool,
    pub width: u32,
    pub height: u32,
    pub screenshot: Option<PathBuf>,
}

const USAGE: &str = "gm-client [--map PATH] [--palette PATH] [--bench N] [--no-vsync] [--present fifo|relaxed|mailbox|immediate] \
[--max-fps N] [--headless] [--software] [--size WxH] [--screenshot out.ppm]";

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        map: PathBuf::from("assets/maps/built/test_room.bsp"),
        palette: PathBuf::from("assets/textures/palette.lmp"),
        bench_frames: None,
        vsync: true,
        present: None,
        max_fps: 250,
        headless: false,
        software: false,
        width: 1280,
        height: 720,
        screenshot: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--map" => o.map = PathBuf::from(value("--map")?),
            "--palette" => o.palette = PathBuf::from(value("--palette")?),
            "--bench" => {
                o.bench_frames = Some(
                    value("--bench")?
                        .parse()
                        .map_err(|e| format!("--bench: {e}"))?,
                )
            }
            "--no-vsync" => o.vsync = false,
            "--present" => {
                o.present = Some(match value("--present")?.as_str() {
                    "fifo" => wgpu::PresentMode::Fifo,
                    "relaxed" => wgpu::PresentMode::FifoRelaxed,
                    "mailbox" => wgpu::PresentMode::Mailbox,
                    "immediate" => wgpu::PresentMode::Immediate,
                    other => return Err(format!("--present: unknown mode {other}")),
                })
            }
            "--max-fps" => {
                o.max_fps = value("--max-fps")?
                    .parse()
                    .map_err(|e| format!("--max-fps: {e}"))?
            }
            "--headless" => o.headless = true,
            "--software" => o.software = true,
            "--screenshot" => o.screenshot = Some(PathBuf::from(value("--screenshot")?)),
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size expects WxH")?;
                o.width = w.parse().map_err(|e| format!("--size: {e}"))?;
                o.height = h.parse().map_err(|e| format!("--size: {e}"))?;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if o.headless && o.bench_frames.is_none() {
        o.bench_frames = Some(120);
    }
    Ok(o)
}

/// Minimal stderr logger: our crates at `GM_LOG` level (default info), wgpu/naga at warn.
struct Logger;

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        let noisy = m.target().starts_with("wgpu") || m.target().starts_with("naga");
        m.level()
            <= if noisy {
                log::Level::Warn
            } else {
                log::max_level().to_level().unwrap_or(log::Level::Info)
            }
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}: {}", r.level(), r.target(), r.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: Logger = Logger;

fn main() {
    let level = match std::env::var("GM_LOG").as_deref() {
        Ok("error") => log::LevelFilter::Error,
        Ok("warn") => log::LevelFilter::Warn,
        Ok("debug") => log::LevelFilter::Debug,
        Ok("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);

    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("gm-client: {e}");
            std::process::exit(2);
        }
    };
    let result = if opts.headless {
        headless::run(&opts)
    } else {
        app::run(opts)
    };
    if let Err(e) = result {
        eprintln!("gm-client: {e}");
        std::process::exit(1);
    }
}
