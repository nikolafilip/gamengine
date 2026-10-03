# The look: props in hands, the skin, the screens as grids, the hotbar (v1)

Phase 14 (PLAN.md 11.8), second half; CONTENT.md is the first. The game plays but looks
like its test harness: a grey mannequin with empty hands, panels of flat colour, lists of
words in a five-by-seven font, three bars and no sign of what a body can do or when it can
do it again. This is the contract for what changes: a weapon drawn in the hand of whoever
wears or swings one, a toolkit that draws pictures, the screens of ITEMS.md 6 and PARTY.md 8
redone as grids of icons with tooltips and drag, and a HUD with a portrait, the party and a
hotbar whose cells show their cooldowns — the menus of Ether Saga Odyssey, measured in
kilobytes.

Code follows this document; a change to either goes in one commit. v0 was the proposal
(2026-10-03, reviewed in 11.1); the director's decisions are in 9; v1 is what Phase 14
built (11.2: found by running it; 12: measured).

## 1. Principles

1. **Everything the toolkit draws is still quads into one atlas, in one draw call.** The
   HUD of Phase 7 draws rectangles and glyphs as quads into a font atlas (`hud.rs`). The
   atlas becomes RGBA and holds the skin, the icons and two real fonts beside the old
   glyphs (CONTENT.md 5.2); a picture is a quad with other texture coordinates. No new
   pipeline, no second texture, no text rasteriser in the client.
2. **Never wait for art** (CONTENT.md 1.4). Until the atlas is loaded, and on a machine
   whose bundle is missing, the toolkit draws exactly what it draws today. Every piece of
   the skin has a plain fallback (a plate, a line, a glyph); a missing icon is a glyph.
3. **The rules of the screens do not change** (ITEMS.md 6): a thing is picked by what it
   is, nothing is picked once it is gone, nothing is said the hub did not say, a screen
   takes only the answer it waits for. Icons, tooltips and drag are new ways of pointing at
   a thing and acting on it, not new rules. Every action a drag does, a button does too, so
   a UI script and a keyboard reach everything.
4. **The sim and the wire carry keys, never looks** (CONTENT.md 1.5). A body's `Look` is
   two indices; the client finds the files.
5. **Measured.** The hotbar and the panels cost a fraction of a frame on the iGPU; a prop is
   one more draw call for a body that holds something; the browser build stays under its
   megabyte or the director raises it knowingly (9).

## 2. The toolkit, second version (`ui.rs`, `hud.rs`, `font.rs`)

### 2.1 Primitives

`Canvas` keeps `size`, `rect` and `text` and gains:

| Call | Draws |
|---|---|
| `image(rect, piece, tint)` | a piece of the atlas stretched to `rect`, times `tint` (white = as painted) |
| `frame(rect, piece)` | a **nine-slice**: the piece's corners unscaled, its edges stretched one way, its middle both ways; insets come from the atlas's piece table |
| `wedge(centre, radius, from, to, colour)` | a filled circular sector in sixteenths of a turn, as a fan of triangles: the cooldown sweep |
| `layer(n)` | everything after this goes to layer `n` (0 plates and wells, 1 bodies drawn into the screen (5), 2 pictures and text, 3 the tooltip and the dragged icon, 4 the cursor, unused); the HUD issues one draw per layer in order. The game's own HUD (3) stays on layer 0 in call order, so a screen's plates cover it as they covered the bars before there were layers |
| `text` with a `font` | `Font::Small` (the five-by-seven of today), `Font::Text` (the atlas's 12-dot proportional face), `Font::Title` (16 dots); a face not in the atlas falls back to `Small` |

An icon drawn in a slot or a portrait is recorded as `Seen { kind: Image, text: <icon key>
}` (`item/sword`, `portrait/striker_mail`), so a test or a script asks `expect image
item/sword` the way it asks for a label.

### 2.2 The atlas (`ui.gma`)

Written by `gm-tools content build` (CONTENT.md 5.2), read in one call with `miniz_oxide`
(already linked): `GMA1`, `w`, `h` (each ≤ 1024), the **piece table** (name hash, rect,
nine-slice insets), the **glyph tables** (per face: char → cell, advance, bearing, the line
height), the **icon table** (key hash → cell), then the RGBA8 texels, one zlib stream whose
inflation is bounded by `w × h × 4` (4 MiB at most; the atlas has its own reader and its own
bound, not the `.gmm` reader's 2 MiB). The five-by-seven glyphs of `font.rs` are copied
into it too, so the client draws with one texture whether the atlas loaded or not: the
HUD's pipeline samples RGBA8 from now on, and the **built-in fallback atlas is made as RGBA8
at start** from the same glyph bitmaps (white, the dot as alpha).

Pieces the toolkit asks for, by name (`skin.toml` maps each to a picture and its insets):

`panel` `panel_title` `well` `button` `button_hot` `button_down` `button_off` `field`
`field_focus` `slot` `slot_hot` `slot_picked` `slot_worn` `slot_off` `bar_frame` `bar_fill`
`portrait_frame` `hotbar_cell` `hotbar_key` `tooltip` `check_off` `check_on` `slider_rail`
`slider_knob` `scroll_rail` `scroll_knob` `cursor` `cursor_drag` `coin_gold` `coin_silver`
`mark_new` `mark_taken` `mark_worn`

A missing piece draws its fallback (a plate or a line of today's colours). A tint is the
colour the old toolkit used (the skin may be greyscale and tinted, or painted).

### 2.3 Fonts

Two faces rasterised by the tool from font files in `assets/content/ui/` (OFL or CC0 only,
named in LICENSES.md) at a fixed dot size, hinted to whole dots, no antialiasing beyond the
coverage the rasteriser gives: `Text` at 12 dots (ASCII, Gaj's letters, the few marks the
chat uses), `Title` at 16. The client knows nothing of TrueType. The scale rule stays
(CLIENT.md 3: whole dots, 1–4 by the frame or the setting); layouts count in dots as now.

### 2.4 Widgets

Kept: label, button, field, list, check, slider, paragraph. New:

- **Grid** of **slots**: `n` columns of 36-dot cells (a 32-dot icon with a 2-dot rim); a
  slot holds a thing by id (an item, a listing, an offer), its icon, a corner mark (`worn`,
  `new`, `taken back`, a stack count later) and, under a stall's cell, its price. Hover
  shows the tooltip; a press picks; a press that moves four dots with the button down starts
  a **drag**. A grid whose things do not fit is scrolled in rows: by the wheel and the keys
  as lists are today (CLIENT.md 3), and by a **scrollbar** at its right edge (`scroll_rail`,
  `scroll_knob`: a drag on the knob, a click on the rail), which lists get too, so a
  machine without a wheel reaches everything.
- **Tooltip**: after 150 ms of hover over a slot, the item in full as 6.1 of ITEMS.md puts
  under the list today (what it is, its budget, everything it does, what is worn in its
  place, what it is made of), in a `tooltip` frame beside the pointer, inside the frame.
  Over a hotbar cell: the ability's name, cost, cooldown and what it does (the pack's
  words). Over a party frame: the member's name and state.
- **Drag**: the icon follows the pointer on layer 3 (`cursor_drag`); a **drop target** is
  any slot or panel that says it takes the thing (the weapon slot takes a weapon, the trade
  pane takes an unworn item, the storage grid takes an item, the stall grid at one's own
  stall opens the price page); a drop elsewhere is nothing. The drop does exactly what the
  button for it does (`Wear`, `Offer`, `Store`, `Take`, `Sell`), through the same code,
  with the same gate (one change a second, ITEMS.md 5).
- **Paperdoll**: a rect that a body is drawn into (5): the own frame, armour tint or model,
  the held prop, idle, turning as the pointer drags across it.
- **Portrait**: a 32-dot icon from the manifest (CONTENT.md 5.3: a frame's mannequin or
  official avatar, head and shoulders) in a `portrait_frame`.
- **Bar** with a frame piece and a tinted fill, numbers inside.

UI scripts (CLIENT.md 9) gain `hover NAME` (the pointer there), `drag NAME TARGET` (a
press on the first, the pointer moved a quarter of the way a frame, let go on the second:
real input is still the gate's xdotool) and `expect image KEY`.

A grid draws every slot of its holder (24 for the inventory, 60 for the storage, 12 for a
stall, six and twelve in a trade), the empty ones as wells. A cell is found by a script as
its row was (`sword  slash +2.0%  worn`; a listing adds its price), and the `Seen` of the
picked thing's words under the grid stays, so every script and test of Phases 11 and 12
runs unchanged.

## 3. The HUD in the game

### 3.1 Top left: the own body

A `portrait_frame` with the own portrait, the name, and the three bars (health, stamina,
focus) in `bar_frame`s with numbers, as today's bars but framed; aspects as the two element
colours on the frame's rim; the armour class as the frame's metal. Under it, **party
frames**: one per other member present (PARTY.md 8; health is on the wire for party
members, PROTOCOL.md 5), a portrait, the name, a health bar, dimmed when away or dead; at
most four.

### 3.2 Bottom centre: the hotbar

One cell per ability of the kit in the order a hand finds them: `LMB` primary, `RMB`
secondary, `C` guard, `1`–`4` the actives (Shift is also 1: `sim_input`); `app::hotbar`
makes the cells, and `--report` prints them (`hotbar=LMB:sword:ready:1.00,...`: key,
ability, state, how ready). Each cell: the
ability's icon (CONTENT.md 3), its key in a `hotbar_key` tab, and its state, read from the
own predicted mover (`Mover::cooldowns[slot]` against the predicted tick, the ability's
`cooldown.ticks`, its `cost`, the own stamina and focus, `statuses.silenced()` for an
elemental):

- **ready**: the icon as painted;
- **cooling**: a dark `wedge` sweeping clockwise from twelve as the cooldown runs out, the
  seconds left in the cell when more than one;
- **unaffordable** (cost above the resource): the icon dimmed and the resource's bar
  flashes once on a press;
- **silenced**: a slash across an elemental's icon;
- **active** (its script runs, or the guard is held): a bright rim.

A cell is a button too: a click fires the ability as the key would, so a tablet has a way.
Nothing here is a rule: the mover decides, the cell only shows what it will decide.

### 3.3 Above the hotbar: statuses

The own statuses (the own block of every snapshot) as 24-dot icons with a ring of the time
left and the stack count; in the pack's status order; harmful ones with a red rim, helpful
with green.

### 3.4 Glyphs

An ability or status without a picture is drawn by the client as its name's first
letters in the cell (`sword`, `fireb`, `parry`; a status's first four), in the small face:
plain on purpose, so that a missing picture is seen and fixed, but a new ability has a
cell the moment it exists. (v0 proposed glyphs by verb kind; the letters say more for
less, and the pictures are the plan: CONTENT.md 8.) An item with a model gets its icon
baked; one without (an armour, today) shows its first two letters in the title face.

### 3.5 Kept

The corner hints (the zone, `Esc menu`, `E look`), the crosshair, the tactical viewport's
own drawing (COMPANIONS.md 6), the chat (CLIENT.md 5): skinned where they have a frame,
otherwise as they are.

## 4. The screens as grids

Every page keeps its name, its buttons and its words (so every UI script of the gates still
runs), and shows its things in grids instead of rows:

| Page | Grid(s) | Besides |
|---|---|---|
| inventory | the 24 slots, 6 × 4 | the purse as two coins with numbers (gold, silver: PLAN.md 5.1 as corrected); the **equip** panel beside the grid: the paperdoll (5) with the weapon and armour slots at its side; the picked thing's words under them (three lines) and the tooltip on hover; **Wear**/**Take off**, **Sell**, **Store**, **Storage**, **Close**; a drag onto the weapon or armour slot wears, a worn thing dragged into the grid comes off |
| storage | the 60 slots, 8 across, scrolled | **Take**, **Back** (a drag between storage and inventory is Phase 15's: the two are pages, not one screen) |
| stall | the keeper's 12 listings, 6 × 2, the price under each cell | the buyer's purse, the picked thing's words and price, **Buy** / **Take back**; a listing is picked, never dragged |
| price | two fields: gold, silver | **List it**, the whole said back in words |
| trade | what you give (six slots) and what they give (six), each with its coin and `accepted` on its header line, marks on their cells (`new`, `taken back`, a dim struck cell for what is gone), the carried things in two rows of six under them | a drag from the carried into your offer offers, one back takes back; **Offer**, **Take back**, **Set coin**, **Accept**, **Cancel**; the 3 s lock and the `changed` word exactly as PARTY.md 6 |
| tavern, people | rows, as PARTY.md 8 (portraits in rows are Phase 15's) | skinned |
| menu, settings, login, characters, new character | skinned panels and widgets | as CLIENT.md 4 |

The tallest panel is now the trade's: `ui::PANEL_HIGH` is 360 units (300 before), so a
window of 720 lines holds scale 2 exactly and one of 600 falls to scale 1 (CLIENT.md 3).

A grid cell is found by `ui.find` by the words its row had, so `click "sword  slash
+2.0%"` still picks it, and the equip panel's slots are found as `weapon: sword` and
`armour: nothing`.

## 5. Bodies drawn into a screen

The paperdoll is the game's own character renderer drawing into a rectangle of the
screen: after the world's pass and the HUD's layer 0, **a second render pass** with the
colour attachment loaded and **the depth cleared** draws the own body (its model or
mannequin, its armour tint, what it holds, idle, lit flat) with `set_viewport` and
`set_scissor` to the panel's rect and a camera of its own (`Renderer::render_with_doll`:
eye 82 units in front at chest height, looking back; the body turned by a drag across
the well); the viewport and the scissor are **set back to the whole frame** before the
HUD's layers 1–4 close the frame. The pass exists only in frames that have a paperdoll,
so the game's frame cost does not move. The doll's draws ride in the same block buffer as
the world's (`Characters::prepare_with_dolls`), after them.

## 6. Props in hands

### 6.1 What is held

A body holds **one prop**, chosen by the zone and sent as an index (4): the `model` of the
weapon it wears (ITEMS.md 2) if it wears one, else the `prop` of its primary ability
(CONTENT.md 3), else nothing. Companions and creatures hold their primary's prop (a hired
avatar wears no gear yet: ITEMS.md 9). A stall keeper holds nothing in v1.

### 6.2 On the wire (protocol v8, hub v1.9)

- The pack (`FromZone::Content`) carries **`props: Vec<String>`**: the keys of every
  `ability.prop` and `template.model` in the content, in order of first appearance. A zone
  reads `items.toml` for these names (it reads the abilities already); it never reads a
  model. The list and the indices into it are **of that session**: a client holds the pack
  the zone sent and every `Look` it gets from that zone indexes that pack (a zone restarts
  to change content, and its clients reconnect and get the new pack).
- `PlayerEntry` (the `Players` message) gains **`look: Look { held: u16, worn: u16 }`**:
  indices into `props` (`NONE = u16::MAX`; an index past the list is read as `NONE`);
  `worn` is always `NONE` in Phase 14 and is the armour overlay of Phase 15.
  **`FromZone::Look { id, look }`**, appended at the end of the enum (ITEMS.md's lesson),
  when a body's look changes.
- `GearReading` (hub → zone) gains **`templates: [String; 2]`**, the **keys** of the
  templates worn by place (empty when nothing is worn): a hub and a zone on different
  content versions then disagree about nothing but what to draw; the zone maps a key it
  knows to its `model` key, then to the prop index, and one it does not know to an empty
  hand. One more reading when wear changes is one `Look` to everybody who has the body
  (nothing on the snapshot).
- Companions and creatures: their `Look` is computed by the zone from their build.

### 6.3 Drawing

A prop `.gmm` (CONTENT.md 4) has every vertex on bone 0 with weight 255. It is drawn by the
character pipeline unchanged: one more `CharacterDraw` with a uniform block of the usual
layout (24 matrices, scale, tint, light: MODELS.md 9) in which matrix 0 is **the wearer's
skinning matrix of `prop_r` times the translation to that bone's pivot** (MODELS.md 2:
`prop_r` follows `hand_r`) and the other twenty-three are unused, the prop's own position
scale, the material `tint` of the item's core (CONTENT.md 3; white for a bare prop) and the
wearer's light. A custom avatar that carries a `prop_r` bone places the grip where its hand is; one
that does not gets the frame's pivot. The prop is in the model cache like any model, keyed by
the manifest's hash for its file; a prop not yet loaded is not drawn (never a stand-in: an
empty hand is correct for a moment).

The animation set gains nothing in v1: the swing, the windup and the cast move the arm as
they do, and the prop follows the hand. (The crossbow's bolt already leaves `spawn.weapon`
= 16 u ahead of the hand, VOCABULARY.md; the musket's the same.)

### 6.4 The view model (first person)

Counter-Strike's root (the director, 2026-10-03): in the FPS viewport the own body is not
drawn, and the held prop is drawn as a **view model**: the same `.gmm`, placed in view
space (`Avatars::view_model`: 14 units out, 7 to the right, 6 under the eye, its business
end along the look, turned 8° inward and tipped 4° up), with a bob read from the own
travel (a figure of eight a stride of 64 units long) and a kick on a launch or a swing
(the own predicted actions: back 3 units and up 14°, decaying over a sixth of a second).
It is drawn in the world pass with the world's depth: a weapon can clip into a wall one is
pressed against (the near plane is 4 units); a pass of its own is written down for Phase
15 with `fit_view` per template, which the manifest already carries. Third person shows
the prop in the hand as everybody sees it; the tactical view shows nothing of it.

## 7. The purse: silver and gold

The director dropped copper (2026-10-03). The ledger's integer is **silver**; **100 silver =
1 gold**; every number in the database, the wire and the tests keeps its value (what was
30 copper is 30 silver). No migration divides anything: there is no live economy, the only
databases are test ones and the director's play stack, which are wiped (`start --fresh`);
a database from before Phase 14 is not carried over, and HUB.md says so. ECONOMY.md 2 and
12 are rewritten to the director's scale (2026-10-03: "top gear tens of gold; fully crafted
with top stones, near a hundred"): a meal 1 s, standard gear 20–50 s, a boss component
1–5 g, a top item 10–30 g, one fully crafted with a boss shard and top gems 60–100 g, a
12 h hire 50 s – 2 g, a carry 2–10 g; the cap on a single coin grant 5 g. ("Infuse and
imbue with stones" is, in this economy, the deterministic crafting of ECONOMY.md 4 and the
two gem sockets: no casino, PLAN.md 0.) The screens show two coins (`coin_gold`,
`coin_silver`) with numbers, gold in threes; the price fields are two. `--grant-coin`,
`--sell-at`, `--trade-for`, `--list-for-hire` take silver. PLAN.md 0 and 5.1 are corrected.

## 8. Budgets and acceptance (PLAN.md 11.8 Phase 14)

`budgets.toml` `[content]` and `[look]`:

| Number | Proposed | Why |
|---|---|---|
| `max_bundle_bytes` | 2 MiB | the whole of `assets/built/content/`: what a browser may have to fetch over a session |
| `max_atlas_bytes` | 262,144 | `ui.gma` as fetched before the first screen (the wasm is 345 KB packed) |
| `max_prop_gmm_bytes` | 131,072 | CONTENT.md 4 |
| `max_webgpu_wasm_bytes` (WEB.md 9) | 2,097,152 | raised from 1 MiB by the director (9); the phase reports what it added |
| `max_native_added_bytes` | 262,144 | the native client |
| `max_page_ms` | 0.8 | inventory with the paperdoll, a frame on the iGPU (today 0.36–0.47 ms for a page) |
| `max_hud_ms` | 0.15 | the hotbar, statuses, portrait and party frames, a frame on the iGPU |
| `max_prop_draw_ms` | 0.5 | 100 bodies holding props in the town over the same scene without (the avatars gate's scene) |
| `min_fps` | 60 | the avatars gate with every body armed |

`scripts/check-look.sh [--desktop] [--browser] [--gate-fps]`: (1) `gm-tools content check
--built`: the tables and their looks, and the bundle rebuilt and compared with the
committed one byte for byte; the bundle's, the atlas's and every prop's bytes against
`[content]`; (2) the unit tests of the format, the ingestion, the baked icons, the looks,
the grids and the scripts; (3) the offline town with 48 synthetic avatars, all holding the
sword, against the same crowd bare: the prop draw cost (`--gate-fps`: on the real GPU, with
the fps budget); (4) `--desktop`: own Xvfb, own hub and town, a walker with a hammer build;
a character made by script, handed a sword and a cuirass; by UI script it opens the
inventory (a grid of pictures: `expect image item/sword`), **drags the sword onto the weapon
slot** and the zone answers `worn`, hovers for a tooltip; its last `--report` line says the
hotbar's first cell is the sword, that two bodies hold something and two props loaded; a
screenshot at every step is kept with `KEEP`; (5) `--browser`: the same by the WebGPU build
in headless Chromium, with the wasm's bytes against WEB.md 9's cap.

## 9. Proposed numbers and open decisions

Proposed: 36-dot cells, 32-dot icons, 24-dot status icons, 150 ms to a tooltip, four dots to
a drag, the paperdoll's camera, four party frames, sixteenths of a turn for the wedge, the
`Text` face at 12 dots and `Title` at 16, every budget in 8.

**Decided by the director, 2026-10-03:**

1. **The browser's megabyte may be doubled or tripled** "as long as it runs smoothly on a
   100 Mbps connection". The WebGPU cap becomes **2 MiB** (`max_webgpu_wasm_bytes`
   2,097,152; packed 786,432): at 100 Mbps two raw megabytes are 0.17 s and the packed
   file under 0.07 s, and the WebGL2 build, which is already 3 MiB, loads today in 142–292
   ms to the first frame on loopback (WEB.md 9). The byte hunt is no longer the gate of the
   phase; the phase still reports what it added. WEB.md 9 and `budgets.toml` carry the new
   number with this reason.
2. **Drag and drop is a must, and keyboard shortcuts stay a must**: every drag has a
   button and a key (principle 3), as proposed.
3. **The purse's scale** (7): top gear is tens of gold, and fully crafted with top
   components it nears a hundred; ECONOMY.md 12's scale is rewritten to that.

Open still (small; the implementation picks and says so): where the weapon rests out of a
fight (v1 holds it always); which two faces (OFL or CC0 pixel faces, named in LICENSES.md);
the party frames' reach (v1: members present in the zone).

## 10. Deliberately absent

Tooltips on the HUD's own cells (the hotbar, the party frames): the HUD has no pointer in
the game; a key that shows the hotbar's words is Phase 15's. Armour drawn on the body
(Phase 16: avatars per frame and class, CONTENT.md 3.2); minimaps
(PLAN.md 0: none, ever); a quest log, an experience bar (no levels); nameplates over every
body (the team pip stays; names on hover are a later call); a cursor theme in the browser
(the page's cursor is the browser's outside pointer lock); animations for holding a prop
(the shared set moves the hand; a real set is later work); dropping an item on the ground
(ECONOMY.md 3 has `ground`, no screen yet); a crafting screen (its grid is this toolkit's,
the phase is later).

## 11. Review log

### 11.1 Design review (Gemini 3.1 Pro, 2026-10-03, over CONTENT.md and this document, v0)

Fourteen findings in all (CONTENT.md 12.1 has the seven on that document). On this one:

| # | Finding | Verdict |
|---|---|---|
| 2 | Reading the stored copper as silver multiplies every balance by 100; migrate by dividing | **Rejected.** No live economy exists; test and play databases are wiped. Written down in 7 so nobody carries a pre-14 database forward |
| 3 | A block of one matrix does not fit a pipeline that binds 24 | **Accepted** (wording): the block is uploaded whole with the hand's matrix at index 0 (6.3, CONTENT.md 3.1) |
| 6 | An RGBA8 pipeline with an R8 fallback atlas fails validation or draws blocks | **Accepted** (wording): the fallback is made as RGBA8 at start (2.2) |
| 7 | A props list "by first appearance" shifts indices when a model changes; old packs draw the wrong weapon | **Rejected.** The pack and the `Look` indices are of one zone session; a zone restarts to change content and its clients get the new pack (6.2). The alternative (template and ability indices on the wire, the client resolves) was weighed and costs the client the worn-or-bare rule for nothing |
| 8 | HUD layers after the paperdoll inherit its viewport and scissor | **Accepted** (wording): set back to the whole frame (5) |
| 12 | A 1024² RGBA8 atlas is 4 MiB and the `.gmm` reader's inflation bound is 2 MiB | **Accepted.** The atlas's reader bounds inflation by `w × h × 4` (2.2) |
| 13 | Grids scroll but there is no scrollbar: no wheel, no reach | **Accepted.** `scroll_rail`/`scroll_knob` on grids and lists (2.2, 2.4) |

### 11.2 Code review (Gemini 3.1 Pro, 2026-10-03, over the phase's diff with this document)

Twelve findings over the two halves of the diff (CONTENT.md 12.3 has the five on the
servers and the pipeline); on the client:

| # | Finding | Verdict |
|---|---|---|
| 1 | `Some(a)` moves the mutable reference before `a.avatars.doll` | **Rejected.** It compiles and runs (a reborrow); the gate's desktop run draws the doll |
| 2 | `drop_slot` starts no drag: a worn thing cannot be dragged off the equip panel | **Accepted.** A press on what the slot holds starts a drag out of it (2.4) |
| 3 | A cuirass dropped on the weapon slot asks the zone to wear it there | **Accepted.** A drop counts only for the slot's place; the zone would have refused it anyway |
| 4 | The browser waits for the manifest and the atlas before the first frame, against "never wait for art" | **Rejected.** They are fetched beside the map, which the page already waits for (61 KB against a 400 KB map); a site without a bundle (404) starts at once with the built-in font. The principle is about a missing or late *model*, which is still drawn as nothing |
| 5 | No tooltips on the hotbar's cells and the party frames (2.4) | **Accepted as a gap**, written down in 10: the HUD is not a `Ui` screen (no pointer in the game) and a tooltip there needs the pointer's own rules; Phase 15 |
| 6 | The seconds left are not drawn on a cooling cell (3.2) | **Accepted.** Drawn when a second or more is left |
| 7 | The HUD's portrait is not recorded as a `Seen` image | **Rejected.** The HUD is not a screen and records nothing; scripts read the hotbar from `--report` (3.2) |

### 11.3 Found by running it

- **The HUD's ink drew over a screen's plates.** The first HUD used the layers as the screens
  do (plates 0, ink 2); a screen's panel on layer 0 is drawn after the HUD's plates but
  before the HUD's ink, so the bars' numbers and the hotbar's keys floated over the
  inventory. The game's HUD now stays on layer 0 in call order (2.1); the layers are the
  screens', for the paperdoll between their plates and their ink.
- **A view model pushed too early.** The online frame decides what is drawn, and the
  avatars' frame begins after it, clearing the draws: the first view model vanished and
  something else was seen in its place. The decision is kept (`ViewModel`) and pushed once
  the frame has begun.
- **Props never arrived in the browser.** The fetches landed in an inbox that nothing
  polled: `Content::poll` runs every frame now, one prop a frame, as the avatars' loader
  does (the desktop reads a prop the moment it is first asked for).
- **A grid picked listings by the wrong id.** A stall's cells took the item's id from the
  thing they were made of; the pick is the listing's (ITEMS.md 6). Nothing could be bought
  until it was.
- **An empty list dropped the pick.** The grid wrote `NONE` back while the hub had not yet
  answered, and the first answer's first thing was no longer picked as a list's first row
  always was; the pick is left alone while the list is unknown or empty.
- A name as long as a name gets, with the dearest coin beside it, did not fit the trade's
  header at the smallest window: the name is shortened on purpose (`Abcdefghijkl.. gives`)
  rather than cut, which the tidiness test refuses.
- The trade window is the tallest panel now: `PANEL_HIGH` 360.

## 12. What was measured (2026-10-03, the reference machine)

- Pages, uncapped, the Radeon iGPU, by `--report` while a UI script holds each (whole
  frames, the town behind): the game with the skinned HUD, portrait and hotbar **0.42 ms**
  (0.36–0.37 before the phase), the inventory with its grid, tooltip and paperdoll pass
  **0.54 ms**, the storage 0.53, the menu 0.61; `[look].max_page_ms` 0.8 and `max_hud_ms`
  0.15 hold.
- Props: 48 synthetic avatars in the town, all armed, **+0.006 ms** a frame over the same
  crowd bare on the iGPU (1.336 → 1.342 ms, 745 fps), +0.20 ms on the software GPU
  (`[look].max_prop_draw_ms` 0.5).
- The gate's desktop run: the inventory open, the sword dragged and worn, the tooltip and
  the report in 12–14 s of a software-GPU client; five screenshots in `KEEP`.
- Sizes: WebGPU wasm **1,104,519 bytes (374,635 packed)**, +95,054 over Phase 13 (WEB.md 9:
  the cap is 2 MiB now); WebGL2 3,094,152 (946,533); the native client **9,747,456 bytes**, +135,856 (`ci/baselines/gm-client-size`
  updated); the bundle 114,614 bytes.
- Tests: 380 in the workspace (367 before); the client's 86 cover the grids, the drag, the
  tooltip, the scale rule and the scripts' new verbs.
