//! The scripted player. Pure: it reads a [`View`] of the world and returns one tick of input.

use glam::Vec3;
use gm_core::build::{ContentPack, Kit};
use gm_core::matrix::Aspects;
use gm_core::movement::yaw_vectors;
use gm_core::rng::Rng;
use gm_core::sim::{Input, Mover, anim, buttons, tick_delta};
use gm_core::vocab::{
    ArchetypeFrame, Guard, MeleeArc, MoveKind, Origin, Shape, StatusTarget, Verb,
};
use gm_net::client::RenderEntity;
use gm_net::snapshot::EntityKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Stand still, face the nearest enemy, swing when it is in reach.
    Hold,
    /// Run around, jump now and then, shoot and swing at whoever is nearest.
    Wander,
    /// Chase the nearest enemy: primary in reach, secondary otherwise.
    Hunter,
    /// Use the whole kit: melee kits close in and guard, ranged kits kite, actives fire when
    /// their shape says so. The arena acceptance test runs this on both sides.
    Duelist,
}

/// What the brain sees this tick.
pub struct View<'a> {
    pub me: &'a Mover,
    pub kit: &'a Kit,
    pub frame: ArchetypeFrame,
    pub team: u8,
    pub alive: bool,
    /// Other entities at the render time (players, projectiles, areas).
    pub others: &'a [RenderEntity],
    /// The brain's tick counter (the mover's frame clock).
    pub tick: u32,
}

/// Rough classification of an active for the duelist.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ActiveUse {
    Buff,
    /// Shockwave at the feet with this radius.
    Burst(f32),
    /// Area placed ahead at this distance.
    Placed(f32),
    Shot,
    Swing(f32),
    Closer,
    Blink,
}

fn classify(kit: &Kit, slot: u8) -> Option<ActiveUse> {
    let ab = &kit.abilities[slot as usize];
    let step = ab.steps.first()?;
    Some(match &step.verb {
        Verb::ApplyStatus(s) if s.target == StatusTarget::Actor => ActiveUse::Buff,
        Verb::ApplyStatus(_) => return None,
        Verb::AreaEffect(a) => {
            let radius = match a.shape {
                Shape::Sphere { radius } | Shape::Cylinder { radius, .. } => radius,
                Shape::Cone { length, .. } => length,
                Shape::Box { half_extents } => half_extents[0],
            };
            match a.origin {
                Origin::Weapon { offset } => ActiveUse::Placed(offset[0] + radius * 0.5),
                _ => ActiveUse::Burst(radius),
            }
        }
        Verb::Projectile(_) => ActiveUse::Shot,
        Verb::MeleeArc(m) => ActiveUse::Swing(m.reach),
        Verb::MoveSelf(m) => match m.kind {
            MoveKind::Blink { .. } => ActiveUse::Blink,
            _ => ActiveUse::Closer,
        },
        Verb::Guard(_) => return None,
    })
}

fn primary_arc(kit: &Kit) -> Option<&MeleeArc> {
    let i = kit.primary? as usize;
    match &kit.abilities[i].steps.first()?.verb {
        Verb::MeleeArc(m) => Some(m),
        _ => None,
    }
}

pub struct Brain {
    rng: Rng,
    pub behaviour: Behaviour,
    yaw: f32,
    pitch: f32,
    next_turn: u32,
    next_shot: u32,
    jump_until: u32,
    strafe: f32,
    next_strafe: u32,
}

impl Brain {
    pub fn new(seed: u64, behaviour: Behaviour) -> Brain {
        let mut rng = Rng::new(seed);
        let yaw = rng.range_f32(0.0, 360.0);
        Brain {
            rng,
            behaviour,
            yaw,
            pitch: 0.0,
            next_turn: 64,
            next_shot: 90,
            jump_until: 0,
            strafe: 1.0,
            next_strafe: 0,
        }
    }

