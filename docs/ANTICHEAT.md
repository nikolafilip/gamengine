# Anti-cheat: statistics, replays, reputation

Status: v1 (Phase 9). This document is the contract for what the server keeps about how
people play and what is done with it. PLAN.md 8 (an open client and server authority stop
speed, teleport, damage and inventory cheats; they do **not** stop aimbots, triggerbots or
ESP; the layers are projectile-only ranged weapons, PVS culling, server-side statistics,
replays with review, reputation and trust tiers; bans are account-level), 4.4 (team-kill
statistics feed reputation), 6 (a public success ledger for contracts) and 11.8 Phase 9
(**a replay of any contested fight is reviewable; an aim-outlier report per account**) are
binding. When the code and this document disagree, the document wins. Section 12 records the
reviews and what running it found.

## 1. Principles

1. **The server judges what the server saw.** Every number here is computed from the
   authoritative simulation: the frames a zone executed, the projectiles it flew, the hits it
   resolved. Nothing a client says about itself is evidence, with one exception: the
   world tick its frames say they were looking at, which is believed only where it does
   not help the client (4.1, 4.2).
2. **Statistics rank, people decide.** A statistic never bans, kicks or demotes. It puts an
   account on a list with the fights to look at; a moderator watches a replay and gives a
   verdict; the verdict is a row with a name on it. The one thing done without a person is
   *keeping the evidence*.
3. **A replay is the zone's own recording**, not a client's: every body, every tick, from
   nobody's point of view, viewable from anybody's eyes.
4. **What can be recomputed is not trusted twice.** The aim statistics are computed from
   recorded frames by one piece of code (`gm-replay`): the zone feeds it the frames as it
   records them, `gm-tools replay aim` feeds it a file's. A moderator checks the number
   against the picture, and the gate checks that both give the same numbers.
5. **Reputation is a ledger**, like coin (ECONOMY.md): every change is a row with a kind, an
   amount, a reference and a time; the total is a sum moved in the same transaction.
6. **Measured.** Recording and analysis have a tick budget and a byte budget.

## 2. What server authority already settles

Listed so that nobody builds a detector for it: position, speed, health, damage, cooldowns,
resources, inventory and coin are the server's; a client that claims otherwise is corrected
(PROTOCOL.md 7.2) or ignored. Frames beyond 64 a second are dropped by the token bucket;
malformed datagrams are counted and end the session past 100. Wallhacks see nothing behind
solid geometry outside the PVS. What is left is what a legitimate input stream can express:
**where the view points and when the trigger is pulled**.

## 3. Replays

### 3.1 The recorder

A zone started with `--replay-dir DIR` records. Each tick the recorder takes the tick's
entity table (the one snapshots are built from, PROTOCOL.md 5) and writes one **frame**: a
snapshot of *every* entity (no PVS, no distance bands, no datagram's size cap, health for
every body), delta-encoded against the previous frame with the snapshot codec, and the
tick's **events**: hits (attacker, target, amount, kind, what a guard absorbed), kills,
parries, guard breaks, respawns, **shots** (a projectile leaving: its owner, where it left
from, its speed, gravity and lifetime, and how far behind the present its shooter's view
was, both as the client claimed and as the zone honoured, 4.1), each client's **view lag**
(how far behind the tick its frames say they look: written when it changes, and for
everybody at a keyframe), **reactions** (4.2), and who joined and left. Every 128th frame is
a full snapshot (a keyframe: a file begins at one). Names, teams, parties, builds, the hub's
character ids and who drives a body (a client, a companion, a creature) are a **roster** in
the header, kept current by the join and leave events.

The recorder holds the last 30 s in memory (a ring, trimmed in whole keyframe groups so it
always begins at a keyframe), and writes nothing until there is a reason:

- **A fight between players.** A hit by one client's party on another's opens the zone's
  *fight*; it stays open while such hits keep coming and closes 10 s after the last. A
  closed fight in which somebody died, or at least 100 damage was dealt between players, is
  written: from the keyframe at least 10 s before its first hit to its end. A fight that
  runs on is cut every 2 minutes at a keyframe and continues in the next file. Companions
  fight for their commander's party; a fight against creatures only is not a fight between
  players. A zone has one fight at a time: in a busy contested zone that is one continuous
  recording in two-minute files, which is what "any contested fight" costs (10).
- **A report** (5): the ring as it stands and the next 10 s, whatever happened in them.
  Reports that arrive while a report's file is open share that file (it names all of
  them) and extend it, up to two minutes: a zone has at most one report file open.

`--replay-mb-per-hour N` (default 512) is a ceiling on fight files per wall-clock hour,
counted before compression; past it fights are dropped and counted. Reports' files have
the same ceiling again, of their own; when it is spent, reports are refused until the hour
turns. A frame holds at most 4,096 entities (what the codec decodes); in a zone with more,
the newest projectiles are left out and the frame is counted. When the zone stops, what is
open is written as it stands.

### 3.2 The file (`.gmr`)

