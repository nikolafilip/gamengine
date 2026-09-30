//! Delta-compressed snapshots (PROTOCOL.md 5).

use crate::bits::{BitReader, BitWriter};
use crate::{Kind, NetError, read_header, write_header};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EntityKind {
    Player = 0,
    Projectile = 1,
}

/// First-sight information, by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpawnInfo {
    Player {
        /// Archetype frame: 0 colossus, 1 striker, 2 caster, 3 infiltrator.
        frame: u8,
    },
    Projectile {
        owner: u32,
        /// Index into the kit's projectile definitions.
        def: u32,
        /// The owner's client tick that fired it.
        input_tick: u32,
    },
}

impl SpawnInfo {
    pub fn kind(&self) -> EntityKind {
        match self {
            SpawnInfo::Player { .. } => EntityKind::Player,
            SpawnInfo::Projectile { .. } => EntityKind::Projectile,
        }
    }
}

/// Entity flag bits (PROTOCOL.md 5).
pub mod flags {
    pub const ALIVE: u8 = 1 << 0;
    pub const ON_GROUND: u8 = 1 << 1;
    pub const GUARDING: u8 = 1 << 2;
    pub const DASHING: u8 = 1 << 3;
    pub const JUMP_HELD: u8 = 1 << 4;
}

mod mask {
    pub const SPAWN: u8 = 1 << 0;
    pub const POS: u8 = 1 << 1;
    pub const YAW: u8 = 1 << 2;
    pub const PITCH: u8 = 1 << 3;
    pub const VEL: u8 = 1 << 4;
    pub const ANIM: u8 = 1 << 5;
    pub const HEALTH: u8 = 1 << 6;
    pub const FLAGS: u8 = 1 << 7;
}

/// One entity as seen by one client at one tick, in wire units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityState {
    pub id: u32,
    pub spawn: SpawnInfo,
    /// Quantized position (1/4 u).
    pub pos: [i32; 3],
    pub yaw: u16,
    pub pitch: u16,
    /// Quantized velocity (1/8 u/s); own entity only.
    pub vel: Option<[i32; 3]>,
    pub anim: u8,
    /// Own entity (Phase 5: party) only.
    pub health: Option<u16>,
    pub flags: u8,
}

/// The **reconstructed** entity table at one tick (PROTOCOL.md 5): what the server tracks per
/// client and what the client rebuilds. Encoding writes only the records that differ from the
/// baseline; decoding carries the rest forward.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub server_tick: u32,
    /// 0 = full snapshot.
    pub baseline_tick: u32,
    pub last_input_tick: u32,
    /// Every entity the client knows after this snapshot, ascending by id.
    pub entities: Vec<EntityState>,
    /// Ids present in the baseline and gone now.
    pub removed: Vec<u32>,
}

impl Snapshot {
    pub fn new(server_tick: u32) -> Self {
        Snapshot {
            server_tick,
            ..Default::default()
        }
    }

    pub fn find(&self, id: u32) -> Option<&EntityState> {
        self.entities
            .binary_search_by_key(&id, |e| e.id)
            .ok()
            .map(|i| &self.entities[i])
    }

    /// Sort entities by id and fill `removed` from `baseline`; call after building the table.
    pub fn normalize(&mut self, baseline: Option<&Snapshot>) {
        self.entities.sort_by_key(|e| e.id);
        let removed: Vec<u32> = baseline.map_or_else(Vec::new, |b| {
            b.entities
                .iter()
                .map(|e| e.id)
                .filter(|&id| self.find(id).is_none())
                .collect()
        });
        self.removed = removed;
    }

    /// Encode against `baseline`, which must be the snapshot at `self.baseline_tick` (or
    /// `None` when that is 0). Entities must be ascending by id and `removed` must list every
    /// baseline entity missing from `entities` (see `normalize`). Records identical to their
    /// baseline record are not written at all.
    pub fn encode(&self, baseline: Option<&Snapshot>) -> Vec<u8> {
        debug_assert_eq!(
            baseline.map_or(0, |b| b.server_tick),
            self.baseline_tick,
            "baseline mismatch"
        );
        debug_assert!(self.entities.windows(2).all(|w| w[0].id < w[1].id));
        let mut w = BitWriter::with_capacity(32 + self.entities.len() * 8);
        write_header(&mut w, Kind::Snapshot);
        w.write_bits(self.server_tick as u64, 32);
        w.write_bits(self.baseline_tick as u64, 32);
        w.write_bits(self.last_input_tick as u64, 32);
        let changed: Vec<(&EntityState, Option<&EntityState>)> = self
            .entities
            .iter()
            .map(|e| (e, baseline.and_then(|b| b.find(e.id))))
            .filter(|(e, base)| base.is_none_or(|b| record_differs(e, b)))
            .collect();
        w.write_uvar(changed.len() as u64);
        for (e, base) in changed {
            write_entity(&mut w, e, base);
        }
        w.write_uvar(self.removed.len() as u64);
        for &id in &self.removed {
            w.write_uvar(id as u64);
        }
        w.finish()
    }

