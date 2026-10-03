//! What is heard, inferred from the stream of states the zone sends (SOUND.md 3): every
//! sample of every entity is read once, in tick order, so that a transition is heard
//! exactly once whatever the frame rate, the render time's jitter or a frame drawn twice.
//! The own body's swings, casts and shots come from its own predicted actions (they would
//! be a round trip late from the zone); its staggers, hurts and death from the zone.

use std::collections::HashMap;

use glam::Vec3;
use gm_core::sim::Action;
use gm_core::sim::anim;

use super::synth::Cue;

/// A step every so many units of ground travelled.
pub const STRIDE: f32 = 64.0;
/// And no oftener than this, per body.
pub const STEP_GAP_SECS: f32 = 0.15;
/// A body that moved further than this between two samples was put somewhere (a respawn,
/// a zone change, a correction): not a step's worth of travel.
const TELEPORT: f32 = 200.0;
/// A landing is heard after a fall of this much, or this long in the air.
const FALL_UNITS: f32 = 48.0;
const AIR_SECS: f32 = 0.15;
/// A parry that ended within this time landed (the zone closes a window that met a blow
/// at once); one that ran its course was an attempt, and is silent.
const PARRY_LANDED_SECS: f32 = 0.25;
/// A body hurt is heard no oftener than this (a bleed pulses four times a second).
const HURT_GAP_SECS: f32 = 0.3;
/// A projectile whose first sample has its owner's body behind it on its own line of
/// flight, this close to the line and no further back than this, was launched here
/// (PROTOCOL.md 7.4: a projectile is stepped its shooter's lag forward at its spawn, so
/// its first sample is hundreds of units ahead of the muzzle); one that merely came into
/// sight is not.
const LAUNCH_OFF_LINE: f32 = 96.0;
const LAUNCH_BEHIND: f32 = 1500.0;
/// A projectile that vanished within this of a body hit it.
const BODY_REACH: f32 = 48.0;
/// At most this many cues begin in one frame: the nearest.
pub const CUES_PER_FRAME: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Body,
    Projectile,
    Area,
}

/// One sample of one entity, as the zone sent it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub id: u32,
    pub kind: Kind,
    pub tick: u32,
    pub pos: Vec3,
    pub anim: u8,
    pub on_ground: bool,
    /// For the bodies whose health the zone sends (the party, creatures).
    pub health: Option<u16>,
    /// A projectile's or an area's maker.
    pub owner: u32,
    pub harmful: bool,
    /// A projectile's direction of flight (unit; zero for anything else).
    pub dir: Vec3,
}

/// The own body this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnNow {
    pub id: u32,
    pub pos: Vec3,
    pub on_ground: bool,
    /// Ground travelled by its own ticks since the last frame (never by a replay).
    pub travel: f32,
    pub health: i32,
    pub alive: bool,
}

/// One sound to start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    pub cue: Cue,
    /// Where; `None` is the listener itself (unpanned, full gain).
    pub at: Option<Vec3>,
    /// Whose: the pitch varies by it.
    pub key: u32,
}

#[derive(Clone, Copy, Debug)]
struct Seen {
    /// The tick of the last sample read: an older or the same one is not read again.
    tick: u32,
    anim: u8,
    pos: Vec3,
    /// Of the sample before, for a projectile's way.
    prev_pos: Option<Vec3>,
    travel: f32,
    last_step: f32,
    steps: u32,
    /// Since when in the air, and the highest it was there.
    air_since: Option<f32>,
    peak_z: f32,
    parry_since: Option<f32>,
    health: Option<u16>,
    last_hurt: f32,
    kind: Kind,
}

impl Seen {
    fn new(s: &Sample, at: f32) -> Seen {
        Seen {
            tick: s.tick,
            anim: s.anim,
            pos: s.pos,
            prev_pos: None,
            travel: 0.0,
            last_step: at - STEP_GAP_SECS,
            steps: 0,
            air_since: (!s.on_ground).then_some(at),
            peak_z: s.pos.z,
            parry_since: (s.anim == anim::PARRY).then_some(at),
            health: s.health,
            last_hurt: at - HURT_GAP_SECS,
            kind: s.kind,
        }
    }
}

