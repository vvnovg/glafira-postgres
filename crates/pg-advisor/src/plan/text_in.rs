//! Разбор текстового `EXPLAIN` (формат TEXT) в тот же `PlanTree`, что из JSON (спец. P3 §8.5).
//!
//! Принимается вывод psql (заголовок `QUERY PLAN`, разделитель `----`, строка `(N rows)`, хвосты `+` у плана одной
//! строкой, рамка `\pset border 2`), строки в кавычках (копия из pgAdmin / DBeaver), «голый» текст и запись
//! auto_explain (строки до плана — `LOG:  duration: … plan:`, `Query Text: …` — служебные; текст запроса уходит в
//! `extra["Query Text"]` корня). Дерево строится по отступам: заголовок узла — первая строка плана
//! и строки `->  …`; подписи `InitPlan N` / `SubPlan N` / `CTE имя` относятся к следующему узлу; строки `Ключ: значение`
//! с отступом атрибутов — свойства узла; строки на уровне корня после дерева — итоги (`Planning Time`, `JIT` …).
//! Нераспознанные свойства узла сохраняются в `extra` под своим ключом (строкой; вложенные строки блока, как у
//! `Worker 0:` в VERBOSE, — массивом), строки без ключа — в `extra["text"]`.

use serde_json::Value;

use crate::plan::{
    Actual, Buffers, Jit, PlanError, PlanNode, PlanTree, Relation, Setting, Trigger,
};

fn err(message: impl Into<String>, line: usize) -> PlanError {
    PlanError {
        message: message.into(),
        line: Some(line),
    }
}

/// Строка текста после снятия обрамления: номер в исходном тексте (с 1), отступ, текст без отступа.
struct Line {
    no: usize,
    col: usize,
    text: String,
}

/// Признак заголовка узла: стоимость, факт или `never executed` в скобках.
fn has_head_marker(s: &str) -> bool {
    s.contains("(cost=") || s.contains("(actual ") || s.contains("(never executed)")
}

/// Разделитель psql под заголовком столбца: `----` или `---+---`.
fn is_separator(t: &str) -> bool {
    t.len() >= 3 && t.chars().all(|c| c == '-' || c == '+')
}

/// Подвал psql `(N rows)` / `(1 row)`.
fn is_footer(t: &str) -> bool {
    t.strip_prefix('(')
        .and_then(|r| r.strip_suffix(" rows)").or_else(|| r.strip_suffix(" row)")))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Строка таблицы psql с рамкой (`\pset border 2`): `| текст |` → `текст` (пробел за левой чертой — часть рамки).
fn unframe(l: &str) -> Option<&str> {
    let inner = l.trim_end().strip_prefix('|')?.strip_suffix('|')?;
    Some(inner.strip_prefix(' ').unwrap_or(inner))
}

/// Строка копии из графического клиента (pgAdmin, DBeaver): `"текст"` с начала строки, кавычки внутри удвоены.
fn unquote(l: &str) -> Option<String> {
    let t = l.trim_end();
    let inner = t.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.replace("\"\"", "\""))
}

/// Разделитель psql под заголовком столбца — только в начале текста: первая непустая строка (заголовка нет), вторая
/// (под заголовком; не под строкой плана или `Query Text:` — в тексте запроса бывают линейки `-----`) или, у таблицы с
/// рамкой, третья (под заголовком между двумя линиями рамки). Номер строки и признак рамки.
fn separator(raw: &[&str]) -> (Option<usize>, bool) {
    let head: Vec<usize> = (0..raw.len())
        .filter(|&i| !raw[i].trim().is_empty())
        .take(3)
        .collect();
    let sep = |k: usize| {
        head.get(k)
            .copied()
            .filter(|&i| is_separator(raw[i].trim()))
    };
    let Some(first) = head.first().map(|&i| raw[i].trim()) else {
        return (None, false);
    };
    if sep(0).is_some() && first.starts_with('+') {
        return (sep(2).or(sep(0)), true);
    }
    let header = !has_head_marker(first) && !first.contains("Query Text:");
    (sep(0).or(sep(1).filter(|_| header)), false)
}

/// Снимает обрамление psql: всё до разделителя (заголовок столбца), подвал, хвосты `+` у многострочного значения,
/// рамку `| … |` (`\pset border 2`) и кавычки вокруг строк (копия из графического клиента). Ведущие табуляции
/// (auto_explain в журнале) заменяются пробелами; пустые строки отбрасываются. Отступ — в символах: неразрывный
/// пробел (план со страницы) занимает одну колонку, как обычный.
fn clean(text: &str) -> Vec<Line> {
    let raw: Vec<&str> = text.lines().collect();
    let (sep, bordered) = separator(&raw);
    let start = sep.map_or(0, |i| i + 1);
    let framed = sep.is_some();
    let mut out = Vec::new();
    for (i, l) in raw.iter().enumerate().skip(start) {
        let mut l: &str = l;
        if bordered {
            if is_separator(l.trim()) {
                continue;
            }
            l = unframe(l).unwrap_or(l);
        }
        let unquoted = unquote(l);
        let l = unquoted.as_deref().unwrap_or(l);
        let lead = l.len() - l.trim_start_matches('\t').len();
        let s = format!("{}{}", "    ".repeat(lead), &l[lead..]);
        let mut t = s.trim_end();
        if framed {
            // Значение из нескольких строк psql печатает с `+` в конце каждой строки, кроме последней.
            t = t.strip_suffix('+').unwrap_or(t).trim_end();
        }
        let trimmed = t.trim_start();
        if trimmed.is_empty() || trimmed == "QUERY PLAN" || is_footer(trimmed) {
            continue;
        }
        out.push(Line {
            no: i + 1,
            col: t[..t.len() - trimmed.len()].chars().count(),
            text: trimmed.to_string(),
        });
    }
    out
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

/// Целое из текста; дробные значения (PG 18 печатает строки с дробной частью) округляются.
fn int(s: &str) -> Option<i64> {
    let s = s.trim();
    s.parse::<i64>()
        .ok()
        .or_else(|| s.parse::<f64>().ok().map(|f| f.round() as i64))
}

/// Объём `25kB` / `24 kB` → килобайты.
fn kb(s: &str) -> Option<i64> {
    int(s.trim().trim_end_matches("kB"))
}

/// Мс из `0.307 ms`.
fn ms(s: &str) -> Option<f64> {
    num(s.trim().trim_end_matches("ms"))
}

/// Делит по `sep` вне скобок и кавычек: списки `Output`, `Sort Key` содержат выражения с запятыми.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut cur = String::new();
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ if c == sep && depth == 0 => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                    continue;
                }
                _ => {}
            },
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Идентификатор с начала строки (в кавычках — с удвоенными кавычками внутри) и остаток.
fn ident(s: &str) -> (String, &str) {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '"' {
                if chars.peek().is_some_and(|(_, n)| *n == '"') {
                    out.push('"');
                    chars.next();
                } else {
                    return (out, &rest[i + 1..]);
                }
            } else {
                out.push(c);
            }
        }
        (out, "")
    } else {
        let end = s
            .find(|c: char| c.is_whitespace() || c == '.')
            .unwrap_or(s.len());
        (s[..end].to_string(), &s[end..])
    }
}