    /// Bytes `encode` would produce, without allocating the payload twice.
    pub fn encoded_len(&self, baseline: Option<&Snapshot>) -> usize {
        self.encode(baseline).len()
    }

    /// Decode; `baseline` resolves a tick to the snapshot decoded earlier.
    pub fn decode<'a, F>(bytes: &[u8], baseline: F) -> Result<Snapshot, NetError>
    where
        F: Fn(u32) -> Option<&'a Snapshot>,
    {
        let mut r = BitReader::new(bytes);
        if read_header(&mut r)? != Kind::Snapshot {
            return Err(NetError::Malformed("not a snapshot datagram"));
        }
        let server_tick = r.read_bits(32)? as u32;
        let baseline_tick = r.read_bits(32)? as u32;
        let base = if baseline_tick == 0 {
            None
        } else {
            Some(baseline(baseline_tick).ok_or(NetError::UnknownBaseline(baseline_tick))?)
        };
        let last_input_tick = r.read_bits(32)? as u32;
        let count = r.read_uvar32()? as usize;
        if count > 4096 {
            return Err(NetError::Malformed("too many entities"));
        }
        let mut listed = Vec::with_capacity(count);
        let mut prev: Option<u32> = None;
        for _ in 0..count {
            let id = r.read_uvar32()?;
            if prev.is_some_and(|p| id <= p) {
                return Err(NetError::Malformed("entity ids not ascending"));
            }
            prev = Some(id);
            let base_e = base.and_then(|b| b.find(id));
            listed.push(read_entity(&mut r, id, base_e)?);
        }
        let removed_count = r.read_uvar32()? as usize;
        if removed_count > 4096 {
            return Err(NetError::Malformed("too many removals"));
        }
        let mut removed = Vec::with_capacity(removed_count);
        for _ in 0..removed_count {
            removed.push(r.read_uvar32()?);
        }
        // Reconstruction: baseline minus removed, listed records applied, the rest carried
        // forward unchanged.
        let mut entities: Vec<EntityState> = match base {
            Some(b) => b
                .entities
                .iter()
                .filter(|e| !removed.contains(&e.id))
                .copied()
                .collect(),
            None => Vec::new(),
        };
        for e in listed {
            match entities.binary_search_by_key(&e.id, |x| x.id) {
                Ok(i) => entities[i] = e,
                Err(i) => entities.insert(i, e),
            }
        }
        Ok(Snapshot {
            server_tick,
            baseline_tick,
            last_input_tick,
            entities,
            removed,
        })
    }
}

fn record_differs(e: &EntityState, b: &EntityState) -> bool {
    e.spawn != b.spawn
        || e.pos != b.pos
        || e.yaw != b.yaw
        || e.pitch != b.pitch
        || (e.vel.is_some() && e.vel != b.vel)
        || e.anim != b.anim
        || (e.health.is_some() && e.health != b.health)
        || e.flags != b.flags
}

fn write_i32x3(w: &mut BitWriter, v: [i32; 3], base: Option<[i32; 3]>) {
    for i in 0..3 {
        let d = base.map_or(v[i], |b| v[i].wrapping_sub(b[i]));
        w.write_svar(d as i64);
    }
}

fn read_i32x3(r: &mut BitReader<'_>, base: Option<[i32; 3]>) -> Result<[i32; 3], NetError> {
    let mut out = [0i32; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let d = r.read_svar32()?;
        *o = base.map_or(d, |b| b[i].wrapping_add(d));
    }
    Ok(out)
}

