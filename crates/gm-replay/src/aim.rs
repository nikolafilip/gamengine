//! Aim statistics (ANTICHEAT.md 4): what a client's view did around each shot, computed from
//! recorded frames. The analyser sees exactly what a replay holds (wire-quantised entity
//! states and events), so a fight's numbers can be recomputed from its file.

use std::collections::{BTreeMap, VecDeque};

use bitcode::{Decode, Encode};
use glam::Vec3;
use gm_core::movement::MoveVars;
use gm_core::sim::view_dir;
use gm_core::trace::Hull;
use gm_net::quant;
use gm_net::snapshot::{EntityKind, EntityState, flags};

use crate::{Event, Hit, RosterEntry, apply_roster};

/// Ticks of view history a shot is judged on.
pub const WINDOW: usize = 8;
/// A shot is analysed when its nearest ideal aim is within this, degrees.
pub const TARGET_CONE_DEG: f32 = 25.0;
/// A body's velocity is taken over this many ticks: positions travel in quarter units, and
/// one tick's difference is 16 u/s coarse.
pub const VELOCITY_TICKS: u32 = 4;
/// "On target": the error is inside the body's half width seen from the shooter (never
/// less than `ON_TARGET_MIN_DEG`).
pub const BODY_HALF_WIDTH: f32 = 12.0;
pub const ON_TARGET_MIN_DEG: f32 = 0.6;

/// Flick: the view turned at least this across `SNAP_TICKS` ticks, the shot followed
/// within `FLICK_SETTLE` ticks of the turn's end, and it was on target.
pub const FLICK_SNAP_DEG: f32 = 35.0;
pub const SNAP_TICKS: u32 = 4;
pub const FLICK_SETTLE: u32 = 3;
/// A view that turns less than this in a tick has stopped turning.
pub const STILL_DEG: f32 = 1.0;
/// Lock: over the window the view stayed this steady *relative to the ideal aim* (the
/// standard deviation of the error, so a constant offset does not hide it) while the ideal
/// aim itself moved at least `LOCK_MOTION_DEG`, and the shot was on target.
pub const LOCK_STEADY_DEG: f32 = 0.8;
pub const LOCK_MOTION_DEG: f32 = 4.0;
/// Laser: the error at most this at `LASER_RANGE` or beyond (a long shot).
pub const LASER_ERROR_DEG: f32 = 0.2;
pub const LASER_RANGE: f32 = 600.0;
/// A hard shot: the lead (ideal aim against the straight line to the body) is at least this.
pub const HARD_LEAD_DEG: f32 = 3.0;
/// Spin: a melee hit on a body this far off the view `SPIN_BEFORE` ticks earlier.
pub const SPIN_OFF_DEG: f32 = 120.0;
pub const SPIN_BEFORE: usize = 6;

/// The middle of a body's hitbox above its origin: what a shot is aimed at.
const CENTRE_Z: f32 = 4.0;

pub const ERROR_BUCKETS: usize = 16;
pub const REACTION_BUCKETS: usize = 12;
/// Upper edges of the error histogram, degrees (logarithmic from 0.1 to the cone).
pub const ERROR_EDGES: [f32; ERROR_BUCKETS] = [
    0.1, 0.15, 0.2, 0.3, 0.45, 0.6, 0.9, 1.3, 1.9, 2.8, 4.0, 6.0, 9.0, 13.0, 18.0, 25.0,
];
/// Upper edges of the reaction histogram, milliseconds (the last bucket is everything above).
pub const REACTION_EDGES: [f32; REACTION_BUCKETS] = [
    -50.0, 0.0, 30.0, 60.0, 100.0, 150.0, 200.0, 280.0, 380.0, 520.0, 750.0, 1500.0,
];

