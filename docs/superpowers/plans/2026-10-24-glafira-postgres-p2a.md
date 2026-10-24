# P2a «Видеть» — каркас диагностики, инструменты дежурного набора и сэмплер: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Реализовать часть P2a спецификации `docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md` (§1, пункты 1–4, 7 без сценариев действий, 8 без раздела действий): каркас диагностики с общим конвертом отчёта, находки в `pg-advisor`, инструменты пульс, сессии и idle in transaction, профиль ожиданий, блокировки и DDL в очереди, горизонт xmin, реплики, слоты и подписки, готовность Patroni к failover, матрица health, сэмплер активности, запланированные действия Patroni в обзоре, стенд `pg-lab` и роль `glafira_app`, глава руководства «Диагностика». Только чтение: действий в P2a нет.

**Architecture:** Вкладка инструмента опрашивает команду `run_diagnostic(id, request)` (react-query, интервал, только видимая вкладка). Rust разрешает узел (`Runtimes::diag_target`: явный участник или «за лидером»), читает факты SQL-наборами, собранными по `server_version_num` (`diag::caps::Caps`, недоступные колонки — `NULL::тип` и запись в `unavailable`), на соединении пула `Catalog` в пределе `catalog_limit`. Чистые функции `pg-advisor` строят данные (лес блокировок, профиль ожиданий, вердикты failover, скорости) и находки. Сэмплер — tokio-задача на узел (пул `PoolPurpose::Sampler`, одно соединение), живёт, пока подписана вкладка; буфер в памяти до закрытия подключения; данные профиля отдаёт тот же `run_diagnostic`, канал несёт только «тики», промежутки и ошибки.

**Tech Stack:** Tauri 2, Rust (edition 2024): tokio, tokio-postgres 0.7, reqwest 0.13, serde/serde_json; React 19 + TS + TanStack Query (+ `@tanstack/react-virtual`) + Zustand + i18next; Vitest + Testing Library; Docker Compose (стенд `gp-*`, новый сервис `pg-lab`).

**Образец кода:** корень репозитория — `$P` (`/Users/vvnovg/postgres-patroni-diag`), команды выполняются из `$P`. Glafira for Kafka (`/Users/vvnovg/kafka-explorer`) — только для чтения (реестр сессий `src-tauri/src/browse/registry.rs`, модель находок `src-tauri/src/health/model.rs`).

## Global Constraints

- PostgreSQL 13–18, Patroni 3.0+ (включая 4.x). Платформы: macOS (arm64, x64), Windows x64, Linux x64.
- Соединения диагностики — пул `Catalog` (до 3 на узел и БД) с параметрами `SESSION_OPTIONS` P1 (`default_transaction_read_only=on`, `statement_timeout=5000`, `lock_timeout=1000`, `search_path=pg_catalog`, `DateStyle=ISO`); получение соединения и запросы — в пределе `catalog_limit` (таймаут подключения профиля + 10 с), таймаут и обрыв закрывают соединение (`on_catalog_node_within`). Сэмплер — пул `PoolPurpose::Sampler` из одного соединения. Опрос топологии — без изменений (свой пул, раз в 10 с).
- Диагностика ничего не меняет на сервере и в Patroni: только `SELECT` и `GET`. Тяжёлых блокировок не брать: `pg_blocking_pids` — только для сессий с `wait_event_type = 'Lock'`; размеры отношений не считаются.
- Длительности считаются в SQL от `now()` сервера (`extract(epoch FROM now() - x)::float8`), часы клиента не участвуют. `backend_start` везде — один фрагмент `diag::sql::BACKEND_START_US` (микросекунды эпохи, `int8`).
- Все команды Tauri возвращают `Result<T, AppError>`; JSON — camelCase; `AppError.code` — SCREAMING_SNAKE_CASE. Новый код P2a — `RECOVERY_CONFLICT` (`retryable`).
- Пользовательские тексты из Rust — только `ltext!` / `app_err!` с английским шаблоном, равным тексту `src/locales/en.json`; переводы — `src/locales/{en,ru}.json`; ru == en — в `docs/i18n/same-as-en.json`; `hints` — последний ключ верхнего уровня; кириллица в литералах Rust вне `#[cfg(test)]` запрещена (`pnpm i18n:check`). Комментарии — по-русски. Тексты находок — ключи `findings.<id>.title` и `findings.<id>.fix`.
- Параметры находок (`Finding.params`) называются по единицам: `…S` — секунды, `…Bytes` — байты, `…Pct` — проценты, `count` — число объектов, `command` — готовая команда для копирования (не переводится, приложение её не выполняет); остальное — как есть. Фронтенд форматирует по суффиксу.
- Ни одна операция не блокирует UI: keychain и файлы — в `blocking`/`spawn_blocking`; у каждой сетевой операции — таймаут (HTTP Patroni — 5 с).
- Стенд Docker (`gp-*`) запущен: никогда не выполнять `docker compose down`, не удалять контейнеры и тома, не трогать `ke-kafka`; новые сервисы — `docker compose -f dev/docker-compose.yml up -d --no-recreate <сервис>`; лидер Patroni — любой узел (не считать им `patroni1`). Тесты, меняющие стенд (switchover), — только под `GPG_IT_DOCKER=1` и с восстановлением при любом выходе (`common::restore_on_drop`).
- Сообщения коммитов — по-русски (`feat(diag): …`, `test(diag): …`), последняя строка — трейлер `Co-Authored-By` из напоминания об атрибуции текущей сессии (на момент написания плана — `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`).
- Проверки каждой задачи: `cargo test --manifest-path crates/pg-advisor/Cargo.toml`, `cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`, `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, `pnpm test`, `pnpm exec tsc --noEmit`, `pnpm i18n:check`, `pnpm test:scripts`, `pnpm build`. У задач со стендом — интеграционные тесты:
  `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_LAB=localhost:5470 GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518 cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1`
  (тест «за лидером после switchover» — ещё с `GPG_IT_DOCKER=1`). Матрица поднимается `docker compose -f dev/docker-compose.yml --profile matrix up -d --no-recreate` и `scripts/wait-matrix.sh`.

## Уточнения спецификации (решения плана)

1. **«За лидером» у списка узлов — всегда primary** (спец. §3.2 называет «узел по умолчанию»): узел по умолчанию списка зависит от `targetServerType` и может быть репликой, а «за лидером» означает узел, принимающий запись. Новая функция `topology::observe::leader_member` (Patroni — лидер или лидер standby-кластера, список узлов — доступный primary, сервер — единственный узел; только доступный).
2. **Одна находка на правило и узел** для правил по множеству объектов (`idleInTransaction`, `longQuery`, `longTransaction`, `slotRetainsWal`, `replicaLag` …): `target` — худший объект, `params.count` — сколько объектов достигли порога предупреждения, `params.maxS` / `maxBytes` — худшее значение. Иначе сотня висящих сессий дала бы сотню одинаковых находок. Исключения, где объект важен сам по себе: `rootBlocker` и `ddlQueue` — по находке на корень и на элемент очереди.
3. **Типы данных инструментов живут в `pg-advisor`** (`pg_advisor::rules::<tool>`): Rust-сбор заполняет их, правила читают, тот же тип уходит во фронтенд. Где в данных нужна `AppError` (строка пульса, клетка матрицы health), обёртка — в `src-tauri/src/diag/`. `pg-advisor` получает зависимость `serde_json` (параметры находок).
4. **Имя слота участника Patroni — точное правило Patroni** (`slot_name_from_member_name`): нижний регистр, `-` и `.` → `_`, прочие символы вне `[a-z0-9_]` → `u` и код символа из четырёх цифр, обрезка до 63 символов. Спецификация (§5.7) упрощала до «заменяются на `_`».
5. **Сопоставление отправителя WAL с участником** (горизонт, реплики): `application_name` = имени участника или его хосту; без DNS (опрос топологии P1 разрешает имена, диагностика — нет). Не сопоставилось — поле `member` пустое, ссылка находки не создаётся.
6. **Текст запроса в списке сессий** — не длиннее 16 384 символов (`left(query, 16384)` и флаг `queryTruncated`): при `track_activity_query_size = 1MB` и тысяче сессий ответ иначе весил бы гигабайт.
7. **DDL в сценарии стенда** выполняет `postgres` (у `glafira_app` нет владения таблицей), держатель и жертва — `glafira_app`; порядок: держатель `UPDATE`, затем `ALTER TABLE` в транзакции, затем `SELECT` жертвы — классическая очередь за миграцией.
8. **`AppState` хранит `live: Arc<LiveSource>`** (тот же экземпляр, что у `Runtimes`): инструменты Patroni берут клиента HTTP подключения (`LiveSource::http_for`, становится `pub`), не строя второго.
9. **F1 на вкладке инструмента** открывает главу `diagnostics` с якорем инструмента: новая функция `helpAnchorFor(tab)` рядом с `helpChapterFor`, `useHelpShortcut` передаёт якорь.
10. **Время сэмпла — часы приложения** (момент ответа на запрос сэмплера), а не `clock_timestamp()` сервера (спец. §6): тики, промежутки и окно отчёта профиля ожиданий считаются одними часами, и пустой сэмпл (нет активных сессий) тоже получает время. Номер версии сервера сэмплер узнаёт первым запросом и запоминает.
11. **Логика длительных сессий вынесена из команд** (`sampler::start(st, id, member, sink)`), команда Tauri только оборачивает `Channel`: так сэмплер проверяется интеграционным тестом без окна приложения.

## Карта файлов

```
crates/pg-advisor/
  Cargo.toml                     + serde_json
  src/lib.rs                     + findings, rates, locktree, ash, failover, rules
  src/findings.rs                Severity, Finding, FindingTarget, FindingLink, grade, sort, thresholds   (задача 1)
  src/rates.rs                   CounterSnapshot, delta, rate                                             (задача 1)
  src/rules/mod.rs               подмодули по инструментам
  src/rules/sessions.rs          SessionRow, SessionsData, evaluate                                       (задача 3)
  src/rules/pulse.rs             PulseFacts, PulseRates, rates, evaluate                                  (задача 4)
  src/locktree.rs, rules/locks.rs                                                                         (задача 5)
  src/rules/horizon.rs, rules/replication.rs                                                              (задача 6)
  src/failover.rs, rules/patroni.rs                                                                       (задача 7)
  src/rules/slots.rs                                                                                      (задача 8)
  src/ash.rs, rules/waits.rs                                                                              (задача 9)
src-tauri/src/
  diag/mod.rs caps.rs sql.rs memory.rs sessions.rs                                                        (задача 3)
  diag/pulse.rs (4)  locks.rs (5)  horizon.rs replication.rs (6)  patroni.rs (7)  slots.rs (8)  waits.rs (9)
  sampler/mod.rs buffer.rs registry.rs query.rs, session.rs                                              (задача 9)
  commands/diag.rs               run_diagnostic (3), start_sampler, stop_session (9)
  topology/{observe, runtime, model, patroni}.rs, conn/{errors, pool}.rs, error.rs, state.rs, lib.rs     (изменения)
src-tauri/tests/
  it_stand.rs (2)  it_diag.rs (3–8)  it_sampler.rs (9)
src/
  api/types.ts, api/client.ts, state/workspace.ts, components/{Layout,TabContent}.tsx                   (задача 10)
  components/diag/DiagTab, DiagHeader, FindingsPanel, findingText.ts                                    (задача 10)
  components/diag/PulseView, SessionsView, LocksView                                                    (задача 11)
  components/diag/HorizonView, ReplicationView, SlotsView, FailoverView, HealthView                     (задача 12)
  components/diag/WaitsView, WaitsChart; lib/useSampler.ts                                              (задача 13)
  components/connections/tree/DiagnosticsNodes.tsx; lib/useDiagnostic.ts; lib/format.ts                 (задача 10)
dev/ docker-compose.yml (pg-lab), lab/30-logical.sh, sql/init.sql (glafira_app); scripts/wait-stand.sh  (задача 2)
docs/guide/{en,ru}/diagnostics.md, docs/admin.md, README.md, CHANGELOG.md, docs/i18n/glossary.md       (задача 14)
```

**Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 (Rust), 10 → 11 → 12 → 13 (фронтенд; 10 можно начинать после 3, но типы данных зеркалят задачи 4–9), 14 (документация, последней).

---

### Task 1: `pg-advisor` — находки, пороги и скорости

**Files:**
- Modify: `crates/pg-advisor/Cargo.toml`, `crates/pg-advisor/src/lib.rs`
- Create: `crates/pg-advisor/src/findings.rs`, `crates/pg-advisor/src/rates.rs`, `crates/pg-advisor/src/rules/mod.rs`
- Test: unit-тесты в `findings.rs` и `rates.rs`

**Interfaces:**
- Consumes: —
- Produces:
  - `pg_advisor::findings::{Severity { Info, Warning, Critical }` (`Ord`: Info < Warning < Critical; JSON `"info" | "warning" | "critical"`), `FindingTarget` (`#[serde(tag = "kind")]`: `session { pid: i32, backendStartUs: i64 }` | `slot { name }` | `member { name }` | `relation { name }` | `database { name }`), `FindingLink` (`openTool { tool: String, member: Option<String> }`), `Finding { id: String, severity, member: Option<String>, target: Option<FindingTarget>, params: serde_json::Map<String, Value>, links: Vec<FindingLink> }` с построителями `Finding::new(id, severity)`, `.member(Option<&str>)`, `.target(FindingTarget)`, `.param(&str, impl Into<Value>)`, `.link_tool(&str, Option<&str>)`; `grade(value: f64, warn: f64, crit: f64) -> Option<Severity>`; `worse(a: Option<Severity>, b: Option<Severity>) -> Option<Severity>`; `sort(&mut [Finding])`; модуль `thresholds` (константы §4 спеки)}`;
  - `pg_advisor::rates::{CounterSnapshot { at_ms: i64, started_ms: i64, reset_ms: Option<i64>, values: BTreeMap<String, i64> }, delta(prev: Option<&CounterSnapshot>, next: &CounterSnapshot, key: &str) -> Option<i64>, rate(prev, next, key) -> Option<f64>}`;
  - пустой `pg_advisor::rules` (подмодули добавляют задачи 3–9).

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/findings.rs`, модуль `tests`:

```rust
    use super::*;
    use serde_json::json;

    #[test]
    fn grade_picks_the_highest_reached_level() {
        assert_eq!(grade(10.0, 30.0, 300.0), None);
        assert_eq!(grade(30.0, 30.0, 300.0), Some(Severity::Warning));
        assert_eq!(grade(299.9, 30.0, 300.0), Some(Severity::Warning));
        assert_eq!(grade(300.0, 30.0, 300.0), Some(Severity::Critical));
        assert_eq!(grade(f64::NAN, 30.0, 300.0), None);
        assert_eq!(worse(Some(Severity::Warning), None), Some(Severity::Warning));
        assert_eq!(worse(Some(Severity::Warning), Some(Severity::Critical)), Some(Severity::Critical));
    }

    #[test]
    fn finding_json_is_camel_case_with_tagged_target_and_links() {
        let f = Finding::new("idleInTransaction", Severity::Critical)
            .member(Some("pg-1"))
            .target(FindingTarget::Session { pid: 7, backend_start_us: 1_700_000_000_000_001 })
            .param("maxS", 1800.5)
            .param("count", 2)
            .link_tool("idleTx", Some("pg-1"));
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            json!({
                "id": "idleInTransaction", "severity": "critical", "member": "pg-1",
                "target": {"kind": "session", "pid": 7, "backendStartUs": 1_700_000_000_000_001_i64},
                "params": {"maxS": 1800.5, "count": 2},
                "links": [{"kind": "openTool", "tool": "idleTx", "member": "pg-1"}]
            })
        );
        let slot = Finding::new("slotLost", Severity::Critical).target(FindingTarget::Slot { name: "s".into() });
        assert_eq!(serde_json::to_value(&slot).unwrap()["target"], json!({"kind": "slot", "name": "s"}));
        assert_eq!(serde_json::to_value(&slot).unwrap()["member"], json!(null));
    }

    #[test]
    fn sort_orders_by_severity_then_member_then_id() {
        let mut v = vec![
            Finding::new("b", Severity::Info).member(Some("x")),
            Finding::new("a", Severity::Critical).member(Some("y")),
            Finding::new("c", Severity::Critical).member(Some("x")),
            Finding::new("a", Severity::Critical).member(Some("x")),
            Finding::new("z", Severity::Warning),
        ];
        sort(&mut v);
        let order: Vec<(&str, Option<&str>)> = v.iter().map(|f| (f.id.as_str(), f.member.as_deref())).collect();
        assert_eq!(order, [("a", Some("x")), ("c", Some("x")), ("a", Some("y")), ("z", None), ("b", Some("x"))]);
    }
```

`crates/pg-advisor/src/rates.rs`, модуль `tests`:

```rust
    use super::*;

    fn snap(at_ms: i64, started_ms: i64, reset_ms: Option<i64>, commits: i64) -> CounterSnapshot {
        CounterSnapshot { at_ms, started_ms, reset_ms, values: [("commits".to_string(), commits)].into() }
    }

    #[test]
    fn rate_is_delta_per_second_of_server_time() {
        let a = snap(1_000, 5, None, 100);
        let b = snap(3_000, 5, None, 300);
        assert_eq!(delta(Some(&a), &b, "commits"), Some(200));
        assert_eq!(rate(Some(&a), &b, "commits"), Some(100.0));
    }

    #[test]
    fn no_rate_without_a_comparable_previous_snapshot() {
        let a = snap(1_000, 5, None, 100);
        assert_eq!(rate(None, &a, "commits"), None, "первый снимок");
        assert_eq!(rate(Some(&a), &snap(3_000, 6, None, 300), "commits"), None, "сервер перезапущен");
        assert_eq!(rate(Some(&a), &snap(3_000, 5, Some(2_000), 300), "commits"), None, "статистику сбросили");
        assert_eq!(rate(Some(&a), &snap(3_000, 5, None, 50), "commits"), None, "счётчик уменьшился");
        assert_eq!(rate(Some(&a), &snap(1_000, 5, None, 300), "commits"), None, "нулевой интервал");
        assert_eq!(rate(Some(&a), &snap(3_000, 5, None, 300), "missing"), None, "нет ключа");
    }
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml`
Expected: ошибки компиляции (`findings`, `rates` не существуют).

- [ ] **Step 3: Реализация**

`crates/pg-advisor/Cargo.toml` — в `[dependencies]` добавить `serde_json = "1"` (из `[dev-dependencies]` строку убрать); описание пакета: «Правила Glafira for PostgreSQL без ввода-вывода: готовность (P1), находки и данные инструментов диагностики (P2)».

`crates/pg-advisor/src/lib.rs`:

```rust
//! Правила и разбор без ввода-вывода (концепция §3): тестируются на наборах фактов и фикстурах.

pub mod findings;
pub mod rates;
pub mod readiness;
pub mod rules;
```

`crates/pg-advisor/src/rules/mod.rs`:

```rust
//! Правила и данные инструментов диагностики (спец. P2 §4–5): по модулю на инструмент.
```

`crates/pg-advisor/src/findings.rs`:

```rust
//! Находки инструментов диагностики (спец. P2 §4): строгость, объект, параметры текста, ссылки.
//! Тексты — в словарях интерфейса по `id` (`findings.<id>.title|fix`); здесь только факты.
//! Параметры называются по единицам: `…S` — секунды, `…Bytes` — байты, `…Pct` — проценты, `count` — число
//! объектов, `command` — готовая команда для копирования (не переводится и не выполняется приложением).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FindingTarget {
    #[serde(rename_all = "camelCase")]
    Session { pid: i32, backend_start_us: i64 },
    Slot { name: String },
    Member { name: String },
    Relation { name: String },
    Database { name: String },
}

/// Куда ведёт кнопка находки. `tool` — id вкладки инструмента во фронтенде (`sessions`, `idleTx`, `locks`…).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FindingLink {
    OpenTool { tool: String, member: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub member: Option<String>,
    pub target: Option<FindingTarget>,
    pub params: Map<String, Value>,
    pub links: Vec<FindingLink>,
}

impl Finding {
    pub fn new(id: &str, severity: Severity) -> Self {
        Self { id: id.to_string(), severity, member: None, target: None, params: Map::new(), links: Vec::new() }
    }

    pub fn member(mut self, member: Option<&str>) -> Self {
        self.member = member.map(str::to_string);
        self
    }

    pub fn target(mut self, target: FindingTarget) -> Self {
        self.target = Some(target);
        self
    }

    pub fn param(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.params.insert(key.to_string(), value.into());
        self
    }

    pub fn link_tool(mut self, tool: &str, member: Option<&str>) -> Self {
        self.links.push(FindingLink::OpenTool { tool: tool.to_string(), member: member.map(str::to_string) });
        self
    }
}

/// Строгость по двум порогам: `value ≥ crit` — critical, `value ≥ warn` — warning, иначе нет находки.
pub fn grade(value: f64, warn: f64, crit: f64) -> Option<Severity> {
    if value >= crit {
        Some(Severity::Critical)
    } else if value >= warn {
        Some(Severity::Warning)
    } else {
        None
    }
}

/// Более строгая из двух оценок.
pub fn worse(a: Option<Severity>, b: Option<Severity>) -> Option<Severity> {
    a.max(b)
}

/// Порядок отчёта: строгость (critical → info), затем узел, затем id.
pub fn sort(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.member.cmp(&b.member))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Пороги находок (спец. P2 §4). Строгость от окружения подключения в P2 не зависит.
pub mod thresholds {
    pub const MIB: f64 = 1024.0 * 1024.0;
    pub const GIB: f64 = 1024.0 * MIB;
    /// 2³¹ — граница возраста XID, от которой считается доля.
    pub const XID_LIMIT: f64 = 2_147_483_648.0;

    pub const CONNECTIONS_PCT: (f64, f64) = (80.0, 95.0);
    pub const IDLE_IN_TRANSACTION_S: (f64, f64) = (300.0, 1800.0);
    pub const LONG_QUERY_S: (f64, f64) = (900.0, 3600.0);
    pub const LONG_TRANSACTION_S: (f64, f64) = (3600.0, 21600.0);
    pub const XID_AGE_PCT: (f64, f64) = (50.0, 75.0);
    pub const LOCK_WAIT_S: (f64, f64) = (30.0, 300.0);
    /// `pg_wal` больше `max_wal_size` во столько раз.
    pub const WAL_DIR_RATIO: (f64, f64) = (2.0, 4.0);
    /// Доля сэмплов `Lock` в окне профиля ожиданий.
    pub const LOCK_SHARE: (f64, f64) = (0.20, 0.50);
    /// Ожидание жертв корневого блокирующего; число жертв, с которого — critical.
    pub const ROOT_BLOCKER_WAIT_S: (f64, f64) = (10.0, 60.0);
    pub const ROOT_BLOCKER_CRIT_VICTIMS: usize = 10;
    pub const XMIN_AGE: (f64, f64) = (10_000_000.0, 100_000_000.0);
    pub const XMIN_DURATION_S: (f64, f64) = (3600.0, 21600.0);
    pub const REPLICA_LAG_BYTES: (f64, f64) = (16.0 * MIB, GIB);
    pub const REPLICA_LAG_S: (f64, f64) = (30.0, 300.0);
    pub const REPLY_STALE_S: f64 = 60.0;
    pub const SLOT_RETAINED_BYTES: (f64, f64) = (GIB, 10.0 * GIB);
    pub const CACHE_HIT_LOW_PCT: f64 = 90.0;
    /// Cache hit оценивается, если за интервал прочитано не меньше блоков.
    pub const CACHE_HIT_MIN_READS: i64 = 1000;
}
```

`crates/pg-advisor/src/rates.rs`:

```rust
//! Скорости и дельты по двум снимкам счётчиков (спец. P2 §3.6). Время снимка — часы сервера.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CounterSnapshot {
    /// `now()` сервера в момент снимка, мс эпохи.
    pub at_ms: i64,
    /// `pg_postmaster_start_time()`, мс: другой — сервер перезапущен, счётчики начались заново.
    pub started_ms: i64,
    /// Наибольший `stats_reset` из `pg_stat_database`, мс: изменился — статистику сбросили.
    pub reset_ms: Option<i64>,
    pub values: BTreeMap<String, i64>,
}

fn comparable(prev: &CounterSnapshot, next: &CounterSnapshot) -> bool {
    prev.started_ms == next.started_ms && prev.reset_ms == next.reset_ms && next.at_ms > prev.at_ms
}

/// Прирост счётчика `key`; `None` — нет прошлого снимка, снимки несравнимы, ключа нет или счётчик уменьшился.
pub fn delta(prev: Option<&CounterSnapshot>, next: &CounterSnapshot, key: &str) -> Option<i64> {
    let prev = prev.filter(|p| comparable(p, next))?;
    let d = next.values.get(key)? - prev.values.get(key)?;
    (d >= 0).then_some(d)
}

/// Прирост в секунду серверного времени.
pub fn rate(prev: Option<&CounterSnapshot>, next: &CounterSnapshot, key: &str) -> Option<f64> {
    let d = delta(prev, next, key)?;
    let secs = (next.at_ms - prev?.at_ms) as f64 / 1000.0;
    (secs > 0.0).then(|| d as f64 / secs)
}
```

- [ ] **Step 4: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check && cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: PASS (src-tauri собирается с новой зависимостью крейта).

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor src-tauri/Cargo.lock
git commit -m "feat(advisor): находки диагностики, пороги и скорости по снимкам счётчиков

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Стенд — роль `glafira_app`, сервис `pg-lab`, подготовка интеграционных тестов

**Files:**
- Modify: `dev/sql/init.sql`, `dev/docker-compose.yml`, `scripts/wait-stand.sh`, `src-tauri/tests/common/mod.rs`, `.github/workflows/ci.yml`, `README.md` (раздел «Стенд»)
- Create: `dev/lab/30-logical.sh`, `src-tauri/tests/it_stand.rs`
- Test: `it_stand.rs` (со стендом)

**Interfaces:**
- Consumes: `common::{env, host_port, server_profile, matrix_addrs}`, `ConnectSpec::build(...).connect()`.
- Produces (в `src-tauri/tests/common/mod.rs`): константы `SUPERUSER = "postgres"`, `SUPERUSER_PASSWORD = "postgres-secret"`, `APP_USER = "glafira_app"`, `APP_PASSWORD = "app-secret"`; `connect_as(addr, db, user, password) -> tokio_postgres::Client`; `superuser(addr, db)`; `app(addr)` (БД `orders`); `lab_addr() -> Option<String>` (`GPG_IT_LAB`); `patroni_leader_addr() -> Option<String>` (узел Patroni `localhost:5441…5443`, где `pg_is_in_recovery() = false`; без `GPG_IT_PATRONI` — `None`); `writable_addrs() -> Vec<String>` (single, primary, лидер Patroni, lab, матрица — где есть переменные); `ensure_fixtures(addr)` (идемпотентно: роль `glafira_app` и права на `shop` в `orders`).
- Сервис `pg-lab` (`gp-pg-lab`, `127.0.0.1:5470`, PG 16): `wal_level=logical`, `max_prepared_transactions=10`; в `orders` — публикация `lab_pub`, слот `lab_sub_slot` (`pgoutput`); БД `lab_sub` с подпиской `lab_sub`.

- [ ] **Step 1: Падающий интеграционный тест**

`src-tauri/tests/it_stand.rs`:

```rust
//! Стенд P2: роль приложения `glafira_app` на пишущих узлах и лаборатория `pg-lab` (`GPG_IT_LAB`):
//! логический слот, подписка, prepared transactions. Без переменных тест ничего не проверяет.

mod common;

#[tokio::test]
async fn app_role_exists_on_every_writable_node() {
    for addr in common::writable_addrs().await {
        common::ensure_fixtures(&addr).await;
        let c = common::app(&addr).await;
        let n: i64 = c.query_one("SELECT count(*) FROM shop.orders", &[]).await.unwrap().get(0);
        assert!(n > 0, "{addr}");
        let sup: bool = c.query_one("SELECT rolsuper FROM pg_roles WHERE rolname = current_user", &[]).await.unwrap().get(0);
        assert!(!sup, "{addr}: glafira_app не суперпользователь");
    }
}

#[tokio::test]
async fn lab_has_logical_slot_subscription_and_prepared_transactions() {
    let Some(lab) = common::lab_addr() else { return };
    let c = common::superuser(&lab, "orders").await;
    let r = c
        .query_one(
            "SELECT current_setting('wal_level'), current_setting('max_prepared_transactions')::int, \
             (SELECT slot_type || ':' || plugin FROM pg_replication_slots WHERE slot_name = 'lab_sub_slot'), \
             (SELECT count(*) FROM pg_stat_subscription WHERE subname = 'lab_sub')::bigint",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(r.get::<_, String>(0), "logical");
    assert_eq!(r.get::<_, i32>(1), 10);
    assert_eq!(r.get::<_, Option<String>>(2).as_deref(), Some("logical:pgoutput"));
    assert!(r.get::<_, i64>(3) >= 1, "воркер подписки запущен");
}
```

- [ ] **Step 2: Убедиться, что тест падает**

Run: `GPG_IT_SINGLE=localhost:5460 GPG_IT_LAB=localhost:5470 cargo test --manifest-path src-tauri/Cargo.toml --test it_stand -- --test-threads=1`
Expected: ошибки компиляции (`writable_addrs`, `ensure_fixtures`, `app`, `lab_addr`, `superuser` нет).

- [ ] **Step 3: Реализация**

`dev/sql/init.sql` — после `GRANT pg_signal_backend TO glafira_diag;`:

```sql
-- Роль приложения для сценариев P2 (сессии, блокировки, idle in transaction): не суперпользователь, потому что
-- pg_signal_backend роли диагностики завершает только такие сессии.
CREATE ROLE glafira_app LOGIN PASSWORD 'app-secret';
```

и в конце файла (после `ANALYZE;`):

```sql
GRANT USAGE ON SCHEMA shop TO glafira_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA shop TO glafira_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA shop TO glafira_app;
```

`dev/docker-compose.yml` — сервис после `pg-single`:

```yaml
  # Лаборатория P2: логические слоты, подписка и prepared transactions — на остальных узлах они возможны только
  # после рестарта с другими параметрами. Подписка — на себя: публикация в orders, подписчик — БД lab_sub.
  pg-lab:
    image: postgres:16
    container_name: gp-pg-lab
    hostname: pg-lab
    environment:
      POSTGRES_PASSWORD: postgres-secret
    command:
      - postgres
      - -c
      - wal_level=logical
      - -c
      - max_prepared_transactions=10
      - -c
      - max_replication_slots=10
      - -c
      - max_wal_senders=10
      - -c
      - shared_preload_libraries=pg_stat_statements
      - -c
      - track_io_timing=on
    volumes:
      - ./sql/init.sql:/docker-entrypoint-initdb.d/20-init.sql:ro
      - ./lab/30-logical.sh:/docker-entrypoint-initdb.d/30-logical.sh:ro
    ports: ["127.0.0.1:5470:5432"]
```

`dev/lab/30-logical.sh` (права 0755):

```bash
#!/bin/bash
# Логическая репликация внутри pg-lab: публикация в orders, слот pgoutput и подписка на неё из БД lab_sub.
# Подписка подключается через unix-сокет (local trust образа postgres): пароль в строке подключения не нужен,
# и сокет доступен уже во время инициализации, когда TCP ещё выключен.
set -euo pipefail
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname orders <<'SQL'
CREATE PUBLICATION lab_pub FOR TABLE shop.customers;
SELECT pg_create_logical_replication_slot('lab_sub_slot', 'pgoutput');
SQL
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres -c "CREATE DATABASE lab_sub"
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname lab_sub <<'SQL'
CREATE SCHEMA shop;
CREATE TABLE shop.customers (id bigint PRIMARY KEY, email text NOT NULL UNIQUE, created_at timestamptz NOT NULL DEFAULT now());
CREATE SUBSCRIPTION lab_sub CONNECTION 'host=/var/run/postgresql dbname=orders user=postgres'
  PUBLICATION lab_pub WITH (create_slot = false, slot_name = 'lab_sub_slot', copy_data = false);
SQL
```

`scripts/wait-stand.sh`: в список `for c in …` добавить `gp-pg-lab`; перед `echo "stand is ready"`:

```bash
# Подписка лаборатории работает: воркер применения держит слот.
lab_streaming() {
  [ "$(docker exec gp-pg-lab psql -U postgres -d orders -tAc "select count(*) from pg_replication_slots where slot_name = 'lab_sub_slot' and active")" = "1" ]
}
wait_for bash -c "$(declare -f lab_streaming); lab_streaming"
```

`src-tauri/tests/common/mod.rs`:

```rust
pub const SUPERUSER: &str = "postgres";
pub const SUPERUSER_PASSWORD: &str = "postgres-secret";
pub const APP_USER: &str = "glafira_app";
pub const APP_PASSWORD: &str = "app-secret";

/// Соединение с узлом стенда под заданной ролью (без TLS). Соединение живёт, пока жив клиент.
pub async fn connect_as(addr: &str, db: &str, user: &str, password: &str) -> tokio_postgres::Client {
    let mut p = server_profile("it-as", addr);
    p.pg.ssl_mode = SslMode::Disable;
    p.pg.username = user.into();
    let s = MemoryStore::default();
    s.set(&secret_key("it-as", PG_PASSWORD), password).unwrap();
    let (h, port) = host_port(addr);
    ConnectSpec::build(&p, &h, port, db, &s).unwrap().connect().await.unwrap_or_else(|e| panic!("{addr} {user}: {e:?}"))
}

pub async fn superuser(addr: &str, db: &str) -> tokio_postgres::Client {
    connect_as(addr, db, SUPERUSER, SUPERUSER_PASSWORD).await
}

/// Сессия приложения (`glafira_app`) в демо-базе `orders`.
pub async fn app(addr: &str) -> tokio_postgres::Client {
    connect_as(addr, "orders", APP_USER, APP_PASSWORD).await
}

pub fn lab_addr() -> Option<String> {
    env("GPG_IT_LAB")
}

/// Узел Patroni, принимающий запись: лидер может быть любым из трёх.
pub async fn patroni_leader_addr() -> Option<String> {
    env("GPG_IT_PATRONI")?;
    for addr in ["localhost:5441", "localhost:5442", "localhost:5443"] {
        let c = superuser(addr, "postgres").await;
        let in_recovery: bool = c.query_one("SELECT pg_is_in_recovery()", &[]).await.unwrap().get(0);
        if !in_recovery {
            return Some(addr.to_string());
        }
    }
    None
}

/// Пишущие узлы стенда, для которых заданы переменные: одиночный, primary пары, лидер Patroni, лаборатория, матрица.
pub async fn writable_addrs() -> Vec<String> {
    let mut out: Vec<String> = ["GPG_IT_SINGLE", "GPG_IT_PRIMARY", "GPG_IT_LAB"].into_iter().filter_map(env).collect();
    out.extend(patroni_leader_addr().await);
    out.extend(matrix_addrs());
    out
}

/// Роль `glafira_app` и её права на схему `shop` — на уже поднятом стенде, созданном до P2 (`init.sql` работает только
/// при первой инициализации). Идемпотентно; только на пишущем узле.
pub async fn ensure_fixtures(addr: &str) {
    let c = superuser(addr, "postgres").await;
    c.batch_execute(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'glafira_app') THEN \
             CREATE ROLE glafira_app LOGIN PASSWORD 'app-secret'; \
           END IF; \
         END $$",
    )
    .await
    .unwrap();
    let o = superuser(addr, "orders").await;
    o.batch_execute(
        "GRANT USAGE ON SCHEMA shop TO glafira_app; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA shop TO glafira_app; \
         GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA shop TO glafira_app;",
    )
    .await
    .unwrap();
}
```

(импорты `ConnectSpec`, `SslMode`, `MemoryStore`, `PG_PASSWORD`, `secret_key` уже есть в файле или добавляются в шапку.)

`.github/workflows/ci.yml` — в `env` шага «Тесты Rust (unit + интеграционные)»: `GPG_IT_LAB: localhost:5470`. Шаг «Стенд» не меняется (`up -d --build` поднимает и `pg-lab`).

`README.md`, таблица стенда — строка `| Лаборатория P2: логические слоты, подписка, prepared transactions (PG 16) | localhost:5470 |`; в «Учётные данные» — роль приложения `glafira_app` / `app-secret` (сценарии P2); в списке переменных тестов — `GPG_IT_LAB=localhost:5470`; абзац: «На стенде, поднятом до P2, лаборатория запускается отдельно: `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-lab`; роль `glafira_app` интеграционные тесты создают сами (`common::ensure_fixtures`)».

- [ ] **Step 4: Поднять `pg-lab` и прогнать тест**

Run: `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-lab && scripts/wait-stand.sh`
Expected: `stand is ready`.

Run: `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_LAB=localhost:5470 cargo test --manifest-path src-tauri/Cargo.toml --test it_stand -- --test-threads=1`
Expected: PASS, 2 теста.

- [ ] **Step 5: Commit**

```bash
git add dev scripts/wait-stand.sh src-tauri/tests .github/workflows/ci.yml README.md
git commit -m "test(stand): роль glafira_app и лаборатория pg-lab для сценариев P2

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Каркас диагностики в Rust и инструмент «Сессии»

