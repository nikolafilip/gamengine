# Entity Vocabulary

Status: v0.4, Phase 7 (v0.3 of Phase 3 with sections 5.2–5.6 tightened and 14 added; section
15 adds the driver of a body, `Origin::Aim`, zero packets and the command stance). This document is the contract for
phases 2–7. `gm-core::vocab` mirrors it as plain data types; when the two disagree, this document
wins and the code is wrong. Changes to either require changing both in the same commit.

## 1. Principles

1. One simulation, one vocabulary. Genres (FPS, third-person, tactical) are camera, control
   scheme and HUD. They never add server rules, with one stated exception: the tactical
   viewport's sight through the squad is a grant, so its price, the command stance (section
   15), is a rule of the simulation. A body is driven by a client or by a mind; either way it
   is the same body and the frames it runs are the same inputs.
2. Six server verbs. Every ability, spell, weapon and monster attack is a timed script of verbs
   with parameters. There is no ability-specific server code path.
3. The server resolves every verb. The client predicts its own movement and `MoveSelf`, shows
   local previews of the rest, and reconciles.
4. Parameters are data in the content repository, validated at load against the ranges in
   section 11. Out-of-range content fails to load; it never fails at runtime.

## 2. Units and time

| Quantity | Unit | Notes |
|---|---|---|
| Length | world unit (u) | Quake unit. 32 u = 1 m by convention. Player hull 32 × 32 × 56 u, eyes 22 u above the hull origin. |
| Time | tick | Combat zones 64 Hz (15.625 ms). Towns 20 Hz. Content authors write milliseconds; `TickRate::ms_to_ticks` rounds **up** so nothing resolves earlier than authored. |
| Angle | degree | Yaw 0 = +X, counter-clockwise seen from above. Pitch positive = looking down. Quake conventions, Z up. |
| Speed | u/s | Gravity 800 u/s² (deliberately 2.5 g). |
| Damage | integer points | No fractions on the wire. |

## 3. Entities

- `EntityId`: u32, zone-local.
- Every simulated entity has origin, velocity, view yaw/pitch, a collision hull and party/guild
  tags. Tags are for **UI only**. Damage never reads them (PLAN.md 4.4).
- Combatants add attributes (STR, AGI, CON, INT, SPR), resources (health, stamina, focus), an
  archetype frame, an armour class, one or two aspects, statuses and a kit (abilities and a
  guard). Derived-stat formulas, the type matrix and the point budget live in `MATRIX.md`.
- **Hitbox** = a capsule from the archetype frame table. Never from the mesh (PLAN.md 2.9).
  **World collision** uses the same 32 × 32 × 56 hull for every player archetype in year one; the
  BSP has exactly two hull sizes and a fatter hull would not fit the same doors.

| Frame | Capsule radius | Capsule height |
|---|---|---|
| Colossus | 18 u | 60 u |
| Striker | 14 u | 56 u |
| Caster | 13 u | 56 u |
| Infiltrator | 12 u | 52 u |

## 4. Common parameter types

- **Timing** `{ windup, active, recovery }` in ticks. The verb resolves during `active`.
  `recovery` is the vulnerable window; whether it can be cancelled is an ability property.
- **Cost** `{ stamina, focus }`.
- **Cooldown** `{ ticks, group }`; abilities in the same group share one cooldown.
- **DamagePacket** `{ amount, type, bypass, knockback, stagger }`. `type` is one of the three
  physical kinds (Slash, Pierce, Blunt) or the five elements (Flame, Shadow, Storm, Frost,
  Stone) of MATRIX.md; `bypass` is a set of `ARMOR`, `MAGIC_SHIELD`, `EVASION`: the RPS "true
  bypasses" of PLAN.md 3.2.
- **Shape**: `Sphere{radius}`, `Cylinder{radius, height}`, `Cone{length, half_angle}`,
  `Box{half_extents}`.
- **Origin**: `SelfFeet`, `SelfEyes`, `Weapon{offset}` (forward, right, up in the actor's view
  frame), `Point`, `Aim{range}` (the spot on the floor the actor looks at, section 15; areas
  only), `Impact` (where the triggering projectile hit).
