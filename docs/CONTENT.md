# Content: the standard and the pipeline (v1)

Phase 14 (PLAN.md 11.8), first half. Until now "content" was five TOML files of numbers and
verbs: what a thing *does*. Nothing says what a thing *looks like* (a sword is drawn as
nothing; an ability has no icon; the inventory is a list of words), and the only models the
game knows are avatars uploaded by players. This document is the standard every piece of
content follows from here on: items, abilities, creatures, the characters' own looks,
pictures, sounds; how a new one is added (a model, a picture, a row of numbers, a link to an
ability), how it is checked, built and shipped, and what the tool that does it is. It is the
"editor's engine" the director asked for: the editor itself is deliberately a text file and
a command line (10).

Code follows this document; a change to either goes in one commit. v0 was the proposal
(2026-10-03, reviewed in 12.1); the director's decisions are in 11; v1 is what Phase 14
built, with what was measured in 13.

## 1. Principles

1. **One source, in text, in the repository.** A thing is a row in a TOML table under
   `assets/content/`; every number, name and link is in that row, and so is the name of the
   file that holds its look. Nothing lives only in a database, a GUI or a binary. Text is
   what the director, a spreadsheet, a reviewer and an AI can all read and write, and `git`
   is the editor's undo. The engine already works this way for what things do (gm-content);
   this extends the rows with what they look and sound like.
2. **Links by key, checked at build.** A row names other things by their key (`prop =
   "sword"`, `icon = "firebolt"`, `drops = ["core/iron"]`). `gm-tools content check` resolves
   every link and refuses a dangling one; CI runs it, so the repository never holds a
   reference to a thing that is not there.
3. **The same pipeline for every model.** An item in a hand, a creature, an official avatar
   and a player's upload are all one `.glb` in and one `.gmm` out (MODELS.md 5), checked by
   the same strict reader, cached and drawn by the same code. A *kind* says what is checked:
   an avatar fills a frame's envelope, a prop fits a hand, a creature fills its own hitbox.
4. **Never wait for art.** Every look is optional. A template without a model is drawn as
   nothing in the hand; an ability without a picture gets a glyph drawn from its verbs; a
   creature without a model is its frame's mannequin. A client whose bundle is older than
   the zone's content draws what it knows and nothing else; it never refuses to play.
5. **Looks cost no wire and no simulation.** A look is a client-side function of keys the
   client already receives or will receive as two-byte indices (LOOK.md 6). The zone and the
   hub never read a model or a picture; what a thing does is unchanged by what it looks like
   (MODELS.md 1: cosmetic only).
6. **Built, deterministic, measured.** `gm-tools content build` turns the sources into the
   bundle the client loads; the same sources give the same bytes; every bundle file has a
   budget in `budgets.toml`.

## 2. The directory

```
assets/content/
  README.md                   the tables, as now
  VERSION                     the content's version: a number, bumped by whoever changes a table
  abilities.toml  builds.toml  creatures.toml  trials.toml  items.toml   (rows; see 3)
  LICENSES.md                 every file that is not ours: where from, which licence
  models/
    props/<key>.glb           a thing held: sword, dagger, hammer, staff, crossbow, musket
    creatures/<key>.glb       a creature on the standard rig (Phase 16)
    avatars/<frame>_<armour>.glb   the official look of a frame in an armour class (Phase 16)
  icons/<key>.png             32 × 32 pictures that override the baked ones (optional; a
                              key's `/` is `_` in the file name: `status_burn.png`)
  ui/
    skin.toml                 the atlas's plan: the two faces and their sizes, which picture
                              is which piece and its nine-slice insets
    *.png                     the skin's pictures (`scripts/dev/skin-gen.py` drew the first)
    *.ttf                     the fonts the tool rasterises (OFL or CC0 only; LICENSES.md)
assets/built/content/         what `gm-tools content build` writes (committed, 5.4)
  manifest.gmc                every entry by key: files, hashes, icon keys, held, fits
  props/<key>.gmm             ingested props
  ui.gma ui2.gma ui3.gma ui4.gma   the atlases, one per UI scale (LOOK.md 2.2)
```

