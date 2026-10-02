# Custom models

Status: v1, implemented in Phase 6 (2026-10-01). The contract for player-uploaded avatar models: what a creator
uploads, what the hub checks, stores and serves, what a client caches and draws, and how a model
is moderated and taken down. PLAN.md 2.6 (asset budgets), 2.9 (streaming), 7 (legal) and 11.4
(data model) are binding. `gm-model`, `gm-ingest`, `gm-hub::models` and the client's model cache
implement this document; when they disagree, the document wins. Section 13 holds the proposed
numbers the director has not confirmed and the review log.

## 1. Principles

1. **Cosmetic only.** A model never touches the simulation. Hitbox, hull, speed and reach come
   from the archetype frame (VOCABULARY.md 3); one database flag swaps any avatar for the
   frame's mannequin with identical stats (PLAN.md 7).
2. **Readability survives customisation** (PLAN.md 3.2, MATRIX.md 1). Frame = silhouette: a
   model is uploaded *for one frame* and must fill that frame's envelope. Stance = the game's
   shared animations: an upload carries none, so nobody can hide a windup. Aspect = the aura,
   drawn by the client whatever the model is.
3. **The server recompresses.** A client only ever loads bytes our own pipeline wrote. Uploads
   are parsed in a sandboxed worker process, never inside the hub and never on a client.
4. **Content-addressed.** A model is named by the SHA-256 of its ingested bytes. The cache key,
   the integrity check and the takedown handle are the same 32 bytes.
5. **Never wait for art.** An entity is drawn the tick it appears, as its frame's mannequin
   tinted by armour class. The model swaps in when it is ready; if it never is (refused, taken
   down, unreachable), the mannequin stays.
6. **Measured.** Every number here is a budget in `budgets.toml` or a measurement in
   PLAN.md 11.10.

## 2. The standard rig

Model space is the game's: **+X forward, +Y left, +Z up**, origin on the ground between the
feet, world units (32 u = 1 m). A model stands in T-pose: arms along ±Y, legs along −Z.

24 bones, fixed indices, fixed parents:

| # | Bone | Parent | # | Bone | Parent |
|---|---|---|---|---|---|
| 0 | `hips` | — | 12 | `hand_r` | `forearm_r` |
| 1 | `spine` | `hips` | 13 | `thigh_l` | `hips` |
| 2 | `chest` | `spine` | 14 | `shin_l` | `thigh_l` |
| 3 | `neck` | `chest` | 15 | `foot_l` | `shin_l` |
| 4 | `head` | `neck` | 16 | `toe_l` | `foot_l` |
| 5 | `clavicle_l` | `chest` | 17 | `thigh_r` | `hips` |
| 6 | `upper_arm_l` | `clavicle_l` | 18 | `shin_r` | `thigh_r` |
| 7 | `forearm_l` | `upper_arm_l` | 19 | `foot_r` | `shin_r` |
| 8 | `hand_l` | `forearm_l` | 20 | `toe_r` | `foot_r` |
| 9 | `clavicle_r` | `chest` | 21 | `prop_r` | `hand_r` |
| 10 | `upper_arm_r` | `clavicle_r` | 22 | `prop_l` | `hand_l` |
| 11 | `forearm_r` | `upper_arm_r` | 23 | `prop_back` | `chest` |

**Required**: `hips`, `spine`, `chest`, `head`, both `upper_arm`, `forearm`, `thigh` and `shin`.
The rest are optional; a missing optional bone behaves as its parent. `prop_*` are attachment
points that follow their parent today.

