//! Fixed-tick time. All durations in the simulation are integer ticks at the zone's rate.

use core::time::Duration;

/// Tick counter. Wraps after ~2 years at 64 Hz; zones restart long before that.
pub type Tick = u32;

/// A fixed simulation rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TickRate {
    hz: u32,
}

impl TickRate {
    /// Combat zones (PLAN.md 2.1).
    pub const COMBAT: TickRate = TickRate { hz: 64 };
    /// Towns and slow zones.
    pub const TOWN: TickRate = TickRate { hz: 20 };

    /// # Panics
    /// If `hz` is zero.
    pub const fn new(hz: u32) -> Self {
        assert!(hz > 0, "tick rate must be positive");
        Self { hz }
    }

    pub const fn hz(self) -> u32 {
        self.hz
    }

    /// Seconds per tick as f32, the `dt` handed to the simulation.
    pub fn dt(self) -> f32 {
        1.0 / self.hz as f32
    }

    /// Exact tick period.
    pub const fn period(self) -> Duration {
        Duration::from_nanos(1_000_000_000 / self.hz as u64)
    }

    /// Content is authored in milliseconds; the loader rounds **up** so nothing resolves
    /// earlier than authored.
    pub const fn ms_to_ticks(self, ms: u32) -> Tick {
        ((ms as u64 * self.hz as u64).div_ceil(1000)) as Tick
    }

    pub const fn ticks_to_ms(self, ticks: Tick) -> u32 {
        (ticks as u64 * 1000 / self.hz as u64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combat_rate_arithmetic() {
        let r = TickRate::COMBAT;
        assert_eq!(r.period(), Duration::from_micros(15_625));
        assert!((r.dt() - 0.015_625).abs() < 1e-9);
        assert_eq!(r.ms_to_ticks(1000), 64);
        assert_eq!(r.ms_to_ticks(200), 13); // 12.8 rounds up
        assert_eq!(r.ms_to_ticks(0), 0);
        assert_eq!(r.ticks_to_ms(64), 1000);
    }

    #[test]
    fn town_rate_arithmetic() {
        let r = TickRate::TOWN;
        assert_eq!(r.period(), Duration::from_millis(50));
        assert_eq!(r.ms_to_ticks(120), 3); // 2.4 rounds up
    }
}
