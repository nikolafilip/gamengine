# GAMENGINE — Design & Engineering Plan

Working title: **gamengine** (rename later). Persistent action-sandbox MMORPG with multi-genre viewports,
built on a lightweight Rust engine. Last updated 2026-10-01 (Phases 0–4 implemented; see 11.10).

Sources: the engine/architecture discussion (Rust, wgpu, netcode, AI-assisted build, open source) and the
game-design discussion (economy, combat, UGC, legal, AI companions). Every finding from those conversations
was reviewed, not just copied. Section 1 is the verdict table; later sections only contain what survived.
**[DECIDED]** = settled. **[OPEN]** = needs a call. **[CORRECTED]** = the conversation said something
wrong or contradictory and the plan uses the fixed version.

---

## 0. Thesis and non-negotiables

The game is an unforgiving ecosystem, not a theme park. Design authority is a single director with the
backbone to say no. Locked, not up for softening when players complain:

- **No levels, no gear tiers, no vertical power creep.** Fixed point budget + rock-paper-scissors (RPS)
  counters. Top gear gives a 15–25% edge over standard gear, never 500%.
- **No soulbound gear.** Everything trades. Anyone can equip anything. "If your build sucks, it's on you."
- **No pay-to-win, no matchmaking gimmicks, no minimaps, no level-skips, no upgrade casino.**
  Monetization = convenience and social cosmetics only (extra build slots, model storage slots).
- **No junk loot, no trash mobs, no millions of gold.** Scarce meaningful encounters, 100% deterministic
  drops, small currency (copper / silver / gold; 1 gold is a small fortune).
- **No chore sinks.** No durability decay, repair fees, crafting tolls, upkeep, travel fees, sink taxes.
- **Skill and coordination beat numbers.** Collision, friendly fire, and terrain are real.
- **Measured in MB.** Client binary a few MB, install tens to a few hundred MB, RAM under 500 MB,
  runs on integrated GPU, 10-year-old laptops, entry-level Android, browser via WASM.
- **Open source from day one.** Official persistent world + accounts + hosted shards is the product.

Reference games, what to keep and what to discard:

| Game | Keep | Discard |
|---|---|---|
| Ether Saga Odyssey | Pet fusion (labor adds value, tradable, consumes inputs). Hand-painted 2–4k-tri art. | Imbue/infuse RNG casino, bind-on-equip, cash-shop protection stones, exponential weapon scaling. |
| Tales of Pirates | Gear economy, guilds, arena PvP, class RPS (Crusader > Voyager > Champion > Crusader), the emergent Con-Voyager, 48h stalls, forging/gem demand for base mats. | Pet/fairy system, stall lag (a 2005 renderer problem, not a design problem). |
| OSRS | Horizontal item utility, upcycling old gear into new, consumable-driven demand for gatherers. | GE sink tax (a last-resort lever, not a design pillar). |
| Pokémon | Type matrix, dual-type 4× weaknesses. Nobody owns math. | — |
| Albion / EVE / UO / Monster Hunter | No levels, "you are what you wear", cheap-gear groups kill expensive solos, horizontal progression. Albion's death-drop is the one honest non-chore item sink. | — |
| Guild Wars 1 / Dragon Quest X / Dragon's Dogma | Henchmen/heroes, offline-player tavern hire, pawns. | — |
| Savage / NS2 / Foxhole | Cross-perspective play, restricted sightlines create interdependence. | Single-commander bottleneck. |
| Darkfall / Mortal Online / UO | Collision, body-blocking, manual aim, friendly fire in 100-player sieges. | — |
| FromSoft | Refuse easy modes. | — |

---

## 1. Findings review — what I confirm, what I corrected, what I rejected

I did not accept the previous conversation wholesale. Verdicts:

### 1.1 Confirmed (sound, kept as-is)
| Finding | Why it holds |
|---|---|
| Bloat is assets, not code; baked lightmaps give 90% of the look for 1% of the cost | This is how GoldSrc/id Tech 3 worked and why they still look fine. |
| Rust + wgpu + tokio, sharded zones, interest management, authoritative server, prediction + reconciliation | Standard, proven (Veloren does exactly this in Rust). |
| Polygon budgets: CS 1.6 ~800 tris, ToP ~1.2–2.5k, Ether Saga ~2–4k; target 2.5–3.5k | Exact historical numbers are approximate, but the budget target is right regardless. |
| Draw calls and bone counts cause lag, not polygons | Correct. One atlas per character, ≤32 bones. |
| Custom-model streaming: server recompression, content-hash addressing, fixed-cap LRU, silhouette fallback, hitbox decoupling | This is how VRChat should have done it. Nothing to fix. |
| No levels / no tiers / unbound gear balanced by physics penalties instead of class locks | Albion, UO, EVE prove it. |
| Point-buy budget with strict opportunity cost; emergent cheese builds must pivot into a different counter | Correct, and it is the only way the "Speed Hulk" stays fair. |
| Modular component drops instead of one indivisible trophy (your call) | Best drama-proof option. Split rules needed corrections (see 1.2). |
| Multi-layer crafting, gems as counter-meta modifiers, 50% decomposition with cross-discipline salvage | Sound. Salvage value is what makes people actually scrap. |
| Hard storage caps + physical housing/guild halls instead of infinite banks | Sound, with the mule caveat in 1.2. |
| Escrow contracts for carries, trade-window mutation lock, grid-snapped stalls, town-board waypoints, contribution floors, immutable loot rules in combat | All standard, all correct. |
| Stall lag is solvable with instancing, dormant state, LOD, grid spacing; no tax needed | Correct. |
| Broker burn on tavern hire; hired avatars get flat coin never loot | Correct, kills self-hire farming. |
| Tactical view via hired AI, bounties, or an officer tool, never commanding humans | Correct; NS2/Savage history supports it. |
| Rejection of durability decay / repair taxes / upkeep (your call) | Correct. Those are the treadmill. |
| Mechanics and archetypes are not copyrightable; public-domain myth is free; ToS + fallback skin + takedown flow | Correct in principle; wording fixed in 1.2. |
| Multi-point randomized lairs + open-world PvP + spawn variance against camping | Correct. |

