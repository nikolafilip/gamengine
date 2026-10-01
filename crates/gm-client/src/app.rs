//! Windowed client: winit event loop, fixed 64 Hz simulation with interpolated rendering,
//! mouse look, PVS-culled drawing, optional benchmark mode. Offline it runs the local
//! simulation; with `--connect` it predicts the own entity, reconciles against the zone and
//! interpolates everyone else (PROTOCOL.md 7). Two viewports share the simulation
//! (VOCABULARY.md 9): first person aims from the eyes, third person aims the camera ray at a
//! world point and re-aims it from the eyes ("camera-to-muzzle re-aim").

use std::collections::{HashMap, HashSet};
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
use gm_core::vocab::Status;
use gm_net::client::ClientState;
use gm_net::control::{BuildChoice, Control};
use gm_net::snapshot::{EntityKind, flags};
use gm_net::transport::fnv1a64;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::net::{NetClient, NetEvent};
use crate::render::{EntityDraw, Gpu, Renderer, view_proj};
use crate::stats::{FrameStats, print_bench};
use crate::world::{self, WorldMesh};
use crate::{Error, Options};

/// Degrees per mouse count: Quake's `sensitivity 3` times `m_yaw 0.022`.
const SENSITIVITY: f32 = 0.066;
const MAX_STEPS_PER_FRAME: u32 = 8;
const BENCH_YAW_DEG_PER_S: f32 = 20.0;
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
        let (origin, yaw) = bsp
            .player_start()
            .unwrap_or((Vec3::new(0.0, 0.0, 64.0), 0.0));
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
    names: HashMap<u32, (String, u8)>,
    kills: u32,
    deaths: u32,
    map_hash: u64,
    respec_note: String,
}

impl Online {
    fn origin(&self) -> Vec3 {
        let alpha = (self.accumulator / self.rate.dt()).clamp(0.0, 1.0);
        self.prev_origin.lerp(self.curr_origin, alpha)
    }

