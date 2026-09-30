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
crates/gm-core      shared simulation: entity vocabulary, movement, fixed tick. No I/O.
crates/gm-bsp       Quake BSP loader: geometry, lightmaps, PVS, hull tracing.
crates/gm-net       wire protocol and transport (Phase 2).
crates/gm-client    wgpu forward renderer, Quake movement, fixed-step loop.
crates/gm-server    authoritative tokio zone server (tick loop with metrics so far).
crates/gm-hub       accounts, characters, shard registry (Phase 4).
crates/gm-ai        AI companions (Phase 7).
crates/gm-tools     CLI: map build (ericw-tools wrapper), WAD generation, asset budget lint.
crates/gm-bot       headless load-test client (Phase 4).
assets/maps/src     TrenchBroom .map sources and gamengine.fgd
assets/maps/built   compiled .bsp (+ .lit colored lightmaps)
assets/textures     generated palette and WAD (gm-tools wad make)
docs/               VOCABULARY.md, PROTOCOL.md, BUILDING.md
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

## CI gates

`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, then:

- `scripts/check-budgets.sh` — every asset in `assets/` within `budgets.toml`;
  `--self-test` proves an over-budget asset fails.
- `scripts/check-binary-size.sh` — release `gm-client` under the absolute cap
  and within 1 MiB of `ci/baselines/gm-client-size`; `--self-test` proves a
  regression fails.
- `scripts/check-perf.sh` — headless render under software Vulkan, RSS under
  the ceiling. Frame-time on a real iGPU needs the self-hosted runner.

See [`docs/BUILDING.md`](docs/BUILDING.md).

## License

Split: GPLv3 client/engine, AGPLv3 server, CC BY-SA 4.0 content. See
[`LICENSE.md`](LICENSE.md).
