//! Windowed client: winit event loop, fixed 64 Hz simulation with interpolated rendering,
//! mouse look, PVS-culled drawing, optional benchmark mode. Offline it runs the local
//! simulation; with `--connect` it predicts the own entity, reconciles against the zone and
//! interpolates everyone else (PROTOCOL.md 7).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use gm_bsp::Bsp;
use gm_core::movement::{MoveInput, MoveVars, PlayerState, player_move};
use gm_core::sim::{Input as SimInput, buttons};
use gm_core::tick::TickRate;
use gm_core::trace::Hull;
use gm_net::client::ClientState;
use gm_net::control::Control;
use gm_net::snapshot::EntityKind;
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
        let alpha = (self.accumulator / self.rate.dt()).clamp(0.0, 1.0);
        self.prev.origin.lerp(self.curr.origin, alpha)
            + Vec3::new(0.0, 0.0, self.curr.hull.eye_height())
    }
}

/// Networked play: the zone connection plus the shared prediction state.
struct Online {
    net: NetClient,
    client: Option<ClientState>,
    accumulator: f32,
    prev_origin: Vec3,
    curr_origin: Vec3,
    last_snapshot: Instant,
    rate: TickRate,
    names: HashMap<u32, String>,
    kills: u32,
    deaths: u32,
    map_hash: u64,
}

impl Online {
    fn eye(&self) -> Vec3 {
        let alpha = (self.accumulator / self.rate.dt()).clamp(0.0, 1.0);
        let hull = self
            .client
            .as_ref()
            .map_or(Hull::Player, |c| c.mover.mv.hull);
        self.prev_origin.lerp(self.curr_origin, alpha) + Vec3::new(0.0, 0.0, hull.eye_height())
    }
}

#[derive(Default)]
struct Input {
    keys: HashSet<KeyCode>,
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

    /// Left click swings, right click shoots, shift dashes (Phase 2 kit).
    fn sim_input(&self, yaw: f32, pitch: f32) -> SimInput {
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
        if self.down(KeyCode::ShiftLeft) || self.down(KeyCode::ShiftRight) {
            b |= buttons::ABILITY1;
        }
        SimInput {
            buttons: b,
            yaw,
            pitch,
            forward,
            side,
            ability: 0,
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
                net: NetClient::connect(addr, cert, opts.name.clone())?,
                client: None,
                accumulator: 0.0,
                prev_origin: sim.curr.origin,
                curr_origin: sim.curr.origin,
                last_snapshot: Instant::now(),
                rate: TickRate::COMBAT,
                names: HashMap::new(),
                kills: 0,
                deaths: 0,
                map_hash,
            })
        }
        None => None,
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        opts,
        bsp,
        mesh: Some(mesh),
        active: None,
        sim,
        online,
        input: Input::default(),
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
        log::info!(
            "{} frames, {:.1} fps average",
            report.frames,
            report.fps_avg
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
fn id_color(id: u32, alive: bool) -> [f32; 4] {
    if !alive {
        return [0.25, 0.25, 0.25, 1.0];
    }
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

    /// Network events, prediction ticks and the entity list for this frame. Returns the eye.
    fn online_frame(&mut self, frame_dt: f32, event_loop: &ActiveEventLoop) -> Option<Vec3> {
        let bsp = &self.bsp;
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
                    o.client = Some(ClientState::new(entity, o.rate));
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
                    Control::PlayerInfo { id, name } => {
                        o.names.insert(id, name);
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
                                .cloned()
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
            return Some(self.sim.eye());
        };
        let dt = o.rate.dt();
        o.accumulator += frame_dt.min(0.25);
        let mut steps = 0;
        while o.accumulator >= dt && steps < MAX_STEPS_PER_FRAME {
            let input = self.input.sim_input(self.sim.yaw, self.sim.pitch);
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
        for e in c.others_at(t) {
            match e.kind {
                EntityKind::Player => {
                    let alive = e.alive();
                    let mins = e.pos + Hull::Player.mins();
                    let mut maxs = e.pos + Hull::Player.maxs();
                    if !alive {
                        maxs.z = mins.z + 12.0;
                    }
                    self.entities.push(EntityDraw {
                        mins,
                        maxs,
                        color: id_color(e.id, alive),
                    });
                }
                EntityKind::Projectile => {
                    self.entities.push(EntityDraw {
                        mins: e.pos - Vec3::splat(2.5),
                        maxs: e.pos + Vec3::splat(2.5),
                        color: [1.0, 0.9, 0.3, 1.0],
                    });
                }
            }
        }
        c.prune(t);
        Some(o.eye())
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

        let eye = if self.online.is_some() {
            match self.online_frame(frame_dt, event_loop) {
                Some(eye) => eye,
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
            self.sim.advance(&self.bsp, &input, frame_dt);
            self.entities.clear();
            self.sim.eye()
        };

        let Some(a) = &mut self.active else { return };
        let leaf = self.bsp.leaf_for_point(eye);
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
            view_proj(eye, self.sim.yaw, self.sim.pitch, aspect),
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
            let title = match &self.online {
                Some(o) => {
                    let (hp, corr, delay) = o.client.as_ref().map_or((0, 0, 0), |c| {
                        (c.own_health, c.stats.corrections, c.delay_ticks)
                    });
                    format!(
                        "gamengine  hp {hp}  kills {}  deaths {}  {} players  delay {delay} ticks  corrections {corr}  {:.0} fps",
                        o.kills,
                        o.deaths,
                        o.names.len(),
                        r.fps_avg
                    )
                }
                None => format!(
                    "gamengine  {:.0} fps  {:.2} ms  {} / {} faces",
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
                            self.input.keys.insert(code);
                            match code {
                                KeyCode::Escape => self.set_grab(false),
                                KeyCode::KeyQ => event_loop.exit(),
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