/// `[схема.]имя [псевдоним]` цели узла.
fn target(s: &str) -> (Option<String>, String, Option<String>) {
    let (first, rest) = ident(s);
    let (schema, name, rest) = match rest.strip_prefix('.') {
        Some(r) => {
            let (n, r) = ident(r);
            (Some(first), n, r)
        }
        None => (None, first, rest),
    };
    let alias = (!rest.trim().is_empty()).then(|| ident(rest).0);
    (schema, name, alias)
}

fn set_extra(n: &mut PlanNode, key: &str, v: impl Into<Value>) {
    n.extra.insert(key.to_string(), v.into());
}

/// Имя узла, как его печатает `ExplainNode`, → тип узла и его признаки (обратное `text_out::node_name`).
fn describe(n: &mut PlanNode, name: &str) {
    // Цель: `using idx`, `Backward using idx`, `on [схема.]отношение [псевдоним]`.
    let (mut kind, target_part) = match [" using ", " on "]
        .iter()
        .filter_map(|m| name.find(m))
        .min()
    {
        Some(i) => (&name[..i], Some(name[i..].trim_start())),
        None => (name, None),
    };
    if let Some(k) = kind.strip_suffix(" Backward") {
        kind = k;
        set_extra(n, "Scan Direction", "Backward");
    }
    loop {
        if let Some(k) = kind.strip_prefix("Parallel ") {
            n.parallel_aware = true;
            kind = k;
        } else if let Some(k) = kind.strip_prefix("Async ") {
            set_extra(n, "Async Capable", true);
            kind = k;
        } else {
            break;
        }
    }
    let mut partial = None;
    for mode in ["Partial ", "Finalize "] {
        if let Some(k) = kind.strip_prefix(mode)
            && k.ends_with("Aggregate")
        {
            partial = Some(mode.trim_end());
            kind = k;
        }
    }
    // `Left Join` / `Right Anti Join` / `Join` после имени соединения → тип соединения (`Inner` без уточнения).
    let join = |rest: &str| -> Option<String> {
        let mid = rest.strip_suffix("Join")?.trim();
        Some(if mid.is_empty() { "Inner" } else { mid }.to_string())
    };
    let (node_type, strategy, join_type) = match kind {
        "Aggregate" => ("Aggregate".to_string(), Some("Plain"), None),
        "GroupAggregate" => ("Aggregate".to_string(), Some("Sorted"), None),
        "HashAggregate" => ("Aggregate".to_string(), Some("Hashed"), None),
        "MixedAggregate" => ("Aggregate".to_string(), Some("Mixed"), None),
        "SetOp" => ("SetOp".to_string(), Some("Sorted"), None),
        "HashSetOp" => ("SetOp".to_string(), Some("Hashed"), None),
        "Insert" | "Update" | "Delete" | "Merge" => {
            set_extra(n, "Operation", kind);
            ("ModifyTable".to_string(), None, None)
        }
        "Nested Loop" => ("Nested Loop".to_string(), None, Some("Inner".to_string())),
        k if k.starts_with("Nested Loop ") => (
            "Nested Loop".to_string(),
            None,
            join(&k["Nested Loop ".len()..]),
        ),
        k if k.starts_with("Hash ") && k.ends_with("Join") => {
            ("Hash Join".to_string(), None, join(&k["Hash ".len()..]))
        }
        k if k.starts_with("Merge ") && k.ends_with("Join") => {
            ("Merge Join".to_string(), None, join(&k["Merge ".len()..]))
        }
        k if k.starts_with("Custom Scan (") && k.ends_with(')') => {
            set_extra(
                n,
                "Custom Plan Provider",
                &k["Custom Scan (".len()..k.len() - 1],
            );
            ("Custom Scan".to_string(), None, None)
        }
        k => (k.to_string(), None, None),
    };
    if node_type == "Aggregate" {
        set_extra(n, "Partial Mode", partial.unwrap_or("Simple"));
    }
    if matches!(node_type.as_str(), "Index Scan" | "Index Only Scan")
        && !n.extra.contains_key("Scan Direction")
    {
        set_extra(n, "Scan Direction", "Forward");
    }
    let mut rest = target_part;
    if let Some(r) = rest.and_then(|r| r.strip_prefix("using ")) {
        let (idx, r) = ident(r);
        n.index_name = Some(idx);
        rest = r.trim_start().strip_prefix("on ");
    } else {
        rest = rest.and_then(|r| r.strip_prefix("on "));
    }
    if let Some(t) = rest {
        let (schema, name, alias) = target(t);
        match node_type.as_str() {
            "Bitmap Index Scan" => n.index_name = Some(name),
            "CTE Scan" | "WorkTable Scan" => {
                set_extra(n, "CTE Name", name.clone());
                set_extra(n, "Alias", alias.unwrap_or(name));
            }
            "Function Scan" => {
                if let Some(s) = schema {
                    set_extra(n, "Schema", s);
                }
                set_extra(n, "Function Name", name.clone());
                set_extra(n, "Alias", alias.unwrap_or(name));
            }
            "Subquery Scan" | "Values Scan" | "Table Function Scan" | "Named Tuplestore Scan" => {
                set_extra(n, "Alias", name);
            }
            _ => {
                // Псевдоним, совпадающий с именем, сервер не печатает; JSON отдаёт его всегда.
                let alias = Some(alias.unwrap_or_else(|| name.clone()));
                n.relation = Some(Relation {
                    schema,
                    name,
                    alias,
                });
            }
        }
    }
    n.node_type = node_type;
    n.strategy = strategy.map(str::to_string);
    n.join_type = join_type;
}

/// Содержимое скобок, начинающихся с `open` (без самих скобок).
fn paren<'a>(s: &'a str, open: &str) -> Option<&'a str> {
    let at = s.find(open)?;
    let inner = &s[at + open.len()..];
    Some(&inner[..inner.find(')')?])
}

