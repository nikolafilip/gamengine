-- Stacks (MODES.md 11.1): an item row carries how many of it there are. Gear and parts
-- are one; a stack of rounds or kits is one row and one slot, merged up to its template's
-- cap by the hub (the strict limit of what a body carries). The cap is the content's, not
-- the schema's; the column's bound is the most any stack may ever hold.
alter table items add column quantity integer not null default 1
    check (quantity >= 1 and quantity <= 1000);
