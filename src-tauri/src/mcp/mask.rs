//! Маскирование литералов на уровне `diagnostics` (спец. P4 §6.1): тексты `pg_stat_activity` и pgss в данных
//! инструментов и выражения узлов планов, построенных не по SQL модели. Лексер — `pg_advisor::sqltext::mask_literals`;
//! здесь — где в данных тексты и маскирование, не зависящее от `standard_conforming_strings` (`mask_fail_closed`,
//! решение владельца 2026-11-04): настройка может быть своей у базы, роли и сеанса, и её значение у узла ничего не
//! гарантирует.

use pg_advisor::ash::text_key;
use pg_advisor::locktree::LockNode;
use pg_advisor::sqltext::{MAX_PARAMS, MaskOptions, count_params, mask_literals};
use serde_json::Value;

use crate::diag::query_detail::QueryDetail;
use crate::diag::{DiagData, DiagReport};

/// Строковые выражения узлов JSON-плана (`EXPLAIN (FORMAT JSON, VERBOSE)`): в них сервер печатает литералы запроса.
pub const PLAN_EXPRESSION_KEYS: [&str; 24] = [
    "Output",
    "Filter",
    "Join Filter",
    "One-Time Filter",
    "Index Cond",
    "Order By",
    "Recheck Cond",
    "TID Cond",
    "Merge Cond",
    "Hash Cond",
    "Run Condition",
    "Sort Key",
    "Presorted Key",
    "Group Key",
    "Group Keys",
    "Hash Key",
    "Hash Keys",
    "Cache Key",
    "Function Call",
    "Table Function Call",
    "Sampling Parameters",
    "Repeatable Seed",
    "Conflict Filter",
    "Window",
];

/// Тексты запросов сеансов в ответе модели — не длиннее (спец. §5.3, как у `observe_activity`); обрезаются после
/// маскирования.
pub const MAX_QUERY_BYTES: usize = 2048;

