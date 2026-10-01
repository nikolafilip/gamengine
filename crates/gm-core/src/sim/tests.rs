use glam::Vec3;

use super::*;
use crate::build::Build;
use crate::collide::BoxWorld;
use crate::matrix::{ArmourClass, Aspects, Attributes, Element};
use crate::sim::test_content::{self, phase2_build};
use crate::tick::TickRate;
use crate::vocab::{ArchetypeFrame, EntityId, Status};

const REST_Z: f32 = 24.0;
const RATE: TickRate = TickRate::COMBAT;

fn zone_with(spawns: Vec<(Vec3, f32)>) -> Zone {
    let spawns = spawns
        .into_iter()
        .map(|(origin, yaw)| Spawn {
            origin,
            yaw,
            team: 0,
        })
        .collect();
    Zone::new(RATE, 7, spawns, test_content::pack(RATE))
}

/// Phase 2 characters placed exactly at the listed `(origin, yaw)` pairs, in order.
fn arena(placements: &[(Vec3, f32)]) -> (BoxWorld, Zone, Vec<EntityId>) {
    let world = BoxWorld::floor();
    let mut zone = zone_with(placements.to_vec());
    let build = phase2_build(&zone.content);
    let ids = placements
        .iter()
        .map(|&(o, y)| zone.add_player_at(build.clone(), 0, o, y))
        .collect();
    (world, zone, ids)
}

/// Preset builds placed at the listed `(name, origin, yaw)` triples.
fn arena_builds(placements: &[(&str, Vec3, f32)]) -> (BoxWorld, Zone, Vec<EntityId>) {
    let world = BoxWorld::floor();
    let mut zone = zone_with(placements.iter().map(|p| (p.1, p.2)).collect());
    let ids = placements
        .iter()
        .map(|&(name, o, y)| {
            let build = zone.content.build(name).expect(name).clone();
            zone.add_player_at(build, 0, o, y)
        })
        .collect();
    (world, zone, ids)
}

fn input(yaw: f32, forward: f32, buttons: u16) -> Input {
    Input {
        buttons,
        yaw,
        pitch: 0.0,
        forward,
        side: 0.0,
        ability: 0,
    }
}

fn active(yaw: f32, forward: f32, slot: u8) -> Input {
    Input {
        ability: slot,
        ..input(yaw, forward, 0)
    }
}

/// Feed every player one new frame per tick and step. The frame tick continues after the
/// frames still queued (the server keeps one in reserve, so execution runs a tick behind).
fn tick(zone: &mut Zone, world: &BoxWorld, inputs: &[(EntityId, Input)], view: Tick) {
    for (id, inp) in inputs {
        let t = zone
            .player(*id)
            .map_or(0, |p| p.last_input_tick + p.queued_frames() as u32)
            + 1;
        zone.queue_input(*id, t, *inp, view);
    }
    zone.step(world);
}

fn run(zone: &mut Zone, world: &BoxWorld, inputs: &[(EntityId, Input)], ticks: usize) {
    for _ in 0..ticks {
        tick(zone, world, inputs, 0);
    }
}

fn hits(zone: &Zone, kind: HitKind) -> usize {
    zone.events
        .iter()
        .filter(|e| matches!(e, ZoneEvent::Hit { kind: k, .. } if *k == kind))
        .count()
}

#[test]
fn phase2_kit_validates_and_has_durations() {
    let sheet = test_content::phase2_sheet(RATE);
    let kit = &sheet.kit;
    assert_eq!(kit.abilities.len(), 3);
    let (sword, crossbow, dash) = (
        kit.primary.unwrap() as usize,
        kit.secondary.unwrap() as usize,
        kit.actives[0].unwrap() as usize,
    );
    assert_eq!(kit.durations[sword], 6 + 3 + 11);
    assert_eq!(kit.durations[dash], 10);
    assert_eq!(kit.durations[crossbow], 8);
    assert_eq!(sheet.derived.health, 110);
    assert_eq!(sheet.derived.stamina, 100.0);
}

