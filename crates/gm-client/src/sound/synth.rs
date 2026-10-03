//! Patches: every sound the game makes, rendered once at start from a few parts each
//! (SOUND.md 2). No file is read, no byte is downloaded, and the two builds make the same
//! samples: the arithmetic here is IEEE basics only (no `sin`, `exp` or `powf` of a
//! platform's libm, which differ in their last bit between the desktop and the browser),
//! so a test pins each patch by a hash of what it renders to, and the browser reports the
//! hash of all of them.

/// The rate every patch is rendered at.
pub const RATE: u32 = 22_050;

/// Loops (the air of a map) are this long, and seamless.
pub const LOOP_SECS: f32 = 4.0;

/// Deterministic arithmetic: the same bits on every platform.
pub mod det {
    pub const PI: f32 = std::f32::consts::PI;
    pub const TAU: f32 = std::f32::consts::TAU;
    const LN2: f32 = std::f32::consts::LN_2;

    /// `sin(x)` for any `x`, by reduction to `[-pi, pi]` and a polynomial.
    pub fn sin(x: f32) -> f32 {
        // Reduce to [-pi, pi].
        let k = (x / TAU + 0.5).floor();
        let x = x - k * TAU;
        // Taylor to the eleventh power, reduced once more to [-pi/2, pi/2] by symmetry.
        let x = if x > PI / 2.0 {
            PI - x
        } else if x < -PI / 2.0 {
            -PI - x
        } else {
            x
        };
        let x2 = x * x;
        x * (1.0
            + x2 * (-1.0 / 6.0
                + x2 * (1.0 / 120.0
                    + x2 * (-1.0 / 5040.0 + x2 * (1.0 / 362_880.0 + x2 * (-1.0 / 39_916_800.0))))))
    }

    pub fn cos(x: f32) -> f32 {
        sin(x + PI / 2.0)
    }

    /// `exp(x)` by halving the argument into `[-0.5, 0.5]`, a short series, and squaring
    /// back.
    pub fn exp(x: f32) -> f32 {
        let x = x.clamp(-80.0, 80.0);
        let mut halvings = 0;
        let mut y = x;
        while y.abs() > 0.5 {
            y *= 0.5;
            halvings += 1;
        }
        let mut term = 1.0_f32;
        let mut sum = 1.0_f32;
        for n in 1..12 {
            term *= y / n as f32;
            sum += term;
        }
        for _ in 0..halvings {
            sum *= sum;
        }
        sum
    }

    /// `ln(x)` for `x > 0`: a power of two apart, then the series of `atanh`.
    pub fn ln(x: f32) -> f32 {
        if x <= 0.0 {
            return -80.0;
        }
        let mut m = x;
        let mut k = 0_i32;
        while m >= 2.0 {
            m *= 0.5;
            k += 1;
        }
        while m < 1.0 {
            m *= 2.0;
            k -= 1;
        }
        // m in [1, 2): ln(m) = 2 atanh((m - 1) / (m + 1)), |t| <= 1/3.
        let t = (m - 1.0) / (m + 1.0);
        let t2 = t * t;
        let mut term = t;
        let mut sum = 0.0_f32;
        for n in 0..12 {
            sum += term / (2 * n + 1) as f32;
            term *= t2;
        }
        2.0 * sum + k as f32 * LN2
    }

    /// `base^e` for `base > 0`.
    pub fn pow(base: f32, e: f32) -> f32 {
        exp(e * ln(base))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Sine,
    Triangle,
    /// Band-limited: its harmonics stop under 6 kHz (a wavetable made for the part's
    /// lowest frequency).
    Saw,
    Noise,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Low,
    Band,
    High,
}

/// A two-pole state-variable filter with a cutoff that may sweep over the part.
#[derive(Clone, Copy, Debug)]
pub struct Filter {
    pub mode: Mode,
    pub from: f32,
    pub to: f32,
    /// Resonance: 0.3 sharp, 1.4 none.
    pub q: f32,
}

/// One part of a patch: a source at a pitch (which may sweep and shake), shaped by an
/// envelope and a filter.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub source: Source,
    /// Hz; ignored by noise.
    pub freq: f32,
    /// Sweep to this frequency over the part's length (exponential).
    pub sweep_to: Option<f32>,
    /// Vibrato: (rate in Hz, depth as a fraction of the pitch).
    pub vibrato: Option<(f32, f32)>,
    pub attack_ms: f32,
    pub hold_ms: f32,
    /// To -60 dB.
    pub decay_ms: f32,
    pub gain: f32,
    pub filter: Option<Filter>,
}