- **Falloff**: `None`, `Linear`, `InverseSquare`.

Friendly fire is not a parameter. It is always on for every verb that deals damage.

## 5. Verbs

### 5.1 MeleeArc

A swing that hits every capsule inside a wedge in front of the attacker.

Parameters: `reach` (u), `arc` (deg), `half_height` (u), `timing`, `damage`, `max_targets`,
`cleave_falloff` (damage multiplier per extra target), `parryable`, `hit_stop` (ticks).

Resolution: on each `active` tick the server rewinds nearby capsules to the attacker's view tick
(bounded to 200 ms, PLAN.md 11.3) and tests the wedge. Each entity is hit at most once per swing.
A hull trace from the attacker's eyes to the target's capsule centre must be clear (no hitting
through walls). If the target is guarding toward the attacker, section 5.6 applies first.

### 5.2 Projectile

Everything ranged: bolts, arrows, thrown axes, fireballs. **No hitscan, no homing** (PLAN.md 4.2).

Parameters: `speed`, `gravity_scale`, `radius`, `lifetime`, `damage`, `pierce`, `bounce
{count, restitution}`, `drag`, `spawn: Origin`, `inherit_velocity`, `spread` (deg), `count`,
`on_hit: [Trigger]`, `on_expire: [Trigger]`. A **Trigger** is an `ApplyStatus` (on the entity
hit, or on the shooter with `target: Actor`) or an `AreaEffect` (anchored at `Impact`). The
lists are typed rather than `[Verb]` so the vocabulary stays non-recursive and wire-encodable;
projectiles that spawn projectiles are deliberately impossible.

Resolution: spawned on the server at the attacker's view tick, never rewound. Every tick the
server sweeps the projectile hull against the world and against every capsule in range,
including the shooter's allies. A hit applies `damage` and then runs `on_hit` with
`Origin::Impact` available (the status lands after the damage, MATRIX.md 7). A world hit runs
`on_hit` at the impact point with no entity; `bounce` reflects off the world instead while
bounces remain; expiry runs `on_expire` at the last position. The shooter's own capsule is
ignored for the first 2 ticks after spawn. The client spawns a predicted copy and reconciles it
by id.

### 5.3 AreaEffect

A volume that pulses.

Parameters: `shape`, `origin`, `delay`, `duration` (0 = one instant pulse), `interval`, `damage`
(optional), `effects: [ApplyStatus]`, `falloff`, `max_targets`, `requires_los`, `exclude_actor`.

Resolution: first pulse at `delay`, then every `interval` until `duration`. Targets are the
capsules overlapping the shape, nearest first up to `max_targets`; with `requires_los`, a trace
from the origin to the capsule centre must also be clear. Damage is scaled by `falloff` from the
centre (`Linear` = 1 − d/r, `InverseSquare` = (1 − d/r)²). `effects` with `target: Area` land on
every target after its damage; `target: Actor` lands on the caster once per pulse. The caster is
a target like anyone else (a fireball at your feet burns you) unless `exclude_actor` is set,
which content uses for shockwaves that originate at the caster's own feet. Areas ignore guards.
The area is sent to clients as an entity (its origin and largest extent) for drawing only.

### 5.4 ApplyStatus

Parameters: `status`, `duration`, `magnitude`, `max_stacks`, `stacking` (`Refresh` |
`Extend` | `Independent`), `target` (`Actor` | `Hit` | `Area`), `dispellable`.

Initial status set: `Slow`, `Haste`, `Root`, `Bleed`, `Burn`, `Chill` (stacks to `Freeze`),
`Shock` (interrupts), `Silence`, `Stagger`, `Fortify`, `Weaken`, `Expose` (grants `ARMOR`
bypass to attackers), `Regen`, `Stealth`. Exact semantics, stacking and immunity windows are
MATRIX.md 8. Statuses act through multipliers on `MoveVars`, resources and damage packets.
`Stealth` reduces the distance at which the entity is included in snapshots; **nothing ever
bypasses PVS culling** (PLAN.md 8).