/// Обрезка до `max` байт по границе символа, без отметки (соглашение текстов `observe_activity`, спец. §5.3).
pub fn cut_text(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

const SCS_ON: MaskOptions = MaskOptions {
    standard_conforming_strings: true,
};
const SCS_OFF: MaskOptions = MaskOptions {
    standard_conforming_strings: false,
};

/// Маскирование, которому не важно значение `standard_conforming_strings` (решение владельца 2026-11-04). Обратная
/// косая в `'…'` при `off` экранирует кавычку, при `on` — нет, и граница литерала у двух прочтений разная. Текст
/// лексится при обоих значениях: совпали итоги — это и есть ответ (у текста без `\` в `'…'` он равен
/// `mask_literals`). Разошлись — остаётся их общее начало, а всё дальше заменяется одной меткой `$N…`: общее начало —
/// начало верного итога при любом значении настройки, поэтому литерал не откроется ни при каком. `$N` на конце общей
/// части (литерал, чья граница и разошлась) уходит в метку; номер метки — следующий после наибольшего `$N` общей части
/// (любого, и приклеенного к слову или числу: `date$1`, `$1$2`) и текста. Второе значение — число литералов:
/// замаскированные в общей части (`literals_in`) и метка.
pub fn mask_fail_closed(text: &str) -> (String, usize) {
    let (on, masked) = mask_literals(text, SCS_ON);
    let (off, _) = mask_literals(text, SCS_OFF);
    if on == off {
        return (on, masked);
    }
    let mut common = on
        .bytes()
        .zip(off.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !on.is_char_boundary(common) {
        common -= 1;
    }
    let mut prefix = on[..common].to_string();
    let tail = prefix.trim_end_matches(|c: char| c.is_ascii_digit());
    if tail.ends_with('$') {
        prefix.truncate(tail.len() - 1);
    }
    // `mask_literals` нумерует с `count_params(text) + 1`; `count_params` не видит `$N` в идентификаторе и после
    // символа слова, а в общей части такой `$N` бывает и номером литерала (`date'…'` → `date$1`). Метка — выше любого.
    let before = count_params(text);
    let n = before.max(largest_number(&prefix)).saturating_add(1);
    let literals = literals_in(text, before, &prefix).unwrap_or(n - before - 1);
    prefix.push('$');
    prefix.push_str(&n.to_string());
    prefix.push('…');
    (prefix, literals + 1)
}

/// Длина `$N` (знак и цифры) в `at`; не `$N` — `None`.
fn number_token(b: &[u8], at: usize) -> Option<usize> {
    (b.get(at) == Some(&b'$') && b.get(at + 1).is_some_and(u8::is_ascii_digit)).then(|| {
        1 + b[at + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    })
}

/// Наибольший номер `$N` в тексте простым просмотром, без правил лексера (в идентификаторе, после буквы или цифры —
/// тоже); номер за пределом `usize` насыщается.
fn largest_number(s: &str) -> usize {
    let b = s.as_bytes();
    let (mut max, mut i) = (0usize, 0usize);
    while i < b.len() {
        match number_token(b, i) {
            Some(len) => {
                let n = b[i + 1..i + len].iter().fold(0usize, |n, d| {
                    n.saturating_mul(10).saturating_add(usize::from(d - b'0'))
                });
                max = max.max(n);
                i += len;
            }
            None => i += 1,
        }
    }
    max
}

/// Сколько литералов замаскировано в `prefix` — начале итога `mask_literals(text)` при `on`. Текст лексится ещё раз с
/// номерами на один выше (`$M` перед ним поднимает `count_params`): у литерала номер в двух итогах разный, а `$N` самого
/// текста (параметр, `$N` в идентификаторе или после буквы) — тот же. `None` — сдвинуть номера нельзя (в тексте уже
/// `$N` предельного номера) или итоги не сошлись по форме.
fn literals_in(text: &str, before: usize, prefix: &str) -> Option<usize> {
    if before >= MAX_PARAMS {
        return None;
    }
    let head = format!("${} ", before + 1);
    let (shifted, _) = mask_literals(&format!("{head}{text}"), SCS_ON);
    let (a, b) = (prefix.as_bytes(), shifted.strip_prefix(&head)?.as_bytes());
    let (mut i, mut j, mut literals) = (0usize, 0usize, 0usize);
    while i < a.len() {
        match (number_token(a, i), number_token(b, j)) {
            (Some(la), Some(lb)) => {
                literals += usize::from(a[i..i + la] != b[j..j + lb]);
                (i, j) = (i + la, j + lb);
            }
            _ if b.get(j) == Some(&a[i]) => (i, j) = (i + 1, j + 1),
            _ => return None,
        }
    }
    Some(literals)
}

/// Маскирует текст на месте (`mask_fail_closed`); итог — число замаскированных литералов (комментарии не считаются).
/// Единственный путь маскирования текстов для модели: сеансы, блокировки, ожидания, pgss, выражения планов.
pub fn mask_text(text: &mut String) -> usize {
    let (masked, n) = mask_fail_closed(text);
    *text = masked;
    n
}

pub fn mask_opt(text: &mut Option<String>) -> usize {
    text.as_mut().map_or(0, mask_text)
}

/// В данных инструмента бывают тексты запросов. Исчерпывающий `match`: новый инструмент (P5) не пройдёт сборку, пока
/// здесь не решено, есть ли у него тексты.
pub fn has_texts(d: &DiagData) -> bool {
    match d {
        DiagData::Sessions(_) | DiagData::Locks(_) | DiagData::Waits(_) | DiagData::Queries(_) => {
            true
        }
        DiagData::Pulse(_)
        | DiagData::Horizon(_)
        | DiagData::Replication(_)
        | DiagData::Slots(_)
        | DiagData::Failover(_)
        | DiagData::Health(_) => false,
    }
}

fn mask_lock_node(n: &mut LockNode) -> usize {
    mask_opt(&mut n.session.query) + n.children.iter_mut().map(mask_lock_node).sum::<usize>()
}

/// Тексты запросов в данных отчёта: сессии, дерево блокировок и очередь DDL, топ профиля ожиданий, строки pgss.
/// Ключ `text:<хэш>` строки профиля ожиданий без `query_id` считается по исходному тексту — он пересчитывается по
/// замаскированному (`ash::text_key`), иначе по хэшу проверялись бы догадки о литералах. Строки с одинаковым после
/// маскирования текстом получают один ключ.
/// Исчерпывающий `match`: новый инструмент (P5) не пройдёт сборку, пока здесь не решено, есть ли у него тексты.
pub fn mask_report(report: &mut DiagReport) -> usize {
    match &mut report.data {
        DiagData::Sessions(d) => d.rows.iter_mut().map(|r| mask_opt(&mut r.query)).sum(),
        DiagData::Locks(d) => {
            let roots: usize = d.roots.iter_mut().map(mask_lock_node).sum();
            let queue: usize = d
                .ddl_queue
                .iter_mut()
                .map(|q| mask_opt(&mut q.session.query))
                .sum();
            roots + queue
        }
        DiagData::Waits(d) => d
            .top_queries
            .iter_mut()
            .map(|q| {
                let n = mask_text(&mut q.text);
                if q.key.starts_with("text:") {
                    q.key = text_key(&q.text);
                }
                n
            })
            .sum(),
        DiagData::Queries(d) => d.rows.iter_mut().map(|r| mask_opt(&mut r.text)).sum(),
        // Держатели горизонта — сессии без текста, остальное — счётчики, имена слотов и состояния узлов.
        DiagData::Pulse(_)
        | DiagData::Horizon(_)
        | DiagData::Replication(_)
        | DiagData::Slots(_)
        | DiagData::Failover(_)
        | DiagData::Health(_) => 0,
    }
}

/// Тексты карточки запроса: текст pgss, строки статистики (с момента сброса, окна, по ролям) и выполняющиеся экземпляры
/// из `pg_stat_activity`. Остальное в карточке — числа, имена ролей и узлов, ожидания и находки (параметры находок
/// pgss — числа).
pub fn mask_query_detail(d: &mut QueryDetail) -> usize {
    let mut n = mask_opt(&mut d.text);
    if let Some(s) = &mut d.since_reset {
        n += mask_opt(&mut s.text);
    }
    for w in &mut d.windows {
        if let Some(s) = &mut w.stat {
            n += mask_opt(&mut s.text);
        }
    }
    for b in &mut d.by_user {
        n += mask_opt(&mut b.stat.text);
    }
    for r in &mut d.running {
        n += mask_text(&mut r.query);
    }
    n
}

fn mask_strings(v: &mut Value) -> usize {
    match v {
        Value::String(s) => mask_text(s),
        Value::Array(a) => a.iter_mut().map(mask_strings).sum(),
        Value::Object(_) => mask_plan_value(v),
        _ => 0,
    }
}

/// Выражения узлов JSON-плана (`PLAN_EXPRESSION_KEYS`) на любой глубине; числа плана, имена отношений и индексов и
/// настройки (`Settings`) не меняются.
pub fn mask_plan_value(raw: &mut Value) -> usize {
    match raw {
        Value::Object(m) => m
            .iter_mut()
            .map(|(k, v)| {
                if PLAN_EXPRESSION_KEYS.contains(&k.as_str()) {
                    mask_strings(v)
                } else {
                    mask_plan_value(v)
                }
            })
            .sum(),
        Value::Array(a) => a.iter_mut().map(mask_plan_value).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use pg_advisor::ash::{TopQuery, WaitsData, text_key};
    use pg_advisor::locktree::{DdlQueueItem, LockNode, LockSession, LocksData};
    use pg_advisor::queries::{Counters, QueryKey, Totals, stat};
    use pg_advisor::rules::horizon::{Holder, HorizonData};
    use pg_advisor::rules::sessions::{SessionRow, SessionsData};
    use pg_advisor::sqltext::{MaskOptions, mask_literals};
    use serde_json::json;

    use super::*;
    use crate::diag::queries::{QueriesData, QueriesMode};
    use crate::diag::{DiagReport, Tool};

    const SECRET: &str = "SELECT * FROM shop.customers WHERE email = 'ann@example.com' AND id = 42 /* tenant=acme */";

    fn report(tool: Tool, data: DiagData) -> DiagReport {
        DiagReport {
            tool,
            member: Some("pg-1".into()),
            follows_leader: true,
            observed_at_ms: 1,
            server_version_num: Some(170_000),
            in_recovery: Some(false),
            data,
            findings: vec![],
            unavailable: vec![],
        }
    }

    fn clean(s: &str) {
        assert!(
            !s.contains("ann@example.com") && !s.contains("42") && !s.contains("tenant=acme"),
            "{s}"
        );
        assert!(s.contains("$1") && s.contains("shop.customers"), "{s}");
    }

    #[test]
    fn session_texts_are_masked() {
        let mut r = report(
            Tool::Sessions,
            DiagData::Sessions(SessionsData {
                rows: vec![
                    SessionRow {
                        pid: 1,
                        query: Some(SECRET.into()),
                        ..Default::default()
                    },
                    SessionRow {
                        pid: 2,
                        query: None,
                        ..Default::default()
                    },
                ],
                settings: Default::default(),
            }),
        );
        assert_eq!(mask_report(&mut r), 2);
        let DiagData::Sessions(d) = &r.data else {
            unreachable!()
        };
        clean(d.rows[0].query.as_deref().unwrap());
        assert_eq!(d.rows[1].query, None);
    }

    #[test]
    fn lock_tree_texts_are_masked_at_every_depth() {
        let s = |pid| LockSession {
            pid,
            query: Some(SECRET.into()),
            ..Default::default()
        };
        let tree = LockNode {
            session: s(1),
            children: vec![LockNode {
                session: s(2),
                children: vec![LockNode {
                    session: s(3),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut r = report(
            Tool::Locks,
            DiagData::Locks(LocksData {
                roots: vec![tree],
                cycles: vec![],
                ddl_queue: vec![DdlQueueItem {
                    session: s(4),
                    mode: "AccessExclusiveLock".into(),
                    ..Default::default()
                }],
            }),
        );
        assert_eq!(mask_report(&mut r), 8, "4 сессии × 2 литерала");
        let v = serde_json::to_string(&r.data).unwrap();
        assert!(
            !v.contains("ann@example.com") && !v.contains("tenant=acme"),
            "{v}"
        );
    }

    #[test]
    fn wait_profile_queries_are_masked() {
        let mut r = report(
            Tool::Waits,
            DiagData::Waits(WaitsData {
                top_queries: vec![TopQuery {
                    key: "id:7".into(),
                    text: SECRET.into(),
                    samples: 3,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(mask_report(&mut r), 2);
        let DiagData::Waits(d) = &r.data else {
            unreachable!()
        };
        clean(&d.top_queries[0].text);
        assert_eq!(d.top_queries[0].key, "id:7", "ключ по queryid не меняется");
    }

    /// Ключ `text:<хэш>` строки без `query_id` считается по исходному тексту: по нему модель проверяла бы догадки о
    /// литералах. На уровне `diagnostics` ключ пересчитывается по замаскированному тексту.
    #[test]
    fn wait_profile_text_keys_come_from_the_masked_text() {
        let raw = "UPDATE users SET password = 'hunter2' WHERE id = 7";
        let mut r = report(
            Tool::Waits,
            DiagData::Waits(WaitsData {
                top_queries: vec![TopQuery {
                    key: text_key(raw),
                    text: raw.into(),
                    samples: 3,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(mask_report(&mut r), 2);
        let DiagData::Waits(d) = &r.data else {
            unreachable!()
        };
        let q = &d.top_queries[0];
        assert_eq!(q.text, "UPDATE users SET password = $1 WHERE id = $2");
        assert_eq!(q.key, text_key(&q.text));
        assert_ne!(q.key, text_key(raw));
    }

    #[test]
    fn statement_texts_are_masked() {
        let mut row = stat(
            QueryKey {
                query_id: 7,
                dbid: 1,
                userid: 10,
                toplevel: true,
            },
            Some("orders".into()),
            None,
            &Counters::default(),
            None,
            &Totals::default(),
        );
        row.text = Some("ALTER ROLE app PASSWORD 'hunter2'".into());
        let mut r = report(
            Tool::Queries,
            DiagData::Queries(QueriesData {
                rows: vec![row],
                ..QueriesData::empty(QueriesMode::SinceReset)
            }),
        );
        assert_eq!(mask_report(&mut r), 1);
        let DiagData::Queries(d) = &r.data else {
            unreachable!()
        };
        assert_eq!(
            d.rows[0].text.as_deref(),
            Some("ALTER ROLE app PASSWORD $1")
        );
    }

    #[test]
    fn query_card_texts_are_masked_everywhere() {
        use crate::diag::query_detail::{ByUser, QueryDetail, RunningInstance, WindowStat};
        let qs = || {
            let mut s = stat(
                QueryKey {
                    query_id: 7,
                    dbid: 1,
                    userid: 10,
                    toplevel: true,
                },
                Some("orders".into()),
                Some("app".into()),
                &Counters::default(),
                None,
                &Totals::default(),
            );
            s.text = Some(SECRET.into());
            s
        };
        let mut d = QueryDetail::unavailable_pgss(170_000, false);
        d.text = Some(SECRET.into());
        d.since_reset = Some(qs());
        d.windows = vec![
            WindowStat {
                window_s: 300,
                stat: Some(qs()),
            },
            WindowStat {
                window_s: 900,
                stat: None,
            },
        ];
        d.by_user = vec![ByUser {
            rolname: Some("app".into()),
            toplevel: true,
            stat: qs(),
        }];
        d.running = vec![RunningInstance {
            pid: 1,
            backend_start_us: 1,
            state: Some("active".into()),
            started_ms: None,
            duration_s: None,
            wait_event_type: None,
            wait_event: None,
            usename: None,
            application_name: None,
            client_addr: None,
            query: SECRET.into(),
            truncated: false,
        }];
        assert_eq!(mask_query_detail(&mut d), 10, "5 текстов × 2 литерала");
        let v = serde_json::to_string(&d).unwrap();
        assert!(
            !v.contains("ann@example.com") && !v.contains("tenant=acme"),
            "{v}"
        );
    }

    #[test]
    fn data_without_query_texts_is_left_as_is() {
        let data = DiagData::Horizon(HorizonData {
            holders: vec![Holder {
                name: "4242".into(),
                xmin_age: 10,
                ..Default::default()
            }],
            hot_standby_feedback: Some(false),
        });
        assert!(!has_texts(&data));
        let mut r = report(Tool::Horizon, data.clone());
        assert_eq!(mask_report(&mut r), 0);
        assert_eq!(r.data, data);
    }

    /// Граница литерала с обратной косой зависит от `standard_conforming_strings` сессии, а её значение у базы, роли
    /// или сеанса может быть любым. Маскирование не читает настройку: текст лексится при `on` и при `off`, и всё, где
    /// результаты расходятся, уходит в одну метку `$N…`.
    #[test]
    fn masking_does_not_depend_on_standard_conforming_strings() {
        // `on`: литерал кончается на второй кавычке, и «OR 1=1» вне его; `off`: «OR 1=1» внутри литерала.
        let (out, n) = mask_fail_closed(r"SELECT 'a\' OR 1=1 --', 'b'");
        assert!(!out.contains("OR") && !out.contains("1=1"), "{out}");
        assert_eq!((out.as_str(), n), ("SELECT $1…", 1));
        // `on`: «Brien» и «123» вне литералов; `off`: «Brien» внутри первого, 123 — внутри второго.
        let (out, _) =
            mask_fail_closed(r"SELECT * FROM people WHERE name = 'O\'Brien' AND ssn = '123'");
        assert!(!out.contains("Brien") && !out.contains("123"), "{out}");
        assert!(
            out.starts_with("SELECT * FROM people WHERE name = $1"),
            "{out}"
        );
        // Номер метки — после литералов общей части и `$N` текста; метка считается одним литералом.
        let (out, n) = mask_fail_closed(r"SELECT $1, 'x', 'a\' OR 1=1");
        assert_eq!((out.as_str(), n), ("SELECT $1, $2, $3…", 2));
        let mut text = r"/* glafira */ SELECT 'a\' OR 1=1 --'".to_string();
        assert_eq!(mask_text(&mut text), 1);
        assert_eq!(text, "/* glafira */ SELECT $1…");
    }

    /// Расхождение после E-строки, долларовых кавычек, вложенного комментария и `N'…'`: общее начало остаётся, всё
    /// дальше — одна метка, и ни одно прочтение не открывает литерал.
    #[test]
    fn divergent_texts_after_other_literal_kinds_are_cut_at_the_first_divergence() {
        for (text, expected, n) in [
            (r"SELECT E'a\'b', 'c\' OR 1=1 --'", "SELECT $1, $2…", 2),
            (r"$$it's \' here$$, 'z\' AND secret = 1 --'", "$1, $2…", 2),
            (r"/* a /* 'b\' */ c */ 'secret\' x'", "/*…*/ $1…", 1),
            (r"N'O\'Brien', 'x'", "$1…", 1),
        ] {
            assert_eq!(mask_fail_closed(text), (expected.to_string(), n), "{text}");
        }
    }

    /// Номер метки — после наибольшего `$N` общей части, даже приклеенного к слову или числу (`date$1`, `$1$2`):
    /// `count_params` такие `$N` за параметры не считает, и метка совпала бы с номером литерала.
    #[test]
    fn divergence_marker_never_repeats_a_number_of_the_common_part() {
        for (text, expected, n) in [
            (
                r"SELECT date'2024-01-01', 'a\' OR 1=1",
                "SELECT date$1, $2…",
                2,
            ),
            (r"SELECT 1'a', 'b\' OR 1=1", "SELECT $1$2, $3…", 3),
            // `$9` в идентификаторе — номер метки выше него, а в счёте только литерал общей части и метка.
            (
                r#"SELECT "col$9", 7, 'a\' OR 1=1"#,
                r#"SELECT "col$9", $1, $10…"#,
                2,
            ),
            (r"SELECT x$1, 'a\' OR 1=1", "SELECT x$1, $2…", 1),
            // Предельный номер в тексте: сдвинуть номера нельзя, счёт — по номеру метки.
            (
                r"SELECT $65535, 'a', 'b\' OR 1=1",
                "SELECT $65535, $65536, $65537…",
                2,
            ),
        ] {
            assert_eq!(mask_fail_closed(text), (expected.to_string(), n), "{text}");
        }
    }

    /// Без обратной косой в '…' оба прочтения совпадают: итог — ровно `mask_literals`.
    #[test]
    fn plain_texts_are_masked_exactly_as_mask_literals_does() {
        for text in [
            SECRET,
            "SELECT 'it''s', E'\\n\\'', $$body$$, U&'d\\0061t\\+000061', B'101', X'ff', 1.5e3, 0x1F",
            "/* glafira */ SELECT n.nspname FROM pg_namespace n WHERE n.oid = 42",
            "INSERT INTO t VALUES ($1, 'x') -- tail",
            r"SELECT '\\', 'C:\\dir\\file'",
            "SELECT 'unclosed",
            "SELECT \"col'name\", 'v' FROM t",
        ] {
            let expected = mask_literals(text, MaskOptions::default());
            assert_eq!(mask_fail_closed(text), expected, "{text}");
        }
        let (kept, _) = mask_fail_closed("/* glafira */ SELECT 1");
        assert_eq!(kept, "/* glafira */ SELECT $1");
    }

    #[test]
    fn cut_keeps_whole_characters() {
        let mut s = "a".repeat(MAX_QUERY_BYTES - 1) + "€tail";
        cut_text(&mut s, MAX_QUERY_BYTES);
        assert_eq!(
            s,
            "a".repeat(MAX_QUERY_BYTES - 1),
            "€ (3 байта) не режется пополам"
        );
        let mut short = "SELECT $1".to_string();
        cut_text(&mut short, MAX_QUERY_BYTES);
        assert_eq!(short, "SELECT $1");
    }

    #[test]
    fn plan_expressions_are_masked_but_plan_numbers_and_names_stay() {
        let mut raw = json!([{
            "Plan": {
                "Node Type": "Seq Scan", "Relation Name": "orders", "Total Cost": 12.5, "Plan Rows": 3,
                "Filter": "(status = 'disputed'::text)", "Output": ["id", "'x'::text"],
                "Plans": [{"Node Type": "Index Scan", "Index Name": "orders_pkey", "Index Cond": "(id = 42)"}]
            },
            "Settings": {"work_mem": "1MB"}
        }]);
        assert_eq!(mask_plan_value(&mut raw), 3);
        let p = &raw[0]["Plan"];
        assert_eq!(p["Filter"], "(status = $1::text)");
        assert_eq!(p["Output"][1], "$1::text");
        assert_eq!(p["Plans"][0]["Index Cond"], "(id = $1)");
        assert_eq!(p["Total Cost"], 12.5);
        assert_eq!(p["Relation Name"], "orders");
        assert_eq!(raw[0]["Settings"]["work_mem"], "1MB");
    }

    #[test]
    fn plan_expressions_are_masked_whatever_standard_conforming_strings_is() {
        let mut raw = json!({"Plan": {"Node Type": "Seq Scan", "Filter": r"(name = 'O\'Brien' AND ssn = '123')"}});
        mask_plan_value(&mut raw);
        let f = raw["Plan"]["Filter"].as_str().unwrap();
        assert!(!f.contains("Brien") && !f.contains("123"), "{f}");
    }
}
