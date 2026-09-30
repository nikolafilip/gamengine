# Entity Vocabulary

Status: v0.1, Phase 0. This document is the contract for phases 2–7. `gm-core::vocab` mirrors it
as plain data types; when the two disagree, this document wins and the code is wrong. Changes to
either require changing both in the same commit.

## 1. Principles

1. One simulation, one vocabulary. Genres (FPS, third-person, tactical) are camera, control
   scheme and HUD. They never add server rules.
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
  archetype frame, statuses and a kit (abilities and counters). Derived-stat formulas and the type
  matrix live in `MATRIX.md` (Phase 3).
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
- **DamagePacket** `{ amount, type, bypass, knockback, stagger }`. `bypass` is a set of
  `ARMOR`, `MAGIC_SHIELD`, `EVASION`: the RPS "true bypasses" of PLAN.md 3.2.
- **Shape**: `Sphere{radius}`, `Cylinder{radius, height}`, `Cone{length, half_angle}`,
  `Box{half_extents}`.
- **Origin**: `SelfFeet`, `SelfEyes`, `Weapon{offset}` (forward, right, up in the actor's view
  frame), `Point`, `Target(entity)`, `Impact` (where the triggering projectile hit).
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
`on_hit: [Verb]`, `on_expire: [Verb]`.

Resolution: spawned on the server at the attacker's view tick, never rewound. Every tick the
server sweeps the projectile hull against the world and against every capsule in range,
including the shooter's allies. A hit applies `damage` and runs `on_hit` with `Origin::Impact`
available. The shooter's own capsule is ignored for the first 2 ticks after spawn. The client
spawns a predicted copy and reconciles it by id.

### 5.3 AreaEffect

A volume that pulses.

Parameters: `shape`, `origin`, `delay`, `duration` (0 = one instant pulse), `interval`, `damage`
(optional), `effects: [Verb]`, `falloff`, `max_targets`, `requires_los`.

Resolution: first pulse at `delay`, then every `interval` until `duration`. Targets are the
capsules overlapping the shape; with `requires_los`, a trace from the origin to the capsule
centre must also be clear. Damage is scaled by `falloff` from the centre.

### 5.4 ApplyStatus

Parameters: `status`, `duration`, `magnitude`, `max_stacks`, `stacking` (`Refresh` |
`Extend` | `Independent`), `target` (`Actor` | `Hit` | `Area`), `dispellable`.

Initial status set: `Slow`, `Haste`, `Root`, `Bleed`, `Burn`, `Chill` (stacks to `Freeze`),
`Shock` (interrupts), `Silence`, `Stagger`, `Fortify`, `Weaken`, `Expose` (grants `ARMOR`
bypass to attackers), `Regen`, `Stealth`. Statuses act through multipliers on `MoveVars`,
resources and damage packets. `Stealth` reduces the distance at which the entity is included in
snapshots; **nothing ever bypasses PVS culling** (PLAN.md 8).

Resolution: statuses tick after all verbs in a tick (section 7).

### 5.5 MoveSelf

The actor moves itself. Predicted on the client, authoritative on the server.

Parameters: `kind` = `Dash{speed, duration}` | `Leap{forward, up}` | `Charge{speed, duration,
stop_on_hit}` | `Blink{distance}`, `cancelable`, `keep_friction`, `iframes` (default 0).

Resolution: overrides or adds to the actor's movement for the duration. `Blink` is hull-traced
and requires line of sight; it never passes through geometry. Invulnerability frames are 0
unless a kit explicitly buys them: dodging is positional.

### 5.6 Guard: Block and Parry

`Block { arc, mitigation, stamina_per_hit, stops_projectiles, move_speed_scale }`: while held,
attacks arriving from within `arc` of the defender's facing lose `mitigation` of their damage and
cost stamina. Shields stop projectiles; weapons do not.

`Parry { arc, window, whiff_recovery, on_success: [Verb] }`: an attack resolving inside the
`window` from within `arc` is negated and `on_success` runs (typically `ApplyStatus Stagger` on
the attacker and a riposte `MeleeArc`). Missing the window costs `whiff_recovery`. Projectiles
cannot be parried by default. [OPEN: kits that can.]

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

1. Apply this tick's inputs: movement, ability activations, guard state.
2. Movement for every mover (`gm-core::movement`), with `MoveSelf` overrides.
3. Ability scripts advance. Verbs whose `at` is reached resolve: `MeleeArc` (with lag
   compensation), `Projectile` spawns, `AreaEffect` scheduling, `ApplyStatus`.
4. Projectiles step and resolve hits.
5. Area effects pulse.
6. Statuses tick: damage over time, expiry, stack decay.
7. Deaths, loot eligibility (contribution ledger), contract state machines.
8. Snapshots: PVS filter, distance bands, delta encode (PROTOCOL.md).

## 8. Friendly fire and collision

Damage never checks team, party, guild or alliance (PLAN.md 4.4). Collision: player-player
capsule sweeps always; projectiles collide with every capsule except the shooter's own for the
first 2 ticks. Team-kill statistics feed the reputation ledger.

## 9. Genre compilation

| Viewport | Aim source | Movement | Verbs available |
|---|---|---|---|
| FPS | eye ray from view yaw/pitch | full | all; precision `Projectile` kits shine |
| Third-person | camera ray resolved to a world point, re-aimed from the eyes ("camera-to-muzzle re-aim") | full, 360° awareness | all; `MeleeArc`, `Guard`, `MoveSelf` kits shine |
| Tactical | none (body stays at the war table or leadership stance) | officer body exposed | issues intents (`MoveTo`, `Attack`, `Hold`) to **hired AI only**; the AI turns intents into ordinary inputs |

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

Final status list and stack rules. Which kits may parry projectiles. Whether `Stealth`
distance is a status magnitude or a kit constant. The damage-type list (`MATRIX.md`).
