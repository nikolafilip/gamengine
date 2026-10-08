//! The raid leader (COMPANIONS.md 14): the bot as a lone player with a squad. It walks the
//! creature posts of the map with `gm_ai::Raider` and commands from the stance. What it knows
//! of the world is what a client knows: its snapshots, the zone's control messages, the map
//! and the content.

use std::collections::HashMap;

use glam::Vec3;
use gm_ai::{AreaSight, Body, Mate, NavGrid, Raider, Request, Role, Senses, objectives};
use gm_bsp::Bsp;
use gm_core::build::ContentPack;
use gm_core::matrix::ArmourClass;
use gm_core::sim::Input;
use gm_core::vocab::{ArchetypeFrame, EntityId, Verb};
use gm_net::client::{ClientState, RenderEntity};
use gm_net::control::{BodyKind, FromClient, Order, SquadEntry};
use gm_net::snapshot::SpawnInfo;

pub struct Raid {
    nav: NavGrid,
    raider: Option<Raider>,
    posts: Vec<(String, Vec3)>,
    /// Who drives each body, as the zone announced it.
    pub kinds: HashMap<EntityId, BodyKind>,
    /// The own squad, as the zone last told it.
    pub squad: Vec<SquadEntry>,
    bodies: Vec<Body>,
    areas: Vec<AreaSight>,
    seed: u64,
    /// Fight beside the squad instead of leading from the back.
    pub fights: bool,
}

fn frame_of(index: u8) -> ArchetypeFrame {
    match index {
        0 => ArchetypeFrame::Colossus,
        1 => ArchetypeFrame::Striker,
        2 => ArchetypeFrame::Caster,
        _ => ArchetypeFrame::Infiltrator,
    }
}

impl Raid {
    /// Flood the map from its spawns and creature posts, as the zone does.
    pub fn new(world: &Bsp, seed: u64) -> Raid {
        let posts: Vec<(String, Vec3)> = world
            .creature_posts()
            .into_iter()
            .map(|p| (p.encounter, p.origin))
            .collect();
        let seeds: Vec<Vec3> = world
            .entities
            .iter()
            .filter(|e| matches!(e.classname(), "info_player_start" | "gm_spawn"))
            .filter_map(|e| e.origin())
            .chain(posts.iter().map(|p| p.1))
            .collect();
        Raid {
            nav: NavGrid::build(world, &seeds),
            raider: None,
            posts,
            kinds: HashMap::new(),
            squad: Vec::new(),
            bodies: Vec::new(),
            areas: Vec::new(),
            seed,
            fights: false,
        }
    }

    /// Every encounter of the map is cleared.
    pub fn done(&self) -> bool {
        self.raider.as_ref().is_some_and(|r| r.done())
    }

    /// Encounters cleared so far, by the raider's own count.
    pub fn objective(&self) -> usize {
        self.raider.as_ref().map_or(0, |r| r.objective())
    }

