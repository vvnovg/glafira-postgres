-- Роль для диагностики (спец. P1 §10) и демо-схема shop: дерево, вкладки объектов, матрица версий.
CREATE ROLE glafira_diag LOGIN PASSWORD 'glafira-secret';
GRANT pg_monitor TO glafira_diag;
GRANT pg_signal_backend TO glafira_diag;
-- Роль приложения для сценариев P2 (сессии, блокировки, idle in transaction): не суперпользователь, потому что
-- pg_signal_backend роли диагностики завершает только такие сессии.
CREATE ROLE glafira_app LOGIN PASSWORD 'app-secret';

-- pg_stat_statements создаётся только там, где он загружен: pg-single нужен для «красного» чек-листа.
DO $$
BEGIN
  IF current_setting('shared_preload_libraries') LIKE '%pg_stat_statements%' THEN
    CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
  END IF;
END $$;

CREATE DATABASE orders;
\connect orders

CREATE SCHEMA shop;
CREATE TABLE shop.customers (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  email text NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT now()
);
COMMENT ON TABLE shop.customers IS 'Покупатели';
CREATE TABLE shop.orders (
  id bigserial PRIMARY KEY,
  customer_id bigint NOT NULL REFERENCES shop.customers (id),
  total numeric(12, 2) NOT NULL CHECK (total >= 0),
  total_with_tax numeric(12, 2) GENERATED ALWAYS AS (total * 1.2) STORED,
  status text NOT NULL DEFAULT 'new'
);
CREATE INDEX orders_customer_idx ON shop.orders (customer_id);
CREATE TABLE shop.events (id bigint NOT NULL, happened_at timestamptz NOT NULL, payload jsonb)
  PARTITION BY RANGE (happened_at);
CREATE TABLE shop.events_2026_09 PARTITION OF shop.events FOR VALUES FROM ('2026-09-01') TO ('2026-10-01');
CREATE TABLE shop.events_2026_10 PARTITION OF shop.events FOR VALUES FROM ('2026-10-01') TO ('2026-11-01');
CREATE VIEW shop.active_orders AS SELECT * FROM shop.orders WHERE status <> 'done';
CREATE MATERIALIZED VIEW shop.order_totals AS
  SELECT customer_id, sum(total) AS total FROM shop.orders GROUP BY customer_id;
CREATE SEQUENCE shop.invoice_seq AS integer;
-- Вкладка последовательности показывает last_value только роли с правом SELECT/USAGE на неё (иначе
-- pg_sequences отдаёт NULL): демо-стенд выдаёт право роли диагностики, интеграционные тесты проверяют обе ветки.
GRANT SELECT ON ALL SEQUENCES IN SCHEMA shop TO glafira_diag;
CREATE FUNCTION shop.order_count(c bigint) RETURNS bigint LANGUAGE sql STABLE
  AS $$ SELECT count(*) FROM shop.orders WHERE customer_id = c $$;
CREATE PROCEDURE shop.close_order(o bigint) LANGUAGE sql
  AS $$ UPDATE shop.orders SET status = 'done' WHERE id = o $$;
CREATE FUNCTION shop.touch() RETURNS trigger LANGUAGE plpgsql
  AS $$ BEGIN NEW.created_at := now(); RETURN NEW; END $$;
CREATE TRIGGER customers_touch BEFORE INSERT ON shop.customers FOR EACH ROW EXECUTE FUNCTION shop.touch();

INSERT INTO shop.customers (email) SELECT 'user' || g || '@example.com' FROM generate_series(1, 100) g;
INSERT INTO shop.orders (customer_id, total) SELECT 1 + g % 100, g FROM generate_series(1, 1000) g;
REFRESH MATERIALIZED VIEW shop.order_totals;
ANALYZE;
GRANT USAGE ON SCHEMA shop TO glafira_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA shop TO glafira_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA shop TO glafira_app;
