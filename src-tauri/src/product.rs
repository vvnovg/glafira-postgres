//! Имя продукта и идентификаторы — в одном месте. Переименование меняет только этот файл
//! и `tauri.conf.json`; тест ниже следит, чтобы они совпадали.

pub const PRODUCT_NAME: &str = "Glafira for PostgreSQL";
pub const IDENTIFIER: &str = "io.github.vvnovg.glafira.postgres";
pub const KEYCHAIN_SERVICE: &str = "glafira-postgres";
pub const POLICY_VENDOR: &str = "Glafira";
pub const POLICY_PRODUCT: &str = "PostgreSQL";
/// `application_name` соединений приложения: так их видно в `pg_stat_activity`.
pub const PG_APPLICATION_NAME: &str = "glafira-pg";
/// `application_name` соединений SQL модели MCP (спец. P4 §7.3): их нагрузку видно в `pg_stat_activity` отдельно от
/// Glafira, и признак «сессия Glafira» (`glafira-pg`) у них не стоит.
pub const MCP_APPLICATION_NAME: &str = "glafira-pg-mcp";
/// Имя MCP-сервера: имя в `initialize`, ключ в конфигах клиентов (`qwen mcp add glafira-pg …`) и основа имени
/// сокета или named pipe (`glafira-pg-mcp…`, спец. P4 §3.3).
pub const MCP_SERVER_NAME: &str = "glafira-pg";

/// Литерал SQL с ведущей меткой запросов Глафиры: `marked_sql!("SELECT …")`. Метка — комментарий:
/// `pg_stat_statements` хранит его в тексте строки, а `queryid` его не учитывает (спец. P3 §4).
#[macro_export]
macro_rules! marked_sql {
    ($sql:literal) => {
        concat!("/* glafira */ ", $sql)
    };
}

/// Метка запросов Глафиры — периодических (опрос топологии, сэмплеры, диагностика по таймеру) и разовых (каталог,
/// чек-лист, действия; замечание 17 смоука P3b), кроме текста пользователя; в строках `format!` — `{SQL_MARKER}` в начале.
pub const SQL_MARKER: &str = marked_sql!("");

/// Текст запроса начинается с метки Глафиры (ведущие пробелы и переводы строк не мешают).
pub fn is_marked_sql(text: &str) -> bool {
    text.trim_start().starts_with(SQL_MARKER.trim_end())
}

#[cfg(test)]
mod tests {
    #[test]
    fn marked_sql_is_recognised() {
        assert_eq!(marked_sql!("SELECT 1"), "/* glafira */ SELECT 1");
        assert!(super::is_marked_sql(marked_sql!("SELECT 1")));
        assert!(super::is_marked_sql(&format!(
            "{}SELECT $1",
            super::SQL_MARKER
        )));
        assert!(super::is_marked_sql("  \n\t/* glafira */ SELECT 1"));
        assert!(super::is_marked_sql("/* glafira */\nSELECT 1"));
        assert!(!super::is_marked_sql("SELECT 1 /* glafira */"));
        assert!(!super::is_marked_sql("/* glafira-like */ SELECT 1"));
        assert!(!super::is_marked_sql(""));
    }

    /// Строки исходника без элементов под `#[cfg(test)]` (модуль тестов, вспомогательная функция посреди файла):
    /// номер строки (с 1) и строка. Элемент — от атрибута до `;` в строке без `{` либо до закрытия первой `{` (подсчёт
    /// скобок по строкам; литералы с непарными скобками в тестовых элементах не встречаются). Код после элемента
    /// проверяется (M4 ревью волны смоука P3b).
    fn production_lines(src: &str) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        let mut skipping = false;
        let mut depth = 0i64;
        let mut opened = false;
        for (i, l) in src.lines().enumerate() {
            if !skipping {
                if l.trim_start().starts_with("#[cfg(test)]") {
                    (skipping, depth, opened) = (true, 0, false);
                } else {
                    out.push((i + 1, l));
                }
                continue;
            }
            let t = l.trim_start();
            if !opened && (t.starts_with("#[") || t.is_empty() || t.starts_with("//")) {
                continue;
            }
            depth += l.matches('{').count() as i64 - l.matches('}').count() as i64;
            opened |= l.contains('{');
            if (opened && depth <= 0) || (!opened && t.trim_end().ends_with(';')) {
                skipping = false;
            }
        }
        out
    }

    #[test]
    fn production_lines_skip_only_test_items() {
        let src = "fn a() {}\n#[cfg(test)]\nfn helper() {\n    x();\n}\nconst Q: &str = \"SELECT 1\";\n\
                   #[cfg(test)]\nuse foo::bar;\nfn b() {}\n#[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    \
                   fn t() {\n    }\n}\nfn c() {}";
        let lines: Vec<usize> = production_lines(src).into_iter().map(|(n, _)| n).collect();
        assert_eq!(lines, [1, 6, 9, 16]);
    }

    /// Замечание 17 смоука P3b: метка у всех запросов приложения, не только периодических. Проверка по исходникам (весь
    /// файл без элементов под `#[cfg(test)]`, `production_lines`): строковый литерал, начинающийся с инструкции SQL, — в `marked_sql!` или с `{SQL_MARKER}`
    /// в той же или двух предыдущих строках. Текст пользователя (консоль EXPLAIN) литералом не бывает.
    #[test]
    fn every_sql_literal_is_marked() {
        const STARTS: [&str; 11] = [
            "\"SELECT ",
            "\"WITH ",
            "\"BEGIN",
            "\"SET ",
            "\"ROLLBACK",
            "\"SAVEPOINT ",
            "\"SHOW ",
            "\"ANALYZE ",
            "\"VACUUM ",
            "\"DISCARD ",
            "\"RESET ",
        ];
        // Куски уже помеченного `format!` и опции EXPLAIN; обёртка курсора `run_select` (P4b): внутри — текст модели,
        // и курсор, как EXPLAIN пользователя, без метки (`mcp::select::cursor_query`).
        const ALLOWED: [&str; 3] = [
            "\"SELECT 't', ",
            "\"ANALYZE, BUFFERS",
            "\"SELECT {} FROM (SELECT * FROM (\\n{body}\\n) q OFFSET 0)",
        ];
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut unmarked = Vec::new();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            let lines = production_lines(&src);
            for (i, (n, l)) in lines.iter().enumerate() {
                if l.trim_start().starts_with("//")
                    || !STARTS.iter().any(|s| l.contains(s))
                    || ALLOWED.iter().any(|s| l.contains(s))
                {
                    continue;
                }
                let near = lines[i.saturating_sub(2)..=i]
                    .iter()
                    .map(|(_, l)| *l)
                    .collect::<Vec<_>>()
                    .join(" ");
                if !near.contains("marked_sql!") && !near.contains("SQL_MARKER") {
                    unmarked.push(format!("{}:{}: {}", f.display(), n, l.trim()));
                }
            }
        }
        assert!(unmarked.is_empty(), "{unmarked:#?}");
    }

    #[test]
    fn matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], super::IDENTIFIER);
        assert_eq!(conf["productName"], super::PRODUCT_NAME);
        assert_eq!(conf["mainBinaryName"], "glafira-pg");
        assert_eq!(conf["app"]["windows"][0]["title"], super::PRODUCT_NAME);
    }
}