/// Заголовок узла: имя с целью, `(cost=a..b rows=n width=w)`, `(actual time=a..b rows=n loops=l)` /
/// `(actual rows=n loops=l)` / `(never executed)`.
fn parse_head(text: &str, line: usize) -> Result<PlanNode, PlanError> {
    let name_end = ["(cost=", "(actual ", "(never executed)"]
        .iter()
        .filter_map(|m| text.find(m))
        .min()
        .unwrap_or(text.len());
    let name = text[..name_end].trim();
    if name.is_empty() {
        return Err(err("plan node without a type", line));
    }
    // Тип узла — слова (`Seq Scan`, `Hash Join`); иное начало — обрамление, которое разбор не знает.
    if !name.starts_with(|c: char| c.is_alphabetic()) {
        return Err(err(
            format!("unsupported plan format: the node type starts with \"{name}\""),
            line,
        ));
    }
    let mut n = PlanNode::default();
    describe(&mut n, name);
    if let Some(costs) = paren(text, "(cost=") {
        let bad = || err(format!("unrecognized costs \"(cost={costs})\""), line);
        let mut parts = costs.split_whitespace();
        let (a, b) = parts
            .next()
            .and_then(|r| r.split_once(".."))
            .ok_or_else(bad)?;
        n.startup_cost = num(a).ok_or_else(bad)?;
        n.total_cost = num(b).ok_or_else(bad)?;
        for p in parts {
            if let Some(v) = p.strip_prefix("rows=") {
                n.plan_rows = num(v).ok_or_else(bad)?;
            } else if let Some(v) = p.strip_prefix("width=") {
                n.plan_width = int(v).ok_or_else(bad)?;
            }
        }
    }
    if text.contains("(never executed)") {
        n.actual = Some(Actual {
            startup_ms: None,
            total_ms: None,
            rows: 0.0,
            loops: 0.0,
        });
        n.never_executed = true;
    } else if let Some(inner) = paren(text, "(actual ") {
        let bad = || err(format!("unrecognized actuals \"(actual {inner})\""), line);
        let mut a = Actual {
            startup_ms: None,
            total_ms: None,
            rows: 0.0,
            loops: 1.0,
        };
        for p in inner.split_whitespace() {
            if let Some(v) = p.strip_prefix("time=") {
                let (s, e) = v.split_once("..").ok_or_else(bad)?;
                a.startup_ms = Some(num(s).ok_or_else(bad)?);
                a.total_ms = Some(num(e).ok_or_else(bad)?);
            } else if let Some(v) = p.strip_prefix("rows=") {
                a.rows = num(v).ok_or_else(bad)?;
            } else if let Some(v) = p.strip_prefix("loops=") {
                a.loops = num(v).ok_or_else(bad)?.max(0.0);
            }
        }
        n.never_executed = a.loops <= 0.0;
        n.actual = Some(a);
    }
    Ok(n)
}

/// Ключ строки свойства: `Ключ: значение`, ключ — слова с заглавной буквы (`Rows Removed by Filter`, `Worker 0`).
fn key_value(t: &str) -> Option<(&str, &str)> {
    let (k, v) = t.split_once(':')?;
    let label = k.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '/' | '.'));
    label.then(|| (k.trim(), v.trim()))
}

