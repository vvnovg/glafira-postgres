# План P2c — зачистка техдолга после P1b/P2a/P2b

Ветка `feat/p2c` от `main` (a1b13bb). Источник: `.superpowers/sdd/p2c-backlog.md`, триаж утверждён владельцем 2026-10-28
(список «(а)» целиком, включая дешёвые дополнения). Спеки: P1 `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md`,
P2 `docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md`. Руководство `docs/guide/{ru,en}/*.md`, администратору
`docs/admin.md`.

## Глобальные ограничения (действуют для каждой задачи)

- Коммиты на русском, формат `тип(область): описание`, трейлер `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
  Коммитить только свои файлы; ветку не менять, `git stash`/`reset` не делать.
- TDD: сначала падающий тест, затем код. Каждая правка поведения — с тестом.
- i18n: все новые строки интерфейса — в `src/locales/en.json` и `src/locales/ru.json`; текст в `app_err!`/`ltext!` в Rust должен
  дословно совпадать с `en.json` (проверка `pnpm i18n:check`). В литералах Rust вне тестов — без кириллицы.
- Документация синхронно с поведением: руководство (ru и en), `docs/admin.md`, спецификация (раздел, который описывает
  изменённое поведение). CHANGELOG в задачах не трогать — он собирается в конце этапа.
- Проверки перед отчётом (по тому, что менялось): Rust — `cargo fmt --all --manifest-path src-tauri/Cargo.toml`,
  `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo test --manifest-path src-tauri/Cargo.toml --lib`
  (+ `-p pg-advisor` при правке crates/pg-advisor); UI — `pnpm vitest run <файлы>` и `pnpm tsc --noEmit` (или `pnpm build`),
  `pnpm i18n:check`; скрипты — `pnpm test:scripts`.
- Интеграционные тесты со стендом (`cargo test --tests`) запускать только те, что названы в задаче, и только с переменными
  `GPG_IT_*` из брифа; стенд не менять (никаких `GPG_IT_DOCKER=1`, никакого `docker compose down`).
- Не трогать не названные в задаче области; найденное попутно — в отчёт, не в код.

---

### Task 1: Причины отказа `candidateNotReady` параметрами, перевод в диалоге

**Файлы:** `src-tauri/src/actions/patroni.rs` (`ensure_candidate_ready`, `reason_code`, тесты ~1230–1280),
`src/components/actions/PatroniActionDialog.tsx` (+ `.test.tsx`), `src/locales/{en,ru}.json`,
спека P2 §8.5 (около строки 711) и §9 (таблица кодов, строка ~890 «`candidateNotReady` (причины в `details`)»).

**Сейчас.** `ensure_candidate_ready` кладёт в `details` строку `reasons: lagTooHigh, notSynchronous; lagBytes: 123` — сырые коды
причин из JSON вердикта. Диалог показывает ошибку через `ErrorBanner` (`tr(e)` + `details` в `<details>`).

**Нужно.**
1. Rust: `ensure_candidate_ready` вместо `details` кладёт параметры: `reasons` — коды причин через запятую без пробелов, в
   порядке вердикта (`"lagTooHigh,notSynchronous"`), и `lagBytes` — число строкой (`"123456"`), только когда лаг известен
   (`c.lag_bytes.is_some()`); при неизвестном лаге параметра `lagBytes` нет. Кандидата нет в вердикте — как сейчас, без
   параметров (кроме `member`). Текст ошибки не меняется: `{{member}} is not ready to become the leader`
   (ключ `errors.actions.candidateNotReady`, `INVALID_INPUT`). `details` у этой ошибки больше нет.
   Параметры `AppError` — `BTreeMap<String, String>`: добавлять через `e.params.insert(...)`.
   Тесты в `patroni.rs`: (а) кандидат с двумя причинами и лагом → `params["reasons"] == "lagTooHigh,notSynchronous"`,
   `params["lagBytes"] == "123456"`, `details == None`; (б) лаг неизвестен → ключа `lagBytes` нет; (в) кандидата нет в
   вердикте → только `member`. Существующие тесты на ключ/код — сохранить.
2. TS: в `PatroniActionDialog.tsx` там, где показывается `run.error` (`<ErrorBanner error={run.error} />`, ~строка 388), после
   баннера для ошибки с ключом `errors.actions.candidateNotReady` и непустым `params.reasons` показать строку
   `t("actions.patroni.notReadyReasons", { reasons })`, где `reasons` — коды из `params.reasons.split(",")`, каждый переведён
   через `REASON_KEYS` (из `../diag/viewText`), неизвестный код — как есть, соединены `"; "`; при наличии `params.lagBytes` —
   вторая строка `t("actions.patroni.notReadyLag", { lag: bytesText(Number(lagBytes)) })`. Разметка: `<p className="muted">`
   на каждую строку. Ошибку получить через `toAppError(run.error)` (`../../api/client`).
   Ключи (en / ru): `actions.patroni.notReadyReasons` = `Reasons: {{reasons}}` / `Причины: {{reasons}}`;
   `actions.patroni.notReadyLag` = `Lag: {{lag}}` / `Лаг: {{lag}}`. Положить рядом с другими `actions.patroni.*`.
   Тест в `PatroniActionDialog.test.tsx` по образцу существующих тестов отказа: мутация выполнения отвергается
   `AppError { code: "INVALID_INPUT", key: "errors.actions.candidateNotReady", params: { member: "pg-2", reasons: "lagTooHigh,unreachable", lagBytes: "1048576" }, ... }`
   → в диалоге видны переведённые причины (`t("diag.failover.reasons.lagTooHigh")`, `…unreachable`) и строка лага с `1 MB`
   (через `bytesText`); без `lagBytes` строки лага нет.
3. Спека P2: §8.5 (около 711) и §9 (строка ~890): «причины в `details`» → «причины — `params.reasons` (коды через запятую),
   лаг — `params.lagBytes` (байты строкой, только если известен); интерфейс переводит коды теми же ключами, что вкладка
   готовности».

**Проверки:** `cargo test --lib actions::patroni`, `pnpm vitest run src/components/actions/PatroniActionDialog.test.tsx`,
`pnpm tsc --noEmit`, `pnpm i18n:check`, fmt, clippy.

---

### Task 2: Пакет сессий — клиентский срок на запрос и остановка при закрытии подключения

**Файлы:** `src-tauri/src/actions/sessions.rs` (`execute` ~590–680, `attempt`, `wait_gone`, `still_there`),
`src-tauri/src/topology/runtime.rs` (новый аксессор), `src/locales/{en,ru}.json`, спека P2 §8.3 (абзац «Сессии идут по
одной…», строка ~634) и §8.8 таблица кодов (строка ~892, строка `TIMEOUT`), `docs/admin.md:180`,
`docs/guide/ru/diagnostics.md:203`, `docs/guide/en/diagnostics.md:203`.

**Сейчас.** Сессии пакета идут по одной на одном соединении действия; у запросов нет клиентского срока (при обрыве сети
запрос ждёт TCP keepalive — серверный `statement_timeout` не поможет), а закрытие подключения (`disconnect`) не останавливает
пакет: `ActionClient` живёт до конца `execute` (до 60 с + одна сессия). Пока идёт пакет, диалог не закрывается.

**Нужно.**
1. `Runtimes::generation(&self, id: &str) -> Option<u64>` — номер текущего открытия подключения (`Entry::generation`),
   `None`, если подключение не открыто. Быстрый, под `entries` (как `view`). Юнит-тест: после `connect` — `Some(g)`, после
   `disconnect` — `None`, после повторного `connect` — `Some(g2)`, `g2 != g`.
2. В `sessions.rs`: константа `pub const STATEMENT_TIMEOUT: Duration = Duration::from_secs(10)` (клиентский срок ответа на один
   запрос к серверу; серверный `statement_timeout` соединения действия — 5 с, клиентский больше него на запас сети). Каждое
   обращение к серверу внутри одной сессии (`attempt`: `c.query`; `still_there`; каждый опрос в `wait_gone`) идёт под
   `tokio::time::timeout(STATEMENT_TIMEOUT, …)`. Истёк — исход `failed { error }` с ошибкой
   `errors.actions.statementTimeout`, текст `The server did not answer within {{secs}} s`, `secs = 10`, `ErrorCode::Timeout`.
   После такого исхода пакет останавливается: соединение действия после зависшего запроса непригодно. Оставшиеся сессии
   получают `failed { errors.actions.batchStopped }`, текст `Not attempted: the batch was stopped after a timeout`,
   `ErrorCode::Timeout`; записи журнала у них — `blank_record` с `result = "failed"` (как у `batchTimeout`).
3. Остановка при закрытии подключения: в начале `execute` (после `st.profile(id)?`) запомнить `let generation = st.runtimes.generation(id)`;
   перед каждой сессией (в том же месте, где проверяется `BATCH_LIMIT`) сравнить: если `st.runtimes.generation(id) != generation`
   (подключение закрыто или открыто заново) — эта и все оставшиеся сессии получают `failed { errors.topology.notConnected }`
   (готовая ошибка `crate::topology::runtime::not_connected()`), запросов к серверу больше нет.
   Порядок проверок перед сессией: сначала закрытие подключения, затем `BATCH_LIMIT`, затем остановка после `statementTimeout`.
4. Логику пакета вынести в функцию без `Client`, чтобы она проверялась юнит-тестами без сервера:
   `async fn run_batch(sessions: &[SessionRef], started: Instant, still_open: impl Fn() -> bool, run_one: impl AsyncFnMut(SessionRef) -> (SessionOutcome, SessionRecord)) -> (Vec<SessionResult>, Vec<SessionRecord>)`
   (сигнатура может отличаться, если так проще; `AsyncFnMut` — Rust 2024, в проекте доступен). `execute` вызывает её с
   `still_open = || st.runtimes.generation(id) == generation` и `run_one = |s| one(&c.client, …)`.
   Юнит-тесты `run_batch`: (а) `still_open` возвращает `false` после первой сессии → `run_one` вызван один раз, остальные —
   `failed` с ключом `errors.topology.notConnected`; (б) первая сессия вернула `failed { statementTimeout }` → остальные
   `failed { batchStopped }`, `run_one` вызван один раз; (в) `started` в прошлом дальше `BATCH_LIMIT` → все `batchTimeout`
   (существующий путь); (г) обычный пакет — все вызваны. Тест срока: `tokio::time::timeout` вокруг никогда не
   завершающегося future даёт `statementTimeout` — через маленький helper `timed(fut) -> Result<T, AppError>` и тест с
   `tokio::time::pause()`/`advance` (фича `test-util` в dev-deps есть).
5. Итог действия (`batch_outcome`/`batch_code`): `batchStopped`, `statementTimeout` и `notConnected` — это `failed`, как
   прочие ошибки; код записи журнала — как для `TIMEOUT`/`CONNECTION_FAILED` по существующему `code_name`. Проверить
   существующими тестами `batch_code`.
6. Ключи en/ru: `errors.actions.statementTimeout` = `The server did not answer within {{secs}} s` / `Сервер не ответил за {{secs}} с`;
   `errors.actions.batchStopped` = `Not attempted: the batch was stopped after a timeout` / `Не выполнялось: пакет остановлен после истечения срока`.
7. Документы: спека §8.3 после фразы про `BATCH_LIMIT`: «У каждого запроса к серверу клиентский срок 10 с
   (`STATEMENT_TIMEOUT`): истёк — `failed { errors.actions.statementTimeout }`, и пакет останавливается, оставшиеся —
   `failed { errors.actions.batchStopped }`. Подключение, закрытое или открытое заново между сессиями, останавливает пакет:
   оставшиеся — `failed { errors.topology.notConnected }`.» §8.8: в строке `TIMEOUT` добавить `statementTimeout` и `batchStopped`.
   `admin.md:180`: одно предложение про клиентский срок 10 с на запрос и остановку пакета при закрытии подключения.
   Руководство ru/en строка 203: то же одним предложением.

**Проверки:** `cargo test --lib actions::sessions topology::runtime`, интеграционный `it_actions` (чтение/действия на стенде, без
смены состояния): `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 cargo test --manifest-path src-tauri/Cargo.toml --test it_actions -- --test-threads=1`,
`pnpm i18n:check`, fmt, clippy.

---

### Task 3: Финальное событие наблюдения при закрытии или переоткрытии подключения

**Файлы:** `src-tauri/src/actions/watch.rs` (`start` ~280–375, `stop_watch` ~377–395, тесты), спека P2 §8.6 (абзац «Канал
`WatchEvent`…», строка ~795), `docs/guide/{ru,en}/diagnostics.md:238`.

**Сейчас.** Остановка сессии наблюдения (`stop_watch`) делает `abort()` задачи: закрытие или переоткрытие подключения, пока
панель наблюдения открыта, оставляет её на последнем шаге с `running`/`aria-current` навсегда — финального события нет.
Остановка самой панелью («Закрыть», размонтирование) события не требует.

**Нужно.**
1. Вместо `AbortHandle` — сигнал отмены (`tokio::sync::watch::channel(false)` или `Arc<tokio::sync::Notify>` + `AtomicBool`;
   `tokio-util` в зависимостях нет, не добавлять). Замыкание `stop_watch` только подаёт сигнал (без `await`, без обращения к
   `Runtimes.entries` — оно вызывается из слушателя закрытия под этим мьютексом) и, как сейчас, снимает ускорение.
2. Цикл задачи: `tokio::select!` между сигналом отмены и `sleep(BOOST_INTERVAL)`. По отмене задача решает, кто остановил:
   `runtimes.generation(&conn)` (аксессор из Task 2) равен номеру, запомненному при старте (`st.runtimes.generation(id)` в
   `start`), и подключение открыто → остановила панель: события нет. Иначе (подключение закрыто или открыто заново) —
   событие `WatchEvent { step: last, status: Failed, at_ms, detail: Some(not_connected()) }`. Затем, как сейчас, `unboost` и
   `sessions.forget`.
3. Ровно одно финальное событие: `Done`, `TimedOut` и `Failed` отправляются через одну точку с флагом
   `Arc<AtomicBool>` «финал отправлен» (`swap(true)`): ветка в `start`, которая сама шлёт `failed` для не открытого или
   закрытого за время старта подключения, ставит тот же флаг, так что задача после её `stop` второго события не пошлёт.
4. Тесты в `watch.rs` (fixture с открытым подключением — по образцу `actions::conn::tests::state_with` / `crate::conn::testutil`;
   если открыть подключение в юнит-тесте нельзя без сервера, смоделировать через `Runtimes::new` с источником топологии
   `Fixed`, как там): (а) существующий `a_watch_on_a_closed_connection_fails_at_once` — по-прежнему ровно два события;
   (б) подключение открыто → `start` → `disconnect` → не позднее `BOOST_INTERVAL + 200 мс` последнее событие
   `Failed` с `detail.key == "errors.topology.notConnected"`, и после него событий нет (подождать ещё один интервал);
   (в) подключение открыто → `start` → `st.sessions.stop(&sid)` (панель закрыта) → после ожидания `BOOST_INTERVAL + 200 мс`
   новых событий нет; (г) переоткрытие (`disconnect` + `connect` с тем же id до того, как задача проснулась) — событие
   `Failed notConnected` ровно одно.
5. Спека §8.6, абзац про канал: «`failed` бывает только у закрытого…» → «`failed` с `errors.topology.notConnected` приходит,
   когда подключение не открыто, закрыто за время старта или закрыто либо открыто заново во время наблюдения; остановка
   панелью (`stop_session`) события не даёт. Задача наблюдения прерывается сигналом отмены, а не `abort`, чтобы послать
   финальное событие; финальное событие у наблюдения одно». Руководство ru/en строка 238: после «закрытие подключения
   останавливает его тоже» добавить «, и панель показывает причину».

**Проверки:** `cargo test --lib actions::watch`, fmt, clippy. UI не меняется (`WatchPanel` уже показывает `failed` с причиной).

---

### Task 4: HTTP Patroni — срок соединения на чтении и пробное чтение через туннель перед изменяющим запросом

**Файлы:** `src-tauri/src/topology/patroni.rs` (`request_error` свободная функция ~305–333, `connect_timed_out`,
`ReqwestHttp::send_json` ~462–530, тесты ~2130–2170 и ~2576–2610), спека P2 §8.5 (абзацы про переход к следующему адресу и
«Сроки», строки ~720–740), `docs/admin.md:193`, `docs/guide/{ru,en}/diagnostics.md:230`.

**Сейчас.**
- Чтение (`get_json`): срок соединения `HTTP_CONNECT_TIMEOUT` = 3 с, но ошибка идёт через `request_error(url, e, HTTP_TIMEOUT)`,
  где `e.is_timeout()` даёт `errors.patroni.timeout` «did not answer within 5 s» — текст лжёт. У `send_json` тот же случай
  уже даёт `connect_timed_out` → `errors.patroni.unreachable`.
- Через туннель старый OpenSSH (`ssh -D` до 10.x, в том числе OpenSSH Windows 8.x/9.x и Ubuntu 22.04 8.9) отвечает на SOCKS
  успехом и лишь потом открывает канал; если бастион до узла не достучался, поток закрывается после отправки запроса. Для
  чтения это распознаётся (`stream_closed` → `forwardFailed`), а в `send_json` такой обрыв неотличим от обрыва после
  доставки и даёт `connectionLost` («исход неизвестен», без перехода к следующему адресу) — тест
  `send_json_through_the_tunnel_treats_a_late_forward_failure_as_lost`.

**Нужно.**
1. Свободная `request_error`: ветка `e.is_connect() && e.is_timeout()` первой → `connect_timed_out(url, e)`
   (`errors.patroni.unreachable`, `details` — цепочка причин). Остальное без изменений. Тест
   `send_json_treats_a_connect_timeout_as_an_unreachable_address`: для чтения теперь тоже `errors.patroni.unreachable`
   (поправить утверждение и doc-комментарий теста). Проверить, что ни один вызывающий не различает `timeout`/`unreachable`
   на чтении логикой (grep по ключам): различий быть не должно.
2. Пробное чтение в `send_json`, только когда `self.proxy.is_some()` (через туннель): до изменяющего запроса —
   `GET {scheme}://{host}:{port}/patroni` того же адреса клиентом чтения (`self.client`, срок `HTTP_TIMEOUT`, basic auth и
   правило `withholds_credentials` — как в `get_json`). Любой HTTP-ответ (любой статус, любое тело) — узел достижим, дальше
   как сейчас. Ошибка пробного чтения классифицируется `self.request_error(url_probe, &e, HTTP_TIMEOUT)`: если ключ —
   `errors.ssh.forwardFailed`, `errors.ssh.tunnelDown` или `errors.patroni.unreachable`, вернуть эту ошибку (запрос не
   отправлялся; вызывающий переходит к следующему адресу, как при сбое соединителя). Любая другая ошибка пробного чтения
   (срок после соединения, TLS, прочее) — продолжить и отправить запрос: узел отвечает, сбой самого запроса классифицируется
   как сейчас. Без туннеля пробного чтения нет.
   Тесты: (а) `send_json_through_the_tunnel_treats_a_late_forward_failure_as_lost` переименовать/переписать: fake_ssh с
   `FAKE_SSH_UPSTREAM=unreachable` (успех SOCKS, затем закрытие) → теперь `errors.ssh.forwardFailed`, `params.kind == "forwardFailed"`,
   `params.host == "patroni1"`, `params.port == "8008"`; (б) достижимый узел через туннель (`reqwest_http_goes_through_the_tunnel`
   с `stub_server` как upstream): `send_json` POST доходит, ответ возвращается, и на заглушке видны ровно два запроса —
   `GET /patroni` и `POST …` (если `stub_server` не считает запросы, расширить его в тестах минимально); (в) без туннеля
   (`send_json_reports_connection_lost_when_the_link_drops_after_the_request`) — поведение прежнее, `accepted() == 1`
   (пробного чтения нет).