A bone is its **pivot** (the joint's rest position in model space) and nothing else. Animation
is a model-space rotation per bone about its pivot, composed down the hierarchy:
`D(bone) = D(parent) · T(pivot) · R(bone) · T(−pivot)`, and `D` is the skinning matrix. Joint
axes and exporter conventions therefore do not exist for us: only joint positions do. The
shared animation set (section 9) is authored against this T-pose and drives every model and
every mannequin alike.

## 3. What a creator uploads

- One **glTF 2.0 binary** (`.glb`), self-contained (buffers in the BIN chunk, images in buffer
  views), at most **8 MiB**, with exactly one skin.
- glTF conventions: metres, +Y up, facing +Z. Ingestion converts to model space
  (`x ← z`, `y ← x`, `z ← y`, × 32).
- Joints are matched to the standard rig **by name** (case-insensitive, any non-alphanumeric
  run reads as `_`, so Blender's `upper_arm.L` matches). `gm-tools model template --frame F`
  writes a `.glb` of the frame's mannequin on the rig, to start from.
- Geometry: triangles (lists, strips or fans), with `POSITION`, `TEXCOORD_0`, `JOINTS_0` and
  `WEIGHTS_0`; `NORMAL` is computed when absent.
- One base-colour texture (PNG or JPEG, at most 4096 × 4096) shared by every primitive: one
  atlas, one draw call. `alphaMode` `MASK` or `BLEND` becomes a 1-bit cutout at the cutoff;
  `doubleSided` is honoured.

**Stripped, silently**: animations, morph targets, cameras, lights, extras, extensions, every
texture but base colour, vertex colours, further UV and joint sets, and every joint that is not
a standard bone (its weights go to its nearest standard ancestor).

**The pose** is the one the file was exported in, and it must be the T-pose: every limb (upper
arm, forearm, thigh, shin) within **15°** of its T-pose direction, the body upright (hips to
chest and chest to head within **30°** of +Z), left bones on the +Y side. Anything else is
refused with the bone and the angle named.

## 4. Validation

Every check runs; the answer lists every violation at once. `(r, h)` is the frame's hitbox
capsule (colossus 18 × 60, striker 14 × 56, caster 13 × 56, infiltrator 12 × 52).

| Check | Limit | Why |
|---|---|---|
| Triangles after merging, without degenerates | ≤ 3,500 | PLAN.md 2.6 |
| Bones in the ingested model | ≤ 24 by construction (budget 32) | PLAN.md 2.6 |
| Joints in the source skin | ≤ 128 | sanity; they are folded |
| Texture after resampling | each axis a power of two, 64..=1024 | PLAN.md 2.6 |
| Ingested bytes | ≤ 1,572,864 | PLAN.md 2.6 |
| UVs | inside [0, 1] (±0.01, clamped) | one atlas, no tiling |
| Body height: the `head` pivot | within ±8% of the frame mannequin's | frame = silhouette |
| Highest vertex (hair, hats, horns) | 0.90 h ..= 1.15 h | frame = silhouette |
| Box: every vertex | \|x\| ≤ 2.5 r, \|y\| ≤ 0.70 h, −0.05 h ≤ z | no screen-filling avatars |
| Coverage, front and side | 50% ..= 150% of the frame mannequin's | no invisible avatars, no impersonating a bigger frame |
| Required bones, hierarchy, pose | sections 2 and 3 | the shared animations must fit |

**Coverage** is the fairness rule. The T-pose mesh is rasterised orthographically at two pixels
per unit from the front and the back (along X) and from both sides (along Y), culled the way
the game draws it and with the texture's cutout applied; of each pair the smaller counts, so a
model that is only there from one side is not there. Only pixels inside the hitbox rectangle
(`2r × h`) count: an avatar must be visible where it can be hit. Front and side must each cover
at least half of what the frame's mannequin covers there, and at most one and a half times as
much: bulk is how a colossus reads against an infiltrator. The
bands of neighbouring frames still touch, because the frame table itself puts a striker and a
caster one unit apart; what the rule guarantees is that no frame can dress as the far one.

## 5. The ingested model (`.gmm`)

What the hub stores, what clients fetch, hash and cache. Little-endian.

```
magic "GMM1" | flags u16 (bit 0 cutout, bit 1 two-sided) | frame u8 | texture format u8 (1 = BC1 sRGB)
raw_len u32                      length of the payload after inflation
payload                          one zlib stream to the end of the file:
  scale        3 × f32           position = i16 / 32767 × scale
  average      4 × u8            mean opaque texel (sRGB): the colour of the stand-in
  bone_mask    u32               a bit per standard bone present
  pivots       24 × 3 × f32      rest position per bone (an absent bone holds its parent's)
  vertex_count u32, index_count u32, tex_w u16, tex_h u16, mip_count u8, 3 zero bytes
  vertices     24 B each: position i16 × 3 + 0, normal i8 × 3 + 0, uv u16 × 2 (unorm),
               joints u8 × 4 (standard bone indices), weights u8 × 4 (sum 255)
  indices      u16 each, a multiple of 3
  texture      BC1 blocks, largest mip first, down to the level whose smaller side is 4
```

The **model id** is the SHA-256 of the whole file. Ingestion is deterministic: the same upload
for the same frame gives the same bytes and the same id. A reader checks everything (magic,
format, lengths against the counts, counts against the budgets, every index below the vertex
count, every joint in the mask, weights summing to 255, inflation bounded by `raw_len` and by
2 MiB) and refuses the file otherwise.

**Why not `.glb` with KTX2/Basis** (PLAN.md 2.9 as first written, **[CORRECTED]**): measured
2026-10-01 with the workspace's release profile, parsing glTF in the client costs **+370 KB**
and the Basis transcoder **+990 KB** (and a C++ toolchain for every target), against **+30 KB**
for this container with inflate and SHA-256. The client has 1.94 MiB left under its 10 MiB cap.
BC1 is what the budget of PLAN.md 2.6 already assumes (a 1024² atlas with mips is 0.70 MB of
VRAM; uncompressed it is 5.6 MB), every desktop GPU since 2008 samples it natively, and a
client without BC support decodes it to RGBA in forty lines. glTF stays the *upload* format,
where the interoperability matters. Mobile GPUs want ETC2 or ASTC: the hub keeps every source
upload, so a second variant is a re-run of ingestion in Phase 8, not a re-upload.

## 6. The hub

### 6.1 Storage

Blobs live in a content-addressed directory (`--models-dir`; an S3-compatible store behind the
same three operations at deployment): `<id>.gmm`, `<id>.png` (the preview) and
`src/<sha256 of the upload>.glb`. Rows live in Postgres:

```
models        (hash bytea pk, frame, status, bytes, triangles, vertices, tex_w, tex_h,
               source_hash, source_bytes, uploaded_by → accounts, uploaded,
               decided_by → accounts, decided, reason_code, reason)
status:       pending | active | rejected | takedown
model_holders (hash → models, account_id → accounts, tos_version, added)     pk (hash, account)
model_events  (id, at, model, actor → accounts, event, detail)                never deleted
characters.model  bytea → models(hash)            what the character wears, or null
accounts      + moderator bool, upload_strikes smallint, model_slots smallint default 4
```

A model belongs to everyone who uploaded it (`model_holders`): two players uploading the same
file hold the same id (the insert is `on conflict do nothing`; the second uploader becomes a
holder). Status is a property of the **upload** (`source_hash`), not of a holder and not of a
frame: every model ingested from the same file shares one status, and a decision on one is a
decision on all of them. Whatever adds a frame to an upload or decides its status first takes
the upload's lock (a transaction-scoped advisory lock on the upload's hash), so a frame that
arrives during a decision is either part of it or waits for it.

### 6.2 Requests (HUB.md 3 framing; a request that carries bytes writes them raw on the same
stream after its framed message, and so does an answer)

