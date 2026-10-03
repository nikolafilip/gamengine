# People together: parties, channels, a trade, the tavern (v1)

Phase 12 (PLAN.md 11.8). Until now a party was one human and its squad: the ledger, the
split and the trials took any number of humans, and no human could join another. This is
the contract for how people join each other, speak to each other alone, hand each other
things, and hire each other's characters, and for the screens of all four.

Code follows this document; a change to either goes in one commit.

## 1. Principles

1. **A party is the hub's.** It holds from one zone to the next, and every zone gives the
   same answer about it. A zone without a hub has no parties of people.
2. **What happens to a body is asked of its zone** (ITEMS.md 1.5). Inviting, answering,
   leaving, removing and asking for a trade go client → zone → hub. The zone applies the
   hub's answer to the bodies it has; other zones are told; a claim reads what is true.
3. **Rules are immutable in combat** (PLAN.md 6). The hub's party may change at any time;
   the party a *body* fights under does not change while that body is in a fight, and no
   body is brought into a party's fight under the party's number. Both change when the
   fight is over.
4. **Nothing here is a new way to pester.** An invitation and a request to trade are
   bounded in number, lapse by themselves and are not shown when they come from somebody
   the player does not hear. A whisper and a party line are chat: the same limits, the
   same list of people not heard.
5. **A trade is the hub's** (ECONOMY.md 6), as it was. The zone only vouches that two
   people stood together and both asked.
6. **Nothing is shown that the hub or the zone did not say**, and what is picked is picked
   by what it is (ITEMS.md 6).

## 2. A party

- Two to **five** characters. A character is in one party at most.
- The one who invited first **leads**. The leader invites and removes; anybody leaves. When
  the leader leaves, or is out of the game at a change of the party or at a sweep, whoever
  of those in the game has been in the party longest leads. A party of one is no party:
  it is dissolved.
- **Away, then out.** A member that leaves the game is still of the party for **two
  minutes** (its row says `away`): a lost connection, a crashed zone or a full one do not
  cost anybody their party. The hub's sweep (every ten seconds) takes out whoever has been
  offline longer than that, whichever way it went offline.
- **In a zone** a party of the hub has one number for as long as the zone runs
  (`PARTY_BASE + n`, `PARTY_BASE = 0x8000_0000`; entity ids stay below it). A body in no
  party carries its own id, as before. A companion carries its commander's number.
- **In combat** (principle 3) the zone keeps, for each human, the hub's last word and the
  number its body carries, and makes the second the same as the first only when both of
  these hold:
  - the **body** (and its squad) is on no ledger of an engaged encounter; and
  - the **party it is to join** is on no ledger of an engaged encounter in this zone: a
    fight's roster is closed. Whoever joins a party that is in a fight fights as itself
    until that fight is over.

  So: somebody who is removed in the middle of a boss fight is paid as a member of the
  party it fought in; nobody is brought into a fight under the party's number, alive in
  the place of the dead or as a sixth; and a ledger never sees a body change sides. The
  party's list on the screen is the hub's word, at once; a line of the zone says when the
  two differ.
- **Nobody who left a fight comes back into it.** A character whose body left the zone
  while it was on the ledger of an engaged encounter is not let into that zone again while
  that encounter is still engaged ("the fight you left is not over"): a new body would be
  alive where the old one was dead and held, at full health where the old one was not. A
  second connection for a character whose body is in a fight here is turned away the
  same way ("your body here is still in a fight"), and the fight goes on with the body it
  has.
- What a party is for: the loot ledger and its split, squad sight, the health of its
  members on the wire and on the HUD, a channel of chat. Damage never reads it (PLAN.md
  4.4). Outside an encounter a party changes at once, and so does what reads it: whose
  kill is a team kill (ANTICHEAT.md 6) follows the party of the moment.
- **The split between people** (COMPANIONS.md 10, ECONOMY.md 9). A member's contribution
  was the damage it and its companions dealt, which is right for one human and a squad and
  wrong for five people: somebody who heals or holds the boss deals little, and the 40%
  floor would pay such a member nothing at every kill. **Within a party** a member's
  contribution is now **what it and its companions did**: damage dealt, healing credited,
  and blows taken (all three are points of health on the ledger already). **Among
  parties** nothing changed: a party's eligibility (the 10% floor) and its share are by
  the damage it dealt alone, so that a stranger who stands in the boss's cleave for a
  minute, or mends another party's tank, earns no part of that party's kill (the code
  review found that work alone would have given it one). Proposed; the director's call
  (11).
- **Trials** are for one player and a squad (COMPANIONS.md 11). A party of people is told
  so when it pulls a fight that has trials, not after the kill. The verdict itself is the
  ledger's, as before: it counts the humans of the candidate's party *on the ledger*, so a
  member who took no part in the fight at all (never hit, never struck, never mended) is
  not of it for the trial, and the one who fought alone passes.

## 3. The hub (v1.8)

### 3.1 What is kept

Migration 0010:

```
parties        (id, leader_id, seq)
party_members  (character_id primary key, party_id, joined)   -- joined: the seq it joined at
party_invites  (to_id, from_id, at, declined)  primary key (to_id, from_id)
characters.party_seq                                          -- its last change of party
characters.offline_since                                      -- set by a trigger when it goes offline
sequence party_seq
hires.build                                                   -- the build a hire was bought with (7)
hire_listings.build                                           -- the build a listing was made with (7)
trades: partial indexes on the open trades of either character
```

