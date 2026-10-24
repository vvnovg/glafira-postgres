-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)
CREATE TABLE shop.events (
    id bigint NOT NULL,
    happened_at timestamp with time zone NOT NULL,
    payload jsonb
) PARTITION BY RANGE (happened_at);
-- partitions: shop.events_2026_09, shop.events_2026_10
