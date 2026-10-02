# GAMENGINE — Design & Engineering Plan

Working title: **gamengine** (rename later). Persistent action-sandbox MMORPG with multi-genre viewports,
built on a lightweight Rust engine. Last updated 2026-10-02 (Phases 0–8 implemented; see 11.10).

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
- **Transport:** QUIC (`quinn`) natively, WebTransport in the browser. Unreliable datagrams for
  snapshots/inputs, reliable streams for chat/inventory/contracts. **[CORRECTED]** in Phase 8: `wtransport`
  is the *server's* WebTransport listener (beside the quinn endpoint, on a second port); in the browser
  the transport is the browser's own `WebTransport`, so the `.wasm` carries no QUIC stack (`docs/WEB.md` 2).
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
| Per-character payload | ~1–1.5 MB ceiling. Measured in Phase 6 for an avatar at the triangle and atlas ceiling: 382 KB on disk and on the wire (`.gmm`, BC1 + zlib), 780 KB on the GPU |
| Scene | 200 characters ≈ 400k tris, ≈ 200 MB texture VRAM worst case desktop; mobile cache 500 MB |
| Stalls | instanced, dormant, billboard LOD, grid-snapped |

### 2.7 CI gates from commit one [DECIDED]
Client binary size, RAM ceiling, frame time on a reference iGPU (software Vulkan in CI as a smoke test,
real iGPU on a self-hosted runner), bytes/player/second under a 200-bot swarm, per-asset budgets.

### 2.8 Cross-platform
Windows/Linux/macOS via wgpu+winit. Browser via WASM + WebGPU + WebTransport is a priority on-ramp.
*(Phase 8 built it: the same `gm-client` compiled to wasm, in two builds the page's loader chooses
between, because the cost is not symmetric: 0.9 MB of `.wasm` for WebGPU, where wgpu is a thin binding,
2.9 MB for WebGL2, where it links its GLES backend and the shader translator; 0.30 and 0.87 MB
compressed. `docs/WEB.md`.)*
Android later (third-person and tactical viewports suit touch; FPS does not). Consoles skipped.

### 2.9 Custom model streaming [DECIDED; container CORRECTED in Phase 6]
Ingestion: strip unused bones/morphs/metadata, resample to the 1024 atlas, re-encode, reject over-budget.
Addressing by SHA-256. Client fixed-cap LRU (2 GB desktop / 500 MB mobile), own + guild avatars pinned.
Tick-0 silhouette mannequin colored by armor weight, custom mesh swaps in async. Server hitbox from
archetype frame, never from mesh.

