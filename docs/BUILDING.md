# Building and running

## Prerequisites

- Rust stable 1.88 or newer with `clippy` and `rustfmt` (`rustup component add clippy rustfmt`).
- A Vulkan driver. Linux: Arch `vulkan-radeon` / `vulkan-intel` / `vulkan-swrast`
  (software), Debian/Ubuntu `mesa-vulkan-drivers`. Without an ICD the client reports
  "no compatible GPU adapter"; `--software` selects the CPU adapter explicitly.
- `curl`, `unzip` (or `python3`) for `scripts/fetch-ericw-tools.sh`.
- Optional: [TrenchBroom](https://trenchbroom.github.io/) to edit maps.

## First build

```sh
scripts/fetch-ericw-tools.sh                                   # pinned ericw-tools into tools/
cargo run -p gm-tools -- wad make                              # assets/textures/base.wad + palette.lmp
cargo run -p gm-tools -- map build assets/maps/src/test_room.map
cargo test --workspace
cargo run --release -p gm-client -- --map assets/maps/built/test_room.bsp
```

`map build` runs `qbsp -leaktest`, `vis` and `light -extra -lit`. The output is
`assets/maps/built/<name>.bsp` plus `<name>.lit` (RGB lightmaps). Both are committed; the
`.prt` portal file is not.

## Client flags

| Flag | Effect |
|---|---|
| `--map PATH` | BSP to load (default `assets/maps/built/test_room.bsp`) |
| `--palette PATH` | 768-byte palette (default `assets/textures/palette.lmp`) |
| `--bench N` | render N frames with the camera sweeping, print statistics, exit. Always uncapped and without vsync unless `--present` says otherwise |
| `--no-vsync` | `AutoNoVsync` (Immediate, else Mailbox) |
| `--present MODE` | force `fifo`, `relaxed`, `mailbox` or `immediate` |
| `--max-fps N` | CPU-side frame cap in normal play (default 250, 0 = uncapped) |
| `--headless` | render offscreen, no window; used by CI |
| `--software` | force the software Vulkan adapter (lavapipe) |
| `--size WxH` | window or offscreen size (default 1280x720) |
| `--screenshot out.ppm` | headless only: write the last frame as a binary PPM |
| `--connect ADDR` | join a zone instead of playing offline (see Multiplayer) |
| `--cert PATH` | DER certificate of the zone, written by `gm-server --cert-out` (default `zone-cert.der`) |
| `--name NAME` | player name for the zone (default `$USER`) |
| `--seconds N` | exit after N seconds and print the network statistics (scripted runs) |

Default present mode is **Mailbox** (no tearing, no blocking) with the frame cap, not Fifo.
Reason, measured 2026-09-30 on Arch, X11, xfwm4 with compositing, RADV (Renoir): Fifo and
FifoRelaxed presented exactly one frame per second (every swapchain acquire hit its one-second
timeout) even with the window focused and the monitor awake, while Mailbox and Immediate ran at
thousands of fps. If Fifo works on your setup, `--present fifo` gives true vsync.

Bench output lines start with `bench:` and are parsed by `scripts/check-perf.sh`. Frame times
in windowed mode are CPU-side (submit to submit) and, with vsync off, they track GPU throughput.
A monitor in DPMS standby lowers iGPU clocks; wake it (`xset dpms force on`) before measuring.

Running from a terminal without a display (ssh, tty): set `DISPLAY=:0` to use the desktop
session's X server, or use `--headless`.

## Multiplayer (Phase 2)

One zone process, any number of clients and bots, QUIC on UDP (`docs/PROTOCOL.md`).

```sh
# terminal 1: the zone. Writes its self-signed certificate for clients, prints a report every 5 s.
cargo run --release -p gm-server -- --map assets/maps/built/test_room.bsp --listen 127.0.0.1:4433 --cert-out zone-cert.der

# terminal 2: sixteen bots for 30 seconds (wander, hunter or hold behaviour)
cargo run --release -p gm-bot -- --connect 127.0.0.1:4433 --cert zone-cert.der --bots 16 --secs 30

# terminal 3: you
cargo run --release -p gm-client -- --connect 127.0.0.1:4433 --cert zone-cert.der --name pezo
```

Controls online: mouse look, `WASD`, `Space` jump, **left click** sword swing, **right click**
crossbow bolt, **Shift** dash (30 stamina). Other players are coloured boxes, corpses are flat
grey boxes, bolts are small yellow boxes; hand-painted meshes arrive in Phase 6. The window
title shows health, kills, deaths, interpolation delay and the reconciliation correction count.

Server flags: `--hz 64|20`, `--report-secs N`, `--ticks N` (stop after N ticks),
`--max-players N`, `--seed N`. The report line carries per-player UDP bytes/s in both
directions (from QUIC's own counters), snapshot payload bytes/s, tick timing, starvation,
hits and kills. `RUST_LOG=debug` shows joins, malformed datagrams and connection ends.

Bot flags: `--bots N`, `--secs N`, `--behaviour wander|hunter|hold`, `--seed N`, `--map PATH`
(the bots predict against the same BSP). The summary prints bytes/s per bot, RTT, snapshot
gaps and reconciliation corrections.

The zone accepts empty session tokens (`--open` behaviour) until the hub exists in Phase 4.
Clients trust exactly the certificate they are given; there is no insecure mode.

## CI gates locally

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-budgets.sh && scripts/check-budgets.sh --self-test
scripts/check-binary-size.sh && scripts/check-binary-size.sh --self-test
scripts/check-perf.sh                 # windowed, real GPU
scripts/check-perf.sh --software      # what CI runs
scripts/check-netcode.sh              # 16 bots at 150 ms / 3% loss in turmoil, bytes/player/s gate
```

`check-netcode.sh` runs the turmoil acceptance test (`crates/gm-server/tests/netcode.rs`) in
simulated time and the real-UDP loopback test; both print per-bot and per-zone numbers with
`--nocapture`.

When the release binary legitimately grows (a new feature), update the baseline in the same
commit with `scripts/check-binary-size.sh --update-baseline` and say why in the commit message.

## Maps in TrenchBroom

1. New map, game "Quake", map format "Standard" (Valve 220 also works with qbsp).
2. Replace the entity definitions with `assets/maps/src/gamengine.fgd` (Map > Entity
   definitions).
3. Add `assets/textures/base.wad` as the texture collection. TrenchBroom previews WAD
   textures with Quake's palette; ours is `assets/textures/palette.lmp`, so previews are
   slightly off-colour in the editor and correct in the game.
4. Save under `assets/maps/src/` and run `cargo run -p gm-tools -- map build <file>`.

Entities: `worldspawn` keys `wad`, `light` (minlight), `_sunlight*`, `_dirt`, `_bounce`;
`light` (point or spot with `mangle`); `info_player_start`; `func_detail`, `func_wall`,
`func_illusionary`; placeholders `gm_spawn`, `gm_zone` for Phase 3.

## Development helper: independent reviews

`scripts/dev/gemini-review.py --prompt "..." --file docs/PROTOCOL.md --file crates/...` sends a
prompt plus files to Google AI Studio (Gemini) and prints the answer; it needs
`~/google-ai-studio-api-key` or `$GEMINI_API_KEY`. It is a development aid for design and code
reviews (PROTOCOL.md section 10 records one); CI never runs it and nothing depends on it.
