# Economy

Status: v1, Phase 5. The contract for items, coin, trade, stalls, escrow contracts, boss drops,
crafting, storage, guild halls and tavern hires. PLAN.md 0, 5 and 6 are binding: small currency,
deterministic drops, no chore sinks, no listing tax, everything trades, and **every coin and item
movement is one database transaction**. `gm-hub::economy` implements this document; when they
disagree, the document wins. Section 12 holds the proposed numbers the director has not yet
confirmed and the review log. What a character wears, what gear does, and the screens for the
inventory and a stall are ITEMS.md (Phase 11).

## 1. Principles

1. **One owner at a time.** Every item and every coin sits in exactly one *holder*. A movement
   is a single transaction that debits one holder and credits another; there is no state in
   which a thing is in two places or in none.
2. **The ledger is the truth about coin.** Every coin movement writes a ledger row in the same
   transaction. Coin enters the world only from the `source` holder (drops) and leaves only to
   the `sink` holder (the hire burn). Money supply = the coin of every holder outside those two.
   Two invariants hold at all times and a test asserts them after every scenario (`audit`):
   created = circulating + burned, and every holder's balance = ledger in − ledger out.
3. **State machines live in the database.** Escrow and trade states are columns with check
   constraints and guarded updates (`where state = 'open'`), not flags a code path remembers to
   test. A replayed or reordered request changes nothing.
4. **Rules that protect players are structural** (PLAN.md 6): the trade window clears both
   accepts on any change and enforces a 3 s cooldown; escrow cannot be cancelled mid-run; stall
   tiles cannot overlap; loot rules cannot change in combat.
5. **No chore sinks.** No fee, tax, durability or upkeep exists in the schema. The only coin
   sink is the 30% broker burn on tavern hires; the only item sink is 50% decomposition.

## 2. Coin

Integer **copper**. 100 copper = 1 silver, 100 silver = 1 gold; 1 gold (10,000 copper) is a small
fortune. Amounts are `bigint`, never negative (check constraint); displays use the largest unit
with a colour per unit, never a long digit string (PLAN.md 6, digit shifting).

## 3. Holders

```
holders (id, kind, owner_account, owner_character, guild_id, min_rank, capacity, coin)
kind: character | storage | stall | escrow | guild_chest | ground | source | sink
```

- `character`: one per character, **24** item slots, carries coin.
- `storage`: one per **account** (PLAN.md 1.2: per account, not per character), **60** slots.
- `stall`: one per open stall, 12 slots, holds the listed items and the coin of escrowed buy
  orders.
- `escrow`: one per contract or pending trade leg; no capacity limit, never owned by a player.
- `guild_chest`: per guild hall chest, **48** slots by default (200 at most); `min_rank` 0 is the
  free-for-all chest ("walk in, see it, take it"), higher ranks gate withdrawals only (anyone in
  the guild may deposit).
- `ground`: per zone; dropped items persist there until picked up.
- `source`, `sink`: singletons; the only way coin is created or destroyed.

Capacity is enforced in the transaction that moves an item in (`count(*) < capacity`, with the
holder row locked). Gear never stacks (PLAN.md 5.4); materials are items too in v1. Emptied
stall and escrow holders are kept: the ledger rows that name them are their history.

## 4. Items

```
items           (id, template, holder_id, created)
item_components (item_id, layer, material)        layer: shard | core | catalyst | frame | gem
```

Templates and materials are content (`assets/content/items.toml`): a template names its kind
(weapon, armour, component) and which layers it takes; a material names its layer and its stat
bias. Materials are named `layer/name` (`core/iron`). A **component** is an item with exactly one
component row and the `component` template; a crafted item carries one row per filled layer (PLAN.md 5.3: shard → passive trait, core →
weight/physical bias, catalyst → element, frame → speed/stamina/crit, gems → counter-meta).
Each material carries an `edge` in per mille of the item's base stats; `gm-content` refuses any
template whose best possible craft exceeds **250** (PLAN.md 0: the gear edge is capped at 25%).
The shipped best sword is exactly 250. A weapon template says what it `strikes` with and an
armour what it `guards` against; what a worn item's edges do, kind by kind, is ITEMS.md 3.

