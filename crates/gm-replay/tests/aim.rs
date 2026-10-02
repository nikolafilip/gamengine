//! ANTICHEAT.md 10.1: synthetic view traces produce exactly their flags. A "hand" (bounded
//! turn rate, an aim error, a reaction time) is clean; a flick, a lock, a laser, a spin and
//! an instant reaction are each caught by their rule and by no other. And a file written
//! and read back gives the same numbers.

use glam::Vec3;
use gm_core::trace::Hull;
use gm_net::control::BodyKind;
use gm_net::quant;
use gm_net::snapshot::{EntityState, Snapshot, SpawnInfo, flags};
use gm_replay::aim::{self, Analyser, ideal_aim};
use gm_replay::{Event, Header, Hit, RawFrame, Reason, Replay, RosterEntry};

const HZ: u16 = 64;
const SHOOTER: u32 = 1;
const TARGET: u32 = 2;
const SPEED: f32 = 1500.0;
const GRAVITY: f32 = 0.15;

fn roster() -> Vec<RosterEntry> {
    let body = |id: u32, name: &str, team: u8| RosterEntry {
        id,
        name: name.into(),
        team,
        kind: BodyKind::Human,
        party: id,
        build: "blade".into(),
        character: 0,
    };
    vec![body(SHOOTER, "shooter", 1), body(TARGET, "target", 2)]
}

fn state(id: u32, team: u8, pos: Vec3, yaw: f32, pitch: f32) -> EntityState {
    EntityState {
        id,
        spawn: SpawnInfo::Player {
            frame: 1,
            team,
            aspects: 0,
            armour: 0,
        },
        pos: quant::quantize_pos3(pos.into()),
        yaw: quant::yaw_to_wire(yaw),
        pitch: quant::pitch_to_wire(pitch),
        vel: None,
        anim: 0,
        health: Some(100),
        flags: flags::ALIVE,
        status: 0,
    }
}

fn angles(dir: Vec3) -> (f32, f32) {
    let yaw = dir.y.atan2(dir.x).to_degrees().rem_euclid(360.0);
    let pitch = (-dir.z).atan2(dir.truncate().length()).to_degrees();
    (yaw, pitch)
}

/// Where the target stands at a tick: strafing across the shooter's front at `distance`.
fn target_at(tick: u32, distance: f32, strafe: f32) -> Vec3 {
    Vec3::new(distance, (tick as f32 - 100.0) * strafe / HZ as f32, 24.0)
}

const SHOOTER_POS: Vec3 = Vec3::new(0.0, 0.0, 24.0);

fn eye() -> Vec3 {
    SHOOTER_POS + Vec3::Z * Hull::Player.eye_height()
}

/// The view that would hit: the ideal aim at a tick, as yaw and pitch.
fn perfect(tick: u32, distance: f32, strafe: f32) -> (f32, f32) {
    let centre = target_at(tick, distance, strafe) + Vec3::Z * 4.0;
    let velocity = Vec3::new(0.0, strafe, 0.0);
    angles(ideal_aim(eye(), centre, velocity, SPEED, GRAVITY))
}

/// Run `ticks` frames with `view(tick)` as the shooter's angles and shots at `shots`.
fn run(
    distance: f32,
    strafe: f32,
    view: &dyn Fn(u32) -> (f32, f32),
    shots: &[u32],
    extra: &dyn Fn(u32) -> Vec<Event>,
) -> (Analyser, Vec<RawFrame>) {
    let mut a = Analyser::new(HZ, true, roster(), true);
    let mut frames = Vec::new();
    let mut previous: Option<Snapshot> = None;
    for tick in 60..200u32 {
        let (yaw, pitch) = view(tick);
        let entities = vec![
            state(SHOOTER, 1, SHOOTER_POS, yaw, pitch),
            state(TARGET, 2, target_at(tick, distance, strafe), 180.0, 0.0),
        ];
        let mut events = extra(tick);
        if shots.contains(&tick) {
            events.push(Event::Shot {
                projectile: 1000 + tick,
                owner: SHOOTER,
                origin: eye().into(),
                speed: SPEED,
                gravity: GRAVITY,
                lifetime: 192,
                lag: 0,
                honoured: 0,
            });
        }
        a.frame(tick, &entities, &events);
        let mut snap = Snapshot::new(tick);
        snap.entities = entities;
        // A keyframe first, deltas after: what the recorder writes.
        let base = previous.as_ref();
        snap.baseline_tick = base.map_or(0, |b| b.server_tick);
        snap.normalize(base);
        frames.push(RawFrame {
            tick,
            snapshot: gm_replay::strip_datagram_header(&snap.encode(base)),
            events,
        });
        previous = Some(snap);
    }
    (a, frames)
}

