//! gm-client: the gamengine client (PLAN.md 11.1).
//!
//! Phase 1: load a compiled map, render it with lightmaps through a wgpu forward renderer, and
//! walk around with Quake movement predicted locally at 64 Hz with interpolated rendering.
#![forbid(unsafe_code)]
// The browser build leaves the native-only flags and reports unused.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

mod app;
mod avatars;
mod bag;
mod cache;
mod characters;
mod content;
mod font;
mod front;
#[cfg(not(target_arch = "wasm32"))]
mod headless;
mod hub;
mod hud;
mod menu;
mod net;
mod people;
#[cfg(not(target_arch = "wasm32"))]
mod playback;
mod render;
mod script;
mod settings;
mod sound;
mod stats;
mod tactical;
mod ui;
#[cfg(target_arch = "wasm32")]
mod web;
mod world;

use std::path::PathBuf;

pub type Error = Box<dyn std::error::Error>;

pub struct Options {
    pub map: PathBuf,
    pub palette: PathBuf,
    /// Zone address; `None` plays offline against the local simulation.
    pub connect: Option<std::net::SocketAddr>,
    /// DER certificate of the zone (written by `gm-server --cert-out`).
    pub cert: PathBuf,
    pub name: String,
    /// Preset build to ask the zone for (`None`: the zone's default).
    pub build: Option<String>,
    /// Team to ask for (0 = let the zone balance).
    pub team: u8,
    /// Start in the third-person viewport.
    pub third_person: bool,
    /// Play through the hub: log in, pick a character, get a ticket (HUB.md).
    pub hub: Option<std::net::SocketAddr>,
    pub hub_cert: PathBuf,
    pub user: String,
    pub password: String,
    pub register: bool,
    pub character: String,
    pub zone: String,
    /// Where `<map>.bsp` files live for zone changes.
    pub maps_dir: PathBuf,
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
    /// Exit after this many seconds (scripted runs); 0 = never.
    pub seconds: f32,
    /// An ingested model (`.gmm`) to wear offline (MODELS.md 12).
    pub avatar: Option<PathBuf>,
    /// `--prop KEY`: offline, the own body holds this prop of the bundle, and the crowd
    /// too (the fitting room of CONTENT.md 9; the armed town of LOOK.md 8).
    pub prop: Option<String>,
    /// Offline: this many bodies standing in front of the start.
    pub crowd: u32,
    /// Ingested models for the crowd to wear, cycled.
    pub crowd_dir: Option<PathBuf>,
    /// The model cache (MODELS.md 8): directory, disk cap and GPU cap in MiB.
    pub cache_dir: Option<PathBuf>,
    pub cache_mb: u64,
    pub vram_mb: u64,
    /// Offline: stand here instead of at the map's start: `(x, y, z, yaw)`.
    pub start: Option<[f32; 4]>,
    /// Start in the tactical viewport (COMPANIONS.md 6); Tab toggles it.
    pub tactical: bool,
    /// A scripted player instead of the keyboard (acceptance runs, WEB.md 8): `fight` walks
    /// at the nearest enemy and attacks.
    pub script: Option<String>,
    /// Print a line of statistics every second (`stats: ...`).
    pub report: bool,
    /// Watch a replay (ANTICHEAT.md 3.4) instead of playing: the file, whose eyes to begin
    /// in, and where to begin, seconds from its start.
    pub replay: Option<PathBuf>,
    pub follow: Option<String>,
    pub from: f32,
    /// The zone `T` asks to travel to; with `travel_after` seconds, asked once by itself.
    pub travel_to: Option<String>,
    pub travel_after: f32,
    /// The browser: where the hub's and a zone's web listeners are (WEB.md 2.3, 5), and
    /// where the page keeps maps and textures.
    pub hub_web: Option<gm_net::control::WebAddr>,
    pub connect_web: Option<gm_net::control::WebAddr>,
    pub assets: String,
    /// The settings file (CLIENT.md 8); `None`: the user's own.
    pub settings: Option<PathBuf>,
    /// A script that plays the person (CLIENT.md 9): its text.
    pub ui_script: Option<String>,
    /// Walk around the map offline without being asked (CLIENT.md 2).
    pub offline: bool,
    /// Render the sound into this WAV from the frame clock instead of a device
    /// (SOUND.md 7).
    pub sound_dump: Option<PathBuf>,
}

