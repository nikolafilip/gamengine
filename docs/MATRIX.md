# Character Matrix

Status: v2, 2026-10-06 (v1 was Phase 3). This document is the contract for point-buy
characters, the type matrix and the damage pipeline. v2 (section 13): the character's own
thirty attribute points apart from the kit's budget, wider bands and seven times the health,
the range as the weapon's, the trainer and the dummy. `gm-core::matrix` and `gm-core::build` mirror it; when they disagree,
the document wins and the code is wrong. Changes to either go in one commit.

PLAN.md 3.1, 3.2 and 0 are binding: no levels, no gear tiers, a fixed budget, rock-paper-scissors
counters with stacking multipliers and a dual-type 4×, compressed stat bands, top gear a 15–25%
edge (the `gear` term of section 7, built in Phase 11: ITEMS.md 3). PLAN.md 12 listed "full type matrix and attribute set" as open; this document is
the proposal that Phase 3 implements. The director's calls are recorded in section 12.

## 1. Principles

1. **Counters move damage more than stats do.** A derived stat spans at most 2× between
   the worst and the best attribute spread (health 700 → 1,500, damage ×0.8 → ×1.6); the
   matrix then moves damage by 4× either way. Builds are won by picking the right counter
   and landing it; the thirty points say what kind of fighter lands it. (v1 compressed the
   bands to 1.5× and 95–140 health: a fight was over in a second and a point bought nothing;
   section 13.)
2. **Readable in three seconds.** Frame = silhouette, armour class = how the body moves,
   aspect = aura colour. Everything that changes the matrix is visible.
3. **Two budgets, reallocatable.** A character has thirty attribute points of its own and a
   kit worth at most 40. Both are redone, free, at the trainer in the town (section 9.1); in
   the arena at the next respawn.
4. **No immunity.** Every multiplier is 0.25–4. Everything can be killed by anything, slower.
5. **No RNG in damage.** Spread on projectiles is the only randomness in combat.

## 2. Attributes

Five attributes, each **5..=25**, integer. The floor is 5; a character has **thirty points**
to put above it, however it likes (all in one, or spread; fewer may be spent, more is
refused). They are not the kit's budget (section 9): a build does not trade a stat for an
ability. The presets spend all thirty.

| Attribute | Governs |
|---|---|
| STR | physical damage multiplier, knockback dealt |
| AGI | move speed, stamina pool and regeneration, evasion |
| CON | health, stamina pool, physical mitigation (armour), stagger threshold |
| INT | elemental damage multiplier, focus pool |
| SPR | elemental mitigation (ward), focus regeneration, status duration taken |

## 3. Frames

The archetype frame is free: its price is the hitbox and the mobility it fixes
(VOCABULARY.md 3). World collision is the 32 × 32 × 56 hull for all four.

| Frame | Hitbox (r × h) | Mass | Mobility | Armour | Ward | Evasion | Focus |
|---|---|---|---|---|---|---|---|
| Colossus | 18 × 60 | 1.5 | 0.92 | +0.10 | 0 | 0 | 0 |
| Striker | 14 × 56 | 1.0 | 1.00 | +0.05 | 0 | 0 | 0 |
| Caster | 13 × 56 | 0.9 | 0.96 | 0 | +0.10 | 0 | +20 |
| Infiltrator | 12 × 52 | 0.8 | 1.06 | 0 | 0 | +0.05 | 0 |

Mass divides knockback taken. Mobility multiplies move speed.

## 4. Armour class

A build choice. (It was to come from the worn gear, with the build points refunded. Phase 11
gave gear its edge and left the class with the build: ITEMS.md 9 says what moving it would
cost.)

| Class | Cost | Speed | Stamina regen | Bonus |
|---|---|---|---|---|
| Cloth | 0 | 1.00 | 1.00 | ward +0.05 |
| Leather | 4 | 1.00 | 1.00 | evasion +0.05 |
| Mail | 8 | 0.95 | 0.90 | — |
| Plate | 12 | 0.90 | 0.75 | — |

### 4.1 Physical kind × armour class

Physical damage comes in three kinds. The multiplier is applied to every physical packet.

| Kind \ Class | Cloth | Leather | Mail | Plate |
|---|---|---|---|---|
| Slash | 1.25 | 1.00 | 0.75 | 0.50 |
| Pierce | 1.00 | 1.00 | 1.00 | 0.75 |
| Blunt | 0.75 | 0.75 | 1.00 | 1.25 |

