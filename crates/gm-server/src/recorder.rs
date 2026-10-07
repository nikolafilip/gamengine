//! The zone's recorder (ANTICHEAT.md 3): every tick one frame of every entity and the tick's
//! events, the last half minute always in memory, written to a `.gmr` when there is a reason:
//! a fight between players, or a report. The live aim analysis reads the same frames.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gm_core::vocab::EntityId;
use gm_net::snapshot::{EntityState, Snapshot};
use gm_replay::aim::{AimStats, Analyser};
use gm_replay::{Event, Header, KEYFRAME_EVERY, RawFrame, Reason, RosterEntry};

use crate::session::TickTable;

/// Seconds of the past always in memory.
pub const RING_SECS: u32 = 30;
/// A fight's file begins this long before its first hit.
pub const LEAD_IN_SECS: u32 = 10;
/// A fight closes this long after its last hit between players.
pub const FIGHT_QUIET_SECS: u32 = 10;
/// A fight's file is cut at this length and continues in the next.
pub const FIGHT_MAX_SECS: u32 = 120;
/// A closed fight is worth a file if somebody died in it or this much damage was dealt.
pub const FIGHT_MIN_DAMAGE: u64 = 100;
/// A report's file runs this long past the report.
pub const REPORT_TAIL_SECS: u32 = 10;
/// Reports that arrive while a report's file is open share it; it is not kept open past
/// this length for them.
pub const REPORT_MAX_SECS: u32 = 120;

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    pub dir: PathBuf,
    pub zone: String,
    pub map: String,
    pub map_hash: u64,
    pub hz: u16,
    /// The zone has teams (not a wild zone).
    pub teams: bool,
    pub content_hash: u64,
    /// Bytes of fight files the zone writes per hour at most, and as much again for
    /// reports' files.
    pub bytes_per_hour: u64,
}

struct RingFrame {
    raw: Arc<RawFrame>,
    /// At a keyframe: the roster as it stood after the frame's events.
    key: Option<Vec<RosterEntry>>,
}

/// Frames being collected for a file.
struct Capture {
    reason: Reason,
    /// The hub's reports this file belongs to.
    reports: Vec<i64>,
    roster: Vec<RosterEntry>,
    started_unix: u64,
    frames: Vec<Arc<RawFrame>>,
    /// A fight: the tick of its last hit between players; a report: the tick it ends at.
    mark: u32,
    damage: u64,
    kills: u32,
}

/// A capture that is complete: to be compressed and written off the tick loop.
pub struct Finished {
    pub header: Header,
    pub frames: Vec<Arc<RawFrame>>,
    pub path: PathBuf,
}

/// A file on disk and what is in it (the hub's row, ANTICHEAT.md 3.3).
#[derive(Clone, Debug)]
pub struct Written {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub header: Header,
    pub seconds: f32,
    pub kills: u32,
    pub damage: u64,
    /// Every client-driven body in the file with its aim numbers over the file.
    pub participants: Vec<(RosterEntry, AimStats)>,
}

impl Finished {
    /// Compress, write, read back and analyse. Runs on a blocking thread.
    pub fn write(self) -> std::io::Result<Written> {
        let frames: Vec<RawFrame> = self.frames.iter().map(|f| (**f).clone()).collect();
        let bytes = gm_replay::write(&self.header, &frames);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Under another name until it is whole: a file with the final name is a replay.
        let partial = self.path.with_extension("part");
        std::fs::write(&partial, &bytes)?;
        std::fs::rename(&partial, &self.path)?;
        Written::of(self.path, bytes)
    }
}

impl Written {
    /// What is in a replay's bytes. The numbers of a file are computed from the file: what
    /// a reviewer recomputes.
    pub fn of(path: PathBuf, bytes: Vec<u8>) -> std::io::Result<Written> {
        let replay = gm_replay::Replay::read(&bytes)
            .map_err(|e| std::io::Error::other(format!("the replay {}: {e}", path.display())))?;
        let (stats, _) = gm_replay::aim::analyse(&replay);
        let mut kills = 0;
        let mut damage = 0u64;
        let everyone = replay.everyone();
        let party = |id: u32| everyone.iter().find(|r| r.id == id).map_or(0, |r| r.party);
        for f in &replay.frames {
            for e in &f.events {
                match e {
                    Event::Killed { victim, killer } if *killer != 0 && killer != victim => {
                        kills += 1;
                    }
                    Event::Hit {
                        attacker,
                        target,
                        amount,
                        ..
                    } if party(*attacker) != 0
                        && party(*target) != 0
                        && party(*attacker) != party(*target) =>
                    {
                        damage += (*amount).max(0) as u64;
                    }
                    _ => {}
                }
            }
        }
        let participants = everyone
            .into_iter()
            .filter(|r| r.human())
            .map(|r| {
                let s = stats.get(&r.id).cloned().unwrap_or_default();
                (r, s)
            })
            .collect();
        Ok(Written {
            path,
            bytes,
            seconds: replay.seconds(),
            header: replay.header,
            kills,
            damage,
            participants,
        })
    }