const USAGE: &str = "gm-client [--map PATH] [--palette PATH] [--connect ADDR --cert PATH [--name NAME] [--build NAME] [--team N]] \
[--third-person] [--tactical] [--bench N] [--no-vsync] [--present fifo|relaxed|mailbox|immediate] [--max-fps N] [--headless] [--software] \
[--size WxH] [--screenshot out.ppm] [--seconds N] [--avatar FILE.gmm] [--prop KEY] [--crowd N [--crowd-dir DIR]] \
[--cache-dir DIR] [--cache-mb N] [--vram-mb N] [--start X,Y,Z,YAW] [--script fight|walk] [--report] [--travel-to ZONE [--travel-after SECS]] \
[--sound-dump FILE.wav]\n\
       gm-client --replay FILE.gmr [--follow NAME] [--from SECS] [--maps-dir DIR] [--third-person] [--headless --screenshot out.ppm]\n\
       gm-client --hub ADDR --hub-cert PATH [--user EMAIL --password PW [--register] [--character NAME [--zone ID] [--build NAME]]] \
[--maps-dir DIR] [--third-person] [--settings FILE] [--ui-script FILE]\n\
       gm-client                         the screens, on the hub the settings name (docs/CLIENT.md); --offline walks the map instead";

impl Default for Options {
    fn default() -> Options {
        Options {
            map: PathBuf::from("assets/maps/built/test_room.bsp"),
            palette: PathBuf::from("assets/textures/palette.lmp"),
            connect: None,
            cert: PathBuf::from("zone-cert.der"),
            name: std::env::var("USER").unwrap_or_else(|_| "player".into()),
            build: None,
            team: 0,
            third_person: false,
            hub: None,
            hub_cert: PathBuf::from("hub-cert.der"),
            user: String::new(),
            password: String::new(),
            register: false,
            character: String::new(),
            zone: String::new(),
            maps_dir: PathBuf::from("assets/maps/built"),
            bench_frames: None,
            vsync: true,
            present: None,
            max_fps: 250,
            headless: false,
            software: false,
            width: 1280,
            height: 720,
            screenshot: None,
            seconds: 0.0,
            avatar: None,
            prop: None,
            crowd: 0,
            crowd_dir: None,
            cache_dir: None,
            cache_mb: 2048,
            vram_mb: 256,
            start: None,
            tactical: false,
            script: None,
            report: false,
            replay: None,
            follow: None,
            from: 0.0,
            travel_to: std::env::var("GM_TRAVEL_TO").ok(),
            travel_after: 0.0,
            hub_web: None,
            connect_web: None,
            assets: "assets".into(),
            settings: None,
            ui_script: None,
            offline: false,
            sound_dump: None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_args() -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--map" => o.map = PathBuf::from(value("--map")?),
            "--palette" => o.palette = PathBuf::from(value("--palette")?),
            "--connect" => {
                o.connect = Some(
                    value("--connect")?
                        .parse()
                        .map_err(|e| format!("--connect: {e}"))?,
                )
            }
            "--cert" => o.cert = PathBuf::from(value("--cert")?),
            "--name" => o.name = value("--name")?,
            "--build" => o.build = Some(value("--build")?),
            "--team" => {
                o.team = value("--team")?
                    .parse()
                    .map_err(|e| format!("--team: {e}"))?
            }
            "--third-person" => o.third_person = true,
            "--tactical" => o.tactical = true,
            "--hub" => o.hub = Some(value("--hub")?.parse().map_err(|e| format!("--hub: {e}"))?),
            "--hub-cert" => o.hub_cert = PathBuf::from(value("--hub-cert")?),
            "--user" => o.user = value("--user")?,
            "--password" => o.password = value("--password")?,
            "--register" => o.register = true,
            "--character" => o.character = value("--character")?,
            "--zone" => o.zone = value("--zone")?,
            "--maps-dir" => o.maps_dir = PathBuf::from(value("--maps-dir")?),
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
            "--seconds" => {
                o.seconds = value("--seconds")?
                    .parse()
                    .map_err(|e| format!("--seconds: {e}"))?
            }
            "--avatar" => o.avatar = Some(PathBuf::from(value("--avatar")?)),
            "--prop" => o.prop = Some(value("--prop")?.to_string()),
            "--crowd" => {
                o.crowd = value("--crowd")?
                    .parse()
                    .map_err(|e| format!("--crowd: {e}"))?
            }
            "--crowd-dir" => o.crowd_dir = Some(PathBuf::from(value("--crowd-dir")?)),
            "--cache-dir" => o.cache_dir = Some(PathBuf::from(value("--cache-dir")?)),
            "--cache-mb" => {
                o.cache_mb = value("--cache-mb")?
                    .parse()
                    .map_err(|e| format!("--cache-mb: {e}"))?
            }
            "--vram-mb" => {
                o.vram_mb = value("--vram-mb")?
                    .parse()
                    .map_err(|e| format!("--vram-mb: {e}"))?
            }
            "--script" => o.script = Some(value("--script")?),
            "--report" => o.report = true,
            "--replay" => o.replay = Some(PathBuf::from(value("--replay")?)),
            "--follow" => o.follow = Some(value("--follow")?),
            "--from" => {
                o.from = value("--from")?
                    .parse()
                    .map_err(|e| format!("--from: {e}"))?
            }
            "--settings" => o.settings = Some(PathBuf::from(value("--settings")?)),
            "--offline" => o.offline = true,
            "--sound-dump" => o.sound_dump = Some(PathBuf::from(value("--sound-dump")?)),
            "--ui-script" => {
                let path = value("--ui-script")?;
                o.ui_script = Some(
                    std::fs::read_to_string(&path)
                        .map_err(|e| format!("--ui-script {path}: {e}"))?,
                );
            }
            "--travel-to" => o.travel_to = Some(value("--travel-to")?),
            "--travel-after" => {
                o.travel_after = value("--travel-after")?
                    .parse()
                    .map_err(|e| format!("--travel-after: {e}"))?
            }
            "--start" => {
                let v = value("--start")?;
                let n: Vec<f32> = v
                    .split(',')
                    .map(|p| p.trim().parse::<f32>())
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("--start: {e}"))?;
                o.start = Some(<[f32; 4]>::try_from(n).map_err(|_| "--start expects X,Y,Z,YAW")?);
            }
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
    // Started from somewhere else than the build's directory (a menu entry, a file
    // manager): what the build ships is looked for beside the program (CLIENT.md 2).
    let root = install_root();
    for path in [&mut o.map, &mut o.palette, &mut o.maps_dir] {
        if path.is_relative() && !path.exists() {
            *path = root.join(&*path);
        }
    }
    Ok(o)
}

/// Where the build's files are: the first of the working directory, the program's own
/// directory and the two above it (a `target/release` build) that holds `assets/maps`.
#[cfg(not(target_arch = "wasm32"))]
pub fn install_root() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(|p| p.parent());
    let mut places = vec![PathBuf::from(".")];
    places.extend(
        exe_dir
            .into_iter()
            .flat_map(|d| d.ancestors().take(3))
            .map(PathBuf::from),
    );
    places
        .into_iter()
        .find(|p| p.join("assets").join("maps").is_dir())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(not(target_arch = "wasm32"))]