#[test]
fn sword_hits_in_front_and_not_behind() {
    for (attacker_yaw, expect_hit) in [(0.0f32, true), (180.0f32, false)] {
        let (world, mut zone, ids) = arena(&[
            (Vec3::new(0.0, 0.0, REST_Z), 0.0),
            (Vec3::new(48.0, 0.0, REST_Z), 180.0),
        ]);
        let (a, b) = (ids[0], ids[1]);
        run(
            &mut zone,
            &world,
            &[
                (a, input(attacker_yaw, 0.0, buttons::PRIMARY)),
                (b, input(180.0, 0.0, 0)),
            ],
            13,
        );
        let hp = zone.player(b).unwrap().health;
        if expect_hit {
            // 35 slash x 1.0 (STR 10) x 1.25 (cloth) x 0.85 (armour) = 37.
            assert_eq!(hp, 110 - 37, "yaw {attacker_yaw}");
            assert!(zone.events.iter().any(|e| matches!(
                e,
                ZoneEvent::Hit { attacker, target, amount: 37, kind: HitKind::Melee } if *attacker == a && *target == b
            )));
        } else {
            assert_eq!(hp, 110, "yaw {attacker_yaw}");
        }
        // Holding the button does not re-trigger; one swing per press.
        assert_eq!(hits(&zone, HitKind::Melee), expect_hit as usize);
    }
}