```
"GMR1"            magic
u32 LE            length of the header
header            bitcode: version, snapshot codec, content hash, teams or not, zone id,
                  map name and hash, tick rate, first tick, wall-clock start (unix seconds),
                  why it was written (fight | report), the hub's reports it was written
                  for, the roster at the first frame
zlib stream       frames: bitcode Vec<Frame { tick, snapshot bytes, events }>
```

The snapshot bytes are stored without the datagram header, and the file names its own
**snapshot codec** version, not the protocol's: a new control message bumps the protocol
and leaves every recorded fight readable. The content hash (FNV-1a of the pack the zone
sends clients) says which abilities the indices in the frames meant. The file name is
`<zone>-<unix seconds>-<first tick>[-report<n>].gmr`; it is written under another name and
renamed when whole. A reader refuses a damaged or truncated file whole, and a file over
64 MiB, inflating past 512 MiB, of more than 38,400 frames (ten minutes) or more than 24
million entity records: a reader holds every frame's table, and a hostile file must not be
small on disk and enormous in memory.

### 3.3 Where replays go

Without a hub they stay in `--replay-dir`. Under a hub the zone uploads each file
(`ZoneReplay`, the bytes raw on the stream as model uploads are) with its summary: kills,
damage between players, duration, the reason, the reports it was written for, and for each
participant its character and its aim numbers over that file. The hub checks that the bytes
are a replay, stores them by SHA-256 under `<models-dir>/replays/`, writes a `replays` row
and one `replay_participants` row per character (with the rules its numbers in that file
break), and answers with the replay's id; the zone then deletes its copy. The same bytes
sent twice are one replay. A zone that cannot reach the hub tries again once a minute for
an hour, then leaves the file where it is; a zone that is stopping tries once, for five
seconds. A zone started on a directory that holds replays sends them first: nothing a
stopped zone left behind is lost, and the file itself names its reports. A file the hub
refuses is set aside as `.refused` and not offered again.

**Retention.** 14 days, then the hub deletes bytes and rows, except a replay that a report
or a flag points to: those are kept while the report is open or the flag is not closed, and
at most 90 days more. A flag closes by itself 90 days after it was raised. Files in the
store that no row names are removed after an hour. Aim weeks are kept 26 weeks. Reports,
flags, the reputation ledger, bans and the moderators' log are kept for the life of the
account (they are its record, and a ban that could be forgotten is not one); deleting the
account deletes them.

### 3.4 Watching one

`gm-client --replay FILE [--follow NAME] [--from SECONDS]` plays a file through the ordinary
renderer: no connection, no prediction, bodies interpolated between frames as other players
are in live play. The camera is a body's own eyes with its recorded view angles, or third
person behind it (`V`), or the tactical camera over it (`Tab`). `[` and `]` step through the
players, `Space` pauses, `,` and `.` step a tick when paused, the arrow keys seek 5 s, `1`
to `4` set the speed (¼, ½, 1, 2). The HUD shows the time, the followed body's name, build,
team and health, its numbers over the file and the rules they break, each hit on or by it,
each kill, and for each of its shots the numbers of 4.1 as it is fired.

**What the eyes do not show.** A third-person player's camera sits behind and beside its
body, and the client aims the eye ray at what that camera's ray hits; the replay has the eye
angles, not the camera. And the replay shows every body, where the zone sent that client
only those in its PVS. A reviewer who sees a view follow a body through a wall is looking at
something the client was sent (the PVS is coarser than sight) and a hand would still not do.

`--headless --replay FILE --screenshot OUT.ppm` renders it without a window.
`gm-tools replay info FILE` prints the header, the participants, the kills, the sizes and
the spread of the clients' view lags;
`gm-tools replay aim FILE [--shots NAME|all]` recomputes the aim statistics of every
participant from the frames, and lists every analysed shot of a player.

## 4. Aim statistics

Everything here is computed per **tick** from the recorded frames: a body's position and
view angles as the wire carries them (a quarter unit, a tenth of a degree) after the tick's
frames ran, and the world tick those frames said they looked at. A client that sends two
frames in a tick shows the view of the second. The frames a zone runs in a tick are not the
frames the client drew in it (they come late, in bursts, two at once): a view is therefore
always held against **the world that view was of**, tick by tick, and the numbers do not
depend on how evenly a client's frames arrive. Sending them unevenly on purpose buys
nothing.

### 4.1 Per shot

A **shot** is a projectile leaving a client-driven body. At that tick:

- **The world the shooter saw.** A client aims at what its screen shows, which is the past:
  its frames carry the world tick they were looking at. Aim is judged against the bodies as
  they were at that tick (bounded to half a second back), not as they are when the frame
  runs: a perfect aim judged against the present would look six degrees off at 100 ms.
  Hits are resolved against a *clamped* view tick (PROTOCOL.md 7.4), and the two can
  differ. Every rule here flags aim that is too good, so a client that claims a view it
  does not have would look clumsy and still hit. Therefore the shot is judged against
  **both** worlds, the claimed and the honoured, and the one its aim fits better counts:
  to hit, a liar must aim at the world its hits are resolved in. A shot that fitted the
  honoured view better is counted (`stale_views`): now and then is jitter, most of the
  time is a lie.
