//! Stall grids (ECONOMY.md 7): where a map lets players open stalls. A `gm_stall_grid` entity
//! is a rectangle of tiles on the ground; a stall occupies exactly one tile, so stalls are
//! grid-snapped and cannot overlap by construction. The zone, the bots and the tools read the
//! same entity.

use glam::Vec3;

use crate::Bsp;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StallGrid {
    /// Centre of the first tile, on the ground.
    pub origin: Vec3,
    pub cols: i32,
    pub rows: i32,
    /// Distance between tile centres; columns run along +X, rows along +Y.
    pub pitch: f32,
    /// Side of a tile.
    pub tile: f32,
    /// Yaw the stall keepers face.
    pub yaw: f32,
    /// Tile number of the first column and row, so several grids of a map never collide.
    pub base_x: i32,
    pub base_y: i32,
}

impl StallGrid {
    /// The tile whose square contains `point` (its height is ignored).
    pub fn tile_at(&self, point: Vec3) -> Option<(i32, i32)> {
        let col = ((point.x - self.origin.x) / self.pitch).round() as i32;
        let row = ((point.y - self.origin.y) / self.pitch).round() as i32;
        if col < 0 || col >= self.cols || row < 0 || row >= self.rows {
            return None;
        }
        let centre = self.centre(self.base_x + col, self.base_y + row)?;
        let half = self.tile * 0.5;
        ((point.x - centre.x).abs() <= half && (point.y - centre.y).abs() <= half)
            .then_some((self.base_x + col, self.base_y + row))
    }

    /// Centre of tile `(x, y)` on the ground, when it is one of this grid's.
    pub fn centre(&self, x: i32, y: i32) -> Option<Vec3> {
        let (col, row) = (x - self.base_x, y - self.base_y);
        (col >= 0 && col < self.cols && row >= 0 && row < self.rows)
            .then(|| self.origin + Vec3::new(col as f32 * self.pitch, row as f32 * self.pitch, 0.0))
    }

    pub fn tiles(&self) -> usize {
        (self.cols * self.rows) as usize
    }

    /// Do the two grids share a tile number? (A map's grids must not: a stall is named by
    /// its zone and its tile.)
    pub fn numbers_overlap(&self, other: &StallGrid) -> bool {
        let apart = |a0: i32, an: i32, b0: i32, bn: i32| a0 + an <= b0 || b0 + bn <= a0;
        !(apart(self.base_x, self.cols, other.base_x, other.cols)
            || apart(self.base_y, self.rows, other.base_y, other.rows))
    }
}

/// The most stall tiles one map may have: every open stall of a zone is told to a joiner in
/// one reliable message (PROTOCOL.md 8), and this many fit it.
pub const MAX_STALL_TILES: usize = 512;

impl Bsp {
    /// The `gm_stall_grid`s of the map, in entity order. A grid that would take the map past
    /// `MAX_STALL_TILES`, or whose tile numbers collide with an earlier grid's, is left out:
    /// the invariants (one stall per tile, the market in one message) hold for any map.
    pub fn stall_grids(&self) -> Vec<StallGrid> {
        let mut grids: Vec<StallGrid> = Vec::new();
        let mut tiles = 0usize;
        for e in self
            .entities
            .iter()
            .filter(|e| e.classname() == "gm_stall_grid")
        {
            let int = |key: &str, default: i32| e.f32(key).map_or(default, |v| v as i32);
            let Some(origin) = e.origin() else {
                continue;
            };
            let pitch = e.f32("pitch").unwrap_or(160.0).clamp(16.0, 4096.0);
            let grid = StallGrid {
                origin,
                cols: int("cols", 1).clamp(1, 256),
                rows: int("rows", 1).clamp(1, 256),
                pitch,
                // A tile wider than the pitch would overlap its neighbours.
                tile: e.f32("tile").unwrap_or(128.0).clamp(16.0, pitch),
                yaw: e.f32("angle").unwrap_or(0.0),
                base_x: int("base_x", 0).clamp(-1_000_000, 1_000_000),
                base_y: int("base_y", 0).clamp(-1_000_000, 1_000_000),
            };
            if tiles + grid.tiles() > MAX_STALL_TILES
                || grids.iter().any(|g| g.numbers_overlap(&grid))
            {
                continue;
            }
            tiles += grid.tiles();
            grids.push(grid);
        }
        grids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> StallGrid {
        StallGrid {
            origin: Vec3::new(112.0, -320.0, 0.0),
            cols: 6,
            rows: 5,
            pitch: 160.0,
            tile: 128.0,
            yaw: 180.0,
            base_x: 10,
            base_y: 0,
        }
    }

    #[test]
    fn a_point_is_on_at_most_one_tile() {
        let g = grid();
        assert_eq!(g.tile_at(Vec3::new(112.0, -320.0, 24.0)), Some((10, 0)));
        assert_eq!(
            g.tile_at(Vec3::new(112.0 + 60.0, -320.0 - 60.0, 0.0)),
            Some((10, 0))
        );
        // The lane between two tiles belongs to neither.
        assert_eq!(g.tile_at(Vec3::new(112.0 + 80.0, -320.0, 0.0)), None);
        assert_eq!(
            g.tile_at(Vec3::new(112.0 + 160.0, -320.0 + 320.0, 0.0)),
            Some((11, 2))
        );
        // A second grid must use other tile numbers.
        let mut other = g;
        other.base_x = 15;
        assert!(g.numbers_overlap(&other));
        other.base_x = 16;
        assert!(!g.numbers_overlap(&other));
        // Outside the grid.
        assert_eq!(g.tile_at(Vec3::new(0.0, -320.0, 0.0)), None);
        assert_eq!(g.tile_at(Vec3::new(112.0 + 6.0 * 160.0, -320.0, 0.0)), None);
        assert_eq!(g.tile_at(Vec3::new(112.0, -320.0 + 5.0 * 160.0, 0.0)), None);
        // Centres round-trip, and tiles outside are not this grid's.
        for x in 10..16 {
            for y in 0..5 {
                let c = g.centre(x, y).unwrap();
                assert_eq!(g.tile_at(c), Some((x, y)));
            }
        }
        assert_eq!(g.centre(9, 0), None);
        assert_eq!(g.centre(16, 0), None);
        assert_eq!(g.tiles(), 30);
    }
}