#[test]
fn crossbow_bolt_hits_the_first_body_in_line_even_an_ally() {
    let (world, mut zone, ids) = arena(&[
        (Vec3::new(0.0, 0.0, REST_Z), 0.0),
        (Vec3::new(120.0, 0.0, REST_Z), 0.0),
        (Vec3::new(240.0, 0.0, REST_Z), 0.0),
    ]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let idle = input(0.0, 0.0, 0);
    tick(
        &mut zone,
        &world,
        &[
            (a, input(0.0, 0.0, buttons::SECONDARY)),
            (b, idle),
            (c, idle),
        ],
        0,
    );
    run(&mut zone, &world, &[(a, idle), (b, idle), (c, idle)], 40);
    // 40 pierce x 1.0 x 1.0 (cloth) x 0.85 = 34.
    assert_eq!(
        zone.player(b).unwrap().health,
        110 - 34,
        "the body in between takes the bolt"
    );
    assert_eq!(zone.player(c).unwrap().health, 110);
    assert!(
        zone.events
            .iter()
            .any(|e| matches!(e, ZoneEvent::ProjectileSpawned { owner, .. } if *owner == a))
    );
    assert!(
        zone.events
            .iter()
            .any(|e| matches!(e, ZoneEvent::ProjectileRemoved(_)))
    );
    assert!(zone.projectiles().is_empty());
}

#[test]
fn dash_moves_fast_costs_stamina_and_pauses_regen() {
    let (world, mut zone, ids) = arena(&[(Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
    let a = ids[0];
    tick(
        &mut zone,
        &world,
        &[(a, input(0.0, 1.0, buttons::ABILITY1))],
        0,
    );
    // The press runs one tick late (reserve); ten dash frames follow.
    run(&mut zone, &world, &[(a, input(0.0, 1.0, 0))], 10);
    let p = zone.player(a).unwrap();
    // 900 u/s for 10 ticks = 140.6 u, minus the epsilon pull-backs.
    assert!(
        p.mover.mv.origin.x > 130.0 && p.mover.mv.origin.x < 142.0,
        "{:?}",
        p.mover.mv.origin
    );
    assert_eq!(
        p.mover.stamina, 70.0,
        "regen pauses for a second after a spend"
    );
    assert_eq!(p.anim, anim::DASH);
    assert!(p.mover.evading(p.last_input_tick));
    run(&mut zone, &world, &[(a, input(0.0, 1.0, 0))], 2);
    assert!(zone.player(a).unwrap().mover.dash.is_none());
    run(&mut zone, &world, &[(a, input(0.0, 1.0, 0))], 64);
    let p = zone.player(a).unwrap();
    assert!(p.mover.stamina > 70.5, "regen resumed: {}", p.mover.stamina);
    assert!(!p.mover.evading(p.last_input_tick));
}

#[test]
fn lag_compensation_rewinds_the_target() {
    for (rewind, expect_hit) in [(true, true), (false, false)] {
        let (world, mut zone, ids) = arena(&[
            (Vec3::new(0.0, 0.0, REST_Z), 0.0),
            (Vec3::new(40.0, 0.0, REST_Z), 0.0),
        ]);
        let (a, b) = (ids[0], ids[1]);
        run(
            &mut zone,
            &world,
            &[(a, input(0.0, 0.0, 0)), (b, input(0.0, 1.0, 0))],
            10,
        );
        let then = zone.tick;
        let old_x = zone.player(b).unwrap().mover.mv.origin.x;
        for _ in 0..MAX_REWIND_TICKS {
            tick(
                &mut zone,
                &world,
                &[(a, input(0.0, 0.0, 0)), (b, input(0.0, 1.0, 0))],
                0,
            );
        }
        let now_x = zone.player(b).unwrap().mover.mv.origin.x;
        assert!(old_x - 14.0 <= 72.0, "old position in reach: {old_x}");
        assert!(
            now_x - 14.0 > 72.0 + 20.0,
            "new position out of reach: {now_x}"
        );
        let view = if rewind { then } else { 0 };
        for _ in 0..12 {
            tick(
                &mut zone,
                &world,
                &[
                    (a, input(0.0, 0.0, buttons::PRIMARY)),
                    (b, input(0.0, 1.0, 0)),
                ],
                view,
            );
        }
        let hit = zone.player(b).unwrap().health < 110;
        assert_eq!(hit, expect_hit, "rewind {rewind}: old {old_x} now {now_x}");
    }
}

#[test]
fn frames_run_exactly_once_and_are_rate_limited() {
    let (world, mut zone, ids) = arena(&[(Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
    let a = ids[0];
    for t in 1..=20 {
        zone.queue_input(a, t, input(0.0, 1.0, 0), 0);
    }
    zone.queue_input(a, 20, input(0.0, 1.0, 0), 0);
    zone.queue_input(a, 3, input(0.0, 1.0, 0), 0);
    assert_eq!(zone.player(a).unwrap().queued_frames(), 20);
    let mut per_tick = Vec::new();
    for _ in 0..8 {
        let before = zone.player(a).unwrap().executed_frames;
        zone.step(&world);
        per_tick.push(zone.player(a).unwrap().executed_frames - before);
    }
    assert_eq!(per_tick, [2, 2, 2, 2, 2, 2, 2, 1]);
    assert_eq!(zone.player(a).unwrap().last_input_tick, 15);
    for _ in 0..8 {
        zone.step(&world);
    }
    let p = zone.player(a).unwrap();
    assert_eq!(p.executed_frames, 19);
    assert_eq!(p.queued_frames(), 1);
    assert_eq!(p.starved_ticks, 4, "reserve held for the last ticks");
    zone.queue_input(a, 21, input(0.0, 1.0, 0), 0);
    zone.step(&world);
    let p = zone.player(a).unwrap();
    assert_eq!(p.executed_frames, 20);
    assert_eq!(p.last_input_tick, 20);
    zone.step(&world);
    assert_eq!(
        zone.player(a).unwrap().executed_frames,
        20,
        "nothing invented"
    );
}

#[test]
fn reordered_datagrams_keep_every_frame() {
    let (world, mut zone, ids) = arena(&[(Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
    let a = ids[0];
    for t in 4..=7 {
        zone.queue_input(a, t, input(0.0, 1.0, 0), 0);
    }
    for t in 1..=4 {
        zone.queue_input(a, t, input(0.0, 1.0, 0), 0);
    }
    assert_eq!(zone.player(a).unwrap().queued_frames(), 7);
    for _ in 0..8 {
        zone.step(&world);
    }
    let p = zone.player(a).unwrap();
    assert_eq!(p.executed_frames, 6, "one in reserve");
    assert_eq!(p.last_input_tick, 6);
    zone.queue_input(a, 3, input(0.0, 1.0, 0), 0);
    assert_eq!(zone.player(a).unwrap().queued_frames(), 1);
}

#[test]
fn players_block_each_other() {
    let (world, mut zone, ids) = arena(&[
        (Vec3::new(-100.0, 0.0, REST_Z), 0.0),
        (Vec3::new(100.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 1.0, 0)), (b, input(180.0, 1.0, 0))],
        128,
    );
    let ax = zone.player(a).unwrap().mover.mv.origin.x;
    let bx = zone.player(b).unwrap().mover.mv.origin.x;
    assert!(ax < bx, "players passed through each other: {ax} {bx}");
    assert!(bx - ax >= 32.0 - 0.1, "hulls overlap: {ax} {bx}");
    assert!(bx - ax < 34.0, "players did not meet: {ax} {bx}");
}

#[test]
fn death_and_respawn() {
    let (world, mut zone, ids) = arena(&[
        (Vec3::new(0.0, 0.0, REST_Z), 0.0),
        (Vec3::new(48.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    let mut swings = 0;
    let mut killed_at = None;
    for t in 0..400 {
        let press = t % 24 == 0;
        if press {
            swings += 1;
        }
        let btn = if press { buttons::PRIMARY } else { 0 };
        tick(
            &mut zone,
            &world,
            &[(a, input(0.0, 1.0, btn)), (b, input(180.0, 0.0, 0))],
            0,
        );
        if killed_at.is_none()
            && zone.events.iter().any(|e| matches!(e, ZoneEvent::Killed { victim, killer } if *victim == b && *killer == a))
        {
            killed_at = Some(zone.tick);
            assert!(!zone.player(b).unwrap().alive);
            assert_eq!(zone.player(b).unwrap().anim, anim::DEAD);
            assert_eq!(zone.player(a).unwrap().kills, 1);
            break;
        }
    }
    let killed_at = killed_at.expect("three swings kill");
    assert!(swings >= 3);
    for _ in 0..zone.rate.ms_to_ticks(RESPAWN_MS) {
        tick(
            &mut zone,
            &world,
            &[(a, input(0.0, 0.0, 0)), (b, input(180.0, 0.0, 0))],
            0,
        );
    }
    let p = zone.player(b).unwrap();
    assert!(
        p.alive,
        "respawned {} ticks after {killed_at}",
        zone.tick - killed_at
    );
    assert_eq!(p.health, 110);
    assert!(
        zone.events
            .iter()
            .any(|e| matches!(e, ZoneEvent::Respawned(id) if *id == b))
    );
}

#[test]
fn history_rewinds_to_the_nearest_recorded_tick() {
    let mut h = History::default();
    for t in 1..=40u32 {
        h.record(t, vec![(1, Vec3::new(t as f32, 0.0, 0.0))]);
    }
    assert_eq!(h.origin_at(10, 1), Some(Vec3::new(10.0, 0.0, 0.0)));
    assert_eq!(h.origin_at(2, 1), Some(Vec3::new(9.0, 0.0, 0.0)));
    assert_eq!(h.origin_at(41, 1), None);
    assert_eq!(h.origin_at(10, 2), None);
}

#[test]
fn add_player_refuses_invalid_builds_and_spawns_by_team() {
    let world = BoxWorld::floor();
    let spawns = vec![
        Spawn {
            origin: Vec3::new(-500.0, 0.0, REST_Z),
            yaw: 0.0,
            team: 1,
        },
        Spawn {
            origin: Vec3::new(500.0, 0.0, REST_Z),
            yaw: 180.0,
            team: 2,
        },
    ];
    let mut zone = Zone::new(RATE, 1, spawns, test_content::pack(RATE));
    let blade = zone.content.build("blade").unwrap().clone();
    assert!(
        zone.add_player(&world, phase2_build(&zone.content), 1)
            .is_err()
    );
    let a = zone.add_player(&world, blade.clone(), 1).unwrap();
    let b = zone.add_player(&world, blade.clone(), 2).unwrap();
    let c = zone.add_player(&world, blade, 2).unwrap();
    assert!(zone.player(a).unwrap().mover.mv.origin.x < 0.0);
    assert!(zone.player(b).unwrap().mover.mv.origin.x > 0.0);
    assert!(zone.player(c).unwrap().mover.mv.origin.x > 0.0);
    assert_ne!(
        zone.player(b).unwrap().mover.mv.origin,
        zone.player(c).unwrap().mover.mv.origin,
        "second spawn is offset"
    );
    assert_eq!(zone.smallest_team(), 1);
    assert_eq!(zone.player(a).unwrap().team(), 1);
}

#[test]
fn shield_wall_blocks_from_the_front_costs_stamina_and_breaks() {
    // An ironclad facing a blade: sword from the front is blocked, from behind it is not.
    for (facing, expect_block) in [(180.0f32, true), (0.0f32, false)] {
        let (world, mut zone, ids) = arena_builds(&[
            ("blade", Vec3::new(0.0, 0.0, REST_Z), 0.0),
            ("ironclad", Vec3::new(50.0, 0.0, REST_Z), facing),
        ]);
        let (a, b) = (ids[0], ids[1]);
        let hold = input(facing, 0.0, buttons::GUARD);
        run(&mut zone, &world, &[(a, input(0.0, 0.0, 0)), (b, hold)], 3);
        assert_eq!(zone.player(b).unwrap().anim, anim::GUARD);
        let stamina_before = zone.player(b).unwrap().mover.stamina;
        run(
            &mut zone,
            &world,
            &[(a, input(0.0, 0.0, buttons::PRIMARY)), (b, hold)],
            13,
        );
        let p = zone.player(b).unwrap();
        let taken = p.max_health() - p.health;
        // Sword 35 x 1.2 (STR 20) x 0.5 (plate) x 0.7 (armour 0.30) = 14.7 -> 15 unblocked,
        // x 0.2 blocked = 2.94 -> 3.
        if expect_block {
            assert_eq!(taken, 3, "facing {facing}");
            assert_eq!(stamina_before - p.mover.stamina, 12.0);
        } else {
            assert_eq!(taken, 15, "facing {facing}");
        }
    }
    // Guard break: with less stamina than the block costs, the hit lands in full and staggers.
    let (world, mut zone, ids) = arena_builds(&[
        ("blade", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("ironclad", Vec3::new(50.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    zone.player_mut(b).unwrap().mover.stamina = 5.0;
    let hold = input(180.0, 0.0, buttons::GUARD);
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0)), (b, hold)], 3);
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, buttons::PRIMARY)), (b, hold)],
        13,
    );
    let p = zone.player(b).unwrap();
    assert_eq!(p.max_health() - p.health, 15);
    assert_eq!(p.mover.stamina, 0.0);
    assert!(p.mover.statuses.has(Status::Stagger));
    assert!(
        zone.events
            .iter()
            .any(|e| matches!(e, ZoneEvent::GuardBroken(id) if *id == b))
    );
}

#[test]
fn parry_negates_staggers_and_ripostes() {
    // Two blades: b parries a's sword inside the window.
    let (world, mut zone, ids) = arena_builds(&[
        ("blade", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("blade", Vec3::new(50.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    // a presses attack; b presses parry three ticks later so the 150 ms window covers the
    // 90 ms windup.
    tick(
        &mut zone,
        &world,
        &[
            (a, input(0.0, 0.0, buttons::PRIMARY)),
            (b, input(180.0, 0.0, 0)),
        ],
        0,
    );
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, 0)), (b, input(180.0, 0.0, 0))],
        2,
    );
    tick(
        &mut zone,
        &world,
        &[
            (a, input(0.0, 0.0, 0)),
            (b, input(180.0, 0.0, buttons::GUARD)),
        ],
        0,
    );
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, 0)), (b, input(180.0, 0.0, 0))],
        12,
    );
    assert_eq!(
        zone.player(b).unwrap().health,
        zone.player(b).unwrap().max_health()
    );
    assert!(
        zone.events
            .iter()
            .any(|e| matches!(e, ZoneEvent::Parried { defender, attacker } if *defender == b && *attacker == a))
    );
    let pa = zone.player(a).unwrap();
    assert!(pa.health < pa.max_health(), "riposte landed");
    assert!(
        zone.events
            .iter()
            .any(|e| matches!(e, ZoneEvent::StatusApplied { target, status: Status::Stagger, .. } if *target == a))
    );
    // A parry pressed with nobody attacking whiffs into recovery.
    let (world, mut zone, ids) = arena_builds(&[("blade", Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
    let a = ids[0];
    tick(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, buttons::GUARD))],
        0,
    );
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0))], 2);
    assert!(matches!(
        zone.player(a).unwrap().mover.guard,
        GuardState::Parry { .. }
    ));
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0))], 12);
    assert!(matches!(
        zone.player(a).unwrap().mover.guard,
        GuardState::Whiff { .. }
    ));
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0))], 30);
    assert_eq!(zone.player(a).unwrap().mover.guard, GuardState::None);
}