/// The lower end of the 95% Wilson interval of `k` out of `n`: what a rate is at least,
/// given how few shots it was measured on.
pub fn wilson_lower(k: u32, n: u32) -> f32 {
    if n == 0 {
        return 0.0;
    }
    let (k, n) = (k as f64, n as f64);
    let z = 1.96f64;
    let p = k / n;
    let centre = p + z * z / (2.0 * n);
    let spread = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt();
    ((centre - spread) / (1.0 + z * z / n)).max(0.0) as f32
}

/// What one client did, summed over a session, a fight or a week.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct AimStats {
    /// Projectiles fired, and those with a target in the cone.
    pub shots: u32,
    pub analysed: u32,
    /// Analysed shots that damaged their target.
    pub hits: u32,
    pub flicks: u32,
    /// Analysed shots at a target whose ideal aim moved `LOCK_MOTION_DEG` or more over the
    /// window, and the locks among them.
    pub moving_shots: u32,
    pub locks: u32,
    /// Analysed shots at `LASER_RANGE` or beyond, and the lasers among them.
    pub long_shots: u32,
    pub lasers: u32,
    /// Analysed shots that needed a lead of `HARD_LEAD_DEG` or more, and the hits among them.
    pub hard_shots: u32,
    pub hard_hits: u32,
    pub melee_hits: u32,
    pub spins: u32,
    pub error_hist: [u32; ERROR_BUCKETS],
    pub reactions: u32,
    pub reaction_hist: [u32; REACTION_BUCKETS],
    pub kills: u32,
    pub deaths: u32,
    /// Damage to and from other clients' parties.
    pub damage_dealt: u64,
    pub damage_taken: u64,
    /// Kills of bodies of its own team or party.
    pub team_kills: u32,
    /// Analysed shots whose aim fitted the view the zone honoured better than the view
    /// the client claimed: now and then is jitter, most of the time is a lie.
    pub stale_views: u32,
}

fn median(hist: &[u32], edges: &[f32]) -> Option<f32> {
    let total: u32 = hist.iter().sum();
    if total == 0 {
        return None;
    }
    let mut seen = 0;
    for (count, edge) in hist.iter().zip(edges) {
        seen += count;
        if seen * 2 >= total {
            return Some(*edge);
        }
    }
    edges.last().copied()
}

/// Why an account stands out (ANTICHEAT.md 4.4), absolute rules only: the hub adds the ones
/// that compare with the population.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Rule {
    Lock,
    Flick,
    Laser,
    Reaction,
    /// Two signals four robust deviations off the population.
    Outlier,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::Lock => "lock",
            Rule::Flick => "flick",
            Rule::Laser => "laser",
            Rule::Reaction => "reaction",
            Rule::Outlier => "outlier",
        }
    }
}

/// Shots below which nothing is said: analysed ones for a flick, ones at a moving target for
/// a lock, long ones for a laser.
pub const MIN_SHOTS: u32 = 15;
pub const MIN_MOVING_SHOTS: u32 = 10;
pub const MIN_LONG_SHOTS: u32 = 10;
/// A rule is broken when the rate is *at least* this with 95% confidence (`wilson_lower`):
/// locks among shots at moving targets, flicks among analysed shots, lasers among long ones.
pub const LOCK_RATE: f32 = 0.35;
pub const FLICK_RATE: f32 = 0.10;
pub const LASER_RATE: f32 = 0.25;
pub const MIN_REACTIONS: u32 = 10;
pub const REACTION_MEDIAN_MS: f32 = 60.0;

impl AimStats {
    pub fn add(&mut self, o: &AimStats) {
        self.shots += o.shots;
        self.analysed += o.analysed;
        self.hits += o.hits;
        self.flicks += o.flicks;
        self.moving_shots += o.moving_shots;
        self.locks += o.locks;
        self.long_shots += o.long_shots;
        self.lasers += o.lasers;
        self.hard_shots += o.hard_shots;
        self.hard_hits += o.hard_hits;
        self.melee_hits += o.melee_hits;
        self.spins += o.spins;
        for (a, b) in self.error_hist.iter_mut().zip(&o.error_hist) {
            *a += b;
        }
        self.reactions += o.reactions;
        for (a, b) in self.reaction_hist.iter_mut().zip(&o.reaction_hist) {
            *a += b;
        }
        self.kills += o.kills;
        self.deaths += o.deaths;
        self.damage_dealt += o.damage_dealt;
        self.damage_taken += o.damage_taken;
        self.team_kills += o.team_kills;
        self.stale_views += o.stale_views;
    }

