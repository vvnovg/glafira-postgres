//! Файл таблицы цен: запись (0600, атомарно), чтение, выбор действующей таблицы.

use std::path::Path;

use glafira_llm_pricing::{
    Origin, ParseError, PriceTable, StoreError, builtin, effective, load, parse_ymd, save,
};
use time::macros::datetime;

/// Загруженная таблица: встроенная с другой ценой gemma4 и днём `date`.
fn fetched(date: &str) -> PriceTable {
    let mut t = builtin().clone();
    t.date = parse_ymd(date).unwrap();
    t.fetched_at = Some(datetime!(2026-12-01 09:30:00 UTC));
    t.models
        .iter_mut()
        .find(|m| m.model == "gemma4")
        .unwrap()
        .standard
        .input = 0.2;
    t
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_saved_table_is_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ollama-prices.json");
    assert!(load(&path).unwrap().is_none(), "файла нет");
    let t = fetched("2026-12-01");
    save(&path, &t).unwrap();
    assert_eq!(load(&path).unwrap(), Some(t.clone()));
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw["schemaVersion"], 1);
    assert_eq!(raw["date"], "2026-12-01");
    assert_eq!(raw["fetchedAt"], "2026-12-01T09:30:00Z");
    assert_eq!(raw["source"], "https://ollama.com/pricing");
    assert_eq!(raw["models"][0]["model"], "beam");
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);
    // Замена файла: права снова 0600, временных файлов не остаётся.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let mut next = t.clone();
    next.date = parse_ymd("2026-12-02").unwrap();
    save(&path, &next).unwrap();
    assert_eq!(load(&path).unwrap(), Some(next));
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["ollama-prices.json"]);
}

#[test]
fn a_broken_newer_or_implausible_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ollama-prices.json");
    std::fs::write(&path, "{not json").unwrap();
    assert!(matches!(load(&path), Err(StoreError::Format(_))));
    let mut newer = serde_json::to_value(fetched("2026-12-01")).unwrap();
    newer["schemaVersion"] = 2.into();
    std::fs::write(&path, newer.to_string()).unwrap();
    assert!(matches!(load(&path), Err(StoreError::NewerSchema(2))));
    let mut few = fetched("2026-12-01");
    few.models.truncate(3);
    let mut few = serde_json::to_value(few).unwrap();
    few["schemaVersion"] = 1.into();
    std::fs::write(&path, few.to_string()).unwrap();
    assert!(matches!(
        load(&path),
        Err(StoreError::Invalid(ParseError::TooFewModels { found: 3 }))
    ));
    // Таблица неправдоподобная — и записать её нельзя.
    let mut bad = fetched("2026-12-01");
    bad.models[0].standard.output = 5000.0;
    assert!(matches!(
        save(&path, &bad),
        Err(StoreError::Invalid(ParseError::PriceOutOfRange { .. }))
    ));
}

#[test]
fn the_saved_table_wins_over_the_builtin_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ollama-prices.json");
    let e = effective(&path);
    assert_eq!((&e.table, e.origin), (builtin(), Origin::Builtin));
    assert!(e.problem.is_none(), "нет файла — не ошибка");
    let t = fetched("2026-12-01");
    save(&path, &t).unwrap();
    let e = effective(&path);
    assert_eq!((e.table, e.origin), (t, Origin::Saved));
    assert!(e.problem.is_none());
    // Загружена в тот же день, что и встроенная, — тоже действует сохранённая.
    let same_day = fetched("2026-11-07");
    save(&path, &same_day).unwrap();
    assert_eq!(effective(&path).origin, Origin::Saved);
}

/// Страница сменила вёрстку и разобралась частично (не меньше `MIN_MODELS` моделей): модели, которых в ней нет, действующая
/// таблица берёт из встроенной — оценки по ним не пропадают; цены разобранных моделей — со страницы. Файл остаётся как
/// записан (итоговое ревью бэкенда, итоговая волна правок смоука P4c).
#[test]
fn models_missing_from_a_partial_page_come_from_the_builtin_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ollama-prices.json");
    let mut partial = fetched("2026-12-01");
    partial.models.retain(|m| {
        [
            "gemma4",
            "glm-5.3-flash",
            "gpt-oss:20b",
            "minimax-m3.1-flash",
            "nemotron-3-super",
        ]
        .contains(&m.model.as_str())
    });
    assert_eq!(partial.models.len(), 5);
    save(&path, &partial).unwrap();
    assert_eq!(
        load(&path).unwrap(),
        Some(partial.clone()),
        "файл — как записан"
    );
    let e = effective(&path);
    assert_eq!(e.origin, Origin::Saved);
    assert_eq!(
        (e.table.date, e.table.fetched_at),
        (partial.date, partial.fetched_at)
    );
    assert_eq!(e.table.models.len(), builtin().models.len());
    assert_eq!(
        e.table.models[..5],
        partial.models[..],
        "сначала модели страницы"
    );
    assert_eq!(
        e.table.find("gemma4").unwrap().standard.input,
        0.2,
        "цена со страницы"
    );
    let deepseek = e.table.find("deepseek-v4.1-flash").unwrap();
    assert_eq!(deepseek, builtin().find("deepseek-v4.1-flash").unwrap());
    let rest: Vec<&str> = e.table.models[5..]
        .iter()
        .map(|m| m.model.as_str())
        .collect();
    let want: Vec<&str> = builtin()
        .models
        .iter()
        .map(|m| m.model.as_str())
        .filter(|m| !partial.models.iter().any(|p| p.model == *m))
        .collect();
    assert_eq!(rest, want, "дополнение — в порядке встроенной таблицы");
    // Полная загруженная таблица дополнять нечего.
    let full = fetched("2026-12-01");
    save(&path, &full).unwrap();
    assert_eq!(effective(&path).table, full);
}

#[test]
fn an_older_saved_table_gives_way_to_a_newer_builtin_one() {
    // Таблицу загрузили, потом поставили выпуск с более свежей встроенной: свежая и действует.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ollama-prices.json");
    save(&path, &fetched("2026-10-01")).unwrap();
    let e = effective(&path);
    assert_eq!((&e.table, e.origin), (builtin(), Origin::Builtin));
    assert!(e.problem.is_none());
}

#[test]
fn a_broken_file_falls_back_to_the_builtin_table_with_the_problem() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ollama-prices.json");
    std::fs::write(&path, "[]").unwrap();
    let e = effective(&path);
    assert_eq!((&e.table, e.origin), (builtin(), Origin::Builtin));
    assert!(matches!(e.problem, Some(StoreError::Format(_))));
}