#[test]
fn stomp_pulses_once_damages_and_slows_everyone_in_range() {
    let (world, mut zone, ids) = arena_builds(&[
        ("ironclad", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("blade", Vec3::new(100.0, 0.0, REST_Z), 180.0),
        ("blade", Vec3::new(300.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let idle = input(180.0, 0.0, 0);
    // Stomp is active slot 1 of the ironclad.
    tick(
        &mut zone,
        &world,
        &[(a, active(0.0, 0.0, 1)), (b, idle), (c, idle)],
        0,
    );
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, 0)), (b, idle), (c, idle)],
        20,
    );
    let pb = zone.player(b).unwrap();
    // 35 stone x 1.16 (INT 18) x 0.5 (Flame resists? no: Stone beats Flame -> 2) ...
    // Stone into Flame is 2x, ward 0.015 * 18 = 0.27: 35 * 1.16 * 2 * 0.73 = 59.3 -> 59.
    assert_eq!(pb.max_health() - pb.health, 59);
    assert!(pb.mover.statuses.has(Status::Slow));
    assert!(
        (pb.mover.statuses.speed_scale() - 0.7).abs() < 1e-6,
        "{}",
        pb.mover.statuses.speed_scale()
    );
    assert_eq!(
        zone.player(c).unwrap().health,
        zone.player(c).unwrap().max_health()
    );
    assert_eq!(hits(&zone, HitKind::Area), 1);
    assert!(
        zone.areas().is_empty(),
        "an instant pulse is gone after one tick"
    );
    assert!(
        zone.events
            .iter()
            .any(|e| matches!(e, ZoneEvent::AreaRemoved(_)))
    );
}

#[test]
fn frost_nova_chills_twice_and_a_shard_freezes() {
    let (world, mut zone, ids) = arena_builds(&[
        ("frostweaver", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("ironclad", Vec3::new(100.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    let idle = input(180.0, 0.0, 0);
    tick(&mut zone, &world, &[(a, active(0.0, 0.0, 1)), (b, idle)], 0);
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0)), (b, idle)], 25);
    let pb = zone.player(b).unwrap();
    assert_eq!(pb.mover.statuses.stacks(Status::Chill), 2);
    assert!((pb.mover.statuses.speed_scale() - 0.7).abs() < 1e-6);
    // Frost into Stone is 2x and ignores plate: 30 * 1.2 * 2 * (1 - 0.3 ward) = 50.4 -> 50.
    assert_eq!(pb.max_health() - pb.health, 50);
    // The ice shard adds the third stack: frozen (rooted), chill cleared, immune after.
    tick(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, buttons::SECONDARY)), (b, idle)],
        0,
    );
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0)), (b, idle)], 20);
    let pb = zone.player(b).unwrap();
    assert!(pb.mover.statuses.has(Status::Root), "frozen");
    assert_eq!(pb.mover.statuses.stacks(Status::Chill), 0);
    assert_eq!(pb.mover.statuses.speed_scale(), 0.0);
    // Rooted: walking does nothing.
    let x = pb.mover.mv.origin.x;
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, 0)), (b, input(180.0, 1.0, 0))],
        10,
    );
    assert!((zone.player(b).unwrap().mover.mv.origin.x - x).abs() < 1.0);
}

