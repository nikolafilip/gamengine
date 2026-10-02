//! Playing a replay back (ANTICHEAT.md 3.4): the bodies at any time between two frames,
//! interpolated the way a client interpolates other players.

use glam::Vec3;
use gm_net::client::{RenderEntity, lerp_angle};
use gm_net::quant;
use gm_net::snapshot::EntityState;

use crate::Replay;

fn render(a: &EntityState, b: Option<&EntityState>, alpha: f32, dt: f32) -> RenderEntity {
    let pa = Vec3::from(quant::dequantize_pos3(a.pos));
    let (pos, yaw, pitch, vel, state) = match b {
        Some(b) => {
            let pb = Vec3::from(quant::dequantize_pos3(b.pos));
            (
                pa.lerp(pb, alpha),
                lerp_angle(quant::wire_to_yaw(a.yaw), quant::wire_to_yaw(b.yaw), alpha),
                quant::wire_to_pitch(a.pitch) * (1.0 - alpha)
                    + quant::wire_to_pitch(b.pitch) * alpha,
                (pb - pa) / dt,
                if alpha < 0.5 { a } else { b },
            )
        }
        None => (
            pa,
            quant::wire_to_yaw(a.yaw),
            quant::wire_to_pitch(a.pitch),
            Vec3::ZERO,
            a,
        ),
    };
    RenderEntity {
        id: state.id,
        kind: state.spawn.kind(),
        spawn: state.spawn,
        pos,
        yaw,
        pitch,
        anim: state.anim,
        flags: state.flags,
        status: state.status,
        health: state.health,
        vel,
    }
}

impl Replay {
    /// The index of the frame at or before `seconds` from the start, and how far into the
    /// step to the next one that is (0..1).
    pub fn locate(&self, seconds: f32) -> (usize, f32) {
        let hz = self.header.hz.max(1) as f32;
        let at = (seconds.max(0.0) * hz).min(self.frames.len().saturating_sub(1) as f32);
        // Frames are one tick apart; a file is never sparse.
        (at.floor() as usize, at.fract())
    }

    /// Every entity at `seconds` from the start.
    pub fn entities_at(&self, seconds: f32) -> Vec<RenderEntity> {
        let (i, alpha) = self.locate(seconds);
        let Some(a) = self.frames.get(i) else {
            return Vec::new();
        };
        let b = self.frames.get(i + 1);
        let dt = 1.0 / self.header.hz.max(1) as f32;
        a.snapshot
            .entities
            .iter()
            .map(|ea| {
                let eb = b.and_then(|b| b.snapshot.find(ea.id));
                render(ea, eb, alpha, dt)
            })
            .collect()
    }
}