/// Пары `Ключ: значение`, разделённые двумя пробелами (`Buckets: 1024  Batches: 1  Memory Usage: 12kB`).
fn pairs(t: &str) -> Vec<(&str, &str)> {
    t.split("  ")
        .filter_map(|p| p.trim().split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
}

/// `слово=число` через пробел (`hit=10 read=2`).
fn assignments(t: &str) -> impl Iterator<Item = (&str, &str)> {
    t.split_whitespace().filter_map(|p| p.split_once('='))
}

/// Добавляет строку к массиву `extra[key]` (строковое значение становится первым элементом).
fn push_extra(n: &mut PlanNode, key: &str, line: &str) {
    let entry = n
        .extra
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::String(s) = entry {
        let first = std::mem::take(s);
        *entry = Value::Array(if first.is_empty() {
            Vec::new()
        } else {
            vec![Value::String(first)]
        });
    }
    if let Value::Array(a) = entry {
        a.push(Value::String(line.to_string()));
    }
}

/// Строка свойства узла. Возвращает ключ нераспознанного свойства: вложенные строки (блок `Worker N:`) идут к нему.
fn detail(n: &mut PlanNode, t: &str) -> Option<String> {
    let Some((key, v)) = key_value(t) else {
        push_extra(n, "text", t);
        return None;
    };
    let s = || Some(v.to_string());
    match key {
        "Filter" => n.filter = s(),
        "Index Cond" => n.index_cond = s(),
        "Recheck Cond" => n.recheck_cond = s(),
        "Hash Cond" => n.hash_cond = s(),
        "Join Filter" => n.join_filter = s(),
        "Merge Cond" => n.merge_cond = s(),
        "Rows Removed by Filter" => n.rows_removed_by_filter = int(v),
        "Rows Removed by Index Recheck" => n.rows_removed_by_index_recheck = int(v),
        "Rows Removed by Join Filter" => n.rows_removed_by_join_filter = int(v),
        "Heap Fetches" => n.heap_fetches = int(v),
        "Workers Planned" => n.workers_planned = int(v),
        "Workers Launched" => n.workers_launched = int(v),
        "Subplans Removed" => n.subplans_removed = int(v),
        "Sort Key" => n.sort_key = split_top(v, ','),
        // Наборов группировки может быть несколько: каждый — своей строкой `Group Key:`.
        "Group Key" => n.group_key.extend(split_top(v, ',')),
        "Output" => n.output = split_top(v, ','),
        "Presorted Key" => set_extra(n, "Presorted Key", split_top(v, ',')),
        "Sort Method" => {
            // `top-N heapsort  Memory: 25kB` | `external merge  Disk: 2048kB`
            let (method, space) = v.split_once("  ").unwrap_or((v, ""));
            n.sort_method = Some(method.trim().to_string());
            if let Some((kind, amount)) = space.trim().split_once(':') {
                n.sort_space_type = Some(kind.trim().to_string());
                n.sort_space_kb = kb(amount);
            }
        }
        // Hash: `Buckets: 1024 (originally 512)  Batches: 2 (originally 1)  Memory Usage: 12kB`.
        "Buckets" => {
            for (k, val) in pairs(t) {
                let (cur, orig) = match val.split_once("(originally") {
                    Some((c, o)) => (c, int(o.trim_end_matches(')'))),
                    None => (val, None),
                };
                match k {
                    "Buckets" => {
                        n.hash_buckets = int(cur);
                        set_extra(n, "Original Hash Buckets", orig.or(n.hash_buckets));
                    }
                    "Batches" => {
                        n.hash_batches = int(cur);
                        set_extra(n, "Original Hash Batches", orig.or(n.hash_batches));
                    }
                    "Memory Usage" => n.peak_memory_kb = kb(cur),
                    _ => set_extra(n, k, val),
                }
            }
        }
        // HashAggregate: `[Planned Partitions: 4  ]Batches: 5  Memory Usage: 4145kB  Disk Usage: 8216kB`;
        // Memoize: `Hits: 1  Misses: 2  Evictions: 0  Overflows: 0  Memory Usage: 1kB`.
        "Batches" | "Planned Partitions" | "Hits" => {
            for (k, val) in pairs(t) {
                match k {
                    "Memory Usage" => n.peak_memory_kb = kb(val),
                    "Batches" => set_extra(n, "HashAgg Batches", int(val)),
                    "Disk Usage" => set_extra(n, "Disk Usage", kb(val)),
                    "Planned Partitions" => set_extra(n, k, int(val)),
                    "Hits" | "Misses" | "Evictions" | "Overflows" => {
                        set_extra(n, &format!("Cache {k}"), int(val))
                    }
                    _ => set_extra(n, k, val),
                }
            }
        }
        "Heap Blocks" => {
            for (k, val) in assignments(v) {
                match k {
                    "exact" => n.exact_heap_blocks = int(val),
                    "lossy" => n.lossy_heap_blocks = int(val),
                    _ => {}
                }
            }
        }
        "Buffers" => {
            let b = n.buffers.get_or_insert_with(Buffers::default);
            for group in v.split(',') {
                let mut words = group.split_whitespace();
                let kind = words.next().unwrap_or("");
                for (k, val) in words.filter_map(|p| p.split_once('=')) {
                    let val = int(val).unwrap_or(0);
                    match (kind, k) {
                        ("shared", "hit") => b.shared_hit = val,
                        ("shared", "read") => b.shared_read = val,
                        ("shared", "dirtied") => b.shared_dirtied = val,
                        ("shared", "written") => b.shared_written = val,
                        ("local", "hit") => b.local_hit = val,
                        ("local", "read") => b.local_read = val,
                        ("local", "dirtied") => b.local_dirtied = val,
                        ("local", "written") => b.local_written = val,
                        ("temp", "read") => b.temp_read = val,
                        ("temp", "written") => b.temp_written = val,
                        _ => {}
                    }
                }
            }
        }
        // До 15 — `read=… write=…`; 15–16 — `shared/local read=…, temp read=…`; с 17 — `shared …, local …, temp …`.
        "I/O Timings" => {
            for group in v.split(',') {
                let g = group.trim();
                let (kind, rest) = match g.split_once(' ') {
                    Some((k, r)) if !k.contains('=') => (k, r),
                    _ => ("", g),
                };
                for (k, val) in assignments(rest) {
                    let val = num(val);
                    match (kind, k) {
                        ("" | "shared" | "shared/local", "read") => n.io_read_ms = val,
                        ("" | "shared" | "shared/local", "write") => n.io_write_ms = val,
                        ("local", "read") => set_extra(n, "Local I/O Read Time", val),
                        ("local", "write") => set_extra(n, "Local I/O Write Time", val),
                        ("temp", "read") => set_extra(n, "Temp I/O Read Time", val),
                        ("temp", "write") => set_extra(n, "Temp I/O Write Time", val),
                        _ => {}
                    }
                }
            }
        }
        "WAL" => {
            for (k, val) in assignments(v) {
                let key = match k {
                    "records" => "WAL Records",
                    "fpi" => "WAL FPI",
                    "bytes" => "WAL Bytes",
                    "full" => "WAL Buffers Full",
                    _ => continue,
                };
                set_extra(n, key, int(val));
            }
        }
        _ => {
            if n.extra.contains_key(key) {
                push_extra(n, key, v);
            } else {
                set_extra(n, key, v);
            }
            return Some(key.to_string());
        }
    }
    None
}

/// Блок итогов после дерева, к которому относятся строки с большим отступом.
#[derive(PartialEq)]
enum Tail {
    None,
    Jit,
    Other,
}

/// Строка итогов на уровне корня: время планирования и выполнения, триггеры, JIT, параметры.
fn tail_line(tree: &mut PlanTree, t: &str) -> Tail {
    let Some((key, v)) = t.split_once(':') else {
        return Tail::None;
    };
    match key {
        "Planning Time" => tree.planning_ms = ms(v),
        "Execution Time" => tree.execution_ms = ms(v),
        "JIT" => {
            tree.jit = Some(Jit {
                functions: 0,
                generation_ms: None,
                inlining_ms: None,
                optimization_ms: None,
                emission_ms: None,
                total_ms: None,
            });
            return Tail::Jit;
        }
        "Settings" => tree.settings = settings(v),
        // `Trigger имя[ on отношение]: time=1.234 calls=5`, `Trigger for constraint имя …`; без TIMING — только calls.
        k if k.starts_with("Trigger ") => {
            let (head, stats) = t.rsplit_once(": ").unwrap_or((k, v));
            let head = &head["Trigger ".len()..];
            let head = head.strip_prefix("for constraint ").unwrap_or(head);
            let (name, relation) = match head.split_once(" on ") {
                Some((n, r)) => (n, Some(r.to_string())),
                None => (head, None),
            };
            let mut tr = Trigger {
                name: name.to_string(),
                relation,
                time_ms: 0.0,
                calls: 0,
            };
            for (k, val) in assignments(stats) {
                match k {
                    "time" => tr.time_ms = num(val).unwrap_or(0.0),
                    "calls" => tr.calls = int(val).unwrap_or(0),
                    _ => {}
                }
            }
            tree.triggers.push(tr);
        }
        // `Planning:` (буферы планирования), `Query Identifier`, `Serialization` — в модели их нет, как и у JSON.
        _ => return Tail::Other,
    }
    Tail::None
}

/// Строка внутри блока `JIT:`.
fn jit_line(j: &mut Jit, t: &str) {
    if let Some(v) = t.strip_prefix("Functions:") {
        j.functions = int(v).unwrap_or(0);
    } else if let Some(v) = t.strip_prefix("Timing:") {
        // `Generation 0.153 ms (Deform 0.038 ms), Inlining 29.496 ms, …, Total 45.677 ms`: Deform входит в Generation.
        for part in split_top(v, ',') {
            let mut words = part.split_whitespace();
            let (k, val) = (words.next().unwrap_or(""), words.next().and_then(num));
            match k {
                "Generation" => j.generation_ms = val,
                "Inlining" => j.inlining_ms = val,
                "Optimization" => j.optimization_ms = val,
                "Emission" => j.emission_ms = val,
                "Total" => j.total_ms = val,
                _ => {}
            }
        }
    }
}

/// `work_mem = '8MB', search_path = 'shop, public'` (значение в кавычках, кавычка внутри удвоена).
fn settings(v: &str) -> Vec<Setting> {
    split_top(v, ',')
        .iter()
        .filter_map(|kv| {
            let (k, val) = kv.split_once(" = ")?;
            let val = val.trim();
            let val = val
                .strip_prefix('\'')
                .and_then(|x| x.strip_suffix('\''))
                .map_or_else(|| val.to_string(), |x| x.replace("''", "'"));
            Some(Setting {
                name: k.trim().to_string(),
                value: val,
            })
        })
        .collect()
}

/// Подпись вложенного плана над узлом: `InitPlan 1 (returns $0)`, `SubPlan 2`, `CTE имя`.
fn subplan_label(t: &str) -> Option<&'static str> {
    if t.contains(": ") || t.ends_with(':') {
        return None;
    }
    if t.starts_with("InitPlan ") || t.starts_with("CTE ") {
        Some("InitPlan")
    } else if t.starts_with("SubPlan ") {
        Some("SubPlan")
    } else {
        None
    }
}

/// Узел в стеке разбора: колонка начала имени узла и ключ блока свойств, к которому идут вложенные строки.
struct Frame {
    col: usize,
    node: PlanNode,
    group: Option<String>,
}

fn close(stack: &mut Vec<Frame>, col: usize) {
    while stack.len() > 1 && stack.last().is_some_and(|f| f.col >= col) {
        let done = stack.pop().expect("len > 1").node;
        stack.last_mut().expect("root").node.children.push(done);
    }
}