Blades shred cloth and bounce off plate; hammers crush plate and bruise cloth; bolts are the
honest middle. The table is the "weapon triangle" half of the matrix.

## 5. Elements and aspects

Five elements. A build has one or two **aspects** (the first is free, the second costs 10
points). Aspects are the build's defensive element types **and** gate which elemental
abilities it may slot (an ability tagged Flame needs the Flame aspect).

The matrix is a pentagram: every element beats exactly two and loses to exactly two, and
resists itself. Order **Flame, Shadow, Storm, Frost, Stone**: element *i* beats *i + 1* and
*i + 3* (mod 5).

| Attack \ Aspect | Flame | Shadow | Storm | Frost | Stone |
|---|---|---|---|---|---|
| Flame | 0.5 | **2** | 0.5 | **2** | 0.5 |
| Shadow | 0.5 | 0.5 | **2** | 0.5 | **2** |
| Storm | **2** | 0.5 | 0.5 | **2** | 0.5 |
| Frost | 0.5 | **2** | 0.5 | 0.5 | **2** |
| Stone | **2** | 0.5 | **2** | 0.5 | 0.5 |

Readings: light burns away shadow and melts ice; darkness swallows the flash and seeps into
stone; wind snuffs fire and lightning shatters ice; cold stills the dark and splits rock;
earth smothers fire and grounds the storm.

**Stacking.** Against a dual-aspect defender the two multipliers multiply: Flame into
Shadow + Frost is **4×**; Flame into Storm + Stone is **0.25×**. In a balanced five-element
matrix exactly five of the ten pairs can share a predator, so the pairs come in two shapes:
- **two apart** in the pentagram order (Flame + Storm, Shadow + Frost, Storm + Stone,
  Frost + Flame, Stone + Shadow): one **4× hole**, two 0.25× walls, two neutral;
- **adjacent** (Flame + Shadow, Shadow + Storm, Storm + Frost, Frost + Stone, Stone + Flame):
  no hole, one 0.25× wall, four neutral.

Spiky or safe is the build choice; both cost 10 points, both still eat the physical table.

Physical packets ignore this table; elemental packets ignore the armour-class table.

## 6. Derived stats

`attr` is the attribute value (5..=20). All results are rounded to integers where they are
pools; multipliers stay f32. Bands are deliberately narrow.

| Stat | Formula | Range (5 → 25) |
|---|---|---|
| Health | `500 + 40·CON` | 700 → 1,500 |
| Stamina | `60 + 3·(CON + AGI)` | 90 → 210 |
| Stamina regen /s | `(10 + 0.6·AGI) · armour.regen` | 13 → 25 |
| Focus | `40 + 5·INT + frame.focus` | 65 → 165 (+20 Caster) |
| Focus regen /s | `5 + 0.6·SPR` | 8 → 20 |
| Move speed (u/s) | `(280 + 2·AGI) · frame.mobility · armour.speed` | 290 → 330 before frame/armour |
| Physical damage × | `0.60 + 0.04·STR` | 0.80 → 1.60 |
| Elemental damage × | `0.60 + 0.04·INT` | 0.80 → 1.60 |
| Armour (physical mitigation) | `min(0.50, 0.015·CON + frame.armour)` | 0.075 → 0.375 |
| Ward (elemental mitigation) | `min(0.60, 0.02·SPR + frame.ward + armour.ward)` | 0.10 → 0.50 |
| Evasion | `min(0.50, 0.015·AGI + frame.evasion + armour.evasion)` | 0.075 → 0.375 |
| Knockback taken × | `1 / frame.mass` | |
| Knockback dealt × | `0.60 + 0.04·STR` | |
| Status duration taken × | `1.30 − 0.03·SPR` | 1.15 → 0.55 |
| Stagger threshold | `40 + 3·CON` | 55 → 115 |

Max speed feeds `MoveVars.max_speed`; jump velocity and acceleration are not attribute-scaled
(movement skill is movement skill). Stamina regen pauses for 1 s after any stamina spend.

Evasion is positional and readable: it applies only while the defender is **evading**, i.e.
inside a `MoveSelf` (dash, leap, charge, blink) or for 2 ticks after one ends. Running or
strafing is not evading; a standing target has no evasion. Packets with `bypass EVASION`
("tracking") ignore it.

## 7. The damage pipeline

For a `DamagePacket` from attacker A landing on defender D:

