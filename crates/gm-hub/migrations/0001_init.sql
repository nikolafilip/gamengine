-- gamengine hub schema v1 (HUB.md 4). Every state change of a character is one transaction;
-- "exactly one place" is a constraint, not a code path.

create table accounts (
    id               bigserial primary key,
    email            text        not null unique,
    password_hash    text        not null,
    created          timestamptz not null default now(),
    trust_tier       smallint    not null default 0,
    upload_privileges boolean    not null default false
);

create table characters (
    id              bigserial primary key,
    account_id      bigint      not null references accounts(id) on delete cascade,
    name            text        not null,
    build           jsonb       not null,
    location_kind   text        not null default 'offline'
                    check (location_kind in ('offline', 'zone', 'transit')),
    location_zone   text,
    transit_to      text,
    transit_since   timestamptz,
    pos_x           real        not null default 0,
    pos_y           real        not null default 0,
    pos_z           real        not null default 0,
    yaw             real        not null default 0,
    viewport        smallint    not null default 0,
    play_seconds    integer     not null default 0,
    created         timestamptz not null default now(),
    updated         timestamptz not null default now(),
    constraint characters_name_unique unique (name),
    constraint characters_location_shape check (
        (location_kind = 'offline' and location_zone is null and transit_to is null and transit_since is null)
        or (location_kind = 'zone' and location_zone is not null and transit_to is null and transit_since is null)
        or (location_kind = 'transit' and location_zone is not null and transit_to is not null and transit_since is not null)
    )
);

create unique index characters_name_ci on characters (lower(name));
create index characters_account on characters (account_id);

create table zones_log (
    id      bigserial primary key,
    zone    text        not null,
    event   text        not null,
    detail  text        not null default '',
    at      timestamptz not null default now()
);
