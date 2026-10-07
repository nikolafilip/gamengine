//! What a fight looks like (LOOK.md 13): the place a swing lands drawn where it lands, a
//! bolt as a streak, an area as a disc on the floor with a burst when it goes off, a spark
//! on whoever is hit. Everything here is coloured, unlit, see-through triangles in the
//! world, made anew every frame from what the snapshot and the own prediction say; the
//! renderer draws them in one call after the bodies, blended, without writing depth.
//!
//! Nothing here decides anything. The wedge is the one `gm_core::sim::melee_hit_point`
//! tests (the reach and the arc of the ability the body is `acting`), so what is drawn is
//! what does the damage; the zone alone says who was hit.

use std::collections::HashMap;

use glam::Vec3;
use gm_core::sim::anim;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FxVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

type Rgba = [f32; 4];

fn fade(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * a.clamp(0.0, 1.0)]
}

/// Degrees of arc one segment of a curve covers.
const SEGMENT_DEG: f32 = 7.5;

/// A bullet's mark on the world (MODES.md 10.2): where, which way the wall faces, how old.
struct Decal {
    at: Vec3,
    normal: Vec3,
    age: f32,
}

/// How long a bullet's mark stays, and how long its fading takes at the end.
const DECAL_SECS: f32 = 20.0;
const DECAL_FADE: f32 = 4.0;
/// The most marks kept: the oldest go first.
const DECALS_MAX: usize = 160;
/// A mark's radius, in units.
const DECAL_RADIUS: f32 = 3.5;

/// The frame's triangles.
#[derive(Default)]
pub struct FxMesh {
    pub verts: Vec<FxVertex>,
}

impl FxMesh {
    pub fn clear(&mut self) {
        self.verts.clear();
    }

    fn tri(&mut self, a: (Vec3, Rgba), b: (Vec3, Rgba), c: (Vec3, Rgba)) {
        for (p, color) in [a, b, c] {
            self.verts.push(FxVertex {
                pos: p.to_array(),
                color,
            });
        }
    }

    fn quad(&mut self, a: (Vec3, Rgba), b: (Vec3, Rgba), c: (Vec3, Rgba), d: (Vec3, Rgba)) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }

    /// A disc of radius `r` lying on a surface that faces `normal`, lifted a little off it
    /// so that it is drawn over the wall and not in it: a bullet's mark (MODES.md 10.2).
    /// Dark in the middle, fading to nothing at the rim.
    pub fn disc(&mut self, at: Vec3, normal: Vec3, r: f32, ink: Rgba) {
        let n = normal.normalize_or(Vec3::Z);
        let seed = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let u = seed.cross(n).normalize_or(Vec3::X);
        let v = n.cross(u);
        let c = at + n * 0.6;
        let rim = fade(ink, 0.0);
        let steps = 10;
        for k in 0..steps {
            let a0 = k as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = (k + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            let p0 = c + (u * a0.cos() + v * a0.sin()) * r;
            let p1 = c + (u * a1.cos() + v * a1.sin()) * r;
            self.tri((c, ink), (p0, rim), (p1, rim));
            self.tri((c, ink), (p1, rim), (p0, rim));
        }
    }

    /// A piece of a ring lying flat at `centre.z`: between the radii `r0` and `r1` and the
    /// angles `a0` and `a1` (degrees, counter-clockwise from +X, as a yaw is). `ink` gives
    /// the colours at the inner and the outer edge for a place along the angle (0 at `a0`,
    /// 1 at `a1`).
    pub fn sector_with(
        &mut self,
        centre: Vec3,
        (a0, a1): (f32, f32),
        (r0, r1): (f32, f32),
        ink: impl Fn(f32) -> (Rgba, Rgba),
    ) {
        if r1 <= r0 || a1 <= a0 {
            return;
        }
        let steps = ((a1 - a0) / SEGMENT_DEG).ceil().max(1.0) as usize;
        let at = |u: f32, r: f32| {
            let (sin, cos) = (a0 + (a1 - a0) * u).to_radians().sin_cos();
            centre + Vec3::new(cos * r, sin * r, 0.0)
        };
        for i in 0..steps {
            let (u0, u1) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
            let ((in0, out0), (in1, out1)) = (ink(u0), ink(u1));
            self.quad(
                (at(u0, r0), in0),
                (at(u0, r1), out0),
                (at(u1, r1), out1),
                (at(u1, r0), in1),
            );
        }
    }

    /// The same in one colour inside and one outside.
    pub fn sector(
        &mut self,
        centre: Vec3,
        angles: (f32, f32),
        radii: (f32, f32),
        inner: Rgba,
        outer: Rgba,
    ) {
        self.sector_with(centre, angles, radii, |_| (inner, outer));
    }

    /// A band standing on an arc of radius `r` round `centre`, from `z0` to `z1` over
    /// `centre.z`: what a blade leaves in the air. `ink` gives the colours at the lower
    /// and the upper edge for a place along the angle (0 at `a0`, 1 at `a1`).
    pub fn band_with(
        &mut self,
        centre: Vec3,
        (a0, a1): (f32, f32),
        r: f32,
        (z0, z1): (f32, f32),
        ink: impl Fn(f32) -> (Rgba, Rgba),
    ) {
        if a1 <= a0 || z1 <= z0 {
            return;
        }
        let steps = ((a1 - a0) / SEGMENT_DEG).ceil().max(1.0) as usize;
        let at = |u: f32, z: f32| {
            let (sin, cos) = (a0 + (a1 - a0) * u).to_radians().sin_cos();
            centre + Vec3::new(cos * r, sin * r, z)
        };
        for i in 0..steps {
            let (u0, u1) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
            let ((low0, high0), (low1, high1)) = (ink(u0), ink(u1));
            self.quad(
                (at(u0, z0), low0),
                (at(u1, z0), low1),
                (at(u1, z1), high1),
                (at(u0, z1), high0),
            );
        }
    }

    /// A line on the floor from `a` to `b`, `width` across.
    pub fn line(&mut self, a: Vec3, b: Vec3, width: f32, color: Rgba) {
        let along = (b - a).truncate();
        if along.length_squared() < 1e-6 {
            return;
        }
        let side = Vec3::new(-along.y, along.x, 0.0).normalize() * (width * 0.5);
        self.quad(
            (a - side, color),
            (a + side, color),
            (b + side, color),
            (b - side, color),
        );
    }

    /// A wall standing on a circle of radius `r` round `centre`, `height` high: the burst
    /// of a blast.
    pub fn wall(&mut self, centre: Vec3, r: f32, height: f32, bottom: Rgba, top: Rgba) {
        let steps = (360.0 / (SEGMENT_DEG * 2.0)) as usize;
        let at = |i: usize| {
            let (sin, cos) = (i as f32 / steps as f32 * std::f32::consts::TAU).sin_cos();
            centre + Vec3::new(cos * r, sin * r, 0.0)
        };
        let up = Vec3::Z * height;
        for i in 0..steps {
            let (p, q) = (at(i), at(i + 1));
            self.quad((p, bottom), (q, bottom), (q + up, top), (p + up, top));
        }
    }

    /// A ribbon from `tail` to `head` that faces `eye`: a bolt in flight, a spark's ray.
    pub fn streak(
        &mut self,
        tail: Vec3,
        head: Vec3,
        half_width: f32,
        eye: Vec3,
        tail_ink: Rgba,
        head_ink: Rgba,
    ) {
        let along = head - tail;
        let side = along.cross(eye - head);
        if side.length_squared() < 1e-9 {
            return;
        }
        let side = side.normalize() * half_width;
        self.quad(
            (tail - side, tail_ink),
            (tail + side, tail_ink),
            (head + side, head_ink),
            (head - side, head_ink),
        );
    }

    /// A small diamond at `at` that faces `eye`.
    pub fn glint(&mut self, at: Vec3, r: f32, eye: Vec3, color: Rgba) {
        let to = (eye - at).normalize_or(Vec3::X);
        let right = to.cross(Vec3::Z).normalize_or(Vec3::Y) * r;
        let up = right.cross(to).normalize_or(Vec3::Z) * r;
        let edge = fade(color, 0.0);
        for (p, q) in [(right, up), (up, -right), (-right, -up), (-up, right)] {
            self.tri((at, color), (at + p, edge), (at + q, edge));
        }
    }
}

