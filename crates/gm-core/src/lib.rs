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
//! - [`matrix`]: elements, armour, derived stats and the damage pipeline (`docs/MATRIX.md`).
//! - [`build`]: point-buy builds, creatures, content packs and the kits they compile to.
//! - [`status`]: status effects on a mover.
//! - [`sim`]: the zone simulation (movers, abilities, projectiles, areas, guards, statuses).
//! - [`loot`]: the boss loot split; [`encounter`]: the ledger an encounter keeps;
//!   [`trial`]: role trials and their verdicts (`docs/COMPANIONS.md`).
//! - [`rng`]: a small deterministic generator.
//!
//! Coordinates are Quake's: Z is up, one world unit is 1/32 m by convention.
#![forbid(unsafe_code)]
#![recursion_limit = "512"]

pub mod build;
pub mod collide;
pub mod encounter;
pub mod geom;
pub mod loot;
pub mod matrix;
pub mod movement;
pub mod rng;
pub mod sim;
pub mod status;
pub mod tick;
pub mod trace;
pub mod trial;
pub mod vocab;
