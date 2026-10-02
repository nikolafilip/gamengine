//! BC1 (DXT1) decoding: the client's path on a GPU without BC support (MODELS.md 8), and how
//! previews and tests look at an ingested texture. Encoding lives in `gm-ingest`.

/// The four colours of a block as RGBA.
fn palette(block: &[u8]) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let expand = |c: u16| -> [u32; 3] {
        let (r, g, b) = ((c >> 11) as u32, ((c >> 5) & 63) as u32, (c & 31) as u32);
        [
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
        ]
    };
    let (a, b) = (expand(c0), expand(c1));
    let mix = |wa: u32, wb: u32, div: u32| -> [u8; 4] {
        [
            ((wa * a[0] + wb * b[0]) / div) as u8,
            ((wa * a[1] + wb * b[1]) / div) as u8,
            ((wa * a[2] + wb * b[2]) / div) as u8,
            255,
        ]
    };
    if c0 > c1 {
        [mix(1, 0, 1), mix(0, 1, 1), mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [mix(1, 0, 1), mix(0, 1, 1), mix(1, 1, 2), [0, 0, 0, 0]]
    }
}

/// Decode one level of BC1 blocks to RGBA8. `blocks` must hold
/// `ceil(w / 4) × ceil(h / 4) × 8` bytes; a short buffer decodes as far as it goes.
pub fn decode(blocks: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let bw = w.div_ceil(4);
    for (i, block) in blocks.chunks_exact(8).enumerate() {
        let (bx, by) = (i % bw, i / bw);
        if by * 4 >= h {
            break;
        }
        let pal = palette(block);
        let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        for ty in 0..4 {
            for tx in 0..4 {
                let (x, y) = (bx * 4 + tx, by * 4 + ty);
                if x < w && y < h {
                    let idx = (bits >> (2 * (ty * 4 + tx))) & 3;
                    out[(y * w + x) * 4..][..4].copy_from_slice(&pal[idx as usize]);
                }
            }
        }
    }
    out
}

/// One texel of a BC1 level, clamped to the edges.
pub fn texel(blocks: &[u8], w: usize, h: usize, x: usize, y: usize) -> [u8; 4] {
    let (x, y) = (x.min(w.saturating_sub(1)), y.min(h.saturating_sub(1)));
    let i = (y / 4) * w.div_ceil(4) + x / 4;
    let Some(block) = blocks.get(i * 8..i * 8 + 8) else {
        return [0, 0, 0, 0];
    };
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    palette(block)[((bits >> (2 * ((y % 4) * 4 + x % 4))) & 3) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_colour_and_cutout_blocks_decode() {
        // c0 = pure red (0xF800) > c1 = pure blue (0x001F): four opaque colours.
        let mut block = [0x00, 0xF8, 0x1F, 0x00, 0, 0, 0, 0];
        // Texel 0 → index 0 (red), texel 1 → index 1 (blue), texel 2 → 2, texel 3 → 3.
        block[4] = 0b11_10_01_00;
        let px = decode(&block, 4, 4);
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[4..8], &[0, 0, 255, 255]);
        assert_eq!(&px[8..12], &[170, 0, 85, 255]);
        assert_eq!(&px[12..16], &[85, 0, 170, 255]);
        assert_eq!(texel(&block, 4, 4, 1, 0), [0, 0, 255, 255]);
        // c0 <= c1: three colours and a transparent one.
        let mut cut = [0x1F, 0x00, 0x00, 0xF8, 0, 0, 0, 0];
        cut[4] = 0b11_10_01_00;
        let px = decode(&cut, 4, 4);
        assert_eq!(&px[0..4], &[0, 0, 255, 255]);
        assert_eq!(&px[8..12], &[127, 0, 127, 255]);
        assert_eq!(&px[12..16], &[0, 0, 0, 0]);
        // Clamped lookups and short buffers do not panic.
        assert_eq!(texel(&cut, 4, 4, 99, 99), texel(&cut, 4, 4, 3, 3));
        assert_eq!(texel(&[], 4, 4, 0, 0), [0, 0, 0, 0]);
        assert_eq!(decode(&[], 8, 8).len(), 256);
    }
}