### 1.2 Corrected (the idea is right but the stated version was wrong or contradictory)
| Original claim | Problem | Plan uses |
|---|---|---|
| Third-person line-of-sight masking: "if the avatar's eyes cannot raycast to the enemy, the server does not render it" | (a) It contradicts your own decision that corner-peeking IS the third-person advantage. (b) Servers do not render, they send. (c) Per-pair raycasts are O(N²) and late pop-in creates a worse peeker's advantage than the one it removes. | **PVS-based server culling.** Quake BSP compilation gives a Potentially Visible Set per leaf for free. The server sends only entities in the PVS of the player's own position. Peeking around a corner with the camera still works (same visible cluster). Wallhacks/ESP through solid geometry are neutralized. Cheap, correct, and consistent with your design. |
| "Friendly fire neutralizes deathballs, no AoE diminishing returns needed" | True against uncoordinated blobs. A coordinated 20 with voice comms still beats 8, and that is fine. But FF with no scope rule invites team-killing griefers in public groups and has no meaning in open world where "friend" is undefined. | FF is on for **all** AoE and projectiles everywhere. Scope: party/guild/alliance are "friendly" only for targeting UI, never for damage. Instanced PvE with randoms: FF stays on (it is the skill check), but TK statistics feed the same reputation ledger as contracts. |
| Contribution split between competing parties, 60/40 | (a) A third party can tag for 5% and leech shards without ever fighting. (b) Contradicts "wipe them to take it all": does a wiped party still get its 40%? (c) With few shards and several parties, rounding gives someone 0. | Only parties with at least one living member in the arena at kill time are eligible. Shares are proportional to contribution among eligible parties, minimum one shard per eligible party that crossed the contribution floor. Wiped = 0. |
| "Attunement ramp: drop quality rises the longer the boss roams" and "anti-zerg radial enrage" | Contradicts 100% deterministic drops; rewards not killing; the dominant scouting guild benefits most from waiting. The enrage is an artificial numbers-punisher; FF and collision already do that job. | **Both removed.** Camping is handled by random lairs, PvP contention, spawn variance, and RPS boss profiles. |
| "Off-peak sweep exploit" and "AFK alts inflate concurrency" | Your rebuttal is right: uncontested off-peak is an invitation to show up. AFK alts in a danger zone are also free kills. | Spawn budget counts players who moved or attacked in the last N minutes in danger zones; high-variance windows. Nothing else. |
| Storage caps stop hoarding | Trivially bypassed by alts and mule accounts on a free-to-play game. This is the real hole, not the scrap rate. | Storage is **per account**, not per character (shared inventory pool and one house per account). Alt-account mules remain possible; accept and monitor, since a mule still eats a stall or guild-hall slot to move items. |
| "Meta rotation makes 25,000 Titan Swords irrelevant" | Only true if the type matrix guarantees every dominant build has a practical counter AND horizontal utility exists (OSRS-style niche use). Total item count still only grows; over years everything trends toward scrap value. | Accepted as a slow drift. One honest non-chore sink stays available: **partial inventory drop on death in contested zones** (Albion). [OPEN: on/off and what fraction; it also delivers "loss must sting".] |
| Coin supply | The conversation focused on items and glossed over coin. Faucets: boss/humanoid coin. Sinks: only the 30% hire burn. Coin supply grows monotonically; prices in gold drift upward. | Keep coin drops tiny and population-scaled. Instrument money supply from day one. Escrow contracts take no fee. If drift becomes a problem the lever is fewer coin drops, never a tax. |
| Bot-assisted dungeon cap "3 per day" | A daily cap is a soft version of the daily-grind mechanic we reject. | Keep the cap only as a bot-farm brake; the real differentiator is the loot ceiling on bot runs. Revisit after load testing. |
| "Cryptographically bound payout" for escrow | Marketing language. It is a server-side state machine on a DB transaction. | Plain wording. Same design. |
| City of Heroes: "judge largely threw out Marvel's claims" | Overstated. Some claims were dismissed in 2005, others survived, then the case settled confidentially. | Precedent still favors generic creation tools, stated accurately. |
| DMCA only | You are in Croatia. US DMCA §512 covers US claims; the **EU Digital Services Act** notice-and-action regime and hosting-provider liability rules are what apply to you at home. "You cannot curate" is also wrong: the DSA has a good-samaritan clause, moderation does not lose the shield. | Both regimes in section 7. Moderation of offensive uploads and likenesses of real people is required anyway. |
| "Open client doesn't help cheaters if the server is authoritative" | **False for aimbots and ESP.** Server authority stops speed/teleport/damage hacks. An aimbot sends legitimate inputs. An open-source client plus an FPS viewport with 200 m headshots is an aimbot magnet. This was the biggest thing the conversation glossed over. | Section 8: projectile-only ranged weapons (no hitscan), PVS culling against ESP, server-side statistical aim analysis, replay review, trust/reputation. Accept that this is an arms race, as it was for CS. |
| Netcode "64 Hz for shooter zones, 64–200 players" | 200 entities × 64 Hz snapshots to every client is too much bandwidth. | Distance-based update rate falloff (close entities full rate, far entities 10 Hz), plus PVS culling. |
| Browser build "comes free" with wgpu | Rendering does. **Networking does not**: browsers have no UDP. | Transport is QUIC via `quinn` natively and WebTransport in the browser, same datagram semantics. One protocol. |

### 1.3 Rejected outright
| Claim | Why |
|---|---|
| Maximum durability decay, repair re-tempering, crafting tolls, weekly territory upkeep, ferry/caravan fees, GE-style sink tax | Chore sinks. Your rebuttal stands. |
| GDKP in-instance auction and physical-relic extraction as the loot model | Superseded by component drops. GDKP survives only as an opt-in party mode for pre-formed groups. |
| Instanced/altar-invoked bosses "immune to sniping" as the primary anti-camp fix | Contradicts open-world contention as a design pillar. Use only for solo/bot tier content. |
| Progressive stall listing tax, limited stall permits | Not needed once rendering is solved. |

### 1.4 Still open after review
Housing as world plots vs instanced interiors (UO had a housing land crisis; recommend instanced interiors
with a world-visible door, world plots for guild halls only). Death-drop fraction in contested zones.
Full type matrix. Currency purchasing-power table. License split. Name.

---

## 2. Engine architecture

### 2.1 Stack [DECIDED]
- **Language:** Rust stable. Bevy is heavier than we want; `bevy_ecs` is the fallback if `hecs` proves too thin.
- **Client:** `wgpu` 30 (Vulkan/Metal/DX12/WebGPU; only the Vulkan backend compiled in until GL is needed), `winit` 0.30, `glam` 0.33, `hecs`. Custom forward renderer.
  Present mode defaults to Mailbox with a CPU frame cap (250 fps) because Fifo stalled to 1 fps on X11 + xfwm4 compositing + RADV (measured 2026-09-30); `--present fifo` remains available.
  Lightmaps baked at level build. A few dynamic lights + shadow maps for characters only.
  Unlit / half-lambert shading, AO baked into textures. No SSR, no volumetrics, no multi-pass PBR.