```
ModelUpload { session, frame, tos_version, len }  + len bytes  → ModelAccepted { model, status } | Err(Invalid(report))
ModelList   { session }                                        → Models(Vec<ModelSummary>)
ModelDrop   { session, model }                                 → Ok       (give up holding it)
SetModel    { session, character, model: Option<ModelId> }     → Ok       (offline characters only)
ModelGet    { session, model }                                 → Blob { len } + len bytes
Mod         { session, op }                                    moderators only (section 10)
```

`ModelUpload` is accepted when the account has `upload_privileges`, has certified the current
terms (`tos_version`, section 10), holds fewer than `model_slots` models, has fewer than **3**
pending, has fewer than **3 strikes** (section 10), and is within **10 uploads per hour**; `len`
is checked against 8 MiB before a byte of the body is read. An account uploads **one file at a
time**, at most **8** uploads are in the hub's memory at once (the ninth is answered `Busy`),
and the body must arrive within 10 s plus its length at 64 KiB/s (138 s for 8 MiB). An upload whose SHA-256 the hub
already knows for that frame skips ingestion: the caller becomes a holder of the existing
model, or is told why it was refused. `ModelDrop` also takes the model off every character of
the account that wears it, and is refused while one of them is in a zone (what is worn does
not change in the world, section 12). A `pending` model that nobody holds any more leaves the
queue and the store with its files (its uploader withdrew it; the uploaded file goes too unless
another frame of it remains), an `active` one stays for its other holders, and a `rejected` or
`takedown` row always stays (section 10, "never again").

