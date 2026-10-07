//! Watching a replay (ANTICHEAT.md 3.4): a `.gmr` played through the ordinary renderer. No
//! connection and no prediction: bodies are interpolated between recorded frames, and the
//! camera sits in a recorded body's eyes with its recorded view angles, so what it looked at
//! is what the reviewer sees. Where a body's camera really was (a third-person player's is
//! behind it) and what the zone was sending it are not in a replay; the eyes are.

use std::collections::VecDeque;

use glam::Vec3;
use gm_core::trace::Hull;
use gm_net::client::RenderEntity;
use gm_net::snapshot::{EntityKind, SpawnInfo};
use gm_replay::aim::{AimStats, Shot};
use gm_replay::{Event, Replay, RosterEntry};

use crate::avatars::{Body, OWN};
use crate::hud::{self, Hud};
use crate::render::EntityDraw;

/// Lines of what happened kept on the screen, and for how long (in replay seconds).
const LINES_KEPT: usize = 8;
const LINE_SECS: f32 = 6.0;

pub struct Playback {
    pub replay: Replay,
    everyone: Vec<RosterEntry>,
    /// Seconds from the first frame.
    pub time: f32,
    pub speed: f32,
    pub paused: bool,
    /// The body whose eyes the camera is in.
    pub follow: u32,
    shots: Vec<Shot>,
    stats: std::collections::BTreeMap<u32, AimStats>,
    /// Frames whose events have been shown.
    shown: usize,
    lines: VecDeque<(f32, String, [f32; 4])>,
}

