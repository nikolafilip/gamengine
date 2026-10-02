//! The zone's side of minds (COMPANIONS.md 3, 8–11): which bodies are driven by what, the
//! encounters and their ledgers, loot and trial verdicts. Pure: it works on a
//! `gm_core::sim::Zone` and says what happened in [`DirectorEvent`]s; the zone process tells
//! clients and the hub. The offline acceptance drives it with no network at all.

use std::collections::BTreeMap;

use glam::Vec3;
use gm_core::build::{Build, Sheet};
use gm_core::encounter::{Ledger, Who};
use gm_core::loot;
use gm_core::sim::{Driver, HitKind, TEAM_WILD, Zone, ZoneEvent, buttons, tick_delta};
use gm_core::tick::Tick;
use gm_core::trace::{CollisionWorld, Hull};
use gm_core::trial::{Failure, Standing, judge};
use gm_core::vocab::EntityId;

use crate::companion::{Companion, Order, Squad};
use crate::creature::Creature;
use crate::kit::Role;
use crate::nav::NavGrid;
use crate::view::ZoneView;

/// A party with no living participant in the leash for this long is out (COMPANIONS.md 9).
pub const OUT_MS: u32 = 5000;
/// No creature dealt or took damage for this long: the encounter resets.
pub const QUIET_MS: u32 = 15_000;
/// An encounter lasts this long at most.
pub const MAX_ENCOUNTER_MS: u32 = 15 * 60 * 1000;
/// A companion that died outside an encounter is back this long after.
pub const COMPANION_RESPAWN_MS: u32 = 10_000;
/// Released by an encounter, it is back this long after.
pub const RELEASE_RESPAWN_MS: u32 = 3000;
/// A party keeps fighting whoever hurt it for this long.
pub const GRIEVANCE_MS: u32 = 10_000;
/// Orders a commander may give per second (COMPANIONS.md 5.3).
pub const ORDERS_PER_S: u8 = 8;
/// A `MoveTo` point may be this far from its commander, and this far from a standable cell.
pub const MOVE_TO_RANGE: f32 = 4096.0;
pub const MOVE_TO_SNAP: f32 = 64.0;

/// A creature post from the map (`gm_creature`).
#[derive(Clone, Debug, PartialEq)]
pub struct CreatureSpawn {
    pub creature: String,
    pub encounter: String,
    pub origin: Vec3,
    pub yaw: f32,
}