**Files:**
- Create: `src-tauri/src/diag/mod.rs`, `src-tauri/src/diag/caps.rs`, `src-tauri/src/diag/sql.rs`, `src-tauri/src/diag/memory.rs`, `src-tauri/src/diag/sessions.rs`, `src-tauri/src/commands/diag.rs`, `crates/pg-advisor/src/rules/sessions.rs`, `src-tauri/tests/it_diag.rs`
- Modify: `crates/pg-advisor/src/rules/mod.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/catalog.rs` (`catalog_limit` → `pub(crate)`), `src-tauri/src/state.rs`, `src-tauri/src/topology/observe.rs`, `src-tauri/src/topology/runtime.rs`, `src-tauri/src/topology/patroni.rs` (только видимость `LiveSource::http_for` в `runtime.rs`), `src-tauri/src/error.rs`, `src-tauri/src/conn/errors.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в новых модулях, `observe.rs`, `runtime.rs`, `error.rs`, `conn/errors.rs`; интеграционные в `it_diag.rs`

**Interfaces:**
- Consumes: `Finding`, `Severity`, `grade`, `worse`, `sort`, `thresholds`, `CounterSnapshot` (задача 1); `common::{app, ensure_fixtures, writable_addrs, matrix_addrs, server_profile, secrets_with_password}` (задача 2); `commands::catalog::on_catalog_node_within`; `AppState::client`.
- Produces:
  - `pg_advisor::rules::sessions::{SessionRow { pid: i32, leader_pid: Option<i32>, backend_type: Option<String>, database: Option<String>, user: Option<String>, application: Option<String>, client_addr: Option<String>, client_port: Option<i32>, backend_start_us: Option<i64>, xact_age_s: Option<f64>, query_age_s: Option<f64>, state_age_s: Option<f64>, state: Option<String>, wait_event_type: Option<String>, wait_event: Option<String>, backend_xid: Option<String>, xmin_age: Option<i64>, query_id: Option<i64>, query: Option<String>, query_truncated: bool, blocked_by: Vec<i32>, locks_held: i64, glafira: bool }, SessionSettings { idle_in_transaction_session_timeout_ms: Option<i64>, track_activity_query_size: Option<i64> }, SessionsData { rows, settings }, evaluate(member: Option<&str>, d: &SessionsData) -> Vec<Finding>, is_idle_in_transaction(&SessionRow) -> bool, quote_ident(&str) -> String}`;
  - `diag::{Tool { Pulse, Sessions, Waits, Locks, Horizon, Replication, Slots, Failover, Health }` (camelCase; `is_cluster()`, `patroni_only()`), `DiagRequest { tool, member: Option<String>, window_s: Option<u32> }`, `WINDOWS_S = [300, 900, 3600]`, `DEFAULT_WINDOW_S = 900`, `Unavailable { part: String, member: Option<String>, reason: LocalizedText }` (`member` — узел у инструментов кластера, иначе `None`; в JSON не выводится, когда пуст), `DiagData` (`#[serde(tag = "kind", rename_all = "camelCase")]`; в этой задаче вариант `Sessions(SessionsData)`, задачи 4–9 добавляют свои), `DiagReport { tool, member, follows_leader, observed_at_ms, server_version_num: Option<i32>, in_recovery: Option<bool>, data, findings, unavailable }`, `Head { version_num, in_recovery, now_ms }`, `head(&Client) -> AppResult<Head>`, `run(st: &AppState, id: &str, req: DiagRequest) -> AppResult<DiagReport>`, `on_member(st, id, target, query)`, `make_report(tool, target, head: Option<&Head>, data, findings, unavailable) -> DiagReport`}`; у каждого инструмента — `diag::<tool>::report(st, id, &DiagTarget) -> AppResult<DiagReport>` и `collect(&Client, Caps, …)`;
  - `diag::caps::{Caps { version_num }`, `Caps::col(since, expr, ty) -> String`, `Caps::missing(since, part) -> Option<Unavailable>`, `V14 = 140000`, `V15`, `V16`, `V17`}`; `diag::sql::{BACKEND_START_US}`; `diag::memory::CounterMemory { swap(connection, generation, member, tool, next) -> Option<CounterSnapshot> }` (+ `impl CloseListener`: закрытие подключения и исчезновение участника стирают его снимки);
  - `topology::observe::leader_member(kind, &Topology) -> Option<&Member>`; `topology::runtime::{DiagTarget { profile, generation: u64, topology: Topology, member: Option<Member>, follows_leader: bool }, CloseListener { connection_closed(&str), member_removed(&str, &str) }, Runtimes::diag_target(id, member: Option<&str>, cluster: bool) -> AppResult<DiagTarget>, Runtimes::add_close_listener(Arc<dyn CloseListener>)}`; `LiveSource::http_for` — `pub`;
  - `ErrorCode::RecoveryConflict` (`RECOVERY_CONFLICT`, retryable), ключ `errors.pg.recoveryConflict`; `conn::errors::is_recovery_conflict(&SqlState, &str) -> bool`;
  - `AppState { live: Arc<LiveSource>, counters: Arc<CounterMemory>, … }`;
  - команда `run_diagnostic(id: String, request: DiagRequest) -> DiagReport`; ключи `errors.diag.patroniOnly`, `errors.diag.badWindow`, `diag.unavailable.version`.

- [ ] **Step 1: Падающие unit-тесты**

`crates/pg-advisor/src/rules/sessions.rs`, `tests`:

```rust
    use super::*;
    use crate::findings::{FindingTarget, Severity};

    pub(crate) fn row(pid: i32, state: &str) -> SessionRow {
        SessionRow {
            pid, leader_pid: None, backend_type: Some("client backend".into()), database: Some("orders".into()),
            user: Some("app".into()), application: Some("api".into()), client_addr: None, client_port: None,
            backend_start_us: Some(1_000 + pid as i64), xact_age_s: None, query_age_s: None, state_age_s: None,
            state: Some(state.into()), wait_event_type: None, wait_event: None, backend_xid: None, xmin_age: None,
            query_id: None, query: Some("select 1".into()), query_truncated: false, blocked_by: vec![], locks_held: 0, glafira: false,
        }
    }

    fn data(rows: Vec<SessionRow>, timeout_ms: i64) -> SessionsData {
        SessionsData { rows, settings: SessionSettings { idle_in_transaction_session_timeout_ms: Some(timeout_ms), track_activity_query_size: Some(4096) } }
    }

    fn ids(f: &[Finding]) -> Vec<(&str, Severity)> {
        f.iter().map(|x| (x.id.as_str(), x.severity)).collect()
    }

    #[test]
    fn one_idle_in_transaction_finding_per_node_with_the_worst_session() {
        let mut a = row(1, "idle in transaction");
        a.state_age_s = Some(400.0);
        let mut b = row(2, "idle in transaction (aborted)");
        b.state_age_s = Some(2000.0);
        let mut c = row(3, "idle in transaction");
        c.state_age_s = Some(10.0);
        let f = evaluate(Some("pg-1"), &data(vec![a, b, c], 60_000));
        assert_eq!(ids(&f), [("idleInTransaction", Severity::Critical)]);
        assert_eq!(f[0].target, Some(FindingTarget::Session { pid: 2, backend_start_us: 1_002 }));
        assert_eq!(f[0].params["count"], 2);
        assert_eq!(f[0].params["maxS"], 2000.0);
        assert_eq!(f[0].member.as_deref(), Some("pg-1"));
    }

    #[test]
    fn long_query_and_transaction_ignore_glafira_and_background_processes() {
        let mut q = row(1, "active");
        q.query_age_s = Some(1000.0);
        q.xact_age_s = Some(4000.0);
        let mut ours = row(2, "active");
        ours.glafira = true;
        ours.query_age_s = Some(99_999.0);
        let mut vacuum = row(3, "active");
        vacuum.backend_type = Some("autovacuum worker".into());
        vacuum.query_age_s = Some(99_999.0);
        let f = evaluate(None, &data(vec![q, ours, vacuum], 60_000));
        // Одна строгость и пустой узел — порядок по id.
        assert_eq!(ids(&f), [("longQuery", Severity::Warning), ("longTransaction", Severity::Warning)]);
    }

    #[test]
    fn no_idle_timeout_and_hidden_queries_are_info() {
        let mut a = row(1, "idle in transaction");
        a.state_age_s = Some(301.0);
        a.user = Some("user".into());
        let mut hidden = row(2, "active");
        hidden.query = Some("<insufficient privilege>".into());
        let f = evaluate(None, &data(vec![a, hidden], 0));
        let no_timeout = f.iter().find(|x| x.id == "noIdleTimeout").unwrap();
        assert_eq!(no_timeout.severity, Severity::Info);
        assert_eq!(no_timeout.params["command"], "ALTER ROLE \"user\" SET idle_in_transaction_session_timeout = '10min';");
        let hidden = f.iter().find(|x| x.id == "hiddenQueries").unwrap();
        assert_eq!((hidden.severity, hidden.params["count"].as_i64()), (Severity::Info, Some(1)));
        // Таймаут задан — совета нет.
        let mut b = row(1, "idle in transaction");
        b.state_age_s = Some(301.0);
        assert!(evaluate(None, &data(vec![b], 600_000)).iter().all(|x| x.id != "noIdleTimeout"));
    }
```

`src-tauri/src/diag/caps.rs`, `tests`:

```rust
    #[test]
    fn missing_columns_become_typed_nulls_with_an_explanation() {
        let old = Caps::new(130012);
        let new = Caps::new(170004);
        assert_eq!(old.col(V14, "a.query_id", "bigint"), "NULL::bigint");
        assert_eq!(new.col(V14, "a.query_id", "bigint"), "a.query_id");
        let u = old.missing(V14, "queryId").unwrap();
        assert_eq!((u.part.as_str(), u.member.as_deref()), ("queryId", None));
        assert_eq!(u.reason.key, "diag.unavailable.version");
        assert_eq!(u.reason.text, "queryId: PostgreSQL 14+");
        assert!(new.missing(V14, "queryId").is_none());
    }
```

`src-tauri/src/diag/memory.rs`, `tests`:

```rust
    #[test]
    fn swap_returns_the_previous_snapshot_of_the_same_opening() {
        let m = CounterMemory::default();
        let s = |at| CounterSnapshot { at_ms: at, started_ms: 1, reset_ms: None, values: Default::default() };
        assert_eq!(m.swap("c", 1, "pg-1", Tool::Pulse, s(1)), None);
        assert_eq!(m.swap("c", 1, "pg-1", Tool::Pulse, s(2)).map(|p| p.at_ms), Some(1));
        assert_eq!(m.swap("c", 2, "pg-1", Tool::Pulse, s(3)), None, "новое открытие подключения — с чистого листа");
        assert_eq!(m.swap("c", 2, "pg-2", Tool::Pulse, s(3)), None, "другой узел");
        assert_eq!(m.swap("c", 2, "pg-1", Tool::Replication, s(3)), None, "другой инструмент");
        m.member_removed("c", "pg-1");
        assert_eq!(m.swap("c", 2, "pg-1", Tool::Pulse, s(4)), None);
        m.connection_closed("c");
        assert_eq!(m.swap("c", 2, "pg-2", Tool::Pulse, s(5)), None);
    }
```

`src-tauri/src/diag/mod.rs`, `tests`:

```rust
    #[test]
    fn request_validation() {
        let req = |tool, window_s| DiagRequest { tool, member: None, window_s };
        assert_eq!(validate(&req(Tool::Failover, None), ConnectionKind::Server).unwrap_err().key, "errors.diag.patroniOnly");
        assert!(validate(&req(Tool::Health, None), ConnectionKind::Patroni).is_ok());
        assert_eq!(validate(&req(Tool::Waits, Some(60)), ConnectionKind::Server).unwrap_err().key, "errors.diag.badWindow");
        assert!(validate(&req(Tool::Waits, Some(3600)), ConnectionKind::Server).is_ok());
        assert!(Tool::Pulse.is_cluster() && !Tool::Sessions.is_cluster());
    }

    #[test]
    fn request_and_report_json() {
        assert!(
            serde_json::from_value::<DiagRequest>(serde_json::json!({"tool": "idleTx"})).is_err(),
            "idleTx — вкладка фронтенда, в Rust это инструмент sessions"
        );
        let r: DiagRequest = serde_json::from_value(serde_json::json!({"tool": "sessions", "member": null})).unwrap();
        assert_eq!((r.tool, r.member, r.window_s), (Tool::Sessions, None, None));
        let report = DiagReport {
            tool: Tool::Sessions, member: Some("pg-1".into()), follows_leader: true, observed_at_ms: 5,
            server_version_num: Some(170004), in_recovery: Some(false),
            data: DiagData::Sessions(SessionsData { rows: vec![], settings: Default::default() }),
            findings: vec![], unavailable: vec![],
        };
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["tool"], "sessions");
        assert_eq!(v["followsLeader"], true);
        assert_eq!(v["data"]["kind"], "sessions");
        assert_eq!(v["data"]["rows"], serde_json::json!([]));
        assert_eq!(v["serverVersionNum"], 170004);
    }
```

`src-tauri/src/topology/observe.rs`, `tests`:

```rust
    #[test]
    fn leader_member_is_the_writable_node_regardless_of_target_server_type() {
        use crate::topology::model::{member, topo};
        let mut t = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        assert_eq!(leader_member(ConnectionKind::HostList, &t).map(|m| m.name.as_str()), Some("b"));
        assert_eq!(leader_member(ConnectionKind::Patroni, &t).map(|m| m.name.as_str()), Some("b"));
        assert_eq!(leader_member(ConnectionKind::Server, &t).map(|m| m.name.as_str()), Some("a"), "сервер — единственный (первый) узел");
        t.members[1].reachable = false;
        assert!(leader_member(ConnectionKind::HostList, &t).is_none(), "недоступный лидер не выбирается");
        let s = topo(vec![member("x", Role::StandbyLeader), member("y", Role::Replica)]);
        assert_eq!(leader_member(ConnectionKind::Patroni, &s).map(|m| m.name.as_str()), Some("x"));
    }
```

`src-tauri/src/topology/runtime.rs`, `tests` (источник `Scripted` и `sample_profile` уже есть в модуле):

```rust
    #[derive(Default)]
    struct Closed(Mutex<Vec<String>>);
    impl CloseListener for Closed {
        fn connection_closed(&self, id: &str) {
            self.0.lock_safe().push(format!("conn:{id}"));
        }
        fn member_removed(&self, id: &str, member: &str) {
            self.0.lock_safe().push(format!("member:{id}:{member}"));
        }
    }

    #[tokio::test]
    async fn diag_target_follows_the_leader_or_takes_the_named_member() {
        let (rt, _src, _rec, _pools) = manual(vec![Ok(two("b"))]);
        rt.connect(patroni_profile()).await.unwrap();
        let t = rt.diag_target("c", None, false).unwrap();
        assert_eq!((t.member.as_ref().map(|m| m.name.as_str()), t.follows_leader), (Some("b"), true));
        let t = rt.diag_target("c", Some("a"), false).unwrap();
        assert_eq!((t.member.as_ref().map(|m| m.name.as_str()), t.follows_leader), (Some("a"), false));
        let t = rt.diag_target("c", Some("a"), true).unwrap();
        assert!(t.member.is_none(), "инструмент кластера узла не берёт");
        assert_eq!(rt.diag_target("c", Some("zz"), false).unwrap_err().key, "errors.topology.unknownMember");
        assert_eq!(rt.diag_target("nope", None, false).unwrap_err().key, "errors.topology.notConnected");
    }

    #[tokio::test]
    async fn close_listeners_hear_disconnect_reopen_and_removed_members() {
        let (rt, src, _rec, _pools) = setup(vec![Ok(two("b"))]);
        let heard = Arc::new(Closed::default());
        rt.add_close_listener(heard.clone());
        let mut p = patroni_profile();
        rt.connect(p.clone()).await.unwrap();
        // Участник «a» исчез из топологии: следующий фоновый опрос (раз в 40 мс) его не увидит.
        src.then(vec![Ok(topo(vec![member("b", Role::Primary)]))]);
        eventually("участник a убран", || heard.0.lock_safe().contains(&"member:c:a".to_string())).await;
        // Переоткрытие с другими настройками соединения и закрытие.
        p.pg.connect_timeout_s += 1;
        rt.connect(p).await.unwrap();
        rt.disconnect("c");
        let log = heard.0.lock_safe().clone();
        assert_eq!(log.iter().filter(|e| *e == "conn:c").count(), 2, "{log:?}");
    }
```

(Помощники `manual`, `setup`, `patroni_profile`, `two(leader)` и `eventually` уже есть в модуле тестов `runtime.rs`: `two("b")` — участники `b` (primary) и `a` (replica).)

`src-tauri/src/error.rs`, `retryable_codes` — добавить `(ErrorCode::RecoveryConflict, true)`; `ssh_codes_serialize_for_frontend` — `assert_eq!(v(ErrorCode::RecoveryConflict)["code"], "RECOVERY_CONFLICT");`.

`src-tauri/src/conn/errors.rs`, `tests`:

```rust
    #[test]
    fn recovery_conflict_is_recognized_by_code_and_text() {
        let msg = "canceling statement due to conflict with recovery";
        assert!(is_recovery_conflict(&SqlState::T_R_SERIALIZATION_FAILURE, msg));
        assert!(is_recovery_conflict(&SqlState::T_R_DEADLOCK_DETECTED, msg));
        assert!(!is_recovery_conflict(&SqlState::T_R_SERIALIZATION_FAILURE, "could not serialize access"));
        assert!(!is_recovery_conflict(&SqlState::QUERY_CANCELED, msg));
    }
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml rules::sessions && cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация `pg-advisor`**

`crates/pg-advisor/src/rules/mod.rs`: `pub mod sessions;`.

`crates/pg-advisor/src/rules/sessions.rs` — структуры из Interfaces (`#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]`, `#[serde(rename_all = "camelCase")]`) и:

```rust
use crate::findings::{Finding, FindingTarget, Severity, grade, thresholds as th};

/// `idle in transaction` и `idle in transaction (aborted)`.
pub fn is_idle_in_transaction(r: &SessionRow) -> bool {
    r.state.as_deref().is_some_and(|s| s.starts_with("idle in transaction"))
}

fn is_client(r: &SessionRow) -> bool {
    !r.glafira && r.backend_type.as_deref() == Some("client backend")
}

/// Имя в SQL всегда в кавычках (как `readiness::quote_ident`): правило «простое — как есть» ломается на ключевых словах.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Худшая строка по `value` среди подходящих и число строк не ниже порога предупреждения.
fn worst<'a>(rows: impl Iterator<Item = &'a SessionRow>, value: impl Fn(&SessionRow) -> Option<f64>, warn: f64) -> Option<(&'a SessionRow, f64, usize)> {
    let mut best: Option<(&SessionRow, f64)> = None;
    let mut count = 0;
    for r in rows {
        let Some(v) = value(r) else { continue };
        if v >= warn {
            count += 1;
        }
        if best.is_none_or(|(_, b)| v > b) {
            best = Some((r, v));
        }
    }
    best.map(|(r, v)| (r, v, count))
}

fn session_finding(id: &str, member: Option<&str>, r: &SessionRow, v: f64, count: usize, (warn, crit): (f64, f64), tool: &str) -> Option<Finding> {
    let sev = grade(v, warn, crit)?;
    let mut f = Finding::new(id, sev).member(member).param("maxS", v).param("count", count).link_tool(tool, member);
    if let Some(us) = r.backend_start_us {
        f = f.target(FindingTarget::Session { pid: r.pid, backend_start_us: us });
    }
    Some(f)
}

pub fn evaluate(member: Option<&str>, d: &SessionsData) -> Vec<Finding> {
    let mut out = Vec::new();
    let idle = worst(d.rows.iter().filter(|r| is_client(r) && is_idle_in_transaction(r)), |r| r.state_age_s, th::IDLE_IN_TRANSACTION_S.0);
    if let Some((r, v, n)) = idle {
        out.extend(session_finding("idleInTransaction", member, r, v, n, th::IDLE_IN_TRANSACTION_S, "idleTx"));
        if v >= th::IDLE_IN_TRANSACTION_S.0 && d.settings.idle_in_transaction_session_timeout_ms == Some(0) {
            let role = r.user.as_deref().map(quote_ident).unwrap_or_else(|| "\"<role>\"".into());
            out.push(
                Finding::new("noIdleTimeout", Severity::Info)
                    .member(member)
                    .param("command", format!("ALTER ROLE {role} SET idle_in_transaction_session_timeout = '10min';")),
            );
        }
    }
    let active = |r: &&SessionRow| is_client(r) && r.state.as_deref() == Some("active");
    if let Some((r, v, n)) = worst(d.rows.iter().filter(active), |r| r.query_age_s, th::LONG_QUERY_S.0) {
        out.extend(session_finding("longQuery", member, r, v, n, th::LONG_QUERY_S, "sessions"));
    }
    if let Some((r, v, n)) = worst(d.rows.iter().filter(|r| is_client(r)), |r| r.xact_age_s, th::LONG_TRANSACTION_S.0) {
        out.extend(session_finding("longTransaction", member, r, v, n, th::LONG_TRANSACTION_S, "sessions"));
    }
    let hidden = d.rows.iter().filter(|r| r.query.as_deref() == Some("<insufficient privilege>")).count();
    if hidden > 0 {
        out.push(Finding::new("hiddenQueries", Severity::Info).member(member).param("count", hidden));
    }
    crate::findings::sort(&mut out);
    out
}
```

- [ ] **Step 4: Реализация каркаса в `src-tauri`**

`src-tauri/src/topology/observe.rs`:

```rust
/// Узел «за лидером» для диагностики (спец. P2 §3.2, уточнение 1 плана P2a): лидер Patroni (или лидер standby-кластера),
/// primary списка узлов независимо от `targetServerType`, единственный узел сервера. Только доступный.
pub fn leader_member(kind: ConnectionKind, t: &Topology) -> Option<&Member> {
    match kind {
        ConnectionKind::Server => t.members.first(),
        ConnectionKind::Patroni => t.leader(),
        ConnectionKind::HostList => t.members.iter().find(|m| m.role == Role::Primary),
    }
    .filter(|m| m.reachable)
}
```

`src-tauri/src/topology/runtime.rs`:
- `pub trait CloseListener: Send + Sync { fn connection_closed(&self, connection_id: &str); fn member_removed(&self, connection_id: &str, member: &str); }` — вызывается под блокировкой `entries`, как `RuntimeSink`: обработчик быстрый и не обращается к `Runtimes` (порядок блокировок в комментарии к `Runtimes` дополнить: «→ слушатели закрытия (std)»).
- Поле `listeners: Mutex<Vec<Arc<dyn CloseListener>>>`, `pub fn add_close_listener(&self, l: Arc<dyn CloseListener>)`, приватные `notify_closed(&self, id)` и `notify_member_removed(&self, id, member)`.
- Вызовы: в `disconnect` — после `entries.remove(id)` (если запись была); в `connect_locked` — когда прежняя запись удаляется (`if let Some(old) = entries.remove(&id)`); в `observe_entry` — рядом с `self.pools.evict_member(id, &m.name)` (участник исчез или сменил адрес).
- `DiagTarget` и метод:

```rust
/// Что нужно инструменту диагностики: профиль, номер открытия, топология и узел.
#[derive(Debug, Clone)]
pub struct DiagTarget {
    pub profile: ConnectionProfile,
    pub generation: u64,
    pub topology: Topology,
    /// `None` — инструмент кластера.
    pub member: Option<Member>,
    /// Узел выбран правилом «за лидером», а не пользователем.
    pub follows_leader: bool,
}

    /// Узел инструмента диагностики: `cluster` — без узла; `member` — названный участник; иначе «за лидером»
    /// (`observe::leader_member`). Подключение не открыто — `notConnected`; лидера нет — `noNode`.
    pub fn diag_target(&self, id: &str, member: Option<&str>, cluster: bool) -> AppResult<DiagTarget> {
        let entries = self.entries.lock_safe();
        let e = entries.get(id).ok_or_else(not_connected)?;
        let t = e.topology.clone().ok_or_else(not_connected)?;
        let (m, follows_leader) = if cluster {
            (None, false)
        } else if let Some(name) = member {
            let m = t.member(name).cloned().ok_or_else(|| {
                app_err!(ErrorCode::InvalidInput, "errors.topology.unknownMember", "Node {{member}} is not part of the connection", member = name)
            })?;
            (Some(m), false)
        } else {
            let m = leader_member(e.profile.kind, &t).cloned().ok_or_else(|| {
                app_err!(ErrorCode::NodeUnavailable, "errors.topology.noNode", "No suitable node is available")
            })?;
            (Some(m), true)
        };
        Ok(DiagTarget { profile: e.profile.clone(), generation: e.generation, topology: t, member: m, follows_leader })
    }
```

- `LiveSource::http_for` — `pub async fn http_for(&self, profile: &ConnectionProfile) -> AppResult<Arc<ReqwestHttp>>`.

`src-tauri/src/error.rs`: вариант `/// Запрос на узле в recovery отменён конфликтом с применением WAL. RecoveryConflict,` (между `FeatureUnavailable` и `Internal`), `retryable_code` — добавить `ErrorCode::RecoveryConflict`.

`src-tauri/src/conn/errors.rs`:

```rust
/// Конфликт recovery: запрос на реплике отменён, чтобы применить WAL (40001 — снимок, 40P01 — взаимная блокировка
/// с буфером). Код один с ошибками сериализации, поэтому решает текст сервера.
pub(crate) fn is_recovery_conflict(code: &SqlState, message: &str) -> bool {
    (*code == SqlState::T_R_SERIALIZATION_FAILURE || *code == SqlState::T_R_DEADLOCK_DETECTED)
        && message.contains("conflict with recovery")
}
```

и в `from_pg` перед `classify_sqlstate`:

```rust
        if is_recovery_conflict(code, db.message()) {
            return app_err!(
                ErrorCode::RecoveryConflict,
                "errors.pg.recoveryConflict",
                "The query on the replica was canceled by a recovery conflict"
            )
            .with_details(details);
        }
```

`src-tauri/src/diag/caps.rs`:

```rust
//! Возможности сервера по `server_version_num` (спец. P2 §3.3): колонка, которой нет в версии, выбирается как
//! `NULL::тип` — форма строки одна на все версии, а `unavailable` объясняет пропуск.

use crate::diag::Unavailable;

pub const V14: i32 = 140_000;
pub const V15: i32 = 150_000;
pub const V16: i32 = 160_000;
pub const V17: i32 = 170_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    pub version_num: i32,
}

impl Caps {
    pub fn new(version_num: i32) -> Self {
        Self { version_num }
    }

    pub fn at_least(self, since: i32) -> bool {
        self.version_num >= since
    }

    /// `expr`, если версия его знает, иначе `NULL::ty`.
    pub fn col(self, since: i32, expr: &str, ty: &str) -> String {
        if self.at_least(since) { expr.to_string() } else { format!("NULL::{ty}") }
    }

    /// Пометка «часть недоступна в этой версии»; `None` — доступна.
    pub fn missing(self, since: i32, part: &str) -> Option<Unavailable> {
        (!self.at_least(since)).then(|| Unavailable {
            part: part.to_string(),
            member: None,
            reason: ltext!("diag.unavailable.version", "{{part}}: PostgreSQL {{version}}+", part = part, version = since / 10_000),
        })
    }
}
```

`src-tauri/src/diag/sql.rs`:

```rust
//! Общие фрагменты SQL диагностики.

/// `backend_start` сессии (`pg_stat_activity` под псевдонимом `a`) в микросекундах эпохи. Одно выражение во всех
/// запросах: на PG 13 `extract(epoch …)` — `double precision`, и только одинаковое вычисление на одном сервере даёт
/// точное равенство при сверке PID (P2b); на 14+ `extract` возвращает `numeric` и точен сам по себе.
pub const BACKEND_START_US: &str = "(extract(epoch FROM a.backend_start) * 1000000)::bigint";

/// Версия, роль узла и время сервера — первая строка каждого узлового отчёта.
pub const HEAD_SQL: &str =
    "SELECT current_setting('server_version_num')::int, pg_is_in_recovery(), (extract(epoch FROM now()) * 1000)::bigint";
```

`src-tauri/src/diag/memory.rs`:

```rust
//! Прошлые счётчики для скоростей и роста (спец. P2 §3.6): ключ — подключение, номер открытия, узел, инструмент.

use std::collections::HashMap;
use std::sync::Mutex;

use pg_advisor::rates::CounterSnapshot;

use crate::diag::Tool;
use crate::sync::MutexExt;
use crate::topology::runtime::CloseListener;

type Key = (String, u64, String, Tool);

#[derive(Default)]
pub struct CounterMemory {
    inner: Mutex<HashMap<Key, CounterSnapshot>>,
}

impl CounterMemory {
    /// Запоминает `next` и возвращает прошлый снимок того же ключа.
    pub fn swap(&self, connection: &str, generation: u64, member: &str, tool: Tool, next: CounterSnapshot) -> Option<CounterSnapshot> {
        self.inner.lock_safe().insert((connection.to_string(), generation, member.to_string(), tool), next)
    }
}

impl CloseListener for CounterMemory {
    fn connection_closed(&self, connection_id: &str) {
        self.inner.lock_safe().retain(|k, _| k.0 != connection_id);
    }
    fn member_removed(&self, connection_id: &str, member: &str) {
        self.inner.lock_safe().retain(|k, _| !(k.0 == connection_id && k.2 == member));
    }
}
```

(Тест вызывает `connection_closed` и `member_removed` через трейт `CloseListener` — его нужно импортировать в модуль тестов.)

`src-tauri/src/diag/mod.rs` — типы из Interfaces (`Tool` — `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]`, `#[serde(rename_all = "camelCase")]`; `DiagReport`, `Unavailable` — `Serialize`, camelCase) и:

```rust
pub mod caps;
pub mod memory;
pub mod sessions;
pub mod sql;

pub fn validate(req: &DiagRequest, kind: ConnectionKind) -> AppResult<()> {
    if req.tool.patroni_only() && kind != ConnectionKind::Patroni {
        return Err(app_err!(ErrorCode::FeatureUnavailable, "errors.diag.patroniOnly", "This tool is available only for Patroni clusters"));
    }
    if let Some(w) = req.window_s
        && !WINDOWS_S.contains(&w)
    {
        return Err(app_err!(ErrorCode::InvalidInput, "errors.diag.badWindow", "The window must be 300, 900 or 3600 seconds"));
    }
    Ok(())
}

pub struct Head {
    pub version_num: i32,
    pub in_recovery: bool,
    pub now_ms: i64,
}

pub async fn head(c: &Client) -> AppResult<Head> {
    let r = c.query_one(sql::HEAD_SQL, &[]).await.map_err(|e| from_pg(&e))?;
    Ok(Head { version_num: r.get(0), in_recovery: r.get(1), now_ms: r.get(2) })
}

/// Запрос узлового инструмента на его узле: соединение пула `Catalog`, предел — `catalog_limit` профиля.
pub(crate) async fn on_member<T, F>(st: &AppState, id: &str, target: &DiagTarget, query: F) -> AppResult<T>
where
    T: Send,
    F: for<'a> FnOnce(&'a Client) -> BoxFuture<'a, AppResult<T>> + Send,
{
    let m = target.member.as_ref().map(|m| m.name.clone()).ok_or_else(|| AppError::internal_with("node tool without a member"))?;
    on_catalog_node_within(catalog_limit(&target.profile), st.client(id, Some(&m), None), query).await
}

pub(crate) fn make_report(tool: Tool, target: &DiagTarget, head: Option<&Head>, data: DiagData, mut findings: Vec<Finding>, unavailable: Vec<Unavailable>) -> DiagReport {
    pg_advisor::findings::sort(&mut findings);
    DiagReport {
        tool,
        member: target.member.as_ref().map(|m| m.name.clone()),
        follows_leader: target.follows_leader,
        observed_at_ms: head.map(|h| h.now_ms).unwrap_or_else(crate::topology::model::now_ms),
        server_version_num: head.map(|h| h.version_num),
        in_recovery: head.map(|h| h.in_recovery),
        data,
        findings,
        unavailable,
    }
}

pub async fn run(st: &AppState, id: &str, req: DiagRequest) -> AppResult<DiagReport> {
    let target = st.runtimes.diag_target(id, req.member.as_deref(), req.tool.is_cluster())?;
    validate(&req, target.profile.kind)?;
    match req.tool {
        Tool::Sessions => sessions::report(st, id, &target).await,
        // Задачи 4–9 добавляют свои ветки; до тех пор инструмент недоступен.
        other => Err(app_err!(ErrorCode::FeatureUnavailable, "errors.diag.notYet", "The tool {{tool}} is not available yet", tool = format!("{other:?}"))),
    }
}
```

(Ветка `other` и ключ `errors.diag.notYet` удаляются в задаче 9, когда покрыты все инструменты; до этого ключ есть в словарях.)

`src-tauri/src/diag/sessions.rs`:

```rust
//! Сессии и idle in transaction (спец. P2 §5.2).

use pg_advisor::rules::sessions::{self as rules, SessionRow, SessionSettings, SessionsData};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14};
use crate::diag::{DiagData, DiagReport, Tool, head, make_report, on_member, sql::BACKEND_START_US};
use crate::error::AppResult;
use crate::state::AppState;
use crate::topology::runtime::DiagTarget;

/// Текст запроса в списке не длиннее (уточнение 6 плана P2a).
pub const QUERY_LIMIT: i32 = 16_384;

pub fn sessions_sql(caps: Caps) -> String {
    let query_id = caps.col(V14, "a.query_id", "bigint");
    format!(
        "SELECT a.pid, a.leader_pid, a.backend_type, a.datname::text, a.usename::text, a.application_name, \
         host(a.client_addr), a.client_port, {BACKEND_START_US}, \
         extract(epoch FROM now() - a.xact_start)::float8, extract(epoch FROM now() - a.query_start)::float8, \
         extract(epoch FROM now() - a.state_change)::float8, a.state, a.wait_event_type, a.wait_event, \
         a.backend_xid::text, age(a.backend_xmin)::bigint, {query_id}, left(a.query, {QUERY_LIMIT}), \
         length(a.query) > {QUERY_LIMIT}, \
         CASE WHEN a.wait_event_type = 'Lock' THEN pg_blocking_pids(a.pid) ELSE '{{}}'::int[] END, \
         COALESCE(l.n, 0)::bigint, a.application_name = 'glafira-pg' \
         FROM pg_stat_activity a \
         LEFT JOIN (SELECT pid, count(*) AS n FROM pg_locks WHERE granted GROUP BY pid) l ON l.pid = a.pid \
         WHERE a.pid <> pg_backend_pid() ORDER BY a.pid"
    )
}

const SETTINGS_SQL: &str = "SELECT name::text, setting FROM pg_settings \
    WHERE name IN ('idle_in_transaction_session_timeout', 'track_activity_query_size')";

pub async fn collect(c: &Client, caps: Caps) -> AppResult<SessionsData> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let rows = c.query(&sessions_sql(caps), &[]).await.map_err(pg)?;
    let rows = rows.iter().map(|r| SessionRow {
        pid: r.get(0), leader_pid: r.get(1), backend_type: r.get(2), database: r.get(3), user: r.get(4),
        application: r.get(5), client_addr: r.get(6), client_port: r.get(7), backend_start_us: r.get(8),
        xact_age_s: r.get(9), query_age_s: r.get(10), state_age_s: r.get(11), state: r.get(12),
        wait_event_type: r.get(13), wait_event: r.get(14), backend_xid: r.get(15), xmin_age: r.get(16),
        query_id: r.get(17), query: r.get(18), query_truncated: r.get::<_, Option<bool>>(19).unwrap_or(false),
        blocked_by: r.get(20), locks_held: r.get(21), glafira: r.get::<_, Option<bool>>(22).unwrap_or(false),
    }).collect();
    let mut settings = SessionSettings::default();
    for r in c.query(SETTINGS_SQL, &[]).await.map_err(pg)? {
        let (name, value): (String, Option<String>) = (r.get(0), r.get(1));
        let v = value.and_then(|v| v.parse::<i64>().ok());
        match name.as_str() {
            "idle_in_transaction_session_timeout" => settings.idle_in_transaction_session_timeout_ms = v,
            _ => settings.track_activity_query_size = v,
        }
    }
    Ok(SessionsData { rows, settings })
}

pub async fn report(st: &AppState, id: &str, target: &DiagTarget) -> AppResult<DiagReport> {
    let (h, data) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let data = collect(c, Caps::new(h.version_num)).await?;
            Ok((h, data))
        })
    })
    .await?;
    let member = target.member.as_ref().map(|m| m.name.as_str());
    let findings = rules::evaluate(member, &data);
    let unavailable = Caps::new(h.version_num).missing(V14, "queryId").into_iter().collect();
    Ok(make_report(Tool::Sessions, target, Some(&h), DiagData::Sessions(data), findings, unavailable))
}
```

(Запрос `track_activity_query_size` в `pg_settings` — байты, `idle_in_transaction_session_timeout` — миллисекунды. Тип `age(…)` — `integer`, приводится к `bigint`; `backend_xid` — `xid`, приводится к тексту.)

`src-tauri/src/commands/diag.rs`:

```rust
//! Команды диагностики (спец. P2 §9).

use tauri::State;

use crate::diag::{DiagReport, DiagRequest};
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn run_diagnostic(state: State<'_, AppState>, id: String, request: DiagRequest) -> AppResult<DiagReport> {
    crate::diag::run(&state, &id, request).await
}
```

`src-tauri/src/commands/mod.rs`: `pub mod diag;`. `src-tauri/src/lib.rs`: `pub mod diag;`, в `generate_handler!` — `commands::diag::run_diagnostic`. `src-tauri/src/commands/catalog.rs`: `pub(crate) fn catalog_limit`.

`src-tauri/src/state.rs`: поля `pub live: Arc<LiveSource>`, `pub counters: Arc<crate::diag::memory::CounterMemory>`; в `new_with_logs` — `let live = Arc::new(LiveSource::new(secrets.clone(), pools.clone(), tunnels.clone()));`, `Runtimes::new(live.clone(), …)`, `let counters = Arc::new(CounterMemory::default()); runtimes.add_close_listener(counters.clone());`.

Словари `src/locales/{en,ru}.json`:
- `errors.pg.recoveryConflict`: «The query on the replica was canceled by a recovery conflict» / «Запрос на реплике отменён конфликтом recovery»;
- `errors.diag.patroniOnly`: «This tool is available only for Patroni clusters» / «Инструмент доступен только для кластера Patroni»;
- `errors.diag.badWindow`: «The window must be 300, 900 or 3600 seconds» / «Окно — 300, 900 или 3600 секунд»;
- `errors.diag.notYet`: «The tool {{tool}} is not available yet» / «Инструмент {{tool}} пока недоступен»;
- новый раздел верхнего уровня `diag` (перед `hints`): `diag.unavailable.version`: «{{part}}: PostgreSQL {{version}}+» (ru — то же; ключ — в `docs/i18n/same-as-en.json`).

- [ ] **Step 5: Интеграционные тесты**

`src-tauri/tests/it_diag.rs`:

```rust
//! Инструменты диагностики на стенде (спец. P2 §5): SQL каждого набора на PG 13–18 (`GPG_IT_MATRIX`) и сценарии
//! на узлах стенда. Запускать с `--test-threads=1`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use glafira_pg_lib::diag::caps::Caps;
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use common::open_server;

fn req(tool: Tool) -> DiagRequest {
    DiagRequest { tool, member: None, window_s: None }
}

/// Все узлы, на которых проверяется SQL: стенд и матрица.
fn sql_addrs() -> Vec<String> {
    ["GPG_IT_SINGLE", "GPG_IT_PRIMARY", "GPG_IT_REPLICA", "GPG_IT_LAB"].into_iter().filter_map(common::env).chain(common::matrix_addrs()).collect()
}

#[tokio::test]
async fn sessions_sql_runs_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(&addr, "postgres", common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let h = diag::head(&c).await.unwrap();
        let d = diag::sessions::collect(&c, Caps::new(h.version_num)).await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(d.settings.track_activity_query_size.is_some(), "{addr}");
        assert!(d.rows.iter().all(|r| r.backend_start_us.is_some_and(|us| us > 1_600_000_000_000_000)), "{addr}");
    }
}

#[tokio::test]
async fn idle_in_transaction_session_is_reported_with_its_locks() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    app.batch_execute("BEGIN; UPDATE shop.orders SET status = status WHERE id = 1;").await.unwrap();
    let pid: i32 = app.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let (_dir, st) = open_server(&addr).await;
    let r = diag::run(&st, "it", req(Tool::Sessions)).await.unwrap();
    assert_eq!((r.tool, r.follows_leader, r.in_recovery), (Tool::Sessions, true, Some(false)));
    let DiagData::Sessions(d) = r.data else { panic!("{:?}", r.data) };
    let row = d.rows.iter().find(|x| x.pid == pid).expect("сессия видна");
    assert_eq!(row.state.as_deref(), Some("idle in transaction"));
    assert!(row.state_age_s.unwrap() >= 1.0);
    assert!(row.locks_held >= 2, "держит блокировку строки и таблицы");
    assert_eq!(row.user.as_deref(), Some("glafira_app"));
    assert!(d.rows.iter().any(|x| x.glafira), "соединения Glafira помечены");
    app.batch_execute("ROLLBACK").await.unwrap();
}

#[tokio::test]
async fn patroni_tool_on_a_server_is_refused() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    let (_dir, st) = open_server(&addr).await;
    let e = diag::run(&st, "it", req(Tool::Failover)).await.unwrap_err();
    assert_eq!(e.key, "errors.diag.patroniOnly");
}
```

