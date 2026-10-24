//! Супервизор туннеля (спец. P1 §5): один процесс `ssh -D` на подключение, готовность по порту SOCKS,
//! перезапуск с паузами после падения, доверие ключу хоста, сироты.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use crate::conn::pool::PoolRegistry;
use crate::conn::profile::{ConnectionProfile, SshAuth, SshSettings};
use crate::error::{AppError, AppResult};
use crate::net::ssh::orphans;
use crate::net::ssh::process::{
    self, AskpassEnv, SshArgs, SshChild, SshErrorKind, askpass_supported, classify_stderr,
    ssh_error,
};
use crate::secrets::{SSH_PASSPHRASE, SSH_PASSWORD, SecretStore, secret_key};
use crate::sync::MutexExt;

/// Паузы перед перезапуском упавшего процесса; последняя повторяется.
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// Срок одного перезапуска упавшего процесса (вход на бастион, ответы askpass).
const RESTART_TIMEOUT: Duration = Duration::from_secs(30);

/// Временные id туннелей проверки подключения (`readiness::check`).
const CHECK_ID_PREFIX: &str = "unsaved-";

/// Логи туннелей проверки подключения (временные id `unsaved-…`): у каждой проверки свой файл, их убирает
/// следующий запуск приложения.
const CHECK_LOG_PREFIX: &str = "ssh-unsaved-";

/// Id временных туннелей проверки, под которыми сейчас лежат секреты для askpass (только id, без значений; файл
/// 0600 в каталоге данных). Если приложение упадёт посреди проверки, следующий запуск снимет эти секреты:
/// перечислить записи keychain нельзя.
const STAGED_FILE: &str = "staged-secrets.json";
/// Чтение-изменение-запись `staged-secrets.json` из разных проверок не перемешиваются.
static STAGED_LOCK: Mutex<()> = Mutex::new(());

/// Виды ошибок, которые повтор не исправит (`params.kind`): вход отклонён (пароль сменили, ключ отозвали), ключ
/// хоста не подтверждён или сменился, клиента OpenSSH нет или он стар.
const PERMANENT_KINDS: [&str; 5] = [
    "authFailed",
    "hostKeyUnknown",
    "hostKeyChanged",
    "notFound",
    "tooOld",
];

#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    /// Путь к `ssh` вместо поиска в PATH (тесты: `fake_ssh`).
    pub ssh_bin: Option<PathBuf>,
    /// Свой `known_hosts` (`GLAFIRA_SSH_KNOWN_HOSTS`): тесты не трогают файл пользователя.
    pub known_hosts: Option<PathBuf>,
    pub backoff: Vec<Duration>,
    /// Дополнительное окружение процесса ssh (тесты управляют `fake_ssh`).
    pub extra_env: Vec<(String, String)>,
    /// Секреты для askpass кладутся в файл 0600 рядом с данными, а не читаются из keychain: только там, где
    /// keychain недоступен (CI) — `common::tunnel_registry_for_tests`. В приложении — всегда keychain (спец. §5.1).
    pub stage_secrets_in_file: bool,
    /// Бинарь для `SSH_ASKPASS` вместо текущего процесса: интеграционные тесты — это бинарь libtest, а askpass
    /// умеет только `glafira-pg` (`env!("CARGO_BIN_EXE_glafira-pg")`).
    pub askpass_bin: Option<PathBuf>,
}