- **Server:** authoritative Rust on `tokio`. Fixed tick, 64 Hz combat zones, 20 Hz towns/slow zones.
  Client prediction, reconciliation, lag compensation for melee (projectiles need none).
- **Transport:** QUIC (`quinn`) natively, WebTransport (`wtransport`) in browser. Unreliable datagrams for
  snapshots/inputs, reliable streams for chat/inventory/contracts.
- **Persistence:** Postgres via `sqlx` (accounts, characters, items, contracts, escrow), in-memory session state.
- **Level pipeline:** TrenchBroom `.map` → `ericw-tools` (qbsp, vis, light) → BSP with lightmaps and PVS.
  We do not write a lightmapper or a visibility compiler in year one; we load the Quake BSP format.
- **Reference sources:** id Tech 3 (GPL), Quake 1/2 source, Valve networking articles, Veloren.

### 2.2 Massive = sharded zones [DECIDED]
Each zone is a separate server process. Combat zones hold 64–200 players; towns and slow zones more.
Interest management: PVS + distance-based rate falloff. One account/character DB behind all shards.

### 2.3 Wire format
Quantized positions, delta-compressed snapshots against last-acked baseline, bit-packed. Target tens of KB/s.

### 2.4 Movement, collision, physics
Quake-style movement written by hand. BSP hull tracing for world collision, capsule-vs-capsule for players.
No general physics engine. Full player collision and friendly fire.

### 2.5 Entity vocabulary
One simulation, one vocabulary; genres are presentation layers. Every ability compiles to server verbs:
hitscan-melee-arc, projectile, area effect, apply status, move self, block/parry. A fireball and a crossbow
bolt are the same projectile with different parameters. Genre = camera + control scheme + HUD.

### 2.6 Asset budgets, CI-enforced [DECIDED]
| Asset | Budget |
|---|---|
| Character mesh | 2,500–3,500 tris |
| Character texture | one 1024×1024 atlas, hand-painted diffuse, 1 draw call per character |
| Skeleton | ≤ 32 bones |
| Per-character payload | ~1–1.5 MB (KTX2/Basis on disk is smaller; this is the ceiling) |
| Scene | 200 characters ≈ 400k tris, ≈ 200 MB texture VRAM worst case desktop; mobile cache 500 MB |
| Stalls | instanced, dormant, billboard LOD, grid-snapped |

### 2.7 CI gates from commit one [DECIDED]
Client binary size, RAM ceiling, frame time on a reference iGPU (software Vulkan in CI as a smoke test,
real iGPU on a self-hosted runner), bytes/player/second under a 200-bot swarm, per-asset budgets.

### 2.8 Cross-platform
Windows/Linux/macOS via wgpu+winit. Browser via WASM + WebGPU + WebTransport is a priority on-ramp.
Android later (third-person and tactical viewports suit touch; FPS does not). Consoles skipped.

### 2.9 Custom model streaming [DECIDED]
Ingestion: strip unused bones/morphs/metadata, resample to the 1024 atlas, encode KTX2/Basis, emit `.glb`,
reject over-budget. Addressing by SHA-256. Client fixed-cap LRU (2 GB desktop / 500 MB mobile), own +
guild avatars pinned. Tick-0 silhouette mannequin colored by armor weight, custom mesh swaps in async.
Server hitbox from archetype frame, never from mesh.

---

## 3. Character system

### 3.1 Point-buy budget, no classes [DECIDED]
Fixed pool split across base attributes (STR, AGI, CON, INT, SPR), archetype modifiers (mass, armor
class, mobility), and kit (counters, multipliers, skill slots). Reallocatable. Visual is decoupled from function.

### 3.2 RPS rules
Opportunity cost on everything. True bypasses (magic through armor, blunt through magic shields, tracking
through evasion). Kits readable within ~3 s of contact through silhouette, stance, aura. Compressed stat
bands. Type matrix with stacking multipliers, dual-type 4×. The matrix, the attribute set and the
budget are `docs/MATRIX.md` (v1, Phase 3, proposed to the director in its section 12).

### 3.3 Onboarding
Archetype first, math later. 3-minute tutorial proving the loop. Build & Model Browser with community presets.

### 3.4 Gear [DECIDED]
Unbound, un-classed, balanced by weight/cast/stamina penalties. Failed experiments sell to someone with the
opposite problem or decompose.

### 3.5 Content gating by competency
Areas and dungeons unlock by clearing the previous area or passing solo/bot-assisted role trials.

---

## 4. Combat and viewports

### 4.1 Multi-genre viewports on one sim [DECIDED]
| Viewport | Role | Radical strength | Radical weakness |
|---|---|---|---|
| FPS | marksman, infiltrator, precision caster | lethal at range, shoots shield gaps | narrow FOV scoped, blind to flanks, slow reload |
| Third-person | tank, brawler, shield wall | 360° awareness, corner-peek, body-block | no range, stamina drain in iron |
| Tactical | squad leader, engineer | sees the field, places infrastructure | body exposed while in the view |

Imbalance is the point. Every unfair mechanic has a radical counterplay. Terrain decides. Corner-peeking
**stays** as the third-person advantage; the server only hides what is behind solid geometry (PVS).

### 4.2 Ranged weapons are projectiles, not hitscan [CORRECTED, DECIDED]
Muskets, crossbows, bolts, spells all have travel time and drop. Reasons: fits the setting, removes the
need for hitscan lag-compensation rewinds, and blunts aimbots because leading a moving target is the
part cheats do worst against server-simulated projectiles with variance.

### 4.3 Tactical/commander view [DECIDED]
Never commands humans. (1) Leadership build commanding 3–5 hired AI; (2) bounty pins funded from the
guild vault; (3) officer tool at a war table/watchtower with the body exposed.

### 4.4 Friendly fire and collision [DECIDED, scoped]
Always on for AoE and projectiles. "Friendly" affects UI only, never damage. Team-kill stats feed reputation.

---

## 5. Economy

### 5.1 Currency and drops [DECIDED]
Copper/silver/gold, small numbers. Scarce conditional spawns, 100% deterministic drops. No junk.
Sub-minimum items: keep or drop on the ground; dropped items persist and seed others.