## 5. Moves and the ledger

```
coin_ledger (id, at, from_holder, to_holder, amount > 0, reason, ref)
item_moves  (id, at, item_id, from_holder, to_holder, reason, ref)
```

`move_coin` and `move_item` are the functions that change `holders.coin` and
`items.holder_id` (the trade commit moves its items in bulk under the same locks); both lock the
holder rows in ascending id order, then the item row, check the balance or the capacity, update,
and append the log row, in the caller's transaction. One lock order everywhere: crossing
movements cannot deadlock. Creation and destruction (drops, crafting, decomposition) write
`item_moves` rows with a null side. Reasons are an enum: `drop`,
`trade`, `stall_sale`, `buy_order`, `escrow_lock`, `escrow_pay`, `escrow_refund`, `hire`,
`hire_burn`, `craft`, `decompose`, `deposit`, `withdraw`, `pickup`, `ground`, and `grant` (an
operator's hand, ITEMS.md 4: out of the source like a drop).

A **worn** item (ITEMS.md 2) is moved by nothing and destroyed by nothing: `move_item` and
decomposition refuse it in words, an offer refuses it, and the database refuses whatever else
would try. The lock order with it: the character's row (shared, when a zone changes what a
character wears), holders, trade rows, the item, the worn row.

## 6. Trade window

A trade is between two characters in the same zone. State: `open → committed | cancelled`.

- Each side has an offer (items + coin) and an `accepted` flag.
- **Mutation lock**: any change to either offer clears **both** flags, stamps `changed_at`,
  bumps the offer **version**, and marks the changed slot (the UI shows it red). `accept` is
  refused for **3 s** after the last change, and it names the version the client was showing:
  a lagging client that has not seen the latest change cannot accept it blind.
- When both flags are set, the commit runs as one transaction: every offered item must still be
  in its owner's character holder, both sides must have the coin and the capacity for the
  **net** change (incoming − outgoing); all moves happen or none. A failed commit leaves the
  trade open with both flags cleared.
- An offered item that is destroyed (crafted with, decomposed) leaves every trade it was in,
  and those trades lose their accepts and get a new version.
- Both characters must be in the same zone when the window opens.
- The hub is the only writer; clients send intents (`offer`, `retract`, `accept`, `cancel`) and
  read the offers back with `TradeView`.
- **Since Phase 12 (PARTY.md 6)** a trade is opened by the zone the two play in, when both
  asked for it standing together (`ZoneEconOp::TradeOpen { a, b }`; a session can no longer
  open one). A character has one open trade: opening another cancels the one before. A
  trade ends at a claim of either character (it went to another zone, or came back from a
  dropped connection), at an accept that finds the two in different zones, and after ten
  minutes in which nothing changed. `TradeView` also says the other's name, whether both
  are still in one zone, and how long until an accept is taken; it is read in three
  statements without a transaction. An accept checks that the caller is one of the two
  and that the trade is open before anything else.

## 7. Stalls (PLAN.md 5.5)

`stalls (id, owner_character, zone, tile_x, tile_y, opened, expires = opened + 48 h, holder_id)`
with a **unique (zone, tile_x, tile_y)**: grid-snapped, non-overlapping by constraint. One open
stall per character. No listing fee, no tax.

**In the world (Phase 6).** A map says where stalls may stand with `gm_stall_grid` entities:
rectangles of square tiles on the ground (the town's market is 6 × 5 tiles of 128 u, 160 u
apart, facing the square; at most 512 tiles per map, tile numbers unique across a map's grids).
A stall is opened **through the zone**, because only the zone knows where a body stands:
the player sends `FromClient::StallOpen` (PROTOCOL.md 8), the zone checks that the player is
alive on a tile and that the tile is free, and asks the hub (`ZoneEconOp::StallOpen {
character, tile_x, tile_y }`), which checks that the character is playing in that zone and
lets the unique constraint settle two players racing for one tile (`Taken`). The session-level
`EconOp::StallOpen` of Phase 5, which let a client name any tile from anywhere, is gone.
Closing is the owner's `FromClient::StallClose` (or `EconOp::StallClose` from anywhere, or the
48 h). The zone loads its stalls from the hub when it starts (`ZoneEconOp::Stalls`), tells
every joiner (`FromZone::Stalls`) and everyone present when one opens or closes; the hub tells
the zone when a stall closes for any reason (`HubNotice::StallClosed`). A stall is drawn as a
counter on its tile with its **keeper**: the owner's frame, armour class and avatar model as a
body that stands there whether the owner is online or not, replaced by the owner in person
while the owner stands behind the counter. Since Phase 11 a stall has a screen (ITEMS.md 6):
what it sells is looked at from anywhere (`EconOp::StallView`) and **bought standing at it**,
through the zone (`FromClient::StallBuy`, `ZoneEconOp::StallBuy`: the session-level
`EconOp::StallBuy`, which let a program buy from anywhere, is gone as `StallOpen` went). Its
keeper lists and unlists (`StallList`, `StallUnlist`) while playing in the stall's zone. The
town board has no screen yet.

