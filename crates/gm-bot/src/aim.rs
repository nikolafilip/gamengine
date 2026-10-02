//! How a bot's view moves (ANTICHEAT.md 10): the test populations of the aim statistics. The
//! brain says where it wants to look; an [`Aimer`] decides where the view actually goes.
//!
//! - `Brain`: the view is the brain's wish, at once (what bots have always done).
//! - `Hand`: a person's hand. It starts turning a reaction time after the wish jumps, turns
//!   at a bounded rate, corrects its aim a handful of times a second rather than every
//!   tick, and never sits still. And a person's eyes: a body behind a pillar is not looked
//!   at (the zone sends it, the client knows where it is, a person does not), and when it
//!   comes out the hand starts towards it a reaction time later.
//! - `Sharp`: the same hand with a good player's head: its wish is the direction that hits,
//!   lead and drop included. What it wants is what the cheat wants; how it gets there is a
//!   hand's.
//! - `Lock`: an aim lock. Whenever an enemy is there the view is exactly the direction that
//!   hits it, lead and drop included, frame after frame.
//! - `Flick`: the view looks away until a shot is two ticks from leaving, then is exactly
//!   on the direction that hits, then looks away again.

use glam::Vec3;
use gm_core::movement::yaw_vectors;
use gm_core::rng::Rng;
use gm_core::sim::Input;
use gm_core::trace::{CollisionWorld, Hull};
use gm_core::vocab::{Origin, Verb};
use gm_net::client::{ClientState, RenderEntity};
use gm_net::snapshot::EntityKind;
use gm_replay::aim::ideal_aim;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AimModel {
    #[default]
    Brain,
    Hand,
    Sharp,
    Lock,
    Flick,
}

impl AimModel {
    pub fn parse(s: &str) -> Option<AimModel> {
        Some(match s {
            "brain" => AimModel::Brain,
            "hand" => AimModel::Hand,
            "sharp" => AimModel::Sharp,
            "lock" => AimModel::Lock,
            "flick" => AimModel::Flick,
            _ => return None,
        })
    }
}

/// A hand's limits: when it starts to move, how fast, how closely it follows, how it shakes.
const HAND_REACTION_TICKS: (u32, u32) = (11, 17);
const HAND_TURN_DEG_PER_TICK: f32 = 9.0;
const HAND_FOLLOW: f32 = 0.22;
const HAND_WISH_JUMP_DEG: f32 = 18.0;
const HAND_SHAKE_DEG: f32 = 0.55;
const HAND_SHAKE_KEEP: f32 = 0.92;
/// A hand corrects its aim every so many ticks (seven to eleven times a second) and coasts
/// in between.
const HAND_CORRECT_TICKS: (u32, u32) = (6, 9);
/// How far the flick looks away between shots, degrees of yaw.
const FLICK_AWAY_DEG: f32 = 55.0;
/// The middle of a body above its origin: what the zone's analysis aims at too.
const CENTRE_Z: f32 = 4.0;

fn wrap(d: f32) -> f32 {
    let mut d = d.rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    d
}

pub struct Aimer {
    pub model: AimModel,
    rng: Rng,
    yaw: f32,
    pitch: f32,
    started: bool,
    /// The wish the hand is following, and the tick it may start following a new one.
    wish: (f32, f32),
    follow_from: u32,
    /// The aim point of the hand's last correction, and when the next one is due.
    held: (f32, f32),
    next_correction: u32,
    shake: (f32, f32),
    /// The nearest enemy was out of sight on the last tick (a hand only).
    hidden: bool,
}

/// Ticks of the world a target's velocity is measured over. One step of a snapshot's
/// positions is too coarse to lead with.
const TRACK_TICKS: f32 = 4.0;

/// A body's velocity as the client's own record of the world shows it: where it stands in
/// the world the client is looking at against where it stood a few ticks of that world
/// earlier. (Counting the client's own frames instead goes wrong whenever frames and
/// snapshots do not come one for one: a busy machine, a bad line.)
fn velocity_seen(client: &ClientState, target: &RenderEntity) -> Vec3 {
    let seen = client.render_tick(0.0);
    client
        .others_at(seen - TRACK_TICKS)
        .iter()
        .find(|e| e.id == target.id && e.alive())
        .map_or(target.vel, |then| {
            (target.pos - then.pos) * (client.rate.hz() as f32 / TRACK_TICKS)
        })
}