/// Whose it is: what colour it is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Own,
    Friend,
    Foe,
}

impl Side {
    fn ink(self) -> Rgba {
        match self {
            Side::Own => [1.0, 0.93, 0.72, 1.0],
            Side::Friend => [0.45, 0.72, 1.0, 1.0],
            Side::Foe => [1.0, 0.36, 0.20, 1.0],
        }
    }
}

/// The wedge of a swing (VOCABULARY.md 5.1) and its times in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swing {
    pub reach: f32,
    pub arc_deg: f32,
    pub windup: f32,
    pub active: f32,
}

/// A body as the frame has it.
#[derive(Clone, Copy, Debug)]
pub struct Fighter {
    pub key: u32,
    pub feet: Vec3,
    /// How far over the feet a blade passes.
    pub chest: f32,
    pub yaw: f32,
    pub anim: u8,
    /// The swing of the ability it is acting, when that is a swing and is known.
    pub swing: Option<Swing>,
    pub side: Side,
    /// Its health, when the frame knows it.
    pub health: Option<u16>,
}

struct Seen {
    anim: u8,
    /// Seconds in this stance.
    since: f32,
    health: Option<u16>,
    frame: u64,
}

/// A swing that landed: where, and how long ago.
struct Slash {
    centre: Vec3,
    chest: f32,
    yaw: f32,
    swing: Swing,
    side: Side,
    age: f32,
}

struct Spark {
    at: Vec3,
    age: f32,
}

/// The own bolt between the hand and the zone's word of it: the frames a shot is aimed
/// by, before the snapshot that carries the bolt arrives.
struct Tracer {
    /// Where it left from: its streak never reaches back past the hand.
    from: Vec3,
    pos: Vec3,
    vel: Vec3,
    radius: f32,
    ink: Rgba,
    age: f32,
}

/// What a number over a body says (LOOK.md 13.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blow {
    /// The own hand took this off another body.
    Dealt,
    /// The own body lost this.
    Taken,
    /// The own body got this back.
    Healed,
    /// The own hand's Regen gave another body this back.
    Mended,
    /// The own hand's blow was all taken by a block.
    Blocked,
}

