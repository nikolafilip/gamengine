//! The client's one texture of pictures (`ui.gma`, LOOK.md 2.2): the skin's pieces with
//! their nine-slice insets, the glyphs of the faces the tool rasterised, and the icons,
//! all in one RGBA8 image, one zlib stream. Written by `gm-tools content build`
//! (gm-ingest assembles it), read here in one call. The reader trusts nothing: every rect
//! is inside the texture and every count is bounded before a byte of texels is touched.
//!
//! Names are looked up by a 32-bit FNV-1a hash of the key; the tool refuses a collision.

use std::collections::HashMap;

pub const MAGIC: [u8; 4] = *b"GMA1";
pub const MAX_SIDE: u16 = 1024;
pub const MAX_PIECES: usize = 512;
pub const MAX_FACES: usize = 4;
pub const MAX_GLYPHS: usize = 4096;
pub const MAX_ICONS: usize = 2048;
/// An icon's side in dots (CONTENT.md 4).
pub const ICON: u16 = 32;

const HEADER: usize = 4 + 2 + 2 + 2 + 1 + 2 + 4;
const PIECE_BYTES: usize = 4 + 2 * 4 + 4;
const GLYPH_BYTES: usize = 4 + 2 + 2 + 1 + 1 + 1 + 1 + 1;
const ICON_BYTES: usize = 4 + 2 + 2;

/// FNV-1a over the key's UTF-8: what the tables are keyed by.
pub fn hash(key: &str) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for b in key.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A rectangle of the texture, in texels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// A piece of the skin: its cell and the nine-slice insets (left, top, right, bottom; all
/// zero for a plain picture).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Piece {
    pub cell: Cell,
    pub inset: [u8; 4],
}

/// One glyph of a face: its cell, where it sits relative to the pen (bearing: right of the
/// pen, down from the line's top), and how far the pen moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Glyph {
    pub cell: Cell,
    pub bearing: [i8; 2],
    pub advance: u8,
}

