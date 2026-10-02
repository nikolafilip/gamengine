//! Phase 7 acceptance, step 1 (COMPANIONS.md 14): on the real tutorial dungeon, a lone
//! player with three companions clears the gate and the Warden, without a network. The same
//! run goes over the protocol in `gm-server/tests/dungeon.rs`.

mod common;

use std::path::Path;

use common::{MAPS, Outcome, Setup, play, report};
use glam::Vec3;
use gm_ai::director::CreatureSpawn;
use gm_bsp::Bsp;
use gm_core::build::ContentPack;

const REFERENCE: [&str; 3] = ["ironclad", "mender", "frostweaver"];

fn dungeon() -> (Bsp, Vec3, f32, Vec<CreatureSpawn>) {
    let bsp = Bsp::load(&Path::new(MAPS).join("dungeon.bsp")).expect("dungeon built");
    let start = bsp
        .entities
        .iter()
        .find(|e| e.classname() == "info_player_start")
        .expect("a start");
    let (spawn, yaw) = (start.origin().unwrap(), start.f32("angle").unwrap_or(0.0));
    let posts = bsp
        .creature_posts()
        .into_iter()
        .map(|p| CreatureSpawn {
            creature: p.creature,
            encounter: p.encounter,
            origin: p.origin,
            yaw: p.yaw,
        })
        .collect();
    (bsp, spawn, yaw, posts)
}

fn run(squad: &[&str], commander: &str, fights: bool, seed: u64, max_secs: u32) -> Outcome {
    let split = std::env::var("GM_FOCUS").is_err();
    run_tuned(squad, commander, fights, seed, max_secs, None, split)
}

fn run_tuned(
    squad: &[&str],
    commander: &str,
    fights: bool,
    seed: u64,
    max_secs: u32,
    tune: Option<&dyn Fn(&mut ContentPack)>,
    split: bool,
) -> Outcome {
    let (bsp, spawn, yaw, posts) = dungeon();
    play(&Setup {
        world: &bsp,
        map: "dungeon",
        spawn,
        yaw,
        posts,
        squad,
        commander,
        fights,
        seed,
        max_secs,
        boss_health: None,
        tune,
        split,
        verbose: std::env::var("GM_VERBOSE").is_ok(),
    })
}

/// Tuning aid, not a gate: how the gate goes for a range of sentinel health and both of
/// the leader's tactics. `cargo test -p gm-ai --release --test dungeon -- --ignored --nocapture`.
#[test]
#[ignore]
fn gate_tuning_matrix() {
    for health in [420u16, 360, 300, 240] {
        for split in [false, true] {
            let tune = move |pack: &mut ContentPack| {
                for c in pack.creatures.iter_mut().filter(|c| c.key == "sentinel") {
                    c.health = health;
                }
            };
            let (mut clean, mut deaths, mut resets, mut secs) = (0, 0, 0, 0);
            let seeds = 24;
            for seed in 1..=seeds {
                let o = run_tuned(&REFERENCE, "blade", false, seed, 150, Some(&tune), split);
                // The gate alone: what happened before the Warden was engaged.
                let gate = o.cleared_in("gate");
                if gate.is_some() && o.resets == 0 {
                    clean += 1;
                }
                resets += o.resets;
                deaths += o.party_deaths;
                secs += gate.unwrap_or(0);
            }
            println!(
                "sentinel {health} split {split}: {clean}/{seeds} without a reset, {resets} resets, {deaths} deaths, mean clear {:.1} s",
                secs as f32 / seeds as f32
            );
        }
    }
}

#[test]
fn a_lone_player_with_three_companions_clears_the_dungeon() {
    let seeds: Vec<u64> = match std::env::var("GM_SEED").ok().and_then(|v| v.parse().ok()) {
        Some(seed) => vec![seed],
        None => (1..=8).collect(),
    };
    let mut cleared = 0;
    let mut slowest = 0;
    for &seed in &seeds {
        let o = run(&REFERENCE, "blade", false, seed, 600);
        report(&format!("dungeon seed {seed}"), &o);
        // The gate always falls; it gives nothing.
        assert!(
            o.cleared_in("gate").is_some(),
            "seed {seed}: the gate stands"
        );
        let Some(secs) = o.cleared_in("warden") else {
            continue;
        };
        if !o.done {
            continue;
        }
        cleared += 1;
        slowest = slowest.max(secs);
        // The kill: three components from the standard list (a party with companions never
        // draws from the top list) and the coin, all to the one human.
        assert_eq!(o.loot.len(), 1, "seed {seed}: one recipient");
        let (_, items, coin) = &o.loot[0];
        assert_eq!(
            items,
            &["core/iron", "frame/ash", "catalyst/basalt"],
            "seed {seed}"
        );
        assert_eq!(*coin, 30, "seed {seed}");
        // A leader who stayed back and commanded passes the leader's trial and no other.
        if o.party_deaths <= 1 && secs <= 300 {
            assert!(o.passed("warden_leader"), "seed {seed}: {:?}", o.trials);
        }
        for other in ["warden_vanguard", "warden_striker", "warden_mender"] {
            assert!(!o.passed(other), "seed {seed}: {other} for a leader");
        }
        // The squad's discipline: next to nothing lands on its own.
        assert!(
            o.friendly_hits <= 12,
            "seed {seed}: {} friendly hits",
            o.friendly_hits
        );
        #[cfg(not(debug_assertions))]
        assert!(
            o.minds_us / (o.minds as f64) < 25.0,
            "seed {seed}: {:.1} us per mind per tick",
            o.minds_us / o.minds as f64
        );
    }
    println!(
        "cleared on {cleared} of {} seeds, slowest Warden kill {slowest} s",
        seeds.len()
    );
    // The acceptance (PLAN.md 11.8 Phase 7): the reference squad clears it, reliably.
    assert!(
        cleared * 4 >= seeds.len() * 3,
        "cleared on {cleared} of {} seeds",
        seeds.len()
    );
    assert!(
        (45..=300).contains(&slowest),
        "the slowest kill took {slowest} s"
    );
}

#[test]
fn roles_matter() {
    // Without a healer, or with three of the same, the Warden is not beaten in ten minutes
    // (COMPANIONS.md 14: a squad is a tank, a healer and damage, or it is a wipe).
    for (label, squad) in [
        ("no healer", ["ironclad", "blade", "frostweaver"]),
        ("three blades", ["blade", "blade", "blade"]),
    ] {
        let mut kills = 0;
        for seed in 1..=3 {
            let o = run(&squad, "blade", false, seed, 600);
            report(&format!("{label} seed {seed}"), &o);
            if o.cleared_in("warden").is_some() {
                kills += 1;
            }
        }
        assert_eq!(kills, 0, "{label} beat the Warden {kills} times of 3");
    }
}
