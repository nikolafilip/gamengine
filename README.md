# gamengine

A lightweight Rust engine and a persistent action-sandbox MMORPG with two
viewports (first-person, third-person) running on one shared
simulation. Measured in megabytes, built for integrated GPUs, open source.

**Read [`PLAN.md`](PLAN.md) first.** It is the source of truth: thesis and
non-negotiables, reviewed design findings, engine architecture, economy,
anti-cheat, and the phased implementation plan with acceptance criteria.

## Layout

```
Cargo.toml          workspace (crates/*)
crates/gm-core      shared simulation: entity vocabulary, matrix, builds, statuses, movement, fixed tick. No I/O.
crates/gm-content   content loader: abilities, builds, creatures, trials and items in TOML, compiled and validated into gm-core
                    packs; what a made item does, in numbers and in words.
crates/gm-bsp       Quake BSP loader: geometry, lightmaps, PVS, hull tracing.
crates/gm-net       wire protocol: bit packing, delta snapshots, inputs, quinn transport, client prediction.
crates/gm-client    wgpu forward renderer, skinned characters, model cache, Quake movement, zone connection,
                    the screens (login, characters, menu, chat, inventory, storage, a stall) on a toolkit of its own;
                    the same crate is the browser client (wasm, WebGPU or WebGL2, WebTransport).
crates/gm-server    authoritative tokio zone server: tick loop, sessions, PVS snapshots, lag compensation.
crates/gm-hub       accounts, characters, zone registry, handoff, the economy, avatar models and their moderation.
crates/gm-hub-proto hub messages, entry tokens and the hub connection used by zones, bots and the client.
crates/gm-model     avatar models: the standard rig, the .gmm container, the shared animation set, the mannequin.
crates/gm-ingest    model ingestion: a glTF upload validated against budgets and the frame envelope, re-encoded.
crates/gm-replay    replays: the .gmr file a zone records, its playback, the aim statistics computed from its frames.
crates/gm-ai        minds: companions, creatures, the nav grid, encounters with their ledger, loot split and trial verdicts.
crates/gm-tools     CLI: map build and generators, WAD generation, budget lint, model and moderation tools, the
                    content pipeline (gm-tools content check|build|report|import, docs/CONTENT.md 5).
crates/gm-bot       headless bots: the client's prediction code with scripted behaviour, for tests and load.
assets/maps/src     TrenchBroom .map sources and gamengine.fgd (test_room, the 8v8 arena, the town, the tutorial dungeon)
assets/maps/built   compiled .bsp (+ .lit colored lightmaps)
assets/textures     generated palette and WAD (gm-tools wad make)
assets/content      abilities, preset builds, creatures, trials and items (TOML) with their looks: prop
                    models (models/props, CC0 and ours), icons, the skin and two fonts (ui/), LICENSES.md
assets/built/content  the bundle the client loads (manifest.gmc, ui*.gma, props/*.gmm), written by
                    gm-tools content build and verified byte for byte by scripts/check-look.sh
web/                the browser client's page and loader (index.html, boot.js)
docs/               VOCABULARY.md, PROTOCOL.md, MATRIX.md, HUB.md, ECONOMY.md, MODELS.md, COMPANIONS.md, WEB.md,
                    ANTICHEAT.md, CLIENT.md, ITEMS.md, PARTY.md, SOUND.md, CONTENT.md, LOOK.md, BUILDING.md
ci/baselines        binary-size baseline for the regression gate
scripts/            CI gates and tool fetching
budgets.toml        every number CI enforces
```

## Quick start

```sh
rustup component add clippy rustfmt
scripts/fetch-ericw-tools.sh                       # pinned qbsp/vis/light into tools/
cargo run -p gm-tools -- wad make                  # regenerate assets/textures/base.wad
cargo run -p gm-tools -- map build assets/maps/src/test_room.map
cargo run --release -p gm-client -- --map assets/maps/built/test_room.bsp
```

Controls: mouse look, `WASD`, `Space` jump, `Esc` releases the cursor (click to grab again),
`Q` quits. Benchmark: `--bench 600` prints frame statistics, peak RSS and binary size, then
exits. `--headless` renders offscreen (CI runs it under software Vulkan). All flags are listed
in [`docs/BUILDING.md`](docs/BUILDING.md).

Multiplayer (Phases 2 and 3): start a zone on the arena, add bots, join it.

```sh
cargo run --release -p gm-server -- --map assets/maps/built/arena.bsp --listen 127.0.0.1:4433 --cert-out zone-cert.der
cargo run --release -p gm-bot -- --connect 127.0.0.1:4433 --cert zone-cert.der --map assets/maps/built/arena.bsp \
    --bots 15 --secs 60 --behaviour duelist --builds ironclad,blade,frostweaver,shade --teams 1,2 --counter-pick
cargo run --release -p gm-client -- --map assets/maps/built/arena.bsp --connect 127.0.0.1:4433 --cert zone-cert.der \
    --build blade --team 1 --third-person
```