fn write_entity(w: &mut BitWriter, e: &EntityState, base: Option<&EntityState>) {
    w.write_uvar(e.id as u64);
    let mut m = 0u8;
    match base {
        None => {
            m |= mask::SPAWN | mask::POS | mask::YAW | mask::PITCH | mask::ANIM | mask::FLAGS;
            if e.vel.is_some() {
                m |= mask::VEL;
            }
            if e.health.is_some() {
                m |= mask::HEALTH;
            }
        }
        Some(b) => {
            if e.pos != b.pos {
                m |= mask::POS;
            }
            if e.yaw != b.yaw {
                m |= mask::YAW;
            }
            if e.pitch != b.pitch {
                m |= mask::PITCH;
            }
            if e.vel.is_some() && e.vel != b.vel {
                m |= mask::VEL;
            }
            if e.anim != b.anim {
                m |= mask::ANIM;
            }
            if e.health.is_some() && e.health != b.health {
                m |= mask::HEALTH;
            }
            if e.flags != b.flags {
                m |= mask::FLAGS;
            }
        }
    }
    w.write_bits(m as u64, 8);
    if m & mask::SPAWN != 0 {
        w.write_bits(e.spawn.kind() as u64, 4);
        match e.spawn {
            SpawnInfo::Player { frame } => w.write_bits(frame as u64, 2),
            SpawnInfo::Projectile {
                owner,
                def,
                input_tick,
            } => {
                w.write_uvar(owner as u64);
                w.write_uvar(def as u64);
                w.write_uvar(input_tick as u64);
            }
        }
    }
    if m & mask::POS != 0 {
        write_i32x3(w, e.pos, base.map(|b| b.pos));
    }
    if m & mask::YAW != 0 {
        w.write_bits(e.yaw as u64, 12);
    }
    if m & mask::PITCH != 0 {
        w.write_bits(e.pitch as u64, 11);
    }
    if m & mask::VEL != 0 {
        write_i32x3(w, e.vel.unwrap_or([0; 3]), base.and_then(|b| b.vel));
    }
    if m & mask::ANIM != 0 {
        w.write_bits(e.anim as u64, 8);
    }
    if m & mask::HEALTH != 0 {
        w.write_uvar(e.health.unwrap_or(0) as u64);
    }
    if m & mask::FLAGS != 0 {
        w.write_bits(e.flags as u64, 8);
    }
}

