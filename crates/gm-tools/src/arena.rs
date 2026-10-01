//! Generator for the 8v8 arena map (PLAN.md 11.8 Phase 3): a symmetric hall with two team
//! bases, a raised centre, pillars, low cover and side walkways. The `.map` it writes is
//! committed and hand-editable in TrenchBroom; the generator exists so the layout is
//! reproducible and reviewable as code.

use std::fmt::Write as _;

/// Quake map units.
const HALF_X: f32 = 1344.0;
const HALF_Y: f32 = 960.0;
const WALL: f32 = 16.0;
const CEILING: f32 = 320.0;

struct Map {
    out: String,
}

impl Map {
    fn new() -> Map {
        Map { out: String::new() }
    }

    /// An axis-aligned box brush `mins..maxs` with a side texture, a top texture and a
    /// bottom texture.
    fn boxb(&mut self, mins: [f32; 3], maxs: [f32; 3], side: &str, top: &str, bottom: &str) {
        let [x0, y0, z0] = mins;
        let [x1, y1, z1] = maxs;
        let p = |a: [f32; 3], b: [f32; 3], c: [f32; 3], tex: &str| {
            format!(
                "( {} {} {} ) ( {} {} {} ) ( {} {} {} ) {tex} 0 0 0 1 1\n",
                a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]
            )
        };
        self.out.push_str("{\n");
        // +x face (points counter-clockwise seen from outside, as qbsp expects).
        self.out
            .push_str(&p([x1, y1, z0], [x1, y0, z0], [x1, y0, z1], side));
        // -x
        self.out
            .push_str(&p([x0, y0, z1], [x0, y0, z0], [x0, y1, z0], side));
        // +y
        self.out
            .push_str(&p([x0, y1, z1], [x0, y1, z0], [x1, y1, z0], side));
        // -y
        self.out
            .push_str(&p([x1, y0, z0], [x0, y0, z0], [x0, y0, z1], side));
        // +z
        self.out
            .push_str(&p([x1, y0, z1], [x0, y0, z1], [x0, y1, z1], top));
        // -z
        self.out
            .push_str(&p([x0, y1, z0], [x0, y0, z0], [x1, y0, z0], bottom));
        self.out.push_str("}\n");
    }

    /// A ramp rising along +x (`rise_x > 0`) or -x from `z0` at the low end to `z1` at the
    /// high end, spanning `y0..y1`. The low end is at `x_low`, the high end at `x_high`.
    #[allow(clippy::too_many_arguments)]
    fn ramp_x(&mut self, x_low: f32, x_high: f32, y0: f32, y1: f32, z0: f32, z1: f32, tex: &str) {
        let (xa, xb) = if x_low < x_high {
            (x_low, x_high)
        } else {
            (x_high, x_low)
        };
        let p = |a: [f32; 3], b: [f32; 3], c: [f32; 3], t: &str| {
            format!(
                "( {} {} {} ) ( {} {} {} ) ( {} {} {} ) {t} 0 0 0 1 1\n",
                a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]
            )
        };
        // Heights at xa and xb.
        let (za, zb) = if x_low < x_high { (z0, z1) } else { (z1, z0) };
        let ztop = za.max(zb);
        self.out.push_str("{\n");
        // bottom
        self.out.push_str(&p(
            [xa, y1, z0 - 16.0],
            [xa, y0, z0 - 16.0],
            [xb, y0, z0 - 16.0],
            "trim_dark",
        ));
        // +y side
        self.out.push_str(&p(
            [xa, y1, ztop],
            [xa, y1, z0 - 16.0],
            [xb, y1, z0 - 16.0],
            "trim_dark",
        ));
        // -y side
        self.out.push_str(&p(
            [xb, y0, z0 - 16.0],
            [xa, y0, z0 - 16.0],
            [xa, y0, ztop],
            "trim_dark",
        ));
        // high end cap (vertical face at the high x end)
        if x_low < x_high {
            self.out.push_str(&p(
                [xb, y1, z0 - 16.0],
                [xb, y0, z0 - 16.0],
                [xb, y0, ztop],
                "trim_dark",
            ));
            // low end cap
            self.out.push_str(&p(
                [xa, y0, ztop],
                [xa, y0, z0 - 16.0],
                [xa, y1, z0 - 16.0],
                "trim_dark",
            ));
        } else {
            self.out.push_str(&p(
                [xb, y1, z0 - 16.0],
                [xb, y0, z0 - 16.0],
                [xb, y0, ztop],
                "trim_dark",
            ));
            self.out.push_str(&p(
                [xa, y0, ztop],
                [xa, y0, z0 - 16.0],
                [xa, y1, z0 - 16.0],
                "trim_dark",
            ));
        }
        // sloped top: three points on the slope, counter-clockwise seen from above.
        self.out
            .push_str(&p([xb, y0, zb], [xa, y0, za], [xa, y1, za], tex));
        self.out.push_str("}\n");
    }

    fn entity(&mut self, props: &[(&str, String)]) {
        self.out.push_str("{\n");
        for (k, v) in props {
            let _ = writeln!(self.out, "\"{k}\" \"{v}\"");
        }
        self.out.push_str("}\n");
    }

    fn light(&mut self, x: f32, y: f32, z: f32, light: u32, color: [u8; 3]) {
        self.entity(&[
            ("classname", "light".into()),
            ("origin", format!("{x} {y} {z}")),
            ("light", light.to_string()),
            ("_color", format!("{} {} {}", color[0], color[1], color[2])),
        ]);
    }

    fn spawn(&mut self, x: f32, y: f32, z: f32, angle: i32, team: u8) {
        self.entity(&[
            ("classname", "gm_spawn".into()),
            ("origin", format!("{x} {y} {z}")),
            ("angle", angle.to_string()),
            ("team", team.to_string()),
        ]);
    }
}