3. Документы: спека §8.5 абзац «К следующему адресу…»/«Сроки»: добавить «Через SSH-туннель перед изменяющим запросом —
   пробное чтение `GET /patroni` того же адреса: сбой проброса (`forwardFailed`, `tunnelDown`) или срок соединения
   (`unreachable`) — запрос не отправляется, адрес считается недоставленным; любой ответ — запрос отправляется. Срок
   соединения на чтении (3 с) даёт `errors.patroni.unreachable`, а не `timeout`». `admin.md:193`: одно предложение про пробное
   чтение через туннель. Руководство ru/en строка 230: то же, одним предложением.

**Проверки:** `cargo test --lib topology::patroni` (тесты туннеля компилируют `fake_ssh` — это нормально, ~1–3 с), fmt, clippy.
Интеграционные тесты Patroni не нужны (поведение без туннеля не меняется).

---

### Task 5: Отправитель WAL по `client_addr`; находка `replicaNotStreaming` без строк `backup`

**Файлы:** `src-tauri/src/diag/mod.rs` (`sender_member` ~194–200, тесты ~385–395), `src-tauri/src/diag/horizon.rs:89,111`,
`src-tauri/src/diag/replication.rs:116`, `crates/pg-advisor/src/rules/replication.rs` (`Sender` doc ~15, фильтр
`not_streaming` ~142–155, тесты), `src/locales/{en,ru}.json` (`findings.replicaNotStreaming.fix`),
`docs/guide/{ru,en}/diagnostics.md:110,116`, спека P2 строка ~249 (таблица правил, `replicaNotStreaming`).

