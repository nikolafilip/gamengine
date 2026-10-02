//! Minds against minds on a bare floor (COMPANIONS.md 4, 8–11): a commander with a squad
//! against the Warden alone, on open ground. The dungeon test repeats it on the real map;
//! this one keeps the fight itself measurable without walls in the way.

mod common;

use common::{Outcome, Setup, play, report};
use glam::Vec3;
use gm_ai::director::CreatureSpawn;
use gm_core::collide::BoxWorld;

/// A hall 2,400 by 1,600 with walls, the party at the west end, the Warden at the east.
fn hall() -> BoxWorld {
    let mut w = BoxWorld::floor();
    w.push(
        Vec3::new(-1216.0, -816.0, 0.0),
        Vec3::new(-1200.0, 816.0, 300.0),
    );
    w.push(
        Vec3::new(1200.0, -816.0, 0.0),
        Vec3::new(1216.0, 816.0, 300.0),
    );
    w.push(
        Vec3::new(-1216.0, -816.0, 0.0),
        Vec3::new(1216.0, -800.0, 300.0),
    );
    w.push(
        Vec3::new(-1216.0, 800.0, 0.0),
        Vec3::new(1216.0, 816.0, 300.0),
    );
    w
}

fn run(squad: &[&str], commander: &str, fights: bool, seed: u64) -> Outcome {
    let world = hall();
    play(&Setup {
        world: &world,
        map: "dungeon",
        spawn: Vec3::new(-900.0, 0.0, 24.0),
        yaw: 0.0,
        posts: vec![CreatureSpawn {
            creature: "warden".into(),
            encounter: "warden".into(),
            origin: Vec3::new(700.0, 0.0, 24.0),
            yaw: 180.0,
        }],
        squad,
        commander,
        fights,
        seed,
        max_secs: 300,
        boss_health: std::env::var("GM_BOSS_HEALTH")
            .ok()
            .and_then(|v| v.parse().ok()),
        tune: None,
        split: true,
        verbose: std::env::var("GM_VERBOSE").is_ok(),
        ally: None,
    })
}

const REFERENCE: [&str; 3] = ["ironclad", "mender", "frostweaver"];

#[test]
fn the_reference_squad_kills_the_warden_on_open_ground() {
    let seeds: Vec<u64> = match std::env::var("GM_SEED").ok().and_then(|v| v.parse().ok()) {
        Some(seed) => vec![seed],
        None => (1..=6).collect(),
    };
    for &seed in &seeds {
        let o = run(&REFERENCE, "blade", false, seed);
        report(&format!("reference seed {seed}"), &o);
        let secs = o
            .cleared_in("warden")
            .unwrap_or_else(|| panic!("seed {seed}: the Warden has {} health", o.boss_health));
        assert!((45..=300).contains(&secs), "seed {seed}: {secs} s");
        assert!(
            o.party_deaths <= 1,
            "seed {seed}: {} deaths",
            o.party_deaths
        );
        assert!(o.passed("warden_leader"), "seed {seed}: {:?}", o.trials);
        assert_eq!(o.orders, 1, "seed {seed}: one order was enough");
    }
}

#[test]
fn other_squads_and_other_leaders() {
    // A leader who fights beside the squad, and a squad around an infiltrator: both clear.
    for (label, squad, commander, fights) in [
        ("fighting leader", REFERENCE, "blade", true),
        ("shades", ["ironclad", "mender", "shade"], "shade", false),
    ] {
        let mut kills = 0;
        for seed in 1..=3 {
            let o = run(&squad, commander, fights, seed);
            report(&format!("{label} seed {seed}"), &o);
            if o.cleared_in("warden").is_some() {
                kills += 1;
            }
        }
        assert!(kills >= 2, "{label}: {kills} kills of 3");
    }
    // No tank and no healer: no kill.
    for (label, squad) in [
        ("no healer", ["ironclad", "blade", "frostweaver"]),
        ("three blades", ["blade", "blade", "blade"]),
    ] {
        for seed in 1..=3 {
            let o = run(&squad, "blade", false, seed);
            report(&format!("{label} seed {seed}"), &o);
            assert!(o.cleared_in("warden").is_none(), "{label} seed {seed}");
        }
    }
}