impl Playback {
    pub fn open(bytes: &[u8], follow: Option<&str>, from: f32) -> Result<Playback, String> {
        let replay = Replay::read(bytes).map_err(|e| e.to_string())?;
        let everyone = replay.everyone();
        let (stats, shots) = gm_replay::aim::analyse(&replay);
        let humans: Vec<&RosterEntry> = everyone.iter().filter(|r| r.human()).collect();
        let follow = match follow {
            Some(name) => humans
                .iter()
                .find(|r| r.name == name)
                .map(|r| r.id)
                .ok_or_else(|| {
                    format!(
                        "nobody called {name} in this replay; there are: {}",
                        humans
                            .iter()
                            .map(|r| r.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?,
            None => humans.first().map_or(0, |r| r.id),
        };
        let mut p = Playback {
            time: 0.0,
            speed: 1.0,
            paused: false,
            follow,
            shots,
            stats,
            shown: 0,
            lines: VecDeque::new(),
            everyone,
            replay,
        };
        p.seek(from);
        Ok(p)
    }

    pub fn seconds(&self) -> f32 {
        self.replay.seconds()
    }

    pub fn name(&self, id: u32) -> String {
        self.everyone
            .iter()
            .find(|r| r.id == id)
            .map_or_else(|| format!("#{id}"), |r| r.name.clone())
    }

    /// Jump to a time: the lines on screen start again from there.
    pub fn seek(&mut self, to: f32) {
        self.time = to.clamp(0.0, self.seconds());
        self.shown = self.replay.locate(self.time).0;
        self.lines.clear();
    }

    /// The next (or previous) client-driven body.
    pub fn cycle(&mut self, step: i32) {
        let humans: Vec<u32> = self
            .everyone
            .iter()
            .filter(|r| r.human())
            .map(|r| r.id)
            .collect();
        if humans.is_empty() {
            return;
        }
        let at = humans.iter().position(|id| *id == self.follow).unwrap_or(0) as i32;
        self.follow = humans[(at + step).rem_euclid(humans.len() as i32) as usize];
    }

    /// One tick forward or back while paused.
    pub fn step(&mut self, ticks: i32) {
        let dt = 1.0 / self.replay.header.hz.max(1) as f32;
        self.time = (self.time + ticks as f32 * dt).clamp(0.0, self.seconds());
        if ticks < 0 {
            self.shown = self.shown.min(self.replay.locate(self.time).0);
        }
    }

    /// Advance by `frame_dt` of wall time and collect what happened on the way.
    pub fn advance(&mut self, frame_dt: f32) {
        if !self.paused {
            self.time = (self.time + frame_dt * self.speed).min(self.seconds());
        }
        let (upto, _) = self.replay.locate(self.time);
        let hz = self.replay.header.hz.max(1) as f32;
        while self.shown < upto {
            self.shown += 1;
            let at = self.shown as f32 / hz;
            let Some(frame) = self.replay.frames.get(self.shown) else {
                break;
            };
            let mut lines: Vec<(String, [f32; 4])> = Vec::new();
            for e in &frame.events {
                match *e {
                    Event::Killed { victim, killer } => lines.push((
                        format!("{} killed {}", self.name(killer), self.name(victim)),
                        hud::ORANGE,
                    )),
                    Event::Hit {
                        attacker,
                        target,
                        amount,
                        ..
                    } if attacker == self.follow || target == self.follow => lines.push((
                        format!(
                            "{} hit {} for {amount}",
                            self.name(attacker),
                            self.name(target)
                        ),
                        if attacker == self.follow {
                            hud::GREEN
                        } else {
                            hud::RED
                        },
                    )),
                    Event::Shot {
                        projectile, owner, ..
                    } if owner == self.follow => {
                        // The numbers of this shot, as the analysis has them.
                        if let Some(s) = self.shots.iter().find(|s| s.projectile == projectile) {
                            lines.push((
                                format!(
                                    "shot at {}: error {:.1} snap {:.0} steady {:.2}{}{}{}",
                                    self.name(s.target),
                                    s.error,
                                    s.snap,
                                    s.steady.min(99.0),
                                    if s.flick { " FLICK" } else { "" },
                                    if s.lock { " LOCK" } else { "" },
                                    if s.laser { " LASER" } else { "" },
                                ),
                                if s.flick || s.lock || s.laser {
                                    hud::YELLOW
                                } else {
                                    hud::DIM
                                },
                            ));
                        }
                    }
                    Event::Reaction { viewer, body, ms } if viewer == self.follow => lines.push((
                        format!("turned onto {} in {ms} ms", self.name(body)),
                        hud::DIM,
                    )),
                    _ => {}
                }
            }
            for (text, colour) in lines {
                self.lines.push_back((at, text, colour));
            }
            while self.lines.len() > LINES_KEPT {
                self.lines.pop_front();
            }
        }
    }

    /// The followed body at the current time.
    pub fn followed(&self, entities: &[RenderEntity]) -> Option<RenderEntity> {
        entities
            .iter()
            .find(|e| e.id == self.follow && e.kind == EntityKind::Player)
            .copied()
    }

    /// The scene at the current time: bodies and boxes to draw, and the camera in the
    /// followed body's eyes `(eye, yaw, pitch)`. `own_body`: draw the followed body too (a
    /// camera outside it).
    pub fn scene(
        &self,
        own_body: bool,
        bodies: &mut Vec<Body>,
        boxes: &mut Vec<EntityDraw>,
    ) -> (Vec3, f32, f32) {
        let entities = self.replay.entities_at(self.time);
        let me = self.followed(&entities);
        let (eye, yaw, pitch) = match me {
            Some(m) => (m.pos + Vec3::Z * Hull::Player.eye_height(), m.yaw, m.pitch),
            None => (Vec3::new(0.0, 0.0, 128.0), 0.0, 0.0),
        };
        for e in &entities {
            match e.kind {
                EntityKind::Player => {
                    let SpawnInfo::Player {
                        frame,
                        aspects,
                        armour,
                        ..
                    } = e.spawn
                    else {
                        continue;
                    };
                    let own = e.id == self.follow;
                    if own && !own_body {
                        continue;
                    }
                    bodies.push(Body {
                        key: if own { OWN } else { e.id },
                        origin: e.pos,
                        yaw: e.yaw,
                        pitch: e.pitch,
                        anim: e.anim,
                        frame,
                        armour,
                        aspects,
                        status: e.status,
                        model: None,
                        distance: (e.pos - eye).length(),
                        lit: 0.0,
                        prop: None,
                    });
                }
                EntityKind::Projectile => boxes.push(EntityDraw {
                    mins: e.pos - Vec3::splat(2.5),
                    maxs: e.pos + Vec3::splat(2.5),
                    color: [1.0, 0.9, 0.3, 1.0],
                }),
                EntityKind::Area => {
                    let (r, harmful) = match e.spawn {
                        SpawnInfo::Area {
                            radius, harmful, ..
                        } => (radius as f32, harmful),
                        _ => (32.0, true),
                    };
                    boxes.push(EntityDraw {
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
        (eye, yaw, pitch)
    }

    /// The reviewer's HUD: where in the replay, whose eyes, its health and its numbers
    /// over the file, and what happened lately.
    pub fn hud(&self, hud: &mut Hud) {
        let (w, h) = hud.size;
        let s = if h >= 1000.0 { 3.0 } else { 2.0 };
        let line = (hud.cap() + 5.0) * s;
        hud.rect(w * 0.5 - 2.0, h * 0.5 - 2.0, 4.0, 4.0, hud::SHADE);
        hud.rect(w * 0.5 - 1.0, h * 0.5 - 1.0, 2.0, 2.0, hud::WHITE);
        let head = format!(
            "replay {:.1} / {:.1} s  x{}{}  {}",
            self.time,
            self.seconds(),
            self.speed,
            if self.paused { " paused" } else { "" },
            self.replay.header.zone
        );
        hud.label(16.0, 16.0, s, hud::WHITE, &head);
        let who = self.everyone.iter().find(|r| r.id == self.follow);
        let entities = self.replay.entities_at(self.time);
        let me = self.followed(&entities);
        if let Some(r) = who {
            let health = me.and_then(|m| m.health).unwrap_or(0);
            hud.label(
                16.0,
                16.0 + line,
                s,
                hud::YELLOW,
                &format!("{}  {}  team {}  hp {health}", r.name, r.build, r.team),
            );
            if let Some(a) = self.stats.get(&r.id) {
                let rules: Vec<&str> = a.rules().iter().map(|x| x.name()).collect();
                hud.label(
                    16.0,
                    16.0 + 2.0 * line,
                    s * 0.5,
                    hud::DIM,
                    &format!(
                        "this file: {} shots  {} hits  {} flicks  {}/{} locks  {}/{} lasers  {}",
                        a.analysed,
                        a.hits,
                        a.flicks,
                        a.locks,
                        a.moving_shots,
                        a.lasers,
                        a.long_shots,
                        if rules.is_empty() {
                            "no rule broken".to_string()
                        } else {
                            rules.join(" ")
                        }
                    ),
                );
            }
        }
        let hint = "[ ] player   space pause   , . step   arrows seek   1-4 speed   v third person";
        hud.label(
            w - hud.width(s * 0.5, hint) - 16.0,
            h - 14.0 * s,
            s * 0.5,
            hud::DIM,
            hint,
        );
        let mut y = h - 16.0 - line * self.lines.len() as f32;
        for (at, text, colour) in &self.lines {
            if self.time - at > LINE_SECS || self.time < *at {
                y += line;
                continue;
            }
            hud.label(16.0, y, s, *colour, text);
            y += line;
        }
    }
}
