//! Тёплая модель Ollama (спец. P4c §6): Glafira предзагружает модель фонового ассистента на старте запуска, держит её в
//! памяти N минут после запуска и умеет её выгрузить. Только локальная Ollama (`localhost`, `127.0.0.1`, `::1`) и только
//! модель из `/api/tags`, не облачная. Запросы короткие (соединение 2 с, ответ 5 с; предзагрузка — до 120 с в фоне);
//! ошибка пишется в журнал приложения и на запуск не влияет.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::models::effective_model;
use super::presets::valid_model;
use super::settings::{AgentSettings, Preset, parse_ollama_url};
use super::warmth::ModelWarmth;
use crate::conn::errors::chain;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::settings::SettingsStore;
use crate::state::AppState;

/// Соединение с Ollama.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Ответ Ollama. Столько же — вся проверка на старте запуска: запуск её ждёт.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
/// Предзагрузка модели — в фоне, запуск её не ждёт.
pub const PRELOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Адрес локальной Ollama: `http(s)://хост[:порт]` (`settings::parse_ollama_url`) с хостом `localhost`, `127.0.0.1` или
/// `::1`. Моделью удалённой Ollama Glafira не управляет (§6.1).
pub fn is_loopback(url: &str) -> bool {
    parse_ollama_url(url).is_some_and(|u| loopback_host(&u))
}

/// URL-парсер приводит хост к одному виду: `LOCALHOST` → `localhost`, `[0:0:0:0:0:0:0:1]` → `[::1]`.
fn loopback_host(u: &reqwest::Url) -> bool {
    matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// Облачная модель Ollama: тег `cloud` (`…:cloud`, `…:120b-cloud`, регистр не важен) или `remote_host` в `/api/show`. По
/// имени — то же правило, что у ключа таблицы цен (`glafira_llm_pricing::model_key`): модель с оценкой стоимости и модель,
/// которую Qwen Code отправляет через Ollama, — одни и те же.
pub fn is_cloud(name: &str, show: &Value) -> bool {
    glafira_llm_pricing::model_key(name).is_some()
        || show
            .get("remote_host")
            .and_then(Value::as_str)
            .is_some_and(|h| !h.is_empty())
}

/// Одна модель Ollama: имя без тега — это `:latest`.
pub fn same_model(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> String {
        if s.contains(':') {
            s.to_string()
        } else {
            format!("{s}:latest")
        }
    }
    norm(a) == norm(b)
}

/// Модель в памяти Ollama (`/api/ps`).
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub name: String,
    /// Когда Ollama выгрузит модель; `None` — срока нет или он не разобрался.
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Deserialize)]
struct Named {
    #[serde(default)]
    name: String,
    #[serde(default)]
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct ModelsBody {
    #[serde(default)]
    models: Vec<Named>,
}

/// HTTP к локальной Ollama: без прокси и переадресаций, без простаивающих соединений (запросы редки).
#[derive(Clone)]
pub struct OllamaClient {
    base: reqwest::Url,
    http: reqwest::Client,
}

impl OllamaClient {
    /// `None` — адрес не локальный (`is_loopback`) или клиент не создаётся.
    pub fn new(url: &str) -> Option<Self> {
        let base = parse_ollama_url(url).filter(loopback_host)?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(RESPONSE_TIMEOUT)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| tracing::warn!("ollama client: {}", chain(&e)))
            .ok()?;
        Some(Self { base, http })
    }

    fn url(&self, path: &str) -> reqwest::Url {
        let mut u = self.base.clone();
        u.set_path(path);
        u
    }

    async fn models(&self, path: &str) -> reqwest::Result<Vec<Named>> {
        let body: ModelsBody = self
            .http
            .get(self.url(path))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(body
            .models
            .into_iter()
            .filter(|m| !m.name.is_empty())
            .collect())
    }

    /// Имена скачанных моделей (`/api/tags`).
    pub async fn tags(&self) -> reqwest::Result<Vec<String>> {
        Ok(self
            .models("/api/tags")
            .await?
            .into_iter()
            .map(|m| m.name)
            .collect())
    }