fn none(_: u32) -> Vec<Event> {
    Vec::new()
}

/// A hand: it follows the ideal aim with a turn-rate limit and a wobble of a degree or so.
fn hand(distance: f32, strafe: f32) -> impl Fn(u32) -> (f32, f32) {
    move |tick| {
        let (yaw, pitch) = perfect(tick.saturating_sub(6), distance, strafe);
        let wobble = ((tick as f32) * 0.7).sin() * 1.2;
        (yaw + wobble, pitch + ((tick as f32) * 0.45).cos() * 0.8)
    }
}

#[test]
fn a_hand_is_clean() {
    // The body crosses the shooter's front fast enough to count as moving.
    let shots: Vec<u32> = (84..124).collect();
    let (a, _) = run(400.0, 320.0, &hand(400.0, 320.0), &shots, &none);
    let s = a.stats(SHOOTER);
    assert_eq!(s.shots, shots.len() as u32);
    assert!(s.analysed >= aim::MIN_SHOTS, "{}", s.line());
    assert!(s.moving_shots >= aim::MIN_MOVING_SHOTS, "{}", s.line());
    assert_eq!((s.flicks, s.locks, s.lasers), (0, 0, 0), "{}", s.line());
    assert!(s.rules().is_empty(), "{}", s.line());
    assert!(s.median_error().unwrap() > 0.6, "{}", s.line());
}

#[test]
fn a_lock_is_a_lock() {
    // Exactly on the ideal aim of a body crossing at speed, frame after frame.
    // The shots fall while it crosses the shooter's front, where its bearing changes fastest.
    let shots: Vec<u32> = (80..124).collect();
    let view = |tick: u32| perfect(tick, 400.0, 450.0);
    let (a, _) = run(400.0, 450.0, &view, &shots, &none);
    let s = a.stats(SHOOTER);
    assert!(s.lock_rate() > 0.9, "{}", s.line());
    assert_eq!(s.flicks, 0, "{}", s.line());
    assert!(s.rules().contains(&aim::Rule::Lock), "{}", s.line());
    assert!(s.median_error().unwrap() <= 0.15, "{}", s.line());
    assert_eq!(
        s.hard_shots,
        s.analysed,
        "a fast crossing body needs a lead: {}",
        s.line()
    );

    // The same with a constant offset of 0.7 degrees: every shot is still on the body, the
    // error is no longer small, and the view is exactly as steady. It is still a lock.
    let offset = |tick: u32| {
        let (yaw, pitch) = perfect(tick, 400.0, 450.0);
        (yaw + 0.7, pitch)
    };
    let (a, _) = run(400.0, 450.0, &offset, &shots, &none);
    let s = a.stats(SHOOTER);
    assert!(s.lock_rate() > 0.9, "{}", s.line());
    assert!(s.median_error().unwrap() >= 0.6, "{}", s.line());
    assert!(s.rules().contains(&aim::Rule::Lock), "{}", s.line());
}

