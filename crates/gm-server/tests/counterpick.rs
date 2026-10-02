//! Phase 3 acceptance over the real protocol (PLAN.md 11.8, MATRIX.md 11): eight ironclads
//! against eight blades in the arena at 150 ms round trip and 3% loss. The blades
//! counter-pick: after ten seconds they look at the enemy's aspects and ask the zone for the
//! preset that beats them (`Control::Respec`, applied at their next respawn). The kill share
//! must flip from the first half of the match to the second.

mod common;

use std::time::Duration;

use common::{ARENA, Match, play, summarize};
use gm_bot::Behaviour;

#[test]
fn blades_counter_pick_into_frostweavers_and_turn_the_match() {
    // Ninety seconds: at sixty the kill share after the counter-pick rested on some forty
    // kills and came within 0.05 of the bar in one run of a dozen (0.65–0.89, the same on
    // the Phase 6 build), and under it about once in forty.
    let secs = 90;
    let o = play(Match {
        map: ARENA,
        bots: 16,
        seconds: secs,
        loss: common::net_budget("test_loss"),
        one_way_ms: (65, 85),
        behaviours: vec![Behaviour::Duelist],
        seed: 21,
        builds: vec![Some("ironclad".into()), Some("blade".into())],
        teams: vec![1, 2],
        counter_pick_teams: vec![2],
        report_every: Duration::from_secs(5),
        squads: false,
        recruits: Vec::new(),
    });
    let text = summarize(&o);
    println!("{text}");
    for w in &o.windows {
        println!(
            "window tick {} team_kills {:?} tx {:.0} rx {:.0}",
            w.tick, w.team_kills, w.tx_bytes_per_player_s, w.rx_bytes_per_player_s
        );
    }
    assert_eq!(o.bots.len(), 16);
    // Every blade asked for a re-spec; a build applies at the next respawn (MATRIX.md 9), so a
    // blade that never died again in the last eighty seconds legitimately stays a blade. Most
    // must have turned.
    let blades: Vec<_> = o.bots.iter().filter(|b| b.team == 2).collect();
    assert_eq!(blades.len(), 8);
    for b in &blades {
        assert!(b.respecs >= 1, "{} never counter-picked", b.name);
        assert!(
            b.final_build == "frostweaver" || b.final_build == "blade",
            "{}: {}",
            b.name,
            b.final_build
        );
    }
    let turned = blades
        .iter()
        .filter(|b| b.final_build == "frostweaver")
        .count();
    assert!(turned >= 6, "only {turned} of 8 blades became frostweavers");
    for b in o.bots.iter().filter(|b| b.team == 1) {
        assert_eq!(b.final_build, "ironclad", "{}", b.name);
    }
    // Kill share: team 2's share of the kills in the first 20 s versus the last 30 s.
    let at = |t: u64| -> [u64; 3] {
        o.windows
            .iter()
            .rev()
            .find(|w| w.tick <= t)
            .map_or([0; 3], |w| w.team_kills)
    };
    let early = at(20 * 64);
    let final_kills = o.server.team_kills;
    let late = [
        final_kills[0] - early[0],
        final_kills[1] - early[1],
        final_kills[2] - early[2],
    ];
    let share = |k: [u64; 3]| k[2] as f64 / (k[1] + k[2]).max(1) as f64;
    println!(
        "team 2 kill share: first 20 s {:.2} ({:?}), after {:.2} ({:?})",
        share(early),
        early,
        share(late),
        late
    );
    assert!(early[1] + early[2] >= 6, "too few early kills: {early:?}");
    assert!(late[1] + late[2] >= 10, "too few late kills: {late:?}");
    // Before the counter-pick the first twenty seconds hold about ten kills: too few to
    // put a bar on by themselves (their share ran from 0.00 to 0.62 over forty runs; that
    // ironclads beat blades is the offline matrix gate's to prove, on three seeds and 115
    // kills). What this test proves is the turn: the share rises, and ends well over half.
    assert!(
        share(late) > share(early),
        "the counter-pick should turn the match: {early:?} then {late:?}"
    );
    assert!(
        share(late) > 0.6,
        "frostweavers should dominate after the counter-pick: {late:?}"
    );
    // The richer kits must still fit the budget.
    let budget = common::net_budget("max_bytes_per_player_s");
    assert!(o.server.max_tx_bytes_per_player_s < budget, "{text}");
    assert!(o.server.max_rx_bytes_per_player_s < budget, "{text}");
    for b in &o.bots {
        assert_eq!(b.client.decode_errors, 0, "{}", b.name);
        assert!(
            b.client.corrections_unexplained <= 6,
            "{}: {} unexplained corrections",
            b.name,
            b.client.corrections_unexplained
        );
    }
}
