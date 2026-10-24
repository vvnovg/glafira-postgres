//! Режим askpass (спец. P1 §5.1): ssh запускает этот же бинарь с текстом запроса единственным аргументом,
//! а окружение `GLAFIRA_ASKPASS*` говорит, чей секрет отдать. Tauri и WebView не инициализируются.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::net::ssh::process::parse_host_key_report;
use crate::secrets::{
    KeyringStore, MemoryStore, SSH_PASSPHRASE, SSH_PASSWORD, SecretStore, secret_key,
};

/// Файл с секретами вместо keychain (только для тестов): JSON-объект `{поле: значение}` для подключения из
/// `GLAFIRA_ASKPASS`. В обычной работе переменная не задаётся.
const SECRET_FILE_VAR: &str = "GLAFIRA_ASKPASS_SECRET_FILE";

/// Что ssh спрашивает и что на это отвечать.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// Пароль или пассфраза: имя поля секрета подключения.
    Secret(&'static str),
    /// Подтверждение неизвестного ключа хоста; отпечаток, если он есть в тексте запроса.
    HostKey { fingerprint: Option<String> },
    /// Любой другой запрос (код подтверждения, интерактивный ввод): отвечать нечем.
    Unknown,
}

/// Распознаёт запрос ssh по тексту. Ключ хоста проверяется первым: в имени хоста может встретиться «password».
pub fn decide(prompt: &str) -> Answer {
    let lower = prompt.to_lowercase();
    if lower.contains("authenticity of host") || lower.contains("yes', 'no' or the fingerprint") {
        let (_, fingerprint) = parse_host_key_report(prompt);
        return Answer::HostKey { fingerprint };
    }
    if lower.contains("passphrase") {
        return Answer::Secret(SSH_PASSPHRASE);
    }
    if lower.contains("password") {
        return Answer::Secret(SSH_PASSWORD);
    }
    Answer::Unknown
}

/// Окружение, которое приложение передаёт ssh (см. `net::ssh::process`).
pub struct AskpassVars {
    /// Идентификатор подключения (или временный идентификатор проверки) — префикс ключей keychain.
    pub connection_id: String,
    /// Куда записать текст запроса о неизвестном ключе хоста.
    pub report_file: PathBuf,
    /// Отпечаток ключа хоста, который пользователь уже подтвердил в интерфейсе.
    pub trust: Option<String>,
}

/// Код выхода и stdout. Секрет печатается без перевода строки, как ждёт ssh.
pub fn respond(prompt: &str, vars: &AskpassVars, secrets: &dyn SecretStore) -> (i32, String) {
    match decide(prompt) {
        Answer::Secret(field) => match secrets.get(&secret_key(&vars.connection_id, field)) {
            Ok(Some(v)) if !v.is_empty() => (0, v),
            _ => (1, String::new()),
        },
        Answer::HostKey { fingerprint } => {
            let trusted = fingerprint.is_some() && fingerprint == vars.trust;
            if trusted {
                (0, "yes".into())
            } else {
                // Отвечаем «no» и оставляем текст запроса: по нему приложение покажет отпечаток пользователю.
                let _ = crate::storage::fs::write_private(&vars.report_file, prompt.as_bytes());
                (0, "no".into())
            }
        }
        Answer::Unknown => (1, String::new()),
    }
}

/// Секреты из файла `path`. Нечитаемый или неверный файл даёт пустое хранилище, а не keychain: тест, который
/// явно выбрал файл, не должен незаметно обращаться к настоящим секретам пользователя.
fn store_from_file(path: &Path, connection_id: &str) -> MemoryStore {
    let store = MemoryStore::default();
    let fields = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<HashMap<String, String>>(&text).ok())
        .unwrap_or_default();
    for (field, value) in fields {
        let _ = store.set(&secret_key(connection_id, &field), &value);
    }
    store
}

/// Единственное место выбора хранилища: файл секретов, если он задан и это обычный файл, иначе keychain.
fn store_for(secret_file: Option<PathBuf>, connection_id: &str) -> Box<dyn SecretStore> {
    match secret_file {
        Some(path) if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) => {
            Box::new(store_from_file(&path, connection_id))
        }
        _ => Box::new(KeyringStore::default()),
    }
}