/// The own body between frames.
#[derive(Clone, Copy, Debug)]
struct OwnSeen {
    anim: u8,
    air_since: Option<f32>,
    peak_z: f32,
    travel: f32,
    last_step: f32,
    steps: u32,
    health: i32,
    last_hurt: f32,
    /// Since when the zone has had it parrying, by the zone's clock (the tick of the
    /// word), so that words that arrived together are timed as the zone timed them.
    parry_since: Option<f32>,
}

/// What the scene remembers between frames.
pub struct Scene {
    dt: f32,
    seen: HashMap<u32, Seen>,
    own: Option<OwnSeen>,
    /// The own body's id (it is not among the samples: the zone's word on it is read
    /// through `own`), for what it makes.
    own_id: Option<u32>,
    heard: Vec<Heard>,
}

impl Scene {
    /// `dt`: seconds a tick (the samples' clock).
    pub fn new(dt: f32) -> Scene {
        Scene {
            dt: dt.max(1e-3),
            seen: HashMap::new(),
            own: None,
            own_id: None,
            heard: Vec::new(),
        }
    }

    /// Forget everything (a zone left): nothing is read across it.
    pub fn clear(&mut self) {
        self.seen.clear();
        self.own = None;
        self.own_id = None;
        self.heard.clear();
    }

    fn cue(&mut self, cue: Cue, at: Option<Vec3>, key: u32) {
        self.heard.push(Heard { cue, at, key });
    }

    /// One sample of an entity; one not newer than its last is not read again. The first
    /// sample of an entity reads no transition: nothing is inferred about what was not
    /// seen. Whether this was the first.
    pub fn sample(&mut self, s: Sample) -> bool {
        let at = s.tick as f32 * self.dt;
        let Some(mut seen) = self.seen.get(&s.id).copied() else {
            self.seen.insert(s.id, Seen::new(&s, at));
            match s.kind {
                // Made here, by somebody in sight: heard where it began (the muzzle: the
                // own body is the listener).
                Kind::Projectile => {
                    if let Some(from) = self.launched_by(&s) {
                        self.cue(Cue::Launch, from, s.id);
                    }
                }
                Kind::Area if s.harmful && self.owner_in_sight(s.owner) => {
                    self.cue(Cue::Burst, Some(s.pos), s.id);
                }
                _ => {}
            }
            return true;
        };
        if s.tick <= seen.tick {
            return false;
        }
        if s.kind == Kind::Body {
            self.body_sample(&mut seen, &s, at);
        }
        seen.tick = s.tick;
        seen.prev_pos = Some(seen.pos);
        seen.pos = s.pos;
        seen.anim = s.anim;
        seen.health = s.health;
        self.seen.insert(s.id, seen);
        false
    }

