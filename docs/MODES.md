# The three modes — v0 (2026-10-07), proposed

Status: a design, nothing built. The director on 2026-10-07: "The Action type (GTA style) /
FPS / 3rd person RPG should be 3 different modes that are not interchangeable during play,
it should be pre selected by character. If character is gun type, you get FPS view, holding
gun, having number of bullets, recoil, spread etc, take as much of mechanics as you can from
Counter-Strike 1.6. GTA style is more like MapleStory 2 or Blade & Soul, where you have
skills and try to combine them and gank. 3rd person RPG should be mouse driven, WASD is ok
as it is in Ether Saga, but focus lock should work with target-actions where you execute an
action and wait for your char to get into range and attack."

What was built instead, and is wrong by this: one control scheme with two cameras, `V`
switching first and third person at any moment (CLIENT.md 4.5, VOCABULARY.md 9). The
camera was treated as a viewport; the director means three **games** on one simulation.

Code follows this document; changes to both go in one commit. The numbers are proposed
(section 9); the director tunes them with the GM hand (GM.md) where it reaches.

## 1. Principles

1. **A mode is the character's, not a key.** It is chosen with the build and shown on the
   archetype at creation (CLIENT.md 4.3). `V`, `--third-person` and the `third_person`
   setting go. A respec at the trainer may change the mode as it changes anything else
   of the build (MATRIX.md 9.1: never in a fight); it takes effect at the `Respawned`.
2. **One simulation, one vocabulary** (VOCABULARY.md 1) still: the verbs, the matrix, the
   bodies, the zone are the same for all three. What a mode adds on the server is listed
   here and nowhere else: a firearm's magazine, recoil and inaccuracy (section 3), the
   chains and the knockdown (4), aim at a named target (5). Everything else is camera,
   control and HUD.
3. **Each mode plays like its root**: Counter-Strike 1.6; MapleStory 2 and Blade & Soul;
   Ether Saga and Tales of Pirates. Where a root and a decision of the plan conflict, the
   decision is written here with its price (bullets, 3.6).
4. **Imbalance is the point** (PLAN.md 4.1). A gun kills at range and by the head; an
   action body cannot be hit while it rolls; an RPG body never misses what it has
   targeted. The counter to each is in the other two and in the ground.

## 2. Which mode

The build carries it, as content (`builds.toml`, `gm-core::build::Build`):

```toml
mode = "gun" | "action" | "rpg"
```

Validated with the build (`BuildError`): `gun` needs a primary with a `firearm` block
(3.2); `action` and `rpg` may not hold one. The client reads the mode from the build it is
given at `Content` and at every `Respawned` and sets its camera, its controls and its HUD
from it; the zone reads it from the same build for the rules of sections 3 and 5.

