-- Phase 7 (COMPANIONS.md 3.3, 11): hires become companions, and role trials are remembered.

-- A hire is active for its 12 h window or until it is ended early: the avatar's owner took
-- the character back, or the hirer dismissed it. Nothing is refunded either way.
alter table hires add column ended timestamptz;
create index hires_hirer_active on hires (hirer_character, at) where ended is null;

-- A character passes a trial once; a faster pass replaces the time.
create table trials (
    character_id  bigint not null references characters(id) on delete cascade,
    trial         text not null,
    zone          text not null,
    secs          integer not null check (secs >= 0),
    passed_at     timestamptz not null default now(),
    primary key (character_id, trial)
);

-- A kill pays once (ECONOMY.md 9). The zone names every kill; its drop and its coin are one
-- transaction that begins by claiming the name, so a report that is repeated because its
-- answer was lost finds the row and grants nothing.
create table kills (
    zone  text not null,
    ref   bigint not null,
    at    timestamptz not null default now(),
    primary key (zone, ref)
);
