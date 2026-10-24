//! Вкладка таблицы (спец. P1 §9): колонки, индексы, ограничения, размер и статистика, партиционирование, триггеры.

use serde::{Deserialize, Serialize};
use tokio_postgres::{Client, Row};

use crate::catalog::{ddl, object_not_found, relation_oid, server_version_num};
use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// `format_type` с модификатором: `numeric(12,2)`, `character varying(20)`.
    pub data_type: String,
    pub not_null: bool,
    /// Выражение `DEFAULT`, у генерируемой колонки — её выражение.
    pub default_expr: Option<String>,
    /// `a` — `GENERATED ALWAYS AS IDENTITY`, `d` — `BY DEFAULT`.
    pub identity: Option<String>,
    /// `s` — `GENERATED ALWAYS AS (…) STORED`.
    pub generated: Option<String>,
    /// Сопоставление, только если отличается от сопоставления типа.
    pub collation: Option<String>,
    /// Цель статистики; `None` — по умолчанию (на любой версии: до 17 в каталоге это -1).
    pub stat_target: Option<i32>,
    /// `attstorage`: `p`, `e`, `m`, `x`.
    pub storage: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub name: String,
    pub definition: String,
    pub primary: bool,
    pub unique: bool,
    pub valid: bool,
    /// У партиционированного индекса — сумма по его партициям.
    pub size_bytes: i64,
    pub idx_scan: Option<i64>,
    pub idx_tup_read: Option<i64>,
    /// Последнее сканирование (PostgreSQL 16+), миллисекунды эпохи.
    pub last_idx_scan_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Constraint {
    pub name: String,
    /// `contype`: `p`, `u`, `f`, `c`, `x`, `t`, `n` (NOT NULL — с PostgreSQL 18).
    pub kind: String,
    pub definition: String,
    pub validated: bool,
}

/// Внешний ключ другой таблицы, ссылающийся на эту.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingFk {
    pub constraint: String,
    pub schema: String,
    pub table: String,
    pub definition: String,
}

