-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)
CREATE TABLE shop.customers (
    id bigint GENERATED ALWAYS AS IDENTITY NOT NULL,
    email text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT customers_pkey PRIMARY KEY (id),
    CONSTRAINT customers_email_key UNIQUE (email)
);

CREATE TRIGGER customers_touch BEFORE INSERT ON shop.customers FOR EACH ROW EXECUTE FUNCTION shop.touch();

COMMENT ON TABLE shop.customers IS 'Покупатели';