    /// Модели в памяти и их срок (`/api/ps`).
    pub async fn ps(&self) -> reqwest::Result<Vec<Loaded>> {
        Ok(self
            .models("/api/ps")
            .await?
            .into_iter()
            .map(|m| Loaded {
                expires_at: m
                    .expires_at
                    .as_deref()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok()),
                name: m.name,
            })
            .collect())
    }

    /// Сведения о модели (`/api/show`): у облачной — `remote_host`.
    pub async fn show(&self, model: &str) -> reqwest::Result<Value> {
        self.http
            .post(self.url("/api/show"))
            .json(&json!({ "model": model }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }

    /// Загрузить модель или продлить её срок в памяти, без текста: `keep_alive` — например, `"15m"`; `timeout` — срок
    /// ответа (`PRELOAD_TIMEOUT` у предзагрузки, `RESPONSE_TIMEOUT` у продления).
    pub async fn load(
        &self,
        model: &str,
        keep_alive: &str,
        timeout: Duration,
    ) -> reqwest::Result<()> {
        self.generate(
            json!({ "model": model, "keep_alive": keep_alive, "stream": false }),
            timeout,
        )
        .await
    }

    /// «Выгрузить модель»: `keep_alive: 0`.
    pub async fn unload(&self, model: &str) -> reqwest::Result<()> {
        self.generate(
            json!({ "model": model, "keep_alive": 0, "stream": false }),
            RESPONSE_TIMEOUT,
        )
        .await
    }

    async fn generate(&self, body: Value, timeout: Duration) -> reqwest::Result<()> {
        self.http
            .post(self.url("/api/generate"))
            .timeout(timeout)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(())
    }
}

/// Ошибка Ollama — только в журнал приложения (§6.1).
fn logged<T>(what: &str, r: reqwest::Result<T>) -> Option<T> {
    r.map_err(|e| tracing::warn!("ollama {what}: {}", chain(&e)))
        .ok()
}

/// Glafira управляет моделью Ollama по этим настройкам (§6.1): «Держать модель в памяти» не 0, пресет ходит в Ollama (не
/// Claude Code без Ollama), Ollama локальная. Одно правило для тёплой модели (`managed`) и «Выгрузить модель»
/// (`unload_model`); то же условие у интерфейса — `unloadOff` в `agentForm.ts`.
pub fn manages(s: &AgentSettings) -> bool {
    s.keep_warm_min != 0
        && !(s.preset == Preset::Claude && !s.claude.via_ollama)
        && is_loopback(&s.ollama_url)
}

/// Модель, которой Glafira управляет (§6.1): `manages`, модель задана, скачана (`/api/tags`) и не облачная (тег `cloud`
/// или `remote_host` в `/api/show`).
async fn managed(s: &AgentSettings, model: Option<&str>) -> Option<(OllamaClient, String)> {
    if !manages(s) {
        return None;
    }
    let model = model.map(str::trim).filter(|m| !m.is_empty())?;
    if is_cloud(model, &Value::Null) {
        return None;
    }
    let client = OllamaClient::new(&s.ollama_url)?;
    let tags = logged("/api/tags", client.tags().await)?;
    if !tags.iter().any(|t| same_model(t, model)) {
        return None;
    }
    let show = logged("/api/show", client.show(model).await)?;
    (!is_cloud(model, &show)).then(|| (client, model.to_string()))
}

/// Старт запуска (§6.2, §6.3): модели нет в памяти — предзагрузка в фоне на срок запуска, и, если в памяти другие
/// модели, предупреждение `evict <другие через запятую> <модель>`. Модель уже в памяти — ни предзагрузки, ни
/// предупреждения: вытеснять другие модели незачем.
async fn start_of_run(s: &AgentSettings, model: Option<&str>) -> Vec<String> {
    let Some((client, model)) = managed(s, model).await else {
        return Vec::new();
    };
    let Some(loaded) = logged("/api/ps", client.ps().await) else {
        return Vec::new();
    };
    if loaded.iter().any(|l| same_model(&l.name, &model)) {
        return Vec::new();
    }
    let keep_alive = format!("{}m", s.timeout_min);
    let name = model.clone();
    tokio::spawn(async move {
        if let Err(e) = client.load(&name, &keep_alive, PRELOAD_TIMEOUT).await {
            tracing::warn!("ollama preload {name}: {}", chain(&e));
        }
    });
    // Имена — для строки `evict` (пробелов и запятых в них нет).
    let others: Vec<&str> = loaded
        .iter()
        .map(|l| l.name.as_str())
        .filter(|n| valid_model(n))
        .collect();
    if others.is_empty() {
        Vec::new()
    } else {
        vec![format!("evict {} {model}", others.join(","))]
    }
}

/// Конец запуска (§6.2): продлить срок до N минут, если по `/api/ps` модель выгрузится раньше (или срок не
/// разобрался). Более долгий срок (`OLLAMA_KEEP_ALIVE=1h`) не укорачивается; выгруженная модель заново не загружается.
async fn end_of_run(s: &AgentSettings, model: Option<&str>) {
    let Some((client, model)) = managed(s, model).await else {
        return;
    };
    let Some(loaded) = logged("/api/ps", client.ps().await) else {
        return;
    };
    let Some(current) = loaded.iter().find(|l| same_model(&l.name, &model)) else {
        return;
    };
    let wanted = OffsetDateTime::now_utc() + time::Duration::minutes(i64::from(s.keep_warm_min));
    if current.expires_at.is_some_and(|at| at >= wanted) {
        return;
    }
    let keep_alive = format!("{}m", s.keep_warm_min);
    if let Err(e) = client.load(&model, &keep_alive, RESPONSE_TIMEOUT).await {
        tracing::warn!("ollama keep_alive {model}: {}", chain(&e));
    }
}

/// Тёплая модель по настройкам фонового ассистента (читаются на каждом запуске).
pub struct OllamaWarmth {
    settings: Arc<SettingsStore>,
}

impl OllamaWarmth {
    pub fn new(settings: Arc<SettingsStore>) -> Self {
        Self { settings }
    }
}

impl ModelWarmth for OllamaWarmth {
    /// Старт запуска: запуск ждёт проверки не дольше `RESPONSE_TIMEOUT`, загрузки модели — не ждёт.
    fn before_run(&self, model: Option<&str>) -> BoxFuture<'_, Vec<String>> {
        let model = model.map(str::to_string);
        Box::pin(async move {
            let s = self.settings.agent();
            tokio::time::timeout(RESPONSE_TIMEOUT, start_of_run(&s, model.as_deref()))
                .await
                .unwrap_or_else(|_| {
                    tracing::warn!(
                        "ollama did not answer within {RESPONSE_TIMEOUT:?}: the run starts without preloading the model"
                    );
                    Vec::new()
                })
        })
    }

    fn after_run(&self, model: Option<&str>) -> BoxFuture<'_, ()> {
        let model = model.map(str::to_string);
        Box::pin(async move {
            let s = self.settings.agent();
            end_of_run(&s, model.as_deref()).await;
        })
    }

    /// `/api/tags` локальной Ollama из настроек; запуск ждёт ответа не дольше `RESPONSE_TIMEOUT`.
    fn has_local_model<'a>(&'a self, model: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let s = self.settings.agent();
            let m = model.trim();
            if m.is_empty() {
                return false;
            }
            tokio::time::timeout(RESPONSE_TIMEOUT, local_tags(&s))
                .await
                .ok()
                .flatten()
                .is_some_and(|tags| tags.iter().any(|t| same_model(t, m)))
        })
    }
}