/// The arena as `.map` text.
pub fn generate() -> String {
    let mut m = Map::new();
    m.out.push_str(
        "// gamengine arena: 8v8, symmetric about x = 0. Generated by `gm-tools map gen-arena`;\n\
         // hand edits are fine, regenerate only to change the layout wholesale.\n",
    );
    m.out.push_str("{\n\"classname\" \"worldspawn\"\n\"message\" \"arena\"\n\"wad\" \"base.wad\"\n\"light\" \"10\"\n\"_minlight_color\" \"190 200 230\"\n\"_dirt\" \"1\"\n\"_bounce\" \"1\"\n");

    // Shell: floor, ceiling, four walls (all overlapping at the corners, like the test room).
    let (hx, hy) = (HALF_X + WALL, HALF_Y + WALL);
    m.boxb(
        [-hx, -hy, -WALL],
        [hx, hy, 0.0],
        "floor_stone",
        "floor_stone",
        "floor_stone",
    );
    m.boxb(
        [-hx, -hy, CEILING],
        [hx, hy, CEILING + WALL],
        "ceil_plaster",
        "ceil_plaster",
        "ceil_plaster",
    );
    m.boxb(
        [-hx, -hy, 0.0],
        [-HALF_X, hy, CEILING],
        "wall_brick",
        "wall_brick",
        "wall_brick",
    );
    m.boxb(
        [HALF_X, -hy, 0.0],
        [hx, hy, CEILING],
        "wall_brick",
        "wall_brick",
        "wall_brick",
    );
    m.boxb(
        [-HALF_X, -hy, 0.0],
        [HALF_X, -HALF_Y, CEILING],
        "wall_brick",
        "wall_brick",
        "wall_brick",
    );
    m.boxb(
        [-HALF_X, HALF_Y, 0.0],
        [HALF_X, hy, CEILING],
        "wall_brick",
        "wall_brick",
        "wall_brick",
    );

    // Centre: a raised platform with ramps on both x sides.
    m.boxb(
        [-192.0, -192.0, 0.0],
        [192.0, 192.0, 48.0],
        "trim_dark",
        "metal_panel",
        "trim_dark",
    );
    m.ramp_x(-320.0, -192.0, -96.0, 96.0, 0.0, 48.0, "ramp_wood");
    m.ramp_x(320.0, 192.0, -96.0, 96.0, 0.0, 48.0, "ramp_wood");
    // Centre pillar on the platform: the thing you peek around.
    m.boxb(
        [-32.0, -32.0, 48.0],
        [32.0, 32.0, 208.0],
        "metal_panel",
        "metal_panel",
        "metal_panel",
    );

    // Four pillars around the centre.
    for (x, y) in [
        (-448.0, -352.0),
        (-448.0, 352.0),
        (448.0, -352.0),
        (448.0, 352.0),
    ] {
        m.boxb(
            [x - 40.0, y - 40.0, 0.0],
            [x + 40.0, y + 40.0, CEILING],
            "metal_panel",
            "metal_panel",
            "metal_panel",
        );
    }

    // Low cover walls (waist high: a crouch-free wall you can shoot over) on each side.
    for sx in [-1.0f32, 1.0] {
        m.boxb(
            [sx * 768.0 - 16.0, -320.0, 0.0],
            [sx * 768.0 + 16.0, -96.0, 40.0],
            "trim_dark",
            "metal_panel",
            "trim_dark",
        );
        m.boxb(
            [sx * 768.0 - 16.0, 96.0, 0.0],
            [sx * 768.0 + 16.0, 320.0, 40.0],
            "trim_dark",
            "metal_panel",
            "trim_dark",
        );
        // Base cover near the spawns: a wall with a gap in the middle.
        m.boxb(
            [sx * 1040.0 - 16.0, -704.0, 0.0],
            [sx * 1040.0 + 16.0, -160.0, 96.0],
            "wall_brick",
            "trim_dark",
            "wall_brick",
        );
        m.boxb(
            [sx * 1040.0 - 16.0, 160.0, 0.0],
            [sx * 1040.0 + 16.0, 704.0, 96.0],
            "wall_brick",
            "trim_dark",
            "wall_brick",
        );
    }

    // Side walkways: elevated ledges along the long walls, reached by ramps at both ends.
    for sy in [-1.0f32, 1.0] {
        let y0 = sy * HALF_Y;
        let y1 = sy * (HALF_Y - 192.0);
        let (ya, yb) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        m.boxb(
            [-640.0, ya, 0.0],
            [640.0, yb, 96.0],
            "wall_brick",
            "floor_stone",
            "trim_dark",
        );
        m.ramp_x(-896.0, -640.0, ya, yb, 0.0, 96.0, "ramp_wood");
        m.ramp_x(896.0, 640.0, ya, yb, 0.0, 96.0, "ramp_wood");
        // Railing posts so the ledge reads as a ledge from below.
        for x in [-512.0f32, -256.0, 0.0, 256.0, 512.0] {
            let yr = if sy < 0.0 { yb - 8.0 } else { ya };
            m.boxb(
                [x - 8.0, yr, 96.0],
                [x + 8.0, yr + 8.0, 128.0],
                "trim_dark",
                "trim_dark",
                "trim_dark",
            );
        }
    }

    // Lights: a grid, cool over team 1 (west), warm over team 2 (east), white in the middle.
    for ix in -4..=4 {
        for iy in -2..=2 {
            let x = ix as f32 * 300.0;
            let y = iy as f32 * 384.0;
            let color = if x < -200.0 {
                [180, 200, 255]
            } else if x > 200.0 {
                [255, 200, 170]
            } else {
                [255, 255, 255]
            };
            m.light(x, y, 260.0, 220, color);
        }
    }
    // Accent lights by the bases.
    m.light(-1200.0, 0.0, 120.0, 300, [120, 160, 255]);
    m.light(1200.0, 0.0, 120.0, 300, [255, 150, 110]);

    // Spawns: 2 x 4 per team, 64 u apart, facing the centre.
    for (team, sx, angle) in [(1u8, -1.0f32, 0), (2u8, 1.0f32, 180)] {
        for row in 0..2 {
            for col in 0..4 {
                let x = sx * (1216.0 - row as f32 * 64.0);
                let y = (col as f32 - 1.5) * 64.0;
                m.spawn(x, y, 40.0, angle, team);
            }
        }
    }
    // A neutral start for the offline client and tests.
    m.entity(&[
        ("classname", "info_player_start".into()),
        ("origin", "0 -640 40".into()),
        ("angle", "90".into()),
    ]);
    m.out.push_str("}\n");
    // Entities must follow the worldspawn block: move the "}" of worldspawn before them.
    fix_worldspawn_closure(m.out)
}