Ingestion runs in a child process (`gm-hub ingest-worker`), at most **2** at a time, each with a
30 s wall clock, a 20 s CPU limit and a 2 GiB address-space limit. A worker is taken only when
the whole body has arrived (a slow sender holds one of the eight places in memory, never a
worker); an upload waits up to 30 s for one and is then answered `Busy`, which does not count
against the hourly limit. A worker that crashes or times out refuses the upload and nothing
else: the hub never parses a glTF, a PNG or a JPEG itself. What a small file may ask for is
bounded inside the worker as well: 4,096 nodes, 200,000 vertices and 200,000 triangles over
all instances of all meshes, and the reader stops after 64 problems.

**The hub believes the bytes, not the worker.** The worker has parsed a hostile file and may
have been subverted by it, so its answer is treated as a second upload, in our own format: the
`.gmm` it hands back (a regular file, at most 1.5 MiB) is read with the strict reader of
section 5, must be the canonical encoding of what it decodes to (one model, one id), must be
for the requested frame, and passes the pose and envelope rules of section 4 again, in the hub,
from those bytes alone (`gm_ingest::verify`). The id, the facts in the database and the preview
a moderator judges are all computed by the hub from the stored bytes; the worker's report is
only the text shown to the uploader. A worker that says "accepted" and returns garbage, another
frame's model, a shrunken model or a link to another file refuses one upload and stores
nothing.

A new model is `active` at once when the uploader's `trust_tier` is **2 or more**, otherwise
`pending` until a moderator decides (or whatever its siblings from the same upload already
are). `SetModel` needs a model that is `active`, held by the account, and ingested for the
character's frame; it locks the model row, so it cannot interleave with a takedown.

`ModelGet` serves an `active` model to any session; a `pending` or `rejected` one to its
holders and to moderators; a `takedown` one to moderators only. Per session: 8 MiB/s with a
128 MiB burst.

The hub's endpoint uses QUIC's loss-based congestion control (Cubic), not the fixed 64 KiB
window of the zones (PROTOCOL.md 1): that window is right for a few KB/s of datagrams and
would cap a download at 640 KB/s on a 100 ms path.

### 6.3 What zones are told

`Claimed` carries `model: Option<ModelRef { id, frame }>`, present only while the model is
`active`. A model leaves `active` only by a takedown (a rejection applies to `pending` models,
which no zone was ever told about): the hub then sends `HubNotice::ModelRevoked { model }` to
every zone, after the transaction commits.

## 7. The zone

A zone never sees model bytes. It keeps each player's `ModelRef` and announces the id while the
player's current frame equals the model's (a respec to another frame shows the mannequin until
the player respecs back):

```
Control::Roster(Vec<PlayerEntry { id, name, team, model }>)   to a joiner: everyone here, one message
Control::PlayerInfo { id, name, team, model: Option<[u8; 32]> }  a join, or a change of what is worn
Control::ModelRevoked([u8; 32])                     to everyone: forget it, delete it
```

Model ids ride on the reliable stream, never in snapshots: 32 bytes per first sight would not
fit a datagram with a hundred players in view. A zone remembers a revoked id for **60 s** and
strips it from anyone who joins meanwhile: a claim that raced the takedown cannot bring the
model back.

## 8. The client

**Lookup order**: GPU cache → disk cache → hub. Until a model is on the GPU its wearer is the
mannequin.

