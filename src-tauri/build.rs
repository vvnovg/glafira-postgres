use std::path::Path;

/// Релизная сборка не должна молча принять тестовый открытый ключ лицензий:
/// парный ему подписывающий ключ лежит в репозитории, и Pro-ключ мог бы выпустить кто угодно.
fn check_license_key() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest_dir = Path::new(&manifest_dir);
    let embedded = manifest_dir.join("license-public.key");
    let dev = manifest_dir.join("license-dev-public.key");
    println!("cargo:rerun-if-changed={}", embedded.display());
    println!("cargo:rerun-if-changed={}", dev.display());
    println!("cargo:rerun-if-env-changed=GPG_ALLOW_DEV_LICENSE_KEY");

    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }
    let embedded_bytes = std::fs::read(&embedded)
        .unwrap_or_else(|e| panic!("не удалось прочитать {}: {e}", embedded.display()));
    let Ok(dev_bytes) = std::fs::read(&dev) else {
        // Нет тестового ключа рядом (сборка вне репозитория) — сравнивать не с чем.
        return;
    };
    if embedded_bytes.trim_ascii() != dev_bytes.trim_ascii() {
        return;
    }
    if std::env::var("GPG_ALLOW_DEV_LICENSE_KEY").as_deref() == Ok("1") {
        println!(
            "cargo:warning=релизная сборка с ТЕСТОВЫМ ключом лицензий (GPG_ALLOW_DEV_LICENSE_KEY=1)"
        );
        return;
    }
    panic!(
        "src-tauri/license-public.key совпадает с тестовым ключом src-tauri/license-dev-public.key: \
         релизная сборка приняла бы ключи, подписанные общедоступным dev-signing.key. \
         Положите боевой открытый ключ продукта или задайте GPG_ALLOW_DEV_LICENSE_KEY=1 \
         для локальной проверки релизной сборки."
    );
}

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
}

fn main() {
    check_license_key();
    git_commit();
    tauri_build::build()
}