    /// One frame: the input, and an order for the zone when the leader gives one.
    pub fn think(
        &mut self,
        client: &ClientState,
        others: &[RenderEntity],
        world: &Bsp,
        pack: &ContentPack,
        team: u8,
    ) -> (Input, Option<FromClient>) {
        let me = client.my_id;
        if !client.own_alive || client.newest_tick == 0 {
            // The dead wait (COMPANIONS.md 9); and before the first snapshot the body is
            // nowhere yet: the raid starts from where the zone put it.
            return (
                Input {
                    yaw: client.mover.yaw,
                    pitch: client.mover.pitch,
                    ..Input::default()
                },
                None,
            );
        }
        self.bodies.clear();
        self.areas.clear();
        for e in others {
            match e.spawn {
                SpawnInfo::Player {
                    frame,
                    team,
                    armour,
                    ..
                } => {
                    let kind = self.kinds.get(&e.id).copied().unwrap_or_default();
                    let (party, creature, max_health) = match kind {
                        BodyKind::Human => (e.id, None, None),
                        BodyKind::Companion { owner } => (
                            owner,
                            None,
                            self.squad
                                .iter()
                                .find(|m| m.id == e.id)
                                .map(|m| m.max_health),
                        ),
                        BodyKind::Creature { def } => (
                            0,
                            Some(def),
                            pack.creatures.get(def as usize).map(|c| c.health),
                        ),
                    };
                    self.bodies.push(Body {
                        id: e.id,
                        pos: e.pos,
                        vel: e.vel,
                        yaw: e.yaw,
                        frame: frame_of(frame),
                        armour: ArmourClass::from_index(armour).unwrap_or(ArmourClass::Cloth),
                        team,
                        party,
                        alive: e.alive(),
                        anim: e.anim,
                        crouched: e.flags & gm_net::snapshot::flags::CROUCHED != 0,
                        status: e.status,
                        // Per mille of its maximum, where the zone sends health at all.
                        health: e.health.zip(max_health).map(|(h, max)| {
                            (h as u32 * 1000 / (max as u32).max(1)).min(1000) as u16
                        }),
                        creature,
                        // A stealthed body the zone still sends is one within its radius.
                        stealth: f32::INFINITY,
                    });
                }
                SpawnInfo::Area {
                    owner,
                    def,
                    radius,
                    harmful,
                } => {
                    // The own kit says whether an own shockwave spares its caster.
                    let spares_owner = owner == me
                        && client
                            .sheet
                            .kit
                            .abilities
                            .get(def as usize)
                            .is_some_and(|ab| {
                                ab.steps.iter().any(
                                    |st| matches!(&st.verb, Verb::AreaEffect(a) if a.exclude_actor),
                                )
                            });
                    self.areas.push(AreaSight {
                        id: e.id,
                        pos: e.pos,
                        radius: radius as f32,
                        owner,
                        harmful,
                        spares_owner,
                    });
                }
                SpawnInfo::Projectile { .. } => {}
            }
        }
        let senses = Senses {
            tick: client.tick.wrapping_add(1),
            hz: client.rate.hz(),
            id: me,
            me: &client.mover,
            sheet: &client.sheet,
            health: client.own_health,
            team,
            party: me,
            bodies: &self.bodies,
            areas: &self.areas,
            world,
            nav: &self.nav,
            pack,
        };
        let fights = self.fights;
        let raider = self.raider.get_or_insert_with(|| {
            let mut r = Raider::new(
                self.seed,
                &client.sheet,
                client.mover.yaw,
                objectives(&self.nav, client.mover.mv.origin, &self.posts),
            );
            r.fights = fights;
            r
        });
        let mates: Vec<Mate> = self
            .squad
            .iter()
            .enumerate()
            .map(|(slot, m)| Mate {
                id: m.id,
                slot: slot as u8,
                role: Role::from_index(m.role),
                attacking: match m.order {
                    Order::Attack(id) => Some(id),
                    _ => None,
                },
            })
            .collect();
        let (input, request) = raider.think(&senses, &mates);
        if client.tick.is_multiple_of(client.rate.hz()) {
            tracing::debug!(
                at = ?client.mover.mv.origin.round(),
                health = client.own_health,
                bodies = self.bodies.len(),
                creatures = self
                    .bodies
                    .iter()
                    .filter(|b| b.alive && b.creature.is_some())
                    .count(),
                squad = self.squad.len(),
                "raid: {}",
                raider.describe()
            );
        }
        let control = request.map(|Request::Order { slots, order }| FromClient::Order {
            slots,
            order: match order {
                gm_ai::Order::Follow => Order::Follow,
                gm_ai::Order::Hold => Order::Hold,
                gm_ai::Order::MoveTo(p) => Order::MoveTo(p.into()),
                gm_ai::Order::Attack(id) => Order::Attack(id),
            },
        });
        (input, control)
    }
}