    fn rate(&self, n: u32) -> f32 {
        n as f32 / self.analysed.max(1) as f32
    }

    pub fn hit_rate(&self) -> f32 {
        self.rate(self.hits)
    }

    pub fn flick_rate(&self) -> f32 {
        self.rate(self.flicks)
    }

    pub fn lock_rate(&self) -> f32 {
        self.locks as f32 / self.moving_shots.max(1) as f32
    }

    pub fn laser_rate(&self) -> f32 {
        self.lasers as f32 / self.long_shots.max(1) as f32
    }

    pub fn hard_hit_rate(&self) -> f32 {
        self.hard_hits as f32 / self.hard_shots.max(1) as f32
    }

    /// Nothing was counted.
    pub fn is_empty(&self) -> bool {
        *self == AimStats::default()
    }

    /// The upper edge of the bucket the median error falls in, degrees.
    pub fn median_error(&self) -> Option<f32> {
        median(&self.error_hist, &ERROR_EDGES)
    }

    /// The upper edge of the bucket the median reaction falls in, milliseconds.
    pub fn median_reaction_ms(&self) -> Option<f32> {
        median(&self.reaction_hist, &REACTION_EDGES)
    }

    /// The absolute rules these numbers break.
    pub fn rules(&self) -> Vec<Rule> {
        let mut out = Vec::new();
        if self.moving_shots >= MIN_MOVING_SHOTS
            && wilson_lower(self.locks, self.moving_shots) >= LOCK_RATE
        {
            out.push(Rule::Lock);
        }
        if self.analysed >= MIN_SHOTS && wilson_lower(self.flicks, self.analysed) >= FLICK_RATE {
            out.push(Rule::Flick);
        }
        if self.long_shots >= MIN_LONG_SHOTS
            && wilson_lower(self.lasers, self.long_shots) >= LASER_RATE
        {
            out.push(Rule::Laser);
        }
        if self.reactions >= MIN_REACTIONS
            && self
                .median_reaction_ms()
                .is_some_and(|m| m <= REACTION_MEDIAN_MS)
        {
            out.push(Rule::Reaction);
        }
        out
    }

    /// One line for logs and gates: `key=value` pairs.
    pub fn line(&self) -> String {
        format!(
            "shots={} analysed={} hits={} flicks={} moving_shots={} locks={} long_shots={} lasers={} \
             hard_shots={} hard_hits={} melee_hits={} spins={} \
             median_error={} reactions={} median_reaction_ms={} kills={} deaths={} \
             damage_dealt={} damage_taken={} team_kills={} stale_views={} rules={}",
            self.shots,
            self.analysed,
            self.hits,
            self.flicks,
            self.moving_shots,
            self.locks,
            self.long_shots,
            self.lasers,
            self.hard_shots,
            self.hard_hits,
            self.melee_hits,
            self.spins,
            self.median_error()
                .map_or_else(|| "-".into(), |m| format!("{m}")),
            self.reactions,
            self.median_reaction_ms()
                .map_or_else(|| "-".into(), |m| format!("{m:.0}")),
            self.kills,
            self.deaths,
            self.damage_dealt,
            self.damage_taken,
            self.team_kills,
            self.stale_views,
            {
                let rules: Vec<&str> = self.rules().iter().map(|r| r.name()).collect();
                if rules.is_empty() {
                    "-".to_string()
                } else {
                    rules.join(",")
                }
            }
        )
    }
}