/// Размеры и статистика. У партиционированной таблицы — по всему дереву партиций (отступление 7 плана):
/// размеры и счётчики — суммы, времена — самые поздние, `reltuples` — сумма по листьям.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableStats {
    pub heap_bytes: i64,
    pub index_bytes: i64,
    pub toast_bytes: i64,
    pub total_bytes: i64,
    /// Оценка числа строк; -1 — таблицу ещё не анализировали (PostgreSQL 14+).
    pub reltuples: f64,
    pub seq_scan: Option<i64>,
    pub idx_scan: Option<i64>,
    pub n_live_tup: Option<i64>,
    pub n_dead_tup: Option<i64>,
    pub n_mod_since_analyze: Option<i64>,
    pub n_ins_since_vacuum: Option<i64>,
    pub last_vacuum_ms: Option<i64>,
    pub last_autovacuum_ms: Option<i64>,
    pub last_analyze_ms: Option<i64>,
    pub last_autoanalyze_ms: Option<i64>,
    pub vacuum_count: Option<i64>,
    pub autovacuum_count: Option<i64>,
    pub analyze_count: Option<i64>,
    pub autoanalyze_count: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Partitioning {
    /// Ключ партиционирования (`RANGE (happened_at)`) — у партиционированной таблицы.
    pub key: Option<String>,
    /// Границы (`FOR VALUES FROM (…) TO (…)`) — у партиции.
    pub bound: Option<String>,
    /// Родитель партиции (`schema.table`, с кавычками где нужно); у потомка по `INHERITS` — `None`.
    pub parent: Option<String>,
    /// Прямые партиции (`schema.table`).
    pub partitions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub name: String,
    /// `tgenabled`: `O`, `D`, `R`, `A`.
    pub enabled: String,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableDetails {
    pub oid: i64,
    pub schema: String,
    pub name: String,
    /// `r`, `p` или `f`.
    pub relkind: String,
    pub owner: String,
    pub tablespace: Option<String>,
    pub comment: Option<String>,
    pub reloptions: Vec<String>,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    pub constraints: Vec<Constraint>,
    pub incoming_fks: Vec<IncomingFk>,
    pub stats: TableStats,
    pub partitioning: Partitioning,
    pub triggers: Vec<Trigger>,
    /// DDL по каталогу (`catalog::ddl::table_ddl`); заполняется в [`get`].
    pub ddl: String,
}

/// Колонки отношения (`$1` — OID); общий запрос для таблиц и представлений.
pub(crate) const COLUMNS_SQL: &str = "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), \
    a.attnotnull, pg_get_expr(d.adbin, d.adrelid), NULLIF(a.attidentity::text, ''), \
    NULLIF(a.attgenerated::text, ''), \
    CASE WHEN a.attcollation <> 0 AND a.attcollation <> t.typcollation \
         THEN quote_ident(cn.nspname) || '.' || quote_ident(co.collname) END, \
    NULLIF(a.attstattarget::int, -1), a.attstorage::text, col_description(a.attrelid, a.attnum) \
  FROM pg_attribute a JOIN pg_type t ON t.oid = a.atttypid \
  LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
  LEFT JOIN pg_collation co ON co.oid = a.attcollation \
  LEFT JOIN pg_namespace cn ON cn.oid = co.collnamespace \
  WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum";

/// Индексы отношения (`$1` — OID). Статистика использования — `pg_stat_all_indexes`; `last_idx_scan`
/// появился в 16: на старых версиях — NULL, иначе запрос не разберётся. У партиционированного индекса
/// (`I`, у партиционированной таблицы) своего хранения и статистики нет: размер и счётчики — суммы по
/// его партициям (отступление 7 плана), время — самое позднее. Партиции индекса обходятся рекурсивно по
/// `pg_inherits` (только каталог), а не `pg_partition_tree`: тот (`find_all_inheritors`) берёт
/// `AccessShareLock` на каждого члена дерева до конца запроса, и на тысячах партиций с несколькими
/// индексами общая таблица блокировок переполнялась бы (`out of shared memory` у других сеансов).
/// `pg_relation_size` открывает отношение лишь на время подсчёта и для исчезнувшего вернёт NULL (`sum` его
/// пропустит). `CASE` обходит дерево только у `I`: у остальных индексов подзапрос не выполняется.
pub fn index_sql(version_num: i32) -> String {
    let last = if version_num >= 160000 {
        "(extract(epoch FROM max(s.last_idx_scan)) * 1000)::bigint"
    } else {
        "NULL::bigint"
    };
    format!(
        "SELECT ic.relname::text, COALESCE(pg_get_indexdef(i.indexrelid), ''), i.indisprimary, \
           i.indisunique, i.indisvalid, \
           (SELECT COALESCE(sum(pg_relation_size(r)), 0) FROM unnest(t.rels) r)::bigint, \
           x.scans, x.tup_read, x.last_ms \
         FROM pg_index i JOIN pg_class ic ON ic.oid = i.indexrelid \
         CROSS JOIN LATERAL (SELECT CASE WHEN ic.relkind = 'I' \
             THEN ARRAY(WITH RECURSIVE tree(relid) AS ( \
                      SELECT i.indexrelid \
                      UNION ALL \
                      SELECT h.inhrelid FROM pg_inherits h JOIN tree tr ON h.inhparent = tr.relid) \
                  SELECT relid FROM tree) \
             ELSE ARRAY[i.indexrelid] END AS rels) t \
         CROSS JOIN LATERAL (SELECT sum(s.idx_scan)::bigint AS scans, \
             sum(s.idx_tup_read)::bigint AS tup_read, {last} AS last_ms \
             FROM pg_stat_all_indexes s WHERE s.indexrelid = ANY (t.rels)) x \
         WHERE i.indrelid = $1 ORDER BY ic.relname"
    )
}

const CONSTRAINTS_SQL: &str = "SELECT conname::text, contype::text, \
    COALESCE(pg_get_constraintdef(oid, true), ''), convalidated \
  FROM pg_constraint WHERE conrelid = $1 ORDER BY contype, conname";

/// Внешние ключи, ссылающиеся на таблицу. Копии ключа партиционированной таблицы на её партициях
/// (`conparentid <> 0`) не повторяются: показывается ключ, который объявил пользователь.
const INCOMING_FK_SQL: &str = "SELECT c.conname::text, n.nspname::text, r.relname::text, \
    COALESCE(pg_get_constraintdef(c.oid, true), '') \
  FROM pg_constraint c JOIN pg_class r ON r.oid = c.conrelid JOIN pg_namespace n ON n.oid = r.relnamespace \
  WHERE c.contype = 'f' AND c.confrelid = $1 AND c.conparentid = 0 \
  ORDER BY n.nspname, r.relname, c.conname";

/// Заголовок, размеры и статистика (`$1` — OID). `t.rels` — отношения, по которым считаются размеры и
/// статистика: у партиционированной таблицы — всё дерево партиций вместе с самим родителем, у остальных —
/// только она сама. Дерево обходится рекурсивно по `pg_inherits` (только каталог), а не `pg_partition_tree`:
/// тот (`find_all_inheritors`) держит `AccessShareLock` на каждой партиции до конца запроса, и на тысячах
/// партиций у других сеансов кончалась бы общая таблица блокировок (`out of shared memory`). Размеры
/// (`pg_relation_size` и т. п.) открывают отношение лишь на время подсчёта, а для исчезнувшего вернут NULL
/// (`sum` его пропустит); `pg_stat_all_tables` и `pg_class` — каталог. `CASE` обходит дерево только у `p`:
/// у обычной таблицы подзапрос не выполняется (EXPLAIN ANALYZE: `never executed`), а у родителя по
/// `INHERITS` обход вернул бы и потомков.
///
/// У партиционированной таблицы своего хранения нет, и всё считается по дереву (отступление 7 плана):
/// размеры и счётчики `pg_stat_all_tables` — суммы, `last_*` — `max`. В суммы входит и строка самого
/// родителя, если у `p` она есть в `pg_stat_all_tables` (проверено на PostgreSQL 15). Поэтому ручной
/// `ANALYZE` одной партиции двигает `last_analyze_ms` родителя, а `analyze_count` и прочие счётчики
/// родителя — сумма по всему дереву, а не его собственные.
/// Вкладка открывает обычную (`r`), партиционированную (`p`) и внешнюю (`f`) таблицу.
const HEAD_SQL: &str = "SELECT n.nspname::text, c.relname::text, c.relkind::text, pg_get_userbyid(c.relowner)::text, \
    ts.spcname::text, obj_description(c.oid, 'pg_class'), COALESCE(c.reloptions, '{}'::text[]), \
    (CASE WHEN c.relkind = 'p' THEN COALESCE((SELECT sum(x.reltuples::float8) FROM pg_class x \
          WHERE x.oid = ANY (t.rels) AND x.relkind <> 'p' AND x.reltuples >= 0), -1) \
     ELSE c.reltuples END)::float8, \
    (SELECT COALESCE(sum(pg_relation_size(r)), 0) FROM unnest(t.rels) r)::bigint, \
    (SELECT COALESCE(sum(pg_indexes_size(r)), 0) FROM unnest(t.rels) r)::bigint, \
    (SELECT COALESCE(sum(pg_total_relation_size(x.reltoastrelid)), 0) FROM pg_class x \
       WHERE x.oid = ANY (t.rels) AND x.reltoastrelid <> 0)::bigint, \
    (SELECT COALESCE(sum(pg_total_relation_size(r)), 0) FROM unnest(t.rels) r)::bigint, \
    s.seq_scan, s.idx_scan, s.n_live_tup, s.n_dead_tup, s.n_mod_since_analyze, s.n_ins_since_vacuum, \
    (extract(epoch FROM s.last_vacuum) * 1000)::bigint, \
    (extract(epoch FROM s.last_autovacuum) * 1000)::bigint, \
    (extract(epoch FROM s.last_analyze) * 1000)::bigint, \
    (extract(epoch FROM s.last_autoanalyze) * 1000)::bigint, \
    s.vacuum_count, s.autovacuum_count, s.analyze_count, s.autoanalyze_count, \
    pg_get_partkeydef(c.oid), pg_get_expr(c.relpartbound, c.oid), \
    (SELECT quote_ident(pn.nspname) || '.' || quote_ident(p.relname) FROM pg_inherits i \
       JOIN pg_class p ON p.oid = i.inhparent JOIN pg_namespace pn ON pn.oid = p.relnamespace \
       WHERE c.relispartition AND i.inhrelid = c.oid LIMIT 1) \
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
  LEFT JOIN pg_tablespace ts ON ts.oid = c.reltablespace \
  CROSS JOIN LATERAL (SELECT CASE WHEN c.relkind = 'p' \
      THEN ARRAY(WITH RECURSIVE tree(relid) AS ( \
               SELECT c.oid \
               UNION ALL \
               SELECT h.inhrelid FROM pg_inherits h JOIN tree tr ON h.inhparent = tr.relid) \
           SELECT relid FROM tree) \
      ELSE ARRAY[c.oid] END AS rels) t \
  CROSS JOIN LATERAL (SELECT sum(st.seq_scan)::bigint AS seq_scan, sum(st.idx_scan)::bigint AS idx_scan, \
      sum(st.n_live_tup)::bigint AS n_live_tup, sum(st.n_dead_tup)::bigint AS n_dead_tup, \
      sum(st.n_mod_since_analyze)::bigint AS n_mod_since_analyze, \
      sum(st.n_ins_since_vacuum)::bigint AS n_ins_since_vacuum, \
      max(st.last_vacuum) AS last_vacuum, max(st.last_autovacuum) AS last_autovacuum, \
      max(st.last_analyze) AS last_analyze, max(st.last_autoanalyze) AS last_autoanalyze, \
      sum(st.vacuum_count)::bigint AS vacuum_count, sum(st.autovacuum_count)::bigint AS autovacuum_count, \
      sum(st.analyze_count)::bigint AS analyze_count, \
      sum(st.autoanalyze_count)::bigint AS autoanalyze_count \
      FROM pg_stat_all_tables st WHERE st.relid = ANY (t.rels)) s \
  WHERE c.oid = $1 AND c.relkind IN ('r', 'p', 'f')";

const PARTITIONS_SQL: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname) FROM pg_inherits i \
  JOIN pg_class c ON c.oid = i.inhrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
  WHERE i.inhparent = $1 ORDER BY n.nspname, c.relname";

