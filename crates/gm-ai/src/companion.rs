//! A companion's mind (COMPANIONS.md 4, 5.3): a role read from its build, a standing order
//! from its commander, and the shared fighter underneath.

use glam::Vec3;
use gm_core::build::Sheet;
use gm_core::movement::yaw_vectors;
use gm_core::sim::{Input, tick_delta};
use gm_core::tick::Tick;
use gm_core::vocab::EntityId;

use crate::fighter::{Engage, Fighter};
use crate::kit::{KitPlan, Role};
use crate::sense::{Body, Senses};

/// How far a companion perceives.
pub const SIGHT: f32 = 1400.0;
/// A `Hold` keeps the companion this close to its post.
pub const HOLD_RADIUS: f32 = 96.0;
/// An `Attack` target unseen for this long is given up.
pub const LOST_MS: u32 = 5000;
/// Targets are chosen again this often, in ticks.
const PICK_EVERY: Tick = 8;
/// The distance a shooting kit keeps.
const RANGE: f32 = 380.0;
/// How long the others wait for the squad's heavy to get onto a target before they go in
/// anyway.
const WAIT_FOR_HEAVY_MS: u32 = 8000;
/// How far a body that is not a creature is taken to see.
const STRANGER_SIGHT: f32 = 800.0;

/// A standing order (COMPANIONS.md 5.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Order {
    Follow,
    Hold,
    MoveTo(Vec3),
    Attack(EntityId),
}

/// What the zone tells a companion about its squad each frame.
pub struct Squad<'a> {
    /// The commander's body while it is alive and in the zone.
    pub commander: Option<&'a Body>,
    /// Bodies the party is fighting: they hurt it lately, or belong to an encounter it is
    /// engaged in. A companion does not start fights; it finishes them.
    pub foes: &'a [EntityId],
}

pub struct Companion {
    pub role: Role,
    order: Order,
    /// What an `Attack` order returns to.
    standing: Order,
    /// Where `Hold`, and `MoveTo` once arrived, keep the companion.
    post: Option<Vec3>,
    /// Its place in the formation.
    slot: u8,
    fighter: Fighter,
    target: Option<EntityId>,
    target_seen: Tick,
    next_pick: Tick,
    /// Ticks spent waiting for the squad's heavy to engage first.
    waited: Tick,
}

impl Companion {
    pub fn new(seed: u64, sheet: &Sheet, yaw: f32, slot: u8) -> Companion {
        Companion {
            role: Role::of(sheet),
            order: Order::Follow,
            standing: Order::Follow,
            post: None,
            slot,
            fighter: Fighter::new(seed, KitPlan::of(&sheet.kit), yaw),
            target: None,
            target_seen: 0,
            next_pick: 0,
            waited: 0,
        }
    }

    pub fn order(&self) -> Order {
        self.order
    }

    /// Take an order; `here` is where the companion stands (a `Hold` holds there).
    pub fn set_order(&mut self, order: Order, here: Vec3, now: Tick) {
        match order {
            Order::Follow => {
                self.standing = order;
                self.post = None;
            }
            Order::Hold => {
                self.standing = order;
                self.post = Some(here);
            }
            Order::MoveTo(_) => {
                self.standing = order;
                self.post = None;
            }
            Order::Attack(id) => {
                self.target = Some(id);
                self.target_seen = now;
            }
        }
        self.order = order;
        self.fighter.nav.clear();
    }

    /// The creature (or anybody) this companion is under an `Attack` order for.
    pub fn attack_order(&self) -> Option<EntityId> {
        match self.order {
            Order::Attack(id) => Some(id),
            _ => None,
        }
    }

    pub fn target(&self) -> Option<EntityId> {
        self.target
    }

    /// Where this companion stands when it follows: by role, spread by slot.
    fn formation(&self, commander: &Body) -> Vec3 {
        const SPREAD: [f32; 5] = [-70.0, 70.0, 0.0, -140.0, 140.0];
        let spread = SPREAD[self.slot as usize % SPREAD.len()];
        let (ahead, right) = match self.role {
            Role::Tank => (90.0, spread * 0.6),
            Role::Dps => (-50.0, spread),
            Role::Scout => (30.0, spread * 2.2),
            Role::Heal => (-140.0, spread * 0.5),
        };
        let (f, r) = yaw_vectors(commander.yaw);
        commander.pos + f * ahead + r * right
    }

