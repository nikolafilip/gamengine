//! Distance bands (PROTOCOL.md 5): how often the server lists an entity for a client by its
//! distance from the client's eye. Shared with the client, which needs the same interval to
//! know how long a body was still before its first update in a while (PROTOCOL.md 7.3).

use gm_core::tick::TickRate;
use gm_core::vocab::EntityId;

/// Up to here every tick.
pub const FULL_RATE_DIST: f32 = 512.0;
/// Up to here every second combat tick (31 ms); beyond, every sixth (94 ms).
pub const HALF_RATE_DIST: f32 = 1536.0;
const HALF_RATE_TICKS: u32 = 2;
const FAR_RATE_TICKS: u32 = 6;

/// The band's time between updates at `rate`, in ticks. The bands are times, not tick
/// counts: six ticks are 94 ms at 64 Hz and would be 300 ms in a 20 Hz town, longer than the
/// client's interpolation delay there, so the next sample would never be in hand when it was
/// needed. Never under one tick.
pub fn band_interval(rate: TickRate, dist: f32) -> u32 {
    let combat_ticks = if dist <= FULL_RATE_DIST {
        return 1;
    } else if dist <= HALF_RATE_DIST {
        HALF_RATE_TICKS
    } else {
        FAR_RATE_TICKS
    };
    // Rounded to the nearest tick of `rate`.
    let hz = rate.hz();
    let combat = TickRate::COMBAT.hz();
    ((combat_ticks * hz + combat / 2) / combat).max(1)
}

/// Whether an entity at `dist` is listed this tick. Ids stagger the phase so one tick does
/// not carry every far body at once.
pub fn band_scheduled(rate: TickRate, tick: u32, id: EntityId, dist: f32) -> bool {
    let interval = band_interval(rate, dist);
    interval == 1 || tick.wrapping_add(id).is_multiple_of(interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_schedule_by_distance() {
        let r = TickRate::COMBAT;
        assert!(band_scheduled(r, 1, 1, 100.0));
        assert!(band_scheduled(r, 2, 1, 100.0));
        let half: Vec<bool> = (0..4).map(|t| band_scheduled(r, t, 1, 1000.0)).collect();
        assert_eq!(half, [false, true, false, true]);
        let far = (0..12).filter(|&t| band_scheduled(r, t, 1, 3000.0)).count();
        assert_eq!(far, 2);
    }

    #[test]
    fn the_bands_keep_their_time_at_another_rate() {
        assert_eq!(band_interval(TickRate::COMBAT, 1000.0), 2);
        assert_eq!(band_interval(TickRate::COMBAT, 3000.0), 6);
        // A 20 Hz town: the half band every tick, the far band every second (100 ms).
        assert_eq!(band_interval(TickRate::TOWN, 100.0), 1);
        assert_eq!(band_interval(TickRate::TOWN, 1000.0), 1);
        assert_eq!(band_interval(TickRate::TOWN, 3000.0), 2);
        assert!((0..10).all(|t| band_scheduled(TickRate::TOWN, t, 1, 1000.0)));
        assert_eq!(
            (0..10)
                .filter(|&t| band_scheduled(TickRate::TOWN, t, 1, 3000.0))
                .count(),
            5
        );
    }
}