    /// The ids known, for whoever checks them against what still exists.
    pub fn known(&self) -> impl Iterator<Item = u32> + '_ {
        self.seen.keys().copied()
    }

    /// Whether `owner` is the own body (which is not among the samples, and is always
    /// here) or a body in the samples.
    fn owner_in_sight(&self, owner: u32) -> bool {
        self.own_id == Some(owner) || self.seen.get(&owner).is_some_and(|o| o.kind == Kind::Body)
    }

    /// Where a projectile's first sample was launched from, if here: the listener for the
    /// own body's; its owner's body, when that body is behind it on its line of flight
    /// (`LAUNCH_OFF_LINE`, `LAUNCH_BEHIND`); nothing for one that came into sight.
    fn launched_by(&self, s: &Sample) -> Option<Option<Vec3>> {
        if self.own_id == Some(s.owner) {
            return Some(None);
        }
        let o = self.seen.get(&s.owner).filter(|o| o.kind == Kind::Body)?;
        let to_owner = o.pos - s.pos;
        let back = -s.dir;
        let along = to_owner.dot(back);
        let off = (to_owner - back * along).length();
        (s.dir.length_squared() > 0.5
            && (-LAUNCH_OFF_LINE..=LAUNCH_BEHIND).contains(&along)
            && off <= LAUNCH_OFF_LINE)
            .then_some(Some(o.pos))
    }

    fn body_sample(&mut self, seen: &mut Seen, s: &Sample, at: f32) {
        let here = Some(s.pos);
        let died = s.anim == anim::DEAD && seen.anim != anim::DEAD;
        if s.anim != seen.anim {
            match (seen.anim, s.anim) {
                // A swing is heard as the blow begins, from whatever came before it (the
                // windup, or anything when the windup's samples were lost), or as the
                // windup ends in its recovery when the blow's own state was too short to
                // be sampled.
                (_, anim::SWING) | (anim::WINDUP, anim::RECOVER) => {
                    self.cue(Cue::Swing, here, s.id)
                }
                (_, anim::STAGGER) => self.cue(Cue::Stagger, here, s.id),
                (_, anim::CAST) => self.cue(Cue::Cast, here, s.id),
                (_, anim::DASH) => self.cue(Cue::Dash, here, s.id),
                (_, anim::DEAD) => self.cue(Cue::Death, here, s.id),
                _ => {}
            }
            // A parry that ended at once met a blow.
            if seen.anim == anim::PARRY
                && let Some(since) = seen.parry_since.take()
                && at - since < PARRY_LANDED_SECS
            {
                self.cue(Cue::Parry, here, s.id);
            }
            if s.anim == anim::PARRY {
                seen.parry_since = Some(at);
            }
        }
        // Hurt: the health the zone sends fell, not by dying, not too often.
        if let (Some(before), Some(now)) = (seen.health, s.health)
            && now < before
            && !died
            && s.anim != anim::DEAD
            && at - seen.last_hurt >= HURT_GAP_SECS
        {
            seen.last_hurt = at;
            self.cue(Cue::Hit, here, s.id);
        }
        // In the air, and down again: a fall of some height, or some time, lands.
        if !s.on_ground {
            if seen.air_since.is_none() {
                seen.air_since = Some(at);
                seen.peak_z = seen.pos.z;
            }
            seen.peak_z = seen.peak_z.max(s.pos.z);
        } else if let Some(since) = seen.air_since.take()
            && s.anim != anim::DEAD
            && (seen.peak_z - s.pos.z >= FALL_UNITS || at - since >= AIR_SECS)
        {
            self.cue(Cue::Land, here, s.id);
        }
        // Steps: ground travelled in any state that walks.
        let moved = (s.pos - seen.pos).truncate().length();
        let walks = s.on_ground && !matches!(s.anim, anim::DEAD | anim::AIR | anim::DASH);
        if moved > TELEPORT {
            seen.travel = 0.0;
        } else if walks {
            seen.travel += moved;
            if seen.travel >= STRIDE && at - seen.last_step >= STEP_GAP_SECS {
                seen.travel -= STRIDE;
                seen.last_step = at;
                seen.steps += 1;
                let step = if seen.steps % 2 == 1 {
                    Cue::StepA
                } else {
                    Cue::StepB
                };
                self.cue(step, here, s.id);
            }
        } else {
            seen.travel = 0.0;
        }
    }

    /// An entity is gone. A projectile that was going somewhere lands where it was: on the
    /// world, if its way on from its last sample hits it (`hits_world`), or on a body it
    /// vanished beside; one that merely went out of sight is silent.
    pub fn removed(&mut self, id: u32, hits_world: &dyn Fn(Vec3, Vec3) -> bool) {
        let Some(seen) = self.seen.remove(&id) else {
            return;
        };
        if seen.kind != Kind::Projectile {
            return;
        }
        let way = seen.prev_pos.map_or(Vec3::ZERO, |p| seen.pos - p);
        let on_world = way.length_squared() > 0.0 && hits_world(seen.pos, seen.pos + way * 2.0);
        let on_body = self
            .seen
            .values()
            .any(|o| o.kind == Kind::Body && o.pos.distance(seen.pos) <= BODY_REACH);
        if on_world || on_body {
            self.cue(Cue::Impact, Some(seen.pos), id);
        }
    }

    /// The own body, once a frame: `anims` the zone's words on it since the last frame,
    /// in order, each with the tick it was said at; `actions` its own predicted actions
    /// since the last frame.
    pub fn own(&mut self, now: f32, o: OwnNow, anims: &[(u32, u8)], actions: &[Action]) {
        self.own_id = Some(o.id);
        let Some(mut own) = self.own else {
            let (tick, anim) = anims.last().copied().unwrap_or((0, anim::IDLE));
            self.own = Some(OwnSeen {
                anim,
                air_since: (!o.on_ground).then_some(now),
                peak_z: o.pos.z,
                travel: 0.0,
                last_step: now - STEP_GAP_SECS,
                steps: 0,
                health: o.health,
                last_hurt: now - HURT_GAP_SECS,
                parry_since: (anim == anim::PARRY).then_some(tick as f32 * self.dt),
            });
            return;
        };
        // What it did, as it predicted it: heard at once.
        let mut swung = false;
        for a in actions {
            match a {
                Action::Swing { .. } if !swung => {
                    swung = true;
                    self.cue(Cue::Swing, None, o.id);
                }
                Action::Swing { .. } | Action::ParryOpened => {}
                Action::Fire { .. } | Action::Area { .. } => self.cue(Cue::Cast, None, o.id),
            }
        }
        // What the zone said of it: a stagger, a death, a parry that met a blow (the
        // window closed at once; one that ran its course was an attempt, and is silent).
        let died = anims.iter().any(|&(_, a)| a == anim::DEAD) && own.anim != anim::DEAD;
        for &(tick, a) in anims {
            if a != own.anim {
                let at = tick as f32 * self.dt;
                match a {
                    anim::STAGGER => self.cue(Cue::Stagger, None, o.id),
                    anim::DEAD => self.cue(Cue::Death, None, o.id),
                    _ => {}
                }
                if own.anim == anim::PARRY
                    && let Some(since) = own.parry_since.take()
                    && at - since < PARRY_LANDED_SECS
                {
                    self.cue(Cue::Parry, None, o.id);
                }
                if a == anim::PARRY {
                    own.parry_since = Some(at);
                }
                own.anim = a;
            }
        }
        if o.health < own.health && !died && o.alive && now - own.last_hurt >= HURT_GAP_SECS {
            own.last_hurt = now;
            self.cue(Cue::Hurt, None, o.id);
        }
        own.health = o.health;
        // Landing: by the fall, or the time in the air.
        if !o.on_ground {
            if own.air_since.is_none() {
                own.air_since = Some(now);
                own.peak_z = o.pos.z;
            }
            own.peak_z = own.peak_z.max(o.pos.z);
        } else if let Some(since) = own.air_since.take()
            && o.alive
            && (own.peak_z - o.pos.z >= FALL_UNITS || now - since >= AIR_SECS)
        {
            self.cue(Cue::Land, None, o.id);
        }
        // Steps, from its own ticks' travel.
        if o.on_ground && o.alive && o.travel < TELEPORT {
            own.travel += o.travel;
            if own.travel >= STRIDE && now - own.last_step >= STEP_GAP_SECS {
                own.travel -= STRIDE;
                own.last_step = now;
                own.steps += 1;
                let step = if own.steps % 2 == 1 {
                    Cue::StepA
                } else {
                    Cue::StepB
                };
                self.cue(step, None, o.id);
            }
        } else if !o.on_ground {
            own.travel = 0.0;
        }
        self.own = Some(own);
    }

    /// The frame's cues, no more than a frame's worth: the nearest, by selection (a
    /// sort would cost the browser build kilobytes for eight picks).
    pub fn take(&mut self, listener: Vec3) -> Vec<Heard> {
        let mut heard = std::mem::take(&mut self.heard);
        let d = |h: &Heard| h.at.map_or(0.0, |p| p.distance_squared(listener));
        let mut n = 0;
        while n < CUES_PER_FRAME && n < heard.len() {
            let mut nearest = n;
            for i in n + 1..heard.len() {
                if d(&heard[i]) < d(&heard[nearest]) {
                    nearest = i;
                }
            }
            heard.swap(n, nearest);
            n += 1;
        }
        heard.truncate(n);
        heard
    }
}

