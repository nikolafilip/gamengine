# Wire Protocol

Status: skeleton, Phase 0. Filled in during Phase 2 alongside `gm-net`. Decisions already made
in PLAN.md 2.1, 2.3 and 11.3 are recorded here; everything marked TBD is decided when the code
that needs it is written, and this document is updated in the same commit.

## 1. Transport

- QUIC. Native clients use `quinn`; browsers use WebTransport (`wtransport`). Same datagram and
  stream semantics, one protocol.
- **Unreliable datagrams**: input frames (client → server), snapshots (server → client).
- **Reliable streams**: one bidirectional control stream (auth, zone handoff, chat, inventory,
  trade, contracts). Messages are `bitcode`-serialized enums with a u16 length prefix.
- Connection: the hub issues a signed session token (ed25519); the zone verifies it on the first
  control-stream message. TBD: token layout, expiry, renewal.

## 2. Datagram header

| Field | Bits | Notes |
|---|---|---|
| protocol version | 8 | mismatch closes the connection |
| kind | 8 | `Input`, `Snapshot`, `Ping` |
| payload | rest | bit-packed, see below |

Bit writer: MSB-first, no byte alignment between fields. TBD: exact writer semantics and
varint encoding, specified with test vectors.

## 3. Input frame (client → server)

Sent every tick. Each datagram carries the last 3 frames so a lost datagram does not stall
movement (PLAN.md 11.3).

| Field | Bits | Notes |
|---|---|---|
| tick | 32 | client's simulation tick |
| buttons | 16 | forward, back, left, right, jump, crouch, primary, secondary, guard, ability 1–4, interact, viewport-switch, reserved |
| yaw | 12 | 0.1° steps, 0–3599 |
| pitch | 11 | 0.1° steps, −90°..+90° |
| forward move | 8 | signed, −127..127 maps to −1..1 |
| side move | 8 | signed |
| ability slot | 8 | when an ability button edge is set |

## 4. Snapshot (server → client)

Per client, per tick, delta-compressed against the last baseline the client acknowledged.

| Field | Notes |
|---|---|
| server tick | 32 bits |
| baseline tick | 32 bits; 0 = full snapshot |
| last processed input tick | 32 bits; drives client reconciliation |
| entity count | varint |
| entity records | see below |

Entity record: id (varint), then a changed-field mask, then only the changed fields:
position quantized to 1/4 u (~0.8 cm, PLAN.md says "1 cm"; 1/4 u is the power-of-two neighbour),
yaw 0.1°, pitch 0.1° (own entity and aimers only), velocity (own entity only, for prediction),
animation state u8, health u16 (party members and the own entity only), status bits, archetype
frame + model hash (on first sight only).

Rules:
- Only entities in the PVS of the client's own leaf are sent (PLAN.md 1.2, 8). No exceptions.
- Distance bands: full rate inside 512 u, half rate to 1536 u, 10 Hz beyond (PLAN.md 1.2).
  TBD: band thresholds after measurement.
- Entities leaving the PVS are removed with an explicit remove record so the client can hide
  them immediately.

## 5. Reliable messages

`enum Control { Auth, Handoff, Chat, Inventory, Trade, Contract, Ping, Kick }` — TBD in
Phase 2 (auth, handoff, chat) and Phase 5 (inventory, trade, contract).

## 6. Budgets

Phase 2 acceptance (PLAN.md 11.8): playable at 150 ms round trip and 3% loss in `turmoil`;
under 30 KB/s per player with 16 players. Phase 4: 200 bots under the same figure with
distance bands active.
