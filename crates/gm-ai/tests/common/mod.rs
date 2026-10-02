//! The offline harness (COMPANIONS.md 14, step 1): a commander driven by the raider brain,
//! its squad and a map's creatures, run straight through `gm_core::sim::Zone` and the
//! director. No network, no clock: five minutes of fighting take a fraction of a second.
#![allow(dead_code)]

use std::path::Path;
use std::time::{Duration, Instant};

use glam::Vec3;
use gm_ai::director::{CompanionSpec, CreatureSpawn, Director, DirectorEvent, EncounterState};
use gm_ai::view::ZoneView;
use gm_ai::{BodyKind, Mate, Raider, Request, objectives};
use gm_core::build::{ContentPack, Sheet};
use gm_core::sim::{Driver, Input, Spawn, Zone, ZoneEvent};
use gm_core::tick::TickRate;
use gm_core::trace::CollisionWorld;
use gm_core::vocab::EntityId;

pub const CONTENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
pub const MAPS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/maps/built");
pub const RATE: TickRate = TickRate::COMBAT;

pub fn content() -> ContentPack {
    gm_content::load_dir(Path::new(CONTENT), RATE).expect("content")
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// Encounters cleared, in order, with the seconds each took.
    pub cleared: Vec<(String, u32)>,
    pub resets: u32,
    pub party_deaths: u32,
    /// Hits by the party on the party.
    pub friendly_hits: u32,
    pub loot: Vec<(EntityId, Vec<String>, u32)>,
    pub trials: Vec<(String, bool, String)>,
    /// Health the boss of the last objective had left.
    pub boss_health: i32,
    /// Simulated seconds until the raider was done, or the time ran out.
    pub secs: f32,
    pub orders: u32,
    /// The raider cleared every objective.
    pub done: bool,
    /// Per mille of the party's damage dealt under the leader's orders in the last kill.
    pub command: u32,
    /// Minds run (companions and creatures) and the mean time they took per tick, all of
    /// them together.
    pub minds: usize,
    pub minds_us: f64,
    pub ticks: u32,
}

impl Outcome {
    pub fn cleared_in(&self, encounter: &str) -> Option<u32> {
        self.cleared
            .iter()
            .find(|(n, _)| n == encounter)
            .map(|(_, s)| *s)
    }

    pub fn passed(&self, trial: &str) -> bool {
        self.trials.iter().any(|(k, ok, _)| k == trial && *ok)
    }
}

pub struct Setup<'a> {
    pub world: &'a dyn CollisionWorld,
    /// The map's name: trials name the map they belong to.
    pub map: &'a str,
    pub spawn: Vec3,
    pub yaw: f32,
    pub posts: Vec<CreatureSpawn>,
    pub squad: &'a [&'a str],
    pub commander: &'a str,
    /// The commander joins the fight itself instead of leading from the back.
    pub fights: bool,
    pub seed: u64,
    pub max_secs: u32,
    /// Override the health of the last post's creature (tuning runs).
    pub boss_health: Option<i32>,
    /// Change the content before the zone is made (tuning runs).
    pub tune: Option<&'a dyn Fn(&mut ContentPack)>,
    /// The raider sends its tanks to a second creature (its default).
    pub split: bool,
    pub verbose: bool,
}

