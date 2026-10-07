# Content v2 (2026-10-06; v1 was Phase 3)

`abilities.toml`, `builds.toml`, `creatures.toml` and `trials.toml` are the shipped content
(MATRIX.md 10, COMPANIONS.md 8 and 11). `gm-content` compiles them against the zone tick rate
and validates them against VOCABULARY.md 11 and MATRIX.md 9; the zone sends the compiled pack
to every client after `Welcome`. New abilities go at the end of `abilities.toml`: stored
builds name abilities by their place in it.
`gm_core::sim::test_content` mirrors this pack in code for tests; `gm-content`'s tests fail
when the two drift.

| Key | Slot | Cost | Aspect | What it does |
|---|---|---|---|---|
| sword | primary | 2 | — | 60 slash, 90° arc, 150/60/300 ms, 600 ms |
| hammer | primary | 2 | — | 90 blunt, ignores magic shields, heavy stagger, 1 s |
| staff | primary | 0 | — | 40 blunt, 600 ms |
| dagger | primary | 0 | — | 35 slash, 400 ms |
| crossbow | primary | 2 | — | 80 pierce bolt, 1.8 s |
| firebolt | primary | 2 | flame | 55 flame bolt + Burn 12/s for 3 s, 1.2 s |
| ice_shard | primary | 2 | frost | 50 frost shard + 1 Chill stack (3 freeze), 1 s |
| stone_throw | primary | 2 | stone | 75 stone, heavy knockback and stagger, 1.8 s |
| shadow_dart | primary | 2 | shadow | 40 shadow, fast, ignores evasion, 0.8 s |
| spark | primary | 2 | storm | 50 storm + Shock (interrupt), 1.2 s |
| musket | primary | 2 | — | 110 pierce, flat and loud, 3 s |
| kick | secondary | 2 | — | 25 blunt, heavy knockback and stagger 40, 1.5 s |
| shield_bash | secondary | 2 | — | 35 blunt, stagger 70, cannot be parried, 4 s |
| throwing_knife | secondary | 3 | — | 40 pierce, flies 300 u and no further, 2 s |
| mend | secondary | 4 | — | a dart that deals nothing: Regen 100/s for 3 s on whoever it hits, friend or foe; 12 focus, 2 s |
| shield_wall | guard | 6 | — | block 150°, 80%, stops projectiles, 12 stamina per hit |
| parry | guard | 4 | — | 150 ms window, 90°; success staggers 600 ms and ripostes 40 slash |
| brace | guard | 2 | — | block 120°, 50%, no projectiles |
| dash | active | 6 | — | 900 u/s for 150 ms, 30 stamina |
| leap | active | 6 | — | 400 forward, 320 up |
| charge | active | 6 | — | 700 u/s for 400 ms, stops on hit |
| blink | active | 10 | shadow | 256 u along the facing, 30 focus |
| overhead | active | 8 | — | 70 slash, 60° arc, slow |
| stomp | active | 10 | stone | 128 u cylinder at the feet: 35 stone, Slow 30% 2 s, knockback |
| fortify | active | 8 | stone | Fortify 40% for 5 s (hammers ignore it) |
| frost_nova | active | 10 | frost | 160 u sphere: 30 frost + 2 Chill stacks |
| haste | active | 8 | — | Haste 25% for 5 s |
| thunderclap | active | 10 | storm | 140 u sphere: 40 storm + Shock |
| vanish | active | 8 | shadow | Stealth 256 u for 6 s |
| poison_cloud | active | 8 | shadow | 96 u cloud ahead for 5 s: Bleed 6/s |
| fireball | active | 12 | flame | 20 flame bolt, 96 u splash 30 flame + Burn |
| sanctuary | active | 10 | — | a circle of 140 u where the caster aims (within 500 u) for 6 s: Regen 60/s on every body in it; 35 focus, 14 s |
| war_standard | active | 10 | — | two more squad slots (five companions); used: Fortify 15% for 8 s on every body within 256 u, enemies included |
| maul | primary | creature | — | 55 blunt through magic shields, 120° arc, 96 u reach, 550 ms windup, heavy knockback, one every 2 s |
| quake | active | creature | stone | a circle of 150 u under whatever is aimed at within 700 u; breaks 1.3 s later for 70 stone |

The range is the weapon's (MATRIX.md 10, 2026-10-06): a primary is a blade or the bow; a
secondary is a short utility. Presets (thirty attribute points each, a kit of at most 40):
**ironclad** (colossus, plate, stone; hammer, shield bash), **blade** (striker, mail, flame;
sword, kick), **frostweaver** (caster, cloth, frost + shadow; ice shard, kick),
**shade** (infiltrator, leather, shadow; dagger, knife), **mender** (caster, cloth, storm:
the healer; staff, mend), **captain** (striker, mail, flame; sword, kick, `war_standard`:
the leader of five) and **marksman** (striker, leather, storm; crossbow, knife). MATRIX.md
11 records how the first four fare against each other; a bot that counter-picks takes the
first preset listed among those that score the same.

## Creatures and trials

`creatures.toml` (COMPANIONS.md 8.1): a creature is a build without a budget, with its health
set here, how far it sees, how far it is leashed to its post, when it comes back, and for a
boss what it drops. Abilities marked `creature = true` are theirs alone. A map places them
with `gm_creature` entities.

| Key | Body | Health | Kit | Sight / leash | Back after | Drops |
|---|---|---|---|---|---|---|
| sentinel | striker, mail, flame | 1,200 | sword, crossbow, parry; overhead, dash | 700 / 900 | 600 s | nothing |
| warden (boss) | colossus, plate, stone | 15,000 (stagger 400) | maul, stone_throw; quake, stomp | 900 / 1,100 | 120 s | 3 components (standard: core/iron, frame/ash, catalyst/basalt; top: core/dragonbone, shard/boss_scale, catalyst/basalt), 30 silver |
| dummy (`still`) | colossus, leather, stone | 5,000 (never staggers) | nothing it uses | — | 5 s | nothing |
| trainer (`still`, `npc`) | striker, cloth, flame | cannot be hurt | nothing it uses | — | — | the build redone beside it (MATRIX.md 9.1) |

`trials.toml` (COMPANIONS.md 11): an encounter of a map judged through a lens. The four of
the tutorial are on the Warden, 300 s, one human: `warden_leader` (500‰ of the party's damage
under the candidate's orders, at most one party death), `warden_vanguard` (500‰ of the blows),
`warden_striker` (350‰ of the damage), `warden_mender` (500‰ of the healing, no death).

## Looks (Phase 14, CONTENT.md 3)

Every row may name what it looks and sounds like: an ability's `icon`, `prop` (held while
it is the primary and nothing is worn) and `sound`; a template's `model` (a prop under
`models/props/`), `icon`, `held`, `fit` and `fit_view`; a material's `icon` and `tint`; a
creature's and a build's `model` and `icon`. `VERSION` holds the content's number;
`LICENSES.md` names every file that is not ours. `gm-tools content check` resolves every
link, `build` writes `assets/built/content/` (committed), `report` lists it all.

| Key | Slot | Cost | Aspect | What it does |
|---|---|---|---|---|
| musket | secondary | 4 | — | 55 pierce ball at 2,600 u/s, flat, one shot every 2.5 s (the gun; CONTENT.md 11) |

## Items

`items.toml` holds the item templates and the materials of the five component layers
(ECONOMY.md 4). `gm_content::items` validates it: materials are `layer/name`, only catalysts
carry an element, every template takes a core and a frame, and the best possible craft of any
template gives at most 250 per mille over a standard item (PLAN.md 0). The hub refuses crafts
of templates that are not in this file.
