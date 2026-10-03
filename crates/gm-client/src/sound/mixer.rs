//! The native mixer (SOUND.md 4): thirty-two voices over the rendered patches, two loop
//! slots for the air, a master gain and a soft clip, rendered into stereo blocks by
//! whoever asks (the device's callback, or the frame clock into a file). Commands come
//! from the frame thread through a queue the renderer only tries to take: the frame
//! thread never waits for the device, and the device never waits for the frame.
//!
//! Rendering allocates nothing: the voices are a fixed array and the patches are read
//! through a shared slice.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::synth::{Cue, RATE};

pub const VOICES: usize = 32;
pub const LOOP_SLOTS: usize = 2;
/// A loop's gain moves toward its target at this rate (per second): the fade.
const LOOP_FADE_PER_SEC: f32 = 1.0;

/// What the frame thread tells the mixer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Start a patch once, at a gain for each ear and a pitch (1.0: as rendered).
    Cue {
        cue: Cue,
        left: f32,
        right: f32,
        pitch: f32,
    },
    /// Put a loop in a slot (`None`: fade the slot out), at a gain.
    Loop {
        slot: u8,
        cue: Option<Cue>,
        gain: f32,
    },
    /// The master gain.
    Master(f32),
    /// Stop every voice and loop at once (a zone left).
    Quiet,
}

/// The queue between the frame thread and the renderer.
#[derive(Clone, Default)]
pub struct Queue(Arc<Mutex<VecDeque<Command>>>);

impl Queue {
    /// The frame thread's side: never blocks for long (the renderer holds the lock for
    /// microseconds, and only when it has it).
    pub fn send(&self, c: Command) {
        if let Ok(mut q) = self.0.lock() {
            // A renderer that never comes (no device) must not let the queue grow for
            // ever: what nobody takes is dropped from the front.
            if q.len() >= 1024 {
                q.pop_front();
            }
            q.push_back(c);
        }
    }

    /// The renderer's side: whatever is there now (up to a block's worth), or nothing if
    /// the frame thread holds the lock at this instant (the commands wait one block). The
    /// lock is let go of before anything is rendered.
    fn take_into(&self, mixer: &mut Mixer) {
        let mut taken: [Option<Command>; 64] = [None; 64];
        let mut n = 0;
        match self.0.try_lock() {
            Ok(mut q) => {
                while n < taken.len()
                    && let Some(c) = q.pop_front()
                {
                    taken[n] = Some(c);
                    n += 1;
                }
            }
            // (A sender that panicked holding the lock: the queue is dead; the mixer
            // plays what it has.)
            Err(_) => return,
        }
        for c in taken.iter().take(n).flatten() {
            mixer.apply(*c);
        }
    }
}

#[derive(Clone, Copy)]
struct Voice {
    cue: Option<Cue>,
    cursor: f32,
    step: f32,
    left: f32,
    right: f32,
    /// What a stolen voice was putting out as it was taken, carried on and dying away
    /// in half a millisecond, so that the take-over is no step in the waveform (a click).
    declick: (f32, f32),
}

const SILENT: Voice = Voice {
    cue: None,
    cursor: 0.0,
    step: 1.0,
    left: 0.0,
    right: 0.0,
    declick: (0.0, 0.0),
};

/// The time constant of the declick, seconds.
const DECLICK_SECS: f32 = 0.0005;

#[derive(Clone, Copy)]
struct LoopSlot {
    cue: Option<Cue>,
    cursor: f32,
    gain: f32,
    target: f32,
}

const EMPTY: LoopSlot = LoopSlot {
    cue: None,
    cursor: 0.0,
    gain: 0.0,
    target: 0.0,
};

pub struct Mixer {
    patches: Arc<Vec<Vec<f32>>>,
    voices: [Voice; VOICES],
    loops: [LoopSlot; LOOP_SLOTS],
    master: f32,
    /// The output rate: a patch's sample is this many output samples long.
    step: f32,
    out_rate: f32,
    /// What a declick is multiplied by each output sample.
    declick_fall: f32,
}

/// A voice's sample now, interpolated; `None` when it is over.
fn sample_of(p: &[f32], cursor: f32) -> Option<f32> {
    let i = cursor as usize;
    if i + 1 >= p.len() {
        return None;
    }
    let frac = cursor - i as f32;
    Some(p[i] * (1.0 - frac) + p[i + 1] * frac)
}

impl Mixer {
    /// `patches`: every patch rendered, in `Cue`'s order (`synth::render_all`).
    pub fn new(patches: Arc<Vec<Vec<f32>>>, out_rate: u32) -> Mixer {
        Mixer {
            patches,
            voices: [SILENT; VOICES],
            loops: [EMPTY; LOOP_SLOTS],
            master: 1.0,
            step: RATE as f32 / out_rate as f32,
            out_rate: out_rate as f32,
            declick_fall: (-1.0 / (DECLICK_SECS * out_rate as f32)).exp(),
        }
    }

