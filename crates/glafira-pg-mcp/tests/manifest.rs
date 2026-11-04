//! Архитектурная граница (спец. P4 §3.1): крейт инструментов не может дотянуться до базы, HTTP, разбора SQL и
//! приложения — только через трейт `PgBackend`.

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
fn tools_crate_cannot_reach_postgres_or_the_app() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let deps = dependency_names(&manifest);
    assert!(
        deps.iter().any(|d| d == "rmcp") && deps.iter().any(|d| d == "glafira-mcp-core"),
        "разбор манифеста: {deps:?}"
    );
    for d in &deps {
        let forbidden = d.contains("tauri")
            || [
                "tokio-postgres",
                "postgres",
                "postgres-openssl",
                "reqwest",
                "pg_query",
                "pg-query",
                "pg-advisor",
                "glafira-pg",
            ]
            .contains(&d.as_str());
        assert!(!forbidden, "запрещённая зависимость {d}");
    }
}