`src-tauri/tests/common/mod.rs` (общий для `it_diag.rs` и `it_sampler.rs` задачи 9):

```rust
/// Открытое подключение-сервер к `addr` под ролью диагностики (id `it`); каталог данных — во временной папке,
/// которая живёт, пока жив возвращённый `TempDir`.
pub async fn open_server(addr: &str) -> (tempfile::TempDir, glafira_pg_lib::state::AppState) {
    let dir = tempfile::tempdir().unwrap();
    let st = glafira_pg_lib::state::AppState::new(dir.path(), secrets_with_password("it")).unwrap();
    let mut p = server_profile("it", addr);
    p.pg.ssl_mode = SslMode::Disable;
    st.connections.upsert(p.clone()).unwrap();
    st.runtimes.connect(p).await.unwrap();
    (dir, st)
}
```

(`common::PG_USER`, `common::PG_PASSWORD_VALUE` — существующие константы роли `glafira_diag`. `AppState::new` и `connections.upsert` — как в тестах `state.rs`.)

- [ ] **Step 6: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS.

Run: интеграционные из Global Constraints с `--test it_diag`
Expected: PASS (без стенда тесты пропускаются).

- [ ] **Step 7: Commit**

```bash
git add crates/pg-advisor src-tauri src/locales docs/i18n/same-as-en.json
git commit -m "feat(diag): каркас диагностики — конверт отчёта, узел за лидером, версии; инструмент «Сессии»

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 4: Пульс узла и кластера

**Files:**
- Create: `crates/pg-advisor/src/rules/pulse.rs`, `src-tauri/src/diag/pulse.rs`
- Modify: `crates/pg-advisor/src/rules/mod.rs`, `src-tauri/src/diag/mod.rs` (вариант `DiagData::Pulse`, ветка `run`), `src-tauri/tests/it_diag.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `rules/pulse.rs`, `diag/pulse.rs` (SQL по версиям); интеграционные в `it_diag.rs`

**Interfaces:**
- Consumes: `CounterSnapshot`, `rate`, `delta`, `grade`, `worse`, `thresholds` (1); `DiagTarget`, `CounterMemory::swap`, `make_report`, `Caps`, `on_catalog_node_within`, `catalog_limit` (3).
- Produces:
  - `pg_advisor::rules::pulse::{ConnectionsByState { active, idle, idle_in_transaction, other: i64 }, PulseFacts { used: i64, available: i64, by_state, longest_transaction_s: Option<f64>, longest_query_s: Option<f64>, longest_idle_in_transaction_s: Option<f64>, lock_waiting: i64, longest_lock_wait_s: Option<f64>, max_xid_age: i64, xid_age_pct: f64, wal_dir_bytes: Option<i64>, max_wal_size_bytes: Option<i64>, lag_bytes: Option<i64>, replay_delay_ms: Option<i64> }, PulseRates { tps, commits_per_s, rollbacks_per_s, cache_hit_pct, temp_files_per_s, temp_bytes_per_s, wal_bytes_per_s: Option<f64>, deadlocks: Option<i64>, temp_files: Option<i64>, blks_read: Option<i64> }, COUNTERS: [&str; 8], rates(prev: Option<&CounterSnapshot>, next: &CounterSnapshot) -> PulseRates, evaluate(member: &str, standby: bool, f: &PulseFacts, r: &PulseRates) -> Vec<Finding>}`;
  - `diag::pulse::{PulseNode { member: String, role: Role, reachable: bool, error: Option<AppError>, facts: Option<PulseFacts>, rates: Option<PulseRates> }, PulseData { nodes: Vec<PulseNode> }, pulse_sql(Caps) -> String, collect(&Client, Caps) -> AppResult<(PulseFacts, CounterSnapshot, Option<Unavailable>)>, report(st, id, target)}`; `DiagData::Pulse(PulseData)`;
  - ключ `diag.unavailable.privilege` («{{part}}: needs the pg_monitor role»).

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/rules/pulse.rs`, `tests`:

```rust
    use super::*;
    use crate::findings::Severity;
    use crate::rates::CounterSnapshot;

    fn facts() -> PulseFacts {
        PulseFacts { used: 10, available: 100, max_xid_age: 1000, xid_age_pct: 0.0, ..Default::default() }
    }

    fn snap(at_ms: i64, v: [i64; 8]) -> CounterSnapshot {
        CounterSnapshot { at_ms, started_ms: 1, reset_ms: None, values: COUNTERS.iter().map(|k| k.to_string()).zip(v).collect() }
    }

    fn ids(f: &[Finding]) -> Vec<(&str, Severity)> {
        f.iter().map(|x| (x.id.as_str(), x.severity)).collect()
    }

    #[test]
    fn rates_from_two_snapshots() {
        // commits, rollbacks, blksHit, blksRead, tempFiles, tempBytes, deadlocks, walLsn
        let a = snap(0, [100, 10, 9000, 1000, 0, 0, 0, 5_000]);
        let b = snap(2_000, [300, 30, 9900, 1100, 2, 4096, 1, 5_000 + 2 * 1024 * 1024]);
        let r = rates(Some(&a), &b);
        assert_eq!(r.tps, Some(110.0));
        assert_eq!((r.commits_per_s, r.rollbacks_per_s), (Some(100.0), Some(10.0)));
        assert_eq!(r.cache_hit_pct, Some(90.0));
        assert_eq!((r.temp_files, r.deadlocks, r.blks_read), (Some(2), Some(1), Some(100)));
        assert_eq!(r.wal_bytes_per_s, Some(1024.0 * 1024.0));
        let first = rates(None, &b);
        assert_eq!((first.tps, first.cache_hit_pct, first.deadlocks), (None, None, None));
    }

    #[test]
    fn findings_follow_the_thresholds() {
        let mut f = facts();
        f.used = 96;
        f.longest_idle_in_transaction_s = Some(400.0);
        f.longest_query_s = Some(4000.0);
        f.xid_age_pct = 60.0;
        f.lock_waiting = 3;
        f.longest_lock_wait_s = Some(31.0);
        f.wal_dir_bytes = Some(5 * 1024);
        f.max_wal_size_bytes = Some(1024);
        let r = PulseRates { deadlocks: Some(2), cache_hit_pct: Some(50.0), blks_read: Some(5000), temp_files: Some(1), ..Default::default() };
        let got = evaluate("pg-1", false, &f, &r);
        assert_eq!(
            ids(&got),
            [
                ("connectionsSaturated", Severity::Critical),
                ("longQuery", Severity::Critical),
                ("walDirSize", Severity::Critical),
                ("deadlocks", Severity::Warning),
                ("idleInTransaction", Severity::Warning),
                ("lockWait", Severity::Warning),
                ("xidAge", Severity::Warning),
                ("cacheHitLow", Severity::Info),
                ("tempFiles", Severity::Info),
            ]
        );
        assert!(got.iter().all(|x| x.member.as_deref() == Some("pg-1")));
        assert_eq!(got[0].params["usedPct"], 96.0);
    }

    #[test]
    fn replica_lag_only_on_standbys_and_cache_hit_needs_enough_reads() {
        let mut f = facts();
        f.lag_bytes = Some(2 * 1024 * 1024 * 1024);
        f.replay_delay_ms = Some(40_000);
        assert_eq!(ids(&evaluate("r", true, &f, &PulseRates::default())), [("replicaLag", Severity::Critical)]);
        assert!(evaluate("p", false, &f, &PulseRates::default()).is_empty(), "у primary лаг из топологии не оценивается");
        let r = PulseRates { cache_hit_pct: Some(10.0), blks_read: Some(10), ..Default::default() };
        assert!(evaluate("p", false, &facts(), &r).is_empty(), "мало чтений — нет вывода о cache hit");
    }
```

`src-tauri/src/diag/pulse.rs`, `tests`:

```rust
    #[test]
    fn pulse_sql_uses_waitstart_and_reserved_connections_only_where_they_exist() {
        let old = pulse_sql(Caps::new(130000));
        let mid = pulse_sql(Caps::new(150000));
        let new = pulse_sql(Caps::new(170000));
        assert!(!old.contains("waitstart") && mid.contains("waitstart") && new.contains("waitstart"));
        assert!(!mid.contains("current_setting('reserved_connections')") && new.contains("current_setting('reserved_connections')"));
        assert!(old.contains("NULL::int"));
    }
```

`src-tauri/tests/it_diag.rs`:

```rust
#[tokio::test]
async fn pulse_sql_runs_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(&addr, "postgres", common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let h = diag::head(&c).await.unwrap();
        let (f, snap, unavailable) = diag::pulse::collect(&c, Caps::new(h.version_num)).await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(f.used >= 1 && f.available > f.used, "{addr}");
        assert!(f.max_xid_age > 0, "{addr}");
        assert!(f.wal_dir_bytes.is_some() && unavailable.is_none(), "{addr}: у glafira_diag есть pg_monitor");
        assert!(snap.values.contains_key("commits"), "{addr}");
    }
}

#[tokio::test]
async fn pulse_on_patroni_has_a_row_per_member_and_rates_from_the_second_call() {
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let req = DiagRequest { tool: Tool::Pulse, member: Some("ignored".into()), window_s: None };
    let first = diag::run(&st, "it", req.clone()).await.unwrap();
    assert_eq!((first.member, first.follows_leader), (None, false));
    let DiagData::Pulse(d) = first.data else { panic!() };
    assert_eq!(d.nodes.len(), 3);
    assert!(d.nodes.iter().all(|n| n.facts.is_some() && n.error.is_none()), "{:?}", d.nodes);
    assert!(d.nodes.iter().all(|n| n.rates.as_ref().unwrap().tps.is_none()), "первый вызов — без скоростей");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let DiagData::Pulse(d) = diag::run(&st, "it", req).await.unwrap().data else { panic!() };
    assert!(d.nodes.iter().all(|n| n.rates.as_ref().unwrap().tps.is_some()));
}
```

и помощник в `src-tauri/tests/common/mod.rs`:

```rust
/// Открытое подключение к кластеру Patroni стенда (`GPG_IT_PATRONI`, id `it`); без переменной — `None`.
pub async fn open_patroni() -> Option<(tempfile::TempDir, glafira_pg_lib::state::AppState)> {
    use glafira_pg_lib::conn::profile::ConnectionKind;
    use glafira_pg_lib::secrets::{PATRONI_PASSWORD, SecretStore, secret_key};
    let settings = patroni_settings()?;
    let dir = tempfile::tempdir().unwrap();
    let secrets = secrets_with_password("it");
    secrets.set(&secret_key("it", PATRONI_PASSWORD), PATRONI_PASSWORD_VALUE).unwrap();
    let st = glafira_pg_lib::state::AppState::new(dir.path(), secrets).unwrap();
    let mut p = server_profile("it", "localhost:5441");
    p.kind = ConnectionKind::Patroni;
    p.hosts.clear();
    p.patroni = Some(settings);
    p.pg.ssl_mode = SslMode::Disable;
    st.connections.upsert(p.clone()).unwrap();
    st.runtimes.connect(p).await.unwrap();
    Some((dir, st))
}
```

(`DiagRequest` получает `#[derive(Clone)]`, если его ещё нет.)

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml rules::pulse && cargo test --manifest-path src-tauri/Cargo.toml --lib diag::`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация `pg-advisor`**

`rules/mod.rs`: `pub mod pulse;`. `rules/pulse.rs` — структуры из Interfaces (`#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]`, camelCase) и:

```rust
use crate::findings::{Finding, Severity, grade, thresholds as th, worse};
use crate::rates::{CounterSnapshot, delta, rate};

/// Ключи счётчиков снимка пульса; `walLsn` — позиция WAL в байтах от `0/0`.
pub const COUNTERS: [&str; 8] = ["commits", "rollbacks", "blksHit", "blksRead", "tempFiles", "tempBytes", "deadlocks", "walLsn"];

pub fn rates(prev: Option<&CounterSnapshot>, next: &CounterSnapshot) -> PulseRates {
    let r = |k| rate(prev, next, k);
    let d = |k| delta(prev, next, k);
    let (commits, rollbacks) = (r("commits"), r("rollbacks"));
    let (hit, read) = (d("blksHit"), d("blksRead"));
    PulseRates {
        tps: commits.zip(rollbacks).map(|(c, b)| c + b),
        commits_per_s: commits,
        rollbacks_per_s: rollbacks,
        cache_hit_pct: hit.zip(read).filter(|(h, rd)| h + rd > 0).map(|(h, rd)| h as f64 * 100.0 / (h + rd) as f64),
        temp_files_per_s: r("tempFiles"),
        temp_bytes_per_s: r("tempBytes"),
        wal_bytes_per_s: r("walLsn"),
        deadlocks: d("deadlocks"),
        temp_files: d("tempFiles"),
        blks_read: read,
    }
}

pub fn evaluate(member: &str, standby: bool, f: &PulseFacts, r: &PulseRates) -> Vec<Finding> {
    let m = Some(member);
    let mut out = Vec::new();
    let mut push = |id: &str, sev: Option<Severity>, f: Finding| {
        if let Some(sev) = sev {
            let mut f = f;
            f.id = id.to_string();
            f.severity = sev;
            out.push(f.member(m));
        }
    };
    let base = |tool: &str| Finding::new("", Severity::Info).link_tool(tool, m);
    if f.available > 0 {
        let pct = f.used as f64 * 100.0 / f.available as f64;
        push("connectionsSaturated", grade(pct, th::CONNECTIONS_PCT.0, th::CONNECTIONS_PCT.1),
            base("sessions").param("usedPct", pct).param("used", f.used).param("available", f.available));
    }
    if let Some(v) = f.longest_idle_in_transaction_s {
        push("idleInTransaction", grade(v, th::IDLE_IN_TRANSACTION_S.0, th::IDLE_IN_TRANSACTION_S.1), base("idleTx").param("maxS", v));
    }
    if let Some(v) = f.longest_query_s {
        push("longQuery", grade(v, th::LONG_QUERY_S.0, th::LONG_QUERY_S.1), base("sessions").param("maxS", v));
    }
    if let Some(v) = f.longest_transaction_s {
        push("longTransaction", grade(v, th::LONG_TRANSACTION_S.0, th::LONG_TRANSACTION_S.1), base("sessions").param("maxS", v));
    }
    push("xidAge", grade(f.xid_age_pct, th::XID_AGE_PCT.0, th::XID_AGE_PCT.1),
        base("horizon").param("agePct", f.xid_age_pct).param("maxXidAge", f.max_xid_age));
    if let Some(v) = f.longest_lock_wait_s {
        push("lockWait", grade(v, th::LOCK_WAIT_S.0, th::LOCK_WAIT_S.1), base("locks").param("maxS", v).param("count", f.lock_waiting));
    }
    if let (Some(wal), Some(max)) = (f.wal_dir_bytes, f.max_wal_size_bytes.filter(|m| *m > 0)) {
        let ratio = wal as f64 / max as f64;
        push("walDirSize", grade(ratio, th::WAL_DIR_RATIO.0, th::WAL_DIR_RATIO.1),
            base("slots").param("walDirBytes", wal).param("maxWalSizeBytes", max));
    }
    if let Some(n) = r.deadlocks.filter(|n| *n > 0) {
        push("deadlocks", Some(Severity::Warning), base("locks").param("count", n));
    }
    if standby {
        let by_bytes = f.lag_bytes.and_then(|b| grade(b as f64, th::REPLICA_LAG_BYTES.0, th::REPLICA_LAG_BYTES.1));
        let by_time = f.replay_delay_ms.and_then(|ms| grade(ms as f64 / 1000.0, th::REPLICA_LAG_S.0, th::REPLICA_LAG_S.1));
        let mut lag = base("replication");
        if let Some(b) = f.lag_bytes { lag = lag.param("lagBytes", b); }
        if let Some(ms) = f.replay_delay_ms { lag = lag.param("delayS", ms as f64 / 1000.0); }
        push("replicaLag", worse(by_bytes, by_time), lag);
    }
    if let (Some(pct), Some(reads)) = (r.cache_hit_pct, r.blks_read)
        && pct < th::CACHE_HIT_LOW_PCT && reads >= th::CACHE_HIT_MIN_READS
    {
        push("cacheHitLow", Some(Severity::Info), base("sessions").param("hitPct", pct));
    }
    if let Some(n) = r.temp_files.filter(|n| *n > 0) {
        push("tempFiles", Some(Severity::Info), base("sessions").param("count", n));
    }
    crate::findings::sort(&mut out);
    out
}
```

(Ожидание теста `findings_follow_the_thresholds` — порядок `sort`: critical → info, внутри строгости — по id.)

- [ ] **Step 4: Реализация сбора**

`src-tauri/src/diag/pulse.rs`:

```rust
//! Пульс узла и кластера (спец. P2 §5.1): все доступные участники параллельно, по одному запросу на узел.

pub fn pulse_sql(caps: Caps) -> String {
    let reserved = caps.col(V16, "current_setting('reserved_connections')::int", "int");
    let lock_wait = if caps.at_least(V14) {
        "(SELECT max(extract(epoch FROM now() - l.waitstart)) FROM pg_locks l WHERE NOT l.granted)"
    } else {
        "(SELECT max(extract(epoch FROM now() - state_change)) FROM a WHERE wait_event_type = 'Lock')"
    };
    format!(
        "WITH a AS (SELECT state, xact_start, query_start, state_change, wait_event_type FROM pg_stat_activity \
                    WHERE backend_type = 'client backend'), \
              d AS (SELECT sum(xact_commit)::bigint AS c, sum(xact_rollback)::bigint AS r, sum(blks_hit)::bigint AS h, \
                           sum(blks_read)::bigint AS rd, sum(temp_files)::bigint AS tf, sum(temp_bytes)::bigint AS tb, \
                           sum(deadlocks)::bigint AS dl, max((extract(epoch FROM stats_reset) * 1000)::bigint) AS reset \
                    FROM pg_stat_database) \
         SELECT (SELECT count(*) FROM a)::bigint, \
                (SELECT count(*) FROM a WHERE state = 'active')::bigint, \
                (SELECT count(*) FROM a WHERE state = 'idle')::bigint, \
                (SELECT count(*) FROM a WHERE state LIKE 'idle in transaction%')::bigint, \
                (current_setting('max_connections')::int - current_setting('superuser_reserved_connections')::int \
                 - COALESCE({reserved}, 0))::bigint, \
                (SELECT max(extract(epoch FROM now() - xact_start)) FROM a)::float8, \
                (SELECT max(extract(epoch FROM now() - query_start)) FROM a WHERE state = 'active')::float8, \
                (SELECT max(extract(epoch FROM now() - state_change)) FROM a WHERE state LIKE 'idle in transaction%')::float8, \
                (SELECT count(*) FROM a WHERE wait_event_type = 'Lock')::bigint, \
                {lock_wait}::float8, \
                (SELECT max(age(datfrozenxid)) FROM pg_database)::bigint, \
                d.c, d.r, d.h, d.rd, d.tf, d.tb, d.dl, d.reset, \
                (CASE WHEN pg_is_in_recovery() THEN pg_wal_lsn_diff(pg_last_wal_receive_lsn(), '0/0') \
                      ELSE pg_wal_lsn_diff(pg_current_wal_lsn(), '0/0') END)::bigint, \
                (extract(epoch FROM pg_postmaster_start_time()) * 1000)::bigint, \
                (extract(epoch FROM now()) * 1000)::bigint, \
                (SELECT setting::bigint * 1024 * 1024 FROM pg_settings WHERE name = 'max_wal_size') \
         FROM d"
    )
}

const WAL_DIR_SQL: &str = "SELECT COALESCE(sum(size), 0)::bigint FROM pg_ls_waldir()";

/// Факты, снимок счётчиков и пометка о недоступном размере `pg_wal` (нет `pg_monitor`).
pub async fn collect(c: &Client, caps: Caps) -> AppResult<(PulseFacts, CounterSnapshot, Option<Unavailable>)> {
    let r = c.query_one(&pulse_sql(caps), &[]).await.map_err(|e| from_pg(&e))?;
    let (used, active, idle, idle_tx): (i64, i64, i64, i64) = (r.get(0), r.get(1), r.get(2), r.get(3));
    let max_xid_age: i64 = r.get::<_, Option<i64>>(10).unwrap_or(0);
    let mut values = BTreeMap::new();
    for (i, key) in COUNTERS.iter().enumerate().take(7) {
        if let Some(v) = r.get::<_, Option<i64>>(11 + i) {
            values.insert(key.to_string(), v);
        }
    }
    if let Some(lsn) = r.get::<_, Option<i64>>(19) {
        values.insert("walLsn".into(), lsn);
    }
    let snapshot = CounterSnapshot { at_ms: r.get(21), started_ms: r.get(20), reset_ms: r.get(18), values };
    let (wal_dir_bytes, unavailable) = match c.query_one(WAL_DIR_SQL, &[]).await {
        Ok(w) => (Some(w.get(0)), None),
        Err(e) if from_pg(&e).code == ErrorCode::NotAuthorized => (None, Some(Unavailable {
            part: "walDir".into(),
            member: None,
            reason: ltext!("diag.unavailable.privilege", "{{part}}: needs the pg_monitor role", part = "walDir"),
        })),
        Err(e) => return Err(from_pg(&e)),
    };
    let facts = PulseFacts {
        used, available: r.get(4),
        by_state: ConnectionsByState { active, idle, idle_in_transaction: idle_tx, other: (used - active - idle - idle_tx).max(0) },
        longest_transaction_s: r.get(5), longest_query_s: r.get(6), longest_idle_in_transaction_s: r.get(7),
        lock_waiting: r.get(8), longest_lock_wait_s: r.get(9),
        max_xid_age, xid_age_pct: max_xid_age as f64 * 100.0 / pg_advisor::findings::thresholds::XID_LIMIT,
        wal_dir_bytes, max_wal_size_bytes: r.get(22), lag_bytes: None, replay_delay_ms: None,
    };
    Ok((facts, snapshot, unavailable))
}
```

(Колонки по порядку: 0–3 соединения, 4 доступно, 5–7 длительности, 8 ждут блокировку, 9 дольше всех ждёт, 10 возраст XID, 11–17 счётчики `commits…deadlocks`, 18 `reset`, 19 LSN, 20 запуск postmaster, 21 `now()`, 22 `max_wal_size`. Порядок `COUNTERS` совпадает с колонками 11–17 и `walLsn`.)

`report`: для каждого участника `target.topology.members` — если `!reachable`, строка `PulseNode { reachable: false, error: m.error.clone(), facts: None, rates: None }`; иначе запрос `on_catalog_node_within(catalog_limit(&target.profile), st.client(id, Some(&m.name), None), |c| Box::pin(async move { let h = head(c).await?; collect(c, Caps::new(h.version_num)).await }))`. Запросы — параллельно (`futures_util::future::join_all`). Ошибка — в строку (`error: Some(e)`). Для успешных: `facts.lag_bytes = m.lag_bytes`, `facts.replay_delay_ms = m.replay_delay_ms`; `prev = st.counters.swap(id, target.generation, &m.name, Tool::Pulse, snapshot)`; `rates = pulse::rates(prev.as_ref(), &next)`; находки — `pulse::evaluate(&m.name, m.role.is_standby(), &facts, &rates)`; `unavailable` узла — с `member = Some(m.name)`. Итог — `make_report(Tool::Pulse, target, None, DiagData::Pulse(PulseData { nodes }), findings, unavailable)`.

`diag/mod.rs`: `pub mod pulse;`, вариант `Pulse(pulse::PulseData)`, ветка `Tool::Pulse => pulse::report(st, id, &target).await`.

Словари: `diag.unavailable.privilege` — «{{part}}: needs the pg_monitor role» / «{{part}}: нужна роль pg_monitor».

- [ ] **Step 5: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`, затем интеграционные (`--test it_diag`).
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/pg-advisor src-tauri src/locales
git commit -m "feat(diag): пульс — показатели всех узлов, скорости по двум снимкам, находки

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Блокировки и DDL в очереди

**Files:**
- Create: `crates/pg-advisor/src/locktree.rs`, `crates/pg-advisor/src/rules/locks.rs`, `src-tauri/src/diag/locks.rs`
- Modify: `crates/pg-advisor/src/lib.rs`, `crates/pg-advisor/src/rules/mod.rs`, `src-tauri/src/diag/mod.rs`, `src-tauri/tests/common/mod.rs`, `src-tauri/tests/it_diag.rs`
- Test: unit-тесты в `locktree.rs`, `rules/locks.rs`; интеграционные в `it_diag.rs`

**Interfaces:**
- Consumes: `Finding`, `grade`, `thresholds` (1); `BACKEND_START_US`, `Caps`, `on_member`, `make_report` (3); `common::{app, superuser, ensure_fixtures}` (2).
- Produces:
  - `pg_advisor::locktree::{LockSession { pid: i32, backend_start_us: Option<i64>, user, application, database, state, query: Option<String>, xact_age_s: Option<f64>, query_age_s: Option<f64> }, WaitingLock { locktype: Option<String>, mode: Option<String>, relation: Option<String>, relation_oid: Option<i64>, relation_database: Option<String>, wait_s: Option<f64>, approx: bool }, LockRow { session, blocked_by: Vec<i32>, waiting: Option<WaitingLock> }, LockNode { session, lock: Option<WaitingLock>, also_blocked_by: Vec<i32>, victims: usize, children: Vec<LockNode> }, DdlQueueItem { session, waits_for: Option<i32>, wait_s: Option<f64>, queued: usize, relation: Option<String>, mode: String }, LocksData { roots: Vec<LockNode>, cycles: Vec<Vec<i32>>, ddl_queue: Vec<DdlQueueItem> }, DDL_MODES, build(&[LockRow]) -> LocksData}`;
  - `pg_advisor::rules::locks::evaluate(member: Option<&str>, d: &LocksData) -> Vec<Finding>`;
  - `diag::locks::{locks_sql(Caps) -> String, collect(&Client, Caps) -> AppResult<Vec<LockRow>>, report(st, id, target)}`; `DiagData::Locks(LocksData)`;
  - `common::wait_until(what: &str, limit: Duration, cond: impl AsyncFn…)` — опрос условия раз в 100 мс (в `tests/common`).

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/locktree.rs`, `tests`:

```rust
    use super::*;

    fn s(pid: i32) -> LockSession {
        LockSession { pid, backend_start_us: Some(pid as i64), ..Default::default() }
    }

    fn waiting(pid: i32, blocked_by: &[i32], mode: &str, wait_s: f64) -> LockRow {
        LockRow {
            session: s(pid),
            blocked_by: blocked_by.to_vec(),
            waiting: Some(WaitingLock { locktype: Some("relation".into()), mode: Some(mode.into()), relation: Some("shop.orders".into()), wait_s: Some(wait_s), ..Default::default() }),
        }
    }

    fn holder(pid: i32) -> LockRow {
        LockRow { session: s(pid), blocked_by: vec![], waiting: None }
    }

    #[test]
    fn ddl_in_the_queue_behind_a_long_transaction() {
        // 1 держит строку; 2 — ALTER, ждёт 1; 3 и 4 — SELECT, ждут 2.
        let rows = [holder(1), waiting(2, &[1], "AccessExclusiveLock", 40.0), waiting(3, &[2], "AccessShareLock", 30.0), waiting(4, &[2], "AccessShareLock", 5.0)];
        let d = build(&rows);
        assert_eq!(d.roots.len(), 1);
        let root = &d.roots[0];
        assert_eq!((root.session.pid, root.victims), (1, 3));
        assert_eq!(root.children.iter().map(|c| c.session.pid).collect::<Vec<_>>(), [2]);
        assert_eq!(root.children[0].children.iter().map(|c| c.session.pid).collect::<Vec<_>>(), [3, 4]);
        assert_eq!(d.ddl_queue.len(), 1);
        let q = &d.ddl_queue[0];
        assert_eq!((q.session.pid, q.waits_for, q.queued, q.mode.as_str()), (2, Some(1), 2, "AccessExclusiveLock"));
        assert!(d.cycles.is_empty());
    }

    #[test]
    fn several_blockers_go_to_the_first_and_the_rest_are_listed() {
        let rows = [holder(1), holder(2), waiting(3, &[2, 1], "RowExclusiveLock", 1.0)];
        let d = build(&rows);
        assert_eq!(d.roots.iter().map(|r| r.session.pid).collect::<Vec<_>>(), [2], "корень без жертв в лес не попадает");
        assert_eq!(d.roots[0].children[0].also_blocked_by, [1]);
        assert!(d.ddl_queue.is_empty(), "RowExclusiveLock — не DDL");
    }

    #[test]
    fn cycles_are_reported_apart_from_the_forest() {
        let rows = [waiting(5, &[6], "ShareLock", 1.0), waiting(6, &[5], "ShareLock", 1.0)];
        let d = build(&rows);
        assert!(d.roots.is_empty());
        assert_eq!(d.cycles, [vec![5, 6]]);
    }
```

`crates/pg-advisor/src/rules/locks.rs`, `tests`:

```rust
    #[test]
    fn root_blocker_and_ddl_queue_findings() {
        use crate::locktree::*;
        let rows = [
            LockRow { session: LockSession { pid: 1, backend_start_us: Some(1), ..Default::default() }, blocked_by: vec![], waiting: None },
            LockRow {
                session: LockSession { pid: 2, backend_start_us: Some(2), ..Default::default() },
                blocked_by: vec![1],
                waiting: Some(WaitingLock { locktype: Some("relation".into()), mode: Some("AccessExclusiveLock".into()), relation: Some("shop.orders".into()), wait_s: Some(12.0), ..Default::default() }),
            },
            LockRow {
                session: LockSession { pid: 3, backend_start_us: Some(3), ..Default::default() },
                blocked_by: vec![2],
                waiting: Some(WaitingLock { locktype: Some("relation".into()), mode: Some("AccessShareLock".into()), wait_s: Some(400.0), ..Default::default() }),
            },
        ];
        let f = evaluate(Some("pg-1"), &build(&rows));
        let got: Vec<(&str, Severity)> = f.iter().map(|x| (x.id.as_str(), x.severity)).collect();
        // Жертва в глубине цепочки (pid 3) ждёт 400 с — корень отвечает и за неё.
        assert_eq!(got, [("ddlQueue", Severity::Critical), ("lockWait", Severity::Critical), ("rootBlocker", Severity::Critical)]);
        let root = f.iter().find(|x| x.id == "rootBlocker").unwrap();
        assert_eq!(root.target, Some(FindingTarget::Session { pid: 1, backend_start_us: 1 }));
        assert_eq!((root.params["count"].as_u64(), root.params["maxS"].as_f64()), (Some(2), Some(400.0)));
        let ddl = f.iter().find(|x| x.id == "ddlQueue").unwrap();
        assert_eq!(ddl.params["relation"], "shop.orders");
    }
```

`src-tauri/tests/it_diag.rs`:

```rust
#[tokio::test]
async fn locks_sql_runs_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(&addr, "postgres", common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let h = diag::head(&c).await.unwrap();
        diag::locks::collect(&c, Caps::new(h.version_num)).await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
    }
}

#[tokio::test]
async fn ddl_queue_behind_a_long_transaction_is_found() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    common::ensure_fixtures(&addr).await;
    let holder = common::app(&addr).await;
    holder.batch_execute("BEGIN; UPDATE shop.orders SET status = status WHERE id = 1;").await.unwrap();
    let holder_pid: i32 = holder.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let ddl = common::superuser(&addr, "orders").await;
    let ddl_pid: i32 = ddl.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let ddl_task = tokio::spawn(async move {
        ddl.batch_execute("BEGIN; ALTER TABLE shop.orders ADD COLUMN it_tmp int; ROLLBACK;").await
    });
    let probe = common::superuser(&addr, "postgres").await;
    common::wait_until("ALTER ждёт", Duration::from_secs(5), async || {
        probe.query_one("SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock'", &[]).await.unwrap().get::<_, i64>(0) >= 1
    })
    .await;
    let victim = common::app(&addr).await;
    let victim_pid: i32 = victim.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let victim_task = tokio::spawn(async move { victim.query_one("SELECT count(*) FROM shop.orders", &[]).await.map(|_| ()) });
    common::wait_until("SELECT встал за ALTER", Duration::from_secs(5), async || {
        probe.query_one("SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock'", &[]).await.unwrap().get::<_, i64>(0) >= 2
    })
    .await;

    let (_dir, st) = open_server(&addr).await;
    let r = diag::run(&st, "it", req(Tool::Locks)).await.unwrap();
    let DiagData::Locks(d) = &r.data else { panic!("{:?}", r.data) };
    let root = d.roots.iter().find(|n| n.session.pid == holder_pid).expect("держатель — корень");
    assert_eq!(root.children.iter().map(|c| c.session.pid).collect::<Vec<_>>(), [ddl_pid]);
    assert_eq!(root.children[0].children.iter().map(|c| c.session.pid).collect::<Vec<_>>(), [victim_pid]);
    let q = d.ddl_queue.iter().find(|q| q.session.pid == ddl_pid).expect("DDL в очереди");
    assert_eq!((q.mode.as_str(), q.relation.as_deref(), q.queued), ("AccessExclusiveLock", Some("shop.orders"), 1));
    assert!(r.findings.iter().any(|f| f.id == "ddlQueue" && f.severity == pg_advisor::findings::Severity::Critical));

    holder.batch_execute("ROLLBACK").await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), ddl_task).await.unwrap().unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(10), victim_task).await.unwrap().unwrap().unwrap();
}
```

(В `src-tauri/Cargo.toml` `[dev-dependencies]` — `pg-advisor = { path = "../crates/pg-advisor" }` не нужен: крейт уже зависимость библиотеки, тесты обращаются к нему как `pg_advisor::…`.)

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml locktree rules::locks`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация `pg-advisor`**

`lib.rs`: `pub mod locktree;`; `rules/mod.rs`: `pub mod locks;`. `locktree.rs` — структуры из Interfaces (`Default`, `Serialize`, `Deserialize`, camelCase) и:

```rust
use std::collections::{BTreeSet, HashMap, HashSet};

/// Режимы, которые запрашивает DDL: ALTER/DROP/TRUNCATE/LOCK (AccessExclusive), CREATE TRIGGER и FK (ShareRowExclusive),
/// CREATE INDEX без CONCURRENTLY (Share).
pub const DDL_MODES: [&str; 3] = ["AccessExclusiveLock", "ShareRowExclusiveLock", "ShareLock"];

/// Лес «блокирующий → жертвы». Родитель жертвы — первый из её `pg_blocking_pids`, кто есть в строках; остальные —
/// в `also_blocked_by`. Корень — сессия, которая сама не ждёт и кого-то держит. Ждущие, от которых по цепочке
/// родителей не дойти до корня, — циклы (deadlock detector разрешит их за `deadlock_timeout`).
pub fn build(rows: &[LockRow]) -> LocksData {
    let index: HashMap<i32, &LockRow> = rows.iter().map(|r| (r.session.pid, r)).collect();
    let parent = |r: &LockRow| r.blocked_by.iter().copied().find(|p| *p != r.session.pid && index.contains_key(p));
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for r in rows {
        if let Some(p) = parent(r) {
            children.entry(p).or_default().push(r.session.pid);
        }
    }
    let mut visited = HashSet::new();
    fn grow(pid: i32, index: &HashMap<i32, &LockRow>, children: &HashMap<i32, Vec<i32>>, visited: &mut HashSet<i32>, parent_pid: Option<i32>) -> LockNode {
        visited.insert(pid);
        let r = index[&pid];
        let kids: Vec<LockNode> = children.get(&pid).into_iter().flatten()
            .filter(|c| !visited.contains(c))
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|c| grow(c, index, children, visited, Some(pid)))
            .collect();
        let victims = kids.iter().map(|k| 1 + k.victims).sum();
        LockNode {
            session: r.session.clone(),
            lock: r.waiting.clone(),
            also_blocked_by: r.blocked_by.iter().copied().filter(|p| Some(*p) != parent_pid && *p != pid).collect(),
            victims,
            children: kids,
        }
    }
    let mut roots: Vec<LockNode> = rows.iter()
        .filter(|r| parent(r).is_none() && r.waiting.is_none() && children.contains_key(&r.session.pid))
        .map(|r| r.session.pid)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|pid| grow(pid, &index, &children, &mut visited, None))
        .collect();
    roots.sort_by(|a, b| b.victims.cmp(&a.victims).then(a.session.pid.cmp(&b.session.pid)));
    // Циклы: ждущие, до которых лес не дошёл; идём по родителям до повтора.
    let mut cycles: BTreeSet<Vec<i32>> = BTreeSet::new();
    for r in rows.iter().filter(|r| r.waiting.is_some() && !visited.contains(&r.session.pid)) {
        let mut path = vec![r.session.pid];
        let mut cur = r;
        while let Some(p) = parent(cur) {
            if let Some(i) = path.iter().position(|x| *x == p) {
                let mut cycle = path[i..].to_vec();
                cycle.sort_unstable();
                cycles.insert(cycle);
                break;
            }
            path.push(p);
            cur = index[&p];
        }
    }
    let mut ddl_queue = Vec::new();
    fn find_ddl(n: &LockNode, parent: Option<i32>, out: &mut Vec<DdlQueueItem>) {
        if let Some(l) = &n.lock
            && l.locktype.as_deref() == Some("relation")
            && l.mode.as_deref().is_some_and(|m| DDL_MODES.contains(&m))
            && !n.children.is_empty()
        {
            out.push(DdlQueueItem { session: n.session.clone(), waits_for: parent, wait_s: l.wait_s, queued: n.victims, relation: l.relation.clone(), mode: l.mode.clone().unwrap_or_default() });
        }
        for c in &n.children {
            find_ddl(c, Some(n.session.pid), out);
        }
    }
    for r in &roots {
        find_ddl(r, None, &mut ddl_queue);
    }
    LocksData { roots, cycles: cycles.into_iter().collect(), ddl_queue }
}
```

`rules/locks.rs`:

```rust
use crate::findings::{Finding, FindingTarget, Severity, grade, thresholds as th};
use crate::locktree::{LockNode, LocksData};

fn session_target(n: &crate::locktree::LockSession) -> Option<FindingTarget> {
    n.backend_start_us.map(|us| FindingTarget::Session { pid: n.pid, backend_start_us: us })
}

fn max_wait(n: &LockNode) -> Option<f64> {
    n.children.iter().filter_map(|c| c.lock.as_ref().and_then(|l| l.wait_s)).chain(n.children.iter().filter_map(max_wait)).reduce(f64::max)
}

pub fn evaluate(member: Option<&str>, d: &LocksData) -> Vec<Finding> {
    let mut out = Vec::new();
    for root in &d.roots {
        let wait = max_wait(root).unwrap_or(0.0);
        let sev = if root.victims >= th::ROOT_BLOCKER_CRIT_VICTIMS { Some(Severity::Critical) } else { grade(wait, th::ROOT_BLOCKER_WAIT_S.0, th::ROOT_BLOCKER_WAIT_S.1) };
        if let Some(sev) = sev {
            let mut f = Finding::new("rootBlocker", sev).member(member).param("count", root.victims).param("maxS", wait).link_tool("sessions", member);
            if let Some(t) = session_target(&root.session) { f = f.target(t); }
            out.push(f);
        }
    }
    for q in &d.ddl_queue {
        let mut f = Finding::new("ddlQueue", Severity::Critical).member(member).param("count", q.queued).param("relation", q.relation.clone().unwrap_or_default());
        if let Some(w) = q.wait_s { f = f.param("maxS", w); }
        if let Some(t) = session_target(&q.session) { f = f.target(t); }
        out.push(f);
    }
    let waiting: Vec<f64> = d.roots.iter().flat_map(|r| all_waits(r)).collect();
    if let Some(max) = waiting.iter().copied().reduce(f64::max)
        && let Some(sev) = grade(max, th::LOCK_WAIT_S.0, th::LOCK_WAIT_S.1)
    {
        out.push(Finding::new("lockWait", sev).member(member).param("maxS", max).param("count", waiting.len()));
    }
    for c in &d.cycles {
        out.push(Finding::new("lockCycle", Severity::Info).member(member).param("count", c.len()));
    }
    crate::findings::sort(&mut out);
    out
}