#[test]
fn burn_ticks_four_times_a_second_and_kills_are_attributed() {
    let (world, mut zone, ids) = arena_builds(&[
        ("blade", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("shade", Vec3::new(200.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    let idle = input(180.0, 0.0, 0);
    tick(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, buttons::SECONDARY)), (b, idle)],
        0,
    );
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0)), (b, idle)], 30);
    let pb = zone.player(b).unwrap();
    assert!(pb.mover.statuses.has(Status::Burn));
    let after_hit = pb.health;
    assert_eq!(hits(&zone, HitKind::Projectile), 1);
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0)), (b, idle)], 64);
    let dots = hits(&zone, HitKind::Dot);
    assert!((4..=5).contains(&dots), "{dots} burn pulses in a second");
    assert!(zone.player(b).unwrap().health < after_hit);
    // Burn to death: attribution goes to the source.
    zone.player_mut(b).unwrap().health = 2;
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0)), (b, idle)], 20);
    assert!(zone.events.iter().any(
        |e| matches!(e, ZoneEvent::Killed { victim, killer } if *victim == b && *killer == a)
    ));
    assert_eq!(zone.player(a).unwrap().kills, 1);
}

#[test]
fn blink_teleports_along_the_facing_and_stops_at_walls() {
    let mut world = BoxWorld::floor();
    world.push(
        Vec3::new(150.0, -256.0, 0.0),
        Vec3::new(200.0, 256.0, 200.0),
    );
    let mut zone = zone_with(vec![(Vec3::new(0.0, 0.0, REST_Z), 0.0)]);
    let shade = zone.content.build("shade").unwrap().clone();
    let a = zone.add_player_at(shade, 0, Vec3::new(0.0, 0.0, REST_Z), 0.0);
    // Blink is active slot 1 of the shade: 256 u, blocked by the wall at x = 150.
    tick(&mut zone, &world, &[(a, active(0.0, 0.0, 1))], 0);
    run(&mut zone, &world, &[(a, input(0.0, 0.0, 0))], 2);
    let x = zone.player(a).unwrap().mover.mv.origin.x;
    assert!(x > 130.0 && x <= 134.0 + 1e-3, "x = {x}");
    let focus = zone.player(a).unwrap().mover.focus;
    assert!(focus < zone.player(a).unwrap().sheet.derived.focus - 29.0);
    // Facing away from the wall, the full distance.
    let mut zone = zone_with(vec![(Vec3::new(0.0, 0.0, REST_Z), 180.0)]);
    let shade = zone.content.build("shade").unwrap().clone();
    let a = zone.add_player_at(shade, 0, Vec3::new(0.0, 0.0, REST_Z), 180.0);
    tick(&mut zone, &world, &[(a, active(180.0, 0.0, 1))], 0);
    run(&mut zone, &world, &[(a, input(180.0, 0.0, 0))], 2);
    let x = zone.player(a).unwrap().mover.mv.origin.x;
    assert!((x + 256.0).abs() < 1.0, "x = {x}");
}