| | Gun | Action | RPG |
|---|---|---|---|
| Root | Counter-Strike 1.6 | MapleStory 2, Blade & Soul | Ether Saga, Tales of Pirates |
| Camera | first person, the view model (LOOK.md 6.4) | over the shoulder, as today (110 u back) | orbit, far and high (240 u back, 90 up); right drag turns it, wheel zooms |
| The body faces | the view | where it goes; the camera at a swing | where it goes, or its target |
| Aim | the eye ray, through the cone of 3.4 | the camera ray re-aimed from the eyes (today's), plus the magnet of 4.2 | the zone's, at the target (5.3) |
| Movement | WASD, crouch, jump; walk is quiet | WASD, dodge | WASD about the camera, or a click on the ground |
| Attack | hold or tap the mouse; reload | combos and cancels on the hotbar | a target, then actions that wait for range |
| HUD | crosshair that opens, ammo, health | hotbar with cooldowns, combo counter | target frame, hotbar, the ground marker |
| Presets | musketeer (new) | blade, ironclad, shade, captain | frostweaver, mender |

The presets' modes are a proposal; an archetype in `action` could as well be `rpg`. The
tavern's list and the people page say the mode with the frame (`blade: striker in mail,
action`).

## 3. Gun: first person, Counter-Strike 1.6

### 3.1 What is kept from the root

Magazine and reserve; a reload that takes its time; a rate of fire per weapon; a recoil
pattern that is the weapon's and can be learned; a cone that opens with movement, with the
air and with sustained fire and closes when standing still or crouched; a tap, a burst, a
spray as three different decisions; a headshot; the knife as the fast and silent fallback;
a weapon that slows the body by its weight; a scope that narrows the view; no iron sights,
no sprint, no leaning, no regeneration.

### 3.2 The firearm, as content

A `firearm` block on a primary whose steps launch a bolt (`Verb::Projectile`); validated
with the ability (VOCABULARY.md 11):

```toml
[[ability]]
key = "musket"
slot = "primary"
prop = "musket"
firearm = { magazine = 1, reserve = 24, reload_ms = 2800, cycle_ms = 1200, fire = "bolt",
            headshot = 4.0, scope = 0,
            recoil = [[0.0, 2.4]],
            cone = { stand = 0.6, crouch = 0.3, move = 3.0, air = 8.0, shot = 1.2, recover_ms = 400 } }
[[ability.step]]
at_ms = 0
projectile = { speed = 20000, gravity = 0.02, ... spread = 0 }
```

| field | means | range |
|---|---|---|
| `magazine`, `reserve` | rounds in the weapon and carried; the reserve refills at a respawn and at an ammo pickup (ITEMS.md: a stack of rounds is an item) | 1–100, 0–400 |
| `reload_ms` | the reload, a script of its own: `R` or an empty magazine with the trigger held; a stagger interrupts it and the rounds are not lost; a switch of weapon cancels it | 500–6,000 |
| `cycle_ms` | the time between two shots; replaces the primary's cooldown | 50–3,000 |
| `fire` | `auto` (held), `semi` (a click a shot), `bolt` (a click a shot and the body works the action for the cycle; no shot while moving faster than a walk) | |
| `headshot` | the multiplier for a hit in the head band (3.5) | 1–5 |
| `scope` | 0 none; else the zoom (2 = FOV 45, 4 = FOV 20): the secondary mouse button toggles it, the view model is not drawn and the body walks at `move_scale` while scoped | 0, 2, 4 |
| `recoil` | the pattern: the view's kick after the n-th shot of a spray as (yaw, pitch) in degrees; past the end it repeats the last; the index resets after `2 × recover_ms` without a shot | up to 32 pairs, each within ±6° |
| `cone` | inaccuracy in degrees (3.4) | each 0–15; `recover_ms` 100–2,000 |

### 3.3 Recoil

The pattern is applied twice, to the same effect: the **client** kicks the own view by the
pair (a punch angle that decays over 150 ms, CS's `v_punchangle`), and the **zone** turns
the bolt's direction by the same pair before it rolls the cone. The player who pulls the
mouse against the pattern lands the spray; the pattern is in the content, so it can be
learned from the file as it was learned from the game. A recoil is not spread: it is
deterministic and shared, and it is what makes the gun a skill.

### 3.4 The cone

The bolt's direction, after the recoil, is rolled uniformly inside a cone of

```
stand  +  move × (speed / max speed)  +  air (while airborne)  +  shot × (shots in the last recover_ms)
```

with `crouch` in place of `stand` while crouched. Today's `spread_deg` on a projectile
(zone.rs, the uniform roll in a cone) is this with a constant cone; a firearm's bolt sets
`spread = 0` and takes the cone from here. The client draws the cone as the crosshair that
opens: four lines whose gap is the cone at 1,024 units. The zone's roll is the only roll;
the client predicts the kick and the opening, never the shot.

### 3.5 The head

The hull is a cylinder (MATRIX.md 3); CS has hit groups, we have none. **The head band is the
top 12 units of the hull** (a sixth of a striker). A bolt that enters the hull within the band
is a headshot: the packet is multiplied by `headshot` **before** armour (MATRIX.md 7 gets a
step: hit zone). Melee arcs and areas have no head. Crouching lowers the hull, so the band
moves with it: a crouched body is the harder headshot, as in the root.

### 3.6 Bullets are bolts, fast

PLAN.md 4.2 decided projectiles, never hitscan, for the aimbot's sake. A bullet is a bolt
at **20,000 u/s**: across the longest sight line in the arena (about 2,000 u) in a tenth of
a second, a lead of a body's width on a runner at full speed. It is swept per tick as every
bolt is, with the forward step against where targets were (PROTOCOL.md 7.4), so the shot
lands where the shooter saw the body, minus that tenth. If the director finds it does not
feel like the root, hitscan for `firearm`s is a day's work on the same rewind: the forward
step collapsed to one sweep, the aim statistics of ANTICHEAT.md 4 unchanged. Decided by
playing it, not here.

### 3.7 Weapons in hand

Three on the keys of the root: `1` the primary (the gun), `2` the secondary (a pistol, a
firearm with a `cone` of its own, or a thrown knife), `3` the knife (a dagger as every body
has a fist: `melee_arc` at full speed, silent). A gun slows the body by its `move_scale`
while in hand, the knife not at all. The guard slot is empty in this mode (a `gun` build
may not buy one: no parry with a musket). Actives stay (a dash, a vanish); `Shift` walks
(half speed, no footsteps, SOUND.md), not guards.

### 3.8 On the wire and in the HUD

The own body's snapshot carries `magazine: u8, reserve: u16, reloading: bool` (the stranger
sees the stance only). The HUD: bottom right the magazine over the reserve, in the ammo
glyph's colour when below a magazine; bottom left health and stamina; the crosshair of 3.4;
no hotbar, the three weapons as a strip that lights the one in hand; the hit marker and
the numbers of LOOK.md 13.8 as now. Sound: a shot is loud (the reach of the thunderclap),
the reload and the empty click audible at 400 u.

## 4. Action: third person, combos (MapleStory 2, Blade & Soul)

### 4.1 What is kept from the root

Free aim from behind the shoulder; skills with stages that chain into each other when the
key is pressed again in time; a dodge that cannot be hit; animation cancels that are
themselves a skill; knockdowns and launches that open a body to a follow-up; a party that
chains its controls on one body — the gank — and diminishing returns so a chain ends.

### 4.2 The magnet

A melee arc with `assist = 30` turns the body, at the swing's start, up to 30° toward the
nearest living enemy body within `reach × 1.5` that it can see; the zone does it and the
client predicts it, both from the same bodies. Bolts and areas are not assisted. Today's
camera-to-muzzle re-aim stays for everything else.

### 4.3 Chains

```toml
[[ability]]
key = "sword"
chain = { next = "sword_2", window_ms = 400 }
```

While `sword`'s recovery runs and for `window_ms` after it, the same slot plays `sword_2`
instead, a full ability with its own numbers and its own `chain`; the third of a chain
usually knocks down. A chain's members cost nothing in the kit: they come with the first.
The hotbar's cell shows the stage (I, II, III) while the window is open; the combo counter
of 4.6 counts hits in one chain.

### 4.4 Cancels and the dodge

An ability may name what may cut it: `cancel = "recovery"` on a `MoveSelf` (the dash)
means a dash pressed during another ability's recovery ends that recovery now. The dash
with `evades = 150` cannot be hit for its first 150 ms: bolts, arcs and areas pass. It
costs stamina as now, so a body dodges twice, then takes the hit. This is the counter to
the gun: a roll through the shot.

### 4.5 Down, up and the chain's end

Two statuses (MATRIX.md 8, VOCABULARY.md 5.4): `Knockdown(ms)` (the body is on the ground,
cannot act or guard, takes hits in full, rises when the time is out) and `Launched(ms)`
(in the air, same, and falls into a `Knockdown` of half the time). **Diminishing
returns**: the second control of the same kind on one body within 10 s lasts half, the
third does nothing and the body is immune to that kind for 10 s. The party frame shows the
mark on the target so a gank is timed, not spammed.

### 4.6 Controls and HUD

As today: the primary on the left button, the secondary on the right, the guard on `Shift`,
the actives on `1`–`4`, the dodge on `Space` when the build has a dash (jump is the dash
without one). The HUD keeps the hotbar with its cooldowns and gets a combo counter at the
right of the crosshair (hits in the current chain, fading two seconds after the last).

## 5. RPG: third person, mouse driven, target-actions (Ether Saga, Tales of Pirates)

### 5.1 What is kept from the root

The camera orbits and the body does not turn with it; WASD moves about the camera; a
click on the ground sends the body there; a click on a body targets it and `Tab` cycles;
an action with a target walks the body into range and then lands, every time; the primary
repeats on the target until it falls; a guard is held.

### 5.2 Target

A left click on a body, `Tab` the nearest visible enemy not yet cycled, `Esc` clears. The
target frame on the HUD (name, build's frame, health as a bar: a targeted stranger's
health goes on the wire to the one who targets it, as a creature's does for everybody,
COMPANIONS.md 8.1). A target is lost when it dies, leaves the zone or has not been seen
for 5 s.

### 5.3 Target-actions

An action pressed with a target:

- **in range and seen**: it fires, aimed by the zone at the target. A `melee_arc` turns the
  body to face it; a `projectile` is launched with the mind's lead (`gm_ai::fighter::lead`,
  the companions' own) and the content's spread; an aimed `area_effect` is put on the
  target. The frame's yaw and pitch stay the camera's for the view: **the aim of an RPG body
  is never the client's** (ANTICHEAT.md 4's statistics skip it: there is nothing to measure);
- **out of range or unseen**: the body turns and walks toward the target along the nav grid
  (COMPANIONS.md 7, built on the client from the same map in 14 ms) until it is in range,
  then fires. One action waits; a new one replaces it; a movement key or a ground click
  cancels it. The walk is made of ordinary frames the client produces, so prediction and
  the ledger are untouched;
- **the primary**: pressed once, it repeats every cooldown on the target until the target is
  lost, the player moves, or presses it again. The root's double click.

Range is the ability's: a melee arc's `reach`, a bolt's `range` (new, content: the musket
600, the crossbow 900, a knife 300), an area's `origin: Aim` radius. Without a target an
action fires where the body faces, as in the action mode without the magnet.

### 5.4 On the wire

`InputFrame` grows for every mode (bits are cheap in the ledger, PROTOCOL.md 4): `target:
u32` (0 none) and `order: u8` (none, the ability in `ability` at the target, move to the
point). A ground click is a `MoveTo` the client resolves to frames; it is not sent. The
zone validates: a target must be a body the sender is being sent, the range the content's,
the sight the zone's trace.

### 5.5 Controls and HUD

Left button: target or move; right drag: the camera; wheel: 120–400 u; `Tab`, `Esc`; the
hotbar on `1`–`8` (the primary and secondary join it: the root's bar is one bar); `Shift`
guards; `Space` jumps. A ground marker where the body is going; the target frame top
centre; a ring under the target (LOOK.md 13).

## 6. What goes

`Viewport` in the client (`app.rs`) becomes `Mode`, read from the build. `V`, `buttons::
VIEWPORT`, `--third-person`, `third_person` in the settings and the page's `third-person=1`
flag (WEB.md 5) are removed. VOCABULARY.md 9's table is rewritten to this document's; the
command stance (COMPANIONS.md 5) keeps its key in every mode.

## 7. Phase plan (proposed: Phase 15 in three parts, the editor to 16, the body's look to 17)

| part | builds | played when |
|---|---|---|
| 15a Action | `mode` in the build and the mode fixed on the client; the toggle gone; the magnet, chains, cancels, the dodge, knockdown and launch with diminishing returns; two chains in content (sword, dagger); the combo counter | the director chains a sword three times into a knockdown, rolls through a firebolt |
| 15b Gun | the firearm block, magazine and reload, cycle, recoil and the cone, the head band, the three weapons, the ammo HUD; the musketeer preset; a pistol and a second long gun in content | a spray controlled against the pattern lands; a crouched tap at 1,500 u lands a headshot; the empty click |
| 15c RPG | the target, the target frame, target-actions with the walk, the repeat, the ground click on the nav grid, the orbit camera; `range` in content; the aim statistics skipping RPG bodies | a frostweaver clicks a dummy, presses the shard, walks into range and lands it; `Tab` across three spar bots |

Where CONTENT.md, LOOK.md and PROTOCOL.md say "Phase 15" they mean the editor (now 16) and
by "Phase 16" the body's look (now 17); they are not rewritten.

Action first because it is today's play made a mode and the smallest step; the gun second
because it is the most new code; the RPG last because it changes the wire. The order is the
director's.

## 8. Deliberately absent (v1)

Wall penetration, a buy menu, iron sights, sprint, leaning, a hard lock in the action mode,
a body walking round other bodies, a mode changed mid-fight, a fourth mode.

## 9. Proposed numbers and open decisions

Every number above. Open for the director: **the presets' modes** (2); **hitscan or a bolt
at 20,000 u/s** (3.6); **the head band** at 12 units and ×4; **whether an RPG body's aim
is the zone's** (5.3: it makes the RPG body the one that never misses and never cheats,
and the gun body the one that can do both); **whether a mode change is a respec** (1); the
**diminishing returns** of 4.5 and whether creatures are under them; **what a body does
when its target walks out of sight** (waits in place, as proposed).

## 10. As built

### 10.1 The action mode (15a, 2026-10-07)

Built as sections 2 and 4 say, with these readings:

- **The mode is in the build** (`Build.mode`, `builds.toml` `mode`), validated with it
  (`BuildError::GunNeedsFirearm`, `FirearmNeedsGun`, `GunHasNoGuard`, `NotSlottable`);
  a build stored before it is read as `action`. The camera follows the mode of the sheet
  the zone sent (`Content`, `BuildApplied`); `V` is a key offline and in a replay only,
  `--third-person` likewise, the `third_person` setting is read and dropped.
- **Chains** are `chain = { next, window_ms }` on an ability and the stages are
  `slot = "extra"` abilities that come with it into the kit (`Kit::chain_next`; the
  kit may hold twelve now, `MAX_ABILITIES`). The pressed slot plays the next stage from
  the moment the running stage is past its last active window (`script_commit`) until
  `window` after it ends; the hotbar's cell shows the stage (II, III). The sword and the
  dagger chain three deep; the third puts the body down (`sword_3`: Knockdown 1.2 s;
  `dagger_3`: Launched 300 u/s up for 0.9 s). The second cut's knockback is 60, not the
  first's 150: a chain's early blows must not carry the body out of the third's reach.
- **The magnet** is `assist` on a melee arc (30° on the chains' arcs): the nearest enemy
  within reach and a half and in sight, with the line of sight traced to twenty units
  short of the body's centre, since the mover's world holds the other bodies as solids.
  The zone reads friend and foe by team and party; the client, which is not told
  parties, reads everyone on another team as an enemy and, in the wild, everyone: a
  swing predicted toward an ally there is corrected by the zone's reading.
- **The dodge** is the dash's `iframes_ms = 150` (already in the vocabulary) with
  `cancel = "recovery"`; Space plays the kit's dash while it is ready, else jumps.
- **Knockdown and Launched** are statuses 14 and 15: no action, guard or movement; the
  `DOWN` stance; diminishing returns per kind (`Player::controls`, ten seconds), creatures
  under them too. A crouch has no hull of its own yet, so the head band of 3.5 does not
  move with it.
- **The combo counter**: the own blows within two seconds of each other, right of the aim.
- The input frame carries `held` and `target` for every mode (PROTOCOL.md 23), so the
  wire changes once for the three parts.
