-- The zone a character's saved position belongs to (HUB.md 3.2). `location_zone` says where
-- the character is; it is null while offline, so it cannot say whose map the position is on.
-- Without this column a first entry, or an entry into another zone after a logout, was placed
-- at coordinates that meant nothing on the new map.
alter table characters add column pos_zone text;
