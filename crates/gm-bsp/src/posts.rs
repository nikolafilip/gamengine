//! Creature posts (COMPANIONS.md 8.1): where a map stands its creatures. A `gm_creature`
//! entity names a creature definition of the content and the encounter it belongs to; the
//! zone, the bots and the tools read the same entity.

use glam::Vec3;

use crate::Bsp;

#[derive(Clone, Debug, PartialEq)]
pub struct CreaturePost {
    /// Key of the creature definition (`assets/content/creatures.toml`).
    pub creature: String,
    /// Creatures that share an encounter name fight, reset and are cleared together.
    pub encounter: String,
    pub origin: Vec3,
    pub yaw: f32,
}

/// The most creatures one map may post.
pub const MAX_CREATURE_POSTS: usize = 256;

impl Bsp {
    /// The `gm_creature`s of the map, in entity order. An entity without an origin or a
    /// creature key is left out; one without an encounter is its own encounter.
    pub fn creature_posts(&self) -> Vec<CreaturePost> {
        self.entities
            .iter()
            .filter(|e| e.classname() == "gm_creature")
            .filter_map(|e| {
                let creature = e.get("creature").filter(|c| !c.is_empty())?.to_string();
                let origin = e.origin()?;
                let encounter = e
                    .get("encounter")
                    .filter(|n| !n.is_empty())
                    .map_or_else(|| format!("{creature}@{}", origin.x as i32), str::to_string);
                Some(CreaturePost {
                    creature,
                    encounter,
                    origin,
                    yaw: e.f32("angle").unwrap_or(0.0),
                })
            })
            .take(MAX_CREATURE_POSTS)
            .collect()
    }
}
