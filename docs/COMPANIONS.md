# Companions, command, creatures and trials

Status: v1, Phase 7 (implemented and measured, 2026-10-01). The contract for AI-driven bodies (hired companions and creatures),
the command stance and the tactical viewport, encounters, boss loot in the world and role
trials. PLAN.md 3.5, 4.1, 4.3, 5.2, 5.6 and 6 are binding: the tactical view never commands
humans, hired avatars get flat coin and never loot, runs with hired avatars have a loot ceiling,
drops are deterministic, a wiped party gets nothing, there are no trash mobs and no levels.
`gm-ai` (minds, navigation), `gm-core` (the sim, the ledger, trial verdicts) and `gm-server`
(who is driven by what) implement this document; when they disagree, the document wins.
Section 14 holds what was measured; section 16 the proposed numbers the director has not
confirmed and the review log.

## 1. Principles

1. **One kind of body.** A companion and a creature are the same simulated body as a player:
   the same hull, capsule, verbs, costs, cooldowns, damage pipeline, collision and friendly
   fire. What differs is the **driver**: a human's frames come from a client, a mind's frames
   come from `gm-ai`. A mind produces the same `Input` a client sends and nothing else; there
   is no AI-only code path in the simulation (VOCABULARY.md 1).
2. **A mind knows what a client in its place would be shown.** It perceives bodies by line of
   sight within a sight radius, respects Stealth, reads animation states (a windup is visible)
   and never reads another body's inputs, cooldowns or resources. It turns, reacts and aims
   with stated limits (section 2.3): a companion is a competent player, not an aimbot.
3. **Command has a price.** Orders are given from the command stance, in which the body does
   nothing else (PLAN.md 4.1: "body exposed while in the view"). What the stance buys is sight
   through the squad's eyes; what it costs is the commander's own sword arm.
4. **The dead wait.** In an encounter nobody respawns until the encounter ends. A wipe is
   therefore a state, not a feeling, and running back in is not a tactic.
5. **Roles are proven, not picked.** A role trial is an encounter measured through a lens
   (damage dealt, blows taken, healing done, orders given). A character passes by doing the
   job, with any build that can.
6. **Measured.** Minds have a time budget per tick, the nav grid a build-time budget, the
   tutorial a clear-time window; `scripts/check-dungeon.sh` gates them (section 14).

## 2. Bodies and drivers

### 2.1 Three drivers

| Driver | Frames from | Party | Respawn |
|---|---|---|---|
| Human | the client, through the frame ledger (PROTOCOL.md 4) | its own, or the one it joined | timed (3 s), or held by an encounter |
| Companion | a mind, one frame per server tick | its commander's | at its commander's side, 10 s after death; held by an encounter while it runs, then back 3 s after its release |
| Creature | a mind, one frame per server tick | none | by its encounter (section 9) |

A mind-driven body runs **exactly one frame per server tick**: no dejitter reserve, no burst,
no starvation. Its frame clock is its own counter, so cooldowns and scripts elapse as they do
for a human. Its swings are not rewound (`view_tick` = now): it has no latency to compensate.
A human therefore meets a mind's swing where the server has the human *now*, which is kinder
to the target than a human attacker's rewound swing (PROTOCOL.md 7.4); what latency still
costs the target is seeing the windup late, and content answers that with windups: a
creature's own melee and telegraphs take at least 400 ms to land.

### 2.2 What a mind perceives

- **Bodies**: every living body within its sight radius (companions 1,400 u; creatures per
  definition) whose capsule centre has a clear point trace from the mind's eyes, and that is
  not stealthed beyond its Stealth radius (MATRIX.md 8). A body that damaged one of a party is
  that party's foe for 10 s wherever it stands (it was felt, not seen). The trace is run lazily: candidates are
  ranked by what the mind wants (nearest, most hurt, highest threat) and tested in that order
  until one is seen, so a decision costs a few traces, not one per body in range.
- Per body: position, velocity, facing, animation state, frame, armour class, aspects, the
  status mask, team and party. The health of its own party and of creatures (what the wire
  shows, section 13); of nobody else.
- **Areas**: the circles on the floor, each with its radius, its owner and whether it harms
  (a telegraph is there to be read, and the wire says which kind it is, section 13). A mind
  steps out of a harmful one it stands in, after its reaction time.
- **The map**: collision traces and the nav grid (section 7).
- Its own mover, sheet and health, as a client knows its own.

### 2.3 Limits

| Limit | Value | Why |
|---|---|---|
| Reaction | 200 ms between perceiving a windup or a telegraph and answering it | human reaction; a 150 ms parry window is hard for a mind too |
| Turn rate | 720°/s | no instant snap-aim |
| Aim error | 1.5° standard deviation on every projectile, on top of the weapon's spread | leading a target is the hard part (PLAN.md 4.2) |
| Decision rate | targets re-chosen every 8 ticks; line of sight to one body looked at every 4; a path planned again when its goal has moved 64 u or the body is stuck; steering every tick | the budget of section 14 |
| Friendly fire | a mind does not release a projectile or a swing while an ally's capsule is in the line or the arc | friendly fire is always on; a companion that shoots its tank is worse than none |

The limits are the same for companions and creatures. Content tunes creatures by their stat
block and kit, never by cheating minds.

## 3. Parties, squads and hires

### 3.1 Party