Left click primary, right click secondary, Ctrl guard, 1–4 actives, V switches viewport,
F1–F4 re-spec to a preset at the next respawn. The wire protocol, prediction and
lag-compensation contract is [`docs/PROTOCOL.md`](docs/PROTOCOL.md); the point-buy budget,
the type matrix and the damage pipeline are [`docs/MATRIX.md`](docs/MATRIX.md).

Persistent play (Phase 4): a hub with accounts and characters hands out tickets to zones and
moves characters between them ([`docs/HUB.md`](docs/HUB.md), setup in
[`docs/BUILDING.md`](docs/BUILDING.md)). The economy (Phase 5) is
[`docs/ECONOMY.md`](docs/ECONOMY.md): every coin and item movement is one database transaction.

Custom avatars and the town (Phase 6): players upload a glTF model for their frame; the hub
validates it in a sandboxed worker, re-encodes it, queues it for moderation and serves it by
hash; clients cache it under a hard disk cap and draw the frame's mannequin until it arrives
([`docs/MODELS.md`](docs/MODELS.md)). Try one without a hub:

```sh
cargo run --release -p gm-tools -- model template --frame striker --out striker.glb
cargo run --release -p gm-tools -- model ingest --frame striker striker.glb
cargo run --release -p gm-client -- --map assets/maps/built/town.bsp --third-person --avatar striker.gmm
```

Companions, command and the tutorial dungeon (Phase 7): a player leads a squad of three
(hired avatars of other players, or recruits a tutorial zone lends) that follows and fights
on its own, and takes it through a gate and a boss; the kill drops its components through the hub and a
role trial is recorded ([`docs/COMPANIONS.md`](docs/COMPANIONS.md)). Companions and creatures
are the same body a player has, driven by minds that send the same inputs.

```sh
cargo run --release -p gm-server -- --map assets/maps/built/dungeon.bsp --cert-out zone-cert.der \
    --squads --recruits ironclad,mender,frostweaver
cargo run --release -p gm-client -- --map assets/maps/built/dungeon.bsp --connect 127.0.0.1:4433 \
    --cert zone-cert.der --build blade --third-person
```

The browser (Phase 8): the same client compiled to wasm joins the same zones through a
WebTransport listener the zone and the hub open beside their QUIC endpoint
([`docs/WEB.md`](docs/WEB.md)). 0.97 MB of `.wasm` on WebGPU (0.32 MB compressed), 2.96 MB
on WebGL2; the loader picks.

```sh
scripts/build-web.sh
cargo run --release -p gm-server -- --map assets/maps/built/arena.bsp --cert-out zone-cert.der \
    --web-listen 127.0.0.1:4434 --web-info-out zone-web.json
(cd target/web && python3 -m http.server 8080 --bind 127.0.0.1)
# http://localhost:8080/?connect=https://127.0.0.1:4434&cert=<cert_sha256 from zone-web.json>&map=arena&build=blade
```

Fair play (Phase 9): a zone records every fight between players and every report as a
replay, computes each client's aim statistics from the same frames (where its view pointed
around each shot, judged against the world as it saw it), and reports both to the hub, which
keeps a reputation ledger, flags accounts whose numbers are not a hand's, and gives a
moderator the list, the replays and the verdicts ([`docs/ANTICHEAT.md`](docs/ANTICHEAT.md)).
Statistics rank; people decide.

```sh
cargo run --release -p gm-server -- --map assets/maps/built/arena.bsp --cert-out zone-cert.der --replay-dir replays
cargo run --release -p gm-tools -- replay aim replays/arena-*.gmr
cargo run --release -p gm-client -- --replay replays/arena-*.gmr --follow NAME
```

The screens (Phase 10): a client that knows where its hub is needs no command line. It
shows a login screen, the account's characters, a screen to make one from the archetypes,
and in the game a menu (`Escape`) and a chat line (`Enter`); a refusal anywhere is said in
words, and a connection that ends leads back to the screen before it
([`docs/CLIENT.md`](docs/CLIENT.md)). The command line that named everything still works:
it is the same screens with nobody clicking.

```sh
cargo run --release -p gm-client -- --hub 127.0.0.1:4400 --hub-cert hub-cert.der    # or name the hub in the settings once
```

Possessions (Phase 11): `I` opens the inventory and `E` the stall the body stands at. A
character wears a weapon and an armour; what is worn moves the zone's damage, each item for
its own kinds and a place for half of what its edge says (the best of both wins an exchange
by 23% over nothing). A stall is looked at from anywhere and bought from standing at it;
what is worn changes through the zone, at once, and not in a fight
([`docs/ITEMS.md`](docs/ITEMS.md)). An operator hands out coin and items and audits the
books with `gm-hub --grant-coin`, `--grant-item`, `--place`, `--audit`.

## CI gates

