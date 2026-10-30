-- Нагрузка pg-perf: «плохие» запросы для топа, эвристик и правил рекомендаций (спец. P3 §13.1).
-- Запуск: docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300
-- Переменные pgbench подставляются литералами (простой протокол), pgss нормализует их в $N: три точечных выбора ниже —
-- одна строка pgss (3 из 13 инструкций, ~23 % вызовов — находка N+1). Выражение `:cid + 1` прямо в запросе дало бы
-- другой текст (`id = $1 + $2`) и другую строку, поэтому соседние id считаются в \set.
\set cid random(1, 200000)
\set cid1 :cid + 1
\set cid2 :cid + 2
\set a random(1, 20)
\set pid random(1, 4990)
\set day random(0, 729)
-- N+1: точечный выбор покупателя — огромные calls при крошечном времени
SELECT * FROM shop.customers WHERE id = :cid;
SELECT * FROM shop.customers WHERE id = :cid1;
SELECT * FROM shop.customers WHERE id = :cid2;
-- Нестабильное время: у заказа 1 — 99 % строк audit, у остальных — единицы (stddev намного больше среднего). Диапазон
-- id держит оценку низкой (BitmapAnd, стоимость ниже jit_above_cost = 1000): JIT добавил бы к каждому вызову ~50 мс и
-- скрыл разницу, а пустой bitmap по order_id BitmapAnd завершает сразу
SELECT count(*) FROM shop.audit WHERE order_id = :a AND id BETWEEN 1 AND 300000;
-- Seq Scan с большим отсевом: status без индекса
SELECT count(*) FROM shop.orders WHERE status = 'disputed';
-- Nested Loop по неверной оценке: audit без статистики, 99 % строк у заказа 1
SELECT o.id, a.note FROM shop.audit a JOIN shop.orders o ON o.id = a.order_id WHERE a.actor = 'app' AND a.order_id = 1;
-- Sort под Limit без индекса и сортировка на диске (work_mem = 1MB)
SELECT * FROM shop.orders ORDER BY total DESC LIMIT 10;
SELECT customer_id, sum(total) FROM shop.orders GROUP BY customer_id ORDER BY 2 DESC;
-- Index Only Scan с heap fetches
SELECT count(*) FROM shop.balances WHERE customer_id BETWEEN :cid AND :cid + 1000;
-- SubPlan на строку: коррелированный подзапрос на ~3000 строк
SELECT oi.id FROM shop.order_items oi
 WHERE oi.product_id BETWEEN :pid AND :pid + 10
   AND oi.price > (SELECT avg(x.price) FROM shop.order_items x WHERE x.order_id = oi.order_id);
-- Append без pruning: фильтр не по ключу партиционирования
SELECT count(*) FROM shop.events WHERE kind = 'pay' AND payload ->> 'k' = '7';
-- Параллельная агрегация (воркеров меньше плана) и JIT
SELECT customer_id % 1000, sum(total * 1.07 + 3), avg(total) FROM shop.orders GROUP BY 1;
-- Temp-файлы и чтения с диска: хеш-соединение больших таблиц
SELECT count(*) FROM shop.order_items oi JOIN shop.orders o ON o.id = oi.order_id
 WHERE o.created_at >= timestamptz '2025-01-01' + :day * interval '1 day' - interval '30 days';
