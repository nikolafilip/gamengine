# The game master's hand — v1 (2026-10-06)

What a game master may do to a running zone while playing in it, so that the numbers of a
fight can be tuned on a body that answers instead of in a file, a rebuild and a restart.
Asked for by the director on 2026-10-06: "animations seem a bit too fast ... what we aim
for is a bit longer animations so skilled players can react ... some kind of admin menu so
I can test and tune it", with "custom builds and a reset of attribute points".

Code follows this document; changes to both go in one commit.

## 1. Who

A **game master** is a character the zone grants the page to, at its entry:

- under a hub, every character of an account that is a **moderator** (`accounts.moderator`,
  `gm-hub --grant-moderator EMAIL`; `scripts/dev/play.sh gm EMAIL`): the hub says so in the
  claim (`HubResponse::Claimed.gm`, hub protocol 10);
- in any zone, a character named on its command line (`gm-server --gm NAME,NAME`): an open
  zone's way, and a hand for tests.

The zone tells the client right after `Content` (`FromZone::Gm(Granted)`), with the tuning
as it stands when it is not the default. Anyone else who asks is answered
`Gm(Refused("not a game master"))` and nothing happens.

## 2. What

`FromClient::Gm(GmOp)` (protocol 10), answered by a new `Content` to **everyone** when the
content changed, and by `FromZone::Gm(GmNews)` to the asker: `Tuning` with what now stands,
or `Refused` with why.

| op | does |
|---|---|
| `Tempo(x)` | every script's windups, active windows, recoveries, cast times (`Step.at`), parry windows, dashes and charges take `x` times as long; 0.25–4. Cooldowns, statuses, bolts' lifetimes and areas' pulses keep their time: the tempo is about what a body shows before it lands something. |
| `Ability(AbilityTuning)` | numbers set outright on one ability, in milliseconds: `cast_ms` (the last step's moment, earlier steps in proportion), `windup_ms`, `active_ms`, `recovery_ms`, `cooldown_ms`; each `None` leaves the tempo's value; all `None` forgets the ability. |
| `ResetTuning` | the content as loaded. |
| `Heal { everyone }` | full health, stamina and focus, every cooldown ready, every status gone, where the body stands; nothing for a dead body. |
| `Respec(build)` | the build **now**, where the body stands, with full pools: validated as a respec is (the thirty points and the kit's budget of 40, MATRIX.md 9), told as a `Respawned` so the own prediction and everyone's picture of the body switch as they do at a respawn. |

A tuning is applied to the pack **as loaded** (`gm_core::tuning::Tuning::apply`), never to
one already tuned, so every change is reversible; what comes out is validated as content is
(`ContentPack::validate`) and refused if content would refuse it, the tuning standing as it
was. Every body's kit is compiled against the new pack in place (`Zone::retune`): builds,
health, cooldowns and a script under way are kept.

## 3. Kept

`gm-server --tuning FILE`: the tuning is read from the file when the zone starts (one that
content refuses is dropped with a warning) and written after every change, as TOML:

```toml
tempo = 1.5

[[ability]]
key = "sword"
windup_ms = 180
```

The play stack gives its three zones one file (`~/.local/share/gamengine/play/tuning.toml`):
numbers set in the town are read by the arena and the dungeon at their next start. When
numbers are decided on, they go into `assets/content/abilities.toml` and the file is deleted.

## 4. The page

`G` in the game, or **Game master** in the menu (both only for whom the zone granted it):

- **Timing**: the tempo (a slider, `Set tempo`), `Reset all tuning`; every ability with
  its cast, windup/active/recovery and cooldown as the zone's ticks make them (at 20 Hz a
  90 ms windup is 100), a `*` on a tuned one; pick one, drag its sliders, `Set ability`
  (only what was moved is set), `Forget ability`.
- **Build**: the character's build editor (`character.rs`, the same one `K` opens for
  everybody, CLIENT.md 4.5, in its two-column shape of 2026-10-06): a preset to start
  from, frame, armour, aspects, the thirty attribute points (`-`/`+`, 5–25, what each buys
  beside it), weapon, secondary, guard, up to four actives, every list whole — with the
  points left and the kit's cost of 40 in gold over them, a line that reads the ability
  under the cursor, and the pack's verdict when the draft is not whole; `Wear it now`:
  anywhere, at once, the game master's privilege. The panel is as wide as the
  character's, and Close sits at the end of the tab row. (A player wears a
  build at the trainer, MATRIX.md 9.1.) Until 2026-10-06 this tab was the only way to a
  custom build.
- **Body**: `Heal me`, `Heal everyone here` (asked twice).

The zone's last word stands at the top right (`asked`, `tuned`, `refused: ...`). The page is
a UI script's `gm timing` / `gm build` / `gm body` (CLIENT.md 9).

## 5. Also in this change

- **Instant areas are seen** (PROTOCOL.md 5, `INSTANT_AREA_ECHO_MS` = 100): an area with
  no duration (Thunderclap, Stomp) was spawned, pulsed and removed within one tick and so
  was in no snapshot: the client, which draws an area's disc and burst from the snapshot,
  never saw one ("the thunder animation is not visible"). It now stays on the wire, spent,
  for 100 ms after its one pulse.
- **Sparring partners** (`gm-bot --behaviour spar`; `play.sh spar [N] [BUILDS]`): the
  duelist's kit, standing where it arrived, fighting only whoever strikes at it (a blow
  taken or a windup within 420 u) for 8 s after the last, never further than 260 u from
  home. Strollers, who never swing, are left alone whatever their team.
- The play stack's people are respawned by `play.sh people` (they live 23 h: a session
  token lives 24, and a bot that outlived its token ended with "hub refused: unauthorized").
- **An old page against a new zone** (found by the director from another machine the same
  evening: "protocol version mismatch", then "the character is still in a zone"): the
  browser's cache held the last build's wasm and glue. The page's files now carry the
  build's stamp (WEB.md 5), the play stack's servers say `no-cache`, and the zone's
  rejection says to reload the page or get the new client. A ticket spent on a rejected
  connection leaves its character in transit until the hub's sweep (a minute): the "still
  in a zone" that followed each try.

## 6. Measured (2026-10-06)

- A Thunderclap seen: 5 frames of the area per cast on a software client at ~30 fps (the
  100 ms echo plus the interpolation delay), the burst 6–7 filmed frames (0.35 s of its
  own); before the echo, 0 frames.
- A tuning round trip by script on the play stack (town, 20 Hz): `Set tempo` → the zone's
  "content tuned" → every client's new `Content` within the same second; the file written
  at once.
- `gm-client` 9,842,640 bytes (+44,400; baseline updated); WebGPU wasm 1,162,150
  (392,222 brotli), WebGL2 3,153,714; 376 workspace tests outside the hub, the hub's
  with the database.

## 7. Open

- Whether the tuning should reach the other zones at once (one hub-held tuning instead of a
  file read at start).
- A tempo on statuses and cooldowns as a second knob.
- A slow-motion knob for watching (the tick rate is the zone's; not built).
- Whether moderators and game masters stay one flag.