const TRIGGERS_SQL: &str = "SELECT t.tgname::text, t.tgenabled::text, \
    COALESCE(pg_get_triggerdef(t.oid, true), '') \
  FROM pg_trigger t WHERE t.tgrelid = $1 AND NOT t.tgisinternal ORDER BY t.tgname";

/// Имена объектов партиции, которые PostgreSQL создал копиями объектов родителя (`PARTITION OF` создаёт их
/// сам, повторять их в DDL нельзя: два первичных ключа, одноимённый индекс, дубль внешнего ключа). Только
/// каталог, без блокировок отношений. Ограничения: `conparentid <> 0` (PRIMARY KEY, UNIQUE, FOREIGN KEY
/// родителя) или не `conislocal` (унаследованный CHECK). Индексы: `relispartition` (присоединён к
/// индексу родителя). Триггеры: `tgparentid <> 0`; до PostgreSQL 15 такие копии помечены внутренними
/// (`tgisinternal`) и в [`TRIGGERS_SQL`] не попадают, а столбец `tgparentid` на них нельзя читать наверняка.
/// `$1` — OID партиции.
pub fn inherited_sql(version_num: i32) -> String {
    let triggers = if version_num >= 150000 {
        "SELECT 't', tgname::text FROM pg_trigger \
         WHERE tgrelid = $1 AND NOT tgisinternal AND tgparentid <> 0"
    } else {
        "SELECT 't', NULL::text WHERE false"
    };
    format!(
        "SELECT 'c', conname::text FROM pg_constraint \
           WHERE conrelid = $1 AND (conparentid <> 0 OR NOT conislocal) \
         UNION ALL \
         SELECT 'i', ic.relname::text FROM pg_index i JOIN pg_class ic ON ic.oid = i.indexrelid \
           WHERE i.indrelid = $1 AND ic.relispartition \
         UNION ALL {triggers}"
    )
}