#[test]
fn a_flick_is_a_flick() {
    // Looking away, then on the target two frames before each shot, then away again.
    let shots: Vec<u32> = (80..190).step_by(10).collect();
    let view = |tick: u32| {
        let to_shot = shots.iter().map(|s| s.wrapping_sub(tick)).min().unwrap();
        if to_shot <= 2 {
            perfect(tick, 400.0, 0.0)
        } else {
            (70.0, 0.0)
        }
    };
    let (a, _) = run(400.0, 0.0, &view, &shots, &none);
    let s = a.stats(SHOOTER);
    assert_eq!(s.flicks, s.analysed, "{}", s.line());
    assert_eq!(
        s.locks,
        0,
        "a standing target moves no ideal aim: {}",
        s.line()
    );
    assert!(a.shots.iter().all(|s| s.snap > 60.0 && s.settle <= 2));
    // Too few shots for a rule: nothing is said about eleven shots.
    assert!(
        s.analysed < aim::MIN_SHOTS && s.rules().is_empty(),
        "{}",
        s.line()
    );

    // The same turn spread over two frames (a client at 30 frames a second sends two input
    // frames with one view): it is the same flick.
    let slow = |tick: u32| {
        let to_shot = shots.iter().map(|s| s.wrapping_sub(tick)).min().unwrap();
        match to_shot {
            0 | 1 => perfect(tick, 400.0, 0.0),
            2 | 3 => {
                let (yaw, pitch) = perfect(tick, 400.0, 0.0);
                ((yaw + 70.0) * 0.5, pitch)
            }
            _ => (70.0, 0.0),
        }
    };
    let (a, _) = run(400.0, 0.0, &slow, &shots, &none);
    let s = a.stats(SHOOTER);
    assert_eq!(s.flicks, s.analysed, "{}", s.line());
}

#[test]
fn a_laser_is_a_laser() {
    let shots: Vec<u32> = (80..190).step_by(2).collect();
    let view = |tick: u32| perfect(tick, 900.0, 0.0);
    let (a, _) = run(900.0, 0.0, &view, &shots, &none);
    let s = a.stats(SHOOTER);
    assert_eq!(s.lasers, s.analysed, "{}", s.line());
    assert_eq!(s.long_shots, s.analysed, "{}", s.line());
    assert_eq!((s.flicks, s.locks), (0, 0), "{}", s.line());
    assert_eq!(s.rules(), vec![aim::Rule::Laser], "{}", s.line());
}

#[test]
fn a_spin_is_a_spin_and_hits_are_credited() {
    // The view is turned away until four frames before a melee hit lands.
    let view = |tick: u32| {
        if tick < 136 {
            (180.0, 0.0)
        } else {
            perfect(tick, 60.0, 0.0)
        }
    };
    let hits = |tick: u32| match tick {
        140 => vec![Event::Hit {
            attacker: SHOOTER,
            target: TARGET,
            amount: 35,
            kind: Hit::Melee,
            absorbed: 0,
        }],
        // A projectile of the shot at 150 arrives.
        160 => vec![Event::Hit {
            attacker: SHOOTER,
            target: TARGET,
            amount: 40,
            kind: Hit::Projectile,
            absorbed: 0,
        }],
        170 => vec![Event::Killed {
            victim: TARGET,
            killer: SHOOTER,
        }],
        _ => Vec::new(),
    };
    let (a, _) = run(60.0, 0.0, &view, &[150], &hits);
    let s = a.stats(SHOOTER);
    assert_eq!((s.melee_hits, s.spins), (1, 1), "{}", s.line());
    assert_eq!((s.analysed, s.hits), (1, 1), "{}", s.line());
    assert_eq!((s.kills, s.damage_dealt), (1, 75), "{}", s.line());
    let t = a.stats(TARGET);
    assert_eq!((t.deaths, t.damage_taken), (1, 75), "{}", t.line());
    assert!(a.shots[0].hit);
}

#[test]
fn reactions_are_counted_and_the_rule_needs_ten() {
    // The zone measures a reaction (it has the map) and records it; the analysis counts.
    let react = |ms: i16| {
        move |tick: u32| {
            if (100..160).contains(&tick) && tick.is_multiple_of(5) {
                vec![Event::Reaction {
                    viewer: SHOOTER,
                    body: TARGET,
                    ms,
                }]
            } else {
                Vec::new()
            }
        }
    };
    let view = |_: u32| (90.0, 0.0);
    let (a, _) = run(400.0, 0.0, &view, &[], &react(240));
    let s = a.stats(SHOOTER);
    assert_eq!(s.reactions, 12, "{}", s.line());
    assert_eq!(s.median_reaction_ms(), Some(280.0));
    assert!(s.rules().is_empty(), "{}", s.line());
    let (a, _) = run(400.0, 0.0, &view, &[], &react(12));
    let s = a.stats(SHOOTER);
    assert_eq!(s.rules(), vec![aim::Rule::Reaction], "{}", s.line());
}