/// The generator writes brushes into worldspawn and entities after it; this closes the
/// worldspawn block right after its last brush.
fn fix_worldspawn_closure(text: String) -> String {
    // Entities start at the first "{\n\"classname\"" after the worldspawn header.
    let header_end = text
        .find("\"_bounce\" \"1\"\n")
        .map(|i| i + "\"_bounce\" \"1\"\n".len())
        .unwrap_or(0);
    let body = &text[header_end..];
    let first_entity = body.find("{\n\"classname\"").unwrap_or(body.len());
    let mut out = String::with_capacity(text.len() + 4);
    out.push_str(&text[..header_end]);
    out.push_str(&body[..first_entity]);
    out.push_str("}\n");
    let rest = &body[first_entity..];
    // Drop the final stray "}\n" the generator appended after the last entity.
    let rest = rest.strip_suffix("}\n").unwrap_or(rest);
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_has_sixteen_team_spawns_and_balanced_blocks() {
        let text = generate();
        let opens = text.matches("{\n").count();
        let closes = text.matches("}\n").count();
        assert_eq!(opens, closes, "unbalanced braces");
        assert_eq!(text.matches("\"gm_spawn\"").count(), 16);
        assert_eq!(text.matches("\"team\" \"1\"").count(), 8);
        assert_eq!(text.matches("\"team\" \"2\"").count(), 8);
        assert!(text.starts_with("// gamengine arena"));
        // Exactly one worldspawn block, closed before the first entity.
        let ws = text.find("\"classname\" \"worldspawn\"").unwrap();
        let first_light = text.find("\"classname\" \"light\"").unwrap();
        assert!(ws < first_light);
    }
}
