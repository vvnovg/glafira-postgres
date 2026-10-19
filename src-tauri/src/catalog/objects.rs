use serde::{Deserialize, Serialize};
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Row};

use crate::catalog::{check_page, like_pattern};
use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Tables,
    Views,
    Matviews,
    Functions,
    Sequences,
}

impl ObjectKind {
    pub fn relkinds(self) -> Option<&'static [&'static str]> {
        match self {
            ObjectKind::Tables => Some(&["r", "p", "f"]),
            ObjectKind::Views => Some(&["v"]),
            ObjectKind::Matviews => Some(&["m"]),
            ObjectKind::Sequences => Some(&["S"]),
            ObjectKind::Functions => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectItem {
    pub oid: i64,
    pub name: String,
    pub schema: String,
    pub relkind: Option<String>,
    /// Для партиционированной таблицы — число партиций.
    pub partitions: Option<i64>,
    pub est_rows: Option<i64>,
    pub total_bytes: Option<i64>,
    /// Аргументы функции (`pg_get_function_identity_arguments`).
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPage {
    pub total: i64,
    pub items: Vec<ObjectItem>,
    pub next_offset: Option<i64>,
}

pub(crate) fn next_offset(offset: i64, got: usize, total: i64) -> Option<i64> {
    let end = offset + got as i64;
    (got > 0 && end < total).then_some(end)
}

/// Столбцы строки отношения; последний — `count(*) OVER ()` (общее число для страницы).
///
/// Партиционированная таблица (`p`): размер не считается (`NULL`, в дереве — «—»), оценка строк — сумма
/// `reltuples` листьев по `pg_inherits` (рекурсивно, только каталог). `pg_partition_tree` здесь нельзя:
/// он (`find_all_inheritors`) берёт `AccessShareLock` на каждую партицию до конца транзакции, и страница
/// с несколькими родителями на тысячи партиций исчерпала бы общую таблицу блокировок (`out of shared
/// memory` у других сеансов). Сумма `pg_total_relation_size` по партициям — те же тысячи открытий файлов на
/// строку списка. Оценка по `relpages` — другая величина (без индексов и TOAST, 0 до первого VACUUM/ANALYZE)
/// в одном столбце с точными размерами, поэтому размер родителя — «неизвестно». У обычной таблицы и
/// материализованного представления размер точный: `pg_total_relation_size` снимает блокировку каждого
/// отношения сразу после подсчёта (`relation_close`), они не копятся за запрос.
const REL_COLUMNS: &str = "c.oid::bigint, c.relname::text, n.nspname::text, c.relkind::text, \
    CASE WHEN c.relkind = 'p' THEN (SELECT count(*) FROM pg_inherits i WHERE i.inhparent = c.oid) END, \
    CASE WHEN c.relkind = 'p' THEN \
           (WITH RECURSIVE tree(relid) AS ( \
                SELECT i.inhrelid FROM pg_inherits i WHERE i.inhparent = c.oid \
                UNION ALL \
                SELECT i.inhrelid FROM pg_inherits i JOIN tree t ON i.inhparent = t.relid) \
            SELECT sum(l.reltuples::bigint) FROM tree t JOIN pg_class l ON l.oid = t.relid \
            WHERE l.relkind <> 'p' AND l.reltuples >= 0)::bigint \
         WHEN c.relkind IN ('r', 'm') AND c.reltuples >= 0 THEN c.reltuples::bigint END, \
    CASE WHEN c.relkind IN ('r', 'm') THEN pg_total_relation_size(c.oid) END, \
    count(*) OVER ()";

/// `FROM … WHERE …` списка отношений схемы (`$1` — схема, `$2` — `relkind`, `$3` — шаблон `ILIKE`).
/// Имена в каталоге имеют тип `name` (сопоставление C): без приведения к `text` с сопоставлением по
/// умолчанию `ILIKE` не сворачивает регистр букв не из ASCII (`Заказы` не находится по `заказ`).
const REL_FROM_WHERE: &str = "FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
    WHERE n.nspname = $1 AND c.relkind::text = ANY($2) AND NOT c.relispartition \
      AND ($3::text IS NULL OR c.relname::text COLLATE \"default\" ILIKE $3)";

/// `FROM … WHERE …` партиций таблицы (`$1` — схема, `$2` — имя родителя). Только настоящие партиции:
/// потомки по наследованию (`INHERITS`) в список не входят.
const PARTITION_FROM_WHERE: &str = "FROM pg_inherits inh \
    JOIN pg_class c ON c.oid = inh.inhrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
    WHERE c.relispartition \
      AND inh.inhparent = (SELECT pc.oid FROM pg_class pc JOIN pg_namespace pn ON pn.oid = pc.relnamespace \
                           WHERE pn.nspname = $1 AND pc.relname = $2)";

const FUNC_COLUMNS: &str = "p.oid::bigint, p.proname::text, n.nspname::text, p.prokind::text, \
    pg_get_function_identity_arguments(p.oid), count(*) OVER ()";

/// `FROM … WHERE …` функций и процедур схемы (`$1` — схема, `$2` — шаблон `ILIKE`).
const FUNC_FROM_WHERE: &str = "FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
    WHERE n.nspname = $1 AND p.prokind IN ('f', 'p') \
      AND ($2::text IS NULL OR p.proname::text COLLATE \"default\" ILIKE $2)";

fn rel_item(r: &Row) -> ObjectItem {
    ObjectItem {
        oid: r.get(0),
        name: r.get(1),
        schema: r.get(2),
        relkind: r.get(3),
        partitions: r.get(4),
        est_rows: r.get(5),
        total_bytes: r.get(6),
        signature: None,
    }
}

fn func_item(r: &Row) -> ObjectItem {
    ObjectItem {
        oid: r.get(0),
        name: r.get(1),
        schema: r.get(2),
        relkind: r.get(3),
        partitions: None,
        est_rows: None,
        total_bytes: None,
        signature: r.get(4),
    }
}

type Param<'a> = &'a (dyn ToSql + Sync);

/// Страница списка вместе с общим числом. Страница и счёт строятся из одного `from_where`
/// (`params` — его параметры `$1…$n`); `limit` и `offset` встают следом. `columns` кончается на
/// `count(*) OVER ()`: общее число приходит со строками страницы. Если страница пуста при ненулевом
/// сдвиге (список стал короче, чем ждал вызывающий), окно считать не из чего — число берётся отдельным
/// запросом `count(*)`, а продолжения (`next_offset`) нет.
async fn paged(
    c: &Client,
    (columns, from_where, order_by): (&str, &str, &str),
    params: &[Param<'_>],
    (offset, limit): (i64, i64),
    item: fn(&Row) -> ObjectItem,
) -> AppResult<ObjectPage> {
    let n = params.len();
    let sql = format!(
        "SELECT {columns} {from_where} ORDER BY {order_by} LIMIT ${} OFFSET ${}",
        n + 1,
        n + 2
    );
    let mut all = params.to_vec();
    all.push(&limit);
    all.push(&offset);
    let rows = c.query(&sql, &all).await.map_err(|e| from_pg(&e))?;
    let items: Vec<ObjectItem> = rows.iter().map(item).collect();
    let total = match rows.first() {
        Some(r) => r.get::<_, i64>(r.len() - 1),
        None if offset > 0 => c
            .query_one(&format!("SELECT count(*) {from_where}"), params)
            .await
            .map_err(|e| from_pg(&e))?
            .get(0),
        None => 0,
    };
    let next_offset = next_offset(offset, items.len(), total);
    Ok(ObjectPage {
        total,
        items,
        next_offset,
    })
}

pub async fn list(
    c: &Client,
    schema: &str,
    kind: ObjectKind,
    filter: Option<&str>,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    check_page(offset, limit)?;
    let pattern = like_pattern(filter);
    match kind.relkinds() {
        Some(relkinds) => {
            paged(
                c,
                (REL_COLUMNS, REL_FROM_WHERE, "c.relname"),
                &[&schema, &relkinds, &pattern],
                (offset, limit),
                rel_item,
            )
            .await
        }
        None => {
            paged(
                c,
                (FUNC_COLUMNS, FUNC_FROM_WHERE, "p.proname, 5"),
                &[&schema, &pattern],
                (offset, limit),
                func_item,
            )
            .await
        }
    }
}

/// Партиции таблицы (могут лежать в другой схеме — поле `schema`).
pub async fn partitions(
    c: &Client,
    schema: &str,
    table: &str,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    check_page(offset, limit)?;
    paged(
        c,
        (REL_COLUMNS, PARTITION_FROM_WHERE, "n.nspname, c.relname"),
        &[&schema, &table],
        (offset, limit),
        rel_item,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_offset_only_when_more_rows() {
        assert_eq!(next_offset(0, 2, 3), Some(2));
        assert_eq!(next_offset(2, 1, 3), None);
        assert_eq!(next_offset(0, 0, 0), None);
    }

    #[test]
    fn kinds_have_relkinds() {
        assert_eq!(ObjectKind::Tables.relkinds(), Some(&["r", "p", "f"][..]));
        assert_eq!(ObjectKind::Sequences.relkinds(), Some(&["S"][..]));
        assert_eq!(ObjectKind::Functions.relkinds(), None);
        let k: ObjectKind = serde_json::from_str("\"matviews\"").unwrap();
        assert_eq!(k, ObjectKind::Matviews);
    }
}
