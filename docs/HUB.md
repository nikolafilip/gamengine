# Hub: accounts, characters, zones, handoff

Status: v1.4 (Phase 8: the web listener, section 3.6; Phase 4; the economy requests of Phase 5; models, stalls in the world and the
saved position's zone of Phase 6; squads, trials and gated zones of Phase 7, section 3.5). This document is the contract between `gm-hub`, `gm-server` and the
clients for everything that outlives a zone process: accounts, characters, where a character is,
and how it moves between zones. PLAN.md 2.1 (Postgres via sqlx, in-memory session state), 11.3
(hub-directed handoff with a ghost until the ack), 11.4 (data model) and 11.7 (one zone process
per map, one hub) are binding. When the code and this document disagree, the document wins.
Section 8 records the independent design review this version went through.

## 1. Principles

1. **The hub owns identity and persistence; zones own simulation.** A zone never touches the
   database. Everything a zone needs about a character arrives in one message when the character
   is claimed, and everything it changed goes back in one message when the character leaves.
2. **Zones verify, the hub issues.** Entry to a zone is a short-lived, single-use, zone-bound
   token signed by the hub (ed25519). A zone can verify it offline and asks the hub only to
   *claim* the character, so a stolen token is worthless after one use and never valid elsewhere.
3. **One transport.** The hub speaks the same QUIC the zones speak (`quinn` natively, WebTransport
   in the browser), with `bitcode` messages on bidirectional streams, one request per stream. The
   client binary gains no HTTP stack; the browser build needs nothing new.
4. **Every state change is one transaction.** A character is in exactly one place at a time
   (`characters.location`), moved only by the hub inside a transaction; the ledger of Phase 5
   builds on the same rule.
5. **Measured.** 200 bots on one zone must leave half the tick free; the login → zone → handoff
   → logout round trip is timed and gated.

## 2. Processes

- `gm-hub`: one process per deployment. Listens on one QUIC endpoint for clients and zones,
  holds the zone registry in memory, talks to Postgres through `sqlx`. Migrations ship with the
  binary and run at start (`gm-hub --database-url ... [--migrate-only]`).
- `gm-server`: one process per zone. At start it connects to the hub (`--hub ADDR --hub-cert
  PATH --zone-secret S --zone-id NAME --public-addr ADDR`), registers, heartbeats every 5 s, and
  keeps the connection as its control channel to the hub. Without `--hub` it runs **open** as in
  Phases 2 and 3 (empty tokens accepted; builds live for the session).
- `gm-client` / `gm-bot`: `--hub ADDR --hub-cert PATH --user EMAIL --password PW
  [--character NAME] [--zone ID]`; they log in, pick a character and a zone, receive a ticket,
  and connect to the zone with it. `--connect` without a hub still works for development.

## 3. Hub protocol

Transport: QUIC with the idle timeout and keep-alive of PROTOCOL.md 1 (10 s, 2 s), **1,024**
concurrent bidirectional streams per connection (a zone saving thirty characters at once must
never block on a stream limit) and, since Phase 6, QUIC's loss-based congestion control (Cubic)
instead of the zones' fixed 64 KiB window: that window is right for a few KB/s of datagrams and
would cap a model download at 640 KB/s on a 100 ms path (MODELS.md 6.2).
Certificates: the hub presents a self-signed certificate written at start (`--cert-out`);
clients and zones trust exactly that file. Every request is one bidirectional stream: the
requester writes one framed `HubRequest` and finishes; the hub writes one framed `HubResponse`
and finishes. Framing is PROTOCOL.md 8 (big-endian u16 length + `bitcode`). Messages over
65,535 bytes are protocol errors. Two requests carry bytes that are not messages (MODELS.md
6.2): `ModelUpload` is followed on the same stream by `len` raw bytes, checked against the
limit before one of them is read, and the answer to `ModelGet` and `Mod(Preview)` is
`Blob { len }` followed by `len` raw bytes. A zone's connection also carries hub-initiated
notices on unidirectional streams opened by the hub (`HubNotice`, section 3.4).

```
enum HubRequest {
    // anyone
    Register { email: String, password: String },
    Login { email: String, password: String },
    // accounts (session: the Login response's session id, 24 h, in-memory)
    Characters { session: SessionId },
    CreateCharacter { session: SessionId, name: String, build: BuildChoice },  // preset name or full build
    SetBuild { session: SessionId, character: CharacterId, build: BuildChoice },
    ListZones { session: SessionId },
    Trials { session: SessionId, character: CharacterId },       // what it has passed (3.5)
    Enter { session: SessionId, character: CharacterId, zone: ZoneId },
    Logout { session: SessionId },
    // zones (authenticated by the zone secret in ZoneHello, once per connection)
    ZoneHello { secret: String, zone: ZoneId, map: String, map_hash: u64, addr: SocketAddr, cert_der: Vec<u8>,
                requires: Vec<String> },         // trials that open the zone; empty = open to all (3.5)
    Heartbeat { players: u32, tick_mean_us: f32 },
    Claim { token: SessionToken },
    Save { character: CharacterId, state: CharacterState, leaving: bool },
    Handoff { character: CharacterId, state: CharacterState, to_zone: ZoneId },
    Trial { character: CharacterId, trial: String, secs: u32 },  // a character here passed it (3.5)
    // the economy (ECONOMY.md): a session for one of its own characters; a zone for itself
    Econ { session: SessionId, character: CharacterId, op: EconOp },
    ZoneEcon(ZoneEconOp),
    // avatar models (MODELS.md 6.2, 10)
    ModelUpload { session: SessionId, frame: u8, tos_version: u16, len: u32 },   // + len bytes
    ModelList { session: SessionId },
    ModelDrop { session: SessionId, model: ModelId },
    SetModel { session: SessionId, character: CharacterId, model: Option<ModelId> },
    ModelGet { session: SessionId, model: ModelId },
    Mod { session: SessionId, op: ModOp },      // moderators: Queue, Preview, Decide, Takedown,
                                                // Reinstate, SetUpload, SetTrust, ClearStrikes
}

enum HubNotice {                   // hub → zone, unidirectional streams
    Claimed { character: CharacterId },
    Kick { character: CharacterId, reason: String },
    ModelRevoked { model: ModelId },            // to every zone (MODELS.md 6.3)
    StallClosed { stall: i64 },                 // to the stall's zone (ECONOMY.md 7)
    HireEnded { hirer: CharacterId, hire: i64 },// to the hirer's zone (3.5)
}

enum HubResponse {
    Ok,
    Err(HubError),                 // typed: Credentials, Taken, NotFound, Busy, Unauthorized,
                                   // Invalid(String), Internal, Insufficient, Full, Cooldown,
                                   // Gone (taken down, not served), Locked(trials) (3.5)
    Session { session: SessionId, account: AccountId },
    Characters(Vec<CharacterSummary>),
    Character(CharacterSummary),
    Trials(Vec<(String, u32)>),    // trial key, best time in seconds
    Zones(Vec<ZoneSummary>),       // id, map, players, address, cert hash, up for ms
    Ticket(ZoneTicket),            // addr, cert_der, token
    Claimed { character: CharacterId, name: String, state: CharacterState, team: u8,
              model: Option<ModelRef>,    // what the character wears, while it is active
              squad: Vec<HiredAvatar> },  // its active hires (3.5)
    Registered { public_key: [u8; 32] },  // the hub's token verification key
    Econ(EconReply),               // Done, Id, Ids, Holder, TradeView, Trade, Decided, Tavern,
                                   // Squad, Stall, Stalls
    Models(Vec<ModelSummary>),
    ModelAccepted { model: ModelId, status: ModelStatus },
    Blob { len: u32 },             // + len bytes
    ModQueue(Vec<ModEntry>),
}
```

`SessionId` is 16 random bytes; sessions live in hub memory for 24 h or until `Logout`.
`CharacterId`, `AccountId` are database ids (i64). `ZoneId` is the zone's configured name.
Character names follow PROTOCOL.md 8 (1..=24 bytes of printable UTF-8, trimmed) and are unique
case-insensitively; an account holds at most **10** characters. Password hashing runs on the
blocking pool behind a semaphore of 8 permits; a ninth concurrent `Register`/`Login` answers
`Busy` rather than stalling the executor that carries the zones' heartbeats. `Register` and
`Login` are rate limited per source address (a token bucket of 10 per minute).

`EconOp` and `ZoneEconOp` are listed in `gm-hub-proto::protocol` and specified by ECONOMY.md:
every op is one database transaction. `Econ` is refused with `NotFound` unless the character
belongs to the session's account. `ZoneEcon` is refused with `Unauthorized` on any connection
that has not said `ZoneHello`; a zone grants only to characters playing in it and decides only
contracts whose instance it is.

`Logout` while the character is in a zone also tells that zone to drop the player
(`HubNotice::Kick`); a session cannot be used to leave a character playing after the account
has logged out.

### 3.1 Tokens

```
SessionToken {
    payload: TokenPayload { account, character, zone: ZoneId, issued_at, expires_at, nonce: [u8; 16] },
    signature: [u8; 64],            // ed25519 over the bitcode-encoded payload
}
```

- Valid for **60 s** from issue; bound to one zone; the nonce is single-use (the zone keeps the
  nonces it accepted until their expiry). Zones allow **5 s** of clock skew on the expiry check;
  NTP on every host is a deployment requirement, not a protocol feature.
- The hub's signing key is persisted (`--key PATH`, created on first start) so a hub restart
  does not invalidate tickets issued a moment before it; zones re-read the public key from
  `Registered` on every reconnect.
- The zone verifies the signature against the hub's public key (from `Registered`), the zone id,
  and the expiry **before** it answers `Hello`; then it sends `Claim` to the hub. The hub moves
  the character to the zone in one transaction and returns the state. A character that is in
  another zone cannot be claimed; a character `in_transit` can be claimed only by the zone the
  ticket names. The zone ignores `Hello.name` when a token is present: the name comes from
  `Claimed`.
- A zone without a hub (`--open`) accepts the empty token as in Phase 2.

### 3.2 Character state

```
CharacterState {
    build: Build,                   // MATRIX.md 9, validated at every write
    zone: Option<ZoneId>,           // where the position belongs; None = spawn
    position: [f32; 3], yaw: f32,
    viewport: u8,                   // last viewport preference
    play_seconds: u32,
}
```

The zone saves every **30 s**, on `Bye`, on disconnect, on handoff, and when it stops. The hub
accepts a `Save` or `Handoff` only from the zone the character is in (`location.zone` equals the
zone authenticated by `ZoneHello`); a late save from an origin zone after a handoff, or a rogue
zone writing someone else's character, is refused with `NotFound`. Streams are unordered, so this
rule, not arrival order, is what keeps the location right.

`zone` is the zone whose map the position is on (the column `pos_zone`, written with every
save), not where the character is: a claim hands the position to a zone only when it is that
zone's own, and the zone uses it only when it is still a place to stand on its map (otherwise
the character spawns). **[CORRECTED in Phase 6]** Until then the hub compared against
`location_zone`, which is null while offline and names the origin during a transit, and a zone
trusted whatever it was given: a first entry and every arrival from another zone were placed at
the saved coordinates of the previous map (the origin for a new character), usually inside a
wall. The Phase 4 round trip did not notice because its assertions were about the database;
it now also requires the bots to cover ground in both zones.

