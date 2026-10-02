//! Windowed client: winit event loop, fixed 64 Hz simulation with interpolated rendering,
//! mouse look, PVS-culled drawing, optional benchmark mode. Offline it runs the local
//! simulation; with `--connect` it predicts the own entity, reconciles against the zone and
//! interpolates everyone else (PROTOCOL.md 7). Two viewports share the simulation
//! (VOCABULARY.md 9): first person aims from the eyes, third person aims the camera ray at a
//! world point and re-aims it from the eyes ("camera-to-muzzle re-aim").

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

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
use gm_net::control::{BuildChoice, Control, StallEntry};
use gm_net::snapshot::{EntityKind, SpawnInfo};
use gm_net::transport::fnv1a64;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::avatars::{Avatars, Body, OWN, stall_boxes, stall_keeper};
use crate::hub::{HubLogin, HubSession};
use crate::net::{NetClient, NetEvent};
use crate::render::{EntityDraw, Gpu, Renderer, view_proj};
use crate::stats::{FrameStats, print_bench, print_bench_avatars};
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
struct Online {
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
    /// The market: open stalls and their keepers (ECONOMY.md 7).
    stalls: Vec<StallEntry>,
    kills: u32,
    deaths: u32,
    map_hash: u64,
    respec_note: String,
    /// The hub session when playing through the hub (HUB.md); logged out on exit.
    hub: Option<HubSession>,
    /// A travel ticket to act on: reconnect to another zone, reloading its map.
    pending_travel: Option<(String, std::net::SocketAddr, Vec<u8>, Vec<u8>)>,
    zone_name: String,
}