#[test]
fn a_rate_on_few_shots_is_not_a_rate() {
    // 6 of 10 is 60%, but with 95% confidence only "at least 31%": not a lock rate of 35%.
    assert!(aim::wilson_lower(6, 10) < aim::LOCK_RATE);
    assert!(aim::wilson_lower(9, 10) > aim::LOCK_RATE);
    assert!(aim::wilson_lower(60, 100) > aim::LOCK_RATE);
    assert_eq!(aim::wilson_lower(0, 0), 0.0);
}

#[test]
fn in_a_zone_without_teams_only_the_party_is_a_friend() {
    // Both on team 1 (a wild zone puts every human there), different parties: a kill.
    let mut wild = roster();
    wild[1].team = 1;
    let kill = vec![Event::Killed {
        victim: TARGET,
        killer: SHOOTER,
    }];
    let mut a = Analyser::new(HZ, false, wild.clone(), false);
    a.frame(1, &[], &kill);
    assert_eq!(
        (a.stats(SHOOTER).kills, a.stats(SHOOTER).team_kills),
        (1, 0)
    );
    // The same two in a zone with teams: a team kill.
    let mut a = Analyser::new(HZ, true, wild.clone(), false);
    a.frame(1, &[], &kill);
    assert_eq!(
        (a.stats(SHOOTER).kills, a.stats(SHOOTER).team_kills),
        (0, 1)
    );
    // One party, anywhere: a team kill.
    wild[1].party = SHOOTER;
    let mut a = Analyser::new(HZ, false, wild, false);
    a.frame(1, &[], &kill);
    assert_eq!(a.stats(SHOOTER).team_kills, 1);
}

#[test]
fn a_file_gives_the_same_numbers_as_the_live_analysis() {
    let shots: Vec<u32> = (80..190).step_by(2).collect();
    let view = |tick: u32| perfect(tick, 400.0, 300.0);
    let (live, frames) = run(400.0, 300.0, &view, &shots, &none);
    let header = Header {
        version: gm_replay::REPLAY_VERSION,
        codec: gm_replay::SNAPSHOT_CODEC,
        content_hash: 9,
        teams: true,
        zone: "test".into(),
        map: "arena".into(),
        map_hash: 7,
        hz: HZ,
        first_tick: frames[0].tick,
        started_unix: 1,
        reason: Reason::Fight,
        reports: Vec::new(),
        roster: roster(),
    };
    let bytes = gm_replay::write(&header, &frames);
    let replay = Replay::read(&bytes).expect("the file reads back");
    assert_eq!(replay.header, header);
    assert_eq!(replay.frames.len(), frames.len());
    assert!((replay.seconds() - 139.0 / 64.0).abs() < 1e-3);
    let (stats, shots_read) = aim::analyse(&replay);
    assert_eq!(stats[&SHOOTER], live.stats(SHOOTER));
    assert_eq!(shots_read, live.shots);
    // Playback: the target half way between two frames stands half way.
    let between = replay.entities_at(10.5 / 64.0);
    let target = between.iter().find(|e| e.id == TARGET).unwrap();
    let a = target_at(70, 400.0, 300.0);
    let b = target_at(71, 400.0, 300.0);
    assert!(
        (target.pos - a.lerp(b, 0.5)).length() < 0.5,
        "{:?}",
        target.pos
    );

    // A damaged file is refused, not half read.
    let mut cut = bytes.clone();
    cut.truncate(bytes.len() - 9);
    assert!(Replay::read(&cut).is_err());
    assert!(Replay::read(b"GMR0").is_err());
    let mut other = header.clone();
    other.codec = 0;
    assert!(Replay::read(&gm_replay::write(&other, &frames)).is_err());
}

