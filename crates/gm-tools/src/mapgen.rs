//! Writing Quake `.map` text from code: axis-aligned brushes, ramps and point entities. The
//! generated maps (`arena`, `town`) are committed and hand-editable in TrenchBroom; the
//! generators exist so a layout is reproducible and reviewable as code.

use std::fmt::Write as _;

pub struct Map {
    pub out: String,
}

/// Three points on a plane (clockwise seen from outside) and its texture.
type Plane<'a> = ([f32; 3], [f32; 3], [f32; 3], &'a str);

/// The six planes of a ramp rising along x; see `Map::ramp_x`.
fn ramp_planes(
    x_low: f32,
    x_high: f32,
    y0: f32,
    y1: f32,
    z0: f32,
    z1: f32,
    tex: &str,
) -> Vec<Plane<'_>> {
    let (xa, xb) = if x_low < x_high {
        (x_low, x_high)
    } else {
        (x_high, x_low)
    };
    // Heights at xa and xb.
    let (za, zb) = if x_low < x_high { (z0, z1) } else { (z1, z0) };
    let ztop = za.max(zb);
    let trim = "trim_dark";
    vec![
        // bottom
        (
            [xa, y1, z0 - 16.0],
            [xa, y0, z0 - 16.0],
            [xb, y0, z0 - 16.0],
            trim,
        ),
        // +y side
        (
            [xa, y1, ztop],
            [xa, y1, z0 - 16.0],
            [xb, y1, z0 - 16.0],
            trim,
        ),
        // -y side
        (
            [xb, y0, z0 - 16.0],
            [xa, y0, z0 - 16.0],
            [xa, y0, ztop],
            trim,
        ),
        // the cap at xb
        (
            [xb, y1, z0 - 16.0],
            [xb, y0, z0 - 16.0],
            [xb, y0, ztop],
            trim,
        ),
        // the cap at xa
        (
            [xa, y0, ztop],
            [xa, y0, z0 - 16.0],
            [xa, y1, z0 - 16.0],
            trim,
        ),
        // the slope: three points on it, counter-clockwise seen from above
        ([xb, y0, zb], [xa, y0, za], [xa, y1, za], tex),
    ]
}

impl Map {
    pub fn new() -> Map {
        Map { out: String::new() }
    }

    /// An axis-aligned box brush `mins..maxs` with a side texture, a top texture and a
    /// bottom texture.
    pub fn boxb(&mut self, mins: [f32; 3], maxs: [f32; 3], side: &str, top: &str, bottom: &str) {
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
    pub fn ramp_x(
        &mut self,
        x_low: f32,
        x_high: f32,
        y0: f32,
        y1: f32,
        z0: f32,
        z1: f32,
        tex: &str,
    ) {
        let planes = ramp_planes(x_low, x_high, y0, y1, z0, z1, tex);
        self.brush(&planes);
    }

    /// The same ramp rising along y (`y_low` to `y_high`), spanning `x0..x1`: the x ramp
    /// mirrored in the diagonal, with every plane's winding turned back.
    #[allow(clippy::too_many_arguments)]
    pub fn ramp_y(
        &mut self,
        y_low: f32,
        y_high: f32,
        x0: f32,
        x1: f32,
        z0: f32,
        z1: f32,
        tex: &str,
    ) {
        let swap = |p: [f32; 3]| [p[1], p[0], p[2]];
        let planes: Vec<Plane<'_>> = ramp_planes(y_low, y_high, x0, x1, z0, z1, tex)
            .into_iter()
            .map(|(a, b, c, t)| (swap(c), swap(b), swap(a), t))
            .collect();
        self.brush(&planes);
    }

    fn brush(&mut self, planes: &[Plane<'_>]) {
        self.out.push_str("{\n");
        for (a, b, c, t) in planes {
            let _ = writeln!(
                self.out,
                "( {} {} {} ) ( {} {} {} ) ( {} {} {} ) {t} 0 0 0 1 1",
                a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]
            );
        }
        self.out.push_str("}\n");
    }

    /// A thin slab whose top shows one copy of `top` across its whole `size` (a square of
    /// 64-texel texture stretched and offset to fit): the marking of a stall tile.
    pub fn tile(&mut self, min: [f32; 2], size: f32, z0: f32, z1: f32, side: &str, top: &str) {
        let [x0, y0] = min;
        let (x1, y1) = (x0 + size, y0 + size);
        let scale = size / 64.0;
        // Standard projection on a floor: s = x / scale + xoff, t = -y / scale + yoff.
        let xoff = (-x0 / scale).rem_euclid(64.0);
        let yoff = (y1 / scale).rem_euclid(64.0);
        let p = |a: [f32; 3], b: [f32; 3], c: [f32; 3], tex: &str, align: &str| {
            format!(
                "( {} {} {} ) ( {} {} {} ) ( {} {} {} ) {tex} {align}\n",
                a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]
            )
        };
        let plain = "0 0 0 1 1";
        self.out.push_str("{\n");
        self.out
            .push_str(&p([x1, y1, z0], [x1, y0, z0], [x1, y0, z1], side, plain));
        self.out
            .push_str(&p([x0, y0, z1], [x0, y0, z0], [x0, y1, z0], side, plain));
        self.out
            .push_str(&p([x0, y1, z1], [x0, y1, z0], [x1, y1, z0], side, plain));
        self.out
            .push_str(&p([x1, y0, z0], [x0, y0, z0], [x0, y0, z1], side, plain));
        self.out.push_str(&p(
            [x1, y0, z1],
            [x0, y0, z1],
            [x0, y1, z1],
            top,
            &format!("{xoff} {yoff} 0 {scale} {scale}"),
        ));
        self.out
            .push_str(&p([x0, y1, z0], [x0, y0, z0], [x1, y0, z0], side, plain));
        self.out.push_str("}\n");
    }

    pub fn entity(&mut self, props: &[(&str, String)]) {
        self.out.push_str("{\n");
        for (k, v) in props {
            let _ = writeln!(self.out, "\"{k}\" \"{v}\"");
        }
        self.out.push_str("}\n");
    }

    pub fn light(&mut self, x: f32, y: f32, z: f32, light: u32, color: [u8; 3]) {
        self.entity(&[
            ("classname", "light".into()),
            ("origin", format!("{x} {y} {z}")),
            ("light", light.to_string()),
            ("_color", format!("{} {} {}", color[0], color[1], color[2])),
        ]);
    }

    pub fn spawn(&mut self, x: f32, y: f32, z: f32, angle: i32, team: u8) {
        self.entity(&[
            ("classname", "gm_spawn".into()),
            ("origin", format!("{x} {y} {z}")),
            ("angle", angle.to_string()),
            ("team", team.to_string()),
        ]);
    }
}

/// The generator writes brushes into worldspawn and entities after it; this closes the
/// worldspawn block right after its last brush.
pub fn fix_worldspawn_closure(text: String) -> String {
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