/// One analysed shot, for the reviewer (`gm-tools replay aim --shots`, the viewer's HUD).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub tick: u32,
    pub owner: u32,
    pub projectile: u32,
    pub target: u32,
    pub distance: f32,
    pub error: f32,
    /// The lead the shot needed, degrees.
    pub lead: f32,
    pub snap: f32,
    pub settle: u32,
    /// How steady the view was against the ideal aim over the window (standard deviation).
    pub steady: f32,
    pub target_motion: f32,
    pub flick: bool,
    pub lock: bool,
    pub laser: bool,
    pub hit: bool,
}

/// One body at one tick, as the wire carries it.
#[derive(Clone, Copy, Debug)]
struct Sample {
    tick: u32,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    alive: bool,
    /// How far behind this tick the world was that this view looked at, where the record
    /// says (`Event::View`).
    lag: Option<u8>,
}

/// Ticks of history kept per body: the window, the snap and velocity look-backs and the
/// deepest lag (PROTOCOL.md 7.4).
const HISTORY: usize = 48;

#[derive(Default)]
struct Trace {
    samples: VecDeque<Sample>,
}

impl Trace {
    fn at(&self, tick: u32) -> Option<&Sample> {
        // Newest last; the tick asked for is at most `HISTORY` back.
        self.samples.iter().rev().find(|s| s.tick == tick)
    }

    fn view(&self, tick: u32) -> Option<Vec3> {
        self.at(tick).map(|s| view_dir(s.yaw, s.pitch))
    }
}

struct Flying {
    shot: usize,
    owner: u32,
    target: u32,
    hard: bool,
    dies: u32,
}

