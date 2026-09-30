//! Per-client state on the server: acked baselines, reconstructed snapshot history, PVS and
//! distance-band scheduling, delta encoding within the datagram budget (PROTOCOL.md 5).

use std::collections::{HashMap, VecDeque};

use glam::Vec3;
use gm_bsp::Bsp;
use gm_core::sim::{Player, Projectile, Zone};
use gm_core::vocab::{ArchetypeFrame, EntityId};
use gm_net::MAX_DATAGRAM_PAYLOAD;
use gm_net::control::Control;
use gm_net::quant;
use gm_net::snapshot::{EntityState, Snapshot, SpawnInfo, flags};
use tokio::sync::mpsc;

use crate::world::ZoneWorld;

/// Snapshots kept per client; acks older than this get a full snapshot.
pub const HISTORY: usize = 64;
pub const MAX_BASELINE_AGE: i32 = 60;
/// Distance bands (PROTOCOL.md 5).
pub const FULL_RATE_DIST: f32 = 512.0;
pub const HALF_RATE_DIST: f32 = 1536.0;
/// Bodies this close are always sent, PVS or not: they can block the client's movement and
/// its prediction must know about them (PROTOCOL.md 5).
pub const TOUCH_DIST: f32 = 128.0;

/// Decompressed PVS rows by leaf, shared by all sessions within a tick loop.
#[derive(Default)]
pub struct PvsCache {
    rows: HashMap<usize, Vec<u8>>,
}

impl PvsCache {
    pub fn row(&mut self, bsp: &Bsp, leaf: usize) -> &[u8] {
        self.rows
            .entry(leaf)
            .or_insert_with(|| bsp.decompress_pvs(leaf))
    }
}

pub struct Session {
    pub id: EntityId,
    pub name: String,
    pub conn: quinn::Connection,
    /// Bounded: a client that stops reading its control stream is kicked, not buffered forever.
    pub control: mpsc::Sender<Control>,
    history: VecDeque<Snapshot>,
    pub acked: u32,
    pub snapshots_sent: u64,
    pub snapshot_bytes: u64,
    pub oversize_drops: u64,
    pub send_failures: u64,
    pub malformed: u32,
    pub last_udp_tx: u64,
    pub last_udp_rx: u64,
}

impl Session {
    pub fn new(
        id: EntityId,
        name: String,
        conn: quinn::Connection,
        control: mpsc::Sender<Control>,
    ) -> Session {
        Session {
            id,
            name,
            conn,
            control,
            history: VecDeque::with_capacity(HISTORY),
            acked: 0,
            snapshots_sent: 0,
            snapshot_bytes: 0,
            oversize_drops: 0,
            send_failures: 0,
            malformed: 0,
            last_udp_tx: 0,
            last_udp_rx: 0,
        }
    }

    /// Queue a reliable message; `false` when the client is not draining its stream.
    pub fn send_control(&self, msg: Control) -> bool {
        self.control.try_send(msg).is_ok()
    }

    /// Record an ack; anything we did not send (or no longer hold) counts as no ack.
    pub fn on_ack(&mut self, ack: u32) {
        self.acked = if ack != 0 && self.history.iter().any(|s| s.server_tick == ack) {
            ack
        } else {
            0
        };
    }

    fn baseline(&self, tick: u32) -> Option<&Snapshot> {
        if self.acked == 0 {
            return None;
        }
        self.history
            .iter()
            .find(|s| s.server_tick == self.acked)
            .filter(|s| tick.wrapping_sub(s.server_tick) as i32 <= MAX_BASELINE_AGE)
    }