**Disk cache**: one directory (`--cache-dir`, default the platform's cache directory +
`gamengine/models`), one file per model named by its id in hex. A file is written to a
temporary name (with the process id, so two clients may share the directory) and renamed. On
every read the SHA-256 is recomputed; a file that does not match
its name is deleted and fetched again. The cache has a byte cap (`--cache-mb`, default **2,048**
desktop; 500 mobile, PLAN.md 2.9; never under 16 MiB). Room is made *before* a file is written:
files are deleted least recently used first (the file's modification time, touched on use)
until the new one fits, and insertions happen one at a time, so the directory is never larger
than the cap, temporary files included. (Two clients sharing one directory see each other's
writes by the directory's modification time; between them they can exceed the cap by the one
file each is writing at that moment.) The own
character's model is **pinned** and evicted last; guild avatars join the pin set when guilds
reach the client, and the pin set is bounded to half the cap, so it can never be the cache. The
cap holds at all times, including while more models are in view than fit: those stay drawn
from memory and are fetched again next session.

**GPU cache**: a hard cap (`--vram-mb`, default **256**). Models not drawn for the longest time
are unloaded first; when everything loaded is in view and the next model does not fit, it is
not loaded and its wearer stays a mannequin: nearest wearers win.

**Fetching**: at most **4** downloads at a time, nearest wearer first, each given 120 s. A
failed fetch is tried again after 2 s, then 4, doubling to 60 s, for as long as somebody in
view wears the model; a model nobody has worn for 240 frames is forgotten. A model the hub
refuses is not asked for again this session. Fetched bytes are hashed before anything parses
them. A model that does not fit under the GPU cap waits the same way.

**Without BC support** the texture is decoded to RGBA8 on the CPU and uploaded uncompressed,
without its largest mip (a quarter of the memory: 1.4 MB instead of 5.6 MB for a 1024² atlas).

**Revocation**: on `ModelRevoked` the model is dropped from the GPU cache, deleted from the
disk cache and never requested again this session, also when the takedown arrives while the
model is being downloaded (the file the download writes is deleted when it lands).

## 9. Drawing and animation

One draw call per character (PLAN.md 2.6): one vertex buffer, one index buffer, one texture,
and one block of a shared uniform buffer (24 skinning matrices, the position scale, a tint and
the light) selected by a dynamic offset. Alpha is a cutout, never a blend. Shading is the
texture times a half-lambert term times the ambient light of the place the character stands,
read from the map's lightmap under its feet.

The animation set is the game's and is procedural in v1: a function from the snapshot's `anim`
state (VOCABULARY.md 14: idle, run, air, windup, swing, recover, dash, dead, guard, parry, cast,
stagger) and time to 24 bone rotations, cross-faded over 120 ms on a state change. It is
placeholder art (PLAN.md 9: animation is human work); the contract is that it is **shared**.

The mannequin is generated from the rig (jointed segments, 784 triangles), tinted by
armour class: cloth pale, leather brown, mail steel, plate dark iron.

What a custom model must not hide is drawn by the client around it: **armour class is in the
gait** (the heavier the class, the longer and heavier the stride, the smaller the arm swing:
MATRIX.md 1, "how the body moves"), **aspects are a ring at the feet** in the element colours
(two halves for a dual aspect), and the team is a pip above the head.

## 10. Moderation and takedown (PLAN.md 7)

**Terms** (version 1; a draft for counsel to review before launch). By uploading, the account
holder certifies: *"I made this model or hold the rights to use it here. It does not depict a
real person without their consent. It is not sexual, hateful or otherwise unlawful. I
understand it may be refused or removed, with a stated reason, and that repeated violations end
my upload privileges."* The hub records the terms version with every holder row.

**Queue.** `Mod { op: Queue }` lists pending models oldest first, with the uploader, the facts
and the preview. `Decide { model, approve, code, reason }` moves `pending → active | rejected`.
Reason codes: `copyright`, `likeness`, `sexual`, `hateful`, `other`.

**Takedown.** `Takedown { model, code, reason, reference }` moves `active → takedown` in one
transaction: every character wearing it wears nothing, the hub stops serving and announcing it,
every zone is told (`ModelRevoked`) and tells its clients, which revert to the mannequin within
the tick. The uploader reads the code and the reason in `ModelList` (the statement of reasons).
`Reinstate { model, reason }` (a counter-notice) moves `takedown → active`; nobody wears it
again until they choose to.

**Repeat infringers.** A rejection or a takedown with any code but `other` is a strike on every
holder of the upload; an account with **3** strikes cannot upload. Reinstatement takes the
strike back, and with it the bar; a moderator can clear strikes.

**Never again.** A `rejected` or `takedown` row is kept, so the same upload is refused at once,
by the hash of the uploaded file, for every frame and without a second look. The bytes are
kept (not served) for appeals; purging them is an operator's decision.

**Records.** Every upload, decision, takedown, reinstatement and privilege change is a
`model_events` row with the actor: the audit trail the DSA transparency report is drawn from.

Moderators are accounts with `accounts.moderator`; the first is made with
`gm-hub --grant-moderator EMAIL`. Tools: `gm-tools model …` (creators) and `gm-tools mod …`
(moderators) speak the hub protocol; there is no HTTP surface.

## 11. Budgets and acceptance (PLAN.md 11.8 Phase 6)

`scripts/check-avatars.sh` is the gate; its numbers are `budgets.toml [avatars]`. The avatars
are generated (`gm-tools model synth`): distinct, at the budget ceiling (3,486 triangles, a
1024² atlas, 382 KB ingested, 780 KB on the GPU each).

- **Offline crowd** (no arguments; `--gate-fps` on the reference iGPU): the town with 100
  distinct avatars served from a directory, 1280 × 720, with a cache cap of 32 MiB, smaller
  than the 38.2 MB of models, so the run evicts. Every model must be drawn; the average must
  be at least 60 fps and the 99th percentile frame under 16 ms; RSS under `[client]`'s 200 MiB;
  the cache directory, sampled every 50 ms while the client runs, never above the cap. CI runs
  `--software`: 48 avatars against a 16 MiB cap under a software Vulkan adapter (no fps, no
  RSS: its video memory is process memory).
- **Online** (`--online`, needs a Postgres it may wipe and a display): the real path. A hub
  with its ingestion worker, 100 accounts each uploading its own `.glb`, a moderator approving
  them, a town zone at 20 Hz, 100 bots each wearing its model, twelve of them opening stalls,
  and the windowed client fetching every model from the hub under the 32 MiB cap, with one
  model taken down while it watches. The gate also bounds the zone's bytes per player and the
  bots' unexplained corrections (the netcode budget, one per 10 s).
- **Hub** (`crates/gm-hub/tests/models.rs`, real Postgres, the real worker process): upload →
  pending → approve → wear → claim carries it → takedown reverts it and tells the zone; an
  over-budget, an invisible and a malformed upload are refused with their reasons; a rejected
  upload cannot come back; a worker that crashes, hangs, is missing or lies refuses one upload
  and leaves the hub serving.
- **Zone** (`crates/gm-server/tests/town.rs`): six bots wearing models in the town, two of them
  racing for the same market tile, a takedown in the middle and a late arrival.

**Measured 2026-10-01** on the reference machine (Ryzen 7 4800U, Radeon Vega iGPU, RADV;
1280 × 720; PLAN.md 11.10 has the full list):

| | Offline, 100 avatars | Online, 100 avatars + viewer |
|---|---|---|
| Frame rate, average | 564–678 fps over eight runs (uncapped) | 242 fps (the client's 250 fps cap) |
| Frame time, 99th percentile | 2.0–2.6 ms | 6.1–6.2 ms |
| Client peak RSS | 141 MB | 146–148 MB |
| Triangles per frame, draw calls | 348,600 in 102 calls | 349,384 in 103 calls |
| Model memory on the GPU | 78.0 MB (779,628 bytes each) | the same |
| Fetched | 38,159,385 bytes (381,594 each) | the same, from the hub |
| Cache directory, largest seen (cap 33,554,432) | 33,218,927 bytes | 33,295,020 bytes |
| Zone → client, per player (20 Hz, 101 in view) | | 4.0–5.8 KB/s (13.4 KB/s at 64 Hz) |

## 12. Deliberately absent

No animations, morphs or shaders in uploads. No blended transparency. No per-model hitbox. No
re-posing of A-pose uploads yet (ingestion can learn it without touching the format). No
public model library (PLAN.md 7: we never supply or promote presets we did not make). No
revocation list for caches that were offline: a revoked id is never announced again, so a stale
file is never drawn and ages out of the LRU. No change of model while in a zone. No showing a
pending model to its owner in the world (`gm-client --avatar FILE` previews one offline).

## 13. Proposed numbers and review log

**Proposed (director to confirm):** the envelope (head pivot ±8%, highest vertex 0.90–1.15 h,
box 2.5 r × 0.70 h, coverage 50–150% of the mannequin, limbs within 15° of the T-pose); 4 model
slots per account; trust tier 2 skips the queue; 3 strikes; 10 uploads per hour and 3 pending;
the 8 MiB upload cap; the 256 MiB GPU cache; who receives `upload_privileges` (today: a
moderator grants it).

**2026-10-01, v1 draft reviewed by Gemini 3.1 Pro** (independent review before implementation;
verdicts are ours):
- Accepted: the hub inherited the zones' fixed 64 KiB congestion window, which caps a download
  at 640 KB/s on a 100 ms path (the hub endpoint uses Cubic); a taken-down upload could come
  back by naming another frame, because the frame byte changes the id (status now belongs to
  the uploaded file, across frames); `ModelDrop` freed a slot without taking the model off the
  account's characters; "a model in view is never unloaded" let a hundred uncompressed atlases
  pass the GPU cap on a device without BC (the cap is hard, nearest wearers win, and the
  uncompressed path drops the largest mip); a guild's pin set could outgrow the disk cap (pins
  are bounded to half of it); reinstatement returned a strike but not the privilege (the strike
  count is the bar, nothing else is flipped); two clients sharing a cache directory wrote the
  same temporary file (the name carries the process id); a model lying face down passed the
  limb checks (the body must be upright).
- Accepted in part: an infiltrator could dress as a colossus inside the old height band. The
  body height is now the head pivot within ±8% and bulk is capped at 150% of the mannequin's
  coverage; the bands of neighbouring frames still touch, as the frames themselves do
  (striker 14 × 56, caster 13 × 56).
- Accepted as a cut: re-posing A-pose uploads at ingestion (v1 refuses them; section 12).
- Rejected: dropping the coverage rule in favour of moderation (an avatar that is 30% thinner
  is a combat advantage no moderator can judge by eye, and the same rasteriser draws the
  moderation preview); dropping the trust tier (the column exists and the rule is three lines);
  dropping `gm-client --avatar` (it is also how the renderer is tested and benchmarked without
  a hub).
- Found by the author while answering the review: a custom model hides the armour-class tint
  of the mannequin, so armour class moved into the gait and aspects into a ring the client
  draws; a claim racing a takedown could re-announce the model (zones remember a revoked id for
  60 s).

**2026-10-01, the implementation reviewed by Gemini 3.1 Pro** (two rounds of three scoped
reviews: ingestion, hub, client and zone; the second round on the fixed code; verdicts are
ours, every accepted finding has a test unless noted):
- Accepted, ingestion: a mesh of three vertices and 600,000 indices instanced by 4,096 nodes
  asked the worker for 2.4 billion indices (totals are now bounded over all instances and the
  reader stops at 64 problems).
- Accepted, hub: the worker permit was held while the body was read, so two slow senders
  stopped all ingestion for 30 s at a time (a worker is taken when the body is complete; one
  upload per account, eight bodies in memory, a body deadline that scales with its length);
  the hourly token was spent on a `Busy` (refunded); two frames of one file uploaded at the
  same moment by a trusted and an untrusted account could get different statuses, and a frame
  inserted while a decision's `select ... for update` was waiting was missed by it (the
  upload's lock; no test can force the second interleaving from outside); `ModelDrop` took the
  model off a character that was in a zone without the zone hearing of it (refused while worn
  in the world; the reviewer proposed leaving it on, which would have a character wear what
  its account does not hold); a withdrawn upload left its `.glb` behind (swept, together with
  the files of an upload refused at the last step).
- Accepted, client: the fetch back-off never doubled, because the delay was read from an
  entry that had already become `Loading`; a failed model was fetched again forever after
  its wearer had left; a takedown that arrived during the download was undone by the
  download's end (the file stayed on disk, or a failure put the model back in the retry
  queue); without BC support the GPU budget counted the compressed size of a texture that is
  held uncompressed.