- `listings (stall_id, item_id, price)`: the item sits in the stall's holder. `buy` names the
  stall the buyer's zone saw it standing at and the price the buyer was shown, and is refused
  when the listing is not that stall's or the price differs; it moves the coin buyer → owner
  and the item stall → buyer in one transaction. An owner cannot buy from their own stall, and
  takes a listing back with `unlist` (the inventory must have room).
- `buy_orders (stall_id, template, material, price, quantity)`: the stall owner escrows
  `price × quantity` into the stall holder when posting (checked multiplication, at most 1,000
  units, prices at most 10^12); a seller fills an order by handing over a matching component
  and receives the price in the same transaction. Delivered items sit in the stall's 12 slots.
  An order cannot be cancelled while a fill is in flight (row lock); cancelling returns the
  remaining coin.
- Expiry or closing **always succeeds**: unspent coin and unsold items return to the owner,
  items to the inventory and then to the storage, which may overflow its cap. While the storage
  is over its cap nothing can be deposited and no new stall can be opened, so a stall is never
  extra storage and a full owner can never keep a tile. The hub sweeps expired stalls once a
  minute.
- The town board is a query: listings by template/material/price with the stall's tile as the
  waypoint.

## 8. Escrow contracts (carries, PLAN.md 6)

```
contracts (id, buyer, seller_party, instance, price, collateral, state, created, started, ended)
state: open → active → paid | refunded        (open → cancelled before anyone accepts)
```

- `open`: posted by the buyer; nothing is locked yet; the buyer may cancel.
- `accept` by the party leader: `price` moves buyer → escrow, optional `collateral` moves party
  leader → escrow (key runs), state `active`. From here **nobody can cancel**.
- `paid`: the zone reports "boss dead and buyer present (alive or not)"; escrow → sellers (split
  evenly, remainder to the leader), collateral back to the leader.
- `refunded`: the zone reports a wipe or an abandon vote (allowed after 5 min idle); price back
  to the buyer; collateral to the buyer too when the party abandoned. A contract still `active`
  **120 min** after it started is refunded by the hub as abandoned: stalling holds nobody's coin
  hostage.
- Only the zone named by `instance` may report, over its authenticated connection. The buyer
  cannot be among the sellers.
- The buyer cannot be kicked from the instance while `active` (zone rule). Every outcome is
  written to the public success ledger (`contracts` rows are never deleted).
- Outcome reports are idempotent: `update ... where state = 'active'` decides once.

## 9. Boss drops (PLAN.md 5.2, corrected split)

A boss kill yields **N components**, always (deterministic drops). The zone computes the split
with `gm_core::loot::split` and asks the hub to create the items (source → character holders):