**One lock.** Every change of any party is made by one writer at a time: the hub holds a
lock of its own for it (changes are as rare as people make them, and those that wait hold
no connection to the database), and the transaction takes an advisory lock of the
database besides, for whoever else writes (a test, a tool). Within a change, the rows of
characters are taken in ascending order, as the economy takes them. A deadlock or a
serialization failure the database reports is answered `Busy`.

**Readings are numbered** (as gear's are, ITEMS.md 3.3). Every change draws one number
from `party_seq` while it holds the lock and writes it to the party's row and to the row
of every character that joined or left by it (`characters.party_seq`). What the hub says of a character's party is
`PartyReading { seq, party }`: the party's `seq` and state when it is in one, its own
`party_seq` and nothing when it is in none; it is read in one statement. The numbers are
in the order of the changes, so a zone that keeps, for each character, the reading with
the largest number holds what the hub holds, in whatever order claims, answers and notices
reach it (notices travel on streams of their own and may overtake each other).

### 3.2 What a zone asks (`HubRequest::ZoneParty(ZonePartyOp)`)

The character a request is made for must play in the asking zone (`Unauthorized`
otherwise), whatever the request.

| Request | Refused when | Answer |
|---|---|---|
| `Invite { from, to: String }` | nobody is called that, or that character is not in the game; it is `from` itself; `from` is in a party it does not lead; the party is full, or with the invitations `from` has waiting would be; the same invitation waits already (answered or not); five unanswered invitations wait for that character | `Invited { name }`; the other's zone is told `Invited { to, from: String }` |
| `Answer { character, from: String, join }` | no such invitation, or it is older than 60 s, or it was declined; (join:) the answerer is in a party; the inviter is not in the game, or is now in a party it does not lead; the party is full | `News(PartyNews)` on a join, `Declined` on a refusal (the inviter's zone is told `Declined`) |
| `Leave { character }` | it is in no party | `News(PartyNews)` |
| `Remove { leader, name: String }` | `leader` does not lead a party; nobody of that name is in it; it is the leader itself | `News(PartyNews)` |
| `Say { from, to: Party \| Whisper(String), text }` | `from` is in no party / nobody of that name is in the game; it is no line | `Said { to: String }`; the hearers' zones are told `Heard` |
| `Read { character }` | | `Reading(PartyReading)` |

- An invitation that was **declined stays** where it is until its 60 s are over: the same
  inviter cannot ask again at once. A declined one does not count among the five that
  may wait for a character.
- `PartyNews { seq, party: PartyState { id, leader, members }, left }`: the party after
  the change (no members: it was dissolved) and who is out of it by this change. Every
  zone where a member or a leaver plays, or is on its way to, is sent it as
  `HubNotice::Party`: the asking zone too (its answer may be late or lost; the notice is
  the same news with the same number). Where they are is read after the change is
  committed.
- `Claimed.party: PartyReading`, read after the character became the zone's (as gear).
- **Repair.** A notice can be lost (the hub gives one up after five seconds; a zone that
  cannot read one skips it). So the answer to every periodic `Save` carries the number of
  the character's present reading, and a zone that holds a smaller one asks (`Read`).
  Whatever was missed is mended within a save's period (30 s).

**The sweep** (every 10 s): a party whose leader is offline is led by the longest-standing
member in the game; members offline for more than two minutes (`gm-hub --party-away
SECS`; measured from `characters.offline_since`, which only going offline sets: a build
set or a model worn while offline is not coming back) are taken out, one change each,
with the rows of the whole party locked in ascending order (the order every lock on
several characters' rows takes) and the member looked at again under them (somebody who
came back meanwhile stays); invitations older than 60 s are deleted. An error in the
middle ends the sweep and what was changed before it is still told; the next sweep takes
the rest up.

### 3.3 Limits

A party request is a transaction or two, and the zone's gates (4) are in front of it. A
relayed line is one indexed read; the limits of chat are the zone's (5): the hub trusts
its zones for them (they hold its secret).

## 4. The zone and the wire (protocol v7)

**The control stream's two directions are two types** from this version: `FromClient` and
`FromZone` (they were one enum, `Control`). Each side carries the code to write one and
to read the other; the browser build lost 62,834 bytes by it (measured), which is the room
this phase's screens are built in. `Hello` is still number 0 of the client's messages and
`Reject` and `Kick` numbers 14 and 25 of the zone's, with the bytes they had: a client and
a zone of different versions still say what is wrong. What a version adds goes at the end
of its enum.

**A health that is no longer sent** (the body left the viewer's party) could not be unsaid
by a snapshot's delta: a reader carried forward the last one it was told. Such a record is
now written whole, as a new body's is (PROTOCOL.md 5): nothing before this phase ever
stopped sending a health.

New in v7:

| Message | Way | Answer |
|---|---|---|
| `PartyInvite { name }` | client → zone | a line of the zone |
| `PartyAnswer { from, join }` | client → zone | a line of the zone; `Party` |
| `PartyLeave`, `PartyRemove { name }` | client → zone | a line of the zone; `Party` |
| `PartySay(text)`, `Whisper { to, text }` | client → zone | `Heard` |
| `TradeAsk { with }` (a body here) | client → zone | a line of the zone; `TradeOpened` |
| `Party(names)` | zone → client | the hub's word on the client's party: its members' names, the leader's first; none: it is in none |
| `Invited { from }` | zone → client | |
| `Heard { channel, from, text }` | zone → client | `channel`: 1 party, 2 a whisper to the client, 3 the client's own whisper (`from` is whom it went to) |
| `TradeAsked { from }` (a body here) | zone → client | |
| `TradeOpened { trade, with }` | zone → client | the hub's trade, and the other's name |

- **A line of the zone** is `ChatFrom { from: 0, .. }`, as every refusal of the zone's is
  (CLIENT.md 5): the screens show the party from `Party`, never from an answer. The page
  of people shows the zone's last line too (it may lie over the chat's).
- What the zone says when something was done: `NAME was asked to join`; `you are in a
  party: A, B`; `you are out of the party`; `the party is no more`; `NAME is out of the
  party`; `NAME declined`; `you asked NAME to trade`; and, when a wait is over, `the fight
  is over: you are of the party now` or `... you are on your own now`.
- **Gates**: the party requests and `TradeAsk` have a gate of their own, the size of the
  stall's and of gear's (one at a time, one a second, per player; a refusal of the zone's
  own costs nothing), behind the connection's flood limit (PROTOCOL.md 18). The zone waits
  ten seconds for the hub, then says so; what the hub answers in the end is still applied
  (and frees no gate a second time). A name in a request is checked by its bytes before
  anything else: what a client sends may be as long as a frame.
- **An invitation for a character on its way here** is kept until its body is (a minute
  at most), and told then.
- **The zone's checks before it asks the hub**: the body is a human's and not a ghost. A
  name is one a character can have (CLIENT.md 7).
- **At a join** the body takes the larger of its claim's reading and what the zone already
  has for the character (news kept two minutes for a character without a body, as gear).
- **Refusals in words** (the hub's reason goes to the zone's log):

| Case | Words |
|---|---|
| nobody of that name / not in the game | nobody called NAME is in the game |
| inviting oneself | that is you |
| the inviter does not lead | only the party's leader invites |
| the party is full | the party is full |
| invited already | NAME has been asked already |
| too many invitations wait for that character | NAME has too many invitations waiting |
| no such invitation | that invitation is gone |
| the answerer is in a party | leave your party first |
| in no party | you are in no party |
| removing somebody who is not in it | nobody called NAME is in the party |
| a zone without a hub | this zone has no parties |
| a name no character can have | nobody can be called that |
| the asker is a ghost (on its way to another zone) | not now |
| a trade with a body that is not a person here | there is nobody there to trade with |
| a trade with somebody out of reach | walk up to them to trade |
| the hub would not open the trade | the trade did not open: WHY |
| the party is in a fight here, the joiner is not | your party is in a fight: you are of it when the fight is over |
| removed, or leaving, in the middle of a fight | the party changes for you when this fight is over |
| entering a zone whose fight one left | the fight you left here is not over: come back when it is |
| a party of people begins a fight that has trials | a party of people passes no trial: those are for one player and a squad |
| the hub is past its limit / silent / anything else | as ITEMS.md 5 |

## 5. Channels

- `/p TEXT` says a line to the party; `/w NAME TEXT` whispers to one character anywhere in
  the game; `/r TEXT` whispers to whoever whispered last. The name is **one word**: a
  name with a space in it is written without (`/w DeVil hello` finds `De Vil`: names are
  told apart by their letters, not by what is between them), and the page's Whisper
  button writes it so. A line without a command goes to
  the zone, as before. `/invite NAME` and `/leave` do what the page's buttons do.
- All three are **chat**: a line is checked as a line is (PROTOCOL.md 8), and it is taken
  from the same account's bucket, in the connection's own task, before the tick loop or
  the hub sees it.
- A party line and a whisper go through the hub, whoever hears them (one path, one test).
  The speaker hears its own party line when the hub has relayed it; a whisper is answered
  to the whisperer as `Heard` on channel 3.
- `/r` becomes `/w NAME ` on the line as soon as it is typed: whom the answer goes to is
  on the screen before it is sent, whatever whisper arrives meanwhile. Whom `/r` answers
  is forgotten with the character (a leave, another character).
- A whisper to a name nobody can have is answered `nobody can be called that` by the
  zone and costs nothing: a mistake of the hand, not a line too many.
- **Shown** as `[party] Ana: text`, `[whisper] Ana: text` and `[to Bojan] text`, each in a
  colour of its own. The bracket is what no name can begin with: somebody called "Ana
  whispers" says nothing aloud that looks like a whisper of Ana's (the first draft had
  `Ana whispers: text`). A name the player does not hear (`/ignore`) is not heard here
  either, and its invitations and requests to trade are not shown: an invitation from
  such a one is declined by the client, a request to trade lapses.
- Two lines of one speaker that the hub relays to another zone travel on streams of their
  own: they arrive in order nearly always, and nothing promises it.

## 6. A trade between two people

- **Asked standing together**: `TradeAsk { with }` names a body. The zone checks that both
  are humans, alive, no ghosts, and within `TRADE_REACH = 160` units along the ground
  (`96` up or down), and tells the other `TradeAsked`. A request lapses after 30 s; a
  player has one at a time. When the other asks back within that time, the zone asks the
  hub: `ZoneEconOp::TradeOpen { a, b }`. The hub checks that both play in that zone,
  calls off any trade either still had open (a character has **one open trade**), opens
  one, and the zone tells both `TradeOpened`.
- **The window is the hub's**: offers, coin, accept and cancel are the session's requests
  (`PlayerEcon::Trade…`), with ECONOMY.md 6's mutation lock: a change clears both accepts
  and restarts three seconds, and an accept names the version it saw.
- **Seen by asking**: while the window is up the client asks for the trade once a second,
  and at once after something the person did. This is the one list that moves by itself.
  What makes that safe is that the window says, for as long as it matters, what the person
  has **not agreed to**:
  - Everything in the other's offer that was not there when the person last accepted (or
    since the window opened, if they never did) is marked `new`; what was there then and
    is gone is still shown, struck, `taken back`. The coin is said on the offer's header
    line like any item (a row before Phase 14; since then the offers are grids of cells
    with the marks on them, LOOK.md 4), marked the same way. The marks stay until the
    person accepts again. They do not fade.
  - **Looked at.** The window also keeps the other's offer as it was when Accept last
    armed. What differs from *that* is marked `changed` rather than `new`, what was in it
    and is gone is struck, and the lines say the other changed the offer: the quick swap
    (a lesser sword put in the place of the one that was looked at, before anything was
    accepted) is told apart from an offer that merely grew. Once the changed offer has
    itself been looked at for three seconds, its rows are `new` again: not agreed to,
    and no longer a surprise.
  - **Accept** is on only when the hub's three seconds are over, *and* this client has
    shown this version of the offer for three seconds, *and* the view is fresh (the last
    answer is less than two seconds old). A version that was on the screen for an instant
    cannot be accepted, whatever the hub's clock says.