impl TunnelConfig {
    pub fn new(data_dir: PathBuf, log_dir: PathBuf) -> Self {
        Self {
            data_dir,
            log_dir,
            ssh_bin: None,
            known_hosts: std::env::var_os("GLAFIRA_SSH_KNOWN_HOSTS").map(PathBuf::from),
            backoff: BACKOFF.to_vec(),
            extra_env: Vec::new(),
            stage_secrets_in_file: false,
            askpass_bin: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TunnelStatus {
    Starting,
    Up { socks: SocketAddr },
    Down { error: AppError, attempt: u32 },
    Closed,
}

/// Живой процесс ssh туннеля. Принадлежит задаче супервизора: `close` прерывает задачу, и при drop процесс
/// завершается, а запись о нём уходит из `tunnels.json`. Так закрытию не нужен рантайм tokio: оно идёт и из
/// главного потока (синхронная команда `disconnect`).
struct Running {
    child: SshChild,
    pid: Option<u32>,
    data_dir: PathBuf,
}

impl Running {
    /// Процесс завершился сам: запись о нём убирается до того, как следующий процесс запишет свою.
    async fn retire(mut self) {
        if let Some(pid) = self.pid.take() {
            let dir = self.data_dir.clone();
            let _ = tokio::task::spawn_blocking(move || orphans::unregister(&dir, pid)).await;
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.child.start_kill();
        if let Some(pid) = self.pid.take() {
            let dir = self.data_dir.clone();
            let unregister = move || {
                let _ = orphans::unregister(&dir, pid);
            };
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    h.spawn_blocking(unregister);
                }
                Err(_) => unregister(),
            }
        }
    }
}

struct Tunnel {
    settings: SshSettings,
    /// Статус с уведомлением об изменениях: «Переподключить» ждёт итога перезапуска.
    status: watch::Sender<TunnelStatus>,
    closed: AtomicBool,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    /// Перезапуск без паузы («Переподключить»); снимает и стоянку супервизора.
    wake: Notify,
}

impl Tunnel {
    fn new(settings: SshSettings, status: TunnelStatus) -> Self {
        Self {
            settings,
            status: watch::channel(status).0,
            closed: AtomicBool::new(false),
            supervisor: Mutex::new(None),
            wake: Notify::new(),
        }
    }

    fn status(&self) -> TunnelStatus {
        self.status.borrow().clone()
    }

    fn set_status(&self, status: TunnelStatus) {
        self.status.send_replace(status);
    }

    /// Итог попытки перезапуска. Пробуждение, пришедшее во время попытки, относилось к ней (её итога и ждёт
    /// `restart_if_down`) и снимается здесь: иначе следующее падение пропустило бы паузу или стоянка снялась бы
    /// сама — лишний вход на бастион. Снятие и смена статуса — под блокировкой записи статуса, а
    /// `restart_if_down` читает статус и будит под блокировкой чтения: пробуждение того, кто уже увидел этот
    /// итог, не снимается.
    fn publish_attempt(&self, status: TunnelStatus) {
        self.status.send_modify(|s| {
            let _ = self.wake.notified().now_or_never();
            *s = status;
        });
    }

    /// Супервизор прерывается; его процесс завершает drop (`Running`).
    fn shut(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.set_status(TunnelStatus::Closed);
        if let Some(h) = self.supervisor.lock_safe().take() {
            h.abort();
        }
    }
}

pub struct TunnelRegistry {
    cfg: TunnelConfig,
    pools: Arc<PoolRegistry>,
    secrets: Arc<dyn SecretStore>,
    tunnels: Mutex<HashMap<String, Arc<Tunnel>>>,
    /// Доверенные отпечатки по цели ssh — до выхода из приложения (отступление 2).
    trust: Mutex<HashMap<String, String>>,
    /// Найденный клиент OpenSSH (неудачный поиск не запоминается: клиент могут установить).
    ssh_path: OnceLock<PathBuf>,
    version: OnceLock<(u32, u32)>,
    /// Сироты прошлого запуска убираются один раз и до первого запуска ssh этого процесса: уборка, начавшаяся
    /// позже открытия туннеля, завершила бы и наш процесс (он уже записан в `tunnels.json`).
    reaped: Arc<OnceLock<usize>>,
}

/// Блокирующая работа (файлы, keychain) вне потоков tokio.
async fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

/// Секреты из файла 0600 (`stage_secrets_in_file`); нет файла или он испорчен — пусто.
fn read_secret_file(path: &Path) -> HashMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn secrets_path(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join(format!("askpass-{id}.secrets"))
}

/// Id из `staged-secrets.json`; нет файла или он испорчен — пусто.
fn read_staged(data_dir: &Path) -> Vec<String> {
    std::fs::read(data_dir.join(STAGED_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Пустой список — файла нет.
fn write_staged(data_dir: &Path, ids: &[String]) -> std::io::Result<()> {
    let path = data_dir.join(STAGED_FILE);
    if ids.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(data_dir)?;
    crate::storage::fs::write_private(&path, &serde_json::to_vec(ids)?)
}

/// Снимает секреты askpass туннеля `id` (keychain или файл 0600); `false` — снять не удалось.
fn remove_staged(cfg: &TunnelConfig, secrets: &dyn SecretStore, id: &str) -> bool {
    if cfg.stage_secrets_in_file {
        return match std::fs::remove_file(secrets_path(&cfg.data_dir, id)) {
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            Ok(()) => true,
        };
    }
    let mut removed = true;
    for f in [SSH_PASSWORD, SSH_PASSPHRASE] {
        removed &= secrets.delete(&secret_key(id, f)).is_ok();
    }
    removed
}

/// Секреты проверок прошлого запуска, оставшиеся после его сбоя: только временные id; что снять не удалось,
/// остаётся в списке до следующего запуска. Блокирующий вызов.
fn remove_staged_of_previous_run(cfg: &TunnelConfig, secrets: &dyn SecretStore) {
    let _g = STAGED_LOCK.lock_safe();
    let ids = read_staged(&cfg.data_dir);
    if ids.is_empty() {
        return;
    }
    let left: Vec<String> = ids
        .into_iter()
        .filter(|id| id.starts_with(CHECK_ID_PREFIX) && !remove_staged(cfg, secrets, id))
        .collect();
    if let Err(e) = write_staged(&cfg.data_dir, &left) {
        tracing::warn!("staged ssh secrets list: {e}");
    }
}

/// Сироты прошлого запуска (`tunnels.json`), логи и секреты его проверок подключения. Блокирующий вызов.
fn reap_previous_run(cfg: &TunnelConfig, secrets: &dyn SecretStore) -> usize {
    let killed = orphans::reap(&cfg.data_dir);
    if let Ok(entries) = std::fs::read_dir(&cfg.log_dir) {
        for e in entries.flatten() {
            if e.file_name()
                .to_string_lossy()
                .starts_with(CHECK_LOG_PREFIX)
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    remove_staged_of_previous_run(cfg, secrets);
    killed
}

/// Перед запуском: каталоги есть, отчёта askpass прошлого запуска нет, лог прошлого запуска перенесён в
/// `ssh-<id>.1.log` (один, для архива диагностики). Текущий лог начинается с пустого: за перезапуски он не
/// растёт, а хвост ошибки читается только из строк этого запуска. Блокирующий вызов.
fn prepare_run(data_dir: &Path, log: &Path, previous_log: &Path, report: &Path) {
    let _ = std::fs::create_dir_all(data_dir);
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::remove_file(report);
    if std::fs::metadata(log).is_ok_and(|m| m.len() > 0) {
        // Не вышло (Windows: файл ещё открыт завершающимся процессом) — хвост всё равно берёт только новые
        // строки (смещение в `process::spawn`), а перенос случится при следующем запуске.
        let _ = std::fs::rename(log, previous_log);
    }
}

/// Отчёт askpass о ключе хоста нужен только для разбора этого запуска: читается и удаляется. Блокирующий вызов.
fn take_report(report: &Path) -> Option<String> {
    let text = std::fs::read_to_string(report).ok();
    let _ = std::fs::remove_file(report);
    text
}

impl TunnelRegistry {
    pub fn new(
        cfg: TunnelConfig,
        pools: Arc<PoolRegistry>,
        secrets: Arc<dyn SecretStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            pools,
            secrets,
            tunnels: Mutex::new(HashMap::new()),
            trust: Mutex::new(HashMap::new()),
            ssh_path: OnceLock::new(),
            version: OnceLock::new(),
            reaped: Arc::new(OnceLock::new()),
        })
    }

    pub fn trust(&self, target: &str, fingerprint: &str) {
        self.trust
            .lock_safe()
            .insert(target.to_string(), fingerprint.to_string());
    }

    /// Сироты прошлого запуска (`tunnels.json`), логи и секреты его проверок подключения: блокирующий вызов при
    /// старте приложения (`spawn_blocking`). Выполняется один раз; первый запуск ssh и первый секрет проверки
    /// дожидаются его. Возвращает число завершённых процессов.
    pub fn reap_orphans(&self) -> usize {
        *self
            .reaped
            .get_or_init(|| reap_previous_run(&self.cfg, self.secrets.as_ref()))
    }

    async fn ensure_reaped(&self) {
        if self.reaped.get().is_some() {
            return;
        }
        let (once, cfg, secrets) = (self.reaped.clone(), self.cfg.clone(), self.secrets.clone());
        off_thread(move || {
            once.get_or_init(|| reap_previous_run(&cfg, secrets.as_ref()));
        })
        .await;
    }

    fn report_file(&self, id: &str) -> PathBuf {
        self.cfg.data_dir.join(format!("askpass-{id}.report"))
    }

    fn secrets_file(&self, id: &str) -> PathBuf {
        secrets_path(&self.cfg.data_dir, id)
    }

    fn log_file(&self, id: &str) -> PathBuf {
        self.cfg.log_dir.join(format!("ssh-{id}.log"))
    }

    fn previous_log_file(&self, id: &str) -> PathBuf {
        self.cfg.log_dir.join(format!("ssh-{id}.1.log"))
    }

    /// Секрет формы для проверки несохранённого профиля: askpass читает его из keychain под id туннеля
    /// (или из файла 0600 — только `stage_secrets_in_file`). Снимается `unstage_secrets`. Временный id
    /// (`unsaved-…`) сначала записывается в `staged-secrets.json`: после сбоя приложения посреди проверки секрет
    /// снимет следующий запуск. Блокирующий вызов.
    pub fn stage_secret(&self, id: &str, field: &str, value: &str) -> AppResult<()> {
        // Секреты прошлого запуска снимаются до первого секрета этого: уборка позже сняла бы и его.
        self.reap_orphans();
        if id.starts_with(CHECK_ID_PREFIX) {
            let _g = STAGED_LOCK.lock_safe();
            let mut ids = read_staged(&self.cfg.data_dir);
            if !ids.iter().any(|i| i == id) {
                ids.push(id.to_string());
                write_staged(&self.cfg.data_dir, &ids)?;
            }
        }
        if self.cfg.stage_secrets_in_file {
            let path = self.secrets_file(id);
            let mut map = read_secret_file(&path);
            map.insert(field.to_string(), value.to_string());
            std::fs::create_dir_all(&self.cfg.data_dir)?;
            crate::storage::fs::write_private(&path, &serde_json::to_vec(&map)?)?;
            Ok(())
        } else {
            self.secrets.set(&secret_key(id, field), value)
        }
    }

    /// Снимает секреты, положенные `stage_secret`. Только для временных id: у сохранённого подключения это
    /// удалило бы его настоящие секреты. Не вышло (keychain недоступен) — id остаётся в `staged-secrets.json`, и
    /// секреты снимет следующий запуск. Блокирующий вызов.
    pub fn unstage_secrets(&self, id: &str) {
        if remove_staged(&self.cfg, self.secrets.as_ref(), id) && id.starts_with(CHECK_ID_PREFIX) {
            let _g = STAGED_LOCK.lock_safe();
            let mut ids = read_staged(&self.cfg.data_dir);
            let before = ids.len();
            ids.retain(|i| i != id);
            if ids.len() != before {
                let _ = write_staged(&self.cfg.data_dir, &ids);
            }
        }
    }

    async fn ssh_bin(&self) -> AppResult<PathBuf> {
        if let Some(b) = &self.cfg.ssh_bin {
            return Ok(b.clone());
        }
        if let Some(b) = self.ssh_path.get() {
            return Ok(b.clone());
        }
        let found = match off_thread(process::find_ssh).await {
            Some(r) => r?,
            None => return Err(AppError::internal_with("ssh lookup task failed")),
        };
        Ok(self.ssh_path.get_or_init(|| found).clone())
    }

    async fn version(&self, bin: &Path) -> AppResult<(u32, u32)> {
        if let Some(v) = self.version.get() {
            return Ok(*v);
        }
        let v = process::ssh_version_with_env(bin, &self.cfg.extra_env).await?;
        let _ = self.version.set(v);
        Ok(v)
    }

    /// Версия клиента OpenSSH (для шага SSH проверки подключения).
    pub async fn openssh_version(&self) -> AppResult<(u32, u32)> {
        let bin = self.ssh_bin().await?;
        self.version(&bin).await
    }

    /// Задан ли секрет `field` для askpass (файл 0600 или keychain).
    async fn has_secret(&self, id: &str, field: &'static str) -> AppResult<bool> {
        if self.cfg.stage_secrets_in_file {
            let path = self.secrets_file(id);
            let map = off_thread(move || read_secret_file(&path))
                .await
                .unwrap_or_default();
            return Ok(map.get(field).is_some_and(|v| !v.is_empty()));
        }
        let (secrets, key) = (self.secrets.clone(), secret_key(id, field));
        let value = crate::conn::run_blocking(move || secrets.get(&key)).await?;
        Ok(value.is_some_and(|v| !v.is_empty()))
    }

    /// Нужен ли запрос через askpass: пароль — всегда, ключ — только с пассфразой (ключ без неё и агент
    /// работают на любой версии OpenSSH).
    async fn needs_askpass(&self, id: &str, s: &SshSettings) -> AppResult<bool> {
        match s.auth {
            SshAuth::Agent => Ok(false),
            SshAuth::Password => Ok(true),
            SshAuth::Key => self.has_secret(id, SSH_PASSPHRASE).await,
        }
    }

    fn askpass_env(&self, id: &str, target: &str) -> AskpassEnv {
        AskpassEnv {
            connection_id: id.to_string(),
            report_file: self.report_file(id),
            trust: self.trust.lock_safe().get(target).cloned(),
            secret_file: self
                .cfg
                .stage_secrets_in_file
                .then(|| self.secrets_file(id)),
            askpass_bin: self.cfg.askpass_bin.clone(),
        }
    }

    fn args(&self, bin: PathBuf, id: &str, settings: &SshSettings, port: u16) -> SshArgs {
        SshArgs {
            bin,
            log_file: self.log_file(id),
            socks_port: port,
            settings: settings.clone(),
            known_hosts: self.cfg.known_hosts.clone(),
            extra_env: self.cfg.extra_env.clone(),
        }
    }

    /// Один запуск процесса до готовности порта SOCKS (внутри — одна повторная попытка на занятом порту).
    async fn spawn_ready(
        &self,
        id: &str,
        settings: &SshSettings,
        bin: &Path,
        deadline: Instant,
    ) -> AppResult<(Running, SocketAddr)> {
        self.ensure_reaped().await;
        let target = settings.target.as_str();
        let mut attempts = 0;
        loop {
            attempts += 1;
            let port = process::free_port()
                .map_err(|e| ssh_error(SshErrorKind::Exited, target, &e.to_string()))?;
            let socks = SocketAddr::from(([127, 0, 0, 1], port));
            let args = self.args(bin.to_path_buf(), id, settings, port);
            let env = self.askpass_env(id, target);
            let (data_dir, log, previous, report) = (
                self.cfg.data_dir.clone(),
                args.log_file.clone(),
                self.previous_log_file(id),
                env.report_file.clone(),
            );
            off_thread(move || prepare_run(&data_dir, &log, &previous, &report)).await;
            let child = process::spawn(&args, &env)?;
            // PID записывается сразу после запуска, а не после готовности. Процесс, который ещё входит на
            // бастион (тот или VPN недоступны — TCP ждёт до срока), иначе не нашла бы ни уборка при выходе
            // (`close_all_blocking`: после `RunEvent::Exit` tao вызывает `process::exit`, и drop с `kill_on_drop`
            // не гарантирован), ни уборка при следующем запуске, — а войдя позже (агент, ключ), он оставил бы на
            // loopback SOCKS-прокси без аутентификации (отступление 8).
            let pid = child.pid();
            if let Some(pid) = pid {
                let (dir, log) = (self.cfg.data_dir.clone(), args.log_file.clone());
                off_thread(move || orphans::register(&dir, pid, &log)).await;
            }
            // Дальше процесс — `Running`: прерванное ожидание (закрытие туннеля, отмена команды) завершит его и
            // уберёт запись.
            let mut running = Running {
                child,
                pid,
                data_dir: self.cfg.data_dir.clone(),
            };
            let ready = running.child.wait_ready(socks, deadline).await;
            let report = env.report_file.clone();
            let report = off_thread(move || take_report(&report)).await.flatten();
            match ready {
                Ok(()) => return Ok((running, socks)),
                Err(code) => {
                    let tail = running.child.stderr_tail();
                    // Процесс завершился сам или по сроку (`wait_ready`): запись о нём снимается.
                    running.retire().await;
                    let kind = classify_stderr(code, &tail, report.as_deref());
                    if kind == SshErrorKind::BindFailed && attempts == 1 {
                        continue;
                    }
                    return Err(ssh_error(kind, target, &tail));
                }
            }
        }
    }

    /// Открывает туннель подключения: живой туннель с теми же настройками возвращает свой порт, упавший или с
    /// другими настройками заменяется. Ожидание — до `timeout` (`connect_timeout_s + 5` у вызывающего).
    /// Неудачный туннель в реестре не остаётся.
    pub async fn open(
        self: &Arc<Self>,
        profile: &ConnectionProfile,
        timeout: Duration,
    ) -> AppResult<SocketAddr> {
        let settings = profile
            .ssh
            .clone()
            .ok_or_else(|| AppError::internal_with("profile has no ssh settings"))?;
        let existing = self.tunnels.lock_safe().get(&profile.id).cloned();
        if let Some(t) = existing {
            if t.settings == settings
                && let TunnelStatus::Up { socks } = t.status()
            {
                return Ok(socks);
            }
            self.close(&profile.id);
        }
        let bin = self.ssh_bin().await?;
        // Версия нужна только для запросов через askpass (`SSH_ASKPASS_REQUIRE` — OpenSSH 8.4+).
        if settings.auth != SshAuth::Agent
            && !askpass_supported(self.version(&bin).await?)
            && self.needs_askpass(&profile.id, &settings).await?
        {
            return Err(ssh_error(SshErrorKind::TooOld, &settings.target, ""));
        }
        let deadline = Instant::now() + timeout;
        let (running, socks) = self
            .spawn_ready(&profile.id, &settings, &bin, deadline)
            .await?;
        let tunnel = Arc::new(Tunnel::new(settings, TunnelStatus::Up { socks }));
        // Супервизор назначается под блокировкой реестра: `close` не увидит туннель без супервизора (такой
        // туннель некому было бы остановить).
        let mut tunnels = self.tunnels.lock_safe();
        let handle = tokio::spawn(Self::supervise(
            self.clone(),
            profile.id.clone(),
            tunnel.clone(),
            running,
        ));
        *tunnel.supervisor.lock_safe() = Some(handle);
        // Параллельное открытие того же подключения (в приложении их нет: открытия идут под замком
        // подключения) — остаётся последнее.
        if let Some(old) = tunnels.insert(profile.id.clone(), tunnel) {
            old.shut();
        }
        Ok(socks)
    }

    /// Следит за процессом: упал — пулы подключения вытеснены (их соединения мертвы), статус Down, пауза по
    /// `backoff`, перезапуск на новом порту; `close` прекращает (задача прерывается, процесс завершает drop).
    ///
    /// Паузы с повтором — только для сбоев, которые проходят сами: обрыв (`exited`), сбой проброса, занятый
    /// порт. Перезапуск, не удавшийся по причине, которую повтор не исправит (`PERMANENT_KINDS`: вход отклонён,
    /// ключ хоста не подтверждён или сменился, клиента нет или он стар), ставит супервизор на стоянку: статус
    /// остаётся Down с этой ошибкой, новых попыток нет до «Переподключить» (`restart_if_down` будит супервизор).
    /// Иначе сменённый пароль или отозванный ключ давали бы неудачный вход раз в 30 с, пока открыто приложение, —
    /// этого хватает `pam_faillock`, блокировке учётной записи AD или fail2ban (а он закрыл бы и другие сессии ssh
    /// пользователя). Так же пулы не повторяют отклонённый пароль PostgreSQL (`PgPool::auth_failure`).
    ///
    /// Пробуждение сокращает только ту паузу (или стоянку), во время которой пришло: пришедшее во время попытки
    /// снимается вместе с публикацией её итога (`Tunnel::publish_attempt`).
    async fn supervise(self: Arc<Self>, id: String, tunnel: Arc<Tunnel>, running: Running) {
        let mut running = Some(running);
        let mut attempt: u32 = 0;
        // Прошлый перезапуск не удался по причине, которую повтор не исправит: ждём только «Переподключить».
        let mut parked = false;
        loop {
            // Есть процесс — ждём его конца; нет (прошлый перезапуск не удался) — сразу к паузе.
            if let Some(mut r) = running.take() {
                let code = r.child.wait().await;
                if tunnel.closed.load(Ordering::SeqCst) {
                    return;
                }
                let tail = r.child.stderr_tail();
                r.retire().await;
                let error = ssh_error(
                    classify_stderr(code, &tail, None),
                    &tunnel.settings.target,
                    &tail,
                );
                tracing::warn!(
                    connection = %id,
                    attempt,
                    details = %tail,
                    "ssh tunnel exited: {}",
                    error.message
                );
                // Сначала пулы, потом статус: увидевший Down не застанет пулы упавшего процесса.
                self.pools.evict_connection(&id);
                tunnel.set_status(TunnelStatus::Down { error, attempt });
            }
            if parked {
                tunnel.wake.notified().await;
            } else {
                let pause = self.cfg.backoff[(attempt as usize).min(self.cfg.backoff.len() - 1)];
                tokio::select! {
                    _ = tokio::time::sleep(pause) => {}
                    _ = tunnel.wake.notified() => {}
                }
            }
            attempt += 1;
            if tunnel.closed.load(Ordering::SeqCst) {
                return;
            }
            let deadline = Instant::now() + RESTART_TIMEOUT;
            let started = match self.ssh_bin().await {
                Ok(bin) => {
                    self.spawn_ready(&id, &tunnel.settings, &bin, deadline)
                        .await
                }
                Err(e) => Err(e),
            };
            match started {
                Ok((r, socks)) => {
                    if tunnel.closed.load(Ordering::SeqCst) {
                        return;
                    }
                    tunnel.publish_attempt(TunnelStatus::Up { socks });
                    running = Some(r);
                    attempt = 0;
                    parked = false;
                }
                Err(error) => {
                    parked = error
                        .params
                        .get("kind")
                        .is_some_and(|k| PERMANENT_KINDS.contains(&k.as_str()));
                    tracing::warn!(
                        connection = %id,
                        attempt,
                        parked,
                        "ssh tunnel restart failed: {}",
                        error.message
                    );
                    tunnel.publish_attempt(TunnelStatus::Down { error, attempt });
                }
            }
        }
    }

    /// Куда идти соединениям подключения `id`: `None` — туннеля нет, `Some` — порт SOCKS; туннель упал — его
    /// ошибка.
    pub fn route(&self, id: &str) -> AppResult<Option<SocketAddr>> {
        let Some(t) = self.tunnels.lock_safe().get(id).cloned() else {
            return Ok(None);
        };
        match &*t.status.borrow() {
            TunnelStatus::Up { socks } => Ok(Some(*socks)),
            TunnelStatus::Down { error, .. } => Err(error.clone()),
            TunnelStatus::Starting | TunnelStatus::Closed => {
                Err(ssh_error(SshErrorKind::Exited, &t.settings.target, ""))
            }
        }
    }

    /// Маршрут для профиля: без ssh — напрямую; с ssh — только через его туннель. Туннеля нет (закрыт, пока шёл
    /// опрос) — ошибка, а не соединение в обход: имя узла разрешилось бы локально, и пароль ушёл бы не туда.
    pub fn route_for(&self, profile: &ConnectionProfile) -> AppResult<Option<SocketAddr>> {
        let Some(ssh) = &profile.ssh else {
            return Ok(None);
        };
        match self.route(&profile.id)? {
            Some(socks) => Ok(Some(socks)),
            None => Err(ssh_error(SshErrorKind::Exited, &ssh.target, "")),
        }
    }

    pub fn status(&self, id: &str) -> Option<TunnelStatus> {
        self.tunnels.lock_safe().get(id).map(|t| t.status())
    }

    /// «Переподключить»: упавший туннель профиля с ssh перезапускается сразу, не дожидаясь паузы (или снимается
    /// со стоянки), и ответ — итог этого перезапуска; живой не трогается. Неудача не снимает туннель с
    /// супервизора: после обрыва попытки идут дальше по паузам, после отказа во входе или ключа хоста — только по
    /// следующему «Переподключить». Туннеля нет (или он с другими настройками) — открывается заново.
    pub async fn restart_if_down(
        self: &Arc<Self>,
        profile: &ConnectionProfile,
        timeout: Duration,
    ) -> AppResult<()> {
        let Some(settings) = &profile.ssh else {
            return Ok(());
        };
        let existing = self.tunnels.lock_safe().get(&profile.id).cloned();
        let Some(t) = existing.filter(|t| t.settings == *settings) else {
            return self.open(profile, timeout).await.map(|_| ());
        };
        let mut rx = t.status.subscribe();
        let seen = {
            // Статус читается и супервизор будится под одной блокировкой чтения: итог попытки, публикуемый под
            // блокировкой записи (`Tunnel::publish_attempt`), не снимет пробуждение того, кто его уже увидел.
            let status = rx.borrow_and_update();
            let seen = match &*status {
                TunnelStatus::Up { .. } => return Ok(()),
                TunnelStatus::Down { attempt, .. } => Some(*attempt),
                TunnelStatus::Starting | TunnelStatus::Closed => None,
            };
            t.wake.notify_one();
            seen
        };
        // Итог — Up, закрытие или новая неудача (номер попытки сменился).
        let outcome = tokio::time::timeout(
            timeout,
            rx.wait_for(|s| match s {
                TunnelStatus::Up { .. } | TunnelStatus::Closed => true,
                TunnelStatus::Down { attempt, .. } => Some(*attempt) != seen,
                TunnelStatus::Starting => false,
            }),
        )
        .await;
        let status = match outcome {
            Ok(Ok(s)) => s.clone(),
            // Срок вышел или туннель уже снят: последнее известное состояние.
            _ => t.status(),
        };
        match status {
            TunnelStatus::Up { .. } => Ok(()),
            TunnelStatus::Down { error, .. } => Err(error),
            TunnelStatus::Starting | TunnelStatus::Closed => {
                Err(ssh_error(SshErrorKind::Exited, &settings.target, ""))
            }
        }
    }

    /// Закрывает туннель. Рантайм не нужен: процесс завершается в задаче супервизора при её прерывании.
    pub fn close(&self, id: &str) {
        let removed = self.tunnels.lock_safe().remove(id);
        if let Some(t) = removed {
            t.shut();
        }
    }

    pub fn close_all(&self) {
        let ids: Vec<String> = self.tunnels.lock_safe().keys().cloned().collect();
        for id in ids {
            self.close(&id);
        }
    }

    /// Id открытых туннелей (тесты других модулей).
    #[cfg(test)]
    pub(crate) fn open_ids(&self) -> Vec<String> {
        self.tunnels.lock_safe().keys().cloned().collect()
    }

    /// Выход из приложения: задачи супервизоров могут уже не выполниться, поэтому процессы завершаются по PID из
    /// `tunnels.json` (с проверкой командной строки, как сироты). Блокирующий вызов; возвращает число завершённых.
    pub fn close_all_blocking(&self) -> usize {
        let tunnels: Vec<Arc<Tunnel>> = self.tunnels.lock_safe().drain().map(|(_, t)| t).collect();
        for t in tunnels {
            t.shut();
        }
        orphans::reap(&self.cfg.data_dir)
    }
}

/// Реестр для тестов без ssh: каталоги не создаются — без профилей с ssh реестр файлов не трогает.
#[cfg(test)]
pub(crate) fn unused_registry(pools: Arc<PoolRegistry>) -> Arc<TunnelRegistry> {
    let dir = std::env::temp_dir().join(format!("glafira-no-tunnels-{}", std::process::id()));
    TunnelRegistry::new(
        TunnelConfig::new(dir.join("data"), dir.join("logs")),
        pools,
        Arc::new(crate::secrets::MemoryStore::default()),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::conn::pool::{PoolKey, PoolPurpose, PoolRegistry};
    use crate::conn::profile::{SshAuth, SshSettings, sample_profile};
    use crate::error::ErrorCode;
    use crate::net::ssh::testutil::fake_ssh_bin;
    use crate::secrets::MemoryStore;

    pub(crate) fn profile(auth: SshAuth) -> ConnectionProfile {
        let mut p = sample_profile("c1");
        p.pg.connect_timeout_s = 2;
        p.ssh = Some(SshSettings {
            target: "bastion".into(),
            port: None,
            user: Some("tester".into()),
            auth,
            key_path: (auth == SshAuth::Key).then(|| "/k/id".to_string()),
        });
        p
    }

    /// Реестр с поддельным ssh: пауза перед перезапуском — 50 мс, окружение `env` — для `fake_ssh`.
    pub(crate) fn registry_with(
        dir: &Path,
        env: Vec<(&str, String)>,
        secrets: Arc<dyn SecretStore>,
        configure: impl FnOnce(&mut TunnelConfig),
    ) -> (Arc<TunnelRegistry>, Arc<PoolRegistry>) {
        let pools = Arc::new(PoolRegistry::default());
        let mut cfg = TunnelConfig::new(dir.join("data"), dir.join("logs"));
        cfg.ssh_bin = Some(fake_ssh_bin());
        cfg.known_hosts = None;
        cfg.backoff = vec![Duration::from_millis(50)];
        cfg.extra_env = env.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        configure(&mut cfg);
        (TunnelRegistry::new(cfg, pools.clone(), secrets), pools)
    }

    pub(crate) fn registry(
        dir: &Path,
        env: Vec<(&str, String)>,
    ) -> (Arc<TunnelRegistry>, Arc<PoolRegistry>) {
        registry_with(dir, env, Arc::new(MemoryStore::default()), |_| {})
    }

    /// Эхо-сервер: что пришло, то и ушло.
    async fn echo_server() -> std::net::SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        addr
    }

    /// Порт, на котором никто не слушает.
    pub(crate) fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Записи о процессах в `tunnels.json` (файла нет — ни одной).
    fn pids(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("data").join("tunnels.json"))
            .unwrap_or_default()
            .matches("\"pid\":")
            .count()
    }

    /// Запуски `fake_ssh` (`FAKE_SSH_RUNS`): попытки входа на бастион.
    fn runs(file: &Path) -> usize {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .count()
    }

    /// Вид ошибки (`params.kind`) и номер попытки упавшего туннеля `c1`.
    fn down(reg: &TunnelRegistry) -> Option<(String, u32)> {
        match reg.status("c1") {
            Some(TunnelStatus::Down { error, attempt }) => Some((
                error.params.get("kind").cloned().unwrap_or_default(),
                attempt,
            )),
            _ => None,
        }
    }

    async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "не дождались: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn tunnel_opens_routes_through_socks_and_closes() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_UPSTREAM", echo.to_string())]);
        let p = profile(SshAuth::Agent);
        let socks = reg.open(&p, Duration::from_secs(5)).await.unwrap();
        assert_eq!(reg.route("c1").unwrap(), Some(socks));
        assert_eq!(reg.route_for(&p).unwrap(), Some(socks));
        // Повторное открытие живого туннеля с теми же настройками — тот же порт, без нового процесса.
        assert_eq!(reg.open(&p, Duration::from_secs(5)).await.unwrap(), socks);
        let mut s = crate::net::ssh::socks::connect(socks, "bastion", "pg-single", 5432)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut s, b"ping")
            .await
            .unwrap();
        let mut b = [0u8; 4];
        tokio::io::AsyncReadExt::read_exact(&mut s, &mut b)
            .await
            .unwrap();
        assert_eq!(&b, b"ping");
        // Лог ssh и запись о PID на месте, пока туннель жив.
        assert!(
            dir.path()
                .join("logs")
                .join("ssh-c1.log")
                .parent()
                .unwrap()
                .exists()
        );
        assert!(
            std::fs::read_to_string(dir.path().join("data").join("tunnels.json"))
                .unwrap()
                .contains("ssh-c1.log")
        );
        assert_eq!(pids(dir.path()), 1);
        reg.close("c1");
        assert_eq!(reg.route("c1").unwrap(), None);
        assert_eq!(reg.status("c1"), None);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            tokio::net::TcpStream::connect(socks).await.is_err(),
            "процесс завершён"
        );
        assert!(
            std::fs::read_to_string(dir.path().join("data").join("tunnels.json"))
                .unwrap()
                .contains("\"pids\":[]")
        );
    }

    /// Узел, до которого бастион не достучался: `ssh -D` отвечает успехом SOCKS и закрывает поток. Это сбой
    /// проброса к узлу — и без TLS (конец потока на стартовом сообщении), и с TLS (на запросе SSLRequest). Порт
    /// самого туннеля закрыт (процесс ssh умер) — туннель не работает (`tunnelDown`, вид `exited`).
    #[tokio::test]
    async fn unreachable_node_behind_the_bastion_is_a_forward_failure() {
        use crate::conn::pg::ConnectSpec;
        use crate::conn::profile::SslMode;
        let dir = tempfile::tempdir().unwrap();
        let (reg, _) = registry(
            dir.path(),
            vec![("FAKE_SSH_UPSTREAM", format!("127.0.0.1:{}", closed_port()))],
        );
        let p = profile(SshAuth::Agent);
        let socks = reg.open(&p, Duration::from_secs(5)).await.unwrap();
        let spec = |mode: SslMode| {
            let mut p = p.clone();
            p.pg.ssl_mode = mode;
            ConnectSpec::build_via(
                &p,
                "pg-single",
                5432,
                "postgres",
                &MemoryStore::default(),
                Some(socks),
            )
            .unwrap()
        };
        for mode in [SslMode::Disable, SslMode::Prefer, SslMode::VerifyFull] {
            let e = spec(mode).connect().await.unwrap_err();
            assert_eq!(
                (e.code, e.key.as_str(), e.params["kind"].as_str()),
                (
                    ErrorCode::SshError,
                    "errors.ssh.forwardFailed",
                    "forwardFailed"
                ),
                "{mode:?}: {e:?}"
            );
            assert_eq!(
                (
                    e.params["target"].as_str(),
                    e.params["host"].as_str(),
                    e.params["port"].as_str()
                ),
                ("bastion", "pg-single", "5432"),
                "{mode:?}: {e:?}"
            );
            assert!(e.details.is_some(), "{e:?}");
            assert!(e.retryable);
        }
        reg.close("c1");
        eventually("процесс завершён", || {
            std::net::TcpStream::connect(socks).is_err()
        })
        .await;
        let e = spec(SslMode::Disable).connect().await.unwrap_err();
        assert_eq!(
            (e.key.as_str(), e.params["kind"].as_str()),
            ("errors.ssh.tunnelDown", "exited"),
            "{e:?}"
        );
        assert_eq!(e.params["port"], socks.port().to_string());
    }

    /// Профиль с ssh без открытого туннеля напрямую не соединяется: имя узла разрешилось бы локально.
    #[tokio::test]
    async fn profile_with_ssh_never_routes_around_a_missing_tunnel() {
        let reg = unused_registry(Arc::default());
        let e = reg.route_for(&profile(SshAuth::Agent)).unwrap_err();
        assert_eq!(
            (e.code, e.params["kind"].as_str()),
            (ErrorCode::SshError, "exited")
        );
        assert_eq!(reg.route_for(&sample_profile("c1")).unwrap(), None);
    }

    #[tokio::test]
    async fn login_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_MODE", "auth".into())]);
        let e = reg
            .open(&profile(SshAuth::Password), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::SshError, "errors.ssh.authFailed")
        );
        assert_eq!(e.params["kind"], "authFailed");
        assert!(
            e.details
                .as_deref()
                .unwrap_or("")
                .contains("Permission denied")
        );
        assert_eq!(
            reg.route("c1").unwrap(),
            None,
            "неудачный туннель не остаётся в реестре"
        );
        assert_eq!(pids(dir.path()), 0, "запись о процессе снята");
    }

    /// PID записывается при запуске, а не после готовности: процесс, который ещё входит на бастион, найдёт
    /// уборка при выходе и при следующем запуске. Готовность не наступила к сроку — процесс завершён, записи нет.
    #[tokio::test]
    async fn pid_is_registered_at_launch_and_removed_when_readiness_fails() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(
            dir.path(),
            vec![
                ("FAKE_SSH_READY_DELAY_MS", "1500".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
            ],
        );
        let p = profile(SshAuth::Agent);
        let opening = tokio::spawn({
            let (reg, p) = (reg.clone(), p.clone());
            async move { reg.open(&p, Duration::from_secs(5)).await }
        });
        eventually("PID записан до готовности", || {
            pids(dir.path()) == 1
        })
        .await;
        assert!(!opening.is_finished(), "порт SOCKS ещё не готов");
        opening.await.unwrap().unwrap();
        assert_eq!(pids(dir.path()), 1);
        reg.close("c1");
        eventually("запись снята при закрытии", || {
            pids(dir.path()) == 0
        })
        .await;
        reg.open(&p, Duration::from_millis(300)).await.unwrap_err();
        assert_eq!(pids(dir.path()), 0, "процесс, не готовый к сроку, забыт");
        assert_eq!(reg.status("c1"), None);
    }

    #[tokio::test]
    async fn unknown_host_key_is_reported_and_trust_lets_the_next_attempt_through() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(
            dir.path(),
            vec![
                ("FAKE_SSH_MODE", "hostkey".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
            ],
        );
        let e = reg
            .open(&profile(SshAuth::Agent), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.ssh.hostKeyUnknown");
        assert_eq!(e.params["kind"], "hostKeyUnknown");
        assert_eq!(e.params["fingerprint"], "SHA256:fake");
        assert_eq!(e.params["host"], "bastion (10.0.0.5)");
        assert_eq!(e.params["target"], "bastion");
        assert!(
            !dir.path().join("data").join("askpass-c1.report").exists(),
            "отчёт askpass прочитан и удалён"
        );
        reg.trust("bastion", "SHA256:fake");
        reg.open(&profile(SshAuth::Agent), Duration::from_secs(5))
            .await
            .unwrap();
        reg.close("c1");
    }

    #[tokio::test]
    async fn busy_local_port_is_retried_once() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let marker = dir.path().join("bind-failed");
        let (reg, _) = registry(
            dir.path(),
            vec![
                (
                    "FAKE_SSH_BIND_FAIL_ONCE",
                    marker.to_string_lossy().into_owned(),
                ),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
            ],
        );
        reg.open(&profile(SshAuth::Agent), Duration::from_secs(5))
            .await
            .unwrap();
        assert!(
            marker.exists(),
            "первая попытка упала на занятом порту, вторая удалась"
        );
        reg.close("c1");
    }

    #[tokio::test]
    async fn dead_tunnel_restarts_with_backoff_and_evicts_pools() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, pools) = registry(
            dir.path(),
            vec![
                ("FAKE_SSH_DIE_MS", "300".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
            ],
        );
        let first = reg
            .open(&profile(SshAuth::Agent), Duration::from_secs(5))
            .await
            .unwrap();
        pools.insert_if_absent(
            PoolKey {
                connection_id: "c1".into(),
                member: "m".into(),
                database: "postgres".into(),
                purpose: PoolPurpose::Catalog,
            },
            crate::conn::pg::ConnectSpec::build(
                &sample_profile("c1"),
                "h",
                1,
                "postgres",
                &MemoryStore::default(),
            )
            .unwrap(),
        );
        // Процесс умирает через 300 мс: статус Down с ошибкой, пулы вытеснены, маршрута нет.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(TunnelStatus::Down { .. }) = reg.status("c1") {
                break;
            }
            assert!(Instant::now() < deadline, "не дождались падения");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(pools.keys().is_empty());
        assert_eq!(reg.route("c1").unwrap_err().params["kind"], "exited");
        // После паузы (50 мс в тестах) процесс перезапущен на новом порту.
        loop {
            if let Some(TunnelStatus::Up { socks }) = reg.status("c1") {
                assert_ne!(socks, first);
                break;
            }
            assert!(Instant::now() < deadline, "не дождались перезапуска");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Запись об упавшем процессе убрана до записи нового (новый записан ещё при запуске, до готовности): в
        // реестре сирот один живой процесс.
        assert_eq!(pids(dir.path()), 1);
        reg.close("c1");
    }

    /// Перезапуск, отклонённый бастионом (пароль сменили, ключ отозвали), ставит супервизор на стоянку: новых
    /// неудачных входов нет до «Переподключить», а оно даёт ровно одну попытку.
    #[tokio::test]
    async fn refused_login_parks_the_supervisor_until_reconnect() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let marker = dir.path().join("first-run");
        let log = dir.path().join("runs");
        let (reg, _) = registry(
            dir.path(),
            vec![
                ("FAKE_SSH_DIE_MS", "300".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
                (
                    "FAKE_SSH_AUTH_AFTER_MARKER",
                    marker.to_string_lossy().into_owned(),
                ),
                ("FAKE_SSH_RUNS", log.to_string_lossy().into_owned()),
            ],
        );
        // Пауза перед перезапуском в тестах — 50 мс; ждём втрое-вшестеро дольше.
        let quiet = Duration::from_millis(300);
        let p = profile(SshAuth::Agent);
        reg.open(&p, Duration::from_secs(5)).await.unwrap();
        // Процесс умер; первый перезапуск отклонён — и всё.
        eventually(
            "отказ во входе при перезапуске",
            || down(&reg).is_some_and(|(kind, _)| kind == "authFailed"),
        )
        .await;
        assert_eq!(down(&reg), Some(("authFailed".to_string(), 1)));
        tokio::time::sleep(quiet).await;
        assert_eq!(
            down(&reg),
            Some(("authFailed".to_string(), 1)),
            "без новых попыток"
        );
        assert_eq!(runs(&log), 2, "открытие и один перезапуск");
        // «Переподключить» — ровно одна попытка, и снова стоянка.
        let e = reg
            .restart_if_down(&p, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(e.params["kind"], "authFailed");
        assert_eq!(down(&reg), Some(("authFailed".to_string(), 2)));
        tokio::time::sleep(quiet).await;
        assert_eq!(down(&reg), Some(("authFailed".to_string(), 2)));
        assert_eq!(runs(&log), 3);
        reg.close("c1");
    }

    /// «Переподключить» во время идущего перезапуска относится к нему: следующее падение не пропускает паузу.
    #[tokio::test]
    async fn wake_during_a_restart_does_not_skip_the_next_pause() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let log = dir.path().join("runs");
        let (reg, _) = registry_with(
            dir.path(),
            vec![
                ("FAKE_SSH_DIE_MS", "300".into()),
                ("FAKE_SSH_READY_DELAY_MS", "500".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
                ("FAKE_SSH_RUNS", log.to_string_lossy().into_owned()),
            ],
            Arc::new(MemoryStore::default()),
            |cfg| cfg.backoff = vec![Duration::from_millis(1500)],
        );
        let p = profile(SshAuth::Agent);
        reg.open(&p, Duration::from_secs(5)).await.unwrap();
        // Первое падение, пауза, перезапуск запущен и ещё входит (готовность через 500 мс): тут «Переподключить».
        eventually("перезапуск идёт", || runs(&log) == 2).await;
        assert!(down(&reg).is_some(), "статус — прежний Down");
        reg.restart_if_down(&p, Duration::from_secs(5))
            .await
            .unwrap();
        // Второе падение: пауза 1,5 с выдерживается, перезапуск не начинается сразу (запас 900 мс — на медленный
        // раннер CI).
        eventually("второе падение", || down(&reg).is_some()).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(runs(&log), 2, "пауза не пропущена");
        assert!(down(&reg).is_some());
        reg.close("c1");
    }

    /// Лог прошлого запуска уходит в `ssh-<id>.1.log`: текущий начинается с пустого и не растёт за перезапуски.
    #[tokio::test]
    async fn previous_log_is_rotated_on_each_start() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("ssh-c1.log"), "old run\n").unwrap();
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_UPSTREAM", echo.to_string())]);
        reg.open(&profile(SshAuth::Agent), Duration::from_secs(5))
            .await
            .unwrap();
        reg.close("c1");
        assert_eq!(
            std::fs::read_to_string(logs.join("ssh-c1.1.log")).unwrap(),
            "old run\n"
        );
        assert!(
            !std::fs::read_to_string(logs.join("ssh-c1.log"))
                .unwrap_or_default()
                .contains("old run")
        );
    }

    #[tokio::test]
    async fn restart_if_down_restarts_at_once_and_leaves_a_live_tunnel_alone() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry_with(
            dir.path(),
            vec![
                ("FAKE_SSH_DIE_MS", "300".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
            ],
            Arc::new(MemoryStore::default()),
            // Пауза перед перезапуском — минута: поднимает туннель только «Переподключить».
            |cfg| cfg.backoff = vec![Duration::from_secs(60)],
        );
        let p = profile(SshAuth::Agent);
        let first = reg.open(&p, Duration::from_secs(5)).await.unwrap();
        reg.restart_if_down(&p, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(reg.route("c1").unwrap(), Some(first), "живой не тронут");
        eventually("падение", || {
            matches!(reg.status("c1"), Some(TunnelStatus::Down { .. }))
        })
        .await;
        reg.restart_if_down(&p, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(matches!(reg.status("c1"), Some(TunnelStatus::Up { .. })));
        reg.close("c1");
    }

    /// Неудачный перезапуск по «Переподключить» возвращает свою ошибку, но туннель остаётся под супервизором.
    #[tokio::test]
    async fn failed_restart_reports_its_error_and_keeps_the_tunnel_supervised() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let refuse = dir.path().join("refuse");
        let (reg, _) = registry_with(
            dir.path(),
            vec![
                ("FAKE_SSH_DIE_MS", "300".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
                (
                    "FAKE_SSH_AUTH_FAIL_IF",
                    refuse.to_string_lossy().into_owned(),
                ),
            ],
            Arc::new(MemoryStore::default()),
            |cfg| cfg.backoff = vec![Duration::from_secs(60)],
        );
        let p = profile(SshAuth::Agent);
        reg.open(&p, Duration::from_secs(5)).await.unwrap();
        std::fs::write(&refuse, "1").unwrap();
        eventually("падение", || {
            matches!(reg.status("c1"), Some(TunnelStatus::Down { .. }))
        })
        .await;
        let e = reg
            .restart_if_down(&p, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.ssh.authFailed");
        assert!(matches!(
            reg.status("c1"),
            Some(TunnelStatus::Down { attempt: 1, .. })
        ));
        std::fs::remove_file(&refuse).unwrap();
        reg.restart_if_down(&p, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(matches!(reg.status("c1"), Some(TunnelStatus::Up { .. })));
        reg.close("c1");
    }

    #[tokio::test]
    async fn old_openssh_blocks_password_and_passphrase_only() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(
            dir.path(),
            vec![
                ("FAKE_SSH_VERSION", "OpenSSH_8.3p1".into()),
                ("FAKE_SSH_UPSTREAM", echo.to_string()),
            ],
        );
        let e = reg
            .open(&profile(SshAuth::Password), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::FeatureUnavailable, "errors.ssh.tooOld")
        );
        reg.open(&profile(SshAuth::Agent), Duration::from_secs(5))
            .await
            .unwrap();
        // Ключ без пассфразы работает на любой версии; с пассфразой нужен askpass — OpenSSH 8.4+.
        reg.open(&profile(SshAuth::Key), Duration::from_secs(5))
            .await
            .unwrap();
        reg.close("c1");
        reg.stage_secret("c1", SSH_PASSPHRASE, "pp").unwrap();
        let e = reg
            .open(&profile(SshAuth::Key), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.ssh.tooOld");
    }

    #[tokio::test]
    async fn staged_secret_is_written_for_askpass_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (reg, _) = registry_with(
            dir.path(),
            vec![],
            Arc::new(MemoryStore::default()),
            |cfg| cfg.stage_secrets_in_file = true,
        );
        reg.stage_secret("c1", SSH_PASSWORD, "pw").unwrap();
        reg.stage_secret("c1", SSH_PASSPHRASE, "pp").unwrap();
        let file = dir.path().join("data").join("askpass-c1.secrets");
        assert!(file.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let map = read_secret_file(&file);
        assert_eq!(
            (map["sshPassword"].as_str(), map["sshPassphrase"].as_str()),
            ("pw", "pp")
        );
        let env = reg.askpass_env("c1", "bastion");
        assert_eq!(env.secret_file.as_deref(), Some(file.as_path()));
        reg.unstage_secrets("c1");
        assert!(!file.exists());
    }

    /// Без `stage_secrets_in_file` секрет кладётся в keychain под id туннеля, а askpass файла не получает.
    #[tokio::test]
    async fn staged_secret_goes_to_the_keychain_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemoryStore::default());
        let (reg, _) = registry_with(dir.path(), vec![], secrets.clone(), |_| {});
        reg.stage_secret("unsaved-1", SSH_PASSWORD, "pw").unwrap();
        assert_eq!(
            secrets
                .get(&secret_key("unsaved-1", SSH_PASSWORD))
                .unwrap()
                .as_deref(),
            Some("pw")
        );
        assert_eq!(reg.askpass_env("unsaved-1", "bastion").secret_file, None);
        let staged = dir.path().join("data").join(STAGED_FILE);
        assert_eq!(read_staged(&dir.path().join("data")), ["unsaved-1"]);
        reg.unstage_secrets("unsaved-1");
        assert_eq!(
            secrets.get(&secret_key("unsaved-1", SSH_PASSWORD)).unwrap(),
            None
        );
        assert!(!staged.exists(), "снятый id ушёл из списка");
    }

    /// Секреты проверок, оставшиеся после сбоя приложения посреди проверки, снимает следующий запуск — до первого
    /// своего секрета. В списке только id временных туннелей, без значений; чужой id не трогается.
    #[test]
    fn check_secrets_left_by_a_crash_are_removed_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let file = data.join(STAGED_FILE);
        let secrets = Arc::new(MemoryStore::default());
        secrets
            .set(&secret_key("c1", SSH_PASSWORD), "saved")
            .unwrap();
        let get = |id: &str, field: &str| secrets.get(&secret_key(id, field)).unwrap();
        // Первый запуск: три проверки, одна сняла свои секреты, две — нет (приложение упало).
        let (first, _) = registry_with(dir.path(), vec![], secrets.clone(), |_| {});
        first
            .stage_secret("unsaved-1", SSH_PASSWORD, "pw-1")
            .unwrap();
        first
            .stage_secret("unsaved-2", SSH_PASSPHRASE, "pp-2")
            .unwrap();
        first
            .stage_secret("unsaved-3", SSH_PASSWORD, "pw-3")
            .unwrap();
        first.unstage_secrets("unsaved-3");
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(read_staged(&data), ["unsaved-1", "unsaved-2"]);
        assert!(
            !text.contains("pw-") && !text.contains("pp-"),
            "только id: {text}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(first);
        // Файл правили вручную: сохранённое подключение в списке не должно потерять секреты.
        std::fs::write(&file, r#"["unsaved-1","unsaved-2","c1"]"#).unwrap();
        // Следующий запуск: первая же проверка сначала снимает секреты прошлого запуска.
        let (next, _) = registry_with(dir.path(), vec![], secrets.clone(), |_| {});
        next.stage_secret("unsaved-4", SSH_PASSWORD, "pw-4")
            .unwrap();
        assert_eq!(get("unsaved-1", SSH_PASSWORD), None);
        assert_eq!(get("unsaved-2", SSH_PASSPHRASE), None);
        assert_eq!(get("c1", SSH_PASSWORD).as_deref(), Some("saved"));
        assert_eq!(get("unsaved-4", SSH_PASSWORD).as_deref(), Some("pw-4"));
        assert_eq!(read_staged(&data), ["unsaved-4"]);
        // Уборка при старте — один раз: секрет этого запуска она не снимает.
        assert_eq!(next.reap_orphans(), 0);
        assert_eq!(get("unsaved-4", SSH_PASSWORD).as_deref(), Some("pw-4"));
        next.unstage_secrets("unsaved-4");
        assert!(!file.exists());
    }

    /// Выход из приложения: процессы завершаются без рантайма — по PID из `tunnels.json`.
    #[tokio::test]
    async fn close_all_blocking_kills_the_processes() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_UPSTREAM", echo.to_string())]);
        let socks = reg
            .open(&profile(SshAuth::Agent), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(reg.close_all_blocking(), 1);
        assert_eq!(reg.status("c1"), None);
        eventually("процесс завершён", || {
            std::net::TcpStream::connect(socks).is_err()
        })
        .await;
    }

    /// Логи проверок подключения прошлого запуска убираются при старте вместе с сиротами; уборка — один раз.
    #[tokio::test]
    async fn reaping_removes_check_logs_of_the_previous_run_once() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("ssh-unsaved-1.log"), "x").unwrap();
        std::fs::write(logs.join("ssh-c1.log"), "x").unwrap();
        let (reg, _) = registry(dir.path(), vec![]);
        assert_eq!(reg.reap_orphans(), 0);
        assert!(!logs.join("ssh-unsaved-1.log").exists());
        assert!(logs.join("ssh-c1.log").exists());
        std::fs::write(logs.join("ssh-unsaved-2.log"), "x").unwrap();
        reg.reap_orphans();
        assert!(
            logs.join("ssh-unsaved-2.log").exists(),
            "лог проверки этого запуска остаётся"
        );
    }
}
