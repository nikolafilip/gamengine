//! Frame timing and memory measurements. Every `bench:` line is parsed by scripts/check-perf.sh.

use std::time::Instant;

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
) {
    let (rss, peak) = rss_bytes();
    println!(
        "bench: mode={mode} adapter=\"{}\" backend={:?} device_type={:?} driver=\"{} {}\"",
        info.name, info.backend, info.device_type, info.driver, info.driver_info
    );
    println!(
        "bench: frames={} seconds={:.3} fps_avg={:.1} frame_ms_avg={:.3} frame_ms_p50={:.3} frame_ms_p99={:.3} frame_ms_max={:.3}",
        report.frames,
        report.seconds,
        report.fps_avg,
        report.ms_avg,
        report.ms_p50,
        report.ms_p99,
        report.ms_max
    );
    println!(
        "bench: peak_rss_bytes={peak} rss_bytes={rss} binary_bytes={}",
        binary_bytes()
    );
    println!("bench: faces_visible={faces_visible} faces_total={faces_total} draw_calls=1");
}