/// A companion to add to a squad.
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionSpec {
    pub name: String,
    pub build: Build,
    /// Lent by the zone (free, lives only here) rather than hired.
    pub recruit: bool,
    /// The hub's hire id.
    pub hire: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Human,
    Companion { owner: EntityId },
    Creature { def: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncounterState {
    Engaged,
    Reset,
    Cleared { secs: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LootGrant {
    pub human: EntityId,
    pub items: Vec<String>,
    pub coin: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DirectorEvent {
    /// A body the director drives appeared (rosters follow).
    Spawned {
        id: EntityId,
        name: String,
        kind: BodyKind,
    },
    Removed {
        id: EntityId,
    },
    /// A commander's squad changed: its members or their orders.
    Squad {
        commander: EntityId,
    },
    Encounter {
        name: String,
        state: EncounterState,
        /// The humans to tell.
        tell: Vec<EntityId>,
    },
    /// A boss died: what each recipient gets. `kill` is unique within the zone process.
    Loot {
        encounter: String,
        kill: u64,
        grants: Vec<LootGrant>,
    },
    Trial {
        human: EntityId,
        key: String,
        name: String,
        verdict: Result<(), Failure>,
        /// What the ledger said of the candidate.
        standing: Standing,
        secs: u32,
    },
}

/// One companion of a squad.
pub struct Member {
    pub id: EntityId,
    pub name: String,
    pub mind: Companion,
    pub recruit: bool,
    pub hire: Option<i64>,
    /// Server tick it may stand up again at; `None` while it lives.
    back_at: Option<Tick>,
    /// The encounter that holds it while it is dead.
    held_by: Option<usize>,
}

impl Member {
    pub fn role(&self) -> Role {
        self.mind.role
    }
}

struct CreatureSlot {
    id: EntityId,
    def: u16,
    name: String,
    encounter: usize,
    home: Vec3,
    yaw: f32,
    mind: Creature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Engaged,
    Cleared,
}

struct Encounter {
    name: String,
    creatures: Vec<usize>,
    /// The boss's post, or the middle of the posts; with the largest leash of its creatures.
    anchor: Vec3,
    leash: f32,
    boss: Option<usize>,
    phase: Phase,
    ledger: Ledger,
    last_damage: Tick,
    /// Parties with nobody alive in the leash, and since when.
    out_since: Vec<(u32, Tick)>,
    /// Humans whose respawn this encounter holds.
    held: Vec<EntityId>,
    /// When its creatures stand on their posts again after a clear.
    respawn_at: Option<Tick>,
}

pub struct Director {
    pub nav: NavGrid,
    map: String,
    squads: BTreeMap<EntityId, Vec<Member>>,
    /// Companion → commander.
    owners: BTreeMap<EntityId, EntityId>,
    creatures: Vec<CreatureSlot>,
    creature_ids: BTreeMap<EntityId, usize>,
    encounters: Vec<Encounter>,
    /// `(party, foe, until)`: whom a party is fighting besides its encounters.
    foes: Vec<(u32, EntityId, Tick)>,
    /// Orders given in the running second, per commander.
    order_budget: BTreeMap<EntityId, (Tick, u8)>,
    view: ZoneView,
    kills: u64,
    seed: u64,
    pub events: Vec<DirectorEvent>,
}

impl Director {
    /// Build the nav grid from `seeds` and the creature posts, and put the creatures on
    /// their posts. `map` is the map's name (trials name the map they belong to).
    pub fn new(
        zone: &mut Zone,
        world: &dyn CollisionWorld,
        map: &str,
        seeds: &[Vec3],
        spawns: &[CreatureSpawn],
        seed: u64,
    ) -> Director {
        let mut all: Vec<Vec3> = seeds.to_vec();
        all.extend(spawns.iter().map(|s| s.origin));
        let mut d = Director {
            nav: NavGrid::build(world, &all),
            map: map.to_string(),
            squads: BTreeMap::new(),
            owners: BTreeMap::new(),
            creatures: Vec::new(),
            creature_ids: BTreeMap::new(),
            encounters: Vec::new(),
            foes: Vec::new(),
            order_budget: BTreeMap::new(),
            view: ZoneView::default(),
            kills: 0,
            seed,
            events: Vec::new(),
        };
        for sp in spawns {
            let Some((def_index, def)) = zone.content.creature(&sp.creature) else {
                continue;
            };
            let def = def.clone();
            let sheet = Sheet::creature(&def, &zone.content, TEAM_WILD);
            // Stand it on the floor under its map entity.
            let down = world.trace(Hull::Player, sp.origin, sp.origin - Vec3::Z * 256.0);
            let home = if down.start_solid {
                sp.origin
            } else {
                down.end
            };
            let id = zone.add_body(sheet.clone(), home, sp.yaw, Driver::Mind);
            zone.set_party(id, 0);
            zone.set_hold(id, true);
            let encounter = match d.encounters.iter().position(|e| e.name == sp.encounter) {
                Some(i) => i,
                None => {
                    d.encounters.push(Encounter {
                        name: sp.encounter.clone(),
                        creatures: Vec::new(),
                        anchor: home,
                        leash: def.leash,
                        boss: None,
                        phase: Phase::Idle,
                        ledger: Ledger::new(0),
                        last_damage: 0,
                        out_since: Vec::new(),
                        held: Vec::new(),
                        respawn_at: None,
                    });
                    d.encounters.len() - 1
                }
            };
            let slot = d.creatures.len();
            d.creatures.push(CreatureSlot {
                id,
                def: def_index,
                name: def.name.clone(),
                encounter,
                home,
                yaw: sp.yaw,
                mind: Creature::new(
                    seed ^ (id as u64) << 20,
                    &sheet,
                    home,
                    sp.yaw,
                    def.sight,
                    def.leash,
                ),
            });
            d.creature_ids.insert(id, slot);
            let e = &mut d.encounters[encounter];
            e.creatures.push(slot);
            e.leash = e.leash.max(def.leash);
            if def.boss && e.boss.is_none() {
                e.boss = Some(slot);
            }
            d.events.push(DirectorEvent::Spawned {
                id,
                name: def.name.clone(),
                kind: BodyKind::Creature { def: def_index },
            });
        }
        // An encounter is anchored at its boss, or at the middle of its posts.
        for e in &mut d.encounters {
            e.anchor = match e.boss {
                Some(b) => d.creatures[b].home,
                None => {
                    e.creatures
                        .iter()
                        .map(|&c| d.creatures[c].home)
                        .sum::<Vec3>()
                        / e.creatures.len().max(1) as f32
                }
            };
        }
        d
    }

    pub fn kind_of(&self, id: EntityId) -> BodyKind {
        if let Some(&slot) = self.creature_ids.get(&id) {
            BodyKind::Creature {
                def: self.creatures[slot].def,
            }
        } else if let Some(&owner) = self.owners.get(&id) {
            BodyKind::Companion { owner }
        } else {
            BodyKind::Human
        }
    }

    /// Every body the director drives: `(id, name, kind)` (a joiner's roster).
    pub fn driven(&self) -> Vec<(EntityId, String, BodyKind)> {
        let mut out: Vec<(EntityId, String, BodyKind)> = self
            .creatures
            .iter()
            .map(|c| (c.id, c.name.clone(), BodyKind::Creature { def: c.def }))
            .collect();
        for (&owner, members) in &self.squads {
            out.extend(
                members
                    .iter()
                    .map(|m| (m.id, m.name.clone(), BodyKind::Companion { owner })),
            );
        }
        out
    }

    pub fn squad(&self, commander: EntityId) -> &[Member] {
        self.squads.get(&commander).map_or(&[], |m| m.as_slice())
    }

    pub fn commander_of(&self, companion: EntityId) -> Option<EntityId> {
        self.owners.get(&companion).copied()
    }

    /// Squad slots of a commander (COMPANIONS.md 3.2).
    pub fn capacity(&self, zone: &Zone, commander: EntityId) -> usize {
        zone.player(commander)
            .map_or(0, |p| p.sheet.build.squad_capacity(&zone.content))
    }

    /// Whether a companion is on the ledger of an engaged encounter (a hire does not end in
    /// the middle of a fight, COMPANIONS.md 3.3).
    pub fn in_encounter(&self, id: EntityId) -> bool {
        self.encounters
            .iter()
            .any(|e| e.phase == Phase::Engaged && e.ledger.is_participant(id))
    }

    /// Put a companion beside its commander. `None` when the commander is not here, the
    /// squad is full or the build is not a valid one.
    pub fn add_companion(
        &mut self,
        zone: &mut Zone,
        world: &dyn CollisionWorld,
        commander: EntityId,
        spec: CompanionSpec,
    ) -> Option<EntityId> {
        let cap = self.capacity(zone, commander);
        let slot = self.squad(commander).len();
        if slot >= cap || spec.build.validate(&zone.content).is_err() {
            return None;
        }
        let (at, yaw, team, party) = {
            let c = zone.player(commander)?;
            (c.mover.mv.origin, c.mover.yaw, c.team(), c.party)
        };
        let sheet = Sheet::new(spec.build, &zone.content, team);
        let spot = zone.spot_near(world, at, Hull::Player);
        let id = zone.add_body(sheet.clone(), spot, yaw, Driver::Mind);
        zone.set_party(id, party);
        zone.set_hold(id, true);
        self.owners.insert(id, commander);
        self.squads.entry(commander).or_default().push(Member {
            id,
            name: spec.name.clone(),
            mind: Companion::new(self.seed ^ (id as u64) << 24, &sheet, yaw, slot as u8),
            recruit: spec.recruit,
            hire: spec.hire,
            back_at: None,
            held_by: None,
        });
        self.events.push(DirectorEvent::Spawned {
            id,
            name: spec.name,
            kind: BodyKind::Companion { owner: commander },
        });
        self.events.push(DirectorEvent::Squad { commander });
        Some(id)
    }

    fn drop_member(&mut self, zone: &mut Zone, id: EntityId) {
        self.owners.remove(&id);
        zone.remove_player(id);
        for c in &mut self.creatures {
            c.mind.forget(id);
        }
        self.events.push(DirectorEvent::Removed { id });
    }

    /// One companion leaves (its hire ended).
    pub fn remove_companion(&mut self, zone: &mut Zone, id: EntityId) {
        let Some(commander) = self.owners.get(&id).copied() else {
            return;
        };
        if let Some(members) = self.squads.get_mut(&commander) {
            members.retain(|m| m.id != id);
        }
        self.drop_member(zone, id);
        self.events.push(DirectorEvent::Squad { commander });
    }

    /// A human's party changes (PARTY.md 2): the number its body carries, and its
    /// companions' with it. Whoever calls sees to it that the body is in no engaged
    /// encounter ([`Director::engaged`]): a ledger never sees a body change sides.
    pub fn set_party(&mut self, zone: &mut Zone, human: EntityId, party: u32) {
        zone.set_party(human, party);
        for m in self.squads.get(&human).into_iter().flatten() {
            zone.set_party(m.id, party);
        }
    }

    /// Whether a human, or any of its squad, is on the ledger of an engaged encounter.
    pub fn engaged(&self, human: EntityId) -> bool {
        self.in_encounter(human) || self.squad(human).iter().any(|m| self.in_encounter(m.id))
    }

    /// Whether a party is on the ledger of an engaged encounter: the roster of its fight
    /// is closed, and nobody takes its number until the fight is over (PARTY.md 2).
    pub fn party_engaged(&self, party: u32) -> bool {
        self.encounters.iter().any(|e| {
            e.phase == Phase::Engaged
                && e.ledger.participants().any(|(_, line)| line.party == party)
        })
    }

    /// A human left the zone: its squad goes with it and nothing waits for it any more.
    pub fn human_left(&mut self, zone: &mut Zone, human: EntityId) {
        if let Some(members) = self.squads.remove(&human) {
            for m in members {
                self.drop_member(zone, m.id);
            }
        }
        for e in &mut self.encounters {
            e.held.retain(|h| *h != human);
        }
        for c in &mut self.creatures {
            c.mind.forget(human);
        }
        self.order_budget.remove(&human);
    }

    /// An order from `commander` for the squad slots in `slots` (COMPANIONS.md 5.3).
    /// `visible`: whether the commander's client is being sent that body.
    pub fn order(
        &mut self,
        zone: &Zone,
        commander: EntityId,
        slots: u8,
        order: Order,
        visible: &dyn Fn(EntityId) -> bool,
    ) -> Result<(), String> {
        let c = zone.player(commander).ok_or("you are not here")?;
        let in_stance = c.alive
            && c.mover.commanding(c.last_input_tick)
            && c.mover.buttons_prev & buttons::COMMAND != 0;
        if !in_stance {
            return Err("orders are given from the command stance".into());
        }
        let hz = zone.rate.hz();
        let budget = self.order_budget.entry(commander).or_insert((zone.tick, 0));
        if tick_delta(zone.tick, budget.0) >= hz as i32 {
            *budget = (zone.tick, 0);
        }
        if budget.1 >= ORDERS_PER_S {
            return Err("too many orders".into());
        }
        budget.1 += 1;
        let members = self.squads.get(&commander).ok_or("you command nobody")?;
        let addressed: Vec<usize> = (0..members.len())
            .filter(|i| slots & (1 << i) != 0)
            .collect();
        if addressed.is_empty() {
            return Err("no such companion".into());
        }
        match order {
            Order::MoveTo(point) => {
                if !point.is_finite() || (point - c.mover.mv.origin).length() > MOVE_TO_RANGE {
                    return Err("too far".into());
                }
                let node = self
                    .nav
                    .nearest(point)
                    .filter(|&n| (self.nav.pos(n) - point).truncate().length() <= MOVE_TO_SNAP)
                    .ok_or("nobody can stand there")?;
                for &i in &addressed {
                    let from = zone
                        .player(members[i].id)
                        .map_or(point, |p| p.mover.mv.origin);
                    let reachable = self
                        .nav
                        .nearest(from)
                        .is_some_and(|a| self.nav.reachable_node(a, node));
                    if !reachable {
                        return Err("there is no way there".into());
                    }
                }
            }
            Order::Attack(target) => {
                let ok = zone.player(target).is_some_and(|t| t.alive)
                    && target != commander
                    && self.owners.get(&target) != Some(&commander)
                    && visible(target);
                if !ok {
                    return Err("no such target in sight".into());
                }
            }
            Order::Follow | Order::Hold => {}
        }
        let now = zone.tick;
        let members = self.squads.get_mut(&commander).expect("checked");
        for i in addressed {
            let m = &mut members[i];
            let here = zone.player(m.id).map_or(Vec3::ZERO, |p| p.mover.mv.origin);
            let frame_now = zone
                .player(m.id)
                .map_or(now, |p| p.last_input_tick.wrapping_add(1));
            // A `MoveTo` goes to the standable spot nearest the point.
            let order = match order {
                Order::MoveTo(point) => {
                    Order::MoveTo(self.nav.nearest(point).map_or(point, |n| self.nav.pos(n)))
                }
                o => o,
            };
            m.mind.set_order(order, here, frame_now);
        }
        self.events.push(DirectorEvent::Squad { commander });
        Ok(())
    }

    fn who(&self, zone: &Zone, id: EntityId) -> Option<Who> {
        if self.creature_ids.contains_key(&id) {
            return None;
        }
        match self.owners.get(&id) {
            Some(&owner) => Some(Who {
                id,
                party: zone.player(owner).map_or(owner, |p| p.party),
                owner,
                human: false,
            }),
            None => zone.player(id).map(|p| Who {
                id,
                party: p.party,
                owner: id,
                human: true,
            }),
        }
    }

    /// Before the simulation step: every mind thinks and hands its frame over.
    pub fn pre_step(&mut self, zone: &mut Zone, world: &dyn CollisionWorld) {
        // A zone without minds (a town, an arena of humans) pays nothing for having a
        // director.
        if self.squads.is_empty() && self.creatures.is_empty() {
            return;
        }
        let now = zone.tick;
        self.foes
            .retain(|(_, _, until)| tick_delta(*until, now) > 0);
        {
            let (ids, creatures) = (&self.creature_ids, &self.creatures);
            self.view
                .refresh(zone, &|id| ids.get(&id).map(|&i| creatures[i].def));
        }
        let mut frames: Vec<(EntityId, gm_core::sim::Input)> = Vec::new();
        let Director {
            view,
            squads,
            creatures,
            foes,
            nav,
            events,
            ..
        } = self;
        for (&commander, members) in squads.iter_mut() {
            let party = zone.player(commander).map_or(commander, |p| p.party);
            let commander_body = view.bodies.iter().find(|b| b.id == commander && b.alive);
            // Whom the party fights: grievances, and every creature that has one of the
            // party on its threat table.
            let mut party_foes: Vec<EntityId> = foes
                .iter()
                .filter(|(p, _, _)| *p == party)
                .map(|(_, f, _)| *f)
                .collect();
            for c in creatures.iter() {
                if c.mind
                    .table()
                    .iter()
                    .any(|(id, _)| view.bodies.iter().any(|b| b.id == *id && b.party == party))
                {
                    party_foes.push(c.id);
                }
            }
            let mut changed = false;
            for m in members.iter_mut() {
                if !zone.player(m.id).is_some_and(|p| p.alive) {
                    continue;
                }
                let Some(s) = view.senses(zone, m.id, world, nav) else {
                    continue;
                };
                let before = m.mind.order();
                let input = m.mind.think(
                    &s,
                    &Squad {
                        commander: commander_body,
                        foes: &party_foes,
                    },
                );
                frames.push((m.id, input));
                // An order that ended by itself (its target fell, or was lost) is news to
                // the commander as much as one it gave.
                changed |= m.mind.order() != before;
            }
            if changed {
                events.push(DirectorEvent::Squad { commander });
            }
        }
        for c in creatures.iter_mut() {
            if !zone.player(c.id).is_some_and(|p| p.alive) {
                continue;
            }
            let Some(s) = view.senses(zone, c.id, world, nav) else {
                continue;
            };
            frames.push((c.id, c.mind.think(&s)));
        }
        for (id, input) in frames {
            zone.drive(id, input);
        }
    }

    /// After the simulation step: the ledgers, the encounters, the dead.
    pub fn post_step(&mut self, zone: &mut Zone, world: &dyn CollisionWorld, events: &[ZoneEvent]) {
        if self.squads.is_empty() && self.creatures.is_empty() {
            self.foes.clear();
            return;
        }
        let now = zone.tick;
        let hz = zone.rate.hz();
        let ticks = |ms: u32| -> Tick { (ms as u64 * hz as u64).div_ceil(1000) as Tick };
        for ev in events {
            match *ev {
                ZoneEvent::Hit {
                    attacker,
                    target,
                    amount,
                    kind,
                    absorbed,
                } => {
                    let a_slot = self.creature_ids.get(&attacker).copied();
                    let t_slot = self.creature_ids.get(&target).copied();
                    match (a_slot, t_slot) {
                        // Somebody hurt a creature.
                        (None, Some(slot)) => {
                            let Some(who) = self.who(zone, attacker) else {
                                continue;
                            };
                            let ordered = !who.human
                                && self.squads.get(&who.owner).is_some_and(|ms| {
                                    ms.iter().any(|m| {
                                        m.id == attacker && m.mind.attack_order() == Some(target)
                                    })
                                });
                            let enc = self.creatures[slot].encounter;
                            self.creatures[slot].mind.hurt_by(attacker, amount as f32);
                            self.engage(zone, enc);
                            let e = &mut self.encounters[enc];
                            e.ledger.dealt(&who, target, amount.max(0) as u32, ordered);
                            e.last_damage = now;
                        }
                        // A creature hurt somebody.
                        (Some(slot), None) => {
                            let Some(who) = self.who(zone, target) else {
                                continue;
                            };
                            let enc = self.creatures[slot].encounter;
                            if absorbed > 0 {
                                self.creatures[slot]
                                    .mind
                                    .blocked_by(target, absorbed as f32);
                            }
                            self.creatures[slot].mind.alert(target);
                            self.engage(zone, enc);
                            let blow = matches!(kind, HitKind::Melee | HitKind::Projectile);
                            let e = &mut self.encounters[enc];
                            e.ledger.struck(
                                &who,
                                amount.max(0) as u32,
                                (amount + absorbed).max(0) as u32,
                                blow,
                            );
                            e.last_damage = now;
                        }
                        // Bodies that are not creatures: a party remembers who hurt it.
                        (None, None) => {
                            let (Some(a), Some(t)) =
                                (self.who(zone, attacker), self.who(zone, target))
                            else {
                                continue;
                            };
                            if a.party != t.party {
                                let until = now.wrapping_add(ticks(GRIEVANCE_MS));
                                self.foes
                                    .retain(|(p, f, _)| !(*p == t.party && *f == attacker));
                                self.foes.push((t.party, attacker, until));
                            }
                        }
                        (Some(_), Some(_)) => {}
                    }
                }
                ZoneEvent::Healed {
                    target,
                    source,
                    amount,
                } => {
                    // Self-applied Regen has no source: it is the target's own.
                    let healer = if source == 0 { target } else { source };
                    let Some(who) = self.who(zone, healer) else {
                        continue;
                    };
                    for e in &mut self.encounters {
                        if e.phase != Phase::Engaged || !e.ledger.is_participant(target) {
                            continue;
                        }
                        let credit = e.ledger.healed(&who, target, amount.max(0) as u32);
                        if credit > 0 {
                            for &c in &e.creatures {
                                if self.creatures[c].mind.knows(target) {
                                    self.creatures[c].mind.healing_by(healer, credit as f32);
                                }
                            }
                        }
                    }
                }
                ZoneEvent::Killed { victim, .. } => {
                    if self.creature_ids.contains_key(&victim) {
                        continue;
                    }
                    // The dead wait while their party is still in the encounter.
                    let holder = self
                        .encounters
                        .iter()
                        .position(|e| e.phase == Phase::Engaged && e.ledger.is_participant(victim));
                    if let Some(enc) = holder {
                        self.encounters[enc].ledger.died(victim);
                    }
                    match self.owners.get(&victim).copied() {
                        Some(owner) => {
                            if let Some(m) = self
                                .squads
                                .get_mut(&owner)
                                .and_then(|ms| ms.iter_mut().find(|m| m.id == victim))
                            {
                                m.held_by = holder;
                                m.back_at = Some(now.wrapping_add(ticks(COMPANION_RESPAWN_MS)));
                            }
                        }
                        None => {
                            if let Some(enc) = holder {
                                zone.set_hold(victim, true);
                                self.encounters[enc].held.push(victim);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        for enc in 0..self.encounters.len() {
            self.run_encounter(zone, enc, now, hz);
        }
        self.raise_companions(zone, world, now);
    }

    /// An idle encounter becomes engaged.
    fn engage(&mut self, zone: &Zone, enc: usize) {
        let e = &mut self.encounters[enc];
        if e.phase != Phase::Idle {
            return;
        }
        e.phase = Phase::Engaged;
        e.ledger = Ledger::new(zone.tick);
        e.last_damage = zone.tick;
        e.out_since.clear();
        let tell = self.audience(zone, enc);
        self.events.push(DirectorEvent::Encounter {
            name: self.encounters[enc].name.clone(),
            state: EncounterState::Engaged,
            tell,
        });
    }

    /// The humans an encounter's news is for: its participants and whoever is near it.
    fn audience(&self, zone: &Zone, enc: usize) -> Vec<EntityId> {
        let e = &self.encounters[enc];
        zone.players()
            .filter(|p| self.kind_of(p.id) == BodyKind::Human)
            .filter(|p| {
                e.ledger.is_participant(p.id)
                    || (p.mover.mv.origin - e.anchor).length() <= e.leash + 1024.0
            })
            .map(|p| p.id)
            .collect()
    }

    fn run_encounter(&mut self, zone: &mut Zone, enc: usize, now: Tick, hz: u32) {
        let ticks = |ms: u32| -> Tick { (ms as u64 * hz as u64).div_ceil(1000) as Tick };
        match self.encounters[enc].phase {
            Phase::Idle => {
                // A creature that perceived somebody engages the whole encounter.
                let seen = self.encounters[enc]
                    .creatures
                    .iter()
                    .any(|&c| self.creatures[c].mind.engaged());
                if seen {
                    self.engage(zone, enc);
                }
            }
            Phase::Engaged => {
                // Pull one, fight all.
                let known: Vec<EntityId> = self.encounters[enc]
                    .creatures
                    .iter()
                    .flat_map(|&c| self.creatures[c].mind.table())
                    .map(|(id, _)| id)
                    .collect();
                for &c in &self.encounters[enc].creatures.clone() {
                    for &id in &known {
                        self.creatures[c].mind.alert(id);
                    }
                }
                let all_dead = self.encounters[enc]
                    .creatures
                    .iter()
                    .all(|&c| !zone.player(self.creatures[c].id).is_some_and(|p| p.alive));
                if all_dead {
                    self.clear(zone, enc, now, hz);
                    return;
                }
                // Parties with nobody alive in the leash go out after a while.
                let (anchor, leash) = (self.encounters[enc].anchor, self.encounters[enc].leash);
                let parties = self.encounters[enc].ledger.parties();
                for party in parties {
                    let present = self.encounters[enc].ledger.participants().any(|(id, e)| {
                        e.party == party
                            && zone.player(id).is_some_and(|p| {
                                p.alive
                                    && !p.ghost
                                    && (p.mover.mv.origin - anchor).length() <= leash + 256.0
                            })
                    });
                    let e = &mut self.encounters[enc];
                    if present {
                        e.out_since.retain(|(p, _)| *p != party);
                        continue;
                    }
                    let since = match e.out_since.iter().find(|(p, _)| *p == party) {
                        Some((_, t)) => *t,
                        None => {
                            e.out_since.push((party, now));
                            now
                        }
                    };
                    if tick_delta(now, since) >= ticks(OUT_MS) as i32 {
                        self.party_out(zone, enc, party, now, hz);
                    }
                }
                let e = &self.encounters[enc];
                let quiet = tick_delta(now, e.last_damage) >= ticks(QUIET_MS) as i32;
                let long = tick_delta(now, e.ledger.engaged) >= ticks(MAX_ENCOUNTER_MS) as i32;
                // The last party went out, nothing has happened for a while, or it has
                // gone on too long.
                let nobody = e.ledger.is_empty()
                    && e.creatures
                        .iter()
                        .all(|&c| !self.creatures[c].mind.engaged());
                if quiet || long || nobody {
                    self.reset(zone, enc, now, hz);
                }
            }
            Phase::Cleared => {
                let due = self.encounters[enc]
                    .respawn_at
                    .is_some_and(|at| tick_delta(now, at) >= 0);
                if !due {
                    return;
                }
                // Not onto a body: a post that is stood on waits.
                let free = self.encounters[enc].creatures.iter().all(|&c| {
                    let home = self.creatures[c].home;
                    !zone
                        .players()
                        .any(|p| p.alive && (p.mover.mv.origin - home).length() < 64.0)
                });
                if free {
                    self.restore(zone, enc);
                    self.encounters[enc].phase = Phase::Idle;
                    self.encounters[enc].respawn_at = None;
                }
            }
        }
    }

    /// Put every creature of an encounter back on its post, whole.
    fn restore(&mut self, zone: &mut Zone, enc: usize) {
        for &c in &self.encounters[enc].creatures.clone() {
            let slot = &mut self.creatures[c];
            zone.revive(slot.id, slot.home, slot.yaw, true);
            slot.mind.reset();
        }
    }

    /// Let an encounter's dead go.
    fn release(&mut self, zone: &mut Zone, enc: usize, party: Option<u32>, now: Tick, hz: u32) {
        let back = now.wrapping_add((RELEASE_RESPAWN_MS as u64 * hz as u64).div_ceil(1000) as Tick);
        let held = std::mem::take(&mut self.encounters[enc].held);
        let mut keep = Vec::new();
        for id in held {
            let of_party = zone.player(id).map(|p| p.party);
            if party.is_none() || of_party == party {
                zone.set_hold(id, false);
            } else {
                keep.push(id);
            }
        }
        self.encounters[enc].held = keep;
        for (&owner, members) in self.squads.iter_mut() {
            let owner_party = zone.player(owner).map(|p| p.party);
            if party.is_some() && owner_party != party {
                continue;
            }
            for m in members.iter_mut().filter(|m| m.held_by == Some(enc)) {
                m.held_by = None;
                m.back_at = Some(back);
            }
        }
    }

    /// A party goes out (COMPANIONS.md 9): its damage is undone and its dead are released.
    fn party_out(&mut self, zone: &mut Zone, enc: usize, party: u32, now: Tick, hz: u32) {
        let members: Vec<EntityId> = self.encounters[enc]
            .ledger
            .participants()
            .filter(|(_, e)| e.party == party)
            .map(|(id, _)| id)
            .collect();
        let back = self.encounters[enc].ledger.remove_party(party);
        for (creature, amount) in back {
            if let Some(p) = zone.player_mut(creature)
                && p.alive
            {
                p.health = (p.health as i64 + amount as i64).min(p.max_health() as i64) as i32;
            }
        }
        for &c in &self.encounters[enc].creatures.clone() {
            for &id in &members {
                self.creatures[c].mind.forget(id);
            }
        }
        self.encounters[enc].out_since.retain(|(p, _)| *p != party);
        self.release(zone, enc, Some(party), now, hz);
    }

    fn reset(&mut self, zone: &mut Zone, enc: usize, now: Tick, hz: u32) {
        let tell = self.audience(zone, enc);
        self.restore(zone, enc);
        self.release(zone, enc, None, now, hz);
        let e = &mut self.encounters[enc];
        e.phase = Phase::Idle;
        e.ledger = Ledger::new(now);
        e.out_since.clear();
        self.events.push(DirectorEvent::Encounter {
            name: e.name.clone(),
            state: EncounterState::Reset,
            tell,
        });
    }

    fn clear(&mut self, zone: &mut Zone, enc: usize, now: Tick, hz: u32) {
        let tell = self.audience(zone, enc);
        let (anchor, leash) = (self.encounters[enc].anchor, self.encounters[enc].leash);
        let secs = (tick_delta(now, self.encounters[enc].ledger.engaged).max(0) as u32) / hz.max(1);
        // Loot (COMPANIONS.md 10): bosses only.
        let boss_loot = self.encounters[enc].boss.and_then(|b| {
            zone.content.creatures[self.creatures[b].def as usize]
                .loot
                .clone()
        });
        let ledger = &self.encounters[enc].ledger;
        if let Some(l) = boss_loot.filter(|l| l.components > 0) {
            let present = |party: u32| {
                ledger.participants().any(|(id, e)| {
                    e.party == party
                        && zone.player(id).is_some_and(|p| {
                            p.alive && (p.mover.mv.origin - anchor).length() <= leash + 256.0
                        })
                })
            };
            let parties = ledger.loot_parties(present, |human| zone.player(human).is_some());
            let split = loot::split(l.components as u32, &parties);
            let mut grants: Vec<LootGrant> = Vec::new();
            let mut next = [0usize; 2];
            for (member, count) in &split {
                let human = *member as EntityId;
                let party = zone.player(human).map_or(human, |p| p.party);
                // The ceiling: a party that brought companions draws from the standard list.
                let (list, cursor) = if ledger.has_companions(party) {
                    (&l.standard, &mut next[0])
                } else {
                    (&l.top, &mut next[1])
                };
                let items = (0..*count)
                    .map(|_| {
                        let item = list[*cursor % list.len()].clone();
                        *cursor += 1;
                        item
                    })
                    .collect();
                grants.push(LootGrant {
                    human,
                    items,
                    coin: 0,
                });
            }
            if !grants.is_empty() {
                let each = l.coin / grants.len() as u32;
                let rest = l.coin - each * grants.len() as u32;
                for (i, g) in grants.iter_mut().enumerate() {
                    g.coin = each + if i == 0 { rest } else { 0 };
                }
                self.kills += 1;
                self.events.push(DirectorEvent::Loot {
                    encounter: self.encounters[enc].name.clone(),
                    kill: self.kills,
                    grants,
                });
            }
        }
        // Trials (COMPANIONS.md 11): every human participant against every trial of this
        // encounter.
        let name = self.encounters[enc].name.clone();
        let humans: Vec<EntityId> = ledger
            .participants()
            .filter(|(_, e)| e.human)
            .map(|(id, _)| id)
            .collect();
        for trial in zone
            .content
            .trials
            .iter()
            .filter(|t| t.map == self.map && t.encounter == name)
        {
            for &human in &humans {
                let Some(standing) = ledger.standing(human, secs) else {
                    continue;
                };
                self.events.push(DirectorEvent::Trial {
                    human,
                    key: trial.key.clone(),
                    name: trial.name.clone(),
                    verdict: judge(trial, &standing),
                    standing,
                    secs,
                });
            }
        }
        self.release(zone, enc, None, now, hz);
        let respawn = self.encounters[enc]
            .creatures
            .iter()
            .map(|&c| zone.content.creatures[self.creatures[c].def as usize].respawn_s)
            .max()
            .unwrap_or(0);
        for &c in &self.encounters[enc].creatures.clone() {
            self.creatures[c].mind.reset();
        }
        let e = &mut self.encounters[enc];
        e.phase = Phase::Cleared;
        e.out_since.clear();
        e.respawn_at = (respawn > 0).then(|| now.wrapping_add(respawn as Tick * hz));
        self.events.push(DirectorEvent::Encounter {
            name,
            state: EncounterState::Cleared { secs },
            tell,
        });
    }

    /// Companions that are due stand up beside their commander.
    fn raise_companions(&mut self, zone: &mut Zone, world: &dyn CollisionWorld, now: Tick) {
        let mut due: Vec<(EntityId, EntityId)> = Vec::new();
        for (&owner, members) in &self.squads {
            if !zone.player(owner).is_some_and(|p| p.alive && !p.ghost) {
                continue;
            }
            for m in members {
                if m.held_by.is_none()
                    && m.back_at.is_some_and(|at| tick_delta(now, at) >= 0)
                    && zone.player(m.id).is_some_and(|p| !p.alive)
                {
                    due.push((owner, m.id));
                }
            }
        }
        for (owner, id) in due {
            let (at, yaw) = {
                let c = zone.player(owner).expect("checked");
                (c.mover.mv.origin, c.mover.yaw)
            };
            let spot = zone.spot_near(world, at, Hull::Player);
            zone.revive(id, spot, yaw, false);
            if let Some(m) = self
                .squads
                .get_mut(&owner)
                .and_then(|ms| ms.iter_mut().find(|m| m.id == id))
            {
                m.back_at = None;
            }
        }
    }

    /// The state of an encounter by name, for reports and tests.
    pub fn encounter_engaged(&self, name: &str) -> bool {
        self.encounters
            .iter()
            .any(|e| e.name == name && e.phase == Phase::Engaged)
    }

    /// The ledger of an engaged encounter.
    pub fn ledger(&self, name: &str) -> Option<&Ledger> {
        self.encounters
            .iter()
            .find(|e| e.name == name && e.phase == Phase::Engaged)
            .map(|e| &e.ledger)
    }

    /// The living creatures of an encounter: `(id, health)`.
    pub fn creatures_of(&self, zone: &Zone, name: &str) -> Vec<(EntityId, i32)> {
        self.encounters
            .iter()
            .filter(|e| e.name == name)
            .flat_map(|e| e.creatures.iter())
            .filter_map(|&c| {
                let p = zone.player(self.creatures[c].id)?;
                p.alive.then_some((p.id, p.health))
            })
            .collect()
    }

    /// Whom a creature is fighting.
    pub fn creature_target(&self, id: EntityId) -> Option<EntityId> {
        self.creature_ids
            .get(&id)
            .and_then(|&slot| self.creatures[slot].mind.target())
    }

    /// Where the creatures of an encounter are posted: its anchor.
    pub fn anchor(&self, name: &str) -> Option<Vec3> {
        self.encounters
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.anchor)
    }

    /// Minds the director runs right now (for the per-mind time budget).
    pub fn minds(&self) -> usize {
        self.creatures.len() + self.squads.values().map(|m| m.len()).sum::<usize>()
    }
}
