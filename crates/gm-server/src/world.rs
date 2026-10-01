//! The zone's map: BSP for collision and PVS, spawn points, and the hash clients must match.

use std::path::Path;

use gm_bsp::Bsp;
use gm_core::sim::Spawn;
use gm_net::transport::fnv1a64;

pub struct ZoneWorld {
    pub bsp: Bsp,
    /// Map name sent in `Welcome` (the file stem).
    pub name: String,
    /// FNV-1a 64 of the `.bsp` bytes (PROTOCOL.md 8).
    pub hash: u64,
    /// Every `info_player_start` (team 0) and `gm_spawn` (its `team` key, 0 = any).
    pub spawns: Vec<Spawn>,
}

impl ZoneWorld {
    pub fn load(path: &Path) -> anyhow::Result<ZoneWorld> {
        let bytes =
            std::fs::read(path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let bsp =
            Bsp::load(path).map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "map".into());
        Ok(ZoneWorld::from_bsp(bsp, &name, fnv1a64(&bytes)))
    }

    pub fn from_bsp(bsp: Bsp, name: &str, hash: u64) -> ZoneWorld {
        let spawns = bsp
            .entities
            .iter()
            .filter(|e| matches!(e.classname(), "info_player_start" | "gm_spawn"))
            .filter_map(|e| {
                Some(Spawn {
                    origin: e.origin()?,
                    yaw: e.f32("angle").unwrap_or(0.0),
                    team: e.f32("team").unwrap_or(0.0) as u8,
                })
            })
            .collect();
        ZoneWorld {
            bsp,
            name: name.to_string(),
            hash,
            spawns,
        }
    }
}
