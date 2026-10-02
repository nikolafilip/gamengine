//! Reactions (ANTICHEAT.md 4.2): how long a client takes to turn onto a hostile body that
//! comes out from behind something. The zone measures it, because it has the map: a body
//! that was in front of a client and hidden, and then is not hidden, has *appeared*; the
//! reaction is the time until the client's view arrives on it, counted in the client's own
//! view of the world (the tick its frames say they were looking at), so that latency is
//! not mistaken for slowness nor a fast connection for a fast hand.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use glam::Vec3;
use gm_core::sim::{Driver, MAX_CLAIMED_VIEW_LAG, Zone, view_dir};
use gm_core::trace::{CollisionWorld, Hull};
use gm_core::vocab::EntityId;
use gm_replay::Event;

/// A hidden body is watched while it is within this of the view (roughly on screen), degrees.
const SCREEN_DEG: f32 = 50.0;
/// An appearance counts if the body was hidden for at least this long while on screen.
const HIDDEN_TICKS_MS: u32 = 250;
/// It counts if the view was at least this far from it then, and has arrived inside
/// `NEAR_DEG` within `LIMIT_MS`.
const FAR_DEG: f32 = 15.0;
const NEAR_DEG: f32 = 3.0;
const LIMIT_MS: u32 = 1500;
/// Bodies further than this are not watched.
const RANGE: f32 = 2500.0;
/// A visible body's line of sight is looked at again this often, in sweeps.
const RECHECK_SWEEPS: u32 = 4;
/// A viewer watches this many hostile bodies: the nearest on its screen.
const WATCHED: usize = 4;
/// Appearances waiting for a view to arrive, zone-wide.
const MAX_PENDING: usize = 1024;
/// What a shot is aimed at: the middle of the hitbox above the origin.
const CENTRE_Z: f32 = 4.0;

/// Entity ids are small numbers: a pair of them hashes by multiplication.
#[derive(Default)]
struct PairHasher(u64);

impl Hasher for PairHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("pairs hash as one u64")
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

#[derive(Clone, Copy, Default)]
struct Pair {
    visible: bool,
    /// On screen and hidden since this tick (0 = not).
    hidden_since: u32,
    checked: u32,
    seen_sweep: u32,
}

/// One body as the sweep needs it.
struct Seen {
    id: EntityId,
    eye: Vec3,
    centre: Vec3,
    view: Vec3,
    view_tick: u32,
    human: bool,
    alive: bool,
    team: u8,
    party: u32,
}

#[derive(Default)]
pub struct Sight {
    pairs: HashMap<u64, Pair, BuildHasherDefault<PairHasher>>,
    /// Counts ticks; a viewer is swept on every second one.
    sweep: u32,
    bodies: Vec<Seen>,
    /// `(viewer, body, tick)`: the body appeared then with the view far from it.
    pending: Vec<(EntityId, EntityId, u32)>,
}

