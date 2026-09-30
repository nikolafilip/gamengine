//! gm-core: the shared simulation.
//!
//! Runs identically on the client (prediction) and the server (authority). No I/O, no rendering,
//! no networking. Everything here is plain data and pure functions over it.
//!
//! - [`tick`]: tick rates and tick arithmetic (64 Hz combat, 20 Hz towns).
//! - [`trace`]: the collision-world trait and the trace result every mover consumes.
//! - [`collide`]: box sweeps and the composite world (BSP plus other movers).
//! - [`geom`]: capsules, segments and swept spheres for hit detection.
//! - [`movement`]: Quake-style player movement (friction, acceleration, step-up, sliding).
//! - [`vocab`]: the entity vocabulary, the data mirror of `docs/VOCABULARY.md`.
//! - [`sim`]: the zone simulation (movers, abilities, projectiles, melee lag compensation).
//! - [`rng`]: a small deterministic generator.
//!
//! Coordinates are Quake's: Z is up, one world unit is 1/32 m by convention.
#![forbid(unsafe_code)]

pub mod collide;
pub mod geom;
pub mod movement;
pub mod rng;
pub mod sim;
pub mod tick;
pub mod trace;
pub mod vocab;