fn read_entity(
    r: &mut BitReader<'_>,
    id: u32,
    base: Option<&EntityState>,
) -> Result<EntityState, NetError> {
    let m = r.read_bits(8)? as u8;
    let spawn = if m & mask::SPAWN != 0 {
        match r.read_bits(4)? {
            0 => SpawnInfo::Player {
                frame: r.read_bits(2)? as u8,
            },
            1 => SpawnInfo::Projectile {
                owner: r.read_uvar32()?,
                def: r.read_uvar32()?,
                input_tick: r.read_uvar32()?,
            },
            _ => return Err(NetError::Malformed("unknown entity kind")),
        }
    } else {
        base.ok_or(NetError::Malformed(
            "delta record without a baseline record",
        ))?
        .spawn
    };
    // With SPAWN set, deltas are absolute: the baseline record is ignored for this entity.
    let base = if m & mask::SPAWN != 0 { None } else { base };
    let mut e = match base {
        Some(b) => EntityState { id, ..*b },
        None => EntityState {
            id,
            spawn,
            pos: [0; 3],
            yaw: 0,
            pitch: 0,
            vel: None,
            anim: 0,
            health: None,
            flags: 0,
        },
    };
    e.spawn = spawn;
    if m & mask::POS != 0 {
        e.pos = read_i32x3(r, base.map(|b| b.pos))?;
    }
    if m & mask::YAW != 0 {
        e.yaw = r.read_bits(12)? as u16;
        if e.yaw >= 3600 {
            return Err(NetError::Malformed("yaw out of range"));
        }
    }
    if m & mask::PITCH != 0 {
        e.pitch = r.read_bits(11)? as u16;
        if e.pitch > 1800 {
            return Err(NetError::Malformed("pitch out of range"));
        }
    }
    if m & mask::VEL != 0 {
        e.vel = Some(read_i32x3(r, base.and_then(|b| b.vel))?);
    }
    if m & mask::ANIM != 0 {
        e.anim = r.read_bits(8)? as u8;
    }
    if m & mask::HEALTH != 0 {
        let h = r.read_uvar()?;
        e.health = Some(u16::try_from(h).map_err(|_| NetError::Malformed("health too large"))?);
    }
    if m & mask::FLAGS != 0 {
        e.flags = r.read_bits(8)? as u8;
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(id: u32, x: i32, own: bool) -> EntityState {
        EntityState {
            id,
            spawn: SpawnInfo::Player { frame: 1 },
            pos: [x, -400, 96],
            yaw: 1234,
            pitch: 900,
            vel: own.then_some([1000, 0, -40]),
            anim: 2,
            health: own.then_some(100),
            flags: flags::ALIVE | flags::ON_GROUND,
        }
    }

    fn scene(tick: u32, shift: i32) -> Snapshot {
        let mut s = Snapshot::new(tick);
        s.entities.push(player(1, shift, true));
        for i in 2..17 {
            s.entities.push(player(i, i as i32 * 100 + shift, false));
        }
        s
    }

    #[test]
    fn full_snapshot_round_trips() {
        let s = scene(10, 0);
        let bytes = s.encode(None);
        let back = Snapshot::decode(&bytes, |_| None).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn delta_snapshot_is_small_and_exact() {
        let base = scene(10, 0);
        let mut next = scene(11, 20); // everyone moved 5 u in x
        next.baseline_tick = 10;
        next.last_input_tick = 77;
        next.entities[3].yaw = 1300;
        next.entities[5].flags = flags::ALIVE;
        let full = {
            let mut f = next.clone();
            f.baseline_tick = 0;
            f.encode(None).len()
        };
        let bytes = next.encode(Some(&base));
        // 16 entities: id (1) + mask (1) + x delta (1) + two zero deltas (2) ≈ 5 bytes each,
        // plus the own entity's velocity and a couple of changed fields, plus 14 bytes of header.
        assert!(bytes.len() < 110, "delta is {} bytes", bytes.len());
        assert!(bytes.len() < full, "delta {} >= full {}", bytes.len(), full);
        let back = Snapshot::decode(&bytes, |t| (t == 10).then_some(&base)).unwrap();
        assert_eq!(back, next);
    }

    #[test]
    fn unchanged_entities_cost_nothing_and_are_carried_forward() {
        let base = scene(10, 0);
        let mut next = scene(11, 0);
        next.baseline_tick = 10;
        next.last_input_tick = 5;
        let bytes = next.encode(Some(&base));
        // header 2 + 3 x 4 + count 1 + removed 1 = 16
        assert_eq!(bytes.len(), 16);
        let back = Snapshot::decode(&bytes, |t| (t == 10).then_some(&base)).unwrap();
        assert_eq!(back, next);
        // One entity moves: only it is on the wire, the rest still come back.
        let mut moved = next.clone();
        moved.server_tick = 12;
        moved.baseline_tick = 10;
        moved.entities[7].pos[0] += 4;
        let bytes = moved.encode(Some(&base));
        assert!(bytes.len() <= 16 + 6, "{}", bytes.len());
        let back = Snapshot::decode(&bytes, |t| (t == 10).then_some(&base)).unwrap();
        assert_eq!(back, moved);
    }

    #[test]
    fn spawns_and_removals() {
        let base = scene(10, 0);
        let mut next = Snapshot::new(11);
        next.baseline_tick = 10;
        next.entities.push(player(1, 0, true));
        next.entities.push(EntityState {
            id: 40,
            spawn: SpawnInfo::Projectile {
                owner: 1,
                def: 2,
                input_tick: 4321,
            },
            pos: [10, 20, 30],
            yaw: 0,
            pitch: 0,
            vel: None,
            anim: 0,
            health: None,
            flags: flags::ALIVE,
        });
        next.normalize(Some(&base));
        assert_eq!(next.removed, (2..17).collect::<Vec<u32>>());
        let bytes = next.encode(Some(&base));
        let back = Snapshot::decode(&bytes, |t| (t == 10).then_some(&base)).unwrap();
        assert_eq!(back, next);
        assert_eq!(back.find(40).unwrap().spawn.kind(), EntityKind::Projectile);
    }

    #[test]
    fn unknown_baseline_and_bad_records_are_errors() {
        let base = scene(10, 0);
        let mut next = scene(11, 4);
        next.baseline_tick = 10;
        let bytes = next.encode(Some(&base));
        assert_eq!(
            Snapshot::decode(&bytes, |_| None),
            Err(NetError::UnknownBaseline(10))
        );
        // A delta record for an entity the baseline never had.
        let mut smaller = base.clone();
        smaller.entities.retain(|e| e.id != 5);
        assert!(matches!(
            Snapshot::decode(&bytes, |_| Some(&smaller)),
            Err(NetError::Malformed(_))
        ));
        next.baseline_tick = 0;
        let mut cut = next.encode(None);
        cut.truncate(20);
        assert_eq!(Snapshot::decode(&cut, |_| None), Err(NetError::Overrun));
    }
}