fn angle_deg(a: Vec3, b: Vec3) -> f32 {
    a.normalize_or_zero()
        .dot(b.normalize_or_zero())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

impl Sight {
    /// One tick: half the viewers are swept (the even ids on even ticks), each over the
    /// nearest few hostile bodies on its screen; every pending appearance is looked at. `teams`: the zone has teams. `human(id)`: a client's body.
    pub fn tick(
        &mut self,
        zone: &Zone,
        world: &dyn CollisionWorld,
        teams: bool,
        human: &dyn Fn(EntityId) -> bool,
        out: &mut Vec<Event>,
    ) {
        let tick = zone.tick;
        let hz = zone.rate.hz();
        self.sweep = self.sweep.wrapping_add(1);
        self.bodies.clear();
        // The measured one-way latency is rounded up to whole ticks: two of them less two
        // is never more than the round trip.
        let least_delay = gm_net::client::base_delay(zone.rate);
        for p in zone.players() {
            let is_human = p.driver == Driver::Client && human(p.id);
            self.bodies.push(Seen {
                id: p.id,
                eye: p.mover.eye(),
                centre: p.mover.mv.origin + Vec3::Z * CENTRE_Z,
                view: view_dir(p.mover.yaw, p.mover.pitch),
                // The world tick its frames say they look at, and never newer than a
                // client's can be: a frame that runs now left the client a one-way trip
                // ago and was drawn from a snapshot a one-way trip older, held back by
                // the interpolation delay every client of this protocol draws with. A
                // client that claims a fresher view than that (to make its reactions
                // look slow) is not believed.
                view_tick: {
                    let floor = tick.wrapping_sub(
                        (least_delay + (2 * p.half_rtt_ticks).saturating_sub(2))
                            .min(MAX_CLAIMED_VIEW_LAG),
                    );
                    if (floor.wrapping_sub(p.view_claimed) as i32) < 0 {
                        floor
                    } else {
                        p.view_claimed
                    }
                },
                human: is_human,
                alive: p.alive && !p.ghost,
                team: p.team(),
                party: p.party,
            });
        }
        // `zone.players()` is in id order; the pending list finds its bodies by search.
        self.bodies.sort_unstable_by_key(|b| b.id);
        let hidden_ticks = HIDDEN_TICKS_MS * hz / 1000;
        let limit_ticks = LIMIT_MS * hz / 1000;
        let cos_screen = SCREEN_DEG.to_radians().cos();
        // Appearances waiting for the view to arrive are looked at every tick; there are few.
        let bodies = &self.bodies;
        let find = |id: EntityId| {
            bodies
                .binary_search_by_key(&id, |b| b.id)
                .ok()
                .map(|i| &bodies[i])
                .filter(|b| b.human && b.alive)
        };
        self.pending.retain(|&(viewer, body, appeared)| {
            let (Some(v), Some(b)) = (find(viewer), find(body)) else {
                return false;
            };
            if tick.wrapping_sub(appeared) > limit_ticks {
                return false;
            }
            if angle_deg(v.view, b.centre - v.eye) > NEAR_DEG {
                return true;
            }
            // In the client's own time: the world tick its frames were looking at when the
            // view arrived, against the tick the body appeared.
            let ticks = v.view_tick.wrapping_sub(appeared) as i32;
            let ms = (ticks as f32 * 1000.0 / hz as f32).clamp(i16::MIN as f32, i16::MAX as f32);
            out.push(Event::Reaction {
                viewer,
                body,
                ms: ms as i16,
            });
            false
        });
        for v in self.bodies.iter().filter(|b| b.human && b.alive) {
            if (v.id ^ tick) & 1 != 0 {
                continue;
            }
            // The nearest few hostile bodies on screen: what a reaction is measured on. In a
            // crowd the rest are not watched (a sample, at a cost that does not grow with
            // the square of the zone).
            let mut near: [(f32, usize); WATCHED] = [(f32::MAX, usize::MAX); WATCHED];
            for (i, b) in self.bodies.iter().enumerate() {
                if !(b.human && b.alive)
                    || b.id == v.id
                    || b.party == v.party
                    || (teams && b.team == v.team)
                {
                    continue;
                }
                let to = b.centre - v.eye;
                let d2 = to.length_squared();
                if d2 > RANGE * RANGE || d2 >= near[WATCHED - 1].0 {
                    continue;
                }
                if v.view.dot(to) < cos_screen * d2.sqrt() {
                    continue;
                }
                let at = near.partition_point(|n| n.0 <= d2);
                near.copy_within(at..WATCHED - 1, at + 1);
                near[at] = (d2, i);
            }
            for &(_, i) in near.iter().filter(|n| n.1 != usize::MAX) {
                let b = &self.bodies[i];
                let key = ((v.id as u64) << 32) | b.id as u64;
                let pair = self.pairs.entry(key).or_default();
                // A pair that was not watched on this viewer's last sweep (off screen, out
                // of the nearest few, somebody dead) starts again: nothing is known of it.
                if self.sweep.wrapping_sub(pair.seen_sweep) != 2 {
                    *pair = Pair::default();
                }
                pair.seen_sweep = self.sweep;
                // A visible body is looked at again now and then; a hidden one every sweep.
                if pair.visible && self.sweep.wrapping_sub(pair.checked) < RECHECK_SWEEPS * 2 {
                    continue;
                }
                pair.checked = self.sweep;
                let clear = world.trace(Hull::Point, v.eye, b.centre).fraction >= 1.0;
                if clear {
                    if !pair.visible
                        && pair.hidden_since != 0
                        && tick.wrapping_sub(pair.hidden_since) >= hidden_ticks
                        && angle_deg(v.view, b.centre - v.eye) >= FAR_DEG
                        && self.pending.len() < MAX_PENDING
                        && !self.pending.iter().any(|p| p.0 == v.id && p.1 == b.id)
                    {
                        self.pending.push((v.id, b.id, tick));
                    }
                    pair.visible = true;
                    pair.hidden_since = 0;
                } else {
                    if pair.visible || pair.hidden_since == 0 {
                        pair.hidden_since = tick.max(1);
                    }
                    pair.visible = false;
                }
            }
        }
        // Pairs nobody swept for a while belong to bodies that left.
        if self.sweep.is_multiple_of(256) {
            let now = self.sweep;
            self.pairs
                .retain(|_, p| now.wrapping_sub(p.seen_sweep) < 512);
        }
    }
}