/// Отношение к родителю, которое текст не печатает: как у JSON — Outer / Inner / Member / Subquery.
fn relationships(n: &mut PlanNode) {
    let member = matches!(
        n.node_type.as_str(),
        "Append" | "Merge Append" | "BitmapAnd" | "BitmapOr"
    );
    let subquery = n.node_type == "Subquery Scan";
    let mut i = 0;
    for c in &mut n.children {
        if c.parent_relationship.is_none() {
            c.parent_relationship = Some(
                if member {
                    "Member"
                } else if subquery {
                    "Subquery"
                } else if i == 0 {
                    "Outer"
                } else {
                    "Inner"
                }
                .to_string(),
            );
            i += 1;
        }
        relationships(c);
    }
}

/// Не выполнявшемуся узлу — нулевые времена, если план с TIMING (так отдаёт JSON).
fn zero_never_executed(n: &mut PlanNode) {
    if n.never_executed
        && let Some(a) = n.actual.as_mut()
    {
        a.startup_ms = Some(0.0);
        a.total_ms = Some(0.0);
    }
    n.children.iter_mut().for_each(zero_never_executed);
}

fn has_times(n: &PlanNode) -> bool {
    n.actual.is_some_and(|a| a.total_ms.is_some()) || n.children.iter().any(has_times)
}

