//! Frame timing and memory measurements. Every `bench:` line is parsed by scripts/check-perf.sh.

use std::time::Instant;

use crate::avatars::Avatars;
use crate::render::Renderer;

#[derive(Default)]
pub struct FrameStats {
    frame_ms: Vec<f32>,
    start: Option<Instant>,
    last: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    pub frames: usize,
    pub seconds: f64,
    pub fps_avg: f64,
    pub ms_avg: f32,
    pub ms_p50: f32,
    pub ms_p99: f32,
    pub ms_max: f32,
    /// Which frame was the slowest (0 = the first counted): a hitch at the start is a
    /// warm-up, one in the middle is a stall.
    pub max_at: usize,
}

impl FrameStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call once per presented frame.
    pub fn frame(&mut self) {
        let now = Instant::now();
        match self.last {
            Some(last) => self.frame_ms.push((now - last).as_secs_f32() * 1000.0),
            None => self.start = Some(now),
        }
        self.last = Some(now);
    }

    pub fn frames(&self) -> usize {
        self.frame_ms.len()
    }

    /// Statistics over the frames since `since_frame`.
    pub fn report_since(&self, since_frame: usize) -> Report {
        let slice = &self.frame_ms[since_frame.min(self.frame_ms.len())..];
        if slice.is_empty() {
            return Report::default();
        }
        let max_at = slice
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i);
        let mut sorted = slice.to_vec();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let n = sorted.len();
        let seconds = sorted.iter().map(|&v| v as f64).sum::<f64>() / 1000.0;
        Report {
            frames: n,
            seconds,
            fps_avg: n as f64 / seconds.max(1e-9),
            ms_avg: (seconds * 1000.0 / n as f64) as f32,
            ms_p50: sorted[n / 2],
            ms_p99: sorted[((n as f64 * 0.99) as usize).min(n - 1)],
            ms_max: sorted[n - 1],
            max_at,
        }
    }

    pub fn report(&self) -> Report {
        self.report_since(0)
    }
}

/// `(VmRSS, VmHWM)` in bytes from /proc; zeros where unavailable.
pub fn rss_bytes() -> (u64, u64) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return (0, 0);
    };
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| {
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map_or(0, |kb| kb * 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

pub fn binary_bytes() -> u64 {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| m.len())
        .unwrap_or(0)
}

pub fn print_bench(
    report: &Report,
    info: &wgpu::AdapterInfo,
    mode: &str,
    faces_visible: usize,
    faces_total: usize,
    draw_calls: usize,
) {
    let (rss, peak) = rss_bytes();
    println!(
        "bench: mode={mode} adapter=\"{}\" backend={:?} device_type={:?} driver=\"{} {}\"",
        info.name, info.backend, info.device_type, info.driver, info.driver_info
    );
    println!(
        "bench: frames={} seconds={:.3} fps_avg={:.1} frame_ms_avg={:.3} frame_ms_p50={:.3} frame_ms_p99={:.3} frame_ms_max={:.3} slowest_frame={}",
        report.frames,
        report.seconds,
        report.fps_avg,
        report.ms_avg,
        report.ms_p50,
        report.ms_p99,
        report.ms_max,
        report.max_at
    );
    println!(
        "bench: peak_rss_bytes={peak} rss_bytes={rss} binary_bytes={}",
        binary_bytes()
    );
    println!(
        "bench: faces_visible={faces_visible} faces_total={faces_total} draw_calls={draw_calls}"
    );
}

/// The avatar line of a run (MODELS.md 11), parsed by scripts/check-avatars.sh. Printed when
/// any character was drawn.
pub fn print_bench_avatars(report: &Report, avatars: &Avatars, renderer: &Renderer) {
    let c = &renderer.characters;
    if c.drawn == 0 {
        return;
    }
    let s = avatars.stats();
    let (cache_bytes, cache_cap) = avatars
        .cache
        .as_ref()
        .map_or((0, 0), |c| (c.disk.total(), c.disk.cap()));
    let (_, peak) = rss_bytes();
    println!(
        "avatars: characters={} with_model={} triangles={} models_ready={} models_pending={} gpu_bytes={} model_gpu_bytes={} \
         cache_bytes={cache_bytes} cache_cap_bytes={cache_cap} fetched={} fetched_bytes={} disk_hits={} failed={} refused={} \
         gpu_evictions={} gpu_deferred={} fps_avg={:.1} frame_ms_p99={:.3} peak_rss_bytes={peak}",
        c.drawn,
        avatars.with_model,
        c.triangles,
        s.ready,
        avatars.cache.as_ref().map_or(0, |c| c.pending()),
        c.gpu_bytes(),
        s.gpu_bytes,
        s.fetched,
        s.fetched_bytes,
        s.disk_hits,
        s.failed,
        s.refused,
        s.gpu_evictions,
        s.gpu_deferred,
        report.fps_avg,
        report.ms_p99,
    );
}
