//! Каркас диагностики (спец. P2 §3): инструменты, запрос, конверт отчёта, узел «за лидером» и диспетчер
//! `run_diagnostic`. Сбор фактов (SQL, HTTP) — здесь, правила и типы данных — в `pg_advisor::rules`.

pub mod caps;
pub mod horizon;
pub mod locks;
pub mod memory;
pub mod pulse;
pub mod replication;
pub mod sessions;
pub mod sql;

use futures_util::future::BoxFuture;
use pg_advisor::findings::Finding;
use pg_advisor::locktree::LocksData;
use pg_advisor::rules::horizon::HorizonData;
use pg_advisor::rules::replication::ReplicationData;
use pg_advisor::rules::sessions::SessionsData;
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::commands::catalog::{catalog_limit, on_catalog_node_within};
use crate::conn::errors::from_pg;
use crate::conn::profile::ConnectionKind;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::i18n::LocalizedText;
use crate::state::AppState;
use crate::topology::model::Topology;
use crate::topology::runtime::DiagTarget;

/// Инструмент диагностики. `idleTx` — вкладка фронтенда поверх `sessions`, отдельного инструмента в Rust нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tool {
    Pulse,
    Sessions,
    Waits,
    Locks,
    Horizon,
    Replication,
    Slots,
    Failover,
    Health,
}

impl Tool {
    /// Инструмент кластера: опрашивает участников сам, узла не берёт (`member` запроса игнорируется).
    pub fn is_cluster(self) -> bool {
        matches!(self, Tool::Pulse | Tool::Failover | Tool::Health)
    }

    /// Только для кластера Patroni.
    pub fn patroni_only(self) -> bool {
        matches!(self, Tool::Failover | Tool::Health)
    }
}

/// Окна профиля ожиданий, секунды.
pub const WINDOWS_S: [u32; 3] = [300, 900, 3600];
pub const DEFAULT_WINDOW_S: u32 = 900;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagRequest {
    pub tool: Tool,
    /// `None` — «следовать за лидером»; у инструментов кластера игнорируется.
    pub member: Option<String>,
    /// Только `waits`: одно из `WINDOWS_S`, по умолчанию `DEFAULT_WINDOW_S`.
    pub window_s: Option<u32>,
}

/// Что не показано и почему (версия сервера, нет права).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unavailable {
    pub part: String,
    /// Узел — у инструментов кластера; у узловых пусто и в JSON не выводится.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    pub reason: LocalizedText,
}

/// Данные инструмента; `kind` — id инструмента. Отчёт живёт недолго и один на запрос: разница в размере вариантов
/// (данные реплик втрое больше остальных) обходится без `Box`, чтобы сопоставление с образцом в тестах и коде осталось
/// прямым.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
pub enum DiagData {
    Pulse(pulse::PulseData),
    Sessions(SessionsData),
    Locks(LocksData),
    Horizon(HorizonData),
    Replication(ReplicationData),
}

/// Конверт отчёта (спец. P2 §3.2). У инструментов кластера `member`, `serverVersionNum` и `inRecovery` пусты.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagReport {
    pub tool: Tool,
    pub member: Option<String>,
    pub follows_leader: bool,
    /// Время сервера (узловые) или приложения (кластера), мс эпохи.
    pub observed_at_ms: i64,
    pub server_version_num: Option<i32>,
    pub in_recovery: Option<bool>,
    pub data: DiagData,
    pub findings: Vec<Finding>,
    pub unavailable: Vec<Unavailable>,
}

pub fn validate(req: &DiagRequest, kind: ConnectionKind) -> AppResult<()> {
    if req.tool.patroni_only() && kind != ConnectionKind::Patroni {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.diag.patroniOnly",
            "This tool is available only for Patroni clusters"
        ));
    }
    if let Some(w) = req.window_s
        && !WINDOWS_S.contains(&w)
    {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.diag.badWindow",
            "The window must be 300, 900 or 3600 seconds"
        ));
    }
    Ok(())
}

/// Первая строка узлового отчёта: версия, роль узла, время сервера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head {
    pub version_num: i32,
    pub in_recovery: bool,
    pub now_ms: i64,
}

pub async fn head(c: &Client) -> AppResult<Head> {
    let r = c
        .query_one(sql::HEAD_SQL, &[])
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(Head {
        version_num: r.get(0),
        in_recovery: r.get(1),
        now_ms: r.get(2),
    })
}

/// Видит ли роль соединения сведения чужих сессий (`pg_read_all_stats`, входит в `pg_monitor`; суперпользователь — да).
/// `USAGE`, а не `MEMBER`: привилегии роли доходят до сессии только при наследовании (`NOINHERIT` членство не
/// открывает чужую статистику) — так же проверяет сам сервер.
pub const READS_ALL_STATS_SQL: &str =
    "SELECT pg_has_role(current_user, 'pg_read_all_stats', 'USAGE')";

/// Один раз за сбор: без права сервер прячет чужие сессии (`backend_xmin` и `backend_xid`, состояние приёмника WAL и
/// отправителей, их LSN), и пустые поля нельзя читать как «всё в порядке» или «всё сломано».
pub async fn reads_all_stats(c: &Client) -> AppResult<bool> {
    let r = c
        .query_one(READS_ALL_STATS_SQL, &[])
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(r.get::<_, Option<bool>>(0).unwrap_or(false))
}

/// Пометка «часть скрыта: нужна роль `pg_monitor`».
pub fn needs_monitor(part: &str) -> Unavailable {
    Unavailable {
        part: part.to_string(),
        member: None,
        reason: ltext!(
            "diag.unavailable.privilege",
            "{{part}}: needs the pg_monitor role",
            part = part
        ),
    }
}