impl Part {
    const fn new(source: Source, freq: f32) -> Part {
        Part {
            source,
            freq,
            sweep_to: None,
            vibrato: None,
            attack_ms: 2.0,
            hold_ms: 0.0,
            decay_ms: 100.0,
            gain: 1.0,
            filter: None,
        }
    }

    const fn sweep(mut self, to: f32) -> Part {
        self.sweep_to = Some(to);
        self
    }

    const fn vibrato(mut self, hz: f32, depth: f32) -> Part {
        self.vibrato = Some((hz, depth));
        self
    }

    const fn env(mut self, attack_ms: f32, hold_ms: f32, decay_ms: f32) -> Part {
        self.attack_ms = attack_ms;
        self.hold_ms = hold_ms;
        self.decay_ms = decay_ms;
        self
    }

    const fn gain(mut self, gain: f32) -> Part {
        self.gain = gain;
        self
    }

    const fn filter(mut self, mode: Mode, from: f32, to: f32, q: f32) -> Part {
        self.filter = Some(Filter { mode, from, to, q });
        self
    }

    fn millis(&self) -> f32 {
        self.attack_ms + self.hold_ms + self.decay_ms
    }
}

/// A patch: its name (what a report calls it), its parts, its level under the master
/// (a mix: a step is quieter than a death), and whether it is a loop.
#[derive(Clone, Copy, Debug)]
pub struct Patch {
    pub name: &'static str,
    pub parts: &'static [Part],
    pub level: f32,
    pub looped: bool,
}

/// Every patch, in the order a cue names them (`Cue as usize`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Cue {
    StepA,
    StepB,
    Land,
    Dash,
    Swing,
    Hit,
    Stagger,
    Parry,
    Cast,
    Launch,
    Impact,
    Burst,
    Hurt,
    Death,
    Click,
    Blip,
    Chime,
    Wind,
    Murmur,
    Drone,
}

pub const CUES: usize = 20;

impl Cue {
    #[cfg(test)]
    pub const ALL: [Cue; CUES] = [
        Cue::StepA,
        Cue::StepB,
        Cue::Land,
        Cue::Dash,
        Cue::Swing,
        Cue::Hit,
        Cue::Stagger,
        Cue::Parry,
        Cue::Cast,
        Cue::Launch,
        Cue::Impact,
        Cue::Burst,
        Cue::Hurt,
        Cue::Death,
        Cue::Click,
        Cue::Blip,
        Cue::Chime,
        Cue::Wind,
        Cue::Murmur,
        Cue::Drone,
    ];

    pub fn patch(self) -> Patch {
        PATCHES[self as usize]
    }

    #[cfg(test)]
    pub fn name(self) -> &'static str {
        self.patch().name
    }

    pub fn is_loop(self) -> bool {
        self.patch().looped
    }
}

const fn one(name: &'static str, level: f32, parts: &'static [Part]) -> Patch {
    Patch {
        name,
        parts,
        level,
        looped: false,
    }
}

const fn looped(name: &'static str, level: f32, parts: &'static [Part]) -> Patch {
    Patch {
        name,
        parts,
        level,
        looped: true,
    }
}

