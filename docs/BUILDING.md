# Building and running

## Prerequisites

- Rust stable 1.85 or newer with `clippy` and `rustfmt` (`rustup component add clippy rustfmt`).
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

## Server

```sh
cargo run -p gm-server -- --hz 64 --report-secs 5
```

Prints tick timing (mean, p99, max, overruns) every report interval. Ctrl-C stops it.

## CI gates locally

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-budgets.sh && scripts/check-budgets.sh --self-test
scripts/check-binary-size.sh && scripts/check-binary-size.sh --self-test
scripts/check-perf.sh                 # windowed, real GPU
scripts/check-perf.sh --software      # what CI runs
```

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