- Accepted, zone: `StallOpen`, `StallClose` and `Travel` each cost the hub a request per
  message (one in flight and one per second per player, the rest dropped); a player claimed
  by another zone was announced as gone twice and counted twice (Phase 4 code).
- Rejected: "two players racing for one tile get two stalls" (the hub's unique constraint
  decides, `Taken`; tested in `economy_protocol.rs` and `town.rs`); "a UV of exactly 1.0
  reads past the texture" (`bc1::texel` clamps to the edge; the reviewer had not been given
  that file). Its side remark stayed: the release profile aborts on a panic and `verify` runs
  in the hub, so `verify` is now fed 200 random containers that the strict reader accepts and
  must judge every one.
- Found by the author while measuring and reviewing: four loader threads each made room for
  one file and together took the cache **405,512 bytes past its cap** (33,959,944 against
  33,554,432 in the first capped run; insertions are one at a time and room is made before
  the write, and the gate samples the directory every 50 ms); the hub believed whatever the
  worker wrote back (`gm_ingest::verify`, section 6.2); a reliable message was dropped
  silently when a client's queue was full, so a missed `ModelRevoked` would have left a client
  drawing a model taken down (the client is disconnected instead); a market of more than about
  880 stalls would not have fitted the joiner's `Stalls` message (512 tiles per map, measured
  38,167 bytes of the 65,535; tile numbers unique across a map's grids); a loader thread waited for ever on a stalled stream.
