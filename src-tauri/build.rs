fn git_commit() {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GIT_COMMIT={commit}");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    // HEAD обычно ссылается на ветку: без слежения за самой веткой коммит в «О программе» устаревал бы
    // до следующей смены HEAD. Следим только за существующими файлами: после `git gc` ссылка лежит в
    // packed-refs, а отсутствующий файл заставлял бы cargo пересобирать crate при каждой сборке.
    if let Ok(head) = std::fs::read_to_string("../.git/HEAD")
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        for path in [
            format!("../.git/{reference}"),
            "../.git/packed-refs".to_string(),
        ] {
            if std::path::Path::new(&path).is_file() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}

fn main() {
    git_commit();
    tauri_build::build()
}