Every body carries a `party` number. A human's party is itself until it joins another
(since Phase 12 the hub's parties of people, PARTY.md: every member's body in a zone
carries one number, and it changes only outside a fight). A companion carries its
commander's party. Creatures carry none
(0). Party is for the loot ledger, squad sight and the health bars a client is sent; **damage
never reads it** (PLAN.md 4.4).

Zones are either **team zones** (the arena: teams 1 and 2, as before) or **wild zones** (the
dungeon): every human and companion is team 1, every creature team 3 (`WILD`). Team is the
colour of the pip above a head and what a mind treats as hostile by default; it is not a rule.
A zone is wild when its map posts creatures (`gm_creature`), or when it is started with
`--wild`. A dungeon is also started with `--arrive-at-entry`: every arrival starts at the
map's spawns, wherever the character logged out (otherwise a player logs out past the gate
and comes back at the boss's feet with a fresh squad; the online gate found exactly that).

### 3.2 Squad

A character commands a **squad** of at most **3** companions; each ability in the build with
a `squad` bonus adds its slots, up to **5** (PLAN.md 4.3: "leadership build commanding 3–5").
v1 content has one: `war_standard` (active, 10 points, +2 slots; its script is a rally: Fortify
15% for 8 s on every body within 256 u, enemies included). A squad is filled in this order:

1. **Hired avatars** (ECONOMY.md 11): the character's active hires, oldest first.
2. **Recruits**: a zone may lend named preset builds (`gm-server --recruits a,b,c`) to fill
   the slots hires left empty. A recruit exists only in that zone, costs nothing and wears the
   frame's mannequin. Only tutorial zones lend recruits: a character with no coin must be able
   to play the tutorial, and a zone without a hub must be able to run it.

Companions appear beside their commander when it enters a zone that allows squads
(`--squads`; the town does not) and leave with it. A commander who logs out, travels or
disconnects takes the squad along: nothing fights on for an absent player.

### 3.3 Hires become companions

- A hired avatar is a **copy** of the listed character (its saved build, its name, its avatar
  model while that is active) driven by a mind. The character itself stays offline; the same
  avatar may serve several hirers at once, each paying (the 30% burn and the 70% to the owner
  are per hire).
- A hire is **active** from `Hire` for 12 h, or until its owner enters a zone with that
  character ("takes it back", no refund, ECONOMY.md 11), or until the hirer dismisses it.
  A character may hold at most its squad capacity in active hires, and at most one copy of
  any one avatar; `Hire` beyond that is refused before any coin moves. A hire made while
  the hirer is in a zone joins the squad at its next entry. A companion that is on the ledger of an engaged encounter
  stays until that encounter ends, whatever the clock or its owner says: a hire never
  vanishes in the middle of a fight (an encounter lasts 15 minutes at most, section 9).
- The hub tells the zone at the claim (`Claimed.squad`: hire id, avatar, name, build, model,
  expiry); the zone spawns the companions. The hub tells the hirer's zone when a hire ends
  early (`HubNotice::HireEnded`); the zone checks expiry itself once a second.
- A companion never receives loot or coin from a zone (section 10). Its owner was paid at the
  hire.

## 4. Roles

A companion's role is read from its build, so a hirer sees it in the tavern list and a
commander on the squad panel:

| Role | The build has | What the mind does when left to itself (`Follow`) |
|---|---|---|
| **Heal** | an ability that puts Regen on others | stays behind the party in line of sight, keeps the most hurt ally above 70%, steps out of melee and telegraphs, attacks only when nobody needs it |
| **Tank** | a Block that stops projectiles, or a Colossus in mail or plate | goes first, stands between the threat and the party, faces the threat away from the others, blocks windups, holds ground |
| **Scout** | an Infiltrator frame or a Stealth ability | keeps to the flank, breaks contact when attacked (blink, stealth), pokes at range; the unit to send ahead with `MoveTo` |
| **Dps** | anything else | melee kits attack from outside the target's guard arc and its swing arc; ranged kits keep their distance and a clear line |

The first match from the top decides. Every role uses its whole kit by the shape of each
ability (a burst at the feet when the target is inside it, a placed area on the target, a
gap closer to close or to flee, a buff when a fight starts), steps out of hostile areas
before they pulse, and guards when a windup it can see will reach it (after the reaction
time). No role knows a script it was not shown.

What the offline fights taught the minds, and what a commander can count on:

- **The heavy goes in first.** While a heavy ally (a colossus in mail or plate) is on its
  way to a target that nobody of the party is fighting at arm's length yet, the others stop
  where the target cannot see them (how far a creature sees is in the content; 800 u is
  assumed of anybody else) and open only once the heavy is on it, or after 8 s.
- **A shield-bearer swings into openings**: when its target faces away, recovers, casts or
  staggers. With a blow coming it holds its guard while it can pay for it, and steps out
  when it cannot.
- **A healer aims.** It mends the most hurt ally below 80% that it can reach, answers its
  own wounds with a circle at its own feet (a dart cannot hit its shooter), and one whose
  kit cannot hurt at range keeps 380 u behind the fight rather than wading in with a staff.
- **Nobody shoots through a friend**: the line is checked wider with distance and a little
  past the target; a shot already running whose line fills is thrown over everyone's head.
- **A body in the way is walked round.** The nav grid knows walls, not who stands in a
  doorway; a mind passes on the side the other body is not on, or the other when a wall is
  there.