**Нужно.**
1. `pub fn sender_member(t: &Topology, application: Option<&str>, client_addr: Option<&str>) -> Option<String>`: сначала как
   сейчас по `application` (имя участника или его хост), затем, если не нашлось, по `client_addr`: участник, у которого
   `m.host.trim_matches(['[', ']']) == addr` (текстовое сравнение IP без DNS; `host(client_addr)` отдаёт IP без скобок).
   Все три вызова передают `client_addr` из строки (`row.client_addr`, `client_addr`, строка отправителя). Тесты в
   `diag/mod.rs`: `walreceiver` + адрес хоста участника → участник; адрес чужой → `None`; и `application`, и адрес подходят
   разным участникам → побеждает `application`; оба `None` → `None`.
   Doc-комментарий `Sender.member` в pg-advisor: «по `application_name` или адресу клиента (без DNS)».
2. Правило `replicaNotStreaming` (pg-advisor): строки с `state == "backup"` (копирование `pg_basebackup`) не считаются
   репликами не в потоковом режиме — исключить из `not_streaming`. Тест: отправитель `backup` → находки нет; `catchup` →
   есть (состояние в `params.state`). Текст подсказки `findings.replicaNotStreaming.fix`: ru «Реплика в startup или catchup
   ещё не кандидат на failover.», en аналогично (сейчас упоминает backup).
