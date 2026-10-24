//! Режим askpass на настоящем бинаре приложения: ssh получает ответ через канал stdout. На Windows бинарь
//! собирается как GUI-приложение, и этот тест доказывает, что stdout при этом доходит до канала.
//! Стенд не нужен: секреты берутся из временного файла (`GLAFIRA_ASKPASS_SECRET_FILE`), keychain не трогается.

use std::path::Path;
use std::process::{Command, Output};

const HOST_KEY_PROMPT: &str = "The authenticity of host 'b (1.2.3.4)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";

/// Запускает бинарь в режиме askpass: окружение как у приложения, `args` — аргументы, которые передал бы ssh.
fn askpass(dir: &Path, trust: Option<&str>, args: &[&str]) -> Output {
    let secrets = dir.join("secrets.json");
    std::fs::write(&secrets, r#"{"sshPassword":"pw-123"}"#).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glafira-pg"));
    cmd.args(args)
        .env("GLAFIRA_ASKPASS", "c1")
        .env("GLAFIRA_ASKPASS_SECRET_FILE", &secrets)
        .env("GLAFIRA_ASKPASS_REPORT", dir.join("report"))
        .env_remove("GLAFIRA_ASKPASS_TRUST");
    if let Some(t) = trust {
        cmd.env("GLAFIRA_ASKPASS_TRUST", t);
    }
    cmd.output().unwrap()
}

#[test]
fn password_prompt_prints_the_secret_without_newline() {
    let dir = tempfile::tempdir().unwrap();
    let out = askpass(dir.path(), None, &["user@host's password: "]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout, b"pw-123");
}

#[test]
fn no_arguments_gives_exit_1_and_empty_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let out = askpass(dir.path(), None, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

#[test]
fn trusted_host_key_is_confirmed_without_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let out = askpass(dir.path(), Some("SHA256:abc"), &[HOST_KEY_PROMPT]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout, b"yes");
    assert!(!dir.path().join("report").exists());
}

#[test]
fn untrusted_host_key_is_refused_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let out = askpass(dir.path(), None, &[HOST_KEY_PROMPT]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout, b"no");
    let report = dir.path().join("report");
    assert_eq!(std::fs::read_to_string(&report).unwrap(), HOST_KEY_PROMPT);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&report).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