/// A number in the air: what a blow did, where, and for how long now.
struct Number {
    at: Vec3,
    amount: u32,
    blow: Blow,
    age: f32,
}

/// A number over a body as the HUD draws it: the body's head in the world, the words,
/// the colour with its fading in it, and how far it has floated (0 at birth, 1 at the
/// end, quick at first).
#[derive(Clone, Debug, PartialEq)]
pub struct Pop {
    pub at: Vec3,
    pub text: String,
    pub ink: Rgba,
    pub lift: f32,
    pub blow: Blow,
}

struct Burst {
    at: Vec3,
    radius: f32,
    harmful: bool,
    age: f32,
}

/// A blade passes in at least this long, however short the active window (three frames
/// would not be seen).
const SWEEP_SECS: f32 = 0.10;
/// And what it leaves stays this long after.
const SLASH_LINGER: f32 = 0.24;
const SPARK_SECS: f32 = 0.22;
/// A tracer flies this long: about what the zone's bolt takes to show.
const TRACER_SECS: f32 = 0.16;
const BURST_SECS: f32 = 0.35;
/// A body that was hit is lit for this long.
const FLASH_SECS: f32 = 0.14;
/// A number floats this long, and fades over the last part of it.
const NUMBER_SECS: f32 = 1.1;
const NUMBER_FADE: f32 = 0.35;
/// A streak is never drawn longer than this behind a bolt's head.
const STREAK_MAX: f32 = 96.0;

/// A bolt of which nothing more is known.
pub const BOLT: Rgba = [1.0, 0.72, 0.25, 1.0];
const HARM: Rgba = [1.0, 0.5, 0.12, 1.0];
const HELP: Rgba = [0.25, 0.9, 0.45, 1.0];

/// The fight's effects between frames: what was seen of each body, and what is still in
/// the air.
#[derive(Default)]
pub struct Effects {
    seen: HashMap<u32, Seen>,
    areas: HashMap<u32, (f32, u64)>,
    /// Where each of the zone's bolts was first seen, and when last: its streak reaches
    /// no further back than that.
    bolts: HashMap<u32, (Vec3, u64)>,
    slashes: Vec<Slash>,
    sparks: Vec<Spark>,
    bursts: Vec<Burst>,
    tracers: Vec<Tracer>,
    numbers: Vec<Number>,
    decals: Vec<Decal>,
    flashes: HashMap<u32, f32>,
    frame: u64,
    dt: f32,
    /// 1 when the own body was just hurt, running down to 0: the frame's red edge.
    pub own_hurt: f32,
}

