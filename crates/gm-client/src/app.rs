//! Windowed client: winit event loop, fixed 64 Hz simulation with interpolated rendering,
//! mouse look, PVS-culled drawing, optional benchmark mode. Offline it runs the local
//! simulation; with `--connect` it predicts the own entity, reconciles against the zone and
//! interpolates everyone else (PROTOCOL.md 7). Two viewports share the simulation
//! (VOCABULARY.md 9): first person aims from the eyes, third person aims the camera ray at a
//! world point and re-aims it from the eyes ("camera-to-muzzle re-aim").

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use web_time::Instant;

use glam::Vec3;
use gm_bsp::Bsp;
use gm_core::build::{ContentPack, Sheet};
use gm_core::collide::{Aabb, sweep_boxes};
use gm_core::movement::{MoveInput, MoveVars, PlayerState, player_move, yaw_vectors};
use gm_core::sim::{Input as SimInput, buttons, view_dir};
use gm_core::tick::TickRate;
use gm_core::trace::{CollisionWorld, Hull};
use gm_model::ModelId;
use gm_net::client::ClientState;
use gm_net::control::{
    BodyKind, BuildChoice, Control, EncounterState, Order, SquadEntry, StallEntry,
};
use gm_net::snapshot::{EntityKind, SpawnInfo};
use gm_net::transport::fnv1a64;
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::avatars::{Avatars, Body, OWN, stall_boxes, stall_keeper};
use crate::hub::{HubLogin, HubSession, ticket_addr};
use crate::hud::{self, Hud};
use crate::net::{NetClient, NetEvent, ZoneAddr};
use crate::render::{EntityDraw, Gpu, Renderer, view_proj};
use crate::stats::FrameStats;
#[cfg(not(target_arch = "wasm32"))]
use crate::stats::{print_bench, print_bench_avatars};
use crate::tactical::{self, Tactical, pick_body, pick_ground, project};
use crate::world::{self, WorldMesh};
use crate::{Error, Options};

/// Degrees per mouse count: Quake's `sensitivity 3` times `m_yaw 0.022`.
const SENSITIVITY: f32 = 0.066;
const MAX_STEPS_PER_FRAME: u32 = 8;
const BENCH_YAW_DEG_PER_S: f32 = 20.0;
/// With a crowd the bench camera swings across it instead of turning away from it.
const BENCH_CROWD_SWING_DEG: f32 = 22.0;
/// Third-person camera: behind, slightly right and above the eyes (VOCABULARY.md 9).
const CAMERA_BACK: f32 = 110.0;
const CAMERA_RIGHT: f32 = 24.0;
const CAMERA_UP: f32 = 12.0;
/// How far the camera ray is resolved for the re-aim.
const AIM_REACH: f32 = 4096.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Viewport {
    First,
    Third,
}

/// Offline simulation: the local player against the map, no server.
pub struct Sim {
    pub rate: TickRate,
    pub accumulator: f32,
    pub prev: PlayerState,
    pub curr: PlayerState,
    pub yaw: f32,
    pub pitch: f32,
    pub vars: MoveVars,
}

impl Sim {
    pub fn new(bsp: &Bsp) -> Sim {
        Sim::at(bsp, None)
    }

    /// Start at `start` (`x, y, z, yaw`), or at the map's start.
    pub fn at(bsp: &Bsp, start: Option<[f32; 4]>) -> Sim {
        let (origin, yaw) = match start {
            Some([x, y, z, yaw]) => (Vec3::new(x, y, z), yaw),
            None => bsp
                .player_start()
                .unwrap_or((Vec3::new(0.0, 0.0, 64.0), 0.0)),
        };
        let st = PlayerState::new(origin);
        Sim {
            rate: TickRate::COMBAT,
            accumulator: 0.0,
            prev: st,
            curr: st,
            yaw,
            pitch: 0.0,
            vars: MoveVars::QUAKE,
        }
    }

    /// Advance the simulation by `frame_dt` seconds of wall time in fixed ticks. Returns the
    /// number of ticks run.
    pub fn advance(&mut self, bsp: &Bsp, input: &MoveInput, frame_dt: f32) -> u32 {
        let dt = self.rate.dt();
        self.accumulator += frame_dt.min(0.25);
        let mut steps = 0;
        while self.accumulator >= dt && steps < MAX_STEPS_PER_FRAME {
            self.prev = self.curr;
            player_move(bsp, &self.vars, &mut self.curr, input, dt);
            self.accumulator -= dt;
            steps += 1;
        }
        if steps == MAX_STEPS_PER_FRAME {
            self.accumulator = 0.0;
        }
        steps
    }

    /// Eye position interpolated between the last two ticks.
    pub fn eye(&self) -> Vec3 {
        self.prev.origin.lerp(self.curr.origin, self.alpha())
            + Vec3::new(0.0, 0.0, self.curr.hull.eye_height())
    }

    pub fn origin(&self) -> Vec3 {
        self.prev.origin.lerp(self.curr.origin, self.alpha())
    }

    fn alpha(&self) -> f32 {
        (self.accumulator / self.rate.dt()).clamp(0.0, 1.0)
    }
}

/// Networked play: the zone connection plus the shared prediction state.
pub(crate) struct Online {
    net: NetClient,
    /// `Welcome` arrived; the client state is built when `Content` follows.
    welcome: Option<(u32, u16)>,
    client: Option<ClientState>,
    pack: Option<ContentPack>,
    team: u8,
    build_name: String,
    accumulator: f32,
    prev_origin: Vec3,
    curr_origin: Vec3,
    last_snapshot: Instant,
    rate: TickRate,
    /// Everyone in the zone: name, team, avatar model.
    names: HashMap<u32, (String, u8, Option<ModelId>)>,
    /// Who drives each body (COMPANIONS.md 2.1), as the zone announced it.
    kinds: HashMap<u32, BodyKind>,
    /// The own squad, in slot order, as the zone last told it.
    squad: Vec<SquadEntry>,
    /// Encounter, loot, trial and order messages: when, what, in which colour.
    messages: VecDeque<(Instant, String, [f32; 4])>,
    /// The market: open stalls and their keepers (ECONOMY.md 7).
    stalls: Vec<StallEntry>,
    kills: u32,
    deaths: u32,
    map_hash: u64,
    respec_note: String,
    /// The hub session when playing through the hub (HUB.md); logged out on exit.
    hub: Option<HubSession>,
    /// A travel ticket to act on: reconnect to another zone, reloading its map.
    pending_travel: Option<(String, ZoneAddr, Vec<u8>)>,
    zone_name: String,
    /// Events taken from the connection and not handled yet: while the zone's map is
    /// still on its way (a browser fetches it), everything after `Welcome` waits here.
    backlog: VecDeque<NetEvent>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    awaiting_map: bool,
}

/// How long a message stays on the HUD, and how many are kept.
const MESSAGE_SECS: f32 = 9.0;
const MESSAGES_KEPT: usize = 6;

impl Online {
    fn say(&mut self, text: String, colour: [f32; 4]) {
        log::info!("{text}");
        self.messages.push_back((Instant::now(), text, colour));
        while self.messages.len() > MESSAGES_KEPT {
            self.messages.pop_front();
        }
    }

    fn build_name_of(&self, pack: &ContentPack, build: &gm_core::build::Build) -> String {
        pack.builds
            .iter()
            .find(|b| &b.build == build)
            .map_or_else(|| "custom".to_string(), |b| b.name.clone())
    }
}

#[derive(Default)]
struct Input {
    keys: HashSet<KeyCode>,
    /// Keys pressed since the last simulation tick (consumed by the next tick).
    just_pressed: HashSet<KeyCode>,
    mouse: HashSet<MouseButton>,
    mouse_dx: f32,
    mouse_dy: f32,
    /// Clicks and wheel turns since the last frame (the tactical viewport reads them).
    clicks: Vec<MouseButton>,
    wheel: f32,
}

impl Input {
    fn down(&self, k: KeyCode) -> bool {
        self.keys.contains(&k)
    }

    fn axes(&self) -> (f32, f32) {
        let axis = |neg, pos| (self.down(pos) as i32 - self.down(neg) as i32) as f32;
        (
            axis(KeyCode::KeyS, KeyCode::KeyW) + axis(KeyCode::ArrowDown, KeyCode::ArrowUp),
            axis(KeyCode::KeyA, KeyCode::KeyD) + axis(KeyCode::ArrowLeft, KeyCode::ArrowRight),
        )
    }

    fn move_input(&self, yaw: f32) -> MoveInput {
        let (forward, side) = self.axes();
        MoveInput {
            yaw,
            forward,
            side,
            jump: self.down(KeyCode::Space),
        }
    }

    /// A frame in the command stance (COMPANIONS.md 5.1): the button, the facing, nothing
    /// else. The tactical viewport sends these.
    fn command_input(&mut self, yaw: f32, pitch: f32) -> SimInput {
        self.just_pressed.clear();
        SimInput {
            buttons: buttons::COMMAND,
            yaw,
            pitch,
            forward: 0.0,
            side: 0.0,
            ability: 0,
        }
    }

    /// Left click primary, right click secondary, Ctrl guard, Shift ability 1, keys 1–4 the
    /// actives, V the viewport switch. `yaw`/`pitch` are the aim angles (re-aimed in third
    /// person).
    fn sim_input(&mut self, yaw: f32, pitch: f32) -> SimInput {
        let (forward, side) = self.axes();
        let mut b = 0u16;
        if self.down(KeyCode::Space) {
            b |= buttons::JUMP;
        }
        if self.mouse.contains(&MouseButton::Left) {
            b |= buttons::PRIMARY;
        }
        if self.mouse.contains(&MouseButton::Right) {
            b |= buttons::SECONDARY;
        }
        // Guard is on C as well: a browser keeps Ctrl+W for itself (WEB.md 3.4).
        if self.down(KeyCode::ControlLeft)
            || self.down(KeyCode::ControlRight)
            || self.down(KeyCode::KeyC)
        {
            b |= buttons::GUARD;
        }
        if self.down(KeyCode::ShiftLeft) || self.down(KeyCode::ShiftRight) {
            b |= buttons::ABILITY1;
        }
        if self.just_pressed.contains(&KeyCode::KeyV) {
            b |= buttons::VIEWPORT;
        }
        let ability = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
        ]
        .iter()
        .position(|k| self.just_pressed.contains(k))
        .map_or(0, |i| i as u8 + 1);
        self.just_pressed.clear();
        SimInput {
            buttons: b,
            yaw,
            pitch,
            forward,
            side,
            ability,
        }
    }
}

struct Active {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// The format frames are drawn in: the surface's, or its sRGB twin as a view format
    /// where the surface itself is not sRGB (a WebGPU canvas).
    view_format: wgpu::TextureFormat,
    gpu: Gpu,
    renderer: Renderer,
    avatars: Avatars,
    /// The leaves the visible faces were last gathered from.
    drawn_from: Vec<usize>,
}