/// Minimal stderr logger: our crates at `GM_LOG` level (default info), wgpu/naga at warn.
/// A client started with nobody's terminal behind it also writes the run to a file beside
/// its settings (`client.log`): that is where its last words are.
struct Logger;

#[cfg(not(target_arch = "wasm32"))]
static LOG_FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

#[cfg(not(target_arch = "wasm32"))]
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
            if let Ok(mut file) = LOG_FILE.lock()
                && let Some(file) = file.as_mut()
            {
                use std::io::Write;
                let _ = writeln!(file, "[{}] {}: {}", r.level(), r.target(), r.args());
            }
        }
    }

    fn flush(&self) {}
}

#[cfg(not(target_arch = "wasm32"))]
static LOGGER: Logger = Logger;

/// The browser client (WEB.md 3): the loader put the options in `globalThis.gmOptions`;
/// everything else happens on the page's event loop.
#[cfg(target_arch = "wasm32")]
fn main() {
    static LOGGER: web::ConsoleLogger = web::ConsoleLogger;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    web::install_panic_hook();
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = app::run_web().await {
            log::error!("{e}");
            web::tell_page("error", &e);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
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
    // A person's run (no bench, no script) leaves a log beside the settings.
    let scripted = opts.headless
        || opts.bench_frames.is_some()
        || opts.seconds > 0.0
        || opts.script.is_some()
        || opts.ui_script.is_some();
    if !scripted
        && let Some(settings) = opts.settings.clone().or_else(settings::default_path)
        && let Some(dir) = settings.parent()
        && std::fs::create_dir_all(dir).is_ok()
        && let Ok(file) = std::fs::File::create(dir.join("client.log"))
    {
        *LOG_FILE.lock().unwrap() = Some(file);
    }
    let result = if opts.headless {
        headless::run(&opts)
    } else {
        app::run(opts)
    };
    if let Err(e) = result {
        log::error!("{e}");
        eprintln!("gm-client: {e}");
        std::process::exit(1);
    }
}