    /// Build this tick's snapshot for the client: PVS filter, distance bands, carry-forward of
    /// unscheduled entities, delta encoding, and the datagram budget. `None` when the client's
    /// entity is gone.
    pub fn build_snapshot(
        &mut self,
        zone: &Zone,
        world: &ZoneWorld,
        pvs: &mut PvsCache,
        tick: u32,
    ) -> Option<Vec<u8>> {
        let me = zone.player(self.id)?;
        let eye = me.mover.eye();
        let my_leaf = world.bsp.leaf_for_point(eye);
        let see_all = my_leaf == 0;
        let row: &[u8] = if see_all {
            &[]
        } else {
            pvs.row(&world.bsp, my_leaf)
        };
        let visible = |origin: Vec3, top: Vec3| -> bool {
            if see_all {
                return true;
            }
            let bsp = &world.bsp;
            Bsp::leaf_in_pvs(row, bsp.leaf_for_point(origin))
                || Bsp::leaf_in_pvs(row, bsp.leaf_for_point(top))
        };

        let baseline_tick = self.baseline(tick).map_or(0, |b| b.server_tick);
        let baseline = self.baseline(tick).cloned();
        let base = baseline.as_ref();
        let mut snap = Snapshot::new(tick);
        snap.baseline_tick = baseline_tick;
        snap.last_input_tick = me.last_input_tick;
        // (id, distance) of droppable (other player) records for the size budget.
        let mut droppable: Vec<(EntityId, f32)> = Vec::new();

        for p in zone.players() {
            if p.id == self.id {
                snap.entities.push(player_state(p, true));
                continue;
            }
            let dist = (p.mover.mv.origin - eye).length();
            if dist > TOUCH_DIST && !visible(p.mover.mv.origin, p.mover.eye()) {
                continue;
            }
            let base_rec = base.and_then(|b| b.find(p.id));
            let scheduled = base_rec.is_none() || band_scheduled(tick, p.id, dist);
            let rec = if scheduled {
                player_state(p, false)
            } else {
                *base_rec.expect("unscheduled implies a baseline record")
            };
            droppable.push((p.id, dist));
            snap.entities.push(rec);
        }
        for pr in zone.projectiles() {
            if !visible(pr.pos, pr.pos) {
                continue;
            }
            snap.entities.push(projectile_state(pr));
        }
        snap.normalize(base);

        let mut bytes = snap.encode(base);
        droppable.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut drop_iter = droppable.into_iter();
        while bytes.len() > MAX_DATAGRAM_PAYLOAD {
            // Drop enough of the farthest records at once (about 6 bytes each) to fit, then
            // re-encode once; loop only if the estimate was short.
            let excess = bytes.len() - MAX_DATAGRAM_PAYLOAD;
            let mut to_drop = excess / 6 + 1;
            let mut dropped_any = false;
            while to_drop > 0 {
                let Some((id, _)) = drop_iter.next() else {
                    break;
                };
                let idx = snap
                    .entities
                    .iter()
                    .position(|e| e.id == id)
                    .expect("droppable entity is listed");
                match base.and_then(|b| b.find(id)) {
                    // Carry the stale record forward: costs nothing on the wire.
                    Some(rec) => snap.entities[idx] = *rec,
                    // Never seen: leave it out entirely; it is first-sight again next tick.
                    None => {
                        snap.entities.remove(idx);
                    }
                }
                self.oversize_drops += 1;
                to_drop -= 1;
                dropped_any = true;
            }
            if !dropped_any {
                break;
            }
            bytes = snap.encode(base);
        }

        self.history.push_back(snap);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.snapshots_sent += 1;
        self.snapshot_bytes += bytes.len() as u64;
        Some(bytes)
    }
}

/// Whether an entity in a distance band is listed this tick (PROTOCOL.md 5).
pub fn band_scheduled(tick: u32, id: EntityId, dist: f32) -> bool {
    if dist <= FULL_RATE_DIST {
        true
    } else if dist <= HALF_RATE_DIST {
        (tick.wrapping_add(id)).is_multiple_of(2)
    } else {
        (tick.wrapping_add(id)).is_multiple_of(6)
    }
}

pub fn frame_index(frame: ArchetypeFrame) -> u8 {
    match frame {
        ArchetypeFrame::Colossus => 0,
        ArchetypeFrame::Striker => 1,
        ArchetypeFrame::Caster => 2,
        ArchetypeFrame::Infiltrator => 3,
    }
}

pub fn frame_from_index(i: u8) -> ArchetypeFrame {
    match i {
        0 => ArchetypeFrame::Colossus,
        2 => ArchetypeFrame::Caster,
        3 => ArchetypeFrame::Infiltrator,
        _ => ArchetypeFrame::Striker,
    }
}

/// Wire state of a player; `own` adds velocity, health and the jump flag (PROTOCOL.md 5).
pub fn player_state(p: &Player, own: bool) -> EntityState {
    let mut f = 0u8;
    if p.alive {
        f |= flags::ALIVE;
    }
    if p.mover.mv.on_ground {
        f |= flags::ON_GROUND;
    }
    if p.mover.dash.is_some() {
        f |= flags::DASHING;
    }
    if own && p.mover.mv.jump_held {
        f |= flags::JUMP_HELD;
    }
    EntityState {
        id: p.id,
        spawn: SpawnInfo::Player {
            frame: frame_index(p.frame),
        },
        pos: quant::quantize_pos3(p.mover.mv.origin.into()),
        yaw: quant::yaw_to_wire(p.mover.yaw),
        pitch: quant::pitch_to_wire(p.mover.pitch),
        vel: own.then(|| quant::quantize_vel3(p.mover.mv.velocity.into())),
        anim: p.anim,
        health: own.then_some(p.health.clamp(0, u16::MAX as i32) as u16),
        flags: f,
    }
}

pub fn projectile_state(pr: &Projectile) -> EntityState {
    let dir = pr.vel.normalize_or_zero();
    let yaw = dir.y.atan2(dir.x).to_degrees();
    let pitch = (-dir.z).asin().to_degrees();
    EntityState {
        id: pr.id,
        spawn: SpawnInfo::Projectile {
            owner: pr.owner,
            def: pr.ability as u32,
            input_tick: pr.input_tick,
        },
        pos: quant::quantize_pos3(pr.pos.into()),
        yaw: quant::yaw_to_wire(yaw),
        pitch: quant::pitch_to_wire(pitch),
        vel: None,
        anim: 0,
        health: None,
        flags: flags::ALIVE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_schedule_by_distance() {
        assert!(band_scheduled(1, 1, 100.0));
        assert!(band_scheduled(2, 1, 100.0));
        let half: Vec<bool> = (0..4).map(|t| band_scheduled(t, 1, 1000.0)).collect();
        assert_eq!(half, [false, true, false, true]);
        let far = (0..12).filter(|&t| band_scheduled(t, 1, 3000.0)).count();
        assert_eq!(far, 2);
    }
}
