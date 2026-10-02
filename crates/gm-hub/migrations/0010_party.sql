-- Parties of people (PARTY.md 2, 3). A character is in one party at most; a party has a
-- leader that is one of its members, and two members or more (the hub dissolves a party
-- of one in the change that would leave it so).
create table parties (
    id        bigserial primary key,
    leader_id bigint not null references characters(id) on delete cascade,
    -- The number of its last change.
    seq       bigint not null
);

create table party_members (
    character_id bigint primary key references characters(id) on delete cascade,
    party_id     bigint not null references parties(id) on delete cascade,
    -- The number of the change it joined by: who has been in the party longest.
    joined       bigint not null
);
create index party_members_party on party_members (party_id);

-- An invitation waits a minute for its answer. One that was declined stays until its
-- minute is over: it cannot be made again at once, and it holds no place.
create table party_invites (
    to_id    bigint not null references characters(id) on delete cascade,
    from_id  bigint not null references characters(id) on delete cascade,
    at       timestamptz not null default now(),
    declined boolean not null default false,
    primary key (to_id, from_id)
);

-- The number of the last change of a character's own membership: what the hub says of a
-- character that is in no party is numbered by it, and of one that is in a party by the
-- party's. Every change draws the next number of this sequence under the lock all changes
-- are made under, so the numbers are in the order of the changes.
alter table characters add column party_seq bigint not null default 0;
create sequence party_seq;

-- A hire keeps the build it was bought with (PARTY.md 7): what the owner makes of the
-- character afterwards is the next hirer's.
alter table hires add column build jsonb;

-- A listing keeps the build the character had when it was listed: that is what the tavern
-- shows and what a hire buys. A listing from before this column is not served until its
-- owner lists again.
alter table hire_listings add column build jsonb;

-- A character's open trade is called off when a zone claims it, and a trade nobody has
-- touched for ten minutes is called off by the hub's sweep (ECONOMY.md 6): both look for
-- open trades, of which there are few among the many that were.
create index trades_open_a on trades (a_character) where state = 'open';
create index trades_open_b on trades (b_character) where state = 'open';

-- When a character last went offline: the party's sweep lets go of a member that has been
-- away for longer than the hub allows (PARTY.md 2), and nothing else that touches an
-- offline row (a build set, a model worn, an operator's placing) may count as coming back.
alter table characters add column offline_since timestamptz not null default now();
create function characters_offline_since() returns trigger language plpgsql as $$
begin
    if new.location_kind = 'offline' and old.location_kind <> 'offline' then
        new.offline_since := now();
    end if;
    return new;
end $$;
create trigger characters_offline_since before update of location_kind on characters
    for each row execute function characters_offline_since();