fn all_waits(n: &LockNode) -> Vec<f64> {
    n.lock.as_ref().and_then(|l| l.wait_s).into_iter().chain(n.children.iter().flat_map(all_waits)).collect()
}
```

- [ ] **Step 4: Реализация сбора**

`src-tauri/src/diag/locks.rs`:

```rust
//! Блокировки и DDL в очереди (спец. P2 §5.4). `pg_blocking_pids` — только для ждущих блокировку.

pub fn locks_sql(caps: Caps) -> String {
    let wait = if caps.at_least(V14) {
        "extract(epoch FROM now() - l.waitstart)::float8"
    } else {
        "CASE WHEN l.pid IS NOT NULL THEN extract(epoch FROM now() - a.state_change)::float8 END"
    };
    format!(
        "WITH w AS (SELECT pid, pg_blocking_pids(pid) AS blocked_by FROM pg_stat_activity WHERE wait_event_type = 'Lock'), \
              inv AS (SELECT pid FROM w UNION SELECT unnest(blocked_by) FROM w) \
         SELECT a.pid, {BACKEND_START_US}, a.usename::text, a.application_name, a.datname::text, a.state, left(a.query, 16384), \
                extract(epoch FROM now() - a.xact_start)::float8, extract(epoch FROM now() - a.query_start)::float8, \
                COALESCE(w.blocked_by, '{{}}'::int[]), l.locktype, l.mode, \
                CASE WHEN l.database = (SELECT oid FROM pg_database WHERE datname = current_database()) THEN l.relation::regclass::text END, \
                l.relation::bigint, ld.datname::text, {wait} \
         FROM inv JOIN pg_stat_activity a ON a.pid = inv.pid \
         LEFT JOIN w ON w.pid = a.pid \
         LEFT JOIN LATERAL (SELECT * FROM pg_locks x WHERE x.pid = a.pid AND NOT x.granted LIMIT 1) l ON true \
         LEFT JOIN pg_database ld ON ld.oid = l.database \
         ORDER BY a.pid"
    )
}
```

`collect` превращает строки в `LockRow` (`waiting = Some(WaitingLock { …, approx: !caps.at_least(V14) })`, если `locktype` не `NULL`). `report` — как у сессий: `head` и `collect` в одном `on_member`, затем `locktree::build`, `rules::locks::evaluate`, `make_report(Tool::Locks, …)`. `diag/mod.rs`: `pub mod locks;`, вариант `Locks(LocksData)`, ветка `Tool::Locks`.

`src-tauri/tests/common/mod.rs`:

```rust
/// Ждёт, пока асинхронное условие станет истинным (опрос раз в 100 мс); по истечении `limit` — паника с `what`.
pub async fn wait_until(what: &str, limit: Duration, mut cond: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !cond().await {
        assert!(Instant::now() < deadline, "не дождались: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
```

- [ ] **Step 5: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib`, затем интеграционные (`--test it_diag`).
Expected: PASS; после теста на `pg-single` не осталось ждущих сессий (`SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock'` — 0).

- [ ] **Step 6: Commit**

```bash
git add crates/pg-advisor src-tauri
git commit -m "feat(diag): дерево блокировок, циклы и DDL в очереди

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Горизонт xmin и реплики

**Files:**
- Create: `crates/pg-advisor/src/rules/horizon.rs`, `crates/pg-advisor/src/rules/replication.rs`, `src-tauri/src/diag/horizon.rs`, `src-tauri/src/diag/replication.rs`
- Modify: `crates/pg-advisor/src/rules/mod.rs`, `src-tauri/src/diag/mod.rs` (варианты, ветки, помощник `sender_member`), `src-tauri/tests/common/mod.rs` (`run_sql_blocking`), `src-tauri/tests/it_diag.rs`
- Test: unit-тесты правил; интеграционные в `it_diag.rs`

**Interfaces:**
- Consumes: задачи 1, 3; `CounterMemory::swap` и `rates::delta` (рост конфликтов); `common::restore_on_drop`.
- Produces:
  - `pg_advisor::rules::horizon::{HolderKind { Session, PreparedXact, Slot, StandbyFeedback }, Holder { kind, name: String, member: Option<String>, xmin_age: i64, duration_s: Option<f64>, pid: Option<i32>, backend_start_us: Option<i64>, user: Option<String>, application: Option<String>, database: Option<String>, state: Option<String>, gid: Option<String>, slot_type: Option<String>, active: Option<bool>, catalog: bool, client_addr: Option<String> }, HorizonData { holders: Vec<Holder>, hot_standby_feedback: Option<bool> }, quote_literal(&str) -> String, evaluate(member: Option<&str>, d: &HorizonData) -> Vec<Finding>}` (`holders` отсортированы по `xmin_age` убыванию — делает сбор);
  - `pg_advisor::rules::replication::{Sender { pid, user, application, member, client_addr, state, sync_state: Option<String>, sync_priority: Option<i32>, sent_lag_bytes, write_lag_bytes, flush_lag_bytes, replay_lag_bytes: Option<i64>, write_lag_s, flush_lag_s, replay_lag_s, reply_age_s: Option<f64>, slot_name: Option<String>, xmin_age: Option<i64> }, Receiver { status, sender_host, slot_name: Option<String>, sender_port: Option<i32>, received_tli: Option<i64>, last_msg_age_s, latest_end_age_s: Option<f64> }, Replay { receive_replay_gap_bytes: Option<i64>, replay_delay_s: Option<f64>, pause: Option<String> }, Conflicts { tablespace, lock, snapshot, bufferpin, deadlock: i64, active_logicalslot: Option<i64>, growth: Option<i64> } (+ total()), ReplicationData { in_recovery, senders, synchronous_standby_names: Option<String>, synchronous_commit: Option<String>, receiver: Option<Receiver>, replay: Option<Replay>, conflicts: Option<Conflicts>, hot_standby_feedback: Option<bool>, max_standby_streaming_delay_ms: Option<i64> }, evaluate(member: Option<&str>, d: &ReplicationData) -> Vec<Finding>}`;
  - `diag::sender_member(t: &Topology, application: Option<&str>) -> Option<String>` (уточнение 5); `diag::horizon::{collect(&Client, Caps, &Topology) -> AppResult<HorizonData>, report}`; `diag::replication::{collect(&Client, Caps, in_recovery: bool, &Topology) -> AppResult<(ReplicationData, Option<CounterSnapshot>)>, report}`; `DiagData::{Horizon, Replication}`;
  - `common::run_sql_blocking(addr, db, sql)` — суперпользователь, свой поток и рантайм (для восстановления стенда из `Drop`).

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/rules/horizon.rs`, `tests`:

```rust
    use super::*;
    use crate::findings::{FindingLink, FindingTarget, Severity};

    fn holder(kind: HolderKind, name: &str, age: i64, dur: Option<f64>) -> Holder {
        Holder { kind, name: name.into(), xmin_age: age, duration_s: dur, ..Default::default() }
    }

    #[test]
    fn the_oldest_holder_is_the_horizon() {
        let mut s = holder(HolderKind::Session, "4242", 12_000_000, Some(100.0));
        s.pid = Some(4242);
        s.backend_start_us = Some(7);
        let d = HorizonData { holders: vec![s, holder(HolderKind::Slot, "dead", 10, None)], hot_standby_feedback: Some(false) };
        let f = evaluate(Some("pg-1"), &d);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].id.as_str(), f[0].severity), ("xminHorizon", Severity::Warning));
        assert_eq!(f[0].target, Some(FindingTarget::Session { pid: 4242, backend_start_us: 7 }));
        assert_eq!(f[0].params["kind"], "session");
        assert_eq!(f[0].links, [FindingLink::OpenTool { tool: "sessions".into(), member: Some("pg-1".into()) }]);
    }

    #[test]
    fn duration_alone_can_raise_and_prepared_gets_a_command() {
        let mut p = holder(HolderKind::PreparedXact, "it's", 5, Some(30_000.0));
        p.gid = Some("it's".into());
        let f = evaluate(None, &HorizonData { holders: vec![p], hot_standby_feedback: None });
        assert_eq!(f[0].severity, Severity::Critical);
        assert_eq!(f[0].params["command"], "ROLLBACK PREPARED 'it''s';");
        let mut fb = holder(HolderKind::StandbyFeedback, "walreceiver", 200_000_000, None);
        fb.member = Some("pg-2".into());
        let f = evaluate(Some("pg-1"), &HorizonData { holders: vec![fb], hot_standby_feedback: None });
        assert_eq!(f[0].target, Some(FindingTarget::Member { name: "pg-2".into() }));
        assert_eq!(f[0].links, [FindingLink::OpenTool { tool: "horizon".into(), member: Some("pg-2".into()) }]);
        assert!(evaluate(None, &HorizonData::default()).is_empty());
    }
```

`crates/pg-advisor/src/rules/replication.rs`, `tests`:

```rust
    use super::*;
    use crate::findings::Severity;

    fn sender(app: &str, state: &str, sync: &str, replay_bytes: i64) -> Sender {
        Sender { pid: 1, application: Some(app.into()), member: Some(app.into()), state: Some(state.into()), sync_state: Some(sync.into()), replay_lag_bytes: Some(replay_bytes), replay_lag_s: Some(0.0), reply_age_s: Some(1.0), ..Default::default() }
    }

    fn ids(f: &[Finding]) -> Vec<(&str, Severity)> {
        f.iter().map(|x| (x.id.as_str(), x.severity)).collect()
    }

    #[test]
    fn primary_side_findings() {
        let d = ReplicationData {
            senders: vec![sender("pg-2", "streaming", "async", 20 * 1024 * 1024), sender("pg-3", "catchup", "async", 0)],
            synchronous_standby_names: Some("ANY 1 (pg-2, pg-3)".into()),
            ..Default::default()
        };
        assert_eq!(ids(&evaluate(Some("pg-1"), &d)), [("syncStandbyMissing", Severity::Critical), ("replicaLag", Severity::Warning), ("replicaNotStreaming", Severity::Warning)]);
        let ok = ReplicationData { senders: vec![sender("pg-2", "streaming", "quorum", 0)], synchronous_standby_names: Some("ANY 1 (pg-2)".into()), ..Default::default() };
        assert!(evaluate(None, &ok).is_empty());
    }

    #[test]
    fn replica_side_findings() {
        let d = ReplicationData {
            in_recovery: true,
            receiver: Some(Receiver { status: Some("stopping".into()), ..Default::default() }),
            replay: Some(Replay { receive_replay_gap_bytes: Some(0), replay_delay_s: Some(40.0), pause: Some("paused".into()) }),
            conflicts: Some(Conflicts { snapshot: 3, growth: Some(2), ..Default::default() }),
            ..Default::default()
        };
        assert_eq!(ids(&evaluate(Some("pg-2"), &d)), [("walReceiverDown", Severity::Critical), ("recoveryConflicts", Severity::Warning), ("replayPaused", Severity::Warning), ("replicaLag", Severity::Warning)]);
        let calm = ReplicationData { in_recovery: true, receiver: Some(Receiver { status: Some("streaming".into()), ..Default::default() }), conflicts: Some(Conflicts { lock: 1, growth: Some(0), ..Default::default() }), ..Default::default() };
        assert_eq!(ids(&evaluate(None, &calm)), [("recoveryConflictsTotal", Severity::Info)]);
    }
```

`src-tauri/tests/it_diag.rs`:

```rust
#[tokio::test]
async fn horizon_and_replication_sql_run_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(&addr, "postgres", common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let h = diag::head(&c).await.unwrap();
        let caps = Caps::new(h.version_num);
        let empty = glafira_pg_lib::topology::model::Topology::empty_for_tests();
        diag::horizon::collect(&c, caps, &empty).await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        diag::replication::collect(&c, caps, h.in_recovery, &empty).await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
    }
}

#[tokio::test]
async fn paused_replay_shows_on_both_sides() {
    let (Some(primary), Some(replica)) = (common::env("GPG_IT_PRIMARY"), common::env("GPG_IT_REPLICA")) else { return };
    let _resume = common::restore_on_drop({
        let replica = replica.clone();
        move || common::run_sql_blocking(&replica, "postgres", "SELECT pg_wal_replay_resume()")
    });
    common::superuser(&replica, "postgres").await.batch_execute("SELECT pg_wal_replay_pause()").await.unwrap();
    let w = common::superuser(&primary, "postgres").await;
    w.batch_execute("CREATE TABLE IF NOT EXISTS it_lag (x int); INSERT INTO it_lag SELECT generate_series(1, 200000);").await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let (_d1, st) = open_server(&primary).await;
    let r = diag::run(&st, "it", req(Tool::Replication)).await.unwrap();
    let DiagData::Replication(d) = r.data else { panic!() };
    assert!(!d.in_recovery);
    assert!(d.senders.iter().any(|s| s.replay_lag_bytes.unwrap_or(0) > 0), "{:?}", d.senders);

    let (_d2, st) = open_server(&replica).await;
    let r = diag::run(&st, "it", req(Tool::Replication)).await.unwrap();
    let DiagData::Replication(d) = &r.data else { panic!() };
    assert!(d.in_recovery);
    assert_eq!(d.receiver.as_ref().and_then(|x| x.status.as_deref()), Some("streaming"));
    assert!(matches!(d.replay.as_ref().and_then(|x| x.pause.as_deref()), Some("paused" | "pause requested")));
    assert!(r.findings.iter().any(|f| f.id == "replayPaused"));
    w.batch_execute("DROP TABLE it_lag").await.unwrap();
}

#[tokio::test]
async fn prepared_transaction_and_logical_slot_hold_the_lab_horizon() {
    let Some(lab) = common::lab_addr() else { return };
    let _cleanup = common::restore_on_drop({
        let lab = lab.clone();
        move || common::run_sql_blocking(&lab, "orders", "DO $$ BEGIN IF EXISTS (SELECT 1 FROM pg_prepared_xacts WHERE gid = 'it_horizon') THEN ROLLBACK PREPARED 'it_horizon'; END IF; END $$")
    });
    let c = common::superuser(&lab, "orders").await;
    c.batch_execute("BEGIN; UPDATE shop.customers SET email = email WHERE id = 1; PREPARE TRANSACTION 'it_horizon';").await.unwrap();
    let (_dir, st) = open_server(&lab).await;
    let r = diag::run(&st, "it", req(Tool::Horizon)).await.unwrap();
    let DiagData::Horizon(d) = r.data else { panic!() };
    assert!(d.holders.iter().any(|h| h.kind == HolderKind::PreparedXact && h.gid.as_deref() == Some("it_horizon")));
    assert!(d.holders.iter().any(|h| h.kind == HolderKind::Slot && h.name == "lab_sub_slot" && h.catalog));
    assert!(d.holders.windows(2).all(|w| w[0].xmin_age >= w[1].xmin_age), "по возрасту");
}

#[tokio::test]
async fn standby_feedback_is_a_holder_on_the_primary() {
    let (Some(primary), Some(replica)) = (common::env("GPG_IT_PRIMARY"), common::env("GPG_IT_REPLICA")) else { return };
    common::ensure_fixtures(&primary).await;
    let _reset = common::restore_on_drop({
        let replica = replica.clone();
        move || {
            common::run_sql_blocking(&replica, "postgres", "ALTER SYSTEM RESET hot_standby_feedback");
            common::run_sql_blocking(&replica, "postgres", "SELECT pg_reload_conf()");
        }
    });
    let su = common::superuser(&replica, "postgres").await;
    su.batch_execute("ALTER SYSTEM SET hot_standby_feedback = on").await.unwrap();
    su.batch_execute("SELECT pg_reload_conf()").await.unwrap();
    let reader = common::app(&replica).await;
    reader.batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) FROM shop.orders;").await.unwrap();
    let probe = common::superuser(&primary, "postgres").await;
    common::wait_until("feedback дошёл до primary", Duration::from_secs(15), async || {
        probe.query_one("SELECT count(*) FROM pg_stat_replication WHERE backend_xmin IS NOT NULL", &[]).await.unwrap().get::<_, i64>(0) >= 1
    })
    .await;
    let (_dir, st) = open_server(&primary).await;
    let DiagData::Horizon(d) = diag::run(&st, "it", req(Tool::Horizon)).await.unwrap().data else { panic!() };
    assert!(d.holders.iter().any(|h| h.kind == HolderKind::StandbyFeedback), "{:?}", d.holders);
    reader.batch_execute("ROLLBACK").await.unwrap();
}
```

(`Topology::empty_for_tests()` — новый `pub fn` в `topology/model.rs` под `#[doc(hidden)]`: топология без участников для проверки SQL; `HolderKind` импортируется из `pg_advisor::rules::horizon`.)

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml rules::`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация правил**

`rules/mod.rs`: `pub mod horizon; pub mod replication;`.

`rules/horizon.rs` (структуры — `Default`, `Serialize`, `Deserialize`, camelCase; `HolderKind` — `#[default] Session`):

```rust
/// Строковая константа SQL: кавычки удваиваются (`standard_conforming_strings = on`).
pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn kind_name(k: HolderKind) -> &'static str {
    match k {
        HolderKind::Session => "session",
        HolderKind::PreparedXact => "preparedXact",
        HolderKind::Slot => "slot",
        HolderKind::StandbyFeedback => "standbyFeedback",
    }
}

/// Горизонт — самый старый держатель; одна находка (остальные видны в таблице).
pub fn evaluate(member: Option<&str>, d: &HorizonData) -> Vec<Finding> {
    let Some(h) = d.holders.first() else { return vec![] };
    let by_age = grade(h.xmin_age as f64, th::XMIN_AGE.0, th::XMIN_AGE.1);
    let by_time = h.duration_s.and_then(|s| grade(s, th::XMIN_DURATION_S.0, th::XMIN_DURATION_S.1));
    let Some(sev) = worse(by_age, by_time) else { return vec![] };
    let mut f = Finding::new("xminHorizon", sev).member(member).param("kind", kind_name(h.kind)).param("name", h.name.clone()).param("xminAge", h.xmin_age);
    if let Some(s) = h.duration_s {
        f = f.param("maxS", s);
    }
    f = match h.kind {
        HolderKind::Session => {
            if let (Some(pid), Some(us)) = (h.pid, h.backend_start_us) {
                f = f.target(FindingTarget::Session { pid, backend_start_us: us });
            }
            f.link_tool("sessions", member)
        }
        HolderKind::Slot => f.target(FindingTarget::Slot { name: h.name.clone() }).link_tool("slots", member),
        HolderKind::PreparedXact => f.param("command", format!("ROLLBACK PREPARED {};", quote_literal(h.gid.as_deref().unwrap_or(&h.name)))),
        HolderKind::StandbyFeedback => match &h.member {
            Some(m) => f.target(FindingTarget::Member { name: m.clone() }).link_tool("horizon", Some(m)),
            None => f,
        },
    };
    vec![f]
}
```

`rules/replication.rs` (структуры — `Default`, `Serialize`, `Deserialize`, camelCase; `Conflicts::total` — сумма пяти счётчиков и `active_logicalslot`):

```rust
pub fn evaluate(member: Option<&str>, d: &ReplicationData) -> Vec<Finding> {
    let mut out = Vec::new();
    let lag_sev = |bytes: Option<i64>, secs: Option<f64>| {
        worse(bytes.and_then(|b| grade(b as f64, th::REPLICA_LAG_BYTES.0, th::REPLICA_LAG_BYTES.1)), secs.and_then(|s| grade(s, th::REPLICA_LAG_S.0, th::REPLICA_LAG_S.1)))
    };
    // Отправители: одна находка на правило, цель — худшая реплика.
    let lagging: Vec<(&Sender, Severity)> = d.senders.iter().filter_map(|s| lag_sev(s.replay_lag_bytes, s.replay_lag_s).map(|v| (s, v))).collect();
    if let Some((s, sev)) = lagging.iter().max_by_key(|(s, v)| (*v, s.replay_lag_bytes.unwrap_or(0))) {
        let mut f = Finding::new("replicaLag", *sev).member(member).param("count", lagging.len());
        if let Some(b) = s.replay_lag_bytes { f = f.param("lagBytes", b); }
        if let Some(t) = s.replay_lag_s { f = f.param("delayS", t); }
        if let Some(m) = &s.member { f = f.target(FindingTarget::Member { name: m.clone() }).link_tool("replication", Some(m)); }
        out.push(f);
    }
    let not_streaming: Vec<&Sender> = d.senders.iter().filter(|s| s.state.as_deref() != Some("streaming")).collect();
    if let Some(s) = not_streaming.first() {
        let mut f = Finding::new("replicaNotStreaming", Severity::Warning).member(member).param("count", not_streaming.len()).param("state", s.state.clone().unwrap_or_default());
        if let Some(m) = &s.member { f = f.target(FindingTarget::Member { name: m.clone() }); }
        out.push(f);
    }
    let stale: Vec<&Sender> = d.senders.iter().filter(|s| s.reply_age_s.is_some_and(|a| a >= th::REPLY_STALE_S)).collect();
    if let Some(s) = stale.first() {
        out.push(Finding::new("replicaReplyStale", Severity::Warning).member(member).param("count", stale.len()).param("maxS", s.reply_age_s.unwrap_or(0.0)));
    }
    if !d.in_recovery
        && d.synchronous_standby_names.as_deref().is_some_and(|n| !n.trim().is_empty())
        && !d.senders.iter().any(|s| matches!(s.sync_state.as_deref(), Some("sync" | "quorum")))
    {
        out.push(Finding::new("syncStandbyMissing", Severity::Critical).member(member));
    }
    if d.in_recovery {
        if d.receiver.as_ref().and_then(|r| r.status.as_deref()) != Some("streaming") {
            out.push(Finding::new("walReceiverDown", Severity::Critical).member(member));
        }
        if let Some(r) = &d.replay {
            if matches!(r.pause.as_deref(), Some("paused" | "pause requested")) {
                out.push(Finding::new("replayPaused", Severity::Warning).member(member));
            }
            if let Some(sev) = lag_sev(r.receive_replay_gap_bytes, r.replay_delay_s) {
                let mut f = Finding::new("replicaLag", sev).member(member);
                if let Some(b) = r.receive_replay_gap_bytes { f = f.param("lagBytes", b); }
                if let Some(t) = r.replay_delay_s { f = f.param("delayS", t); }
                out.push(f);
            }
        }
        if let Some(c) = &d.conflicts {
            if c.growth.is_some_and(|g| g > 0) {
                out.push(Finding::new("recoveryConflicts", Severity::Warning).member(member).param("count", c.growth.unwrap_or(0)));
            } else if c.total() > 0 {
                out.push(Finding::new("recoveryConflictsTotal", Severity::Info).member(member).param("count", c.total()));
            }
        }
    }
    crate::findings::sort(&mut out);
    out
}
```

- [ ] **Step 4: Реализация сбора**

`src-tauri/src/diag/mod.rs`:

```rust
/// Участник топологии для `application_name` отправителя WAL (уточнение 5 плана P2a): имя участника или его хост.
pub fn sender_member(t: &Topology, application: Option<&str>) -> Option<String> {
    let app = application?;
    t.members.iter().find(|m| m.name == app || m.host == app).map(|m| m.name.clone())
}
```

`src-tauri/src/diag/horizon.rs` — четыре запроса и настройка:

```rust
pub fn sessions_sql() -> String {
    format!(
        "SELECT a.pid, {BACKEND_START_US}, a.usename::text, a.application_name, a.datname::text, a.state, \
                GREATEST(age(a.backend_xmin), age(a.backend_xid))::bigint, extract(epoch FROM now() - a.xact_start)::float8 \
         FROM pg_stat_activity a \
         WHERE (a.backend_xmin IS NOT NULL OR a.backend_xid IS NOT NULL) AND a.pid <> pg_backend_pid() \
           AND a.backend_type IS DISTINCT FROM 'walsender'"
    )
}
pub const PREPARED_SQL: &str = "SELECT gid, owner::text, database::text, age(transaction)::bigint, \
    extract(epoch FROM now() - prepared)::float8 FROM pg_prepared_xacts";
pub fn slots_sql(caps: Caps) -> String {
    let inactive = caps.col(V17, "extract(epoch FROM now() - inactive_since)::float8", "float8");
    format!("SELECT slot_name::text, slot_type, active, age(xmin)::bigint, age(catalog_xmin)::bigint, {inactive} \
             FROM pg_replication_slots WHERE xmin IS NOT NULL OR catalog_xmin IS NOT NULL")
}
pub const FEEDBACK_SQL: &str = "SELECT application_name, host(client_addr), age(backend_xmin)::bigint \
    FROM pg_stat_replication WHERE backend_xmin IS NOT NULL";
pub const FEEDBACK_SETTING_SQL: &str = "SELECT setting = 'on' FROM pg_settings WHERE name = 'hot_standby_feedback'";
```

Сбор: сессия — `name = pid.to_string()`; слот — `xmin_age = max(age(xmin), age(catalog_xmin))`, `catalog = age(catalog_xmin) > age(xmin)` (или `xmin` пуст), `duration_s` — `inactive_since`; prepared — `name = gid`; feedback — `name = application_name`, `member = sender_member(topology, application_name)`. Затем `holders.sort_by(|a, b| b.xmin_age.cmp(&a.xmin_age))`. `report`: `head`, `collect`, `rules::horizon::evaluate`, `unavailable` — `caps.missing(V17, "slotInactiveSince")`.

`src-tauri/src/diag/replication.rs`:

```rust
pub const SENDERS_SQL: &str = "SELECT r.pid, r.usename::text, r.application_name, host(r.client_addr), r.state, r.sync_state, \
    r.sync_priority, pg_wal_lsn_diff(c.lsn, r.sent_lsn)::bigint, pg_wal_lsn_diff(c.lsn, r.write_lsn)::bigint, \
    pg_wal_lsn_diff(c.lsn, r.flush_lsn)::bigint, pg_wal_lsn_diff(c.lsn, r.replay_lsn)::bigint, \
    extract(epoch FROM r.write_lag)::float8, extract(epoch FROM r.flush_lag)::float8, extract(epoch FROM r.replay_lag)::float8, \
    extract(epoch FROM now() - r.reply_time)::float8, \
    (SELECT s.slot_name::text FROM pg_replication_slots s WHERE s.active_pid = r.pid LIMIT 1), age(r.backend_xmin)::bigint \
  FROM pg_stat_replication r, \
    LATERAL (SELECT CASE WHEN pg_is_in_recovery() THEN pg_last_wal_receive_lsn() ELSE pg_current_wal_lsn() END AS lsn) c \
  ORDER BY r.application_name";
pub const RECEIVER_SQL: &str = "SELECT status, sender_host, sender_port, slot_name, received_tli::bigint, \
    extract(epoch FROM now() - last_msg_receipt_time)::float8, extract(epoch FROM now() - latest_end_time)::float8 \
  FROM pg_stat_wal_receiver";
pub fn replay_sql(caps: Caps) -> String {
    let pause = if caps.at_least(V14) { "pg_get_wal_replay_pause_state()" } else { "CASE WHEN pg_is_wal_replay_paused() THEN 'paused' ELSE 'not paused' END" };
    format!("SELECT pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())::bigint, \
             (CASE WHEN pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn() THEN 0 \
                   ELSE GREATEST(extract(epoch FROM now() - pg_last_xact_replay_timestamp()), 0) END)::float8, {pause}")
}
pub fn conflicts_sql(caps: Caps) -> String {
    let logical = caps.col(V16, "COALESCE(sum(confl_active_logicalslot), 0)::bigint", "bigint");
    format!("SELECT COALESCE(sum(confl_tablespace), 0)::bigint, COALESCE(sum(confl_lock), 0)::bigint, \
             COALESCE(sum(confl_snapshot), 0)::bigint, COALESCE(sum(confl_bufferpin), 0)::bigint, \
             COALESCE(sum(confl_deadlock), 0)::bigint, {logical}, \
             (extract(epoch FROM now()) * 1000)::bigint, (extract(epoch FROM pg_postmaster_start_time()) * 1000)::bigint, \
             (SELECT max((extract(epoch FROM stats_reset) * 1000)::bigint) FROM pg_stat_database) \
             FROM pg_stat_database_conflicts")
}
pub const SETTINGS_SQL: &str = "SELECT name::text, setting FROM pg_settings WHERE name IN \
    ('synchronous_standby_names', 'synchronous_commit', 'hot_standby_feedback', 'max_standby_streaming_delay')";
```

`collect(c, caps, in_recovery, topology)`: отправители всегда (каскадная реплика тоже отправляет), `member = sender_member(...)`; на узле в recovery — приёмник (нет строки — `Receiver { status: None, .. }`), replay и конфликты; снимок конфликтов `CounterSnapshot { values: {"conflicts": total} }`. `report`: `prev = st.counters.swap(id, generation, member, Tool::Replication, snap)`, `conflicts.growth = rates::delta(prev.as_ref(), &snap, "conflicts")`, затем правила; `max_standby_streaming_delay` в `pg_settings` — миллисекунды. `unavailable` — `caps.missing(V14, "replayPauseState")`, `caps.missing(V16, "conflictActiveLogicalSlot")` на узле в recovery.

`diag/mod.rs`: `pub mod horizon; pub mod replication;`, варианты `Horizon(HorizonData)`, `Replication(ReplicationData)`, ветки `run`.

`src-tauri/src/topology/model.rs`:

```rust
/// Топология без участников — для проверки SQL инструментов в интеграционных тестах.
#[doc(hidden)]
pub fn empty_for_tests() -> Topology { … }   // как `topo(vec![])`, но `pub` и не под `cfg(test)`
```

(метод `impl Topology { pub fn empty_for_tests() -> Self }`).

`src-tauri/tests/common/mod.rs`:

```rust
/// SQL от суперпользователя в своём потоке и рантайме: для восстановления стенда из `Drop`, где `await` нельзя.
pub fn run_sql_blocking(addr: &str, db: &str, sql: &str) {
    let (addr, db, sql) = (addr.to_string(), db.to_string(), sql.to_string());
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async { superuser(&addr, &db).await.batch_execute(&sql).await.unwrap() });
    })
    .join()
    .expect("восстановление стенда");
}
```

- [ ] **Step 5: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib`, затем интеграционные (`--test it_diag`).
Expected: PASS; после тестов `pg-replica` не на паузе (`SELECT pg_is_wal_replay_paused()` — `f`), `hot_standby_feedback` — `off`, в `pg-lab` нет prepared transactions.

- [ ] **Step 6: Commit**

```bash
git add crates/pg-advisor src-tauri
git commit -m "feat(diag): горизонт xmin и реплики — держатели, лаги, приёмник WAL, конфликты recovery

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 7: Patroni — запланированные действия, `/config`, готовность к failover, матрица health

**Files:**
- Create: `crates/pg-advisor/src/failover.rs`, `crates/pg-advisor/src/rules/patroni.rs`, `src-tauri/src/diag/patroni.rs`
- Modify: `crates/pg-advisor/src/lib.rs`, `crates/pg-advisor/src/rules/mod.rs`, `src-tauri/src/topology/model.rs`, `src-tauri/src/topology/patroni.rs`, `src-tauri/src/topology/probe.rs` (литерал `Topology`), `src-tauri/src/diag/mod.rs`, `src-tauri/tests/it_diag.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `failover.rs`, `rules/patroni.rs`, `topology/patroni.rs`, `topology/model.rs`, `diag/patroni.rs`; интеграционные в `it_diag.rs`

**Interfaces:**
- Consumes: `Finding`, `Severity` (1); `DiagTarget`, `make_report`, `AppState::live` (3); `cluster_bases`, `api_base`, `PatroniHttp`, `ReqwestHttp` (P1); `common::{open_patroni, patronictl, restore_on_drop, wait_stand, env}`.
- Produces:
  - `topology::model::{ScheduledSwitchover { at: String, from: Option<String>, to: Option<String> }, ScheduledRestart { at: String, postgres_version: Option<String> }}`; поля `Topology.scheduled_switchover: Option<ScheduledSwitchover>`, `Member.scheduled_restart: Option<ScheduledRestart>` (JSON `scheduledSwitchover`, `scheduledRestart`); `same_except_time` сравнивает и их;
  - `topology::patroni::{PatroniHttp::get_status(&self, url) -> BoxFuture<AppResult<u16>>, fetch_config(http: &dyn PatroniHttp, bases: &[String]) -> AppResult<serde_json::Value>}`;
  - `pg_advisor::failover::{SyncMode { Off, On, Quorum }, DynConfig { maximum_lag_on_failover: i64, synchronous_mode, synchronous_mode_strict: bool, synchronous_node_count: Option<i64>, ttl, loop_wait, retry_timeout: Option<i64>, permanent_slots: Vec<String> }, DEFAULT_MAXIMUM_LAG_ON_FAILOVER = 1_048_576, CandidateRole { Leader, Sync, Quorum, Replica, Unknown }, CandidateFacts { name, role, state: Option<String>, reachable, lag_bytes: Option<i64>, nofailover: bool, failover_priority: Option<i64>, pending_restart: bool }, Reason { Unreachable, NotStreaming, LagTooHigh, LagUnknown, NoFailoverTag, ZeroPriority, NotSynchronous }, Candidate { member, ready, reasons, lag_bytes, pending_restart }, FailoverData { paused, config, candidates, ready_count }, evaluate(&[CandidateFacts], &DynConfig, paused: bool) -> (FailoverData, Vec<Finding>)}`;
  - `pg_advisor::rules::patroni::{HEALTH_ENDPOINTS: [&str; 7], HealthFacts { member, role: CandidateRole, codes: BTreeMap<String, Option<u16>> }, evaluate_health(&[HealthFacts]) -> Vec<Finding>}`;
  - `diag::patroni::{parse_dyn_config(&Value) -> DynConfig, candidate_facts(&Topology) -> Vec<CandidateFacts>, dyn_config(st, target) -> AppResult<DynConfig>, HealthCell { endpoint, status: Option<u16>, error: Option<AppError> }, HealthMember { member, api_url: Option<String>, cells }, HealthData { endpoints: Vec<String>, members }, failover_report, health_report}`; `DiagData::{Failover(FailoverData), Health(HealthData)}`.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/failover.rs`, `tests`:

```rust
    use super::*;
    use crate::findings::Severity;

    fn m(name: &str, role: CandidateRole, lag: Option<i64>) -> CandidateFacts {
        CandidateFacts { name: name.into(), role, state: Some(if role == CandidateRole::Leader { "running" } else { "streaming" }.into()), reachable: true, lag_bytes: lag, ..Default::default() }
    }

    #[test]
    fn verdicts_and_reasons() {
        let mut far = m("c", CandidateRole::Replica, Some(5_000_000));
        far.nofailover = true;
        let mut down = m("d", CandidateRole::Replica, None);
        down.reachable = false;
        let mut zero = m("e", CandidateRole::Replica, Some(0));
        zero.failover_priority = Some(0);
        let cfg = DynConfig { maximum_lag_on_failover: DEFAULT_MAXIMUM_LAG_ON_FAILOVER, ..Default::default() };
        let (d, f) = evaluate(&[m("a", CandidateRole::Leader, None), m("b", CandidateRole::Replica, Some(0)), far, down, zero], &cfg, false);
        let r: Vec<(&str, bool, Vec<Reason>)> = d.candidates.iter().map(|c| (c.member.as_str(), c.ready, c.reasons.clone())).collect();
        assert_eq!(r, [
            ("b", true, vec![]),
            ("c", false, vec![Reason::LagTooHigh, Reason::NoFailoverTag]),
            ("d", false, vec![Reason::Unreachable]),
            ("e", false, vec![Reason::ZeroPriority]),
        ]);
        assert_eq!(d.ready_count, 1);
        assert_eq!(f.iter().map(|x| (x.id.as_str(), x.severity)).collect::<Vec<_>>(), [("failoverCandidates", Severity::Warning)]);
    }

    #[test]
    fn synchronous_mode_and_pause_and_timing() {
        let cfg = DynConfig { maximum_lag_on_failover: DEFAULT_MAXIMUM_LAG_ON_FAILOVER, synchronous_mode: SyncMode::On, ttl: Some(20), loop_wait: Some(10), retry_timeout: Some(10), ..Default::default() };
        let (d, f) = evaluate(&[m("a", CandidateRole::Leader, None), m("b", CandidateRole::Replica, Some(0))], &cfg, true);
        assert_eq!(d.candidates[0].reasons, [Reason::NotSynchronous]);
        let ids: Vec<(&str, Severity)> = f.iter().map(|x| (x.id.as_str(), x.severity)).collect();
        assert_eq!(ids, [("failoverCandidates", Severity::Critical), ("syncModeWithoutSync", Severity::Critical), ("clusterPaused", Severity::Warning), ("patroniTiming", Severity::Warning)]);
        let quorum = DynConfig { synchronous_mode: SyncMode::Quorum, ..cfg.clone() };
        let (d, _) = evaluate(&[m("a", CandidateRole::Leader, None), m("b", CandidateRole::Quorum, Some(0))], &quorum, false);
        assert!(d.candidates[0].ready);
    }
```

`crates/pg-advisor/src/rules/patroni.rs`, `tests`:

```rust
    use super::*;
    use crate::failover::CandidateRole;
    use crate::findings::Severity;

    fn h(member: &str, role: CandidateRole, codes: &[(&str, Option<u16>)]) -> HealthFacts {
        HealthFacts { member: member.into(), role, codes: codes.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    #[test]
    fn health_matrix_findings() {
        let ok = [
            h("a", CandidateRole::Leader, &[("primary", Some(200)), ("replica", Some(503)), ("health", Some(200))]),
            h("b", CandidateRole::Replica, &[("primary", Some(503)), ("replica", Some(200)), ("health", Some(200))]),
        ];
        assert!(evaluate_health(&ok).is_empty());
        let bad = [
            h("a", CandidateRole::Leader, &[("primary", Some(200)), ("health", Some(200))]),
            h("b", CandidateRole::Replica, &[("primary", Some(200)), ("replica", Some(503)), ("health", Some(503))]),
            h("c", CandidateRole::Replica, &[("primary", None), ("replica", None), ("health", None)]),
        ];
        let f = evaluate_health(&bad);
        let got: Vec<(&str, Severity, Option<&str>)> = f.iter().map(|x| (x.id.as_str(), x.severity, x.member.as_deref())).collect();
        assert_eq!(got, [
            ("healthPrimary", Severity::Critical, None),
            ("healthUnhealthy", Severity::Critical, Some("b")),
            ("replicaOutOfPool", Severity::Warning, Some("b")),
        ]);
    }
```

`src-tauri/src/topology/patroni.rs`, `tests` (рядом с тестами разбора `/cluster`):

```rust
    #[tokio::test]
    async fn scheduled_switchover_and_restart_are_parsed() {
        let http = FakeHttp::with(&[(
            "http://a:8008/cluster",
            serde_json::json!({
                "members": [
                    {"name": "a", "role": "leader", "state": "running", "host": "a", "port": 5432, "api_url": "http://a:8008/patroni", "timeline": 3},
                    {"name": "b", "role": "replica", "state": "streaming", "host": "b", "port": 5432, "api_url": "http://b:8008/patroni",
                     "scheduled_restart": {"schedule": "2026-10-25T03:00:00+00:00", "postgres_version": "17.2"}}
                ],
                "scheduled_switchover": {"at": "2026-10-25T02:00:00+00:00", "from": "a", "to": "b"}
            }),
        )]);
        let t = discover(&http, &settings(&["http://a:8008"]), "c", None, 1).await.unwrap();
        assert_eq!(t.scheduled_switchover, Some(ScheduledSwitchover { at: "2026-10-25T02:00:00+00:00".into(), from: Some("a".into()), to: Some("b".into()) }));
        assert_eq!(t.member("b").unwrap().scheduled_restart.as_ref().map(|r| r.at.as_str()), Some("2026-10-25T03:00:00+00:00"));
        assert_eq!(t.member("a").unwrap().scheduled_restart, None);
    }
```

(Имена `FakeHttp::with` и `settings` — те, что уже используют соседние тесты разбора `/cluster` в этом модуле; если конструктор поддельного HTTP называется иначе — использовать его.)

`src-tauri/src/topology/model.rs`, `same_except_time_ignores_observation_moment` — добавить:

```rust
        let mut c = a.clone();
        c.scheduled_switchover = Some(ScheduledSwitchover { at: "x".into(), from: None, to: None });
        assert!(!a.same_except_time(&c), "запланированный switchover — изменение топологии");
```

`src-tauri/src/diag/patroni.rs`, `tests`:

```rust
    #[test]
    fn dynamic_configuration_is_read_leniently() {
        let c = parse_dyn_config(&serde_json::json!({
            "ttl": 30, "loop_wait": 10, "retry_timeout": 10, "synchronous_mode": "quorum", "synchronous_node_count": 1,
            "slots": {"debezium": {"type": "logical", "database": "orders", "plugin": "pgoutput"}}
        }));
        assert_eq!((c.synchronous_mode, c.maximum_lag_on_failover, c.ttl), (SyncMode::Quorum, DEFAULT_MAXIMUM_LAG_ON_FAILOVER, Some(30)));
        assert_eq!(c.permanent_slots, ["debezium"]);
        assert_eq!(parse_dyn_config(&serde_json::json!({"synchronous_mode": true, "maximum_lag_on_failover": 5})).synchronous_mode, SyncMode::On);
        assert_eq!(parse_dyn_config(&serde_json::json!({"synchronous_mode": "off"})).synchronous_mode, SyncMode::Off);
        assert_eq!(parse_dyn_config(&serde_json::json!({})).maximum_lag_on_failover, DEFAULT_MAXIMUM_LAG_ON_FAILOVER);
    }

    #[test]
    fn candidate_facts_from_topology_tags() {
        use crate::topology::model::{Role, member, topo};
        let mut t = topo(vec![member("a", Role::Primary), member("b", Role::SyncStandby)]);
        t.members[1].tags.insert("nofailover".into(), serde_json::json!(true));
        t.members[1].tags.insert("failover_priority".into(), serde_json::json!(2));
        let f = candidate_facts(&t);
        assert_eq!((f[0].role, f[1].role), (CandidateRole::Leader, CandidateRole::Sync));
        assert_eq!((f[1].nofailover, f[1].failover_priority), (true, Some(2)));
    }
```

`src-tauri/tests/it_diag.rs`:

```rust
#[tokio::test]
async fn patroni_health_matrix_and_failover_readiness_on_the_stand() {
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let r = diag::run(&st, "it", req(Tool::Health)).await.unwrap();
    let DiagData::Health(h) = &r.data else { panic!() };
    assert_eq!(h.members.len(), 3);
    let primary_ok = h.members.iter().filter(|m| m.cells.iter().any(|c| c.endpoint == "primary" && c.status == Some(200))).count();
    assert_eq!(primary_ok, 1);
    assert!(h.members.iter().all(|m| m.cells.iter().any(|c| c.endpoint == "health" && c.status == Some(200))));
    assert!(r.findings.is_empty(), "{:?}", r.findings);

    let r = diag::run(&st, "it", req(Tool::Failover)).await.unwrap();
    let DiagData::Failover(f) = &r.data else { panic!() };
    assert_eq!((f.ready_count, f.paused, f.config.maximum_lag_on_failover), (2, false, 1_048_576));
    assert!(r.findings.is_empty(), "{:?}", r.findings);
}

#[tokio::test]
async fn follow_leader_moves_to_the_new_leader_after_switchover() {
    if common::env("GPG_IT_DOCKER").is_none() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let _restore = common::restore_on_drop(common::wait_stand);
    let before = diag::run(&st, "it", req(Tool::Sessions)).await.unwrap();
    let leader = before.member.clone().unwrap();
    let candidate = ["patroni1", "patroni2", "patroni3"].into_iter().find(|m| *m != leader).unwrap();
    common::patronictl(&["switchover", "--leader", &leader, "--candidate", candidate, "--force"]);
    let started = std::time::Instant::now();
    loop {
        let r = diag::run(&st, "it", req(Tool::Sessions)).await;
        if r.as_ref().is_ok_and(|r| r.member.as_deref() == Some(candidate) && r.follows_leader) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(15), "вкладка «за лидером» не перешла на {candidate}: {r:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib topology:: diag::`
Expected: ошибки компиляции.

- [ ] **Step 3: Модель топологии и клиент Patroni**

`src-tauri/src/topology/model.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledSwitchover {
    /// Время из `/cluster` как есть (ISO 8601 со смещением).
    pub at: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledRestart {
    pub at: String,
    pub postgres_version: Option<String>,
}
```

Поле `pub scheduled_restart: Option<ScheduledRestart>` в `Member` (в `Member::new` — `None`), `pub scheduled_switchover: Option<ScheduledSwitchover>` в `Topology` (все литералы `Topology { … }` — `patroni.rs`, `probe.rs`, `model.rs::topo`, `empty_for_tests` — получают `scheduled_switchover: None`, кроме разбора `/cluster`); `same_except_time` — `&& self.scheduled_switchover == other.scheduled_switchover`.

`src-tauri/src/topology/patroni.rs`:
- `ClusterJson` — `#[serde(default)] pub scheduled_switchover: Option<ScheduledSwitchoverJson>`; `MemberJson` — `#[serde(default)] pub scheduled_restart: Option<ScheduledRestartJson>`; структуры `ScheduledSwitchoverJson { at: Option<String>, from: Option<String>, to: Option<String> }`, `ScheduledRestartJson { schedule: Option<String>, postgres_version: Option<String> }` (`Deserialize`, все поля `#[serde(default)]`). В `members_from_cluster` — `member.scheduled_restart = m.scheduled_restart.as_ref().and_then(|r| Some(ScheduledRestart { at: r.schedule.clone()?, postgres_version: r.postgres_version.clone() }))`; в `discover` — `scheduled_switchover` из `cluster.scheduled_switchover` (`at` обязателен, иначе `None`); при недоступном Patroni (ветка с прошлыми участниками) — `previous.and_then(|p| p.scheduled_switchover.clone())`.
- В трейт `PatroniHttp` — метод:

```rust
    /// HTTP-код ответа на GET (матрица health: тело не читается, авторизация не нужна).
    fn get_status<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<u16>>;
```

  реализация у `ReqwestHttp`:

```rust
    fn get_status<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<u16>> {
        Box::pin(async move {
            let resp = self.client.get(url).send().await.map_err(|e| self.request_error(url, &e))?;
            Ok(resp.status().as_u16())
        })
    }
```

  у поддельных клиентов тестов модуля (`FakeHttp`, `SlowHttp`, `HangingHttp`) — по смыслу их `get_json`: ответ из таблицы адресов (`FakeHttp` хранит и код; для существующих записей — 200), задержка, зависание.
- Чтение динамической конфигурации:

```rust
/// `GET /config` по адресам `bases` до первого ответа (лидер прошлого опроса — первым, как у `/cluster`).
pub async fn fetch_config(http: &dyn PatroniHttp, bases: &[String]) -> AppResult<serde_json::Value> {
    let mut last = None;
    for base in bases {
        match http.get_json(&format!("{base}/config")).await {
            Ok(v) => return Ok(v),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses")))
}
```

- [ ] **Step 4: Правила `pg-advisor`**

`lib.rs`: `pub mod failover;`; `rules/mod.rs`: `pub mod patroni;`. `failover.rs` — типы из Interfaces (`Default`, `Serialize`, `Deserialize`, camelCase; `SyncMode` и `CandidateRole` — `#[default]` у `Off` и `Unknown`; `Reason` — camelCase) и:

```rust
pub const DEFAULT_MAXIMUM_LAG_ON_FAILOVER: i64 = 1_048_576;

pub fn evaluate(members: &[CandidateFacts], config: &DynConfig, paused: bool) -> (FailoverData, Vec<Finding>) {
    let mut candidates = Vec::new();
    for m in members.iter().filter(|m| m.role != CandidateRole::Leader) {
        let mut reasons = Vec::new();
        if !m.reachable {
            reasons.push(Reason::Unreachable);
        } else {
            if !matches!(m.state.as_deref(), Some("streaming" | "running")) {
                reasons.push(Reason::NotStreaming);
            }
            match m.lag_bytes {
                Some(l) if l > config.maximum_lag_on_failover => reasons.push(Reason::LagTooHigh),
                Some(_) => {}
                None => reasons.push(Reason::LagUnknown),
            }
        }
        if m.nofailover {
            reasons.push(Reason::NoFailoverTag);
        }
        if m.failover_priority == Some(0) {
            reasons.push(Reason::ZeroPriority);
        }
        let sync_ok = match config.synchronous_mode {
            SyncMode::Off => true,
            SyncMode::On => m.role == CandidateRole::Sync,
            SyncMode::Quorum => matches!(m.role, CandidateRole::Quorum | CandidateRole::Sync),
        };
        if !sync_ok {
            reasons.push(Reason::NotSynchronous);
        }
        candidates.push(Candidate { member: m.name.clone(), ready: reasons.is_empty(), reasons, lag_bytes: m.lag_bytes, pending_restart: m.pending_restart });
    }
    let ready_count = candidates.iter().filter(|c| c.ready).count();
    let mut f = Vec::new();
    if ready_count == 0 && !candidates.is_empty() {
        f.push(Finding::new("failoverCandidates", Severity::Critical).param("count", 0).link_tool("failover", None));
    } else if ready_count == 1 && members.len() >= 3 {
        f.push(Finding::new("failoverCandidates", Severity::Warning).param("count", 1).link_tool("failover", None));
    }
    if config.synchronous_mode != SyncMode::Off && !members.iter().any(|m| matches!(m.role, CandidateRole::Sync | CandidateRole::Quorum)) {
        f.push(Finding::new("syncModeWithoutSync", Severity::Critical));
    }
    if paused {
        f.push(Finding::new("clusterPaused", Severity::Warning));
    }
    if let (Some(ttl), Some(lw), Some(rt)) = (config.ttl, config.loop_wait, config.retry_timeout)
        && ttl < lw + 2 * rt
    {
        f.push(Finding::new("patroniTiming", Severity::Warning).param("ttlS", ttl).param("loopWaitS", lw).param("retryTimeoutS", rt));
    }
    crate::findings::sort(&mut f);
    (FailoverData { paused, config: config.clone(), candidates, ready_count }, f)
}
```

`rules/patroni.rs`:

```rust
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::failover::CandidateRole;
use crate::findings::{Finding, Severity};

/// Адреса проверки здоровья Patroni — то, что видит балансировщик.
pub const HEALTH_ENDPOINTS: [&str; 7] = ["primary", "standby-leader", "replica", "read-only", "sync", "async", "health"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthFacts {
    pub member: String,
    pub role: CandidateRole,
    /// Адрес → HTTP-код; `None` — API не ответил.
    pub codes: BTreeMap<String, Option<u16>>,
}

pub fn evaluate_health(members: &[HealthFacts]) -> Vec<Finding> {
    let code = |m: &HealthFacts, e: &str| m.codes.get(e).copied().flatten();
    let mut out = Vec::new();
    let answered: Vec<&HealthFacts> = members.iter().filter(|m| code(m, "primary").is_some()).collect();
    let primaries = answered.iter().filter(|m| code(m, "primary") == Some(200)).count();
    if !answered.is_empty() && primaries != 1 {
        out.push(Finding::new("healthPrimary", Severity::Critical).param("count", primaries));
    }
    for m in members {
        if code(m, "health").is_some_and(|c| c != 200) {
            out.push(Finding::new("healthUnhealthy", Severity::Critical).member(Some(&m.member)));
        }
        if matches!(m.role, CandidateRole::Replica | CandidateRole::Sync | CandidateRole::Quorum) && code(m, "replica").is_some_and(|c| c != 200) {
            out.push(Finding::new("replicaOutOfPool", Severity::Warning).member(Some(&m.member)));
        }
    }
    crate::findings::sort(&mut out);
    out
}
```

- [ ] **Step 5: Сбор и отчёты**

`src-tauri/src/diag/patroni.rs`:

```rust
//! Patroni: готовность к failover и матрица health (спец. P2 §5.8–5.9). Только `GET`.

pub fn parse_dyn_config(v: &Value) -> DynConfig {
    let int = |k: &str| v.get(k).and_then(Value::as_i64);
    let synchronous_mode = match v.get("synchronous_mode") {
        Some(Value::Bool(true)) => SyncMode::On,
        Some(Value::String(s)) if s.eq_ignore_ascii_case("quorum") => SyncMode::Quorum,
        Some(Value::String(s)) if ["on", "true"].contains(&s.to_ascii_lowercase().as_str()) => SyncMode::On,
        _ => SyncMode::Off,
    };
    DynConfig {
        maximum_lag_on_failover: int("maximum_lag_on_failover").unwrap_or(DEFAULT_MAXIMUM_LAG_ON_FAILOVER),
        synchronous_mode,
        synchronous_mode_strict: v.get("synchronous_mode_strict").and_then(Value::as_bool).unwrap_or(false),
        synchronous_node_count: int("synchronous_node_count"),
        ttl: int("ttl"),
        loop_wait: int("loop_wait"),
        retry_timeout: int("retry_timeout"),
        permanent_slots: v.get("slots").and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default(),
    }
}

pub fn candidate_role(r: Role) -> CandidateRole {
    match r {
        Role::Primary | Role::StandbyLeader => CandidateRole::Leader,
        Role::SyncStandby => CandidateRole::Sync,
        Role::QuorumStandby => CandidateRole::Quorum,
        Role::Replica => CandidateRole::Replica,
        Role::Unknown => CandidateRole::Unknown,
    }
}

pub fn candidate_facts(t: &Topology) -> Vec<CandidateFacts> {
    t.members.iter().map(|m| CandidateFacts {
        name: m.name.clone(),
        role: candidate_role(m.role),
        state: m.state.clone(),
        reachable: m.reachable,
        lag_bytes: m.lag_bytes,
        nofailover: m.tags.get("nofailover").and_then(Value::as_bool).unwrap_or(false),
        failover_priority: m.tags.get("failover_priority").and_then(Value::as_i64),
        pending_restart: m.pending_restart,
    }).collect()
}

/// Динамическая конфигурация кластера подключения: клиент HTTP подключения (`LiveSource::http_for`), адреса — как у `/cluster`.
pub async fn dyn_config(st: &AppState, target: &DiagTarget) -> AppResult<DynConfig> {
    let settings = target.profile.patroni.as_ref().ok_or_else(|| app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses"))?;
    let http = st.live.http_for(&target.profile).await?;
    let v = fetch_config(http.as_ref(), &cluster_bases(settings, Some(&target.topology))).await?;
    Ok(parse_dyn_config(&v))
}

pub async fn failover_report(st: &AppState, target: &DiagTarget) -> AppResult<DiagReport> {
    let config = dyn_config(st, target).await?;
    let (data, findings) = pg_advisor::failover::evaluate(&candidate_facts(&target.topology), &config, target.topology.paused.unwrap_or(false));
    Ok(make_report(Tool::Failover, target, None, DiagData::Failover(data), findings, vec![]))
}

pub async fn health_report(st: &AppState, target: &DiagTarget) -> AppResult<DiagReport> {
    let http = st.live.http_for(&target.profile).await?;
    let members = futures_util::future::join_all(target.topology.members.iter().map(|m| {
        let http = http.clone();
        async move {
            let Some(base) = m.api_url.clone() else {
                return HealthMember { member: m.name.clone(), api_url: None, cells: vec![] };
            };
            let cells = futures_util::future::join_all(HEALTH_ENDPOINTS.iter().map(|e| {
                let (http, url) = (http.clone(), format!("{base}/{e}"));
                async move {
                    match http.get_status(&url).await {
                        Ok(code) => HealthCell { endpoint: e.to_string(), status: Some(code), error: None },
                        Err(err) => HealthCell { endpoint: e.to_string(), status: None, error: Some(err) },
                    }
                }
            })).await;
            HealthMember { member: m.name.clone(), api_url: Some(base), cells }
        }
    })).await;
    let facts: Vec<HealthFacts> = members.iter().zip(&target.topology.members).map(|(h, m)| HealthFacts {
        member: h.member.clone(),
        role: candidate_role(m.role),
        codes: h.cells.iter().map(|c| (c.endpoint.clone(), c.status)).collect(),
    }).collect();
    let findings = evaluate_health(&facts);
    let data = HealthData { endpoints: HEALTH_ENDPOINTS.iter().map(|e| e.to_string()).collect(), members };
    Ok(make_report(Tool::Health, target, None, DiagData::Health(data), findings, vec![]))
}
```

(`HealthCell`, `HealthMember`, `HealthData` — `#[derive(Debug, Clone, Serialize)]`, camelCase.) `diag/mod.rs`: `pub mod patroni;`, варианты `Failover(pg_advisor::failover::FailoverData)`, `Health(patroni::HealthData)`, ветки `Tool::Failover => patroni::failover_report(st, &target).await`, `Tool::Health => patroni::health_report(st, &target).await`.

- [ ] **Step 6: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib`, затем интеграционные (`--test it_diag`, `--test it_patroni`, `--test it_runtime`; тест «за лидером» — с `GPG_IT_DOCKER=1`).
Expected: PASS; существующие тесты разбора `/cluster` не изменились.

- [ ] **Step 7: Commit**

```bash
git add crates/pg-advisor src-tauri
git commit -m "feat(diag): Patroni — готовность к failover, матрица health, запланированные switchover и restart

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Слоты репликации и подписки

**Files:**
- Create: `crates/pg-advisor/src/rules/slots.rs`, `src-tauri/src/diag/slots.rs`
- Modify: `crates/pg-advisor/src/rules/mod.rs`, `src-tauri/src/diag/mod.rs`, `src-tauri/tests/it_diag.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `rules/slots.rs`, `diag/slots.rs`; интеграционные в `it_diag.rs`

**Interfaces:**
- Consumes: задачи 1, 3; `diag::patroni::dyn_config` (7); `CounterMemory`, `rates::delta`.
- Produces:
  - `pg_advisor::rules::slots::{PatroniSlot { Member, Permanent }, Slot { name, slot_type: String, plugin: Option<String>, database: Option<String>, temporary: bool, active: bool, active_pid: Option<i32>, xmin_age: Option<i64>, catalog_xmin_age: Option<i64>, restart_lsn: Option<String>, confirmed_flush_lsn: Option<String>, wal_status: Option<String>, safe_wal_size: Option<i64>, retained_bytes: Option<i64>, two_phase: Option<bool>, conflicting: Option<bool>, inactive_since_ms: Option<i64>, invalidation_reason: Option<String>, failover: Option<bool>, synced: Option<bool>, patroni: Option<PatroniSlot> }, Subscription { name, pid: Option<i32>, received_lsn: Option<String>, last_msg_age_s: Option<f64>, latest_end_age_s: Option<f64>, apply_error_count: Option<i64>, sync_error_count: Option<i64> }, SlotsData { slots, subscriptions, max_slot_wal_keep_size_mb: Option<i64>, subscription_errors_growth: Option<i64> }, patroni_slot_name(&str) -> String, evaluate(member: Option<&str>, &SlotsData) -> Vec<Finding>}`;
  - `diag::slots::{slots_sql(Caps), subscriptions_sql(Caps), collect(&Client, Caps) -> AppResult<(SlotsData, CounterSnapshot)>, mark_patroni(&mut SlotsData, member_names: &[String], permanent: &[String]), report}`; `DiagData::Slots(SlotsData)`; ключ `diag.unavailable.patroniConfig`.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/rules/slots.rs`, `tests`:

```rust
    use super::*;
    use crate::findings::{FindingTarget, Severity};

    const GIB: i64 = 1024 * 1024 * 1024;

    fn slot(name: &str, active: bool, retained: i64, wal_status: &str) -> Slot {
        Slot { name: name.into(), slot_type: "physical".into(), active, retained_bytes: Some(retained), wal_status: Some(wal_status.into()), ..Default::default() }
    }

    #[test]
    fn patroni_member_slot_names() {
        assert_eq!(patroni_slot_name("patroni1"), "patroni1");
        assert_eq!(patroni_slot_name("PG-1.dc"), "pg_1_dc");
        assert_eq!(patroni_slot_name("a b"), "au0032b");
        assert_eq!(patroni_slot_name(&"x".repeat(70)).len(), 63);
    }

    #[test]
    fn retained_wal_lost_and_subscription_errors() {
        let d = SlotsData {
            slots: vec![slot("busy", true, 50 * GIB, "extended"), slot("dead", false, 2 * GIB, "extended"), slot("old", false, 0, "lost")],
            subscription_errors_growth: Some(3),
            ..Default::default()
        };
        let f = evaluate(Some("pg-1"), &d);
        let got: Vec<(&str, Severity)> = f.iter().map(|x| (x.id.as_str(), x.severity)).collect();
        assert_eq!(got, [("slotLost", Severity::Critical), ("slotRetainsWal", Severity::Warning), ("subscriptionErrors", Severity::Warning)]);
        let retains = f.iter().find(|x| x.id == "slotRetainsWal").unwrap();
        assert_eq!(retains.target, Some(FindingTarget::Slot { name: "dead".into() }));
        assert_eq!(retains.params["retainedBytes"], 2 * GIB);
        let unreserved = SlotsData { slots: vec![slot("u", false, 10, "unreserved")], ..Default::default() };
        assert_eq!(evaluate(None, &unreserved)[0].severity, Severity::Critical);
    }
```

`src-tauri/src/diag/slots.rs`, `tests`:

```rust
    #[test]
    fn version_columns_and_patroni_marks() {
        assert!(slots_sql(Caps::new(130000)).contains("NULL::bool"));
        assert!(slots_sql(Caps::new(170000)).contains("inactive_since"));
        assert!(!subscriptions_sql(Caps::new(140000)).contains("pg_stat_subscription_stats"));
        assert!(subscriptions_sql(Caps::new(150000)).contains("pg_stat_subscription_stats"));
        let mut d = SlotsData {
            slots: ["patroni2", "debezium", "other"].iter().map(|n| Slot { name: n.to_string(), ..Default::default() }).collect(),
            ..Default::default()
        };
        mark_patroni(&mut d, &["patroni1".into(), "patroni2".into()], &["debezium".into()]);
        let marks: Vec<Option<PatroniSlot>> = d.slots.iter().map(|s| s.patroni).collect();
        assert_eq!(marks, [Some(PatroniSlot::Member), Some(PatroniSlot::Permanent), None]);
    }
```

`src-tauri/tests/it_diag.rs`:

```rust
#[tokio::test]
async fn slots_sql_runs_on_every_version() {
    for addr in sql_addrs() {
        let c = common::connect_as(&addr, "postgres", common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let h = diag::head(&c).await.unwrap();
        diag::slots::collect(&c, Caps::new(h.version_num)).await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
    }
}

#[tokio::test]
async fn dead_slot_retains_wal_and_the_active_one_does_not_count() {
    let Some(primary) = common::env("GPG_IT_PRIMARY") else { return };
    let _drop = common::restore_on_drop({
        let primary = primary.clone();
        move || common::run_sql_blocking(&primary, "postgres", "SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'")
    });
    let su = common::superuser(&primary, "postgres").await;
    su.batch_execute("SELECT pg_create_physical_replication_slot('it_dead_slot', true)").await.unwrap();
    su.batch_execute("CREATE TABLE IF NOT EXISTS it_wal (x int); INSERT INTO it_wal SELECT generate_series(1, 100000); SELECT pg_switch_wal(); DROP TABLE it_wal;").await.unwrap();
    let (_dir, st) = open_server(&primary).await;
    let DiagData::Slots(d) = diag::run(&st, "it", req(Tool::Slots)).await.unwrap().data else { panic!() };
    let dead = d.slots.iter().find(|s| s.name == "it_dead_slot").unwrap();
    assert!(!dead.active && dead.retained_bytes.unwrap_or(0) > 0);
    assert!(d.slots.iter().any(|s| s.name == "replica1" && s.active));
}

#[tokio::test]
async fn patroni_member_slots_are_marked_and_lab_subscription_is_seen() {
    if let Some((_dir, st)) = common::open_patroni().await {
        let DiagData::Slots(d) = diag::run(&st, "it", req(Tool::Slots)).await.unwrap().data else { panic!() };
        assert!(d.slots.iter().filter(|s| s.name.starts_with("patroni")).all(|s| s.patroni == Some(PatroniSlot::Member)), "{:?}", d.slots);
        assert!(d.slots.iter().any(|s| s.patroni == Some(PatroniSlot::Member)), "у лидера есть слоты реплик");
    }
    if let Some(lab) = common::lab_addr() {
        let (_dir, st) = open_server(&lab).await;
        let DiagData::Slots(d) = diag::run(&st, "it", req(Tool::Slots)).await.unwrap().data else { panic!() };
        assert!(d.slots.iter().any(|s| s.name == "lab_sub_slot" && s.slot_type == "logical" && s.plugin.as_deref() == Some("pgoutput")));
        assert!(d.subscriptions.iter().any(|s| s.name == "lab_sub" && s.pid.is_some()));
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml rules::slots`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация**

`rules/mod.rs`: `pub mod slots;`. `rules/slots.rs` — типы из Interfaces (`Default`, `Serialize`, `Deserialize`, camelCase; `PatroniSlot` — `Copy`) и:

```rust
/// Имя слота участника, как его строит Patroni (`slot_name_from_member_name`): нижний регистр, `-` и `.` → `_`,
/// прочие символы вне `[a-z0-9_]` → `u` и четыре цифры кода символа, не длиннее 63 символов.
pub fn patroni_slot_name(member: &str) -> String {
    let mut out = String::new();
    for c in member.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if c == '-' || c == '.' {
            out.push('_');
        } else {
            out.push_str(&format!("u{:04}", c as u32));
        }
    }
    out.chars().take(63).collect()
}

pub fn evaluate(member: Option<&str>, d: &SlotsData) -> Vec<Finding> {
    let mut out = Vec::new();
    let lost: Vec<&Slot> = d.slots.iter().filter(|s| s.wal_status.as_deref() == Some("lost")).collect();
    if let Some(s) = lost.first() {
        out.push(Finding::new("slotLost", Severity::Critical).member(member).target(FindingTarget::Slot { name: s.name.clone() }).param("count", lost.len()));
    }
    let inactive = d.slots.iter().filter(|s| !s.active && s.wal_status.as_deref() != Some("lost"));
    let graded: Vec<(&Slot, Severity)> = inactive.filter_map(|s| {
        let by_bytes = s.retained_bytes.and_then(|b| grade(b as f64, th::SLOT_RETAINED_BYTES.0, th::SLOT_RETAINED_BYTES.1));
        let unreserved = (s.wal_status.as_deref() == Some("unreserved")).then_some(Severity::Critical);
        worse(by_bytes, unreserved).map(|v| (s, v))
    }).collect();
    if let Some((s, sev)) = graded.iter().max_by_key(|(s, v)| (*v, s.retained_bytes.unwrap_or(0))) {
        let mut f = Finding::new("slotRetainsWal", *sev).member(member).target(FindingTarget::Slot { name: s.name.clone() }).param("count", graded.len()).link_tool("slots", member);
        if let Some(b) = s.retained_bytes { f = f.param("retainedBytes", b); }
        if let Some(w) = &s.wal_status { f = f.param("walStatus", w.clone()); }
        out.push(f);
    }
    if let Some(g) = d.subscription_errors_growth.filter(|g| *g > 0) {
        out.push(Finding::new("subscriptionErrors", Severity::Warning).member(member).param("count", g));
    }
    crate::findings::sort(&mut out);
    out
}
```

`src-tauri/src/diag/slots.rs`:

```rust
//! Слоты репликации и подписки (спец. P2 §5.7).

pub fn slots_sql(caps: Caps) -> String {
    let two_phase = caps.col(V14, "two_phase", "bool");
    let conflicting = caps.col(V16, "conflicting", "bool");
    let inactive_since = caps.col(V17, "(extract(epoch FROM inactive_since) * 1000)::bigint", "bigint");
    let reason = caps.col(V17, "invalidation_reason", "text");
    let failover = caps.col(V17, "failover", "bool");
    let synced = caps.col(V17, "synced", "bool");
    format!(
        "SELECT slot_name::text, slot_type, plugin::text, database::text, temporary, active, active_pid, \
                age(xmin)::bigint, age(catalog_xmin)::bigint, restart_lsn::text, confirmed_flush_lsn::text, wal_status, \
                safe_wal_size, pg_wal_lsn_diff(CASE WHEN pg_is_in_recovery() THEN pg_last_wal_replay_lsn() \
                                                    ELSE pg_current_wal_lsn() END, restart_lsn)::bigint, \
                {two_phase}, {conflicting}, {inactive_since}, {reason}, {failover}, {synced} \
         FROM pg_replication_slots ORDER BY slot_name"
    )
}

pub fn subscriptions_sql(caps: Caps) -> String {
    let (stats, apply, sync) = if caps.at_least(V15) {
        ("LEFT JOIN pg_stat_subscription_stats st ON st.subid = s.subid", "st.apply_error_count", "st.sync_error_count")
    } else {
        ("", "NULL::bigint", "NULL::bigint")
    };
    format!(
        "SELECT s.subname::text, s.pid, s.received_lsn::text, extract(epoch FROM now() - s.last_msg_receipt_time)::float8, \
                extract(epoch FROM now() - s.latest_end_time)::float8, {apply}, {sync} \
         FROM pg_stat_subscription s {stats} WHERE s.relid IS NULL ORDER BY s.subname"
    )
}

const KEEP_SIZE_SQL: &str = "SELECT setting::bigint FROM pg_settings WHERE name = 'max_slot_wal_keep_size'";

/// Отметки слотов Patroni: слоты участников (имя по правилу Patroni) и постоянные из `/config`.
pub fn mark_patroni(d: &mut SlotsData, member_names: &[String], permanent: &[String]) {
    let members: Vec<String> = member_names.iter().map(|m| patroni_slot_name(m)).collect();
    for s in &mut d.slots {
        s.patroni = if members.contains(&s.name) {
            Some(PatroniSlot::Member)
        } else if permanent.contains(&s.name) {
            Some(PatroniSlot::Permanent)
        } else {
            None
        };
    }
}
```

`collect` читает три запроса (`max_slot_wal_keep_size` в МБ, `-1` — без ограничения) и строит снимок `CounterSnapshot { values: {"subscriptionErrors": сумма apply + sync} }` с `now()`, временем запуска postmaster и `stats_reset` (как у конфликтов в задаче 6). `report`: `head` и `collect` в `on_member`; `subscription_errors_growth = rates::delta(prev, &snap, "subscriptionErrors")` через `st.counters.swap(…, Tool::Slots, …)`; у подключения `patroni` — `diag::patroni::dyn_config(st, target)`: удалось — `mark_patroni(&mut data, &имена участников топологии, &config.permanent_slots)`, нет — отметки только по участникам и `Unavailable { part: "patroniSlots", member: None, reason: ltext!("diag.unavailable.patroniConfig", "{{part}}: the Patroni configuration could not be read", part = "patroniSlots") }`. Плюс `caps.missing(V14, "slotTwoPhase")`, `caps.missing(V16, "slotConflicting")`, `caps.missing(V17, "slotInactiveSince")`, `caps.missing(V15, "subscriptionErrors")`. `diag/mod.rs`: `pub mod slots;`, вариант `Slots(SlotsData)`, ветка `Tool::Slots`.

Словари: `diag.unavailable.patroniConfig` — «{{part}}: the Patroni configuration could not be read» / «{{part}}: не удалось прочитать конфигурацию Patroni».

- [ ] **Step 4: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`, затем интеграционные (`--test it_diag`).
Expected: PASS; слота `it_dead_slot` после теста нет.

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor src-tauri src/locales
git commit -m "feat(diag): слоты репликации и подписки, отметки слотов Patroni

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Сэмплер активности, длительные сессии и профиль ожиданий

**Files:**
- Create: `src-tauri/src/session.rs`, `src-tauri/src/sampler/mod.rs`, `src-tauri/src/sampler/buffer.rs`, `src-tauri/src/sampler/registry.rs`, `src-tauri/src/sampler/query.rs`, `crates/pg-advisor/src/ash.rs`, `crates/pg-advisor/src/rules/waits.rs`, `src-tauri/src/diag/waits.rs`, `src-tauri/tests/it_sampler.rs`
- Modify: `crates/pg-advisor/src/lib.rs`, `crates/pg-advisor/src/rules/mod.rs`, `crates/pg-advisor/src/findings.rs` (`thresholds::LOCK_SHARE_MIN_SAMPLES`), `src-tauri/src/conn/pool.rs` (`PoolPurpose::Sampler`), `src-tauri/src/state.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/commands/diag.rs`, `src-tauri/src/diag/mod.rs` (вариант `Waits`, ветка `run`, удаление ветки `other` и ключа `errors.diag.notYet`), `src/locales/{en,ru}.json`
- Test: unit-тесты в `ash.rs`, `rules/waits.rs`, `sampler/buffer.rs`, `sampler/registry.rs`, `session.rs`, `conn/pool.rs`; интеграционные в `it_sampler.rs`

**Interfaces:**
- Consumes: задачи 1, 3; `CloseListener`, `diag_target` (3); `on_catalog_node_within`; `common::{open_server, app, ensure_fixtures, wait_until}`.
- Produces:
  - `pg_advisor::ash::{WaitClass { Cpu, Lock, Lwlock, Io, Ipc, Client, Timeout, BufferPin, Extension, Activity, Other }` (`WaitClass::of(Option<&str>)`), `Sample<'a> { at_ms, pid: i32, leader_pid: Option<i32>, wait_type: Option<&'a str>, wait_event: Option<&'a str>, query_id: Option<i64>, query: &'a str, application: &'a str, user: &'a str, database: &'a str, client: &'a str }`, `Bucket { start_ms, sampled_s: f64, aas: BTreeMap<WaitClass, f64> }`, `Gap { from_ms, to_ms }`, `TopEvent { class, event: Option<String>, samples: u64, share: f64 }`, `TopQuery { key, query_id: Option<i64>, text, samples, share, top_event: Option<String> }`, `TopName { name, samples, share }`, `WaitsData { window_s: u32, bucket_s: u32, sampler_running: bool, truncated: bool, total_samples: u64, buckets, gaps, top_events, top_queries, by_application, by_user, by_database, by_client }`, `bucket_s(window_s) -> u32`, `profile(samples, ticks: &[i64], gaps: &[(i64, i64)], window_s, now_ms, oldest_tick_ms: Option<i64>, sampler_running) -> WaitsData}`;
  - `pg_advisor::rules::waits::evaluate(member: Option<&str>, &WaitsData) -> Vec<Finding>`;
  - `session::{SessionKind { Sampler, Watch }, SessionRegistry { insert(id: String, connection_id, kind, stop: Box<dyn FnOnce() + Send>), stop(&str) -> bool, forget(&str), stop_all() }` (+ `CloseListener`)};
  - `sampler::buffer::{RawSample { pid, leader_pid, backend_type, database, user, application, client, wait_type, wait_event: Option<String>, query_id: Option<i64>, query: Option<String> }, Buffer { new(max_rows, max_age_ms), push(at_ms, Vec<RawSample>) -> Option<(i64, i64)>, mark_gap, samples(from_ms) -> Vec<Sample<'_>>, ticks(from_ms), gaps(from_ms), oldest_tick() -> Option<i64>, len() }, DEFAULT_MAX_ROWS = 500_000, DEFAULT_MAX_AGE_MS = 3_600_000}`;
  - `sampler::registry::{SamplerEvent` (`#[serde(tag = "kind", rename_all = "camelCase")]`: `tick { atMs, active, waiting }` | `gap { fromMs, toMs }` | `error { error }` | `stopped`), `SamplerSink { send(SamplerEvent) -> bool }`, `SampleSource { sample() -> BoxFuture<AppResult<Vec<RawSample>>> }`, `SamplerRegistry { new(interval, sessions: Arc<SessionRegistry>) -> Arc<Self>, subscribe(connection, generation, member, session_id, sink, source), unsubscribe(connection, member, session_id), buffer(connection, member) -> Option<Arc<Mutex<Buffer>>>, is_running(connection, member) -> bool }` (+ `CloseListener`)};
  - `sampler::query::{sample_sql(Caps) -> String, PoolSampleSource}`; `sampler::start(st: &AppState, id: &str, member: Option<&str>, sink: Arc<dyn SamplerSink>) -> AppResult<String>`;
  - `PoolPurpose::Sampler` (одно соединение); `AppState { sessions: Arc<SessionRegistry>, samplers: Arc<SamplerRegistry>, … }`, `AppState::client_with(connection_id, member, database, purpose)`;
  - `diag::waits::report(st, id, target, window_s)`; `DiagData::Waits(WaitsData)`;
  - команды `start_sampler(id, member: Option<String>, channel: Channel<SamplerEvent>) -> String`, `stop_session(session_id) -> bool`.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/ash.rs`, `tests`:

```rust
    use super::*;

    fn s(at_ms: i64, wait_type: Option<&'static str>, wait_event: Option<&'static str>, query_id: Option<i64>, query: &'static str) -> Sample<'static> {
        Sample { at_ms, pid: 1, leader_pid: None, wait_type, wait_event, query_id, query, application: "api", user: "app", database: "orders", client: "10.0.0.1" }
    }

    #[test]
    fn buckets_count_active_sessions_per_sampled_second() {
        let now = 300_000;
        let ticks: Vec<i64> = (0..300).map(|i| i * 1000 + 500).collect();
        // Две сессии каждую секунду первого интервала (5 с): одна на CPU, одна ждёт блокировку.
        let mut samples = Vec::new();
        for t in &ticks[..5] {
            samples.push(s(*t, None, None, Some(7), "select 1"));
            samples.push(s(*t, Some("Lock"), Some("transactionid"), Some(8), "update t"));
        }
        let d = profile(&samples, &ticks, &[], 300, now, Some(500), true);
        assert_eq!((d.bucket_s, d.buckets.len(), d.total_samples), (5, 60, 10));
        assert_eq!(d.buckets[0].sampled_s, 5.0);
        assert_eq!(d.buckets[0].aas[&WaitClass::Cpu], 1.0);
        assert_eq!(d.buckets[0].aas[&WaitClass::Lock], 1.0);
        assert!(d.buckets[1].aas.is_empty());
        assert_eq!(d.top_events[0].share, 0.5);
        assert_eq!(d.top_queries.len(), 2);
        assert_eq!(d.top_queries.iter().find(|q| q.query_id == Some(8)).unwrap().top_event.as_deref(), Some("Lock:transactionid"));
        assert_eq!(d.by_application[0].name, "api");
        assert!(!d.truncated);
    }

    #[test]
    fn window_longer_than_history_is_truncated_and_text_keys_without_query_id() {
        let d = profile(&[s(10_000, None, None, None, "select 2"), s(11_000, None, None, Some(0), "select 2")], &[10_000, 11_000], &[(5_000, 9_000)], 900, 12_000, Some(10_000), false);
        assert!(d.truncated && !d.sampler_running);
        assert_eq!(d.gaps, [Gap { from_ms: 5_000, to_ms: 9_000 }]);
        assert_eq!(d.top_queries.len(), 1, "query_id 0 и пустой — ключ по тексту");
        assert_eq!(WaitClass::of(Some("LWLock")), WaitClass::Lwlock);
        assert_eq!(WaitClass::of(Some("InjectionPoint")), WaitClass::Other);
    }
```

`crates/pg-advisor/src/rules/waits.rs`, `tests`:

```rust
    #[test]
    fn lock_share_needs_enough_samples() {
        use crate::ash::{TopEvent, WaitClass, WaitsData};
        let data = |lock: u64, total: u64| WaitsData {
            total_samples: total,
            top_events: vec![TopEvent { class: WaitClass::Lock, event: Some("relation".into()), samples: lock, share: lock as f64 / total as f64 }],
            ..Default::default()
        };
        assert_eq!(evaluate(None, &data(60, 100))[0].severity, Severity::Critical);
        assert_eq!(evaluate(None, &data(25, 100))[0].severity, Severity::Warning);
        assert!(evaluate(None, &data(10, 100)).is_empty());
        assert!(evaluate(None, &data(10, 12)).is_empty(), "мало сэмплов");
    }
```

`src-tauri/src/sampler/buffer.rs`, `tests`:

```rust
    use super::*;

    fn raw(pid: i32, query: &str) -> RawSample {
        RawSample { pid, query: Some(query.into()), application: Some("api".into()), ..Default::default() }
    }

    #[test]
    fn push_records_ticks_gaps_and_interns_strings() {
        let mut b = Buffer::new(1000, 60_000);
        assert_eq!(b.push(1_000, vec![raw(1, "select 1"), raw(2, "select 1")]), None);
        assert_eq!(b.push(2_000, vec![]), None, "пустой тик — тоже тик");
        assert_eq!(b.push(10_000, vec![raw(1, "select 1")]), Some((3_000, 10_000)), "пауза больше 2 с — промежуток");
        assert_eq!(b.ticks(0), [1_000, 2_000, 10_000]);
        assert_eq!(b.gaps(0), [(3_000, 10_000)]);
        let s = b.samples(0);
        assert_eq!(s.len(), 3);
        assert!(std::ptr::eq(s[0].query.as_ptr(), s[1].query.as_ptr()), "текст хранится один раз");
    }

    #[test]
    fn eviction_by_age_and_by_rows() {
        let mut b = Buffer::new(3, 10_000);
        b.push(1_000, vec![raw(1, "a"), raw(2, "b")]);
        b.push(2_000, vec![raw(3, "c"), raw(4, "d")]);
        assert_eq!(b.len(), 3, "лимит строк");
        b.push(20_000, vec![]);
        assert_eq!((b.len(), b.oldest_tick()), (0, Some(20_000)), "старше срока хранения — вытеснено");
    }
```

`src-tauri/src/sampler/registry.rs`, `tests`:

```rust
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake(Mutex<Vec<AppResult<Vec<RawSample>>>>, AtomicUsize);
    impl SampleSource for Fake {
        fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<Vec<RawSample>>> {
            Box::pin(async move {
                self.1.fetch_add(1, Ordering::SeqCst);
                let mut q = self.0.lock_safe();
                if q.len() > 1 { q.remove(0) } else { q[0].clone() }
            })
        }
    }

    #[derive(Default)]
    struct Sink(Mutex<Vec<SamplerEvent>>, std::sync::atomic::AtomicBool);
    impl SamplerSink for Sink {
        fn send(&self, e: SamplerEvent) -> bool {
            self.0.lock_safe().push(e);
            !self.1.load(Ordering::SeqCst)
        }
    }

    fn reg() -> Arc<SamplerRegistry> {
        SamplerRegistry::new(Duration::from_millis(20), Arc::new(SessionRegistry::default()))
    }

    #[tokio::test]
    async fn one_task_per_node_stops_with_the_last_subscriber() {
        let r = reg();
        let src = Arc::new(Fake(Mutex::new(vec![Ok(vec![RawSample { pid: 1, wait_type: Some("Lock".into()), ..Default::default() }])]), AtomicUsize::new(0)));
        let (a, b) = (Arc::new(Sink::default()), Arc::new(Sink::default()));
        r.subscribe("c", 1, "m", "s1", a.clone(), src.clone());
        r.subscribe("c", 1, "m", "s2", b.clone(), src.clone());
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(r.is_running("c", "m"));
        r.unsubscribe("c", "m", "s1");
        assert!(r.is_running("c", "m"), "второй подписчик держит сэмплер");
        r.unsubscribe("c", "m", "s2");
        let calls = src.1.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.is_running("c", "m"));
        assert!(src.1.load(Ordering::SeqCst) <= calls + 1, "запросы прекратились");
        assert!(r.buffer("c", "m").unwrap().lock_safe().len() > 0, "буфер живёт до закрытия подключения");
        assert!(a.0.lock_safe().iter().any(|e| matches!(e, SamplerEvent::Tick { waiting: 1, .. })));
        r.connection_closed("c");
        assert!(r.buffer("c", "m").is_none());
    }

    #[tokio::test]
    async fn auth_failure_stops_and_closed_sink_unsubscribes() {
        let r = reg();
        let denied = app_err!(ErrorCode::AuthFailed, "errors.pg.authFailed", "Authentication failed: check the user, the password and pg_hba.conf");
        let src = Arc::new(Fake(Mutex::new(vec![Err(denied)]), AtomicUsize::new(0)));
        let sink = Arc::new(Sink::default());
        r.subscribe("c", 1, "m", "s1", sink.clone(), src);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.is_running("c", "m"));
        assert!(matches!(sink.0.lock_safe().last(), Some(SamplerEvent::Stopped)));

        let ok = Arc::new(Fake(Mutex::new(vec![Ok(vec![])]), AtomicUsize::new(0)));
        let gone = Arc::new(Sink::default());
        gone.1.store(true, Ordering::SeqCst);
        r.subscribe("c", 1, "n", "s2", gone, ok);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.is_running("c", "n"), "закрытый канал — отписка");
    }
```

`src-tauri/src/session.rs`, `tests`:

```rust
    #[test]
    fn stop_runs_once_and_connection_close_stops_its_sessions() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let r = SessionRegistry::default();
        let n = Arc::new(AtomicUsize::new(0));
        let add = |id: &str, conn: &str| {
            let n = n.clone();
            r.insert(id.into(), conn, SessionKind::Sampler, Box::new(move || { n.fetch_add(1, Ordering::SeqCst); }));
        };
        add("a", "c1");
        add("b", "c1");
        add("x", "c2");
        assert!(r.stop("a"));
        assert!(!r.stop("a"), "второй раз — сессии нет");
        r.connection_closed("c1");
        assert_eq!(n.load(Ordering::SeqCst), 2);
        r.forget("x");
        r.stop_all();
        assert_eq!(n.load(Ordering::SeqCst), 2, "забытая сессия не останавливается");
    }
```

`src-tauri/src/conn/pool.rs`, `tests`: `assert_eq!(PoolPurpose::Sampler.max_connections(), 1);`.

`src-tauri/tests/it_sampler.rs`:

```rust
//! Сэмплер активности на стенде (`GPG_IT_SINGLE`): ожидания `pg_sleep` и блокировки, остановка с последним подписчиком.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::sampler::{self, registry::{SamplerEvent, SamplerSink}};
use pg_advisor::ash::WaitClass;

#[derive(Default)]
struct Events(Mutex<Vec<SamplerEvent>>);
impl SamplerSink for Events {
    fn send(&self, e: SamplerEvent) -> bool {
        self.0.lock().unwrap().push(e);
        true
    }
}

#[tokio::test]
async fn sampler_sees_sleep_and_lock_waits_and_stops_with_the_last_subscriber() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    common::ensure_fixtures(&addr).await;
    let (_dir, st) = common::open_server(&addr).await;
    let events = Arc::new(Events::default());
    let sid = sampler::start(&st, "it", None, events.clone()).unwrap();

    let holder = common::app(&addr).await;
    holder.batch_execute("BEGIN; UPDATE shop.orders SET status = status WHERE id = 2;").await.unwrap();
    let waiter = common::app(&addr).await;
    let waiting = tokio::spawn(async move { waiter.batch_execute("UPDATE shop.orders SET status = status WHERE id = 2").await });
    let sleeper = common::app(&addr).await;
    let sleeping = tokio::spawn(async move { sleeper.batch_execute("SELECT pg_sleep(4)").await });
    tokio::time::sleep(Duration::from_secs(3)).await;

    let r = diag::run(&st, "it", DiagRequest { tool: Tool::Waits, member: None, window_s: Some(300) }).await.unwrap();
    let DiagData::Waits(d) = &r.data else { panic!() };
    assert!(d.sampler_running && d.total_samples >= 2, "{d:?}");
    assert!(d.top_events.iter().any(|e| e.class == WaitClass::Timeout && e.event.as_deref() == Some("PgSleep")), "{:?}", d.top_events);
    assert!(d.top_events.iter().any(|e| e.class == WaitClass::Lock && e.event.as_deref() == Some("transactionid")), "{:?}", d.top_events);
    assert!(events.0.lock().unwrap().iter().any(|e| matches!(e, SamplerEvent::Tick { .. })));

    holder.batch_execute("ROLLBACK").await.unwrap();
    waiting.await.unwrap().unwrap();
    sleeping.await.unwrap().unwrap();
    assert!(st.sessions.stop(&sid));
    let member = r.member.clone().unwrap();
    common::wait_until("сэмплер остановлен", Duration::from_secs(3), async || !st.samplers.is_running("it", &member)).await;
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml ash rules::waits && cargo test --manifest-path src-tauri/Cargo.toml --lib sampler session`
Expected: ошибки компиляции.

- [ ] **Step 3: `pg-advisor` — агрегация и правило**

`lib.rs`: `pub mod ash;`; `rules/mod.rs`: `pub mod waits;`; в `thresholds` — `pub const LOCK_SHARE_MIN_SAMPLES: u64 = 30;`.

`ash.rs` — типы из Interfaces (выходные — `Default`, `Serialize`, `Deserialize`, camelCase; `WaitClass` — `Copy`, `Ord`, `Hash`, camelCase, служит ключом карты в JSON) и:

```rust
impl WaitClass {
    /// Класс по `wait_event_type`; активная сессия без ожидания — `Cpu` (на процессоре или в неинструментированном ожидании).
    pub fn of(wait_type: Option<&str>) -> Self {
        match wait_type {
            None => Self::Cpu,
            Some("Lock") => Self::Lock,
            Some("LWLock") => Self::Lwlock,
            Some("IO") => Self::Io,
            Some("IPC") => Self::Ipc,
            Some("Client") => Self::Client,
            Some("Timeout") => Self::Timeout,
            Some("BufferPin") => Self::BufferPin,
            Some("Extension") => Self::Extension,
            Some("Activity") => Self::Activity,
            Some(_) => Self::Other,
        }
    }
}

pub fn bucket_s(window_s: u32) -> u32 {
    match window_s {
        300 => 5,
        900 => 10,
        _ => 40,
    }
}

fn event_label(s: &Sample) -> String {
    match (s.wait_type, s.wait_event) {
        (Some(t), Some(e)) => format!("{t}:{e}"),
        (Some(t), None) => t.to_string(),
        _ => "CPU*".into(),
    }
}

fn top_names<'a>(samples: &[Sample<'a>], key: impl Fn(&Sample<'a>) -> &'a str, total: u64) -> Vec<TopName> {
    let mut m: HashMap<&str, u64> = HashMap::new();
    for s in samples {
        *m.entry(key(s)).or_default() += 1;
    }
    let mut v: Vec<TopName> = m.into_iter().map(|(n, c)| TopName { name: n.to_string(), samples: c, share: c as f64 / total as f64 }).collect();
    v.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.name.cmp(&b.name)));
    v.truncate(10);
    v
}