/// The patches of SOUND.md 2, in `Cue`'s order.
pub static PATCHES: [Patch; CUES] = [
    one(
        "step_a",
        0.4,
        &[Part::new(Source::Noise, 0.0)
            .env(2.0, 8.0, 50.0)
            .filter(Mode::Low, 1100.0, 700.0, 0.8)],
    ),
    one(
        "step_b",
        0.4,
        &[Part::new(Source::Noise, 0.0)
            .env(2.0, 8.0, 50.0)
            .filter(Mode::Low, 900.0, 600.0, 0.8)],
    ),
    one(
        "land",
        0.6,
        &[
            Part::new(Source::Sine, 140.0)
                .sweep(40.0)
                .env(2.0, 8.0, 110.0),
            Part::new(Source::Noise, 0.0)
                .env(1.0, 4.0, 60.0)
                .gain(0.4)
                .filter(Mode::Low, 1800.0, 500.0, 1.0),
        ],
    ),
    one(
        "dash",
        0.5,
        &[Part::new(Source::Noise, 0.0).env(20.0, 60.0, 170.0).filter(
            Mode::Band,
            500.0,
            2500.0,
            0.5,
        )],
    ),
    one(
        "swing",
        0.5,
        &[Part::new(Source::Noise, 0.0).env(10.0, 30.0, 120.0).filter(
            Mode::Band,
            800.0,
            3000.0,
            0.4,
        )],
    ),
    one(
        "hit",
        0.8,
        &[
            Part::new(Source::Sine, 110.0)
                .sweep(70.0)
                .env(1.0, 10.0, 100.0),
            Part::new(Source::Noise, 0.0)
                .env(1.0, 6.0, 50.0)
                .gain(0.7)
                .filter(Mode::Low, 2500.0, 800.0, 0.9),
        ],
    ),
    one(
        "stagger",
        1.0,
        &[
            Part::new(Source::Sine, 90.0)
                .sweep(50.0)
                .env(1.0, 20.0, 160.0),
            Part::new(Source::Noise, 0.0)
                .env(1.0, 10.0, 90.0)
                .gain(0.8)
                .filter(Mode::Low, 3000.0, 600.0, 0.7),
        ],
    ),
    // A clang: inharmonic partials with fast decays of their own, and a click.
    one(
        "parry",
        0.7,
        &[
            Part::new(Source::Sine, 1320.0)
                .env(0.5, 2.0, 180.0)
                .gain(0.5),
            Part::new(Source::Sine, 1980.0)
                .env(0.5, 2.0, 140.0)
                .gain(0.35),
            Part::new(Source::Sine, 3564.0)
                .env(0.5, 1.0, 90.0)
                .gain(0.25),
            Part::new(Source::Sine, 5412.0)
                .env(0.5, 1.0, 50.0)
                .gain(0.15),
            Part::new(Source::Noise, 0.0)
                .env(0.5, 2.0, 15.0)
                .gain(0.6)
                .filter(Mode::High, 3000.0, 3000.0, 1.0),
        ],
    ),
    one(
        "cast",
        0.5,
        &[Part::new(Source::Sine, 330.0)
            .sweep(1320.0)
            .vibrato(9.0, 0.03)
            .env(30.0, 100.0, 170.0)],
    ),
    one(
        "launch",
        0.4,
        &[Part::new(Source::Sine, 600.0)
            .sweep(2400.0)
            .env(2.0, 18.0, 80.0)],
    ),
    one(
        "impact",
        0.8,
        &[
            Part::new(Source::Noise, 0.0).env(1.0, 10.0, 90.0).filter(
                Mode::Low,
                3500.0,
                1200.0,
                0.8,
            ),
            Part::new(Source::Sine, 1000.0)
                .sweep(100.0)
                .env(1.0, 8.0, 120.0)
                .gain(0.8),
        ],
    ),
    one(
        "burst",
        1.0,
        &[
            Part::new(Source::Sine, 60.0).env(5.0, 60.0, 330.0),
            Part::new(Source::Noise, 0.0)
                .env(5.0, 40.0, 200.0)
                .gain(0.5)
                .filter(Mode::Low, 300.0, 150.0, 0.9),
        ],
    ),
    one(
        "hurt",
        0.7,
        &[Part::new(Source::Saw, 110.0)
            .vibrato(14.0, 0.06)
            .env(5.0, 25.0, 120.0)
            .filter(Mode::Low, 1500.0, 800.0, 0.8)],
    ),
    one(
        "death",
        1.0,
        &[
            Part::new(Source::Saw, 220.0)
                .sweep(55.0)
                .env(10.0, 90.0, 500.0)
                .filter(Mode::Low, 1800.0, 400.0, 0.8),
            Part::new(Source::Noise, 0.0)
                .env(10.0, 100.0, 300.0)
                .gain(0.3)
                .filter(Mode::Low, 800.0, 300.0, 1.0),
        ],
    ),
    // A tick: a short, high noise.
    one(
        "click",
        0.2,
        &[Part::new(Source::Noise, 0.0).env(0.5, 2.5, 12.0).filter(
            Mode::High,
            2500.0,
            2500.0,
            0.8,
        )],
    ),
    one(
        "blip",
        0.25,
        &[Part::new(Source::Triangle, 880.0).env(2.0, 8.0, 40.0)],
    ),
    one(
        "chime",
        0.4,
        &[
            Part::new(Source::Sine, 660.0).env(2.0, 30.0, 90.0),
            Part::new(Source::Sine, 990.0)
                .env(130.0, 30.0, 100.0)
                .gain(0.9),
        ],
    ),
    looped(
        "wind",
        0.5,
        &[Part::new(Source::Noise, 0.0).filter(Mode::Low, 500.0, 500.0, 0.9)],
    ),
    looped(
        "murmur",
        0.4,
        &[Part::new(Source::Noise, 0.0).filter(Mode::Band, 600.0, 600.0, 0.6)],
    ),
    looped(
        "drone",
        0.5,
        &[
            Part::new(Source::Saw, 55.0).filter(Mode::Low, 200.0, 200.0, 0.9),
            Part::new(Source::Saw, 55.5).filter(Mode::Low, 200.0, 200.0, 0.9),
            Part::new(Source::Saw, 110.0)
                .filter(Mode::Low, 200.0, 200.0, 0.9)
                .gain(0.5),
        ],
    ),
];