impl Effects {
    /// A frame begins: everything in the air is `dt` older.
    pub fn begin(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.25);
        self.dt = dt;
        self.frame += 1;
        for s in &mut self.slashes {
            s.age += dt;
        }
        self.slashes
            .retain(|s| s.age < s.swing.active.max(SWEEP_SECS) + SLASH_LINGER);
        for s in &mut self.sparks {
            s.age += dt;
        }
        self.sparks.retain(|s| s.age < SPARK_SECS);
        for t in &mut self.tracers {
            t.age += dt;
            t.pos += t.vel * dt;
        }
        self.tracers.retain(|t| t.age < TRACER_SECS);
        for b in &mut self.bursts {
            b.age += dt;
        }
        self.bursts.retain(|b| b.age < BURST_SECS);
        for f in self.flashes.values_mut() {
            *f -= dt / FLASH_SECS;
        }
        self.flashes.retain(|_, f| *f > 0.0);
        for n in &mut self.numbers {
            n.age += dt;
        }
        self.numbers.retain(|n| n.age < NUMBER_SECS);
        for d in &mut self.decals {
            d.age += dt;
        }
        self.decals.retain(|d| d.age < DECAL_SECS);
        self.own_hurt = (self.own_hurt - dt * 3.0).max(0.0);
    }

    /// A number over a body's head: the own hurts and healings come from the own health
    /// (`fighter`), what the own hand dealt from the zone's word of it (`hit`).
    fn number(&mut self, at: Vec3, amount: u32, blow: Blow) {
        self.numbers.push(Number {
            at,
            amount,
            blow,
            age: 0.0,
        });
    }

    /// The zone says the own hand landed a blow on the body whose head is at `head`:
    /// `amount` came off its health after its block took `absorbed`.
    pub fn hit(&mut self, at: Vec3, amount: u32, absorbed: u32) {
        // Since LOOK.md 13.11 `at` is where the blow landed, not the head: the number
        // floats from the wound, and a spark marks it.
        self.sparks.push(Spark { at, age: 0.0 });
        if amount == 0 && absorbed > 0 {
            self.number(at, 0, Blow::Blocked);
        } else {
            self.number(at, amount, Blow::Dealt);
        }
    }

    /// The zone says a bullet met the world at `at`, a surface facing `normal`: a dark
    /// mark there for a while (MODES.md 10.2), the oldest forgotten past `DECALS_MAX`.
    pub fn impact(&mut self, at: Vec3, normal: Vec3) {
        if self.decals.len() >= DECALS_MAX {
            self.decals.remove(0);
        }
        self.decals.push(Decal {
            at,
            normal,
            age: 0.0,
        });
    }

    /// The zone says a Regen of the own hand gave the body whose head is at `head`
    /// `amount` health back.
    pub fn healed(&mut self, head: Vec3, amount: u32) {
        self.number(head, amount, Blow::Mended);
    }

    /// The numbers in the air, as the HUD draws them (LOOK.md 13.8): what was dealt in
    /// gold, what was taken in red, what was healed (either way) in green, a block in grey; each
    /// floats up, quickly at first, and fades out at the end.
    pub fn numbers(&self) -> Vec<Pop> {
        self.numbers
            .iter()
            .map(|n| {
                let t = (n.age / NUMBER_SECS).clamp(0.0, 1.0);
                let left = ((NUMBER_SECS - n.age) / NUMBER_FADE).clamp(0.0, 1.0);
                let (text, ink) = match n.blow {
                    Blow::Dealt => (n.amount.to_string(), [1.0, 0.88, 0.40, 1.0]),
                    Blow::Taken => (format!("-{}", n.amount), [1.0, 0.30, 0.24, 1.0]),
                    Blow::Healed | Blow::Mended => {
                        (format!("+{}", n.amount), [0.45, 1.0, 0.50, 1.0])
                    }
                    Blow::Blocked => ("blocked".to_string(), [0.75, 0.78, 0.85, 1.0]),
                };
                Pop {
                    at: n.at,
                    text,
                    ink: fade(ink, left),
                    lift: 1.0 - (1.0 - t) * (1.0 - t),
                    blow: n.blow,
                }
            })
            .collect()
    }

    /// One body of the frame: its windup is drawn on the floor as it fills, a swing that
    /// begins leaves its slash, a drop of its health a spark.
    pub fn fighter(&mut self, f: &Fighter, mesh: &mut FxMesh) {
        let (frame, dt) = (self.frame, self.dt);
        let seen = self.seen.entry(f.key).or_insert(Seen {
            anim: f.anim,
            since: 0.0,
            health: f.health,
            frame,
        });
        let entered = seen.anim != f.anim || seen.frame + 1 < frame;
        if entered {
            seen.anim = f.anim;
            seen.since = 0.0;
        } else {
            seen.since += dt;
        }
        seen.frame = frame;
        // Hit: the health the frame knows went down. The own body says by how much; a
        // body hit by the own hand has the zone's word (`hit`), and a body hit by
        // another's shows the spark alone.
        // (The own body has no name over its head to clear; its numbers start lower.)
        let over_head = f.feet + Vec3::Z * (f.chest + 26.0);
        if let (Some(was), Some(now)) = (seen.health, f.health)
            && now < was
        {
            self.sparks.push(Spark {
                at: f.feet + Vec3::Z * f.chest,
                age: 0.0,
            });
            self.flashes.insert(f.key, 1.0);
            if f.side == Side::Own {
                self.own_hurt = 1.0;
                self.numbers.push(Number {
                    at: over_head,
                    amount: (was - now) as u32,
                    blow: Blow::Taken,
                    age: 0.0,
                });
            }
        }
        // Healed: the own health went up, from a body that was alive (a respawn is not a
        // healing).
        if let (Some(was), Some(now)) = (seen.health, f.health)
            && now > was
            && was > 0
            && f.side == Side::Own
        {
            self.numbers.push(Number {
                at: over_head,
                amount: (now - was) as u32,
                blow: Blow::Healed,
                age: 0.0,
            });
        }
        seen.health = f.health;
        let Some(swing) = f.swing else { return };
        match f.anim {
            anim::WINDUP => {
                let filled = (seen.since / swing.windup.max(0.02)).clamp(0.0, 1.0);
                wedge_on_the_floor(mesh, f.feet, f.yaw, &swing, f.side, filled, 1.0);
            }
            anim::SWING if entered => self.slashes.push(Slash {
                centre: f.feet,
                chest: f.chest,
                yaw: f.yaw,
                swing,
                side: f.side,
                age: 0.0,
            }),
            _ => {}
        }
    }

    /// A bolt in flight, as the snapshot has it. `ink` is its colour (its damage's, when
    /// the frame knows what it is). Its streak grows behind it from where it was first
    /// seen: never back through its shooter, or into the camera behind the shoulder.
    #[allow(clippy::too_many_arguments)]
    pub fn projectile(
        &mut self,
        id: u32,
        pos: Vec3,
        vel: Vec3,
        radius: f32,
        ink: Rgba,
        eye: Vec3,
        mesh: &mut FxMesh,
    ) {
        let frame = self.frame;
        let seen = self.bolts.entry(id).or_insert((pos, frame));
        seen.1 = frame;
        let flown = (pos - seen.0).length();
        bolt(mesh, pos, vel, radius, ink, eye, flown);
    }

    /// The own body let a bolt go, as its prediction says: shown at once, from the hand.
    pub fn launch(&mut self, pos: Vec3, vel: Vec3, radius: f32, ink: Rgba) {
        self.tracers.push(Tracer {
            from: pos,
            pos,
            vel,
            radius,
            ink,
            age: 0.0,
        });
    }

    /// An area on the floor: a disc with a rim and a ring running outward; a burst the
    /// frame it is first seen.
    pub fn area(&mut self, id: u32, pos: Vec3, radius: f32, harmful: bool, mesh: &mut FxMesh) {
        let frame = self.frame;
        let fresh = self.areas.get(&id).is_none_or(|(_, last)| last + 1 < frame);
        let age = if fresh {
            self.bursts.push(Burst {
                at: pos,
                radius,
                harmful,
                age: 0.0,
            });
            0.0
        } else {
            self.areas[&id].0 + self.dt
        };
        self.areas.insert(id, (age, frame));
        let ink = if harmful { HARM } else { HELP };
        let at = pos + Vec3::Z * 0.6;
        let all = (0.0, 360.0);
        mesh.sector(at, all, (0.0, radius), fade(ink, 0.10), fade(ink, 0.26));
        let rim = (radius * 0.06).clamp(1.5, 4.0);
        mesh.sector(
            at + Vec3::Z * 0.1,
            all,
            (radius - rim, radius),
            fade(ink, 0.9),
            fade(ink, 0.9),
        );
        let wave = (age * 0.9).fract();
        let r = radius * wave;
        mesh.sector(
            at + Vec3::Z * 0.2,
            all,
            ((r - rim).max(0.0), r),
            fade(ink, 0.0),
            fade(ink, 0.55 * (1.0 - wave)),
        );
    }

    /// What is in the air: the slashes, the sparks, the bursts, the own tracers.
    pub fn draw(&self, eye: Vec3, mesh: &mut FxMesh) {
        for d in &self.decals {
            let left = ((DECAL_SECS - d.age) / DECAL_FADE).clamp(0.0, 1.0);
            mesh.disc(
                d.at,
                d.normal,
                DECAL_RADIUS,
                [0.02, 0.02, 0.02, 0.85 * left],
            );
        }
        for t in &self.tracers {
            let left = 1.0 - t.age / TRACER_SECS;
            let flown = (t.pos - t.from).length();
            bolt(
                mesh,
                t.pos,
                t.vel,
                t.radius,
                fade(t.ink, left.sqrt()),
                eye,
                flown,
            );
        }
        for s in &self.slashes {
            let sweep = s.swing.active.max(SWEEP_SECS);
            let gone = ((s.age - sweep) / SLASH_LINGER).clamp(0.0, 1.0);
            // The floor under it, lit while the blade passes: the place that was hit.
            wedge_on_the_floor(
                mesh,
                s.centre,
                s.yaw,
                &s.swing,
                s.side,
                1.0,
                (1.0 - gone) * 0.9,
            );
            // The blade's way at chest height, from the right to the left: bright at its
            // leading edge, thinning behind it. Flat, as it is seen from above and from
            // behind the eyes, and standing at the reach itself, as it is seen from the
            // side: the far edge of what was hit.
            let lead = (s.age / sweep).clamp(0.0, 1.0);
            let half = s.swing.arc_deg * 0.5;
            let ink = s.side.ink();
            let strength = 1.0 - gone;
            let swept = (s.yaw - half, s.yaw - half + s.swing.arc_deg * lead);
            let centre = s.centre + Vec3::Z * s.chest;
            let white = [1.0, 1.0, 1.0, 1.0];
            let along = |u: f32| (0.12 + 0.88 * u * u) * strength;
            mesh.sector_with(centre, swept, (s.swing.reach * 0.7, s.swing.reach), |u| {
                (fade(ink, 0.0), fade(white, along(u) * 0.5))
            });
            let tall = (s.swing.reach * 0.09).clamp(4.0, 8.0);
            mesh.band_with(centre, swept, s.swing.reach, (-tall, 0.0), |u| {
                (fade(ink, 0.0), fade(white, along(u) * 0.9))
            });
            mesh.band_with(centre, swept, s.swing.reach, (0.0, tall), |u| {
                (fade(white, along(u) * 0.9), fade(ink, 0.0))
            });
        }
        for s in &self.sparks {
            let t = s.age / SPARK_SECS;
            let r = 6.0 + 22.0 * t;
            mesh.glint(s.at, 9.0 * (1.0 - t) + 3.0, eye, [1.0, 1.0, 0.9, 1.0 - t]);
            for k in 0..6 {
                let a = k as f32 * std::f32::consts::TAU / 6.0 + 0.4;
                let to = (eye - s.at).normalize_or(Vec3::X);
                let right = to.cross(Vec3::Z).normalize_or(Vec3::Y);
                let up = right.cross(to);
                let dir = right * a.cos() + up * a.sin();
                mesh.streak(
                    s.at + dir * r * 0.45,
                    s.at + dir * r,
                    1.2,
                    eye,
                    [1.0, 0.8, 0.3, 0.0],
                    [1.0, 0.95, 0.7, 1.0 - t],
                );
            }
        }
        for b in &self.bursts {
            let t = b.age / BURST_SECS;
            let ink = if b.harmful { HARM } else { HELP };
            let grown = 1.0 - (1.0 - t) * (1.0 - t);
            let at = b.at + Vec3::Z * 0.8;
            mesh.sector(
                at,
                (0.0, 360.0),
                (0.0, b.radius * grown),
                fade(ink, 0.0),
                fade(ink, 0.55 * (1.0 - t)),
            );
            mesh.wall(
                at,
                b.radius * grown,
                30.0 * (1.0 - t * 0.5),
                fade(ink, 0.75 * (1.0 - t)),
                fade(ink, 0.0),
            );
        }
    }

    /// The frame ends: what was not seen in it is forgotten.
    pub fn end(&mut self) {
        let frame = self.frame;
        self.seen.retain(|_, s| s.frame == frame);
        self.areas.retain(|_, (_, last)| *last == frame);
        self.bolts.retain(|_, (_, last)| *last == frame);
    }

    /// How brightly a body that was just hit is lit: 1 down to 0.
    pub fn flash(&self, key: u32) -> f32 {
        self.flashes.get(&key).copied().unwrap_or(0.0).max(0.0)
    }
}

