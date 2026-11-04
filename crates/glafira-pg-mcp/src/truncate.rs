//! Усечение ответа до 100 КБ (спец. P4 §10.1): в `data` отчёта самый длинный массив урезается вдвое, пока ответ не
//! поместится. Не помещается и после этого — `Err(())`, обвязка отвечает `response_too_large`. Списки других
//! инструментов ограничены своими `limit`.

use glafira_mcp_core::limits::HARD_LIMIT_BYTES;
use serde_json::Value;

/// Предел ответа после усечения: запас под `truncated: true` и `hint` до [`HARD_LIMIT_BYTES`].
pub const FIT_LIMIT: usize = HARD_LIMIT_BYTES - 512;
pub const TRUNCATED_HINT: &str =
    "The response was truncated to 100 KB; narrow the request: member, filters, limit";

/// Длина JSON; ошибка сериализации — «бесконечно длинный».
pub fn json_len(v: &Value) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

/// Шаг пути к массиву внутри `data`.
#[derive(Debug, Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// Урезает массивы в `value["data"]`, пока JSON всего `value` не станет не длиннее `limit`. Второе значение — что-то
/// урезано. Ошибка одна: не помещается и без массивов.
#[allow(clippy::result_unit_err)]
pub fn fit(mut value: Value, limit: usize) -> Result<(Value, bool), ()> {
    let mut truncated = false;
    loop {
        if json_len(&value) <= limit {
            return Ok((value, truncated));
        }
        let Some(path) = value.get("data").and_then(largest_array) else {
            return Err(());
        };
        let Some(Value::Array(items)) = value.get_mut("data").and_then(|d| descend(d, &path))
        else {
            return Err(());
        };
        items.truncate(items.len() / 2);
        truncated = true;
    }
}

/// Путь к массиву, который урезать: самый большой по JSON среди массивов из двух и более элементов, иначе самый
/// большой из одного элемента (так урезается вложенный массив, а не единственный узел над ним).
fn largest_array(data: &Value) -> Option<Vec<Step>> {
    let mut best: [Option<(usize, Vec<Step>)>; 2] = [None, None];
    walk(data, &mut Vec::new(), &mut best);
    let [many, one] = best;
    many.or(one).map(|(_, path)| path)
}

fn walk(v: &Value, path: &mut Vec<Step>, best: &mut [Option<(usize, Vec<Step>)>; 2]) {
    match v {
        Value::Array(items) => {
            if !items.is_empty() {
                let slot = usize::from(items.len() == 1);
                let size = json_len(v);
                if best[slot].as_ref().is_none_or(|(s, _)| size > *s) {
                    best[slot] = Some((size, path.clone()));
                }
            }
            for (i, item) in items.iter().enumerate() {
                path.push(Step::Index(i));
                walk(item, path, best);
                path.pop();
            }
        }
        Value::Object(map) => {
            for (k, item) in map {
                path.push(Step::Key(k.clone()));
                walk(item, path, best);
                path.pop();
            }
        }
        _ => {}
    }
}

fn descend<'a>(mut v: &'a mut Value, path: &[Step]) -> Option<&'a mut Value> {
    for step in path {
        v = match step {
            Step::Key(k) => v.get_mut(k.as_str())?,
            Step::Index(i) => v.get_mut(*i)?,
        };
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(n: usize, width: usize) -> Vec<Value> {
        (0..n)
            .map(|i| json!({"pid": i, "query": "q".repeat(width)}))
            .collect()
    }

    #[test]
    fn small_value_is_left_alone() {
        let v = json!({"tool": "locks", "data": {"rows": rows(3, 10)}});
        assert_eq!(fit(v.clone(), FIT_LIMIT), Ok((v, false)));
    }

    #[test]
    fn longest_array_in_data_is_halved_until_it_fits() {
        let v = json!({
            "tool": "sessions",
            "findings": rows(5, 10),
            "data": {"sessions": rows(4000, 100), "waits": rows(10, 10), "summary": {"total": 4000}},
        });
        let (out, truncated) = fit(v, FIT_LIMIT).unwrap();
        assert!(truncated);
        assert!(json_len(&out) <= FIT_LIMIT, "{}", json_len(&out));
        let n = out["data"]["sessions"].as_array().unwrap().len();
        assert!(n > 0 && n < 4000, "{n}");
        assert_eq!(
            out["data"]["waits"].as_array().unwrap().len(),
            10,
            "короткий массив цел"
        );
        assert_eq!(out["data"]["summary"]["total"], 4000);
        assert_eq!(
            out["findings"].as_array().unwrap().len(),
            5,
            "вне data не урезается"
        );
    }

    #[test]
    fn nested_array_is_cut_when_the_outer_has_one_element() {
        let v = json!({"data": {"members": [{"name": "pg-1", "slots": rows(3000, 100)}]}});
        let (out, truncated) = fit(v, FIT_LIMIT).unwrap();
        assert!(truncated);
        assert_eq!(
            out["data"]["members"].as_array().unwrap().len(),
            1,
            "узел остаётся"
        );
        let slots = out["data"]["members"][0]["slots"].as_array().unwrap().len();
        assert!(slots > 0 && slots < 3000, "{slots}");
    }

    #[test]
    fn value_without_arrays_does_not_fit() {
        let v = json!({"data": {"text": "x".repeat(FIT_LIMIT + 1)}});
        assert_eq!(fit(v, FIT_LIMIT), Err(()));
        let no_data = json!({"text": "x".repeat(FIT_LIMIT + 1), "rows": rows(10, 10)});
        assert_eq!(
            fit(no_data, FIT_LIMIT),
            Err(()),
            "массивы вне data не трогаются"
        );
    }

    #[test]
    fn fit_limit_leaves_room_for_the_hint() {
        assert!(FIT_LIMIT + 2 * TRUNCATED_HINT.len() < glafira_mcp_core::limits::HARD_LIMIT_BYTES);
    }
}
