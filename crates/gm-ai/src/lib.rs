//! gm-ai: minds (`docs/COMPANIONS.md`).
//!
//! A mind drives a body exactly as a client does: it reads what a client in its place would
//! be shown and returns one [`gm_core::sim::Input`] per tick. Nothing here touches the
//! simulation's rules, the network or a clock.
//!
//! - [`nav`]: the navigation grid built from a map's collision, paths and steering.
//! - [`sense`]: what a mind is shown.
//! - [`kit`]: reading a kit by the shape of its abilities; roles.
//! - [`fighter`]: what every mind shares: aiming within limits, routes, guarding, the kit.
//! - [`companion`]: a hired avatar's or a recruit's mind: a role and a standing order.
//! - [`creature`]: a creature's mind: a post, a threat table, a leash.
//! - [`raider`]: a player's stand-in for headless clients and offline runs.
//! - [`view`]: a `gm_core::sim::Zone` as its minds are shown it.
//! - [`director`]: the zone's side: squads, creature posts, encounters, ledgers, loot, trials.
#![forbid(unsafe_code)]

pub mod companion;
pub mod creature;
pub mod director;
pub mod fighter;
pub mod kit;
pub mod nav;
pub mod raider;
pub mod sense;
pub mod view;

pub use companion::{Companion, Order, Squad};
pub use creature::Creature;
pub use director::{
    BodyKind, CompanionSpec, CreatureSpawn, Director, DirectorEvent, EncounterState, LootGrant,
};
pub use fighter::{Engage, Fighter};
pub use kit::{KitPlan, Role, Use};
pub use nav::{NavGrid, Navigator};
pub use raider::{Mate, Raider, Request, objectives};
pub use sense::{AreaSight, Body, Senses};
