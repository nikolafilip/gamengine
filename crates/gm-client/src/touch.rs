//! Fingers on the screen (WEB.md 3.5, MODES.md 5.6): what a touch screen makes of the
//! mouse and the keys. The app says what each finger landed on; this module follows the
//! finger from there and says what it did: a tap, a long press, a drag that turns the
//! camera, a pinch that moves it, a stick that walks the body, a button held.
//! Pixels throughout, and the app's clock.

use web_time::Instant;

/// A press held this long without moving is a long press (the secondary, MODES.md 5.6).
pub const LONG_PRESS_SECS: f32 = 0.45;
/// A finger that moves less than this (in dots: scaled by the app) is still a tap.
pub const SLOP_DOTS: f32 = 8.0;
/// The stick's reach from where the finger landed, in dots: at the rim the body runs.
pub const STICK_DOTS: f32 = 56.0;
/// Within this of the stick's centre nothing moves.
const STICK_DEAD: f32 = 0.12;
/// The share of the frame's width, from the left, that is the stick's.
pub const STICK_SHARE: f32 = 0.45;
/// A tap on the look is the primary held this long: a tick or more, one shot.
pub const TAP_HOLD_SECS: f32 = 0.12;
/// A finger's drag turns the camera by this many mouse counts a CSS pixel: a swipe across
/// a phone's width (about 900) is half a turn at the default sensitivity.
pub const LOOK_GAIN: f32 = 3.0;

/// What a finger does from where it landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// A screen is up: the finger is the pointer, a press and a release on a widget.
    Ui,
    /// The RPG world (MODES.md 5.6): a tap targets or walks, a drag turns, a long press
    /// is the secondary.
    World,
    /// The left of the frame in the action and gun modes: a stick around where it landed.
    Stick,
    /// The right of the frame there: a drag looks, a tap is the primary.
    Look,
    /// A control drawn on the HUD: held while the finger is down.
    Button(Button),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Jump,
    Secondary,
    Menu,
    /// A cell of the hotbar, by its index.
    Hot(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// A finger landed: on a widget (the press of a click) or on a button (held).
    Press(Zone, (f32, f32)),
    /// A finger on the pointer's or a button's duty moved: the pointer follows.
    Move(Zone, (f32, f32)),
    /// That finger lifted (a click's release, a button let go).
    Release(Zone, (f32, f32)),
    /// A finger on the world or the look lifted where it landed, quickly.
    Tap(Zone, (f32, f32)),
    /// ... after `LONG_PRESS_SECS` without moving.
    LongPress(Zone, (f32, f32)),
    /// A finger on the world or the look moved: the camera turns by this, in pixels.
    Drag(f32, f32),
    /// Two fingers on the world moved apart (over 1) or together: the ratio of their
    /// distance to what it was.
    Pinch(f32),
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    id: u64,
    zone: Zone,
    landed: (f32, f32),
    at: (f32, f32),
    since: Instant,
    /// It left the slop: a drag, not a tap.
    dragged: bool,
    /// It is one of a pinching pair.
    pinching: bool,
}

/// The fingers down now, and what they did since the app last asked.
#[derive(Default)]
pub struct Fingers {
    fingers: Vec<Finger>,
    events: Vec<Event>,
    /// A finger has touched the screen, or the page said the pointer is a finger's from
    /// the start (WEB.md 3.5): the HUD draws its controls and the UI is at the touch
    /// scale from then on.
    pub seen: bool,
}

impl Fingers {
    /// Fingers that have, or have not, been seen before the first touch: the page says
    /// its pointer is coarse (WEB.md 3.5), so the HUD and the scale are a finger's from
    /// the first frame.
    pub fn expecting(seen: bool) -> Fingers {
        Fingers {
            seen,
            ..Fingers::default()
        }
    }