pub fn from_text(text: &str) -> Result<PlanTree, PlanError> {
    let lines = clean(text);
    // Корень — первая строка с заголовком узла; без стоимостей (`COSTS OFF`) — первая строка, если ниже есть `->`.
    let root_at = lines
        .iter()
        .position(|l| has_head_marker(&l.text))
        .or_else(|| lines.iter().any(|l| l.text.starts_with("->")).then_some(0))
        .ok_or_else(|| err("no plan node found", lines.first().map_or(1, |l| l.no)))?;
    // Строки до корня — служебные (журнал auto_explain); текст запроса сохраняется.
    let mut query_text: Option<Vec<&str>> = None;
    for l in &lines[..root_at] {
        if let Some(q) = l.text.strip_prefix("Query Text:") {
            query_text = Some(vec![q.trim()]);
        } else if l.text.starts_with("Query Parameters:") {
            break;
        } else if let Some(q) = query_text.as_mut() {
            q.push(&l.text);
        }
    }
    let root_line = &lines[root_at];
    let root_col = root_line.col;
    let mut root = parse_head(&root_line.text, root_line.no)?;
    if let Some(q) = query_text {
        root.extra
            .insert("Query Text".to_string(), Value::String(q.join("\n")));
    }
    let mut tree = PlanTree {
        root: PlanNode::default(),
        planning_ms: None,
        execution_ms: None,
        triggers: Vec::new(),
        jit: None,
        settings: Vec::new(),
        total_ms: None,
    };
    let mut stack = vec![Frame {
        col: root_col,
        node: root,
        group: None,
    }];
    let mut pending: Option<(String, &'static str)> = None;
    let mut in_tail = false;
    let mut tail = Tail::None;
    for (i, l) in lines.iter().enumerate().skip(root_at + 1) {
        let t = l.text.as_str();
        // Итоги — на уровне корня после дерева; строки с большим отступом за ними — содержимое блока (`JIT:`, `Planning:`).
        if l.col <= root_col || in_tail {
            if l.col <= root_col {
                in_tail = true;
                tail = tail_line(&mut tree, t);
            } else if tail == Tail::Jit
                && let Some(j) = tree.jit.as_mut()
            {
                jit_line(j, t);
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("->") {
            let name = rest.trim_start();
            let col = l.col + t[..t.len() - name.len()].chars().count();
            close(&mut stack, l.col);
            let mut node = parse_head(name, l.no)?;
            if let Some((label, rel)) = pending.take() {
                node.subplan_name = Some(label);
                node.parent_relationship = Some(rel.to_string());
            }
            stack.push(Frame {
                col,
                node,
                group: None,
            });
            continue;
        }
        let next_is_node = lines.get(i + 1).is_some_and(|n| n.text.starts_with("->"));
        if next_is_node && let Some(rel) = subplan_label(t) {
            close(&mut stack, l.col);
            pending = Some((t.to_string(), rel));
            continue;
        }
        close(&mut stack, l.col);
        let top = stack.last_mut().expect("root");
        // Свойства узла — на два пробела глубже имени; глубже — вложенные строки блока (`Worker 0:` в VERBOSE).
        if l.col > top.col + 2 {
            match top.group.clone() {
                Some(g) => push_extra(&mut top.node, &g, t),
                None => push_extra(&mut top.node, "text", t),
            }
            continue;
        }
        top.group = detail(&mut top.node, t);
    }
    close(&mut stack, 0);
    let mut root = stack.pop().expect("root").node;
    relationships(&mut root);
    if has_times(&root) {
        zero_never_executed(&mut root);
    }
    tree.root = root;
    tree.finish();
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PSQL: &str = "\
                                                                QUERY PLAN
------------------------------------------------------------------------------------------------------------------------------
 Hash Join  (cost=10.00..100.00 rows=1000 width=8) (actual time=1.000..50.000 rows=10 loops=1)
   Hash Cond: (o.customer_id = c.id)
   Buffers: shared hit=100 read=20
   ->  Seq Scan on orders o  (cost=0.00..60.00 rows=500 width=8) (actual time=0.100..30.000 rows=5 loops=1)
         Filter: (o.status = 'disputed'::text)
         Rows Removed by Filter: 499995
   ->  Hash  (cost=5.00..5.00 rows=100 width=8) (actual time=0.500..0.500 rows=100 loops=1)
         Buckets: 1024  Batches: 1  Memory Usage: 12kB
         ->  Index Scan using customers_pkey on customers c  (cost=0.30..4.00 rows=100 width=8) (never executed)
               Index Cond: (c.id < 100)
 Planning Time: 0.500 ms
 JIT:
   Functions: 3
   Options: Inlining false, Optimization false, Expressions true, Deforming true
   Timing: Generation 1.000 ms (Deform 0.100 ms), Inlining 0.000 ms, Optimization 2.000 ms, Emission 3.000 ms, Total 6.000 ms
 Execution Time: 55.000 ms
(16 rows)
";

    #[test]
    fn parses_psql_text_into_the_model() {
        let t = from_text(PSQL).unwrap();
        assert_eq!((t.planning_ms, t.execution_ms), (Some(0.5), Some(55.0)));
        let j = t.jit.unwrap();
        assert_eq!(
            (j.functions, j.generation_ms, j.inlining_ms, j.total_ms),
            (3, Some(1.0), Some(0.0), Some(6.0))
        );
        let root = &t.root;
        assert_eq!(
            (
                root.node_type.as_str(),
                root.join_type.as_deref(),
                root.total_cost,
                root.plan_rows,
                root.plan_width
            ),
            ("Hash Join", Some("Inner"), 100.0, 1000.0, 8)
        );
        assert_eq!(root.hash_cond.as_deref(), Some("(o.customer_id = c.id)"));
        assert_eq!(
            root.buffers.map(|b| (b.shared_hit, b.shared_read)),
            Some((100, 20))
        );
        assert_eq!(
            root.actual
                .map(|a| (a.startup_ms, a.total_ms, a.rows, a.loops)),
            Some((Some(1.0), Some(50.0), 10.0, 1.0))
        );
        let seq = &root.children[0];
        assert_eq!(seq.parent_relationship.as_deref(), Some("Outer"));
        assert_eq!(
            seq.relation.as_ref().map(|r| (
                r.schema.as_deref(),
                r.name.as_str(),
                r.alias.as_deref()
            )),
            Some((None, "orders", Some("o")))
        );
        assert_eq!(
            (seq.rows_removed_by_filter, seq.filter.as_deref()),
            (Some(499_995), Some("(o.status = 'disputed'::text)"))
        );
        let hash = &root.children[1];
        assert_eq!(
            (
                hash.node_type.as_str(),
                hash.parent_relationship.as_deref(),
                hash.hash_buckets,
                hash.hash_batches,
                hash.peak_memory_kb
            ),
            ("Hash", Some("Inner"), Some(1024), Some(1), Some(12))
        );
        let idx = &hash.children[0];
        assert_eq!(
            (
                idx.node_type.as_str(),
                idx.index_name.as_deref(),
                idx.never_executed
            ),
            ("Index Scan", Some("customers_pkey"), true)
        );
        // Не выполнявшийся узел в плане с TIMING — нулевые времена, как в JSON.
        assert_eq!(
            idx.actual.map(|a| (a.total_ms, a.loops)),
            Some((Some(0.0), 0.0))
        );
        assert_eq!(idx.index_cond.as_deref(), Some("(c.id < 100)"));
        // Вычисляемые поля — те же, что у JSON.
        assert!(seq.hot && seq.misestimated);
        assert_eq!(seq.inclusive_ms, Some(30.0));
        assert!(
            t.root.extra.is_empty() && seq.extra.is_empty(),
            "{:?}",
            t.root.extra
        );
    }

    #[test]
    fn plain_explain_and_psql_wrapped_value() {
        // План одной строкой (функция, вернувшая текст EXPLAIN): `+` в конце строк, у самой длинной — без пробела.
        let text = "                         QUERY PLAN                          \n\
                    ------------------------------------------------------------\n \
                    Limit  (cost=0.42..1.50 rows=5 width=14)                   +\n   \
                    ->  Index Scan Backward using orders_pkey on orders  (cost=0.42..21.50 rows=100 width=14)+\n         \
                    Index Cond: (id < 100)\n\
                    (1 row)\n";
        let t = from_text(text).unwrap();
        assert_eq!(
            (t.root.node_type.as_str(), t.root.actual, t.execution_ms),
            ("Limit", None, None)
        );
        let idx = &t.root.children[0];
        assert_eq!(
            (
                idx.node_type.as_str(),
                idx.index_name.as_deref(),
                idx.plan_width
            ),
            ("Index Scan", Some("orders_pkey"), 14)
        );
        assert_eq!(idx.extra["Scan Direction"], json!("Backward"));
        assert_eq!(idx.index_cond.as_deref(), Some("(id < 100)"));
    }

    #[test]
    fn node_names_map_back_to_types() {
        let text = "\
Finalize GroupAggregate  (cost=1.00..2.00 rows=3 width=12)
  Group Key: o.status
  ->  Gather Merge  (cost=1.00..2.00 rows=6 width=12)
        Workers Planned: 2
        ->  Partial HashAggregate  (cost=1.00..2.00 rows=3 width=12)
              ->  Parallel Hash Right Anti Join  (cost=1.00..2.00 rows=3 width=12)
                    ->  Parallel Seq Scan on shop.\"Order Items\" \"oi x\"  (cost=0.00..1.00 rows=3 width=12)
                    ->  Parallel Hash  (cost=1.00..1.00 rows=3 width=8)
                          ->  Nested Loop Left Join  (cost=0.00..1.00 rows=3 width=8)
                                ->  Function Scan on pg_catalog.generate_series g  (cost=0.00..1.00 rows=3 width=8)
                                ->  Update on shop.balances b  (cost=0.00..1.00 rows=3 width=8)
                                      ->  Custom Scan (ChunkAppend) on events  (cost=0.00..1.00 rows=3 width=8)
";
        let t = from_text(text).unwrap();
        let mut seen = Vec::new();
        fn walk(n: &PlanNode, out: &mut Vec<String>) {
            out.push(format!(
                "{}|{}|{}|{}|{}",
                n.node_type,
                n.strategy.as_deref().unwrap_or(""),
                n.join_type.as_deref().unwrap_or(""),
                n.parallel_aware,
                n.extra
                    .get("Partial Mode")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ));
            n.children.iter().for_each(|c| walk(c, out));
        }
        walk(&t.root, &mut seen);
        assert_eq!(
            seen,
            [
                "Aggregate|Sorted||false|Finalize",
                "Gather Merge|||false|",
                "Aggregate|Hashed||false|Partial",
                "Hash Join||Right Anti|true|",
                "Seq Scan|||true|",
                "Hash|||true|",
                "Nested Loop||Left|false|",
                "Function Scan|||false|",
                "ModifyTable|||false|",
                "Custom Scan|||false|",
            ]
        );
        let scan = &t.root.children[0].children[0].children[0].children[0];
        assert_eq!(
            scan.relation.as_ref().map(|r| (
                r.schema.as_deref(),
                r.name.as_str(),
                r.alias.as_deref()
            )),
            Some((Some("shop"), "Order Items", Some("oi x")))
        );
        let nl = &t.root.children[0].children[0].children[0].children[1].children[0];
        assert_eq!(
            (
                &nl.children[0].extra["Function Name"],
                &nl.children[0].extra["Schema"],
                &nl.children[0].extra["Alias"]
            ),
            (&json!("generate_series"), &json!("pg_catalog"), &json!("g"))
        );
        let upd = &nl.children[1];
        assert_eq!(upd.extra["Operation"], json!("Update"));
        assert_eq!(
            upd.relation
                .as_ref()
                .map(|r| (r.name.as_str(), r.alias.as_deref())),
            Some(("balances", Some("b")))
        );
        assert_eq!(
            upd.children[0].extra["Custom Plan Provider"],
            json!("ChunkAppend")
        );
        // Псевдоним, совпадающий с именем, сервер не печатает — модель хранит его, как JSON.
        assert_eq!(
            upd.children[0]
                .relation
                .as_ref()
                .map(|r| r.alias.as_deref()),
            Some(Some("events"))
        );
    }

    #[test]
    fn subplans_ctes_and_relationships() {
        let text = "\
CTE Scan on recent r  (cost=16.09..470.03 rows=99 width=16) (actual time=0.045..0.387 rows=299 loops=1)
  Filter: (customer_id > (InitPlan 4).col1)
  CTE recent
    ->  Index Scan using orders_pkey on orders  (cost=0.42..15.64 rows=298 width=16) (actual time=0.007..0.079 rows=299 loops=1)
          Index Cond: (id < 300)
  InitPlan 4
    ->  Result  (cost=0.45..0.46 rows=1 width=8) (actual time=0.017..0.017 rows=1 loops=1)
  SubPlan 2
    ->  Aggregate  (cost=4.51..4.52 rows=1 width=8) (actual time=0.001..0.001 rows=1 loops=299)
          ->  BitmapOr  (cost=0.43..4.50 rows=4 width=0) (actual time=0.000..0.001 rows=3 loops=299)
                ->  Bitmap Index Scan on order_items_order_idx  (cost=0.00..1.00 rows=4 width=0) (actual time=0.000..0.001 rows=3 loops=299)
                      Index Cond: (order_id = r.id)
Planning Time: 0.286 ms
Execution Time: 0.436 ms
";
        let t = from_text(text).unwrap();
        let r = &t.root;
        assert_eq!(
            (&r.extra["CTE Name"], &r.extra["Alias"]),
            (&json!("recent"), &json!("r"))
        );
        let rels: Vec<(Option<&str>, Option<&str>)> = r
            .children
            .iter()
            .map(|c| (c.parent_relationship.as_deref(), c.subplan_name.as_deref()))
            .collect();
        assert_eq!(
            rels,
            [
                (Some("InitPlan"), Some("CTE recent")),
                (Some("InitPlan"), Some("InitPlan 4")),
                (Some("SubPlan"), Some("SubPlan 2"))
            ]
        );
        let or = &r.children[2].children[0];
        assert_eq!(or.parent_relationship.as_deref(), Some("Outer"));
        assert_eq!(
            or.children[0].parent_relationship.as_deref(),
            Some("Member")
        );
        assert_eq!(
            or.children[0].index_name.as_deref(),
            Some("order_items_order_idx")
        );
        assert!(or.children[0].relation.is_none());
        assert_eq!(t.execution_ms, Some(0.436));
    }

    #[test]
    fn unknown_properties_and_worker_blocks_go_to_extra() {
        let text = "\
Sort  (cost=1.00..2.00 rows=3 width=44) (actual time=80.994..80.995 rows=3 loops=3)
  Output: status, (PARTIAL count(*)), coalesce(a, b)
  Sort Key: orders.status, (lower(name, 'a, b'))
  Sort Method: external merge  Disk: 2048kB
  Worker 0:  actual time=78.123..78.123 rows=3 loops=1
    Sort Method: quicksort  Memory: 25kB
    JIT:
      Functions: 6
    Buffers: shared hit=1426
  Worker 1:  Sort Method: quicksort  Memory: 25kB
  Index Searches: 3
  Mystery: something new
  Mystery: twice
  Update on events_2025_01 events_1
  Buffers: shared hit=4183 dirtied=2, local read=1, temp read=5 written=6
  I/O Timings: shared read=0.057 write=0.010, local read=1.000, temp read=0.500 write=0.250
  WAL: records=404 fpi=99 bytes=736193
  ->  HashAggregate  (cost=1.00..2.00 rows=3 width=44) (actual time=80.977..80.978 rows=3 loops=3)
        Planned Partitions: 4  Batches: 5  Memory Usage: 4145kB  Disk Usage: 8216kB
        ->  Memoize  (cost=1.00..2.00 rows=3 width=44) (actual time=0.977..0.978 rows=3 loops=3)
              Hits: 1  Misses: 2  Evictions: 0  Overflows: 0  Memory Usage: 1kB
              ->  Bitmap Heap Scan on orders  (cost=1.00..2.00 rows=3 width=44) (actual time=0.977..0.978 rows=3 loops=3)
                    Heap Blocks: exact=109 lossy=3
                    I/O Timings: read=0.100 write=0.200
Settings: work_mem = '8MB', search_path = 'shop, public', application_name = 'it''s'
Query Identifier: 2223818613946063395
Planning:
  Buffers: shared hit=117 read=1
Planning Time: 0.187 ms
Trigger for constraint orders_customer_fkey on orders: time=0.500 calls=3
Trigger audit: calls=2
";
        let t = from_text(text).unwrap();
        let s = &t.root;
        assert_eq!(s.output, ["status", "(PARTIAL count(*))", "coalesce(a, b)"]);
        assert_eq!(s.sort_key, ["orders.status", "(lower(name, 'a, b'))"]);
        assert_eq!(
            (
                s.sort_method.as_deref(),
                s.sort_space_type.as_deref(),
                s.sort_space_kb
            ),
            (Some("external merge"), Some("Disk"), Some(2048))
        );
        assert_eq!(
            s.extra["Worker 0"],
            json!([
                "actual time=78.123..78.123 rows=3 loops=1",
                "Sort Method: quicksort  Memory: 25kB",
                "JIT:",
                "Functions: 6",
                "Buffers: shared hit=1426"
            ])
        );
        assert_eq!(
            s.extra["Worker 1"],
            json!("Sort Method: quicksort  Memory: 25kB")
        );
        assert_eq!(s.extra["Index Searches"], json!("3"));
        assert_eq!(s.extra["Mystery"], json!(["something new", "twice"]));
        assert_eq!(
            s.extra["text"],
            json!(["Update on events_2025_01 events_1"])
        );
        let b = s.buffers.unwrap();
        assert_eq!(
            (
                b.shared_hit,
                b.shared_dirtied,
                b.local_read,
                b.temp_read,
                b.temp_written
            ),
            (4183, 2, 1, 5, 6)
        );
        assert_eq!((s.io_read_ms, s.io_write_ms), (Some(0.057), Some(0.010)));
        assert_eq!(
            (
                &s.extra["Local I/O Read Time"],
                &s.extra["Temp I/O Write Time"]
            ),
            (&json!(1.0), &json!(0.25))
        );
        assert_eq!(
            (
                &s.extra["WAL Records"],
                &s.extra["WAL FPI"],
                &s.extra["WAL Bytes"]
            ),
            (&json!(404), &json!(99), &json!(736_193))
        );
        let agg = &s.children[0];
        assert_eq!(agg.peak_memory_kb, Some(4145));
        assert_eq!(
            (
                &agg.extra["HashAgg Batches"],
                &agg.extra["Disk Usage"],
                &agg.extra["Planned Partitions"]
            ),
            (&json!(5), &json!(8216), &json!(4))
        );
        let memo = &agg.children[0];
        assert_eq!(
            (memo.peak_memory_kb, &memo.extra["Cache Misses"]),
            (Some(1), &json!(2))
        );
        let bhs = &memo.children[0];
        assert_eq!(
            (bhs.exact_heap_blocks, bhs.lossy_heap_blocks),
            (Some(109), Some(3))
        );
        assert_eq!((bhs.io_read_ms, bhs.io_write_ms), (Some(0.1), Some(0.2)));
        // Итоги: параметры (кавычки внутри значения удвоены), блок планирования не мешает, триггеры.
        assert_eq!(
            t.settings
                .iter()
                .map(|s| (s.name.as_str(), s.value.as_str()))
                .collect::<Vec<_>>(),
            [
                ("work_mem", "8MB"),
                ("search_path", "shop, public"),
                ("application_name", "it's")
            ]
        );
        assert_eq!(t.planning_ms, Some(0.187));
        assert_eq!(
            t.triggers,
            [
                Trigger {
                    name: "orders_customer_fkey".into(),
                    relation: Some("orders".into()),
                    time_ms: 0.5,
                    calls: 3
                },
                Trigger {
                    name: "audit".into(),
                    relation: None,
                    time_ms: 0.0,
                    calls: 2
                }
            ]
        );
    }

    #[test]
    fn auto_explain_log_entry() {
        let text = "2026-11-01 14:23:40.262 UTC [175440] LOG:  duration: 55.579 ms  plan:\n\
            \tQuery Text: SELECT c.region, count(*) FROM shop.customers c\n\
            \t  JOIN shop.orders o ON o.customer_id = c.id\n\
            \tHashAggregate  (cost=2075.93..2075.97 rows=4 width=12) (actual time=55.572..55.573 rows=4 loops=1)\n\
            \t  Group Key: c.region\n\
            \t  ->  Seq Scan on customers c  (cost=0.42..26.92 rows=543 width=12) (actual time=0.011..0.050 rows=499 loops=1)\n\
            \tJIT:\n\
            \t  Functions: 10\n";
        let t = from_text(text).unwrap();
        assert_eq!(
            (t.root.node_type.as_str(), t.root.strategy.as_deref()),
            ("Aggregate", Some("Hashed"))
        );
        assert_eq!(
            t.root.extra["Query Text"],
            json!(
                "SELECT c.region, count(*) FROM shop.customers c\nJOIN shop.orders o ON o.customer_id = c.id"
            )
        );
        assert_eq!(t.root.children.len(), 1);
        assert_eq!(t.jit.map(|j| (j.functions, j.total_ms)), Some((10, None)));
    }

    #[test]
    fn costs_off_and_errors_carry_line_numbers() {
        let t = from_text("Limit\n  ->  Seq Scan on orders\n        Filter: (id > 1)\n").unwrap();
        assert_eq!(t.root.children[0].filter.as_deref(), Some("(id > 1)"));
        let e = from_text("\n\nno plan here").unwrap_err();
        assert_eq!(e.line, Some(3), "{e:?}");
        let e = from_text("Limit  (cost=1.00..2.00 rows=1 width=4)\n  ->  Seq Scan on t  (cost=x..2.00 rows=1 width=4)\n")
            .unwrap_err();
        assert_eq!(e.line, Some(2), "{e:?}");
        assert!(e.message.contains("cost=x..2.00"), "{e:?}");
        assert!(from_text("").is_err());
    }

    #[test]
    fn timing_off_and_never_executed_without_timing() {
        let text = "\
Nested Loop  (cost=1.00..2.00 rows=1 width=4) (actual rows=0 loops=1)
  ->  Seq Scan on a  (cost=0.00..1.00 rows=1 width=4) (actual rows=0 loops=1)
  ->  Seq Scan on b  (cost=0.00..1.00 rows=1 width=4) (never executed)
Planning Time: 0.050 ms
JIT:
  Functions: 2
  Options: Inlining false, Optimization false, Expressions true, Deforming true
Execution Time: 0.100 ms
";
        let t = from_text(text).unwrap();
        assert_eq!(
            t.root.actual.map(|a| (a.total_ms, a.rows, a.loops)),
            Some((None, 0.0, 1.0))
        );
        let b = &t.root.children[1];
        assert!(b.never_executed);
        assert_eq!(b.actual.and_then(|a| a.total_ms), None);
        assert_eq!(t.jit.map(|j| (j.functions, j.total_ms)), Some((2, None)));
        assert_eq!(t.execution_ms, Some(0.1));
        assert!(t.root.share_pct.is_none());
    }

    #[test]
    fn nbsp_indent_counts_characters() {
        // План со страницы или из чата: отступы — неразрывные пробелы (U+00A0, два байта), колонка — в символах.
        let text = "Limit  (cost=0.00..0.02 rows=1 width=4)\n\
            \u{a0}\u{a0}->\u{a0}\u{a0}Seq Scan on t  (cost=0.00..35.50 rows=2550 width=4)\n\
            \u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}Filter: (a > 1)\n";
        let t = from_text(text).unwrap();
        let seq = &t.root.children[0];
        assert_eq!(seq.filter.as_deref(), Some("(a > 1)"));
        assert!(seq.extra.is_empty(), "{:?}", seq.extra);
        let t = from_text(
            "Seq Scan on t  (cost=0.00..35.50 rows=2550 width=4)\n\u{a0}\u{a0}Filter: (a > 1)\n",
        )
        .unwrap();
        assert_eq!(t.root.filter.as_deref(), Some("(a > 1)"));
    }

    #[test]
    fn psql_border_2_frame_is_stripped() {
        let text = "\
+-------------------------------------------------------------+
|                         QUERY PLAN                          |
+-------------------------------------------------------------+
| Limit  (cost=0.00..0.02 rows=1 width=4)                     |
|   ->  Seq Scan on t  (cost=0.00..35.50 rows=2550 width=4)   |
|         Filter: (a > 1)                                     |
+-------------------------------------------------------------+
(3 rows)
";
        let t = from_text(text).unwrap();
        assert_eq!(t.root.node_type, "Limit");
        assert_eq!(t.root.children.len(), 1);
        assert_eq!(t.root.children[0].filter.as_deref(), Some("(a > 1)"));
    }

    #[test]
    fn quoted_lines_of_graphical_clients_are_unquoted() {
        // pgAdmin / DBeaver: каждая строка в кавычках, кавычки внутри удвоены.
        let text = "\"QUERY PLAN\"\n\
            \"Limit  (cost=0.00..0.02 rows=1 width=4)\"\n\
            \"  ->  Seq Scan on \"\"T\"\"  (cost=0.00..35.50 rows=2550 width=4)\"\n\
            \"        Filter: (a > 1)\"\n";
        let t = from_text(text).unwrap();
        assert_eq!(t.root.node_type, "Limit");
        let seq = &t.root.children[0];
        assert_eq!(seq.relation.as_ref().map(|r| r.name.as_str()), Some("T"));
        assert_eq!(seq.filter.as_deref(), Some("(a > 1)"));
    }

    #[test]
    fn node_type_must_start_with_a_letter() {
        // Обрамление, которое разбор не снял, не превращается молча в узел `* Limit`.
        let e = from_text("* Limit  (cost=0.00..0.02 rows=1 width=4)\n").unwrap_err();
        assert_eq!(e.line, Some(1), "{e:?}");
        assert!(e.message.contains("unsupported"), "{e:?}");
    }

    #[test]
    fn dashes_inside_query_text_are_not_the_psql_separator() {
        let text = "2026-11-01 14:23:40.262 UTC [175440] LOG:  duration: 1.000 ms  plan:\n\
            \tQuery Text: SELECT 1\n\
            \t---------------\n\
            \tFROM t\n\
            \tSeq Scan on t  (cost=0.00..35.50 rows=2550 width=4)\n\
            \t  Filter: (a > 1)+\n";
        let t = from_text(text).unwrap();
        assert_eq!(
            t.root.extra["Query Text"],
            json!("SELECT 1\n---------------\nFROM t")
        );
        // Не psql: хвост `+` не снимается как перенос.
        assert_eq!(t.root.filter.as_deref(), Some("(a > 1)+"));
    }
}