/// A bolt: a streak along its way and a bright head in a halo, large enough to be
/// followed by the one who shot it and sees it end-on. The streak is as long as the
/// bolt is fast, and never longer than the `flown` units it has come: a bolt just let go
/// is a head at the hand, not a line from behind the shooter's back.
fn bolt(mesh: &mut FxMesh, pos: Vec3, vel: Vec3, radius: f32, ink: Rgba, eye: Vec3, flown: f32) {
    let speed = vel.length();
    let core = [
        0.5 + 0.5 * ink[0],
        0.5 + 0.5 * ink[1],
        0.5 + 0.5 * ink[2],
        ink[3],
    ];
    let length = (speed * 0.07).clamp(14.0, STREAK_MAX).min(flown);
    if speed > 1.0 && length > 1.0 {
        let tail = pos - vel / speed * length;
        mesh.streak(
            tail,
            pos,
            radius.max(2.0) * 1.2,
            eye,
            fade(ink, 0.0),
            fade(core, 0.9),
        );
    }
    let r = radius.max(2.5);
    mesh.glint(pos, r * 5.0, eye, fade(ink, 0.35));
    mesh.glint(pos, r * 2.4, eye, core);
}

/// The ring at a body's feet in the colours of its aspects (MODELS.md 9: whatever a body
/// wears, its elements are read at a glance): one aspect the whole ring, two a half each.
pub fn aspect_ring(mesh: &mut FxMesh, feet: Vec3, yaw: f32, aspects: u8) {
    let which: Vec<usize> = (0..5).filter(|i| aspects & (1 << i) != 0).take(2).collect();
    let at = feet + Vec3::Z * 0.5;
    for (k, a) in which.iter().enumerate() {
        let c = crate::avatars::ASPECT_COLOURS[*a];
        let ink = [c[0], c[1], c[2], 0.85];
        let angles = match (which.len(), k) {
            (1, _) => (0.0, 360.0),
            (_, 0) => (yaw, yaw + 180.0),
            _ => (yaw + 180.0, yaw + 360.0),
        };
        mesh.sector(at, angles, (11.0, 14.0), ink, ink);
        mesh.sector(at, angles, (8.5, 11.0), fade(ink, 0.0), fade(ink, 0.45));
    }
}