A thing's **key** is its table's id (`sword`, `catalyst/ember`, `warden`); file names are the
key with `/` as `_`. Keys are lowercase ASCII letters, digits, `_` and one `/` for layers.

## 3. The rows

The existing tables keep every field they have. Each gains **look fields**, all optional,
each the key of a file in 2:

| Table | Field | Means |
|---|---|---|
| `[[ability]]` | `icon` | `icons/<icon>.png`; absent: a glyph from the verbs (LOOK.md 3.4) |
| | `prop` | the prop drawn in the hand while this ability is the primary and nothing is worn (a sword build with no sword still swings one) |
| | `sound` | the patch (SOUND.md 2) its cues use; absent: the patch of its verb kind as today |
| `[[template]]` (items) | `model` | `models/props/<model>.glb`, drawn in the hand of whoever wears an item of this template |
| | `icon` | absent: baked from `model` (5.3); absent both: a glyph of the template's kind |
| | `held` | `right` (default), `left`, `back`: which `prop_*` bone (MODELS.md 2) |
| `[[material]]` | `icon` | for the crafting screens of a later phase; absent: a glyph by layer |
| | `tint` | `[r, g, b]`, multiplies the worn item's texture when this material is its core: a dragonbone sword is pale (LOOK.md 6) |
| `[[creature]]` | `model` | `models/creatures/<model>.glb`; absent: the frame's mannequin |
| | `icon` | its portrait; absent: baked from the model |
| `[[build]]` | `icon` | the archetype's picture on the new-character screen; absent: baked from the frame's avatar or mannequin |
| the frame × armour table (new, `avatars.toml`, Phase 15) | `model` | `models/avatars/<frame>_<armour>.glb`: the official look, worn when a character has no model of its own |

Since content v3 (2026-10-07, MODES.md 11.1) `[[template]]` has a third `kind`, `stack`:
no layers, a `cap` (the most one holder carries, 1–1,000) and an optional `heals`; a
firearm's `ammo` names one. A stack has no model or icon yet: a glyph of its kind.

Order matters where it did before and nowhere new: `abilities.toml` is **append-only**
(stored builds name abilities by their place; README). Items are named by their **key**
everywhere that lasts (the database's `template_id`, `ItemSummary.template`, the hub's
reading to a zone: LOOK.md 6.2), so `items.toml` may be reordered; a template whose items
exist keeps its row, with `retired = true` once nothing new is to be made of it. Indices
exist only inside one zone session, in the pack that zone itself sent (LOOK.md 6.2).

### 3.1 Props

A prop is authored like a glTF avatar (MODELS.md 3: metres, +Y up, facing +Z; one base-colour
texture; triangles) with two differences: it has **no skin** (a skin, if present, is
dropped), and its **grip** is its origin: the hand closes on the origin, the business end
points along +Z (a blade's tip, a barrel's muzzle, a staff's head), the edge or the sight
along +Y. The fist holds +Z **across the forearm** (LOOK.md 6.3: a blade, a hammer); what
is aimed and not swung is turned about Y by its fit to lie along the arm (`turn = [0, 100,
0]` for the crossbow, 88 for the musket), a staff a little the other way to stand up.
Models from packs rarely come the right way round either, so a row may carry a `fit`:

```toml
[[template]]
id = "sword"
model = "sword"
fit = { move = [0.0, -0.02, 0.0], turn = [0, 90, 0], scale = 1.0 }   # metres, degrees, after
```

`gm-tools content build` applies the fit before quantising, so the `.gmm` is already in hand
space and the client does nothing at draw time. `gm-tools content look sword --out x.png`
(9) shows the result on the mannequin in every stance, to tune a fit by eye.

A row may carry a `fit_view` as well: a second fit in the same terms (metres, degrees
about glTF's X, Y, Z, a scale) that the first-person view model applies to the built prop
and nothing else does (LOOK.md 6.4; the manifest carries it, `gm_model::pose::view_fit`
turns it into model space). What the fit laid along the arm for the third person is
turned back to point along the look: `fit_view = { turn = [0, -88, 0] }` for the musket,
the pistol's also moved 0.3 m nearer the eye. `gm-client --offline --prop KEY` in the
first person shows it (`R` held: the reload).

A prop's vertices are written with `joints = [0, 0, 0, 0]`, `weights = [255, 0, 0, 0]`: the
character pipeline's uniform block (MODELS.md 9: 24 matrices, the scale, a tint, the light)
is uploaded whole for a prop too, with the hand's matrix at index 0 and the other
twenty-three unused; nothing about the block's layout or the shader changes.

A pack's file without a texture (flat colours per material, or `COLOR_0` per vertex, the
way the low-poly packs come) gets a texture made for it: every flat colour becomes an
8 × 8 swatch of a small atlas and the vertices point at their swatch (at most 256 colours;
a file with some textured and some flat primitives is refused: it must be one or the
other). Our own hammer, musket and pistol are made this way (`gm-tools content synth`).

Two-handed holds and sheathing to the back out of a fight are `held` values a later phase
can act on. **The off hand** (2026-10-09, LOOK.md 6.5): an ability may carry a `prop` whatever
its slot, and the prop of a build's **guard** ability is drawn in the left hand (`shield_wall`
holds `shield`, ours: `gm-tools content synth shield`, built in hand space already, its face
along +Y). `gm-tools content look KEY --left` is the fitting room for it, and offline
`gm-client --offline --prop KEY --off KEY` holds one in each hand.