/// Профиль ожиданий за окно `window_s`, заканчивающееся в `now_ms`. Среднее число активных сессий интервала —
/// сэмплы интервала, делённые на секунды, когда сэмплер работал (тики). Параллельные воркеры несут текст и
/// `query_id` запроса лидера, поэтому учитываются за его запросом.
pub fn profile(samples: &[Sample], ticks: &[i64], gaps: &[(i64, i64)], window_s: u32, now_ms: i64, oldest_tick_ms: Option<i64>, sampler_running: bool) -> WaitsData {
    let bucket = bucket_s(window_s);
    let (bucket_ms, from) = (bucket as i64 * 1000, now_ms - window_s as i64 * 1000);
    let n = (window_s / bucket) as usize;
    let mut buckets: Vec<Bucket> = (0..n).map(|i| Bucket { start_ms: from + i as i64 * bucket_ms, sampled_s: 0.0, aas: BTreeMap::new() }).collect();
    let index = |at: i64| ((at - from) / bucket_ms) as usize;
    for t in ticks.iter().filter(|t| **t >= from && **t < now_ms) {
        buckets[index(*t).min(n - 1)].sampled_s += 1.0;
    }
    let in_window: Vec<&Sample> = samples.iter().filter(|s| s.at_ms >= from && s.at_ms < now_ms).collect();
    for s in &in_window {
        *buckets[index(s.at_ms).min(n - 1)].aas.entry(WaitClass::of(s.wait_type)).or_default() += 1.0;
    }
    for b in &mut buckets {
        if b.sampled_s > 0.0 {
            for v in b.aas.values_mut() {
                *v /= b.sampled_s;
            }
        }
    }
    let total = in_window.len() as u64;
    let owned: Vec<Sample> = in_window.iter().map(|s| (*s).clone()).collect();
    // Топ ожиданий.
    let mut events: HashMap<(WaitClass, Option<&str>), u64> = HashMap::new();
    for s in &owned {
        *events.entry((WaitClass::of(s.wait_type), s.wait_event)).or_default() += 1;
    }
    let mut top_events: Vec<TopEvent> = events.into_iter().map(|((class, e), c)| TopEvent { class, event: e.map(str::to_string), samples: c, share: c as f64 / total.max(1) as f64 }).collect();
    top_events.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.class.cmp(&b.class)));
    top_events.truncate(10);
    // Топ запросов: ключ — query_id (если не 0), иначе текст.
    let mut queries: HashMap<String, (Option<i64>, &str, u64, HashMap<String, u64>)> = HashMap::new();
    for s in &owned {
        let key = match s.query_id { Some(id) if id != 0 => format!("id:{id}"), _ => format!("text:{}", s.query) };
        let e = queries.entry(key).or_insert((s.query_id.filter(|i| *i != 0), s.query, 0, HashMap::new()));
        e.2 += 1;
        *e.3.entry(event_label(s)).or_default() += 1;
    }
    let mut top_queries: Vec<TopQuery> = queries.into_iter().map(|(key, (qid, text, c, ev))| TopQuery {
        key: if key.starts_with("text:") { format!("text:{:016x}", hash64(&key)) } else { key },
        query_id: qid,
        text: text.to_string(),
        samples: c,
        share: c as f64 / total.max(1) as f64,
        top_event: ev.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(k, _)| k),
    }).collect();
    top_queries.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.key.cmp(&b.key)));
    top_queries.truncate(10);
    let t = total.max(1);
    WaitsData {
        window_s, bucket_s: bucket, sampler_running,
        // История короче окна: первый тик позже начала окна больше чем на интервал.
        truncated: oldest_tick_ms.is_none_or(|o| o > from + bucket_ms),
        total_samples: total,
        gaps: gaps.iter().filter(|(_, to)| *to > from).map(|(a, b)| Gap { from_ms: *a, to_ms: *b }).collect(),
        top_events, top_queries,
        by_application: top_names(&owned, |s| s.application, t),
        by_user: top_names(&owned, |s| s.user, t),
        by_database: top_names(&owned, |s| s.database, t),
        by_client: top_names(&owned, |s| s.client, t),
        buckets,
    }
}