Targets: `Actor` statuses are predicted on the client (self-buffs resolve in `step_mover` on both
sides). `Hit` means every entity the previous hitting verb of the same ability hit: a step
`ApplyStatus { target: Hit }` after a `MeleeArc` is attached to that swing and lands with each
hit, after the hit's damage; inside a projectile's `on_hit` it lands on the entity hit. `Area`
is only valid inside an `AreaEffect`'s `effects`. An entity holds at most 8 status slots; a ninth
application is refused.

Resolution: statuses tick after all verbs in a tick (section 7); damage and healing over time
pulse four times a second. Durations run in the target's frame ticks, like cooldowns.

### 5.5 MoveSelf

The actor moves itself. Predicted on the client, authoritative on the server.

Parameters: `kind` = `Dash{speed, duration}` | `Leap{forward, up}` | `Charge{speed, duration,
stop_on_hit}` | `Blink{distance}`, `cancelable`, `keep_friction`, `iframes` (default 0).

Resolution: overrides or adds to the actor's movement for the duration. `Dash` follows the
movement wish (or the facing when standing); `Charge` always follows the facing and, with
`stop_on_hit`, ends when the way is blocked by a wall or a body. `Blink` is hull-traced along the
facing on the ground plane and requires line of sight; it never passes through geometry and
leaves the actor with no velocity. Every `MoveSelf` makes the actor **evading** (MATRIX.md 6)
for its duration plus 2 ticks. Invulnerability frames are 0 unless a kit explicitly buys them:
dodging is positional.

### 5.6 Guard: Block and Parry

`Block { arc, mitigation, stamina_per_hit, stops_projectiles, move_speed_scale }`: while held,
attacks arriving from within `arc` of the defender's facing lose `mitigation` of their damage and
cost stamina. Shields stop projectiles; weapons do not.

`Parry { arc, window, whiff_recovery, on_success: [Riposte] }`: an attack resolving inside the
`window` from within `arc` is negated and `on_success` runs on the attacker: a **Riposte** is an
`ApplyStatus` (typically Stagger) or a `MeleeArc` swung by the defender. Missing the window costs
`whiff_recovery`, during which nothing can be activated. Projectiles cannot be parried; only
swings with `parryable` set can. [OPEN: kits that parry projectiles.]