## 5. The command stance and orders

### 5.1 The stance

Button bit 11, `command`, held in an input frame puts the body in the **command stance**:

- it begins on the first frame with the bit set while no script runs and the body is not
  staggered; it lasts while the bit is held and for **400 ms** after it is released (standing
  up);
- in the stance the body does not move by itself (no wish direction, no jump), activates
  nothing and cannot guard; it can turn. Knockback, statuses and damage work as always;
- its animation state is `command` (12): everyone sees a commander is at it.

The stance is part of the predicted mover like the guard (`step_mover` runs it on both
sides), so a client that sets the bit and moves anyway is simply corrected; own-entity flag
bit 7 (`commanding`) lets a client whose prediction disagrees (a stagger it had not seen)
adopt the server's state, as the guard flags do. It is the one place where the server knows
the viewport: sight through the squad is a grant, and the stance is its price
(VOCABULARY.md 5.7, 9).

What the stance grants (squad sight, orders) it grants **only while the bit is held**. The
400 ms of standing up are pure cost: tapping the button buys one snapshot of squad sight for
400 ms without a guard.

### 5.2 Squad sight

A client is always sent its own companions (like its own entity, by the distance bands of
PROTOCOL.md 5), wherever they are. **While its body is in the stance with the button held**,
it is also sent every entity in the PVS of each living companion's eyes, with the distance
band taken from the nearest squad member. Outside the stance it sees with its own eyes only.
Nothing is ever sent that no squad member could see (PLAN.md 8); the tactical viewport has no
minimap and no map-wide view.

### 5.3 Orders

```
enum Order { Follow, Hold, MoveTo([f32; 3]), Attack(u32) }
FromClient::Order { slots: u8, order: Order }   // bit i of `slots` = squad slot i
```

- Accepted only while the sender is in the stance with the button held, for its own
  companions, at most 8 messages a second. Anything else is answered `OrderRefused(reason)`.
- `Follow` (the default): stay with the commander, fight by role.
- `Hold`: stay within 96 u of where the companion stands now; fight what comes within reach
  or range; do not chase.
- `MoveTo(point)`: the point must be within 4,096 u of the commander and within 64 u of a
  standable cell that the nav grid can reach from where the companion stands (a point behind
  a wall is fine if there is a way round, and refused if there is none); the companion walks
  there like anyone would and holds. A commander may send a scout where nobody of the squad
  can see yet: that is what scouting is.
- `Attack(entity)`: the entity must be a living body the commander is being sent. The
  companion fights it until it dies or has not been seen for 5 s, then returns to the order
  it had before. `Attack` is how a commander focuses fire, and it names any body: orders are
  not limited to creatures.
- Orders persist across the commander's death and respawn; they end with the squad.
- The commander is told its squad (`Squad`, section 13) whenever a member or an order
  changes, also when an order ends by itself (its target fell, or was lost): a commander
  who is not told cannot order again, and half a fight then runs without orders (the
  simulated-network runs found this: 5 of 24 kills at 50–60% under orders before, 24 of 24
  at 100% after).

The intents are the vocabulary's (`MoveTo`, `Attack`, `Hold`, VOCABULARY.md 9); a mind turns
them into ordinary inputs. Nothing here can name a human's body as a recipient.

## 6. The tactical viewport (client)

**Removed 2026-10-06** at the director's decision (PLAN.md 4.3 [REVERSED]): "tactical" in
the pitch meant rock-paper-scissors combat, not a view from above. The client has no `Tab`
view, no squad selection and sends no `FromClient::Order`; companions follow their commander
and fight on their own (section 5, the `Follow` default). The command stance (button bit 11),
squad sight and the `Order`/`OrderRefused` messages remain on the wire for a later
point-and-order from the third person. The HUD keeps the squad panel (slot, name, role,
order, health), the bar of the creature being fought (the nearest that is hurt), and the
encounter, loot and trial messages.

## 7. Navigation

`gm_ai::nav::NavGrid` is built once per map from collision alone (no authoring):

- Cells of 32 u. From every spawn, creature post and stall tile, a flood fill walks the eight
  neighbours with the player hull exactly as movement would: up a step (18 u), across, down
  onto the ground. A neighbour is linked when the hull gets there and the ground is walkable
  (normal z ≥ 0.7); a drop of more than a step and at most 256 u is a one-way link.
  Two floors over the same cell are two nodes.
- Paths are A* over the links (octile heuristic, drops cost extra). A path is followed by
  steering at the farthest of the next six nodes that can be walked to in a straight line
  (a hull trace, and the ground sampled every 32 u so a straight line never crosses a pit).
- A mind that has not come 8 u closer in 750 ms sidesteps and asks for a new path. Other
  bodies are not in the grid: a body within 76 u on the way is walked round (section 4).
  (Before that rule a commander kneeling in the inside lane of a corridor's corner held
  its own tank there for a whole fight.)

## 8. Creatures

### 8.1 Definition (content)

`assets/content/creatures.toml`; compiled into the content pack and sent to clients with it
(a client shows the name and scales the health bar):

