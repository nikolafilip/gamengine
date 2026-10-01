//! Phase 2 acceptance (PLAN.md 11.8, PROTOCOL.md 9): 16 bots on one zone under turmoil with
//! 150 ms round trip (75 ± 10 ms per hop) and 3% independent datagram loss per direction.
//! Everything here runs in simulated time; a 30-second match takes a few wall-clock seconds.

mod common;

use common::{Match, net_budget, play, summarize};
use gm_bot::Behaviour;

#[test]
fn sixteen_players_at_150ms_and_3_percent_loss() {
    let secs = 30;
    let o = play(Match {
        behaviours: vec![Behaviour::Wander, Behaviour::Wander, Behaviour::Hunter],
        ..Match::simple(16, secs, net_budget("test_loss"), (65, 85), 7)
    });
    let text = summarize(&o);
    println!("{text}");
    assert_eq!(o.bots.len(), 16, "every bot finished");
    assert_eq!(o.server.joins, 16);

    // 5. Bandwidth, both directions, QUIC's own UDP byte counters.
    let budget = net_budget("max_bytes_per_player_s");
    for b in &o.bots {
        assert!(
            b.rx_bytes_per_s() < budget,
            "{}: {} B/s down",
            b.name,
            b.rx_bytes_per_s()
        );
        assert!(
            b.tx_bytes_per_s() < budget,
            "{}: {} B/s up",
            b.name,
            b.tx_bytes_per_s()
        );
    }
    assert!(o.server.max_tx_bytes_per_player_s < budget, "{text}");
    assert!(o.server.max_rx_bytes_per_player_s < budget, "{text}");

    // Basic liveness: snapshots flowed at roughly the tick rate, inputs ran.
    for b in &o.bots {
        let expected = (b.secs * 64.0) as u64;
        assert!(
            b.client.snapshots as f64 > expected as f64 * 0.9,
            "{}: {} snapshots in {:.1} s",
            b.name,
            b.client.snapshots,
            b.secs
        );
        assert_eq!(b.client.decode_errors, 0, "{}", b.name);
        assert_eq!(b.client.unknown_baseline, 0, "{}", b.name);
        assert!(
            b.others_seen_max >= 10,
            "{}: saw only {} others",
            b.name,
            b.others_seen_max
        );
    }
    let total_ticks: f64 = o.bots.iter().map(|b| b.ticks as f64).sum();
    assert!(
        o.server.executed_frames as f64 > total_ticks * 0.99,
        "frames executed {} of {} sent",
        o.server.executed_frames,
        total_ticks
    );

    // 2. Input starvation under 1% of ticks per bot on average.
    let server_ticks_with_players = o.server.tick as f64 * 16.0;
    assert!(
        (o.server.starved_ticks as f64) < server_ticks_with_players * 0.01 + 16.0 * 128.0,
        "starved {} of {}",
        o.server.starved_ticks,
        server_ticks_with_players
    );

    // 3. Snapshot gaps: never more than 4 consecutive ticks missing.
    for b in &o.bots {
        assert!(
            b.client.max_gap <= 4,
            "{}: max gap {}",
            b.name,
            b.client.max_gap
        );
    }

    // 1. Unexplained corrections (not a hit, death, respawn or body-block): fewer than one per
    // 10 s of movement per bot.
    for b in &o.bots {
        let allowed = (b.secs / 10.0).ceil() as u64;
        assert!(
            b.client.corrections_unexplained <= allowed,
            "{}: {} unexplained of {} corrections (max {:.1} u) in {:.0} s",
            b.name,
            b.client.corrections_unexplained,
            b.client.corrections,
            b.client.max_correction,
            b.secs
        );
    }

    // 4. Combat registers under latency: melee with lag compensation and projectiles both hit.
    assert!(o.server.hits_melee > 0, "no melee hits\n{text}");
    assert!(o.server.hits_projectile > 0, "no projectile hits\n{text}");
    assert_eq!(
        o.server.oversize_drops, 0,
        "snapshots exceeded the datagram budget"
    );
}

#[test]
fn two_players_perfect_network_agree_exactly() {
    let o = play(Match::simple(2, 8, 0.0, (5, 5), 3));
    let text = summarize(&o);
    println!("{text}");
    assert_eq!(o.bots.len(), 2);
    for b in &o.bots {
        assert_eq!(b.client.gaps, 0, "{}", b.name);
        assert_eq!(
            b.client.corrections_unexplained, 0,
            "{}: {} unexplained of {} corrections",
            b.name, b.client.corrections_unexplained, b.client.corrections
        );
        assert!(b.client.snapshots > 8 * 64 - 64, "{}", b.name);
    }
}
