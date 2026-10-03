//! The native output (SOUND.md 4): the default device through `cpal`, whose callback
//! renders the mixer; or, for a run that must be measured on a machine without a device,
//! the frame clock rendering the mixer into a WAV (`--sound-dump FILE`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::mixer::{Mixer, Queue, wav_bytes};
use super::synth::RATE;

/// What the callback measured of itself: microseconds per block, the largest and the sum
/// (the mean is the sum over the count), and how many blocks and output frames.
#[derive(Default)]
pub struct Meter {
    pub max_us: AtomicU64,
    pub sum_us: AtomicU64,
    pub blocks: AtomicU64,
    pub frames: AtomicU64,
}

impl Meter {
    fn note(&self, us: u64, frames: u64) {
        self.max_us.fetch_max(us, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
        self.blocks.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(frames, Ordering::Relaxed);
    }

    /// Microseconds per 512 output frames, the mean and the largest block as it was.
    pub fn per_512(&self) -> (f64, u64) {
        let frames = self.frames.load(Ordering::Relaxed).max(1) as f64;
        let sum = self.sum_us.load(Ordering::Relaxed) as f64;
        (sum * 512.0 / frames, self.max_us.load(Ordering::Relaxed))
    }
}

/// The device, playing for as long as it is held.
pub struct Device {
    _stream: cpal::Stream,
    pub meter: Arc<Meter>,
    pub rate: u32,
    /// Frames a block, when the device took the asking; 0 when it chose itself.
    pub buffer: u32,
}

impl Device {
    /// Open the default output device. `None` when there is none, or it cannot be opened:
    /// the game plays silently.
    pub fn open(patches: Arc<Vec<Vec<f32>>>, queue: Queue) -> Option<Device> {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            log::info!("sound device: none is offered by {:?}", host.id());
            return None;
        };
        let supported = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                log::info!("sound device: no output configuration: {e}");
                return None;
            }
        };
        let format = supported.sample_format();
        let mut config: cpal::StreamConfig = supported.into();
        let rate = config.sample_rate;
        let channels = config.channels as usize;
        if channels == 0 {
            return None;
        }
        // A short buffer, so that a sound follows what it is the sound of (SOUND.md 4):
        // 512 frames are 11.6 ms at 44.1 kHz. A device that will not is opened as it
        // likes.
        config.buffer_size = cpal::BufferSize::Fixed(512);
        let meter = Arc::new(Meter::default());
        let mut buffer = 512;
        let mut stream = Self::stream(
            &device,
            &config,
            format,
            patches.clone(),
            queue.clone(),
            meter.clone(),
            channels,
        );
        if stream.is_none() {
            config.buffer_size = cpal::BufferSize::Default;
            buffer = 0;
            stream = Self::stream(
                &device,
                &config,
                format,
                patches,
                queue,
                meter.clone(),
                channels,
            );
        }
        let stream = stream?;
        if let Err(e) = stream.play() {
            log::info!("sound device: cannot play: {e}");
            return None;
        }
        log::info!(
            "sound device: {rate} Hz, {channels} channels, {format}, buffer {}",
            if buffer == 0 {
                "the device's own".to_string()
            } else {
                format!("{buffer} frames")
            }
        );
        Some(Device {
            _stream: stream,
            meter,
            rate,
            buffer,
        })
    }

    fn stream(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        format: cpal::SampleFormat,
        patches: Arc<Vec<Vec<f32>>>,
        queue: Queue,
        m: Arc<Meter>,
        channels: usize,
    ) -> Option<cpal::Stream> {
        let mut mixer = Mixer::new(patches, config.sample_rate);
        // Stereo is rendered into this and spread over the device's channels: made once,
        // never grown in the callback.
        let mut stereo = vec![0.0_f32; 2 * 8192];
        let err = |e: cpal::Error| log::warn!("sound device: {e}");
        let config = *config;
        let stream = match format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    config,
                    move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let began = Instant::now();
                        render_into(&mut mixer, &queue, &mut stereo, channels, out, |v| v);
                        m.note(
                            began.elapsed().as_micros() as u64,
                            (out.len() / channels) as u64,
                        );
                    },
                    err,
                    None,
                )
                .map_err(|e| {
                    log::info!("sound device: cannot open ({:?}): {e}", config.buffer_size)
                })
                .ok()?,
            cpal::SampleFormat::I16 => device
                .build_output_stream(
                    config,
                    move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        let began = Instant::now();
                        render_into(&mut mixer, &queue, &mut stereo, channels, out, |v| {
                            (v * 32767.0) as i16
                        });
                        m.note(
                            began.elapsed().as_micros() as u64,
                            (out.len() / channels) as u64,
                        );
                    },
                    err,
                    None,
                )
                .map_err(|e| {
                    log::info!("sound device: cannot open ({:?}): {e}", config.buffer_size)
                })
                .ok()?,
            other => {
                log::info!("sound device: {other} samples are not played; the game is silent");
                return None;
            }
        };
        Some(stream)
    }
}