### 5.2 Boss loot = modular components [DECIDED, corrected split]
N guaranteed components per kill. Among competing parties: only parties with a living member present at
kill time and above the contribution floor are eligible; proportional split; minimum one each; wiped = 0.
Boss resets/stasis if the aggro-holding party is wiped. No last-hit sniping.

### 5.3 Crafting [DECIDED]
Boss shard → passive trait; refined core → weight/durability/physical bias; catalyst → element;
grip/frame → speed/stamina/crit. Gems are counter-meta modifiers.

### 5.4 Sinks without chores [DECIDED]
- 50% decomposition; salvage feeds other disciplines.
- Storage caps **per account**; non-stacking gear; housing/guild halls as physical footprint-limited storage.
- Monster scarcity scaled to active players (moved/attacked recently) in danger zones, high-variance windows.
- Living meta as the demand shifter.
- Reserve lever: partial inventory drop on death in contested zones [OPEN].
- Coin: tiny drops, money supply instrumented, no taxes. Hire burn 30% is the only coin sink.

### 5.5 Stalls, 48 h [DECIDED]
Grid-snapped, non-overlapping, instanced rendering, no listing tax, town-board search with waypoint.

### 5.6 Tavern hire, 12 h [DECIDED]
Broker burn 30%; hired avatars get flat coin only; diminishing priority after ~3 hires per window;
bot-run loot ceiling (standard gear/mats), top components human-only.

### 5.7 Guild halls [DECIDED]
Free-for-all chest + rank chests. Walk in, see it, take it.

---

## 6. Anti-scam engineering

| Scam | Fix |
|---|---|
| Digit shifting | Small numbers + metric/color formatting; trade mutation lock (both accepts cleared, 3 s cooldown, changed slot red) |
| Icon/skin swap | Borders/badges from stat budget, not cosmetic; hover diff vs equipped |
| Bait-and-switch buy orders | Server escrow, uncancelable mid-transaction |
| Fake stall overlays | Grid snapping, non-overlapping clickboxes, town-board waypoints |
| Carry theft / hostage / buyer bail | Escrow state machine: locked on accept, paid on boss dead + buyer present (alive or not), refunded on wipe/abandon, no kicking the buyer, vote-abandon after 5 min idle, public success ledger, collateral for key runs |
| Ninja looting | Rules immutable in combat; no master-looter for public groups; Need requires build eligibility; 2 h trade window among participants |
| Leeching | ~40% contribution floor |
| Camping | Random lairs + PvP + spawn variance + RPS boss profiles (no single comp clears every boss) |

---

## 7. Legal: user-generated models [DECIDED, corrected]

- Upload under a ToS certification of rights with a takedown warning. Cosmetic decoupled from stats; on a
  valid notice one DB flag reverts the avatar to a generic frame with identical stats.
- **EU (home jurisdiction):** DSA notice-and-action, hosting-provider liability conditions, transparency
  obligations, point of contact. Moderation does not forfeit protection.
- **US claims:** DMCA §512 agent registration (~$6, renew every 3 years), expeditious removal, repeat-infringer policy.
- We never supply, promote, or sell infringing presets.
- Sliders/morphs are safe (Marvel v. NCSoft: partial dismissal, then settlement, precedent favors generic tools).
- Content moderation beyond copyright is mandatory: likenesses of real people, hateful or sexual uploads.

---

## 8. Anti-cheat [CORRECTED]
Open client + server authority stops speed/teleport/damage/inventory cheats. It does **not** stop aimbots,
triggerbots, or ESP. Layers:
1. Projectile-only ranged (4.2).
2. PVS server culling: the client never receives entities behind solid geometry, so wallhacks show nothing.
3. Server-side statistics: reaction time distributions, angular snap analysis, hit-rate outliers per weapon.
4. Server replays for every contested-zone fight; community report + admin review.
5. Account reputation and trust tiers gating high-stakes content; hardware-free, so bans are account-level
   and the deterrent is progression loss.
Accept that this is an arms race. It was one for CS with a closed client too.

---

## 9. AI-assisted build and open source
- Engine, netcode, tooling, bot load tests, deterministic sim tests, ports: AI compresses these. The CI gates
  are what keep the output lean.
- Art consistency, animation, balance, game-feel at 60 ms: human and iterative.
- **License [OPEN, recommendation]:** GPLv3 for client/engine, **AGPLv3 for server** (anyone hosting a fork
  must publish it), CC-BY-SA for official content. Private servers are welcome; nobody can close it.
- Remaining real costs: hosting, accounts/payments/GDPR, community management.

---

## 10. Growth
Replay/death-cam exporter (last 15 s, vertical, stat breakdown). Territory and contested bosses as the
rivalry engine. Seed ToP/RO/Ether Saga private-server Discords, WoW arena and Albion PvP veterans,
VRChat avatar makers. Solo-viable day one; human-only endgame pulls people into guilds.

---

## 11. Implementation plan

### 11.1 Repository layout (Cargo workspace)
```
gamengine/
  Cargo.toml                 workspace
  crates/
    gm-core      shared simulation: entity vocabulary, movement, damage/type matrix, builds, status, fixed-tick step.
                 Runs identically on client (prediction) and server (authority). No I/O, no rendering.
    gm-content   content loader: abilities and preset builds authored in TOML, compiled and validated into
                 gm-core packs. Zones load it; clients receive the compiled pack over the wire.
    gm-bsp       Quake BSP loader: geometry, lightmaps, PVS, hull tracing. Used by client and server.
    gm-net       protocol: bit writer/reader, snapshot delta encoding, input frames, reliable messages.
                 Transport abstraction over quinn (native) and wtransport (wasm).
    gm-client    winit, wgpu renderer, input, prediction/reconciliation, interpolation, audio (kira),
                 dev UI (egui), game HUD, asset cache (LRU), viewport modules (fps, tps, tactical).
    gm-server    tokio zone process: tick loop, interest management (PVS + distance rates), lag comp for
                 melee, projectile sim, AI companions, loot/contract state machines, zone handoff.
    gm-hub       account/auth service, character DB, shard registry, escrow ledger, asset ingestion API.
    gm-hub-proto hub messages, entry tokens and the hub connection (what zones, bots and the client link).
    gm-ai        companion behavior trees (tank/heal/dps/scout) driving gm-core inputs like a player.
    gm-tools     CLI: model ingestion (gltf → glb + KTX2, budget lint), map build wrapper (ericw-tools),
                 budget checker used by CI.
    gm-bot       headless client for load tests and soak tests.
  assets/        maps (.map source + built .bsp/.lit + gamengine.fgd), textures (generated palette + WAD),
                 content (abilities + builds, TOML), later models/audio
  docs/          VOCABULARY.md, PROTOCOL.md, MATRIX.md, HUB.md, BUILDING.md
  PLAN.md        this file stays at the repository root (it is the entry point; README links it)
  budgets.toml   every number CI enforces; read by gm-tools and scripts/
  scripts/       CI gates and the pinned ericw-tools fetch
  ci/baselines/  binary-size baseline for the regression gate
```