/// The projectile a kit's ability fires: speed, gravity scale, muzzle offset.
fn projectile(client: &ClientState, ability: u8) -> Option<(f32, f32, [f32; 3])> {
    let ab = client.sheet.kit.abilities.get(ability as usize)?;
    ab.steps.iter().find_map(|s| match &s.verb {
        Verb::Projectile(p) => Some((
            p.speed,
            p.gravity_scale,
            match p.spawn {
                Origin::Weapon { offset } => offset,
                _ => [0.0; 3],
            },
        )),
        _ => None,
    })
}

/// Yaw and pitch that hit `target` with a projectile from this client's muzzle: the lead
/// and the drop, and the muzzle standing beside the eye.
fn perfect(
    client: &ClientState,
    target: &RenderEntity,
    velocity: Vec3,
    shot: (f32, f32, [f32; 3]),
) -> (f32, f32) {
    let (speed, gravity, offset) = shot;
    let eye = client.mover.eye();
    let centre = target.pos + Vec3::Z * CENTRE_Z;
    let mut dir = centre - eye;
    for _ in 0..3 {
        let yaw = dir.y.atan2(dir.x).to_degrees();
        let (fwd, right) = yaw_vectors(yaw);
        let muzzle = eye + fwd * offset[0] + right * offset[1] + Vec3::Z * offset[2];
        dir = ideal_aim(muzzle, centre, velocity, speed, gravity);
    }
    (
        dir.y.atan2(dir.x).to_degrees().rem_euclid(360.0),
        (-dir.z).atan2(dir.truncate().length()).to_degrees(),
    )
}

impl Aimer {
    pub fn new(model: AimModel, seed: u64) -> Aimer {
        Aimer {
            model,
            rng: Rng::new(seed ^ 0xA1A1_5EED),
            yaw: 0.0,
            pitch: 0.0,
            started: false,
            wish: (0.0, 0.0),
            follow_from: 0,
            held: (0.0, 0.0),
            next_correction: 0,
            shake: (0.0, 0.0),
            hidden: false,
        }
    }

