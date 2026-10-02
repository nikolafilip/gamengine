-- Phase 9 (ANTICHEAT.md): aim statistics, replays, reports, reputation, bans.

alter table accounts add column reputation integer not null default 0;

-- A replay a zone wrote: its bytes are a file named by `sha256` in the hub's replay store.
create table replays (
    id         bigserial primary key,
    zone       text not null,
    sha256     bytea not null unique,
    bytes      integer not null check (bytes > 0),
    started    timestamptz not null,
    seconds    real not null,
    reported   boolean not null,
    kills      integer not null,
    damage     bigint not null,
    stored     timestamptz not null default now(),
    -- A report or a flag points here: kept until the case is closed, and a while after.
    keep       boolean not null default false
);
create index replays_stored on replays (stored);

-- Who was in it, with what its numbers in that file break (empty: nothing).
create table replay_participants (
    replay_id     bigint not null references replays(id) on delete cascade,
    character_id  bigint not null references characters(id) on delete cascade,
    account_id    bigint not null references accounts(id) on delete cascade,
    analysed      integer not null,
    rules         text not null default '',
    primary key (replay_id, character_id)
);
create index replay_participants_account on replay_participants (account_id, replay_id desc);

-- An account's aim numbers per week (weeks since the epoch); `stats` is the bitcode of
-- `AimStats`, summed over every report of the week.
create table aim_weeks (
    account_id  bigint not null references accounts(id) on delete cascade,
    week        integer not null,
    analysed    integer not null,
    -- The layout of `stats` (bitcode of gm_replay::aim::AimStats): a row of another layout
    -- is not added to, it is begun again.
    layout      integer not null,
    stats       bytea not null,
    primary key (account_id, week)
);

-- A zone's aim report counts once, however often it is repeated.
create table aim_reports (
    zone   text not null,
    nonce  bigint not null,
    at     timestamptz not null default now(),
    primary key (zone, nonce)
);

-- An account that broke a rule in a week: one row per rule and week. It sets nothing; a
-- moderator closes it.
create table flags (
    id          bigserial primary key,
    account_id  bigint not null references accounts(id) on delete cascade,
    rule        text not null,
    week        integer not null,
    detail      text not null,
    replay_id   bigint references replays(id) on delete set null,
    at          timestamptz not null default now(),
    closed      timestamptz,
    unique (account_id, rule, week)
);

create table reports (
    id                  bigserial primary key,
    reporter_account    bigint not null references accounts(id) on delete cascade,
    reporter_character  bigint not null,
    target_account      bigint not null references accounts(id) on delete cascade,
    target_character    bigint not null,
    zone                text not null,
    reason              text not null,
    state               text not null default 'open'
                        check (state in ('open', 'upheld', 'not_proven', 'abusive')),
    replay_id           bigint references replays(id) on delete set null,
    note                text not null default '',
    decided_by          bigint,
    created             timestamptz not null default now(),
    decided             timestamptz
);
-- One open report per reporter and target.
create unique index reports_one_open on reports (reporter_account, target_account)
    where state = 'open';

-- Reputation is a ledger: `accounts.reputation` is the sum, moved in the same transaction.
create table reputation (
    id          bigserial primary key,
    account_id  bigint not null references accounts(id) on delete cascade,
    kind        text not null,
    delta       integer not null,
    reference   text not null default '',
    note        text not null default '',
    at          timestamptz not null default now()
);
create index reputation_account on reputation (account_id, at desc);

-- A ban is account-level and has an end; lifting it is a row, not a deletion.
create table bans (
    id          bigserial primary key,
    account_id  bigint not null references accounts(id) on delete cascade,
    until       timestamptz not null,
    reason      text not null,
    by_account  bigint not null,
    at          timestamptz not null default now(),
    lifted      timestamptz,
    lifted_by   bigint
);
create index bans_account on bans (account_id, until) where lifted is null;

-- Everything a moderator does about conduct, and everything it looks at.
create table mod_log (
    id          bigserial primary key,
    moderator   bigint not null,
    action      text not null,
    account_id  bigint,
    reference   text not null default '',
    note        text not null default '',
    at          timestamptz not null default now()
);