/// A pitch of its own for a cue: within ±8% of 1, from whose it is and how many so far.
pub fn pitch_of(key: u32, count: u32) -> f32 {
    let mut h = key.wrapping_mul(0x9e37_79b9) ^ count.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    1.0 + ((h & 0xffff) as f32 / 65_535.0 - 0.5) * 0.16
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 64.0;

    fn body(id: u32, tick: u32, x: f32, anim: u8, on_ground: bool) -> Sample {
        Sample {
            id,
            kind: Kind::Body,
            tick,
            pos: Vec3::new(x, 0.0, 24.0),
            anim,
            on_ground,
            health: None,
            owner: 0,
            harmful: false,
            dir: Vec3::ZERO,
        }
    }

    fn cues(heard: &[Heard]) -> Vec<Cue> {
        heard.iter().map(|h| h.cue).collect()
    }

    fn no_world(_: Vec3, _: Vec3) -> bool {
        false
    }

    #[test]
    fn a_transition_sounds_once_from_the_samples_whatever_the_frames() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        s.sample(body(5, 1, 100.0, anim::IDLE, true));
        for t in 2..10 {
            s.sample(body(5, t, 100.0, anim::IDLE, true));
        }
        assert!(s.take(o).is_empty(), "a state is nothing");
        // Windup, blow, recovery: one swing, as the windup ends.
        s.sample(body(5, 10, 100.0, anim::WINDUP, true));
        s.sample(body(5, 11, 100.0, anim::WINDUP, true));
        assert!(s.take(o).is_empty());
        s.sample(body(5, 12, 100.0, anim::SWING, true));
        s.sample(body(5, 13, 100.0, anim::RECOVER, true));
        let h = s.take(o);
        assert_eq!(cues(&h), [Cue::Swing]);
        assert_eq!(h[0].at, Some(Vec3::new(100.0, 0.0, 24.0)));
        // A blow too short to be sampled: windup straight to recovery is a swing too.
        s.sample(body(5, 14, 100.0, anim::WINDUP, true));
        s.sample(body(5, 15, 100.0, anim::RECOVER, true));
        assert_eq!(cues(&s.take(o)), [Cue::Swing]);
        // A windup cut short by a stagger is no swing.
        s.sample(body(5, 16, 100.0, anim::WINDUP, true));
        s.sample(body(5, 17, 100.0, anim::STAGGER, true));
        assert_eq!(cues(&s.take(o)), [Cue::Stagger]);
        for (i, (a, c)) in [
            (anim::CAST, Cue::Cast),
            (anim::DASH, Cue::Dash),
            (anim::DEAD, Cue::Death),
        ]
        .into_iter()
        .enumerate()
        {
            let t = 18 + 2 * i as u32;
            s.sample(body(5, t, 100.0, anim::IDLE, true));
            s.sample(body(5, t + 1, 100.0, a, true));
            assert_eq!(cues(&s.take(o)), [c]);
        }
        // A sample not newer than the last is not read again (a frame that fed it twice).
        s.sample(body(5, 23, 100.0, anim::IDLE, true));
        assert!(s.take(o).is_empty());
        // The windup's samples lost (or none: a weapon without one): the blow is a swing
        // from whatever came before it; its recovery alone from idle is not.
        s.sample(body(5, 24, 100.0, anim::RUN, true));
        s.sample(body(5, 25, 100.0, anim::SWING, true));
        s.sample(body(5, 26, 100.0, anim::RECOVER, true));
        s.sample(body(5, 27, 100.0, anim::IDLE, true));
        s.sample(body(5, 28, 100.0, anim::RECOVER, true));
        assert_eq!(cues(&s.take(o)), [Cue::Swing]);
        // Taking no frame between two samples loses nothing: both read in order.
        s.sample(body(5, 30, 100.0, anim::IDLE, true));
        s.sample(body(5, 31, 100.0, anim::WINDUP, true));
        s.sample(body(5, 32, 100.0, anim::SWING, true));
        s.sample(body(5, 33, 100.0, anim::WINDUP, true));
        s.sample(body(5, 34, 100.0, anim::SWING, true));
        assert_eq!(cues(&s.take(o)), [Cue::Swing, Cue::Swing]);
    }

    #[test]
    fn a_parry_that_met_a_blow_clangs_and_an_attempt_is_silent() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        s.sample(body(2, 1, 0.0, anim::IDLE, true));
        // Opened and closed at once by the zone: it met a blow.
        s.sample(body(2, 2, 0.0, anim::PARRY, true));
        s.sample(body(2, 5, 0.0, anim::IDLE, true));
        assert_eq!(cues(&s.take(o)), [Cue::Parry]);
        // Opened and left to run its window and recovery: nothing.
        s.sample(body(2, 10, 0.0, anim::PARRY, true));
        s.sample(body(2, 50, 0.0, anim::IDLE, true));
        assert!(s.take(o).is_empty());
    }

    #[test]
    fn steps_come_every_stride_in_any_walking_state_and_not_too_fast() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        // 300 units a second, sampled every tick, taken every sixteen (a frame's worth):
        // 4.69 units a tick.
        let mut steps = Vec::new();
        for t in 0..=128 {
            let a = if t % 2 == 0 { anim::RUN } else { anim::GUARD };
            s.sample(body(1, t, t as f32 * DT * 300.0, a, true));
            if t % 16 == 0 {
                steps.extend(cues(&s.take(o)));
            }
        }
        steps.extend(cues(&s.take(o)));
        assert_eq!(steps.len(), 9, "{steps:?}");
        for (i, c) in steps.iter().enumerate() {
            assert_eq!(*c, if i % 2 == 0 { Cue::StepA } else { Cue::StepB });
        }
        // Standing: nothing. In the air: nothing. Dashing: the dash, and no step.
        for t in 129..140 {
            s.sample(body(1, t, 600.0, anim::IDLE, true));
        }
        s.sample(body(1, 140, 700.0, anim::AIR, false));
        s.sample(body(1, 141, 800.0, anim::DASH, true));
        assert_eq!(cues(&s.take(o)), [Cue::Dash]);
        // Fast travel is still at most a step every 150 ms.
        let mut s = Scene::new(DT);
        for t in 0..=64 {
            s.sample(body(1, t, t as f32 * DT * 2000.0, anim::RUN, true));
        }
        let n = s.take(o).len();
        assert!(n <= 7, "{n} steps in a second");
        // Put somewhere else between two samples: no step for the jump.
        let mut s = Scene::new(DT);
        s.sample(body(1, 0, 0.0, anim::RUN, true));
        s.sample(body(1, 1, 500.0, anim::RUN, true));
        assert!(s.take(o).is_empty());
    }

    #[test]
    fn a_fall_lands_and_a_step_off_a_kerb_does_not() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        s.sample(body(1, 1, 0.0, anim::IDLE, true));
        // A tick off the ground on a stair's edge, no height lost: nothing.
        s.sample(body(1, 2, 0.0, anim::AIR, false));
        s.sample(body(1, 3, 0.0, anim::IDLE, true));
        assert!(s.take(o).is_empty());
        // A jump: 60 units up, back down over 40 ticks.
        s.sample(body(1, 4, 0.0, anim::AIR, false));
        let mut up = body(1, 20, 0.0, anim::AIR, false);
        up.pos.z += 60.0;
        s.sample(up);
        s.sample(body(1, 44, 0.0, anim::IDLE, true));
        assert_eq!(cues(&s.take(o)), [Cue::Land]);
        // A long drop with no height sampled on the way: time in the air lands it.
        s.sample(body(1, 50, 0.0, anim::AIR, false));
        s.sample(body(1, 70, 0.0, anim::IDLE, true));
        assert_eq!(cues(&s.take(o)), [Cue::Land]);
    }

    #[test]
    fn a_hit_is_a_health_the_zone_sends_falling_and_not_a_bleed_s_every_pulse() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        let mut c = body(9, 1, 300.0, anim::IDLE, true);
        c.health = Some(500);
        s.sample(c);
        c.tick = 2;
        c.health = Some(460);
        s.sample(c);
        assert_eq!(cues(&s.take(o)), [Cue::Hit]);
        // Four pulses in a second: one more hit, not four.
        for t in 0..4 {
            c.tick = 3 + t * 16;
            c.health = Some(450 - t as u16 * 10);
            s.sample(c);
        }
        assert_eq!(cues(&s.take(o)), [Cue::Hit]);
        // Healed: nothing. Dying: the death, not a hit.
        c.tick = 80;
        c.health = Some(500);
        s.sample(c);
        c.tick = 81;
        c.health = Some(0);
        c.anim = anim::DEAD;
        s.sample(c);
        assert_eq!(cues(&s.take(o)), [Cue::Death]);
    }

    #[test]
    fn projectiles_and_areas_are_heard_from_their_makers_and_where_they_land() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        s.sample(body(1, 1, 0.0, anim::CAST, true));
        // A bolt flying along +x from the body at the origin (its first sample already
        // ahead by the shooter's lag).
        let bolt = |tick: u32, x: f32| Sample {
            id: 90,
            kind: Kind::Projectile,
            tick,
            pos: Vec3::new(x, 0.0, 30.0),
            anim: 0,
            on_ground: false,
            health: None,
            owner: 1,
            harmful: true,
            dir: Vec3::X,
        };
        s.sample(bolt(2, 300.0));
        let h = s.take(o);
        assert_eq!(cues(&h), [Cue::Launch]);
        assert_eq!(
            h[0].at,
            Some(Vec3::new(0.0, 0.0, 24.0)),
            "heard at the muzzle"
        );
        s.sample(bolt(3, 340.0));
        s.sample(bolt(4, 380.0));
        // Gone, with the world in its way: it hit.
        let wall = |from: Vec3, to: Vec3| from.x < 390.0 && to.x >= 390.0;
        s.removed(90, &wall);
        let h = s.take(o);
        assert_eq!(cues(&h), [Cue::Impact]);
        assert_eq!(h[0].at, Some(Vec3::new(380.0, 0.0, 30.0)));
        // Gone with nothing in its way and nobody near: out of sight, silent.
        s.sample(bolt(10, 300.0));
        s.sample(bolt(11, 340.0));
        s.take(o);
        s.removed(90, &no_world);
        assert!(s.take(o).is_empty());
        // Gone beside a body: it hit the body.
        s.sample(body(7, 12, 500.0, anim::IDLE, true));
        s.sample(bolt(12, 470.0));
        s.take(o);
        s.removed(90, &no_world);
        assert_eq!(cues(&s.take(o)), [Cue::Impact]);
        // A bolt whose maker is not in sight (it came into sight): silent. One whose maker
        // is in sight but not behind it on its line (it is somebody else's, or it came
        // round a corner): silent too.
        s.sample(Sample {
            owner: 99,
            ..bolt(20, 900.0)
        });
        s.sample(Sample {
            id: 91,
            pos: Vec3::new(300.0, 400.0, 30.0),
            ..bolt(20, 300.0)
        });
        s.sample(Sample {
            id: 92,
            dir: Vec3::Y,
            ..bolt(20, 300.0)
        });
        assert!(s.take(o).is_empty());
        // The own body's bolt: the own body is not among the samples, and is always here.
        s.own(
            0.5,
            OwnNow {
                id: 42,
                pos: Vec3::new(1000.0, 0.0, 24.0),
                on_ground: true,
                travel: 0.0,
                health: 100,
                alive: true,
            },
            &[(32, anim::CAST)],
            &[],
        );
        s.sample(Sample {
            id: 95,
            owner: 42,
            ..bolt(21, 1900.0)
        });
        assert_eq!(cues(&s.take(o)), [Cue::Launch]);
        // A harmful area by somebody in sight bursts; a helpful one, or a stranger's, is
        // silent.
        let area = |id: u32, owner: u32, harmful: bool| Sample {
            id,
            kind: Kind::Area,
            tick: 30,
            pos: Vec3::new(50.0, 50.0, 0.0),
            anim: 0,
            on_ground: true,
            health: None,
            owner,
            harmful,
            dir: Vec3::ZERO,
        };
        s.sample(area(191, 1, true));
        s.sample(area(192, 1, false));
        s.sample(area(193, 99, true));
        assert_eq!(cues(&s.take(o)), [Cue::Burst]);
    }

    #[test]
    fn the_own_body_is_heard_from_its_own_actions_and_the_zone_s_word() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        let me = |pos_z: f32, on_ground: bool, travel: f32, health: i32| OwnNow {
            id: 1,
            pos: Vec3::new(0.0, 0.0, pos_z),
            on_ground,
            travel,
            health,
            alive: true,
        };
        s.own(0.0, me(24.0, true, 0.0, 100), &[(0, anim::IDLE)], &[]);
        assert!(s.take(o).is_empty(), "the first frame reads nothing");
        // A predicted swing and a shot are heard at once; the zone's stagger as it comes.
        let swing = Action::Swing {
            ability: 1,
            step: 0,
            active_from: 10,
            active_until: 12,
        };
        s.own(
            0.1,
            me(24.0, true, 0.0, 100),
            &[(6, anim::WINDUP), (7, anim::SWING)],
            &[swing],
        );
        assert_eq!(
            cues(&s.take(o)),
            [Cue::Swing],
            "once, not again from the zone's word"
        );
        s.own(
            0.2,
            me(24.0, true, 0.0, 100),
            &[(13, anim::RECOVER)],
            &[Action::Fire {
                ability: 2,
                step: 0,
            }],
        );
        assert_eq!(cues(&s.take(o)), [Cue::Cast]);
        s.own(0.3, me(24.0, true, 0.0, 70), &[(19, anim::STAGGER)], &[]);
        let h = s.take(o);
        assert_eq!(cues(&h), [Cue::Stagger, Cue::Hurt]);
        assert!(h.iter().all(|x| x.at.is_none()));
        // Steps from its own travel, a stride at a time.
        for i in 0..10 {
            s.own(
                0.5 + i as f32 * 0.1,
                me(24.0, true, 30.0, 70),
                &[(32 + i * 6, anim::RUN)],
                &[],
            );
        }
        let steps = cues(&s.take(o));
        assert_eq!(steps.len(), 4, "300 units: four strides, {steps:?}");
        // A jump lands; a kerb does not.
        s.own(2.0, me(24.0, false, 0.0, 70), &[(128, anim::AIR)], &[]);
        s.own(2.3, me(84.0, false, 0.0, 70), &[(147, anim::AIR)], &[]);
        s.own(2.6, me(24.0, true, 0.0, 70), &[(166, anim::IDLE)], &[]);
        assert_eq!(cues(&s.take(o)), [Cue::Land]);
        s.own(3.0, me(24.0, false, 0.0, 70), &[(192, anim::AIR)], &[]);
        s.own(3.05, me(24.0, true, 0.0, 70), &[(195, anim::IDLE)], &[]);
        assert!(s.take(o).is_empty());
        // A parry the zone closed at once met a blow (its words in one frame, or the
        // next); one it let run its window was an attempt, and is silent.
        s.own(
            3.5,
            me(24.0, true, 0.0, 70),
            &[(224, anim::PARRY), (225, anim::IDLE)],
            &[Action::ParryOpened],
        );
        assert_eq!(cues(&s.take(o)), [Cue::Parry]);
        s.own(
            3.6,
            me(24.0, true, 0.0, 70),
            &[(230, anim::PARRY)],
            &[Action::ParryOpened],
        );
        s.own(3.7, me(24.0, true, 0.0, 70), &[(237, anim::RECOVER)], &[]);
        assert_eq!(cues(&s.take(o)), [Cue::Parry]);
        s.own(
            3.8,
            me(24.0, true, 0.0, 70),
            &[(243, anim::PARRY)],
            &[Action::ParryOpened],
        );
        s.own(3.9, me(24.0, true, 0.0, 70), &[], &[]);
        s.own(4.2, me(24.0, true, 0.0, 70), &[(269, anim::IDLE)], &[]);
        assert!(s.take(o).is_empty(), "an attempt is silent");
        // The words of a whole window arriving in one frame (a stall of the network):
        // timed by the zone's ticks, not the frame, so an attempt is still silent.
        s.own(
            4.3,
            me(24.0, true, 0.0, 70),
            &[(275, anim::PARRY), (280, anim::PARRY), (295, anim::IDLE)],
            &[Action::ParryOpened],
        );
        assert!(s.take(o).is_empty(), "twenty ticks of parrying met nothing");
        // Death is a death, not a hurt.
        let mut dead = me(24.0, true, 0.0, 0);
        dead.alive = false;
        s.own(4.5, dead, &[(288, anim::DEAD)], &[]);
        assert_eq!(cues(&s.take(o)), [Cue::Death]);
    }

    #[test]
    fn a_crowd_is_cut_to_the_nearest_and_a_clear_forgets() {
        let mut s = Scene::new(DT);
        let o = Vec3::ZERO;
        for i in 1..=20 {
            s.sample(body(i, 1, i as f32 * 50.0, anim::IDLE, true));
        }
        for i in 1..=20 {
            s.sample(body(i, 2, i as f32 * 50.0, anim::STAGGER, true));
        }
        let h = s.take(o);
        assert_eq!(h.len(), CUES_PER_FRAME);
        assert!(h.iter().all(|x| x.cue == Cue::Stagger && x.key <= 8));
        s.clear();
        for i in 1..=20 {
            s.sample(body(i, 3, i as f32 * 50.0, anim::IDLE, true));
        }
        assert!(s.take(o).is_empty(), "nothing read across a clear");
    }

    #[test]
    fn a_pitch_varies_a_little_and_is_the_same_for_the_same_key() {
        for key in [1u32, 7, 500, 70_000] {
            for count in 0..20 {
                let p = pitch_of(key, count);
                assert!((0.92..=1.08).contains(&p), "{p}");
                assert_eq!(p, pitch_of(key, count));
            }
        }
        let spread: Vec<f32> = (0..20).map(|c| pitch_of(3, c)).collect();
        assert!(spread.iter().any(|p| *p > 1.03) && spread.iter().any(|p| *p < 0.97));
    }
}