    fn nearest<'a>(
        client: &ClientState,
        team: u8,
        others: &'a [RenderEntity],
    ) -> Option<&'a RenderEntity> {
        let me = client.mover.mv.origin;
        others
            .iter()
            .filter(|e| e.kind == EntityKind::Player && e.alive() && e.id != client.my_id)
            .filter(|e| team == 0 || e.team() == 0 || e.team() != team)
            .min_by(|a, b| {
                (a.pos - me)
                    .length_squared()
                    .total_cmp(&(b.pos - me).length_squared())
            })
    }

    /// The frame as it leaves: the brain's, with the view this model gives it.
    pub fn apply(
        &mut self,
        mut input: Input,
        client: &ClientState,
        world: &dyn CollisionWorld,
        team: u8,
        others: &[RenderEntity],
        tick: u32,
    ) -> Input {
        if !self.started {
            (self.yaw, self.pitch) = (input.yaw, input.pitch);
            self.wish = (input.yaw, input.pitch);
            self.held = self.wish;
            self.started = true;
        }
        // What a person sees: the nearest enemy only if nothing stands between. While it is
        // hidden the hand looks where it looked; when it comes out, the hand has to notice.
        if matches!(self.model, AimModel::Hand | AimModel::Sharp) {
            let eye = client.mover.eye();
            let sees = Self::nearest(client, team, others).is_some_and(|t| {
                world
                    .trace(Hull::Point, eye, t.pos + Vec3::Z * CENTRE_Z)
                    .fraction
                    >= 1.0
            });
            if !sees {
                self.hidden = true;
                input.yaw = self.yaw.rem_euclid(360.0);
                input.pitch = self.pitch.clamp(-89.0, 89.0);
                // A person does not shoot at what it cannot see either.
                input.buttons &=
                    !(gm_core::sim::buttons::PRIMARY | gm_core::sim::buttons::SECONDARY);
                input.ability = 0;
                return input;
            }
            if self.hidden {
                self.hidden = false;
                let (lo, hi) = HAND_REACTION_TICKS;
                self.follow_from = tick.wrapping_add(lo + self.rng.below(hi - lo + 1));
            }
        }
        match self.model {
            AimModel::Brain => return input,
            AimModel::Hand => self.hand(input.yaw, input.pitch, tick),
            AimModel::Sharp | AimModel::Lock | AimModel::Flick => {
                // The shot being fired, or the one the secondary would fire.
                let firing = client
                    .mover
                    .script
                    .and_then(|s| projectile(client, s.ability).map(|p| (p, s)));
                let ready = client
                    .sheet
                    .kit
                    .secondary
                    .and_then(|slot| projectile(client, slot));
                let target = Self::nearest(client, team, others);
                // Its velocity over the last few ticks.
                let velocity = target.map_or(Vec3::ZERO, |t| velocity_seen(client, t));
                match (self.model, target, firing, ready) {
                    (AimModel::Sharp, Some(t), firing, ready)
                        if firing.is_some() || ready.is_some() =>
                    {
                        let shot = firing.map(|f| f.0).or(ready).expect("one of them");
                        let (yaw, pitch) = perfect(client, t, velocity, shot);
                        self.hand(yaw, pitch, tick);
                    }
                    (AimModel::Sharp, _, _, _) => self.hand(input.yaw, input.pitch, tick),
                    (AimModel::Lock, Some(t), firing, ready)
                        if firing.is_some() || ready.is_some() =>
                    {
                        let shot = firing.map(|f| f.0).or(ready).expect("one of them");
                        (self.yaw, self.pitch) = perfect(client, t, velocity, shot);
                    }
                    (AimModel::Flick, Some(t), Some((shot, script)), _) => {
                        // The projectile leaves at the script's step: on it two ticks
                        // before, away from it otherwise.
                        let release = client.sheet.kit.abilities[script.ability as usize]
                            .steps
                            .iter()
                            .find(|s| matches!(s.verb, Verb::Projectile(_)))
                            .map_or(0, |s| s.at);
                        let since = tick.wrapping_sub(script.started);
                        let (yaw, pitch) = perfect(client, t, velocity, shot);
                        if since + 2 >= release && since <= release {
                            (self.yaw, self.pitch) = (yaw, pitch);
                        } else {
                            (self.yaw, self.pitch) =
                                ((yaw + FLICK_AWAY_DEG).rem_euclid(360.0), pitch);
                        }
                    }
                    (AimModel::Flick, Some(t), None, Some(shot)) => {
                        let (yaw, pitch) = perfect(client, t, velocity, shot);
                        (self.yaw, self.pitch) = ((yaw + FLICK_AWAY_DEG).rem_euclid(360.0), pitch);
                    }
                    _ => (self.yaw, self.pitch) = (input.yaw, input.pitch),
                }
            }
        }
        input.yaw = self.yaw.rem_euclid(360.0);
        input.pitch = self.pitch.clamp(-89.0, 89.0);
        input
    }

    fn hand(&mut self, wish_yaw: f32, wish_pitch: f32, tick: u32) {
        // A wish that jumps (another target, a turn about) is noticed, then followed.
        let jump = wrap(wish_yaw - self.wish.0)
            .abs()
            .max((wish_pitch - self.wish.1).abs());
        if jump >= HAND_WISH_JUMP_DEG {
            let (lo, hi) = HAND_REACTION_TICKS;
            self.follow_from = tick.wrapping_add(lo + self.rng.below(hi - lo + 1));
        }
        self.wish = (wish_yaw, wish_pitch);
        if (tick.wrapping_sub(self.follow_from) as i32) >= 0 {
            // A correction: the hand takes a new aim point, a little off, and holds it
            // until the next one.
            if (tick.wrapping_sub(self.next_correction) as i32) >= 0 {
                let (lo, hi) = HAND_CORRECT_TICKS;
                self.next_correction = tick.wrapping_add(lo + self.rng.below(hi - lo + 1));
                self.held = (
                    wish_yaw + self.rng.range_f32(-1.0, 1.0) * HAND_SHAKE_DEG,
                    wish_pitch + self.rng.range_f32(-1.0, 1.0) * HAND_SHAKE_DEG,
                );
            }
            let (wish_yaw, wish_pitch) = self.held;
            let step =
                |d: f32| (d * HAND_FOLLOW).clamp(-HAND_TURN_DEG_PER_TICK, HAND_TURN_DEG_PER_TICK);
            // The shake is taken out before following and put back after: it rides on the
            // hand, it is not where the hand means to be.
            let (base_yaw, base_pitch) = (self.yaw - self.shake.0, self.pitch - self.shake.1);
            let base_yaw = base_yaw + step(wrap(wish_yaw - base_yaw));
            let base_pitch = base_pitch + step(wish_pitch - base_pitch);
            self.shake.0 = self.shake.0 * HAND_SHAKE_KEEP
                + self.rng.range_f32(-1.0, 1.0) * HAND_SHAKE_DEG * (1.0 - HAND_SHAKE_KEEP) * 4.0;
            self.shake.1 = self.shake.1 * HAND_SHAKE_KEEP
                + self.rng.range_f32(-1.0, 1.0) * HAND_SHAKE_DEG * (1.0 - HAND_SHAKE_KEEP) * 4.0;
            self.yaw = base_yaw + self.shake.0;
            self.pitch = base_pitch + self.shake.1;
        }
    }
}