/// Модели локальной Ollama (`/api/tags`, по алфавиту, без повторов) для поля «Модель» (у Claude Code — список, у Qwen Code
/// — группа «Ollama») и для выбора маршрута Qwen Code. Удалённая Ollama или ошибка — `None` (ошибка — в журнал
/// приложения). «Через Ollama» не проверяется: список у Claude Code интерфейс показывает по черновику.
pub async fn local_tags(s: &AgentSettings) -> Option<Vec<String>> {
    let client = OllamaClient::new(&s.ollama_url)?;
    let mut ids = logged("/api/tags", client.tags().await)?;
    ids.sort();
    ids.dedup();
    Some(ids)
}

/// «Выгрузить модель» (§6.2): модель фонового ассистента — `keep_alive: 0`. Неактивна, пока идёт запуск, и когда Glafira
/// моделью не управляет (`manages`: тогда в Ollama не ходит вовсе). Модели нет в памяти (`/api/ps`) или она облачная —
/// выгружать нечего. `home` — домашний каталог (модель Qwen Code по умолчанию).
pub async fn unload_model(st: &AppState, s: &AgentSettings, home: Option<&Path>) -> AppResult<()> {
    if st.agent.is_busy() {
        return Err(busy());
    }
    if !manages(s) {
        return Err(not_managed());
    }
    let model = {
        // Модель Qwen Code по умолчанию — чтение его настроек (блокирующий вызов).
        let (s, home) = (s.clone(), home.map(Path::to_path_buf));
        tokio::task::spawn_blocking(move || effective_model(&s, home.as_deref()))
            .await
            .ok()
            .flatten()
            .ok_or_else(no_model)?
    };
    let client = OllamaClient::new(&s.ollama_url).ok_or_else(not_managed)?;
    let loaded = client.ps().await.map_err(|e| unavailable(&e))?;
    if is_cloud(&model, &Value::Null) || !loaded.iter().any(|l| same_model(&l.name, &model)) {
        return Ok(());
    }
    client.unload(&model).await.map_err(|e| unavailable(&e))?;
    tracing::info!("ollama: {model} unloaded");
    Ok(())
}

fn busy() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.busy",
        "Wait for the background run to finish"
    )
}

fn no_model() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.noModel",
        "No model to unload: choose the model of the background assistant"
    )
}

fn not_managed() -> AppError {
    app_err!(
        ErrorCode::FeatureUnavailable,
        "errors.agent.notManaged",
        "Glafira does not manage the model: “Keep the model in memory” is 0, Claude Code runs without Ollama, or the Ollama address is not local"
    )
}