```
base   = amount
       × (A.physical_mult  if kind is physical  else A.elemental_mult)
       × (A.Weaken ? 1 − weaken : 1)
type   = kind_table[kind][D.armour_class]        physical
       | Π element_matrix[element][aspect]        elemental, over D's 1–2 aspects
layer  = physical  && !bypass ARMOR        ? 1 − D.armour            : 1
       × elemental && !bypass MAGIC_SHIELD ? 1 − D.ward              : 1
       × D.Fortify && !bypass MAGIC_SHIELD ? 1 − fortify             : 1
       × D.evading && !bypass EVASION      ? 1 − D.evasion           : 1
guard  = D guarding toward A && (melee || stops_projectiles) ? 1 − block.mitigation : 1
gear   = (2000 + A.dealt[t]) / (2000 + D.taken[t])              ITEMS.md 3.1: the edge of
         A's worn weapon and of D's worn armour on the packet's type, per mille, at most 250
         each; 1 for the pulses of a status, and for two bodies in nothing
zone   = the bolt is a firearm's and entered the hull within its top 12 u ? headshot : 1
         (the head band, MODES.md 3.5: before armour, on the amount)
damage = max(1, round(base × type × layer × guard × gear × zone))
```

`Expose` on the defender grants every attacker `bypass ARMOR`. A parry inside its window
negates the packet entirely (the only zero) and runs the parry's `on_success` on the attacker.

**A packet whose `amount` is 0 is not an attack** (Phase 7): it never enters this pipeline.
It deals nothing (not the minimum of 1), knocks nobody back, builds no stagger, is not
blocked, parried or evaded and interrupts nothing; only its triggers land. It is how a
projectile carries a boon: the healer's dart is a zero packet with Regen on whoever it hits.

The three true bypasses of PLAN.md 3.2 are structural, not special-cased:
- **magic through armour**: elemental packets never consult `D.armour` or the kind table;
- **blunt through magic shields**: hammers are content with `bypass MAGIC_SHIELD`, so they
  ignore `Fortify` bubbles and the ward (but Blunt vs Cloth is 0.75, so they are not free);
- **tracking through evasion**: wide arcs and seeking-free tracking shots carry
  `bypass EVASION`.

Stagger: every packet adds its `stagger` value to D's build-up, which decays 20 points per
second. Guard mitigation does not reduce build-up (a shield wall is broken by weight of
blows, not by damage). Crossing the threshold applies `Stagger` (section 8) for 400 ms,
resets the build-up and starts a **1 s stagger immunity** (build-up is discarded) so no cadence
can chain-lock. Knockback is applied after damage, scaled by `knockback_dealt × 1/mass`.

Guard break: when a blocked hit costs more stamina than the defender has, the guard breaks:
the hit applies without the block factor, the stamina goes to 0, the guard is released and
the defender is Staggered (immunity rules apply).

Order within a tick: statuses from `ApplyStatus` steps (VOCABULARY.md 7 step 3) land before
projectiles (step 4) and areas (step 5) resolve; a status attached to a hit (`on_hit`, or a
`Hit`-targeted step) lands after that hit's damage, so it benefits the next packet, not the one
that carried it.

## 8. Statuses

The initial set of VOCABULARY.md 5.4 with resolved semantics. `m` is the magnitude from the
`ApplyStatus` verb; durations are multiplied by the defender's status duration factor
(section 6), except Stagger and Shock. Statuses tick after all verbs (VOCABULARY.md 7).

| Status | Effect | Stacking default |
|---|---|---|
| Slow | move speed × (1 − m) | Refresh |
| Haste | move speed × (1 + m) | Refresh |
| Root | move speed 0; turning, abilities and guard still work | Refresh |
| Bleed | m physical damage per second (as Pierce, bypass ARMOR), 4 pulses/s | Independent, max 5 |
| Burn | m Flame damage per second through the matrix and ward, 4 pulses/s | Refresh |
| Chill | move speed × (1 − 0.15 per stack); at `max_stacks` it becomes Freeze: Root for 1 s, all stacks clear and the target is immune to Chill for 2 s | Extend, max 3 |
| Shock | interrupts the running script and guard; lasts 1 tick | — |
| Silence | elemental abilities cannot be activated | Refresh |
| Stagger | interrupts; no activation or guard; move speed × 0.3 | Refresh |
| Fortify | damage taken × (1 − m) unless bypass MAGIC_SHIELD | Refresh |
| Weaken | damage dealt × (1 − m) | Refresh |
| Expose | attackers gain bypass ARMOR | Refresh |
| Regen | heals m health per second, 4 pulses/s; heals enemies too | Refresh |
| Stealth | included in snapshots only within `m` units (never past PVS) | Refresh |
| Knockdown | on the ground (MODES.md 4.5): interrupts; no activation, guard or movement; hits land in full | Refresh |
| Launched | as Knockdown, with a lift of `m` u/s straight up when it lands | Refresh |