3. Документы: руководство ru/en строка 110: «по `application_name`: … или по адресу клиента, равному хосту участника (имена
   не разрешаются через DNS)»; строка 116 ru/en после «реплика не в режиме streaming» — «(строки `backup` — копирование
   `pg_basebackup` — не считаются)». Спека, таблица правил (строка ~249): условие `state ≠ streaming и ≠ backup`.

**Проверки:** `cargo test --lib diag::`, `cargo test -p pg-advisor` (из `src-tauri` workspace: `cargo test --manifest-path src-tauri/Cargo.toml -p pg-advisor`
или по месту crate), `pnpm i18n:check`, fmt, clippy.

---

### Task 6: Контраст цветов классов ожиданий в светлой теме и постоянный тест контраста

**Файлы:** `src/styles.css` (`:root` строки 15–32 и тёмная тема 41–62), новые `scripts/theme-contrast.mjs` и
`scripts/theme-contrast.test.mjs` (`pnpm test:scripts` = `node --test "scripts/*.test.mjs"`).

**Сейчас.** В светлой теме (`--bg: #ffffff`) `--wait-io #eda100`, `--wait-ipc #e87ba4`, `--wait-lwlock #1baf7a` и
`--wait-extension #b1b5bd` дают контраст ниже 3:1 (комментарий в CSS это признаёт). Столбцы графика профиля ожиданий и
образцы легенды видны плохо.

