-- gamengine economy schema v1 (ECONOMY.md). One owner at a time: every item and every coin
-- sits in exactly one holder; every movement is one transaction and one log row.

create table holders (
    id            bigserial primary key,
    kind          text    not null check (kind in
                  ('character', 'storage', 'stall', 'escrow', 'guild_chest', 'ground', 'source', 'sink')),
    account_id    bigint  references accounts(id) on delete cascade,
    character_id  bigint  references characters(id) on delete cascade,
    guild_id      bigint,
    min_rank      smallint not null default 0,
    zone          text,
    capacity      integer not null default 0,           -- 0 = unlimited
    -- Never negative. The source and the sink keep no balance: what they created and
    -- destroyed is the sum of their ledger rows.
    coin          bigint  not null default 0 check (coin >= 0)
);
create unique index holders_one_per_character on holders (character_id) where kind = 'character';
create unique index holders_one_storage on holders (account_id) where kind = 'storage';
create unique index holders_singletons on holders (kind) where kind in ('source', 'sink');
create unique index holders_ground_per_zone on holders (zone) where kind = 'ground';
insert into holders (kind) values ('source'), ('sink');

create table items (
    id         bigserial primary key,
    template   text   not null,
    holder_id  bigint not null references holders(id),
    created    timestamptz not null default now()
);
create index items_holder on items (holder_id);

create table item_components (
    item_id   bigint   not null references items(id) on delete cascade,
    layer     text     not null check (layer in ('shard', 'core', 'catalyst', 'frame', 'gem')),
    position  smallint not null default 0,
    material  text     not null,
    primary key (item_id, layer, position)
);

create table coin_ledger (
    id           bigserial primary key,
    at           timestamptz not null default now(),
    from_holder  bigint not null references holders(id),
    to_holder    bigint not null references holders(id),
    amount       bigint not null check (amount > 0),
    reason       text   not null,
    ref          bigint not null default 0
);

create index coin_ledger_from on coin_ledger (from_holder);
create index coin_ledger_to on coin_ledger (to_holder);

create table item_moves (
    id           bigserial primary key,
    at           timestamptz not null default now(),
    item_id      bigint not null,
    from_holder  bigint,
    to_holder    bigint,
    reason       text   not null,
    ref          bigint not null default 0
);

create table trades (
    id           bigserial primary key,
    a_character  bigint not null references characters(id),
    b_character  bigint not null references characters(id),
    state        text   not null default 'open' check (state in ('open', 'committed', 'cancelled')),
    a_coin       bigint not null default 0 check (a_coin >= 0),
    b_coin       bigint not null default 0 check (b_coin >= 0),
    a_accepted   boolean not null default false,
    b_accepted   boolean not null default false,
    -- Bumped by every change to either offer; an accept names the version it saw.
    version      integer not null default 0,
    changed_at   timestamptz not null default now(),
    created      timestamptz not null default now(),
    constraint trades_two_people check (a_character <> b_character)
);
create table trade_items (
    trade_id  bigint not null references trades(id) on delete cascade,
    side      char(1) not null check (side in ('a', 'b')),
    item_id   bigint not null references items(id),
    primary key (trade_id, item_id)
);

create table stalls (
    id               bigserial primary key,
    owner_character  bigint not null unique references characters(id),
    zone             text   not null,
    tile_x           integer not null,
    tile_y           integer not null,
    opened           timestamptz not null default now(),
    expires          timestamptz not null,
    holder_id        bigint not null references holders(id),
    constraint stalls_grid unique (zone, tile_x, tile_y)
);
create table listings (
    id        bigserial primary key,
    stall_id  bigint not null references stalls(id) on delete cascade,
    item_id   bigint not null unique references items(id),
    price     bigint not null check (price > 0 and price <= 1000000000000)
);
create table buy_orders (
    id        bigserial primary key,
    stall_id  bigint not null references stalls(id) on delete cascade,
    material  text   not null,
    price     bigint not null check (price > 0 and price <= 1000000000000),
    quantity  integer not null check (quantity >= 0 and quantity <= 1000)
);

create table contracts (
    id               bigserial primary key,
    buyer_character  bigint not null references characters(id),
    leader_character bigint references characters(id),
    instance         text   not null,
    price            bigint not null check (price > 0 and price <= 1000000000000),
    collateral       bigint not null default 0 check (collateral >= 0 and collateral <= 1000000000000),
    state            text   not null default 'open'
                     check (state in ('open', 'active', 'paid', 'refunded', 'cancelled')),
    escrow_holder    bigint references holders(id),
    outcome          text   not null default '',
    created          timestamptz not null default now(),
    started          timestamptz,
    ended            timestamptz,
    constraint contracts_active_has_escrow check (state in ('open', 'cancelled') or escrow_holder is not null)
);
create table contract_sellers (
    contract_id   bigint not null references contracts(id) on delete cascade,
    character_id  bigint not null references characters(id),
    primary key (contract_id, character_id)
);

create table guilds (
    id    bigserial primary key,
    name  text not null unique
);
create table guild_members (
    guild_id      bigint not null references guilds(id) on delete cascade,
    character_id  bigint not null unique references characters(id) on delete cascade,
    rank          smallint not null default 0
);

create table hire_listings (
    character_id  bigint primary key references characters(id) on delete cascade,
    price         bigint not null check (price > 0 and price <= 1000000000000)
);
create table hires (
    id                bigserial primary key,
    avatar_character  bigint not null references characters(id),
    hirer_character   bigint not null references characters(id),
    price             bigint not null,
    burned            bigint not null,
    at                timestamptz not null default now()
);
create index hires_avatar_at on hires (avatar_character, at);
