//! Архитектурная граница (спец. P4 §3.1): ядро не зависит ни от драйвера PostgreSQL, ни от HTTP-клиента, ни от
//! разбора SQL, ни от приложения — дотянуться до базы оно не может.

/// Имена зависимостей из всех разделов `[*dependencies]`, включая `[target.'cfg(…)'.dependencies]`.
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut section = String::new();
    let mut deps = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
        } else if section.contains("dependencies")
            && !line.starts_with('#')
            && let Some((name, _)) = line.split_once('=')
        {
            deps.push(name.trim().to_string());
        }
    }
    deps
}

#[test]
fn core_cannot_reach_postgres_or_the_app() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let deps = dependency_names(&manifest);
    assert!(
        deps.iter().any(|d| d == "rmcp"),
        "разбор манифеста: {deps:?}"
    );
    for d in &deps {
        let forbidden = d.contains("tauri")
            || d.starts_with("glafira")
            || [
                "tokio-postgres",
                "postgres",
                "postgres-openssl",
                "reqwest",
                "pg_query",
                "pg-query",
                "pg-advisor",
            ]
            .contains(&d.as_str());
        assert!(!forbidden, "запрещённая зависимость {d}");
    }
}
