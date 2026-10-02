-- A zone's room is counted by the characters that are in it (HUB.md 3.8), at every entry.
create index characters_in_zone on characters (location_zone) where location_kind = 'zone';