/// A rasterised face: the line height and the ascent in dots, and its glyphs by char.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Face {
    pub line_height: u8,
    pub ascent: u8,
    pub glyphs: HashMap<char, Glyph>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Atlas {
    pub w: u16,
    pub h: u16,
    pub pieces: HashMap<u32, Piece>,
    /// By face id (LOOK.md 2.3: 0 the small five-by-seven face, written into the file
    /// from the shared table so one texture draws everything; 1 text; 2 title).
    pub faces: HashMap<u8, Face>,
    /// By key hash; every icon is `ICON` square.
    pub icons: HashMap<u32, Cell>,
    /// RGBA8, straight alpha, `w × h`.
    pub texels: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AtlasError {
    #[error("not a .gma file")]
    Magic,
    #[error("out of range: {0}")]
    Range(&'static str),
    #[error("truncated or padded: {0}")]
    Length(&'static str),
    #[error("the payload does not inflate to the declared length")]
    Inflate,
    #[error("two keys hash alike: {0}")]
    Collision(String),
}

impl Atlas {
    pub fn piece(&self, name: &str) -> Option<&Piece> {
        self.pieces.get(&hash(name))
    }

    pub fn icon(&self, key: &str) -> Option<Cell> {
        self.icons.get(&hash(key)).copied()
    }

    fn cell_fits(&self, c: Cell) -> bool {
        c.w > 0
            && c.h > 0
            && (c.x as u32 + c.w as u32) <= self.w as u32
            && (c.y as u32 + c.h as u32) <= self.h as u32
    }

    /// Everything the reader checks, checked on the writer's side too.
    pub fn validate(&self) -> Result<(), AtlasError> {
        if self.w == 0 || self.h == 0 || self.w > MAX_SIDE || self.h > MAX_SIDE {
            return Err(AtlasError::Range("size"));
        }
        if self.texels.len() != self.w as usize * self.h as usize * 4 {
            return Err(AtlasError::Length("texels"));
        }
        if self.pieces.len() > MAX_PIECES {
            return Err(AtlasError::Range("pieces"));
        }
        for p in self.pieces.values() {
            if !self.cell_fits(p.cell)
                || p.inset[0] as u16 + p.inset[2] as u16 > p.cell.w
                || p.inset[1] as u16 + p.inset[3] as u16 > p.cell.h
            {
                return Err(AtlasError::Range("a piece"));
            }
        }
        if self.faces.len() > MAX_FACES {
            return Err(AtlasError::Range("faces"));
        }
        for f in self.faces.values() {
            if f.glyphs.len() > MAX_GLYPHS || f.line_height == 0 {
                return Err(AtlasError::Range("a face"));
            }
            for g in f.glyphs.values() {
                // A glyph may be empty (a space): then its cell is nothing at all. Its
                // sides are written as bytes.
                if (g.cell.w > 0 || g.cell.h > 0) && !self.cell_fits(g.cell)
                    || g.cell.w > 255
                    || g.cell.h > 255
                {
                    return Err(AtlasError::Range("a glyph"));
                }
            }
        }
        if self.icons.len() > MAX_ICONS {
            return Err(AtlasError::Range("icons"));
        }
        for c in self.icons.values() {
            if c.w != ICON || c.h != ICON || !self.cell_fits(*c) {
                return Err(AtlasError::Range("an icon"));
            }
        }
        Ok(())
    }

    /// The `.gma` bytes. Deterministic: tables are written in key order.
    pub fn encode(&self) -> Result<Vec<u8>, AtlasError> {
        self.validate()?;
        let mut raw = Vec::with_capacity(self.texels.len() + 4096);
        let mut pieces: Vec<(&u32, &Piece)> = self.pieces.iter().collect();
        pieces.sort_by_key(|(k, _)| **k);
        for (k, p) in pieces {
            raw.extend_from_slice(&k.to_le_bytes());
            for v in [p.cell.x, p.cell.y, p.cell.w, p.cell.h] {
                raw.extend_from_slice(&v.to_le_bytes());
            }
            raw.extend_from_slice(&p.inset);
        }
        let mut faces: Vec<(&u8, &Face)> = self.faces.iter().collect();
        faces.sort_by_key(|(k, _)| **k);
        for (id, f) in faces {
            raw.push(*id);
            raw.push(f.line_height);
            raw.push(f.ascent);
            raw.extend_from_slice(&(f.glyphs.len() as u16).to_le_bytes());
            let mut glyphs: Vec<(&char, &Glyph)> = f.glyphs.iter().collect();
            glyphs.sort_by_key(|(c, _)| **c);
            for (c, g) in glyphs {
                raw.extend_from_slice(&(*c as u32).to_le_bytes());
                raw.extend_from_slice(&g.cell.x.to_le_bytes());
                raw.extend_from_slice(&g.cell.y.to_le_bytes());
                raw.push(g.cell.w as u8);
                raw.push(g.cell.h as u8);
                raw.push(g.bearing[0] as u8);
                raw.push(g.bearing[1] as u8);
                raw.push(g.advance);
            }
        }
        let mut icons: Vec<(&u32, &Cell)> = self.icons.iter().collect();
        icons.sort_by_key(|(k, _)| **k);
        for (k, c) in icons {
            raw.extend_from_slice(&k.to_le_bytes());
            raw.extend_from_slice(&c.x.to_le_bytes());
            raw.extend_from_slice(&c.y.to_le_bytes());
        }
        raw.extend_from_slice(&self.texels);
        let mut out = Vec::with_capacity(HEADER + raw.len() / 3);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&self.w.to_le_bytes());
        out.extend_from_slice(&self.h.to_le_bytes());
        out.extend_from_slice(&(self.pieces.len() as u16).to_le_bytes());
        out.push(self.faces.len() as u8);
        out.extend_from_slice(&(self.icons.len() as u16).to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9));
        Ok(out)
    }

    pub fn decode(file: &[u8]) -> Result<Atlas, AtlasError> {
        if file.len() < HEADER || file[..4] != MAGIC {
            return Err(AtlasError::Magic);
        }
        let w = u16::from_le_bytes([file[4], file[5]]);
        let h = u16::from_le_bytes([file[6], file[7]]);
        let pieces = u16::from_le_bytes([file[8], file[9]]) as usize;
        let faces = file[10] as usize;
        let icons = u16::from_le_bytes([file[11], file[12]]) as usize;
        let raw_len = u32::from_le_bytes([file[13], file[14], file[15], file[16]]) as usize;
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return Err(AtlasError::Range("size"));
        }
        if pieces > MAX_PIECES || faces > MAX_FACES || icons > MAX_ICONS {
            return Err(AtlasError::Range("counts"));
        }
        let texel_bytes = w as usize * h as usize * 4;
        // The tables are bounded by their counts; the texels by the size: the whole
        // inflation is bounded before it starts (LOOK.md 2.2: by `w × h × 4`, plus tables).
        let most = texel_bytes
            + pieces * PIECE_BYTES
            + faces * (5 + MAX_GLYPHS * GLYPH_BYTES)
            + icons * ICON_BYTES;
        if raw_len < texel_bytes || raw_len > most {
            return Err(AtlasError::Range("payload length"));
        }
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&file[HEADER..], raw_len)
            .map_err(|_| AtlasError::Inflate)?;
        if raw.len() != raw_len {
            return Err(AtlasError::Inflate);
        }
        let mut r = Reader { buf: &raw, at: 0 };
        let mut atlas = Atlas {
            w,
            h,
            ..Default::default()
        };
        for _ in 0..pieces {
            let k = r.u32()?;
            let cell = Cell {
                x: r.u16()?,
                y: r.u16()?,
                w: r.u16()?,
                h: r.u16()?,
            };
            let inset = r.take::<4>()?;
            if atlas.pieces.insert(k, Piece { cell, inset }).is_some() {
                return Err(AtlasError::Range("a piece twice"));
            }
        }
        for _ in 0..faces {
            let id = r.u8()?;
            let line_height = r.u8()?;
            let ascent = r.u8()?;
            let n = r.u16()? as usize;
            if n > MAX_GLYPHS {
                return Err(AtlasError::Range("glyphs"));
            }
            let mut face = Face {
                line_height,
                ascent,
                glyphs: HashMap::with_capacity(n),
            };
            for _ in 0..n {
                let c = char::from_u32(r.u32()?).ok_or(AtlasError::Range("a char"))?;
                let cell = Cell {
                    x: r.u16()?,
                    y: r.u16()?,
                    w: r.u8()? as u16,
                    h: r.u8()? as u16,
                };
                let bearing = [r.u8()? as i8, r.u8()? as i8];
                let advance = r.u8()?;
                if face
                    .glyphs
                    .insert(
                        c,
                        Glyph {
                            cell,
                            bearing,
                            advance,
                        },
                    )
                    .is_some()
                {
                    return Err(AtlasError::Range("a glyph twice"));
                }
            }
            if atlas.faces.insert(id, face).is_some() {
                return Err(AtlasError::Range("a face twice"));
            }
        }
        for _ in 0..icons {
            let k = r.u32()?;
            let cell = Cell {
                x: r.u16()?,
                y: r.u16()?,
                w: ICON,
                h: ICON,
            };
            if atlas.icons.insert(k, cell).is_some() {
                return Err(AtlasError::Range("an icon twice"));
            }
        }
        atlas.texels = r.bytes(texel_bytes)?.to_vec();
        if r.at != raw.len() {
            return Err(AtlasError::Length("trailing bytes"));
        }
        atlas.validate()?;
        Ok(atlas)
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], AtlasError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or(AtlasError::Length("payload"))?;
        let out = &self.buf[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], AtlasError> {
        Ok(self.bytes(N)?.try_into().expect("length checked"))
    }

    fn u8(&mut self) -> Result<u8, AtlasError> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, AtlasError> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32, AtlasError> {
        Ok(u32::from_le_bytes(self.take()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Atlas {
        let (w, h) = (64u16, 48u16);
        let mut a = Atlas {
            w,
            h,
            texels: (0..w as usize * h as usize * 4)
                .map(|i| (i * 13) as u8)
                .collect(),
            ..Default::default()
        };
        a.pieces.insert(
            hash("panel"),
            Piece {
                cell: Cell {
                    x: 0,
                    y: 0,
                    w: 24,
                    h: 24,
                },
                inset: [6, 6, 6, 6],
            },
        );
        let mut face = Face {
            line_height: 14,
            ascent: 11,
            glyphs: HashMap::new(),
        };
        face.glyphs.insert(
            'A',
            Glyph {
                cell: Cell {
                    x: 24,
                    y: 0,
                    w: 7,
                    h: 10,
                },
                bearing: [0, 1],
                advance: 8,
            },
        );
        face.glyphs.insert(
            ' ',
            Glyph {
                cell: Cell::default(),
                bearing: [0, 0],
                advance: 4,
            },
        );
        a.faces.insert(1, face);
        a.icons.insert(
            hash("sword"),
            Cell {
                x: 32,
                y: 0,
                w: ICON,
                h: ICON,
            },
        );
        a
    }

    #[test]
    fn round_trip_is_exact_and_deterministic() {
        let a = sample();
        let file = a.encode().unwrap();
        assert_eq!(file, a.encode().unwrap());
        let back = Atlas::decode(&file).unwrap();
        assert_eq!(back, a);
        assert_eq!(back.piece("panel").unwrap().inset, [6; 4]);
        assert_eq!(back.icon("sword").unwrap().x, 32);
        assert!(back.icon("axe").is_none());
    }

    #[test]
    fn the_reader_refuses_what_does_not_fit() {
        let mut a = sample();
        a.icons.insert(
            hash("far"),
            Cell {
                x: 40,
                y: 40,
                w: ICON,
                h: ICON,
            },
        );
        assert_eq!(a.encode(), Err(AtlasError::Range("an icon")));
        let a = sample();
        let file = a.encode().unwrap();
        // A declared length that is too small, and a truncated stream.
        let mut bad = file.clone();
        bad[13..17].copy_from_slice(&100u32.to_le_bytes());
        assert!(Atlas::decode(&bad).is_err());
        assert!(Atlas::decode(&file[..file.len() / 2]).is_err());
        assert_eq!(Atlas::decode(b"nope"), Err(AtlasError::Magic));
    }

    #[test]
    fn hashes_are_stable() {
        // FNV-1a's published vector.
        assert_eq!(hash(""), 0x811c_9dc5);
        assert_eq!(hash("a"), 0xe40c_292c);
    }
}
