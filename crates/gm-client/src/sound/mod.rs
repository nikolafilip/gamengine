//! Sound (SOUND.md): patches synthesized at start, cues inferred from what the client
//! draws, a mixer of our own on the native device, the browser's own nodes in the browser.

pub mod cues;
#[cfg(not(target_arch = "wasm32"))]
pub mod device;
pub mod mixer;
pub mod synth;
#[cfg(target_arch = "wasm32")]
pub mod web;

use std::sync::Arc;

use glam::Vec3;

use cues::{Heard, OwnNow, Sample, Scene};
use gm_core::sim::Action;
use mixer::{Command, Queue};
use synth::{CUES, Cue};

/// Nothing is heard from further than this (units).
pub const HEARING: f32 = 2048.0;
/// The distance at which a cue is at half its gain.
pub const REFERENCE: f32 = 256.0;
/// The air's gain, under the master.
const AIR_GAIN: f32 = 0.35;

/// Where the cues are heard from: the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub pos: Vec3,
    /// Radians, the camera's yaw (forward is `(cos, sin, 0)`).
    pub yaw: f32,
}

/// Gain and pan of a cue at `at` for a listener (SOUND.md 3.1): `(left, right)`, or
/// nothing when it is too far to hear.
pub fn ears(listener: Listener, at: Option<Vec3>) -> Option<(f32, f32)> {
    let Some(at) = at else {
        return Some((1.0, 1.0));
    };
    let to = at - listener.pos;
    let d = to.length();
    if d >= HEARING {
        return None;
    }
    let gain = 1.0 / (1.0 + d / REFERENCE);
    // Constant power between the ears from the direction's side of the listener's
    // forward; something at the listener itself is in the middle.
    let right = Vec3::new(listener.yaw.sin(), -listener.yaw.cos(), 0.0);
    let flat = to.truncate().length();
    let pan = if flat > 1.0 {
        (to.dot(right) / flat).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    Some((gain * angle.cos(), gain * angle.sin()))
}

/// What the client plays through.
#[cfg(not(target_arch = "wasm32"))]
enum Out {
    Device(device::Device),
    Dump(Box<device::Dump>),
    None,
}

/// Counts of what was started, in `Cue`'s order, for the report.
#[derive(Default)]
pub struct Counts(pub [u32; CUES]);

impl Counts {
    pub fn total(&self) -> u32 {
        self.0.iter().sum()
    }

    pub fn of(&self, cue: Cue) -> u32 {
        self.0[cue as usize]
    }
}

/// The sound of the game: one of these in the app.
pub struct Sound {
    scene: Scene,
    queue: Queue,
    #[cfg(not(target_arch = "wasm32"))]
    out: Out,
    #[cfg(target_arch = "wasm32")]
    web: web::Audio,
    pub counts: Counts,
    /// Cues that could not be started (no device, or the browser not yet allowed).
    pub dropped: u32,
    started: u32,
    volume: u8,
    muted: bool,
    air: [Option<Cue>; 2],
    /// The hash of every patch rendered, for the report (SOUND.md 2: both builds alike).
    patches_hash: u64,
    /// A dump renders the cues alone, at full gain: what the gate measures (SOUND.md 7).
    dumping: bool,
    /// Entities first seen, by kind (bodies, projectiles, areas): what there was to hear.
    saw: [u32; 3],
}

impl Sound {
    /// Synthesize the patches and open the output. `dump`: natively, render into this
    /// WAV from the frame clock instead of a device.
    pub fn new(volume: u8, muted: bool, dump: Option<std::path::PathBuf>) -> Sound {
        let patches = Arc::new(synth::render_all());
        let patches_hash = synth::fingerprint_all(&patches);
        let dumping = dump.is_some();
        let queue = Queue::default();
        #[cfg(not(target_arch = "wasm32"))]
        let out = match dump {
            Some(path) => Out::Dump(Box::new(device::Dump::new(
                patches.clone(),
                queue.clone(),
                path,
            ))),
            None => match device::Device::open(patches.clone(), queue.clone()) {
                Some(d) => Out::Device(d),
                None => {
                    log::info!("sound: no device; the game plays silently");
                    Out::None
                }
            },
        };
        #[cfg(target_arch = "wasm32")]
        let _ = dump;
        let mut sound = Sound {
            scene: Scene::new(1.0 / 64.0),
            queue,
            #[cfg(not(target_arch = "wasm32"))]
            out,
            #[cfg(target_arch = "wasm32")]
            web: web::Audio::new(patches),
            counts: Counts::default(),
            dropped: 0,
            started: 0,
            volume,
            muted,
            air: [None, None],
            patches_hash,
            dumping,
            saw: [0; 3],
        };
        sound.send_master();
        sound
    }

    fn master(&self) -> f32 {
        if self.dumping {
            1.0
        } else if self.muted {
            0.0
        } else {
            let v = self.volume.min(100) as f32 / 100.0;
            v * v
        }
    }

    /// The samples' clock: seconds a tick of the zone played.
    pub fn set_rate(&mut self, dt: f32) {
        self.scene = Scene::new(dt);
    }

    fn send_master(&mut self) {
        let m = self.master();
        self.send(Command::Master(m));
    }

    pub fn set_volume(&mut self, volume: u8, muted: bool) {
        self.volume = volume;
        self.muted = muted;
        self.send_master();
    }

    fn send(&mut self, c: Command) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.queue.send(c);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = &self.queue;
            if !self.web.apply(c) {
                self.dropped += 1;
            }
        }
    }

    /// Start a cue at a place, as the listener hears it.
    fn start(&mut self, listener: Listener, heard: Heard) {
        let Some((left, right)) = ears(listener, heard.at) else {
            return;
        };
        self.started = self.started.wrapping_add(1);
        self.counts.0[heard.cue as usize] += 1;
        let pitch = match heard.cue {
            Cue::StepA | Cue::StepB | Cue::Hit | Cue::Stagger | Cue::Swing | Cue::Land => {
                cues::pitch_of(heard.key, self.started)
            }
            _ => 1.0,
        };
        self.send(Command::Cue {
            cue: heard.cue,
            left,
            right,
            pitch,
        });
    }

    /// Every new sample of every entity, in tick order, up to the render tick, and the
    /// removal of what the render tick has passed (SOUND.md 3); the scene itself knows
    /// which samples it has read. `hits_world`: whether a segment hits the map.
    pub fn feed(
        &mut self,
        tracks: &std::collections::BTreeMap<u32, gm_net::client::Track>,
        render_tick: f32,
        hits_world: &dyn Fn(Vec3, Vec3) -> bool,
    ) {
        use gm_net::quant::{dequantize_pos3, wire_to_pitch, wire_to_yaw};
        use gm_net::snapshot::{SpawnInfo, flags};
        for (&id, track) in tracks {
            if track.removed_at.is_some_and(|at| render_tick >= at as f32) {
                // (Idempotent: an entity already let go of is not known.)
                self.scene.removed(id, hits_world);
                continue;
            }
            for &(tick, ref e) in &track.samples {
                if tick as f32 > render_tick {
                    break;
                }
                let (kind, owner, harmful) = match e.spawn {
                    SpawnInfo::Player { .. } => (cues::Kind::Body, 0, false),
                    SpawnInfo::Projectile { owner, .. } => (cues::Kind::Projectile, owner, true),
                    SpawnInfo::Area { owner, harmful, .. } => (cues::Kind::Area, owner, harmful),
                };
                let first = self.scene.sample(Sample {
                    id,
                    kind,
                    tick,
                    pos: Vec3::from(dequantize_pos3(e.pos)),
                    anim: e.anim,
                    on_ground: e.flags & flags::ON_GROUND != 0,
                    health: e.health,
                    owner,
                    harmful,
                    dir: if kind == cues::Kind::Projectile {
                        gm_core::sim::view_dir(wire_to_yaw(e.yaw), wire_to_pitch(e.pitch))
                    } else {
                        Vec3::ZERO
                    },
                });
                if first {
                    self.saw[e.spawn.kind() as usize] += 1;
                }
            }
        }
        // A track dropped before its removal was read (the tracks are pruned by the same
        // clock): the entity is gone all the same.
        let dropped: Vec<u32> = self
            .scene
            .known()
            .filter(|id| !tracks.contains_key(id))
            .collect();
        for id in dropped {
            self.scene.removed(id, hits_world);
        }
    }

    /// The own body this frame (SOUND.md 3): what the zone said of it since the last
    /// frame (each word with its tick), and what it did itself.
    pub fn own(&mut self, now: f32, o: OwnNow, anims: &[(u32, u8)], actions: &[Action]) {
        self.scene.own(now, o, anims, actions);
    }

    /// The end of a frame: the cues read this frame start, as the listener hears them,
    /// and what renders by the frame clock advances.
    pub fn end_frame(&mut self, frame_dt: f32, listener: Listener) {
        let heard = self.scene.take(listener.pos);
        for h in heard {
            self.start(listener, h);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Out::Dump(d) = &mut self.out {
            d.advance(frame_dt);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = frame_dt;
            self.web.frame();
        }
    }

    /// A cue at the listener (a click, a line arriving, a chime).
    pub fn play(&mut self, cue: Cue) {
        let here = Listener {
            pos: Vec3::ZERO,
            yaw: 0.0,
        };
        self.start(
            here,
            Heard {
                cue,
                at: None,
                key: 0,
            },
        );
    }

    /// The air of a map (SOUND.md 3), by its name until a map says it itself.
    pub fn air(&mut self, map: &str) {
        let name = map.rsplit('/').next().unwrap_or(map);
        let name = name.strip_suffix(".bsp").unwrap_or(name);
        let loops: [Option<Cue>; 2] = match name {
            "town" => [Some(Cue::Murmur), Some(Cue::Wind)],
            "dungeon" => [Some(Cue::Drone), None],
            "" => [None, None],
            _ => [Some(Cue::Wind), None],
        };
        self.set_air(loops);
    }

    /// The air as the map's worldspawn names it (`gm_ambience`): `wind`, `murmur`,
    /// `drone`, `none`, or two of them with a comma.
    pub fn air_named(&mut self, names: &str) {
        let mut loops = [None, None];
        for (slot, n) in names.split(',').map(str::trim).take(2).enumerate() {
            loops[slot] = match n {
                "wind" => Some(Cue::Wind),
                "murmur" => Some(Cue::Murmur),
                "drone" => Some(Cue::Drone),
                _ => None,
            };
        }
        self.set_air(loops);
    }

    fn set_air(&mut self, loops: [Option<Cue>; 2]) {
        // (A dump is of the cues: the gate reads silence between them.)
        let loops = if self.dumping { [None, None] } else { loops };
        if loops == self.air {
            return;
        }
        self.air = loops;
        for (slot, cue) in loops.iter().enumerate() {
            self.send(Command::Loop {
                slot: slot as u8,
                cue: *cue,
                gain: AIR_GAIN,
            });
        }
    }

    /// A zone left: every voice stops, the air fades, nothing is read from the next
    /// frame.
    pub fn quiet(&mut self) {
        self.scene.clear();
        self.air = [None, None];
        self.send(Command::Quiet);
    }

    /// Microseconds per 512 output frames (mean, max) of what rendered, when something
    /// did.
    pub fn block_us(&self) -> Option<(f64, u64)> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match &self.out {
                Out::Device(d) => Some(d.meter.per_512()),
                Out::Dump(d) => Some(d.meter.per_512()),
                Out::None => None,
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }

    /// One line for the log and the gates: what was started, by kind, and what it cost.
    pub fn report(&self) -> String {
        let mut line = format!(
            "sound: cues={} dropped={} steps={} swings={} hits={} staggers={} parries={} casts={} launches={} impacts={} bursts={} hurts={} deaths={} lands={} dashes={} clicks={} blips={} chimes={} patches=0x{:016x}",
            self.counts.total(),
            self.dropped,
            self.counts.of(Cue::StepA) + self.counts.of(Cue::StepB),
            self.counts.of(Cue::Swing),
            self.counts.of(Cue::Hit),
            self.counts.of(Cue::Stagger),
            self.counts.of(Cue::Parry),
            self.counts.of(Cue::Cast),
            self.counts.of(Cue::Launch),
            self.counts.of(Cue::Impact),
            self.counts.of(Cue::Burst),
            self.counts.of(Cue::Hurt),
            self.counts.of(Cue::Death),
            self.counts.of(Cue::Land),
            self.counts.of(Cue::Dash),
            self.counts.of(Cue::Click),
            self.counts.of(Cue::Blip),
            self.counts.of(Cue::Chime),
            self.patches_hash,
        );
        line.push_str(&format!(
            " saw_bodies={} saw_bolts={} saw_areas={}",
            self.saw[0], self.saw[1], self.saw[2]
        ));
        if let Some((mean, max)) = self.block_us() {
            line.push_str(&format!(" block_us_mean={mean:.1} block_us_max={max}"));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Out::Device(d) = &self.out {
            line.push_str(&format!(
                " device_rate={} device_buffer={}",
                d.rate, d.buffer
            ));
        }
        #[cfg(target_arch = "wasm32")]
        line.push_str(&self.web.report());
        line
    }

    /// The end of the run: a dump is written. Returns what to log.
    pub fn finish(&mut self) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Out::Dump(d) = &self.out {
            return Some(match d.finish() {
                Ok(secs) => format!("sound dump: {secs:.2} s written"),
                Err(e) => format!("sound dump: not written: {e}"),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cue_is_louder_near_and_on_its_side() {
        let l = Listener {
            pos: Vec3::ZERO,
            yaw: 0.0,
        };
        assert_eq!(ears(l, None), Some((1.0, 1.0)));
        let ahead = ears(l, Some(Vec3::new(256.0, 0.0, 0.0))).unwrap();
        assert!(
            (ahead.0 - ahead.1).abs() < 1e-5,
            "{ahead:?}: ahead is in the middle"
        );
        assert!((ahead.0 - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        // Forward is +x at yaw 0; the right ear is toward -y.
        let right = ears(l, Some(Vec3::new(0.0, -100.0, 0.0))).unwrap();
        assert!(right.1 > right.0 * 10.0, "{right:?}");
        let left = ears(l, Some(Vec3::new(0.0, 100.0, 0.0))).unwrap();
        assert!(left.0 > left.1 * 10.0, "{left:?}");
        let behind = ears(l, Some(Vec3::new(-100.0, 0.0, 0.0))).unwrap();
        assert!(
            (behind.0 - behind.1).abs() < 1e-5,
            "behind is as loud as ahead"
        );
        let near = ears(l, Some(Vec3::new(10.0, 0.0, 0.0))).unwrap();
        assert!(near.0 > ahead.0);
        assert_eq!(ears(l, Some(Vec3::new(HEARING, 0.0, 0.0))), None);
        assert!(ears(l, Some(Vec3::new(1900.0, 0.0, 0.0))).is_some());
        // Turned around, the sides swap.
        let turned = Listener {
            pos: Vec3::ZERO,
            yaw: std::f32::consts::PI,
        };
        let now_left = ears(turned, Some(Vec3::new(0.0, -100.0, 0.0))).unwrap();
        assert!(now_left.0 > now_left.1 * 10.0, "{now_left:?}");
    }
}