**Нужно.**
1. Перед подбором цветов загрузить навык `dataviz` (инструмент Skill) — метод палитры и проверка; цвета столбцов —
   категориальные, соседние классы в порядке `cpu, lock, lwlock, io, ipc, client, timeout, bufferPin` должны оставаться
   различимыми (ΔE OKLab ≥ 8 между соседями), три серых (`extension, activity, other`) — нейтральными.
2. Новые значения для светлой темы: каждая из 11 переменных `--wait-*` в `:root` — контраст ≥ 3:1 к `#ffffff` (WCAG 1.4.11,
   нетекстовые элементы). Тёмную тему менять только если её переменная не проходит 3:1 к `#1b1d21` (проверить). Комментарий в
   CSS (строки 15–21) переписать по факту: каветы «ниже 3:1» больше нет.
3. `scripts/theme-contrast.mjs`: чистые функции `parseThemeVars(css) -> { light: Map, dark: Map }` (переменные `--bg` и
   `--wait-*` из `:root` и из блока `@media (prefers-color-scheme: dark)`), `contrastRatio(hexA, hexB)` (WCAG), `deltaEOk(hexA, hexB)`
   (OKLab, формулы из CSS Color 4). `scripts/theme-contrast.test.mjs`: читает `src/styles.css` и проверяет (а) все `--wait-*`
   в обеих темах ≥ 3:1 к `--bg` своей темы; (б) соседние пары восьми основных классов в указанном порядке — ΔE ≥ 8 в обеих
   темах; (в) юнит-тесты функций на известных парах (`#000000`/`#ffffff` = 21, одинаковые цвета ΔE 0).
