//! Генератор DDL по каталогу (спец. P1 §9): CREATE TABLE с колонками, ограничениями, партиционированием,
//! индексами, триггерами и комментариями. Не замена pg_dump: без прав, политик RLS и владельцев.
//!
//! Вход — только [`TableDetails`] (определения ограничений, индексов и триггеров уже собраны сервером:
//! `pg_get_constraintdef`, `pg_get_indexdef`, `pg_get_triggerdef`), так что результат можно проверять
//! снимками без стенда. Порядок инструкций: заголовок, `CREATE TABLE`, строка `-- partitions:`, индексы,
//! `ALTER TABLE … ADD CONSTRAINT` (внешние ключи и непроверенные ограничения), триггеры, комментарии.
//! Результат пригоден для PostgreSQL 13–18.

use super::table::{Column, Constraint, RelationRef, TableDetails, Trigger};

pub const DDL_HEADER: &str = "-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)";

/// Слова, которые `quote_ident()` сервера берёт в кавычки (зарезервированные, «типо-функциональные» и
/// «колоночные» — все, кроме `UNRESERVED_KEYWORD`). Снято с `pg_get_keywords() WHERE catcode <> 'U'` на
/// PostgreSQL 15, 16 и 17 (объединение). Лишние слова безвредны: кавычки вокруг идентификатора допустимы
/// всегда; интеграционный тест `quote_ident_matches_the_server` сверяет список с работающим сервером.
#[rustfmt::skip]
const QUOTED_KEYWORDS: &[&str] = &[
    "all", "analyse", "analyze", "and", "any", "array", "as", "asc", "asymmetric", "authorization",
    "between", "bigint", "binary", "bit", "boolean", "both", "case", "cast", "char", "character",
    "check", "coalesce", "collate", "collation", "column", "concurrently", "constraint", "create",
    "cross", "current_catalog", "current_date", "current_role", "current_schema", "current_time",
    "current_timestamp", "current_user", "dec", "decimal", "default", "deferrable", "desc", "distinct",
    "do", "else", "end", "except", "exists", "extract", "false", "fetch", "float", "for", "foreign",
    "freeze", "from", "full", "grant", "greatest", "group", "grouping", "having", "ilike", "in",
    "initially", "inner", "inout", "int", "integer", "intersect", "interval", "into", "is", "isnull",
    "join", "json", "json_array", "json_arrayagg", "json_exists", "json_object", "json_objectagg",
    "json_query", "json_scalar", "json_serialize", "json_table", "json_value", "lateral", "leading",
    "least", "left", "like", "limit", "localtime", "localtimestamp", "merge_action", "national",
    "natural", "nchar", "none", "normalize", "not", "notnull", "null", "nullif", "numeric", "offset",
    "on", "only", "or", "order", "out", "outer", "overlaps", "overlay", "placing", "position",
    "precision", "primary", "real", "references", "returning", "right", "row", "select", "session_user",
    "setof", "similar", "smallint", "some", "substring", "symmetric", "system_user", "table",
    "tablesample", "then", "time", "timestamp", "to", "trailing", "treat", "trim", "true", "union",
    "unique", "user", "using", "values", "varchar", "variadic", "verbose", "when", "where", "window",
    "with", "xmlattributes", "xmlconcat", "xmlelement", "xmlexists", "xmlforest", "xmlnamespaces",
    "xmlparse", "xmlpi", "xmlroot", "xmlserialize", "xmltable",
];