#[test]
fn a_view_claimed_stale_does_not_hide_a_lock() {
    // A client that locks onto the present and says its view is half a second old: its
    // hits are resolved in the present (the zone honours no such lag), and its aim is
    // judged against the view the zone honoured when that fits better.
    let view = |tick: u32| perfect(tick, 400.0, 450.0);
    let liar = |tick: u32| {
        if (112..124).contains(&tick) {
            vec![Event::Shot {
                projectile: 1000 + tick,
                owner: SHOOTER,
                origin: eye().into(),
                speed: SPEED,
                gravity: GRAVITY,
                lifetime: 192,
                lag: 30,
                honoured: 0,
            }]
        } else {
            Vec::new()
        }
    };
    let (a, _) = run(400.0, 450.0, &view, &[], &liar);
    let s = a.stats(SHOOTER);
    assert_eq!(s.analysed, 12, "{}", s.line());
    assert_eq!(s.stale_views, s.analysed, "{}", s.line());
    assert!(s.rules().contains(&aim::Rule::Lock), "{}", s.line());

    // An honest client whose view really is that old aims at the world as it was then;
    // the claim fits, and nothing is stale.
    let old_view = |tick: u32| perfect(tick - 8, 400.0, 450.0);
    let honest = |tick: u32| {
        if (112..124).contains(&tick) {
            vec![Event::Shot {
                projectile: 1000 + tick,
                owner: SHOOTER,
                origin: eye().into(),
                speed: SPEED,
                gravity: GRAVITY,
                lifetime: 192,
                lag: 8,
                honoured: 5,
            }]
        } else {
            Vec::new()
        }
    };
    let (a, _) = run(400.0, 450.0, &old_view, &[], &honest);
    let s = a.stats(SHOOTER);
    assert_eq!(s.stale_views, 0, "{}", s.line());
    assert!(s.median_error().unwrap() <= 0.15, "{}", s.line());
}

/// The client frame a zone ran last in a tick, when frames come unevenly: none for a few
/// ticks, then the backlog. The client drew frame `c` looking at the world of tick `c - 3`.
fn uneven(tick: u32) -> (u32, u8) {
    const BEHIND: [u32; 8] = [0, 1, 2, 3, 2, 1, 0, 0];
    let behind = BEHIND[(tick % 8) as usize];
    (tick - behind, (3 + behind) as u8)
}

#[test]
fn a_lock_whose_frames_arrive_unevenly_is_still_a_lock() {
    // Every frame the client drew sits exactly on the ideal aim of the world it looked at.
    // The zone runs those frames late and in bursts, so the view it holds after a tick is
    // of a world up to six ticks old, a different age every tick. Held against the world
    // each tick's frames said they looked at, the view is as steady as it was drawn.
    let view = |tick: u32| perfect(uneven(tick).0 - 3, 400.0, 450.0);
    let events = |with_lags: bool| {
        move |tick: u32| {
            let lag = uneven(tick).1;
            let mut events = Vec::new();
            if with_lags && (tick == 60 || uneven(tick - 1).1 != lag) {
                events.push(Event::View { id: SHOOTER, lag });
            }
            if (104..124).contains(&tick) {
                events.push(Event::Shot {
                    projectile: 1000 + tick,
                    owner: SHOOTER,
                    origin: eye().into(),
                    speed: SPEED,
                    gravity: GRAVITY,
                    lifetime: 192,
                    lag: lag as u32,
                    honoured: lag as u32,
                });
            }
            events
        }
    };
    let (a, frames) = run(400.0, 450.0, &view, &[], &events(true));
    let s = a.stats(SHOOTER);
    assert_eq!(s.analysed, 20, "{}", s.line());
    assert!(s.lock_rate() > 0.9, "{}", s.line());
    assert!(s.rules().contains(&aim::Rule::Lock), "{}", s.line());
    assert!(s.median_error().unwrap() <= 0.15, "{}", s.line());

    // And the file says the same.
    let header = Header {
        version: gm_replay::REPLAY_VERSION,
        codec: gm_replay::SNAPSHOT_CODEC,
        content_hash: 9,
        teams: true,
        zone: "test".into(),
        map: "arena".into(),
        map_hash: 7,
        hz: HZ,
        first_tick: frames[0].tick,
        started_unix: 1,
        reason: Reason::Fight,
        reports: Vec::new(),
        roster: roster(),
    };
    let replay = Replay::read(&gm_replay::write(&header, &frames)).expect("reads");
    let (stats, _) = aim::analyse(&replay);
    assert_eq!(stats[&SHOOTER].line(), s.line());

    // Without the per-tick lags (the shot's own lag held for its whole window) the same
    // play does not look like a lock: this is what the event is recorded for.
    let (a, _) = run(400.0, 450.0, &view, &[], &events(false));
    let s = a.stats(SHOOTER);
    assert!(!s.rules().contains(&aim::Rule::Lock), "{}", s.line());
}