- **The hub's view** (`PlayerEcon::TradeView`) says the state (`open`, `committed`,
  `cancelled`), the version, the milliseconds until an accept is taken, the other's name,
  whether both still play in one zone, and both offers in the hub's words (ITEMS.md 3.2).
  An item that is worn cannot be offered. The marks are kept by the client: a window that
  is opened again has agreed to nothing, and marks everything.
- The marks are said in a word as well as in a colour (`new`, `changed`; `taken back`,
  struck): a colour alone is not seen by everybody.
- **At the accept that commits**, the hub checks once more that both characters play in
  one zone; if not, the trade is called off. The hub answers a stranger's accept
  `Forbidden` and changes nothing by it, and an accept of a trade that is over with the
  trade's state; the two checks come before anything else.
- The hub's view is three statements and no transaction (a window asks for it once a
  second): an offer that changes between them is seen with a version whose accept the hub
  refuses.
- **Ends**: committed ("the trade is done"); cancelled by either (`Cancel`, `Escape`, the
  window closed); called off by the hub: when either opens another trade, when either is
  claimed by a zone (it travelled, or left the game and came back), when an accept finds
  the two in different places, and when nothing in it has changed for ten minutes. No
  trade waits, accepted on one side, for a day when its two meet again.

## 7. The tavern