4. Документы: руководство (глава diagnostics, раздел профиля ожиданий) цвета по именам не называет — проверить grep «цвет»;
   если называет, поправить. Спека — не трогать.

**Проверки:** `pnpm test:scripts`, `pnpm vitest run src/components/diag/WaitsChart.test.tsx` (не должен измениться),
`pnpm build`.

---

### Task 7: Мелочи Rust после смоука P1b — хост в диалоге доверия и пустые пути сертификатов

**Файлы:** `src-tauri/src/net/ssh/process.rs` (`parse_host_key_report` ~457–467, тест ~766), `src-tauri/src/conn/profile.rs`
(`validate_path` ~348, `validate_tls` ~368–390, таблица недопустимых профилей в тестах ~820–900, нормализация профиля —
найти по `normalize`/месту, где Rust уже чистит `key_path` при смене способа входа), `src/locales/{en,ru}.json`.

**Нужно.**
1. `parse_host_key_report`: ssh пишет `authenticity of host 'X (Y)'`; для IP-целей `X == Y`
   (`[127.0.0.1]:2222 ([127.0.0.1]:2222)`) — вернуть только `X`. `bastion (10.0.0.5)` — как сейчас. Тесты на оба случая.
2. `validate_tls`: путь клиентского сертификата пустой или из пробелов → `errors.connections.certRequired`,
   текст `Specify the client certificate file` (ru «Укажите файл клиентского сертификата»), до проверки абсолютности;
   PEM с `key_path == Some("")` (пустой/пробелы) — как `None` → существующий `keyRequired`; `ca_path == Some("")` (пустой/пробелы)
   — нормализуется в `None` там же, где профиль нормализуется перед проверкой и сохранением (поле в форме можно очистить).
   Тесты: три новые строки в таблице недопустимых профилей (`certRequired` для `""` и `"  "`; `keyRequired` для `Some("")`),
   тест нормализации `ca_path: Some("")` → `None` после сохранения/валидации.
