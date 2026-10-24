-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)
CREATE TABLE shop.orders (
    id bigint DEFAULT nextval('shop.orders_id_seq'::regclass) NOT NULL,
    customer_id bigint NOT NULL,
    total numeric(12,2) NOT NULL,
    total_with_tax numeric(12,2) GENERATED ALWAYS AS ((total * 1.2)) STORED,
    status text DEFAULT 'new'::text NOT NULL,
    CONSTRAINT orders_total_check CHECK (total >= 0::numeric),
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);

CREATE INDEX orders_customer_idx ON shop.orders USING btree (customer_id);

ALTER TABLE shop.orders ADD CONSTRAINT orders_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES shop.customers(id);