    pub fn apply(&mut self, c: Command) {
        match c {
            Command::Cue {
                cue,
                left,
                right,
                pitch,
            } => {
                if cue.is_loop() || self.patches[cue as usize].is_empty() {
                    return;
                }
                let mut voice = Voice {
                    cue: Some(cue),
                    cursor: 0.0,
                    step: self.step * pitch.clamp(0.25, 4.0),
                    left,
                    right,
                    declick: (0.0, 0.0),
                };
                // A free voice, or else the quietest, whose last output the new voice
                // carries on from (every patch begins at nothing: the step would be a
                // click).
                let slot = match self.voices.iter().position(|v| v.cue.is_none()) {
                    Some(i) => i,
                    None => {
                        let mut quietest = 0;
                        for (i, v) in self.voices.iter().enumerate() {
                            if v.left + v.right
                                < self.voices[quietest].left + self.voices[quietest].right
                            {
                                quietest = i;
                            }
                        }
                        let old = self.voices[quietest];
                        let s = old
                            .cue
                            .and_then(|c| sample_of(&self.patches[c as usize], old.cursor))
                            .unwrap_or(0.0);
                        voice.declick =
                            (s * old.left + old.declick.0, s * old.right + old.declick.1);
                        quietest
                    }
                };
                self.voices[slot] = voice;
            }
            Command::Loop { slot, cue, gain } => {
                let Some(l) = self.loops.get_mut(slot as usize) else {
                    return;
                };
                match cue {
                    Some(cue) if cue.is_loop() && !self.patches[cue as usize].is_empty() => {
                        if l.cue != Some(cue) {
                            l.cue = Some(cue);
                            l.cursor = 0.0;
                            l.gain = 0.0;
                        }
                        l.target = gain.max(0.0);
                    }
                    _ => l.target = 0.0,
                }
            }
            Command::Master(g) => self.master = g.clamp(0.0, 1.0),
            Command::Quiet => {
                self.voices = [SILENT; VOICES];
                self.loops = [EMPTY; LOOP_SLOTS];
            }
        }
    }

    /// Render interleaved stereo into `out` (its length a whole number of frames),
    /// taking the queue's commands first.
    pub fn render(&mut self, queue: &Queue, out: &mut [f32]) {
        queue.take_into(self);
        self.render_block(out);
    }

    /// Render interleaved stereo into `out` from what the mixer holds now.
    pub fn render_block(&mut self, out: &mut [f32]) {
        let fade = LOOP_FADE_PER_SEC / self.out_rate;
        for frame in out.chunks_exact_mut(2) {
            let (mut l, mut r) = (0.0_f32, 0.0_f32);
            for v in self.voices.iter_mut() {
                let Some(cue) = v.cue else { continue };
                let Some(s) = sample_of(&self.patches[cue as usize], v.cursor) else {
                    v.cue = None;
                    continue;
                };
                l += s * v.left + v.declick.0;
                r += s * v.right + v.declick.1;
                v.declick.0 *= self.declick_fall;
                v.declick.1 *= self.declick_fall;
                v.cursor += v.step;
            }
            for slot in self.loops.iter_mut() {
                let Some(cue) = slot.cue else { continue };
                // Toward the target, a step a sample; a slot faded out is empty.
                if slot.gain < slot.target {
                    slot.gain = (slot.gain + fade).min(slot.target);
                } else if slot.gain > slot.target {
                    slot.gain = (slot.gain - fade).max(slot.target);
                    if slot.gain <= 0.0 && slot.target <= 0.0 {
                        slot.cue = None;
                        continue;
                    }
                }
                let p = &self.patches[cue as usize];
                let n = p.len();
                let i = slot.cursor as usize % n;
                let j = (i + 1) % n;
                let frac = slot.cursor - slot.cursor.floor();
                let s = p[i] * (1.0 - frac) + p[j] * frac;
                l += s * slot.gain;
                r += s * slot.gain;
                slot.cursor += self.step;
                if slot.cursor >= n as f32 {
                    slot.cursor -= n as f32;
                }
            }
            frame[0] = soft(l * self.master);
            frame[1] = soft(r * self.master);
        }
    }

    /// Voices playing now (not loops).
    #[cfg(test)]
    pub fn playing(&self) -> usize {
        self.voices.iter().filter(|v| v.cue.is_some()).count()
    }

    #[cfg(test)]
    pub fn loop_gains(&self) -> [f32; LOOP_SLOTS] {
        [self.loops[0].gain, self.loops[1].gain]
    }
}