**Diminishing returns on controls** (MODES.md 4.5, `gm_core::sim::CONTROL_WINDOW_MS`):
Knockdown, Launched and an explicit Root are controls. The second of a kind within ten
seconds of the last lasts half, the third does nothing; ten seconds after the last, the
count is forgotten. Freeze's Root (Chill at its stacks) is not under them: Chill has its own
immunity.

Dispellable statuses are removed by `ApplyStatus { status: Cleanse }`-style content later;
Phase 3 content has no dispel. A status with `max_stacks` 1 and `Refresh` restarts its
duration on reapplication; `Extend` adds the duration (capped at 3× the verb's duration);
`Independent` tracks each application separately.

## 9. Builds and the budget

```
Build {
  frame:        Colossus | Striker | Caster | Infiltrator       free
  attributes:   STR AGI CON INT SPR, each 5..=25                 Σ(attr − 5) ≤ 30, the character's own
  armour:       Cloth | Leather | Mail | Plate                   0 / 4 / 8 / 12
  aspects:      1..=2 of Flame Shadow Storm Frost Stone          0 / 10
  kit:          primary, secondary, guard (optional), up to 4 actives   Σ ability cost
}
armour + aspects + kit ≤ 40 (the kit's budget); unspent is allowed.
```

Rules enforced by `gm-core::build::validate`:
- every slot holds a distinct ability of the matching slot type (primary, secondary, guard,
  active); an ability's `aspect` requirement must be in the build's aspects;
- at most one ability per cooldown group;
- the attribute floor and cap, the thirty points, the kit's budget, no duplicate aspects.

A rejected build never enters the zone. A stored build the rules no longer take (the v1
presets had 56–68 attribute points, and a bolt in the secondary slot) is **repaired** by the
hub when the character enters (`Build::repaired`): the points are scaled down to thirty in
their own proportion, and a kit the pack refuses becomes the preset's with the same primary
(else the first); what cannot be repaired is refused in words.

### 9.1 The trainer, the dummy and the arena

`FromClient::Respec(Build)` is validated at once, and then:
- in a **zone of the world** (the town; any zone that posts creatures) it is worn **now**,
  with full pools, if the body is alive, out of any fight (ITEMS.md 5's ten seconds) and
  within `TRAINER_REACH` = 160 u of the **trainer**: a creature marked `npc` (nothing hurts
  it; the town posts one beside its board). Else it is refused with the reason;
- in a **team zone** (the arena, the practice ground) it is worn at the next respawn, as in
  v1, so a match can be re-specced mid-way (section 11).

The client's page (`K`, CLIENT.md 4.5) edits the points and the kit anywhere and offers
"Wear it" by the trainer; a game master wears any build at once, anywhere (GM.md 2).

**The build chosen is the character's** (2026-10-08). Under a hub the zone, having taken
a respec, saves the character at once (`Save`, HUB.md 3.2) and answers `RespecResult`
only when the hub has: `Ok` means worn (now in the world, at the next respawn in a team
zone) *and* the hub's, so it is what the character wears when it next enters anywhere,
after a logout or a travel. In a team zone the build saved is the one chosen, whether or
not the respawn that wears it came before the character left. A save the hub refuses is
answered `Err("worn here, but not saved: ...")`: the zone wore it, the next claim would
not have. Before this a build worn at the trainer reached the hub only with the next
periodic save (up to 30 s later) or the leaving one, and one chosen in a team zone was
lost when the character left before its respawn; the client never called the hub's
`SetBuild`, which is for a character that is offline (HUB.md 2), and never will from a
zone: while a character plays, its zone is the only writer of it.

Three **training dummies** (`dummy`: a creature marked `still`, 5,000 health, back after
5 s) stand in a triangle in the gated south street behind the trainer (2026-10-06; the one
on the green north of the market was out of the way): each is hit like any body and the
attacker reads what it dealt (LOOK.md 13.8). A `still` creature has no mind: it is driven an empty frame a tick.

## 10. Kits as data

Abilities and preset builds live in `assets/content/*.toml`, loaded and validated at zone
start against VOCABULARY.md 11 and this document. The zone sends the loaded content to every
client after `Welcome` (`FromZone::Content`), so a client can never run different numbers than
its zone and needs no content parser.

Each ability carries: `slot` (primary | secondary | guard | active), `cost` (kit points),
`aspect` (optional; the required aspect, and every elemental packet in the script must be of
that element), and the verb script of VOCABULARY.md 6. Price bands: primaries 0–2,
secondaries 2–4, guards 4–6, actives 6–12. **The range is the weapon's** (v2): a primary is
either a melee weapon (sword, hammer, staff, dagger) or the ranged one (crossbow, musket, and
the five elemental bolts); a secondary is a short utility (kick, shield bash, a knife of
300 u, mend). A melee build has nothing that reaches past the knife; a ranged build's primary
is the whole of its damage at range. (v1 gave every build a bolt in the secondary slot:
melee had free range and range had no edge; section 13.) A creature may hold a second
primary in its secondary slot (the sentinel's crossbow, the Warden's stone). The content
ships the abilities and seven presets (`ironclad`, `blade`, `frostweaver`, `shade`, `mender`,
`captain`, `marksman`); `assets/content/README.md` has the table. Phase 7 adds `mend`
(secondary, 4), `sanctuary` (active, 10) and `war_standard` (active, 10; two more squad
slots), the presets `mender` and `captain`, and two more files: `creatures.toml` (creatures
are builds without a budget, with abilities marked `creature = true` that no player build may
slot) and `trials.toml` (COMPANIONS.md 8, 11). An ability may carry `squad = N`: companions
it adds to the squad of whoever has it in the build.

## 11. Acceptance: a dominant build can be countered by re-speccing

PLAN.md 11.8 Phase 3. Measured with bot matches in the arena map (`gm-bot` arena test), eight
against eight, identical brains on both sides, several seeds:

1. **`ironclad`** (Colossus, Plate, Stone; hammer, stone throw, stomp, fortify, shield wall)
   against **`blade`** (Striker, Mail, Flame; sword, firebolt, dash, overhead, parry): ironclad
   must win the kill count by at least 1.3 : 1. Slash into Plate is 0.5, Flame into Stone is
   0.5, the hammer crushes Mail at 1.0 and ignores Fortify.
2. The blade side re-specs to **`frostweaver`** (Caster, Cloth, Frost; staff, ice shard,
   frost nova, haste, brace): frostweaver must now win by at least 1.3 : 1. Frost into Stone is
   2 and ignores armour; Plate's 0.90 speed cannot close on a hasted caster; Blunt into Cloth
   is 0.75.

3. The cycle closes: **`blade`** against **`frostweaver`** must be decided for the blade
   (Flame is 4× into Frost + Shadow).
4. A mirror match (blade against blade) must be even within noise, or the map or the brains
   are lopsided and the other results mean nothing.

`scripts/check-matrix.sh` runs these (`crates/gm-bot/tests/arena.rs`, three seeds, 60 s each);
`crates/gm-server/tests/counterpick.rs` repeats the first two legs over the real protocol at
150 ms and 3% loss, with the blades re-speccing to frostweavers through `FromClient::Respec` after
ten seconds. The ratios are recorded in PLAN.md 11.10 with the seeds. The test fails if any leg
is not decided, which would mean the matrix is not doing its job.

Measured 2026-10-01 (v1 content, three seeds × 60 s, 8 v 8, offline): ironclad : blade
**98 : 34** (2.9); frostweaver : ironclad **156 : 5** (31); blade : frostweaver **100 : 58**
(1.7); mirror 74 : 75. The second leg is a hard counter: Frost is 2× into Stone and ignores
plate, Stone is 0.25× into Frost + Shadow, and a hasted caster kites a plate colossus
indefinitely. Its counter-counter is the third leg, or a teammate with Flame.

## 12. Decisions and review log

**Proposed by Phase 3 (director to confirm or change):** the attribute set and ranges
(section 2), the pentagram of five elements (section 5), the kind × class table (4.1), the
formulas of section 6, the exact-100 budget (section 9). Everything here is tuned in
playtests; the structure (two tables, multiplicative layers, three structural bypasses) is
what the code depends on.

**2026-10-01, v1 draft reviewed by Gemini 3.1 Pro** (independent review before
implementation; verdicts are ours):
- Accepted: evasion while merely running was an unreadable permanent 30% reduction for a
  circle-strafing melee (now only inside a `MoveSelf` and 2 ticks after); stagger with no
  immunity chain-locked at any cadence above the threshold (1 s immunity added); guard break
  was undefined (defined above); Freeze could be re-applied the moment it ended (2 s Chill
  immunity); the tie-break between a status and the packet that carried it was undefined
  (defined above); attributes at 1 point per 2% were overpriced against abilities (price
  bands in section 10 so the budget binds); effective-HP spread of 2.4× contradicted the
  "compressed" principle (health is now 80 + 3·CON, armour capped at 0.35, and principle 1
  states the real bands).
- Corrected, not as proposed: "every dual aspect has a 4× hole" was false, but the suggested
  rule (`i` beats `i + 1`, `i + 2`) cannot fix it either: in any balanced 5-element matrix
  exactly five of the ten pairs share a predator. Section 5 now states the two pair shapes.
- Noted for tuning, not a document change: with placeholder damage numbers the frostweaver
  loses a pure DPS race to the ironclad; the content numbers are set by the acceptance
  matches (section 11), not by this document.
- Rejected: capping armour at 0.15 (the physical table is the counter layer; a plate colossus
  at 7% from the front with a shield up is the design, broken by stagger, stamina and magic).

**2026-10-01, implementation reviewed by Gemini 3.1 Pro** (after the acceptance matches passed):
- Accepted: the client only adopted the server's stamina, focus, statuses and guard state when
  the position disagreed, so a blocked hit's stamina or a Bleed never reached the prediction
  (now adopted and replayed whenever they differ, without counting a correction); a block's
  facing test used the shooter's current position for projectiles instead of the shot's
  direction; a swing pushed during its windup still landed after a stagger cleared the script
  (swings now require their script, ripostes excepted, and a stagger drops them); damage over
  time rounded each pulse so a magnitude of 1 dealt nothing and 6 dealt 8 (pulses now
  accumulate to exactly the magnitude per second); the parry's cooldown group was not applied;
  `Extend` could shorten a longer application. Blink's landing is now nudged out of solid as a
  hardening, though the tracer already pulls back from the plane.
- Rejected: sending the chill and stagger immunity timers to the client (the client never
  predicts a status an enemy applies, so its timers are never consulted).
- Deferred to Phase 4: the per-tick body list is O(N²) at 200 players (spatial partition with
  the swarm test).

## 13. v2 (2026-10-06): the director played it

The director, after playing Phase 14 with the fx and the GM hand: "char stats editable in
game like Tales of Pirates or Ether Saga: start with 30 points to distribute however we like;
an NPC for stat reset and a training dummy in town; very unfair that everybody has a right
click fireball shooting far and DD one-hit killing spellcasters; fights are tap tap tap dead;
'tactical' meant rock-paper-scissors, not the top view." Measured before the change: health
95–140, the sword 35 slash at 300 ms ×1.25 on cloth = a caster dead in two hits, about one
second of contact; the firebolt needed five; the presets spent 56–68 of 100 points on
attributes, so every build was near 20 on four stats and a point moved health by 3 of 140.

Done, as proposed and accepted ("do it all as you proposed"):
- attributes 5..=25 with thirty points of the character's own (section 2), the kit's budget
  40 apart from them (section 9); `Attributes::default()` is 11 across;
- the bands of section 6 widened (health ×7, damage ×0.8–1.6), armour cap 0.50, ward 0.60;
- the swings slowed and made heavier (sword 60 slash, 150/60/300 ms, 600 ms cooldown; the
  hammer 90 at 1 s; the dagger 35 at 400 ms), the bolts made primaries at about one and a
  half times their damage, and four secondaries in their place (section 10);
- the trainer, the dummy and the arena rule (9.1); the character's page `K`; old builds
  repaired at the hub; creature health ×3 (sentinel 1,200, Warden 15,000 at twice a player's
  blows: `might`, COMPANIONS.md 8.1) and the heals ×5 (Mend 100/s, Sanctuary 60/s: the
  healer's share of a body a second is what it was);
- the tactical viewport removed from the client (PLAN.md 4.3 [REVERSED]; COMPANIONS.md 6):
  1.3 KB of code and about 2% of the browser download, not the bloat, but no longer wanted.

Numbers measured after the change are in PLAN.md 11.10 and section 11 (re-run). The
director's call still open: the fight length he wants in play (the tempo and these numbers
are the knobs, GM.md 3).