/// The wedge a swing hits, on the floor at `feet`: its outline, a faint fill, and a
/// brighter fill out to `filled` of its reach (the windup running out).
fn wedge_on_the_floor(
    mesh: &mut FxMesh,
    feet: Vec3,
    yaw: f32,
    swing: &Swing,
    side: Side,
    filled: f32,
    strength: f32,
) {
    if strength <= 0.0 {
        return;
    }
    let ink = side.ink();
    let at = feet + Vec3::Z * 0.7;
    let half = swing.arc_deg * 0.5;
    let angles = (yaw - half, yaw + half);
    let reach = swing.reach;
    mesh.sector(
        at,
        angles,
        (0.0, reach),
        fade(ink, 0.05 * strength),
        fade(ink, 0.12 * strength),
    );
    mesh.sector(
        at + Vec3::Z * 0.1,
        angles,
        (0.0, reach * filled),
        fade(ink, 0.08 * strength),
        fade(ink, 0.30 * strength),
    );
    // The outline: the far edge and the two sides.
    let line = fade(ink, 0.7 * strength);
    let rim = (reach * 0.03).clamp(1.0, 2.5);
    mesh.sector(at + Vec3::Z * 0.2, angles, (reach - rim, reach), line, line);
    for a in [angles.0, angles.1] {
        let (sin, cos) = a.to_radians().sin_cos();
        mesh.line(
            at + Vec3::Z * 0.2,
            at + Vec3::new(cos * reach, sin * reach, 0.2),
            rim,
            line,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWORD: Swing = Swing {
        reach: 72.0,
        arc_deg: 90.0,
        windup: 0.09,
        active: 0.045,
    };

    fn body(anim: u8, health: Option<u16>) -> Fighter {
        Fighter {
            key: 7,
            feet: Vec3::new(100.0, 50.0, 0.0),
            chest: 30.0,
            yaw: 90.0,
            anim,
            swing: Some(SWORD),
            side: Side::Foe,
            health,
        }
    }

    /// Every vertex of what a body's frame drew.
    fn frame(fx: &mut Effects, f: &Fighter, dt: f32) -> Vec<FxVertex> {
        let mut mesh = FxMesh::default();
        fx.begin(dt);
        fx.fighter(f, &mut mesh);
        fx.draw(Vec3::new(0.0, 0.0, 60.0), &mut mesh);
        fx.end();
        mesh.verts
    }

    #[test]
    fn a_sector_stays_between_its_radii_and_its_angles() {
        let mut mesh = FxMesh::default();
        let c = Vec3::new(10.0, -4.0, 3.0);
        mesh.sector(c, (45.0, 135.0), (20.0, 72.0), [1.0; 4], [1.0; 4]);
        assert_eq!(mesh.verts.len() % 3, 0);
        assert!(mesh.verts.len() >= 6 * 12, "a segment every 7.5 degrees");
        for v in &mesh.verts {
            let p = Vec3::from(v.pos) - c;
            let (r, a) = (p.truncate().length(), p.y.atan2(p.x).to_degrees());
            assert!((19.99..=72.01).contains(&r), "radius {r}");
            assert!((44.9..=135.1).contains(&a), "angle {a}");
            assert_eq!(p.z, 0.0);
        }
        // Nothing for an empty one.
        let n = mesh.verts.len();
        mesh.sector(c, (10.0, 10.0), (0.0, 5.0), [1.0; 4], [1.0; 4]);
        mesh.sector(c, (0.0, 90.0), (5.0, 5.0), [1.0; 4], [1.0; 4]);
        assert_eq!(mesh.verts.len(), n);
    }

    #[test]
    fn a_windup_is_drawn_where_the_swing_will_land_and_nowhere_else() {
        let mut fx = Effects::default();
        assert!(frame(&mut fx, &body(anim::IDLE, None), 0.016).is_empty());
        let drawn = frame(&mut fx, &body(anim::WINDUP, None), 0.016);
        assert!(!drawn.is_empty());
        let feet = body(anim::WINDUP, None).feet;
        for v in &drawn {
            let p = Vec3::from(v.pos) - feet;
            // The body faces +Y: the wedge is 45 degrees either side of it, out to the
            // reach, on the floor.
            assert!(p.truncate().length() <= SWORD.reach + 0.01);
            assert!(p.y >= -0.9 && p.x.abs() <= p.y + 2.0, "{p:?}");
            assert!(p.z < 2.0);
        }
    }

    #[test]
    fn a_swing_leaves_one_slash_that_sweeps_and_fades() {
        let mut fx = Effects::default();
        frame(&mut fx, &body(anim::WINDUP, None), 0.016);
        let first = frame(&mut fx, &body(anim::SWING, None), 0.016);
        assert_eq!(fx.slashes.len(), 1);
        // Still swinging the frame after: no second slash.
        frame(&mut fx, &body(anim::SWING, None), 0.016);
        assert_eq!(fx.slashes.len(), 1);
        // At chest height something is drawn, and more of it once the blade has passed.
        let at_chest = |verts: &[FxVertex]| verts.iter().filter(|v| v.pos[2] > 20.0).count();
        let later = frame(&mut fx, &body(anim::RECOVER, None), 0.08);
        assert!(at_chest(&later) > at_chest(&first));
        // It is gone within half a second, though the body stands there still.
        for _ in 0..30 {
            frame(&mut fx, &body(anim::IDLE, None), 0.016);
        }
        assert!(fx.slashes.is_empty());
        assert!(frame(&mut fx, &body(anim::IDLE, None), 0.016).is_empty());
        // A second swing is a second slash.
        frame(&mut fx, &body(anim::SWING, None), 0.016);
        assert_eq!(fx.slashes.len(), 1);
    }

    #[test]
    fn a_body_whose_health_drops_sparks_and_is_lit() {
        let mut fx = Effects::default();
        frame(&mut fx, &body(anim::IDLE, Some(100)), 0.016);
        assert_eq!(fx.flash(7), 0.0);
        let drawn = frame(&mut fx, &body(anim::IDLE, Some(80)), 0.016);
        assert!(!drawn.is_empty());
        assert_eq!(fx.flash(7), 1.0);
        assert_eq!(fx.own_hurt, 0.0, "it was not the own body");
        // Healing is no hit; neither is a health that stops being told.
        frame(&mut fx, &body(anim::IDLE, Some(95)), 0.5);
        frame(&mut fx, &body(anim::IDLE, None), 0.5);
        assert!(frame(&mut fx, &body(anim::IDLE, Some(60)), 0.5).is_empty());
        let mut own = body(anim::IDLE, Some(50));
        own.side = Side::Own;
        frame(&mut fx, &own, 0.016);
        assert_eq!(fx.own_hurt, 1.0);
    }

    #[test]
    fn an_area_bursts_once_and_lies_on_the_floor() {
        let mut fx = Effects::default();
        let mut mesh = FxMesh::default();
        for _ in 0..3 {
            fx.begin(0.016);
            fx.area(3, Vec3::new(0.0, 0.0, 10.0), 64.0, true, &mut mesh);
            fx.end();
        }
        assert_eq!(fx.bursts.len(), 1);
        for v in &mesh.verts {
            let p = Vec3::from(v.pos);
            assert!(p.truncate().length() <= 64.01 && (10.0..12.0).contains(&p.z));
        }
        // Not seen for a frame, then again: another area as far as anyone can tell.
        fx.begin(0.016);
        fx.end();
        fx.begin(0.016);
        fx.area(3, Vec3::ZERO, 64.0, true, &mut mesh);
        assert_eq!(fx.bursts.len(), 2);
    }

    #[test]
    fn a_bolt_is_a_streak_behind_its_head() {
        let mut fx = Effects::default();
        let mut mesh = FxMesh::default();
        let (pos, vel) = (Vec3::new(50.0, 0.0, 30.0), Vec3::new(900.0, 0.0, 0.0));
        let eye = Vec3::new(0.0, -200.0, 40.0);
        // Where it is first seen, it is a head alone: the streak grows as it flies, and
        // never reaches back behind its shooter (the camera, behind the shoulder).
        fx.begin(0.016);
        fx.projectile(9, pos, vel, 3.0, BOLT, eye, &mut mesh);
        let behind = mesh.verts.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!(behind >= pos.x - 15.1, "a head alone at first: {behind}");
        fx.end();
        fx.begin(0.016);
        let mut mesh = FxMesh::default();
        let pos = pos + Vec3::X * 200.0;
        fx.projectile(9, pos, vel, 3.0, BOLT, eye, &mut mesh);
        let behind = mesh.verts.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!((pos.x - 64.0..pos.x - 40.0).contains(&behind), "{behind}");
        // Its head is a halo several times its size, so it is seen end-on too.
        let wide = mesh
            .verts
            .iter()
            .map(|v| (Vec3::from(v.pos) - pos).length())
            .fold(0.0, f32::max);
        assert!(wide >= 15.0, "{wide}");
        // Standing still (its first sample): the head alone, its two diamonds.
        let n = mesh.verts.len();
        fx.projectile(10, pos, Vec3::ZERO, 3.0, BOLT, eye, &mut mesh);
        assert_eq!(mesh.verts.len() - n, 24);
        // The own shot is drawn from the hand at once, flies, trailing a streak that
        // starts at the hand, and is gone when the zone's bolt has had time to show.
        fx.launch(pos, vel, 3.0, BOLT);
        let mut mesh = FxMesh::default();
        fx.begin(0.05);
        fx.draw(eye, &mut mesh);
        let ahead = mesh.verts.iter().map(|v| v.pos[0]).fold(f32::MIN, f32::max);
        assert!(ahead > pos.x + 40.0, "it flew: {ahead}");
        let behind = mesh.verts.iter().map(|v| v.pos[0]).fold(f32::MAX, f32::min);
        assert!(behind >= pos.x - 0.1, "from the hand: {behind}");
        fx.begin(0.2);
        let mut mesh = FxMesh::default();
        fx.draw(eye, &mut mesh);
        assert!(mesh.verts.is_empty());
    }

    #[test]
    fn numbers_float_up_and_fade() {
        let mut fx = Effects::default();
        let mut mesh = FxMesh::default();
        let own = |health: u16| Fighter {
            side: Side::Own,
            ..body(anim::IDLE, Some(health))
        };
        let me = own(100);
        fx.begin(0.016);
        fx.fighter(&me, &mut mesh);
        // The own body loses 30: "-30" in red. Another's hand on another body says
        // nothing of itself; the zone's word of the own hand's blow is gold.
        fx.begin(0.016);
        fx.fighter(&own(70), &mut mesh);
        let other = Fighter {
            key: 2,
            side: Side::Foe,
            health: Some(50),
            ..me
        };
        fx.fighter(&other, &mut mesh);
        fx.fighter(
            &Fighter {
                health: Some(20),
                ..other
            },
            &mut mesh,
        );
        fx.hit(Vec3::new(100.0, 0.0, 70.0), 30, 0);
        fx.hit(Vec3::new(100.0, 0.0, 70.0), 0, 12);
        let pops = fx.numbers();
        let texts: Vec<&str> = pops.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["-30", "30", "blocked"]);
        assert_eq!(pops[0].blow, Blow::Taken);
        assert_eq!(pops[1].blow, Blow::Dealt);
        assert!(pops.iter().all(|p| p.lift < 0.05 && p.ink[3] > 0.99));
        // Healed from alive: green. Up from dead (a respawn): not a healing.
        fx.begin(0.016);
        fx.fighter(&own(90), &mut mesh);
        assert_eq!(fx.numbers()[3].text, "+20");
        fx.begin(0.016);
        fx.fighter(&own(0), &mut mesh);
        fx.begin(0.016);
        fx.fighter(&own(100), &mut mesh);
        assert_eq!(fx.numbers().len(), 5, "-90 and no +100");
        // Half way it has floated most of its way; at the end it is faded and gone.
        // (A frame is at most a quarter of a second to the effects.)
        fx.begin(0.25);
        fx.begin(0.25);
        let p = &fx.numbers()[0];
        assert!(p.lift > 0.7 && p.lift < 0.9, "{}", p.lift);
        fx.begin(0.25);
        fx.begin(0.25);
        let p = &fx.numbers()[0];
        assert!(p.ink[3] < 0.5 && p.ink[3] > 0.0, "{}", p.ink[3]);
        fx.begin(0.2);
        assert!(fx.numbers().is_empty());
    }
}
