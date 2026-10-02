//! `gm-tools replay` (ANTICHEAT.md 3.4): what a `.gmr` holds, and its aim statistics
//! recomputed from its frames by the code the zone ran.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use gm_replay::{Event, Replay};

#[derive(Subcommand)]
pub enum ReplayCmd {
    /// The header, who was in it, the kills, the sizes.
    Info { file: PathBuf },
    /// The aim statistics of every participant, recomputed from the frames.
    Aim {
        file: PathBuf,
        /// Also every analysed shot of this player (a name), or of everybody (`all`).
        #[arg(long)]
        shots: Option<String>,
    },
}

fn load(file: &Path) -> Result<(Replay, usize)> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let replay = Replay::read(&bytes).with_context(|| format!("{}", file.display()))?;
    Ok((replay, bytes.len()))
}

pub fn run(cmd: ReplayCmd) -> Result<()> {
    match cmd {
        ReplayCmd::Info { file } => {
            let (replay, bytes) = load(&file)?;
            let h = &replay.header;
            let frame_bytes: usize = replay
                .frames
                .iter()
                .map(|f| f.snapshot.entities.len())
                .sum();
            println!(
                "replay: zone={} map={} hz={} reason={:?} teams={} started_unix={} first_tick={} frames={} seconds={:.1} bytes={} bytes_per_minute={:.0} entity_records={}",
                h.zone,
                h.map,
                h.hz,
                h.reason,
                h.teams,
                h.started_unix,
                h.first_tick,
                replay.frames.len(),
                replay.seconds(),
                bytes,
                bytes as f32 * 60.0 / replay.seconds().max(1.0),
                frame_bytes,
            );
            let everyone = replay.everyone();
            let name = |id: u32| {
                everyone
                    .iter()
                    .find(|r| r.id == id)
                    .map_or_else(|| format!("#{id}"), |r| r.name.clone())
            };
            for r in &everyone {
                println!(
                    "participant: id={} name={} team={} party={} kind={:?} build={} character={}",
                    r.id, r.name, r.team, r.party, r.kind, r.build, r.character
                );
            }
            let (mut hits, mut shots, mut reactions) = (0, 0, 0);
            let mut lags: Vec<u8> = Vec::new();
            for (i, f) in replay.frames.iter().enumerate() {
                for e in &f.events {
                    match e {
                        Event::Killed { victim, killer } => println!(
                            "kill: at={:.1}s {} killed {}",
                            i as f32 / h.hz.max(1) as f32,
                            name(*killer),
                            name(*victim)
                        ),
                        Event::Hit { .. } => hits += 1,
                        Event::Shot { .. } => shots += 1,
                        Event::Reaction { .. } => reactions += 1,
                        Event::View { lag, .. } => lags.push(*lag),
                        _ => {}
                    }
                }
            }
            println!("events: hits={hits} shots={shots} reactions={reactions}");
            // How far behind the present the clients' frames said they looked, in ticks.
            lags.sort_unstable();
            if let (Some(min), Some(max)) = (lags.first(), lags.last()) {
                println!(
                    "view lags: changes={} min={min} median={} max={max}",
                    lags.len(),
                    lags[lags.len() / 2]
                );
            }
        }
        ReplayCmd::Aim { file, shots } => {
            let (replay, _) = load(&file)?;
            let (stats, all_shots) = gm_replay::aim::analyse(&replay);
            let everyone = replay.everyone();
            for r in everyone.iter().filter(|r| r.human()) {
                let s = stats.get(&r.id).cloned().unwrap_or_default();
                println!("aim: name={} {}", r.name, s.line());
            }
            if let Some(who) = shots {
                let name = |id: u32| {
                    everyone
                        .iter()
                        .find(|r| r.id == id)
                        .map_or_else(|| format!("#{id}"), |r| r.name.clone())
                };
                let first = replay.header.first_tick;
                for s in all_shots
                    .iter()
                    .filter(|s| who == "all" || name(s.owner) == who)
                {
                    println!(
                        "shot: at={:.2}s by={} at_whom={} distance={:.0} error={:.2} lead={:.2} snap={:.1} settle={} steady={:.2} target_motion={:.1} {}{}{}{}",
                        s.tick.wrapping_sub(first) as f32 / replay.header.hz.max(1) as f32,
                        name(s.owner),
                        name(s.target),
                        s.distance,
                        s.error,
                        s.lead,
                        s.snap,
                        s.settle,
                        s.steady,
                        s.target_motion,
                        if s.hit { "hit " } else { "" },
                        if s.flick { "FLICK " } else { "" },
                        if s.lock { "LOCK " } else { "" },
                        if s.laser { "LASER" } else { "" },
                    );
                }
            }
        }
    }
    Ok(())
}