/// A small, fast, seedable noise.
struct XorShift(u32);

impl XorShift {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// FNV-1a over a name: the seed of its noise, so that every build makes the same.
fn seed(name: &str, part: usize) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes().chain([part as u8]) {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h | 1
}

/// Harmonics of a band-limited saw stop under this.
const SAW_TOP_HZ: f32 = 6000.0;
const TABLE: usize = 2048;

/// One period of a saw whose harmonics stop under `SAW_TOP_HZ` at `lowest` Hz.
fn saw_table(lowest: f32) -> Vec<f32> {
    let harmonics = ((SAW_TOP_HZ / lowest.max(20.0)).floor() as usize).clamp(1, 400);
    let mut table = vec![0.0_f32; TABLE];
    for (i, v) in table.iter_mut().enumerate() {
        let phase = i as f32 / TABLE as f32;
        let mut sum = 0.0;
        for n in 1..=harmonics {
            let sign = if n % 2 == 0 { -1.0 } else { 1.0 };
            sum += sign * det::sin(det::TAU * phase * n as f32) / n as f32;
        }
        *v = sum * (2.0 / det::PI);
    }
    table
}

/// Render a patch to mono samples at `RATE`, its peak at 0.8 times its level.
pub fn render(patch: &Patch) -> Vec<f32> {
    let secs = if patch.looped {
        LOOP_SECS
    } else {
        patch.parts.iter().map(Part::millis).fold(0.0_f32, f32::max) / 1000.0
    };
    let len = (secs * RATE as f32).round() as usize;
    let mut out = vec![0.0_f32; len];
    for (i, part) in patch.parts.iter().enumerate() {
        render_part(patch, part, seed(patch.name, i), &mut out);
    }
    if patch.looped {
        fold_tail(&mut out);
        // The loop's slow movement (wind that gusts, a crowd that swells) is a pair of
        // slow waves over the whole loop, whole numbers of periods so that it joins.
        let n = out.len() as f32;
        for (i, v) in out.iter_mut().enumerate() {
            let t = i as f32 / n;
            let gust = 0.65
                + 0.25 * det::sin(t * det::TAU * 3.0)
                + 0.10 * det::sin(t * det::TAU * 7.0 + 1.0);
            *v *= gust;
        }
    }
    normalise(&mut out, 0.8 * patch.level);
    out
}

fn render_part(patch: &Patch, part: &Part, seed: u32, out: &mut [f32]) {
    let rate = RATE as f32;
    let total = if patch.looped {
        out.len()
    } else {
        ((part.millis() / 1000.0) * rate).round() as usize
    }
    .min(out.len());
    if total == 0 {
        return;
    }
    let mut noise = XorShift(seed);
    let table = (part.source == Source::Saw)
        .then(|| saw_table(part.freq.min(part.sweep_to.unwrap_or(part.freq))));
    let mut phase = 0.0_f32;
    let (mut low, mut band) = (0.0_f32, 0.0_f32);
    let attack = (part.attack_ms / 1000.0 * rate).max(1.0);
    let hold = part.hold_ms / 1000.0 * rate;
    let decay = (part.decay_ms / 1000.0 * rate).max(1.0);
    // -60 dB at the end of the decay: a multiply a sample.
    let fall = det::exp(-6.9078 / decay);
    let mut level = 1.0_f32;
    // A sweep is exponential: a multiply a sample too.
    let sweep = part
        .sweep_to
        .filter(|_| part.freq > 0.0)
        .map_or(1.0, |to| det::pow(to / part.freq, 1.0 / total as f32));
    let mut freq = part.freq;
    let cutoff_sweep = part
        .filter
        .map_or(1.0, |f| det::pow(f.to / f.from, 1.0 / total as f32));
    let mut cutoff = part.filter.map_or(0.0, |f| f.from);
    for (i, slot) in out.iter_mut().enumerate().take(total) {
        let mut f = freq;
        if let Some((hz, depth)) = part.vibrato {
            f *= 1.0 + depth * det::sin(i as f32 / rate * hz * det::TAU);
        }
        let v = match part.source {
            Source::Noise => noise.next(),
            Source::Sine => det::sin(phase * det::TAU),
            Source::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            Source::Saw => {
                let t = table.as_ref().expect("a saw has its table");
                let x = phase * TABLE as f32;
                let j = x as usize % TABLE;
                let frac = x - x.floor();
                t[j] * (1.0 - frac) + t[(j + 1) % TABLE] * frac
            }
        };
        phase += f / rate;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        freq *= sweep;
        // The envelope (a loop has none: it is shaped after).
        let env = if patch.looped {
            1.0
        } else {
            let i = i as f32;
            if i < attack {
                i / attack
            } else if i < attack + hold {
                1.0
            } else {
                level *= fall;
                level
            }
        };
        let mut s = v * env * part.gain;
        // The filter: two poles, a cutoff that sweeps, a resonance.
        if let Some(filter) = part.filter {
            let fc = cutoff.clamp(20.0, rate / 6.0);
            let fk = 2.0 * det::sin(det::PI * fc / rate);
            low += fk * band;
            let high = s - low - filter.q * band;
            band += fk * high;
            s = match filter.mode {
                Mode::Low => low,
                Mode::Band => band,
                Mode::High => high,
            };
            cutoff *= cutoff_sweep;
        }
        *slot += s;
    }
}

/// Fold the last tenth of a loop over its first tenth with an equal-power crossfade, so
/// that its end joins its beginning without a click; the loop ends where the fold began.
fn fold_tail(out: &mut Vec<f32>) {
    let n = out.len();
    let fade = n / 10;
    if fade == 0 {
        return;
    }
    for i in 0..fade {
        let t = i as f32 / fade as f32;
        let (a, b) = (det::sin(t * det::PI / 2.0), det::cos(t * det::PI / 2.0));
        let tail = out[n - fade + i];
        out[i] = out[i] * a + tail * b;
    }
    out.truncate(n - fade);
}

fn normalise(out: &mut [f32], peak: f32) {
    let max = out.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
    if max > 0.0 {
        let k = peak / max;
        for v in out.iter_mut() {
            *v *= k;
        }
    }
}

/// Every patch rendered, in `Cue`'s order.
pub fn render_all() -> Vec<Vec<f32>> {
    PATCHES.iter().map(render).collect()
}

/// FNV-1a over the samples' bits: what a test pins a patch by, and what the browser
/// reports of all of them.
pub fn fingerprint(samples: &[f32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for s in samples {
        for b in s.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// One number for all the patches, in order.
pub fn fingerprint_all(patches: &[Vec<f32>]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in patches {
        let f = fingerprint(p);
        for b in f.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arithmetic_is_close_to_the_library_s() {
        for i in -200..200 {
            let x = i as f32 * 0.1;
            assert!((det::sin(x) - x.sin()).abs() < 2e-4, "sin {x}");
            if x.abs() < 20.0 {
                let rel = (det::exp(x) - x.exp()).abs() / x.exp();
                assert!(rel < 1e-4, "exp {x}: {rel}");
            }
            if x > 0.0 {
                assert!((det::ln(x) - x.ln()).abs() < 1e-4, "ln {x}");
            }
        }
        assert!((det::pow(2.0, 10.0) - 1024.0).abs() < 0.2);
        assert!((det::pow(0.25, 0.5) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn every_patch_renders_within_bounds_and_is_not_silent() {
        for cue in Cue::ALL {
            let p = cue.patch();
            let s = render(&p);
            assert!(!s.is_empty(), "{}", p.name);
            let secs = s.len() as f32 / RATE as f32;
            if p.looped {
                assert!((3.5..=4.0).contains(&secs), "{}: {secs} s", p.name);
            } else {
                assert!(secs <= 0.6 + 1e-3, "{}: {secs} s", p.name);
            }
            let peak = s.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
            let want = 0.8 * p.level;
            assert!(
                (peak - want).abs() < 0.01,
                "{}: peak {peak}, level {}",
                p.name,
                p.level
            );
            assert!(s.iter().all(|v| v.is_finite()), "{}", p.name);
            let energy: f32 = s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32;
            assert!(energy > 1e-5, "{}: energy {energy}", p.name);
        }
    }

    #[test]
    fn a_loop_joins_without_a_step() {
        for cue in [Cue::Wind, Cue::Murmur, Cue::Drone] {
            let s = render(&cue.patch());
            let inside = s
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0_f32, f32::max);
            let join = (s[0] - s[s.len() - 1]).abs();
            assert!(join <= inside, "{}: join {join} > {inside}", cue.name());
        }
    }

    #[test]
    fn the_same_patch_renders_to_the_same_bytes() {
        let a = render_all();
        let b = render_all();
        for (cue, (x, y)) in Cue::ALL.iter().zip(a.iter().zip(b.iter())) {
            assert_eq!(fingerprint(x), fingerprint(y), "{}", cue.name());
        }
        // Pinned: whoever changes a sound changes its number here, on purpose. (Printed
        // so that the new numbers can be read off a failing run.)
        let pinned: [(Cue, u64); CUES] = [
            (Cue::StepA, 0xff5a3f5710f4f72a),
            (Cue::StepB, 0x003f962d5728c917),
            (Cue::Land, 0xa55d2b4ee8ae803e),
            (Cue::Dash, 0x277bbaeb7a8e4fe8),
            (Cue::Swing, 0x425809ee90b0e259),
            (Cue::Hit, 0x4a7665b9279579a7),
            (Cue::Stagger, 0x8cbfc93a6687df30),
            (Cue::Parry, 0xe0167dd1c98eb951),
            (Cue::Cast, 0x682a56b5c4564c87),
            (Cue::Launch, 0xb00e6ff24543709f),
            (Cue::Impact, 0xee2201c833429acf),
            (Cue::Burst, 0xcfb7d89985c99bcf),
            (Cue::Hurt, 0x46629424de3ec7f2),
            (Cue::Death, 0xa1d0bdcb20e9aef5),
            (Cue::Click, 0xb7e468bbdc311a7b),
            (Cue::Blip, 0x09772684c57ad8ff),
            (Cue::Chime, 0x32ba7d90847eecbb),
            (Cue::Wind, 0x9067feb98da578c5),
            (Cue::Murmur, 0xb74bbd25495d6f5b),
            (Cue::Drone, 0xc3909e9fa05989be),
        ];
        let mut wrong = Vec::new();
        for (cue, want) in pinned {
            let got = fingerprint(&a[cue as usize]);
            if got != want {
                wrong.push(format!("(Cue::{cue:?}, 0x{got:016x})"));
            }
        }
        assert!(
            wrong.is_empty(),
            "fingerprints changed:\n{}\nall: 0x{:016x}",
            wrong.join(",\n"),
            fingerprint_all(&a)
        );
    }

    #[test]
    fn memory_of_the_rendered_patches_is_within_its_budget() {
        let began = web_time::Instant::now();
        let all = render_all();
        let bytes: usize = all.iter().map(|s| s.len() * 4).sum();
        assert!(bytes <= 2 * 1024 * 1024, "{bytes} bytes");
        println!(
            "patches: {} bytes rendered in {:.1} ms (this build)",
            bytes,
            began.elapsed().as_secs_f64() * 1000.0
        );
    }
}
