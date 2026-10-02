# gamengine

A lightweight Rust engine and a persistent action-sandbox MMORPG with three
viewports (first-person, third-person, tactical) running on one shared
simulation. Measured in megabytes, built for integrated GPUs, open source.

**Read [`PLAN.md`](PLAN.md) first.** It is the source of truth: thesis and
non-negotiables, reviewed design findings, engine architecture, economy,
anti-cheat, and the phased implementation plan with acceptance criteria.

## Layout

```
Cargo.toml          workspace (crates/*)
crates/gm-core      shared simulation: entity vocabulary, matrix, builds, statuses, movement, fixed tick. No I/O.
crates/gm-content   content loader: abilities and builds in TOML, compiled and validated into gm-core packs.
crates/gm-bsp       Quake BSP loader: geometry, lightmaps, PVS, hull tracing.
crates/gm-net       wire protocol: bit packing, delta snapshots, inputs, quinn transport, client prediction.
crates/gm-client    wgpu forward renderer, skinned characters, model cache, Quake movement, zone connection.
crates/gm-server    authoritative tokio zone server: tick loop, sessions, PVS snapshots, lag compensation.
crates/gm-hub       accounts, characters, zone registry, handoff, the economy, avatar models and their moderation.
crates/gm-hub-proto hub messages, entry tokens and the hub connection used by zones, bots and the client.
crates/gm-model     avatar models: the standard rig, the .gmm container, the shared animation set, the mannequin.
crates/gm-ingest    model ingestion: a glTF upload validated against budgets and the frame envelope, re-encoded.
crates/gm-ai        AI companions (Phase 7).
crates/gm-tools     CLI: map build and generators, WAD generation, budget lint, model and moderation tools.
crates/gm-bot       headless bots: the client's prediction code with scripted behaviour, for tests and load.
assets/maps/src     TrenchBroom .map sources and gamengine.fgd (test_room, the 8v8 arena, the town)
assets/maps/built   compiled .bsp (+ .lit colored lightmaps)
assets/textures     generated palette and WAD (gm-tools wad make)
assets/content      abilities and preset builds (TOML), the v1 kits
docs/               VOCABULARY.md, PROTOCOL.md, MATRIX.md, HUB.md, ECONOMY.md, MODELS.md, BUILDING.md
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

See [`docs/BUILDING.md`](docs/BUILDING.md).

## License

Split: GPLv3 client/engine, AGPLv3 server, CC BY-SA 4.0 content. See
[`LICENSE.md`](LICENSE.md).