### 3.2 Creatures and official avatars (Phase 15)

The same `.glb` rules as an upload (MODELS.md 3 and 4): on the standard rig, T-pose, one
texture, the frame's envelope. A creature is checked against the envelope of *its* frame
(creatures are builds, COMPANIONS.md 8.1). Nothing else differs: the ingestion is
`gm_ingest::ingest` as the hub runs it.

## 4. Kinds and budgets

| Kind | Triangles | Texture | `.gmm` bytes | Extent | Rig |
|---|---|---|---|---|---|
| avatar (player upload, official) | ≤ 3,500 | ≤ 1024², BC1 | ≤ 1,572,864 | the frame's envelope | 24 bones, T-pose |
| creature | ≤ 3,500 | ≤ 1024² | ≤ 1,572,864 | its frame's envelope | 24 bones |
| **prop** | **≤ 1,000** | **≤ 256²** | **≤ 131,072** | **≤ 96 u from the grip** (3 m) | none: every vertex on bone 0, weight 255 (LOOK.md 6) |

The `.gmm` header's `frame` byte is **255 for a prop** and a flag bit (`FLAG_PROP`, bit 2)
says so; `bone_mask` is 1 and the pivots are zero. The strict reader (MODELS.md 5) accepts a
prop only with that combination, and a zone or hub asked to treat a prop as an avatar refuses
it (the hub's `verify` requires an avatar frame).

Pictures: an icon is **32 × 32** dots, RGBA, straight alpha, and is in each atlas at that
atlas's density (LOOK.md 2.2): 32 to 128 texels, baked from its model at that size, or
given as `icons/<key>.png` (32 × 32) with `<key>@2x.png`, `@3x`, `@4x` beside it when
somebody has drawn them (64, 96, 128; a density without a file is the 32 enlarged). The
skin's pictures are what `skin.toml` declares, each ≤ 256² dots, with their finer
pictures beside them the same way.

## 5. The tool: `gm-tools content`

```
gm-tools content check [DIR] [--built assets/built/content]
    every row, every link, every file, every budget; exit 1 with every violation listed;
    with --built, also rebuild into memory and compare with the committed bundle, byte
    for byte (CI's check, 5.4)
gm-tools content build [DIR] [--out assets/built/content]
    check, ingest, bake, assemble, write the manifest (and remove what is no longer made)
gm-tools content report [DIR]
    a table of everything: props with their triangles, bytes and reach; templates with
    their models and icons (baked or given); abilities with icons, props and sounds; the
    faces' metrics; the atlas
gm-tools content import prop|creature|avatar|icon KEY FILE
    a model (`.gltf` with its .bin and image beside it, or a `.glb`) packed as one `.glb`
    under models/<kind>s/<key>.glb and checked as its kind, or a 32 × 32 picture as
    icons/<key>.png; the row that names the key is the author's to write
gm-tools content synth sword|hammer|musket --out FILE.glb
    one of the tool's own flat-coloured props (7)
gm-tools content atlas [DIR] [--density 1..4] --out FILE.png
    a built atlas as a picture, to look at
gm-tools content look KEY [--dir DIR] --out FILE.png
    the fitting room: the mannequin holding the prop KEY in nine stances (standing,
    running, the windup, the swing, the recovery, the guard, the parry, the cast, the
    dash), from the front and from its right
```

Still to come (Phase 15, the editor, carries them): `new` (a row written for you), `import csv`
(a spreadsheet's rows into a table), and a fitting room that moves (`look` draws stills;
`gm-client --offline --prop KEY` holds a prop in the own hand and arms the crowd, 9).

### 5.1 `check`

Runs in CI (`scripts/check-content.sh`) and before every `build`. It parses every table
with `gm-content` (so what a thing does is validated as before), then: every look field
names a file that exists; every `.glb` ingests under its kind's budget; every `.png` is the
size its place wants; every font is a file the tool can rasterise and is listed in
`LICENSES.md`; `skin.toml` names every piece the client's toolkit asks for (LOOK.md 2.2);
`abilities.toml` and `items.toml` have not had a row removed or moved (against the built
manifest in the tree). Every violation is listed, then it fails.

### 5.2 `build`

For every model: `gm_ingest` by kind (3.1's fit for props), the `.gmm` written by key. For
every icon: the file given, or baked (5.3). Then the atlas (LOOK.md 2.2) and
`manifest.gmc`, a `bitcode` record (the wire's codec, already in the client) read in one
call: **every entry carries its key** — the templates with their model file names and icon
cells, the abilities with their icon cells and prop keys, the props with their files and
SHA-256, the statuses' icon cells, the frames' portrait cells, the font tables, the skin's
pieces. The manifest is the only thing a client parses besides `.gmm` files and the atlas,
and the client looks everything up **by key**, never by position, so a bundle older or newer
than the zone's content resolves what it can and draws nothing for the rest (6).

### 5.3 Baking icons

An icon baked from a model is **rendered by the tool's own rasteriser**
(`gm_ingest::raster`, the one the coverage rule uses, extended with texture sampling and the
game's half-lambert light), never by a GPU: the same bytes on every machine, no device in
CI. The rule that makes it so is SOUND.md 2's: IEEE basics only (add, multiply, divide,
`sqrt`), integer texel fetches, and tables where a curve is needed (sRGB in and out); no
`powf`, `sin` or anything from the platform's libm, whose last bits differ between
machines. A test pins the icons' hashes as it pins the patches'. The view is orthographic,
the model's longest axis from bottom-left to top-right (as every RPG draws a sword), lit
from the top-left, filling 28 of the 32 dots, on transparent. A creature's or avatar's
portrait is the head and shoulders from the front, the same way.

### 5.4 What is committed

`assets/built/content/` is committed, like the built maps are: a client runs from a checkout
without the tool, the browser build copies it as it copies maps, and CI checks that
`gm-tools content build` into a temporary directory produces **the same bytes** as the tree
holds (hash for hash), so nobody commits a stale bundle or a hand-edited one. This holds
because ingestion is deterministic already (MODELS.md 5: the hub re-encodes an upload
canonically and believes only bytes that match) and the compressor is `miniz_oxide`, pure
Rust at the version `Cargo.lock` pins: the same input gives the same stream on every
platform. A crate bump that changes the stream shows up as a CI failure, and the bundle is
rebuilt in the same commit.

## 6. From the sources to the players

- **Zones** read the TOML as today (gm-content) and send the compiled pack after `Welcome`.
  The pack gains nothing but **keys**: the list of prop keys the content names (LOOK.md
  6.2), so that a two-byte index on the wire, valid for that session's pack only, names a
  prop. Looks never travel.
- **The hub** reads `items.toml` as today; a gear reading now also tells the zone the
  **keys** of the templates worn (hub v1.9): strings, so that a hub and a zone on different
  content versions disagree about nothing but what to draw (a key the zone does not know is
  an empty hand).
- **Clients** load the bundle: natively from `assets/built/content/` next to the other
  assets; in a browser from the same path on the site (`scripts/build-web.sh` copies it with
  the maps; the browser's HTTP cache is the cache: nothing goes through the model store).
  The manifest and the atlas are fetched at start (the screens before the game use them;
  until they arrive the toolkit draws with its built-in font and plain plates: principle 4);
  props are loaded when first seen, one at a time, like avatars, through the same model
  cache, **from the bundle and never from the hub**: the cache gets a second source (the
  bundle's directory or URL, by file name, checked against the manifest's hash), and a
  prop the bundle lacks is logged once and never asked for again. The hub holds no
  official content and is not asked for any.
- **Version skew.** A bundle names its content by key and holds the hashes; a zone's pack
  names props by key, and its `Look` indices are of that pack. A client with an older
  bundle lacks some keys and draws those as nothing; a client with a newer bundle than the
  zone has keys the zone never names; an index past the pack's list is `NONE`. None of it is
  an error, and nothing indexes a list without a bounds check (the release profile aborts
  on a panic). The manifest carries the content's git-free **content version** (a number
  in `README.md`, bumped by whoever changes a table) and a client says it in its report, so
  a mismatch can be read off a log.

## 7. Where the first models come from

The pipeline is indifferent to the source: anything that is one `.glb` with one texture.
For the first props (Phase 14) three sources were weighed on 2026-10-03:

| Source | Look | Cost | Fit |
|---|---|---|---|
| CC0 packs on itch.io/OpenGameArt (KayKit *Fantasy Weapons*, *Medieval Weapons – Small Low Poly Pack*, *50 Low Poly Swords*, Quaternius *Medieval Weapons* and *Guns*) | stylised low-poly, flat or palette-shaded, 100–800 triangles | none; CC0 needs no attribution, and LICENSES.md names them anyway | in the budget as they are; glTF or GLB offered by most (Quaternius ships FBX/OBJ/Blend, which the tool does not read) |
| A 3D-generation service (Tripo, Meshy: image or text to a textured mesh; free tiers exist) | "realistic" textured meshes, 5–30k triangles, PBR maps | an account the director opens; a decimation to ≤ 1,000 triangles and a bake of the base colour, which the tool does not do yet | the base-colour texture is what this renderer draws (half-lambert, one texture, MODELS.md 9): a scan-like model is flattened to its diffuse anyway |
| Procedural in the tool (a sword from blade, guard, grip and pommel parameters; a texture painted by an image model, Gemini's `gemini-3-pro-image`) | ours, consistent, parametric | code | the mannequin and the sounds were made this way; the look is "programmer's" |

**Recommendation**: the CC0 packs for Phase 14 (fetched by a pinned script with SHA-256
checks like `scripts/fetch-ericw-tools.sh`, the chosen files committed under
`models/props/` with their lines in LICENSES.md), because the pipeline, the drawing, the
icons and the screens are the work and a known-good mesh gets them built; and
`fit` plus the kind budget make a generated or commissioned model a drop-in later. What
"realistic" means in this engine is a well-painted 256² texture on a few hundred triangles,
the look of Tales of Pirates and Ether Saga (PLAN.md 1.1: 2–4k hand-painted); not geometry.

## 8. Pictures: icons and the skin

Same indifference: a PNG is a PNG. The baked icons (5.3) are the floor for items and
creatures. Abilities have no model to bake from; their floor is the **glyph from the verbs**
(LOOK.md 3.4). For a set that looks like one hand drew it, the plan is an **image model**
(`gemini-3-pro-image`, offered in Phase 3 and unused since) prompted with one style sheet,
one icon per call, the results committed under `icons/` as any artist's would be, replaced
one file at a time when a person draws better ones. The skin (frames, slot wells, bars,
cursor) the same way, or by hand in any paint program; LOOK.md 2.2 says what pieces exist.

## 9. Looking at content

The fitting room of v1 is the client itself: `gm-client --offline --map
assets/maps/built/town.bsp --prop sword` (CLIENT.md 2) stands the own body in the town
holding the prop of that key from the bundle, and arms the crowd with it (`--crowd N
--crowd-dir DIR`): walk, swing, look in third person (`V`). Change the row's `fit`, run
`gm-tools content build`, run again. Faster, and without a GPU: `gm-tools content look
sword --out sword.png` draws the mannequin holding the prop in nine stances from the front
and from its right (the CPU rasteriser that bakes the icons), which is how the grip and
the fits of LOOK.md 6.3 were chosen. The orbiting camera and the animation cycling are
Phase 15's, with the editor.

## 10. Why the editor is a text file (deliberately absent)

A GUI editor — import a model, set the numbers, link the abilities, see it — is a product of
its own: forms for every table, a 3D view, undo, a save format, and it has to be kept
working as the tables change. Everything it would do, this standard does with a text
editor and `gm-tools content`: a row is the form; `check` is the validation the form would
do; `look` is the 3D view; `git` is undo and history; `import csv` takes a spreadsheet. The
AI that writes most of this code edits TOML better than it drives a GUI. **The director
wants the editor** (11): it is Phase 15, `gm-tools content edit`, a native-only screen on
the client's own toolkit (LOOK.md 2) over these same files, nothing else changes; this
phase builds what it runs on, and the standard is what makes it a pure addition.

Also absent on purpose: hot reload of a zone's pack (a content change restarts zones and
the hub, as now); per-instance item models (a player-made sword skin: the item row would
carry a model id and the wire a hash, exactly as avatars do — the standard leaves the place
for it, Phase 11's `ItemSummary` and LOOK.md 6's `Look` both extend by a field); animations
as content (MODELS.md 9: the set is shared and procedural; a real set is Phase 15's or
later's); content in the database.

## 11. Proposed numbers and open decisions

Proposed: a prop at 1,000 triangles, 256², 128 KiB, 96 u; icons 32 × 32; the atlas and the
bundle budgets in LOOK.md 9; `abilities.toml` and `items.toml` append-only with `retired`;
a content version number in README.md; the built bundle committed and verified by CI.

**Decided by the director, 2026-10-03** (the questions v0 asked, with the answers):

1. **The source of the first models** (7): finished models may be reused — CC0 packs it
   is. The look to aim for is "between Tales of Pirates and Ether Saga": painted low-poly,
   Ether Saga is realistic enough. Counter-Strike 1.6 is the other root: its *standard* is
   taken (world-model weapons of a few hundred triangles with painted textures, and a
   first-person view model, LOOK.md 6.4), never its assets, which are Valve's.
2. **A GUI editor**: yes. It is Phase 15 (PLAN.md 11.8): `gm-tools content edit`, native
   only, on the client's toolkit, over these same files; this phase builds its engine (5).
3. **The gun**: the `musket` ability and template as proposed; the numbers stay the
   director's to tune.
4. **Sources**: CC0 and ours only, until the director says otherwise (LICENSES.md).

Still open: nothing in this document.

## 12. Review log

### 12.1 Design review (Gemini 3.1 Pro, 2026-10-03, over this document and LOOK.md v0)

Fourteen findings; nine accepted, four rejected, one in part. The ones on this document:

| # | Finding | Verdict |
|---|---|---|
| 1 | `items.toml` indices as permanent ids: a merge that reorders rows corrupts every character's gear | **Accepted in part.** Items were never named by index anywhere that lasts (the database and `ItemSummary` carry the key); the draft's "append-only" rule for items was unnecessary and is gone (3). Abilities stay by place (an older rule). Indices live only inside one zone session (6) |
| 4 | An older client indexing a longer template list panics | **Accepted.** Every manifest entry carries its key, the client resolves by key, every index is bounds-checked (5.2, 6) |
| 5 | A hub newer than a zone sends an index the zone cannot hold | **Accepted.** The hub sends template keys (6, LOOK.md 6.2) |
| 9 | A floating-point rasteriser is not bit-identical across CPUs | **Accepted in part.** It is, for IEEE basics without libm (SOUND.md 2 proved it between x86-64 and wasm32); 5.3 now states the rule and pins the hashes |
| 10 | zlib output varies across platforms, the byte-for-byte check fails | **Rejected.** `miniz_oxide` is pure Rust at a pinned version; the hub's verification of uploads already relies on canonical re-encoding (MODELS.md 5). Written down in 5.4 |
| 11 | A client missing a bundle prop asks the hub for its hash, with retries | **Accepted.** The bundle is a second source of the model cache; the hub is never asked for official content (6) |
| 14 | `bitcode` for the manifest costs wasm | **Rejected.** It is the wire's codec and already linked (PLAN.md 11.2) |

The findings on LOOK.md are in its 11.1.

### 12.2 Code review (Gemini 3.1 Pro, 2026-10-03, over the phase's diff)

Five findings on the servers and the pipeline (LOOK.md 11.2 has the client's seven):

| # | Finding | Verdict |
|---|---|---|
| 1 | `read_kind` returns a `String` where a `Vec<String>` is wanted | **Rejected.** It compiles: the line is inside the primitive's closure, whose error is a string |
| 2 | The zone measured a body's look *after* writing the reading's templates, so a change never differed and `FromZone::Look` never went out | **Accepted, a real defect**: the look is read before the slot is written. Nobody saw a weapon put on or taken off until they rejoined |
| 3 | `images[0]` on an upload without a textured primitive | **Rejected** (every path to it has a textured primitive, or returned a violation), and hardened anyway: `images.first()` |
| 4 | `pack` of a `.gltf` with images but no `bufferViews` panics | **Accepted.** The array is made when absent |
| 5 | A glyph wider or taller than 255 is truncated to a byte on encode | **Accepted.** `validate` refuses it (the rasteriser clamps to 255 already) |

### 12.3 Found by building it

- A `.gltf` from a pack has its buffers and its texture as files beside it; the standard
  takes one `.glb`, so `import` packs them (buffer views moved into one BIN chunk, images
  as views). KayKit's four weapons came in that way; their 1024² palette texture is
  resampled to the prop's 256 (palette cells are large: nothing is lost).
- A face's glyphs are stored with the coverage the rasteriser gives as alpha, not cut to
  bits. (v1 added: "a pixel face at its design size is crisp either way, and a drawn face
  is smooth at every whole scale". Neither was true on a screen: both were magnified with
  nearest sampling, and the director saw blocks. The faces are rasterised per density
  now, LOOK.md 2.2, 2.3 and 11.4.)
- The small five-by-seven font moved from the client into `gm_model::smallfont` so that
  the tool writes the same dots into the atlas as face 0 and the client's fallback atlas is
  made of them; nothing in the client draws text any other way than through an `Atlas`.
- `det::pow` (the sound's deterministic arithmetic, now `gm_model::det`) replaced `powf`
  in the texture encoder's sRGB curves, so an ingested `.gmm` is the same bytes on every
  machine, which the byte-for-byte check of 5.4 needs; the baked icons use integer
  averaging and texel fetches on top of the rasteriser's IEEE basics.

## 13. What was measured (2026-10-03, the reference machine)

- The bundle: **114,614 bytes** in 8 files; the atlas 512 × 176, **58,985 bytes**
  (33 pieces, 3 faces of 105 glyphs each, 22 icons: 6 baked from props, 16 portraits of the
  mannequin); props 2,240 (hammer) to 15,830 (crossbow) bytes, 48 to 584 triangles.
  With an atlas per density (LOOK.md 11.4, the same evening): **713,302 bytes** in 11
  files, the atlases 49,641 / 123,997 / 200,633 / 282,595 bytes; the build takes about
  three seconds. With the faces' generic character set (LOOK.md 13.12, 2026-10-08; 308
  characters a face instead of 105): **1,153,054 bytes**, the atlases 82,847 / 207,092 /
  332,624 / 470,976 bytes.
- `gm-tools content build`: the sources to the bundle in about a second in release; the
  rebuild in CI compares hash for hash and found no difference between runs.
- Props drawn: 48 avatars in the town, every one holding the sword, cost **0.006 ms a
  frame** on the Radeon iGPU over the same crowd bare (1.336 → 1.342 ms; 745 fps), and
  0.20 ms on the software GPU (39.16 → 39.36 ms).