### 11.2 Key crates
| Concern | Crate | Note |
|---|---|---|
| Rendering | `wgpu`, `winit`, `glam`, `bytemuck` | forward renderer, lightmap + diffuse, skinned meshes in a storage buffer |
| ECS | `hecs` | fallback `bevy_ecs`. **Not used yet:** the Phase 2 zone holds players in a `BTreeMap` and projectiles in a `Vec` (deterministic iteration, a handful of entity kinds); `hecs` enters when Phase 3 adds statuses, areas and many entity kinds |
| Async/server | `tokio` | one runtime per zone process |
| Transport | `quinn` (native), `wtransport` (browser) | QUIC datagrams + streams; the hub API is the same QUIC with one request per stream (HUB.md 3), no HTTP stack in the client |
| Serialization | hand-rolled bit packing for snapshots; `bitcode` for reliable messages | snapshots must be byte-tight |
| DB | `sqlx` 0.9 + Postgres, embedded sqlx migrations | typed location columns with a check constraint (HUB.md 4); items normalized, components as rows, escrow as transactions (Phase 5) |
| Auth | `argon2`, signed entry tokens (`ed25519-dalek`) | hub issues, zones verify offline; 60 s, single use, zone-bound (HUB.md 3.1). **[CORRECTED]** not JWT: `bitcode` payload + raw signature, no header, no algorithm negotiation |
| Models | `gltf`, `ktx2`, `basis-universal` | ingestion tool |
| Audio | `kira` | |
| Dev UI | `egui` + `egui-wgpu` | not shipped in HUD |
| Testing | `turmoil` (simulated network for tokio), `proptest`, `criterion` | |
| WASM | `wasm-bindgen`, `trunk` | |
| Map compile | `ericw-tools` 2.0.0-alpha11 (external binaries: qbsp, vis, light) | **[CORRECTED]** not vendored: the Linux archive is 18 MB and this repo is measured in MB. `scripts/fetch-ericw-tools.sh` downloads the pinned release with SHA-256 verification into `tools/` (gitignored); `gm-tools map build` wraps it. CI runs the fetch. |

### 11.3 Simulation and netcode design
- **Fixed timestep** in `gm-core` (64 Hz combat, 20 Hz towns). f32, not fixed-point: the server is the
  only authority, so cross-platform bit-determinism is not required, only tolerance-based reconciliation.
- **Client sends** input frames (tick, move bits, look angles quantized, action bits) at tick rate, redundantly
  (last 4 frames per packet, **[CORRECTED]** from 3 after the Phase 2 design review) so loss does not stall movement.
  The server runs every frame exactly once, in order, at most 3 per tick and 64 per second on average (token bucket),
  keeps one frame in reserve as a dejitter buffer (one tick of added latency, took starvation from 14% to about 1%
  of ticks at 75 ± 10 ms one-way latency) and never invents or reuses a frame, so `last_input_tick` in a snapshot
  means exactly "the state after that frame" on both sides (PROTOCOL.md 4, 7.1).
- **Server sends** snapshots: per-entity deltas vs the client's last acked baseline, quantized position
  (1/4 world unit ≈ 0.8 cm; the power-of-two neighbour of the "1 cm" intent), yaw (0.1°), state bits; rate by
  distance band. Entities outside the client's PVS are not sent.