/// Участник топологии для `application_name` отправителя WAL (уточнение 5 плана P2a): имя участника или его хост.
/// Без DNS: опрос топологии разрешает имена, диагностика — нет. Не сопоставилось — `None`.
pub fn sender_member(t: &Topology, application: Option<&str>) -> Option<String> {
    let app = application?;
    t.members
        .iter()
        .find(|m| m.name == app || m.host == app)
        .map(|m| m.name.clone())
}

/// Запрос узлового инструмента на его узле: соединение пула `Catalog`, предел — `catalog_limit` профиля.
pub(crate) async fn on_member<T, F>(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    query: F,
) -> AppResult<T>
where
    T: Send,
    F: for<'a> FnOnce(&'a Client) -> BoxFuture<'a, AppResult<T>> + Send,
{
    let m = target
        .member
        .as_ref()
        .map(|m| m.name.clone())
        .ok_or_else(|| AppError::internal_with("node tool without a member"))?;
    on_catalog_node_within(
        catalog_limit(&target.profile),
        st.client(id, Some(&m), None),
        query,
    )
    .await
}

pub(crate) fn make_report(
    tool: Tool,
    target: &DiagTarget,
    head: Option<&Head>,
    data: DiagData,
    mut findings: Vec<Finding>,
    unavailable: Vec<Unavailable>,
) -> DiagReport {
    pg_advisor::findings::sort(&mut findings);
    DiagReport {
        tool,
        member: target.member.as_ref().map(|m| m.name.clone()),
        follows_leader: target.follows_leader,
        observed_at_ms: head
            .map(|h| h.now_ms)
            .unwrap_or_else(crate::topology::model::now_ms),
        server_version_num: head.map(|h| h.version_num),
        in_recovery: head.map(|h| h.in_recovery),
        data,
        findings,
        unavailable,
    }
}

/// `run_diagnostic`: узел инструмента, проверка запроса, сбор и правила.
pub async fn run(st: &AppState, id: &str, req: DiagRequest) -> AppResult<DiagReport> {
    let target = st
        .runtimes
        .diag_target(id, req.member.as_deref(), req.tool.is_cluster())?;
    validate(&req, target.profile.kind)?;
    match req.tool {
        Tool::Pulse => pulse::report(st, id, &target).await,
        Tool::Sessions => sessions::report(st, id, &target).await,
        Tool::Locks => locks::report(st, id, &target).await,
        Tool::Horizon => horizon::report(st, id, &target).await,
        Tool::Replication => replication::report(st, id, &target).await,
        // Остальные задачи добавляют свои ветки; до тех пор инструмент недоступен.
        other => Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.diag.notYet",
            "The tool {{tool}} is not available yet",
            tool = format!("{other:?}")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_validation() {
        let req = |tool, window_s| DiagRequest {
            tool,
            member: None,
            window_s,
        };
        assert_eq!(
            validate(&req(Tool::Failover, None), ConnectionKind::Server)
                .unwrap_err()
                .key,
            "errors.diag.patroniOnly"
        );
        assert!(validate(&req(Tool::Health, None), ConnectionKind::Patroni).is_ok());
        assert_eq!(
            validate(&req(Tool::Waits, Some(60)), ConnectionKind::Server)
                .unwrap_err()
                .key,
            "errors.diag.badWindow"
        );
        assert!(validate(&req(Tool::Waits, Some(3600)), ConnectionKind::Server).is_ok());
        assert!(Tool::Pulse.is_cluster() && !Tool::Sessions.is_cluster());
    }

    #[test]
    fn horizon_and_replication_data_are_tagged_with_the_tool_id() {
        let h = serde_json::to_value(DiagData::Horizon(HorizonData::default())).unwrap();
        assert_eq!(
            (&h["kind"], &h["holders"]),
            (&"horizon".into(), &serde_json::json!([]))
        );
        let r = serde_json::to_value(DiagData::Replication(ReplicationData::default())).unwrap();
        assert_eq!(r["kind"], "replication");
        assert_eq!(r["inRecovery"], false);
    }

    #[test]
    fn sender_member_needs_an_application_name() {
        let t = crate::topology::model::topo(vec![crate::topology::model::Member::new(
            "pg-1", "db1", 5432,
        )]);
        assert_eq!(sender_member(&t, Some("pg-1")).as_deref(), Some("pg-1"));
        assert_eq!(sender_member(&t, Some("db1")).as_deref(), Some("pg-1"));
        assert_eq!(sender_member(&t, Some("other")), None);
        assert_eq!(sender_member(&t, None), None);
    }

    #[test]
    fn request_and_report_json() {
        assert!(
            serde_json::from_value::<DiagRequest>(serde_json::json!({"tool": "idleTx"})).is_err(),
            "idleTx — вкладка фронтенда, в Rust это инструмент sessions"
        );
        let r: DiagRequest =
            serde_json::from_value(serde_json::json!({"tool": "sessions", "member": null}))
                .unwrap();
        assert_eq!((r.tool, r.member, r.window_s), (Tool::Sessions, None, None));
        let report = DiagReport {
            tool: Tool::Sessions,
            member: Some("pg-1".into()),
            follows_leader: true,
            observed_at_ms: 5,
            server_version_num: Some(170004),
            in_recovery: Some(false),
            data: DiagData::Sessions(SessionsData {
                rows: vec![],
                settings: Default::default(),
            }),
            findings: vec![],
            unavailable: vec![],
        };
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["tool"], "sessions");
        assert_eq!(v["followsLeader"], true);
        assert_eq!(v["data"]["kind"], "sessions");
        assert_eq!(v["data"]["rows"], serde_json::json!([]));
        assert_eq!(v["serverVersionNum"], 170004);
    }
}
