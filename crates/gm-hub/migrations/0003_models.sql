-- gamengine models schema v1 (MODELS.md 6.1). A model is named by the SHA-256 of its ingested
-- bytes; its status belongs to the upload it was ingested from (source_hash), across frames.

alter table accounts add column moderator boolean not null default false;
alter table accounts add column upload_strikes smallint not null default 0 check (upload_strikes >= 0);
alter table accounts add column model_slots smallint not null default 4 check (model_slots >= 0);

create table models (
    hash          bytea primary key check (octet_length(hash) = 32),
    frame         smallint not null check (frame between 0 and 3),
    status        text not null check (status in ('pending', 'active', 'rejected', 'takedown')),
    bytes         integer not null check (bytes > 0),
    triangles     integer not null,
    vertices      integer not null,
    tex_w         smallint not null,
    tex_h         smallint not null,
    source_hash   bytea not null check (octet_length(source_hash) = 32),
    source_bytes  integer not null,
    uploaded_by   bigint not null references accounts(id),
    uploaded      timestamptz not null default now(),
    decided_by    bigint references accounts(id),
    decided       timestamptz,
    reason_code   text not null default ''
                  check (reason_code in ('', 'copyright', 'likeness', 'sexual', 'hateful', 'other')),
    reason        text not null default '',
    -- One ingested model per upload and frame.
    constraint models_source_frame unique (source_hash, frame)
);
create index models_pending on models (uploaded) where status = 'pending';

-- Who holds a model: everyone who uploaded it and certified the terms.
create table model_holders (
    hash         bytea not null references models(hash) on delete cascade,
    account_id   bigint not null references accounts(id) on delete cascade,
    tos_version  smallint not null,
    added        timestamptz not null default now(),
    primary key (hash, account_id)
);
create index model_holders_account on model_holders (account_id);

-- What a character wears. A takedown sets it to null in the same transaction.
alter table characters add column model bytea references models(hash) on delete set null;
create index characters_model on characters (model) where model is not null;

-- The audit trail (MODELS.md 10): never deleted.
create table model_events (
    id      bigserial primary key,
    at      timestamptz not null default now(),
    model   bytea,
    actor   bigint references accounts(id),
    event   text not null,
    detail  text not null default ''
);
create index model_events_model on model_events (model);