    fn eye(&self) -> Vec3 {
        let hull = self
            .client
            .as_ref()
            .map_or(Hull::Player, |c| c.mover.mv.hull);
        self.origin() + Vec3::new(0.0, 0.0, hull.eye_height())
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
    /// The aim resolved by the last third-person tick, for the HUD.
    aim: (f32, f32),
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
    let sim = Sim::new(&bsp);
    let online = match opts.connect {
        Some(addr) => {
            let cert = std::fs::read(&opts.cert)
                .map_err(|e| format!("reading zone certificate {}: {e}", opts.cert.display()))?;
            let map_hash = fnv1a64(&std::fs::read(&opts.map)?);
            log::info!("connecting to {addr} as {}", opts.name);
            Some(Online {
                net: NetClient::connect(
                    addr,
                    cert,
                    opts.name.clone(),
                    opts.build.clone(),
                    opts.team,
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
                kills: 0,
                deaths: 0,
                map_hash,
                respec_note: String::new(),
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
        aim: (0.0, 0.0),
    };
    event_loop.run_app(&mut app)?;
    if let Some(o) = &mut app.online {
        o.net.close();
    }

    let report = app.stats.report();
    if let (Some(_), Some(info)) = (app.opts.bench_frames, &app.adapter_info) {
        let faces = app.active.as_ref().map_or(0, |a| a.renderer.faces_drawn);
        print_bench(&report, info, "windowed", faces, app.faces_total);
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

/// Stable, saturated colour per entity id.
fn id_color(id: u32) -> [f32; 4] {
    let h = (id.wrapping_mul(2654435761) >> 8) as f32 / (1u32 << 24) as f32 * 6.0;
    let c = 0.9;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + 0.1, g + 0.1, b + 0.1, 1.0]
}

/// Team colours: own side cool, the other side warm; no team keeps the id colour.
fn player_color(id: u32, team: u8, my_team: u8, alive: bool, status: u16) -> [f32; 4] {
    if !alive {
        return [0.25, 0.25, 0.25, 1.0];
    }
    let mut c = if team == 0 || my_team == 0 {
        id_color(id)
    } else if team == my_team {
        [0.25, 0.45, 1.0, 1.0]
    } else {
        [1.0, 0.3, 0.2, 1.0]
    };
    // Auras: a staggered or frozen body reads darker, a hasted one brighter.
    if status & (1 << Status::Stagger.index()) != 0 || status & (1 << Status::Root.index()) != 0 {
        c = [c[0] * 0.5, c[1] * 0.5, c[2] * 0.5, 1.0];
    } else if status & (1 << Status::Haste.index()) != 0 {
        c = [c[0].min(1.0) + 0.3, c[1] + 0.3, c[2] + 0.3, 1.0];
    }
    c
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
                        log::error!(
                            "zone runs map {map} with hash {map_hash:016x}; ours is {:016x} (wrong map build)",
                            o.map_hash
                        );
                        self.exit_requested = true;
                        event_loop.exit();
                        return None;
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
                    Control::PlayerInfo { id, name, team } => {
                        o.names.insert(id, (name, team));
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
                                .map(|(n, _)| n.clone())
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
        let my_team = o.team;
        for e in c.others_at(t) {
            match e.kind {
                EntityKind::Player => {
                    let alive = e.alive();
                    let mins = e.pos + Hull::Player.mins();
                    let mut maxs = e.pos + Hull::Player.maxs();
                    if !alive {
                        maxs.z = mins.z + 12.0;
                    }
                    let mut color = player_color(e.id, e.team(), my_team, alive, e.status);
                    if alive && e.flags & flags::GUARDING != 0 {
                        color = [color[0], color[1], color[2].max(0.6), 1.0];
                    }
                    self.entities.push(EntityDraw { mins, maxs, color });
                    // A small box shows where the body is facing.
                    if alive {
                        let (fwd, _) = yaw_vectors(e.yaw);
                        let nose = e.pos + fwd * 18.0 + Vec3::Z * 20.0;
                        self.entities.push(EntityDraw {
                            mins: nose - Vec3::splat(3.0),
                            maxs: nose + Vec3::splat(3.0),
                            color: [1.0, 1.0, 1.0, 1.0],
                        });
                    }
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
                        gm_net::snapshot::SpawnInfo::Area { radius, .. } => radius as f32,
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
        let alive = c.own_alive || !c.synced();
        let eye = o.eye();
        match viewport {
            Viewport::First => Some((eye, self.sim.yaw, self.sim.pitch)),
            Viewport::Third => {
                // Draw the own body too.
                let origin = o.origin();
                let mins = origin + Hull::Player.mins();
                let mut maxs = origin + Hull::Player.maxs();
                if !alive {
                    maxs.z = mins.z + 12.0;
                }
                self.entities.push(EntityDraw {
                    mins,
                    maxs,
                    color: [0.85, 0.85, 0.9, 1.0],
                });
                let camera = third_person_camera(bsp, eye, self.sim.yaw, self.sim.pitch);
                Some((camera, self.sim.yaw, self.sim.pitch))
            }
        }
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let frame_dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Mouse look is applied per frame for responsiveness; movement uses it at tick time.
        let bench = self.opts.bench_frames.is_some();
        if bench {
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
            match self.viewport {
                Viewport::First => (self.sim.eye(), self.sim.yaw, self.sim.pitch),
                Viewport::Third => {
                    let origin = self.sim.origin();
                    self.entities.push(EntityDraw {
                        mins: origin + Hull::Player.mins(),
                        maxs: origin + Hull::Player.maxs(),
                        color: [0.85, 0.85, 0.9, 1.0],
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
        a.renderer.render(
            &a.gpu,
            &view,
            view_proj(camera, cam_yaw, cam_pitch, aspect),
            &self.entities,
        );
        a.window.pre_present_notify();
        a.gpu.queue.present(frame);
        self.stats.frame();
        self.acquire_timeouts = 0;
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
                    format!(
                        "gamengine [{} team {} {vp}]  hp {hp}/{max_hp}  st {st:.0}  fo {fo:.0}  k {} d {}  {} players  delay {delay}  corr {corr}  {:.0} fps  {}",
                        o.build_name,
                        o.team,
                        o.kills,
                        o.deaths,
                        o.names.len(),
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
            let renderer = Renderer::new(&gpu, config.format, &mesh, (config.width, config.height));
            self.adapter_info = Some(gpu.info.clone());
            Ok(Active {
                window: window.clone(),
                surface,
                config,
                gpu,
                renderer,
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