fn angle_deg(a: Vec3, b: Vec3) -> f32 {
    a.normalize_or_zero()
        .dot(b.normalize_or_zero())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// Where to aim from `eye` to meet a body at `centre` moving with `velocity`, for a
/// projectile of `speed` under `gravity` times the world's (the formula a mind uses).
pub fn ideal_aim(eye: Vec3, centre: Vec3, velocity: Vec3, speed: f32, gravity: f32) -> Vec3 {
    let mut p = centre;
    let mut t = 0.0;
    for _ in 0..2 {
        t = (p - eye).length() / speed.max(1.0);
        p = centre + velocity * t;
    }
    p.z += 0.5 * MoveVars::QUAKE.gravity * gravity * t * t;
    p - eye
}

/// Reads frames in order and keeps the statistics of every client-driven body.
pub struct Analyser {
    hz: f32,
    /// The zone has teams; without them only parties tell friend from foe.
    teams: bool,
    roster: Vec<RosterEntry>,
    traces: BTreeMap<u32, Trace>,
    /// Each client's view lag as last recorded (`Event::View`).
    lags: BTreeMap<u32, u8>,
    stats: BTreeMap<u32, AimStats>,
    pub shots: Vec<Shot>,
    flying: Vec<Flying>,
    /// Keep every analysed shot (a reviewer's run), or only the sums (a zone's).
    keep_shots: bool,
}

impl Analyser {
    pub fn new(hz: u16, teams: bool, roster: Vec<RosterEntry>, keep_shots: bool) -> Analyser {
        Analyser {
            hz: hz.max(1) as f32,
            teams,
            roster,
            traces: BTreeMap::new(),
            lags: BTreeMap::new(),
            stats: BTreeMap::new(),
            shots: Vec::new(),
            flying: Vec::new(),
            keep_shots,
        }
    }

    fn entry(&self, id: u32) -> Option<&RosterEntry> {
        self.roster.iter().find(|r| r.id == id)
    }

    fn human(&self, id: u32) -> bool {
        self.entry(id).is_some_and(|r| r.human())
    }

    /// Whether `a` and `b` fight each other: different parties, and different teams where
    /// the zone has teams.
    fn hostile(&self, a: u32, b: u32) -> bool {
        match (self.entry(a), self.entry(b)) {
            (Some(x), Some(y)) => {
                x.id != y.id && x.party != y.party && (!self.teams || x.team != y.team)
            }
            _ => false,
        }
    }

    /// The sums of one body so far.
    pub fn stats(&self, id: u32) -> AimStats {
        self.stats.get(&id).cloned().unwrap_or_default()
    }

    /// Take a body's sums and start it again from zero (a report to the hub).
    pub fn take(&mut self, id: u32) -> AimStats {
        self.stats.remove(&id).unwrap_or_default()
    }

    /// Everybody with numbers, by entity id.
    pub fn all(&self) -> impl Iterator<Item = (u32, &AimStats)> {
        self.stats.iter().map(|(id, s)| (*id, s))
    }

    /// One tick: the entity table and the events, as recorded.
    pub fn frame(&mut self, tick: u32, entities: &[EntityState], events: &[Event]) {
        apply_roster(&mut self.roster, events);
        // The tick's view lags first: they belong to the views of this tick.
        for ev in events {
            if let Event::View { id, lag } = *ev {
                self.lags.insert(id, lag);
            }
        }
        for e in entities {
            if e.spawn.kind() != EntityKind::Player {
                continue;
            }
            let trace = self.traces.entry(e.id).or_default();
            trace.samples.push_back(Sample {
                tick,
                pos: Vec3::from(quant::dequantize_pos3(e.pos)),
                yaw: quant::wire_to_yaw(e.yaw),
                pitch: quant::wire_to_pitch(e.pitch),
                alive: e.flags & flags::ALIVE != 0,
                lag: self.lags.get(&e.id).copied(),
            });
            while trace.samples.len() > HISTORY {
                trace.samples.pop_front();
            }
        }
        // A body that left takes its history with it (its sums stay until taken).
        if events.iter().any(|e| matches!(e, Event::Left(_))) {
            let roster = &self.roster;
            self.traces
                .retain(|id, _| roster.iter().any(|r| r.id == *id));
            self.lags.retain(|id, _| roster.iter().any(|r| r.id == *id));
        }
        for ev in events {
            match *ev {
                Event::Shot {
                    projectile,
                    owner,
                    origin,
                    speed,
                    gravity,
                    lifetime,
                    lag,
                    honoured,
                } => self.shot(
                    tick,
                    &Fired {
                        projectile,
                        owner,
                        origin: Vec3::from(origin),
                        speed,
                        gravity,
                        lifetime,
                        lag,
                        honoured,
                    },
                ),
                Event::Hit {
                    attacker,
                    target,
                    amount,
                    kind,
                    ..
                } => self.hit(tick, attacker, target, amount, kind),
                Event::Killed { victim, killer } => self.killed(victim, killer),
                Event::Reaction { viewer, ms, .. } => self.reaction(viewer, ms),
                _ => {}
            }
        }
        self.flying.retain(|f| tick_before(tick, f.dies));
    }

    fn eye(&self, id: u32, tick: u32) -> Option<Vec3> {
        let s = self.traces.get(&id)?.at(tick)?;
        Some(s.pos + Vec3::Z * Hull::Player.eye_height())
    }

    /// A body's centre and velocity at a tick, if it lived then. The velocity is taken over
    /// `VELOCITY_TICKS` (or as many as are on record).
    fn body(&self, id: u32, tick: u32) -> Option<(Vec3, Vec3)> {
        let trace = self.traces.get(&id)?;
        let now = trace.at(tick)?;
        if !now.alive {
            return None;
        }
        let mut velocity = Vec3::ZERO;
        for back in (1..=VELOCITY_TICKS).rev() {
            if let Some(before) = trace.at(tick.wrapping_sub(back)).filter(|b| b.alive) {
                velocity = (now.pos - before.pos) * (self.hz / back as f32);
                break;
            }
        }
        Some((now.pos + Vec3::Z * CENTRE_Z, velocity))
    }

    /// The shot against the world its shooter's view was of: its target and the numbers of
    /// ANTICHEAT.md 4.1, or nothing if no hostile body is near the view. Each tick's view is
    /// held against the world as far back as that tick's frames said they looked (the
    /// shot's own lag where the record has no per-tick one), and no further back than
    /// `cap` if there is one: the view the zone honoured.
    fn judge(&self, tick: u32, f: &Fired, cap: Option<u32>) -> Option<Judged> {
        let owner = f.owner;
        let owner_trace = self.traces.get(&owner)?;
        let lag_at = |t: u32| {
            let lag = owner_trace
                .at(t)
                .and_then(|s| s.lag)
                .map_or(f.lag, u32::from);
            cap.map_or(lag, |cap| lag.min(cap))
        };
        let (view, eye) = (owner_trace.view(tick)?, self.eye(owner, tick)?);
        // The projectile leaves the muzzle parallel to the view: the aim that hits is the
        // direction from there, not from the eye.
        let muzzle = f.origin - eye;
        let seen = tick.wrapping_sub(lag_at(tick));
        let range = f.speed * f.lifetime as f32 / self.hz;
        let mut best: Option<(f32, u32, f32, f32)> = None;
        for other in &self.roster {
            if !self.hostile(owner, other.id) {
                continue;
            }
            let Some((centre, velocity)) = self.body(other.id, seen) else {
                continue;
            };
            let distance = (centre - f.origin).length();
            if distance > range {
                continue;
            }
            let ideal = ideal_aim(f.origin, centre, velocity, f.speed, f.gravity);
            let error = angle_deg(view, ideal);
            if error <= TARGET_CONE_DEG && best.is_none_or(|b| error < b.0) {
                best = Some((
                    error,
                    other.id,
                    distance,
                    angle_deg(ideal, centre - f.origin),
                ));
            }
        }
        let (error, target, distance, lead) = best?;
        // The largest turn of the view across `SNAP_TICKS` ticks of the window, and how
        // long before the shot the view came to rest.
        let mut snap = 0.0f32;
        let mut settle = WINDOW as u32;
        // The view against the ideal aim at the same target over the window: yaw and pitch
        // residuals, and how far the ideal aim itself travelled.
        let mut residuals: Vec<(f32, f32)> = Vec::with_capacity(WINDOW);
        let mut first_ideal: Option<Vec3> = None;
        let mut last_ideal = Vec3::ZERO;
        for back in (0..WINDOW as u32).rev() {
            let t = tick.wrapping_sub(back);
            let (Some(now), Some(before)) = (
                owner_trace.at(t),
                owner_trace.view(t.wrapping_sub(SNAP_TICKS)),
            ) else {
                continue;
            };
            let view_t = view_dir(now.yaw, now.pitch);
            snap = snap.max(angle_deg(view_t, before));
            // The turn ended at the last tick the view still moved noticeably.
            if owner_trace
                .view(t.wrapping_sub(1))
                .is_some_and(|last| angle_deg(view_t, last) >= STILL_DEG)
            {
                settle = back;
            }
            let (Some(eye_t), Some((centre, velocity))) = (
                self.eye(owner, t),
                self.body(target, t.wrapping_sub(lag_at(t))),
            ) else {
                continue;
            };
            let ideal = ideal_aim(eye_t + muzzle, centre, velocity, f.speed, f.gravity);
            let ideal_yaw = ideal.y.atan2(ideal.x).to_degrees();
            let ideal_pitch = (-ideal.z).atan2(ideal.truncate().length()).to_degrees();
            let mut dyaw = (now.yaw - ideal_yaw).rem_euclid(360.0);
            if dyaw > 180.0 {
                dyaw -= 360.0;
            }
            residuals.push((
                dyaw * ideal_pitch.to_radians().cos(),
                now.pitch - ideal_pitch,
            ));
            first_ideal.get_or_insert(ideal);
            last_ideal = ideal;
        }
        let steady = if residuals.len() == WINDOW {
            let n = WINDOW as f32;
            let mean = residuals
                .iter()
                .fold((0.0, 0.0), |m, r| (m.0 + r.0 / n, m.1 + r.1 / n));
            let var = residuals.iter().fold(0.0, |v, r| {
                v + ((r.0 - mean.0).powi(2) + (r.1 - mean.1).powi(2)) / n
            });
            var.sqrt()
        } else {
            f32::INFINITY
        };
        Some(Judged {
            target,
            distance,
            error,
            lead,
            snap,
            settle,
            steady,
            target_motion: first_ideal.map_or(0.0, |f| angle_deg(f, last_ideal)),
        })
    }

    fn shot(&mut self, tick: u32, f: &Fired) {
        let owner = f.owner;
        if !self.human(owner) {
            return;
        }
        self.stats.entry(owner).or_default().shots += 1;
        let Some(owner_trace) = self.traces.get(&owner) else {
            return;
        };
        // The whole window of the shooter's view must be on record.
        let first = tick.wrapping_sub(WINDOW as u32 + SNAP_TICKS);
        if owner_trace.at(first).is_none() {
            return;
        }
        // The shooter aimed at the world as it saw it. Its frame says how long ago that
        // was; the zone resolved its hits against a view it may have clamped. A client
        // that lies about its view to look clumsy must still aim at the world its hits
        // are resolved in: the shot is judged against both, and the better fit counts.
        let claimed = self.judge(tick, f, None);
        let honoured = (f.honoured < f.lag)
            .then(|| self.judge(tick, f, Some(f.honoured)))
            .flatten();
        let (judged, stale) = match (claimed, honoured) {
            (Some(c), Some(h)) if h.error < c.error => (h, true),
            (Some(c), _) => (c, false),
            (None, Some(h)) => (h, true),
            (None, None) => return,
        };
        let Judged {
            target,
            distance,
            error,
            lead,
            snap,
            settle,
            steady,
            target_motion,
        } = judged;
        let on_target = error
            <= (BODY_HALF_WIDTH / distance.max(1.0))
                .atan()
                .to_degrees()
                .max(ON_TARGET_MIN_DEG);
        let flick = snap >= FLICK_SNAP_DEG && settle <= FLICK_SETTLE && on_target;
        let moving = target_motion >= LOCK_MOTION_DEG;
        let lock = moving && steady <= LOCK_STEADY_DEG && on_target;
        let long = distance >= LASER_RANGE;
        let laser = long && error <= LASER_ERROR_DEG;
        let hard = lead >= HARD_LEAD_DEG;

        let stats = self.stats.entry(owner).or_default();
        stats.analysed += 1;
        stats.stale_views += stale as u32;
        stats.flicks += flick as u32;
        stats.moving_shots += moving as u32;
        stats.locks += lock as u32;
        stats.long_shots += long as u32;
        stats.lasers += laser as u32;
        stats.hard_shots += hard as u32;
        let bucket = ERROR_EDGES
            .iter()
            .position(|edge| error <= *edge)
            .unwrap_or(ERROR_BUCKETS - 1);
        stats.error_hist[bucket] += 1;
        let index = self.shots.len();
        if self.keep_shots {
            self.shots.push(Shot {
                tick,
                owner,
                projectile: f.projectile,
                target,
                distance,
                error,
                lead,
                snap,
                settle,
                steady,
                target_motion,
                flick,
                lock,
                laser,
                hit: false,
            });
        }
        self.flying.push(Flying {
            shot: index,
            owner,
            target,
            hard,
            dies: tick.wrapping_add(f.lifetime),
        });
    }

    fn hit(&mut self, tick: u32, attacker: u32, target: u32, amount: i32, kind: Hit) {
        let amount = amount.max(0) as u64;
        let between_players = attacker != target && self.hostile_parties(attacker, target);
        if between_players && self.human(attacker) {
            self.stats.entry(attacker).or_default().damage_dealt += amount;
        }
        if between_players && self.human(target) {
            self.stats.entry(target).or_default().damage_taken += amount;
        }
        if !self.human(attacker) {
            return;
        }
        match kind {
            Hit::Projectile => {
                // The oldest shot of this owner still in the air against this target. With
                // two in the air the credit can go to the wrong one of them; the sums are
                // right either way.
                if let Some(i) = self
                    .flying
                    .iter()
                    .position(|f| f.owner == attacker && f.target == target)
                {
                    let f = self.flying.remove(i);
                    let stats = self.stats.entry(attacker).or_default();
                    stats.hits += 1;
                    stats.hard_hits += f.hard as u32;
                    if self.keep_shots
                        && let Some(s) = self.shots.get_mut(f.shot)
                    {
                        s.hit = true;
                    }
                }
            }
            Hit::Melee => {
                let spin = self.spin(tick, attacker, target);
                let stats = self.stats.entry(attacker).or_default();
                stats.melee_hits += 1;
                stats.spins += spin as u32;
            }
            Hit::Area | Hit::Dot => {}
        }
    }

    /// Both bodies belong to clients' parties (a creature's is 0) and not to the same one.
    fn hostile_parties(&self, a: u32, b: u32) -> bool {
        match (self.entry(a), self.entry(b)) {
            (Some(x), Some(y)) => x.party != 0 && y.party != 0 && x.party != y.party,
            _ => false,
        }
    }

    /// Whether the body just hit in melee was behind the attacker a moment ago.
    fn spin(&self, tick: u32, attacker: u32, target: u32) -> bool {
        let before = tick.wrapping_sub(SPIN_BEFORE as u32);
        let (Some(trace), Some(eye), Some((centre, _))) = (
            self.traces.get(&attacker),
            self.eye(attacker, before),
            self.body(target, before),
        ) else {
            return false;
        };
        trace
            .view(before)
            .is_some_and(|view| angle_deg(view, centre - eye) >= SPIN_OFF_DEG)
    }

    fn killed(&mut self, victim: u32, killer: u32) {
        if self.human(victim) {
            self.stats.entry(victim).or_default().deaths += 1;
        }
        if killer == 0 || killer == victim || !self.human(killer) {
            return;
        }
        // Friend or foe: the party always; the team only where the zone has teams (in a
        // wild zone every human is on one team and kills between parties are the game).
        let same_side = match (self.entry(killer), self.entry(victim)) {
            (Some(k), Some(v)) => k.party == v.party || (self.teams && k.team == v.team),
            _ => false,
        };
        let stats = self.stats.entry(killer).or_default();
        if same_side {
            stats.team_kills += 1;
        } else {
            stats.kills += 1;
        }
    }

    fn reaction(&mut self, viewer: u32, ms: i16) {
        if !self.human(viewer) {
            return;
        }
        let stats = self.stats.entry(viewer).or_default();
        stats.reactions += 1;
        let bucket = REACTION_EDGES
            .iter()
            .position(|edge| ms as f32 <= *edge)
            .unwrap_or(REACTION_BUCKETS - 1);
        stats.reaction_hist[bucket] += 1;
    }
}

/// A projectile as it left.
struct Fired {
    projectile: u32,
    owner: u32,
    origin: Vec3,
    speed: f32,
    gravity: f32,
    lifetime: u32,
    lag: u32,
    honoured: u32,
}

/// A shot judged against the world as it was `lag` ticks before it.
struct Judged {
    target: u32,
    distance: f32,
    error: f32,
    lead: f32,
    snap: f32,
    settle: u32,
    steady: f32,
    target_motion: f32,
}

/// `a` is before `b` on the wrapping tick line.
fn tick_before(a: u32, b: u32) -> bool {
    (b.wrapping_sub(a) as i32) > 0
}

/// The sums of every client-driven body over a whole file, and the analysed shots.
pub fn analyse(replay: &crate::Replay) -> (BTreeMap<u32, AimStats>, Vec<Shot>) {
    let mut a = Analyser::new(
        replay.header.hz,
        replay.header.teams,
        replay.header.roster.clone(),
        true,
    );
    for f in &replay.frames {
        a.frame(f.tick, &f.snapshot.entities, &f.events);
    }
    (a.stats, a.shots)
}
