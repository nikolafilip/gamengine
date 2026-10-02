//! A player's stand-in (the headless client and the offline acceptance): it walks to the
//! creatures a map posts, commands its squad from the stance, keeps itself out of what it
//! cannot fight, and moves on when a post is cleared. It plays the leader: its squad does
//! the work, under its orders.

use glam::Vec3;
use gm_core::build::Sheet;
use gm_core::sim::{Input, buttons, tick_delta};
use gm_core::tick::Tick;
use gm_core::vocab::EntityId;

use crate::companion::Order;
use crate::fighter::{Engage, Fighter};
use crate::kit::{KitPlan, Role};
use crate::nav::NavGrid;
use crate::sense::{Body, Senses};

/// How long the stance is held for one round of orders, when in it the first order is sent,
/// and how long after it the second.
const STANCE_MS: u32 = 800;
const ORDER_AT_MS: u32 = 350;
const SECOND_ORDER_MS: u32 = 200;
/// A post counts as cleared when nothing lives near it for this long.
const CLEAR_MS: u32 = 1500;
/// The raider judges a post from this near, with the post in view.
const POST_RADIUS: f32 = 640.0;
/// The squad is ordered again this often while it has members without an order.
const REORDER_MS: u32 = 4000;
/// A second creature seen after the orders went out is answered no sooner than this.
const RESPLIT_MS: u32 = 1000;

/// What the raider asks of its zone besides its input frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    Order { slots: u8, order: Order },
}

/// A companion as its commander knows it (the zone's `Squad` message).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mate {
    pub id: EntityId,
    /// Its squad slot: the bit an order names it by.
    pub slot: u8,
    pub role: Role,
    /// The body its `Attack` order names, if it has one.
    pub attacking: Option<EntityId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Advance,
    Command {
        since: Tick,
        /// The creature the squad kills first.
        target: EntityId,
        /// Another one for the tanks to keep busy meanwhile.
        other: Option<EntityId>,
        /// Orders sent in this stance.
        sent: u8,
    },
    Watch,
    Done,
}

