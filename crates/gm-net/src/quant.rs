//! Quantization (PROTOCOL.md 2). Both sides simulate on the dequantized values so the server
//! compares like with like when it reconciles.

/// 1/4 world unit per position quantum.
pub const POS_SCALE: f32 = 4.0;
/// 1/8 u/s per velocity quantum.
pub const VEL_SCALE: f32 = 8.0;
pub const YAW_BITS: u32 = 12;
pub const PITCH_BITS: u32 = 11;

pub fn quantize_pos(x: f32) -> i32 {
    (x * POS_SCALE).round() as i32
}

pub fn dequantize_pos(q: i32) -> f32 {
    q as f32 / POS_SCALE
}

pub fn quantize_vel(v: f32) -> i32 {
    (v * VEL_SCALE).round() as i32
}

pub fn dequantize_vel(q: i32) -> f32 {
    q as f32 / VEL_SCALE
}

pub fn quantize_pos3(p: [f32; 3]) -> [i32; 3] {
    [quantize_pos(p[0]), quantize_pos(p[1]), quantize_pos(p[2])]
}

pub fn dequantize_pos3(q: [i32; 3]) -> [f32; 3] {
    [
        dequantize_pos(q[0]),
        dequantize_pos(q[1]),
        dequantize_pos(q[2]),
    ]
}

pub fn quantize_vel3(v: [f32; 3]) -> [i32; 3] {
    [quantize_vel(v[0]), quantize_vel(v[1]), quantize_vel(v[2])]
}

pub fn dequantize_vel3(q: [i32; 3]) -> [f32; 3] {
    [
        dequantize_vel(q[0]),
        dequantize_vel(q[1]),
        dequantize_vel(q[2]),
    ]
}

/// Yaw in degrees to 0.1° steps, `0..3600`.
pub fn yaw_to_wire(yaw: f32) -> u16 {
    let y = yaw.rem_euclid(360.0);
    ((y * 10.0).round() as u32 % 3600) as u16
}

pub fn wire_to_yaw(w: u16) -> f32 {
    (w % 3600) as f32 / 10.0
}

/// Pitch in degrees (positive = looking down) to 0.1° steps, `0..=1800`.
pub fn pitch_to_wire(pitch: f32) -> u16 {
    ((pitch.clamp(-90.0, 90.0) + 90.0) * 10.0).round() as u16
}

pub fn wire_to_pitch(w: u16) -> f32 {
    w.min(1800) as f32 / 10.0 - 90.0
}

/// Move axis `-1..=1` to an i8.
pub fn axis_to_wire(a: f32) -> i8 {
    (a.clamp(-1.0, 1.0) * 127.0).round() as i8
}

pub fn wire_to_axis(w: i8) -> f32 {
    (w.max(-127) as f32) / 127.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_within_half_a_quantum() {
        for x in [-4096.0f32, -1.3, 0.0, 0.1, 0.125, 0.126, 7.9, 1000.37] {
            let q = quantize_pos(x);
            assert!(
                (dequantize_pos(q) - x).abs() <= 0.5 / POS_SCALE + 1e-6,
                "{x}"
            );
        }
        assert_eq!(quantize_pos(0.125), 1); // half away from zero
        assert_eq!(quantize_pos(-0.125), -1);
    }

    #[test]
    fn angles_wrap_and_clamp() {
        assert_eq!(yaw_to_wire(0.0), 0);
        assert_eq!(yaw_to_wire(359.99), 0);
        assert_eq!(yaw_to_wire(-90.0), 2700);
        assert_eq!(yaw_to_wire(720.5), 5);
        assert!((wire_to_yaw(yaw_to_wire(123.45)) - 123.45).abs() <= 0.05 + 1e-4);
        assert_eq!(pitch_to_wire(-90.0), 0);
        assert_eq!(pitch_to_wire(90.0), 1800);
        assert_eq!(pitch_to_wire(500.0), 1800);
        assert_eq!(wire_to_pitch(900), 0.0);
        assert_eq!(wire_to_pitch(u16::MAX), 90.0);
    }

    #[test]
    fn axes_are_symmetric() {
        assert_eq!(axis_to_wire(1.0), 127);
        assert_eq!(axis_to_wire(-1.0), -127);
        assert_eq!(axis_to_wire(5.0), 127);
        assert_eq!(wire_to_axis(-128), -1.0);
        assert_eq!(wire_to_axis(0), 0.0);
        assert!((wire_to_axis(axis_to_wire(0.5)) - 0.5).abs() < 0.005);
    }
}
