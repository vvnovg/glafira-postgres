-- Нагрузка pg-perf: «плохие» запросы для топа, эвристик и правил рекомендаций (спец. P3 §13.1).
-- Запуск: docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300
\set cid random(1, 200000)
\set oid random(1, 500000)
\set pid random(1, 4990)
\set day random(0, 729)
-- N+1: точечный выбор покупателя — огромные calls при крошечном времени
SELECT * FROM shop.customers WHERE id = :cid;
SELECT * FROM shop.customers WHERE id = :cid + 1;
SELECT * FROM shop.customers WHERE id = :cid + 2;
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