The guard lives in the build's guard slot (MATRIX.md 9) and is held or pressed with the `guard`
button; it is never a step of a script. Blocking from behind does nothing: the attacker (or the
shot's origin) must be within `arc` of the defender's facing. A blocked hit costs
`stamina_per_hit` and pauses stamina regeneration; a hit the defender cannot pay for breaks the
guard (MATRIX.md 7). Attacking releases a held block; a parry window or its whiff recovery
refuses activations.

## 6. Abilities are timed verb scripts

```
Ability {
  id, name, cost, cooldown,
  steps: [ { at: ticks, verb } ],   // relative to activation
  move_scale: 0..1,                  // movement multiplier while the script runs
  interrupt: Never | OnDamage | OnStagger
}
```

Examples (values illustrative, tuning happens in playtests):

| Ability | Script |
|---|---|
| Crossbow shot | at 120 ms: `Projectile{speed 2400, gravity 0.4, radius 1, damage 45 pierce}`; cooldown 1.8 s; move 0.6 |
| Fireball | at 400 ms: `Projectile{speed 900, gravity 0, radius 6, on_hit: [AreaEffect{Sphere 96, damage 30 fire, effects: [ApplyStatus Burn 3 s]}]}`; interrupt OnDamage |
| Greatsword overhead | at 350 ms: `MeleeArc{reach 80, arc 60, active 4 ticks, damage 70 slash, max_targets 3, cleave 0.7}`; recovery 500 ms |
| Shield wall | `Guard::Block{arc 150, mitigation 0.8, stamina 12/hit, stops_projectiles}`; move 0.5 |
| Dash | `MoveSelf::Dash{speed 900, 10 ticks}`; cost 25 stamina |
| Healing circle | `AreaEffect{Cylinder r 128 h 96, duration 6 s, interval 1 s, effects: [ApplyStatus Regen]}` — heals enemies standing in it too |

## 7. Tick order (server)

1. Apply this tick's inputs: statuses expire, guard state, ability activations, script steps
   (`Actor` statuses and `MoveSelf` resolve here on both sides).
2. Movement for every mover (`gm-core::movement`), with `MoveSelf` overrides, players in
   ascending id order blocking each other.
3. `MeleeArc` swings resolve (with lag compensation) and their `Hit` statuses land; then
   `Projectile` spawns (forward-stepped), then `AreaEffect` spawns.
4. Projectiles step and resolve hits (with `on_hit` triggers).
5. Area effects pulse.
6. Statuses tick: damage and healing over time (every 16 ticks), stagger build-up decays.
7. Respawns (a pending respec takes effect here), loot eligibility (contribution ledger, Phase 5),
   contract state machines (Phase 5).
8. Snapshots: PVS filter, distance bands, delta encode (PROTOCOL.md).

## 8. Friendly fire and collision

Damage never checks team, party, guild or alliance (PLAN.md 4.4). Collision: player-player
capsule sweeps always; projectiles collide with every capsule except the shooter's own for the
first 2 ticks. Team-kill statistics feed the reputation ledger. A body that begins a step
inside another body is not held by it for that step (PROTOCOL.md 7.5): overlap is an accident
of spawning or blinking, never a prison.

## 9. Genre compilation

| Viewport | Aim source | Movement | Verbs available |
|---|---|---|---|
| FPS | eye ray from view yaw/pitch | full | all; precision `Projectile` kits shine |
| Third-person | camera ray resolved to a world point, re-aimed from the eyes ("camera-to-muzzle re-aim") | full, 360° awareness | all; `MeleeArc`, `Guard`, `MoveSelf` kits shine |
| Tactical | none (the body is in the command stance, section 15) | none: the officer's body is exposed | issues intents (`Follow`, `Hold`, `MoveTo`, `Attack`) to **its own companions only**; their minds turn intents into ordinary inputs (COMPANIONS.md 5) |

The tactical view never commands humans (PLAN.md 4.3).

## 10. Deliberately absent

No hitscan. No homing. No team check in damage. No default invulnerability frames. No stat
that scales with level (there are no levels). No ability-specific server code.

## 11. Validation ranges

| Parameter | Range |
|---|---|
| `MeleeArc.reach` | 0–160 u |
| `MeleeArc.arc`, `Guard.arc` | 0–360° |
| `Projectile.speed` | 1–4000 u/s (a 200 m shot takes at least 1.6 s at max speed) |
| `Projectile.lifetime` | 1 tick – 20 s |
| `AreaEffect` radius / length | 0–512 u |
| `Dash`/`Charge` speed | 0–1600 u/s |
| `Blink.distance` | 0–384 u |
| `Ability.steps[].at` | 0–10 s |
| `Ability.move_scale`, `Block.mitigation` | 0–1 |
| `ApplyStatus.duration`, `max_stacks`, `Parry.window` | ≥ 1 |

## 12. Open questions

Which kits may parry projectiles. Whether `Stealth` distance stays a status magnitude.
(`Origin::Target` is gone: section 15.)

## 13. Phase 2 implementation notes

`gm-core::sim` implements sections 5.1, 5.2 and 5.5 (`MeleeArc`, `Projectile`, `MoveSelf`
dash/leap) and the tick order of section 7 for players and projectiles. `AreaEffect`,
`ApplyStatus`, `Guard` and `Blink` are parsed and validated but do not resolve yet (Phase 3).

Every player carries the same three-ability kit until point-buy exists (Phase 3). The numbers
are placeholders for netcode work, not tuning:

| Slot | Button | Script | Cooldown | Move scale |
|---|---|---|---|---|
| Sword | primary | at 0: `MeleeArc{reach 72, arc 90°, half_height 40, windup 90 ms, active 45 ms, recovery 160 ms, damage 35 slash, knockback 150, max_targets 3, cleave 0.7}` | 300 ms | 0.6 |
| Crossbow | secondary | at 125 ms: `Projectile{speed 1800, gravity 0.3, radius 2, lifetime 3 s, damage 40 pierce, knockback 100, spread 0.3°, spawn Weapon(16, 4, −2)}` | 1.5 s | 0.7 |
| Dash | ability 1 | at 0: `MoveSelf::Dash{900 u/s, 150 ms}`, 30 stamina | 1 s | 1.0 |

Health 100, stamina 100 regenerating 15/s, respawn 3 s after death at a free spawn point.
(Historical: the Phase 2 numbers. Since MATRIX.md v2, 2026-10-06, the sword is 60 slash at
150/60/300 ms and 600 ms, the crossbow a primary of 80 pierce at 1.8 s, and health 700–1,500;
`gm_core::sim::test_content::phase2_build` keeps the Phase 2 kit shape over the current
numbers.)

Timing rule: script clocks run in the **client's frame ticks** on both sides (the server passes
each frame's tick to `step_mover`, never its own tick), so cooldowns and dash durations elapse
identically even when the server runs two of a client's frames in one tick. The melee active
window is re-anchored to server ticks when the swing is created.

Animation states carried in snapshots (`sim::anim`): idle, run, air, windup, swing, recover,
dash, dead. They are derived on the server and never simulated on the client.

Damage never checks teams (section 8): the crossbow test in `sim.rs` fires through an ally and
hits it.

## 14. Phase 3 implementation notes

`gm-core::sim` now resolves every verb of section 5: `AreaEffect` (server), `ApplyStatus`
(`Actor` on both sides, `Hit`/`Area` on the server), `Guard` block and parry, and every
`MoveSelf` kind including `Blink` and `Charge`. The Phase 2 placeholder kit is gone: every
player runs a validated `Build` (MATRIX.md 9) compiled against the zone's content pack
(`assets/content`, MATRIX.md 10), and the zone sends that pack to every client after `Welcome`.

Animation states carried in snapshots (`sim::anim`): idle, run, air, windup, swing, recover,
dash, dead, guard, parry, cast, stagger. Derived on the server, never simulated on the client.

The vocabulary became non-recursive in v0.3: `Trigger` (projectile hits), `[ApplyStatus]`
(area effects) and `Riposte` (parries) replaced `[Verb]`. Nothing content could express before
is lost; projectile chains were never allowed by the validator.

## 15. Phase 7: drivers, aimed areas, zero packets, the command stance

- **Drivers.** A body's frames come from a client (through the frame ledger, PROTOCOL.md 4)
  or from a mind (`gm-ai`), which hands the zone one `Input` per server tick. A mind's body
  runs exactly one frame a tick in its own frame clock; its swings are not rewound. Nothing
  else in the simulation knows the difference (COMPANIONS.md 2.1).
- **`Origin::Aim { range }`** (1..=1,024 u) replaces `Origin::Target`: the first body or world
  surface along the actor's view ray within `range`, dropped to the ground (under a body:
  its feet; with nothing in the way: the point at `range`). Current positions, no rewind,
  nothing homes: what is placed is a spot on the floor. Only an `AreaEffect` may use it.
- **A packet of amount 0 is not an attack**: no damage, knockback or stagger, not blocked,
  parried or evaded, interrupts nothing; only its triggers land (MATRIX.md 7). The healer's
  dart is such a packet with Regen on whoever it hits, friend or foe.
- **The command stance.** Button bit 11 held, while no script runs and the body is not
  staggered, puts the body in the stance; it lasts while the bit is held and 400 ms after.
  In it the frame is read as view angles only: no movement, no jump, no action, no guard.
  It is stepped in `step_mover` on both sides of the wire, like the guard; the snapshot's
  own-entity flag `commanding` settles a disagreement. Animation state 12, `command`.
- **Creatures** are builds without a budget (`CreatureDef`): attributes may leave 5..=25,
  health is set by content (up to 60,000), and they may slot abilities marked `creature`,
  which no player build can. Everything else is the build rules.
- Animation states: idle, run, air, windup, swing, recover, dash, dead, guard, parry, cast,
  stagger, command.