A page of the menu (ECONOMY.md 11 is the rule; the hub had the requests since Phase 5):

- **The list**: every character listed for hire whose owner is away: its name, what it is
  (frame, armour, and the role a mind will play it in), the price, how often it was hired
  in the window. **Hire** names the price it was shown, and the hub refuses when the price
  is another now (`EconOp::Hire { avatar, price }`).
- **What is hired is what was listed**: a listing keeps the build the character had when
  it was listed (`hire_listings.build`, migration 0010); that is what the tavern shows and
  what a hire buys and keeps (`hires.build`). An owner who makes something else of the
  character afterwards changes no listing and no hire: listing again lists the build of
  that moment. (The code review found that pinning the price alone let an offline owner
  swap the build between the listing and the hire.) A listing whose build the content no
  longer has is not shown and sells nothing: `that character is not for hire now: its
  owner must list it again`; the hub validates the build before any coin moves and before
  it says a word of it (the first build could have aborted the hub on a stale one).
- **In the hub's words**: what somebody is (`ironclad: colossus in plate`) and the role a
  mind will play it in (`tank`), which the hub reads from the build as the zone's minds do
  (`gm_ai::Role::of`). No client works either out.
- The requests (the players' messages v3): `Tavern`, `Hires`, `HireListed`, `Hire { avatar,
  price }`, `Dismiss { hire }`, `HireList { price }`, `HireUnlist`.
- The page says, before anybody pays, what ECONOMY.md 11 rules: the owner may take the
  character back at any time, and nothing is refunded.
- **Your hires**: the character's active hires and when each ends; **Dismiss**.
- **List this character**: a price (three fields, as a stall's); it serves while its owner
  is away. **Withdraw** takes it off the list.
- A hire takes effect where squads are let in, at the next zone the character enters.
- The tavern has no place in the town yet: it is a page, in any zone under a hub.

## 8. Screens

### 8.1 People (`P`, or the menu; script name `people`)

One list: the party first, then whoever asked something, then every other person in the
zone. Beside each name what is to be known of it: `leads`, `party`, `invites you`, `asks
to trade`, `away` (of the party, and not in this zone), `elsewhere` (asking from another
zone), `gone` (it left while the page was up). The buttons are what applies to the row
that is picked: **Invite**, **Join**, **Decline**, **Remove**, **Trade** (only in reach),
**Whisper** (opens the chat line at `/w NAME `), and always **Leave** (in a party),
**Tavern**, **Close**. A button that asks the zone is off for a second after any that did
(the zone takes one a second, whichever page or line it came from; the client's own
declining of an invitation from somebody not heard counts too). A row is picked by its
name; when it is gone, nothing is picked. An invitation stays on the page until the party
itself answers it (a join the zone refused is still there to answer); declining takes it
off at once. An invitation made again begins its minute anew. **The rows keep their places while the page is up**: whoever arrives is added at
the end, and the row of whoever left stays, empty of buttons, until the page is closed.
Nothing slides under the pointer.

An invitation or a request to trade is also a line of the chat (`* Ana invites you to a
party: P`), so that it is seen without the page; an invitation lapses after a minute, a
request to trade after half of one.

### 8.2 The party on the HUD

Under the squad: each other member of the party, with the health the wire carries for its
body when it does (`Bojan  140`; `down` at none), the name alone when its body is here
and the wire carries none (out of sight, or fighting as itself under a closed roster),
and `away` when no body of its is here.

### 8.3 Trade (`trade with NAME`; opened by the zone's `TradeOpened`, over whatever screen was up)

Two offers one above the other, each as wide as the panel (a row has room for the
longest name the content has, what the thing does and, of gear, how much of it is left,
and its mark: `sword  slash +11.0%, 250 of 250  new`); the coin is the last row of each.
A header says whose offer it is and `accepted` when they have. Below, the inventory to
offer from. **Offer**, **Take back**, a price in three fields and **Set coin**, **Accept**
(6), **Cancel**. The two lines above the buttons say what matters most at the moment:
how it ended; that the other is not here any more; the hub's answer to the last thing
done (which the next version of the offer clears, it was about the one before; the answer
to one's own change stays through the version that change made); that the other changed
the offer since it was looked at; how long until the offer can be accepted; that
one's own accept waits for the other's. When it is over, only **Close** is left. The
window opens over an open chat line (which is dropped), and a second word of the same
trade does not open it again.

### 8.4 Tavern (`tavern`; from the page of people)

As 7: the rule, the list (name, role, price, how often hired) with the picked one in full
under it (`what, role` and `NAME for PRICE`: the longest name and the dearest price fit
the line together), the character's hires and when each ends, and its own listing with a
price in three fields. **Hire**, **Dismiss**, **List**, **Withdraw**, **Back**.

## 9. Acceptance (`scripts/check-party.sh`)

1. **The simulation** (`gm-ai/tests/party.rs`, `gm-core`): two humans and their squads at
   the Warden, as one party (one party on the ledger, both paid, no trial passed) and as
   two; a squad follows its commander's party; five people without companions, of whom one
   holds the boss and one mends: all are over the member floor.
2. **The hub**, with a database (`gm-hub/tests/party.rs`): invitations and their limits; a
   party made, joined, left, led on, dissolved; removed by the leader only; a declined
   invitation that stays; the numbers of the readings in the order of the changes, with a
   claim racing two members who leave at once, 24 times, heard by a zone in the worst
   order; the repair by a save's answer; away, then out, and the lead passed on; lines to
   the zones of those who hear, and to both zones of somebody on the way.
3. **Two zones with clients driven by hand** (`gm-server/tests/party.rs`): an invitation by
   a name as a person writes it; the party on both wires and the health of a member, and
   of nobody else; a party's line and a whisper, and chat's limit over all three kinds of
   line; a trade asked from across the square (refused) and standing together (opened),
   the other told once; the window's three seconds and its commit; a trade that ends when
   one of the two is claimed elsewhere; the party held from the town to the dungeon; and
   in the dungeon a fight in whose middle a third joins the party (and fights as itself
   until it is over) and one of the two is removed (and fights on as it began), both told
   when the fight is over; nobody who left a fight let back into it.
4. **By somebody who is not a person** (`--online`, `--desktop`, `--browser`): two bots
   form a party in the town and leave the game; within the time a party waits they meet
   again in the tutorial dungeon, of the party there, and clear it together, each with the
   zone's recruits; the hub splits what the Warden drops between them, and they are told
   at the pull that a party passes no trial. One of them goes back to the town and stays;
   the other stays away, and the hub lets go of it. Then a character made at a real client
   (the desktop's, and both browser builds'), by UI script: asks the one in the town into
   a party by the page, says a party's line and a whisper and reads the answers, buys what
   the other got from the Warden through the trade window for three silver, hires an
   avatar in the tavern, and leaves the party.
5. The hub's audit is sound after all of it, to the silver, and no client logged an error.

## 10. Deliberately absent, and known gaps

- Friends, a list of who is in the game, guild chat, a channel for trade.
- A party finder, a ready check, marks on targets, a party on the map.
- Loot rules other than the split (PLAN.md 6: no master looter).
- A leader handing the lead over.
- Trading at a distance; trading more than the inventory holds; a history of trades.
- A place for the tavern; a hire's portrait; sorting, searching and pages in the tavern
  (the list is the two hundred cheapest: somebody with many characters can fill it).
- Carry contracts (ECONOMY.md 8) still have no screen.
- **The list of people not heard is the client's.** The hub knows nothing of it: a line
  from somebody not heard is carried and dropped, and their invitation is declined by the
  client, not refused by the hub.
- **Chat's limit is per zone.** An account with characters in several zones at once has a
  bucket in each (how many of an account's characters may play at once is open, PLAN.md
  12).
- **Whether somebody is in the game can be asked** by whispering to them or inviting them.
- Two lines of one speaker that the hub relays travel on streams of their own: they
  arrive in order nearly always, and nothing promises it. A whisper to somebody on the way
  between two zones is told to both, and is lost if neither has a body for it then.
- **An owner can take a hired character back** the moment it was paid for (ECONOMY.md 11
  rules that nothing is refunded, and the page says so). Paying the owner by the time
  served is a change of that rule (11).
- Outside an encounter a party changes at once, and what reads it follows: somebody who
  leaves a party and kills a member a moment later has killed no team-mate.
- A member who is away holds its place for two minutes; the leader can remove it sooner.

## 11. Proposed numbers and open decisions

| Number | Value | Why |
|---|---|---|
| Party size | 5 | five squads of three are twenty bodies at a boss tuned for four |
| Invitation's life | 60 s | long enough to open a page, short enough not to be a list |
| Unanswered invitations waiting for one character | 5 | |
| Invitations one leader has waiting | 5 less the party's size | an invitation is for a place there is |
| Away before the party lets go | 120 s | a reconnect, a crashed zone |
| The sweep | every 10 s | |
| A request to trade | 30 s; the other is told once per pair in that time | |
| `TRADE_REACH` | 160 units, 96 up | a little further than a stall's 120: two bodies do not stand in each other |
| The trade window's asking | once a second | under the 3 s lock |
| A trade nobody touches | called off after 10 min | |

Open, for the director:

- **What a member's contribution is.** As built: damage, healing credited and blows
  taken, of the member and its companions. Before this phase: damage only. And whether,
  with fewer components than members, the same members should be paid at every kill (the
  split is by rank, and starts from the top each time).
- **Whether a party of people passes trials.** As built: none (they are for one player
  and a squad).
- **Who invites**: the leader only, as built, or any member.
- **How long a member that left the game stays of the party** (two minutes, as built).
- **Whether somebody who left a fight may come back into it** (as built: not while it is
  engaged; a lost connection in a boss fight is the end of that fight for that player).
- **Whether a party of five may bring five squads** to a boss tuned for one (COMPANIONS.md
  16's numbers are for one human and three companions): a cap on bodies per party, or
  bosses that scale, is not decided here.
- **The tavern as a place** in the town; **paying a hired character's owner by the time
  served**, and a listing that serves only after its owner has been away some minutes.
- **Whether whispers reach somebody who is in a fight**, whether a player can close its
  whispers to strangers, and whether an account without standing (ANTICHEAT.md 6) may
  whisper to strangers at all.

## 12. Review log

**Design review, 2026-10-02** (an independent agent; Google AI Studio still answered HTTP
402). Three findings were High, seven Medium, eight Low; the reviewer found the control
split, the numbered readings and "one open trade" sound. All but two points were accepted.

| Finding | Verdict |
|---|---|
| **High.** Deferring the *body's* number is not enough: a member who died and was held leaves the zone and comes back, alive, a new body on no ledger, under the party's number at once ("the dead wait" held for no party of two people); a leader removes the dead and invites fresh people in their place, more than five under one number; a late joiner's squad turns the whole party's list from `top` to `standard` | **Accepted.** A fight's roster is closed: no body takes the number of a party that is on an engaged ledger (2). And nobody who left a fight comes back into it while it is engaged (the reviewer would have had the new body inherit the old one's line and its death; refusing the entry is the same rule with less to go wrong) |
| **High.** The trade window dropped its marks at the moment Accept armed (both after three seconds), and a thing taken back left no row to mark: accept, the other swaps the sword for a lesser one of the same name, two seconds of red, and the window looks agreed again | **Accepted.** What is marked is what the person has not agreed to: everything that was not in the offer when they last accepted, with what was taken back shown struck, until they accept again (6). Accept arms on time *shown*, and only on a fresh view. Kept at the client: the mark fails towards marking everything (a window opened again has agreed to nothing) |
| **High.** Inside a party the split pays damage only: a person who heals or tanks is under the 40% floor at every kill | **Accepted, as a proposal for the director** (2, 11): a member's contribution is damage, healing credited and blows taken. A mender and a tank are in the tests |
| Party state rides on notices and nothing repairs a miss: the asking zone hears only its answer; a notice is given up after 5 s; the zone's reader stopped for good at the first notice it could not read; the sweep read "offline" and wrote later | **Accepted.** The asking zone is sent the notice too; the reader skips what it cannot read; every periodic save's answer carries the number of the character's reading and a zone that is behind asks (3.2); the sweep looks at each member again under its row's lock; whereabouts are read after the commit. *Not done:* one ordered stream per zone for notices (lines may still swap; written down, 10) |
| The lock order of the first draft could not be kept (the last member of a dissolved party is not known before the party's row is read); what was built instead held a database connection for every waiting change, and stamped rows in an order that could deadlock with a trade's accept | **Accepted.** One lock, held in the hub, written down (3.1); rows in ascending order; a deadlock the database reports is `Busy`; the sweep is one change per member |
| "Within ten seconds" was the sweep's phase, not a grace: a crashed zone cost everybody in it their party before it was back | **Accepted.** Two minutes away, shown as `away`; an offline leader's lead passes at the next sweep (2) |
| Pestering the limits did not bound: five characters keep a victim's five places full; a declined invitation is asked again at once; a request to trade to A, then B, then A tells both every two seconds; whispers from any new account | **Accepted in part.** A declined invitation stays its minute and holds no place; a leader's waiting invitations are bounded by the party's room; the other is told of a request to trade once per pair in thirty seconds; the client declines the invitations of those it does not hear. *Not done, written down (10, 11):* the list of people not heard at the hub; the chat bucket at the hub; whether an account without standing may whisper |
| A trade's end was left to a zone request that does not exist; two openings for one character could both pass; a trade accepted on one side waits for ever | **Accepted.** The hub calls a character's open trades off at every claim, and any trade untouched for ten minutes; opening takes both characters' rows whole. *Rejected:* refusing to open a trade within ten seconds of a fight (both asked for the window; nobody has it put on them) |
| The tavern page makes old holes reachable: the build sold can be changed after the sale; an owner enters for a second, keeps the 70% and the listing; two hundred listings at a copper are the whole list | **Accepted in part.** A hire keeps the build it was bought with, and names its price. The owner's taking back is ECONOMY.md 11's rule: the page says it before anybody pays, and paying by time served is put to the director (11). The list's pages: written down (10) |
| Two people who clear the tutorial together pass no trial, and learn it after the kill | **Accepted.** Said when the fight begins (2); whether a party should pass is the director's (11) |
| `/w De Vil hello` goes to "De" | **Accepted.** One word; names are found without what is between their letters (5) |
| An invitation for a character between two zones finds no body and is not kept | **Accepted.** Kept for the body (4) |
| `Party` is names only: a hired copy bears a real character's name | **Accepted in part.** The client matches the names among the bodies that are people. (A hired copy's owner is offline; the two overlap only for the moment a hire ends) |
| The People page moves by itself | **Accepted.** The rows keep their places while it is up (8.1) |
| 3.2 said "plays in the asking zone" for one request only; "the hub's limit" for `Say` contradicted 3.3 | **Fixed** |
| Other readers of `party` (team kills, what opens a recording) follow a party that changes at once outside encounters | **Written down** (2, 10) |
| The sweep did not look again at whom its first query listed | **Fixed** (with the repair above) |
| Two lines of one speaker can swap | **Written down** (10) |

**Code review, 2026-10-02** (three independent agents, one each for the hub, the zone with
the wire and the simulation, and the client with the bot and the gate; Google AI Studio
still answered HTTP 402). 36 findings: 1 High, 9 Medium, 2 between, 24 Low; 33 acted on,
2 in part, 1 written down. What each found sound (the numbered readings in every
interleaving tried, the authorization of every request, the lock order of the economy,
the enum split's bytes, the snapshot codec, the reconcile loop under every sequence tried,
the gates, the arming of Accept, the rows that never move) is in the session's log; what
changed:

| Finding | Verdict |
|---|---|
| **High (hub).** The tavern's page made words of a listed build without validating it; a build the content no longer has indexes an ability out of bounds, and the hub is built with `panic = "abort"`: any player opening the page restarts the hub, again and again | **Accepted.** `build_words` validates first and says nothing of a build it cannot; such a listing is not shown and sells nothing (7) |
| **Medium (hub).** A hire pinned the price it was shown, not the build: an offline owner could swap the build between the listing and the hire | **Accepted.** The listing keeps the build it was made with; the tavern shows it and the hire buys it (7) |
| **Medium (hub).** The shown price, the kept build, unlist, trade expiry and the accept while apart had no hub-level test; the race rounds all ended in no party, where any order of hearing gives the same answer | **Accepted.** A test of each rule in `tests/economy.rs`; every other race round ends in a party of two |
| **Medium (hub).** `trades` had no index, and a character's open trades are looked for at every claim | **Accepted.** Partial indexes on the open trades of either character (0010) |
| **Medium (zone).** `work()` as a party's contribution let a stranger standing in the boss's cleave for a minute, or mending another party's tank, pass the 10% floor and take a `top`-list drop | **Accepted.** Among parties by damage, as before; work ranks members within a party (2) |
| **Medium (client).** The two offers side by side cut names at eight characters and what a thing does at thirteen; the gate's own trade was shown cut and passed, because `expect` reads the recorded row, not the drawn one | **Accepted.** The offers one above the other, each as wide as the panel; the sizes test uses the content's longest words and the dearest price (8.3) |
| **Medium (client).** The coin line's notes (`was …`, `accepted`) were cut mid-number or drawn over the other column | **Accepted.** The coin is a row of the offer, marked like any item; `accepted` is in the header |
| **Medium (client).** The hub's last word (`the coin is set`) stood over `changed the offer` and `not here any more` for ever | **Accepted.** A new version clears the hub's word; `not here` comes first, then the hub's word about this version, then the change (8.3) |
| **Medium (client).** `/r` found its target when Enter was pressed: a whisper arriving meanwhile turned the answer elsewhere | **Accepted.** `/r ` becomes `/w NAME ` as it is typed (5) |
| **Medium (client, design).** Before the first accept a swap looked like any new row | **Accepted.** Marks against what was looked at when Accept last armed: `changed`, and the struck row (6) |
| Low (hub): `trade_view` made a statement per offered item under a one-second poll; a stranger's accept cancelled a trade whose two were apart; the away clock was `characters.updated`, which a build set while offline refreshes; the sweep held a member's row and then locked ascending, and `offline_zone` updated in heap order against the new multi-row locks; the sweep dropped committed news on an error; `Heard.to` and the stamping of joiners differed from the documents | **Accepted, all.** Three statements; participant and state checked first; `offline_since` by a trigger; the party's rows locked ascending, `offline_zone` through an ordered locking subquery; the sweep returns what it changed; joiners stamped, HUB.md fixed |
| Low (zone): a reconnect for the same character dropped the fighting body and then refused the join; a refused rejoin left a bodiless reading; a late `TradeOpened` freed the gate a second time and `trade_told` outlived the opening; a whisper to a name nobody can have was counted as flooding; names were counted in characters before bytes on the tick thread; party numbers were kept for the life of the zone and nothing kept entity ids below them; a stray `Hello` was logged whole | **Accepted, all.** A body in a fight is left alone and the newcomer turned away; `parties.left` on replacement; `tell` on the trade's answer; `nobody can be called that`; bytes first; a dissolved party's number let go of, `MAX_ENTITY_ID` asserted at allocation; not quoted |
| Low (zone): the trials line is told by the hub's word while the verdict is the ledger's | **Written down** (2): a member who took no part in the fight is not of it for the trial |
| Low (zone): the mid-fight tests assert lines and the wire but no loot; the zones' `max_ticks` was tight | **Partly.** More ticks; the split with people is `gm-ai/tests/party.rs`'s (written down) |
| Low (client): `reply_to` outlived the character; the window opened over an open chat line; the HUD said `away` of a member standing here out of sight; a join took the invitation off the page before the zone answered, and the client's own declining skipped the gate; an invitation made again kept the old clock; `NAME gives` and `hire NAME for PRICE` overflowed with a 24-byte name; the LOOK time and the refused hire's price were not pinned by the tests; whispers were logged at info; an inviter from another zone read `gone`; a second `TradeOpened` rebuilt the window | **Accepted, all.** (8.1–8.4, 5) |
| Low (gate, bot): the inviter had two tries in a ten-second stay; `quiet` passed on a missing log; the bot set `offered` before the hub took the offer | **Accepted.** Sixteen seconds; a missing log fails; `offered` is the hub's answer, and a bot with nothing offered accepts nothing |
| Low (client): the other's coin was abbreviated in the row | **Accepted** by the coin row (27 characters: the dearest price fits) |
| Low (hub): `offline_zone`'s deadlock at a zone's disconnect is only logged | **Written down**: the characters stay in the zone until it registers again |

### 12.4 Review by Gemini 3.1 Pro, after the fact (2026-10-03)

The Gemini account had no credit when this phase was written (the reviews above are
independent agents'); with credit back, Gemini 3.1 Pro read this document and the commit's
hub, zone and protocol parts (the whole diff, 700 KB, left it no room to answer: its first
attempt argued with itself and was cut off; the one claim it made before, that `trade_cancel`
takes no caller, is false: it checks `trade_side`), asked for what the earlier reviews missed.
2 findings.

1. Medium, *a zone's party numbers leak: a party dissolved after its last member left the
   zone is news the zone never hears, and its number stays for the zone's life*: **accepted,
   fixed**. When a body leaves, the numbers of parties nobody here nor anybody kept (two
   minutes) is held to be of are let go of; a test leaves everybody and waits them out.
2. Low, *a trader who disconnects while the hub opens the trade leaves the other untold: the
   zone looked the names up after the answer*: **accepted, fixed**. The names are taken when
   both asked and travel with the answer; whoever is still here is told, and can close a trade
   the hub holds open in their name.

Its verdict on the earlier reviews: sound overall.

## 13. What was measured

All with the final build on the development machine (AMD Renoir integrated GPU, Postgres 18
in a private cluster), none estimated; ranges are the spread over repeated runs.

- **The hub's one writer**: four parties of five made and unmade at once, six rounds each,
  with invitations and lines through it all: 192 changes in 0.57–0.66 s, **291–336 changes a
  second**, nothing answered `Busy`, every change numbered once, the zone's model of the
  largest number per character equal to the hub's at the end (`gm-hub/tests/party.rs`).
- **Numbered readings under races**: 24 rounds of a claim racing two leaves (every other
  round ending in a party of two), heard in the worst order: the model equals the hub's
  reading for every character in every round.
- **Two zones with clients by hand** (`gm-server/tests/party.rs`): 55 s for the whole
  story, including a dungeon fight with a joiner and a removal in its middle.
- **Two people through the dungeon** (`check-party.sh --online`): a party formed in the town
  within a second of the invitation; both back in the dungeon of the party; the Warden down
  in **120.0–149.2 s** from the first bot's entry (165 s in the first try by hand), three
  components and 30 silver split between the two, told at the pull that a party passes no
  trial; the member that stayed away let go of by the hub 20–30 s after it went offline (the
  hub ran with `--party-away 20`). The final run: **120.0 s**.
- **By UI script** (software GPU, the desktop client): from being in the party to the trade
  being done, two chat lines and the three-second look in between, **6.2–6.3 s**. Both
  browser builds did the same.
- **A frame with a page up** on the integrated GPU at 1280×720, uncapped, in the town
  (three runs): the game alone 0.36–0.37 ms, the people 0.44–0.45, the trade window
  0.46–0.47, the tavern 0.45–0.46: under a tenth of a millisecond for a page.
- **The browser build**: the control enum's split saved 62,834 bytes and dropping the
  whole-message formats 27,741; with the three screens the WebGPU build is **977,242
  bytes** (332,973 packed), 27,844 less than Phase 11's, the WebGL2 build 2,972,499
  (906,803 packed).
- The workspace suite: **344 tests** (329 before this phase).