    /// A finger's event from the window. `zone_of` says what a landing finger is for;
    /// `slop` and `stick` are the tap's room and the stick's reach, in pixels.
    pub fn touch(
        &mut self,
        id: u64,
        phase: Phase,
        at: (f32, f32),
        now: Instant,
        slop: f32,
        zone_of: impl FnOnce((f32, f32)) -> Zone,
    ) {
        self.seen = true;
        match phase {
            Phase::Started => {
                self.fingers.retain(|f| f.id != id);
                let mut zone = zone_of(at);
                let mut pinching = false;
                // A second finger on the world or the look with the first still down:
                // the two pinch, and neither taps.
                if matches!(zone, Zone::World | Zone::Look)
                    && let Some(first) = self
                        .fingers
                        .iter_mut()
                        .find(|f| matches!(f.zone, Zone::World | Zone::Look) && !f.pinching)
                {
                    first.pinching = true;
                    first.dragged = true;
                    zone = first.zone;
                    pinching = true;
                }
                // One stick, one pointer at a time: a second finger there is nothing.
                if matches!(zone, Zone::Stick | Zone::Ui)
                    && self.fingers.iter().any(|f| f.zone == zone)
                {
                    return;
                }
                self.fingers.push(Finger {
                    id,
                    zone,
                    landed: at,
                    at,
                    since: now,
                    // A pinching finger never taps when it lifts.
                    dragged: pinching,
                    pinching,
                });
                if matches!(zone, Zone::Ui | Zone::Button(_)) {
                    self.events.push(Event::Press(zone, at));
                }
            }
            Phase::Moved => {
                let Some(i) = self.fingers.iter().position(|f| f.id == id) else {
                    return;
                };
                let was = self.fingers[i].at;
                self.fingers[i].at = at;
                let f = self.fingers[i];
                if f.pinching {
                    if let Some(other) = self.fingers.iter().find(|o| o.pinching && o.id != id) {
                        let before = dist(was, other.at);
                        let after = dist(at, other.at);
                        if before > 1.0 && after > 1.0 {
                            self.events.push(Event::Pinch(after / before));
                        }
                    }
                    return;
                }
                let far = dist(at, f.landed) > slop;
                match f.zone {
                    Zone::Ui | Zone::Button(_) => self.events.push(Event::Move(f.zone, at)),
                    Zone::World | Zone::Look => {
                        if f.dragged || far {
                            self.fingers[i].dragged = true;
                            self.events.push(Event::Drag(at.0 - was.0, at.1 - was.1));
                        }
                    }
                    Zone::Stick => {}
                }
            }
            Phase::Ended | Phase::Cancelled => {
                let Some(i) = self.fingers.iter().position(|f| f.id == id) else {
                    return;
                };
                let f = self.fingers.remove(i);
                if phase == Phase::Cancelled {
                    if matches!(f.zone, Zone::Ui | Zone::Button(_)) {
                        self.events.push(Event::Release(f.zone, f.at));
                    }
                    return;
                }
                match f.zone {
                    Zone::Ui | Zone::Button(_) => self.events.push(Event::Release(f.zone, at)),
                    Zone::World | Zone::Look if !f.dragged => {
                        let held = now.duration_since(f.since).as_secs_f32();
                        if held >= LONG_PRESS_SECS {
                            self.events.push(Event::LongPress(f.zone, f.landed));
                        } else {
                            self.events.push(Event::Tap(f.zone, f.landed));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// What happened since the last frame, in order.
    pub fn drain(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The stick's axes, forward and side in -1 to 1, while a finger holds it; `reach`
    /// is the rim's distance from the centre in pixels.
    pub fn stick(&self, reach: f32) -> Option<(f32, f32)> {
        let f = self.fingers.iter().find(|f| f.zone == Zone::Stick)?;
        let dx = (f.at.0 - f.landed.0) / reach.max(1.0);
        let dy = (f.at.1 - f.landed.1) / reach.max(1.0);
        let len = (dx * dx + dy * dy).sqrt();
        if len < STICK_DEAD {
            return Some((0.0, 0.0));
        }
        let scale = (len.min(1.0) / len).max(0.0);
        // Up on the screen is forward.
        Some((-dy * scale, dx * scale))
    }

    /// Where the stick is drawn: its centre and the finger's offset from it (the knob),
    /// in pixels.
    pub fn stick_drawn(&self) -> Option<((f32, f32), (f32, f32))> {
        let f = self.fingers.iter().find(|f| f.zone == Zone::Stick)?;
        Some((f.landed, (f.at.0 - f.landed.0, f.at.1 - f.landed.1)))
    }

    /// A finger drags the camera: the frame's look applies its deltas.
    pub fn turning(&self) -> bool {
        self.fingers
            .iter()
            .any(|f| matches!(f.zone, Zone::World | Zone::Look) && f.dragged && !f.pinching)
    }

    /// Whether a button is held by a finger.
    pub fn holds(&self, button: Button) -> bool {
        self.fingers.iter().any(|f| f.zone == Zone::Button(button))
    }

    /// Everything let go (the window lost its focus, the game left the screen).
    pub fn clear(&mut self) {
        self.fingers.clear();
        self.events.clear();
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn world(_: (f32, f32)) -> Zone {
        Zone::World
    }

    #[test]
    fn a_tap_on_the_world_is_a_tap_and_a_drag_is_not() {
        let t0 = Instant::now();
        let mut f = Fingers::default();
        f.touch(1, Phase::Started, (100.0, 100.0), t0, 16.0, world);
        f.touch(1, Phase::Moved, (104.0, 102.0), t0, 16.0, world);
        f.touch(
            1,
            Phase::Ended,
            (104.0, 102.0),
            t0 + Duration::from_millis(120),
            16.0,
            world,
        );
        assert_eq!(f.drain(), vec![Event::Tap(Zone::World, (100.0, 100.0))]);
        // Past the slop: the camera turns by each move, and lifting is no tap.
        f.touch(2, Phase::Started, (100.0, 100.0), t0, 16.0, world);
        f.touch(2, Phase::Moved, (130.0, 100.0), t0, 16.0, world);
        assert!(f.turning());
        f.touch(2, Phase::Moved, (140.0, 90.0), t0, 16.0, world);
        f.touch(
            2,
            Phase::Ended,
            (140.0, 90.0),
            t0 + Duration::from_millis(900),
            16.0,
            world,
        );
        assert_eq!(
            f.drain(),
            vec![Event::Drag(30.0, 0.0), Event::Drag(10.0, -10.0)]
        );
        assert!(!f.turning());
    }

    #[test]
    fn a_long_press_without_moving_is_the_secondary() {
        let t0 = Instant::now();
        let mut f = Fingers::default();
        f.touch(1, Phase::Started, (50.0, 50.0), t0, 16.0, world);
        f.touch(
            1,
            Phase::Ended,
            (52.0, 50.0),
            t0 + Duration::from_millis(600),
            16.0,
            world,
        );
        assert_eq!(f.drain(), vec![Event::LongPress(Zone::World, (50.0, 50.0))]);
    }

    #[test]
    fn the_stick_walks_from_where_the_finger_landed() {
        let t0 = Instant::now();
        let mut f = Fingers::default();
        let stick = |_: (f32, f32)| Zone::Stick;
        f.touch(1, Phase::Started, (200.0, 500.0), t0, 16.0, stick);
        assert_eq!(
            f.stick(100.0),
            Some((0.0, 0.0)),
            "at the centre nothing moves"
        );
        f.touch(1, Phase::Moved, (200.0, 450.0), t0, 16.0, stick);
        let (fwd, side) = f.stick(100.0).unwrap();
        assert!(
            (fwd - 0.5).abs() < 1e-5 && side.abs() < 1e-5,
            "{fwd} {side}"
        );
        f.touch(1, Phase::Moved, (400.0, 500.0), t0, 16.0, stick);
        assert_eq!(f.stick(100.0), Some((0.0, 1.0)), "clamped to the rim");
        // A second finger on the stick is nothing; the first's lift ends the walk.
        f.touch(2, Phase::Started, (210.0, 510.0), t0, 16.0, stick);
        f.touch(2, Phase::Ended, (210.0, 510.0), t0, 16.0, stick);
        assert_eq!(f.stick(100.0), Some((0.0, 1.0)));
        f.touch(1, Phase::Ended, (400.0, 500.0), t0, 16.0, stick);
        assert_eq!(f.stick(100.0), None);
        assert!(f.drain().is_empty(), "the stick makes no events");
    }

    #[test]
    fn two_fingers_pinch_and_neither_taps() {
        let t0 = Instant::now();
        let mut f = Fingers::default();
        f.touch(1, Phase::Started, (100.0, 100.0), t0, 16.0, world);
        f.touch(2, Phase::Started, (200.0, 100.0), t0, 16.0, world);
        f.touch(2, Phase::Moved, (300.0, 100.0), t0, 16.0, world);
        assert!(!f.turning());
        f.touch(1, Phase::Ended, (100.0, 100.0), t0, 16.0, world);
        f.touch(2, Phase::Ended, (300.0, 100.0), t0, 16.0, world);
        assert_eq!(f.drain(), vec![Event::Pinch(2.0)]);
    }

    #[test]
    fn a_finger_on_a_screen_is_the_pointer_and_on_a_button_a_hold() {
        let t0 = Instant::now();
        let mut f = Fingers::default();
        f.touch(1, Phase::Started, (10.0, 10.0), t0, 16.0, |_| Zone::Ui);
        f.touch(1, Phase::Moved, (12.0, 10.0), t0, 16.0, |_| Zone::Ui);
        f.touch(1, Phase::Ended, (12.0, 10.0), t0, 16.0, |_| Zone::Ui);
        assert_eq!(
            f.drain(),
            vec![
                Event::Press(Zone::Ui, (10.0, 10.0)),
                Event::Move(Zone::Ui, (12.0, 10.0)),
                Event::Release(Zone::Ui, (12.0, 10.0)),
            ]
        );
        let jump = |_: (f32, f32)| Zone::Button(Button::Jump);
        f.touch(3, Phase::Started, (900.0, 600.0), t0, 16.0, jump);
        assert!(f.holds(Button::Jump));
        f.touch(3, Phase::Cancelled, (900.0, 600.0), t0, 16.0, jump);
        assert!(!f.holds(Button::Jump));
        assert_eq!(
            f.drain(),
            vec![
                Event::Press(Zone::Button(Button::Jump), (900.0, 600.0)),
                Event::Release(Zone::Button(Button::Jump), (900.0, 600.0)),
            ]
        );
    }
}