### 3.3 Handoff

1. Client → zone: `Control::Travel { zone: ZoneId }` (new control message).
2. Zone → hub: `Handoff { character, state, to_zone }`. The hub checks the target is registered,
   marks the character `in_transit(from, to)` in one transaction, and answers `Ticket` for the
   target zone.
3. Zone → client: `Control::Travel { ticket }`. The player's body becomes a **ghost**: it stays in
   the zone (visible, can be hit, cannot act) for at most **10 s**.
4. Client: `Bye` to the origin zone, connect to the target with the ticket's token, `Hello`.
5. Target zone: verifies, `Claim`s; the hub moves the character `in_transit → in target zone` and
   tells the origin zone `Control`-side (`HubNotice::Claimed { character }` on the zone's hub
   stream) to drop the ghost. If no claim arrives within 10 s the origin zone keeps the player
   where it was (the hub reverts `in_transit` to the origin zone when the origin zone saves next).

A character in transit cannot `Enter` any zone except with the ticket it was issued, unless the
transit is older than **15 s**: then the hub treats it as abandoned (the origin zone died or the
client never arrived) and `Enter` moves the character directly to the requested zone, at its
spawn. Nothing can wedge a character forever.

### 3.4 Hub notices

The hub opens a unidirectional stream to a zone for each notice: `HubNotice::Claimed {
character }` (drop the ghost), `HubNotice::Kick { character, reason }` (the account logged
out, or an operator removed it), `HubNotice::ModelRevoked { model }` (to every zone: a takedown,
MODELS.md 6.3), `HubNotice::StallClosed { stall }` (to the stall's zone: its owner closed it
or its 48 h ran out, ECONOMY.md 7) and `HubNotice::HireEnded { hirer, hire }` (to the zone the
hirer plays in: the avatar's owner took it back, or the hirer dismissed it). Notices are
advisory for the zone's bookkeeping; the database is already updated when they are sent.

### 3.5 Squads, trials and gated zones (COMPANIONS.md 3.3, 10, 11)

- **The squad at the claim.** `Claimed.squad` lists the character's active hires, oldest
  first, at most the squad capacity of its build (three, five with a leadership ability):
  `HiredAvatar { hire, character, name, build, model, expires_at }`. The zone spawns a
  companion for each; a hire whose stored build no longer validates against the content is
  left out. The same claim ends every active hire *of* the claimed character as an avatar
  (its owner is playing it now, ECONOMY.md 11) and tells the hirers' zones.
- **Trials.** `Trial { character, trial, secs }` is accepted from a zone only for a
  character playing in it and only for a trial the content gives to that zone's map; it is
  stored once per character and trial with the fastest time (`trials`). `Trials` answers a
  session what one of its characters has passed.
- **Gated zones.** A zone that registers with `requires` (trial keys of the content; at most
  16) is entered only by characters that have passed one of them: `Enter` and `Handoff`
  answer `Locked` with the trials' names otherwise.
- **A kill** is reported with `ZoneEconOp::GrantKill` (ECONOMY.md 9): one transaction, once
  per `(zone, reference)`.

### 3.6 Browsers (WEB.md 2)

- `gm-hub --web-listen ADDR [--web-cert PEM --web-key PEM] [--web-url URL] [--web-origin
  ORIGIN]... [--web-info-out FILE]` opens a WebTransport listener beside the QUIC endpoint.
  A session's bidirectional streams carry the same requests with the same framing, one per
  stream; uploads and downloads write their bytes raw on the stream as before. Zones keep
  talking QUIC.
- `ZoneHello` gains `web: Option<WebAddr>`: the zone's own WebTransport listener. `ZoneTicket`
  carries it on, so a ticket names both ways into a zone; a browser needs `web`, a native
  client `addr` and `cert_der`.
- Rate limits and sessions do not know the transport. The `Origin` allow-list of the web
  listener is not authentication (WEB.md 2.1).

## 4. Database (PLAN.md 11.4)

```
accounts   (id bigserial, email text unique, password_hash text, created timestamptz,
            trust_tier smallint default 0, upload_privileges bool default false)
characters (id bigserial, account_id → accounts, name text unique, build jsonb, location jsonb,
            viewport smallint, play_seconds int, created, updated)
zones_log  (id bigserial, zone text, event text, at timestamptz)        -- registry events, ops
```

`location` is typed, not free JSON, because "exactly one place" is an invariant the database
must hold: columns `location_kind` (`offline` | `zone` | `transit`), `location_zone` (the zone,
or the transit origin), `transit_to`, `transit_since`, `position` (three reals), `yaw`, with a
check constraint that the zone columns are null exactly when offline. Builds are stored as JSON
for inspection; the hub validates them against the content pack it loads at start (the hub and
every zone of a deployment load the same `assets/content`, and the hub refuses a zone whose
`map_hash`/content differ from what it knows for that zone id). Items, stalls, escrow and the
ledger arrive in Phase 5 as separate tables (ECONOMY.md); Phase 6 adds `models`,
`model_holders`, `model_events`, `characters.model` and three account columns (MODELS.md 6.1),
and `characters.pos_zone` (section 3.2); Phase 7 adds `hires.ended`, `trials (character_id,
trial, zone, secs, passed_at)` and `kills (zone, ref)` (migration 0005). Migrations are embedded in the binary and run at
start in order; the hub refuses to start on an unknown newer schema.

Passwords: argon2id with the crate defaults (19 MiB, 2 iterations, parallelism 1), one hash per
account, never logged. Emails are stored lowercase and trimmed. A `Register` with an existing
email answers `Taken` without timing leaks worth defending in Phase 4.

## 5. Budgets and acceptance (PLAN.md 11.8 Phase 4)

- **200-bot swarm** (`scripts/check-swarm.sh`, real UDP on loopback, one zone on the arena, 200
  duelist bots for 20 s, everyone in one hall so nothing is culled, the worst case): server tick
  mean under `[server].max_tick_mean_us` and p99 under `max_tick_p99_us` from `budgets.toml`,
  server RSS under `max_rss_bytes`, per-player bytes under the Phase 2 budget. Numbers are set
  from the first measurement with margin, like the netcode gate. The gate runs on the reference
  machine and the self-hosted runner; CI runs a 32-bot smoke with the same assertions.
- **Round trip** (`crates/gm-hub/tests/handoff.rs`, loopback: one hub on a test database, two
  zones): register → login → create character → enter zone A → travel to zone B → logout, with
  the character's location checked in the database at every step; the whole trip under
  `[hub].round_trip_ms`. The test is event-driven (it waits for `Travel`, `Welcome`, `Claimed`),
  never sleeps. It needs `GM_TEST_DATABASE_URL`; without it the test is skipped with a message
  (CI provides a Postgres service).
- Every zone state change is a transaction: a `Claim` for a character already in a zone fails,
  and a `Save` from the wrong zone fails.

## 6. Deliberately absent

No HTTP API: the tools speak the same protocol (`gm-tools model`, `gm-tools mod`,
`gm-tools hub`; the first moderator is made with `gm-hub --grant-moderator EMAIL`). No OAuth, no email
verification, no password reset (Phase ∞, with the website). No zone-to-zone direct links: every
move goes through the hub. No sharding of the hub.

## 7. Open

Whether zone secrets become per-zone certificates signed by the hub. (Resolved in Phase 6: the
asset ingestion API lives on the same endpoint, bytes raw on the request's own stream.)

## 8. Implementation notes (Phase 4)

`gm-hub-proto` (GPL, what clients link): `protocol` (the messages), `token` (ed25519 issue and
verify with a nonce memory), `client` (the connection zones, bots and the client use). `gm-hub`
(AGPL): `db` (sqlx, embedded migrations, every move one transaction) and `hub` (the process).
`CreateCharacter` and `SetBuild` take a preset name or a full build, so clients carry no content
parser. `gm-server` gains `hub_link` and the zone loop
does saves every 30 s (staggered), ghosts, travel and notices; `--hub` makes tokens mandatory.
`gm-bot --hub ...` and `gm-client --hub ...` log in, enter, travel and log out through the hub;
the client reloads the map of a zone it travels to from `--maps-dir`.

Three things the implementation settled that the draft left implicit:
- A ghost's `Bye` keeps its body and the hub's transit; the origin zone drops the ghost on
  `HubNotice::Claimed`, or after 10 s: back to play if the client is still connected, otherwise
  the body leaves and the character goes offline (the origin may save a transit it started).
- A zone that disconnects from the hub takes its characters offline at the hub; the players
  keep playing but their next saves are refused until they re-enter. A zone restart does the
  same (`ZoneHello` orphans). This is the conservative choice; a network blip costs a re-login.
- The hub verifies tokens too (its own key, its own nonce memory per zone), so a compromised
  zone cannot claim with a token it did not receive from a client.

Measured 2026-10-01 (loopback, reference machine): the hub's share of login → enter → travel →
logout is **24 + 110 + 82 + 2 ms**; a character is claimed in the first zone 55 ms after the
flow starts and in the second zone 160 ms after the travel request; argon2id costs ~60 ms per
hash. The 200-bot swarm gate (`scripts/check-swarm.sh`) measured **6.6 ms mean / 9.1 ms p99**
per tick with 200 duelists in the arena hall, 272 MB server RSS, 20 KB/s down per player; the
snapshot pass runs on a `rayon` pool, the simulation step is single-threaded (3 ms of the 6.6).

**Phase 6** added `gm-hub::models` (the store, quotas, moderation; MODELS.md 6 and 10) and the
`ingest-worker` subcommand of the hub binary, which is the same executable started as a child
with resource limits. Measured 2026-10-01: 100 uploads of 1.17 MB each through the worker,
verification, approval and `SetModel` take 37 to 50 s on loopback (0.4 to 0.5 s per avatar);
the two model tests against Postgres with the real worker take 3 to 5 s.

## 9. Design review log

**2026-10-01, v1 draft reviewed by Gemini 3.1 Pro** (independent review before implementation;
verdicts are ours):
- Accepted: the 4-stream transport limit would deadlock a zone saving several characters (hub
  connections allow 1,024); argon2 on the executor would starve the heartbeats (blocking pool
  plus a semaphore); a late `Save` from the origin zone could overwrite a handoff, and a rogue
  zone could write any character (saves are scoped to the character's current zone); a zone
  crash mid-transit wedged the character (transits older than 15 s are abandoned); `Logout`
  left the player in the zone (`HubNotice::Kick`); an ephemeral signing key broke tickets across
  a hub restart (persisted key); clock skew (5 s tolerance, NTP required); unbounded characters
  per account (10) and names (PROTOCOL.md 8 rule); `Hello.name` could be spoofed when a token
  names the character (ignored); the round-trip test must be event-driven.
- Accepted with a different reading: "200 bots in one hall exceed the budget" is the worst case
  the gate is for; measured before this document was written, 200 duelists in the arena cost
  18 KB/s per player on the server side with 29 KB/s peaks per bot, under the 30 KB/s budget.
  The far band stays at 1/6 rate; if a real map ever breaks the budget the lever is the band
  rates, not the test.
- Correct as drafted: nonce memory, single-transaction claims, the 10 s ghost, message sizes,
  30 s crash loss, keep-alives.

**2026-10-01, implementation reviewed by Gemini 3.1 Pro** (after the round trip passed):
- Accepted, all six: a periodic `Save` sent just before a `Handoff` could arrive after it on
  another stream and revert the transit (only transits older than 5 s can be reverted by a
  save); `Logout` took the character offline before the zone's final save, losing up to 30 s
  (the kick now lets the zone save and offline it; only transits go offline at the hub); the
  10-character cap raced without a lock on the account row; the rate limiter swept its map on
  every attempt under the global lock (now once a minute); every character joined at once saved
  at once (saves are staggered by entity id); heartbeats were recorded but never read (zones
  silent for 15 s get no tickets and are not listed).
