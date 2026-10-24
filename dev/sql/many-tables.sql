-- Схема wide с 10 000 таблиц для проверки прокрутки и пагинации дерева (запускать вручную, в отдельной базе):
--   docker exec gp-pg-single createdb -U postgres wide_demo
--   docker exec -i gp-pg-single psql -U postgres -d wide_demo -v ON_ERROR_STOP=1 < dev/sql/many-tables.sql
-- Повторный запуск безопасен (CREATE TABLE IF NOT EXISTS), создание занимает около 10 секунд.
-- Убрать схему командой DROP SCHEMA wide CASCADE нельзя: на 10 000 таблиц не хватит блокировок
-- (max_locks_per_transaction); создавайте её в отдельной базе и удаляйте базу целиком (DROP DATABASE).

-- Без тысяч уведомлений «already exists, skipping» при повторном запуске.
SET client_min_messages = warning;

CREATE SCHEMA IF NOT EXISTS wide;

-- Все таблицы одной транзакцией не создать: каждая берёт AccessExclusiveLock (таблица, индекс первичного
-- ключа, TOAST и его индекс) до конца транзакции, и примерно на 2 000-й таблице при
-- max_locks_per_transaction = 64 приходит «out of shared memory». Цена одной таблицы в блокировках зависит
-- от версии (около 6 на PG 13, около 8 на PG 18), предел достигается между ~1 600 и ~2 000 таблицами;
-- пачка в 500 таблиц оставляет запас примерно в 3 раза. Поэтому COMMIT после каждых 500 таблиц:
-- блок DO управляет транзакцией, пока psql не обёрнут в одну (без флага -1 / --single-transaction).
DO $$
BEGIN
  FOR i IN 1..10000 LOOP
    EXECUTE format('CREATE TABLE IF NOT EXISTS wide.t_%s (id int PRIMARY KEY, v text)', lpad(i::text, 5, '0'));
    IF i % 500 = 0 THEN
      COMMIT;
    END IF;
  END LOOP;
END $$;
