//! A zone as its minds are shown it: the bridge from `gm_core::sim::Zone` to [`Senses`].
//! Built once per tick and shared; each mind still has to see a body before it acts on it.

use gm_core::sim::Zone;
use gm_core::trace::CollisionWorld;
use gm_core::vocab::{EntityId, Status};

use crate::nav::NavGrid;
use crate::sense::{AreaSight, Body, Senses};

#[derive(Clone, Debug, Default)]
pub struct ZoneView {
    pub bodies: Vec<Body>,
    pub areas: Vec<AreaSight>,
}

impl ZoneView {
    /// Every body and area of the zone. `creature_of` names the creature definition a body
    /// is an instance of.
    pub fn of(zone: &Zone, creature_of: &dyn Fn(EntityId) -> Option<u16>) -> ZoneView {
        let mut view = ZoneView::default();
        view.refresh(zone, creature_of);
        view
    }

    pub fn refresh(&mut self, zone: &Zone, creature_of: &dyn Fn(EntityId) -> Option<u16>) {
        self.bodies.clear();
        self.areas.clear();
        for p in zone.players() {
            self.bodies.push(Body {
                id: p.id,
                pos: p.mover.mv.origin,
                vel: p.mover.mv.velocity,
                yaw: p.mover.yaw,
                frame: p.frame(),
                armour: p.sheet.build.armour,
                team: p.team(),
                party: p.party,
                alive: p.alive && !p.ghost,
                anim: p.anim,
                crouched: p.mover.crouched,
                status: p.mover.statuses.mask(),
                health: Some((p.health.max(0) as i64 * 1000 / p.max_health().max(1) as i64) as u16),
                creature: creature_of(p.id),
                stealth: if p.mover.statuses.has(Status::Stealth) {
                    p.mover.statuses.magnitude(Status::Stealth)
                } else {
                    f32::INFINITY
                },
            });
        }
        for a in zone.areas() {
            self.areas.push(AreaSight {
                id: a.id,
                pos: a.origin,
                radius: a.radius(),
                owner: a.owner,
                harmful: a.def.harmful(),
                spares_owner: a.def.exclude_actor,
            });
        }
    }

    /// What the mind of body `id` is shown this tick; `None` when the body is gone.
    pub fn senses<'a>(
        &'a self,
        zone: &'a Zone,
        id: EntityId,
        world: &'a dyn CollisionWorld,
        nav: &'a NavGrid,
    ) -> Option<Senses<'a>> {
        let p = zone.player(id)?;
        Some(Senses {
            tick: p.last_input_tick.wrapping_add(1),
            hz: zone.rate.hz(),
            id,
            me: &p.mover,
            sheet: &p.sheet,
            health: p.health,
            team: p.team(),
            party: p.party,
            bodies: &self.bodies,
            areas: &self.areas,
            world,
            nav,
            pack: &zone.content,
        })
    }
}