```
[[creature]]
key, name
frame, armour, aspects, attributes      # as a build: the body is read by the same rules
health                                   # overrides 500 + 40·CON; 1..=60,000
stagger_threshold                        # optional override
might = 1.0                              # multiplies its blows and spells (MATRIX.md v2: 1 = a player's)
primary, secondary, guard, actives       # abilities, slot-typed like a build's; a ranged
                                         # primary may sit in the secondary slot (the bow beside the blade)
sight, leash                             # units (a `still` one needs none)
boss = true|false
still = true|false                       # stands and does nothing: no mind (the training dummy)
npc = true|false                         # nothing hurts it; `E` speaks to it (the trainer, MATRIX.md 9.1)
respawn_s                                # 0 = never while the zone runs
[creature.loot]                          # bosses only
components, standard = [materials], top = [materials], coin
```

A creature's sheet is a build without a budget: attributes may leave 5..=25, nothing is
priced, and it may slot abilities marked `creature = true`, which no player build can.
(2026-10-06: players have seven times v1's health; the sentinel has 1,200 at a player's
blows, the Warden 15,000 at twice them, and the heals scaled with the health (Mend 100/s,
Sanctuary 60/s), so that a squad with a healer clears the tutorial in 175–200 s on the Warden
without a death and one without a healer never does: `gm-ai/tests/{dungeon,skirmish}.rs`.)
Everything else is the build rules (slot types, aspect gating, one ability per cooldown
group). Its frame is one of the four: the hitbox table has no fifth row yet, so the Warden
is a colossus among colossi and told apart by its name, its team pip and its health bar. (A
`Hull::Large` frame is a later phase: it needs a mannequin, a capsule and a doorway rule.)

### 8.2 The creature mind

- **Post**: a creature stands at its map entity (`gm_creature`: `origin`, `angle`,
  `creature`, `encounter`) until something hostile is perceived within `sight`, or hurts it.
- **Threat**: per attacker, the damage it dealt to the creature plus half the damage the
  creature's own blows lost to that body's block (a shield holds the eye), plus half the
  healing it did to anyone on the creature's threat table; all of it decays 5% per second.
  Threat is weighed by nearness: times 3 for a body within the creature's reach, falling to
  times 1 at 400 u. The target is the highest weighed threat; a new target must exceed the
  current one by 30% (10% if it is nearer). Nothing in the vocabulary taunts: a tank holds
  a creature by being in its face, absorbing its blows and hurting it, and a body that
  hurts it from afar must out-damage the one in its face three to one to turn it.
- **Fight**: as a Dps role with its own kit, at its own limits (2.3). An aimed area (the
  Warden's quake) goes to the farthest body it knows and sees: artillery is for whoever
  thinks distance is safety.
- **Leash**: a creature chases no farther than `leash` − 256 u from its post, so whoever
  its weapons reach stands inside the encounter (which counts presence to `leash` + 256 u);
  at the edge it stops chasing and uses what reaches. Whether a fight is over is the encounter's call
  (section 9), and a reset puts the creature back on its post at once: there is no walking
  home to be exploited.

## 9. Encounters

An encounter is the set of creatures that share an `encounter` name on a map. The zone runs
one state machine per encounter:

```
idle ──(a creature gets a target or is hurt)──▶ engaged ──(every creature dead)──▶ cleared
  ▲                                               │                                  │
  └──────────(reset: see below)───────────────────┘        (respawn_s later) ────────┘
```

- **Engaged** opens the **ledger**: for every body, the damage it dealt to each of the
  encounter's creatures, the blows those creatures aimed at it (melee and projectiles, before
  block), the damage it took from them, the healing it did (below), its deaths, and for a
  commander the damage its companions dealt to a creature while under its `Attack` order
  naming that creature. A **participant** is a body with any entry. All creatures of an
  encounter share what they perceive: pull one, fight all.
- **Healing** is credited to the healer only as far as it restores health a creature of the
  encounter took: per target, credited healing never exceeds the damage the creatures have
  dealt to that target. Topping up a full body, or a body that hurt itself, earns nothing.
- **The dead wait**: a participant that dies does not respawn while its party is still in
  the encounter. (A death elsewhere in the zone is timed as before.)
- **A party is out** when it has had no living participant within the leash (plus 256 u)
  of the encounter's anchor (its boss's post, or the middle of its posts) for 5 s (all
  dead, or gone). Then, at once: every living creature is
  healed by the damage that party dealt to it, the party's entries leave the ledger, and its
  dead are released to respawn. This is PLAN.md 5.2's stasis, per party: nobody finishes a
  boss another party softened, and a party that throws itself away undoes only its own work,
  never anyone else's. A released party that comes back starts from nothing.
- **Reset**: when the last party is out, or no creature has dealt or taken damage for 15 s,
  or the encounter has been engaged for **15 minutes** (nobody is held hostage by a kiter).
  Every creature is put back on its post at once and restored (alive, health, statuses,
  cooldowns, threat); the ledger is discarded; the dead are released.
- **Cleared**: loot (section 10), trial verdicts (section 11), the dead are released.
  Creatures with `respawn_s` return to their posts that much later (not onto a body: a post
  that is stood on waits) and the encounter is idle again.

Clients are told `Encounter { name, state }` (engaged, reset, cleared with the time) so the
HUD can say so; only participants and bodies in sight of a creature of the encounter are told.
The outcomes of carry contracts (ECONOMY.md 8) hang on `cleared` and on a party going out;
reporting them to the hub comes with the contract board, not in this phase.

## 10. Loot in the world (ECONOMY.md 9 applied)

On `cleared`, for an encounter with a boss:

1. Parties are the ledger's participants grouped by `party`. A party is **present** when a
   member is alive within the boss's leash (plus 256 u) of its post at the kill.
2. A human member's contribution within its party is its own work plus the work of the
   companions it commands: damage dealt, healing credited and blows the creatures aimed at
   it (since Phase 12, PARTY.md 2: among people one holds the boss and one mends, and has
   done its part; before, damage alone). Among the parties only damage counts, as before:
   the floor and the shares of step 3 are by the damage each party dealt. A commander who
   fought from the stance is not a leech: its squad's work is its work. Companions are not
   members and receive nothing.
3. `gm_core::loot::split(components, parties)` decides who gets how many (10% party floor
   among the present, wiped parties nothing, minimum one, 40% member floor).
4. **The ceiling** (PLAN.md 5.6): a party that had any companion on the ledger draws from
   the creature's `standard` list; a party of humans only draws from `top`. Materials are
   taken from the list in order, round-robin over the recipients in split order: the same
   kill always drops the same things.
5. The zone reports the kill to the hub once (`ZoneEconOp::GrantKill`: the components per
   recipient and the coin, `coin` silver split evenly among the recipients, remainder to the
   first). The hub does all of it in **one transaction that begins by claiming the kill's
   reference** for that zone, so the zone repeats the report until it is answered (six
   times over eight seconds) and a repeat pays nothing. Full inventories overflow to the
   zone's ground. A recipient who is no longer playing in the zone when the report arrives
   forfeits: no coin is made for it and its components lie on the ground where the boss
   died. A human who left the zone before the kill is no member of its party any more: its
   share is split among those who stayed.
6. Each recipient is told `Loot { encounter, items, coin }`. A zone without a hub says the
   same and persists nothing.

Creatures that are not bosses drop nothing (PLAN.md 0: no junk loot).

## 11. Role trials (PLAN.md 3.5)

A trial is an encounter plus conditions, judged **per human participant** when the
encounter is cleared (`assets/content/trials.toml`, compiled into the pack):

```
[[trial]]
key, name
map, encounter
time_limit_s            # engaged → cleared
max_party_deaths        # deaths among the candidate and its companions
max_humans              # humans in the candidate's party (1 = solo or bot-assisted)
[trial.role]            # the lens; every share is per mille of the encounter's total
damage  = N             # the candidate's own damage ≥ N‰ of all damage dealt to the creatures
tank    = N             # ≥ N‰ of the blows the creatures aimed at anyone were aimed at it
healing = N             # ≥ N‰ of all credited healing was its own
command = N             # ≥ N‰ of its party's damage was dealt by its companions to a creature
                        #   they were under its Attack order for
```

The lenses count what cannot be padded. Blows are melee swings and projectiles, before the
block: standing in a telegraphed area is not tanking. Healing is credited only against damage
the creatures dealt (section 9). Command counts damage done *under an order on that target*,
not the number of orders: pressing `Follow` eight times proves nothing, focusing a squad's
fire does.

- The verdict is a pure function of the trial and the ledger (`gm_core::trial::judge`); it
  names the first condition that failed, and the client shows it. A pass comes with what
  the ledger said of the candidate (seconds, deaths, the four shares).
- A pass is reported to the hub by the zone that ran it (`HubRequest::Trial`, zone
  connections only, the character must be playing there) and stored once per character and
  trial, with the fastest time.
- **Gating**: a zone may register with `requires = [trials]` (`gm-server --requires a,b`);
  the hub refuses `Enter` and `Handoff` into it (`Locked`, naming the trials) for a
  character that has passed none of them. The hub accepts a pass only from the zone the
  character is playing in, and only for a trial of that zone's map.

v1 ships four trials on the Warden (section 12), one per role; passing any of them is what
"cleared the tutorial" means. All four are for one human (`max_humans = 1`): since Phase
12 people can be in a party, and a party of people that pulls an encounter with trials is
told so at the pull (`a party of people passes no trial: those are for one player and a
squad`, PARTY.md 2) rather than after a fight that could not count.

| Trial | Lens | Conditions |
|---|---|---|
| `warden_leader` | command 500‰ | 300 s, at most 1 party death, 1 human |
| `warden_vanguard` | tank 500‰ | 300 s, at most 1 party death, 1 human |
| `warden_striker` | damage 350‰ | 300 s, at most 1 party death, 1 human |
| `warden_mender` | healing 500‰ | 300 s, no party death, 1 human |

## 12. The tutorial dungeon