    /// One frame.
    pub fn think(&mut self, s: &Senses<'_>, squad: &Squad<'_>) -> Input {
        let me = s.pos();
        let party = s.party;
        let my_id = s.id;
        let ordered = self.attack_order();
        let team = s.team;
        let ally = move |b: &Body| b.party == party && b.id != my_id;
        let foes = squad.foes;
        let hostile = move |b: &Body| {
            b.party != party
                && (foes.contains(&b.id)
                    || ordered == Some(b.id)
                    || (team != 0 && b.team != 0 && b.team != team && b.creature.is_none()))
        };

        // An Attack order ends with its target, or when the target has been lost too long.
        if let Order::Attack(id) = self.order {
            let alive = s.body(id).is_some_and(|b| b.alive);
            let lost = tick_delta(s.tick, self.target_seen) > s.ticks(LOST_MS) as i32;
            if !alive || lost {
                self.order = self.standing;
                self.target = None;
                self.next_pick = s.tick;
            }
        }

        // A MoveTo in progress: walk, do not fight.
        if let Order::MoveTo(point) = self.order
            && self.post.is_none()
        {
            let (input, arrived) = self.fighter.go(s, point, 24.0);
            if arrived || s.nav.nearest(point).is_none() {
                self.post = Some(me);
            }
            return input;
        }

        // Choose what to fight (and for a healer, whom to mend), a few times a second.
        if tick_delta(s.tick, self.next_pick) >= 0 {
            self.next_pick = s.tick.wrapping_add(PICK_EVERY);
            if let Some(id) = ordered {
                self.target = Some(id);
            } else {
                let near_post = |b: &Body| match (self.order, self.post) {
                    (Order::Hold | Order::MoveTo(_), Some(post)) => {
                        (b.pos - post).truncate().length() <= HOLD_RADIUS + 520.0
                    }
                    _ => true,
                };
                // Nearest to the commander for a tank (it peels), nearest to itself otherwise.
                let anchor = match (self.role, squad.commander) {
                    (Role::Tank, Some(c)) => c.pos,
                    _ => me,
                };
                let mut candidates: Vec<(f32, &Body)> = s
                    .bodies
                    .iter()
                    .filter(|b| b.alive && hostile(b) && s.dist(b) <= SIGHT && near_post(b))
                    .map(|b| ((b.pos - anchor).length() + s.dist(b) * 0.25, b))
                    .collect();
                candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
                // Keep the current target while it is still a candidate and seen: no dithering.
                let keep = self
                    .target
                    .and_then(|id| candidates.iter().find(|(_, b)| b.id == id))
                    .filter(|(_, b)| s.sees(b))
                    .map(|(_, b)| b.id);
                self.target = keep.or_else(|| {
                    candidates
                        .iter()
                        .take(3)
                        .find(|(_, b)| s.sees(b))
                        .map(|(_, b)| b.id)
                        // Unseen but known: go and look.
                        .or(candidates.first().map(|(_, b)| b.id))
                });
            }
        }
        // Taunted (MATRIX.md 8): the taunter is the one to fight while it lasts.
        if let Some(by) = s.taunted_by() {
            self.target = Some(by);
        }
        let target = self.target.and_then(|id| s.body(id)).filter(|b| b.alive);
        if let Some(t) = target
            && self.fighter.sees(s, t)
        {
            self.target_seen = s.tick;
        }

        // The healer's first duty, while anyone of the party is hurt and in reach; its own
        // wounds it can only answer with a circle at its feet (a dart cannot hit its shooter).
        if self.role == Role::Heal {
            let patient = s
                .bodies
                .iter()
                .filter(|b| b.alive && b.party == party && b.id != my_id)
                .filter(|b| s.dist(b) < SIGHT)
                .filter_map(|b| Some((b.health_for(party)?, b)))
                .filter(|(h, _)| *h < 800)
                .min_by_key(|(h, _)| *h);
            let own = (s.health_frac() * 1000.0) as u16;
            if own < 500
                && patient.is_none_or(|(h, _)| own < h)
                && let Some(input) = self.fighter.circle_self(s)
            {
                return input;
            }
            if let Some((_, patient)) = patient {
                return self.fighter.mend(s, patient, &hostile);
            }
        }

        // The squad's heavy goes in first: while a heavy ally is on its way to a target that
        // nobody of the party is fighting at arm's length yet, the others stay where the
        // target cannot see them (how far a creature sees is in the content), and open
        // only once the heavy is on it.
        if self.role != Role::Tank
            && let Some(t) = target
        {
            let engaged = s
                .bodies
                .iter()
                .any(|b| b.alive && b.party == party && (b.pos - t.pos).length() < 150.0);
            let heavy_coming = s.bodies.iter().any(|b| {
                b.alive
                    && b.party == party
                    && b.id != my_id
                    && b.heavy()
                    && (b.pos - t.pos).length() < 1600.0
            });
            if engaged || !heavy_coming {
                self.waited = 0;
            } else if self.waited < s.ticks(WAIT_FOR_HEAVY_MS) {
                let sight = t
                    .creature
                    .and_then(|c| s.pack.creatures.get(c as usize))
                    .map_or(STRANGER_SIGHT, |d| d.sight);
                let keep = sight + 60.0;
                let d = s.dist(t);
                // Keeping out of its sight on purpose is not losing it: the order stands.
                if d > keep + 80.0 {
                    self.target_seen = s.tick;
                    return self.fighter.go(s, t.pos, keep + 40.0).0;
                }
                // At the edge of its sight, or nearer but with a wall between: wait there,
                // not for ever (a heavy that never arrives is no reason to stand around).
                // Seen in the open, waiting is over.
                if d >= keep || !self.fighter.sees(s, t) {
                    self.target_seen = s.tick;
                    self.waited = self.waited.saturating_add(1);
                    return self.fighter.idle(s, Some(t.centre()));
                }
            }
        }

        // A healer whose kit cannot hurt at range does not wade in with a staff: with nobody
        // to mend it keeps its distance, near its place in the formation.
        if self.role == Role::Heal
            && !self.fighter.plan.shoots()
            && let Some(t) = target
        {
            // Behind the fight, not behind the commander: where its darts reach the squad
            // and the target's swings do not reach it.
            let place = match self.post {
                Some(post) => post,
                None => t.pos + (me - t.pos).truncate().extend(0.0).normalize_or_zero() * 380.0,
            };
            return self.fighter.stay_back(s, &hostile, place, 300.0);
        }

        match (target, self.order, self.post) {
            (Some(t), order, post) => {
                let tether = match (order, post) {
                    (Order::Hold | Order::MoveTo(_), Some(p)) => Some((p, HOLD_RADIUS)),
                    _ => None,
                };
                let shoots = self.fighter.plan.shoots();
                let range = match self.role {
                    Role::Tank => 0.0,
                    Role::Heal => RANGE,
                    _ if shoots && self.fighter.plan.reach() < 70.0 => RANGE,
                    Role::Scout if shoots => RANGE * 0.85,
                    _ => 0.0,
                };
                self.fighter.fight(
                    s,
                    &Engage {
                        target: t,
                        tether,
                        flank: self.role != Role::Tank,
                        range,
                        artillery: None,
                        ally: &ally,
                        hostile: &hostile,
                    },
                )
            }
            (None, Order::Hold | Order::MoveTo(_), Some(post)) => {
                if (me - post).truncate().length() > 40.0 {
                    self.fighter.go(s, post, 24.0).0
                } else {
                    self.fighter.idle(s, None)
                }
            }
            (None, _, _) => match squad.commander {
                Some(c) => {
                    let place = self.formation(c);
                    // A place inside a wall or off the floor is no place: stand by the
                    // commander instead.
                    let place = if s.nav.nearest(place).is_some() {
                        place
                    } else {
                        c.pos
                    };
                    let far = (place - me).truncate().length();
                    if far > 70.0 {
                        self.fighter.go(s, place, 36.0).0
                    } else {
                        self.fighter.idle(s, Some(s.eye() + c.facing() * 200.0))
                    }
                }
                None => self.fighter.idle(s, None),
            },
        }
    }
}