3. Ключ `errors.connections.certRequired` в en/ru рядом с `keyRequired`.

**Проверки:** `cargo test --lib net::ssh::process conn::profile`, `pnpm i18n:check`, fmt, clippy.

---

### Task 8: Косметика интерфейса после смоука P1b и мелочи журнала

**Файлы:** `src/components/connections/TestPanel.tsx` (+ `.test.tsx`), `src/components/connections/tree/ObjectList.tsx` и
`src/styles.css` (`.tree-wrap` ~221–223), `src/components/connections/TrustHostKeyDialog.tsx` (+ `.test.tsx`),
`src/components/connections/ConnectionForm.tsx:308` (+ `.test.tsx`), `src/components/actions/ActionsJournal.tsx` (+ `.test.tsx`),
`src/components/catalog/ObjectTab.tsx:46` (+ `.test.tsx`), `src/locales/{en,ru}.json`, `docs/guide/{ru,en}/connections.md:50`.

**Нужно.**
1. `TestPanel`: шаг без `message` и без `error` (например, `readiness` со статусом `ok`) рисуется без двоеточия:
   `<strong>{название}{member}</strong>`; двоеточие — только когда есть текст. Тест: событие `{ step: "readiness", status: "ok" }`
   → в `<strong>` текст ровно `Готовность` (без `:`); шаг с сообщением — как раньше, с `:`.
2. Узкая боковая панель: имя объекта переносится внутри своей кнопки, а размер, «N партиций» и глиф «—» остаются в той же
   строке справа и не переносятся. CSS: у `.tree-wrap` убрать `flex-wrap: wrap`; `.tree-wrap .tree-name` оставить
   переносимым (`white-space: normal; overflow-wrap: anywhere; min-width: 0; flex: 1 1 auto`); спутники строки — `.tree-wrap > .muted`
   и глиф — `flex: none; white-space: nowrap`. Проверить, что `.tree-wrap .tree-folder-toggle { flex: 1 1 auto }` не растягивает
   кнопку-стрелку (ей нужен `flex: none`). Разметка `ObjectRow` — без изменений, если CSS хватает. Тест вёрстки в jsdom
   невозможен — описать в отчёте, что именно изменилось и почему перенос исчезает; существующие тесты `ObjectList.test.tsx`
   должны пройти.
3. `TrustHostKeyDialog`: проп `action?: "connect" | "check"` (по умолчанию `"connect"`). Подпись кнопки: `hostKey.trust` для
   `connect`, новый ключ `hostKey.trustAndCheck` (`Trust and check` / `Доверять и проверить`) для `check`. `ConnectionForm`
   передаёт `action="check"`; `ConnectionNode` и `Overview` — без изменений. Тесты: диалог с `action="check"` показывает
   «Доверять и проверить», без пропа — «Доверять и подключиться»; в `ConnectionForm.test.tsx` существующий тест диалога
   доверия проверяет новую подпись. Руководство `connections.md:50` ru/en: «Кнопка [[ui:hostKey.trust]] (в форме —
   [[ui:hostKey.trustAndCheck]]) повторяет то, что вы делали…».
4. `ActionsJournal`: пробел после `&&` (`{q.isLoading && <p …`); кнопка раскрытия строки получает `aria-controls` с id строки
   с подробностями (id через `useId` + ключ строки), сама строка подробностей — этот `id`. Тест: у раскрытой строки
   `aria-controls` указывает на элемент с подробностями.
5. `ObjectTab` «Обновить»: вместо `disabled={query.isFetching}` — `aria-disabled={query.isFetching}` и игнорирование клика,
   пока идёт загрузка (`aria-busy` остаётся). Тест: во время загрузки у кнопки нет атрибута `disabled`, клик не вызывает
   повторный `refetch`.

**Проверки:** `pnpm vitest run src/components/connections src/components/actions/ActionsJournal.test.tsx src/components/catalog/ObjectTab.test.tsx`,
`pnpm tsc --noEmit`, `pnpm i18n:check`.

---

### Task 9: Каталог — ссылки на родителя и партиции из `schema`+`name`, поля последовательностей строками

