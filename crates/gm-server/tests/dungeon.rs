//! Phase 7 acceptance, step 2 (PLAN.md 11.8, COMPANIONS.md 14): the tutorial dungeon over the
//! real protocol. One raid-leader bot at 150 ms round trip and 3% loss joins a wild zone that
//! lends it three recruits, and clears the gate and the Warden by orders it sends from the
//! command stance; everything it knows comes from its snapshots and control messages.

mod common;

use std::time::Duration;

use common::{DUNGEON, Match, net_budget, play, summarize};
use gm_bot::Behaviour;

fn raid(seed: u64, recruits: &[&str], seconds: u64) -> common::Outcome {
    play(Match {
        map: DUNGEON,
        bots: 1,
        seconds,
        loss: net_budget("test_loss"),
        one_way_ms: (65, 85),
        behaviours: vec![Behaviour::Raid],
        seed,
        builds: vec![Some("blade".into())],
        teams: Vec::new(),
        counter_pick_teams: Vec::new(),
        report_every: Duration::from_secs(5),
        squads: true,
        recruits: recruits.iter().map(|r| r.to_string()).collect(),
    })
}

#[test]
fn a_lone_player_with_three_companions_clears_the_dungeon_over_the_protocol() {
    // A gate that wipes the squad once or twice costs half a minute each time: the run has
    // time for that (24 runs of 24 finished inside 230 s when this was written).
    let o = raid(3, &["ironclad", "mender", "frostweaver"], 420);
    println!("{}", summarize(&o));
    assert_eq!(o.bots.len(), 1, "the bot finished");
    let b = &o.bots[0];
    println!(
        "raid: done {} in {:.1} s, squad {}, orders {} (refused {}), cleared {:?}, resets {}, deaths {}, loot {:?}, coin {}, trials {:?}, creatures {} (health seen {})",
        b.raid_done,
        b.secs,
        b.squad_max,
        b.orders,
        b.orders_refused,
        b.cleared,
        b.resets,
        b.own_deaths,
        b.loot,
        b.coin,
        b.trials,
        b.creatures_announced,
        b.creature_health_seen
    );
    let minds = o.windows.iter().map(|w| w.minds).max().unwrap_or(0);
    let minds_us = o
        .windows
        .iter()
        .map(|w| w.minds_us_mean)
        .fold(0.0, f64::max);
    println!(
        "server: encounters engaged {} reset {} cleared {} (last in {} s), loot items {}, trials passed {}, orders {} (refused {}), minds {minds} at most {minds_us:.1} us/tick",
        o.server.encounters_engaged,
        o.server.encounters_reset,
        o.server.encounters_cleared,
        o.server.last_clear_secs,
        o.server.loot_items,
        o.server.trials_passed,
        o.server.orders,
        o.server.orders_refused
    );

    // The squad: three recruits, told to the commander; the map's creatures, announced
    // with their kind, their health on the wire.
    assert_eq!(b.team, 1, "a wild zone puts every human on team 1");
    assert_eq!(b.squad_max, 3);
    assert_eq!(b.squad_hired, 0, "recruits are lent, not hired");
    assert_eq!(b.creatures_announced, 3);
    assert!(b.creature_health_seen >= 1);
    assert_eq!(minds, 6, "three creatures and three companions");

    // The clear: both encounters, the Warden inside the trial's window.
    assert!(b.raid_done, "the raid was not finished: {:?}", b.cleared);
    let secs_of = |name: &str| b.cleared.iter().find(|c| c.0 == name).map(|c| c.1);
    assert!(secs_of("gate").is_some(), "{:?}", b.cleared);
    let warden = secs_of("warden").expect("the Warden fell");
    assert!((45..=300).contains(&warden), "the Warden took {warden} s");
    assert_eq!(o.server.encounters_cleared, 2);
    assert_eq!(
        o.server.encounters_engaged,
        o.server.encounters_reset + 2,
        "every engagement ended in a reset or a clear"
    );

    // Orders went over the wire from the stance and were taken. One that names a body
    // which died while the order was on its way is refused, and the bot is told.
    assert!(b.orders >= 2, "{} orders", b.orders);
    assert!(
        b.orders_refused * 2 <= b.orders,
        "{} of {} orders refused",
        b.orders_refused,
        b.orders
    );
    assert_eq!(o.server.orders_refused, b.orders_refused as u64);
    assert_eq!(o.server.orders + o.server.orders_refused, b.orders as u64);

    // The kill: the standard list and the coin to the one human; the leader's trial.
    assert_eq!(b.loot, ["core/iron", "frame/ash", "catalyst/basalt"]);
    assert_eq!(b.coin, 30);
    assert_eq!(o.server.loot_items, 3);
    let passed: Vec<&str> = b
        .trials
        .iter()
        .filter(|t| t.1)
        .map(|t| t.0.as_str())
        .collect();
    assert_eq!(passed, ["warden_leader"], "{:?}", b.trials);
    assert_eq!(
        b.trials.len(),
        4,
        "every trial of the encounter gave a verdict"
    );
    assert_eq!(o.server.trials_passed, 1);

    // The wire: inside the per-player budget with squad sight and health on it, and the
    // command stance predicted (corrections with no visible cause stay rare).
    let budget = net_budget("max_bytes_per_player_s");
    assert!(
        b.rx_bytes_per_s() < budget,
        "{} B/s down",
        b.rx_bytes_per_s()
    );
    assert!(b.tx_bytes_per_s() < budget, "{} B/s up", b.tx_bytes_per_s());
    assert_eq!(b.client.decode_errors, 0);
    assert_eq!(b.client.unknown_baseline, 0);
    let allowed = (b.secs / 10.0).ceil() as u64;
    assert!(
        b.client.corrections_unexplained <= allowed,
        "{} unexplained corrections in {:.0} s",
        b.client.corrections_unexplained,
        b.secs
    );
}

#[test]
fn a_zone_without_recruits_gives_a_lone_player_no_squad() {
    // Squads allowed, nobody lent and nobody hired: the leader stands alone at the gate.
    let o = raid(4, &[], 40);
    assert_eq!(o.bots.len(), 1);
    let b = &o.bots[0];
    assert_eq!(b.squad_max, 0);
    assert!(!b.raid_done);
    assert_eq!(o.server.encounters_cleared, 0);
    assert_eq!(o.server.trials_passed, 0);
    assert_eq!(o.windows.iter().map(|w| w.minds).max(), Some(3));
}