- **Prediction:** client runs `gm-core` locally for its own entity, keeps a ring of predicted states, replays
  from the server-acked tick on mismatch beyond tolerance (2 u / 16 u/s). It predicts on the dequantized wire
  input, exactly what the server runs. Ability clocks (cooldowns, scripts, dashes) run in the client's frame
  ticks on both sides so they elapse identically when the server runs two frames in one tick. An adopted server
  position is nudged out of solid first (QuakeWorld's `PM_NudgePosition`): the 1/4-unit rounding can land
  exactly on a clip plane and freeze the mover (PROTOCOL.md 7.2).
- **Interpolation:** other entities rendered ~100 ms behind with two buffered snapshots.
- **Melee lag comp:** server rewinds capsule positions of nearby entities to the attacker's view tick
  (bounded to ~200 ms) when resolving swing arcs and parries.
- **Projectiles:** spawned server-side at the attacker's view tick, simulated forward; no rewind. Client shows a
  predicted local projectile and reconciles.
- **Collision:** BSP hull tracing for world; **[CORRECTED]** player-player body blocking uses axis-aligned box
  sweeps with the same 32×32×56 hull inside the movement trace (sliding and stepping against a player work
  exactly as against a wall, and the Minkowski sweep is exact), while hitboxes for damage are the archetype
  capsules. Resolved on the server; the client predicts its own against the newest known positions of others.
  World collision uses the Quake player hull for every archetype in year one, because the BSP carries exactly
  two hull sizes; hitbox capsules differ per frame (VOCABULARY.md 3). Players within 128 u of a client are always
  sent even outside its PVS, because a body that can block you must be predictable (PROTOCOL.md 5).
- **Zone handoff:** hub-directed; character state serialized, token issued for the target zone, client
  reconnects; the origin zone keeps a ghost until the ack.

### 11.4 Data model (hub)
- `accounts` (id, email, argon2 hash, created, trust_tier, upload_privileges)
- `characters` (account_id, name, budget allocation JSON, viewport prefs, zone, position)
- `items` (id, owner_ref: character | container | stall | escrow, template_id, component rows)
- `item_components` (item_id, layer: shard | core | catalyst | frame | gem, material_id)
- `containers` (account storage pool, house slots, guild chests with rank)
- `stalls` (owner, tile_id, expiry, listings with escrowed buy-orders)
- `hires` (avatar, window expiry, hire count)
- `contracts` (buyer, party, instance, price, state: open | active | paid | refunded)
- `models` (sha256, owner, status: active | takedown, generic_fallback_frame)
- `ledger` (every coin movement, for money-supply monitoring)
All item and coin movements are DB transactions; escrow states are enforced by constraints, not by code paths.

### 11.5 Asset pipeline
1. Maps: author in TrenchBroom with our entity definitions (.fgd) → `gm-tools map build` runs qbsp/vis/light →
   `.bsp` committed to `assets/maps/built`. Lightmap resolution and light entities are the whole lighting model.
2. Base characters: modular skeleton frames (colossus, striker, caster, infiltrator) with a shared 32-bone rig
   and a shared animation set; armor weight classes as texture/mesh overlays on the same rig.
3. Player uploads: `gm-hub` accepts glTF/glb; `gm-tools ingest` validates tris/bones/atlas, re-encodes, stores
   by hash in object storage; CDN in front.
4. Budget lint runs in CI on every asset in the repo.

### 11.6 Development process
- Docs first: `VOCABULARY.md` (entity verbs and parameters), `PROTOCOL.md` (packet layouts), `MATRIX.md`
  (type table). AI implements against the spec; the spec is the contract, not the prompt.
- Every crate has: unit tests, a `criterion` benchmark for its hot path, and a budget assertion.
- Netcode is developed against `turmoil` with injected latency (50–200 ms) and loss (0–5%) from day one.
- `gm-bot` swarm of 200 in CI asserts server CPU and bytes/player/second.
- Weekly playtest build; feel tuning is human, never merged on metrics alone.

### 11.7 Deployment
- Zone servers: one static binary each, `systemd` units on Hetzner-class VPS (you already run user systemd
  deploys). One 4-core box should run several 64-player zones; measure.
- Hub: one instance + Postgres + object storage (S3-compatible) + CDN.
- Browser client served as static files.
- Observability: `tracing` + Prometheus exporter per zone (tick time, entity counts, bytes, money supply).

### 11.8 Milestones with acceptance criteria
| Phase | Deliverable | Done when |
|---|---|---|
| 0 | Workspace, CI gates, `VOCABULARY.md`, `PROTOCOL.md` skeleton | CI fails on a 1 MB binary-size regression and on an over-budget test asset. **Done 2026-09-30** |
| 1 | `gm-bsp` + `gm-client`: load a TrenchBroom map compiled by ericw-tools, lightmapped, Quake movement, collision | 60+ fps on an Intel iGPU; binary < 10 MB; RAM < 200 MB. **Done 2026-09-30** |
| 2 | `gm-net` + `gm-server`: 16 players, prediction, reconciliation, interpolation, melee lag comp, projectiles, collision, FF | playable at 150 ms / 3% loss in `turmoil`; bytes/player < 30 KB/s. **Done 2026-09-30** (11.10) |
| 3 | Point-buy characters, type matrix, FPS + third-person viewports, PVS culling | one arena map, 8v8 playtest, a build can be countered by re-speccing. **Done 2026-10-01** (11.10) |
| 4 | Hub: accounts, persistence, zones, handoff, 200-bot swarm | 200 bots on one zone under CPU budget; login → zone → handoff → logout round trip. **Done 2026-10-01** (11.10) |
| 5 | Economy: stalls, escrow contracts, component drops with corrected split, crafting, decomposition, account storage caps, guild halls, tavern hires, ledger | every coin/item movement is a DB transaction; scam test suite passes (mutation lock, escrow, floors) |
| 6 | Custom models: ingestion, hash cache, LRU, silhouette fallback, takedown flag, moderation queue | 100 unique uploaded avatars in a town at 60 fps on iGPU, no disk growth past cap |
| 7 | Tactical viewport + AI companions + role trials | solo player clears a tutorial dungeon with 3 hired avatars |
| 8 | WASM/WebGPU/WebTransport build | browser client joins the same zone as native clients |
| 9 | Anti-cheat statistics, replays, reputation | replay of any contested fight reviewable; aim-outlier report per account |
| ∞ | Content, balance, ops, community | permanent |

### 11.9 First concrete step
Phase 0 and 1: scaffold the workspace, CI gates, vendored ericw-tools, a BSP loader, a wgpu forward
renderer with lightmaps, Quake movement with hull tracing, and the empty tokio tick loop. Then the
vocabulary doc, because everything in phases 2–7 compiles down to it.

### 11.10 Status log
**2026-09-30, Phases 0 and 1 done** (measured on the development machine: AMD Ryzen 7 4800U with Radeon
Vega integrated graphics, RADV on Mesa 26.1.4, Arch Linux, X11; not an Intel iGPU, but an iGPU of the same
class):
- Workspace with all nine crates, Rust stable 1.97 (MSRV 1.88, edition 2024), licenses per section 9,
  `budgets.toml`, CI workflow, gate scripts with self-tests. `scripts/check-budgets.sh --self-test` rejects a
  5,000-tri / 40-bone / 2-texture / 2048² model and accepts an in-budget one; `scripts/check-binary-size.sh
  --self-test` fails a 1 MiB + 1 byte regression and a 10 MiB + 1 byte binary.
- Test map (room, platform, ramp, pillars, low wall, five coloured lights) compiles with qbsp/vis/light in
  0.01 s: 51.6 KB BSP + 14.7 KB `.lit`, 73 faces all lightmapped, 14 leaves.
- `gm-bsp`: BSP29 + BSP2, PVS, lightmaps + `.lit`, Quake hull tracer; 11 tests including full player
  movement through the room (walls, 16-unit step vs 40-unit wall, ramp onto the platform).
- `gm-client` release binary **5,862,792 bytes (5.59 MiB)**, peak RSS **127 MB** windowed / 118 MB headless
  / 171 MB under lavapipe. Windowed 1280×720, vsync off: **2,400–5,900 fps** (0.17–0.42 ms per frame; the
  spread follows iGPU clocks, lower with the monitor in standby). Under software Vulkan (lavapipe, what CI
  runs) 307 fps. The room has 168 triangles, so these numbers prove the pipeline, not the renderer's ceiling.
- `gm-server`: 64 Hz tick loop, 3 µs of work per tick, zero overruns, wake-up lateness 1.4 ms mean /
  2.4 ms max (tokio's 1 ms timer; a spin-wait for the last millisecond is a later optimisation).
- Not done from 11.6: `criterion` benchmarks per crate and `turmoil` tests (Phase 2 with gm-net).

**2026-09-30, Phase 2 done** (same machine; all numbers measured, none estimated):
- `docs/PROTOCOL.md` v1 (bit packing with test vectors, 4-frame input datagrams, carry-forward delta
  snapshots, frame ledger with a one-frame dejitter reserve and a 64/s token bucket, latency-bounded lag
  compensation, fixed QUIC congestion window) after an independent design review (its log is section 10 of
  the document: 14 points accepted, 3 rejected with reasons). `gm-core::sim` runs movers, ability scripts,
  melee with rewind, projectiles with forward step, deaths and respawns; `gm-net` implements the wire format
  and the shared client prediction/interpolation; `gm-server` adds sessions, PVS and distance-band
  snapshots, quinn connection handling; `gm-bot` is the headless client; `gm-client` joins a zone.
- **Acceptance test** (`scripts/check-netcode.sh`, turmoil, simulated time, 16 bots, 30 s, 75 ± 10 ms per
  hop = 150 ms round trip, 3% independent datagram loss per direction, run four times): every bot received
  61–62 snapshots/s (max gap 3 ticks, most 2); per player **5.4–5.8 KB/s up and 8.2–8.7 KB/s down** as UDP
  bytes counted by QUIC (budget 30 KB/s each way); reconciliation corrections 22–91 per bot in 30 s, all
  under 6.5 u, of which **0–2 per bot had no visible cause** (the rest follow a hit, a respawn or a body within
  blocking distance); input starvation 1.3% of player-ticks including joins and the reserve fill; 419–435
  melee hits and 6–12 crossbow hits registered under latency; 106–110 kills; no oversize snapshots, no send
  failures, no decode errors. The same test at 10 ms round trip and no loss: zero gaps, zero unexplained
  corrections. Real UDP on loopback (`tests/loopback.rs` and a 4-player live run): 0.2 ms RTT, 4.4 KB/s
  down / 5.5 KB/s up per player, server tick 43–55 µs mean with 4 players, 2 starved ticks per bot in 10 s.
- Three defects found by the test's correction log and fixed before the numbers above: ability clocks keyed
  to server ticks drifted when two frames ran in one tick (now frame ticks); the client predicted on the raw
  float yaw instead of the dequantized wire value; a rounded server position landing exactly on a clip
  plane froze the mover (now nudged out first, as QuakeWorld's `PM_NudgePosition`). Corrections went from
  100–250 per bot with 30-u jumps to the figures above.
- Independent code review (Gemini 3.1 Pro, PROTOCOL.md 10): fixed four real defects it found (reordered
  datagrams lost frames; `view_tick` taken from the wrong datagram; the projectile catch-up swept current
  instead of historical positions, an exploit for anyone delaying their acknowledgements; unbounded per-client
  control channel) plus three cheap scaling items; rejected one finding that misread the respawn replay.
- Binaries (release, LTO): `gm-client` **8,051,416 bytes (7.68 MiB)**, up 2.09 MiB for quinn + rustls/ring +
  tokio; baseline updated on purpose (cap 10 MiB). `gm-server` 3.34 MB, `gm-bot` 3.25 MB, `gm-tools` 1.91 MB.
  Peak RSS **128 MB** windowed (RADV), 172 MB under lavapipe. Windowed bench 1280×720, no vsync:
  **1,447 fps** (0.69 ms mean, 3.96 ms p99) drawing the room plus the entity pass; default play mode 218 fps
  under the 250 cap including start-up.
- Criterion (this CPU): snapshot delta encode 0.80 µs and decode 1.20 µs for 16 entities; input datagram
  encode 103 ns, decode 114 ns; one movement tick 84 ns on the box world; one full zone tick with 16
  running, swinging players 11.1 µs; BSP player-hull trace across the room 128 ns, point LOS trace 142 ns,
  leaf lookup 51 ns, PVS row decompression 17 ns.
- Known limits: other players are boxes; the tick loop is single-threaded (fine to hundreds of players by the
  numbers above); prediction against other players uses their newest known positions, so body-block
  corrections are frequent in a crowd (visible as small snaps, never desync); `hecs` still unused (11.2);
  `wtransport` untouched until Phase 8; `SO_RCVBUF` tuning and time-scaled interpolation drain deferred.

**2026-10-01, Phase 3 done** (same machine; all numbers measured, none estimated):
- `docs/MATRIX.md` v1: five attributes 5..=20, four free frames, four armour classes with a physical
  kind × class table, five elements in a pentagram (each beats two, loses to two, resists itself; dual
  aspects two apart have one 4× hole and two 0.25× walls, adjacent ones no hole and one wall), derived
  stats in bands of at most 1.5× per stat and 2× effective health, a multiplicative damage pipeline with
  the three structural bypasses, fourteen statuses with stacking and immunity rules, an exact 100-point
  budget. Independent design review before coding (section 12 of the document: 7 accepted, 1 corrected
  differently, 1 rejected): it found that "every dual aspect has a 4× hole" is impossible in a balanced
  five-element matrix, which is now stated honestly.
- `gm-core`: `matrix` (tables, derived stats, pipeline), `build` (budget validation, kits, sheets,
  content packs), `status` (eight slots, Chill→Freeze, immunities), `sim` split into mover/zone with
  every verb resolved: areas (shapes, falloff, LOS, `exclude_actor`), block/parry with facing, guard break,
  stagger build-up with a 1 s immunity, damage and healing over time at 4 pulses/s, blink and charge,
  triggers on projectile hits, respec at respawn, teams and team spawns. The vocabulary became
  non-recursive (typed trigger lists) so it is `bitcode`-encodable. `gm-content` compiles TOML content
  (26 abilities, 4 presets) and a test proves the in-code fixture mirrors it.
- Protocol v2: own block (stamina, focus, statuses), status aura mask, script/parry flags, areas as
  entities, build/team spawn info, content sent after `Welcome`, `Respec`/`BuildApplied`. The client adopts
  resources, statuses and guard state whenever they differ, not only on a position mismatch (found by the
  code review). Turmoil, 16 bots, 150 ms, 3% loss: test_room **5.6 KB/s up / 9.2 KB/s down** (v1: 5.6 /
  8.5); arena with full kits **5.3–5.6 / 10.0–10.9 KB/s**; real UDP arena with 16 duelists 5.5 / 9.4.
- Client: third-person viewport (camera 110 u back, 24 right, 12 up, point-traced out of walls) with
  camera-to-muzzle re-aim, `V` toggles; abilities on 1–4, guard on Ctrl, F1–F4 respec; team colours,
  facing markers, auras, areas. Arena map (`gm-tools map gen-arena`, 688 faces, 62 leaves, 16 team
  spawns). Windowed bench 1280×720 no vsync: **arena 2,312 fps (0.43 ms mean, 4.6 ms p99)**, test_room
  5,663 fps; live 8v8 match in third person **243 fps (250 cap), peak RSS 124.6 MB**, 0 unexplained
  corrections. Software Vulkan (CI path) arena 216 fps, 181 MB.
- Bots: a duelist brain that uses the whole kit (closes or kites by frame, blocks and parries wind-ups,
  fires actives by their shape) and counter-picks from the matrix (the preset whose elements score best
  against the enemy's aspects, requested through `Respec`).
- **Acceptance** (`scripts/check-matrix.sh`, offline 8v8 in the arena, identical brains, three seeds ×
  60 s): ironclad : blade **85–98 : 30–34** (2.8–2.9), the blades re-specced to frostweavers :
  ironclad **156 : 5** (31, a hard counter: Frost 2× through plate, Stone 0.25× into Frost + Shadow,
  haste out-kites plate), blade : frostweaver **100 : 58** (1.7, the 4× Flame hole closes the cycle),
  mirror **70–74 : 69–75**. Over the real protocol (`tests/counterpick.rs`, turmoil, 150 ms, 3% loss,
  team 2 counter-picks after 10 s): team 2's kill share goes from **0.20–0.36 to 0.72–0.88**.
- Independent code review (Gemini 3.1 Pro, MATRIX.md 12): six defects fixed (own-state adoption,
  projectile block facing, swings surviving a stagger, DoT rounding, parry cooldown groups, Extend
  shrinking), one rejected, one deferred to Phase 4 (the O(N²) body list).
- Binaries (release, LTO): `gm-client` **8,264,392 bytes (7.88 MiB)**, +213 KB for the matrix, statuses,
  areas, content types and the viewport; baseline updated. `gm-server` 4.15 MB, `gm-bot` 3.47 MB.
  Server tick with 16 duelists on the arena **460 µs mean, 0.8 ms max** (Phase 2: 11 µs for 16 runners
  on a box world; the arena PVS checks, areas, statuses and per-session snapshots add the rest; the
  200-player budget is Phase 4's problem and the body list is already known to be O(N²)).
- Known limits: unexplained corrections are 0–3 per bot per minute with full kits at 150 ms (velocity-only
  wall-contact corrections and positional abilities against stale bodies are classified, not hidden);
  areas draw as flat discs; bots never use cover; `Origin::Target` resolves to the caster; the hub
  still does not exist, so builds live only for the session.

**2026-10-01, Phase 4 done** (same machine; all numbers measured, none estimated):
- `docs/HUB.md` v1: one hub process, QUIC with one request per stream (1,024 streams per hub
  connection), `bitcode` messages, Postgres via `sqlx` with embedded migrations and typed location
  columns under a check constraint, argon2id on the blocking pool behind a semaphore, ed25519 entry
  tokens (60 s, single use, zone-bound, 5 s skew, persisted key), claims and saves scoped to the
  character's zone, handoff with a 10 s ghost, transits abandoned after 15 s, logout kicks. Design
  review before coding (section 9 of the document: 12 accepted, 1 accepted with a different reading).
- `gm-hub`: protocol, token, db, hub, client modules, 1 migration, the binary; `gm-server`: hub link
  (register, heartbeat, claim, save every 30 s staggered, handoff, notices), tokens mandatory under
  `--hub`, ghosts in `gm-core` (`Player.ghost`); `gm-bot --hub` and `gm-client --hub` log in, enter,
  travel (the client reloads the destination's map) and log out.
- Server scaling for the swarm: the tick is instrumented (events, sim, snapshot, send); snapshots are
  built from one shared per-tick table (wire records and leaves computed once) with merged walks
  against the baseline, in parallel over sessions on a `rayon` pool. 200 duelists in the arena hall
  (nothing culled): **14.5 ms → 6.6 ms mean, 9.1 ms p99** per tick (snapshots 12 → 2.8 ms, simulation
  3.0 ms), 272 MB RSS, 20 KB/s down and 5.6 KB/s up per player, with the 200 bots on the same machine.
  Gate `scripts/check-swarm.sh` with `budgets.toml [server]` (8 ms mean, 12 ms p99, 512 MiB).
- **Acceptance** (`crates/gm-server/tests/handoff.rs`, loopback, one hub on a test database, two zones):
  register → enter zone A → travel to zone B → logout with the location checked in the database at
  every step; the hub's share **218 ms** (login 24, enter 110, travel 82, logout 2; budget 2 s), claimed
  in A 55 ms after start and in B 160 ms after the travel request. Wrong password refused, a second
  `Enter` during a transit refused, name and email normalisation checked. Live smoke: the windowed
  client through the hub (243 fps, 122.6 MB) while a bot travelled from an arena zone to a test-room
  zone with a different map, 0 corrections.
- Binaries (release, LTO): `gm-client` **8,415,016 bytes (8.03 MiB)**, +151 KB for the hub
  connection, tokens and the map switch; baseline updated. Linking the whole hub crate into the
  client first cost +682 KB (sqlx and argon2 came along), so the protocol, tokens and connection
  live in `gm-hub-proto` and the hub resolves preset names itself. `gm-hub` 5.34 MB, `gm-server`
  4.72 MB, `gm-bot` 3.66 MB.
- Independent code review (Gemini 3.1 Pro, HUB.md 9): six defects fixed, none rejected (a save
  racing a handoff on another stream, logout losing the last save, the character cap race, the
  rate limiter's sweep under the global lock, simultaneous saves, unread heartbeats).
- Known limits: a zone that loses its hub connection orphans its characters (conservative); the
  simulation step is still O(N²) in bodies (3 ms at 200; a grid is the next lever); no ops tool yet
  (`gm-hub ctl`); sessions and the zone registry are in memory (a hub restart logs everyone out).

## 12. Open decisions
License split (recommend GPLv3 client / AGPLv3 server / CC-BY-SA content). The type matrix and attribute
set are **proposed** in `docs/MATRIX.md` 12 (implemented and measured; the director confirms or changes
the numbers, the structure is what the code depends on). Currency purchasing-power table. Death-drop in
contested zones: on/off and fraction. Housing: instanced interiors vs world plots. Salvaged-component
recipes. Storage slot counts. Name.
