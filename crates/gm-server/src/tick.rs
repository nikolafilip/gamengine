//! Tick scheduling and timing metrics. Uses tokio's clock so turmoil's paused time works.

use std::time::Duration;

use gm_core::tick::TickRate;
use tokio::time::Instant;

/// Absolute-timeline scheduler: tick `n` is due at `start + n * period`.
pub struct TickScheduler {
    period: Duration,
    start: Instant,
    tick: u64,
    /// Falling further behind than this resets the timeline instead of catching up.
    resync_after: Duration,
}

impl TickScheduler {
    pub fn new(rate: TickRate, start: Instant) -> Self {
        TickScheduler {
            period: rate.period(),
            start,
            tick: 0,
            resync_after: Duration::from_secs(1),
        }
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn next_deadline(&self) -> Instant {
        self.start + self.period * (self.tick as u32)
    }

    pub fn advance(&mut self) {
        self.tick += 1;
    }

    /// If `now` is more than `resync_after` past the current deadline, restart the timeline at
    /// `now` and return how far behind we were.
    pub fn resync_if_behind(&mut self, now: Instant) -> Option<Duration> {
        let deadline = self.next_deadline();
        let behind = now.saturating_duration_since(deadline);
        if behind > self.resync_after {
            self.start = now - self.period * (self.tick as u32);
            Some(behind)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    pub count: u64,
    pub mean_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
    pub late_mean_us: f64,
    pub late_max_us: f64,
    /// Ticks whose work took longer than the period.
    pub overruns: u64,
}

/// Tick duration and lateness statistics for the current report window and for the whole run.
pub struct TickMetrics {
    period: Duration,
    window: Vec<u32>,
    window_late_sum: u64,
    window_late_max: u32,
    window_overruns: u64,
    total_count: u64,
    total_sum_us: u64,
    total_max_us: u32,
    total_late_max_us: u32,
    total_overruns: u64,
}

impl TickMetrics {
    pub fn new(period: Duration) -> Self {
        TickMetrics {
            period,
            window: Vec::with_capacity(4096),
            window_late_sum: 0,
            window_late_max: 0,
            window_overruns: 0,
            total_count: 0,
            total_sum_us: 0,
            total_max_us: 0,
            total_late_max_us: 0,
            total_overruns: 0,
        }
    }

    pub fn record(&mut self, lateness: Duration, work: Duration) {
        let work_us = work.as_micros().min(u32::MAX as u128) as u32;
        let late_us = lateness.as_micros().min(u32::MAX as u128) as u32;
        let overrun = work > self.period;
        self.window.push(work_us);
        self.window_late_sum += late_us as u64;
        self.window_late_max = self.window_late_max.max(late_us);
        self.window_overruns += overrun as u64;
        self.total_count += 1;
        self.total_sum_us += work_us as u64;
        self.total_max_us = self.total_max_us.max(work_us);
        self.total_late_max_us = self.total_late_max_us.max(late_us);
        self.total_overruns += overrun as u64;
    }

    pub fn summary(&self) -> Summary {
        let n = self.window.len() as u64;
        if n == 0 {
            return Summary::default();
        }
        let mut sorted = self.window.clone();
        sorted.sort_unstable();
        let p99 = sorted[((n as f64 * 0.99) as usize).min(sorted.len() - 1)];
        Summary {
            count: n,
            mean_us: sorted.iter().map(|&v| v as f64).sum::<f64>() / n as f64,
            p99_us: p99 as f64,
            max_us: *sorted.last().unwrap() as f64,
            late_mean_us: self.window_late_sum as f64 / n as f64,
            late_max_us: self.window_late_max as f64,
            overruns: self.window_overruns,
        }
    }

    pub fn summary_total(&self) -> Summary {
        Summary {
            count: self.total_count,
            mean_us: if self.total_count == 0 {
                0.0
            } else {
                self.total_sum_us as f64 / self.total_count as f64
            },
            p99_us: 0.0,
            max_us: self.total_max_us as f64,
            late_mean_us: 0.0,
            late_max_us: self.total_late_max_us as f64,
            overruns: self.total_overruns,
        }
    }

    pub fn reset_window(&mut self) {
        self.window.clear();
        self.window_late_sum = 0;
        self.window_late_max = 0;
        self.window_overruns = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlines_follow_an_absolute_timeline() {
        let start = Instant::now();
        let mut s = TickScheduler::new(TickRate::COMBAT, start);
        assert_eq!(s.next_deadline(), start);
        s.advance();
        s.advance();
        assert_eq!(s.next_deadline(), start + Duration::from_micros(31_250));
        assert_eq!(s.tick(), 2);
    }

    #[test]
    fn resync_only_when_far_behind() {
        let start = Instant::now();
        let mut s = TickScheduler::new(TickRate::TOWN, start);
        assert_eq!(s.resync_if_behind(start + Duration::from_millis(500)), None);
        let late = start + Duration::from_millis(1500);
        let behind = s.resync_if_behind(late).expect("should resync");
        assert_eq!(behind, Duration::from_millis(1500));
        assert_eq!(s.next_deadline(), late);
    }

    #[test]
    fn metrics_summarise_the_window() {
        let mut m = TickMetrics::new(Duration::from_micros(1000));
        for us in [100u64, 200, 300, 400, 5000] {
            m.record(Duration::from_micros(10), Duration::from_micros(us));
        }
        let s = m.summary();
        assert_eq!(s.count, 5);
        assert!((s.mean_us - 1200.0).abs() < 1e-9);
        assert_eq!(s.max_us, 5000.0);
        assert_eq!(s.p99_us, 5000.0);
        assert_eq!(s.overruns, 1);
        assert_eq!(s.late_max_us, 10.0);
        m.reset_window();
        assert_eq!(m.summary(), Summary::default());
        assert_eq!(m.summary_total().count, 5);
        assert_eq!(m.summary_total().overruns, 1);
    }
}
