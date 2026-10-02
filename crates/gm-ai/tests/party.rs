//! Two humans and their squads at one boss (PARTY.md 2, 9 step 1): as one party, and as
//! two. The harness is the skirmish's: minds against minds on a bare floor.

mod common;

use common::{Ally, Outcome, Setup, content, play, report};
use glam::Vec3;
use gm_ai::director::{CompanionSpec, CreatureSpawn, Director};
use gm_core::build::Sheet;
use gm_core::collide::BoxWorld;
use gm_core::sim::{Driver, Spawn, Zone};

const SQUAD: [&str; 3] = ["ironclad", "mender", "frostweaver"];

fn hall() -> BoxWorld {
    let mut w = BoxWorld::floor();
    for (lo, hi) in [
        ((-1216.0, -816.0), (-1200.0, 816.0)),
        ((1200.0, -816.0), (1216.0, 816.0)),
        ((-1216.0, -816.0), (1216.0, -800.0)),
        ((-1216.0, 800.0), (1216.0, 816.0)),
    ] {
        w.push(Vec3::new(lo.0, lo.1, 0.0), Vec3::new(hi.0, hi.1, 300.0));
    }
    w
}

fn warden() -> Vec<CreatureSpawn> {
    vec![CreatureSpawn {
        creature: "warden".into(),
        encounter: "warden".into(),
        origin: Vec3::new(700.0, 0.0, 24.0),
        yaw: 180.0,
    }]
}

fn run(together: bool, seed: u64) -> Outcome {
    let world = hall();
    play(&Setup {
        world: &world,
        map: "dungeon",
        spawn: Vec3::new(-900.0, 0.0, 24.0),
        yaw: 0.0,
        posts: warden(),
        squad: &SQUAD,
        commander: "blade",
        fights: false,
        seed,
        max_secs: 300,
        boss_health: None,
        tune: None,
        split: true,
        verbose: std::env::var("GM_VERBOSE").is_ok(),
        ally: Some(Ally {
            commander: "blade",
            squad: &SQUAD,
            party: together,
        }),
    })
}

#[test]
fn two_humans_of_one_party_are_one_party_to_the_ledger_and_both_are_paid() {
    for seed in 1..=2 {
        let o = run(true, seed);
        report(&format!("one party, seed {seed}"), &o);
        assert!(o.cleared_in("warden").is_some(), "seed {seed}: no kill");
        assert_eq!(o.parties, 1, "seed {seed}: one party on the ledger");
        // Both are paid: something each, and the coin evenly.
        let paid = |h| o.loot.iter().find(|(id, _, _)| *id == h);
        let (a, b) = (
            paid(o.humans[0]).expect("the first"),
            paid(o.humans[1]).expect("the second"),
        );
        assert!(
            !a.1.is_empty() && !b.1.is_empty(),
            "seed {seed}: {:?}",
            o.loot
        );
        assert!(a.2.abs_diff(b.2) <= 1, "seed {seed}: {:?}", o.loot);
        // A trial for one human and its squad is not passed by two.
        assert!(
            o.trials
                .iter()
                .all(|(_, passed, why)| !passed && why.contains("players in the party")),
            "seed {seed}: {:?}",
            o.trials
        );
    }
}

#[test]
fn two_humans_who_are_not_a_party_are_two_parties_at_the_same_boss() {
    let o = run(false, 1);
    report("two parties", &o);
    assert!(o.cleared_in("warden").is_some(), "no kill");
    assert_eq!(o.parties, 2);
    // Both were there and both worked: both are paid, by the split between parties.
    for h in &o.humans {
        assert!(
            o.loot
                .iter()
                .any(|(id, items, _)| id == h && !items.is_empty()),
            "{:?}",
            o.loot
        );
    }
}

#[test]
fn a_squad_follows_its_commander_s_party() {
    let pack = content();
    let world = hall();
    let spawn = Vec3::new(-900.0, 0.0, 24.0);
    let mut zone = Zone::new(
        common::RATE,
        1,
        vec![Spawn {
            origin: spawn,
            yaw: 0.0,
            team: 1,
        }],
        pack.clone(),
    );
    let sheet = Sheet::new(pack.build("blade").unwrap().clone(), &pack, 1);
    let a = zone.add_body(sheet.clone(), spawn, 0.0, Driver::Mind);
    let b = zone.add_body(sheet, spawn + Vec3::new(0.0, 96.0, 0.0), 0.0, Driver::Mind);
    let mut director = Director::new(&mut zone, &world, "dungeon", &[spawn], &warden(), 1);
    let mut squad = Vec::new();
    for (i, name) in SQUAD.iter().enumerate() {
        let spec = CompanionSpec {
            name: format!("{name}-{i}"),
            build: pack.build(name).unwrap().clone(),
            recruit: true,
            hire: None,
        };
        squad.push(director.add_companion(&mut zone, &world, b, spec).unwrap());
    }
    let party = |zone: &Zone, id| zone.player(id).unwrap().party;
    assert!(
        squad.iter().all(|&c| party(&zone, c) == b),
        "its own at first"
    );
    // It joins the other's party: the squad with it. And back.
    director.set_party(&mut zone, b, 0x8000_0001);
    director.set_party(&mut zone, a, 0x8000_0001);
    assert_eq!(party(&zone, b), 0x8000_0001);
    assert!(squad.iter().all(|&c| party(&zone, c) == 0x8000_0001));
    director.set_party(&mut zone, b, b);
    assert!(squad.iter().all(|&c| party(&zone, c) == b));
    assert_eq!(party(&zone, a), 0x8000_0001);
    // Nobody has fought: nobody is engaged.
    assert!(!director.engaged(a) && !director.engaged(b));
}