    fn nearest<'a>(
        &self,
        me: Vec3,
        team: u8,
        others: &'a [RenderEntity],
    ) -> Option<(&'a RenderEntity, f32)> {
        others
            .iter()
            .filter(|e| e.kind == EntityKind::Player && e.alive())
            .filter(|e| team == 0 || e.team() == 0 || e.team() != team)
            .map(|e| (e, (e.pos - me).length()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    fn face(&mut self, me_eye: Vec3, target: Vec3) {
        let to = target + Vec3::new(0.0, 0.0, 28.0) - me_eye;
        let horiz = to.truncate().length();
        self.yaw = to.y.atan2(to.x).to_degrees().rem_euclid(360.0);
        self.pitch = (-to.z).atan2(horiz).to_degrees().clamp(-89.0, 89.0);
    }

    pub fn think(&mut self, v: &View<'_>) -> Input {
        let me = v.me.mv.origin;
        let eye = v.me.eye();
        let tick = v.tick;
        let mut buttons = 0u16;
        let mut forward = 0.0f32;
        let mut side = 0.0f32;
        let mut ability = 0u8;
        let nearest = self.nearest(me, v.team, v.others);
        let reach = primary_arc(v.kit).map_or(70.0, |m| m.reach);
        match self.behaviour {
            Behaviour::Hold => {
                if let Some((e, d)) = nearest {
                    self.face(eye, e.pos);
                    if d < reach && tick.is_multiple_of(24) {
                        buttons |= buttons::PRIMARY;
                    }
                }
            }
            Behaviour::Wander => {
                if tick >= self.next_turn {
                    self.yaw = self.rng.range_f32(0.0, 360.0);
                    self.pitch = 0.0;
                    self.next_turn = tick + 64 + self.rng.below(128);
                }
                forward = 1.0;
                if self.rng.next_f32() < 0.02 {
                    self.jump_until = tick + 2;
                }
                if tick < self.jump_until {
                    buttons |= buttons::JUMP;
                }
                if let Some((e, d)) = nearest {
                    if d < reach && tick.is_multiple_of(24) {
                        self.face(eye, e.pos);
                        buttons |= buttons::PRIMARY;
                    } else if tick >= self.next_shot {
                        self.face(eye, e.pos);
                        buttons |= buttons::SECONDARY;
                        self.next_shot = tick + 96 + self.rng.below(64);
                    }
                }
            }
            Behaviour::Hunter => {
                if let Some((e, d)) = nearest {
                    self.face(eye, e.pos);
                    if d > 50.0 {
                        forward = 1.0;
                    }
                    if d < reach {
                        if tick.is_multiple_of(20) {
                            buttons |= buttons::PRIMARY;
                        }
                    } else if d > 150.0 && tick >= self.next_shot {
                        buttons |= buttons::SECONDARY;
                        self.next_shot = tick + 96;
                    }
                } else {
                    forward = 1.0;
                    if tick >= self.next_turn {
                        self.yaw = self.rng.range_f32(0.0, 360.0);
                        self.next_turn = tick + 96;
                    }
                }
            }
            Behaviour::Duelist => {
                let ranged = matches!(
                    v.frame,
                    ArchetypeFrame::Caster | ArchetypeFrame::Infiltrator
                );
                let Some((e, d)) = nearest else {
                    // Nobody in sight: patrol.
                    forward = 1.0;
                    if tick >= self.next_turn {
                        self.yaw = self.rng.range_f32(0.0, 360.0);
                        self.pitch = 0.0;
                        self.next_turn = tick + 96;
                    }
                    return self.input(buttons, forward, side, ability);
                };
                self.face(eye, e.pos);
                let ready = |slot: u8| tick_delta(tick, v.me.cooldowns[slot as usize]) >= 0;
                let busy = v.me.script.is_some();
                let enemy_winding = matches!(e.anim, anim::WINDUP | anim::CAST);
                // Movement: melee closes, ranged kites; everyone strafes a little.
                if tick >= self.next_strafe {
                    self.strafe = if self.rng.next_f32() < 0.5 { 1.0 } else { -1.0 };
                    self.next_strafe = tick + 40 + self.rng.below(60);
                }
                let want = if ranged { 320.0 } else { reach * 0.7 };
                if d > want + 20.0 {
                    forward = 1.0;
                } else if ranged && d < want - 80.0 {
                    forward = -1.0;
                }
                if d < 500.0 {
                    side = self.strafe * 0.6;
                }
                // Guard: block or parry a wind-up in reach.
                if !busy
                    && d < reach + 40.0
                    && enemy_winding
                    && let Some(g) = v.kit.guard_verb()
                {
                    match g {
                        Guard::Block(_) => buttons |= buttons::GUARD,
                        Guard::Parry(_) => {
                            if let Some(gi) = v.kit.guard
                                && ready(gi)
                                && tick.is_multiple_of(2)
                            {
                                buttons |= buttons::GUARD;
                            }
                        }
                    }
                }
                // Actives, highest slot first so the expensive ones get used.
                if !busy && buttons & buttons::GUARD == 0 {
                    for (i, slot) in v.kit.actives.iter().enumerate().rev() {
                        let Some(slot) = *slot else { continue };
                        if !ready(slot) {
                            continue;
                        }
                        let ab = &v.kit.abilities[slot as usize];
                        if v.me.stamina < ab.cost.stamina as f32
                            || v.me.focus < ab.cost.focus as f32
                        {
                            continue;
                        }
                        let use_it = match classify(v.kit, slot) {
                            Some(ActiveUse::Buff) => d < 450.0,
                            Some(ActiveUse::Burst(r)) => d < r * 0.8,
                            Some(ActiveUse::Placed(at)) => d < at + 60.0 && d > 40.0,
                            Some(ActiveUse::Shot) => (120.0..700.0).contains(&d),
                            Some(ActiveUse::Swing(r)) => d < r,
                            Some(ActiveUse::Closer) => {
                                if ranged {
                                    d < 140.0 && {
                                        forward = -1.0;
                                        true
                                    }
                                } else {
                                    (200.0..600.0).contains(&d)
                                }
                            }
                            Some(ActiveUse::Blink) => {
                                if ranged {
                                    d < 120.0 && {
                                        // Blink away: turn around for the blink tick.
                                        self.yaw = (self.yaw + 180.0).rem_euclid(360.0);
                                        true
                                    }
                                } else {
                                    (260.0..600.0).contains(&d)
                                }
                            }
                            None => false,
                        };
                        if use_it {
                            ability = i as u8 + 1;
                            break;
                        }
                    }
                }
                if ability == 0 && !busy && buttons & buttons::GUARD == 0 {
                    if d < reach && tick.is_multiple_of(4) {
                        buttons |= buttons::PRIMARY;
                    } else if d > reach
                        && d < 900.0
                        && tick >= self.next_shot
                        && let Some(si) = v.kit.secondary
                        && ready(si)
                    {
                        buttons |= buttons::SECONDARY;
                        self.next_shot = tick + 8;
                    }
                }
            }
        }
        self.input(buttons, forward, side, ability)
    }

    fn input(&self, buttons: u16, forward: f32, side: f32, ability: u8) -> Input {
        Input {
            buttons,
            yaw: self.yaw,
            pitch: self.pitch,
            forward,
            side,
            ability,
        }
    }
}

/// Facing vectors for tests and the arena driver.
pub fn facing(yaw: f32) -> (Vec3, Vec3) {
    yaw_vectors(yaw)
}

/// The preset that best counters what the enemy is fielding, from the matrix alone: for each
/// preset, the product of its elements' multipliers into the enemy aspects over the enemy
/// elements' multipliers into the preset's aspects. `enemy` is the aspect mask seen most often
/// among enemies. Returns `None` when nothing beats the current build by a margin.
pub fn counter_pick(pack: &ContentPack, current: &str, enemy: Aspects) -> Option<String> {
    if enemy.count() == 0 {
        return None;
    }
    let score = |aspects: Aspects| -> f32 {
        // Best element we can bring against them, over the worst they can bring against us.
        let offence: f32 = aspects
            .iter()
            .map(|e| enemy.mult_against(e))
            .fold(0.0, f32::max);
        let defence: f32 = enemy
            .iter()
            .map(|e| aspects.mult_against(e))
            .fold(0.0, f32::max);
        offence / defence.max(1e-3)
    };
    let current_score = pack.build(current).map_or(0.0, |b| score(b.aspects));
    let best = pack
        .builds
        .iter()
        .map(|nb| (nb, score(nb.build.aspects)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    (best.0.name != current && best.1 > current_score * 1.5).then(|| best.0.name.clone())
}

/// The aspect mask seen most often among living enemies.
pub fn dominant_enemy_aspects(team: u8, others: &[RenderEntity]) -> Aspects {
    let mut counts: Vec<(u8, u32)> = Vec::new();
    for e in others {
        if e.kind != EntityKind::Player || !e.alive() {
            continue;
        }
        let gm_net::snapshot::SpawnInfo::Player {
            team: t, aspects, ..
        } = e.spawn
        else {
            continue;
        };
        if team != 0 && t == team {
            continue;
        }
        match counts.iter_mut().find(|(a, _)| *a == aspects) {
            Some((_, n)) => *n += 1,
            None => counts.push((aspects, 1)),
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map_or(Aspects::NONE, |(a, _)| Aspects(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_core::matrix::Element;
    use gm_core::sim::test_content;
    use gm_core::tick::TickRate;

    #[test]
    fn counter_pick_follows_the_matrix() {
        let pack = test_content::pack(TickRate::COMBAT);
        // Against ironclads (Stone): frostweaver (Frost beats Stone; Stone into Frost+Shadow is 0.25x).
        assert_eq!(
            counter_pick(&pack, "blade", Aspects::one(Element::Stone)).as_deref(),
            Some("frostweaver")
        );
        // Against frostweavers (Frost + Shadow): blade (Flame is 4x into both).
        assert_eq!(
            counter_pick(
                &pack,
                "ironclad",
                Aspects::two(Element::Frost, Element::Shadow)
            )
            .as_deref(),
            Some("blade")
        );
        // Against blades (Flame): ironclad (Stone beats Flame, Flame into Stone is 0.5).
        assert_eq!(
            counter_pick(&pack, "frostweaver", Aspects::one(Element::Flame)).as_deref(),
            Some("ironclad")
        );
        // Already the counter: nothing to change.
        assert_eq!(
            counter_pick(&pack, "frostweaver", Aspects::one(Element::Stone)),
            None
        );
        assert_eq!(counter_pick(&pack, "blade", Aspects::NONE), None);
    }
}
