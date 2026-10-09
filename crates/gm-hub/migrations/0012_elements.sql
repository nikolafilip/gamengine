-- The six elements (MATRIX.md 14, 2026-10-09). A stored build's aspects are a bitmask over
-- the elements in their order; the five old ones were flame 0, shadow 1, storm 2, frost 3,
-- stone 4, the six are fire 0, water 1, grass 2, electric 3, ground 4, air 5, and the old
-- became the new as flame -> fire, frost -> water, storm -> electric, stone -> ground,
-- shadow -> air. Every build the hub keeps (characters, hires, their listings) is moved.
create function gm_remap_aspects(old int) returns int language sql immutable as $$
    select (old & 1)
         | (((old >> 3) & 1) << 1)
         | (((old >> 2) & 1) << 3)
         | (((old >> 4) & 1) << 4)
         | (((old >> 1) & 1) << 5)
$$;

update characters
   set build = jsonb_set(build, '{aspects}', to_jsonb(gm_remap_aspects((build->>'aspects')::int)))
 where jsonb_typeof(build->'aspects') = 'number';
update hires
   set build = jsonb_set(build, '{aspects}', to_jsonb(gm_remap_aspects((build->>'aspects')::int)))
 where build is not null and jsonb_typeof(build->'aspects') = 'number';
update hire_listings
   set build = jsonb_set(build, '{aspects}', to_jsonb(gm_remap_aspects((build->>'aspects')::int)))
 where build is not null and jsonb_typeof(build->'aspects') = 'number';

drop function gm_remap_aspects(int);