impl Online {
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
        if self.down(KeyCode::ControlLeft) || self.down(KeyCode::ControlRight) {
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
    gpu: Gpu,
    renderer: Renderer,
    avatars: Avatars,
    current_leaf: Option<usize>,
}

struct App {
    opts: Options,
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
    pending_map: Option<PathBuf>,
}

pub fn run(opts: Options) -> Result<(), Error> {
    let bsp = Bsp::load(&opts.map).map_err(|e| format!("loading {}: {e}", opts.map.display()))?;
    log::info!(
        "loaded {} ({} faces, {} leaves)",
        opts.map.display(),
        bsp.faces.len(),
        bsp.leaves.len()
    );
    let palette = world::load_palette(&opts.palette);
    let mesh = world::build(&bsp, &palette);
    let faces_total = mesh
        .face_ranges
        .iter()
        .filter(|r| r.index_count > 0)
        .count();
    let sim = Sim::at(&bsp, opts.start);
    // Through the hub: log in and get a ticket first; the ticket names the zone, its address
    // and its certificate. The map comes from `Welcome` and is loaded then.
    let (connect, cert, token, hub, zone_name) = match (opts.hub, opts.connect) {
        (Some(hub_addr), _) => {
            let cert_der = std::fs::read(&opts.hub_cert)
                .map_err(|e| format!("reading hub certificate {}: {e}", opts.hub_cert.display()))?;
            let (session, ticket) = HubSession::enter(HubLogin {
                hub: hub_addr,
                cert_der,
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
            (
                Some(ticket.addr),
                ticket.cert_der,
                bitcode::encode(&ticket.token),
                Some(session),
                ticket.zone,
            )
        }
        (None, Some(addr)) => {
            let cert = std::fs::read(&opts.cert)
                .map_err(|e| format!("reading zone certificate {}: {e}", opts.cert.display()))?;
            (Some(addr), cert, Vec::new(), None, String::new())
        }
        (None, None) => (None, Vec::new(), Vec::new(), None, String::new()),
    };
    let online = match connect {
        Some(addr) => {
            let map_hash = fnv1a64(&std::fs::read(&opts.map)?);
            log::info!("connecting to {addr} as {}", opts.name);
            Some(Online {
                net: NetClient::connect(
                    addr,
                    cert,
                    opts.name.clone(),
                    opts.build.clone(),
                    opts.team,
                    token,
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
                stalls: Vec::new(),
                kills: 0,
                deaths: 0,
                map_hash,
                respec_note: String::new(),
                hub,
                pending_travel: None,
                zone_name,
            })
        }
        None => None,
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let viewport = if opts.third_person {
        Viewport::Third
    } else {
        Viewport::First
    };
    let mut app = App {
        opts,
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
    };
    app.bench_yaw0 = app.sim.yaw;
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

impl App {
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
        if self.input.just_pressed.remove(&KeyCode::KeyT)
            && let Ok(target) = std::env::var("GM_TRAVEL_TO")
        {
            o.net.send_control(Control::Travel(target.clone()));
            o.respec_note = format!("travel to {target} requested");
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
        let bsp = &self.bsp;
        let viewport = self.viewport;
        let o = self.online.as_mut()?;
        for ev in o.net.poll() {
            match ev {
                NetEvent::Welcome {
                    entity,
                    hz,
                    map,
                    map_hash,
                } => {
                    if map_hash != o.map_hash {
                        // Another map: load it (a zone change through the hub lands here).
                        let path = self.opts.maps_dir.join(format!("{map}.bsp"));
                        match std::fs::read(&path) {
                            Ok(bytes) if fnv1a64(&bytes) == map_hash => {
                                log::info!("zone runs map {map}; loading {}", path.display());
                                self.pending_map = Some(path);
                                o.map_hash = map_hash;
                            }
                            _ => {
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
                    o.rate = TickRate::new(hz as u32);
                    o.welcome = Some((entity, hz));
                    log::info!("joined as entity {entity} on {map} at {hz} Hz");
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
                    } => match addr.parse::<std::net::SocketAddr>() {
                        Ok(addr) => {
                            log::info!("travel ticket for {zone} at {addr}");
                            o.pending_travel = Some((zone, addr, cert_der, token));
                        }
                        Err(e) => log::error!("bad travel address {addr}: {e}"),
                    },
                    Control::TravelRefused(reason) => {
                        o.respec_note = format!("travel refused: {reason}");
                        log::info!("{}", o.respec_note);
                    }
                    Control::Roster(players) => {
                        o.names.clear();
                        for p in players {
                            o.names.insert(p.id, (p.name, p.team, p.model));
                        }
                    }
                    Control::PlayerInfo {
                        id,
                        name,
                        team,
                        model,
                    } => {
                        o.names.insert(id, (name, team, model));
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
                    self.exit_requested = true;
                    event_loop.exit();
                    return None;
                }
            }
        }
        // A travel ticket: say goodbye here, connect there (the body stays as a ghost until
        // the other zone claims it, HUB.md 3.3).
        if let Some((zone, addr, cert_der, token)) = o.pending_travel.take() {
            o.net.close();
            match NetClient::connect(
                addr,
                cert_der,
                self.opts.name.clone(),
                None,
                self.opts.team,
                token,
            ) {
                Ok(net) => {
                    o.net = net;
                    o.welcome = None;
                    o.client = None;
                    o.pack = None;
                    o.names.clear();
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
        while o.accumulator >= dt && steps < MAX_STEPS_PER_FRAME {
            let (yaw, pitch) = match viewport {
                Viewport::First => (self.sim.yaw, self.sim.pitch),
                Viewport::Third => {
                    let eye = c.mover.eye();
                    let camera = third_person_camera(bsp, eye, self.sim.yaw, self.sim.pitch);
                    re_aim(bsp, &bodies, camera, self.sim.yaw, self.sim.pitch, eye)
                }
            };
            self.aim = (yaw, pitch);
            let input = self.input.sim_input(yaw, pitch);
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
        let my_team = o.team;
        let eye = {
            let alpha = (o.accumulator / o.rate.dt()).clamp(0.0, 1.0);
            o.prev_origin.lerp(o.curr_origin, alpha) + Vec3::Z * c.mover.mv.hull.eye_height()
        };
        let camera = match viewport {
            Viewport::First => eye,
            Viewport::Third => third_person_camera(bsp, eye, self.sim.yaw, self.sim.pitch),
        };
        for e in c.others_at(t) {
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
                    let r = match e.spawn {
                        SpawnInfo::Area { radius, .. } => radius as f32,
                        _ => 32.0,
                    };
                    self.entities.push(EntityDraw {
                        mins: e.pos - Vec3::new(r, r, 0.0),
                        maxs: e.pos + Vec3::new(r, r, 2.0),
                        color: [1.0, 0.5, 0.1, 1.0],
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
        match viewport {
            Viewport::First => Some((eye, self.sim.yaw, self.sim.pitch)),
            Viewport::Third => {
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
                Some((camera, self.sim.yaw, self.sim.pitch))
            }
        }
    }

    /// Replace the world (BSP, mesh, renderer) with another map.
    fn switch_map(&mut self, path: &std::path::Path) {
        let bsp = match Bsp::load(path) {
            Ok(b) => b,
            Err(e) => {
                log::error!("loading {}: {e}", path.display());
                return;
            }
        };
        let palette = world::load_palette(&self.opts.palette);
        let mesh = world::build(&bsp, &palette);
        self.faces_total = mesh
            .face_ranges
            .iter()
            .filter(|r| r.index_count > 0)
            .count();
        if let Some(a) = &mut self.active {
            a.renderer.set_world(&a.gpu, &mesh);
            a.current_leaf = None;
        } else {
            self.mesh = Some(mesh);
        }
        self.sim = Sim::new(&bsp);
        self.bsp = bsp;
        self.opts.map = path.to_path_buf();
        log::info!("switched to {}", path.display());
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(path) = self.pending_map.take() {
            self.switch_map(&path);
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
            self.input.just_pressed.clear();
            self.sim.advance(&self.bsp, &input, frame_dt);
            self.entities.clear();
            self.bodies.clear();
            match self.viewport {
                Viewport::First => (self.sim.eye(), self.sim.yaw, self.sim.pitch),
                Viewport::Third => {
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
        };

        let Some(a) = &mut self.active else { return };
        let leaf = self.bsp.leaf_for_point(camera);
        if a.current_leaf != Some(leaf) {
            a.current_leaf = Some(leaf);
            if leaf == 0 {
                a.renderer.set_visible_faces(&a.gpu, None);
            } else {
                let faces = self.bsp.visible_faces(leaf);
                a.renderer.set_visible_faces(&a.gpu, Some(&faces));
            }
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
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
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
        a.renderer.render(
            &a.gpu,
            &view,
            view_proj(camera, cam_yaw, cam_pitch, aspect),
            &self.entities,
            &a.avatars.draws,
        );
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
        if self.opts.seconds > 0.0 && self.started.elapsed().as_secs_f32() >= self.opts.seconds {
            event_loop.exit();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("gamengine")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                self.opts.width,
                self.opts.height,
            ));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("window creation failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let result = (|| -> Result<Active, Error> {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let surface = instance.create_surface(window.clone())?;
            let gpu = Gpu::new(&instance, Some(&surface), self.opts.software)?;
            let size = window.inner_size();
            let caps = surface.get_capabilities(&gpu.adapter);
            let mut config = surface
                .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
                .ok_or("surface not supported by the adapter")?;
            config.format = caps
                .formats
                .iter()
                .copied()
                .find(|f| f.is_srgb())
                .unwrap_or(caps.formats[0]);
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
                "surface {}x{} {:?} {:?}",
                config.width,
                config.height,
                config.format,
                config.present_mode
            );
            let mesh = self.mesh.take().ok_or("world mesh already consumed")?;
            let mut renderer =
                Renderer::new(&gpu, config.format, &mesh, (config.width, config.height));
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
            Ok(Active {
                window: window.clone(),
                surface,
                config,
                gpu,
                renderer,
                avatars,
                current_leaf: None,
            })
        })();
        match result {
            Ok(a) => {
                self.active = Some(a);
                if self.opts.bench_frames.is_none() {
                    self.set_grab(true);
                }
                self.last_frame = Instant::now();
                window.request_redraw();
            }
            Err(e) => {
                log::error!("renderer setup failed: {e}");
                self.exit_requested = true;
                event_loop.exit();
            }
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
                if self.opts.bench_frames.is_none() {
                    self.set_grab(true);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => {
                    if !self.grabbed && self.opts.bench_frames.is_none() {
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
                                KeyCode::KeyQ => event_loop.exit(),
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
        if let Some(a) = &self.active {
            a.window.request_redraw();
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