/// Унаследованные объекты партиции по виду: ограничения, индексы, триггеры.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Inherited {
    constraints: Vec<String>,
    indexes: Vec<String>,
    triggers: Vec<String>,
}

impl Inherited {
    fn from_rows(rows: &[Row]) -> Self {
        let mut inh = Inherited::default();
        for r in rows {
            let (kind, name): (String, String) = (r.get(0), r.get(1));
            match kind.as_str() {
                "c" => inh.constraints.push(name),
                "i" => inh.indexes.push(name),
                _ => inh.triggers.push(name),
            }
        }
        inh
    }
}

/// Копия описания без унаследованных объектов: из неё строится DDL партиции (вкладки по-прежнему
/// показывают все ограничения, индексы и триггеры).
pub(crate) fn without_inherited(t: &TableDetails, inh: &Inherited) -> TableDetails {
    let mut own = t.clone();
    own.constraints
        .retain(|k| !inh.constraints.contains(&k.name));
    own.indexes.retain(|i| !inh.indexes.contains(&i.name));
    own.triggers.retain(|x| !inh.triggers.contains(&x.name));
    own
}

pub(crate) fn column_from(r: &Row) -> Column {
    Column {
        name: r.get(0),
        data_type: r.get(1),
        not_null: r.get(2),
        default_expr: r.get(3),
        identity: r.get(4),
        generated: r.get(5),
        collation: r.get(6),
        stat_target: r.get(7),
        storage: r.get(8),
        comment: r.get(9),
    }
}