fn unavailable(e: &reqwest::Error) -> AppError {
    app_err!(
        ErrorCode::ConnectionFailed,
        "errors.agent.ollamaUnavailable",
        "Ollama did not answer: {{error}}",
        error = chain(e)
    )
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, RecvTimeoutError};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::Notify;
    use tokio::time::Instant;

    use super::*;
    use crate::agent::settings::{ClaudeSettings, Preset};
    use crate::secrets::MemoryStore;

    const MODEL: &str = "qwen3.6-64k:35b-a3b";
    const OTHER: &str = "qwen3.8:27b";
    /// Сколько настоящего времени тест ждёт события фальшивой Ollama: часы tokio стоят, их сроки не сработают.
    const REAL_LIMIT: Duration = Duration::from_secs(10);

    /// Ответ фальшивой Ollama.
    enum Reply {
        Json(u16, Value),
        /// Принять запрос и молчать: соединение открыто, пока Glafira не бросит запрос по сроку.
        Silent,
    }

    fn ok(v: Value) -> Reply {
        Reply::Json(200, v)
    }

    /// Что видела фальшивая Ollama.
    #[derive(Default)]
    struct Seen {
        /// Запросы: «METHOD /path» и тело.
        requests: Mutex<Vec<(String, Value)>>,
        /// Неотвеченные запросы, которые Glafira бросила, и когда (часы tokio).
        abandoned: Mutex<Vec<(String, Instant)>>,
        changed: Notify,
    }

    impl Seen {
        fn calls(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|(r, _)| r.clone())
                .collect()
        }

        fn called(&self, request: &str) -> bool {
            self.calls().iter().any(|c| c == request)
        }

        fn generated(&self) -> Vec<Value> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(r, _)| r == "POST /api/generate")
                .map(|(_, b)| b.clone())
                .collect()
        }

        fn abandoned(&self) -> Vec<(String, Instant)> {
            self.abandoned.lock().unwrap().clone()
        }

        fn clear(&self) {
            self.requests.lock().unwrap().clear();
        }

        /// Ждать, пока `ready` не даст значение: по событиям сервера, не по часам.
        async fn wait<T>(&self, what: &str, ready: impl Fn(&Self) -> Option<T>) -> T {
            within_real_limit(what, async {
                loop {
                    let changed = self.changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if let Some(v) = ready(self) {
                        return v;
                    }
                    changed.await;
                }
            })
            .await
        }

        fn note(&self, push: impl FnOnce(&Self)) {
            push(self);
            self.changed.notify_waiters();
        }
    }

    /// Остановленное время без самохода. Часы стоят (`start_paused`), и, пока жив страж (задача `spawn_blocking`),
    /// tokio не переводит их при простое. Иначе, пока ядро доставляет байты loopback, tokio перевёл бы часы к сроку
    /// запроса reqwest, и запрос «истёк» бы сам. Время идёт только по `tokio::time::advance`.
    struct Frozen {
        _release: mpsc::Sender<()>,
    }

    fn frozen() -> Frozen {
        let (tx, rx) = mpsc::channel::<()>();
        tokio::task::spawn_blocking(move || {
            let _ = rx.recv();
        });
        Frozen { _release: tx }
    }

    /// Не дольше `REAL_LIMIT` настоящего времени: тест не виснет, если события нет.
    async fn within_real_limit<T>(what: &str, fut: impl Future<Output = T>) -> T {
        let (tx, rx) = mpsc::channel::<()>();
        let watchdog = tokio::task::spawn_blocking(move || {
            matches!(rx.recv_timeout(REAL_LIMIT), Err(RecvTimeoutError::Timeout))
        });
        tokio::select! {
            v = fut => {
                drop(tx);
                v
            }
            _ = watchdog => panic!("{what}: nothing within {REAL_LIMIT:?} of real time"),
        }
    }

    /// Дать задачам и соединениям доиграть: 200 мс настоящего времени, часы tokio стоят.
    async fn settle() {
        tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(200)))
            .await
            .unwrap();
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    struct Fake {
        url: String,
        seen: Arc<Seen>,
        _frozen: Frozen,
    }

    /// Фальшивая Ollama на случайном порту `127.0.0.1` (настоящая на 11434 не трогается): `handler(«METHOD /path»,
    /// тело запроса)`. Часы tokio стоят, пока она жива.
    async fn fake(handler: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static) -> Fake {
        let frozen = frozen();
        let handler = Arc::new(handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Seen::default());
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                tokio::spawn(serve(sock, handler.clone(), log.clone()));
            }
        });
        Fake {
            url,
            seen,
            _frozen: frozen,
        }
    }

    async fn serve<H>(mut sock: TcpStream, handler: Arc<H>, seen: Arc<Seen>)
    where
        H: Fn(&str, &Value) -> Reply + Send + Sync + 'static,
    {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let n = sock.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(p) = find(&buf, b"\r\n\r\n") {
                break p + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut first = head.lines().next().unwrap_or_default().split(' ');
        let request = format!(
            "{} {}",
            first.next().unwrap_or_default(),
            first.next().unwrap_or_default()
        );
        let len = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while buf.len() < head_end + len {
            let n = sock.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
        let reply = handler(&request, &body);
        seen.note(|s| s.requests.lock().unwrap().push((request.clone(), body)));
        match reply {
            Reply::Json(status, v) => {
                let text = v.to_string();
                let out = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
            Reply::Silent => {
                // Молчим, пока Glafira не закроет соединение.
                let mut rest = [0u8; 256];
                while sock.read(&mut rest).await.is_ok_and(|n| n > 0) {}
                seen.note(|s| s.abandoned.lock().unwrap().push((request, Instant::now())));
            }
        }
    }

    fn in_minutes(m: i64) -> String {
        (OffsetDateTime::now_utc() + time::Duration::minutes(m))
            .format(&Rfc3339)
            .unwrap()
    }

    fn settings_with(
        url: &str,
        edit: impl FnOnce(&mut AgentSettings),
    ) -> (tempfile::TempDir, Arc<SettingsStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SettingsStore::open(dir.path()));
        let mut s = AgentSettings {
            ollama_url: url.into(),
            ..AgentSettings::default()
        };
        edit(&mut s);
        store.set_agent(s).unwrap();
        (dir, store)
    }

    #[test]
    fn only_a_local_ollama_is_managed() {
        for url in [
            "http://127.0.0.1:11434",
            "http://localhost:11434",
            "http://localhost:11434/",
            "http://[::1]:11434",
            "http://[0:0:0:0:0:0:0:1]:11434",
            "https://localhost",
        ] {
            assert!(is_loopback(url), "{url}");
            assert!(OllamaClient::new(url).is_some(), "{url}");
        }
        for url in [
            "http://10.0.0.5:11434",
            "http://127.0.0.2:11434",
            "http://ollama.local:11434",
            "http://localhost.example.com:11434",
            "ftp://127.0.0.1",
            "127.0.0.1:11434",
            "http://127.0.0.1:11434/api",
            "http://user:secret@127.0.0.1:11434",
            "",
        ] {
            assert!(!is_loopback(url), "{url}");
            assert!(OllamaClient::new(url).is_none(), "{url}");
        }
    }

    #[test]
    fn cloud_models_and_model_names() {
        assert!(is_cloud("gpt-oss:120b-cloud", &json!({})));
        assert!(is_cloud("kimi-k2:cloud", &json!({})));
        assert!(is_cloud(
            "qwen3-coder:480b",
            &json!({ "remote_host": "https://ollama.com:443", "remote_model": "qwen3-coder:480b" })
        ));
        assert!(!is_cloud(
            MODEL,
            &json!({ "details": { "family": "qwen3" } })
        ));
        assert!(!is_cloud("qwen3", &json!({ "remote_host": "" })));
        assert!(!is_cloud("cloud", &Value::Null), "имя без тега");
        // Тег — без учёта регистра, как у ключа таблицы цен (`glafira_llm_pricing::model_key`, итоговое ревью бэкенда):
        // облачная модель с оценкой стоимости и маршрутом Qwen Code через Ollama — одна и та же.
        for name in [
            "kimi-k2:CLOUD",
            "GPT-OSS:120B-Cloud",
            "glm-5.3-flash:cloud",
            "gpt-oss:120b-cloud",
            " deepseek-v4.1-flash:cloud ",
            "qwen3:8b",
            "qwen3",
            "cloud",
            ":cloud",
            "x:-cloud",
            "x:cloudy",
        ] {
            assert_eq!(
                is_cloud(name, &Value::Null),
                glafira_llm_pricing::model_key(name).is_some(),
                "{name}"
            );
        }
        assert!(is_cloud("kimi-k2:CLOUD", &Value::Null));
        assert!(same_model("qwen3", "qwen3:latest"));
        assert!(same_model(MODEL, MODEL));
        assert!(!same_model("qwen3:8b", "qwen3:latest"));
        assert!(!same_model(MODEL, OTHER));
    }

    #[tokio::test(start_paused = true)]
    async fn before_a_run_the_model_is_preloaded_and_other_models_are_named() {
        let fake = fake(|req, _| match req {
            "GET /api/tags" => ok(json!({ "models": [{ "name": MODEL }, { "name": OTHER }] })),
            "POST /api/show" => ok(json!({ "details": { "family": "qwen3" } })),
            "GET /api/ps" => {
                ok(json!({ "models": [{ "name": OTHER, "expires_at": in_minutes(3) }] }))
            }
            "POST /api/generate" => ok(json!({ "done": true, "done_reason": "load" })),
            _ => Reply::Json(404, json!({})),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = OllamaWarmth::new(settings);
        let warnings = within_real_limit("before_run", w.before_run(Some(MODEL))).await;
        assert_eq!(warnings, [format!("evict {OTHER} {MODEL}")]);
        let preload = fake
            .seen
            .wait("preload", |s| s.generated().first().cloned())
            .await;
        assert_eq!(
            preload,
            json!({ "model": MODEL, "keep_alive": "15m", "stream": false }),
            "предзагрузка на срок запуска, без текста: {:?}",
            fake.seen.calls()
        );
    }

    /// Других моделей в памяти несколько: в `evict` — через запятую, в порядке `/api/ps` (строку разбирает
    /// `agentText.ts`); имена, которые не прошли бы `valid_model`, не попадают.
    #[tokio::test(start_paused = true)]
    async fn several_other_models_are_named_with_commas() {
        const THIRD: &str = "llama3.2:3b";
        let fake = fake(|req, _| match req {
            "GET /api/tags" => ok(json!({ "models": [{ "name": MODEL }] })),
            "POST /api/show" => ok(json!({})),
            "GET /api/ps" => ok(json!({
                "models": [{ "name": OTHER }, { "name": "bad name" }, { "name": THIRD }]
            })),
            _ => ok(json!({ "done": true })),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = OllamaWarmth::new(settings);
        let warnings = within_real_limit("before_run", w.before_run(Some(MODEL))).await;
        assert_eq!(warnings, [format!("evict {OTHER},{THIRD} {MODEL}")]);
    }

    /// Ошибка Ollama (код 500, закрытый порт) на старте запуска — только в журнал приложения (§6.1): запуск идёт без
    /// предупреждений и без предзагрузки.
    #[tokio::test(start_paused = true)]
    async fn ollama_errors_before_a_run_give_no_warnings() {
        for broken in ["GET /api/tags", "POST /api/show", "GET /api/ps"] {
            let fake = fake(move |req, _| match req {
                r if r == broken => Reply::Json(500, json!({ "error": "boom" })),
                "GET /api/tags" => ok(json!({ "models": [{ "name": MODEL }] })),
                "POST /api/show" => ok(json!({})),
                "GET /api/ps" => ok(json!({ "models": [{ "name": OTHER }] })),
                _ => ok(json!({ "done": true })),
            })
            .await;
            let (_d, settings) = settings_with(&fake.url, |_| {});
            let w = OllamaWarmth::new(settings);
            let warnings = within_real_limit(broken, w.before_run(Some(MODEL))).await;
            assert!(warnings.is_empty(), "{broken}: {warnings:?}");
            settle().await;
            assert!(
                fake.seen.called(broken),
                "{broken}: {:?}",
                fake.seen.calls()
            );
            assert!(
                fake.seen.generated().is_empty(),
                "{broken}: {:?}",
                fake.seen.calls()
            );
            w.after_run(Some(MODEL)).await;
        }
        // Ollama не запущена: порт 9 закрыт (ни тестовый сервер, ни настоящая Ollama). Часы держит страж фальшивой
        // Ollama, чтобы сроки reqwest не «истекли» сами.
        let _frozen = fake(|_, _| ok(json!({}))).await;
        let (_d, down) = settings_with("http://127.0.0.1:9", |_| {});
        let w = OllamaWarmth::new(down);
        assert!(
            within_real_limit("refused", w.before_run(Some(MODEL)))
                .await
                .is_empty()
        );
        within_real_limit("refused", w.after_run(Some(MODEL))).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_loaded_model_is_not_preloaded_and_gives_no_warning() {
        let fake = fake(|req, _| match req {
            "GET /api/tags" => ok(json!({
                "models": [{ "name": MODEL }, { "name": OTHER }, { "name": "qwen3:latest" }]
            })),
            "GET /api/ps" => ok(json!({
                "models": [
                    { "name": MODEL, "expires_at": in_minutes(4) },
                    { "name": OTHER },
                    { "name": "qwen3:latest" }
                ]
            })),
            _ => ok(json!({})),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = OllamaWarmth::new(settings);
        assert!(w.before_run(Some(MODEL)).await.is_empty());
        // `qwen3` — это `qwen3:latest`.
        assert!(w.before_run(Some("qwen3")).await.is_empty());
        settle().await;
        assert!(fake.seen.generated().is_empty(), "{:?}", fake.seen.calls());
        assert_eq!(
            fake.seen
                .calls()
                .iter()
                .filter(|c| *c == "GET /api/ps")
                .count(),
            2
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cloud_unknown_or_switched_off_models_are_left_alone() {
        let fake = fake(|req, body| match req {
            "GET /api/tags" => ok(json!({
                "models": [{ "name": MODEL }, { "name": "gpt-oss:120b-cloud" }, { "name": "remote:latest" }]
            })),
            "POST /api/show" if body["model"] == "remote:latest" => {
                ok(json!({ "remote_host": "https://ollama.com:443" }))
            }
            "POST /api/show" => ok(json!({})),
            "GET /api/ps" => ok(json!({ "models": [] })),
            _ => ok(json!({})),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = OllamaWarmth::new(settings);
        for m in ["gpt-oss:120b-cloud", "remote:latest", "not-pulled:7b"] {
            assert!(w.before_run(Some(m)).await.is_empty(), "{m}");
            w.after_run(Some(m)).await;
        }
        assert!(
            w.before_run(None).await.is_empty(),
            "модель Claude Code по умолчанию — не Ollama"
        );
        assert!(
            !fake.seen.called("GET /api/ps") && !fake.seen.called("POST /api/generate"),
            "{:?}",
            fake.seen.calls()
        );
        // 0 минут — Glafira в Ollama не ходит вовсе; Claude Code без Ollama — тоже.
        for edit in [
            (|s: &mut AgentSettings| s.keep_warm_min = 0) as fn(&mut AgentSettings),
            |s: &mut AgentSettings| {
                s.preset = Preset::Claude;
                s.claude.model = MODEL.into();
            },
        ] {
            fake.seen.clear();
            let (_d, off) = settings_with(&fake.url, edit);
            let w = OllamaWarmth::new(off);
            assert!(w.before_run(Some(MODEL)).await.is_empty());
            w.after_run(Some(MODEL)).await;
            assert!(fake.seen.calls().is_empty(), "{:?}", fake.seen.calls());
        }
        // Удалённая Ollama — тоже нет (и сети не нужно).
        let (_d, remote) = settings_with("http://10.0.0.5:11434", |_| {});
        let w = OllamaWarmth::new(remote);
        assert!(w.before_run(Some(MODEL)).await.is_empty());
        w.after_run(Some(MODEL)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_run_keep_alive_is_extended_but_never_shortened() {
        let expires: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some(in_minutes(3))));
        let at = expires.clone();
        let fake = fake(move |req, _| match req {
            "GET /api/tags" => ok(json!({ "models": [{ "name": MODEL }] })),
            "POST /api/show" => ok(json!({})),
            "GET /api/ps" => match at.lock().unwrap().clone() {
                Some(e) => ok(json!({ "models": [{ "name": MODEL, "expires_at": e }] })),
                None => ok(json!({ "models": [] })),
            },
            _ => ok(json!({ "done": true })),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = OllamaWarmth::new(settings);
        // Агент через OpenAI-совместимый API оставил срок Ollama по умолчанию (5 мин): продлеваем до 10.
        w.after_run(Some(MODEL)).await;
        assert_eq!(
            fake.seen.generated(),
            [json!({ "model": MODEL, "keep_alive": "10m", "stream": false })]
        );
        // OLLAMA_KEEP_ALIVE=1h — модель и так проживёт дольше 10 минут: не укорачиваем.
        *expires.lock().unwrap() = Some(in_minutes(60));
        w.after_run(Some(MODEL)).await;
        // Модель уже выгружена — заново не загружается.
        *expires.lock().unwrap() = None;
        w.after_run(Some(MODEL)).await;
        assert_eq!(fake.seen.generated().len(), 1, "{:?}", fake.seen.calls());
    }

    /// Запуск не ждёт загрузки модели (§6.2): предзагрузка идёт в фоне до 120 с, а не до 5 с ответа.
    #[tokio::test(start_paused = true)]
    async fn the_preload_goes_on_in_the_background_for_two_minutes_at_most() {
        let fake = fake(|req, _| match req {
            "GET /api/tags" => ok(json!({ "models": [{ "name": MODEL }] })),
            "POST /api/show" => ok(json!({})),
            "GET /api/ps" => ok(json!({ "models": [] })),
            "POST /api/generate" => Reply::Silent,
            _ => ok(json!({})),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = OllamaWarmth::new(settings);
        let start = Instant::now();
        let warnings = within_real_limit(
            "before_run while Ollama loads the model",
            w.before_run(Some(MODEL)),
        )
        .await;
        assert!(warnings.is_empty(), "в памяти других моделей нет");
        fake.seen
            .wait("preload", |s| s.called("POST /api/generate").then_some(()))
            .await;
        tokio::time::advance(PRELOAD_TIMEOUT - Duration::from_secs(1)).await;
        settle().await;
        assert!(
            fake.seen.abandoned().is_empty(),
            "предзагрузка ждёт загрузки дольше срока ответа"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        let (request, at) = fake
            .seen
            .wait("preload given up", |s| s.abandoned().first().cloned())
            .await;
        assert_eq!(request, "POST /api/generate");
        assert!(at - start > PRELOAD_TIMEOUT - Duration::from_secs(1));
    }

    /// Молчащая Ollama задерживает старт запуска не больше чем на 5 с; без ответа `/api/ps` — без предзагрузки.
    #[tokio::test(start_paused = true)]
    async fn a_silent_ollama_holds_the_start_of_a_run_for_five_seconds_at_most() {
        let fake = fake(|req, _| match req {
            "GET /api/tags" => ok(json!({ "models": [{ "name": MODEL }] })),
            "POST /api/show" => ok(json!({})),
            "GET /api/ps" => Reply::Silent,
            _ => ok(json!({})),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = Arc::new(OllamaWarmth::new(settings));
        let run = {
            let w = w.clone();
            tokio::spawn(async move { w.before_run(Some(MODEL)).await })
        };
        fake.seen
            .wait("/api/ps", |s| s.called("GET /api/ps").then_some(()))
            .await;
        tokio::time::advance(RESPONSE_TIMEOUT - Duration::from_secs(1)).await;
        settle().await;
        assert!(!run.is_finished(), "ответа ждём до 5 с");
        tokio::time::advance(Duration::from_secs(2)).await;
        let warnings = within_real_limit("before_run", run).await.unwrap();
        assert!(warnings.is_empty());
        settle().await;
        assert!(fake.seen.generated().is_empty(), "{:?}", fake.seen.calls());
    }

    /// Продление после запуска — с обычным сроком ответа (5 с), а не предзагрузки.
    #[tokio::test(start_paused = true)]
    async fn the_extension_after_a_run_waits_five_seconds_at_most() {
        let fake = fake(|req, _| match req {
            "GET /api/tags" => ok(json!({ "models": [{ "name": MODEL }] })),
            "POST /api/show" => ok(json!({})),
            "GET /api/ps" => {
                ok(json!({ "models": [{ "name": MODEL, "expires_at": in_minutes(3) }] }))
            }
            "POST /api/generate" => Reply::Silent,
            _ => ok(json!({})),
        })
        .await;
        let (_d, settings) = settings_with(&fake.url, |_| {});
        let w = Arc::new(OllamaWarmth::new(settings));
        let end = {
            let w = w.clone();
            tokio::spawn(async move { w.after_run(Some(MODEL)).await })
        };
        fake.seen
            .wait("keep_alive", |s| {
                s.called("POST /api/generate").then_some(())
            })
            .await;
        tokio::time::advance(RESPONSE_TIMEOUT - Duration::from_secs(1)).await;
        settle().await;
        assert!(!end.is_finished());
        tokio::time::advance(Duration::from_secs(2)).await;
        within_real_limit("after_run", end).await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn unload_and_the_model_list_of_claude_through_ollama() {
        let loaded = Arc::new(Mutex::new(true));
        let is_loaded = loaded.clone();
        let fake = fake(move |req, _| match req {
            "GET /api/tags" => ok(json!({ "models": [{ "name": OTHER }, { "name": MODEL }] })),
            "GET /api/ps" if *is_loaded.lock().unwrap() => {
                ok(json!({ "models": [{ "name": MODEL, "expires_at": in_minutes(3) }] }))
            }
            "GET /api/ps" => ok(json!({ "models": [{ "name": OTHER }] })),
            _ => ok(json!({ "done": true })),
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let s = AgentSettings {
            preset: Preset::Claude,
            claude: ClaudeSettings {
                path: "/x/claude".into(),
                model: MODEL.into(),
                via_ollama: true,
                budget_usd: None,
            },
            ollama_url: fake.url.clone(),
            ..AgentSettings::default()
        };
        unload_model(&st, &s, None).await.unwrap();
        assert_eq!(
            fake.seen.generated(),
            [json!({ "model": MODEL, "keep_alive": 0, "stream": false })]
        );
        // Модели нет в памяти — выгружать нечего, Ollama не трогаем.
        *loaded.lock().unwrap() = false;
        fake.seen.clear();
        unload_model(&st, &s, None).await.unwrap();
        assert_eq!(fake.seen.calls(), ["GET /api/ps"]);

        assert_eq!(
            local_tags(&s).await,
            Some(vec![MODEL.to_string(), OTHER.to_string()])
        );
        // Сохранённое «Через Ollama» список не прячет: у Claude Code его показывает интерфейс по черновику (итоговая волна
        // правок смоука P4c), у Qwen Code это группа «Ollama».
        let direct = AgentSettings {
            claude: ClaudeSettings {
                via_ollama: false,
                ..s.claude.clone()
            },
            ..s.clone()
        };
        assert_eq!(local_tags(&direct).await.map(|t| t.len()), Some(2));
        let qwen = AgentSettings {
            preset: Preset::Qwen,
            ..direct.clone()
        };
        assert_eq!(local_tags(&qwen).await.map(|t| t.len()), Some(2));
        let remote = AgentSettings {
            ollama_url: "http://192.168.1.10:11434".into(),
            ..s.clone()
        };
        assert_eq!(local_tags(&remote).await, None);
        // Glafira моделью не управляет (0 минут, Claude Code без Ollama, удалённый адрес) — отказ без запросов к Ollama,
        // как и у тёплой модели (`manages`).
        fake.seen.clear();
        let keep_off = AgentSettings {
            keep_warm_min: 0,
            ..s.clone()
        };
        for off in [&keep_off, &direct, &remote] {
            assert_eq!(
                unload_model(&st, off, None).await.unwrap_err().key,
                "errors.agent.notManaged"
            );
        }
        assert!(fake.seen.calls().is_empty(), "{:?}", fake.seen.calls());
        let no_model = AgentSettings {
            claude: ClaudeSettings {
                model: String::new(),
                ..s.claude.clone()
            },
            ..s.clone()
        };
        assert_eq!(
            unload_model(&st, &no_model, None).await.unwrap_err().key,
            "errors.agent.noModel"
        );
        // Ollama не запущена: порт 9 закрыт (ни тестовый сервер, ни настоящая Ollama).
        let down = AgentSettings {
            ollama_url: "http://127.0.0.1:9".into(),
            ..s.clone()
        };
        let e = within_real_limit("refused", unload_model(&st, &down, None))
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.agent.ollamaUnavailable");
        assert!(e.params["error"].contains("127.0.0.1:9"), "{e:?}");
        assert_eq!(within_real_limit("refused", local_tags(&down)).await, None);
    }
    /// Итоговая волна правок смоука P4c: Qwen Code с моделью не из своих настроек идёт к Ollama, если модель скачана в
    /// локальную Ollama (`/api/tags`; имя без тега — это `:latest`). Не скачана, Ollama молчит, не запущена или адрес не
    /// локальный — нет; у удалённой Ollama сети не нужно.
    #[tokio::test(start_paused = true)]
    async fn a_local_model_is_one_from_the_tags_of_a_local_ollama() {
        let silent = fake(|_, _| Reply::Silent).await;
        let ollama = fake(|req, _| match req {
            "GET /api/tags" => {
                ok(json!({ "models": [{ "name": MODEL }, { "name": "gemma3:latest" }] }))
            }
            _ => ok(json!({})),
        })
        .await;
        let (_d, settings) = settings_with(&ollama.url, |s| s.keep_warm_min = 0);
        let w = OllamaWarmth::new(settings);
        for (m, want) in [
            (MODEL, true),
            (" gemma3 ", true),
            ("gemma3:latest", true),
            (OTHER, false),
            ("", false),
        ] {
            assert_eq!(w.has_local_model(m).await, want, "{m}");
        }
        // «Держать модель» 0 маршруту не мешает: он про адрес, а не про тёплую модель.
        assert!(ollama.seen.called("GET /api/tags"));
        let (_d, s) = settings_with(&silent.url, |_| {});
        let w = Arc::new(OllamaWarmth::new(s));
        let ask = tokio::spawn({
            let w = w.clone();
            async move { w.has_local_model(MODEL).await }
        });
        silent
            .seen
            .wait("tags", |s| s.called("GET /api/tags").then_some(()))
            .await;
        tokio::time::advance(RESPONSE_TIMEOUT + Duration::from_secs(1)).await;
        assert!(!within_real_limit("silent", ask).await.unwrap());
        let (_d, down) = settings_with("http://127.0.0.1:9", |_| {});
        assert!(
            !within_real_limit("refused", OllamaWarmth::new(down).has_local_model(MODEL)).await
        );
        let (_d, remote) = settings_with("http://10.0.0.5:11434", |_| {});
        assert!(!OllamaWarmth::new(remote).has_local_model(MODEL).await);
    }
}