**[CORRECTED]** The first version said "encode KTX2/Basis, emit `.glb`". glTF stays the *upload* format;
what the hub stores and clients fetch is `.gmm`, our own container (quantised vertices, BC1 mip chain,
zlib; `docs/MODELS.md` 5). Measured with the workspace's release profile before deciding: parsing glTF in
the client costs +370 KB of binary and the Basis transcoder +990 KB (plus a C++ toolchain for every
target), against +30 KB for the container with inflate and SHA-256; the client had 1.94 MiB left under
its 10 MiB cap. BC1 is what 2.6 already assumes for VRAM, every desktop GPU samples it natively, and the
client decodes it to RGBA where it is not supported. Mobile (ETC2/ASTC) is a second ingestion output in
Phase 8: the hub keeps every source upload, so nothing is uploaded twice. The same section's "colored by
armor weight" has a consequence the first version missed: a custom model hides the mannequin's tint, so
armor class is also carried by the gait of the shared animation set and aspects by a ring the client
draws at the feet (MODELS.md 9).

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
*(Phase 7 built (1): any character commands 3 companions, a leadership ability in the build makes it
5; orders are given from the command stance, in which the body does nothing else. (2) and (3) are
not built. `docs/COMPANIONS.md` 3 and 5.)*

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
*(Phase 7, to confirm: taken literally this lets a second party reset the first one's attempt by
dying on purpose. As built, a party that is out for 5 s has exactly its own damage healed back and
leaves the ledger; the boss resets when the last party is out. `docs/COMPANIONS.md` 9 and 16.)*

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
                 `link`: one connection type over quinn and a wtransport (WebTransport) session, for
                 zones and the hub. On wasm only the codecs and the prediction are built.
    gm-client    winit, wgpu renderer, input, prediction/reconciliation, interpolation, audio (kira),
                 dev UI (egui), game HUD, asset cache (LRU), viewport modules (fps, tps, tactical).
                 The same crate is the browser client (`src/web/`: the browser's WebTransport, its
                 Cache API as the model store, fetch for maps).
    gm-model     avatar models as the client needs them: the standard rig, the `.gmm` container and its
                 strict reader, the shared animation set, the mannequin. Small on purpose (the client
                 links it); nothing here parses an upload.
    gm-ingest    upload ingestion: glTF validated against the budgets and the frame envelope, re-encoded
                 to `.gmm`. Linked by the hub (run in a sandboxed worker process) and by gm-tools.
    gm-server    tokio zone process: tick loop, interest management (PVS + distance rates), lag comp for
                 melee, projectile sim, AI companions, loot/contract state machines, zone handoff.
    gm-hub       account/auth service, character DB, shard registry, escrow ledger, asset ingestion API.
    gm-hub-proto hub messages, entry tokens and the hub connection (what zones, bots and the client link).
    gm-replay    replays: the `.gmr` file a zone records (every entity every tick, and the tick's events),
                 its reader and playback, and the aim analysis computed from its frames: the same code
                 runs live in the zone and offline over a file.
    gm-ai        minds: companions (heal/tank/scout/dps, read from the build) and creatures driving gm-core
                 inputs like a player, the nav grid, and the director a zone runs them with: squads, orders,
                 encounters with their ledger, the loot split of a kill, trial verdicts.
    gm-tools     CLI: model tools (ingest locally, template, synthetic avatars, upload, wear), moderation
                 tools, map build wrapper (ericw-tools) and map generators, budget checker used by CI.
    gm-bot       headless client for load tests and soak tests.
  assets/        maps (.map source + built .bsp/.lit + gamengine.fgd), textures (generated palette + WAD),
                 content (abilities, builds, creatures, trials, items; TOML), later models/audio
  web/           the browser client's page and loader (index.html, boot.js); scripts/build-web.sh
                 assembles target/web/ from it
  docs/          VOCABULARY.md, PROTOCOL.md, MATRIX.md, HUB.md, ECONOMY.md, MODELS.md, COMPANIONS.md,
                 WEB.md, ANTICHEAT.md, BUILDING.md
  PLAN.md        this file stays at the repository root (it is the entry point; README links it)
  budgets.toml   every number CI enforces; read by gm-tools and scripts/
  scripts/       CI gates, the pinned fetches (ericw-tools; wasm-bindgen and wasm-opt), the web build
  ci/baselines/  binary-size baseline for the regression gate
```

### 11.2 Key crates
| Concern | Crate | Note |
|---|---|---|
| Rendering | `wgpu`, `winit`, `glam`, `bytemuck` | forward renderer, lightmap + diffuse; skinned characters, one draw call each, their 24 matrices in a block of a shared uniform buffer selected by dynamic offset (**[CORRECTED]** from "a storage buffer": uniform buffers with dynamic offsets exist on every backend including WebGL2-class ones, and a block is 1.6 KB) |
| ECS | `hecs` | fallback `bevy_ecs`. **Not used yet:** the Phase 2 zone holds players in a `BTreeMap` and projectiles in a `Vec` (deterministic iteration, a handful of entity kinds); `hecs` enters when Phase 3 adds statuses, areas and many entity kinds |
| Async/server | `tokio` | one runtime per zone process |
| Transport | `quinn` (native), `wtransport` (the servers' WebTransport listener) | QUIC datagrams + streams; the hub API is the same QUIC with one request per stream (HUB.md 3), no HTTP stack in the client. **[CORRECTED]** the browser side is the browser's `WebTransport`, bound by hand in `gm-client/src/web/wt.rs` (fifteen declarations; web-sys keeps it behind an unstable-API flag) |
| Serialization | hand-rolled bit packing for snapshots; `bitcode` for reliable messages | snapshots must be byte-tight |
| DB | `sqlx` 0.9 + Postgres, embedded sqlx migrations | typed location columns with a check constraint (HUB.md 4); items normalized, components as rows, escrow as transactions (Phase 5) |
| Auth | `argon2`, signed entry tokens (`ed25519-dalek`) | hub issues, zones verify offline; 60 s, single use, zone-bound (HUB.md 3.1). **[CORRECTED]** not JWT: `bitcode` payload + raw signature, no header, no algorithm negotiation |
| Models | `gltf`, `image`, `texpresso` (BC1), `miniz_oxide`, `sha2` | **[CORRECTED]** (2.9): `gltf`, `image` and `texpresso` only in `gm-ingest` (hub worker and tools); the client links `miniz_oxide` and `sha2`. No `ktx2`, no `basis-universal`; `lru` dropped (the caches order by file time and by last frame drawn) |
| Audio | `kira` | |
| Dev UI | `egui` + `egui-wgpu` | not shipped in HUD |
| Testing | `turmoil` (simulated network for tokio), `proptest`, `criterion` | |
| WASM | `wasm-bindgen`, `wasm-bindgen-futures`, `web-sys`, `web-time`; `wasm-opt` | **[CORRECTED]** no `trunk`: `scripts/build-web.sh` runs cargo, the pinned `wasm-bindgen` CLI and `wasm-opt` (fetched like ericw-tools) and copies one page and one loader script |
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
- `models` (sha256 of the ingested bytes, frame, status: pending | active | rejected | takedown, sha256 of the
  upload, uploader, decision and reason), `model_holders` (who may wear it, with the terms version they
  certified), `model_events` (the audit trail), `characters.model` (MODELS.md 6.1). **[CORRECTED]** from
  "(sha256, owner, status: active | takedown, generic_fallback_frame)": a model has holders, not one owner
  (two players may upload the same file), a moderation queue needs `pending` and `rejected`, and the
  fallback frame is the model's own frame
- `ledger` (every coin movement, for money-supply monitoring)
All item and coin movements are DB transactions; escrow states are enforced by constraints, not by code paths.

### 11.5 Asset pipeline
1. Maps: author in TrenchBroom with our entity definitions (.fgd) → `gm-tools map build` runs qbsp/vis/light →
   `.bsp` committed to `assets/maps/built`. Lightmap resolution and light entities are the whole lighting model.
2. Base characters: modular skeleton frames (colossus, striker, caster, infiltrator) with a shared 32-bone rig
   and a shared animation set; armor weight classes as texture/mesh overlays on the same rig.
3. Player uploads: `gm-hub` accepts a `.glb`; `gm-ingest` (in a worker process of the hub; the same code as
   `gm-tools model ingest`) validates budgets, rig, pose and the frame envelope and re-encodes to `.gmm`; the
   hub verifies the worker's output from its bytes and stores it by hash (a directory today, object storage
   and a CDN at deployment).
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
| 5 | Economy: stalls, escrow contracts, component drops with corrected split, crafting, decomposition, account storage caps, guild halls, tavern hires, ledger | every coin/item movement is a DB transaction; scam test suite passes (mutation lock, escrow, floors). **Done 2026-10-01** (11.10) |
| 6 | Custom models: ingestion, hash cache, LRU, silhouette fallback, takedown flag, moderation queue | 100 unique uploaded avatars in a town at 60 fps on iGPU, no disk growth past cap. **Done 2026-10-01** (11.10) |
| 7 | Tactical viewport + AI companions + role trials | solo player clears a tutorial dungeon with 3 hired avatars. **Done 2026-10-01** (11.10) |
| 8 | WASM/WebGPU/WebTransport build | browser client joins the same zone as native clients. **Done 2026-10-02** (11.10) |
| 9 | Anti-cheat statistics, replays, reputation | replay of any contested fight reviewable; aim-outlier report per account. **Done 2026-10-02** (11.10) |
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

**2026-10-01, Phase 5 done** (same machine; all numbers measured, none estimated):
- `docs/ECONOMY.md` v1 is the contract: holders (character 24 slots, account storage 60, stall 12,
  escrow, guild chest 48, ground), items with components as rows, integer copper, `coin_ledger` and
  `item_moves` written in the same transaction as every movement. Coin is created only from the
  `source` and destroyed only into the `sink`; `audit` checks created = circulating + burned and
  every balance against the ledger.
- `gm-hub::economy` (one transaction per operation, one lock order: holders ascending, then trade
  rows, then items): trade window with the mutation lock, the 3 s cooldown and offer versions;
  48 h grid-snapped stalls with listings and escrowed buy orders, no fee and no tax; carry
  contracts (`open → active → paid | refunded`, no cancel mid-run, idempotent zone reports, 120 min
  refund); component and coin drops; crafting and ⌊k/2⌋ decomposition by an unsteerable hash;
  per-account storage; guild chests gated by rank on withdrawal; tavern hires with the 30% burn.
  `gm_core::loot` is the corrected boss split (10% party floor over the surviving parties, wiped
  parties get nothing, minimum one, 40% leech floor inside a party). `assets/content/items.toml`
  holds templates and materials; the loader refuses a template whose best craft exceeds the 25%
  edge cap (the shipped best sword is exactly 250 per mille).
- Over the wire: `HubRequest::Econ` (a session, for its own character) and `HubRequest::ZoneEcon`
  (drops, ground, contract outcomes; a zone speaks only for itself), HUB.md 3. The zone server and
  the client do not call them yet: there is no boss, no inventory screen and no stall in the world.
- **Acceptance** (`crates/gm-hub/tests/economy.rs`, `economy_protocol.rs`, real Postgres): the scam
  suite is 15 tests in 1.6 s: the swap scam and the blind accept refused, a commit that verifies
  again, eight buyers racing for one listing (one wins, 300 copper paid once), the price-swap
  refused, tiles that cannot overlap, a full owner who cannot keep a tile, escrow that pays only on
  completion and refunds on wipe, abandon and stalling, the split with a wiped party, a leech and a
  tagger, crafting that creates nothing, storage capped under concurrent deposits, the 30% burn.
  The storm test runs 792 mixed operations between six characters in 1.4 to 1.5 s (520 to 560 per
  second, debug build) with no deadlock victim, five runs of five. The wire test runs a drop, a
  craft, a trade with the real cooldown, a contract and a stall sale through a live hub in 3.4 s
  and ends with 900 copper created, 0 burned, 900 circulating.
- Reviews (ECONOMY.md 12): design review 8 accepted, 3 already covered, 3 rejected (collateral
  forfeited on a wipe, a deposit rank on guild chests, stacking now). Code review: four findings,
  all fixed, three of them lock-order deadlocks also found by the author's own pass, which added
  four more (the global `source`/`sink` row lock, client-side pickup, deadlock victims reported as
  internal errors, an unlocked location read in `hire`).
- Binaries (release, LTO): `gm-client` **8,455,592 bytes (8.06 MiB)**, +40,576 for the economy
  messages it does not use yet; baseline updated. `gm-hub` 5.69 MB, `gm-server` 4.77 MB, `gm-bot`
  3.70 MB. Swarm gate after the Phase 4 review fixes: 6.9 ms mean, 8.8 ms p99, 273 MB, 18 KB/s
  per player (one run taken straight after the netcode gate, with the machine still loaded, read
  10.1 ms and failed; the gate needs a quiet machine).
- Known limits: nothing in the world uses the economy yet (Phase 6 and 7 bring the town, the boss
  and the avatars); materials do not stack; a trade does not check the zone again at commit; the
  hire window is recorded but no avatar is spawned; item edges are validated but not read by the
  simulation.

**2026-10-01, Phase 6 done** (same machine, 1280 × 720, RADV; all numbers measured with the final
build, none estimated; ranges are the spread over repeated runs):
- `docs/MODELS.md` v1 is the contract, reviewed before coding (8 findings accepted, 1 in part, 1 as
  a cut, 3 rejected). A creator uploads a glTF `.glb` **for one frame**, on the standard rig of 24
  named bones in T-pose, with no animations: the animation set is the game's, shared by every
  model and every mannequin, so a custom avatar cannot hide a windup. Ingestion checks budgets,
  rig, pose and the **envelope** (head height within 8% of the frame's, a bounding box, and a
  rasterised coverage of 50–150% of the frame's mannequin inside the hitbox rectangle: no
  invisible avatars, no infiltrator dressed as a colossus) and re-encodes to `.gmm` (2.9
  [CORRECTED]). An avatar at the ceiling (3,486 triangles, 1024² atlas) is **1.17 MB as uploaded,
  382 KB stored and on the wire, 780 KB on the GPU**.
- New crates: `gm-model` (rig, container, strict reader, procedural animation set, mannequin;
  what the client links) and `gm-ingest` (glTF reading with its own bounds-checked accessor
  reader, texture resampling, BC1, the rasteriser, the envelope; hub worker and tools only).
- Hub: content-addressed store, `models` / `model_holders` / `model_events`, upload → pending →
  active | rejected, takedown and reinstatement, strikes, slots and quotas, the audit trail;
  ingestion in a child process under rlimits (2 GiB, 20 s CPU, 30 s wall clock), whose output the
  hub **verifies from the bytes** and whose preview the hub draws itself; uploads and downloads as
  raw bytes on the request's stream; Cubic on the hub endpoint. Tools: `gm-tools model`,
  `gm-tools mod`, `gm-hub --grant-moderator`.
- Zone: model ids ride the reliable stream (`Roster`, `PlayerInfo.model`, `ModelRevoked`), never
  snapshots; a takedown reaches every client through its zone within the tick. Client: a skinned
  character renderer (one draw call each), mannequins from the tick an entity appears, a model
  cache with a hard disk cap (LRU, pins) and a hard GPU cap, four loader threads, hash checked
  before parsing.
- **The town** (`assets/maps/src/town.map`, generated by `gm-tools map gen-town`; 255 KB `.bsp` +
  173 KB `.lit`, 1,236 faces, 128 spawns, sunlight through sky brushes) with a market of 30 stall
  tiles: the first place the economy shows in the world. Stalls are opened **through the zone**
  (it knows where a body stands; ECONOMY.md 7), persist for their 48 h, and are drawn with their
  keeper (the owner's frame, armour and avatar) whether the owner is online or not.
- **Acceptance** (`scripts/check-avatars.sh`, `budgets.toml [avatars]`), 100 distinct avatars at
  the ceiling in the town:
  - Offline, uncapped: **564–678 fps** average over eight runs (99th percentile frame 2.0–2.6 ms),
    348,600 triangles in 102 draw calls, client
    peak RSS **141 MB**, 78.0 MB of models on the GPU. With a 32 MiB cache cap against 38.2 MB of
    models the directory, sampled every 50 ms, never exceeded **33,218,927 of 33,554,432 bytes**;
    87 models stayed on disk and all 100 on screen. Most windowed runs have one stalled frame
    of 16–47 ms about two seconds after the window opens (the bench now prints which frame was
    the slowest); it also happens with mannequins only, never offscreen and not in the 25 s
    online runs (worst frame 12 ms), so it is taken to be the window system and is not chased.
  - Online, the real path (100 accounts each uploading its `.glb` through the hub's worker, a
    moderator approving, a town zone at 20 Hz, 100 bots wearing them and strolling, 12 stalls
    opened, the windowed client): **242 fps** (the client's 250 fps cap), 99th percentile
    6.1–6.2 ms, worst frame 12 ms, peak RSS **146–148 MB**; all 100 models fetched from the hub
    (38,159,385 bytes, 381,594 each); cache directory at most 33,295,020 bytes; a takedown while
    the client watched left 99 models drawn and no copy on its disk. Zone → client **5.1–5.8 KB/s
    per player** while the crowd leaves the spawns (4.0 KB/s once it has spread), 1.8–2.1 KB/s up;
    zone tick 2.0–2.5 ms mean with 101 players. At 64 Hz the same crowd costs 13.4 KB/s down and
    5.6 KB/s up. The worst of the 100 bots had 0 or 1 unexplained correction in its minute over
    three runs (the netcode budget is one per 10 s); the viewer had none.
  - More bodies: 200 (each model worn twice, 697,200 triangles) **395 fps**; 100 mannequins
    650 fps; the empty town 4,818 fps; the arena with the same 100 avatars 1,020 fps; headless
    offscreen 478 fps with 170 MB peak RSS. CI runs 48 avatars against a 16 MiB cap under
    software Vulkan (29 fps on llvmpipe, not gated).
  - Throughput: generating and ingesting 100 avatars takes 37–44 s (0.4 s each, generation
    included); uploading them through the hub, worker, verification and approval 37–50 s.
- Tests: 210 in the workspace, all green; among them the hub against real Postgres with the real
  worker process (upload to takedown; workers that crash, hang, are missing or lie), ingestion
  against hostile files (truncations, byte flips, bombs, an instanced index bomb, 200 random
  valid containers through `verify`), the cache (cap under concurrent writers, two processes on
  one directory, back-off, a takedown racing a download) and `crates/gm-server/tests/town.rs`
  (six bots with models in the town, two racing for one tile, a takedown, a late joiner).
- Reviews (MODELS.md 13): two rounds of independent code review, 14 findings accepted and fixed
  (an instancing bomb in the upload parser, slow senders holding the ingestion workers, statuses
  of one upload splitting under races, the fetch back-off that never doubled, takedowns undone
  by a download in flight, hub request floods through the zone), 2 rejected; the author's own
  pass found the cache overshooting its cap by 405,512 bytes with four loaders and the hub
  trusting its worker.
- Fixed on the way, from earlier phases: a character's saved position was applied on whatever
  map it entered next, so first entries and arrivals from another zone started at foreign
  coordinates, usually inside a wall, and could not move (Phase 4; HUB.md 3.2, migration 0004;
  the handoff test now requires the bots to cover ground); `check-perf.sh --gate-fps` could
  never pass (it parsed a line that does not exist; Phase 1); a reliable message to a client
  with a full queue was dropped silently (now the client is disconnected); the interpolation
  delay was six ticks at any rate, 300 ms in a 20 Hz town (now a time: 2 ticks, PROTOCOL.md 7.3);
  `Travel` could flood the hub; a claimed ghost was announced as gone twice.
- Binaries (release, LTO): `gm-client` **8,810,448 bytes (8.40 MiB)**, +354,856 for `gm-model`,
  the character renderer, the cache, SHA-256 and inflate; baseline updated. `gm-hub` 7.03 MB
  (+1.34 MB: glTF, image decoding and BC1 for the worker), `gm-server` 4.90 MB, `gm-bot` 3.82 MB,
  `gm-tools` 4.55 MB. Other gates unchanged: swarm 6.5–7.0 ms mean, 8.5–9.4 ms p99, 252–272 MB,
  10–19 KB/s per player over three runs; netcode and matrix green; test room 2,300–3,200 fps,
  128 MB.
- Known limits: no screen for any of it yet (uploading, wearing and moderating are `gm-tools`
  commands; a stall shows its keeper and counter but its listings have no UI; the HUD is Phase
  7); the animations are procedural placeholders; characters are not culled or given LODs (not
  needed at these numbers); the store is a directory and one hub process (object storage and a
  CDN at deployment); no ETC2/ASTC variant for mobile GPUs (Phase 8); the upload terms are a
  draft; who receives upload privileges is a moderator's decision per account.

**2026-10-01, Phase 7 done** (same machine; all numbers measured with the final build, none
estimated; ranges are the spread over repeated runs):
- `docs/COMPANIONS.md` v1 is the contract, reviewed before coding (8 findings accepted, 1 corrected
  differently, 1 rejected) and after (3 accepted, 1 that was wrong as stated but exposed a real gap,
  1 already covered, 1 rejected). **One kind of body, three drivers**: a companion and a creature
  are the same simulated body as a player and send the same inputs; a mind (`gm-ai`) knows what a
  client in its place would be shown and reacts, turns and aims within stated limits (200 ms, 720°/s,
  1.5° aim error, no shot through a friend).
- **Squads and hires.** A character commands 3 companions, 5 with a leadership ability
  (`war_standard`). A tavern hire is now what it buys: a copy of the listed character (name, build,
  model) in the hirer's squad for the hire's 12 h, spawned by the zone at the claim; at most the
  squad's capacity and one copy of an avatar; a hire ends when its owner plays the character or the
  hirer dismisses it, never in the middle of a fight. Tutorial zones lend recruits to the slots
  hires left empty, so a character without coin can play the tutorial.
- **The command stance and the tactical viewport.** Button bit 11 kneels the body: no movement, no
  action, no guard, 400 ms to stand up; it is part of the predicted mover. Only while it is held
  does the client see through its companions' eyes and may it give orders (`Follow`, `Hold`,
  `MoveTo`, `Attack`; eight a second; an `Attack` only on a body the client is being sent). The
  client's third viewport (`Tab`) is a camera 760 u from the body at 60°, a free cursor, selection
  and orders by keys and clicks; the world is drawn from the leaves the squad stands in, so where
  nobody of the squad sees the screen is dark. The client has a HUD for the first time: a 5×7 font
  drawn in the client, own bars, the squad panel, the bar of the creature being fought, messages.
- **Creatures, encounters, loot, trials.** Creatures are builds without a budget
  (`creatures.toml`) with threat tables (no taunt: nearness weighs threat times three), a leash, and
  kits a reader can learn. An encounter keeps a ledger; the dead wait; a party that is out has its
  own damage healed back; a clear splits the boss's components with the corrected split of Phase 5,
  from the `standard` list for a party with companions and the `top` list for humans only (the loot
  ceiling of 5.6), as **one idempotent hub transaction per kill**. Role trials (3.5) are an encounter
  judged through a lens (damage, blows taken, healing, damage dealt under orders), recorded at the
  hub, and a zone can require one (`--requires`).
- **The tutorial dungeon** (`gm-tools map gen-dungeon`; 123,568 bytes, 2,677 nav nodes): an entry
  hall, a passage with two turns, a gate of two sentinels (420 health each), a stair, the Warden's
  hall (7,500 health, a maul every 2 s that must be blocked or stood behind, a telegraphed quake).
  New content: `mend` and `sanctuary` for a healer (`Origin::Aim`; a packet of amount 0 is not an
  attack), the presets `mender` and `captain`, four trials on the Warden.
- **Acceptance: a solo player clears the tutorial dungeon with 3 hired avatars**, shown four ways.
  - Offline (`crates/gm-ai/tests/dungeon.rs`; five minutes of fighting simulate in 0.1 s): the
    reference squad (ironclad, mender, frostweaver) under a leader that only commands clears gate
    and Warden on **8 of 8 seeds in 141–270 s** (the Warden in 94–126 s; one reset in all), drops
    `core/iron, frame/ash, catalyst/basalt` and 30 copper to the one human, passes the leader's
    trial with 100% of the damage under orders. Without a healer, or with three blades, the Warden
    is not beaten in ten minutes on any seed (8–28 resets): the roles matter.
  - Over the protocol (`crates/gm-server/tests/dungeon.rs`, simulated network, 150 ms round trip,
    3% loss): a headless client that knows only its snapshots and messages clears it in **24 of 24
    runs** (121–229 s), 4.6–5.6 KB/s down, 5.5 KB/s up, no unexplained correction.
  - Through the hub (`crates/gm-server/tests/companions.rs`, real time, Postgres): three owners
    list avatars at 100 copper, the leader hires them (90 burned, 70 to each owner), enters with
    them and clears in 155 s (the test takes 162–174 s); the database holds the three components, the coin and the trial,
    the ledger is sound, a zone that requires the trial was locked before and opens after, and an
    owner playing its avatar ends that hire without a refund. `scripts/check-dungeon.sh --online`
    does it with the real binaries: a first clear with recruits (221 s) pays for the three hires
    of the second (273 s, the Warden in 215 s: a slow one, the damage dealer fell early).
  - Seen: the windowed client on a virtual display under software Vulkan, driven by real key and
    mouse events; a companion selected and sent to a point, the squad sent into the dark of the
    gate room, two right clicks on the sentinels, **the gate cleared in 14 s**.
- Budgets (`budgets.toml [companions]`, `scripts/check-dungeon.sh`): **16 leaders with 48
  companions and the 3 creatures in one zone: tick 1.2–1.8 ms mean, 1.8–2.4 ms p99**, no overrun
  (budget 4 ms); **3.9–6.5 µs per mind per tick** (budget 25); 15.3–17.9 KB/s down per leader, 19.3
  for the worst (67 bodies in a small dungeon); nav grid of the dungeon in 6–14 ms, of the arena
  (4,586 nodes) in 14 ms (budget 500 ms). The client in the tactical viewport: 5,400–7,300 fps
  headless at 1280 × 720, 120 MB peak RSS.
- What running it found, each with a test or a gate now (COMPANIONS.md 16):
  - **Two bodies in one place held each other for good**, since Phase 2. A spawn point offered 9
    places, so a team of 100 in the arena had 72: every swarm run since Phase 4 put the bots that
    found none inside a body. A body that begins a step inside another is no longer held by it
    (both sides of the wire, PROTOCOL.md 7.5) and a spawn point offers 25 places; the swarm now
    lands 40% more hits (737–988 a run against 485–698). That cost the simulation a millisecond,
    and the gate was at 7.2–7.9 of its 8 ms, until **sweeps against bodies got a grid** (bit-identical
    to walking the list): **200 bots now take 4.9–6.1 ms a tick (p99 7.1–8.9), the simulation
    1.3–1.6 ms of it** (Phase 6: 6.5–7.0 and 2.7–3.3 with part of the swarm stuck). This removes
    the O(N²) of movement against bodies that the Phase 3 review deferred.
  - A commander was not told when an order ended by itself, and led half its fights without
    orders (5 of 24 kills at 50–60% under orders; 24 of 24 at 100% after).
  - A kill's drop was not idempotent although ECONOMY.md said the kill's id made it so, and a
    grant whose answer was lost was lost: now one transaction that claims `(zone, kill)` first.
  - A leader who logged out past the gate came back at the Warden's feet (`--arrive-at-entry`).
  - `tests/counterpick.rs` came within 0.05 of its bar by chance in one run of a dozen, on the
    Phase 6 build as well (it failed once in a full run): ninety seconds instead of sixty, and it
    asserts the turn of the match. Since then one failure in 106 runs, the last 90 clean; which
    assertion it was is not known.
- Protocol v3 (PROTOCOL.md 14), hub v1.3 (HUB.md 3.5, migration 0005: `hires.ended`, `trials`,
  `kills`), vocabulary v0.4. `gm-ai` is no longer empty: minds, the nav grid (flood fill of the
  player hull, A*, strongly connected components tabulated so "is there a way" is two lookups),
  and the `Director` a zone runs them with.
- Tests: 249 in the workspace, all green (and one tuning run that is ignored). Other gates on the final build: matrix unchanged to the
  kill (85 : 30, 156 : 5, 100 : 58, mirror 70 : 69: the simulation changes are neutral for what
  existed); netcode green; avatars 100 at 507–564 fps (the Phase 6 build measured 450–483 fps on
  the same day); test room 4,797 fps, 129 MB; software Vulkan 227 fps, 170 MB.
- Binaries (release, LTO): `gm-client` **8,963,560 bytes (8.55 MiB)**, +153,112 for the HUD, the
  tactical viewport and the new messages; baseline updated. `gm-server` 5.49 MB (+0.59 MB: the
  minds), `gm-hub` 7.19 MB, `gm-bot` 4.06 MB, `gm-tools` 4.59 MB.
- Known limits: the HUD is bars, a squad panel and messages, nothing more (no inventory, tavern,
  stall, trade or upload screen: hiring is still `gm-bot --hire` or the hub API); no party
  invitations between humans, so a party is one human and its squad although the ledger, the split
  and the trials take any number; companions do not use cover, do not kite out of melee and do not
  interrupt; a fight in which the damage dealer falls early is won slowly by tank and healer (215 s
  seen, 488 s once) and then fails the trial's 300 s; the simulated-network tests are not
  bit-for-bit repeatable (QUIC draws its own random numbers), which is why they were run in dozens;
  one swarm run in twenty lost a bot in its first second and its cause is not known; how often a
  boss may be farmed is not decided (the Warden is back 120 s after it falls); markers in the
  tactical view are flat boxes, and an area is a square plate.

**2026-10-02, Phase 8 done** (same machine; Chromium 150 headless given the real GPU with
`--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan`; all numbers measured with the
final build, none estimated; ranges are the spread over repeated runs):
- `docs/WEB.md` v1 is the contract. **The browser client is `gm-client` compiled to wasm**: the same
  prediction, interpolation, renderer, HUD and viewports; what differs is what a browser forbids
  (sockets, threads, files, blocking). Zones and the hub open a **WebTransport listener beside their
  QUIC endpoint** (`--web-listen`; `wtransport`, on the same quinn) and a session carries the same
  bytes: PROTOCOL.md's datagrams, the control stream, the hub's one request per stream. One type,
  `gm_net::link::Link`, hides the difference from the tick loop, the sessions and the hub's handler.
  In the browser the transport is the browser's own `WebTransport`, bound by hand (2.1 and 11.2
  [CORRECTED]): no QUIC stack, no RNG and no tokio in the `.wasm`.
- **Two builds, chosen by the page's loader**, because the cost is not symmetric: on WebGPU wgpu is
  a thin binding (**905,937 bytes** of `.wasm`, **298,756** with brotli); for WebGL2 it links its GLES
  backend and the shader translator (**2,899,339 / 871,977**). A browser with WebGPU never downloads
  the second. JavaScript (wasm-bindgen glue, the loader, the page): 111 KB and 166 KB uncompressed.
  No framework, no bundler, no npm; the two build tools (wasm-bindgen CLI, wasm-opt) are fetched
  pinned and hash-checked like ericw-tools.
- **Certificates.** A browser takes a WebTransport server with a public certificate chain
  (`--web-cert/--web-key`) or with a pinned hash, for which the certificate must be ECDSA P-256 and
  valid for at most 14 days: without a certificate the listener makes one for 13 days and advertises
  its hash, and such a process must be restarted before it expires. Tickets carry the zone's web
  address beside its QUIC one (`ZoneTicket.web`, `TravelTicket.web`: protocol v4, hub v1.4).
- **The model cache without a filesystem**: the Cache API under the same rules as the directory
  (our own byte cap that holds at all times, room made before a write, the hash before the parser, a
  revoked model gone for the session); async fetches for the four loader threads; models parsed on
  the main thread, one a frame. The cache's state machine is shared with the native client.
- New for every client, found by asking what a hidden browser tab does: **a client that sends no
  input for 10 s is kicked** (60 s for its first input: it may be loading the map). A QUIC
  connection lives on keep-alives alone, which a browser's network stack answers whether or not
  the page runs. And a `Reject` or `Kick` is now followed by the close, not accompanied by it.
- **Acceptance: a browser client joins the same zone as native clients** (`scripts/check-web.sh
  --browser`): the arena with 15 native duelist bots and headless Chromium playing by script for
  40 s, once per build. Both builds: **64 snapshots a second with no gap, 60.0 fps** (the display's
  rate; p99 17.6–17.9 ms), 0 or 1 unexplained correction, damage dealt (30–312) and taken (291–768)
  as the zone counts it, zone → browser **7.0–8.7 KB/s** and browser → zone **6.6–6.7 KB/s** as the
  zone's QUIC statistics count UDP bytes (a native client sends 5.5), first frame **142–192 ms**
  after navigation on WebGPU and 258–292 ms on WebGL2, wasm linear memory **3.3 MiB** and 4.5 MiB.
  Without a browser: a QUIC bot and a WebTransport bot in one zone over real UDP
  (`tests/loopback.rs`), and the handoff test's second half, which registers, enters and plays
  through the hub's and a zone's web listeners.
- **Through the hub** (`--browser --hub`): login from the page over WebTransport, a town of 48 bots
  wearing uploaded avatars at the budget ceiling (18.3 MB) against a **16 MiB cache cap: 16,715,540
  bytes held** (44 models; read back entry by entry through the DevTools protocol, and the client's
  own count agrees to the byte), all 48 drawn at **60.0 fps** (p99 18.2–18.6 ms) with 8.6–9.3 MiB
  of wasm memory; on the next visit 32–37 of the 48 come from the cache; a travel to the arena
  fetches its map and ends there on a WebTransport session.
- Reviews (WEB.md 12). **Google AI Studio refused every request of the session with HTTP 402
  (prepayment credits depleted)**, so both reviews were done by independent agents given only the
  document or the diff. Design review: 15 findings, 13 accepted, 2 in part (the hidden tab that is
  never disconnected; Chromium's datagram queue of one; WebGL2 limits; lost `Reject`s; `connect=`
  links on a production site). Code review: 13 findings, 11 fixed, 1 accepted as intended, 1
  rejected for now (the idle kick hitting a browser still fetching its map; link options that could
  play a visitor's character; gate checks that passed on empty values; sessions never closed; a
  cache entry orphaned by a takedown during its write; a zone-chosen map name in a URL). Gemini
  should be run over both once the account has credit.
- Tests: the workspace suite green (252 tests); every earlier gate green on the final build: swarm
  5.9 ms mean / 8.6 ms p99 with 200 bots, netcode, matrix, perf, avatars, dungeon, handoff.
- Binaries (release, LTO): `gm-client` **8,990,752 bytes (8.57 MiB)**, +27,192 for the shared hub
  flow and the split cache (it links no WebTransport); baseline updated. `gm-server` 6.03 MB
  (+0.54 MB: wtransport and HTTP/3), `gm-hub` 7.67 MB, `gm-bot` 4.75 MB, `gm-tools` 4.59 MB.
- Known limits: the gate runs Chromium only (Firefox and Safari have the APIs; untested); pinned
  certificates are not rotated without a restart; one UDP port per transport; no threads in the
  browser (models parse on the main thread); two tabs of one browser can together hold twice the
  cache cap and the last-use order does not survive a reload; no touch controls; mouse acceleration
  under pointer lock is the system's; Chromium on Linux needs flags for WebGPU today and otherwise
  takes the WebGL2 build; on a software GPU the WebGL2 build runs at 41 fps and costs the zone
  19.6 KB/s; there is still no UI beyond the HUD and a login form on the page.

**2026-10-02, Phase 9 done** (same machine; all numbers measured with the final build, none estimated;
ranges are the spread over repeated runs):
- `docs/ANTICHEAT.md` v1 is the contract. **Statistics rank, people decide**: nothing here bans, kicks or
  demotes by itself. A zone started with `--replay-dir` records every tick as one frame of *every* entity
  (the snapshot codec, no PVS, no bands) plus the tick's events, holds the last 30 s in memory, and writes a
  `.gmr` file for **every fight between players** (opened by a hit between two clients' parties, closed
  10 s after the last, cut every 2 minutes, with 10 s of lead-in) and for **every report** (the ring and the
  next 10 s). New crate `gm-replay`: the file, its reader and playback, and the aim analysis, **one piece
  of code fed the same frames live in the zone and offline from a file**.
- **Aim statistics per shot** (ranged weapons are projectiles, so a shot has an ideal aim): the view against
  the direction that would hit the nearest hostile body from the muzzle with first-order lead, judged
  **against the world the shooter's view was of** (the tick its frames say they look at, per tick, so the
  numbers do not depend on how evenly frames arrive), and against the view the zone honoured too (the
  better fit counts: a client that lies about its view must still aim at the world its hits are resolved
  in). Flags: **flick** (≥ 35° within 4 ticks, at rest ≤ 3 ticks, on the body), **lock** (the view rides a
  moving body's ideal aim within 0.8° for 8 ticks, whatever the offset), **laser** (≤ 0.2° at ≥ 600 u),
  **spin** (a melee hit on a body 120° off the view 6 ticks before). **Reaction**: measured by the zone by
  line of sight, in the client's own view time. Rules are Wilson lower bounds (95%) on rates, so a rate on
  few shots is not a rate; `outlier` is two of three robust z-scores ≥ 4 against the week's accounts.
- **The hub's conduct module** (migration 0006, hub v1.5): aim numbers per account and week, flags (one per
  rule and week), replays stored by hash, players' reports (`Control::Report`, protocol v5; the hub opens
  the report first, the zone records after), three verdicts, a **reputation ledger** (team kills capped at
  5 a day, contracts paid or abandoned, reports upheld or abusive, model strikes, a confirmed cheat), trust
  tiers (only 0 → 1 is automatic: 10 h played, reputation ≥ 0, nothing upheld in 30 days; a zone may ask
  for a tier with `--min-trust`), account bans (login, enter, claim and handoff refused; sessions ended,
  characters kicked, stalls closed), and a log of everything a moderator does or looks at.
- **What a moderator has**: `gm-tools mod aim-report | replays | replay-get | reports | report | ban | unban
  | reputation | adjust`, `gm-tools replay info | aim [--shots NAME]`, and **`gm-client --replay FILE --follow
  NAME`**: the fight through the ordinary renderer from anybody's eyes (or third person, or the tactical
  camera), with pause, tick stepping, seeking and each shot's numbers on the HUD. In play, `F9` reports the
  player under the crosshair.
- **Acceptance: a replay of any contested fight is reviewable; an aim-outlier report per account**
  (`scripts/check-anticheat.sh --online --swarm`). The arena with 12 bots whose view moves like a hand
  (reaction time, turn rate, corrections a handful of times a second, a shake, no sight through walls; half
  of them lead their shots) and 4 that aim by program (2 locks, 2 flicks), 90 s: **all four programs
  flagged by the rule made for them, none of the twelve hands by any**, in five of five runs on a quiet
  machine and two of two under threefold CPU oversubscription. Lower bounds measured: lock programs 58–80% (51–76%
  loaded) against 6% or less for any hand; flick programs 90–94% (76–91% loaded) against 0%. The fight's
  replay (101 s, 16 players: **0.29–0.33 MB a minute**) reads back, **recomputes to exactly the line the
  zone logged for every participant**, and renders from a program's eyes. Through the hub: the two locks
  are flagged by `lock` in the aim report and neither hand is, a third lock that travels on to another
  zone keeps the numbers it made in the arena; the replay comes back as the bytes the zone
  wrote; a report is filed, gets its replay, is upheld once and only once, and both reputations move; a ban
  refuses the login with its reason; a `--min-trust 1` zone refuses a new account and admits it after a
  moderator's word; seven team kills are five ledger rows however often the zone repeats itself.
- **Cost**: recorder + sight + analysis **55–69 µs a tick with 16 players, 592–626 µs with 200** (the
  swarm gate with recording on: 6.4–6.6 ms mean / 8.7–9.1 ms p99, inside `[server]`); the ring holds
  0.32–0.38 MB for 16 players.
- **Found by running it** (ANTICHEAT.md 12.3): judged against the zone's clamped view tick a lock looked
  like a hand (the clamp is a tick or two off what the client saw); locks are a rate among shots at
  *moving* targets; **the first honest bots were cheats** (they turned towards bodies behind pillars, and
  the reaction rule said so); a hand that follows smoothly is a lock with an offset (the hand model now
  corrects a handful of times a second); the sight sweep was quadratic (3.5 ms a tick at 200 players:
  now the four nearest on screen, 0.3 ms); **uneven frame delivery hid the lock** (one gate run in nine:
  under load four of six lock programs escaped, until each tick's view was held against its own world and
  the cheat model stopped counting its own frames).
- Reviews (ANTICHEAT.md 12). **Google AI Studio again answered HTTP 402** (credits depleted), so both
  reviews were done by independent agents. Design review: 19 findings, 13 accepted, 5 in part, 1 rejected
  (view-time targets, the muzzle, party-only hostility in wild zones, steadiness instead of error for the
  lock, line-of-sight reactions, hub-first reports, three verdicts, no automatic demotion). Code review: 14
  findings, 13 fixed, 1 in part (aim numbers lost on a handoff; a lying view tick; a zone that waited an
  hour for a dead hub at shutdown; colliding report files; flags that never closed; unbounded reports; a
  week of numbers lost to a race; gate checks that compared a file with itself). Gemini should be run over
  the document and the diff once the account has credit.
- Tests: the workspace suite green (266 tests); every earlier gate green on the final build: netcode,
  matrix, swarm 6.2 ms mean / 8.5 ms p99 with 200 bots, perf 2,433 fps, 100 avatars at 712 fps
  offline and 243 fps through the hub (cap 250), dungeon, web (both builds in headless Chromium, and
  through the hub).
- Binaries (release, LTO): `gm-client` **9,168,512 bytes (8.74 MiB)**, +177,760 for the replay
  viewer and the analysis it shows; baseline updated. `gm-server` 6.51 MB, `gm-hub` 7.95 MB, `gm-bot` 4.85 MB, `gm-tools` 4.84
  MB. Browser builds: WebGPU 938,472 bytes, WebGL2 2,930,902 (the report key and its messages).
- Known limits (ANTICHEAT.md 9, 11): **the thresholds were tuned on models, not on people**, and the first
  weeks of real play are their calibration; aim assistance tuned to stay inside a hand's distribution, a
  triggerbot, ESP inside the PVS by a patient cheat, and anything in melee beyond spin are not caught; a
  build that barely shoots gives the rules nothing; one fight per zone at a time (a busy contested zone
  records continuously); no replay browser in the client and no replays for players; no asset freeze on a
  ban; anybody with an account can report (five open reports each); the privacy notice, the impact
  assessment and a self-service export of an account's record are the operator's to do before launch.

## 12. Open decisions
License split (recommend GPLv3 client / AGPLv3 server / CC-BY-SA content). The type matrix and attribute
set are **proposed** in `docs/MATRIX.md` 12 (implemented and measured; the director confirms or changes
the numbers, the structure is what the code depends on). The economy numbers are **proposed** in `docs/ECONOMY.md` 12 (slot counts, the purchasing-power scale,
the floors, the salvage rule, the contract timeout, no self-hire, no stacking of materials in v1).
The model numbers are **proposed** in `docs/MODELS.md` 13 (the envelope: head pivot ±8%, top 0.90–1.15 h,
box, coverage 50–150%, T-pose within 15°; 4 slots, 3 pending, 10 uploads an hour, 3 strikes, trust tier 2
skips the queue, 8 MiB uploads, 256 MiB of models on the GPU) and so is **who may upload**: today a moderator
grants `upload_privileges` per account. The upload terms in MODELS.md 10 are a draft for counsel.
The companion numbers are **proposed** in `docs/COMPANIONS.md` 16 (squad of 3 and 5, the 400 ms of the
stance, the limits of a mind, the threat rule, the Warden's and the sentinels' health, the four trials and
their thresholds, respawns of 120 s and 600 s, creature health on the wire, one copy of an avatar per squad,
dungeons putting every arrival at their entry) and so is the reading of 5.2 noted there. **How often a boss
may be farmed** is open: the plan keeps a daily cap only as a bot-farm brake and asked to revisit it after
load testing; nothing limits it today but the fight's length and the respawn.
The web numbers are **proposed** in `docs/WEB.md` 9 and 2.1 (the size budgets of both builds, 10 s
without input and 60 s for the first, the grace after `Reject` and `Kick`, a 128 MiB default cache in
the browser, 13-day pinned certificates), and so is **what a link may set** on a production site
(WEB.md 5: only what is seen).
The anti-cheat numbers are **proposed** in `docs/ANTICHEAT.md` 12 (every threshold of the aim rules and
their minimum samples, the recorder's windows and its byte ceiling, retention of 14 and 90 days and 26
weeks, the reputation weights, ten hours of play for tier 1, five open reports per account, who may
report at all), and so are two readings of section 8: in a wild zone a "team kill" is a kill inside one's
own party only (every human is on one team there and kills between parties are the game), and a flag
never does anything by itself. **Who moderates** is open: today an account whose `accounts.moderator`
flag an operator has set in the database.
Death-drop in contested zones: on/off and fraction. Housing: instanced interiors vs world plots. Name.