#[test]
fn hammer_staggers_at_the_threshold_then_immunity_holds() {
    // Ironclad hammer (35 stagger) into a blade (threshold 40 + 2 * 20 = 80): three hits.
    let (world, mut zone, ids) = arena_builds(&[
        ("ironclad", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("blade", Vec3::new(55.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    let idle = input(180.0, 0.0, 0);
    let mut staggered_at = None;
    for t in 0..200 {
        let btn = if t % 40 == 0 { buttons::PRIMARY } else { 0 };
        // The attacker keeps walking into the target so knockback cannot carry it away.
        tick(
            &mut zone,
            &world,
            &[(a, input(0.0, 1.0, btn)), (b, idle)],
            0,
        );
        if staggered_at.is_none() && zone.player(b).unwrap().mover.statuses.staggered() {
            staggered_at = Some(t);
        }
    }
    let t = staggered_at.expect("staggered");
    assert!(t >= 80, "needs three hits, staggered at tick {t}");
    let staggers = zone
        .events
        .iter()
        .filter(|e| matches!(e, ZoneEvent::Staggered(id) if *id == b))
        .count();
    assert_eq!(staggers, 1, "immunity after the first stagger");
    assert!(
        hits(&zone, HitKind::Melee) >= 4,
        "{}",
        hits(&zone, HitKind::Melee)
    );
}

#[test]
fn fortify_cuts_frost_but_not_the_hammer() {
    let (world, mut zone, ids) = arena_builds(&[
        ("ironclad", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("ironclad", Vec3::new(55.0, 0.0, REST_Z), 180.0),
        ("frostweaver", Vec3::new(0.0, 200.0, REST_Z), 270.0),
    ]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let idle = input(180.0, 0.0, 0);
    // b fortifies (active slot 2), then a hammers b and c throws an ice shard at b.
    tick(
        &mut zone,
        &world,
        &[
            (a, input(0.0, 0.0, 0)),
            (b, active(180.0, 0.0, 2)),
            (c, idle),
        ],
        0,
    );
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, 0)), (b, idle), (c, idle)],
        3,
    );
    assert!(zone.player(b).unwrap().mover.statuses.has(Status::Fortify));
    tick(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, buttons::PRIMARY)), (b, idle), (c, idle)],
        0,
    );
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, 0)), (b, idle), (c, idle)],
        25,
    );
    let pb = zone.player(b).unwrap();
    // Hammer 50 x 1.2 x 1.25 (blunt vs plate) x 0.7 (armour) = 52.5 -> 53, fortify ignored.
    assert_eq!(pb.max_health() - pb.health, 53);
    let before = pb.health;
    // c aims at b where the hammer's knockback left it.
    let to = pb.mover.mv.origin - zone.player(c).unwrap().mover.mv.origin;
    let yaw = to.y.atan2(to.x).to_degrees().rem_euclid(360.0);
    tick(
        &mut zone,
        &world,
        &[
            (a, input(0.0, 0.0, 0)),
            (b, idle),
            (c, input(yaw, 0.0, buttons::SECONDARY)),
        ],
        0,
    );
    run(
        &mut zone,
        &world,
        &[(a, input(0.0, 0.0, 0)), (b, idle), (c, input(yaw, 0.0, 0))],
        40,
    );
    let pb = zone.player(b).unwrap();
    // Ice shard 40 x 1.2 x 2 (frost into stone) x 0.7 (ward 0.30) x 0.6 (fortify) = 40.3 -> 40.
    assert_eq!(
        before - pb.health,
        40,
        "{}",
        hits(&zone, HitKind::Projectile)
    );
}

