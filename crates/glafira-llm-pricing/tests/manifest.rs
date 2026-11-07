//! Граница крейта: он встраивается в любой инструмент семейства Glafira без правок, поэтому не зависит ни от кода
//! Glafira for PostgreSQL (приложение, другие крейты репозитория), ни от HTTP-клиента: страницу цен загружает
//! приложение.

/// Имена зависимостей из всех разделов `[*dependencies]`, включая `[target.'cfg(…)'.dependencies]`.
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut section = String::new();
    let mut deps = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
        } else if section.contains("dependencies")
            && !section.contains("dev-dependencies")
            && !line.starts_with('#')
            && let Some((name, _)) = line.split_once('=')
        {
            deps.push(name.trim().to_string());
        }
    }
    deps
}

#[test]
fn the_crate_depends_only_on_serde_and_time() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let mut deps = dependency_names(&manifest);
    deps.sort();
    assert_eq!(deps, ["serde", "serde_json", "time"], "зависимости крейта");
    assert!(
        !manifest.contains("path ="),
        "крейт не зависит от других крейтов репозитория"
    );
}