`assets/maps/src/dungeon.map`, generated by `gm-tools map gen-dungeon`: an entry hall, a
passage with two turns (the PVS must cut), the gate room, a short stair, the Warden's hall
with four pillars and a dais (2,677 nav nodes; the walk from the entry to the Warden is
3,928 u; the entry hall does not see the Warden's hall). Run as a wild zone at 64 Hz with
`--squads --recruits ironclad,mender,frostweaver --arrive-at-entry`.

- **The gate** (encounter `gate`): two `sentinel`s (striker, mail, Water; sword, crossbow,
  parry; 420 health). No loot. They teach that orders matter: with the tank ordered onto one
  sentinel and the rest onto the other, the reference squad takes the gate at the first
  attempt in 23 of 24 offline runs (mean 14 s); with everybody on one target, in 15 of 24.
  (Water is weak into their Water: the frostweaver that melts the Warden scratches them.)
  They respawn after 600 s, so a party's first attempts at the Warden are not taken in the
  back by them.
- **The Warden** (encounter `warden`, boss): colossus, plate, Ground; 7,500 health, stagger
  threshold 400. Slash is halved by its plate and Electric by its ground; Blunt, Water and
  Grass are what a party should bring, which is the matrix lesson of the tutorial.
  - `maul` (creature): a 120° frontal arc, 96 u reach, 550 ms windup, 55 blunt through magic
    shields, heavy knockback, one every 2 s; cleaves everything in front. Stand behind it,
    or block it.
  - `quake` (creature): after a 300 ms cast, a circle of 150 u appears under whatever it aims
    at within 700 u and breaks 1,300 ms later for 70 ground; the circle is an area entity, so
    everyone sees it. Walk out.
  - `stone_throw` and `stomp` from the player content, for whoever stands far or crowds it.
  - Loot: 3 components; `standard` core/iron, frame/ash, catalyst/basalt; `top`
    core/dragonbone, shard/boss_scale, catalyst/basalt; 30 silver. Respawns after 120 s.

Two abilities and one build join the player content for the healer the dungeon needs, and
one for the leader (MATRIX.md 10 bands):

- `mend` (secondary, 4): a dart that deals nothing and puts Regen 20/s for 3 s on whoever it
  hits, friend or foe. No homing: a healer aims.
- `sanctuary` (active, 10): a circle of 140 u placed where the caster aims (within 500 u)
  for 6 s: Regen 12/s on every body in it.
- `mender` preset: caster, cloth, Electric; staff, mend, brace; sanctuary, haste, thunderclap.
- `captain` preset: striker, mail, Water; sword, crossbow, parry; war_standard, dash (cut
  2026-10-09, MATRIX.md 15: the war standard is an active any build slots).

Two vocabulary rules make them possible (VOCABULARY.md 4, MATRIX.md 7):

- **`Origin::Aim { range }`** replaces the placeholder `Origin::Target`: the first body or
  world surface along the actor's view ray within `range`, dropped to the ground (under a
  body: its feet; with nothing in the way: the point at `range`). Current positions, no
  rewind: what is placed is a spot on the floor. Nothing homes: the area stays where it was
  put. Only an `AreaEffect` step may use it.
- **A packet of amount 0 is not an attack**: it deals no damage, no knockback, no stagger,
  is not blocked, parried or evaded and interrupts nothing; only its triggers land.

## 13. Wire and hub changes

Protocol version byte **3** (PROTOCOL.md 14):

- Input: button bit 11 is `command`.
- Snapshot: `health` is sent for the own entity, for the viewer's party and for creatures;
  animation state 12 is `command`; own-entity flag bit 7 is `commanding`; an area's spawn
  record carries one more bit, `harmful` (it deals damage or puts a status on who stands in
  it that is not a boon: what its look says). A commander's own companions are always sent;
  in the stance, squad sight (5.2).
- Control: `Order`, `OrderRefused`, `Squad(Vec<SquadEntry>)` (to the commander, on change:
  id, name, role, order, maximum health, recruit or hire), `Encounter`, `Loot`, `Trial`;
  `PlayerEntry` and `PlayerInfo` carry `kind`: human, companion of an owner, or creature of a
  definition.
- Content pack: `creatures`, `trials`; abilities carry `squad` and `creature`.

Hub v1.3 (HUB.md 3):

- `ZoneHello.requires`; `Enter`/`Handoff` answer `Locked(trial names)`.
- `Claimed.squad` (active hires, at most the capacity of the claimed build: hire id,
  avatar, name, build, model while active, expiry); `HubNotice::HireEnded { hirer, hire }`.
- `HubRequest::Trial { character, trial, secs }` from a zone; `HubRequest::Trials { session,
  character }` answers what a character has passed, with its best times.
- `EconOp::Squad`, `EconOp::Dismiss { hire }`; `EconReply::Tavern` entries carry the name and
  the build (the tavern list shows roles); `Hire` enforces the capacity.
- Migration 0005: `hires.ended`, `trials (character_id, trial, passed_at, zone, secs)`.

## 14. Budgets, acceptance and what was measured (PLAN.md 11.8 Phase 7)

`budgets.toml [companions]`, gated by `scripts/check-dungeon.sh` on the reference machine
(Ryzen 7 4800U; bots on the same machine):