- **The target**: among living hostile bodies (another party; where the zone has teams,
  another team) within the projectile's range, the one whose **ideal aim** is angularly
  nearest to the view, if that is within 25°. The ideal aim is the direction from the muzzle
  (where the projectile left, not the eye) that meets the body's centre, given the
  projectile's speed and gravity and the body's velocity over the last 4 ticks: first-order
  lead, the formula a mind uses. A shot with no such target is counted and not analysed.
  There is no line-of-sight test: a view locked onto a body behind a pillar is a finding.
- **error**: the angle between the view and the ideal aim. **On target**: the error is
  inside the body's half width seen from there (12 u at that distance; at least 0.6°).
- **lead**: the angle between the ideal aim and the straight line to the body. A **hard**
  shot needed 3° or more.
- **snap**: the largest angle the view turned across any 4 consecutive ticks of the 8 before
  the shot, and **settle**: the ticks since the view last moved by a degree or more.
- **steady**: the standard deviation, over those 8 ticks, of the view's offset from the
  ideal aim at the same target (yaw and pitch), each tick's view against the target as it
  stood in the world that tick's frames looked at. A constant offset does not change it.
- **target motion**: how far the ideal aim itself travelled over those 8 ticks.
- **hit**: whether a projectile of that owner damaged that target while the shot was in
  flight (each hit credits the oldest such shot).

Three flags a shot can carry:

| Flag | Rule | What it is |
|---|---|---|
| **flick** | snap ≥ 35° and settle ≤ 3 ticks and on target | a turn of 35° that ends on the body within 47 ms and is fired from at once |
| **lock** | target motion ≥ 4° and steady ≤ 0.8° and on target | the view rides a moving body's ideal aim, with whatever offset, for an eighth of a second |
| **laser** | error ≤ 0.2° at 600 u or beyond | the wire's own resolution is 0.1°; a hand does not sit on it at range |

Melee swings are not analysed for aim (an arc is 80° to 90° wide), with one exception,
**spin**: a melee hit on a body that was more than 120° off the view 6 ticks earlier.

### 4.2 Reaction

