//! gm-core: the shared simulation.
//!
//! Runs identically on the client (prediction) and the server (authority). No I/O, no rendering,
//! no networking. Everything here is plain data and pure functions over it.
//!
//! - [`tick`]: tick rates and tick arithmetic (64 Hz combat, 20 Hz towns).
//! - [`trace`]: the collision-world trait and the trace result every mover consumes.
//! - [`movement`]: Quake-style player movement (friction, acceleration, step-up, sliding).
//! - [`vocab`]: the entity vocabulary, the data mirror of `docs/VOCABULARY.md`.
//!
//! Coordinates are Quake's: Z is up, one world unit is 1/32 m by convention.
#![forbid(unsafe_code)]

pub mod movement;
pub mod tick;
pub mod trace;
pub mod vocab;
