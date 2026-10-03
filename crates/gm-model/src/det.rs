//! Arithmetic the two builds and every machine agree on, bit for bit (SOUND.md 2, CONTENT.md
//! 5.3): the platform's `sin`, `exp`, `ln` and `powf` differ in their last bits between
//! libms, so whatever must hash the same everywhere (the sound's patches, a baked icon, an
//! sRGB curve on the way to a `.gmm`) uses these: IEEE basics and short series only.
#![allow(clippy::excessive_precision)]

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