struct App {
    opts: Options,
    /// The window, from its creation: the renderer on it (`active`) may come later.
    window: Option<Arc<Window>>,
    bsp: Bsp,
    mesh: Option<WorldMesh>,
    active: Option<Active>,
    sim: Sim,
    online: Option<Online>,
    input: Input,
    viewport: Viewport,
    stats: FrameStats,
    last_frame: Instant,
    last_title: Instant,
    started: Instant,
    title_frame: usize,
    grabbed: bool,
    adapter_info: Option<wgpu::AdapterInfo>,
    faces_total: usize,
    exit_requested: bool,
    acquire_timeouts: u32,
    entities: Vec<EntityDraw>,
    /// Bodies to draw this frame (players, the own body in third person).
    bodies: Vec<Body>,
    /// Models the zone revoked since the last frame.
    revoked: Vec<ModelId>,
    bench_yaw0: f32,
    /// The aim resolved by the last third-person tick, for the HUD.
    aim: (f32, f32),
    /// A map to switch to before the next frame (a zone change).
    pending_map: Option<Bsp>,
    palette: world::Palette,
    /// The script's aim (`--script fight`): the nearest living enemy as of the last frame,
    /// its id, position and velocity, and when it was seen.
    script_target: Option<(u32, Vec3, Vec3, Instant)>,
    last_report: Instant,
    report_frame: usize,
    /// The browser: the GPU device and a fetched map arrive here from their tasks.
    #[cfg(target_arch = "wasm32")]
    pending_gpu: PendingSlot<(wgpu::Surface<'static>, Gpu)>,
    #[cfg(target_arch = "wasm32")]
    pending_fetch: PendingSlot<(Bsp, u64)>,
    /// Milliseconds from navigation to the first frame presented (WEB.md 9).
    #[cfg(target_arch = "wasm32")]
    first_frame_ms: f64,
    /// The tactical viewport (COMPANIONS.md 6): over either of the other two.
    tactical: Tactical,
    /// Leaves the world is drawn from in the tactical view (the squad's sight).
    tactical_leaves: Vec<usize>,
    /// Health bars to draw over bodies this frame: where, how full, the colour.
    bars: Vec<(Vec3, f32, [f32; 4])>,
    /// Per squad slot: the companion's health as last sent, and whether it lives.
    squad_view: Vec<(Option<u16>, bool)>,
    /// The creature being fought: name, health, maximum.
    target_view: Option<(String, u16, u16)>,
}

/// A value a browser task delivers to the frame that waits for it.
#[cfg(target_arch = "wasm32")]
type PendingSlot<T> = std::rc::Rc<std::cell::RefCell<Option<Result<T, String>>>>;

/// How the client reaches its zone: directly, or with a ticket from the hub.
struct Entry {
    zone: ZoneAddr,
    token: Vec<u8>,
    hub: Option<HubSession>,
    zone_name: String,
}

fn online(opts: &Options, sim: &Sim, map_hash: u64, entry: Entry) -> Result<Online, Error> {
    log::info!("connecting to the zone as {}", opts.name);
    Ok(Online {
        net: NetClient::connect(
            entry.zone,
            opts.name.clone(),
            opts.build.clone(),
            opts.team,
            entry.token,
        )?,
        welcome: None,
        client: None,
        pack: None,
        team: 0,
        build_name: String::new(),
        accumulator: 0.0,
        prev_origin: sim.curr.origin,
        curr_origin: sim.curr.origin,
        last_snapshot: Instant::now(),
        rate: TickRate::COMBAT,
        names: HashMap::new(),
        kinds: HashMap::new(),
        squad: Vec::new(),
        messages: VecDeque::new(),
        stalls: Vec::new(),
        kills: 0,
        deaths: 0,
        map_hash,
        respec_note: String::new(),
        hub: entry.hub,
        pending_travel: None,
        zone_name: entry.zone_name,
        backlog: VecDeque::new(),
        awaiting_map: false,
    })
}

fn app(opts: Options, bsp: Bsp, palette: world::Palette, sim: Sim, online: Option<Online>) -> App {
    let mesh = world::build(&bsp, &palette);
    let faces_total = mesh
        .face_ranges
        .iter()
        .filter(|r| r.index_count > 0)
        .count();
    let viewport = if opts.third_person {
        Viewport::Third
    } else {
        Viewport::First
    };
    let mut app = App {
        opts,
        window: None,
        bsp,
        mesh: Some(mesh),
        active: None,
        sim,
        online,
        input: Input::default(),
        viewport,
        stats: FrameStats::new(),
        last_frame: Instant::now(),
        last_title: Instant::now(),
        started: Instant::now(),
        title_frame: 0,
        grabbed: false,
        adapter_info: None,
        faces_total,
        exit_requested: false,
        acquire_timeouts: 0,
        entities: Vec::new(),
        bodies: Vec::new(),
        revoked: Vec::new(),
        bench_yaw0: 0.0,
        aim: (0.0, 0.0),
        pending_map: None,
        palette,
        script_target: None,
        last_report: Instant::now(),
        report_frame: 0,
        #[cfg(target_arch = "wasm32")]
        pending_gpu: Default::default(),
        #[cfg(target_arch = "wasm32")]
        pending_fetch: Default::default(),
        #[cfg(target_arch = "wasm32")]
        first_frame_ms: 0.0,
        tactical: Tactical::new(),
        tactical_leaves: Vec::new(),
        bars: Vec::new(),
        squad_view: Vec::new(),
        target_view: None,
    };
    if app.opts.tactical {
        app.tactical.enter(app.sim.yaw);
    }
    app.bench_yaw0 = app.sim.yaw;
    app
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run(opts: Options) -> Result<(), Error> {
    let map_bytes =
        std::fs::read(&opts.map).map_err(|e| format!("loading {}: {e}", opts.map.display()))?;
    let bsp = Bsp::load(&opts.map).map_err(|e| format!("loading {}: {e}", opts.map.display()))?;
    log::info!(
        "loaded {} ({} faces, {} leaves)",
        opts.map.display(),
        bsp.faces.len(),
        bsp.leaves.len()
    );
    let palette = world::load_palette(&opts.palette);
    let sim = Sim::at(&bsp, opts.start);
    // Through the hub: log in and get a ticket first; the ticket names the zone, its address
    // and its certificate. The map comes from `Welcome` and is loaded then.
    let entry = match (opts.hub, opts.connect) {
        (Some(hub_addr), _) => {
            let cert_der = std::fs::read(&opts.hub_cert)
                .map_err(|e| format!("reading hub certificate {}: {e}", opts.hub_cert.display()))?;
            let (session, ticket) = HubSession::enter(HubLogin {
                hub: ZoneAddr {
                    addr: Some(hub_addr),
                    cert_der,
                    web: None,
                },
                email: opts.user.clone(),
                password: opts.password.clone(),
                register: opts.register,
                character: opts.character.clone(),
                new_preset: opts.build.clone(),
                zone: opts.zone.clone(),
            })?;
            log::info!(
                "hub ticket for zone {} at {} (character {})",
                ticket.zone,
                ticket.addr,
                session.character
            );
            Some(Entry {
                zone: ticket_addr(&ticket),
                token: bitcode::encode(&ticket.token),
                hub: Some(session),
                zone_name: ticket.zone,
            })
        }
        (None, Some(addr)) => {
            let cert = std::fs::read(&opts.cert)
                .map_err(|e| format!("reading zone certificate {}: {e}", opts.cert.display()))?;
            Some(Entry {
                zone: ZoneAddr {
                    addr: Some(addr),
                    cert_der: cert,
                    web: None,
                },
                token: Vec::new(),
                hub: None,
                zone_name: String::new(),
            })
        }
        (None, None) => None,
    };
    let online = match entry {
        Some(entry) => Some(online(&opts, &sim, fnv1a64(&map_bytes), entry)?),
        None => None,
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = app(opts, bsp, palette, sim, online);
    event_loop.run_app(&mut app)?;
    if let Some(o) = &mut app.online {
        o.net.close();
        if let Some(hub) = &o.hub {
            hub.logout();
            log::info!("logged out of the hub");
        }
    }

    let report = app.stats.report();
    let scripted = app.opts.bench_frames.is_some() || app.opts.seconds > 0.0;
    if let (true, Some(info)) = (scripted, &app.adapter_info) {
        let faces = app.active.as_ref().map_or(0, |a| a.renderer.faces_drawn);
        let draws = app.active.as_ref().map_or(1, |a| a.renderer.draw_calls);
        print_bench(&report, info, "windowed", faces, app.faces_total, draws);
    } else if report.frames > 0 {
        let (_, peak) = crate::stats::rss_bytes();
        log::info!(
            "{} frames, {:.1} fps average, peak RSS {} bytes ({:.1} MiB)",
            report.frames,
            report.fps_avg,
            peak,
            peak as f64 / 1048576.0
        );
    }
    if let Some(a) = &app.active {
        print_bench_avatars(&report, &a.avatars, &a.renderer);
    }
    if let Some(o) = &app.online
        && let Some(c) = &o.client
    {
        let s = c.stats;
        log::info!(
            "net: {} snapshots, {} gaps (max {}), {} corrections ({} unexplained, max {:.1} u), {} inputs sent, delay {} ticks",
            s.snapshots,
            s.gaps,
            s.max_gap,
            s.corrections,
            s.corrections_unexplained,
            s.max_correction,
            s.inputs_sent,
            c.delay_ticks
        );
    }
    if app.exit_requested {
        return Err("exited on error".into());
    }
    Ok(())
}

/// A map and its coloured lightmaps from the page's assets, with the hash of the `.bsp`.
#[cfg(target_arch = "wasm32")]
async fn fetch_map(assets: &str, name: &str) -> Result<(Bsp, u64), String> {
    /// A map that has not arrived in this long is not coming (the zone waits 60 s for a
    /// client's first input).
    const FETCH_MS: u32 = 45_000;
    let url = format!("{assets}/maps/{name}.bsp");
    let fetch = async {
        let bytes = crate::web::fetch_bytes(&url)
            .await?
            .ok_or_else(|| format!("{url}: no such map on this site"))?;
        let mut bsp = Bsp::parse(&bytes).map_err(|e| format!("{url}: {e}"))?;
        if let Some(lit) = crate::web::fetch_bytes(&format!("{assets}/maps/{name}.lit")).await? {
            bsp.attach_lit(&lit).map_err(|e| format!("{url}: {e}"))?;
        }
        Ok((bsp, fnv1a64(&bytes)))
    };
    crate::web::timeout_ms(FETCH_MS, fetch)
        .await
        .unwrap_or_else(|| Err(format!("{url}: the download took too long")))
}

/// The browser client (WEB.md 3, 5): options from the page, assets by `fetch`, the login
/// awaited, then the event loop on the page's own.
#[cfg(target_arch = "wasm32")]
pub async fn run_web() -> Result<(), String> {
    use crate::web::{PageOptions, parse_hash, tell_page};
    use gm_net::control::WebAddr;
    use winit::platform::web::EventLoopExtWebSys;

    let page = PageOptions::read();
    let web_addr = |url: &str, hash: &str| -> Result<Option<WebAddr>, String> {
        let Some(url) = page.string(url) else {
            return Ok(None);
        };
        let cert_sha256 = match page.string(hash) {
            Some(h) => Some(parse_hash(&h).ok_or("a certificate hash must be 64 hex digits")?),
            None => None,
        };
        Ok(Some(WebAddr { url, cert_sha256 }))
    };
    let mut opts = Options {
        name: page.string("name").unwrap_or_else(|| "web".into()),
        build: page.string("build"),
        team: page.number("team").unwrap_or(0.0) as u8,
        third_person: page.flag("third-person"),
        tactical: page.flag("tactical"),
        user: page.string("user").unwrap_or_default(),
        password: page.take("password").unwrap_or_default(),
        register: page.flag("register"),
        character: page.string("character").unwrap_or_default(),
        seconds: page.number("seconds").unwrap_or(0.0) as f32,
        script: page.string("script"),
        report: page.flag("report"),
        travel_to: page.string("travel-to"),
        travel_after: page.number("travel-after").unwrap_or(0.0) as f32,
        cache_mb: page.number("cache-mb").unwrap_or(128.0) as u64,
        hub_web: web_addr("hub", "hub-cert")?,
        connect_web: web_addr("connect", "cert")?,
        assets: page.string("assets").unwrap_or_else(|| "assets".into()),
        ..Options::default()
    };
    if let Some(zone) = page.string("zone") {
        opts.zone = zone;
    }
    if let Some(mb) = page.number("vram-mb") {
        opts.vram_mb = mb as u64;
    }
    let map = page.string("map").unwrap_or_else(|| "test_room".into());
    if !valid_map_name(&map) {
        return Err(format!("not a map name: {map:?}"));
    }

    tell_page("status", "loading the map");
    let (bsp, map_hash) = fetch_map(&opts.assets, &map).await?;
    let palette = world::palette_from_bytes(
        crate::web::fetch_bytes(&format!("{}/textures/palette.lmp", opts.assets))
            .await?
            .as_deref(),
    );
    let sim = Sim::at(&bsp, opts.start);
    let logging_in = !opts.user.is_empty() && opts.hub_web.is_some();
    let entry = if logging_in {
        tell_page("status", "logging in");
        let (session, ticket) = HubSession::enter(HubLogin {
            hub: ZoneAddr {
                addr: None,
                cert_der: Vec::new(),
                web: opts.hub_web.clone(),
            },
            email: opts.user.clone(),
            password: opts.password.clone(),
            register: opts.register,
            character: opts.character.clone(),
            new_preset: opts.build.clone(),
            zone: opts.zone.clone(),
        })
        .await?;
        // The password has done its work; nothing keeps it (WEB.md 5).
        opts.password.clear();
        log::info!("hub ticket for zone {}", ticket.zone);
        if ticket.web.is_none() {
            return Err(format!("zone {} has no web listener", ticket.zone));
        }
        Some(Entry {
            zone: ticket_addr(&ticket),
            token: bitcode::encode(&ticket.token),
            hub: Some(session),
            zone_name: ticket.zone,
        })
    } else {
        opts.connect_web.clone().map(|web| Entry {
            zone: ZoneAddr {
                addr: None,
                cert_der: Vec::new(),
                web: Some(web),
            },
            token: Vec::new(),
            hub: None,
            zone_name: String::new(),
        })
    };
    let online = match entry {
        Some(entry) => {
            tell_page("status", "connecting to the zone");
            Some(online(&opts, &sim, map_hash, entry).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    // Frames come from `requestAnimationFrame`: each redraw asks for the next.
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.spawn_app(app(opts, bsp, palette, sim, online));
    Ok(())
}

/// Third-person camera position: pulled in by a point trace so it never enters a wall.
fn third_person_camera(world: &dyn CollisionWorld, eye: Vec3, yaw: f32, pitch: f32) -> Vec3 {
    let fwd = view_dir(yaw, pitch);
    let (_, right) = yaw_vectors(yaw);
    let desired = eye - fwd * CAMERA_BACK + right * CAMERA_RIGHT + Vec3::Z * CAMERA_UP;
    let tr = world.trace(Hull::Point, eye, desired);
    if tr.fraction < 1.0 {
        let back = (eye - tr.end).normalize_or_zero();
        tr.end + back * 6.0
    } else {
        desired
    }
}

/// Camera-to-muzzle re-aim (VOCABULARY.md 9): resolve the camera ray to the first world or
/// player hit, then aim at that point from the eyes. Returns `(yaw, pitch)` for the input.
fn re_aim(
    world: &dyn CollisionWorld,
    bodies: &[Aabb],
    camera: Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    eye: Vec3,
) -> (f32, f32) {
    let dir = view_dir(cam_yaw, cam_pitch);
    let far = camera + dir * AIM_REACH;
    let tr = world.trace(Hull::Point, camera, far);
    let mut point = if tr.fraction < 1.0 { tr.end } else { far };
    let bt = sweep_boxes(Hull::Point, camera, point, bodies);
    if bt.fraction < 1.0 && !bt.start_solid {
        point = bt.end;
    }
    let to = point - eye;
    // A hit between the camera and the body: aim straight along the camera.
    let aim = if to.dot(dir) < 16.0 {
        dir
    } else {
        to.normalize_or_zero()
    };
    let yaw = aim.y.atan2(aim.x).to_degrees().rem_euclid(360.0);
    let pitch = (-aim.z).clamp(-1.0, 1.0).asin().to_degrees();
    (yaw, pitch)
}

/// The faces visible from any of `leaves`, each listed once.
fn visible_from(bsp: &Bsp, leaves: &[usize]) -> Vec<u32> {
    let mut seen = vec![false; bsp.faces.len()];
    let mut out = Vec::new();
    for &leaf in leaves {
        for f in bsp.visible_faces(leaf) {
            if !seen[f as usize] {
                seen[f as usize] = true;
                out.push(f);
            }
        }
    }
    out
}

/// A map's name as a zone may send it: letters, digits, `_` and `-`, at most 64. It is joined
/// to a directory natively and put into a URL in a browser.
fn valid_map_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// What the script assumes of the weapons it holds (the v1 content's are close to it): a
/// bolt's speed and the wind-up before it leaves, a melee weapon's reach from the eye, and
/// how far the first active (an area around the caster for most builds) reaches.
const SCRIPT_BOLT_SPEED: f32 = 1400.0;
const SCRIPT_BOLT_WINDUP: f32 = 0.15;
const SCRIPT_MELEE_REACH: f32 = 84.0;
const SCRIPT_AREA_REACH: f32 = 150.0;

/// `--script fight` (WEB.md 8): face the nearest enemy and walk at it; in reach strike and
/// use the first active (an area for most builds, which a guard does not turn away as it
/// does a swing); further off loose the ranged secondary where the enemy will be. With
/// nobody in sight, walk and turn. `target` is the enemy's position and velocity.
fn fight_input(eye: Vec3, target: Option<(Vec3, Vec3)>, sim: &mut Sim, tick: u32) -> SimInput {
    let mut input = SimInput {
        buttons: 0,
        yaw: sim.yaw,
        pitch: 0.0,
        forward: 1.0,
        side: 0.0,
        ability: 0,
    };
    match target {
        Some((at, velocity)) => {
            let distance = (at - eye).length().max(1.0);
            let melee = distance < SCRIPT_MELEE_REACH;
            // A bolt is aimed where the enemy will be when it arrives.
            let aim_at = if melee {
                at
            } else {
                at + velocity * (SCRIPT_BOLT_WINDUP + distance / SCRIPT_BOLT_SPEED)
            };
            let to = aim_at - eye;
            let reach = to.length().max(1.0);
            sim.yaw = to.y.atan2(to.x).to_degrees().rem_euclid(360.0);
            sim.pitch = (-to.z / reach).clamp(-1.0, 1.0).asin().to_degrees();
            input.yaw = sim.yaw;
            input.pitch = sim.pitch;
            if melee {
                input.buttons |= buttons::PRIMARY;
                if distance < 48.0 {
                    input.forward = 0.0;
                }
            } else if distance < 900.0 {
                input.buttons |= buttons::SECONDARY;
            }
            // The first active as often as it comes back, the second now and then.
            if distance < SCRIPT_AREA_REACH && tick.is_multiple_of(32) {
                input.ability = if tick.is_multiple_of(256) { 2 } else { 1 };
            }
        }
        None => {
            sim.yaw = (sim.yaw + 0.6).rem_euclid(360.0);
            sim.pitch = 0.0;
            input.yaw = sim.yaw;
        }
    }
    input
}

fn role_name(role: u8) -> &'static str {
    match role {
        0 => "heal",
        1 => "tank",
        2 => "scout",
        _ => "dps",
    }
}

fn order_name(order: &Order) -> &'static str {
    match order {
        Order::Follow => "follow",
        Order::Hold => "hold",
        Order::MoveTo(_) => "move",
        Order::Attack(_) => "attack",
    }
}

/// The HUD of one frame (COMPANIONS.md 6): the own bars, the squad panel, the creature
/// being fought and the messages in every viewport; in the tactical one also the health
/// bars over the bodies and the keys.
pub(crate) fn build_hud(
    hud: &mut Hud,
    online: Option<&Online>,
    tac: &Tactical,
    vp: glam::Mat4,
    bars: &[(Vec3, f32, [f32; 4])],
    squad_view: &[(Option<u16>, bool)],
    target: Option<&(String, u16, u16)>,
) {
    let (w, h) = hud.size;
    let s = if h >= 1000.0 { 3.0 } else { 2.0 };
    let line = (hud::GLYPH_H + 5.0) * s;
    if tac.active {
        for (at, frac, colour) in bars {
            if let Some((x, y)) = project(vp, hud.size, *at) {
                hud.bar(x - 22.0, y - 3.0, 44.0, 5.0, *frac, *colour);
            }
        }
        let hint = "tab back   1-5 ` select   lmb pick   rmb move / attack   f follow   h hold   wasd pan   q e turn   wheel zoom";
        // Bottom right: the own bars are bottom left.
        let tw = Hud::text_width(s * 0.5, hint);
        hud.label(w - tw - 16.0, h - 14.0 * s, s * 0.5, hud::DIM, hint);
        let title = "tactical";
        hud.label(
            w - Hud::text_width(s, title) - 16.0,
            16.0,
            s,
            hud::YELLOW,
            title,
        );
    } else {
        // The aim: a dot in the middle.
        hud.rect(w * 0.5 - 2.0, h * 0.5 - 2.0, 4.0, 4.0, hud::SHADE);
        hud.rect(w * 0.5 - 1.0, h * 0.5 - 1.0, 2.0, 2.0, hud::WHITE);
    }
    let Some(o) = online else { return };
    let Some(c) = &o.client else { return };

    // The own bars, bottom left.
    let max_health = c.sheet.derived.health.max(1) as f32;
    let rows: [(&str, f32, f32, [f32; 4]); 3] = [
        ("hp", c.own_health.max(0) as f32, max_health, hud::RED),
        (
            "st",
            c.mover.stamina,
            c.sheet.derived.stamina.max(1.0),
            hud::YELLOW,
        ),
        (
            "fo",
            c.mover.focus,
            c.sheet.derived.focus.max(1.0),
            hud::BLUE,
        ),
    ];
    let mut y = h - 16.0 - line * rows.len() as f32;
    if c.mover.commanding(c.tick) {
        hud.label(16.0, y - line, s, hud::YELLOW, "command stance");
    }
    for (name, have, max, colour) in rows {
        hud.label(16.0, y, s, hud::WHITE, name);
        let x = 16.0 + 3.0 * hud::ADVANCE * s;
        hud.bar(
            x,
            y + s,
            180.0,
            hud::GLYPH_H * s - 2.0 * s,
            have / max,
            colour,
        );
        hud.label(
            x + 188.0,
            y,
            s,
            hud::DIM,
            &format!("{:.0}/{:.0}", have, max),
        );
        y += line;
    }

    // The squad, top left: slot, name, role, order, and its health under it.
    let mut y = 16.0;
    for (i, m) in o.squad.iter().enumerate() {
        let (health, alive) = squad_view.get(i).copied().unwrap_or((None, false));
        let picked = tac.active && tac.selected & (1 << i) != 0;
        let colour = match (alive, picked) {
            (false, _) => hud::RED,
            (true, true) => hud::GREEN,
            (true, false) => hud::WHITE,
        };
        let text = format!(
            "{} {}  {}  {}",
            i + 1,
            m.name,
            role_name(m.role),
            if alive { order_name(&m.order) } else { "down" }
        );
        hud.label(16.0, y, s, colour, &text);
        y += line;
        let frac = health.map_or(0.0, |v| v as f32 / m.max_health.max(1) as f32);
        hud.bar(16.0, y - 2.0 * s, 150.0, 3.0 * s, frac, hud::GREEN);
        y += 6.0 * s;
    }

    // The creature being fought, top middle; messages under it.
    let mut y = 16.0;
    if let Some((name, health, max)) = target {
        let tw = Hud::text_width(s, name);
        hud.label((w - tw) * 0.5, y, s, hud::WHITE, name);
        y += line;
        let bw = (w * 0.34).min(520.0);
        hud.bar(
            (w - bw) * 0.5,
            y,
            bw,
            5.0 * s,
            *health as f32 / (*max).max(1) as f32,
            hud::RED,
        );
        let count = format!("{health}/{max}");
        hud.text(
            (w - Hud::text_width(s * 0.5, &count)) * 0.5,
            y + 0.75 * s,
            s * 0.5,
            hud::WHITE,
            &count,
        );
        y += 5.0 * s + 10.0;
    }
    for (at, text, colour) in &o.messages {
        if at.elapsed().as_secs_f32() > MESSAGE_SECS {
            continue;
        }
        // A long line is cut to the screen: the log has the whole of it.
        let fit = ((w - 40.0) / (hud::ADVANCE * s)) as usize;
        let shown: String = text.chars().take(fit).collect();
        let tw = Hud::text_width(s, &shown);
        hud.label((w - tw) * 0.5, y, s, *colour, &shown);
        y += line;
    }
}

impl App {
    /// Give up: say why (on the page too, in a browser) and leave the loop.
    fn fail(&mut self, event_loop: &ActiveEventLoop, why: &str) {
        log::error!("{why}");
        #[cfg(target_arch = "wasm32")]
        crate::web::tell_page("error", why);
        self.exit_requested = true;
        event_loop.exit();
    }

    /// The window has its device: configure the surface and build the renderer on it.
    fn activate(
        &mut self,
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        gpu: Gpu,
    ) -> Result<(), Error> {
        let size = window.inner_size();
        let caps = surface.get_capabilities(&gpu.adapter);
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by the adapter")?;
        // Frames are drawn in sRGB. Where the surface has no sRGB format of its own (a
        // WebGPU canvas is `bgra8unorm`), its sRGB twin is asked for as a view format.
        let view_format = match caps.formats.iter().copied().find(|f| f.is_srgb()) {
            Some(format) => {
                config.format = format;
                format
            }
            None => {
                config.format = caps.formats[0];
                let srgb = config.format.add_srgb_suffix();
                if srgb != config.format {
                    config.view_formats.push(srgb);
                }
                srgb
            }
        };
        // Mailbox is the default: no tearing, no blocking, and it works where Fifo stalls
        // (X11 with a compositing window manager on RADV presented one frame per second).
        // Benchmarks measure throughput, so they run without vsync unless told otherwise.
        let bench = self.opts.bench_frames.is_some();
        config.present_mode = match self.opts.present {
            Some(mode) if caps.present_modes.contains(&mode) => mode,
            Some(mode) => {
                log::warn!(
                    "present mode {mode:?} unsupported here (available: {:?}); using the default",
                    caps.present_modes
                );
                wgpu::PresentMode::AutoVsync
            }
            // A browser presents with the display, whatever is asked.
            None if cfg!(target_arch = "wasm32") => wgpu::PresentMode::AutoVsync,
            None if bench || !self.opts.vsync => wgpu::PresentMode::AutoNoVsync,
            None if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) => {
                wgpu::PresentMode::Mailbox
            }
            None => wgpu::PresentMode::AutoVsync,
        };
        log::info!("present modes available: {:?}", caps.present_modes);
        config.desired_maximum_frame_latency = 2;
        surface.configure(&gpu.device, &config);
        log::info!(
            "surface {}x{} {:?} (drawn as {:?}) {:?}",
            config.width,
            config.height,
            config.format,
            view_format,
            config.present_mode
        );
        let mesh = self.mesh.take().ok_or("world mesh already consumed")?;
        let mut renderer = Renderer::new(&gpu, view_format, &mesh, (config.width, config.height));
        // Online, models come from the hub the session is logged in to.
        let source = self
            .online
            .as_ref()
            .and_then(|o| o.hub.as_ref())
            .map(|h| h.model_source());
        let avatars = Avatars::new(
            &gpu,
            &mut renderer.characters,
            &self.opts,
            &self.bsp,
            source,
        )?;
        self.adapter_info = Some(gpu.info.clone());
        self.active = Some(Active {
            window: window.clone(),
            surface,
            config,
            view_format,
            gpu,
            renderer,
            avatars,
            drawn_from: vec![usize::MAX],
        });
        // A browser gives the pointer only to a click (WEB.md 3.4): there the first click grabs.
        if cfg!(not(target_arch = "wasm32"))
            && self.opts.bench_frames.is_none()
            && !self.tactical.active
            && self.opts.script.is_none()
        {
            self.set_grab(true);
        }
        self.last_frame = Instant::now();
        self.started = Instant::now();
        // Offline there is nothing left to wait for; online the status clears at `Welcome`.
        #[cfg(target_arch = "wasm32")]
        if self.online.is_none() {
            crate::web::tell_page("status", "");
        }
        window.request_redraw();
        Ok(())
    }

    /// Tab: into the tactical viewport, or back out of it.
    fn toggle_tactical(&mut self) {
        if self.tactical.active {
            self.tactical.active = false;
            self.set_grab(self.opts.bench_frames.is_none());
        } else {
            self.tactical.enter(self.sim.yaw);
            self.set_grab(false);
        }
        log::info!(
            "tactical viewport: {}",
            if self.tactical.active { "on" } else { "off" }
        );
    }

    fn set_grab(&mut self, grab: bool) {
        let Some(a) = &self.active else { return };
        if grab {
            let ok = a
                .window
                .set_cursor_grab(CursorGrabMode::Confined)
                .or_else(|_| a.window.set_cursor_grab(CursorGrabMode::Locked))
                .is_ok();
            if !ok {
                log::warn!("cursor grab unsupported here; mouse look still works while focused");
            }
        } else {
            let _ = a.window.set_cursor_grab(CursorGrabMode::None);
        }
        a.window.set_cursor_visible(!grab);
        self.grabbed = grab;
    }

    fn configure_surface(&mut self) {
        let Some(a) = &mut self.active else { return };
        let size = a.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        a.config.width = size.width;
        a.config.height = size.height;
        a.surface.configure(&a.gpu.device, &a.config);
        a.renderer.resize(&a.gpu, (size.width, size.height));
    }

    /// F1–F4 ask the zone for the preset builds, applied at the next respawn.
    fn respec_hotkeys(&mut self) {
        let Some(o) = self.online.as_mut() else {
            return;
        };
        let Some(pack) = &o.pack else { return };
        // T asks to travel to the zone named on the command line; a scripted run asks by
        // itself, once, after `travel_after` seconds.
        let timed = self.opts.travel_after > 0.0
            && self.started.elapsed().as_secs_f32() >= self.opts.travel_after;
        if (self.input.just_pressed.remove(&KeyCode::KeyT) || timed)
            && let Some(target) = self.opts.travel_to.clone()
        {
            if timed {
                self.opts.travel_after = 0.0;
            }
            o.net.send_control(Control::Travel(target.clone()));
            o.respec_note = format!("travel to {target} requested");
            log::info!("{}", o.respec_note);
        }
        // B opens a stall on the market tile underfoot, N closes the own stall.
        if self.input.just_pressed.remove(&KeyCode::KeyB) {
            o.net.send_control(Control::StallOpen);
            o.respec_note = "stall requested".into();
        }
        if self.input.just_pressed.remove(&KeyCode::KeyN) {
            o.net.send_control(Control::StallClose);
            o.respec_note = "closing the stall".into();
        }
        let keys = [KeyCode::F1, KeyCode::F2, KeyCode::F3, KeyCode::F4];
        for (i, k) in keys.iter().enumerate() {
            if self.input.just_pressed.remove(k)
                && let Some(b) = pack.builds.get(i)
            {
                o.net
                    .send_control(Control::Respec(BuildChoice::Preset(b.name.clone())));
                o.respec_note = format!("respec {} requested", b.name);
            }
        }
    }

    /// Network events, prediction ticks and the entity list for this frame. Returns the
    /// camera `(position, yaw, pitch)`.
    fn online_frame(
        &mut self,
        frame_dt: f32,
        event_loop: &ActiveEventLoop,
    ) -> Option<(Vec3, f32, f32)> {
        self.respec_hotkeys();
        let size = self.active.as_ref().map_or((1280.0, 720.0), |a| {
            (a.config.width as f32, a.config.height as f32)
        });
        let bsp = &self.bsp;
        let viewport = self.viewport;
        let o = self.online.as_mut()?;
        o.backlog.extend(o.net.poll());
        // The zone's map is still being fetched (a browser): what the zone sent after its
        // `Welcome` waits, except snapshots, which would be stale by then anyway.
        #[cfg(target_arch = "wasm32")]
        if o.awaiting_map {
            // The zone hung up meanwhile: say so now, not after a map nobody needs.
            if let Some(NetEvent::Disconnected(reason)) = o
                .backlog
                .iter()
                .find(|e| matches!(e, NetEvent::Disconnected(_)))
            {
                let why = format!("disconnected: {reason}");
                self.fail(event_loop, &why);
                return None;
            }
            match self.pending_fetch.borrow_mut().take() {
                None => {
                    o.backlog.retain(|e| !matches!(e, NetEvent::Snapshot(_)));
                    return Some((self.sim.eye(), self.sim.yaw, self.sim.pitch));
                }
                Some(Ok((map, hash))) if hash == o.map_hash => {
                    o.awaiting_map = false;
                    crate::web::tell_page("status", "");
                    self.pending_map = Some(map);
                    // The world is replaced at the start of the next frame.
                    return Some((self.sim.eye(), self.sim.yaw, self.sim.pitch));
                }
                Some(result) => {
                    let why = match result {
                        Err(e) => e,
                        Ok(_) => "this site's copy of the zone's map is another build".into(),
                    };
                    log::error!("{why}");
                    crate::web::tell_page("error", &why);
                    self.exit_requested = true;
                    event_loop.exit();
                    return None;
                }
            }
        }
        while let Some(ev) = o.backlog.pop_front() {
            match ev {
                NetEvent::Welcome {
                    entity,
                    hz,
                    map,
                    map_hash,
                } => {
                    o.rate = TickRate::new(hz as u32);
                    o.welcome = Some((entity, hz));
                    log::info!("joined as entity {entity} on {map} at {hz} Hz");
                    #[cfg(target_arch = "wasm32")]
                    crate::web::tell_page("status", "");
                    // The name becomes a file name or a URL: a zone does not choose paths.
                    if !valid_map_name(&map) {
                        let why =
                            format!("the zone named a map this client will not load: {map:?}");
                        self.fail(event_loop, &why);
                        return None;
                    }
                    if map_hash != o.map_hash {
                        // Another map: load it (a zone change through the hub lands here).
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let path = self.opts.maps_dir.join(format!("{map}.bsp"));
                            let loaded = std::fs::read(&path)
                                .ok()
                                .filter(|bytes| fnv1a64(bytes) == map_hash)
                                .and_then(|_| Bsp::load(&path).ok());
                            match loaded {
                                Some(bsp) => {
                                    log::info!("zone runs map {map}; loading {}", path.display());
                                    self.pending_map = Some(bsp);
                                    o.map_hash = map_hash;
                                }
                                None => {
                                    log::error!(
                                        "zone runs map {map} with hash {map_hash:016x}; ours is {:016x} and {} does not match (wrong map build)",
                                        o.map_hash,
                                        path.display()
                                    );
                                    self.exit_requested = true;
                                    event_loop.exit();
                                    return None;
                                }
                            }
                        }
                        #[cfg(target_arch = "wasm32")]
                        {
                            log::info!("zone runs map {map}; fetching it");
                            crate::web::tell_page("status", "loading the zone's map");
                            o.map_hash = map_hash;
                            o.awaiting_map = true;
                            let (slot, assets) =
                                (self.pending_fetch.clone(), self.opts.assets.clone());
                            wasm_bindgen_futures::spawn_local(async move {
                                let fetched = fetch_map(&assets, &map).await;
                                *slot.borrow_mut() = Some(fetched);
                            });
                            return Some((self.sim.eye(), self.sim.yaw, self.sim.pitch));
                        }
                    }
                }
                NetEvent::Snapshot(bytes) => {
                    if let Some(c) = &mut o.client {
                        match c.on_snapshot(bsp, &bytes) {
                            Ok(()) => o.last_snapshot = Instant::now(),
                            Err(e) => log::debug!("snapshot dropped: {e}"),
                        }
                    }
                }
                NetEvent::Control(msg) => match msg {
                    Control::Content { pack, own, team } => {
                        let Some((entity, _)) = o.welcome else {
                            log::error!("content before welcome");
                            continue;
                        };
                        o.team = team;
                        o.build_name = o.build_name_of(&pack, &own);
                        log::info!(
                            "content: {} abilities, {} presets; playing {} on team {team}",
                            pack.abilities.len(),
                            pack.builds.len(),
                            o.build_name
                        );
                        o.client = Some(ClientState::new(
                            entity,
                            o.rate,
                            Sheet::new(own, &pack, team),
                        ));
                        o.pack = Some(pack);
                    }
                    Control::BuildApplied(build) => {
                        let changed = o.client.as_ref().is_some_and(|c| c.sheet.build != build);
                        if changed && let Some(pack) = &o.pack {
                            let name = o.build_name_of(pack, &build);
                            let sheet = Sheet::new(build, pack, o.team);
                            if let Some(c) = &mut o.client {
                                c.set_sheet(sheet);
                            }
                            o.build_name = name;
                            o.respec_note = format!("now {}", o.build_name);
                            log::info!("build applied: {}", o.build_name);
                        }
                    }
                    Control::RespecResult(result) => {
                        o.respec_note = match result {
                            Ok(()) => "respec accepted (next respawn)".into(),
                            Err(e) => format!("respec refused: {e}"),
                        };
                        log::info!("{}", o.respec_note);
                    }
                    Control::TravelTicket {
                        zone,
                        addr,
                        cert_der,
                        token,
                        web,
                    } => {
                        log::info!("travel ticket for {zone} at {addr}");
                        let to = ZoneAddr {
                            addr: addr.parse().ok(),
                            cert_der,
                            web,
                        };
                        // The connection here is given up only for one that can be opened: a browser
                        // needs the zone's web listener, a native client its address.
                        let reachable = if cfg!(target_arch = "wasm32") {
                            to.web.is_some()
                        } else {
                            to.addr.is_some()
                        };
                        if reachable {
                            o.pending_travel = Some((zone, to, token));
                        } else if cfg!(target_arch = "wasm32") {
                            o.say(format!("{zone} has no web listener"), hud::ORANGE);
                        } else {
                            o.say(format!("{zone}: bad address {addr:?}"), hud::ORANGE);
                        }
                    }
                    Control::TravelRefused(reason) => {
                        o.respec_note = format!("travel refused: {reason}");
                        log::info!("{}", o.respec_note);
                    }
                    Control::Roster(players) => {
                        o.names.clear();
                        o.kinds.clear();
                        for p in players {
                            o.kinds.insert(p.id, p.kind);
                            o.names.insert(p.id, (p.name, p.team, p.model));
                        }
                    }
                    Control::PlayerInfo {
                        id,
                        name,
                        team,
                        model,
                        kind,
                    } => {
                        o.kinds.insert(id, kind);
                        o.names.insert(id, (name, team, model));
                    }
                    Control::Squad(entries) => o.squad = entries,
                    Control::OrderRefused(why) => {
                        o.say(format!("order refused: {why}"), hud::ORANGE);
                    }
                    Control::Encounter { name, state } => {
                        let (text, colour) = match state {
                            EncounterState::Engaged => (format!("{name}: engaged"), hud::WHITE),
                            EncounterState::Reset => (format!("{name}: reset"), hud::ORANGE),
                            EncounterState::Cleared { secs } => {
                                (format!("{name}: cleared in {secs} s"), hud::GREEN)
                            }
                        };
                        o.say(text, colour);
                    }
                    Control::Loot {
                        encounter,
                        items,
                        coin,
                    } => {
                        let mut what = items.join(", ");
                        if coin > 0 {
                            if !what.is_empty() {
                                what.push_str(", ");
                            }
                            what.push_str(&format!("{coin} copper"));
                        }
                        o.say(format!("loot ({encounter}): {what}"), hud::YELLOW);
                    }
                    Control::Trial {
                        name,
                        passed,
                        detail,
                        ..
                    } => {
                        if passed {
                            o.say(format!("trial passed: {name}"), hud::GREEN);
                            o.say(detail, hud::DIM);
                        } else {
                            o.say(format!("trial not passed: {name}: {detail}"), hud::DIM);
                        }
                    }
                    Control::ModelRevoked(id) => {
                        for entry in o.names.values_mut() {
                            if entry.2 == Some(id) {
                                entry.2 = None;
                            }
                        }
                        for stall in &mut o.stalls {
                            if stall.model == Some(id) {
                                stall.model = None;
                            }
                        }
                        self.revoked.push(id);
                    }
                    Control::Stalls(stalls) => o.stalls = stalls,
                    Control::StallOpened(stall) => {
                        log::info!("{} opened a stall", stall.owner);
                        o.stalls.retain(|s| s.id != stall.id);
                        o.stalls.push(stall);
                    }
                    Control::StallClosed(id) => o.stalls.retain(|s| s.id != id),
                    Control::StallResult(result) => {
                        o.respec_note = match result {
                            Ok(()) => "stall: done".into(),
                            Err(e) => format!("stall refused: {e}"),
                        };
                        log::info!("{}", o.respec_note);
                    }
                    Control::PlayerLeft(id) => {
                        o.names.remove(&id);
                        o.kinds.remove(&id);
                    }
                    Control::Killed { victim, killer } => {
                        let me = o.client.as_ref().map(|c| c.my_id);
                        if Some(killer) == me && victim != killer {
                            o.kills += 1;
                        }
                        if Some(victim) == me {
                            o.deaths += 1;
                        }
                        let name = |id: u32| {
                            o.names
                                .get(&id)
                                .map(|(n, _, _)| n.clone())
                                .unwrap_or_else(|| format!("#{id}"))
                        };
                        log::info!("{} killed {}", name(killer), name(victim));
                    }
                    Control::ChatFrom { from, text } => log::info!("<{from}> {text}"),
                    other => log::debug!("control: {other:?}"),
                },
                NetEvent::Disconnected(reason) => {
                    log::error!("disconnected: {reason}");
                    #[cfg(target_arch = "wasm32")]
                    crate::web::tell_page("error", &format!("disconnected: {reason}"));
                    self.exit_requested = true;
                    event_loop.exit();
                    return None;
                }
            }
        }
        // A travel ticket: say goodbye here, connect there (the body stays as a ghost until
        // the other zone claims it, HUB.md 3.3).
        if let Some((zone, to, token)) = o.pending_travel.take() {
            o.net.close();
            o.backlog.clear();
            match NetClient::connect(to, self.opts.name.clone(), None, self.opts.team, token) {
                Ok(net) => {
                    o.net = net;
                    o.welcome = None;
                    o.client = None;
                    o.pack = None;
                    o.names.clear();
                    o.kinds.clear();
                    o.squad.clear();
                    o.stalls.clear();
                    o.zone_name = zone;
                    o.respec_note = format!("travelling to {}", o.zone_name);
                    self.entities.clear();
                    self.bodies.clear();
                }
                Err(e) => log::error!("travel failed: {e}"),
            }
            return Some((self.sim.eye(), self.sim.yaw, self.sim.pitch));
        }
        let Some(c) = &mut o.client else {
            return Some((self.sim.eye(), self.sim.yaw, self.sim.pitch));
        };
        let dt = o.rate.dt();
        o.accumulator += frame_dt.min(0.25);
        let mut steps = 0;
        let bodies = c.latest_boxes();
        // The tactical viewport's keys are read before the ticks consume them: 1–5 select a
        // companion, ` selects all, F and H order the selection to follow or hold.
        let in_tactical = self.tactical.active;
        let scripted = self.opts.script.as_deref() == Some("fight");
        let mut orders: Vec<Order> = Vec::new();
        if in_tactical {
            let slots = [
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
            ];
            for (i, k) in slots.iter().enumerate() {
                if self.input.just_pressed.contains(k) && i < o.squad.len() {
                    self.tactical.selected = 1 << i;
                }
            }
            if self.input.just_pressed.contains(&KeyCode::Backquote) {
                self.tactical.selected = 0b1_1111;
            }
            if self.input.just_pressed.contains(&KeyCode::KeyF) {
                orders.push(Order::Follow);
            }
            if self.input.just_pressed.contains(&KeyCode::KeyH) {
                orders.push(Order::Hold);
            }
            let (forward, side) = self.input.axes();
            let turn =
                self.input.down(KeyCode::KeyE) as i32 - self.input.down(KeyCode::KeyQ) as i32;
            self.tactical
                .steer(forward, side, turn as f32, self.input.wheel, frame_dt);
        }
        self.input.wheel = 0.0;
        while o.accumulator >= dt && steps < MAX_STEPS_PER_FRAME {
            let (yaw, pitch) = match viewport {
                _ if in_tactical => (self.sim.yaw, self.sim.pitch),
                Viewport::First => (self.sim.yaw, self.sim.pitch),
                Viewport::Third => {
                    let eye = c.mover.eye();
                    let camera = third_person_camera(bsp, eye, self.sim.yaw, self.sim.pitch);
                    re_aim(bsp, &bodies, camera, self.sim.yaw, self.sim.pitch, eye)
                }
            };
            self.aim = (yaw, pitch);
            let input = if in_tactical {
                self.input.command_input(yaw, pitch)
            } else if scripted {
                let target = self
                    .script_target
                    .map(|(_, at, velocity, _)| (at, velocity));
                let input = fight_input(c.mover.eye(), target, &mut self.sim, c.tick);
                self.aim = (input.yaw, input.pitch);
                input
            } else {
                self.input.sim_input(yaw, pitch)
            };
            let datagram = c.local_tick(bsp, input);
            o.net.send_input(datagram.encode());
            o.prev_origin = o.curr_origin;
            o.curr_origin = c.mover.mv.origin;
            o.accumulator -= dt;
            steps += 1;
        }
        if steps == MAX_STEPS_PER_FRAME {
            o.accumulator = 0.0;
        }
        if !c.synced() {
            // Until the first snapshot lands, show the map from the local spawn.
            o.prev_origin = c.mover.mv.origin;
            o.curr_origin = c.mover.mv.origin;
        }

        // Other entities at the interpolation time (PROTOCOL.md 7.3).
        let extra = o.last_snapshot.elapsed().as_secs_f32() / dt;
        let t = c.render_tick(extra);
        self.entities.clear();
        self.bodies.clear();
        self.bars.clear();
        let my_team = o.team;
        let eye = {
            let alpha = (o.accumulator / o.rate.dt()).clamp(0.0, 1.0);
            o.prev_origin.lerp(o.curr_origin, alpha) + Vec3::Z * c.mover.mv.hull.eye_height()
        };
        let centre = eye - Vec3::Z * c.mover.mv.hull.eye_height();
        let camera = match viewport {
            _ if in_tactical => self.tactical.camera(centre),
            Viewport::First => eye,
            Viewport::Third => third_person_camera(bsp, eye, self.sim.yaw, self.sim.pitch),
        };
        let others = c.others_at(t);
        if scripted {
            let nearest = others
                .iter()
                .filter(|e| e.kind == EntityKind::Player && e.alive())
                .filter(|e| match e.spawn {
                    SpawnInfo::Player { team, .. } => my_team == 0 || team != my_team,
                    _ => false,
                })
                .min_by(|a, b| {
                    (a.pos - centre)
                        .length_squared()
                        .total_cmp(&(b.pos - centre).length_squared())
                })
                .map(|e| (e.id, e.pos));
            // Its velocity from where it stood a frame ago, if it is the same body.
            let now = Instant::now();
            self.script_target = nearest.map(|(id, at)| {
                let velocity = match self.script_target {
                    Some((was, before, _, then)) if was == id => {
                        let dt = (now - then).as_secs_f32();
                        if dt > 1.0e-3 {
                            ((at - before) / dt).clamp_length_max(600.0)
                        } else {
                            Vec3::ZERO
                        }
                    }
                    _ => Vec3::ZERO,
                };
                (id, at, velocity, now)
            });
        }
        // What the HUD shows of the squad and of the creature being fought: the one the
        // squad is ordered onto, or else the nearest one that is hurt.
        self.squad_view = o
            .squad
            .iter()
            .map(|m| {
                let e = others.iter().find(|e| e.id == m.id);
                (e.and_then(|e| e.health), e.is_some_and(|e| e.alive()))
            })
            .collect();
        self.target_view = None;
        if let Some(pack) = &o.pack {
            let mut best: Option<(f32, String, u16, u16)> = None;
            for e in others.iter().filter(|e| e.alive()) {
                let (Some(BodyKind::Creature { def }), Some(health)) =
                    (o.kinds.get(&e.id), e.health)
                else {
                    continue;
                };
                let Some(d) = pack.creatures.get(*def as usize) else {
                    continue;
                };
                let ordered = o
                    .squad
                    .iter()
                    .any(|m| matches!(m.order, Order::Attack(id) if id == e.id));
                if !ordered && health >= d.health {
                    continue;
                }
                let rank = (e.pos - centre).length() - if ordered { 1.0e6 } else { 0.0 };
                if best.as_ref().is_none_or(|b| rank < b.0) {
                    best = Some((rank, d.name.clone(), health, d.health));
                }
            }
            self.target_view = best.map(|(_, name, health, max)| (name, health, max));
        }
        if in_tactical {
            // The world is drawn from where the squad stands: the commander's leaf and
            // each companion's (squad sight, COMPANIONS.md 5.2).
            let mut leaves = vec![bsp.leaf_for_point(eye)];
            for e in &others {
                if e.alive() && o.squad.iter().any(|m| m.id == e.id) {
                    leaves.push(bsp.leaf_for_point(e.pos + Vec3::Z * 22.0));
                }
            }
            leaves.retain(|l| *l != 0);
            leaves.sort_unstable();
            leaves.dedup();
            self.tactical_leaves = leaves;
            // Clicks: left selects the companion under the cursor, right orders the
            // selection onto the body under it, or to the ground under it.
            let vp = view_proj(camera, self.tactical.yaw, tactical::PITCH, size.0 / size.1);
            let (origin, dir) = self.tactical.ray(vp, size);
            let (mut mates, mut strangers) = (Vec::new(), Vec::new());
            for e in others
                .iter()
                .filter(|e| e.kind == EntityKind::Player && e.alive())
            {
                if o.squad.iter().any(|m| m.id == e.id) {
                    mates.push((e.id, e.pos));
                } else {
                    strangers.push((e.id, e.pos));
                }
            }
            for click in self.input.clicks.drain(..) {
                match click {
                    MouseButton::Left => {
                        if let Some(slot) = pick_body(origin, dir, &mates)
                            .and_then(|id| o.squad.iter().position(|m| m.id == id))
                        {
                            self.tactical.selected = 1 << slot;
                        }
                    }
                    MouseButton::Right => match pick_body(origin, dir, &strangers) {
                        Some(id) => orders.push(Order::Attack(id)),
                        None => {
                            if let Some(point) = pick_ground(bsp, origin, dir) {
                                orders.push(Order::MoveTo(point.into()));
                            }
                        }
                    },
                    _ => {}
                }
            }
            if !o.squad.is_empty() {
                for order in orders.drain(..) {
                    o.net.send_control(Control::Order {
                        slots: self.tactical.selected,
                        order,
                    });
                }
            }
        }
        self.input.clicks.clear();
        for e in others {
            match e.kind {
                EntityKind::Player => {
                    let SpawnInfo::Player {
                        frame,
                        team,
                        aspects,
                        armour,
                    } = e.spawn
                    else {
                        continue;
                    };
                    // Health the zone sends (the own party's and creatures') over the body.
                    let mate = o.squad.iter().position(|m| m.id == e.id);
                    let max_health = match (mate, o.kinds.get(&e.id), &o.pack) {
                        (Some(i), _, _) => Some(o.squad[i].max_health),
                        (None, Some(BodyKind::Creature { def }), Some(pack)) => {
                            pack.creatures.get(*def as usize).map(|d| d.health)
                        }
                        _ => None,
                    };
                    if in_tactical
                        && e.alive()
                        && let (Some(h), Some(max)) = (e.health, max_health)
                    {
                        let colour = if mate.is_some() { hud::GREEN } else { hud::RED };
                        self.bars.push((
                            e.pos + Vec3::Z * 44.0,
                            h as f32 / max.max(1) as f32,
                            colour,
                        ));
                    }
                    if in_tactical
                        && e.alive()
                        && let Some(i) = mate
                    {
                        // A plate under each companion, bright when it is selected.
                        let lit = self.tactical.selected & (1 << i) != 0;
                        let feet = e.pos - Vec3::Z * 23.0;
                        self.entities.push(EntityDraw {
                            mins: feet - Vec3::new(22.0, 22.0, 0.0),
                            maxs: feet + Vec3::new(22.0, 22.0, 1.5),
                            color: if lit {
                                [0.35, 1.0, 0.45, 1.0]
                            } else {
                                [0.12, 0.40, 0.18, 1.0]
                            },
                        });
                    }
                    if in_tactical
                        && e.alive()
                        && o.squad
                            .iter()
                            .any(|m| matches!(m.order, Order::Attack(id) if id == e.id))
                    {
                        // The mark on whom the squad is ordered to attack.
                        let feet = e.pos - Vec3::Z * 23.0;
                        self.entities.push(EntityDraw {
                            mins: feet - Vec3::new(30.0, 30.0, 0.0),
                            maxs: feet + Vec3::new(30.0, 30.0, 1.0),
                            color: [1.0, 0.15, 0.10, 1.0],
                        });
                    }
                    self.bodies.push(Body {
                        key: e.id,
                        origin: e.pos,
                        yaw: e.yaw,
                        pitch: e.pitch,
                        anim: e.anim,
                        frame,
                        armour,
                        aspects,
                        team,
                        friendly: my_team != 0 && team == my_team,
                        status: e.status,
                        model: o.names.get(&e.id).and_then(|n| n.2),
                        distance: (e.pos - camera).length(),
                    });
                }
                EntityKind::Projectile => {
                    self.entities.push(EntityDraw {
                        mins: e.pos - Vec3::splat(2.5),
                        maxs: e.pos + Vec3::splat(2.5),
                        color: [1.0, 0.9, 0.3, 1.0],
                    });
                }
                EntityKind::Area => {
                    // What hurts is orange, what helps is green (PROTOCOL.md 5).
                    let (r, harmful) = match e.spawn {
                        SpawnInfo::Area {
                            radius, harmful, ..
                        } => (radius as f32, harmful),
                        _ => (32.0, true),
                    };
                    self.entities.push(EntityDraw {
                        mins: e.pos - Vec3::new(r, r, 0.0),
                        maxs: e.pos + Vec3::new(r, r, 2.0),
                        color: if harmful {
                            [1.0, 0.5, 0.1, 1.0]
                        } else {
                            [0.2, 0.8, 0.4, 1.0]
                        },
                    });
                }
            }
        }
        c.prune(t);
        // The market: every stall with its keeper, who never moves and costs no snapshot.
        for stall in &o.stalls {
            stall_boxes(stall, &mut self.entities);
            // While somebody stands behind the counter (the owner, usually), that body is
            // the keeper; the stand-in is drawn when the tile is empty.
            let at = Vec3::from(stall.pos);
            let attended = self
                .bodies
                .iter()
                .any(|b| (b.origin - at).truncate().length() < 40.0)
                || (o.curr_origin - at).truncate().length() < 40.0;
            if !attended {
                self.bodies.push(stall_keeper(stall, camera));
            }
        }
        if in_tactical {
            // Where each companion was told to go.
            for m in &o.squad {
                if let Order::MoveTo(p) = m.order {
                    let p = Vec3::from(p);
                    self.entities.push(EntityDraw {
                        mins: p - Vec3::new(8.0, 8.0, 24.0),
                        maxs: p + Vec3::new(8.0, 8.0, -20.0),
                        color: [0.25, 0.55, 1.0, 1.0],
                    });
                }
            }
        }
        match viewport {
            Viewport::First if !in_tactical => Some((eye, self.sim.yaw, self.sim.pitch)),
            _ => {
                // The own body, posed by the server's animation state.
                let build = &c.sheet.build;
                self.bodies.push(Body {
                    key: OWN,
                    origin: eye - Vec3::Z * c.mover.mv.hull.eye_height(),
                    yaw: self.sim.yaw,
                    pitch: self.sim.pitch,
                    anim: if c.synced() {
                        c.own_anim
                    } else {
                        gm_core::sim::anim::IDLE
                    },
                    frame: gm_model::rig::frame_index(build.frame),
                    armour: build.armour as u8,
                    aspects: build.aspects.0,
                    team: my_team,
                    friendly: true,
                    status: c.mover.statuses.mask(),
                    model: o.names.get(&c.my_id).and_then(|n| n.2),
                    distance: 0.0,
                });
                if in_tactical {
                    Some((camera, self.tactical.yaw, tactical::PITCH))
                } else {
                    Some((camera, self.sim.yaw, self.sim.pitch))
                }
            }
        }
    }

    /// Replace the world (BSP, mesh, renderer) with another map.
    fn switch_map(&mut self, bsp: Bsp) {
        let mesh = world::build(&bsp, &self.palette);
        self.faces_total = mesh
            .face_ranges
            .iter()
            .filter(|r| r.index_count > 0)
            .count();
        if let Some(a) = &mut self.active {
            a.renderer.set_world(&a.gpu, &mesh);
            a.drawn_from = vec![usize::MAX];
        } else {
            self.mesh = Some(mesh);
        }
        self.sim = Sim::new(&bsp);
        self.bsp = bsp;
        log::info!(
            "switched map ({} faces, {} leaves)",
            self.bsp.faces.len(),
            self.bsp.leaves.len()
        );
    }

    /// One line of what the client measured since the last one (`--report`, WEB.md 8).
    fn report_line(&mut self) -> String {
        let r = self.stats.report_since(self.report_frame);
        self.report_frame = self.stats.frames();
        let mut line = format!(
            "fps={:.1} frame_ms_avg={:.2} frame_ms_p99={:.2} frame_ms_max={:.2}",
            r.fps_avg, r.ms_avg, r.ms_p99, r.ms_max
        );
        if let Some(a) = &self.active {
            let m = a.avatars.stats();
            line.push_str(&format!(
                " backend={:?} draw_calls={} characters={} with_model={} models={} fetched={} fetched_bytes={} cache_hits={} failed={} refused={}",
                a.gpu.info.backend,
                a.renderer.draw_calls,
                a.renderer.characters.drawn,
                a.avatars.with_model,
                m.ready,
                m.fetched,
                m.fetched_bytes,
                m.disk_hits,
                m.failed,
                m.refused
            ));
            #[cfg(target_arch = "wasm32")]
            if let Some(cache) = &a.avatars.cache {
                line.push_str(&format!(
                    " cache_bytes={} cache_cap_bytes={}",
                    cache.loader.store.total(),
                    cache.loader.store.cap()
                ));
            }
        }
        if let Some(o) = &self.online {
            if let Some(c) = &o.client {
                let s = c.stats;
                line.push_str(&format!(
                    " entity={} snapshots={} gaps={} max_gap={} corrections={} unexplained={} inputs={} health={}",
                    c.my_id,
                    s.snapshots,
                    s.gaps,
                    s.max_gap,
                    s.corrections,
                    s.corrections_unexplained,
                    s.inputs_sent,
                    c.own_health
                ));
            }
            line.push_str(&format!(
                " kills={} deaths={} players={} zone={}",
                o.kills,
                o.deaths,
                o.names.len(),
                if o.zone_name.is_empty() {
                    "-"
                } else {
                    &o.zone_name
                }
            ));
            #[cfg(target_arch = "wasm32")]
            {
                let (rx, tx) = o.net.bytes();
                line.push_str(&format!(" rx_bytes={rx} tx_bytes={tx}"));
            }
        }
        #[cfg(target_arch = "wasm32")]
        line.push_str(&format!(
            " wasm_memory_bytes={} first_frame_ms={:.0}",
            crate::web::wasm_memory_bytes(),
            self.first_frame_ms
        ));
        line
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_arch = "wasm32")]
        if self.active.is_none() {
            let made = self.pending_gpu.borrow_mut().take();
            match (made, self.window.clone()) {
                (Some(Ok((surface, gpu))), Some(window)) => {
                    if let Err(e) = self.activate(window, surface, gpu) {
                        self.fail(event_loop, &format!("renderer setup failed: {e}"));
                        return;
                    }
                }
                (Some(Err(e)), _) => {
                    self.fail(event_loop, &format!("renderer setup failed: {e}"));
                    return;
                }
                _ => {}
            }
        }
        if let Some(bsp) = self.pending_map.take() {
            self.switch_map(bsp);
        }
        let now = Instant::now();
        let frame_dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Mouse look is applied per frame for responsiveness; movement uses it at tick time.
        let bench = self.opts.bench_frames.is_some();
        if bench && self.opts.crowd > 0 {
            let t = self.started.elapsed().as_secs_f32();
            self.sim.yaw = self.bench_yaw0 + BENCH_CROWD_SWING_DEG * (t * 0.7).sin();
        } else if bench {
            self.sim.yaw += BENCH_YAW_DEG_PER_S * frame_dt;
        } else if self.grabbed {
            self.sim.yaw -= self.input.mouse_dx * SENSITIVITY;
            self.sim.pitch =
                (self.sim.pitch + self.input.mouse_dy * SENSITIVITY).clamp(-89.0, 89.0);
        }
        self.sim.yaw = self.sim.yaw.rem_euclid(360.0);
        self.input.mouse_dx = 0.0;
        self.input.mouse_dy = 0.0;

        let (camera, cam_yaw, cam_pitch) = if self.online.is_some() {
            match self.online_frame(frame_dt, event_loop) {
                Some(cam) => cam,
                None => return,
            }
        } else {
            let input = if bench {
                MoveInput {
                    yaw: self.sim.yaw,
                    ..Default::default()
                }
            } else {
                self.input.move_input(self.sim.yaw)
            };
            // Offline the tactical viewport is a camera and nothing else: there is no squad.
            let in_tactical = self.tactical.active;
            let input = if in_tactical {
                let (forward, side) = self.input.axes();
                let turn =
                    self.input.down(KeyCode::KeyE) as i32 - self.input.down(KeyCode::KeyQ) as i32;
                let turn = if bench { 0.2 } else { turn as f32 };
                self.tactical
                    .steer(forward, side, turn, self.input.wheel, frame_dt);
                MoveInput {
                    yaw: self.sim.yaw,
                    ..Default::default()
                }
            } else {
                input
            };
            self.input.wheel = 0.0;
            self.input.clicks.clear();
            self.input.just_pressed.clear();
            self.sim.advance(&self.bsp, &input, frame_dt);
            self.entities.clear();
            self.bodies.clear();
            self.bars.clear();
            self.tactical_leaves = vec![self.bsp.leaf_for_point(self.sim.eye())];
            self.tactical_leaves.retain(|l| *l != 0);
            match self.viewport {
                Viewport::First if !in_tactical => (self.sim.eye(), self.sim.yaw, self.sim.pitch),
                _ => {
                    let v = self.sim.curr.velocity;
                    self.bodies.push(Body {
                        key: OWN,
                        origin: self.sim.origin(),
                        yaw: self.sim.yaw,
                        pitch: self.sim.pitch,
                        anim: if !self.sim.curr.on_ground {
                            gm_core::sim::anim::AIR
                        } else if v.truncate().length() > 20.0 {
                            gm_core::sim::anim::RUN
                        } else {
                            gm_core::sim::anim::IDLE
                        },
                        frame: 1,
                        armour: 0,
                        aspects: 0,
                        team: 0,
                        friendly: true,
                        status: 0,
                        model: None,
                        distance: 0.0,
                    });
                    if in_tactical {
                        if let Some(b) = self.bodies.last_mut() {
                            b.anim = gm_core::sim::anim::COMMAND;
                        }
                        (
                            self.tactical.camera(self.sim.origin()),
                            self.tactical.yaw,
                            tactical::PITCH,
                        )
                    } else {
                        (
                            third_person_camera(
                                &self.bsp,
                                self.sim.eye(),
                                self.sim.yaw,
                                self.sim.pitch,
                            ),
                            self.sim.yaw,
                            self.sim.pitch,
                        )
                    }
                }
            }
        };

        let Some(a) = &mut self.active else { return };
        // The world is drawn from the camera's leaf; in the tactical viewport, whose camera
        // hangs in the rock above the ceiling, from the leaves the squad stands in.
        let leaves = if self.tactical.active {
            self.tactical_leaves.clone()
        } else {
            vec![self.bsp.leaf_for_point(camera)]
        };
        if a.drawn_from != leaves {
            if leaves.is_empty() || leaves.contains(&0) {
                a.renderer.set_visible_faces(&a.gpu, None);
            } else {
                a.renderer
                    .set_visible_faces(&a.gpu, Some(&visible_from(&self.bsp, &leaves)));
            }
            a.drawn_from = leaves;
        }

        let frame = match a.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                log::debug!("surface suboptimal; reconfiguring");
                a.gpu.queue.present(f);
                self.configure_surface();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.acquire_timeouts += 1;
                if self.acquire_timeouts == 3 {
                    log::warn!(
                        "the display is not releasing frames (Fifo on a compositor, or a sleeping monitor); try --present mailbox"
                    );
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                log::debug!("surface occluded");
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                log::debug!("surface outdated or lost; reconfiguring");
                self.configure_surface();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error");
                event_loop.exit();
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(a.view_format),
            ..Default::default()
        });
        let aspect = a.config.width as f32 / a.config.height.max(1) as f32;
        for id in self.revoked.drain(..) {
            a.avatars.revoke(&id, &mut a.renderer.characters);
        }
        // The own avatar is the last thing the disk cache lets go of (MODELS.md 8).
        if let Some(o) = &self.online
            && let Some(c) = &o.client
            && let Some(id) = o.names.get(&c.my_id).and_then(|n| n.2)
        {
            a.avatars.pin(&id);
        }
        a.avatars.begin_frame();
        for body in &self.bodies {
            a.avatars.push(
                body,
                frame_dt,
                &self.bsp,
                &a.renderer.characters,
                &mut self.entities,
            );
        }
        a.avatars.push_crowd(
            self.started.elapsed().as_secs_f32(),
            frame_dt,
            camera,
            &self.bsp,
            &a.renderer.characters,
            &mut self.entities,
        );
        let vp = view_proj(camera, cam_yaw, cam_pitch, aspect);
        a.renderer.hud.begin((a.config.width, a.config.height));
        if !bench || self.opts.tactical {
            build_hud(
                &mut a.renderer.hud,
                self.online.as_ref(),
                &self.tactical,
                vp,
                &self.bars,
                &self.squad_view,
                self.target_view.as_ref(),
            );
        }
        a.renderer
            .render(&a.gpu, &view, vp, &self.entities, &a.avatars.draws);
        a.avatars.end_frame(&a.gpu, &mut a.renderer.characters);
        a.window.pre_present_notify();
        a.gpu.queue.present(frame);
        self.acquire_timeouts = 0;
        // A bench counts frames once every model its crowd wears is on the GPU.
        if bench
            && a.avatars.cache.as_ref().is_some_and(|c| c.pending() > 0)
            && self.started.elapsed().as_secs_f32() < 60.0
        {
            return;
        }
        self.stats.frame();
        #[cfg(target_arch = "wasm32")]
        if self.first_frame_ms == 0.0 {
            self.first_frame_ms = web_sys::window()
                .and_then(|w| w.performance())
                .map_or(0.0, |p| p.now());
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !bench && self.opts.max_fps > 0 {
            // Cheap CPU-side cap so an uncapped present mode does not spin the GPU at 100%.
            let budget = std::time::Duration::from_secs_f64(1.0 / self.opts.max_fps as f64);
            let spent = now.elapsed();
            if spent < budget {
                std::thread::sleep(budget - spent);
            }
        }
        if bench && self.stats.frames() > 0 && self.stats.frames().is_multiple_of(60) {
            log::debug!("bench frame {}", self.stats.frames());
        }

        if self.last_title.elapsed().as_secs_f32() >= 1.0 {
            let r = self.stats.report_since(self.title_frame);
            let vp = match self.viewport {
                Viewport::First => "1st",
                Viewport::Third => "3rd",
            };
            let title = match &self.online {
                Some(o) => {
                    let (hp, max_hp, st, fo, corr, delay) =
                        o.client.as_ref().map_or((0, 0, 0.0, 0.0, 0, 0), |c| {
                            (
                                c.own_health,
                                c.sheet.derived.health,
                                c.mover.stamina,
                                c.mover.focus,
                                c.stats.corrections,
                                c.delay_ticks,
                            )
                        });
                    let models = a.avatars.stats();
                    format!(
                        "gamengine [{}{} team {} {vp}]  hp {hp}/{max_hp}  st {st:.0}  fo {fo:.0}  k {} d {}  {} players  {} stalls  {} models  delay {delay}  corr {corr}  {:.0} fps  {}",
                        if o.zone_name.is_empty() {
                            String::new()
                        } else {
                            format!("{} ", o.zone_name)
                        },
                        o.build_name,
                        o.team,
                        o.kills,
                        o.deaths,
                        o.names.len(),
                        o.stalls.len(),
                        models.ready,
                        r.fps_avg,
                        o.respec_note
                    )
                }
                None => format!(
                    "gamengine [{vp}]  {:.0} fps  {:.2} ms  {} / {} faces",
                    r.fps_avg, r.ms_avg, a.renderer.faces_drawn, self.faces_total
                ),
            };
            a.window.set_title(&title);
            self.title_frame = self.stats.frames();
            self.last_title = Instant::now();
        }
        if let Some(n) = self.opts.bench_frames
            && self.stats.frames() >= n as usize
        {
            event_loop.exit();
        }
        if self.opts.report && self.last_report.elapsed().as_secs_f32() >= 1.0 {
            self.last_report = Instant::now();
            let line = self.report_line();
            #[cfg(not(target_arch = "wasm32"))]
            println!("stats: {line}");
            #[cfg(target_arch = "wasm32")]
            crate::web::tell_page("stats", &line);
        }
        if self.opts.seconds > 0.0 && self.started.elapsed().as_secs_f32() >= self.opts.seconds {
            #[cfg(target_arch = "wasm32")]
            {
                // The whole run in one line, for whoever scripted it.
                self.report_frame = 0;
                let line = self.report_line();
                if let Some(o) = &mut self.online {
                    o.net.close();
                    if let Some(hub) = &o.hub {
                        hub.logout();
                    }
                }
                crate::web::tell_page("done", &line);
            }
            event_loop.exit();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() || self.window.is_some() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let attrs = Window::default_attributes()
            .with_title("gamengine")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                self.opts.width,
                self.opts.height,
            ));
        // The page owns the canvas and its size (WEB.md 5).
        #[cfg(target_arch = "wasm32")]
        let attrs = {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;
            let canvas = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("gm-canvas"))
                .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
            Window::default_attributes()
                .with_title("gamengine")
                .with_canvas(canvas)
                .with_prevent_default(true)
                .with_focusable(true)
        };
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.fail(event_loop, &format!("window creation failed: {e}"));
                return;
            }
        };
        self.window = Some(window.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let result = instance
                .create_surface(window.clone())
                .map_err(Error::from)
                .and_then(|surface| {
                    let gpu = Gpu::new(&instance, Some(&surface), self.opts.software)?;
                    self.activate(window.clone(), surface, gpu)
                });
            if let Err(e) = result {
                self.fail(event_loop, &format!("renderer setup failed: {e}"));
            }
        }
        // A browser hands out its GPU device asynchronously: the frame picks it up.
        #[cfg(target_arch = "wasm32")]
        {
            let slot = self.pending_gpu.clone();
            let software = self.opts.software;
            wasm_bindgen_futures::spawn_local(async move {
                let made = async {
                    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
                    // Each build draws through one API (WEB.md 3.2).
                    desc.backends = if cfg!(feature = "webgl") {
                        wgpu::Backends::GL
                    } else {
                        wgpu::Backends::BROWSER_WEBGPU
                    };
                    let instance = wgpu::Instance::new(desc);
                    let surface = instance
                        .create_surface(window.clone())
                        .map_err(|e| e.to_string())?;
                    let gpu = Gpu::request(&instance, Some(&surface), software)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok((surface, gpu))
                }
                .await;
                *slot.borrow_mut() = Some(made);
                window.request_redraw();
            });
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => self.configure_surface(),
            WindowEvent::Focused(false) => {
                self.input.keys.clear();
                self.input.mouse.clear();
                self.set_grab(false);
            }
            WindowEvent::Focused(true) => {
                if cfg!(not(target_arch = "wasm32"))
                    && self.opts.bench_frames.is_none()
                    && !self.tactical.active
                    && self.opts.script.is_none()
                {
                    self.set_grab(true);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.tactical.cursor = (position.x as f32, position.y as f32);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.wheel += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => {
                    if self.tactical.active {
                        // The cursor is free here: a click picks or orders.
                        self.input.clicks.push(button);
                    } else if !self.grabbed && self.opts.bench_frames.is_none() {
                        self.set_grab(true);
                    } else {
                        self.input.mouse.insert(button);
                    }
                }
                ElementState::Released => {
                    self.input.mouse.remove(&button);
                }
            },
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if self.input.keys.insert(code) {
                                self.input.just_pressed.insert(code);
                            }
                            match code {
                                KeyCode::Escape => self.set_grab(false),
                                // Q turns the tactical camera; elsewhere it quits.
                                #[cfg(not(target_arch = "wasm32"))]
                                KeyCode::KeyQ if !self.tactical.active => event_loop.exit(),
                                KeyCode::Tab if !event.repeat => self.toggle_tactical(),
                                KeyCode::KeyV if !event.repeat => {
                                    self.viewport = match self.viewport {
                                        Viewport::First => Viewport::Third,
                                        Viewport::Third => Viewport::First,
                                    };
                                    log::info!("viewport: {:?}", self.viewport);
                                }
                                _ => {}
                            }
                        }
                        ElementState::Released => {
                            self.input.keys.remove(&code);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.grabbed
        {
            self.input.mouse_dx += delta.0 as f32;
            self.input.mouse_dy += delta.1 as f32;
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window
            && !self.exit_requested
        {
            w.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::collide::BoxWorld;

    #[test]
    fn third_person_camera_stays_out_of_walls() {
        let mut world = BoxWorld::floor();
        // A wall right behind the player (yaw 0 looks +x, the camera goes -x).
        world.push(
            Vec3::new(-60.0, -256.0, 0.0),
            Vec3::new(-40.0, 256.0, 200.0),
        );
        let eye = Vec3::new(0.0, 0.0, 46.0);
        let cam = third_person_camera(&world, eye, 0.0, 0.0);
        assert!(cam.x > -40.0, "camera inside the wall: {cam:?}");
        assert!(cam.x < -20.0, "camera not pulled back at all: {cam:?}");
        let open = third_person_camera(&BoxWorld::floor(), eye, 0.0, 0.0);
        assert!((open.x + CAMERA_BACK).abs() < 1e-3);
    }

    #[test]
    fn re_aim_targets_the_point_under_the_crosshair() {
        let world = BoxWorld::floor();
        let eye = Vec3::new(0.0, 0.0, 46.0);
        let cam = third_person_camera(&world, eye, 0.0, 0.0);
        // A body 300 u ahead on the camera ray (the camera sits CAMERA_RIGHT to the right,
        // which is -y at yaw 0): the ray hits it and the eye aims at the hit point, a few
        // degrees to the right of straight ahead.
        let body = Aabb::around(Vec3::new(300.0, -CAMERA_RIGHT, 40.0), Hull::Player);
        let (yaw, pitch) = re_aim(&world, &[body], cam, 0.0, 0.0, eye);
        assert!(yaw > 350.0 && yaw < 359.0, "yaw {yaw}");
        assert!(pitch.abs() < 5.0, "pitch {pitch}");
        // With nothing to hit, the eye ray converges on the far point: nearly parallel.
        let (yaw, _) = re_aim(&world, &[], cam, 0.0, 0.0, eye);
        assert!(!(1.0..=359.0).contains(&yaw), "yaw {yaw}");
    }
}
