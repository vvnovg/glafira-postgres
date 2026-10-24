-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)
CREATE TABLE shop.events_2026_09 PARTITION OF shop.events
    FOR VALUES FROM ('2026-09-01 00:00:00+00') TO ('2026-10-01 00:00:00+00');
