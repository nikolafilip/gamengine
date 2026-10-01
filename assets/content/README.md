# Content v1

`abilities.toml` and `builds.toml` are the shipped content (MATRIX.md 10). `gm-content`
compiles them against the zone tick rate and validates them against VOCABULARY.md 11 and
MATRIX.md 9; the zone sends the compiled pack to every client after `Welcome`.
`gm_core::sim::test_content` mirrors this pack in code for tests; `gm-content`'s tests fail
when the two drift.

| Key | Slot | Cost | Aspect | What it does |
|---|---|---|---|---|
| sword | primary | 2 | — | 35 slash, 90° arc, 300 ms |
| hammer | primary | 2 | — | 50 blunt, ignores magic shields, heavy stagger, 600 ms |
| staff | primary | 0 | — | 25 blunt, quick |
| dagger | primary | 0 | — | 22 slash, 200 ms |
| crossbow | secondary | 4 | — | 40 pierce bolt, 1.5 s |
| firebolt | secondary | 4 | flame | 35 flame bolt + Burn 8/s for 3 s |
| ice_shard | secondary | 4 | frost | 40 frost shard + 1 Chill stack (3 freeze) |
| stone_throw | secondary | 4 | stone | 45 stone, heavy knockback and stagger |
| shadow_dart | secondary | 3 | shadow | 30 shadow, fast, ignores evasion |
| spark | secondary | 4 | storm | 32 storm + Shock (interrupt) |
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

Presets (all exactly 100 points): **ironclad** (colossus, plate, stone), **blade** (striker,
mail, flame), **frostweaver** (caster, cloth, frost + shadow), **shade** (infiltrator, leather,
shadow). MATRIX.md 11 records how they fare against each other.

## Items

`items.toml` holds the item templates and the materials of the five component layers
(ECONOMY.md 4). `gm_content::items` validates it: materials are `layer/name`, only catalysts
carry an element, every template takes a core and a frame, and the best possible craft of any
template gives at most 250 per mille over a standard item (PLAN.md 0). The hub refuses crafts
of templates that are not in this file.
