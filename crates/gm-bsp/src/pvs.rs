//! Potentially visible set: which leaves can see which. The server culls snapshots with it
//! (PLAN.md 1.2, 8) and the client culls draw lists with it.

use glam::Vec3;

use crate::Bsp;

impl Bsp {
    /// Bytes in one decompressed PVS row. Bit `i` of a row refers to leaf `i + 1`; leaf 0 is
    /// the shared solid leaf and is never visible.
    pub fn pvs_row_bytes(&self) -> usize {
        self.leaves.len().div_ceil(8)
    }

    /// Decompress the PVS row of `leaf`. Run-length zeros: a zero byte is followed by a count of
    /// zero bytes. Leaves without vis data see everything, as in Quake.
    pub fn decompress_pvs(&self, leaf: usize) -> Vec<u8> {
        let row = self.pvs_row_bytes();
        let ofs = self.leaves[leaf].vis_ofs;
        if ofs < 0 || self.visdata.is_empty() {
            return vec![0xff; row];
        }
        let mut out = Vec::with_capacity(row);
        let mut i = ofs as usize;
        while out.len() < row {
            let Some(&b) = self.visdata.get(i) else {
                break;
            };
            if b != 0 {
                out.push(b);
                i += 1;
                continue;
            }
            let count = self.visdata.get(i + 1).copied().unwrap_or(0) as usize;
            i += 2;
            let n = count.min(row - out.len());
            out.extend(std::iter::repeat_n(0u8, n));
            if count == 0 {
                break;
            }
        }
        out.resize(row, 0);
        out
    }

    /// Whether `leaf` is marked visible in a decompressed row.
    pub fn leaf_in_pvs(row: &[u8], leaf: usize) -> bool {
        if leaf == 0 {
            return false;
        }
        let i = leaf - 1;
        row.get(i >> 3).is_some_and(|b| b & (1 << (i & 7)) != 0)
    }

    /// Leaf containing `point` in the world model.
    pub fn leaf_for_point(&self, point: Vec3) -> usize {
        self.leaf_for_point_in_model(0, point)
    }

    pub fn leaf_for_point_in_model(&self, model: usize, point: Vec3) -> usize {
        let mut num = self.models[model].head_nodes[0];
        loop {
            if num < 0 {
                return (-1 - num) as usize;
            }
            let node = &self.nodes[num as usize];
            let plane = &self.planes[node.plane as usize];
            let d = plane.normal.dot(point) - plane.dist;
            num = node.children[if d >= 0.0 { 0 } else { 1 }];
        }
    }

    /// World faces potentially visible from `leaf`, each listed once.
    pub fn visible_faces(&self, leaf: usize) -> Vec<u32> {
        let row = self.decompress_pvs(leaf);
        let mut seen = vec![false; self.faces.len()];
        let mut out = Vec::new();
        for (i, l) in self.leaves.iter().enumerate() {
            if !Self::leaf_in_pvs(&row, i) {
                continue;
            }
            let ms = &self.marksurfaces
                [l.first_marksurface as usize..(l.first_marksurface + l.num_marksurfaces) as usize];
            for &f in ms {
                if !seen[f as usize] {
                    seen[f as usize] = true;
                    out.push(f);
                }
            }
        }
        out
    }

    /// Count of leaves visible from `leaf` (diagnostics).
    pub fn visible_leaf_count(&self, leaf: usize) -> usize {
        let row = self.decompress_pvs(leaf);
        (1..self.leaves.len())
            .filter(|&i| Self::leaf_in_pvs(&row, i))
            .count()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn run_length_zero_expansion() {
        // Emulate the decompressor on a standalone buffer: 0xAB, then 3 zero bytes, then 0x01.
        let visdata = [0xABu8, 0x00, 0x03, 0x01];
        let row = 5usize;
        let mut out = Vec::new();
        let mut i = 0;
        while out.len() < row {
            let b = visdata[i];
            if b != 0 {
                out.push(b);
                i += 1;
            } else {
                let count = visdata[i + 1] as usize;
                i += 2;
                out.extend(std::iter::repeat_n(0u8, count.min(row - out.len())));
            }
        }
        assert_eq!(out, vec![0xAB, 0, 0, 0, 0x01]);
    }
}