#[test]
fn respec_applies_at_the_next_respawn() {
    let (world, mut zone, ids) = arena_builds(&[
        ("blade", Vec3::new(0.0, 0.0, REST_Z), 0.0),
        ("blade", Vec3::new(50.0, 0.0, REST_Z), 180.0),
    ]);
    let (a, b) = (ids[0], ids[1]);
    let frost = zone.content.build("frostweaver").unwrap().clone();
    let bad = Build {
        attributes: Attributes::new(5, 5, 5, 5, 5),
        ..frost.clone()
    };
    assert!(zone.request_respec(b, bad).is_err());
    zone.request_respec(b, frost).unwrap();
    assert_eq!(
        zone.player(b).unwrap().sheet.build.frame,
        ArchetypeFrame::Striker
    );
    zone.player_mut(b).unwrap().health = 1;
    run(
        &mut zone,
        &world,
        &[
            (a, input(0.0, 0.0, buttons::PRIMARY)),
            (b, input(180.0, 0.0, 0)),
        ],
        13,
    );
    assert!(!zone.player(b).unwrap().alive);
    for _ in 0..zone.rate.ms_to_ticks(RESPAWN_MS) {
        tick(
            &mut zone,
            &world,
            &[(a, input(0.0, 0.0, 0)), (b, input(180.0, 0.0, 0))],
            0,
        );
    }
    let pb = zone.player(b).unwrap();
    assert!(pb.alive);
    assert_eq!(pb.sheet.build.frame, ArchetypeFrame::Caster);
    assert_eq!(pb.sheet.build.armour, ArmourClass::Cloth);
    assert!(pb.sheet.build.aspects.contains(Element::Frost));
    assert_eq!(pb.health, pb.sheet.derived.health);
    assert_eq!(pb.health, 80 + 3 * 16);
    assert_eq!(
        pb.sheet.build.aspects,
        Aspects::two(Element::Frost, Element::Shadow)
    );
}
