# Wire Protocol

Status: v1, Phase 2. `gm-net` implements exactly this document; the test vectors in section 2
are unit tests. Decisions from PLAN.md 2.1, 2.3 and 11.3 are binding here. When the code and this
document disagree, the document wins and the code is wrong; changes to either go in one commit.

Protocol version byte: **1**. Any change to sections 2–5 bumps it.

Section 10 records the independent design review this version went through and what changed.

## 1. Transport

- QUIC. Native clients use `quinn`; browsers will use WebTransport (`wtransport`, Phase 8). Same
  datagram and stream semantics, one protocol.
- **Unreliable datagrams**: input frames (client → server), snapshots (server → client), pings.
  One datagram is one message; a message never spans datagrams. Payloads are capped at
  **1,100 bytes** (`MAX_DATAGRAM_PAYLOAD`), well under the 1,200-byte initial QUIC MTU minus
  framing, so nothing depends on MTU discovery or on VPN/PPPoE paths. A snapshot that would not
  fit drops the farthest entities' updates for that tick (they are still tracked and catch up on
  the next tick); the server counts the event.
- **Reliable stream**: one bidirectional control stream opened by the client right after the
  handshake. Messages are `bitcode`-encoded `Control` values (section 8), each prefixed by a
  big-endian **u16 length**. A message longer than 65,535 bytes is a protocol error.