1. Eligible parties: at least one living member in the arena at kill time **and** contribution
   (the damage the party dealt to the boss) at or above the **floor**: 10% of what the parties still
   standing dealt (a wiped party's damage must not turn everyone else into taggers).
2. Wiped parties get nothing. No last-hit bonus exists.
3. Shares are proportional to contribution among eligible parties, by largest remainder, with a
   **minimum of one** component per eligible party as far as the components go: with more
   eligible parties than components, the top contributors get one each and the rest none.
4. Inside a party the components go round-robin by contribution rank; a member below **40%** of
   the party's average contribution is skipped (the leech floor). Since Phase 12 a member's
   contribution here is its **work**: the damage it and its companions dealt, the healing
   they are credited with and the blows the creatures aimed at them, in points of health
   (PARTY.md 2); between parties, step 1, damage alone counts.
5. Coin drops are tiny and go the same way (source → holders), scaled by the zone to its active
   population (PLAN.md 5.4); the hub refuses a single grant above 1 gold.
6. A zone grants only to characters playing in it. A component for a full inventory lands on
   the zone's ground, never nowhere.
7. **A kill pays once** (Phase 7, COMPANIONS.md 10). The zone reports a kill as one
   `GrantKill { reference, components, coin }`: the hub claims `(zone, reference)` in the
   `kills` table and creates every component and every purse in that same transaction; a
   reference that is already claimed answers `Done` and changes nothing, so the zone repeats
   its report until it is answered. A recipient who has stopped playing in the zone by then
   forfeits: no coin is made for it and its components lie on the zone's ground. A party
   that brought companions draws from the boss's `standard` list, a party of humans only
   from its `top` list (the loot ceiling of PLAN.md 5.6); the companions themselves, hired
   or lent, receive nothing.

## 10. Crafting and decomposition (PLAN.md 5.3, 5.4)

- **Craft**: a template plus component items for its layers (core and frame mandatory, the rest
  optional, up to 2 gems; nothing of a layer the template has no room for: a cuirass takes no
  catalyst). One transaction: the component items are deleted, the new item is created with
  their rows. No fee, no failure chance.
- **Decompose**: an item with k component rows returns **⌊k / 2⌋** of them as component items;
  the rest are destroyed. Which ones is decided by a hash of the item id and the component's
  index (`salvage_indices`): fixed for the item, visible before decomposing, and not steerable
  by the crafter, who does not choose the id. Padding a craft with junk never raises the
  salvage above half. One transaction; refused when the holder lacks the slots. A lone
  component cannot be decomposed.

## 11. Tavern hires and guild halls (PLAN.md 5.6, 5.7)

- Since Phase 12 (PARTY.md 7): a hire **names the price** it was shown and is refused when
  the listing's is another now; a listing **keeps the build** the character had when it
  was listed (`hire_listings.build`), the tavern shows that build, and a hire buys and
  keeps it (`hires.build`), whatever the owner makes of the character afterwards; a
  listing can be withdrawn (`HireUnlist`) and looked at (`HireListed`); the list says what
  each avatar is and the role a mind plays it in, in words; a listing whose build the
  content no longer has is not served.
- A character whose owner is offline can be listed for hire at a flat price. `hire` moves the
  price from the hirer: **30% to the sink** (`hire_burn`), 70% to the avatar's owner, and records
  the hire for a **12 h** window. Hired avatars receive flat coin only, never loot. After **3**
  hires inside a window the avatar sorts last in the tavern list (diminishing priority). An
  account cannot hire its own characters. An owner who logs in takes their character back and
  ends the hire without a refund; the tavern says so before the hirer pays.
- What a hire buys (Phase 7, COMPANIONS.md 3.3): a **copy** of the listed character (its
  name, its stored build, its model) that fights in the hirer's squad, driven by a mind,
  wherever the hirer enters a zone that lets squads in. The character itself stays offline
  and may serve several hirers at once, each paying. A hire is **active** until its 12 h are
  over, its owner enters a zone with the character, or the hirer dismisses it (`Dismiss`,
  no refund). A character holds at most its squad capacity in active hires (3; 5 with a
  leadership ability in its build) and at most one copy of any one avatar; a hire beyond
  that is refused before any coin moves, under the hirer's holder lock so two hires at once
  cannot both see room. The tavern list carries each avatar's name and build: a hirer reads
  the role from it. `Squad` lists a character's active hires.
- A guild has members with ranks and a hall with chests (`guild_chest` holders). Depositing is
  open to members; withdrawing needs `rank >= min_rank` of the chest.

## 12. Proposed numbers and review log

**Proposed (director to confirm; PLAN.md 12 lists them as open):** 24 inventory slots, 60
account storage slots, 12 stall slots, 3 s trade cooldown, the 10% party floor and 40% member
floor, the every-other-layer salvage rule. Purchasing power, as a sanity scale for content and
drop tuning: a meal 5 copper; standard gear 2–5 silver; a boss component 10–40 silver; a fully
crafted top item 1–3 gold; a 12 h tavern hire 20–60 silver; a carry 50 silver – 2 gold.
Death-drop in contested zones and housing remain open and are not in this version.

Also proposed: stacking of materials is **not** in v1 (every component is a row and a slot);
the 120 min contract timeout; the 1 gold cap on a single coin grant; no self-hire; one copy
of an avatar per squad; a recipient who left the zone before its kill is reported forfeits
(Phase 7).

**Phase 12 (2026-10-02, PARTY.md)** made trades something two people ask for standing
together, gave them ends, changed what a member's contribution to a kill is (work, not
damage alone: proposed, PARTY.md 11) and made a hire name its price and keep its build.