Measured by the zone, which has the map, and recorded as an event; the analysis counts it.
For each client and each hostile client's body within 2,500 u: while the body is on the
client's screen (within 50° of its view) and hidden (a trace from eye to body centre is
blocked), the zone watches; when it has been hidden for at least 250 ms and then is not, and
the view is at least 15° from it at that moment, it has **appeared**. The reaction is the
time from there to the first tick the view is within 3° of the body, if within 1.5 s,
counted **in the client's own view of the world**: the world tick its frames say they were
looking at when the view arrived, minus the tick the body appeared. Latency is in neither
term. The claimed view is not believed to be newer than a client's can be: a frame that runs
now left the client one way ago and was drawn from a snapshot one way older, held back by
the interpolation delay the protocol's client draws with (six ticks at 64 Hz, PROTOCOL.md
4). Claiming a fresher view to make reactions look slow buys nothing. (A modified client
that draws with less delay really does see the world sooner; its reactions are then measured
up to 94 ms too short, which a person's reaction still clears by far.) A value near zero or
below it means the view was on its way before the client could have seen the body.

Half the viewers are swept each tick, each over the **four nearest** hostile bodies on its
screen (a sample: the cost must not grow with the square of the zone, 12.3); a visible
pair's line of sight is rechecked every fourth sweep, a hidden pair's on every sweep. A pair
that leaves the watched set starts again when it returns.

### 4.3 Per session, per account

The zone sums per client: shots, analysed shots, hits, flicks, shots at moving targets and
locks among them, long shots and lasers among them, hard shots and hits among them, melee
hits, spins, a 16-bucket histogram of error (0.1° to 25°, logarithmic), reactions and a
12-bucket histogram of them, kills, deaths, damage dealt to and taken from other clients'
parties, team kills (6), shots that fitted the honoured view better than the claimed one.
When the body leaves the zone, however it leaves (the session ends, another zone claims
it, its ghost runs out, the character joins again), the zone logs them ("aim at leave") and,
under a hub, reports them (`ZoneAim`, with a nonce so a repeated report counts once); the
hub adds them to the account's row for the current week (`aim_weeks`), one report of an
account at a time. The row names the layout of its numbers; after a change of layout a
week begins again rather than being misread.

### 4.4 Rules and the outlier report

A rate is believed only as far as its sample carries it: the **lower end of the 95% Wilson
interval**. Rules:

| Rule | Broken when |
|---|---|
| `lock` | at least 10 shots at moving targets, and locks among them ≥ 35% |
| `flick` | at least 15 analysed shots, and flicks among them ≥ 10% |
| `laser` | at least 10 long shots, and lasers among them ≥ 25% |
| `reaction` | at least 10 reactions with a median of 60 ms or less |
| `outlier` | two of three robust z-scores ≥ 4 against the accounts of the report: hit rate on hard shots (20 or more), lock rate (10 or more shots at moving targets), flick rate (15 or more shots), the last two as Wilson lower bounds |

The first four are absolute, because a population of cheaters has a cheater's median. The
z-scores use the median and the median absolute deviation with a floor, so that a signal
most accounts have at zero gives a large finite score and not an infinite one.

A rule broken is a **flag**: a row (account, rule, week, the numbers, the replay if a file
showed it), one per rule and week. The hub writes flags when a `ZoneAim` report breaks a
rule by itself (one session) or brings the week's sum to break one, and when a replay's
summary shows a participant breaking one in that file (the replay is then kept). A flag sets
nothing else (principle 2).

`gm-tools mod aim-report [--weeks N] [--min-shots N]` lists accounts with enough shots, rule
breakers first, with their rates, medians, z-scores and their five most recent replays.

## 5. Reports

A client may report a client-driven body that is in the zone, or left it within the last two
minutes: `Control::Report { target, reason }` (reasons: aim, griefing, other; there is no
"speech" reason, because chat is not recorded and a report without evidence is a rumour).
The zone first checks that there is such a body, then allows one report per client per
30 s, and refuses when the hour's bytes for report files are spent (3.1). Under a hub the
**hub is asked first** (`ZoneReport`): it refuses a reporter with five open reports, or
with one already open on that target, or one reporting its own account; only when it has
opened the report does the zone start (or extend) the replay of 3.1, which is uploaded
with the ids of the reports it was written for and attached to each.
Without a hub the zone records at once. The client is answered `ReportResult`.

A report is `open`, then decided once by a moderator who is not a party to it:
`gm-tools mod report ID uphold|not-proven|abusive [--note TEXT]`.

## 6. Reputation and trust

`reputation` is a ledger at the hub: `(id, account, kind, delta, reference, note, at)`; an
account's reputation is the sum, kept in `accounts.reputation` by the same transaction.

| Kind | Delta | Written when |
|---|---|---|
| `team_kill` | −2 | a client kills a body of its own party, or of its own team in a zone that has teams (in a wild zone every human is on one team and kills between parties are the game). At most 5 a day count. |
| `contract_paid` | +1 | a carry contract is paid: to each seller, once per buyer account per week (ECONOMY.md 8) |
| `contract_abandoned` | −3 | a contract is refunded because the sellers abandoned it |
| `report_upheld` | −10 | to the target of a report a moderator upheld |
| `report_helpful` | +1 | to the reporter of an upheld report |
| `report_abusive` | −3 | to the reporter of a report a moderator found made to harm |
| `model_strike` | −5 | a model of the account is rejected or taken down for cause (MODELS.md 10); `model_strike_returned` +5 when the decision is undone |
| `cheat_confirmed` | −100 | a moderator bans with `--cheat` |
| `moderator` | any | by hand, with a note |

A report found `not_proven` costs nobody anything.

**Trust tier** (`accounts.trust_tier`): 0 new, 1 established, 2 trusted. The only automatic
movement is **0 → 1**: at least 10 hours played across the account's characters, a
reputation that is not negative, and no report upheld in the last 30 days; looked at when a
character leaves a zone, when aim numbers arrive, and at the door of a zone that asks for a
tier. Every other change, and every demotion, is a moderator's (`gm-tools mod trust`). Tier
2 also skips the model queue, as before (MODELS.md 6). A zone may require a tier (`gm-server
--min-trust N`, registered with the hub): `Enter` and a handoff are refused below it.

**A ban** is account-level (PLAN.md 8: no hardware identifiers) and lives in `bans` only:
`(account, until, reason, by, at, lifted, lifted_by)`. While one is in force `Login` is
refused with the reason and the end (after the password: the reason is the account's own
business), `Enter`, `Claim` and handoffs are refused, the account's sessions end, its
characters are kicked from their zones and its stalls close. Lifting a ban marks the row.
The reason shown at login is the statement of reasons; an appeal goes to the operator's
contact and its outcome is a `moderator` row or an `unban`.

## 7. What a moderator does

```
gm-tools mod aim-report                  who stands out, and why
gm-tools mod replays --account A         the fights of an account (or --reported, --flagged)
gm-tools mod replay-get ID --out F.gmr   then: gm-client --replay F.gmr --follow NAME
gm-tools replay aim F.gmr --shots NAME   the numbers of that fight, shot by shot
gm-tools mod reports                     players' reports, open ones first
gm-tools mod report ID uphold|not-proven|abusive
gm-tools mod ban A --days 30 --reason "aim assistance, replay 4711" [--cheat]
gm-tools mod unban A --note TEXT
gm-tools mod reputation A                the ledger, the tier, the flags, the ban
gm-tools mod adjust A -- -5 --note TEXT
```

Every one of these is a row in `mod_log` (who, what, on whom, when), including the ones
that only look: a replay is other people's play.

## 8. Hub protocol and database

- `ZoneHello` gains `min_trust`. Zone requests: `ZoneAim { nonce, character, stats }`,
  `ZoneReplay { summary, len }` + bytes, `ZoneReport { reporter, target, reason }`. Client
  to zone: `Control::Report`, answered `Control::ReportResult`. Moderator: `ModOp::{AimReport,
  Replays, ReplayGet, Reports, ReportVerdict, Ban, Unban, Reputation, Adjust}`.
  `HubError::Banned { until, reason }`.
- Migration 0006: `replays`, `replay_participants`, `aim_weeks`, `aim_reports`, `flags`,
  `reports`, `reputation`, `bans`, `mod_log`; `accounts.reputation`.
- Protocol v5 (the two control messages), hub v1.5.
- A zone speaks for the characters that play or played in it: the hub accepts aim numbers
  for any existing character from any registered zone (a leaver's numbers arrive after it
  went offline). Zones are the operator's own processes, behind the zone secret.

## 9. Privacy, and the limits of the claim

**What is kept and why.** A replay holds positions, view angles and names of everyone in a
fight, never chat. The statistics are derived from play. The purpose of both is one: to
decide whether play was fair; they are not used for anything else, and the death-cam
exporter of PLAN.md 10 will be a separate purpose with its own consent when it is built.
The legal basis is the operator's legitimate interest in a fair game, to be stated in the
terms and the privacy notice with the retention of 3.3; systematic analysis of behaviour
calls for a data-protection impact assessment before launch. Replays and reports are
readable by moderators only; a reporter's identity is not shown to the reported. An
account's own record (its statistics, flags, ledger, bans) is to be given to it on request;
that is a moderator's export today. A third party that runs its own hub from this source is
its own controller.

**What the statistics do not catch**, stated so that nobody believes they do:

- Aim assistance tuned to stay inside a hand's distribution: lower gain, added noise, a
  delay. It gives up most of what it was for; what remains shows as a hit rate, which is in
  the report and is no rule by itself.
- A triggerbot (the trigger pulled when the view crosses a body): there is no press-timing
  signal yet.
- ESP inside the PVS: a client is sent bodies behind a pillar in its own leaf's visible set.
  The reaction rule sees a view that was on its way before the body showed; a patient
  cheat is not caught by it.
- Anything in melee beyond spin.
- A build that barely shoots: the rules need shots (ten at moving targets, ten long ones,
  fifteen in all). The gate's own cheat with a sword and thirty bolts in ninety seconds was
  not flagged; with a kit that shoots it is.
- The view tick is whole ticks. A client draws between ticks and reports the tick below:
  against a body crossing at more than two degrees a tick the residual carries up to that
  much sawtooth. And a frame that reaches the zone in a later datagram (its own was lost)
  carries that datagram's view tick, a tick or two newer than its own.
- **The thresholds were tuned on models, not on people**: bots whose view moves like a hand
  (a reaction time, a turn rate, corrections a handful of times a second, a shake, no sight
  through walls) against bots that aim by program. They are proposed numbers; the first
  weeks of real play are their calibration, and until then a flag is a reason to watch a
  replay and nothing more. That is what principle 2 is for, and why PLAN.md 8 calls it an
  arms race.

## 10. Budgets and acceptance (PLAN.md 11.8 Phase 9)

`budgets.toml [anticheat]`, gated by `scripts/check-anticheat.sh`:

| What | Budget |
|---|---|
| recorder + sight + aim analysis, mean per tick, 16 players in a fight | 150 µs |
| replay bytes per minute of a 16-player fight | 1.5 MiB |
| the swarm gate with the recorder on (`--swarm`) | inside `[server]` |

Acceptance:

1. **Synthetic traces** (`crates/gm-replay/tests/aim.rs`): a hand is clean; a lock (with and
   without a constant offset), a flick (in one frame and spread over two), a laser, a spin
   each produce their flag and no other; reactions are counted and the rule needs ten; a
   rate on few shots is not a rate; in a zone without teams only the party is a friend; a
   lock that claims a stale view is still a lock, and an honest old view is not stale; a
   file written and read back gives the same numbers, and a damaged one is refused. And the
   recorder (`gm-server`, `recorder::tests`): the numbers the zone computes live are the
   numbers of the file it writes; a fight that runs on is cut into files that each read,
   with no frame and no damage lost at the cut.
2. **A real zone** (`scripts/check-anticheat.sh`): the arena with `--replay-dir`, 12 bots
   whose view moves like a hand (half of them "sharp": they lead their shots) and 4 that aim
   by program (2 locks, 2 flicks, with kits that shoot), 90 s, one bot reporting another.
   Passes when all four programs are flagged by the rule made for them (a lock by `lock`, a
   flick by `flick`) and none of the twelve hands by any; a fight and a report were written;
   `gm-tools replay info` reads each; `gm-tools replay aim` recomputes, for every
   participant of every file, exactly the line the zone logged for that file; the costs are
   inside the budgets; and the client renders the fight headless from a program's eyes.
3. **Through the hub** (`--online`, `crates/gm-server/tests/anticheat.rs`): two hands and
   two locks play in a recorded arena, and a third lock travels on to a zone that records
   nothing after half a minute; their numbers and the replays are in the database (the
   traveller's too: the arena reported them when the other zone claimed the character); the
   aim report flags the two locks by `lock` and neither hand; the fight's replay comes back
   as the bytes the zone wrote and analyses to what the hub was told; a report was filed,
   has its replay, is upheld once and only once, and both reputations move; a ban refuses
   the login with its reason and is lifted; a zone with `--min-trust 1` refuses a new
   account and admits it after a moderator raised its tier; a zone's report of seven team
   kills is five ledger rows, and repeated it is still five; the moderators' log has it all.

## 11. Deliberately absent

- Automatic bans, automatic kicks, automatic demotion, shadow bans. Client-side anti-cheat
  of any kind.
- Hardware or IP bans. A public reputation number. An asset freeze on a ban (a banned
  account cannot log in to trade, but what it gave away before is gone; nothing is
  soulbound, PLAN.md 0).
- What happens to a banned account's running contracts, hires and guild chests: they run
  out as they would for any account that stays away.
- A replay browser in the client and replays for the players themselves.
- Per-ability statistics (the three ranged kits are pooled), tracking latency by
  cross-correlation, press-timing (triggerbots), dwell on hidden bodies, movement analysis,
  chat moderation.
- Chunked recording spooled to disk with a fight index, and fights per connected group of
  parties: a zone has one fight at a time and holds an open one in memory for at most two
  minutes.
- Voting, juries, community review queues (PLAN.md 8 "community report + admin review": the
  report exists, the review is a moderator's).

## 12. Proposed numbers and review log

Every threshold in 4.1, 4.2 and 4.4, the window lengths and the byte ceiling of 3.1, the
retention of 3.3, the weights and tiers of 6 are **proposed**; the director confirms or
changes them. The thresholds of 4 were tuned on models (9).

Google AI Studio answered every request of this session with HTTP 402 (credits depleted), so
the reviews were done by independent agents given only the document or the diff; Gemini
should be run over this document and the Phase 9 diff once the account has credit.

### 12.1 Design review (before coding)

Nineteen findings; thirteen accepted, five in part, one rejected.

| # | Finding | Verdict |
|---|---|---|
| 1 | The ideal aim was computed against the present; the shooter aimed at the world as it saw it (6–9° of false error at ordinary latency) | **Accepted**, and taken further by what running it found (12.3): the client's claimed view tick, not the clamped one (4.1) |
| 2 | The acceptance could not pass: the duelist bot snaps without lead and so misses, and there was no bot with a hand | **Accepted.** `gm-bot --aim hand|sharp|lock|flick` (4 populations) |
| 3 | In a wild zone every human is on team 1: every kill would be a team kill; griefers can die into friendly areas | **Accepted** the first (the party always, the team only where the zone has teams). The second in part: team kills are capped at five a day and demote nobody |
| 4 | Every threshold was narrower than the target: a 0.7° offset evades all; a flick is evaded by snapping earlier | **In part.** Lock is steadiness against the ideal aim (offset-invariant) and "on target" is the body's width; flick's error bound is the body's width; hard-shot hit rate is reported. Tracking latency and press timing are not built (11) |
| 5 | The per-tick table cannot reproduce the statistics: executed frames, velocities, lag, sent sets, shot records, content | **In part.** The statistics are *defined* on ticks and on recorded events (shot, reaction), so they reproduce exactly (principle 4); velocity is taken over 4 ticks; the header names the content. The camera mode and the sent set are not recorded (3.4) |
| 6 | Reaction from "entered the sent set" is unsound: the PVS is not sight, nothing appears in an open hall, RTT subtraction over-corrects | **Accepted.** Line of sight, measured by the zone, in the client's own view time (4.2) |
| 7 | Sizes: squads triple a file; a busy zone records continuously; an open fight is buffered outside the ring budget; the ring must be trimmed at keyframes | **In part.** Keyframe-group trimming, a 2-minute cut, a byte ceiling per hour with reports exempt, the cost stated (3.1, 10). Chunked spooling and per-group fights are not built (11) |
| 8 | A dead player cannot report its killer; the zone wrote before the hub's limits applied; the report named a replay that did not exist yet | **Accepted.** Leavers stay reportable for two minutes; the hub opens the report first; the replay attaches later (5) |
| 9 | Reputation farming with one-copper carries between alts; a dismissed report punished good faith | **Accepted.** Once per buyer account per week; three verdicts, `not_proven` costs nothing (6) |
| 10 | Automatic tier loss contradicts "people decide"; −1 lived in both tier and bans; upload trust was implied | **Accepted.** Only 0 → 1 is automatic; bans live in `bans` only (6) |
| 11 | A ban costs little: assets move to an alt, stalls keep selling, `Claim` did not check | **In part.** `Claim`, `Enter` and handoffs check; stalls close. No asset freeze (11) |
| 12 | MAD of zero, correlated signals, raw rates at n = 40, weekly dilution, an undefined laser denominator, duplicate flags, no idempotency | **Accepted.** A floor on the deviation; Wilson lower bounds; rules on one session's report as well as the week; long shots as the laser's denominator; one flag per rule and week; a nonce on `ZoneAim` (4.4) |
| 13 | A snap measured per frame penalises 30 fps clients (two frames share a view) and third-person re-aim jumps | **Accepted.** The turn across 4 ticks (4.1); tested with a flick spread over two frames |
| 14 | The projectile leaves the muzzle, not the eye: 0.43° of parallax at 600 u, more than the laser threshold | **Accepted.** The shot event carries its origin (4.1) |
| 15 | GDPR and DSA: basis, retention per table, access, purpose, statement of reasons, reporter confidentiality, a "speech" reason without evidence | **Accepted as text** (9, 3.3, 6); the "speech" reason is gone. The notice, the assessment and a self-service export are the operator's to do before launch |
| 16 | Moderator audit: reads were not logged, one report could be decided twice, a moderator could judge its own case | **Accepted** (7, 5) |
| 17 | Shutdown lost the open fight | **Accepted.** Flushed and uploaded at stop |
| 18 | "Own eyes" misleads: the third-person camera and the sent set are not in a replay | **Accepted as a stated limit** (3.4) |
| 19 | The analysis must be one shared crate; the protocol version would orphan replays at the next bump | **Accepted.** `gm-replay`; the file has its own codec version (3.2) |

Rejected: recording executed frames between ticks (finding 5 in part): the statistics are
defined on ticks instead, which is what a replay holds and what a reviewer can step through.

### 12.2 Code review (after coding)

Fourteen findings by an independent reviewer who read the diff and ran nothing; thirteen
fixed, one in part.

| # | Finding | Verdict |
|---|---|---|
| 1 | Aim numbers were taken and reported only on a plain leave: a handoff, an expired ghost and a rejoin dropped them (hopping zones before logging out avoided the weekly row), and a traveller could not be reported | **Fixed.** One `depart` on every way a body leaves (4.3). Tested: a lock that travels on to a zone that records nothing is in the hub's aim report with the arena's numbers |
| 2 | The claimed view tick helps a liar: claim a stale view and the analysis judges good aim against the wrong world; claim a fresh one and reactions look slow | **Fixed.** Shots are judged against both views and the better fit counts, with `stale_views` counted (4.1); the view for reactions is bounded by measured latency and the protocol's interpolation delay (4.2). Tested (`a_view_claimed_stale_does_not_hide_a_lock`). Principle 1 corrected |
| 3 | A stopping zone awaited an hour of upload retries; files still being written at the stop were neither logged nor sent; nothing re-sent leftovers | **Fixed.** One try of five seconds at the stop; writers are awaited and drained; a zone sends what an earlier one left; the header names the reports (3.3) |
| 4 | Two reports in the same keyframe group wrote the same file name at once | **Fixed.** One open report file that reports share; a sequence number in the name; written under another name and renamed (3.1, 3.2) |
| 5 | Nothing ever closed a flag, so flagged replays were kept for ever | **Fixed.** A flag closes by itself after 90 days (3.3) |
| 6 | Reports were unbounded: a ring copy and a file each, exempt from the hour's bytes; the rate limit was spent before the target was checked; the list of leavers grew without reports | **Fixed.** Frames are shared, not copied; one report file at a time; an hour's ceiling for reports; the target is checked first; the list is trimmed as it grows (3.1, 5). **Not done:** a trust tier or play time required to report (the hub's five open reports per account stand) |
| 7 | A week's numbers could be lost: a layout change decoded to nothing and overwrote the row; two first reports of a week raced; the team-kill cap raced | **Fixed.** A layout number on the row; a row that claims the layout and does not decode is an error, not a zero; one report of an account at a time (the account's row is the lock) |
| 8 | The gate: "recomputes" compared the file with itself; any rule counted as flagged; a failed parse skipped the byte budget; checks without a failing branch; a fixed sleep; the cut never exercised | **Fixed.** A recorder test compares live numbers with the file's and another exercises the cut; a lock must break `lock` and a flick `flick`; every check fails loudly; the gate waits for the fight's file |
| 9 | The sight sweep did quadratic work before any cheap test | **Fixed** before the review came in (12.3): a pending list, the four nearest on screen |
| 10 | `outlier` used raw rates with no sample floor: one lock in three shots was z = 16 | **Fixed.** Wilson lower bounds and the rules' minimum samples (4.4) |
| 11 | Promotion to tier 1 ran only when aim numbers arrived: an account that never shoots was locked out of trust-gated zones | **Fixed.** At the leaving save and at the gated door (6) |
| 12 | A small hostile file could expand to hundreds of gigabytes in a reader; the recorder could write frames its own reader refuses | **Fixed.** Frame and entity-record limits in the reader; a frame is cut at what the codec decodes (3.1, 3.2) |
| 13 | The replay store wrote the final path before the row existed; a retried upload made a second row | **Fixed.** Written under another name and renamed; orphans swept; one row per hash (3.3) |
| 14 | Smaller: a ghost was described every tick; tick 0 as a delta's baseline; the cut tick's damage went to the wrong file; listing reports was not logged; an error after a committed ban; a week's flag carried the session's line; a dead clause; a login racing a ban | **Fixed**, each. The login is checked again after its session exists |

In part: finding 6 (no trust requirement for reporting). Finding 2's remainder (a liar
gaining its interpolation delay on reactions) was closed afterwards by counting the
protocol's interpolation delay into the freshest view believed (4.2).

### 12.3 Found by running it

- **The clamp hid the cheat.** Judged against the zone's clamped view tick, an aim lock's
  error was 0.4° to 1.5° and its lock rate swung between 43% and 93% from run to run: the
  clamp was a tick or two off what the client saw, and a body crossing at 60°/s moves a
  degree a tick. Judged against the tick the client's frame claims, the same bots are
  steady (4.1). The simulation now keeps both.
- **Locks are a rate among shots at moving targets**, not among all shots: a third of an aim
  lock's shots are at bodies that hardly move, and those say nothing.
- **The honest bots were cheats.** A bot knows where every body the zone sends it is, pillar
  or not, and the first "hand" turned towards hidden bodies before they showed: the reaction
  rule flagged it, correctly. A hand now looks only at what it has a line to (and that is
  the ESP signal the rule exists for).
- **A hand that follows a moving wish smoothly is an aim lock with an offset.** The first
  hand model (a low-pass with a shake) would have tripped the steadiness rule the moment it
  aimed well; the hand now corrects its aim a handful of times a second and coasts between,
  as people do. The thresholds are only as good as that model (9).
- **The recorder re-summed its ring every tick** (28 µs with nobody in the zone); the sum is
  kept as frames come and go.
- **The sight sweep was quadratic.** Watching every hostile pair cost 3.5 ms a tick with
  200 players in one hall and broke the swarm budget. A viewer now watches the four nearest
  hostile bodies on its screen (4.2): 0.34 ms a tick with 200 players, beside 0.26 to
  0.29 ms for the recorder and the analysis.
- **Uneven delivery hid the lock.** One gate run in nine failed. Under threefold CPU
  oversubscription (the zone starved of frames on some 130 to 620 ticks in 90 s, against
  32 on a quiet machine) four of six lock programs in three runs escaped their rule: 12 of
  35, 11 of 24, 6 of 29 and 3 of 21 locks among shots at moving targets, with a median
  error of 0.6° to 0.9° against 0.3° to 0.45°. Two causes, found in this order. The
  analysis held a whole window of views against one view lag, the shot's, while frames
  that run late and in bursts were drawn in worlds of different ages: the recorder now
  writes each client's view lag as it changes, and every tick's view is held against its
  own world (4; `a_lock_whose_frames_arrive_unevenly_is_still_a_lock` shows the same play
  caught with the lags and missed without them). That alone did not turn the gate: the
  cheat model itself aimed worse under load, because it measured its target's velocity by
  counting its own frames, which is wrong whenever frames and snapshots do not come one
  for one. It now reads the velocity from the world it looks at, as a program reading the
  client's memory would. After both, under the same load (134 and 301 starved ticks):
  14 of 19, 20 of 28, 21 of 29 and 24 of 26, all four flagged, median error 0.3° to 0.45°.
- **The lock rate's threshold** went from 40% to 35% as a Wilson lower bound. Measured
  after all of the above over three gate runs on a quiet machine (48 bots): the lock
  programs' bound stood between 58% and 80%, every hand's at 6% or less; the flick
  programs' flick bound between 90% and 94%, every hand's at 0%. Over two runs under
  threefold load: 51% to 76% against 2% or less, and 76% to 91% against 0%. The margin is
  the models', not people's (9).
- A sword-and-bolt build fired thirty bolts in ninety seconds and was not flagged; the rules
  need shots (9).
- **A short fight is a thin sample** (found in Phase 10). The test of the way to the hub
  (`gm-server/tests/anticheat.rs`: five players, 45 seconds) asked that both lock programs
  be flagged. On a busy machine one program's own aim ran loose in two runs of nineteen
  (8 locks in 14 shots at moving targets: a bound of 33% against the rule's 35%); in twelve
  runs on a quiet machine, and eight at the commit before, never. The test now asks for
  what it is for (both programs' numbers in the hub's report, one of them flagged, no hand
  flagged); that every program is flagged stays the gate's claim, on sixteen players and
  ninety seconds.
