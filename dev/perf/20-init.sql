-- Стенд P3: роли, база perf с расширениями и объёмная схема shop (спец. P3 §13.1). Таблицы audit и balances
-- нарочно без ANALYZE и автовакуума: ошибка оценки и heap fetches для правил рекомендаций.
CREATE ROLE glafira_diag LOGIN PASSWORD 'glafira-secret';
GRANT pg_monitor, pg_signal_backend, pg_read_all_data TO glafira_diag;
CREATE ROLE glafira_app LOGIN PASSWORD 'app-secret';

CREATE DATABASE perf;
\connect perf

CREATE EXTENSION pg_stat_statements;
CREATE EXTENSION pg_stat_kcache;
CREATE EXTENSION pg_wait_sampling;
CREATE EXTENSION hypopg;

CREATE SCHEMA shop;
CREATE TABLE shop.customers (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  email text NOT NULL,
  region text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE shop.orders (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  customer_id bigint NOT NULL,
  status text NOT NULL,
  total numeric(12, 2) NOT NULL,
  created_at timestamptz NOT NULL
);
CREATE INDEX orders_customer_idx ON shop.orders (customer_id);
CREATE TABLE shop.order_items (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  order_id bigint NOT NULL,
  product_id int NOT NULL,
  qty int NOT NULL,
  price numeric(12, 2) NOT NULL
);
CREATE INDEX order_items_order_idx ON shop.order_items (order_id);
CREATE INDEX order_items_product_idx ON shop.order_items (product_id);
CREATE TABLE shop.events (id bigint NOT NULL, happened_at timestamptz NOT NULL, kind text NOT NULL, payload jsonb)
  PARTITION BY RANGE (happened_at);
DO $$
DECLARE m date := date '2025-01-01';
BEGIN
  FOR i IN 0..23 LOOP
    EXECUTE format('CREATE TABLE shop.events_%s PARTITION OF shop.events FOR VALUES FROM (%L) TO (%L)',
                   to_char(m, 'YYYY_MM'), m, m + interval '1 month');
    m := m + interval '1 month';
  END LOOP;
END $$;
-- Скошенные данные без статистики: 99 % строк относятся к заказу 1, планировщик оценивает соединение неверно.
CREATE TABLE shop.audit (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  order_id bigint NOT NULL,
  actor text NOT NULL,
  note text
) WITH (autovacuum_enabled = off);
CREATE INDEX audit_order_idx ON shop.audit (order_id);
-- Массовое обновление без VACUUM: карта видимости грязная, Index Only Scan ходит в heap.
CREATE TABLE shop.balances (customer_id bigint PRIMARY KEY, balance numeric(12, 2) NOT NULL)
  WITH (autovacuum_enabled = off);

INSERT INTO shop.customers (email, region)
  SELECT 'user' || g || '@example.com', (ARRAY['eu', 'us', 'apac', 'latam'])[1 + g % 4] FROM generate_series(1, 200000) g;
INSERT INTO shop.orders (customer_id, status, total, created_at)
  SELECT 1 + (random() * 199999)::int,
         CASE WHEN g % 1000 = 0 THEN 'disputed' WHEN g % 10 = 0 THEN 'new' ELSE 'done' END,
         (random() * 1000)::numeric(12, 2),
         timestamptz '2025-01-01' + (random() * 700) * interval '1 day'
  FROM generate_series(1, 500000) g;
INSERT INTO shop.order_items (order_id, product_id, qty, price)
  SELECT 1 + (random() * 499999)::int, 1 + (random() * 4999)::int, 1 + (random() * 5)::int, (random() * 200)::numeric(12, 2)
  FROM generate_series(1, 1500000) g;
INSERT INTO shop.events (id, happened_at, kind, payload)
  SELECT g, timestamptz '2025-01-01' + (random() * 729) * interval '1 day', (ARRAY['view', 'cart', 'pay'])[1 + g % 3],
         jsonb_build_object('k', g % 100)
  FROM generate_series(1, 1000000) g;
INSERT INTO shop.audit (order_id, actor, note)
  SELECT CASE WHEN g % 100 = 0 THEN 1 + (random() * 499999)::int ELSE 1 END, 'app', repeat('x', 20)
  FROM generate_series(1, 200000) g;
INSERT INTO shop.balances SELECT id, 0 FROM shop.customers;
UPDATE shop.balances SET balance = balance + 1;
ANALYZE shop.customers, shop.orders, shop.order_items, shop.events;

GRANT USAGE ON SCHEMA shop TO glafira_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA shop TO glafira_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA shop TO glafira_app;