fn store_for_env(connection_id: &str) -> Box<dyn SecretStore> {
    store_for(
        std::env::var_os(SECRET_FILE_VAR).map(PathBuf::from),
        connection_id,
    )
}

/// Точка входа режима askpass; `args` — аргументы процесса без имени программы.
pub fn run(args: &[String]) -> i32 {
    let Ok(connection_id) = std::env::var("GLAFIRA_ASKPASS") else {
        return 1;
    };
    let Some(report_file) = std::env::var_os("GLAFIRA_ASKPASS_REPORT").map(PathBuf::from) else {
        return 1;
    };
    let vars = AskpassVars {
        connection_id,
        report_file,
        trust: std::env::var("GLAFIRA_ASKPASS_TRUST")
            .ok()
            .filter(|t| !t.is_empty()),
    };
    let prompt = args.join(" ");
    let secrets = store_for_env(&vars.connection_id);
    let (code, out) = respond(&prompt, &vars, secrets.as_ref());
    if !out.is_empty() {
        use std::io::Write;
        let _ = std::io::stdout().write_all(out.as_bytes());
        let _ = std::io::stdout().flush();
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_recognised() {
        assert_eq!(
            decide("tester@bastion's password: "),
            Answer::Secret(SSH_PASSWORD)
        );
        assert_eq!(
            decide("Enter passphrase for key '/k/id_ed25519': "),
            Answer::Secret(SSH_PASSPHRASE)
        );
        assert_eq!(
            decide(
                "The authenticity of host 'bastion (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?"
            ),
            Answer::HostKey {
                fingerprint: Some("SHA256:abc".into())
            }
        );
        assert_eq!(
            decide("Please type 'yes', 'no' or the fingerprint: "),
            Answer::HostKey { fingerprint: None }
        );
        assert_eq!(decide("Verification code: "), Answer::Unknown);
    }

    #[test]
    fn responses_follow_the_spec() {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report");
        let secrets = MemoryStore::default();
        secrets.set(&secret_key("c1", SSH_PASSWORD), "pw").unwrap();
        let vars = |trust: Option<&str>| AskpassVars {
            connection_id: "c1".into(),
            report_file: report.clone(),
            trust: trust.map(String::from),
        };
        assert_eq!(
            respond("tester@bastion's password: ", &vars(None), &secrets),
            (0, "pw".into())
        );
        // Секрета нет — код 1, stdout пуст.
        assert_eq!(
            respond("Enter passphrase for key '/k': ", &vars(None), &secrets),
            (1, String::new())
        );
        let prompt = "The authenticity of host 'b (1.2.3.4)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
        assert_eq!(
            respond(prompt, &vars(Some("SHA256:abc")), &secrets),
            (0, "yes".into())
        );
        assert_eq!(
            respond(prompt, &vars(Some("SHA256:other")), &secrets),
            (0, "no".into())
        );
        assert_eq!(std::fs::read_to_string(&report).unwrap(), prompt);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&report).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            respond("Verification code: ", &vars(None), &secrets),
            (1, String::new())
        );
    }

    #[test]
    fn secret_file_replaces_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secrets.json");
        std::fs::write(&file, r#"{"sshPassword":"from-file","sshPassphrase":"pp"}"#).unwrap();
        let store = store_for(Some(file.clone()), "c7");
        assert_eq!(
            store
                .get(&secret_key("c7", SSH_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("from-file")
        );
        assert_eq!(
            store
                .get(&secret_key("c7", SSH_PASSPHRASE))
                .unwrap()
                .as_deref(),
            Some("pp")
        );
        // Ключи привязаны к подключению: чужой идентификатор секрета не видит.
        assert_eq!(store.get(&secret_key("c8", SSH_PASSWORD)).unwrap(), None);
        // Отвечает именно этот файл: askpass печатает пароль из него.
        let vars = AskpassVars {
            connection_id: "c7".into(),
            report_file: dir.path().join("report"),
            trust: None,
        };
        assert_eq!(
            respond("tester@bastion's password: ", &vars, store.as_ref()),
            (0, "from-file".into())
        );
        // Файл есть, но не JSON-объект строк: пустое хранилище, а не keychain.
        std::fs::write(&file, "not json").unwrap();
        let broken = store_for(Some(file), "c7");
        assert_eq!(broken.get(&secret_key("c7", SSH_PASSWORD)).unwrap(), None);
    }
}