/// Идентификатор в кавычках, если он не простой (`[a-z_][a-z0-9_]*`) или совпадает с ключевым словом, —
/// как `quote_ident()` PostgreSQL.
pub fn quote_ident(s: &str) -> String {
    let simple = s
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !QUOTED_KEYWORDS.contains(&s);
    if simple {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

/// Строковый литерал SQL. С обратной косой чертой — `E'…'` с удвоенными `\\`: обычный литерал при
/// `standard_conforming_strings = off` читал бы `\n` как перевод строки, и результат зависел бы от настройки.
fn literal(s: &str) -> String {
    let quoted = s.replace('\'', "''");
    if s.contains('\\') {
        format!("E'{}'", quoted.replace('\\', "\\\\"))
    } else {
        format!("'{quoted}'")
    }
}

/// Текст для строки-комментария `--`: перевод строки (и `\r`, который лексер PostgreSQL тоже считает концом
/// комментария) и другие управляющие символы заменяются пробелом. Идентификатор вправе содержать `\n`, а
/// `quote_ident` его сохраняет: без замены остаток имени после `\n` выполнился бы как отдельная инструкция.
fn comment_text(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn column_def(c: &Column) -> String {
    let mut parts = vec![quote_ident(&c.name), c.data_type.clone()];
    if let Some(coll) = &c.collation {
        parts.push(format!("COLLATE {coll}"));
    }
    match (
        c.generated.as_deref(),
        c.identity.as_deref(),
        &c.default_expr,
    ) {
        (Some("s"), _, Some(expr)) => parts.push(format!("GENERATED ALWAYS AS ({expr}) STORED")),
        // PostgreSQL 18: виртуальная генерируемая колонка.
        (Some("v"), _, Some(expr)) => parts.push(format!("GENERATED ALWAYS AS ({expr}) VIRTUAL")),
        (_, Some("a"), _) => parts.push("GENERATED ALWAYS AS IDENTITY".into()),
        (_, Some("d"), _) => parts.push("GENERATED BY DEFAULT AS IDENTITY".into()),
        (_, _, Some(expr)) => parts.push(format!("DEFAULT {expr}")),
        _ => {}
    }
    // У генерируемой колонки NOT NULL допустим (`... STORED NOT NULL`), и каталог его хранит.
    if c.not_null {
        parts.push("NOT NULL".into());
    }
    parts.join(" ")
}

/// Ограничение, которое пишется в `CREATE TABLE`. Не пишутся: `n` (NOT NULL, с PostgreSQL 18 живёт в
/// `pg_constraint`, но уже напечатан у колонки), `t` (constraint trigger: `pg_get_constraintdef` отдаёт для
/// него одно слово «TRIGGER», сам триггер выводится вместе с триггерами) и ограничения без определения.
fn is_table_constraint(k: &Constraint) -> bool {
    !matches!(k.kind.as_str(), "n" | "t") && !k.definition.is_empty()
}

/// Определение ограничения для DDL. `pg_get_constraintdef` сам дописывает NOT VALID (а в PostgreSQL 18 —
/// NOT ENFORCED), повторять нельзя.
fn constraint_body(k: &Constraint) -> String {
    let def = k.definition.trim_end();
    if !k.validated && !def.ends_with("NOT VALID") && !def.ends_with("NOT ENFORCED") {
        format!("{def} NOT VALID")
    } else {
        def.to_string()
    }
}

/// Ограничение пишется отдельным `ALTER TABLE … ADD CONSTRAINT` после индексов (порядок `pg_dump`), если это
/// внешний ключ или оно непроверенное. `NOT VALID` внутри `CREATE TABLE` PostgreSQL принимает, но молча
/// игнорирует (ограничение становилось бы проверенным); внешнему ключу нужен уже созданный уникальный индекс
/// (в том числе у самой таблицы при ссылке на себя). `NOT ENFORCED` (PostgreSQL 18) в `CREATE TABLE`
/// учитывается, поэтому остаётся внутри.
fn is_added_later(k: &Constraint) -> bool {
    k.kind == "f" || constraint_body(k).ends_with("NOT VALID")
}

fn constraint_def(k: &Constraint) -> String {
    format!("CONSTRAINT {} {}", quote_ident(&k.name), constraint_body(k))
}

/// Слово перед `TABLE` в `CREATE`: у нежурналируемой таблицы (в Patroni она не реплицируется) — `UNLOGGED `.
/// У партиционированной таблицы слово опускается: данных у неё нет, а PostgreSQL 18 такой `CREATE` отвергает.
fn persistence(t: &TableDetails) -> &'static str {
    if t.unlogged && t.relkind != "p" {
        "UNLOGGED "
    } else {
        ""
    }
}

/// `pg_get_indexdef` печатает у индекса партиционированной таблицы `CREATE INDEX … ON ONLY схема.таблица`
/// (индекс только на самой таблице, без партиций); в DDL нужен обычный `ON`, чтобы индекс создался и на
/// партициях. Разбирается только начало определения, чтобы не задеть ` ON ONLY ` внутри имени индекса.
fn index_on_all_partitions(def: &str) -> String {
    let Some(rest) = def
        .strip_prefix("CREATE UNIQUE INDEX ")
        .or_else(|| def.strip_prefix("CREATE INDEX "))
    else {
        return def.to_string();
    };
    // Имя индекса: в кавычках (кавычка внутри удваивается) или до первого пробела.
    let name_len = if rest.starts_with('"') {
        let bytes = rest.as_bytes();
        let mut i = 1;
        loop {
            match bytes.get(i) {
                None => return def.to_string(),
                Some(b'"') if bytes.get(i + 1) == Some(&b'"') => i += 2,
                Some(b'"') => break i + 1,
                Some(_) => i += 1,
            }
        }
    } else {
        rest.find(' ').unwrap_or(rest.len())
    };
    let (name, tail) = rest.split_at(name_len);
    match tail.strip_prefix(" ON ONLY ") {
        Some(after) => format!("{}{name} ON {after}", &def[..def.len() - rest.len()]),
        None => def.to_string(),
    }
}

/// `ALTER TABLE … {DISABLE | ENABLE REPLICA | ENABLE ALWAYS} TRIGGER …` для триггера в нестандартном
/// состоянии. `pg_get_triggerdef` состояния не передаёт: выключенный (или «только реплика») триггер без
/// этого включился бы при выполнении DDL. У включённого по умолчанию (`O`) строки нет.
fn trigger_state_stmt(qname: &str, x: &Trigger) -> Option<String> {
    let name = quote_ident(&x.name);
    match x.enabled.as_str() {
        "D" => Some(format!("ALTER TABLE {qname} DISABLE TRIGGER {name};")),
        "R" => Some(format!(
            "ALTER TABLE {qname} ENABLE REPLICA TRIGGER {name};"
        )),
        "A" => Some(format!("ALTER TABLE {qname} ENABLE ALWAYS TRIGGER {name};")),
        _ => None,
    }
}

pub fn table_ddl(t: &TableDetails) -> String {
    build_ddl(t, &[])
}

/// DDL партиции, из описания которой уже убраны унаследованные объекты (`table::without_inherited`).
/// `inherited_triggers` — убранные копии триггеров родителя: `PARTITION OF` создаёт их сам, но выключенную
/// (или «только реплика») копию пришлось бы снова переключить, поэтому для них пишутся строки состояния.
pub(crate) fn partition_ddl(t: &TableDetails, inherited_triggers: &[Trigger]) -> String {
    build_ddl(t, inherited_triggers)
}

/// `schema.name` с кавычками там, где они нужны (как `quote_ident` сервера); каталог отдаёт имена сырыми.
fn qualified(r: &RelationRef) -> String {
    format!("{}.{}", quote_ident(&r.schema), quote_ident(&r.name))
}

fn build_ddl(t: &TableDetails, inherited_triggers: &[Trigger]) -> String {
    let qname = format!("{}.{}", quote_ident(&t.schema), quote_ident(&t.name));
    let mut out = format!("{DDL_HEADER}\n");
    // Внешней таблице нужны сервер и параметры (`SERVER …`, `OPTIONS (…)`), каталог вкладки их не читает:
    // вместо неполного и невыполнимого оператора — явная заглушка.
    if t.relkind == "f" {
        out.push_str(&format!(
            "-- CREATE FOREIGN TABLE {} is not generated: the foreign server and its options are not read from the catalog\n",
            comment_text(&qname)
        ));
        return out;
    }
    let constraints: Vec<&Constraint> = t
        .constraints
        .iter()
        .filter(|k| is_table_constraint(k))
        .collect();
    // Внутри `CREATE TABLE` — проверенные PK, UNIQUE, EXCLUDE, CHECK; остальные — после индексов.
    let (later, inline): (Vec<&Constraint>, Vec<&Constraint>) =
        constraints.iter().partition(|k| is_added_later(k));
    let partition_of = t
        .partitioning
        .parent
        .as_ref()
        .zip(t.partitioning.bound.as_ref());
    match partition_of {
        Some((parent, bound)) => {
            out.push_str(&format!(
                "CREATE {}TABLE {qname} PARTITION OF {}\n",
                persistence(t),
                qualified(parent)
            ));
            // Колонки партиции — колонки родителя; здесь только её собственные ограничения (унаследованные
            // отбирает `table::get`, см. `without_inherited`).
            if !inline.is_empty() {
                let own: Vec<String> = inline.iter().map(|k| constraint_def(k)).collect();
                out.push_str(&format!("(\n    {}\n)\n", own.join(",\n    ")));
            }
            out.push_str(&format!("    {bound}"));
        }
        None => {
            let mut items: Vec<String> = t.columns.iter().map(column_def).collect();
            items.extend(inline.iter().map(|k| constraint_def(k)));
            let unlogged = persistence(t);
            if items.is_empty() {
                out.push_str(&format!("CREATE {unlogged}TABLE {qname} ()"));
            } else {
                out.push_str(&format!(
                    "CREATE {unlogged}TABLE {qname} (\n    {}\n)",
                    items.join(",\n    ")
                ));
            }
        }
    }
    // Ключ есть и у партиционированной таблицы, и у партиции, которая сама делится дальше.
    if let Some(key) = &t.partitioning.key {
        out.push_str(&format!(" PARTITION BY {key}"));
    }
    if !t.reloptions.is_empty() {
        out.push_str(&format!(" WITH ({})", t.reloptions.join(", ")));
    }
    if let Some(ts) = &t.tablespace {
        out.push_str(&format!(" TABLESPACE {}", quote_ident(ts)));
    }
    out.push_str(";\n");
    if !t.partitioning.partitions.is_empty() {
        out.push_str(&format!(
            "-- partitions: {}\n",
            comment_text(
                &t.partitioning
                    .partitions
                    .iter()
                    .map(qualified)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        ));
    }
    // Индексы ограничений (PK, UNIQUE, EXCLUDE) уже созданы ограничениями; у ограничения и индекса имя одно.
    let constraint_indexes: Vec<&str> = constraints
        .iter()
        .filter(|k| matches!(k.kind.as_str(), "p" | "u" | "x"))
        .map(|k| k.name.as_str())
        .collect();
    let indexes: Vec<String> = t
        .indexes
        .iter()
        .filter(|i| !i.definition.is_empty() && !constraint_indexes.contains(&i.name.as_str()))
        .map(|i| {
            let definition = if t.relkind == "p" {
                index_on_all_partitions(&i.definition)
            } else {
                i.definition.clone()
            };
            format!("{definition};{}", if i.valid { "" } else { " -- INVALID" })
        })
        .collect();
    if !indexes.is_empty() {
        out.push_str(&format!("\n{}\n", indexes.join("\n")));
    }
    let added_later: Vec<String> = later
        .iter()
        .map(|k| format!("ALTER TABLE {qname} ADD {};", constraint_def(k)))
        .collect();
    if !added_later.is_empty() {
        out.push_str(&format!("\n{}\n", added_later.join("\n")));
    }
    let mut triggers = Vec::new();
    for x in t.triggers.iter().filter(|x| !x.definition.is_empty()) {
        triggers.push(format!("{};", x.definition));
        triggers.extend(trigger_state_stmt(&qname, x));
    }
    // Копии унаследованных триггеров партиция получает сама, но их нестандартное состояние — своё.
    triggers.extend(
        inherited_triggers
            .iter()
            .filter_map(|x| trigger_state_stmt(&qname, x)),
    );
    if !triggers.is_empty() {
        out.push_str(&format!("\n{}\n", triggers.join("\n")));
    }
    let mut comments = Vec::new();
    if let Some(c) = &t.comment {
        comments.push(format!("COMMENT ON TABLE {qname} IS {};", literal(c)));
    }
    for c in &t.columns {
        if let Some(text) = &c.comment {
            comments.push(format!(
                "COMMENT ON COLUMN {qname}.{} IS {};",
                quote_ident(&c.name),
                literal(text)
            ));
        }
    }
    if !comments.is_empty() {
        out.push_str(&format!("\n{}\n", comments.join("\n")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::table::{Index, Partitioning, RelationRef, Trigger};

    fn table(name: &str) -> TableDetails {
        TableDetails {
            oid: 1,
            schema: "shop".into(),
            name: name.into(),
            relkind: "r".into(),
            unlogged: false,
            owner: "postgres".into(),
            tablespace: None,
            comment: None,
            reloptions: vec![],
            columns: vec![],
            indexes: vec![],
            constraints: vec![],
            incoming_fks: vec![],
            stats: Default::default(),
            partitioning: Default::default(),
            triggers: vec![],
            ddl: String::new(),
        }
    }

    /// Сырое имя отношения, как его отдаёт каталог (без кавычек).
    fn rel(schema: &str, name: &str) -> RelationRef {
        RelationRef {
            schema: schema.into(),
            name: name.into(),
        }
    }

    fn column(name: &str, data_type: &str) -> Column {
        Column {
            name: name.into(),
            data_type: data_type.into(),
            not_null: false,
            default_expr: None,
            identity: None,
            generated: None,
            collation: None,
            stat_target: None,
            storage: "p".into(),
            comment: None,
        }
    }

    fn constraint(name: &str, kind: &str, definition: &str, validated: bool) -> Constraint {
        Constraint {
            name: name.into(),
            kind: kind.into(),
            definition: definition.into(),
            validated,
        }
    }

    fn index(name: &str, definition: &str, valid: bool) -> Index {
        Index {
            name: name.into(),
            definition: definition.into(),
            primary: false,
            unique: false,
            valid,
            size_bytes: 0,
            idx_scan: None,
            idx_tup_read: None,
            last_idx_scan_ms: None,
        }
    }

    #[test]
    fn minimal_table() {
        let mut t = table("t");
        t.columns = vec![column("id", "integer"), column("note", "text")];
        assert_eq!(
            table_ddl(&t),
            format!("{DDL_HEADER}\nCREATE TABLE shop.t (\n    id integer,\n    note text\n);\n")
        );
    }

    #[test]
    fn columns_with_defaults_identity_generated_collation_and_not_null() {
        let mut t = table("t");
        let mut id = column("id", "bigint");
        id.not_null = true;
        id.identity = Some("a".into());
        let mut total = column("total", "numeric(12,2)");
        total.not_null = true;
        total.default_expr = Some("0".into());
        let mut tax = column("tax", "numeric(12,2)");
        tax.generated = Some("s".into());
        tax.default_expr = Some("(total * 1.2)".into());
        let mut name = column("name", "text");
        name.collation = Some("pg_catalog.\"C\"".into());
        t.columns = vec![id, total, tax, name];
        let ddl = table_ddl(&t);
        assert!(ddl.contains("    id bigint GENERATED ALWAYS AS IDENTITY NOT NULL,\n"));
        assert!(ddl.contains("    total numeric(12,2) DEFAULT 0 NOT NULL,\n"));
        assert!(
            ddl.contains("    tax numeric(12,2) GENERATED ALWAYS AS ((total * 1.2)) STORED,\n")
        );
        assert!(ddl.contains("    name text COLLATE pg_catalog.\"C\"\n"));
    }

    #[test]
    fn constraints_indexes_triggers_comments_and_options() {
        let mut t = table("orders");
        t.columns = vec![column("id", "bigint"), column("customer_id", "bigint")];
        t.constraints = vec![
            Constraint {
                name: "orders_pkey".into(),
                kind: "p".into(),
                definition: "PRIMARY KEY (id)".into(),
                validated: true,
            },
            Constraint {
                name: "orders_customer_id_fkey".into(),
                kind: "f".into(),
                definition: "FOREIGN KEY (customer_id) REFERENCES shop.customers(id)".into(),
                validated: false,
            },
        ];
        t.indexes = vec![
            Index {
                name: "orders_pkey".into(),
                definition: "CREATE UNIQUE INDEX orders_pkey ON shop.orders USING btree (id)"
                    .into(),
                primary: true,
                unique: true,
                valid: true,
                size_bytes: 0,
                idx_scan: None,
                idx_tup_read: None,
                last_idx_scan_ms: None,
            },
            Index {
                name: "orders_customer_idx".into(),
                definition:
                    "CREATE INDEX orders_customer_idx ON shop.orders USING btree (customer_id)"
                        .into(),
                primary: false,
                unique: false,
                valid: true,
                size_bytes: 0,
                idx_scan: None,
                idx_tup_read: None,
                last_idx_scan_ms: None,
            },
        ];
        t.triggers = vec![Trigger {
            name: "touch".into(),
            enabled: "O".into(),
            definition: "CREATE TRIGGER touch BEFORE INSERT ON shop.orders FOR EACH ROW EXECUTE FUNCTION shop.touch()".into(),
        }];
        t.reloptions = vec!["fillfactor=90".into(), "autovacuum_enabled=false".into()];
        t.comment = Some("Заказы 'клиентов'".into());
        t.columns[0].comment = Some("id".into());
        let ddl = table_ddl(&t);
        assert_eq!(
            ddl,
            format!(
                "{DDL_HEADER}\nCREATE TABLE shop.orders (\n    id bigint,\n    customer_id bigint,\n    CONSTRAINT orders_pkey PRIMARY KEY (id)\n) WITH (fillfactor=90, autovacuum_enabled=false);\n\nCREATE INDEX orders_customer_idx ON shop.orders USING btree (customer_id);\n\nALTER TABLE shop.orders ADD CONSTRAINT orders_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES shop.customers(id) NOT VALID;\n\nCREATE TRIGGER touch BEFORE INSERT ON shop.orders FOR EACH ROW EXECUTE FUNCTION shop.touch();\n\nCOMMENT ON TABLE shop.orders IS 'Заказы ''клиентов''';\nCOMMENT ON COLUMN shop.orders.id IS 'id';\n"
            )
        );
    }

    #[test]
    fn partitioned_parent_and_partition() {
        let mut parent = table("events");
        parent.relkind = "p".into();
        parent.columns = vec![
            column("id", "bigint"),
            column("happened_at", "timestamp with time zone"),
        ];
        parent.partitioning = Partitioning {
            key: Some("RANGE (happened_at)".into()),
            bound: None,
            parent: None,
            partitions: vec![rel("shop", "events_2026_09")],
        };
        let ddl = table_ddl(&parent);
        assert!(ddl.contains(") PARTITION BY RANGE (happened_at);\n"));
        assert!(ddl.contains("-- partitions: shop.events_2026_09\n"));
        let mut part = table("events_2026_09");
        part.partitioning = Partitioning {
            key: None,
            bound: Some("FOR VALUES FROM ('2026-09-01') TO ('2026-10-01')".into()),
            parent: Some(rel("shop", "events")),
            partitions: vec![],
        };
        assert!(table_ddl(&part).contains("CREATE TABLE shop.events_2026_09 PARTITION OF shop.events\n    FOR VALUES FROM ('2026-09-01') TO ('2026-10-01');\n"));
    }

    #[test]
    fn parent_and_partition_references_are_quoted_from_raw_names() {
        // Каталог отдаёт сырые имена; кавычки ставит только генератор DDL (тем же правилом, что и для самой таблицы).
        let mut part = table("Events 1");
        part.schema = "Shop".into();
        part.partitioning = Partitioning {
            key: Some("LIST (k)".into()),
            bound: Some("DEFAULT".into()),
            parent: Some(rel("My Schema", "select")),
            partitions: vec![rel("Shop", "Events 1a"), rel("a\"b", "c")],
        };
        assert_eq!(
            table_ddl(&part),
            format!(
                "{DDL_HEADER}\nCREATE TABLE \"Shop\".\"Events 1\" PARTITION OF \"My Schema\".\"select\"\n    DEFAULT PARTITION BY LIST (k);\n-- partitions: \"Shop\".\"Events 1a\", \"a\"\"b\".c\n"
            )
        );
    }

    #[test]
    fn identifiers_are_quoted_when_needed() {
        let mut t = table("Order Items");
        t.schema = "Shop".into();
        t.columns = vec![column("select", "text"), column("ok_name", "text")];
        let ddl = table_ddl(&t);
        assert!(ddl.contains(
            "CREATE TABLE \"Shop\".\"Order Items\" (\n    \"select\" text,\n    ok_name text\n);"
        ));
    }

    // Ниже — тесты правил, которых нет в брифе: данные настоящего каталога (PG 13–18).

    #[test]
    fn quote_ident_follows_server_rules() {
        for ok in [
            "a",
            "_a",
            "a1_b",
            "orders",
            "events_2026_09",
            "keyword_like",
        ] {
            assert_eq!(quote_ident(ok), ok);
        }
        // Нижний регистр, цифра в начале, `$`, пробел, кириллица, пустая строка — в кавычки.
        for bad in ["A", "1a", "a$b", "a b", "заказы", "", "Shop"] {
            assert_eq!(quote_ident(bad), format!("\"{bad}\""));
        }
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        // Зарезервированные, «типо-функциональные» и «колоночные» ключевые слова — в кавычки;
        // незарезервированные (`name`, `value`, `data`) — нет.
        for kw in [
            "select", "user", "table", "left", "like", "time", "none", "position", "analyse",
        ] {
            assert_eq!(quote_ident(kw), format!("\"{kw}\""), "{kw}");
        }
        for plain in ["name", "value", "data", "type", "status", "comment", "year"] {
            assert_eq!(quote_ident(plain), plain, "{plain}");
        }
    }

    #[test]
    fn pg18_not_null_constraints_and_constraint_triggers_are_not_table_constraints() {
        let mut t = table("t");
        let mut id = column("id", "bigint");
        id.not_null = true;
        t.columns = vec![id];
        t.constraints = vec![
            constraint("t_id_not_null", "n", "NOT NULL id", true),
            constraint("t_chk", "c", "CHECK (id > 0)", true),
            // pg_get_constraintdef для constraint trigger возвращает просто «TRIGGER …».
            constraint("ct", "t", "TRIGGER DEFERRABLE INITIALLY DEFERRED", true),
        ];
        t.triggers = vec![Trigger {
            name: "ct".into(),
            enabled: "O".into(),
            definition: "CREATE CONSTRAINT TRIGGER ct AFTER INSERT ON shop.t DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION shop.f()".into(),
        }];
        let ddl = table_ddl(&t);
        assert!(
            ddl.contains("    id bigint NOT NULL,\n    CONSTRAINT t_chk CHECK (id > 0)\n);"),
            "{ddl}"
        );
        assert!(!ddl.contains("t_id_not_null"), "{ddl}");
        assert!(
            !ddl.contains("TRIGGER DEFERRABLE INITIALLY DEFERRED,"),
            "{ddl}"
        );
        assert!(
            ddl.contains("CREATE CONSTRAINT TRIGGER ct AFTER INSERT"),
            "{ddl}"
        );
    }

    #[test]
    fn virtual_and_stored_generated_columns_keep_not_null() {
        let mut t = table("t");
        let mut s = column("s", "integer");
        s.generated = Some("s".into());
        s.default_expr = Some("(a * 2)".into());
        s.not_null = true;
        let mut v = column("v", "integer");
        v.generated = Some("v".into());
        v.default_expr = Some("(a * 3)".into());
        t.columns = vec![column("a", "integer"), s, v];
        let ddl = table_ddl(&t);
        assert!(
            ddl.contains("    s integer GENERATED ALWAYS AS ((a * 2)) STORED NOT NULL,\n"),
            "{ddl}"
        );
        assert!(
            ddl.contains("    v integer GENERATED ALWAYS AS ((a * 3)) VIRTUAL\n"),
            "{ddl}"
        );
    }

    #[test]
    fn identity_by_default_and_identity_wins_over_missing_default() {
        let mut t = table("t");
        let mut id = column("id", "integer");
        id.identity = Some("d".into());
        id.not_null = true;
        t.columns = vec![id];
        assert!(
            table_ddl(&t).contains("    id integer GENERATED BY DEFAULT AS IDENTITY NOT NULL\n")
        );
    }

    #[test]
    fn not_valid_is_not_repeated_when_the_definition_already_has_it() {
        let mut t = table("t");
        t.columns = vec![column("a", "integer"), column("r", "bigint")];
        t.constraints = vec![
            // pg_get_constraintdef сам дописывает NOT VALID у непроверенных CHECK и FOREIGN KEY.
            constraint("t_chk", "c", "CHECK (a > 0) NOT VALID", false),
            constraint(
                "t_fk",
                "f",
                "FOREIGN KEY (r) REFERENCES shop.ref(id) DEFERRABLE INITIALLY DEFERRED NOT VALID",
                false,
            ),
            // PostgreSQL 18: NOT ENFORCED вместо NOT VALID.
            constraint("t_ne", "c", "CHECK (a < 100) NOT ENFORCED", false),
            constraint("t_ok", "c", "CHECK (a <> 7)", true),
        ];
        let ddl = table_ddl(&t);
        assert!(!ddl.contains("NOT VALID NOT VALID"), "{ddl}");
        assert!(!ddl.contains("NOT ENFORCED NOT VALID"), "{ddl}");
        // NOT ENFORCED и проверенный CHECK остаются внутри CREATE TABLE (PostgreSQL 18 учитывает NOT ENFORCED),
        // NOT VALID CHECK и внешний ключ уходят в ALTER TABLE.
        assert!(
            ddl.contains(
                "    CONSTRAINT t_ne CHECK (a < 100) NOT ENFORCED,\n    CONSTRAINT t_ok CHECK (a <> 7)\n);"
            ),
            "{ddl}"
        );
        assert!(
            ddl.ends_with(
                "\nALTER TABLE shop.t ADD CONSTRAINT t_chk CHECK (a > 0) NOT VALID;\nALTER TABLE shop.t ADD CONSTRAINT t_fk FOREIGN KEY (r) REFERENCES shop.ref(id) DEFERRABLE INITIALLY DEFERRED NOT VALID;\n"
            ),
            "{ddl}"
        );
    }

    #[test]
    fn not_valid_check_and_foreign_keys_follow_the_indexes_as_alter_table() {
        let mut t = table("node");
        t.columns = vec![column("id", "integer"), column("parent_id", "integer")];
        t.constraints = vec![
            // `NOT VALID` внутри CREATE TABLE молча игнорируется: такой CHECK пишется отдельно.
            constraint("node_chk", "c", "CHECK (id > 0) NOT VALID", false),
            // Самоссылка на колонку с одним уникальным индексом: индекс обязан существовать раньше ключа.
            constraint(
                "node_parent_fkey",
                "f",
                "FOREIGN KEY (parent_id) REFERENCES shop.node(id)",
                true,
            ),
            constraint("node_pkey", "p", "PRIMARY KEY (id)", true),
        ];
        t.indexes = vec![index(
            "node_parent_idx",
            "CREATE INDEX node_parent_idx ON shop.node USING btree (parent_id)",
            true,
        )];
        assert_eq!(
            table_ddl(&t),
            format!(
                "{DDL_HEADER}\nCREATE TABLE shop.node (\n    id integer,\n    parent_id integer,\n    CONSTRAINT node_pkey PRIMARY KEY (id)\n);\n\nCREATE INDEX node_parent_idx ON shop.node USING btree (parent_id);\n\nALTER TABLE shop.node ADD CONSTRAINT node_chk CHECK (id > 0) NOT VALID;\nALTER TABLE shop.node ADD CONSTRAINT node_parent_fkey FOREIGN KEY (parent_id) REFERENCES shop.node(id);\n"
            )
        );
        // Только внешние ключи и непроверенные ограничения: у таблицы без остальных скобок нет.
        let mut only_fk = table("child");
        only_fk.partitioning.parent = Some(rel("shop", "parent"));
        only_fk.partitioning.bound = Some("DEFAULT".into());
        only_fk.constraints = vec![constraint(
            "child_fk",
            "f",
            "FOREIGN KEY (a) REFERENCES shop.ref(id)",
            true,
        )];
        assert_eq!(
            table_ddl(&only_fk),
            format!(
                "{DDL_HEADER}\nCREATE TABLE shop.child PARTITION OF shop.parent\n    DEFAULT;\n\nALTER TABLE shop.child ADD CONSTRAINT child_fk FOREIGN KEY (a) REFERENCES shop.ref(id);\n"
            )
        );
    }

    #[test]
    fn constraint_backed_indexes_are_skipped_and_invalid_ones_are_marked() {
        let mut t = table("t");
        t.columns = vec![column("a", "integer")];
        t.constraints = vec![
            constraint("t_pkey", "p", "PRIMARY KEY (a)", true),
            constraint("t_a_key", "u", "UNIQUE (a)", true),
            constraint("t_excl", "x", "EXCLUDE USING gist (a WITH =)", true),
            // CHECK и FOREIGN KEY индексов не создают: одноимённый индекс — отдельный объект.
            constraint("t_chk_idx", "c", "CHECK (a > 0)", true),
        ];
        t.indexes = vec![
            index(
                "t_pkey",
                "CREATE UNIQUE INDEX t_pkey ON shop.t USING btree (a)",
                true,
            ),
            index(
                "t_a_key",
                "CREATE UNIQUE INDEX t_a_key ON shop.t USING btree (a)",
                true,
            ),
            index(
                "t_excl",
                "CREATE INDEX t_excl ON shop.t USING gist (a)",
                true,
            ),
            index(
                "t_chk_idx",
                "CREATE INDEX t_chk_idx ON shop.t USING btree (a)",
                true,
            ),
            index(
                "t_broken",
                "CREATE INDEX t_broken ON shop.t USING btree (a)",
                false,
            ),
        ];
        let ddl = table_ddl(&t);
        assert_eq!(
            ddl.split_once(");\n").unwrap().1,
            "\nCREATE INDEX t_chk_idx ON shop.t USING btree (a);\nCREATE INDEX t_broken ON shop.t USING btree (a); -- INVALID\n"
        );
    }

    #[test]
    fn non_default_trigger_states_are_restored() {
        let mut t = table("t");
        t.columns = vec![column("a", "integer")];
        let trig = |name: &str, enabled: &str| Trigger {
            name: name.into(),
            enabled: enabled.into(),
            definition: format!(
                "CREATE TRIGGER {name} BEFORE INSERT ON shop.t FOR EACH ROW EXECUTE FUNCTION shop.f()"
            ),
        };
        t.triggers = vec![
            trig("on", "O"),
            trig("off", "D"),
            trig("replica", "R"),
            trig("always", "A"),
        ];
        assert_eq!(
            table_ddl(&t).split_once(");\n").unwrap().1,
            "\nCREATE TRIGGER on BEFORE INSERT ON shop.t FOR EACH ROW EXECUTE FUNCTION shop.f();\n\
             CREATE TRIGGER off BEFORE INSERT ON shop.t FOR EACH ROW EXECUTE FUNCTION shop.f();\n\
             ALTER TABLE shop.t DISABLE TRIGGER off;\n\
             CREATE TRIGGER replica BEFORE INSERT ON shop.t FOR EACH ROW EXECUTE FUNCTION shop.f();\n\
             ALTER TABLE shop.t ENABLE REPLICA TRIGGER replica;\n\
             CREATE TRIGGER always BEFORE INSERT ON shop.t FOR EACH ROW EXECUTE FUNCTION shop.f();\n\
             ALTER TABLE shop.t ENABLE ALWAYS TRIGGER always;\n"
        );
    }

    #[test]
    fn partition_keeps_own_constraints_sub_partitioning_options_and_default_bound() {
        let mut part = table("events_old");
        part.partitioning = Partitioning {
            key: Some("LIST (kind)".into()),
            bound: Some("FOR VALUES FROM ('2020-01-01') TO ('2021-01-01')".into()),
            parent: Some(rel("shop", "events")),
            partitions: vec![rel("shop", "events_old_a")],
        };
        part.constraints = vec![constraint("events_old_chk", "c", "CHECK (id > 0)", true)];
        part.reloptions = vec!["fillfactor=70".into()];
        part.tablespace = Some("Fast Disk".into());
        assert_eq!(
            table_ddl(&part),
            format!(
                "{DDL_HEADER}\nCREATE TABLE shop.events_old PARTITION OF shop.events\n(\n    CONSTRAINT events_old_chk CHECK (id > 0)\n)\n    FOR VALUES FROM ('2020-01-01') TO ('2021-01-01') PARTITION BY LIST (kind) WITH (fillfactor=70) TABLESPACE \"Fast Disk\";\n-- partitions: shop.events_old_a\n"
            )
        );
        let mut def = table("events_default");
        def.partitioning.parent = Some(rel("shop", "events"));
        def.partitioning.bound = Some("DEFAULT".into());
        assert!(
            table_ddl(&def).contains(
                "CREATE TABLE shop.events_default PARTITION OF shop.events\n    DEFAULT;\n"
            )
        );
    }

    #[test]
    fn table_without_columns_and_constraints_is_valid_sql() {
        assert_eq!(
            table_ddl(&table("t")),
            format!("{DDL_HEADER}\nCREATE TABLE shop.t ();\n")
        );
    }

    #[test]
    fn foreign_table_is_an_explicit_stub() {
        let mut t = table("remote");
        t.relkind = "f".into();
        t.columns = vec![column("id", "integer")];
        t.comment = Some("x".into());
        assert_eq!(
            table_ddl(&t),
            format!(
                "{DDL_HEADER}\n-- CREATE FOREIGN TABLE shop.remote is not generated: the foreign server and its options are not read from the catalog\n"
            )
        );
    }

    #[test]
    fn comments_escape_quotes_and_quote_identifiers() {
        let mut t = table("Odd");
        t.columns = vec![column("Col", "text")];
        t.columns[0].comment = Some("it's".into());
        assert!(table_ddl(&t).ends_with("\nCOMMENT ON COLUMN shop.\"Odd\".\"Col\" IS 'it''s';\n"));
    }

    #[test]
    fn newlines_in_identifiers_cannot_break_out_of_comment_lines() {
        // Идентификатор вправе содержать перевод строки: `quote_ident` сервера и наш его сохраняют.
        let mut parent = table("events");
        parent.relkind = "p".into();
        parent.columns = vec![column("id", "integer")];
        parent.partitioning.key = Some("RANGE (id)".into());
        parent.partitioning.partitions = vec![
            rel("shop", "p_1\nSELECT 'INJECTED'; --"),
            rel("shop", "p_2\r\nDROP TABLE shop.events; --"),
            rel("shop", "ok"),
        ];
        let ddl = table_ddl(&parent);
        // Заголовок, CREATE TABLE (3 строки: заголовок, колонка, PARTITION BY) и одна строка-комментарий.
        assert_eq!(ddl.lines().count(), 1 + 3 + 1, "{ddl}");
        assert!(!ddl.contains('\r'), "{ddl:?}");
        assert!(
            ddl.lines().all(|l| l.starts_with("--")
                || l.starts_with("CREATE TABLE")
                || l.starts_with("    id")
                || l.starts_with(") PARTITION BY")),
            "{ddl}"
        );
        assert!(
            ddl.ends_with(
                "-- partitions: shop.\"p_1 SELECT 'INJECTED'; --\", shop.\"p_2  DROP TABLE shop.events; --\", shop.ok\n"
            ),
            "{ddl}"
        );
        // Заглушка внешней таблицы строится из имени таблицы — то же правило.
        let mut foreign = table("remote\nSELECT 1; --");
        foreign.relkind = "f".into();
        let ddl = table_ddl(&foreign);
        assert_eq!(ddl.lines().count(), 2, "{ddl}");
        assert!(
            ddl.lines().nth(1).unwrap().starts_with(
                "-- CREATE FOREIGN TABLE shop.\"remote SELECT 1; --\" is not generated"
            )
        );
    }

    #[test]
    fn comment_text_replaces_control_characters_only() {
        assert_eq!(
            comment_text("a\nb\rc\td\u{1}e\u{85}f\u{2028}g"),
            "a b c d e f g"
        );
        assert_eq!(
            comment_text("shop.\"Таблица 1\", ok"),
            "shop.\"Таблица 1\", ok"
        );
    }

    #[test]
    fn indexes_of_partitioned_tables_lose_only() {
        let mut t = table("events");
        t.relkind = "p".into();
        t.columns = vec![column("id", "integer")];
        t.indexes = vec![
            index(
                "events_id_idx",
                "CREATE INDEX events_id_idx ON ONLY shop.events USING btree (id)",
                true,
            ),
            index(
                "events_u",
                "CREATE UNIQUE INDEX events_u ON ONLY shop.events USING btree (id)",
                true,
            ),
            // ` ON ONLY ` в имени индекса не трогаем: меняется только оператор.
            index(
                "x ON ONLY y",
                "CREATE INDEX \"x ON ONLY y\" ON ONLY shop.events USING btree (id)",
                true,
            ),
            index(
                "q\"a ON ONLY b",
                "CREATE INDEX \"q\"\"a ON ONLY b\" ON ONLY shop.events USING btree (id)",
                true,
            ),
            // Индекс, который сам присоединён к индексу родителя, печатается с обычным ON.
            index(
                "events_plain",
                "CREATE INDEX events_plain ON shop.events USING btree (id)",
                true,
            ),
        ];
        let ddl = table_ddl(&t);
        assert!(
            ddl.ends_with(
                "\nCREATE INDEX events_id_idx ON shop.events USING btree (id);\nCREATE UNIQUE INDEX events_u ON shop.events USING btree (id);\nCREATE INDEX \"x ON ONLY y\" ON shop.events USING btree (id);\nCREATE INDEX \"q\"\"a ON ONLY b\" ON shop.events USING btree (id);\nCREATE INDEX events_plain ON shop.events USING btree (id);\n"
            ),
            "{ddl}"
        );
        // У обычной таблицы определение остаётся как есть.
        let mut r = table("r");
        r.indexes = vec![index(
            "r_idx",
            "CREATE INDEX r_idx ON ONLY shop.r USING btree (id)",
            true,
        )];
        assert!(table_ddl(&r).contains("ON ONLY shop.r"));
    }

    #[test]
    fn literals_with_backslashes_do_not_depend_on_standard_conforming_strings() {
        assert_eq!(literal("plain"), "'plain'");
        assert_eq!(literal("it's"), "'it''s'");
        assert_eq!(literal("C:\\new\\dir"), "E'C:\\\\new\\\\dir'");
        assert_eq!(literal("a\\'b"), "E'a\\\\''b'");
        let mut t = table("t");
        t.columns = vec![column("a", "integer")];
        t.comment = Some("C:\\new\\dir".into());
        assert!(table_ddl(&t).ends_with("COMMENT ON TABLE shop.t IS E'C:\\\\new\\\\dir';\n"));
    }

    #[test]
    fn disabled_copies_of_inherited_triggers_keep_their_state() {
        let mut part = table("events_2026_09");
        part.partitioning.parent = Some(rel("shop", "events"));
        part.partitioning.bound = Some("DEFAULT".into());
        let trig = |name: &str, enabled: &str| Trigger {
            name: name.into(),
            enabled: enabled.into(),
            definition: format!(
                "CREATE TRIGGER {name} BEFORE INSERT ON shop.events_2026_09 FOR EACH ROW EXECUTE FUNCTION shop.f()"
            ),
        };
        // Собственный триггер партиции выводится целиком, унаследованные копии — только со своим состоянием.
        part.triggers = vec![trig("own", "O")];
        let inherited = vec![
            trig("on_copy", "O"),
            trig("off_copy", "D"),
            trig("replica_copy", "R"),
            trig("always_copy", "A"),
        ];
        assert_eq!(
            partition_ddl(&part, &inherited)
                .split_once("DEFAULT;\n")
                .unwrap()
                .1,
            "\nCREATE TRIGGER own BEFORE INSERT ON shop.events_2026_09 FOR EACH ROW EXECUTE FUNCTION shop.f();\n\
             ALTER TABLE shop.events_2026_09 DISABLE TRIGGER off_copy;\n\
             ALTER TABLE shop.events_2026_09 ENABLE REPLICA TRIGGER replica_copy;\n\
             ALTER TABLE shop.events_2026_09 ENABLE ALWAYS TRIGGER always_copy;\n"
        );
        // Без унаследованных копий результат прежний.
        assert_eq!(partition_ddl(&part, &[]), table_ddl(&part));
        // Только включённые копии: лишних строк нет.
        assert_eq!(partition_ddl(&part, &inherited[..1]), table_ddl(&part));
        // Партиция без собственных триггеров, но с выключенной копией: строка есть.
        part.triggers.clear();
        assert!(
            partition_ddl(&part, &inherited[1..2]).ends_with(
                "DEFAULT;\n\nALTER TABLE shop.events_2026_09 DISABLE TRIGGER off_copy;\n"
            )
        );
    }

    #[test]
    fn unlogged_tables_and_partitions_are_created_unlogged() {
        let mut t = table("t");
        t.unlogged = true;
        t.columns = vec![column("a", "integer")];
        assert_eq!(
            table_ddl(&t),
            format!("{DDL_HEADER}\nCREATE UNLOGGED TABLE shop.t (\n    a integer\n);\n")
        );
        t.columns.clear();
        assert!(table_ddl(&t).contains("CREATE UNLOGGED TABLE shop.t ();\n"));
        let mut part = table("p");
        part.unlogged = true;
        part.partitioning.parent = Some(rel("shop", "t"));
        part.partitioning.bound = Some("DEFAULT".into());
        assert!(table_ddl(&part).contains("CREATE UNLOGGED TABLE shop.p PARTITION OF shop.t\n"));
        // Партиционированная таблица — без слова даже при relpersistence = 'u' (PG 18 такой CREATE отвергает).
        let mut parent = table("t");
        parent.unlogged = true;
        parent.relkind = "p".into();
        parent.partitioning.key = Some("RANGE (a)".into());
        parent.columns = vec![column("a", "integer")];
        assert!(table_ddl(&parent).contains("\nCREATE TABLE shop.t (\n"));
        // Обычная таблица без слова.
        t.unlogged = false;
        assert!(table_ddl(&t).contains("\nCREATE TABLE shop.t ();\n"));
    }
}