/// Nothing clips however many voices there are: as it is up to a knee, and past the knee
/// bent toward 1 and never over it (a lone sound is not bent at all).
const KNEE: f32 = 0.7;

fn soft(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let over = (a - KNEE) / (1.0 - KNEE);
    let y = KNEE + (1.0 - KNEE) * over / (1.0 + over);
    y.copysign(x)
}

/// Write interleaved stereo `f32` samples as a 16-bit WAV.
pub fn wav_bytes(rate: u32, samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sound::synth::render_all;
    use web_time::Instant;

    // The renderer's promise of no allocation is asserted by an allocator that panics on
    // one inside `assert_no_alloc` (a dev-dependency; nothing of it is in the client).
    #[global_allocator]
    static GLOBAL: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    fn mixer(rate: u32) -> (Mixer, Queue) {
        (Mixer::new(Arc::new(render_all()), rate), Queue::default())
    }

    fn rms(out: &[f32]) -> f32 {
        (out.iter().map(|v| v * v).sum::<f32>() / out.len().max(1) as f32).sqrt()
    }

    #[test]
    fn a_cue_is_heard_where_it_was_put_and_ends() {
        let (mut m, q) = mixer(44_100);
        let mut out = vec![0.0_f32; 2 * 441]; // 10 ms
        m.render(&q, &mut out);
        assert_eq!(rms(&out), 0.0, "silence before anything");
        q.send(Command::Cue {
            cue: Cue::Blip,
            left: 0.0,
            right: 1.0,
            pitch: 1.0,
        });
        m.render(&q, &mut out);
        let (l, r): (Vec<f32>, Vec<f32>) = out.chunks_exact(2).map(|f| (f[0], f[1])).unzip();
        assert_eq!(rms(&l), 0.0, "nothing in the left ear");
        assert!(rms(&r) > 0.05, "{}", rms(&r));
        assert_eq!(m.playing(), 1);
        // Blip is 50 ms: five more hundredths of a second and it is over.
        for _ in 0..5 {
            m.render(&q, &mut out);
        }
        assert_eq!(m.playing(), 0);
        assert_eq!(rms(&out), 0.0);
    }

    #[test]
    fn a_pitch_plays_a_patch_faster() {
        let (mut m, q) = mixer(22_050);
        let mut out = vec![0.0_f32; 2 * 2205]; // 0.1 s
        q.send(Command::Cue {
            cue: Cue::Death, // 600 ms
            left: 1.0,
            right: 1.0,
            pitch: 2.0,
        });
        for _ in 0..3 {
            m.render(&q, &mut out);
        }
        assert_eq!(m.playing(), 1, "at double speed, 300 ms in: still going");
        m.render(&q, &mut out);
        assert_eq!(m.playing(), 0, "and over at 400 ms");
    }

    #[test]
    fn the_quietest_voice_is_stolen_and_loops_are_not() {
        let (mut m, q) = mixer(44_100);
        q.send(Command::Loop {
            slot: 0,
            cue: Some(Cue::Wind),
            gain: 0.5,
        });
        for i in 0..VOICES {
            q.send(Command::Cue {
                cue: Cue::Burst,
                left: 0.1 + i as f32 * 0.01,
                right: 0.1 + i as f32 * 0.01,
                pitch: 1.0,
            });
        }
        let mut out = vec![0.0_f32; 2 * 441];
        m.render(&q, &mut out);
        assert!(m.playing() >= VOICES - 2, "{}", m.playing());
        // One more: the quietest (the first) is taken over.
        q.send(Command::Cue {
            cue: Cue::Burst,
            left: 0.9,
            right: 0.9,
            pitch: 1.0,
        });
        m.render(&q, &mut out);
        assert!(m.playing() >= VOICES - 2, "{}", m.playing());
        assert!((m.voices[0].left - 0.9).abs() < 1e-6);
        assert!(m.loops[0].cue == Some(Cue::Wind), "the loop stays");
    }

    #[test]
    fn a_stolen_voice_is_no_click() {
        // Thirty-two chimes at a quarter of their pitch (a 165 Hz tone, no noise) and, 600
        // output samples in (the tone near its peak, in its hold), one more: the quietest
        // (the first) is taken over mid-wave. The first output sample after the take-over
        // must sit on the wave; without the declick it would jump by what the stolen voice
        // was putting out.
        let (mut m, q) = mixer(44_100);
        for _ in 0..VOICES {
            q.send(Command::Cue {
                cue: Cue::Chime,
                left: 0.02,
                right: 0.02,
                pitch: 0.25,
            });
        }
        let mut out = vec![0.0_f32; 2 * 600];
        m.render(&q, &mut out);
        let last_before = out[2 * 599];
        let old = sample_of(&m.patches[Cue::Chime as usize], m.voices[0].cursor).unwrap() * 0.02;
        assert!(old.abs() > 0.004, "the test must steal mid-wave: {old}");
        q.send(Command::Cue {
            cue: Cue::Blip,
            left: 0.9,
            right: 0.9,
            pitch: 1.0,
        });
        let mut next = vec![0.0_f32; 2 * 441];
        m.render(&q, &mut next);
        assert_eq!(
            m.voices[0].cue,
            Some(Cue::Blip),
            "the first voice was taken"
        );
        let jump = (next[0] - last_before).abs();
        assert!(
            jump < old.abs() / 4.0,
            "the take-over stepped {jump}; the stolen voice was at {old}"
        );
        // And ten milliseconds on, nothing of the carried value remains.
        assert!(m.voices[0].declick.0.abs() < old.abs() * 1e-3);
    }

    #[test]
    fn a_loop_fades_in_and_out_and_a_loop_is_no_cue() {
        let (mut m, q) = mixer(22_050);
        q.send(Command::Cue {
            cue: Cue::Wind,
            left: 1.0,
            right: 1.0,
            pitch: 1.0,
        });
        let mut out = vec![0.0_f32; 2 * 2205];
        m.render(&q, &mut out);
        assert_eq!(m.playing(), 0, "a loop cannot be cued");
        q.send(Command::Loop {
            slot: 1,
            cue: Some(Cue::Drone),
            gain: 1.0,
        });
        m.render(&q, &mut out);
        let g = m.loop_gains()[1];
        assert!((0.09..=0.11).contains(&g), "a tenth of a second in: {g}");
        for _ in 0..10 {
            m.render(&q, &mut out);
        }
        assert_eq!(m.loop_gains()[1], 1.0);
        assert!(rms(&out) > 0.05);
        q.send(Command::Loop {
            slot: 1,
            cue: None,
            gain: 0.0,
        });
        for _ in 0..11 {
            m.render(&q, &mut out);
        }
        assert_eq!(m.loop_gains()[1], 0.0);
        assert_eq!(m.loops[1].cue, None, "faded out: empty");
        assert_eq!(rms(&out), 0.0);
    }

    #[test]
    fn nothing_clips_and_quiet_stops_everything() {
        let (mut m, q) = mixer(44_100);
        for _ in 0..VOICES {
            q.send(Command::Cue {
                cue: Cue::Burst,
                left: 1.0,
                right: 1.0,
                pitch: 1.0,
            });
        }
        let mut out = vec![0.0_f32; 2 * 4410];
        m.render(&q, &mut out);
        assert!(out.iter().all(|v| v.abs() < 1.0));
        assert!(rms(&out) > 0.3);
        // The knee: a lone sound under it is as rendered, a sum over it is bent, not cut.
        assert_eq!(soft(0.5), 0.5);
        assert_eq!(soft(-0.7), -0.7);
        assert!(soft(1.0) > 0.8 && soft(1.0) < 1.0);
        assert!(soft(10.0) < 1.0 && soft(10.0) > soft(1.0));
        q.send(Command::Quiet);
        m.render(&q, &mut out);
        assert_eq!(m.playing(), 0);
        assert_eq!(rms(&out), 0.0);
    }

    #[test]
    fn the_renderer_allocates_nothing_and_its_block_is_cheap() {
        let (mut m, q) = mixer(44_100);
        let mut out = vec![0.0_f32; 2 * 512];
        q.send(Command::Loop {
            slot: 0,
            cue: Some(Cue::Murmur),
            gain: 0.5,
        });
        q.send(Command::Loop {
            slot: 1,
            cue: Some(Cue::Wind),
            gain: 0.5,
        });
        m.render(&q, &mut out);
        // (The sender may allocate when its queue grows; the renderer never, which the
        // allocator asserts for every block below.)
        let began = Instant::now();
        let blocks = 10_000;
        for _ in 0..blocks {
            // Keep every voice busy: a long cue every block.
            q.send(Command::Cue {
                cue: Cue::Death,
                left: 0.5,
                right: 0.5,
                pitch: 1.0,
            });
            assert_no_alloc::assert_no_alloc(|| m.render(&q, &mut out));
        }
        let us = began.elapsed().as_secs_f64() * 1e6 / blocks as f64;
        // (An unoptimised build's number: the budget is checked on the release client's
        // own report, SOUND.md 7.)
        println!(
            "mixer bench (this build): {us:.1} us per 512-frame stereo block with {} voices playing",
            m.playing()
        );
        assert!(m.playing() >= VOICES - 2, "{}", m.playing());
    }

    #[test]
    fn a_wav_has_its_header_right() {
        let w = wav_bytes(22_050, &[0.0, 0.5, -0.5, 1.0]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 22_050);
        assert_eq!(i16::from_le_bytes(w[46..48].try_into().unwrap()), 16383);
    }
}