**Файлы:** Rust `src-tauri/src/catalog/table.rs` (`Partitioning` ~95–106, SQL `HEAD` подзапрос родителя ~235–238,
`PARTITIONS_SQL` ~260–262, сборка ~410–465, тесты), `src-tauri/src/catalog/ddl.rs` (строки `-- partitions:` ~272–276,
`PARTITION OF` по `parent`, тесты ~505–525, ~700–705, ~795–810), `src-tauri/src/catalog/sequence.rs`,
`crates/pg-advisor/src/serde_util.rs` (новый модуль `i64_string` рядом с `opt_i64_string`), `src-tauri/tests/it_catalog.rs`
(~300–340, ~400–445, ~607); TS `src/api/types.ts` (`Partitioning` ~386, `SequenceDetails` ~461),
`src/components/catalog/TableTab.tsx` (`PartitioningPane` ~139–167, пропсы), `src/components/catalog/SequenceTab.tsx`,
`src/lib/format.ts` (`formatInt64` ~75–82, тест), тесты `TableTab.test.tsx`, `SequenceTab.test.tsx`; спека P1 (описание
полей последовательности ~540 и типов каталога), `docs/guide/{ru,en}/objects.md:35`.

**Нужно.**
1. Rust: `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(rename_all = "camelCase")] pub struct RelationRef { pub schema: String, pub name: String }`
   в `catalog` (там, где лежит `Partitioning`). `Partitioning.parent: Option<RelationRef>`, `partitions: Vec<RelationRef>` —
   сырые имена из каталога (`nspname`, `relname`), без `quote_ident`. SQL: подзапрос родителя отдаёт два столбца (или `ROW`
   → два столбца `h.get`), `PARTITIONS_SQL` — `n.nspname::text, c.relname::text`. Порядок партиций прежний.
   `ddl.rs`: квалифицированное имя с кавычками где нужно строит helper `fn qualified(r: &RelationRef) -> String`
   (через существующий `quote_ident`); `PARTITION OF` и `-- partitions:` используют его — текст DDL не меняется (тесты DDL
   и `it_catalog:607` это подтверждают). Тесты `table.rs`/`ddl.rs` обновить под структуру; `it_catalog` ~331:
   `part.partitioning.parent == Some(RelationRef { schema: "shop", name: "events" })`, список партиций — по `name`.
2. Rust: `serde_util::i64_string` — как `opt_i64_string`, но для `i64` (сериализация строкой, десериализация строки или
   числа), с тестами крайних значений. `SequenceDetails`: `start, increment, min, max, cache` — `#[serde(with = "pg_advisor::serde_util::i64_string")]`,
   `last_value` — `opt_i64_string`. `oid` остаётся числом. `percent_used` считается в Rust как сейчас. Тест JSON в
   `sequence.rs`: `max = i64::MAX` → `"max":"9223372036854775807"`, `last_value: None` → `null`.
3. TS: `Partitioning.parent: { schema: string; name: string } | null`, `partitions: { schema: string; name: string }[]`
   (тип `RelationRef` экспортировать из `types.ts`). `SequenceDetails.start/increment/min/max/cache: string`,
   `lastValue: string | null`. `formatInt64(n: number | string | null)`: строка — `BigInt(s.trim()).toLocaleString(locale)`
   (число — как сейчас). Тест `format.test`: `"9223372036854775807"` → `9 223 372 036 854 775 807` в ru (с неразрывными
   пробелами, как печатает `toLocaleString`), `"-5"` → `-5`.
4. TS `PartitioningPane`: родитель и каждая партиция — кнопки `className="link"` с текстом `schema.name`, по клику
   `useWorkspace((s) => s.open)({ kind: "table", connectionId, database, schema, name })`; `connectionId` и `database`
   пробросить из `TableTab`. Пустой список — как сейчас (прочерк). Тесты `TableTab.test.tsx`: моки с новой структурой; клик
   по родителю вызывает `open` с нужным `TabSpec` (как делает `ObjectList.test.tsx`); `SequenceTab.test.tsx`: строки вместо
   чисел, случай `max: "9223372036854775807"` показывает точное значение.
5. Документы: спека P1 — описание полей последовательности (~540): «числовые поля передаются строками (точность bigint выше
   2^53)»; описание `Partitioning` (если есть): родитель и партиции — `{ schema, name }`. Руководство `objects.md:35` ru/en:
   «родительская таблица и список партиций — ссылки, открывающие вкладку таблицы».

**Проверки:** `cargo test --lib catalog::`, `cargo test --manifest-path src-tauri/Cargo.toml -p pg-advisor serde_util`,
интеграционный `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 cargo test --manifest-path src-tauri/Cargo.toml --test it_catalog -- --test-threads=1`,
`pnpm vitest run src/components/catalog src/lib/format.test.ts`, `pnpm tsc --noEmit`, `pnpm i18n:check`, fmt, clippy.