`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, then:

- `scripts/check-budgets.sh` — every asset in `assets/` within `budgets.toml`;
  `--self-test` proves an over-budget asset fails.
- `scripts/check-binary-size.sh` — release `gm-client` under the absolute cap
  and within 1 MiB of `ci/baselines/gm-client-size`; `--self-test` proves a
  regression fails.
- `scripts/check-perf.sh` — headless render under software Vulkan, RSS under
  the ceiling. Frame-time on a real iGPU needs the self-hosted runner.
- `scripts/check-netcode.sh` — 16 bots on one zone in `turmoil` at 150 ms round
  trip and 3% loss: under 30 KB/s per player each way, no unexplained prediction
  corrections, melee and projectiles register; plus a real-UDP loopback test and
  the counter-pick match (blades re-spec into frostweavers and turn the match).
- `scripts/check-matrix.sh` — 8v8 bot matches in the arena: ironclads beat blades,
  frostweavers beat ironclads, blades beat frostweavers, a mirror match is even.
- `scripts/check-swarm.sh` — 200 duelist bots on one zone over real UDP: server tick
  time, RSS and bytes per player under `budgets.toml`.
- `crates/gm-server/tests/handoff.rs` — login → zone → handoff → logout through the hub
  against Postgres, the character's location checked in the database at every step.
- `scripts/check-avatars.sh` — 100 distinct avatars at the budget ceiling in the town: every
  one drawn, 60 fps on the reference iGPU, RSS under the ceiling, and the model cache
  directory never above its cap. CI runs a 48-avatar smoke under software Vulkan; `--online`
  runs the whole path (uploads, ingestion, moderation, zone, bots, client).
- `scripts/check-dungeon.sh` — one leader and three companions clear the tutorial dungeon:
  offline on eight seeds, over a simulated network, and on a real zone (the clear, the drop,
  the leader's trial, the wire); then sixteen leaders with their squads in one zone against
  the tick and mind budgets. `--online` plays it through the hub: recruits first, then three
  avatars hired with the coin of the first kill, then the zone the trial opens.
- `crates/gm-server/tests/companions.rs` — the same through hub and Postgres in a test:
  three hires paid and burned, the squad by name, the drop and the trial in the database,
  the ledger sound, the gated zone, a hire ended by its avatar's owner.

- `scripts/check-anticheat.sh` — the aim analysis on traces whose answer is known; then a
  recorded arena of twelve bots whose view moves like a hand and four that aim by program:
  every program flagged, no hand flagged, the fight's replay reads back and recomputes to
  the numbers the zone logged, the client renders it from a program's eyes; with `--online`
  through the hub: flags, replays, a report upheld, a ban, a trust-gated zone.
- `scripts/check-web.sh` — the browser client: both `.wasm` under their size budgets, a QUIC
  bot and a WebTransport bot in one zone; with `--browser` headless Chromium plays in an
  arena with fifteen native bots on each build (snapshots, corrections, damage both ways,
  bytes, memory, frame rate); with `--hub` it logs in, fills its model cache under a cap
  smaller than the town's avatars, comes back to find them cached, and travels.
- `scripts/check-screens.sh` — the client's screens: every screen whole at five window
  sizes and every refusal in words, as tests; with `--desktop` the windowed client on a
  display of its own, by UI script (a new account, a character, the town, chat with a bot,
  the menu, two zones both ways, rounds of Play and Leave) and then by real keys and clicks
  (the password pasted, Enter, a click on Play, Escape, a click on Quit); with `--browser`
  both builds in headless Chromium, the page's form filled by the browser's own input.
- `scripts/check-items.sh` — possessions: the gear term in the pipeline and in a simulated
  fight, the item content and its words, the inventory and stall screens at five window
  sizes; with a database the hub (worn items refused by everything that moves or destroys,
  a storm of transactions with wearing in it) and a zone with clients driven by hand (the
  counter, the fight lock, the zone's hits before and after); with `--desktop` a bot keeps a
  stall and a new character, given coin and stood at the counter by an operator, buys a
  sword and wears it through the windowed client, by UI script and then by real keys; with
  `--browser` the same purchase in both browser builds.
- `scripts/check-party.sh` — people together: the split with people in it, the wire's two
  directions, the people, trade and tavern screens at five window sizes; with a database
  the hub's parties (invitations, numbered readings whatever races, the sweep) and two
  zones with clients driven by hand (a party across zones, a trade asked standing together,
  a fight whose roster is closed); with `--online` two bots form a party in the town and
  clear the tutorial dungeon together, and the hub splits what it drops; with `--desktop` a
  new character asks one of them into a party, says a party's line and a whisper, buys
  what the other looted through the trade window and hires an avatar in the tavern, by UI
  script; with `--browser` the same in both browser builds.
- `scripts/check-sound.sh` — sound: the patches (pinned by hash), the mixer (no allocation
  in its callback, no click when a voice is stolen) and every rule of what is heard as
  tests; then the client on an Xvfb of its own rendering its sound into a WAV from the frame
  clock (no device needed): an offline walk read for silence while standing and steps while
  running, the arena with fifteen bots read for swings, blows and launches; what the phase
  added to the native binary; with `--browser` both builds in headless Chromium: the audio
  context running after the gate's click, cues started, the patches hashing to the native
  build's bytes, what the phase added to the wasm.

See [`docs/BUILDING.md`](docs/BUILDING.md).

## License

Split: GPLv3 client/engine, AGPLv3 server, CC BY-SA 4.0 content. See
[`LICENSE.md`](LICENSE.md).