- **Congestion control**: a fixed window (64 KiB, `FixedWindow`) that never shrinks. The traffic
  is a small application-limited rate; a loss-based controller would throttle 64 Hz datagrams
  under the random loss we must survive (3% loss halves NewReno's window every few seconds) and
  queue stale snapshots. The datagram send buffer is small (4 KiB) so quinn drops the oldest
  queued snapshot instead of delivering it late. The per-player budget gate (section 9) is what
  keeps the rate honest.
- Certificates: the zone presents a self-signed certificate generated at start (or loaded from a
  file). Clients trust a certificate DER passed on the command line or, in Phase 4, receive the
  zone's certificate hash from the hub in the session token. There is no "accept anything" mode.
- Idle timeout 10 s, QUIC keep-alive every 2 s.
- The session token (hub-signed, ed25519) rides in `Hello.token`. Phase 2 zones accept an empty
  token when started with `--open` (development and tests). Phase 4 makes it mandatory.

## 2. Bit packing

All datagram payloads use one bit writer. Rules:

1. Bits are written **MSB-first** into bytes: the first bit written is bit 7 of byte 0.
2. `bits(v, n)`: the `n` low bits of `v`, most significant bit first. `n` is 0..=64.
3. `bool` = `bits(v, 1)`.
4. `uvar(v)`: unsigned variable length. Groups of 7 bits from the least significant group up;
   each group is written as one continuation bit (1 = another group follows) then the 7 data bits.
   At a byte boundary this is exactly LEB128.
5. `svar(v)`: zigzag (`(v << 1) ^ (v >> 63)`) then `uvar`.
6. `finish()` pads the last byte with zero bits. Readers must tolerate trailing pad bits and must
   fail (not panic) on reading past the end.

Test vectors (hex bytes after `finish()`):

| Writes | Bytes |
|---|---|
| `bits(0b101, 3)`, `bits(0b1111, 4)` | `BE` |
| `uvar(0)` | `00` |
| `uvar(127)` | `7F` |
| `uvar(128)` | `80 01` |
| `uvar(300)` | `AC 02` |
| `svar(0)`, `svar(-1)`, `svar(1)`, `svar(-2)` | `00 01 02 03` |
| `svar(-64)` | `7F` |
| `svar(64)` | `80 01` |
| `bits(1, 1)`, `uvar(300)` | `D6 01 00` |
| `bits(0xABCD, 16)`, `bool(true)` | `AB CD 80` |

Quantization (all deterministic, round half away from zero):

| Quantity | Encoding |
|---|---|
| position component | `svar(round(x * 4))`: 1/4 world unit (≈ 0.8 cm) |
| velocity component | `svar(round(v * 8))`: 1/8 u/s |
| yaw | `bits(round(yaw mod 360 * 10) mod 3600, 12)`: 0.1° |
| pitch | `bits(round((clamp(pitch, -90, 90) + 90) * 10), 11)`: 0.1°, 0..=1800 |
| move axis | `bits(round(clamp(a, -1, 1) * 127) as i8 as u8, 8)` |

Dequantization divides back; the simulation runs on the dequantized values on both sides so the
server compares like with like when it reconciles.

Ticks are `u32` and compare with **wrapping arithmetic** (`a.wrapping_sub(b) as i32`), like TCP
sequence numbers. Zones restart long before a wrap (2.1 years at 64 Hz), but nothing may break
if one happens.

## 3. Datagram header

| Field | Bits | Notes |
|---|---|---|
| version | 8 | must equal 1, else the datagram is dropped and counted |
| kind | 8 | 0 `Input`, 1 `Snapshot`, 2 `Ping`, 3 `Pong` |

## 4. Input datagram (client → server)

Sent once per client tick, as soon as the tick's input is sampled. Carries the newest
**1..=4 consecutive frames**: three consecutive datagram losses (0.0003% at 3% loss, once in
a few hours at 64 Hz) cost one tick of movement, anything less costs nothing (PLAN.md 11.3).

| Field | Bits | Notes |
|---|---|---|
| ack_tick | 32 | newest server tick whose snapshot was decoded; 0 = none yet |
| view_tick | 32 | server tick the client is displaying for other entities (its interpolation time); 0 = none. Used for melee lag compensation (section 7.4) |
| frame_count − 1 | 2 | 1..=4 frames |
| first_tick | 32 | client tick of the oldest frame; frame `i` is tick `first_tick + i` |
| frames | 63 each | oldest first |

Frame:

| Field | Bits | Notes |
|---|---|---|
| buttons | 16 | bit 0 jump, 1 crouch, 2 primary, 3 secondary, 4 guard, 5–8 ability 1–4, 9 interact, 10 viewport switch, 11–15 reserved (must be 0) |
| yaw | 12 | 0.1° |
| pitch | 11 | 0.1° |
| forward | 8 | i8, −127..=127 → −1..=1 |
| side | 8 | i8, right positive |
| ability | 8 | slot activated this tick (1-based), 0 = none |

Movement direction lives in `forward`/`side` only; there are no forward/back/left/right buttons
(the Phase 0 skeleton listed both, which was redundant).

Server rules (the frame ledger):
- **Every frame is executed exactly once**, in tick order. Frames with
  `tick <= last executed tick` and frames already queued are ignored; a frame from a datagram
  that arrives after a newer one still slots into the queue in tick order (UDP reorders), so the
  redundancy is never wasted. Each queued frame keeps the `view_tick` of the datagram that carried
  it. A frame is never reused: when no frame is available the player is simply not simulated
  that tick (no movement, no gravity), and the tick is counted as *starved* for diagnostics.
  Reusing or inventing frames would make `last_input_tick` lie to the client's reconciliation
  (section 7.2).
- Dejitter: the server keeps **one frame in reserve** (a frame runs only when a newer one is
  already queued), so one tick of arrival jitter never starves the simulation, at the cost of one
  tick (15.6 ms) of added input latency. With four or more frames queued it runs two per tick
  until the queue drains. Measured in the acceptance test, this took starvation from 14% of
  ticks to about 1% at 75 ± 10 ms one-way latency.
- Rate limit: a token bucket per client with **1 credit per server tick, burst 8**, one credit
  per executed frame, and at most **3 frames per server tick**. A client cannot execute more than
  64 frames per second on average, so sending inputs faster than the tick rate buys nothing.
  Frames arriving over budget wait in the queue; beyond 32 queued frames the oldest are dropped
  (the client will be reconciled to the server's state; that is the price of flooding).
- `ack_tick` must name a tick actually sent to this client within the last 64 ticks; anything else
  is treated as 0 (full snapshot follows).
- `view_tick` is clamped to `[server_tick - max_rewind, server_tick]` where
  `max_rewind = min(13, half_rtt_ticks + 8)`: the client's measured one-way latency (QUIC RTT
  estimate / 2) plus the interpolation delay (6 ticks) plus 2 ticks of jitter, never more than
  200 ms. A low-ping client cannot claim a 200 ms rewind.
- Malformed datagrams (bad version, short, reserved bits set) are dropped and counted per client;
  100 in a minute kicks.

## 5. Snapshot datagram (server → client)

One per client per server tick. Delta-compressed against the **baseline**: the snapshot of
`ack_tick` the client last acknowledged, provided it is at most 60 ticks old and still in the
server's per-client history; otherwise `baseline_tick = 0` and the snapshot is full.

The server keeps, per client, the **reconstructed** snapshot for each tick it sent: exactly the
entity table the client rebuilds from the wire (section "Reconstruction" below). Deltas are
always computed against that table, never against what happened to be on the wire.

| Field | Bits | Notes |
|---|---|---|
| server_tick | 32 | ≥ 1 |
| baseline_tick | 32 | 0 = full snapshot |
| last_input_tick | 32 | client tick of the last frame executed; drives reconciliation. 0 = none |
| entity_count | uvar | |
| entities | | records, ascending id |
| removed_count | uvar | entities present in the baseline and gone now |
| removed | uvar each | ids |

Entity record:

| Field | Bits | Present when |
|---|---|---|
| id | uvar | always |
| mask | 8 | always. bit 0 SPAWN, 1 POS, 2 YAW, 3 PITCH, 4 VEL, 5 ANIM, 6 HEALTH, 7 FLAGS |
| kind | 4 | SPAWN. 0 player, 1 projectile |
| spawn info | | SPAWN, by kind (below) |
| pos | 3 × svar | POS. **Absolute** quanta when SPAWN is set, **delta** against the baseline record otherwise |
| yaw | 12 | YAW |
| pitch | 11 | PITCH |
| vel | 3 × svar | VEL. Absolute when SPAWN (or the baseline record has no velocity), delta otherwise |
| anim | 8 | ANIM |
| health | uvar | HEALTH |
| flags | 8 | FLAGS. bit 0 alive, 1 on ground, 2 guarding, 3 dashing, 4 jump held, 5–7 reserved |

Spawn info: player → `frame` 2 bits (0 colossus, 1 striker, 2 caster, 3 infiltrator).
Projectile → `owner` uvar, `def` uvar (ability index in the kit), `input_tick` uvar (the owner's
client tick that fired it, for matching the owner's predicted copy).

Entity ids are **monotonic** within a zone process and never reused, so a delta can never be
applied to a different entity's baseline record.

Rules:
- A field is sent when it differs from the baseline record, or when there is no baseline record
  (SPAWN set: every field the server sends for this entity is included). SPAWN therefore repeats
  on every tick until the client acks a snapshot containing the entity; the client cannot miss it.
- **Reconstruction**: the decoded snapshot is the baseline table with `removed` ids deleted,
  listed records applied (fields not received are copied from the baseline record), and every
  baseline entity that is neither listed nor removed **carried forward unchanged**. Both sides
  perform this identically; the server's per-client history stores the result.
- An entity that is unchanged since the baseline is not listed at all (carry-forward costs zero
  bytes). An entity that is not scheduled this tick by its distance band is likewise not listed
  and is **not** in `removed`.
- `vel`, `health` and the `jump held` flag are sent for the **own entity only**. Party members
  will get `health` in Phase 5. Nothing else about other players' resources is sent.
- Only entities in the PVS of the client's eye leaf are sent (PLAN.md 1.2, 8). An entity counts
  as in the PVS when the leaf of its origin or of its eye point is in the row. The client's own
  entity is always sent, and so is any player within **128 u** of the client's eye whatever the
  PVS says: a body that close can block the client's movement, and prediction cannot handle a
  wall it was never told about (a pillar corner is exactly where this happens). At 4 m the
  anti-ESP value of hiding it is nil. Entities leaving the PVS appear in `removed`.
- Distance bands (PLAN.md 1.2), measured from the client's eye to the entity's origin:
  full rate to 512 u; every second tick to 1,536 u; every sixth tick beyond (≈ 10.7 Hz).
  Projectiles, the own entity and any entity absent from the baseline (first sight) are always
  full rate.

Client rules:
- The client keeps the last 64 reconstructed snapshots keyed by `server_tick`. A snapshot whose
  `baseline_tick` is unknown is dropped and counted (the server's baseline is the client's own
  ack, so this only happens after the client evicted it).
- Every input datagram acks the newest reconstructed `server_tick`.
- A `removed` entity stays in the interpolation buffer until the client's render time
  (section 7.3) passes the removal tick; it is not hidden the instant the datagram arrives, or
  the last 100 ms of its visible movement would be cut.

## 6. Ping / Pong

`Ping`: `bits(client_ms, 32)`. `Pong` echoes the same 32 bits. Reserved for Phase 4 clock
statistics; Phase 2 uses the QUIC RTT estimate on both ends.

## 7. Simulation contract

### 7.1 Ticks
Server ticks start at 1 and advance at the zone rate (64 Hz combat). Client input ticks start
at 1 and advance with the client's own fixed step; they are **not** synchronised with server
ticks. Reconciliation works in client-tick space (`last_input_tick`), interpolation in server-tick
space (`server_tick`). No clock synchronisation is needed for either. The server may execute 0,
1, 2 or 3 of a client's frames in one server tick (section 4); because every frame runs exactly
once, the state after frame `t` is the same on both sides regardless of when it ran.

### 7.2 Prediction and reconciliation (own entity)
1. Each client tick: sample input, quantize it to the wire frame and **dequantize it back**
   (the server runs exactly those values), run `gm-core` (`sim::step_mover`) for the own entity
   against the newest known positions of other players, store `(tick, input, predicted mover
   state)` in a ring of 128.
2. On a snapshot with `last_input_tick = t`: compare the server's own-entity state with the
   predicted state stored for `t`. If position differs by more than **2 u** or velocity by more
   than **16 u/s**, replace the state at `t` with the server's and replay inputs `t + 1 ..= now`.
   Below tolerance nothing happens (quantization noise, PLAN.md 11.3 "tolerance-based"). 2 u is
   6 ms of running; anything smaller is invisible, anything larger is a real disagreement (usually
   a body-block against a player whose position the client only had interpolated).
3. Corrections are applied to the simulation state instantly; the renderer may smooth the eye
   over up to 100 ms (cosmetic, never affects the simulation). Before a server position is
   adopted it is nudged out of solid (`movement::nudge_position`, QuakeWorld's
   `PM_NudgePosition`): the rounded position can land exactly on a clip plane, which the hull
   tracer classifies as solid and then refuses to move, while the server's true position sits
   `DIST_EPSILON` away. Without the nudge a client pressed against a wall corner froze for as
   long as the server kept it there (found by the acceptance test's correction log).
4. Ability activation, cooldowns, stamina and `MoveSelf` (dash) are part of the predicted mover
   state and replay deterministically. Attacks and projectiles are not predicted beyond
   spawning a local projectile copy that is replaced by the server's when a record with matching
   `(owner, input_tick)` arrives.

### 7.3 Interpolation (other entities)
Render time for other entities is `newest_server_tick - delay`, with `delay` = 6 ticks (94 ms)
plus one extra tick per snapshot gap observed in the last second, capped at 13. The delay shrinks
by one tick per second without gaps. Positions and angles are interpolated between the two
reconstructed snapshots bracketing the render time. When only the older sample exists (a gap),
the entity holds its last position; no extrapolation. (Draining the buffer by time-scaling
instead of stepping is a Phase 3 refinement.)

### 7.4 Lag compensation (melee)
A `MeleeArc` activated by a frame is resolved against other entities' positions at the
attacker's `view_tick` (the interpolation time the attacker was looking at), rewound from the
server's position history and clamped as in section 4. The server keeps 32 ticks of history per
entity (the rewind bound plus the longest windup and active window). Projectiles are **not** rewound: they spawn at the attacker's current server position
and are then stepped forward `server_tick - view_tick` ticks (after the same clamp) in the
spawning tick. Each caught-up tick is swept against the targets' **historical** positions at that
tick, so the catch-up hits exactly what the attacker saw and nothing more; a client that inflates
its measured latency (delaying acknowledgements) only buys itself older target positions. The
clamp bounds the catch-up to 200 ms whatever the client claims.

### 7.5 Collision
World: BSP hull traces (gm-bsp). Players against players: axis-aligned box sweeps with the same
32 × 32 × 56 hull, resolved inside the movement trace so sliding and stepping work against
players exactly as against walls. Hitboxes for damage are the archetype capsules
(VOCABULARY.md 3). Projectiles are swept as spheres against the world (point hull) and against
capsules.

## 8. Reliable messages

```
enum Control {
    // client → server
    Hello { version: u16, name: String, token: Vec<u8> },
    Chat(String),
    Bye,
    // server → client
    Welcome { entity: u32, server_tick: u32, hz: u16, map: String, map_hash: u64 },
    Reject(String),
    PlayerInfo { id: u32, name: String },
    PlayerLeft(u32),
    Killed { victim: u32, killer: u32 },       // killer 0 = world
    ChatFrom { from: u32, text: String },
    Kick(String),
}
```

Handshake: connect → client opens the control stream → `Hello` → `Welcome` (or `Reject`, then
close). The server sends snapshots from the tick after `Welcome`; the client sends inputs after
`Welcome`. `map_hash` is FNV-1a 64 of the `.bsp` bytes; a mismatch is a client-side error
("wrong map build").

`Hello.name`: 1..=24 bytes of printable UTF-8 after trimming; anything else is rejected.

## 9. Budgets and the acceptance test

Phase 2 acceptance (PLAN.md 11.8): playable at 150 ms round trip and 3% datagram loss in
`turmoil`; under **30 KB/s per player** in each direction with 16 players, measured as UDP bytes
reported by QUIC (`ConnectionStats`), so QUIC overhead counts. `budgets.toml [net]` holds the
number; `scripts/check-netcode.sh` gates it.

"Playable" is asserted, not felt. The turmoil test (`crates/gm-server/tests/netcode.rs`) runs
16 bots for 30 simulated seconds at 75 ms one-way latency (±10 ms jitter) and 3% independent
loss each way, and asserts:
1. Reconciliation corrections above tolerance that have no visible cause (no hit, no death or
   respawn, no other body within blocking distance): fewer than 1 per bot per 10 s. Corrections
   caused by the server pushing the player (knockback, body-blocks against players whose
   positions the client only had a few ticks stale) are counted and reported but are legitimate.
2. Input starvation: under 1% of server ticks per bot.
3. Snapshot gaps seen by a bot: never more than 4 consecutive ticks (62 ms) missing.
4. Melee with lag compensation: a swing aimed at where the attacker *sees* a target moving at
   full speed registers on the server.
5. Bytes per player per second in both directions under the budget.

Expected at 64 Hz with 16 players in one room: snapshot ≈ 14 B header + 16 × ~6 B records ≈
110 B payload + ~45 B QUIC/UDP/IP framing ≈ 155 B × 64 ≈ 10 KB/s down; inputs ≈ 46 B payload
+ framing ≈ 90 B × 64 ≈ 5.8 KB/s up.

## 10. Design review log

**2026-09-30, v1 draft reviewed by Gemini 3.1 Pro** (independent review requested before
implementation; verdicts are ours):
- Accepted: baseline must be the reconstructed table, not the wire payload (carry-forward rule
  made explicit); reused frames on input starvation would desync reconciliation (frames now run
  exactly once, starvation just skips the tick); catch-up was a speed hack (token bucket added);
  `view_tick` must be bounded by measured latency (clamp added) which also bounds the projectile
  forward step; removed entities must leave at render time, not on arrival; 0.5 u tolerance was
  too tight (2 u / 16 u/s); 4 redundant frames instead of 3; loss-based congestion control would
  throttle datagrams (fixed window); payload cap 1,100 B; monotonic entity ids; wrapping tick
  arithmetic; the acceptance test list in section 9.
- Rejected: "projectile SPAWN can be missed" (SPAWN repeats until acked by construction);
  "drop the removed list" (PVS-boundary flicker is rare and explicit removal is what makes the
  client's state provably equal to the server's; revisit if measured); Cubic/BBR instead of a
  fixed window (the rate is known and gated; a fixed window is what a raw-UDP game does anyway).
- Deferred: time-scaled interpolation buffer drain (Phase 3), `SO_RCVBUF` tuning on the real
  socket (Phase 4 with the 200-bot swarm).

**2026-09-30, implementation reviewed by Gemini 3.1 Pro** (after the acceptance test passed):
- Accepted: late datagrams' frames were dropped because they compared against the newest queued
  tick (now inserted in tick order, duplicates ignored, only executed frames refused); `view_tick`
  was taken from the newest datagram instead of the one that carried the frame (now per frame);
  the projectile forward step swept current positions (now historical, see 7.4); the per-client
  control channel was unbounded (now 256 messages, a client that stops reading is closed); the
  acknowledged frame is kept in the prediction ring for re-comparison; a forward-stepped
  projectile's lifetime is shortened by the lag; per-tick shared body list instead of one per
  player; PVS rows borrowed instead of copied; oversize snapshots drop several records per
  re-encode. The client tick wrap now also restarts the consecutive frame run.
- Rejected: "dead inputs are replayed after respawn" (only frames the server will execute alive
  are replayed; frames run while dead are acknowledged and dropped first).