/// The commander is a body the harness drives with the raider brain: it walks to each
/// encounter, takes the stance when a creature is in sight, orders the squad onto it and
/// keeps out of its sight.
pub fn play(setup: &Setup<'_>) -> Outcome {
    let Setup {
        world,
        squad,
        seed,
        max_secs,
        verbose,
        spawn,
        ..
    } = *setup;
    let mut pack = content();
    if let Some(tune) = setup.tune {
        tune(&mut pack);
    }
    let mut zone = Zone::new(
        RATE,
        seed,
        vec![Spawn {
            origin: spawn,
            yaw: setup.yaw,
            team: 1,
        }],
        pack.clone(),
    );
    let build = pack.build(setup.commander).expect(setup.commander).clone();
    let sheet = Sheet::new(build, &pack, 1);
    let commander = zone.add_body(sheet.clone(), spawn, setup.yaw, Driver::Mind);
    let mut director = Director::new(&mut zone, world, setup.map, &[spawn], &setup.posts, seed);
    for (i, name) in squad.iter().enumerate() {
        let id = director.add_companion(
            &mut zone,
            world,
            commander,
            CompanionSpec {
                name: format!("{name}-{i}"),
                build: pack.build(name).expect(name).clone(),
                recruit: true,
                hire: None,
            },
        );
        assert!(id.is_some(), "{name} joins the squad");
    }
    let last = setup.posts.last().expect("a post");
    let boss = director.creatures_of(&zone, &last.encounter)[0].0;
    if let Some(h) = setup.boss_health {
        let w = zone.player_mut(boss).unwrap();
        w.sheet.derived.health = h;
        w.health = h;
    }
    let posts: Vec<(String, Vec3)> = setup
        .posts
        .iter()
        .map(|p| (p.encounter.clone(), p.origin))
        .collect();
    let mut raider = Raider::new(
        seed ^ 0xc0de,
        &sheet,
        setup.yaw,
        objectives(&director.nav, spawn, &posts),
    );
    raider.fights = setup.fights;
    raider.split = setup.split;
    let mut view = ZoneView::default();
    let mut out = Outcome {
        minds: director.minds(),
        ..Outcome::default()
    };
    let mut spent = Duration::ZERO;
    let hz = RATE.hz();
    let mut done_at: Option<u32> = None;
    for t in 0..max_secs * hz {
        let now = t as f32 / hz as f32;
        let alive = zone.player(commander).is_some_and(|p| p.alive);
        let kind = |id: EntityId| match director.kind_of(id) {
            BodyKind::Creature { def } => Some(def),
            _ => None,
        };
        view.refresh(&zone, &kind);
        let input = if !alive {
            Input::default()
        } else {
            let mates: Vec<Mate> = director
                .squad(commander)
                .iter()
                .enumerate()
                .map(|(slot, m)| Mate {
                    id: m.id,
                    slot: slot as u8,
                    role: m.role(),
                    attacking: m.mind.attack_order(),
                })
                .collect();
            let s = view.senses(&zone, commander, world, &director.nav).unwrap();
            let (input, request) = raider.think(&s, &mates);
            if let Some(Request::Order { slots, order }) = request {
                let r = director.order(&zone, commander, slots, order, &|_| true);
                if verbose {
                    println!("[{now:6.1}s] order {order:?}: {r:?}");
                }
                if r.is_ok() {
                    out.orders += 1;
                }
            }
            input
        };
        zone.drive(commander, input);
        let t0 = Instant::now();
        director.pre_step(&mut zone, world);
        spent += t0.elapsed();
        zone.step(world);
        let events: Vec<ZoneEvent> = zone.events.drain(..).collect();
        for ev in &events {
            match ev {
                ZoneEvent::Hit {
                    attacker,
                    target,
                    amount,
                    kind,
                    ..
                } if attacker != target
                    && !matches!(director.kind_of(*attacker), BodyKind::Creature { .. })
                    && !matches!(director.kind_of(*target), BodyKind::Creature { .. }) =>
                {
                    out.friendly_hits += 1;
                    if verbose {
                        println!(
                            "[{now:6.1}s] friendly fire: {attacker} hit {target} for {amount} ({kind:?})"
                        );
                    }
                }
                ZoneEvent::Hit {
                    attacker,
                    target,
                    amount,
                    kind,
                    absorbed,
                } if verbose && std::env::var("GM_HITS").is_ok() => {
                    let hp = zone.player(*target).map_or(0, |p| p.health);
                    println!(
                        "[{now:6.1}s] {attacker} hit {target} for {amount} ({kind:?}, {absorbed} absorbed), {hp} left"
                    );
                }
                ZoneEvent::Killed { victim, killer } => {
                    let who = director.kind_of(*victim);
                    if !matches!(who, BodyKind::Creature { .. }) {
                        out.party_deaths += 1;
                    }
                    if verbose {
                        println!("[{now:6.1}s] {victim} ({who:?}) killed by {killer}");
                    }
                }
                _ => {}
            }
        }
        let t0 = Instant::now();
        director.post_step(&mut zone, world, &events);
        spent += t0.elapsed();
        for ev in director.events.drain(..) {
            match ev {
                DirectorEvent::Encounter { name, state, .. } => {
                    if verbose {
                        println!("[{now:6.1}s] encounter {name}: {state:?}");
                    }
                    match state {
                        EncounterState::Cleared { secs } => out.cleared.push((name, secs)),
                        EncounterState::Reset => out.resets += 1,
                        EncounterState::Engaged => {}
                    }
                }
                DirectorEvent::Loot { grants, .. } => {
                    for g in grants {
                        out.loot.push((g.human, g.items, g.coin));
                    }
                }
                DirectorEvent::Trial {
                    key,
                    verdict,
                    standing,
                    ..
                } => {
                    let why = verdict
                        .as_ref()
                        .err()
                        .map_or(String::new(), |f| f.to_string());
                    if key == "warden_leader" {
                        out.command = standing.command;
                    }
                    out.trials.push((key, verdict.is_ok(), why));
                }
                _ => {}
            }
        }
        if verbose && std::env::var("GM_POS").is_ok() && t % (hz / 2) == 0 {
            let line: Vec<String> = director
                .squad(commander)
                .iter()
                .map(|m| {
                    let p = zone.player(m.id).unwrap();
                    format!(
                        "{} ({:.0},{:.0}) hp {} {:?} anim {}",
                        m.role().name(),
                        p.mover.mv.origin.x,
                        p.mover.mv.origin.y,
                        p.health,
                        m.mind.order(),
                        p.anim
                    )
                })
                .collect();
            println!("[{now:6.1}s] {}", line.join(" | "));
        }
        if verbose && t % (hz * 10) == 0 {
            let hp = |id: EntityId| {
                zone.player(id)
                    .map_or(-1, |p| if p.alive { p.health } else { 0 })
            };
            let squad_hp: Vec<String> = director
                .squad(commander)
                .iter()
                .map(|m| format!("{}:{}", m.role().name(), hp(m.id)))
                .collect();
            let at = zone.player(commander).map(|p| p.mover.mv.origin.round());
            println!(
                "[{now:6.1}s] objective {} | boss {} | commander {} at {at:?} | {}",
                raider.objective(),
                hp(boss),
                hp(commander),
                squad_hp.join(" ")
            );
        }
        out.secs = now;
        out.ticks = t + 1;
        // Done: every objective cleared. Run two seconds more so the loot and the trial
        // verdicts of the last kill are in.
        if raider.done() && done_at.is_none() {
            done_at = Some(t);
            out.done = true;
        }
        if done_at.is_some_and(|d| t >= d + 2 * hz) {
            break;
        }
    }
    out.boss_health = zone.player(boss).map_or(0, |p| p.health);
    out.minds_us = spent.as_secs_f64() * 1e6 / out.ticks.max(1) as f64;
    out
}

pub fn report(label: &str, o: &Outcome) {
    println!(
        "{label}: done {} after {:.1} s, cleared {:?}, resets {}, party deaths {}, friendly hits {}, orders {}, under orders {:.1}%, boss health left {}, loot {:?}, trials {:?}, {} minds at {:.1} us/tick",
        o.done,
        o.secs,
        o.cleared,
        o.resets,
        o.party_deaths,
        o.friendly_hits,
        o.orders,
        o.command as f32 / 10.0,
        o.boss_health,
        o.loot,
        o.trials,
        o.minds,
        o.minds_us
    );
}