/// Render as many stereo frames as `out` holds (in blocks that fit the scratch), and
/// spread them over the device's channels: the first two get left and right, the rest
/// get nothing (a surround device plays the pair in front).
fn render_into<T: Copy + Default>(
    mixer: &mut Mixer,
    queue: &Queue,
    stereo: &mut [f32],
    channels: usize,
    out: &mut [T],
    convert: impl Fn(f32) -> T,
) {
    let frames = out.len() / channels;
    let per_block = stereo.len() / 2;
    let mut done = 0;
    let mut first = true;
    while done < frames {
        let n = (frames - done).min(per_block);
        let block = &mut stereo[..2 * n];
        if first {
            mixer.render(queue, block);
            first = false;
        } else {
            mixer.render_block(block);
        }
        for (i, frame) in out[done * channels..(done + n) * channels]
            .chunks_exact_mut(channels)
            .enumerate()
        {
            frame[0] = convert(block[2 * i]);
            if channels > 1 {
                frame[1] = convert(block[2 * i + 1]);
            }
            for extra in frame.iter_mut().skip(2) {
                *extra = T::default();
            }
        }
        done += n;
    }
}

/// The mixer rendered by the frame clock at the patches' own rate, kept, and written as
/// a WAV at the end: a run that can be measured on a machine without a device.
pub struct Dump {
    mixer: Mixer,
    queue: Queue,
    samples: Vec<f32>,
    carry: f32,
    pub meter: Meter,
    path: std::path::PathBuf,
}

impl Dump {
    pub fn new(patches: Arc<Vec<Vec<f32>>>, queue: Queue, path: std::path::PathBuf) -> Dump {
        Dump {
            mixer: Mixer::new(patches, RATE),
            queue,
            samples: Vec::new(),
            carry: 0.0,
            meter: Meter::default(),
            path,
        }
    }

    /// Render the frame's worth of time (at most a quarter of a second: a stalled frame
    /// is not an hour of silence).
    pub fn advance(&mut self, secs: f32) {
        let want = secs.clamp(0.0, 0.25) * RATE as f32 + self.carry;
        let frames = want.floor() as usize;
        self.carry = want - frames as f32;
        if frames == 0 {
            return;
        }
        let began = Instant::now();
        let start = self.samples.len();
        self.samples.resize(start + 2 * frames, 0.0);
        self.mixer.render(&self.queue, &mut self.samples[start..]);
        self.meter
            .note(began.elapsed().as_micros() as u64, frames as u64);
    }

    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / 2.0 / RATE as f32
    }

    /// Write the WAV; how many seconds it holds.
    pub fn finish(&self) -> std::io::Result<f32> {
        std::fs::write(&self.path, wav_bytes(RATE, &self.samples))?;
        Ok(self.seconds())
    }
}
