-- Character names are unique by what they look like at a glance (CLIENT.md 7): the hub
-- stores each name's skeleton (gm_hub_proto::names::skeleton) and keeps that unique.
alter table characters add column name_key text;
-- The names made before this get the skeleton the hub would compute: small letters without
-- their marks, i and 1 as l, 0 as o, nothing between the letters. (The "C" collation: the
-- small letter of an I is an i, whatever language the database was made for.)
update characters set name_key =
    translate(lower(name collate "C"), 'i10čćČĆšŠžŽđĐ -''', 'lloccccsszzdd');
-- Two old names that are alike at a glance both keep their owners: all but the oldest get
-- a key no new name can have (a control character and the row's id).
update characters c set name_key = c.name_key || chr(1) || c.id::text
    where exists (select 1 from characters o where o.name_key = c.name_key and o.id < c.id);
alter table characters alter column name_key set not null;
create unique index characters_name_key on characters (name_key);