fn hash64(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
```

(`Sample` — `#[derive(Debug, Clone, Copy)]`; ключ текста в ответе — хэш, чтобы не дублировать текст в JSON; пустое окно даёт нули без деления на ноль.)

`rules/waits.rs`:

```rust
use crate::ash::{WaitClass, WaitsData};
use crate::findings::{Finding, grade, thresholds as th};

pub fn evaluate(member: Option<&str>, d: &WaitsData) -> Vec<Finding> {
    if d.total_samples < th::LOCK_SHARE_MIN_SAMPLES {
        return vec![];
    }
    let lock: u64 = d.top_events.iter().filter(|e| e.class == WaitClass::Lock).map(|e| e.samples).sum();
    let share = lock as f64 / d.total_samples as f64;
    grade(share, th::LOCK_SHARE.0, th::LOCK_SHARE.1)
        .map(|sev| vec![Finding::new("lockShare", sev).member(member).param("sharePct", share * 100.0).link_tool("locks", member)])
        .unwrap_or_default()
}
```

(`top_events` урезан до 10 строк — доля `Lock` по нему занижается, только если классов ожиданий больше десяти; это приемлемо. `use crate::findings::Severity` в тестах.)

- [ ] **Step 4: Буфер, реестр сессий, реестр сэмплеров**

`src-tauri/src/session.rs`:

```rust
//! Длительные сессии (сэмплер, наблюдение — P2b): id → подключение и остановка (спец. P2 §3.5).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind { Sampler, Watch }

struct Entry { connection_id: String, _kind: SessionKind, stop: Box<dyn FnOnce() + Send> }

#[derive(Default)]
pub struct SessionRegistry { inner: Mutex<HashMap<String, Entry>> }

impl SessionRegistry {
    pub fn insert(&self, id: String, connection_id: &str, kind: SessionKind, stop: Box<dyn FnOnce() + Send>) {
        self.inner.lock_safe().insert(id, Entry { connection_id: connection_id.to_string(), _kind: kind, stop });
    }

    /// Останавливает сессию; `false` — её уже нет. Остановка — вне блокировки реестра.
    pub fn stop(&self, id: &str) -> bool {
        let e = self.inner.lock_safe().remove(id);
        e.map(|e| (e.stop)()).is_some()
    }

    /// Сессия закончилась сама (закрытый канал): убрать без вызова остановки.
    pub fn forget(&self, id: &str) {
        self.inner.lock_safe().remove(id);
    }

    pub fn stop_all(&self) {
        let all: Vec<Entry> = self.inner.lock_safe().drain().map(|(_, e)| e).collect();
        for e in all {
            (e.stop)();
        }
    }
}

impl CloseListener for SessionRegistry {
    fn connection_closed(&self, connection_id: &str) {
        let mine: Vec<Entry> = {
            let mut m = self.inner.lock_safe();
            let ids: Vec<String> = m.iter().filter(|(_, e)| e.connection_id == connection_id).map(|(k, _)| k.clone()).collect();
            ids.into_iter().filter_map(|k| m.remove(&k)).collect()
        };
        for e in mine {
            (e.stop)();
        }
    }
    fn member_removed(&self, _: &str, _: &str) {}
}
```

(Слушатели закрытия вызываются под блокировкой `entries` `Runtimes`: остановка сессии сэмплера лишь отписывает её в `SamplerRegistry` (std-мьютекс, без обращения к `Runtimes`) — порядок блокировок соблюдён.)

`src-tauri/src/sampler/buffer.rs` — `RawSample` (`Default`, `Clone`), `Interner { strings: Vec<String>, index: HashMap<String, u32> }` с `intern(&mut self, &str) -> u32` и `get(&self, u32) -> &str`, внутренняя строка `Row { at_ms, pid, leader_pid, wait_type: Option<u32>, wait_event: Option<u32>, query_id, query, application, user, database, client: u32 }`, и:

```rust
pub const DEFAULT_MAX_ROWS: usize = 500_000;
pub const DEFAULT_MAX_AGE_MS: i64 = 3_600_000;
/// Пауза между тиками дольше — промежуток в истории.
const GAP_AFTER_MS: i64 = 2_000;
/// Интернированных строк больше — словарь пересобирается по живым строкам.
const COMPACT_AFTER: usize = 100_000;

pub struct Buffer {
    rows: VecDeque<Row>,
    ticks: VecDeque<i64>,
    gaps: Vec<(i64, i64)>,
    strings: Interner,
    max_rows: usize,
    max_age_ms: i64,
}

impl Buffer {
    pub fn new(max_rows: usize, max_age_ms: i64) -> Self { … }

    /// Тик сэмплера в `at_ms` с его строками (пустой список — тоже тик). Возвращает промежуток, если пауза после
    /// прошлого тика дольше 2 с (сэмплер стоял: вкладка была закрыта, запрос не успевал или падал).
    pub fn push(&mut self, at_ms: i64, samples: Vec<RawSample>) -> Option<(i64, i64)> {
        let gap = self.ticks.back().copied().filter(|last| at_ms - last > GAP_AFTER_MS).map(|last| (last + 1_000, at_ms));
        if let Some(g) = gap {
            self.gaps.push(g);
        }
        self.ticks.push_back(at_ms);
        for s in samples {
            let row = Row { at_ms, pid: s.pid, leader_pid: s.leader_pid, wait_type: s.wait_type.as_deref().map(|x| self.strings.intern(x)), … };
            self.rows.push_back(row);
        }
        self.evict(at_ms);
        gap
    }

    pub fn mark_gap(&mut self, from_ms: i64, to_ms: i64) { self.gaps.push((from_ms, to_ms)); }

    fn evict(&mut self, now_ms: i64) {
        let oldest = now_ms - self.max_age_ms;
        while self.rows.front().is_some_and(|r| r.at_ms < oldest) || self.rows.len() > self.max_rows { self.rows.pop_front(); }
        while self.ticks.front().is_some_and(|t| *t < oldest) { self.ticks.pop_front(); }
        self.gaps.retain(|(_, to)| *to >= oldest);
        if self.strings.len() > COMPACT_AFTER { self.compact(); }
    }
    …
}
```

`samples(from_ms)` отдаёт `Sample<'_>` со ссылками на строки словаря (`None` строки → `""`), `ticks(from_ms)` и `gaps(from_ms)` — копии, `oldest_tick()` — первый тик, `len()` — число строк сэмплов.

`src-tauri/src/sampler/registry.rs` — типы из Interfaces и:

```rust
const BACKOFF_S: [u64; 5] = [1, 2, 5, 10, 30];
/// Предел одного сэмпла: молчащий узел не держит задачу.
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(5);

struct Node {
    generation: u64,
    buffer: Arc<Mutex<Buffer>>,
    subscribers: HashMap<String, Arc<dyn SamplerSink>>,
    task: Option<tokio::task::AbortHandle>,
}

pub struct SamplerRegistry {
    nodes: Mutex<HashMap<(String, String), Node>>,
    interval: Duration,
    sessions: Arc<SessionRegistry>,
}

impl SamplerRegistry {
    pub fn new(interval: Duration, sessions: Arc<SessionRegistry>) -> Arc<Self> { … }

    /// Подписка вкладки на сэмплер узла: узла нет или он от прошлого открытия подключения — новый буфер;
    /// задача сэмплера запускается, если не идёт.
    pub fn subscribe(self: &Arc<Self>, connection: &str, generation: u64, member: &str, session_id: &str, sink: Arc<dyn SamplerSink>, source: Arc<dyn SampleSource>) {
        let key = (connection.to_string(), member.to_string());
        let mut nodes = self.nodes.lock_safe();
        let node = nodes.entry(key.clone()).or_insert_with(|| Node::new(generation));
        if node.generation != generation {
            if let Some(t) = node.task.take() { t.abort(); }
            *node = Node::new(generation);
        }
        node.subscribers.insert(session_id.to_string(), sink);
        if node.task.is_none() {
            let (weak, buffer) = (Arc::downgrade(self), node.buffer.clone());
            node.task = Some(tokio::spawn(run(weak, key, source, buffer, self.interval)).abort_handle());
        }
    }

    pub fn unsubscribe(&self, connection: &str, member: &str, session_id: &str) {
        let mut nodes = self.nodes.lock_safe();
        if let Some(n) = nodes.get_mut(&(connection.to_string(), member.to_string())) {
            n.subscribers.remove(session_id);
            if n.subscribers.is_empty() && let Some(t) = n.task.take() { t.abort(); }
        }
    }

    pub fn buffer(&self, connection: &str, member: &str) -> Option<Arc<Mutex<Buffer>>> { … }
    pub fn is_running(&self, connection: &str, member: &str) -> bool { … node.task.is_some() … }

    /// Рассылка подписчикам; закрытые каналы отписываются (и забываются в реестре сессий). Без подписчиков — `false`.
    fn broadcast(&self, key: &(String, String), e: &SamplerEvent) -> bool { … }

    /// Задача закончилась сама (отказ во входе): `task = None`.
    fn finished(&self, key: &(String, String)) { … }
}

async fn run(weak: Weak<SamplerRegistry>, key: (String, String), source: Arc<dyn SampleSource>, buffer: Arc<Mutex<Buffer>>, interval: Duration) {
    let mut tick = tokio::time::interval(interval);
    // Запрос дольше интервала: пропущенные тики не догоняются — запросы не накладываются.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut failures = 0usize;
    loop {
        tick.tick().await;
        let result = tokio::time::timeout(SAMPLE_TIMEOUT, source.sample()).await;
        let failed = !matches!(result, Ok(Ok(_)));
        let Some(reg) = weak.upgrade() else { return };
        let alive = match result {
            Ok(Ok(rows)) => {
                failures = 0;
                let at_ms = crate::topology::model::now_ms();
                let (active, waiting) = (rows.len() as u32, rows.iter().filter(|r| r.wait_type.as_deref() == Some("Lock")).count() as u32);
                let gap = buffer.lock_safe().push(at_ms, rows);
                let mut alive = true;
                if let Some((from_ms, to_ms)) = gap { alive &= reg.broadcast(&key, &SamplerEvent::Gap { from_ms, to_ms }); }
                alive && reg.broadcast(&key, &SamplerEvent::Tick { at_ms, active, waiting })
            }
            Ok(Err(e)) if e.code == ErrorCode::AuthFailed => {
                reg.broadcast(&key, &SamplerEvent::Error { error: e });
                reg.broadcast(&key, &SamplerEvent::Stopped);
                reg.finished(&key);
                return;
            }
            Ok(Err(e)) => reg.broadcast(&key, &SamplerEvent::Error { error: e }),
            Err(_) => reg.broadcast(&key, &SamplerEvent::Error { error: crate::topology::probe::no_answer(SAMPLE_TIMEOUT) }),
        };
        if !alive {
            reg.finished(&key);
            return;
        }
        if failed {
            drop(reg);
            tokio::time::sleep(Duration::from_secs(BACKOFF_S[failures.min(BACKOFF_S.len() - 1)])).await;
            failures += 1;
        }
    }
}
```

(Порядок: тест `auth_failure_stops…` ждёт `Stopped` последним событием. `impl CloseListener for SamplerRegistry`: `connection_closed` — прерывает задачи и удаляет узлы подключения, подписчикам — `Stopped`; `member_removed` — то же для узла.)

`src-tauri/src/sampler/query.rs`:

```rust
pub fn sample_sql(caps: Caps) -> String {
    let query_id = caps.col(V14, "a.query_id", "bigint");
    format!(
        "SELECT a.pid, a.leader_pid, a.backend_type, a.datname::text, a.usename::text, a.application_name, \
                host(a.client_addr), a.wait_event_type, a.wait_event, {query_id}, left(a.query, 16384) \
         FROM pg_stat_activity a \
         WHERE a.state = 'active' AND a.pid <> pg_backend_pid() \
           AND a.backend_type IN ('client backend', 'autovacuum worker', 'parallel worker')"
    )
}

/// Сэмпл с узла через пул `PoolPurpose::Sampler`; версия сервера — первым запросом, затем запомнена.
pub struct PoolSampleSource { st: AppState, connection_id: String, member: String, caps: OnceLock<Caps> }
```

`sample()`: `on_catalog_node_within(Duration::from_secs(5), st.client_with(&id, Some(&member), None, PoolPurpose::Sampler), |c| …)` — при пустом `caps` сначала `diag::head(c)`; строки → `RawSample`.

`src-tauri/src/sampler/mod.rs`:

```rust
//! Сэмплер активности (спец. P2 §6): раз в секунду не-idle сессии узла, в памяти, пока подписана вкладка.

pub mod buffer;
pub mod query;
pub mod registry;

/// Подписка на сэмплер узла (`member = None` — «за лидером» на момент подписки). Возвращает id сессии для `stop_session`.
pub fn start(st: &AppState, id: &str, member: Option<&str>, sink: Arc<dyn SamplerSink>) -> AppResult<String> {
    let target = st.runtimes.diag_target(id, member, false)?;
    let m = target.member.map(|m| m.name).ok_or_else(|| AppError::internal_with("sampler without a member"))?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let source = Arc::new(query::PoolSampleSource::new(st.clone(), id, &m));
    let (samplers, conn, member_name, sid) = (st.samplers.clone(), id.to_string(), m.clone(), session_id.clone());
    st.sessions.insert(session_id.clone(), id, SessionKind::Sampler, Box::new(move || samplers.unsubscribe(&conn, &member_name, &sid)));
    st.samplers.subscribe(id, target.generation, &m, &session_id, sink, source);
    Ok(session_id)
}
```

- [ ] **Step 5: Профиль ожиданий, состояние, команды**

`src-tauri/src/diag/waits.rs`:

```rust
pub async fn report(st: &AppState, id: &str, target: &DiagTarget, window_s: u32) -> AppResult<DiagReport> {
    let member = target.member.as_ref().map(|m| m.name.clone()).ok_or_else(|| AppError::internal_with("waits without a member"))?;
    let now = crate::topology::model::now_ms();
    let from = now - window_s as i64 * 1000;
    let running = st.samplers.is_running(id, &member);
    let data = match st.samplers.buffer(id, &member) {
        Some(b) => {
            let b = b.lock_safe();
            pg_advisor::ash::profile(&b.samples(from), &b.ticks(from), &b.gaps(from), window_s, now, b.oldest_tick(), running)
        }
        None => pg_advisor::ash::profile(&[], &[], &[], window_s, now, None, running),
    };
    let findings = pg_advisor::rules::waits::evaluate(Some(&member), &data);
    Ok(make_report(Tool::Waits, target, None, DiagData::Waits(data), findings, vec![]))
}
```

`diag/mod.rs`: `pub mod waits;`, вариант `Waits(pg_advisor::ash::WaitsData)`, ветка `Tool::Waits => waits::report(st, id, &target, req.window_s.unwrap_or(DEFAULT_WINDOW_S)).await`; ветка `other` и ключ `errors.diag.notYet` удаляются — `match` теперь полный.

`src-tauri/src/conn/pool.rs`: `PoolPurpose::Sampler` (комментарий: «сэмплер активности — своё соединение, чтобы опрос раз в секунду не ждал страниц каталога»), `max_connections` — 1.

`src-tauri/src/state.rs`: поля `pub sessions: Arc<SessionRegistry>`, `pub samplers: Arc<SamplerRegistry>` (`SamplerRegistry::new(Duration::from_secs(1), sessions.clone())`), оба — слушатели закрытия `runtimes`; метод `client_with(connection_id, member, database, purpose)` (тело — нынешнее `client` с параметром `purpose`), `client` вызывает его с `PoolPurpose::Catalog`.

`src-tauri/src/commands/diag.rs`:

```rust
struct ChannelSink(Channel<SamplerEvent>);
impl SamplerSink for ChannelSink {
    fn send(&self, e: SamplerEvent) -> bool {
        self.0.send(e).is_ok()
    }
}

#[tauri::command]
pub fn start_sampler(state: State<'_, AppState>, id: String, member: Option<String>, channel: Channel<SamplerEvent>) -> AppResult<String> {
    crate::sampler::start(&state, &id, member.as_deref(), Arc::new(ChannelSink(channel)))
}

#[tauri::command]
pub fn stop_session(state: State<'_, AppState>, session_id: String) -> AppResult<bool> {
    Ok(state.sessions.stop(&session_id))
}
```

`src-tauri/src/lib.rs`: `pub mod sampler; pub mod session;`, команды `commands::diag::start_sampler`, `commands::diag::stop_session`; в обработчике `RunEvent::Exit` — `st.sessions.stop_all();` до `st.runtimes.shutdown()`. `start_sampler` — синхронная команда, но `subscribe` вызывает `tokio::spawn`: команда помечается `async` (рантайм Tauri), либо задача запускается через `tauri::async_runtime::spawn` — выбрать `async fn` команды, как у `run_diagnostic`.

Словари: убрать `errors.diag.notYet`.

- [ ] **Step 6: Тесты проходят**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`, затем интеграционные (`--test it_sampler`, `--test it_diag`).
Expected: PASS; после теста на `pg-single` нет сессий с `application_name = 'glafira-pg'`, выполняющих запрос сэмплера (соединение пула может простаивать до 5 мин — это нормально).

- [ ] **Step 7: Commit**

```bash
git add crates/pg-advisor src-tauri src/locales
git commit -m "feat(diag): сэмплер активности, длительные сессии и профиль ожиданий

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 10: Фронтенд — каркас вкладки инструмента, находки, дерево

**Files:**
- Create: `src/components/diag/DiagTab.tsx` (+ `.test.tsx`), `src/components/diag/DiagHeader.tsx`, `src/components/diag/FindingsPanel.tsx` (+ `.test.tsx`), `src/components/diag/findingText.ts` (+ `.test.ts`), `src/components/connections/tree/DiagnosticsNodes.tsx` (+ `.test.tsx`), `src/lib/useDiagnostic.ts` (+ `.test.ts`)
- Modify: `src/api/types.ts`, `src/api/client.ts`, `src/api/client.channel.test.ts`, `src/state/workspace.ts` (+ `.test.ts`), `src/components/Layout.tsx`, `src/components/TabContent.tsx`, `src/components/connections/tree/CatalogNodes.tsx`, `src/components/connections/tree/ConnectionNode.tsx` (пункт меню «Пульс»), `src/lib/useConnectionEvents.ts` (`PG_QUERY_KEYS`), `src/lib/format.ts` (+ `.test.ts`), `src/test/fixtures.ts`, `src/locales/{en,ru}.json`, `docs/i18n/same-as-en.json`, `src/styles.css`
- Test: vitest

**Interfaces:**
- Consumes: команды `run_diagnostic`, `start_sampler`, `stop_session` и JSON отчётов задач 3–9; `useTopology`, `useWorkspace`, `TreeGroup`, `ErrorBanner`, `CopyButton`, `formatBytes`, `formatPercent`, `formatCount`, `formatTimestamp`.
- Produces:
  - TS-типы (зеркало Rust, camelCase): `DiagTool = "pulse" | "sessions" | "idleTx" | "waits" | "locks" | "horizon" | "replication" | "slots" | "failover" | "health"`, `RustTool = Exclude<DiagTool, "idleTx">`, `Severity`, `FindingTarget`, `FindingLink` (`{ kind: "openTool"; tool: DiagTool; member: string | null }`), `Finding`, `Unavailable { part; member?; reason: LocalizedText }`, `DiagRequest { tool: RustTool; member: string | null; windowS?: number }`, `DiagReport<D = DiagData>`, `DiagData` (объединение по `kind`: `PulseData`, `SessionsData`, `WaitsData`, `LocksData`, `HorizonData`, `ReplicationData`, `SlotsData`, `FailoverData`, `HealthData` с их строками — поля как в Interfaces задач 3–9), `SamplerEvent`; `ErrorCode` + `"RECOVERY_CONFLICT"`; `Topology.scheduledSwitchover: ScheduledSwitchover | null`, `Member.scheduledRestart: ScheduledRestart | null`;
  - `api.runDiagnostic(id, request)`, `api.startSampler(id, member, onEvent) -> Promise<string>`, `api.stopSession(sessionId) -> Promise<boolean>`;
  - `TabSpec` — `{ kind: "diag"; connectionId: string; tool: DiagTool; member?: string | null }`, `tabId` — `diag:<connectionId>:<tool>`; заголовок вкладки `tabs.diag` («{{tool}} — {{conn}}»);
  - `useDiagnostic(connectionId, tool, member, { active, intervalS, windowS?, live })` — `UseQueryResult<DiagReport>`; `diagRefetchInterval(active, intervalS, error) -> number | false`; `rustTool(tool)`; `defaultIntervalS(tool)`; `DIAG_INTERVALS_S = [2, 5, 10, 30]`; ключ кэша `["diag", connectionId, rustTool, member ?? "", windowS ?? 0]`, `"diag"` — в `PG_QUERY_KEYS`;
  - `findingParams(f) -> Record<string, string>`, `findingTitle(f)`, `findingFix(f)`; `formatDuration(seconds: number | null) -> string`;
  - `DiagTab({ connectionId, tool, member, active, tabId })` — шапка, находки, недоступное, тело инструмента (`ToolBody` — `switch` по `report.data.kind`, задачи 11–13 подставляют свои виды; до них — `<pre>` с JSON); `DIAG_TOOLS_FOR(kind: ConnectionKind) -> DiagTool[]`.

- [ ] **Step 1: Падающие тесты**

`src/state/workspace.test.ts`:

```ts
    expect(tabId({ kind: "diag", connectionId: "c", tool: "pulse" })).toBe("diag:c:pulse");
    expect(tabId({ kind: "diag", connectionId: "c", tool: "idleTx", member: "pg-2" })).toBe("diag:c:idleTx");
```

и в тесте `open`: повторное открытие `diag` с другим `member` не создаёт вторую вкладку, а меняет `member` у существующей.

`src/lib/useDiagnostic.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { defaultIntervalS, diagRefetchInterval, rustTool } from "./useDiagnostic";

describe("опрос инструмента", () => {
  it("скрытая вкладка, пауза и отказ во входе останавливают опрос", () => {
    expect(diagRefetchInterval(true, 5, null)).toBe(5000);
    expect(diagRefetchInterval(false, 5, null)).toBe(false);
    expect(diagRefetchInterval(true, null, null)).toBe(false);
    expect(diagRefetchInterval(true, 5, { code: "AUTH_FAILED", message: "x", retryable: false })).toBe(false);
    expect(diagRefetchInterval(true, 5, { code: "TIMEOUT", message: "x", retryable: true })).toBe(5000);
  });

  it("idleTx — инструмент sessions; Patroni опрашивается реже", () => {
    expect(rustTool("idleTx")).toBe("sessions");
    expect(rustTool("locks")).toBe("locks");
    expect([defaultIntervalS("health"), defaultIntervalS("failover"), defaultIntervalS("sessions")]).toEqual([10, 10, 5]);
  });
});
```

`src/components/diag/findingText.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { i18n } from "../../i18n";
import type { Finding } from "../../api/types";
import { findingParams, findingTitle } from "./findingText";

const f = (patch: Partial<Finding>): Finding => ({ id: "idleInTransaction", severity: "warning", member: "pg-1", target: null, params: {}, links: [], ...patch });

describe("тексты находок", () => {
  it("параметры форматируются по суффиксу имени", async () => {
    await i18n.changeLanguage("ru");
    const p = findingParams(f({ params: { maxS: 3725, retainedBytes: 2048, usedPct: 96.5, count: 1200, state: "catchup" }, target: { kind: "slot", name: "dead" } }));
    // Разделитель разрядов в ru — неразрывный пробел: ожидание строится той же функцией.
    expect(p).toMatchObject({ maxS: "1 ч 2 мин", retainedBytes: "2,0 KB", usedPct: "96,5 %", count: formatCount(1200), state: "catchup", name: "dead", member: "pg-1" });
  });

  it("лаг собирается из байт и времени, вид держателя горизонта переводится", async () => {
    await i18n.changeLanguage("ru");
    expect(findingParams(f({ id: "replicaLag", params: { lagBytes: 1024 * 1024 * 20, delayS: 40 } })).lag).toBe("20,0 MB, 40 с");
    expect(findingParams(f({ id: "xminHorizon", params: { kind: "slot", name: "s", xminAge: 5 } })).kindLabel).toBe("слот");
    expect(findingTitle(f({ id: "deadlocks", params: { count: 2 } }))).toBe("Взаимных блокировок за интервал: 2");
  });
});
```

(`formatCount` импортируется из `../../lib/format`.)

`src/components/diag/FindingsPanel.test.tsx`:

```tsx
  it("находки по строгости с текстом, «что сделать», командой и переходом в инструмент", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    renderWithClient(
      <FindingsPanel
        connectionId="c"
        findings={[
          { id: "noIdleTimeout", severity: "info", member: "pg-1", target: null, params: { command: "ALTER ROLE \"app\" SET idle_in_transaction_session_timeout = '10min';" }, links: [] },
          { id: "idleInTransaction", severity: "critical", member: "pg-1", target: { kind: "session", pid: 42, backendStartUs: 1 }, params: { maxS: 2000, count: 3 }, links: [{ kind: "openTool", tool: "idleTx", member: "pg-1" }] },
        ]}
      />,
    );
    const items = screen.getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("Критично");
    expect(items[0]).toHaveTextContent("Простой в транзакции 33 мин 20 с");
    await user.click(within(items[0]).getByRole("button", { name: "Открыть «Idle in transaction»" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", tool: "idleTx", member: "pg-1" });
    expect(within(items[1]).getByRole("button", { name: "Скопировать команду" })).toBeInTheDocument();
  });

  it("без находок — «Проблем не найдено»", () => {
    renderWithClient(<FindingsPanel connectionId="c" findings={[]} />);
    expect(screen.getByText("Проблем не найдено")).toBeInTheDocument();
  });
```

`src/components/diag/DiagTab.test.tsx`:

```tsx
vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn() } };
});

describe("DiagTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", kind: "patroni" })]);
  });

  it("за лидером по умолчанию; выбор узла перечитывает отчёт с этим узлом", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "sessions", rows: [], settings: { idleInTransactionSessionTimeoutMs: 0, trackActivityQuerySize: 4096 } }));
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="sessions" member={null} active tabId="diag:c:sessions" />);
    expect(await screen.findByRole("heading", { name: "Сессии" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "sessions", member: null });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Узел" })).toHaveDisplayValue("За лидером (pg-1)"));
    await user.selectOptions(screen.getByRole("combobox", { name: "Узел" }), "pg-2");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", { tool: "sessions", member: "pg-2" }));
  });

  it("смена лидера в режиме «за лидером» объявляется строкой", async () => {
    vi.mocked(api.runDiagnostic)
      .mockResolvedValueOnce(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: "pg-1" }))
      .mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: "pg-2" }));
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    await screen.findByRole("heading", { name: "Блокировки" });
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    expect(await screen.findByText("Узел сменился: pg-1 → pg-2")).toBeInTheDocument();
  });

  it("ошибка обновления оставляет прошлые данные и помечает их устаревшими", async () => {
    vi.mocked(api.runDiagnostic)
      .mockResolvedValueOnce(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }))
      .mockRejectedValue({ code: "TIMEOUT", key: "errors.pg.statementTimeout", message: "x", retryable: true });
    const user = userEvent.setup();
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    await screen.findByText("Проблем не найдено");
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    expect(await screen.findByText("устарело")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("Проблем не найдено")).toBeInTheDocument();
  });

  it("недоступное по версии показано серой строкой", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, {
      unavailable: [{ part: "queryId", reason: { key: "diag.unavailable.version", params: { part: "queryId", version: "14" }, text: "queryId: PostgreSQL 14+" } }],
    }));
    renderWithClient(<DiagTab connectionId="c" tool="locks" member={null} active tabId="diag:c:locks" />);
    expect(await screen.findByText("queryId: PostgreSQL 14+")).toBeInTheDocument();
  });
});
```

(`connection`, `member`, `topology`, `topologyView` — фикстуры `src/test/fixtures.ts`; если фикстуры подключения нет — добавить `connection(patch)` с полным `ConnectionProfile`; `diagReport(data, patch)` — новая фикстура: `tool` = `data.kind`, `member: "pg-1"`, `followsLeader: true`, `observedAtMs`, `serverVersionNum: 170004`, `inRecovery: false`, пустые находки и недоступное.)

`src/components/connections/tree/DiagnosticsNodes.test.tsx`:

```tsx
  it("у сервера нет инструментов Patroni; клик открывает вкладку инструмента «за лидером»", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    renderWithClient(<ul><DiagnosticsNodes connectionId="c" kind="server" /></ul>);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    const names = screen.getAllByRole("button").map((b) => b.textContent);
    expect(names).toEqual(["Диагностика", "Пульс", "Сессии", "Idle in transaction", "Профиль ожиданий", "Блокировки", "Горизонт xmin", "Реплики", "Слоты репликации"]);
    await user.click(screen.getByRole("button", { name: "Блокировки" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", connectionId: "c", tool: "locks", id: "diag:c:locks" });
  });

  it("у кластера Patroni — ещё готовность к failover и матрица health", async () => {
    const user = userEvent.setup();
    renderWithClient(<ul><DiagnosticsNodes connectionId="c" kind="patroni" /></ul>);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    expect(screen.getByRole("button", { name: "Готовность к failover" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Матрица health" })).toBeInTheDocument();
  });
```

`src/lib/format.test.ts`:

```ts
  it("длительность — две старшие единицы", async () => {
    await i18n.changeLanguage("ru");
    expect([formatDuration(null), formatDuration(0.4), formatDuration(42), formatDuration(3725), formatDuration(90061)]).toEqual(["—", "0 с", "42 с", "1 ч 2 мин", "1 д 1 ч"]);
    await i18n.changeLanguage("en");
    expect(formatDuration(125)).toBe("2 min 5 s");
  });
```

`src/api/client.channel.test.ts` — строки таблицы «команды и аргументы»:

```ts
    ["runDiagnostic", () => api.runDiagnostic("c", { tool: "pulse", member: null }), "run_diagnostic", { id: "c", request: { tool: "pulse", member: null } }],
    ["stopSession", () => api.stopSession("s"), "stop_session", { sessionId: "s" }],
```

и тест `startSampler`: события канала доходят до `onEvent`, команда — `start_sampler` с `{ id, member, channel }`, итог — id сессии.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm vitest run src/state src/lib src/components/diag src/components/connections/tree/DiagnosticsNodes.test.tsx src/api`
Expected: FAIL (модулей нет).

- [ ] **Step 3: Типы, клиент, вкладка в рабочем пространстве**

`src/api/types.ts` — типы из Interfaces. Строки данных повторяют поля Rust (camelCase), например:

```ts
export type Severity = "info" | "warning" | "critical";
export type DiagTool = "pulse" | "sessions" | "idleTx" | "waits" | "locks" | "horizon" | "replication" | "slots" | "failover" | "health";
export type RustTool = Exclude<DiagTool, "idleTx">;
export type FindingTarget =
  | { kind: "session"; pid: number; backendStartUs: number }
  | { kind: "slot"; name: string }
  | { kind: "member"; name: string }
  | { kind: "relation"; name: string }
  | { kind: "database"; name: string };
export type FindingLink = { kind: "openTool"; tool: DiagTool; member: string | null };
export interface Finding {
  id: string;
  severity: Severity;
  member: string | null;
  target: FindingTarget | null;
  params: Record<string, string | number | boolean | null>;
  links: FindingLink[];
}
export interface Unavailable { part: string; member?: string; reason: LocalizedText }
export interface DiagRequest { tool: RustTool; member: string | null; windowS?: number }
export interface SessionRow {
  pid: number; leaderPid: number | null; backendType: string | null; database: string | null; user: string | null;
  application: string | null; clientAddr: string | null; clientPort: number | null; backendStartUs: number | null;
  xactAgeS: number | null; queryAgeS: number | null; stateAgeS: number | null; state: string | null;
  waitEventType: string | null; waitEvent: string | null; backendXid: string | null; xminAge: number | null;
  queryId: number | null; query: string | null; queryTruncated: boolean; blockedBy: number[]; locksHeld: number; glafira: boolean;
}
export interface SessionsData { rows: SessionRow[]; settings: { idleInTransactionSessionTimeoutMs: number | null; trackActivityQuerySize: number | null } }
// … PulseNode/PulseData, LockSession/WaitingLock/LockNode/DdlQueueItem/LocksData, Holder/HorizonData,
//   Sender/Receiver/Replay/Conflicts/ReplicationData, Slot/Subscription/SlotsData, Candidate/DynConfig/FailoverData,
//   HealthCell/HealthMember/HealthData, WaitClass/Bucket/Gap/TopEvent/TopQuery/TopName/WaitsData — по Interfaces задач 4–9.
export type DiagData =
  | ({ kind: "pulse" } & PulseData)
  | ({ kind: "sessions" } & SessionsData)
  | ({ kind: "waits" } & WaitsData)
  | ({ kind: "locks" } & LocksData)
  | ({ kind: "horizon" } & HorizonData)
  | ({ kind: "replication" } & ReplicationData)
  | ({ kind: "slots" } & SlotsData)
  | ({ kind: "failover" } & FailoverData)
  | ({ kind: "health" } & HealthData);
export interface DiagReport<D extends DiagData = DiagData> {
  tool: RustTool; member: string | null; followsLeader: boolean; observedAtMs: number;
  serverVersionNum: number | null; inRecovery: boolean | null; data: D; findings: Finding[]; unavailable: Unavailable[];
}
export type SamplerEvent =
  | { kind: "tick"; atMs: number; active: number; waiting: number }
  | { kind: "gap"; fromMs: number; toMs: number }
  | { kind: "error"; error: AppError }
  | { kind: "stopped" };
export interface ScheduledSwitchover { at: string; from: string | null; to: string | null }
export interface ScheduledRestart { at: string; postgresVersion: string | null }
```

(`Topology.scheduledSwitchover`, `Member.scheduledRestart`, `ErrorCode` + `"RECOVERY_CONFLICT"`; `src/test/fixtures.ts` — значения `null` в `member()` и `topology()`.)

`src/api/client.ts`:

```ts
  runDiagnostic: (id: string, request: DiagRequest) => call<DiagReport>("run_diagnostic", { id, request }),
  /** Подписка на сэмплер узла: события — в `onEvent`, итог — id сессии для `stopSession`. */
  startSampler: (id: string, member: string | null, onEvent: (e: SamplerEvent) => void) => {
    const channel = new Channel<SamplerEvent>();
    channel.onmessage = onEvent;
    return call<string>("start_sampler", { id, member, channel });
  },
  stopSession: (sessionId: string) => call<boolean>("stop_session", { sessionId }),
```

`src/state/workspace.ts`: вариант `{ kind: "diag"; connectionId: string; tool: DiagTool; member?: string | null }` (комментарий: «инструмент диагностики; member — закреплённый узел, нет — за лидером»), `tabId` — `` `diag:${spec.connectionId}:${spec.tool}` ``. `Layout.tsx`: `case "diag": return i18n.t("tabs.diag", { tool: i18n.t(`diag.tools.${tab.tool}`), conn });`. `TabContent.tsx`: `case "diag": return <DiagTab connectionId={tab.connectionId} tool={tab.tool} member={tab.member ?? null} active={active} tabId={tab.id} />;`. `useConnectionEvents.ts`: `PG_QUERY_KEYS = ["catalog", "memberFacts", "diag"]`.

- [ ] **Step 4: Опрос, тексты находок, длительность**

`src/lib/useDiagnostic.ts`:

```ts
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { api, toAppError } from "../api/client";
import type { DiagTool, RustTool } from "../api/types";

export const DIAG_INTERVALS_S = [2, 5, 10, 30] as const;

export const rustTool = (tool: DiagTool): RustTool => (tool === "idleTx" ? "sessions" : tool);

/** Patroni опрашивается раз в 10 с, как топология; остальное — раз в 5 с (спец. P2 §3.6). */
export const defaultIntervalS = (tool: DiagTool) => (tool === "health" || tool === "failover" ? 10 : 5);

/** Скрытая вкладка и пауза не опрашивают; после отказа во входе — тоже (повтор блокирует учётную запись). */
export function diagRefetchInterval(active: boolean, intervalS: number | null, error: unknown): number | false {
  if (!active || intervalS === null) return false;
  if (error != null && toAppError(error).code === "AUTH_FAILED") return false;
  return intervalS * 1000;
}

export function useDiagnostic(
  connectionId: string,
  tool: DiagTool,
  member: string | null,
  opts: { active: boolean; intervalS: number | null; windowS?: number; live: boolean },
) {
  const rt = rustTool(tool);
  return useQuery({
    queryKey: ["diag", connectionId, rt, member ?? "", opts.windowS ?? 0],
    queryFn: () => api.runDiagnostic(connectionId, { tool: rt, member, ...(opts.windowS ? { windowS: opts.windowS } : {}) }),
    enabled: opts.live,
    // Прошлый ответ остаётся на экране, пока идёт новый и если новый упал («устарело»).
    placeholderData: keepPreviousData,
    refetchInterval: (q) => diagRefetchInterval(opts.active, opts.intervalS, q.state.error),
  });
}
```

`src/lib/format.ts`:

```ts
/** Длительность в секундах — две старшие единицы («1 ч 2 мин», «42 с»); null — «—». */
export function formatDuration(seconds: number | null): string {
  if (seconds === null) return "—";
  const s = Math.max(0, Math.floor(seconds));
  const units: [number, string][] = [[86_400, "d"], [3_600, "h"], [60, "m"], [1, "s"]];
  const parts: string[] = [];
  let rest = s;
  for (const [size, key] of units) {
    const n = Math.floor(rest / size);
    if (n > 0 || (key === "s" && parts.length === 0)) parts.push(i18n.t(`diag.duration.${key}`, { n }));
    rest -= n * size;
    if (parts.length === 2) break;
  }
  return parts.join(" ");
}
```

(импорт `i18n` из `../i18n`; ключи `diag.duration.d|h|m|s`.)

`src/components/diag/findingText.ts`:

```ts
import type { Finding } from "../../api/types";
import { i18n } from "../../i18n";
import { formatBytes, formatCount, formatDuration, formatPercent } from "../../lib/format";

/**
 * Параметры текста находки: значения форматируются по суффиксу имени (`…S` — длительность, `…Bytes` — байты,
 * `…Pct` — проценты, числа — с разделителями), добавляются узел, объект (`name` или `pid`), подпись вида держателя
 * горизонта (`kindLabel`) и общий «лаг» из байт и времени.
 */
export function findingParams(f: Finding): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(f.params)) {
    if (v === null) continue;
    if (typeof v === "number" && k.endsWith("Bytes")) out[k] = formatBytes(v);
    else if (typeof v === "number" && k.endsWith("Pct")) out[k] = formatPercent(v);
    else if (typeof v === "number" && k.endsWith("S")) out[k] = formatDuration(v);
    else if (typeof v === "number") out[k] = formatCount(v);
    else out[k] = String(v);
  }
  if (f.member) out.member = f.member;
  if (f.target?.kind === "session") out.pid = String(f.target.pid);
  else if (f.target && !("name" in out)) out.name = f.target.name;
  if (typeof f.params.kind === "string") out.kindLabel = i18n.t(`diag.horizon.kinds.${f.params.kind}`);
  if ("lagBytes" in f.params || "delayS" in f.params) out.lag = [out.lagBytes, out.delayS].filter(Boolean).join(", ");
  return out;
}