pub(crate) fn index_from(r: &Row) -> Index {
    Index {
        name: r.get(0),
        definition: r.get(1),
        primary: r.get(2),
        unique: r.get(3),
        valid: r.get(4),
        size_bytes: r.get(5),
        idx_scan: r.get(6),
        idx_tup_read: r.get(7),
        last_idx_scan_ms: r.get(8),
    }
}

/// Строки запроса с единственным параметром — OID отношения (`oid` в PostgreSQL — `u32`).
pub(crate) async fn rows(c: &Client, sql: &str, oid: u32) -> AppResult<Vec<Row>> {
    c.query(sql, &[&oid]).await.map_err(|e| from_pg(&e))
}

pub async fn get(c: &Client, schema: &str, name: &str) -> AppResult<TableDetails> {
    // OID из каталога всегда помещается в `u32`.
    let oid = relation_oid(c, schema, name).await? as u32;
    // Нет строки — отношение не таблица (представление, последовательность…) или исчезло между запросами.
    let h = c
        .query_opt(HEAD_SQL, &[&oid])
        .await
        .map_err(|e| from_pg(&e))?
        .ok_or_else(|| object_not_found(schema, name))?;
    let relkind: String = h.get(2);
    let version = server_version_num(c).await?;
    let columns = rows(c, COLUMNS_SQL, oid)
        .await?
        .iter()
        .map(column_from)
        .collect();
    let indexes = rows(c, &index_sql(version), oid)
        .await?
        .iter()
        .map(index_from)
        .collect();
    let constraints = rows(c, CONSTRAINTS_SQL, oid)
        .await?
        .iter()
        .map(|r| Constraint {
            name: r.get(0),
            kind: r.get(1),
            definition: r.get(2),
            validated: r.get(3),
        })
        .collect();
    let incoming_fks = rows(c, INCOMING_FK_SQL, oid)
        .await?
        .iter()
        .map(|r| IncomingFk {
            constraint: r.get(0),
            schema: r.get(1),
            table: r.get(2),
            definition: r.get(3),
        })
        .collect();
    let partitions = if relkind == "p" {
        rows(c, PARTITIONS_SQL, oid)
            .await?
            .iter()
            .map(|r| r.get::<_, String>(0))
            .collect()
    } else {
        Vec::new()
    };
    let triggers = rows(c, TRIGGERS_SQL, oid)
        .await?
        .iter()
        .map(|r| Trigger {
            name: r.get(0),
            enabled: r.get(1),
            definition: r.get(2),
        })
        .collect();
    let mut details = TableDetails {
        oid: i64::from(oid),
        schema: h.get(0),
        name: h.get(1),
        relkind,
        owner: h.get(3),
        tablespace: h.get(4),
        comment: h.get(5),
        reloptions: h.get(6),
        stats: TableStats {
            reltuples: h.get(7),
            heap_bytes: h.get(8),
            index_bytes: h.get(9),
            toast_bytes: h.get(10),
            total_bytes: h.get(11),
            seq_scan: h.get(12),
            idx_scan: h.get(13),
            n_live_tup: h.get(14),
            n_dead_tup: h.get(15),
            n_mod_since_analyze: h.get(16),
            n_ins_since_vacuum: h.get(17),
            last_vacuum_ms: h.get(18),
            last_autovacuum_ms: h.get(19),
            last_analyze_ms: h.get(20),
            last_autoanalyze_ms: h.get(21),
            vacuum_count: h.get(22),
            autovacuum_count: h.get(23),
            analyze_count: h.get(24),
            autoanalyze_count: h.get(25),
        },
        partitioning: Partitioning {
            key: h.get(26),
            bound: h.get(27),
            parent: h.get(28),
            partitions,
        },
        columns,
        indexes,
        constraints,
        incoming_fks,
        triggers,
        ddl: String::new(),
    };
    // DDL партиции не повторяет объекты, унаследованные от родителя.
    details.ddl = if details.partitioning.parent.is_some() {
        let inherited = rows(c, &inherited_sql(version), oid).await?;
        ddl::table_ddl(&without_inherited(
            &details,
            &Inherited::from_rows(&inherited),
        ))
    } else {
        ddl::table_ddl(&details)
    };
    Ok(details)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constraint(name: &str, kind: &str) -> Constraint {
        Constraint {
            name: name.into(),
            kind: kind.into(),
            definition: "X".into(),
            validated: true,
        }
    }

    fn index(name: &str) -> Index {
        Index {
            name: name.into(),
            definition: "Y".into(),
            primary: false,
            unique: false,
            valid: true,
            size_bytes: 0,
            idx_scan: None,
            idx_tup_read: None,
            last_idx_scan_ms: None,
        }
    }

    fn trigger(name: &str) -> Trigger {
        Trigger {
            name: name.into(),
            enabled: "O".into(),
            definition: "Z".into(),
        }
    }

    #[test]
    fn partition_ddl_input_drops_only_inherited_objects() {
        let t = TableDetails {
            oid: 1,
            schema: "shop".into(),
            name: "p1".into(),
            relkind: "r".into(),
            owner: "postgres".into(),
            tablespace: None,
            comment: None,
            reloptions: vec![],
            columns: vec![],
            indexes: vec![index("p1_pkey"), index("p1_own_idx")],
            constraints: vec![
                constraint("p1_pkey", "p"),
                constraint("par_chk", "c"),
                constraint("p1_own_chk", "c"),
            ],
            incoming_fks: vec![],
            stats: Default::default(),
            partitioning: Default::default(),
            triggers: vec![trigger("par_trg"), trigger("p1_own_trg")],
            ddl: String::new(),
        };
        let inherited = Inherited {
            constraints: vec!["p1_pkey".into(), "par_chk".into()],
            indexes: vec!["p1_pkey".into()],
            triggers: vec!["par_trg".into()],
        };
        let own = without_inherited(&t, &inherited);
        let names = |v: &[Constraint]| v.iter().map(|k| k.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&own.constraints), ["p1_own_chk"]);
        assert_eq!(
            own.indexes
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>(),
            ["p1_own_idx"]
        );
        assert_eq!(
            own.triggers
                .iter()
                .map(|x| x.name.as_str())
                .collect::<Vec<_>>(),
            ["p1_own_trg"]
        );
        // Описание вкладки не меняется: фильтр работает на копии.
        assert_eq!(t.constraints.len(), 3);
        assert_eq!(without_inherited(&t, &Inherited::default()), t);
    }

    #[test]
    fn inherited_trigger_copies_are_read_only_from_postgres_15() {
        assert!(inherited_sql(150000).contains("tgparentid"));
        assert!(inherited_sql(170000).contains("tgparentid"));
        assert!(!inherited_sql(140012).contains("tgparentid"));
        assert!(!inherited_sql(130000).contains("tgparentid"));
    }
}