    /// The replays a stopped zone left in its directory (the hub was away, or the zone
    /// went down before it had sent them): to be read and uploaded by the next one, one
    /// at a time.
    pub fn leftovers(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "gmr"))
            .collect();
        found.sort();
        found
    }

    /// Read a replay file back into what the hub is told of it.
    pub fn read(path: PathBuf) -> std::io::Result<Written> {
        let bytes = std::fs::read(&path)?;
        Written::of(path, bytes)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecorderStats {
    pub frames: u64,
    /// Encoded bytes of the frames recorded (before the file's compression).
    pub frame_bytes: u64,
    pub fights_written: u64,
    pub reports_written: u64,
    /// Fights that closed without a death or enough damage.
    pub fights_dropped: u64,
    /// Fights not written because the hour's byte budget was spent.
    pub over_budget: u64,
    /// Frames that held more entities than a frame can: the newest were left out.
    pub crowded_frames: u64,
    pub ring_bytes: usize,
}

pub struct Recorder {
    cfg: RecorderConfig,
    ring: VecDeque<RingFrame>,
    previous: Option<Snapshot>,
    /// Sorted by id.
    roster: Vec<RosterEntry>,
    /// Each client's view lag as last recorded.
    lags: std::collections::BTreeMap<u32, u8>,
    fight: Option<Capture>,
    /// The one open report file: reports that come while it is open share it.
    report: Option<Capture>,
    /// Report files started, for their names.
    report_seq: u32,
    /// The live aim analysis: fed every frame as it is recorded.
    pub analyser: Analyser,
    pub stats: RecorderStats,
    /// The wall-clock hour being counted, and the fight bytes and the report bytes handed
    /// out in it.
    hour: (u64, u64, u64),
}

/// What a frame costs the ring, roughly: its snapshot, its events, its bookkeeping.
fn frame_cost(f: &RawFrame) -> usize {
    f.snapshot.len() + 16 * f.events.len() + 32
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Recorder {
    pub fn new(cfg: RecorderConfig) -> Recorder {
        Recorder {
            analyser: Analyser::new(cfg.hz, cfg.teams, Vec::new(), false),
            cfg,
            ring: VecDeque::new(),
            previous: None,
            roster: Vec::new(),
            lags: std::collections::BTreeMap::new(),
            fight: None,
            report: None,
            report_seq: 0,
            stats: RecorderStats::default(),
            hour: (0, 0, 0),
        }
    }

    fn secs(&self, s: u32) -> u32 {
        s * self.cfg.hz as u32
    }

    fn party(&self, id: u32) -> u32 {
        self.roster
            .binary_search_by_key(&id, |r| r.id)
            .map_or(0, |i| self.roster[i].party)
    }

    /// Record one tick. `events` are the simulation's; joins and leaves are found here by
    /// comparing the table with the roster (`describe` names a body that is new).
    /// `view_lag` says how far behind this tick a client's frames claim to look (nothing
    /// for a body no client drives): recorded when it changes and at every keyframe.
    /// Returns the captures that are complete.
    pub fn tick(
        &mut self,
        tick: u32,
        table: &TickTable,
        mut events: Vec<Event>,
        describe: &dyn Fn(EntityId) -> Option<RosterEntry>,
        view_lag: &dyn Fn(EntityId) -> Option<u8>,
    ) -> Vec<Finished> {
        // A full snapshot at a keyframe, a delta against the last frame otherwise (tick 0
        // cannot be named as a baseline: 0 says "none").
        let key = self.stats.frames.is_multiple_of(KEYFRAME_EVERY as u64)
            || self.previous.as_ref().is_some_and(|p| p.server_tick == 0);
        // Who came, who went, whose team changed: one merged walk, both ascend by id.
        let mut entities: Vec<EntityState> = Vec::with_capacity(table.entries.len());
        let mut present: Vec<u32> = Vec::new();
        for e in &table.entries {
            if e.is_player {
                present.push(e.id);
                let team = match e.state.spawn {
                    gm_net::snapshot::SpawnInfo::Player { team, .. } => team,
                    _ => 0,
                };
                let known = self
                    .roster
                    .binary_search_by_key(&e.id, |r| r.id)
                    .ok()
                    .map(|i| &self.roster[i]);
                if known.is_none_or(|r| r.team != team || r.party != e.party) {
                    // A body nobody can describe any more (its session went, it waits for
                    // another zone) keeps its entry with what the table says of it.
                    let entry = describe(e.id).or_else(|| {
                        known.map(|r| RosterEntry {
                            team,
                            party: e.party,
                            ..r.clone()
                        })
                    });
                    if let Some(entry) = entry {
                        events.push(Event::Joined(entry));
                    }
                }
                if let Some(lag) = view_lag(e.id)
                    && (key || self.lags.get(&e.id) != Some(&lag))
                {
                    self.lags.insert(e.id, lag);
                    events.push(Event::View { id: e.id, lag });
                }
                entities.push(EntityState {
                    health: Some(e.health),
                    ..e.state
                });
            } else {
                entities.push(e.state);
            }
        }
        for r in &self.roster {
            if present.binary_search(&r.id).is_err() {
                events.push(Event::Left(r.id));
                self.lags.remove(&r.id);
            }
        }
        gm_replay::apply_roster(&mut self.roster, &events);
        self.roster.sort_by_key(|r| r.id);

        if entities.len() > gm_replay::MAX_FRAME_ENTITIES {
            // More than a frame decodes: bodies come first in the table, what is cut is
            // the newest projectiles.
            entities.truncate(gm_replay::MAX_FRAME_ENTITIES);
            self.stats.crowded_frames += 1;
        }
        // The frame.
        let base = self.previous.as_ref().filter(|_| !key);
        let mut snap = Snapshot::new(tick);
        snap.entities = entities;
        snap.baseline_tick = base.map_or(0, |b| b.server_tick);
        snap.normalize(base);
        let bytes = gm_replay::strip_datagram_header(&snap.encode(base));
        self.stats.frames += 1;
        self.stats.frame_bytes += bytes.len() as u64;

        self.analyser.frame(tick, &snap.entities, &events);

        // Fights: a hit between two clients' parties opens one and keeps it open.
        let mut damage = 0u64;
        let mut kills = 0u32;
        for e in &events {
            match *e {
                Event::Hit {
                    attacker,
                    target,
                    amount,
                    ..
                } => {
                    let (a, t) = (self.party(attacker), self.party(target));
                    if a != 0 && t != 0 && a != t {
                        damage += amount.max(0) as u64;
                    }
                }
                Event::Killed { victim, killer } if killer != 0 && killer != victim => {
                    let (k, v) = (self.party(killer), self.party(victim));
                    if k != 0 && v != 0 {
                        kills += 1;
                    }
                }
                _ => {}
            }
        }
        let raw = Arc::new(RawFrame {
            tick,
            snapshot: bytes,
            events,
        });
        self.stats.ring_bytes += frame_cost(&raw);
        self.ring.push_back(RingFrame {
            raw: raw.clone(),
            key: key.then(|| self.roster.clone()),
        });
        self.previous = Some(snap);
        self.trim();

        if damage > 0 && self.fight.is_none() {
            self.fight =
                Some(self.capture(Reason::Fight, Vec::new(), self.secs(LEAD_IN_SECS), tick));
        } else if let Some(f) = &mut self.fight {
            f.frames.push(raw.clone());
        }
        if let Some(f) = &mut self.fight {
            f.damage += damage;
            f.kills += kills;
            if damage > 0 {
                f.mark = tick;
            }
        }
        if let Some(r) = &mut self.report {
            r.frames.push(raw.clone());
        }

        let mut done = Vec::new();
        // A fight closes when it has been quiet long enough; one that runs on is cut at a
        // keyframe and goes on in the next file.
        let quiet = self.secs(FIGHT_QUIET_SECS);
        let longest = self.secs(FIGHT_MAX_SECS) as usize;
        let close = self
            .fight
            .as_ref()
            .is_some_and(|f| tick.wrapping_sub(f.mark) >= quiet);
        let cut = !close
            && key
            && self
                .fight
                .as_ref()
                .is_some_and(|f| f.frames.len() >= longest);
        if close || cut {
            let mut fight = self.fight.take().expect("checked");
            if cut {
                // This keyframe starts the next file and is not in this one, and neither
                // is what happened in it.
                fight.frames.pop();
                fight.damage -= damage;
                fight.kills -= kills;
                let mut next = self.capture(Reason::Fight, Vec::new(), 0, tick);
                next.mark = fight.mark;
                next.damage = damage;
                next.kills = kills;
                self.fight = Some(next);
            }
            if fight.kills == 0 && fight.damage < FIGHT_MIN_DAMAGE {
                self.stats.fights_dropped += 1;
            } else if !self.within_budget(&fight, false) {
                self.stats.over_budget += 1;
            } else {
                self.stats.fights_written += 1;
                done.push(self.finish(fight));
            }
        }
        if self
            .report
            .as_ref()
            .is_some_and(|r| tick.wrapping_sub(r.mark) as i32 >= 0)
        {
            let r = self.report.take().expect("checked");
            // Its bytes were promised when the report was taken (`may_report`).
            self.within_budget(&r, true);
            self.stats.reports_written += 1;
            done.push(self.finish(r));
        }
        done
    }

    /// Keep `RING_SECS` and whole keyframe groups: the ring always begins at a keyframe.
    fn trim(&mut self) {
        let keep = self.secs(RING_SECS) as usize;
        while self.ring.len() > keep + KEYFRAME_EVERY as usize {
            // The next keyframe after the first: at most one group away.
            match self
                .ring
                .iter()
                .skip(1)
                .take(KEYFRAME_EVERY as usize)
                .position(|f| f.key.is_some())
            {
                Some(next) if self.ring.len() - (next + 1) >= keep => {
                    let gone: usize = self.ring.drain(..=next).map(|f| frame_cost(&f.raw)).sum();
                    self.stats.ring_bytes = self.stats.ring_bytes.saturating_sub(gone);
                }
                _ => break,
            }
        }
    }

    /// Start a capture from the ring: from the last keyframe at least `back` ticks before
    /// `tick` (the earliest in the ring if there is none that old) to the newest frame.
    fn capture(&self, reason: Reason, reports: Vec<i64>, back: u32, tick: u32) -> Capture {
        let wanted = tick.wrapping_sub(back);
        let start = self
            .ring
            .iter()
            .enumerate()
            .filter(|(_, f)| f.key.is_some() && (wanted.wrapping_sub(f.raw.tick) as i32) >= 0)
            .map(|(i, _)| i)
            .next_back()
            .or_else(|| self.ring.iter().position(|f| f.key.is_some()))
            .unwrap_or(0);
        let roster = self
            .ring
            .get(start)
            .and_then(|f| f.key.clone())
            .unwrap_or_default();
        let frames: Vec<Arc<RawFrame>> = self
            .ring
            .iter()
            .skip(start)
            .map(|f| f.raw.clone())
            .collect();
        // The wall clock of the first frame, from how far back it lies.
        let age = frames
            .first()
            .map_or(0, |f| tick.wrapping_sub(f.tick) / self.cfg.hz.max(1) as u32);
        Capture {
            reason,
            reports,
            roster,
            started_unix: unix_now().saturating_sub(age as u64),
            frames,
            mark: tick,
            damage: 0,
            kills: 0,
        }
    }

    /// Whether a report's file can be had now: not when the hour's bytes for reports are
    /// spent.
    pub fn may_report(&mut self) -> bool {
        let hour = unix_now() / 3600;
        if self.hour.0 != hour {
            self.hour = (hour, 0, 0);
        }
        self.hour.2 < self.cfg.bytes_per_hour
    }

    /// A report (ANTICHEAT.md 5): the ring as it stands and the next seconds. Reports that
    /// come while one's file is open share the file: it runs on to cover them, up to
    /// `REPORT_MAX_SECS`.
    pub fn report(&mut self, tick: u32, report: Option<i64>) {
        let (tail, longest) = (self.secs(REPORT_TAIL_SECS), self.secs(REPORT_MAX_SECS));
        match &mut self.report {
            Some(open) => {
                open.reports.extend(report);
                if open.frames.len() < longest as usize {
                    open.mark = tick.wrapping_add(tail);
                }
            }
            None => {
                let mut c = self.capture(
                    Reason::Report,
                    report.into_iter().collect(),
                    self.secs(RING_SECS),
                    tick,
                );
                c.mark = tick.wrapping_add(tail);
                self.report_seq += 1;
                self.report = Some(c);
            }
        }
    }

    fn within_budget(&mut self, c: &Capture, report: bool) -> bool {
        let hour = unix_now() / 3600;
        if self.hour.0 != hour {
            self.hour = (hour, 0, 0);
        }
        // Counted before compression: the budget is a ceiling, not a measurement.
        let bytes: u64 = c.frames.iter().map(|f| f.snapshot.len() as u64 + 16).sum();
        if report {
            self.hour.2 += bytes;
            return true;
        }
        if self.hour.1 + bytes > self.cfg.bytes_per_hour {
            return false;
        }
        self.hour.1 += bytes;
        true
    }

    fn finish(&self, c: Capture) -> Finished {
        let first_tick = c.frames.first().map_or(0, |f| f.tick);
        let header = Header {
            version: gm_replay::REPLAY_VERSION,
            codec: gm_replay::SNAPSHOT_CODEC,
            content_hash: self.cfg.content_hash,
            teams: self.cfg.teams,
            zone: self.cfg.zone.clone(),
            map: self.cfg.map.clone(),
            map_hash: self.cfg.map_hash,
            hz: self.cfg.hz,
            first_tick,
            started_unix: c.started_unix,
            reason: c.reason,
            reports: c.reports,
            roster: c.roster,
        };
        let name = format!(
            "{}-{}-{}{}.gmr",
            self.cfg.zone,
            c.started_unix,
            first_tick,
            if c.reason == Reason::Report {
                format!("-report{}", self.report_seq)
            } else {
                String::new()
            }
        );
        Finished {
            header,
            frames: c.frames,
            path: self.cfg.dir.join(name),
        }
    }

    /// The zone stops: what is open is written as it stands.
    pub fn flush(&mut self) -> Vec<Finished> {
        let mut done = Vec::new();
        if let Some(f) = self.fight.take()
            && (f.kills > 0 || f.damage >= FIGHT_MIN_DAMAGE)
        {
            self.stats.fights_written += 1;
            done.push(self.finish(f));
        }
        if let Some(r) = self.report.take() {
            self.stats.reports_written += 1;
            done.push(self.finish(r));
        }
        done
    }

    /// The current roster entry of a body.
    pub fn entry(&self, id: u32) -> Option<&RosterEntry> {
        self.roster
            .binary_search_by_key(&id, |r| r.id)
            .ok()
            .map(|i| &self.roster[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::TableEntry;
    use glam::Vec3;
    use gm_net::control::BodyKind;
    use gm_net::quant;
    use gm_net::snapshot::{SpawnInfo, flags};

    fn body(id: u32, team: u8, pos: Vec3, yaw: f32, pitch: f32) -> TableEntry {
        TableEntry {
            id,
            origin: pos,
            leaf: 0,
            leaf_top: 0,
            state: EntityState {
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
                acting: 0,
                health: None,
                flags: flags::ALIVE,
                status: 0,
            },
            stealth: f32::INFINITY,
            is_player: true,
            health: 100,
            party: id,
            creature: false,
            owner: 0,
        }
    }

    /// What the zone computes as it records and what anybody computes from the file are the
    /// same numbers (ANTICHEAT.md principle 4): one scene, recorded from its first frame.
    #[test]
    fn the_live_numbers_are_the_files() {
        let dir = std::env::temp_dir().join(format!("gm-recorder-{}", std::process::id()));
        let mut rec = Recorder::new(RecorderConfig {
            dir: dir.clone(),
            zone: "test".into(),
            map: "arena".into(),
            map_hash: 1,
            hz: 64,
            teams: true,
            content_hash: 2,
            bytes_per_hour: u64::MAX,
        });
        let describe = |id: EntityId| {
            Some(RosterEntry {
                id,
                name: format!("p{id}"),
                team: id as u8,
                kind: BodyKind::Human,
                party: id,
                build: "shade".into(),
                character: 0,
                mode: 1,
            })
        };
        let shooter = Vec3::new(0.0, 0.0, 24.0);
        let mut table = TickTable::default();
        let mut files = Vec::new();
        for tick in 1..=400u32 {
            let target = Vec3::new(500.0, (tick as f32 - 200.0) * 5.0, 24.0);
            // The view rides the target with a wobble: some shots lock, some do not.
            let to = target - shooter;
            let yaw =
                to.y.atan2(to.x).to_degrees() + ((tick / 40) % 2) as f32 * (tick as f32).sin();
            table.entries = vec![
                body(1, 1, shooter, yaw.rem_euclid(360.0), 0.0),
                body(2, 2, target, 180.0, 0.0),
            ];
            let mut events = Vec::new();
            if tick > 40 && tick.is_multiple_of(7) {
                events.push(Event::Shot {
                    projectile: 1000 + tick,
                    owner: 1,
                    origin: (shooter + Vec3::Z * 40.0).into(),
                    speed: 1400.0,
                    gravity: 0.0,
                    lifetime: 128,
                    lag: 3,
                    honoured: 2,
                });
            }
            if tick > 60 && tick.is_multiple_of(21) {
                events.push(Event::Hit {
                    attacker: 1,
                    target: 2,
                    amount: 30,
                    kind: gm_replay::Hit::Projectile,
                    absorbed: 0,
                });
            }
            // The view lag wanders, as a client's does when its frames come unevenly.
            let lag = |id: EntityId| (id == 1).then_some(3 + ((tick / 5) % 3) as u8);
            files.extend(rec.tick(tick, &table, events, &describe, &lag));
            if tick == 1 {
                // A report at the first frame: its file holds everything from there.
                rec.report(tick, None);
            }
        }
        let live = rec.analyser.stats(1);
        assert!(live.analysed >= 40 && live.hits > 0, "{}", live.line());
        files.extend(rec.flush());
        let report = files
            .into_iter()
            .find(|f| f.header.reason == Reason::Report)
            .expect("the report's file");
        let written = report.write().expect("written");
        let (_, from_file) = written
            .participants
            .iter()
            .find(|(who, _)| who.id == 1)
            .expect("the shooter is in the file");
        assert_eq!(from_file.line(), live.line());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A fight that runs on is cut at a keyframe into files that each read by themselves,
    /// and what happens in the cut's tick belongs to the file that holds its frame.
    #[test]
    fn a_long_fight_is_cut() {
        let dir = std::env::temp_dir().join(format!("gm-recorder-cut-{}", std::process::id()));
        let mut rec = Recorder::new(RecorderConfig {
            dir: dir.clone(),
            zone: "test".into(),
            map: "arena".into(),
            map_hash: 1,
            hz: 64,
            teams: true,
            content_hash: 2,
            bytes_per_hour: u64::MAX,
        });
        let describe = |id: EntityId| {
            Some(RosterEntry {
                id,
                name: format!("p{id}"),
                team: id as u8,
                kind: BodyKind::Human,
                party: id,
                build: "blade".into(),
                character: 0,
                mode: 1,
            })
        };
        let mut table = TickTable::default();
        let mut files = Vec::new();
        let ticks = 64 * (FIGHT_MAX_SECS + 40);
        for tick in 1..=ticks {
            table.entries = vec![
                body(1, 1, Vec3::new(0.0, 0.0, 24.0), (tick % 360) as f32, 0.0),
                body(2, 2, Vec3::new(100.0, (tick % 50) as f32, 24.0), 180.0, 0.0),
            ];
            // A hit in every tick: whichever tick the cut falls on has one.
            let events = vec![Event::Hit {
                attacker: 1,
                target: 2,
                amount: 1,
                kind: gm_replay::Hit::Melee,
                absorbed: 0,
            }];
            files.extend(rec.tick(tick, &table, events, &describe, &|_| Some(2)));
        }
        assert_eq!(files.len(), 1, "one file cut, the next still open");
        files.extend(rec.flush());
        assert_eq!(files.len(), 2);
        let mut frames = 0;
        let mut damage = 0;
        let mut next_tick = None;
        for f in files {
            let w = f.write().expect("written and read back");
            let replay = gm_replay::Replay::read(&w.bytes).expect("reads by itself");
            if let Some(expected) = next_tick {
                assert_eq!(replay.frames[0].tick, expected, "no frame lost at the cut");
            }
            next_tick = Some(replay.frames.last().unwrap().tick + 1);
            frames += replay.frames.len();
            damage += w.damage;
        }
        // The first file starts at the fight's first frame (there is no ring before it).
        assert_eq!(frames as u32, ticks);
        assert_eq!(damage, ticks as u64);
        let _ = std::fs::remove_dir_all(dir);
    }
}
