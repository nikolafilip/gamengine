-- What a character wears (ITEMS.md 2): one item in each place, an item in one place only.
-- The worn row refers to the item, so a worn item cannot be destroyed; the trigger keeps it
-- in its wearer's inventory whatever code moves items. The hub refuses both in words before
-- the database has to.
create table worn (
    character_id bigint not null references characters(id) on delete cascade,
    slot         text   not null check (slot in ('weapon', 'armour')),
    item_id      bigint not null unique references items(id),
    primary key (character_id, slot)
);

create function worn_stays() returns trigger language plpgsql as $$
begin
    if new.holder_id is distinct from old.holder_id
       and exists (select 1 from worn where item_id = old.id) then
        raise exception 'item % is worn', old.id using errcode = 'GM001';
    end if;
    return new;
end $$;

create trigger worn_stays before update of holder_id on items
    for each row execute function worn_stays();

-- Orders the hub's readings of what a character wears (ITEMS.md 3.3). A reading draws its
-- number and then reads what is committed; a zone that has two readings for one character
-- believes the one with the larger number, and the reading made after the last change has
-- the largest of all.
create sequence gear_seq;