export const findingTitle = (f: Finding) => i18n.t(`findings.${f.id}.title`, findingParams(f));
export const findingFix = (f: Finding) => i18n.t(`findings.${f.id}.fix`, findingParams(f));
```

- [ ] **Step 5: Вкладка, шапка, находки, дерево**

`DiagTab.tsx`:
- `useTopology(connectionId)` → живо ли подключение (`connected | degraded`) и участники; `useState` для выбранного узла (`member` из спеки — начальное значение; `null` — «за лидером»), интервала (`defaultIntervalS(tool)`; `null` — пауза), окна (`waits`: 900).
- `const q = useDiagnostic(connectionId, tool, selected, { active, intervalS, windowS, live })`.
- Смена узла «за лидером»: `useRef` прошлого `report.member`; если `report.followsLeader` и узел изменился — строка `diag.header.nodeChanged`.
- Подключение закрыто — `diag.closed` и кнопка `diag.connect` (вызывает `api.connect`, затем `refreshAfterConnect`, как обзор).
- Порядок: `<DiagHeader …/>`, `ErrorBanner` (если `q.error`), `<FindingsPanel connectionId findings={report.findings} />`, список `unavailable` (`<p className="muted">` с `tr(u.reason)` и узлом, если есть), `<ToolBody tool report connectionId />`.
- `ToolBody`: `switch (report.data.kind)`; в этой задаче — `<pre className="diag-raw">{JSON.stringify(report.data, null, 2)}</pre>` для всех видов; задачи 11–13 заменяют ветки своими видами.

`DiagHeader.tsx`: `<h2>{t(`diag.tools.${tool}`)}</h2>`; `select` «Узел» (`aria-label`) — `diag.header.followLeader` с именем узла из последнего отчёта (или без имени, пока отчёта нет) и участники `имя — роль` (`roleLabel`); у `server` и инструментов кластера (`pulse`, `failover`, `health`) — нет; `select` «Обновление» — `2 с`, `5 с`, `10 с`, `30 с`, `Пауза`; кнопка «Обновить» (`q.refetch()`); `diag.header.updated` с `formatTimestamp(observedAtMs)`; метка `diag.header.stale` при `q.isError && q.data`.

`FindingsPanel.tsx`: `<section aria-labelledby>` с заголовком `diag.findings.title` и кнопкой сворачивания; `<ul>` — по находке `<li className={`finding sev-${f.severity}`}>`: метка строгости текстом (`diag.severity.*`), `findingTitle(f)`, узел (если у отчёта кластера), `<details><summary>{t("diag.findings.fix")}</summary>{findingFix(f)}</details>`, `CopyButton` для `params.command` (`label` — `diag.findings.copy`), кнопки ссылок `openTool` (`diag.findings.open` с названием инструмента) → `useWorkspace.open({ kind: "diag", connectionId, tool: link.tool, member: link.member })`. Пусто — `diag.findings.none`.

`DiagnosticsNodes.tsx` (`{ connectionId, kind }`): `TreeGroup` «Диагностика» с кнопками-ссылками по `DIAG_TOOLS_FOR(kind)` = `["pulse", "sessions", "idleTx", "waits", "locks", "horizon", "replication", "slots"]` и для `patroni` — ещё `"failover", "health"`. В `CatalogNodes` — после группы «Роли» (`{topo && <DiagnosticsNodes connectionId={connection.id} kind={connection.kind} />}`). В меню подключения (`ConnectionNode`) после «Обзор» — `{ label: t("sidebar.pulse"), onSelect: () => open({ kind: "diag", connectionId: c.id, tool: "pulse" }) }`.

`styles.css`: `.finding`, `.sev-critical` / `.sev-warning` / `.sev-info` (цвет полосы слева и метка строгости), `.diag-head` (шапка в одну строку с переносом), `.diag-raw`.

- [ ] **Step 6: Словари**

`src/locales/en.json` и `ru.json`; ключи, где ru совпадает с en, — в `docs/i18n/same-as-en.json` (`tabs.diag`, `diag.tools.idleTx`, `diag.horizon.kinds.preparedXact`, `diag.unavailable.version`):
- `tabs.diag`: «{{tool}} — {{conn}}» (ru — то же, в `same-as-en`); `sidebar.diagnostics`: «Diagnostics» / «Диагностика»; `sidebar.pulse`: «Pulse» / «Пульс»;
- `diag.tools`: pulse «Pulse» / «Пульс», sessions «Sessions» / «Сессии», idleTx «Idle in transaction» / «Idle in transaction», waits «Wait profile» / «Профиль ожиданий», locks «Locks» / «Блокировки», horizon «xmin horizon» / «Горизонт xmin», replication «Replicas» / «Реплики», slots «Replication slots» / «Слоты репликации», failover «Failover readiness» / «Готовность к failover», health «Health matrix» / «Матрица health»;
- `diag.header`: node «Node» / «Узел», followLeader «Follow the leader ({{member}})» / «За лидером ({{member}})», followLeaderPending «Follow the leader» / «За лидером», interval «Refresh» / «Обновление», seconds «{{n}} s» / «{{n}} с», pause «Pause» / «Пауза», refresh «Refresh» / «Обновить», updated «Updated {{time}}» / «Обновлено {{time}}», stale «stale» / «устарело», nodeChanged «Node changed: {{from}} → {{to}}» / «Узел сменился: {{from}} → {{to}}»;
- `diag.closed` «The connection is closed» / «Подключение закрыто»; `diag.connect` «Connect» / «Подключиться»;
- `diag.findings`: title «Findings» / «Находки», none «No problems found» / «Проблем не найдено», fix «What to do» / «Что сделать», open «Open “{{tool}}”» / «Открыть «{{tool}}»», copy «Copy command» / «Скопировать команду», collapse «Collapse findings» / «Свернуть находки»;
- `diag.severity`: info «Info» / «Сведения», warning «Warning» / «Внимание», critical «Critical» / «Критично»;
- `diag.duration`: d «{{n}} d» / «{{n}} д», h «{{n}} h» / «{{n}} ч», m «{{n}} min» / «{{n}} мин», s «{{n}} s» / «{{n}} с»;
- `diag.horizon.kinds`: session «session» / «сессия», preparedXact «prepared transaction» / «prepared transaction», slot «slot» / «слот», standbyFeedback «replica feedback» / «feedback реплики»;
- `findings.<id>.title` / `findings.<id>.fix` — таблица:

| id | title en / ru | fix en / ru |
|---|---|---|
| `connectionsSaturated` | Connections {{usedPct}} used ({{used}} of {{available}}) / Соединения заняты на {{usedPct}} ({{used}} из {{available}}) | See who holds connections in Sessions grouped by application; use a pooler or smaller application pools. / Посмотрите, кто держит соединения, в «Сессиях» с группировкой по приложению; нужен пулер или меньшие пулы приложений. |
| `idleInTransaction` | Idle in transaction for {{maxS}} / Простой в транзакции {{maxS}} | Such transactions hold locks and the xmin horizon: fix the missing COMMIT in the application, set idle_in_transaction_session_timeout. / Такие транзакции держат блокировки и горизонт xmin: найдите в приложении пропущенный COMMIT, задайте idle_in_transaction_session_timeout. |
| `longQuery` | A query has been running for {{maxS}} / Запрос выполняется {{maxS}} | Check its plan and locks; cancel it if it blocks others. / Проверьте план и блокировки запроса; отмените его, если он мешает другим. |
| `longTransaction` | A transaction has been open for {{maxS}} / Транзакция открыта {{maxS}} | Long transactions hold the xmin horizon and bloat tables: find the session and the application behind it. / Долгие транзакции держат горизонт xmin и раздувают таблицы: найдите сессию и приложение. |
| `xidAge` | Transaction ID age is {{agePct}} of the wraparound limit / Возраст XID — {{agePct}} от предела wraparound | Find what holds vacuum (xmin horizon) and let autovacuum freeze the oldest tables. / Найдите, что держит vacuum (горизонт xmin), и дайте autovacuum заморозить самые старые таблицы. |
| `lockWait` | {{count}} sessions wait for locks, the longest {{maxS}} / Ждут блокировку: {{count}}, дольше всех — {{maxS}} | Open Locks to find the root blocker. / Откройте «Блокировки» и найдите корневого блокирующего. |
| `walDirSize` | pg_wal is {{walDirBytes}} with max_wal_size {{maxWalSizeBytes}} / pg_wal — {{walDirBytes}} при max_wal_size {{maxWalSizeBytes}} | WAL is usually held by an inactive replication slot or failing archiving: check Replication slots. / WAL обычно держит неактивный слот репликации или сбой архивирования: проверьте «Слоты репликации». |
| `deadlocks` | Deadlocks in the interval: {{count}} / Взаимных блокировок за интервал: {{count}} | Details are only in the server log; make the application take locks in one order. / Подробности — только в логе сервера; приложение должно брать блокировки в одном порядке. |
| `lockShare` | Lock waits take {{sharePct}} of active time / Ожидание блокировок — {{sharePct}} активного времени | Open Locks to find who blocks. / Откройте «Блокировки» и найдите, кто блокирует. |
| `rootBlocker` | Session {{pid}} blocks {{count}} sessions, the longest wait {{maxS}} / Сессия {{pid}} блокирует сессий: {{count}}, дольше всех ждут {{maxS}} | Look at its transaction; cancelling or ending it releases the chain. / Посмотрите на её транзакцию; отмена или завершение освобождает цепочку. |
| `ddlQueue` | DDL on {{relation}} is queued and blocks {{count}} sessions / DDL на {{relation}} стоит в очереди и блокирует сессий: {{count}} | Cancel the DDL to release the queue, or the query it waits for; run migrations with lock_timeout. / Отмените DDL, чтобы снять очередь, или запрос, которого он ждёт; запускайте миграции с lock_timeout. |
| `xminHorizon` | The xmin horizon is held by {{kindLabel}} {{name}} ({{xminAge}} transactions) / Горизонт xmin держит {{kindLabel}} {{name}} (транзакций: {{xminAge}}) | Vacuum cannot clean rows newer than the horizon: end the transaction, drop or fix the slot, or roll back the prepared transaction. / Vacuum не чистит строки новее горизонта: завершите транзакцию, удалите или почините слот, откатите prepared transaction. |
| `replicaLag` | Replica lag: {{lag}} / Лаг реплики: {{lag}} | Check the replica's load, network and replay conflicts in Replicas. / Проверьте нагрузку реплики, сеть и конфликты применения в «Репликах». |
| `replicaNotStreaming` | Replicas not streaming: {{count}} (state {{state}}) / Реплик не в потоковом режиме: {{count}} (состояние {{state}}) | A replica in catchup or backup is not a failover candidate yet. / Реплика в catchup или backup ещё не кандидат на failover. |
| `replicaReplyStale` | Replicas silent for {{maxS}}: {{count}} / Реплик без ответа {{maxS}}: {{count}} | The replica stopped confirming WAL: check its network and wal_receiver_status_interval. / Реплика перестала подтверждать WAL: проверьте сеть и wal_receiver_status_interval. |
| `syncStandbyMissing` | synchronous_standby_names is set but no synchronous replica is connected / synchronous_standby_names задан, но синхронной реплики нет | Commits wait for a synchronous replica: bring one back or change synchronous_standby_names. / Коммиты ждут синхронную реплику: верните её или измените synchronous_standby_names. |
| `walReceiverDown` | The WAL receiver is not streaming / WAL receiver не в потоковом режиме | The replica is not receiving WAL from its upstream: check primary_conninfo, the slot and the network. / Реплика не получает WAL: проверьте primary_conninfo, слот и сеть. |
| `replayPaused` | WAL replay is paused / Применение WAL на паузе | Resume it with pg_wal_replay_resume() if the pause is not intentional. / Если пауза не нарочная, снимите её: pg_wal_replay_resume(). |
| `recoveryConflicts` | New recovery conflicts: {{count}} / Новых конфликтов recovery: {{count}} | Queries on the replica are cancelled to apply WAL: consider hot_standby_feedback or max_standby_streaming_delay. / Запросы на реплике отменяются ради применения WAL: подумайте о hot_standby_feedback или max_standby_streaming_delay. |
| `recoveryConflictsTotal` | Recovery conflicts since the statistics reset: {{count}} / Конфликтов recovery с момента сброса статистики: {{count}} | No new conflicts now; watch the counter. / Новых конфликтов сейчас нет; следите за счётчиком. |
| `slotRetainsWal` | Inactive slot {{name}} retains {{retainedBytes}} of WAL / Неактивный слот {{name}} держит {{retainedBytes}} WAL | If its consumer is gone, drop the slot; otherwise bring the consumer back; set max_slot_wal_keep_size. / Если потребителя нет, удалите слот; иначе верните потребителя; задайте max_slot_wal_keep_size. |
| `slotLost` | Slot {{name}} lost the WAL it needs / Слот {{name}} потерял нужный ему WAL | The consumer cannot continue: drop the slot and resynchronize the consumer. / Потребитель не продолжит: удалите слот и пересинхронизируйте потребителя. |
| `subscriptionErrors` | New subscription errors: {{count}} / Новых ошибок подписок: {{count}} | Details are in the subscriber's server log. / Подробности — в логе сервера подписчика. |
| `failoverCandidates` | Failover candidates ready: {{count}} / Готовых кандидатов на failover: {{count}} | See the reasons per replica in Failover readiness. / Причины по каждой реплике — в «Готовности к failover». |
| `clusterPaused` | The cluster is paused: automatic failover is off / Кластер на паузе: автоматический failover выключен | Resume the cluster when maintenance is over. / Снимите паузу, когда обслуживание закончено. |
| `syncModeWithoutSync` | synchronous_mode is on but there is no synchronous replica / synchronous_mode включён, синхронной реплики нет | Failover is impossible and with synchronous_mode_strict writes stop: bring a replica back to sync. / Failover невозможен, а при synchronous_mode_strict встаёт запись: верните реплику в sync. |
| `patroniTiming` | ttl {{ttlS}} is less than loop_wait {{loopWaitS}} + 2 × retry_timeout {{retryTimeoutS}} / ttl {{ttlS}} меньше loop_wait {{loopWaitS}} + 2 × retry_timeout {{retryTimeoutS}} | The leader key may expire during a DCS hiccup: raise ttl (patronictl edit-config). / Ключ лидера может истечь при сбое DCS: увеличьте ttl (patronictl edit-config). |
| `healthPrimary` | /primary answers 200 on {{count}} members / /primary отвечает 200 у участников: {{count}} | A load balancer would send writes to the wrong place or nowhere: check the topology. / Балансировщик отправит запись не туда или никуда: проверьте топологию. |
| `healthUnhealthy` | {{member}}: /health does not answer 200 / {{member}}: /health не отвечает 200 | PostgreSQL on the member is not running. / PostgreSQL участника не работает. |
| `replicaOutOfPool` | {{member}} is a replica but /replica does not answer 200 / {{member}} — реплика, но /replica не отвечает 200 | The load balancer excludes it from reads: lag, the noloadbalance tag or a stopped replica. / Балансировщик исключил её из чтения: лаг, тег noloadbalance или реплика остановлена. |
| `cacheHitLow` | Cache hit in the interval: {{hitPct}} / Cache hit за интервал: {{hitPct}} | Reads go to disk: check heavy queries and shared_buffers. / Чтения идут с диска: проверьте тяжёлые запросы и shared_buffers. |
| `tempFiles` | Temporary files in the interval: {{count}} / Временных файлов за интервал: {{count}} | Sorts and hashes spill to disk: look at work_mem for the heavy queries. / Сортировки и хэши уходят на диск: посмотрите work_mem для тяжёлых запросов. |
| `noIdleTimeout` | idle_in_transaction_session_timeout is off / idle_in_transaction_session_timeout выключен | Set it for the application role: {{command}} / Задайте его роли приложения: {{command}} |
| `hiddenQueries` | Texts of {{count}} sessions are hidden / Скрыты тексты сессий: {{count}} | Grant pg_monitor to the diagnostics role. / Выдайте роли диагностики pg_monitor. |
| `lockCycle` | {{count}} sessions wait for each other / Сессий ждут друг друга: {{count}} | The deadlock detector will resolve it within deadlock_timeout; details are in the server log. / Deadlock detector разрешит это за deadlock_timeout; подробности — в логе сервера. |

- [ ] **Step 7: Тесты проходят**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src docs/i18n
git commit -m "feat(ui): вкладка инструмента диагностики — шапка, опрос, находки, пункт «Диагностика» в дереве

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Фронтенд — пульс, сессии и idle in transaction, блокировки

**Files:**
- Create: `src/components/diag/PulseView.tsx` (+ `.test.tsx`), `src/components/diag/SessionsView.tsx` (+ `.test.tsx`), `src/components/diag/LocksView.tsx` (+ `.test.tsx`)
- Modify: `src/components/diag/DiagTab.tsx` (ветки `ToolBody`), `src/locales/{en,ru}.json`, `src/styles.css`
- Test: vitest

**Interfaces:**
- Consumes: типы и фикстура `diagReport` (10); `formatDuration`, `formatBytes`, `formatPercent`, `formatCount`; `CopyButton`, `ErrorBanner`, `Hint`; `useVirtualizer`.
- Produces: `PulseView({ connectionId, report })`, `SessionsView({ report, preset: "all" | "active" | "idleTx" | "waiting" })`, `LocksView({ report })`; `PULSE_COLUMNS` (показатель → инструмент перехода); ключи `diag.pulse.*`, `diag.sessions.*`, `diag.locks.*`.

- [ ] **Step 1: Падающие тесты**

`PulseView.test.tsx`:

```tsx
  it("строка на узел, клетка окрашена находкой, клик открывает инструмент на узле", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    const report = diagReport(
      { kind: "pulse", nodes: [
        { member: "pg-1", role: "primary", reachable: true, error: null, facts: pulseFacts({ used: 96, available: 100, longestIdleInTransactionS: 400 }), rates: pulseRates({ tps: 1234.5 }) },
        { member: "pg-2", role: "replica", reachable: false, error: { code: "NODE_UNAVAILABLE", key: "errors.topology.noNode", message: "x", retryable: true }, facts: null, rates: null },
      ] },
      { member: null, followsLeader: false, findings: [{ id: "connectionsSaturated", severity: "critical", member: "pg-1", target: null, params: { usedPct: 96, used: 96, available: 100 }, links: [] }] },
    );
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const row = screen.getByRole("row", { name: /pg-1/ });
    const cell = within(row).getByRole("button", { name: /96 \/ 100/ });
    expect(cell.closest("td")).toHaveClass("sev-critical");
    expect(within(row).getByText(/1\s234,5/)).toBeInTheDocument(); // \s — и неразрывный пробел разрядов
    await user.click(cell);
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", tool: "sessions", member: "pg-1" });
    expect(within(screen.getByRole("row", { name: /pg-2/ })).getByText("Узел недоступен")).toBeInTheDocument();
  });
```

(`pulseFacts(patch)`, `pulseRates(patch)` — фикстуры с нулями и `null`.)

`SessionsView.test.tsx`:

```tsx
  const rows = [
    sessionRow({ pid: 1, state: "active", application: "api", queryAgeS: 5, query: "select 1" }),
    sessionRow({ pid: 2, state: "idle in transaction", application: "api", stateAgeS: 900, locksHeld: 3, xminAge: 10, query: "update t" }),
    sessionRow({ pid: 3, state: "idle", application: "worker" }),
    sessionRow({ pid: 4, state: "active", application: "glafira-pg", glafira: true }),
    sessionRow({ pid: 5, state: "active", waitEventType: "Lock", waitEvent: "transactionid", blockedBy: [2], application: "api" }),
  ];

  /** Строка списка по pid: первая ячейка строки — pid (имена строк содержат и длительности, поиск по имени неоднозначен). */
  const rowOf = (pid: number) => screen.getAllByRole("row").find((r) => within(r).queryAllByRole("cell")[0]?.textContent === String(pid))!;

  it("пресеты, скрытие сессий Glafira и группировка по приложению", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionsView report={diagReport({ kind: "sessions", rows, settings: { idleInTransactionSessionTimeoutMs: 0, trackActivityQuerySize: 4096 } })} preset="all" />);
    // Сессия Glafira скрыта по умолчанию: строк данных — четыре.
    expect(screen.getAllByRole("row").length - 1).toBe(4);
    await user.click(screen.getByRole("tab", { name: "Ждущие" }));
    expect(rowOf(5)).toHaveTextContent("Lock:transactionid");
    expect(rowOf(5)).toHaveTextContent("2");
    await user.selectOptions(screen.getByRole("combobox", { name: "Группировать" }), "application");
    const api = screen.getByRole("row", { name: /^api/ });
    expect(api).toHaveTextContent("3");
  });

  it("пресет idle in transaction: сортировка по простою, колонки блокировок и xmin, настройка таймаута", async () => {
    renderWithClient(<SessionsView report={diagReport({ kind: "sessions", rows, settings: { idleInTransactionSessionTimeoutMs: 0, trackActivityQuerySize: 4096 } })} preset="idleTx" />);
    expect(screen.getByRole("tab", { name: "Idle in transaction", selected: true })).toBeInTheDocument();
    expect(rowOf(2)).toHaveTextContent("15 мин");
    expect(screen.getByRole("columnheader", { name: "Держит блокировок" })).toBeInTheDocument();
    expect(screen.getByText(/idle_in_transaction_session_timeout = 0/)).toBeInTheDocument();
  });

  it("строка открывает панель с полным текстом запроса и копированием", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionsView report={diagReport({ kind: "sessions", rows, settings: { idleInTransactionSessionTimeoutMs: null, trackActivityQuerySize: null } })} preset="all" />);
    await user.click(rowOf(2));
    expect(screen.getByRole("region", { name: "Сессия 2" })).toHaveTextContent("update t");
    expect(screen.getByRole("button", { name: "Копировать запрос" })).toBeInTheDocument();
  });
