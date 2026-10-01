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
`.prt` portal file is not. `map gen-arena` regenerates `assets/maps/src/arena.map`, the 8v8
arena of Phase 3 (symmetric, two team bases, pillars, low cover, side walkways).

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
| `--build NAME` | preset build to ask the zone for: `ironclad`, `blade`, `frostweaver`, `shade` (default: the zone's default) |
| `--team N` | team 1 or 2 (default 0: the zone balances) |
| `--third-person` | start in the third-person viewport (`V` toggles at any time) |
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

## Multiplayer (Phases 2 and 3)

One zone process, any number of clients and bots, QUIC on UDP (`docs/PROTOCOL.md`). The zone
loads `assets/content` (abilities and preset builds, `docs/MATRIX.md`) and sends it to every
client, so clients need no content files.

```sh
# terminal 1: the zone on the 8v8 arena. Writes its self-signed certificate for clients,
# prints a report every 5 s.
cargo run --release -p gm-server -- --map assets/maps/built/arena.bsp --listen 127.0.0.1:4433 --cert-out zone-cert.der

# terminal 2: fifteen duelist bots for a minute, four presets, alternating teams; they
# counter-pick (re-spec to whatever beats the enemy's aspects) every ten seconds
cargo run --release -p gm-bot -- --connect 127.0.0.1:4433 --cert zone-cert.der --map assets/maps/built/arena.bsp \
    --bots 15 --secs 60 --behaviour duelist --builds ironclad,blade,frostweaver,shade --teams 1,2 --counter-pick

# terminal 3: you, as a blade on team 1, in third person
cargo run --release -p gm-client -- --map assets/maps/built/arena.bsp --connect 127.0.0.1:4433 --cert zone-cert.der \
    --name pezo --build blade --team 1 --third-person
```

Controls online: mouse look, `WASD`, `Space` jump, **left click** primary, **right click**
secondary, **Ctrl** guard (hold to block, press to parry, whichever the build has), **1–4** the
actives (**Shift** is also active 1), **V** switches first/third person, **F1–F4** ask the zone
for preset 1–4 (applied at your next respawn), `Esc` releases the cursor, `Q` quits. In third
person the camera sits behind and above you and your shots go where the crosshair points
(VOCABULARY.md 9). Players are boxes coloured by team (blue own side, red the other), a white
nose box shows their facing, blocking bodies turn blue-ish, staggered or frozen bodies darken,
hasted ones brighten; corpses are flat grey, bolts small yellow boxes, area effects flat orange
discs. The window title shows the build, team and viewport, health, stamina, focus, kills,
deaths, interpolation delay, the correction count, fps and the last respec reply.

Server flags: `--content DIR` (default `assets/content`), `--default-build NAME` (default
`blade`), `--hz 64|20`, `--report-secs N`, `--ticks N` (stop after N ticks),
`--max-players N`, `--seed N`. The report line carries per-player UDP bytes/s in both
directions (from QUIC's own counters), snapshot payload bytes/s, tick timing, starvation, hits
and kills; the final report adds kills per team. `RUST_LOG=debug` shows joins, malformed
datagrams and connection ends.

Bot flags: `--bots N`, `--secs N`, `--behaviour wander|hunter|hold|duelist`, `--builds a,b,...`
and `--teams 1,2,...` (cycled over the bots), `--counter-pick`, `--seed N`, `--map PATH` (the
bots predict against the same BSP). The summary prints bytes/s per bot, RTT, snapshot gaps,
reconciliation corrections, kills and the re-specs that happened.

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
scripts/check-netcode.sh              # 16 bots at 150 ms / 3% loss in turmoil, bytes/player/s gate, counter-pick
scripts/check-matrix.sh               # 8v8 arena: a dominant build is countered by re-speccing
```

`check-netcode.sh` runs the turmoil acceptance tests (`crates/gm-server/tests/netcode.rs` and
`counterpick.rs`) in simulated time and the real-UDP loopback test; all print per-bot and
per-zone numbers with `--nocapture`. `check-matrix.sh` runs the offline 8v8 matches of
MATRIX.md 11 (`crates/gm-bot/tests/arena.rs`).

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
`light` (point or spot with `mangle`); `info_player_start` (a spawn for any team);
`gm_spawn` with `team` 1 or 2 (0 = any) and `angle`; `func_detail`, `func_wall`,
`func_illusionary`; `gm_zone` is a placeholder for later phases.

## Development helper: independent reviews

`scripts/dev/gemini-review.py --prompt "..." --file docs/PROTOCOL.md --file crates/...` sends a
prompt plus files to Google AI Studio (Gemini) and prints the answer; it needs
`~/google-ai-studio-api-key` or `$GEMINI_API_KEY`. It is a development aid for design and code
reviews (PROTOCOL.md section 10 records one); CI never runs it and nothing depends on it.