**Phase 11 (2026-10-02, ITEMS.md)** added what is worn and moved buying to the zone; its
proposed numbers and open decisions are ITEMS.md 9 (among them: the storage is reached from
anywhere, buy orders are still filled from anywhere). The economy's requests of a session are
now limited per account (five a second, twenty in hand).

**2026-10-01, design reviewed by Gemini 3.1 Pro** (before the module was finished):
- Accepted: the ledger invariant was worded as a sum of the ledger, which is volume, not supply
  (now the two `audit` invariants in 1); the every-other-layer salvage let a crafter pad with a
  junk shard to choose which expensive part survives (now ⌊k/2⌋ by an unsteerable hash); a
  stall that could not be emptied never expired and was 12 free slots on a blocked tile (close
  always succeeds, storage overflows and locks deposits and new stalls); a lagging client could
  accept an offer it had not seen (accept names the version); a party could hold escrow hostage
  by never finishing (120 min refund); guild chests had no capacity (48, at most 200); the
  minimum-one rule contradicted the more-parties-than-components rule (reworded); an owner
  logging in during a hire was undefined (defined, no refund).
- Already covered by the code: checked multiplication and bounded prices for buy orders; zero
  coin moves return before taking any lock; trade capacity as a net change.
- Rejected: collateral forfeited to the buyer on a wipe (a buyer could then sabotage a run to
  collect it; wipes are recorded on the public contract ledger instead); a minimum rank to
  deposit into guild chests (the free chest is the point of PLAN.md 5.7, and items are not free
  to create); stacking materials now (a schema change the director should decide; proposed
  above as open).

**2026-10-01, implementation reviewed by Gemini 3.1 Pro** (after the scam suite passed), and
by the author in parallel; the two reviews overlapped:
- Accepted and fixed, found by both: `trade_accept` took the trade row before the holders while
  `craft` and `decompose` took the holder before the trade row (now holders, trade rows, items
  everywhere; a destroyed item leaves its trades before any item row is locked); `stall_buy` and
  `buy_order_fill` took the listing or order row before the holders while `stall_close` and
  `buy_order_cancel` did the reverse (now holders first, then the row, re-read under the lock);
  `hire`, `contract_accept` and `contract_report` took holders one `move_coin` at a time (now
  one ordered `lock_holders` call up front; `grant_components` likewise).
- Accepted: the total of a buy order was capped at 10^12 although price and quantity are each
  bounded and their product fits (the cap on the total is gone).
- Found by the author only: every coin drop and every hire queued on the single `source` or
  `sink` row (they now keep no balance and take no lock; created and burned are sums of their
  ledger rows, with indexes on both ledger columns); `Drop` and `Pickup` were client requests,
  so a client could take anything on the zone's ground from anywhere (they are zone requests
  now: only the zone knows where a character stands); a deadlock or serialization victim was
  reported as an internal error (now `Busy`, nothing happened, repeat); `hire` read the
  avatar's location without locking the character row.
- Nothing rejected.

Measured on the development machine (debug build, Postgres 18 on the same host): the scam
suite is 15 tests in 1.6 s; the storm test (six characters trading, selling, filling, crafting,
contracting and receiving drops against each other at once) runs 792 operations in 1.4 to 1.5 s,
**520 to 560 per second**, with no deadlock victim and a sound audit, five runs out of five.