```

`LocksView.test.tsx`:

```tsx
  it("DDL в очереди карточкой сверху, дерево от корня с числом жертв, ожидание ≈ на PG 13", () => {
    const s = (pid: number, query: string) => ({ pid, backendStartUs: pid, user: "app", application: "api", database: "orders", state: "active", query, xactAgeS: 100, queryAgeS: 10 });
    const lock = (mode: string, waitS: number, approx = false) => ({ locktype: "relation", mode, relation: "shop.orders", relationOid: 1, relationDatabase: "orders", waitS, approx });
    const data = {
      kind: "locks" as const,
      roots: [{ session: s(1, "update shop.orders"), lock: null, alsoBlockedBy: [], victims: 2, children: [
        { session: s(2, "alter table shop.orders add column x int"), lock: lock("AccessExclusiveLock", 40), alsoBlockedBy: [], victims: 1, children: [
          { session: s(3, "select count(*) from shop.orders"), lock: lock("AccessShareLock", 30, true), alsoBlockedBy: [], victims: 0, children: [] },
        ] },
      ] }],
      cycles: [],
      ddlQueue: [{ session: s(2, "alter table shop.orders add column x int"), waitsFor: 1, waitS: 40, queued: 1, relation: "shop.orders", mode: "AccessExclusiveLock" }],
    };
    renderWithClient(<LocksView report={diagReport(data)} />);
    const card = screen.getByRole("region", { name: "DDL в очереди" });
    expect(card).toHaveTextContent("alter table shop.orders");
    expect(card).toHaveTextContent("ждёт сессию 1");
    const tree = screen.getByRole("tree", { name: "Дерево блокировок" });
    expect(within(tree).getByRole("treeitem", { name: /сессия 1/i })).toHaveTextContent("жертв: 2");
    expect(within(tree).getByText("≈ 30 с")).toBeInTheDocument();
  });
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm vitest run src/components/diag`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`PulseView.tsx`: `<table aria-label={t("diag.pulse.table")}>`; колонки `PULSE_COLUMNS` — `{ key, label, tool, findings: string[], value(node) }`:

| key | Значение | Находки клетки | Переход |
|---|---|---|---|
| `connections` | `used / available` и доля | `connectionsSaturated` | `sessions` |
| `tps` | TPS (`formatCount` с 1 знаком), commit/rollback в подсказке | `deadlocks` | `locks` |
| `cache` | cache hit | `cacheHitLow` | `sessions` |
| `wal` | WAL в секунду (`formatBytes`/с) | — | `replication` |
| `xact` | самая долгая транзакция | `longTransaction` | `sessions` |
| `query` | самый долгий запрос | `longQuery` | `sessions` |
| `idle` | самый долгий idle in transaction | `idleInTransaction` | `idleTx` |
| `locks` | ждут блокировку и дольше всех | `lockWait` | `locks` |
| `xid` | возраст XID и доля | `xidAge` | `horizon` |
| `walDir` | размер `pg_wal` | `walDirSize`, `tempFiles` | `slots` |
| `lag` | лаг и задержка replay реплики | `replicaLag` | `replication` |

Клетка — кнопка внутри `<td className={sev-<худшая строгость находок узла по колонке>}>`; узел недоступен — строка с `diag.pulse.unreachable` и `ErrorBanner` в подсказке/строке; «…» — скорость ещё не известна (первый вызов).

`SessionsView.tsx`: `role="tablist"` пресетов (`diag.sessions.presets.all|active|idleTx|waiting`), поле фильтра (подстрока по pid, роли, приложению, клиенту, БД, запросу), `select` «Группировать» (`none`, `application`, `user`, `client`, `database`) — таблица групп: имя, всего, active, idle, idle in transaction, прочие; флажок «Скрыть сессии Glafira» (включён). Список — `useVirtualizer` (высота строки как в `ObjectList`), строки — `role="row"`, колонки: pid, состояние, ожидание (`тип:событие`), запрос (длительность), транзакция (длительность), роль, приложение, клиент, БД, ждёт (`blockedBy`), текст (первые 120 символов). Пресет `idleTx`: фильтр `state` начинается с `idle in transaction`, сортировка по `stateAgeS` убыванию, дополнительные колонки «Простой», «Держит блокировок», «Возраст xmin», строка настройки `idle_in_transaction_session_timeout = N` (`Hint` `hints.diag.idleTimeout`). Клик по строке — панель `<section role="region" aria-label={t("diag.sessions.panel", { pid })}>` с полями сессии, полным `query` (`queryTruncated` — пометка «текст обрезан до 16 КБ»), `CopyButton` (`diag.sessions.copyQuery`).

`LocksView.tsx`: сверху — `<section role="region" aria-label={t("diag.locks.ddlQueue")}>` с карточкой на элемент очереди: запрос DDL, режим, отношение, «ждёт сессию N {{waitS}}», «за ней сессий: queued»; дерево — `<ul role="tree" aria-label={t("diag.locks.tree")}>`, узел — `role="treeitem"` с `aria-expanded` и кнопкой раскрытия (корни раскрыты): «Сессия {{pid}}», роль, приложение, состояние, длительность транзакции, запрос (обрезанный), запрошенная блокировка («{{mode}} на {{relation}}», для другой БД — `diag.locks.otherDb` «oid {{oid}} в БД {{db}}»), ожидание одним элементом (`≈ 30 с`, если `approx`, иначе `30 с`), «жертв: N», «ещё ждёт: pids» (`alsoBlockedBy`); пусто — `diag.locks.none` «Никто не ждёт блокировок»; циклы — список групп pid с `diag.locks.cycle`; подпись `diag.locks.logNote` «Подробности взаимных блокировок есть только в логе сервера».

Словари — ключи `diag.pulse.*` (table «Pulse by node» / «Пульс по узлам», колонки, unreachable «Node unavailable» / «Узел недоступен», ratePending «…»), `diag.sessions.*` (presets, filter, groupBy и варианты, hideGlafira, колонки, panel «Session {{pid}}» / «Сессия {{pid}}», copyQuery «Copy query» / «Копировать запрос», truncated, timeoutSetting «idle_in_transaction_session_timeout = {{value}}»), `diag.locks.*` (ddlQueue «DDL in the queue» / «DDL в очереди», tree «Lock tree» / «Дерево блокировок», session «Session {{pid}}» / «Сессия {{pid}}», waitsFor «waits for session {{pid}} {{waitS}}» / «ждёт сессию {{pid}} {{waitS}}», queued «sessions behind it: {{count}}» / «за ней сессий: {{count}}», victims «victims: {{count}}» / «жертв: {{count}}», alsoBlockedBy, otherDb, none, cycle, logNote), подсказка `hints.diag.idleTimeout` (en ≤ 200 символов: «Ends sessions idle in a transaction longer than this. Set it per application role; 0 — off.»).

- [ ] **Step 4: Тесты проходят**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat(ui): пульс по узлам, сессии с пресетами и группировкой, дерево блокировок и DDL в очереди

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Фронтенд — горизонт, реплики, слоты, Patroni; запланированные действия в обзоре

**Files:**
- Create: `src/components/diag/HorizonView.tsx`, `src/components/diag/ReplicationView.tsx`, `src/components/diag/SlotsView.tsx`, `src/components/diag/FailoverView.tsx`, `src/components/diag/HealthView.tsx` (+ `.test.tsx` у каждого)
- Modify: `src/components/diag/DiagTab.tsx`, `src/components/overview/Overview.tsx` (+ `Overview.test.tsx`), `src/locales/{en,ru}.json`, `docs/i18n/same-as-en.json`, `src/styles.css`
- Test: vitest

**Interfaces:**
- Consumes: типы (10), `formatDuration`, `formatBytes`, `formatTimestamp`, `useWorkspace`.
- Produces: `HorizonView`, `ReplicationView`, `SlotsView`, `FailoverView`, `HealthView` (`{ connectionId, report }`); баннер `overview.scheduledSwitchover` и пометка `overview.scheduledRestart`; ключи `diag.horizon.*`, `diag.replication.*`, `diag.slots.*`, `diag.failover.*`, `diag.health.*`.

- [ ] **Step 1: Падающие тесты**

`HorizonView.test.tsx`:

```tsx
  it("держатели по возрасту, вид держателя, переход к реплике feedback", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    const holders = [
      holder({ kind: "standbyFeedback", name: "walreceiver", member: "pg-2", xminAge: 500 }),
      holder({ kind: "preparedXact", name: "it", gid: "it", xminAge: 100, durationS: 4000 }),
      holder({ kind: "slot", name: "dead", catalog: true, xminAge: 10 }),
    ];
    renderWithClient(<HorizonView connectionId="c" report={diagReport({ kind: "horizon", holders, hotStandbyFeedback: false })} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows.map((r) => r.cells[0].textContent)).toEqual(["feedback реплики", "prepared transaction", "слот (catalog_xmin)"]);
    await user.click(within(rows[0]).getByRole("button", { name: "Открыть горизонт на pg-2" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ tool: "horizon", member: "pg-2" });
    expect(within(rows[1]).getByText("ROLLBACK PREPARED 'it';")).toBeInTheDocument();
  });
```

`ReplicationView.test.tsx`:

```tsx
  it("на primary — отправители с лагами, на реплике — карточка приёмника и пауза", () => {
    const { rerender } = renderWithClient(<ReplicationView connectionId="c" report={diagReport({ kind: "replication", ...replication({ senders: [sender({ application: "pg-2", member: "pg-2", replayLagBytes: 20 * 1024 * 1024, replayLagS: 3 })] }) })} />);
    expect(screen.getByRole("row", { name: /pg-2/ })).toHaveTextContent("20,0 MB");
    rerender(<ReplicationView connectionId="c" report={diagReport({ kind: "replication", ...replication({ inRecovery: true, receiver: { status: "streaming", senderHost: "pg-1", senderPort: 5432, slotName: "pg_2", receivedTli: 3, lastMsgAgeS: 1, latestEndAgeS: 1 }, replay: { receiveReplayGapBytes: 0, replayDelayS: 0, pause: "paused" } }) }, { inRecovery: true })} />);
    const card = screen.getByRole("region", { name: "Приёмник WAL" });
    expect(card).toHaveTextContent("pg-1:5432");
    expect(card).toHaveTextContent("на паузе");
  });
```

`SlotsView.test.tsx`:

```tsx
  it("слоты с удерживаемым WAL и отметкой Patroni, подписки", () => {
    renderWithClient(<SlotsView connectionId="c" report={diagReport({ kind: "slots", slots: [slot({ name: "patroni2", active: true, patroni: "member" }), slot({ name: "dead", active: false, retainedBytes: 2 * 1024 ** 3, walStatus: "extended" })], subscriptions: [subscription({ name: "lab_sub", pid: 77, applyErrorCount: 0 })], maxSlotWalKeepSizeMb: -1, subscriptionErrorsGrowth: null })} />);
    expect(screen.getByRole("row", { name: /patroni2/ })).toHaveTextContent("участник Patroni");
    expect(screen.getByRole("row", { name: /dead/ })).toHaveTextContent("2,0 GB");
    expect(screen.getByText(/max_slot_wal_keep_size: без ограничения/)).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Подписки" })).toHaveTextContent("lab_sub");
  });
```

`FailoverView.test.tsx`:

```tsx
  it("вердикт по кандидату с причинами и сводка конфигурации", () => {
    const data = { kind: "failover" as const, paused: false, readyCount: 1,
      config: { maximumLagOnFailover: 1048576, synchronousMode: "off" as const, synchronousModeStrict: false, synchronousNodeCount: null, ttl: 30, loopWait: 10, retryTimeout: 10, permanentSlots: [] },
      candidates: [{ member: "pg-2", ready: true, reasons: [], lagBytes: 0, pendingRestart: false }, { member: "pg-3", ready: false, reasons: ["lagTooHigh", "noFailoverTag"], lagBytes: 5000000, pendingRestart: true }] };
    renderWithClient(<FailoverView connectionId="c" report={diagReport(data, { member: null })} />);
    expect(screen.getByRole("row", { name: /pg-2/ })).toHaveTextContent("готов");
    const bad = screen.getByRole("row", { name: /pg-3/ });
    expect(bad).toHaveTextContent("лаг больше maximum_lag_on_failover");
    expect(bad).toHaveTextContent("тег nofailover");
    expect(bad).toHaveTextContent("ждёт рестарта");
    expect(screen.getByText(/maximum_lag_on_failover: 1,0 MB/)).toBeInTheDocument();
  });
```

`HealthView.test.tsx`:

```tsx
  it("сетка «участники × адреса» с кодами и ошибкой", () => {
    const cell = (endpoint: string, status: number | null) => ({ endpoint, status, error: status === null ? { code: "PATRONI_ERROR", key: "errors.patroni.timeout", message: "x", retryable: false } : null });
    const data = { kind: "health" as const, endpoints: ["primary", "replica", "health"], members: [
      { member: "pg-1", apiUrl: "http://pg-1:8008", cells: [cell("primary", 200), cell("replica", 503), cell("health", 200)] },
      { member: "pg-2", apiUrl: "http://pg-2:8008", cells: [cell("primary", null), cell("replica", null), cell("health", null)] },
    ] };
    renderWithClient(<HealthView connectionId="c" report={diagReport(data, { member: null })} />);
    const grid = screen.getByRole("table", { name: "Матрица health" });
    expect(within(grid).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Участник", "/primary", "/replica", "/health"]);
    expect(within(within(grid).getByRole("row", { name: /pg-1/ })).getAllByRole("cell").map((c) => c.textContent)).toEqual(["pg-1", "200", "503", "200"]);
    expect(within(grid).getByRole("row", { name: /pg-2/ })).toHaveTextContent("нет ответа");
    expect(screen.getByText(/HAProxy/)).toBeInTheDocument();
  });
```

`Overview.test.tsx`:

```tsx
  it("запланированный switchover — баннер, запланированный restart — пометка участника", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({
      scheduledSwitchover: { at: "2026-10-25T02:00:00+00:00", from: "pg-1", to: "pg-2" },
      members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica", scheduledRestart: { at: "2026-10-25T03:00:00+00:00", postgresVersion: null } })],
    }) }));
    renderOverview();
    expect(await screen.findByText(/Запланирован switchover pg-1 → pg-2/)).toBeInTheDocument();
    expect(screen.getByRole("row", { name: /pg-2/ })).toHaveTextContent("Запланирован рестарт");
  });
```

(Фикстуры `holder`, `sender`, `replication`, `slot`, `subscription` — в `src/test/fixtures.ts`, со всеми полями по умолчанию; `renderOverview` — существующий помощник теста обзора или `renderWithClient(<Overview connectionId="c" />)`.)

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm vitest run src/components/diag src/components/overview`
Expected: FAIL.

- [ ] **Step 3: Реализация**

- `HorizonView`: таблица по `holders` (уже по убыванию возраста): вид (`diag.horizon.kinds.*`, у слота с `catalog` — «(catalog_xmin)»), объект (pid и роль/приложение; имя слота и активность; gid, владелец, БД; приложение и клиент реплики), возраст xmin (`formatCount`), длительность; у feedback с участником — кнопка `diag.horizon.openOn` «Open the horizon on {{member}}» / «Открыть горизонт на {{member}}» (вкладка `horizon` с этим `member`); у prepared — команда `ROLLBACK PREPARED …` (из находки или строится так же — `quoteLiteral` во фронтенде) и `CopyButton`; над таблицей — `hot_standby_feedback = on|off` узла.
- `ReplicationView`: таблица отправителей (участник или приложение, клиент, состояние, sync, лаги sent/write/flush/replay в байтах, replay во времени, ответ реплики давно, слот, возраст xmin); `synchronous_standby_names` и `synchronous_commit`; на узле в recovery — `<section role="region" aria-label={t("diag.replication.receiver")}>`: статус, `sender_host:sender_port`, слот, таймлайн, последнее сообщение, разрыв receive/replay, задержка replay, пауза (`diag.replication.pause.paused` «на паузе», `pauseRequested`, `notPaused`), конфликты (по видам и рост), `hot_standby_feedback`, `max_standby_streaming_delay`.
- `SlotsView`: таблица слотов (имя, тип, plugin, БД, активен/pid, `wal_status`, удерживаемый WAL, `safe_wal_size`, возраст xmin/catalog_xmin, неактивен с (`formatTimestamp`), причина инвалидации, `failover`/`synced`, отметка Patroni: `diag.slots.patroni.member` «участник Patroni», `permanent` «постоянный слот Patroni»); строка `max_slot_wal_keep_size` (`-1` — `diag.slots.unlimited` «без ограничения», иначе МБ → `formatBytes`); таблица подписок `aria-label` «Подписки».
- `FailoverView`: сводка (`paused`, `synchronous_mode` и `strict`, `maximum_lag_on_failover` — `formatBytes`, `ttl` / `loop_wait` / `retry_timeout`, постоянные слоты); таблица кандидатов: участник, вердикт (`diag.failover.ready` «готов» / `notReady` «не готов»), причины (`diag.failover.reasons.<reason>`: unreachable «недоступен», notStreaming «не streaming», lagTooHigh «лаг больше maximum_lag_on_failover», lagUnknown «лаг неизвестен», noFailoverTag «тег nofailover», zeroPriority «failover_priority = 0», notSynchronous «не синхронная реплика»), лаг, пометка `pendingRestart` «ждёт рестарта».
- `HealthView`: `<table aria-label={t("diag.tools.health")}>`; заголовки — «Участник» и `/<адрес>`; клетка — код с классом `ok` (200) / `bad` (иной) / `none` (`diag.health.noAnswer` «нет ответа», ошибка — в `title`); участник без `apiUrl` — `diag.health.noApi`; подпись `diag.health.note` («Так видит кластер балансировщик, например HAProxy с option httpchk GET /primary; параметры вида /replica?lag= не проверяются»).
- `Overview.tsx`: в шапке после бейджа паузы — `{topo?.scheduledSwitchover && <p className="warn">{t("overview.scheduledSwitchover", { at: formatTimestamp(Date.parse(s.at)), from: s.from ?? "—", to: s.to ?? "—" })}</p>}` («Scheduled switchover {{from}} → {{to}} at {{at}}» / «Запланирован switchover {{from}} → {{to}} на {{at}}»); в ячейке тегов участника — `{m.scheduledRestart && <div className="warn">{t("overview.scheduledRestart", { at: … })}</div>}` («Restart scheduled at {{at}}» / «Запланирован рестарт на {{at}}»).
- `DiagTab.tsx`: ветки `ToolBody` — новые виды.

- [ ] **Step 4: Тесты проходят**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src docs/i18n
git commit -m "feat(ui): горизонт xmin, реплики, слоты, готовность к failover, матрица health; запланированные действия в обзоре

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Фронтенд — профиль ожиданий и подписка на сэмплер

**Files:**
- Create: `src/lib/useSampler.ts` (+ `.test.ts`), `src/components/diag/WaitsView.tsx` (+ `.test.tsx`), `src/components/diag/WaitsChart.tsx` (+ `.test.tsx`)
- Modify: `src/components/diag/DiagTab.tsx` (окно для `waits`, ветка `ToolBody`), `src/components/diag/DiagHeader.tsx` (выбор окна 5 / 15 / 60 мин), `src/locales/{en,ru}.json`, `src/styles.css`
- Test: vitest

**Interfaces:**
- Consumes: `api.startSampler`, `api.stopSession`, `SamplerEvent`, `WaitsData` (10); `formatDuration`, `formatPercent`.
- Produces: `useSampler(connectionId, member: string | null, enabled: boolean) -> { live: { atMs, active, waiting } | null; error: AppError | null }`; `WaitsView({ connectionId, report })`; `WaitsChart({ data })` — SVG и табличная альтернатива; `WAIT_CLASS_ORDER`; ключи `diag.waits.*`.

- [ ] **Step 1: Падающие тесты**

`src/lib/useSampler.test.ts`:

```ts
vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { startSampler: vi.fn(), stopSession: vi.fn() } };
});

describe("useSampler", () => {
  beforeEach(() => vi.resetAllMocks());

  it("подписывается на узел, показывает тики и отписывается при смене узла и размонтировании", async () => {
    let emit: (e: SamplerEvent) => void = () => {};
    vi.mocked(api.startSampler).mockImplementation(async (_id, _m, onEvent) => {
      emit = onEvent;
      return `s-${vi.mocked(api.startSampler).mock.calls.length}`;
    });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    const { result, rerender, unmount } = renderHook(({ member }) => useSampler("c", member, true), { initialProps: { member: "pg-1" as string | null } });
    await waitFor(() => expect(api.startSampler).toHaveBeenCalledWith("c", "pg-1", expect.any(Function)));
    act(() => emit({ kind: "tick", atMs: 5, active: 3, waiting: 1 }));
    expect(result.current.live).toEqual({ atMs: 5, active: 3, waiting: 1 });
    rerender({ member: "pg-2" });
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("s-1"));
    await waitFor(() => expect(api.startSampler).toHaveBeenLastCalledWith("c", "pg-2", expect.any(Function)));
    unmount();
    await waitFor(() => expect(api.stopSession).toHaveBeenLastCalledWith("s-2"));
  });

  it("без узла не подписывается; ответ, пришедший после размонтирования, сразу отписывается", async () => {
    let resolve: (id: string) => void = () => {};
    vi.mocked(api.startSampler).mockImplementation(() => new Promise((r) => (resolve = r)));
    const { rerender, unmount } = renderHook(({ member }) => useSampler("c", member, true), { initialProps: { member: null as string | null } });
    expect(api.startSampler).not.toHaveBeenCalled();
    rerender({ member: "pg-1" });
    unmount();
    resolve("late");
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("late"));
  });
});
```

`WaitsChart.test.tsx`:

```tsx
  it("столбцы с накоплением по классам и таблица для экранного диктора", () => {
    const data = waitsData({ bucketS: 5, buckets: [
      { startMs: 0, sampledS: 5, aas: { cpu: 1, lock: 0.4 } },
      { startMs: 5000, sampledS: 0, aas: {} },
    ], gaps: [{ fromMs: 5000, toMs: 10000 }] });
    renderWithClient(<WaitsChart data={data} />);
    const svg = screen.getByRole("img", { name: "Среднее число активных сессий по классам ожиданий" });
    expect(svg.querySelectorAll("rect[data-class='lock']")).toHaveLength(1);
    expect(svg.querySelectorAll(".gap")).toHaveLength(1);
    const table = screen.getByRole("table", { name: "Профиль ожиданий по интервалам" });
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent("1,4");
  });
```

`WaitsView.test.tsx`:

```tsx
  it("топ ожиданий и запросов, «CPU*» с пояснением, сэмплер не работает — подсказка", () => {
    const data = waitsData({ samplerRunning: false, totalSamples: 10,
      topEvents: [{ class: "cpu", event: null, samples: 6, share: 0.6 }, { class: "lock", event: "transactionid", samples: 4, share: 0.4 }],
      topQueries: [{ key: "id:8", queryId: 8, text: "update t", samples: 4, share: 0.4, topEvent: "Lock:transactionid" }],
      byApplication: [{ name: "api", samples: 10, share: 1 }] });
    renderWithClient(<WaitsView connectionId="c" report={diagReport(data)} />);
    const events = screen.getByRole("table", { name: "Топ ожиданий" });
    expect(within(events).getByRole("row", { name: /CPU\*/ })).toHaveTextContent("60 %");
    expect(screen.getByText(/на процессоре или в ожидании без инструментирования/)).toBeInTheDocument();
    expect(within(screen.getByRole("table", { name: "Топ запросов" })).getByRole("row", { name: /update t/ })).toHaveTextContent("Lock:transactionid");
    expect(screen.getByText("Сэмплер не работает: история не пополняется")).toBeInTheDocument();
  });
```

(`waitsData(patch)` — фикстура с нулями и пустыми списками.)

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm vitest run src/lib/useSampler.test.ts src/components/diag/Waits*`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`src/lib/useSampler.ts`:

```ts
import { useEffect, useState } from "react";
import { api, toAppError } from "../api/client";
import type { AppError } from "../api/types";

/**
 * Подписка вкладки на сэмплер узла (спец. P2 §6): держится, пока вкладка смонтирована (в том числе скрытая —
 * история не прерывается). Смена узла — отписка от старого и подписка на новый. Ответ подписки, пришедший после
 * размонтирования, сразу отписывается: иначе сэмплер работал бы без вкладки.
 */
export function useSampler(connectionId: string, member: string | null, enabled: boolean) {
  const [live, setLive] = useState<{ atMs: number; active: number; waiting: number } | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  useEffect(() => {
    if (!enabled || !member) return;
    let session: string | null = null;
    let cancelled = false;
    setError(null);
    api
      .startSampler(connectionId, member, (e) => {
        if (e.kind === "tick") setLive({ atMs: e.atMs, active: e.active, waiting: e.waiting });
        else if (e.kind === "error") setError(e.error);
        else if (e.kind === "stopped") setLive(null);
      })
      .then((id) => {
        if (cancelled) void api.stopSession(id);
        else session = id;
      })
      .catch((e: unknown) => setError(toAppError(e)));
    return () => {
      cancelled = true;
      if (session) void api.stopSession(session);
    };
  }, [connectionId, member, enabled]);
  return { live, error };
}
```

`WaitsChart.tsx`: SVG `role="img"` с `aria-label` `diag.waits.chartLabel`; ширина по контейнеру (`viewBox` по числу интервалов), высота — фиксированная; столбец интервала — прямоугольники по классам в порядке `WAIT_CLASS_ORDER = ["cpu", "lock", "lwlock", "io", "ipc", "client", "timeout", "bufferPin", "extension", "activity", "other"]` с `data-class`, цвет — CSS-переменные `--wait-<class>` (палитра — по навыку `dataviz` при реализации: различимые в светлой и тёмной теме, `lock` — заметный тёплый), подсказка `<title>` у столбца (время, классы и значения); промежутки — `rect.gap` со штриховкой; ось Y — максимум с округлением, подписи «0» и максимум; легенда — список классов с образцом цвета (`diag.waits.classes.<class>`, у `cpu` — «CPU*»). Табличная альтернатива — `<table aria-label={t("diag.waits.tableLabel")}>` (визуально скрыта классом `sr-only`, но доступна): интервал (`formatTimestamp`), всего AAS, по классам.

`WaitsView.tsx`: индикатор сэмплера (`useSampler(connectionId, report.member, true)`: «живой» — `diag.waits.live` «Сэмплер: активных {{active}}, ждут блокировку {{waiting}}»; ошибка — `ErrorBanner`); `samplerRunning: false` — `diag.waits.notRunning` «Сэмплер не работает: история не пополняется»; `truncated` — `diag.waits.truncated` «История короче окна: сэмплер работает с {{time}}»; `WaitsChart`; таблицы: топ ожиданий (`aria-label` — `diag.waits.topEvents` «Top waits» / «Топ ожиданий»; класс, событие, доля `formatPercent(share * 100)`, сэмплы), топ запросов (`diag.waits.topQueries` «Top queries» / «Топ запросов»; текст, `query_id`, доля, главное ожидание), по приложениям / ролям / БД / клиентам (вкладки или четыре маленькие таблицы); сноска `diag.waits.cpuNote` «CPU* — активная сессия без события ожидания: на процессоре или в ожидании без инструментирования».

`DiagHeader.tsx`: у `waits` — `select` «Окно» (`diag.waits.window`: 5, 15, 60 мин; значения 300 / 900 / 3600); `DiagTab` передаёт окно в `useDiagnostic` (`windowS`).

- [ ] **Step 4: Тесты проходят**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 5: Проверка в приложении**

Run: `pnpm tauri dev` (если порт 1420 занят — `pnpm tauri dev -c '{"build":{"devUrl":"http://localhost:1425","beforeDevCommand":"pnpm vite --port 1425 --strictPort"}}'`).
Проверить на подключении к `pg-single` (`localhost:5460`, `glafira_diag` / `glafira-secret`): «Диагностика» → «Профиль ожиданий»; в терминале — `docker exec gp-pg-single psql -U postgres -c "select pg_sleep(20)"`; за 5–10 с в графике появляется `Timeout`, в топе — `Timeout:PgSleep`; закрытие вкладки — в `pg_stat_activity` больше нет запросов сэмплера (`select count(*) from pg_stat_activity where application_name = 'glafira-pg' and query like '%backend_type IN%' and state = 'active'` → 0 через пару секунд).

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat(ui): профиль ожиданий — график по классам, топы, подписка на сэмплер

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 14: Руководство «Диагностика», F1, admin.md, README, CHANGELOG, спецификация

**Files:**
- Create: `docs/guide/en/diagnostics.md`, `docs/guide/ru/diagnostics.md`
- Modify: `src/guide/chapters.ts`, `scripts/i18n-check.mjs` (`GUIDE_CHAPTERS`), `scripts/i18n-check.test.mjs`, `src/components/help/helpChapterFor.ts` (+ `.test.ts`), `src/lib/useHelpShortcut.ts`, `docs/admin.md`, `README.md`, `CHANGELOG.md`, `docs/i18n/glossary.md`, `docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md`
- Test: `pnpm test:scripts`, `helpChapterFor.test.ts`

**Interfaces:**
- Consumes: ключи интерфейса задач 10–13 (метки `[[ui:…]]`).
- Produces: `GUIDE_CHAPTERS = ["getting-started", "connections", "objects", "diagnostics", "updates"]`; `helpChapterFor({ kind: "diag" })` → `"diagnostics"`; `helpAnchorFor(tab: TabSpec | null) -> string | undefined` (`diag` → якорь инструмента, `idleTx` → `sessions`); F1 открывает `{ kind: "help", chapter, anchor }`.

- [ ] **Step 1: Падающие тесты**

`src/components/help/helpChapterFor.test.ts`:

```ts
  it("вкладка инструмента — глава «Диагностика» с якорем инструмента", () => {
    expect(helpChapterFor({ kind: "diag", connectionId: "c", tool: "locks" })).toBe("diagnostics");
    expect(helpAnchorFor({ kind: "diag", connectionId: "c", tool: "locks" })).toBe("locks");
    expect(helpAnchorFor({ kind: "diag", connectionId: "c", tool: "idleTx" })).toBe("sessions");
    expect(helpAnchorFor({ kind: "overview", connectionId: "c" })).toBeUndefined();
    expect(helpAnchorFor(null)).toBeUndefined();
  });
```

`scripts/i18n-check.test.mjs`: ожидание `GUIDE_CHAPTERS` — `["getting-started", "connections", "objects", "diagnostics", "updates"]`.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm test:scripts && pnpm vitest run src/components/help`
Expected: FAIL.

- [ ] **Step 3: Главы руководства**

`docs/guide/ru/diagnostics.md` (по правилам `docs/i18n/README.md`, раздел «Руководство»: `# Заголовок`, разделы `## … {#id}`, метки `[[ui:ключ]]`, термины — по глоссарию):

- `# Диагностика`; вводный абзац: группа [[ui:sidebar.diagnostics]] в дереве под подключением, у каждого инструмента своя вкладка, «за лидером» по умолчанию и выбор узла в шапке, обновление раз в 5 с, пока вкладка видна, пауза и [[ui:diag.header.refresh]]; всё только читает сервер.
- `## Пульс {#pulse}` — показатели по узлам, откуда берутся, что значит цвет клетки, клик по клетке; скорости — со второго обновления.
- `## Сессии и idle in transaction {#sessions}` — пресеты, группировка, скрытие сессий Glafira, панель сессии; почему висящая транзакция опасна; `idle_in_transaction_session_timeout`.
- `## Профиль ожиданий {#waits}` — сэмплер: запрос раз в секунду к `pg_stat_activity`, пока открыта вкладка; история в памяти до отключения (до 60 мин); окна 5 / 15 / 60 мин; классы ожиданий и `CPU*`; промежутки.
- `## Блокировки {#locks}` — корневой блокирующий и жертвы, DDL в очереди и что делать (отменить DDL или запрос, `lock_timeout` в миграциях), циклы, детали deadlock только в логе.
- `## Горизонт xmin {#horizon}` — кто держит vacuum: сессии, prepared transactions, слоты, feedback реплик; переход на реплику; команда `ROLLBACK PREPARED` для копирования.
- `## Реплики {#replication}` — на primary и на реплике; лаги в байтах и во времени; пауза replay; конфликты recovery.
- `## Слоты репликации {#slots}` — удерживаемый WAL, `wal_status`, слоты Patroni (участника и постоянные), подписки.
- `## Готовность к failover {#failover}` — вердикт по кандидату и причины, `maximum_lag_on_failover`, `synchronous_mode`, pause, тайминги.
- `## Матрица health {#health}` — что видит балансировщик, коды адресов.
- `## Находки {#findings}` — строгость, «что сделать», ссылки, пороги (таблица §4 спецификации кратко), недоступное по версии.

`docs/guide/en/diagnostics.md` — тот же текст по-английски, с теми же якорями.

`src/guide/chapters.ts`, `scripts/i18n-check.mjs`: `GUIDE_CHAPTERS = ["getting-started", "connections", "objects", "diagnostics", "updates"]`.

`src/components/help/helpChapterFor.ts`:

```ts
    case "diag":
      return "diagnostics";
```

и

```ts
/** Якорь главы для вкладки (F1): у инструмента диагностики — его раздел; «Idle in transaction» описан в «Сессиях». */
export function helpAnchorFor(tab: TabSpec | null): string | undefined {
  if (tab?.kind !== "diag") return undefined;
  return tab.tool === "idleTx" ? "sessions" : tab.tool;
}
```

`src/lib/useHelpShortcut.ts`: `open({ kind: "help", chapter: override ?? helpChapterFor(active), anchor: override ? undefined : helpAnchorFor(active) })`.

- [ ] **Step 4: admin.md, README, CHANGELOG, глоссарий, спецификация**

- `docs/admin.md`, новый раздел «Диагностика»: какие права нужны (`pg_monitor` — тексты чужих запросов, `pg_stat_replication`, `pg_ls_waldir`; без него часть данных скрыта и инструмент об этом говорит), нагрузка (запрос раз в 5 с на открытую видимую вкладку, раз в 10 с — Patroni; сэмплер — один лёгкий запрос в секунду на узел, пока открыт профиль ожиданий; соединений на узел — до 3 для каталога и вкладок, 1 опрос, 1 сэмплер), `application_name = glafira-pg`, Patroni API — только `GET` (`/cluster`, `/patroni`, `/config` и адреса здоровья).
- `README.md`: строка о диагностике в описании возможностей; в разделе тестов — `GPG_IT_LAB` уже есть (задача 2), добавить, что `it_diag` и `it_sampler` проходят по матрице при `GPG_IT_MATRIX`.
- `CHANGELOG.md`: раздел `## Unreleased` над `## 0.2.0` — диагностика дежурного набора (инструменты списком), сэмплер и профиль ожиданий, запланированные switchover и restart в обзоре, стенд `pg-lab`.
- `docs/i18n/glossary.md`: горизонт xmin (xmin horizon), idle in transaction (без перевода), профиль ожиданий (wait profile), событие ожидания (wait event), корневой блокирующий (root blocker), DDL в очереди (queued DDL), удерживаемый WAL (retained WAL), готовность к failover (failover readiness), матрица health (health matrix), находка (finding).
- Спецификация P2 (`docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md`) — привести в соответствие с уточнениями плана: §3.2 — «за лидером» у списка узлов всегда primary; §4 — одна находка на правило и узел с `target` худшего объекта и `params.count`, кроме `rootBlocker` и `ddlQueue`; §5.7 — точное правило имени слота Patroni; §6 — время сэмпла по часам приложения, версия — первым запросом; §12.2 — DDL сценария выполняет `postgres`; §3.4 / §9 — `sampler::start`, `AppState::client_with`.

- [ ] **Step 5: Проверки**

Run: `pnpm test:scripts && pnpm i18n:check && pnpm test && pnpm exec tsc --noEmit && pnpm build`
Expected: PASS; глава открывается по F1 на вкладке инструмента (проверка в `pnpm tauri dev`: открыть «Блокировки», нажать F1 — раздел «Блокировки»).

- [ ] **Step 6: Commit**

```bash
git add docs src scripts README.md CHANGELOG.md
git commit -m "docs: глава «Диагностика», F1 с якорем инструмента, права и нагрузка в admin.md, спецификация P2 по уточнениям плана

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Самопроверка плана

- **Покрытие части P2a спецификации:**
  - §1 п. 1 (каркас, конверт, SQL по версиям, находки) — задачи 1, 3; п. 2 (инструменты): пульс — 4, сессии и idle in transaction — 3 и 11, профиль ожиданий — 9 и 13, блокировки и DDL в очереди — 5 и 11, горизонт — 6 и 12, реплики — 6 и 12, слоты и подписки — 8 и 12, готовность к failover и матрица health — 7 и 12, топология Patroni (обзор с запланированными действиями) — 7 и 12; п. 3 (сэмплер) — 9, 13; п. 4 (`/config`, `scheduled_*`) — 7; п. 7 (стенд, сценарии, матрица 13–18) — 2 и тесты `*_sql_runs_on_every_version` в 3–9; п. 8 (руководство, admin.md) — 14.
  - §2 (что берём): `same_except_time`, `get_status`, `fetch_config` — 7; `CloseListener`, `diag_target` — 3; `PoolPurpose::Sampler` — 9; `catalog_limit` — 3; `RECOVERY_CONFLICT` — 3; `TabSpec`, `PG_QUERY_KEYS` — 10; реестр сессий — 9.
  - §3.3 (версии) — `Caps` (3) и колонки каждого инструмента; §3.4 (соединения, слушатели закрытия) — 3, 9; §3.5 (сессии) — 9; §3.6 (интервалы, скорости) — 4, 6, 8, 10.
  - §4 (находки, таблица порогов) — 1 (константы) и правила 3–9; тексты — 10.
  - §5.1–5.10 — задачи 4–8 и виды 11–12; §6 — 9, 13; §7 (интерфейс) — 10–13; §7.5 (справка) — 14; §9 (IPC P2a) — 3, 9; §10 (`RECOVERY_CONFLICT`) — 3; §12.1 (`glafira_app`, `pg-lab`, `wait-stand`) — 2; §12.2 (сценарии P2a) — 3, 5, 6, 7, 8, 9; §12.3 (CI) — 2; §13 — 14; §15 (критерии P2a): 1 — 10 и 7 (Patroni-инструменты только у `patroni`), 2 — 7, 3 — 5, 6, 8, 3, 4 — 9 и 13, 5 — матрица в 3–9, 6 — 14 и CI.
  - Не входит в план (часть P2b): §8 целиком, `guard.rs`, `ConfirmDialog`, `READ_ONLY`, `CONFIRMATION_REQUIRED`, `actions.jsonl`, `replicationAttr`, кнопки действий в обзоре и инструментах, сценарии действий на стенде.
- **Типы между задачами:** `Finding`/`Severity`/`thresholds` (1) → правила 3–9; `CounterSnapshot`/`rate`/`delta` (1) → пульс (4), конфликты (6), ошибки подписок (8); `DiagTarget`, `make_report`, `on_member`, `Caps`, `Unavailable { part, member, reason }` (3) → все инструменты; `sender_member` (6) → горизонт и реплики; `CandidateRole` (7) → матрица health; `dyn_config` (7) → слоты (8); `SessionRegistry` (9) → P2b (наблюдение); `DiagReport`/`DiagData` в Rust (3–9) ↔ TS-типы (10) — по полям Interfaces задач; `findingParams` использует суффиксы имён параметров из правил (`maxS`, `lagBytes`, `usedPct`, `count`, `command`, `kind`).
- **Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13 → 14. Задача 8 зависит от 7 (`dyn_config`), 9 — от 3 (слушатели, `diag_target`), 10 — от JSON задач 3–9, 14 — последняя (`GUIDE_CHAPTERS` требует файлы глав).
- **Риски, названные заранее:** тест «за лидером после switchover» меняет стенд — только под `GPG_IT_DOCKER=1`, восстановление `wait_stand`; сценарии с `pg-replica` (пауза replay, `hot_standby_feedback`) восстанавливают стенд в `Drop` через `run_sql_blocking`; `pg-lab` на уже поднятом стенде поднимается отдельной командой `up -d --no-recreate pg-lab` (README, задача 2); `DefaultHasher` в ключах текста запросов профиля — стабилен в пределах процесса, ключи не сохраняются; доля `Lock` считается по топу из 10 событий (оговорено в задаче 9).