/// The creature posts of a map as a raider's objectives: one per encounter (the middle of
/// its posts), in the order a walk from `from` reaches them.
pub fn objectives(nav: &NavGrid, from: Vec3, posts: &[(String, Vec3)]) -> Vec<Vec3> {
    let mut groups: Vec<(&str, Vec3, f32)> = Vec::new();
    for (encounter, at) in posts {
        match groups.iter_mut().find(|g| g.0 == encounter) {
            Some(g) => {
                g.1 += *at;
                g.2 += 1.0;
            }
            None => groups.push((encounter, *at, 1.0)),
        }
    }
    let mut out: Vec<(f32, Vec3)> = groups
        .into_iter()
        .map(|(_, sum, n)| {
            let at = sum / n;
            let walk = nav.path_between(from, at).map_or(f32::MAX, |path| {
                path.windows(2).map(|w| (w[1] - w[0]).length()).sum()
            });
            (walk, at)
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|(_, at)| at).collect()
}

pub struct Raider {
    fighter: Fighter,
    /// Creature posts to clear, in order.
    objectives: Vec<Vec3>,
    at: usize,
    phase: Phase,
    /// Where it came from: the way back out of trouble.
    home: Option<Vec3>,
    target: Option<EntityId>,
    clear_since: Option<Tick>,
    last_order: Tick,
    /// Fight with its own kit after ordering, instead of staying back.
    pub fights: bool,
    /// Against two creatures, send the tanks to the second while the rest kill the first
    /// (otherwise everybody on the first).
    pub split: bool,
    /// Orders asked for so far.
    pub orders: u32,
}

impl Raider {
    pub fn new(seed: u64, sheet: &Sheet, yaw: f32, objectives: Vec<Vec3>) -> Raider {
        Raider {
            fighter: Fighter::new(seed, KitPlan::of(&sheet.kit), yaw),
            objectives,
            at: 0,
            phase: Phase::Advance,
            home: None,
            target: None,
            clear_since: None,
            last_order: 0,
            fights: false,
            split: true,
            orders: 0,
        }
    }

    /// Every objective is cleared.
    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }

    pub fn objective(&self) -> usize {
        self.at
    }

    /// What it is doing, for logs.
    pub fn describe(&self) -> String {
        format!(
            "objective {} of {} {:?} target {:?}",
            self.at,
            self.objectives.len(),
            self.phase,
            self.target
        )
    }

    /// One frame. `squad`: the companions it commands, as the zone's `Squad` message lists
    /// them.
    pub fn think(&mut self, s: &Senses<'_>, squad: &[Mate]) -> (Input, Option<Request>) {
        let me = s.pos();
        let home = *self.home.get_or_insert(me);
        let Some(&objective) = self.objectives.get(self.at) else {
            self.phase = Phase::Done;
            return (self.fighter.idle(s, None), None);
        };
        // What lives at this post and can be seen, by the raider or through its squad.
        let near_post = |b: &Body| (b.pos - objective).length() < 1400.0;
        let living: Vec<&Body> = s
            .bodies
            .iter()
            .filter(|b| b.alive && b.creature.is_some() && near_post(b))
            .collect();
        let squad_bodies: Vec<&Body> = s
            .bodies
            .iter()
            .filter(|b| b.alive && squad.iter().any(|m| m.id == b.id))
            .collect();
        // A living companion without an `Attack` order, or with one that names a body
        // known to be dead: new, risen again, or its target fell. (A target the leader is
        // not shown is not known to be dead: the zone ends an order with its target.)
        let unordered = squad_bodies.iter().any(|b| {
            squad.iter().any(|m| {
                m.id == b.id
                    && m.attacking
                        .is_none_or(|t| s.body(t).is_some_and(|t| !t.alive))
            })
        });

        let tanks: u8 = squad
            .iter()
            .filter(|m| self.split && m.role == Role::Tank)
            .fold(0, |mask, m| mask | 1 << m.slot);
        let seen_by_squad = |b: &Body| {
            squad_bodies.iter().any(|m| {
                (m.pos - b.pos).length() < 1400.0
                    && s.world
                        .trace(
                            gm_core::trace::Hull::Point,
                            m.pos + Vec3::Z * 22.0,
                            b.centre(),
                        )
                        .fraction
                        >= 1.0
            })
        };
        let mut seen: Vec<&Body> = living
            .iter()
            .copied()
            .filter(|b| s.sees(b) || seen_by_squad(b))
            .collect();
        seen.sort_by(|a, b| s.dist(a).total_cmp(&s.dist(b)));
        // What the squad's damage dealers are on already is what they stay on.
        let focus = squad
            .iter()
            .filter(|m| m.role != Role::Tank)
            .filter_map(|m| m.attacking)
            .find_map(|id| seen.iter().find(|b| b.id == id).copied());
        // A creature that can see the raider within its sight: time to be elsewhere.
        let exposed_to = |b: &Body| {
            let sight = b
                .creature
                .and_then(|c| s.pack.creatures.get(c as usize))
                .map_or(800.0, |d| d.sight);
            s.dist(b) < sight + 80.0 && s.sees(b)
        };
        let hostile = |b: &Body| b.creature.is_some();
        let party = s.party;
        let my_id = s.id;
        let ally = move |b: &Body| b.party == party && b.id != my_id;

        // The post is cleared when the raider stands before it, looks at it, and nothing has
        // lived there for a while. (From afar an empty view proves nothing: a client is not
        // shown what is round the corner.)
        let before_it = (me - objective).truncate().length() < POST_RADIUS
            && s.world
                .trace(
                    gm_core::trace::Hull::Point,
                    s.eye(),
                    objective + Vec3::Z * 24.0,
                )
                .fraction
                >= 1.0;
        if living.is_empty() && before_it {
            let since = *self.clear_since.get_or_insert(s.tick);
            if tick_delta(s.tick, since) >= s.ticks(CLEAR_MS) as i32 {
                self.at += 1;
                self.clear_since = None;
                self.target = None;
                self.phase = Phase::Advance;
                self.home = Some(me);
            }
        } else {
            self.clear_since = None;
        }

        match self.phase {
            Phase::Done => (self.fighter.idle(s, None), None),
            Phase::Advance => {
                if let Some(first) = focus.or(seen.first().copied()) {
                    self.phase = Phase::Command {
                        since: s.tick,
                        target: first.id,
                        other: seen.iter().find(|b| b.id != first.id).map(|b| b.id),
                        sent: 0,
                    };
                    return (self.stance(s), None);
                }
                (self.fighter.go(s, objective, 300.0).0, None)
            }
            Phase::Command {
                since,
                target,
                other,
                sent,
            } => {
                // Dead is a body seen dead; one not shown at all may well be alive.
                let dead = s.body(target).is_some_and(|b| !b.alive);
                let held = tick_delta(s.tick, since);
                if dead || held >= s.ticks(STANCE_MS) as i32 {
                    self.phase = if dead { Phase::Advance } else { Phase::Watch };
                    self.target = (!dead).then_some(target);
                    return (self.fighter.idle(s, None), None);
                }
                // Two creatures and a tank: the tank keeps the second one busy while the
                // rest kill the first; otherwise everybody on the one.
                let split = other.filter(|_| tanks != 0 && tanks != 0b1_1111);
                let due = [ORDER_AT_MS, ORDER_AT_MS + SECOND_ORDER_MS];
                let mut request = None;
                if !squad.is_empty()
                    && (sent as usize) < 1 + split.is_some() as usize
                    && held >= s.ticks(due[sent as usize]) as i32
                {
                    request = Some(match (sent, split) {
                        (0, Some(_)) => Request::Order {
                            slots: 0b1_1111 & !tanks,
                            order: Order::Attack(target),
                        },
                        (0, None) => Request::Order {
                            slots: 0b1_1111,
                            order: Order::Attack(target),
                        },
                        (_, second) => Request::Order {
                            slots: tanks,
                            order: Order::Attack(second.unwrap_or(target)),
                        },
                    });
                    self.phase = Phase::Command {
                        since,
                        target,
                        other,
                        sent: sent + 1,
                    };
                    self.orders += 1;
                    self.last_order = s.tick;
                }
                (self.stance(s), request)
            }
            Phase::Watch => {
                let Some(id) = self.target else {
                    self.phase = Phase::Advance;
                    return (self.fighter.idle(s, None), None);
                };
                // The target is gone when it is seen dead, or, while it is out of the
                // leader's view, when no companion's order names it any more.
                let known = s.body(id);
                let gone = match known {
                    Some(b) => !b.alive,
                    None => !squad.iter().any(|m| m.attacking == Some(id)),
                };
                if gone {
                    // The next one, if any is in sight.
                    self.target = None;
                    self.phase = Phase::Advance;
                    return (self.fighter.idle(s, None), None);
                }
                let since_order = tick_delta(s.tick, self.last_order);
                let Some(t) = known else {
                    // Fought where the leader is not shown it: the squad's eyes are a stance
                    // away when somebody needs an order.
                    if unordered
                        && !squad_bodies.is_empty()
                        && since_order >= s.ticks(REORDER_MS) as i32
                    {
                        self.phase = Phase::Command {
                            since: s.tick,
                            target: id,
                            other: None,
                            sent: 0,
                        };
                        return (self.stance(s), None);
                    }
                    return (self.fighter.idle(s, None), None);
                };
                // Companions without an order (new, risen again, or their target fell): order
                // them again, not more often than every few seconds.
                // A second creature came into view after the orders went out: the tanks are
                // sent to it.
                let split_due = tanks != 0
                    && tanks != 0b1_1111
                    && seen.iter().any(|b| b.id != t.id)
                    && squad.iter().any(|m| {
                        m.role == Role::Tank
                            && m.attacking == Some(t.id)
                            && squad_bodies.iter().any(|b| b.id == m.id)
                    })
                    && since_order >= s.ticks(RESPLIT_MS) as i32;
                if (split_due || (unordered && since_order >= s.ticks(REORDER_MS) as i32))
                    && !squad_bodies.is_empty()
                {
                    self.phase = Phase::Command {
                        since: s.tick,
                        target: t.id,
                        other: seen.iter().find(|b| b.id != t.id).map(|b| b.id),
                        sent: 0,
                    };
                    return (self.stance(s), None);
                }
                if self.fights && s.health_frac() > 0.45 {
                    let input = self.fighter.fight(
                        s,
                        &Engage {
                            target: t,
                            tether: None,
                            flank: true,
                            range: 0.0,
                            artillery: None,
                            ally: &ally,
                            hostile: &hostile,
                        },
                    );
                    return (input, None);
                }
                // The leader's place is out of every creature's sight.
                let exposed = seen.iter().any(|b| exposed_to(b));
                if exposed {
                    (self.fighter.go(s, home, 48.0).0, None)
                } else {
                    (self.fighter.idle(s, Some(t.centre())), None)
                }
            }
        }
    }

    /// A frame in the command stance: the button held, nothing else.
    fn stance(&mut self, s: &Senses<'_>) -> Input {
        let mut input = self.fighter.idle(s, None);
        input.buttons = buttons::COMMAND;
        input.forward = 0.0;
        input.side = 0.0;
        input.ability = 0;
        input
    }
}