| Budget | Value | Measured 2026-10-01 |
|---|---|---|
| Mind time, mean per mind per tick | ≤ 25 µs | 3.9–6.5 µs with 51 minds; 6.7–14 µs in a zone of one squad (cold caches between ticks); 0.35 µs back to back offline |
| Nav grid build, per map | ≤ 500 ms | dungeon 2,677 nodes in 5.6–14 ms; arena 4,586 in 14 ms; town 3,120 in 7 ms |
| Zone tick p99 with 16 humans, 48 companions and the dungeon's creatures | ≤ 4 ms | 1.8–2.4 ms (mean 1.2–1.8 ms), no overrun |
| Zone → client | under `[net] max_bytes_per_player_s` (30,720) | 4.7–5.1 kB/s for one squad in the Warden fight; 17.9 kB/s mean, 19.3 kB/s worst with 67 bodies in the dungeon |
| The Warden, engaged to dead, reference squad | 45–300 s | offline 94–126 s over 8 seeds; over the protocol 82–130 s over 24 runs; through the hub 97–215 s over 5 runs (the slow ones are fights in which the damage dealer fell early: a tank and a healer grind the Warden down, and one offline run perturbed by a code change took 488 s that way; the trial's 300 s then say no) |
| Unexplained corrections of the human | under 1 per 10 s (PROTOCOL.md 9) | 0 in every run but one (1 in 60 s for one of 16 leaders) |
| Client in the tactical viewport | `[client]` budgets | 5,400–7,300 fps headless at 1280×720 on the reference iGPU, 120 MB peak RSS, 2 draw calls without bodies |

**Acceptance**: one player and three hired avatars clear the tutorial dungeon. Four steps,
all green:

1. *Offline* (`crates/gm-ai/tests/dungeon.rs`, no network, 0.7 s for 8 seeds in release): a
   body driven by the raider brain with the reference squad (ironclad, mender, frostweaver)
   on the real map and content clears the gate and the Warden on **8 of 8 seeds** in
   141–270 s, one reset in all (a friendly shard hit the tank at 89 health), 0–3 party
   deaths; the drop is `core/iron, frame/ash, catalyst/basalt` and 30 silver (copper, before Phase 14) to the one
   human; `warden_leader` passes with 100% of the damage under orders and the other three
   trials fail as they must for a leader who stood back. Without a healer, and with three
   blades, the Warden is not beaten in ten minutes on any of 3 seeds each (8–28 resets).
   On open ground (`tests/skirmish.rs`) a leader who fights beside the squad and a squad
   round an infiltrator also clear.
2. *Over the protocol* (`crates/gm-server/tests/dungeon.rs`, simulated network, 150 ms round
   trip, 3% loss): a headless client that knows only its snapshots and control messages
   walks in, takes the stance, orders its recruits and clears: **24 of 24 runs** in
   121–229 s, the leader's trial passed in all, 5.3 kB/s down. The runs are not bit-for-bit
   repeatable (QUIC draws its own random numbers), which is why 24 were run.
3. *Through the hub* (`crates/gm-server/tests/companions.rs` with `GM_TEST_DATABASE_URL`,
   real time, about three minutes): three accounts list an ironclad, a mender and a
   frostweaver at 100 silver; the leader finds the gated zone `Locked`, hires the three (300
   silver leave it, 90 burn, 70 reach each owner), enters with them (the squad is the three
   hires by name; the zone's recruits get no slot) and clears in 155 s; the database holds
   the three components and 30 silver for the leader, nothing for the avatars, the trial
   with its time, and a sound ledger; the gated zone lets it in; an owner who then plays
   its avatar ends that hire without a refund. `scripts/check-dungeon.sh --online` does the
   same with the real binaries and no help from a test: a first clear with recruits pays
   for the three hires of the second.
4. *Seen*: the windowed client on a virtual display with a software adapter, driven by
   real key and mouse events: Tab into the tactical viewport, a companion selected with a
   key and sent to a point with a right click, the squad sent into the dark of the gate
   room and the room appearing as they see it, two right clicks on the sentinels (the tank
   on one, the rest on the other), the gate cleared in 14 s. The HUD showed the squad, the
   orders, the sentinel's bar, the mender's sanctuary in green and the reset and clear
   messages.

## 15. Deliberately absent

No taunt, no threat meter on the wire, no enrage timer (PLAN.md 1.2), no boss immunity
phases, no scripted boss choreography outside its kit, no homing heals, no resurrection, no
instanced copies of a dungeon (a zone is a process; two parties in
the same dungeon contest the same Warden, and the split of section 10 settles it), no
companion in the town, no orders outside the stance, no order that names a human, no map
view, no creature larger than a colossus.

## 16. Proposed numbers and review log

**Proposed (director to confirm):** squad of 3, 5 with `war_standard` at 10 points; the
stance's 400 ms; the mind limits of 2.3; sight 1,400 u; the threat rule (half for blocked
blows and for healing, 5%/s decay, 30%/10% to switch, nearness times 3 in reach); the reset
after 15 s without damage; recruits in the tutorial zone only; the four Warden trials and
their thresholds; the Warden's 7,500 health, its kit and one maul every 2 s; the sentinels'
420 health; respawns of 120 s (the Warden) and 600 s (the gate); creature health on the wire
(a boss bar; the alternative is to show creatures' health to nobody); `Attack` orders naming
any body; the 5 s before a party is out and the 15 minute cap on an encounter; one copy of
an avatar per squad; dungeons put every arrival at their entry; whether an area harms is on
the wire.

**Numbers the fights set, not this document.** The first draft's Warden (9,000 health, then
14,000, a maul every 1.4 s) could not be sustained by the reference squad: a shield spends
12 stamina a block against about 4 regained between blows. At 7,500 health and a maul every
2 s the squad wins on every seed and a squad without a healer or a tank never does. The
sentinels kept their 420 health: what lost the gate was not their numbers but the squad's
opening (the healer or the damage dealer walked into both crossbows first) and a tank stuck
behind its own kneeling commander; with the heavy going in first, the others waiting out of
sight and bodies walked round, the gate went from 16 of 24 clean attempts to 23 of 24.

**Still open, found by this phase.** How often a boss may be farmed (the Warden respawns
120 s after it falls; PLAN.md 6 keeps a daily cap only "as a bot-farm brake" and asks to
revisit it after load testing) is not decided here. Party invitations between humans are not
built: the ledger, the split and the trials take any number of humans, but a party is one
human and its squad until the HUD can invite.

**One consequence of PLAN.md 5.2 the director should know.** The plan says the boss resets
when the aggro-holding party is wiped. Taken literally, a second party can ruin a first
party's attempt by stealing the top of the threat table and dying on purpose. Section 9
keeps the plan's intent (nobody finishes what a wiped party softened; wiped = 0) without that
lever: a party that goes out takes its own damage with it and nothing else.

**2026-10-01, v1 draft reviewed by Gemini 3.1 Pro** (independent review before
implementation; verdicts are ours):
- Accepted: the dead of one party could be held hostage by a kiter of another (the dead now
  wait for their own party only, and an encounter ends after 15 minutes whatever happens);
  squad sight lasted through the 400 ms of standing up, so a tap bought sight for free (the
  grants now need the button held); a `MoveTo` to a point with no way to it was undefined
  (refused when the nav grid has no path); a hire could expire or be taken back in the middle
  of a boss fight (deferred to the end of the encounter); `Origin::Aim` with nothing in the way
  was undefined (the point at `range`, dropped); the tank lens could be padded by standing in
  telegraphs (blows only) and the command lens by spamming orders (now the damage done under
  an `Attack` order on that target, no order count); 9,000 health dies in about 36 s to the
  reference party by its arithmetic, under the 45 s floor (the number in section 12 is set by
  the offline acceptance run, not by this document).
- Corrected, not as proposed: "reset only when no party remains" would let a second party
  finish a boss the first one softened and died to, which PLAN.md 5.2 forbids; the reverse
  (the plan's literal rule) lets a second party reset the first one's attempt by dying.
  Section 9 now undoes exactly the damage of the party that went out.
- Accepted in substance: line-of-sight traces for every body in range were said to break the
  budget (63 traces per decision). At the measured 140 ns per point trace they would not, but
  there is no reason to pay for them: candidates are tested lazily in order of preference.
- Rejected: rewinding a mind's swings by the interpolation delay "so players are not hit
  after dodging on their screen". It is backwards: a rewound swing lands where the target
  *was*, which is exactly the hit after the dodge; resolving at the server's present is the
  kindest a server can be to a target (section 2.1 says so now, with the windup rule).

**2026-10-01, implementation reviewed by Gemini 3.1 Pro** (the contract, `gm-ai`, the ledger
and the trial code in full, and the diff of every file the phase changed; verdicts are ours):
- Accepted: a `MoveTo` to a place without a way ran a whole A* over the map for every
  addressed companion, eight times a second per client (the nav grid now numbers its
  strongly connected components when it is built and tabulates which leads to which: "is
  there a way" is two lookups, also for a mind that wants to go where it cannot, and a
  search is capped at 200,000 nodes on a map too broken up to tabulate); a human who left
  the zone before the kill was still split a share that then went to nobody (it is no
  member any more); a creature with a reach of 400 u would divide by zero in the threat
  weighting (clamped).
- Wrong as stated, and it found a real gap: "the second character's coin fails on the
  duplicate reference". There was no such constraint: the section above *claimed* that the
  kill's id stops a second payment and nothing enforced it, and a grant whose answer was
  lost was simply lost. A kill is now one `GrantKill` transaction that claims `(zone,
  reference)` first, and the zone repeats it until answered.
- Already covered by the code: the mover's own box for the "walk out of a body" rule is
  taken before every frame, not once per tick, on the server as on the client.
- Rejected: crediting healing only inside the healer's own party, against "farming credit"
  off a party that keeps leaving and coming back. The healing lens is a share of all
  credited healing, capped by what the creatures dealt: a sole healer already has all of
  it after one dart, and what makes the mender's trial hard is that nobody may die. Two
  friends are two parties until invitations exist, and healing a friend's tank is healing.

**Found by running it, not by reading it** (each has a test or a gate now):
- A commander who was not told that an order had ended by itself (5.3) led half its fights
  without orders: 5 of 24 simulated-network kills at 50–60% under orders.
- A client-side leader took "not shown to me" for "dead" and swapped the squad's targets
  mid-fight; and judged a post cleared from a room away, where an empty view proves nothing.
- Two bodies in one place held each other for good. A crowded spawn (ten spawn points, 64
  bodies arriving in one tick) put two leaders in each other. So had the swarm since
  Phase 4: a spawn point offered 9 places, a team of 100 in the arena has 8 points and so
  72 places, and a bot that found none was put inside a body. How many stood stuck was not
  counted; with the fix the same swarm lands 40% more hits (737–988 in a run against
  485–698 before). A body that begins a step inside another is no longer held by it
  (`Composite::own`, both sides of the wire) and a spawn point offers 25 places. The extra
  fighting cost the simulation a millisecond (2.7–3.3 to 3.8–4.1 ms with 200 bots, the gate
  at 7.2–7.9 of its 8 ms) until sweeps against bodies got a grid (`BodyGrid`, bit-identical
  to walking the list: 9,600 sweeps compared): 200 bots now take 5.3–6.1 ms a tick, the
  simulation 1.4–1.6 ms of it.
- The online gate's second run began at the Warden's feet, because the leader had logged
  out there (3.1, `--arrive-at-entry`).
- One swarm run of seventeen lost a bot in its first second (199 of 200 played); it did not
  happen again in sixteen more and its cause is not known.
