//! Предложения LLM (спец. P4 §8.3): переписанный запрос и индексы, ответ пользователя, итог «Сравнить планы». Файл
//! `llm-proposals.json` (0600, атомарная запись), до `MAX_PROPOSALS`, до `MAX_PER_REQUEST` на запрос; порядок в файле —
//! порядок создания.

use std::path::Path;

use pg_advisor::plan::PlanTree;
use pg_advisor::plan_rules::Recommendation;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::list_file::ListFile;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::topology::model::now_ms;

pub const PROPOSALS_FILE: &str = "llm-proposals.json";
pub const MAX_PROPOSALS: usize = 100;
pub const MAX_PER_REQUEST: usize = 5;
pub const MAX_REASON_CHARS: usize = 500;

/// Откуда предложение: модель через MCP; `app` — этап «LLM в приложении» (спец. §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalSource {
    Mcp,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    New,
    Accepted,
    Rejected,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::New => "new",
            ProposalStatus::Accepted => "accepted",
            ProposalStatus::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexUse {
    pub ddl: String,
    pub used: bool,
}

/// Итог «Сравнить планы» (спец. §8.4): планы обоих запросов без выполнения на одном узле, рекомендации P3b, индексы.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub cost_before: f64,
    pub cost_after: f64,
    pub indexes_used: Vec<IndexUse>,
    pub member: String,
    pub at_ms: i64,
    pub plan_before: PlanTree,
    pub plan_after: PlanTree,
    pub recommendations_before: Vec<Recommendation>,
    pub recommendations_after: Vec<Recommendation>,
    /// hypopg в базе нет: предложенный запрос сравнивался без индексов.
    pub without_hypopg: bool,
}

impl Comparison {
    /// Итог для модели (`list_proposals`, спец. §8.2–§8.3): без планов и рекомендаций.
    pub fn for_model(&self) -> Value {
        json!({
            "costBefore": self.cost_before,
            "costAfter": self.cost_after,
            "indexesUsed": self.indexes_used,
            "member": self.member,
            "atMs": self.at_ms,
            "withoutHypopg": self.without_hypopg,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProposal {
    /// `p` + 6 hex.
    pub id: String,
    pub source: ProposalSource,
    /// Имя клиента MCP (`qwen-code`…); `None` — безымянный клиент.
    pub client: Option<String>,
    pub request_id: Option<String>,
    pub connection_id: String,
    pub database: String,
    pub member: Option<String>,
    pub original_sql: String,
    pub original_query_id: Option<String>,
    pub rewritten_sql: String,
    #[serde(default)]
    pub indexes: Vec<String>,
    pub rationale: String,
    pub expected_effect: Option<String>,
    pub status: ProposalStatus,
    pub reason: Option<String>,
    pub comparison: Option<Comparison>,
    pub created_at_ms: i64,
    pub answered_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewProposal {
    pub source: ProposalSource,
    pub client: Option<String>,
    pub request_id: Option<String>,
    pub connection_id: String,
    pub database: String,
    pub member: Option<String>,
    pub original_sql: String,
    pub original_query_id: Option<String>,
    pub rewritten_sql: String,
    pub indexes: Vec<String>,
    pub rationale: String,
    pub expected_effect: Option<String>,
}

/// Чьи предложения показывать. `connection_id` — всегда условие. Остальное — «или»: по запросам (`request_ids`), по
/// базе для предложений без запроса (`database`; панель консоли), по `queryId` исходного запроса (`query_id`; карточка).
/// Без них — все предложения подключения.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalFilter {
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub request_ids: Option<Vec<String>>,
    #[serde(default)]
    pub query_id: Option<String>,
}

impl ProposalFilter {
    /// `connection_id` сужает всегда; `request_ids`, `database` и `query_id` объединяются (ИЛИ): подходит
    /// предложение, совпавшее хотя бы с одним из заданных; если ни одного нет, подходят все.
    pub fn matches(&self, p: &LlmProposal) -> bool {
        if self
            .connection_id
            .as_ref()
            .is_some_and(|c| *c != p.connection_id)
        {
            return false;
        }
        if self.request_ids.is_none() && self.database.is_none() && self.query_id.is_none() {
            return true;
        }
        let by_request = self
            .request_ids
            .as_ref()
            .is_some_and(|ids| p.request_id.as_ref().is_some_and(|r| ids.contains(r)));
        let by_database = self
            .database
            .as_ref()
            .is_some_and(|d| p.request_id.is_none() && *d == p.database);
        let by_query = self
            .query_id
            .as_ref()
            .is_some_and(|q| p.original_query_id.as_ref() == Some(q));
        by_request || by_database || by_query
    }
}

pub fn proposal_not_found(id: &str) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.llm.proposalNotFound",
        "LLM proposal {{id}} not found",
        id = id
    )
}

pub fn bad_status() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.llm.badStatus",
        "The answer must be accepted or rejected"
    )
}

fn reason_too_long() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.llm.reasonTooLong",
        "The reason is longer than {{max}} characters",
        max = MAX_REASON_CHARS
    )
}

fn too_many(id: &str) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.llm.tooManyProposals",
        "Optimization request {{id}} already has {{max}} proposals",
        id = id,
        max = MAX_PER_REQUEST
    )
}

/// Тот же исходный запрос: тот же запрос на оптимизацию, а без него — подключение, база и SQL.
fn same_origin(p: &LlmProposal, n: &NewProposal) -> bool {
    match (&p.request_id, &n.request_id) {
        (Some(a), Some(b)) => a == b,
        (None, None) => {
            p.connection_id == n.connection_id
                && p.database == n.database
                && p.original_sql.trim() == n.original_sql.trim()
        }
        _ => false,
    }
}

fn index_set(v: &[String]) -> Vec<String> {
    let mut s: Vec<String> = v.iter().map(|d| d.trim().to_string()).collect();
    s.sort();
    s
}

/// Дедупликация (спец. §8.2): тот же исходный запрос, `rewrittenSql` и набор индексов.
fn same_proposal(p: &LlmProposal, n: &NewProposal) -> bool {
    same_origin(p, n)
        && p.rewritten_sql.trim() == n.rewritten_sql.trim()
        && index_set(&p.indexes) == index_set(&n.indexes)
}

/// Не больше `MAX_PROPOSALS`: сначала уходят самые старые с ответом, затем самые старые новые.
fn evict(items: &mut Vec<LlmProposal>) {
    while items.len() > MAX_PROPOSALS {
        let i = items
            .iter()
            .position(|p| p.status != ProposalStatus::New)
            .unwrap_or(0);
        items.remove(i);
    }
}

pub struct ProposalStore(ListFile<LlmProposal>);

impl ProposalStore {
    pub fn open(dir: &Path) -> Self {
        Self(ListFile::open(dir, PROPOSALS_FILE, "proposals"))
    }

    fn update<T>(&self, f: impl FnOnce(&mut Vec<LlmProposal>) -> AppResult<T>) -> AppResult<T> {
        self.0.update(f)
    }

    /// Новое предложение или уже сохранённое такое же (`false`); предел на запрос — `tooManyProposals`.
    pub fn add_or_existing(&self, n: NewProposal) -> AppResult<(LlmProposal, bool)> {
        if let Some(p) = self
            .0
            .read(|items| items.iter().find(|p| same_proposal(p, &n)).cloned())
        {
            return Ok((p, false));
        }
        self.update(|items| {
            if let Some(p) = items.iter().find(|p| same_proposal(p, &n)) {
                return Ok((p.clone(), false));
            }
            if let Some(rid) = &n.request_id
                && items
                    .iter()
                    .filter(|p| p.request_id.as_ref() == Some(rid))
                    .count()
                    >= MAX_PER_REQUEST
            {
                return Err(too_many(rid));
            }
            let proposal = LlmProposal {
                id: super::new_id('p', |id| items.iter().any(|x| x.id == id)),
                source: n.source,
                client: n.client,
                request_id: n.request_id,
                connection_id: n.connection_id,
                database: n.database,
                member: n.member,
                original_sql: n.original_sql,
                original_query_id: n.original_query_id,
                rewritten_sql: n.rewritten_sql,
                indexes: n.indexes,
                rationale: n.rationale,
                expected_effect: n.expected_effect,
                status: ProposalStatus::New,
                reason: None,
                comparison: None,
                created_at_ms: now_ms(),
                answered_at_ms: None,
            };
            items.push(proposal.clone());
            evict(items);
            Ok((proposal, true))
        })
    }

    /// Ответ пользователя: `accepted` или `rejected` с необязательной причиной до `MAX_REASON_CHARS` (видна модели).
    pub fn answer(
        &self,
        id: &str,
        status: ProposalStatus,
        reason: Option<String>,
    ) -> AppResult<LlmProposal> {
        if status == ProposalStatus::New {
            return Err(bad_status());
        }
        let reason = match status {
            ProposalStatus::Rejected => reason
                .map(|r| super::plain_text(&r))
                .filter(|r| !r.is_empty()),
            _ => None,
        };
        if reason
            .as_ref()
            .is_some_and(|r| r.chars().count() > MAX_REASON_CHARS)
        {
            return Err(reason_too_long());
        }
        self.update(|items| {
            let p = items
                .iter_mut()
                .find(|x| x.id == id)
                .ok_or_else(|| proposal_not_found(id))?;
            p.status = status;
            p.reason = reason;
            p.answered_at_ms = Some(now_ms());
            Ok(p.clone())
        })
    }

    pub fn set_comparison(&self, id: &str, c: Comparison) -> AppResult<LlmProposal> {
        self.update(|items| {
            let p = items
                .iter_mut()
                .find(|x| x.id == id)
                .ok_or_else(|| proposal_not_found(id))?;
            p.comparison = Some(c);
            Ok(p.clone())
        })
    }

    pub fn get(&self, id: &str) -> Option<LlmProposal> {
        self.0
            .read(|items| items.iter().find(|x| x.id == id).cloned())
    }

    /// Предложения по фильтру, сначала новые.
    pub fn list(&self, f: &ProposalFilter) -> Vec<LlmProposal> {
        self.0.read(|items| {
            items
                .iter()
                .rev()
                .filter(|p| f.matches(p))
                .cloned()
                .collect()
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    pub(crate) fn new(request: Option<&str>, sql: &str, indexes: &[&str]) -> NewProposal {
        NewProposal {
            source: ProposalSource::Mcp,
            client: Some("qwen-code".into()),
            request_id: request.map(String::from),
            connection_id: "c1".into(),
            database: "shop".into(),
            member: Some("pg-1".into()),
            original_sql: "SELECT * FROM orders WHERE status = 'new'".into(),
            original_query_id: None,
            rewritten_sql: sql.into(),
            indexes: indexes.iter().map(|s| s.to_string()).collect(),
            rationale: "An index on status avoids the Seq Scan".into(),
            expected_effect: Some("cost 12000 → 40".into()),
        }
    }

    fn plan(cost: f64) -> PlanTree {
        pg_advisor::plan::json::from_json(&json!([{ "Plan": {
            "Node Type": "Result", "Startup Cost": 0, "Total Cost": cost, "Plan Rows": 1, "Plan Width": 4
        } }]))
        .unwrap()
    }

    fn comparison() -> Comparison {
        Comparison {
            cost_before: 12000.0,
            cost_after: 40.0,
            indexes_used: vec![IndexUse {
                ddl: "CREATE INDEX ON orders (status)".into(),
                used: true,
            }],
            member: "pg-1".into(),
            at_ms: 5,
            plan_before: plan(12000.0),
            plan_after: plan(40.0),
            recommendations_before: vec![],
            recommendations_after: vec![],
            without_hypopg: false,
        }
    }

    #[test]
    fn the_same_proposal_returns_the_existing_one() {
        let dir = tempfile::tempdir().unwrap();
        let s = ProposalStore::open(dir.path());
        let (p, created) = s
            .add_or_existing(new(
                Some("r1"),
                "SELECT id FROM orders WHERE status = 'new'",
                &[
                    "CREATE INDEX ON orders (status)",
                    "CREATE INDEX ON orders (id)",
                ],
            ))
            .unwrap();
        assert!(created);
        assert!(p.id.len() == 7 && p.id.starts_with('p'), "{}", p.id);
        assert_eq!(p.status, ProposalStatus::New);
        // Тот же запрос, SQL (пробелы по краям не в счёт) и набор индексов в другом порядке — то же предложение.
        let (again, created) = s
            .add_or_existing(new(
                Some("r1"),
                "  SELECT id FROM orders WHERE status = 'new' ",
                &[
                    "CREATE INDEX ON orders (id)",
                    "CREATE INDEX ON orders (status)",
                ],
            ))
            .unwrap();
        assert_eq!((again.id.as_str(), created), (p.id.as_str(), false));
        // Другой SQL или другой запрос — новое.
        assert!(
            s.add_or_existing(new(Some("r1"), "SELECT 2", &[]))
                .unwrap()
                .1
        );
        assert!(
            s.add_or_existing(new(
                Some("r2"),
                "SELECT id FROM orders WHERE status = 'new'",
                &[]
            ))
            .unwrap()
            .1
        );
        // Без запроса — по подключению, базе и исходному SQL.
        let (a, _) = s.add_or_existing(new(None, "SELECT 3", &[])).unwrap();
        assert_eq!(
            s.add_or_existing(new(None, "SELECT 3", &[])).unwrap().0.id,
            a.id
        );
        assert_eq!(ProposalStore::open(dir.path()).get(&a.id), Some(a));
    }

    #[test]
    fn at_most_five_per_request_and_a_hundred_in_all() {
        let dir = tempfile::tempdir().unwrap();
        let s = ProposalStore::open(dir.path());
        for i in 0..MAX_PER_REQUEST {
            s.add_or_existing(new(Some("r1"), &format!("SELECT {i}"), &[]))
                .unwrap();
        }
        let e = s
            .add_or_existing(new(Some("r1"), "SELECT 99", &[]))
            .unwrap_err();
        assert_eq!(e.key, "errors.llm.tooManyProposals");
        // Без запроса предела на запрос нет; всего — до 100: сначала уходят старые с ответом, затем старые новые.
        let mut ids = Vec::new();
        for i in 0..(MAX_PROPOSALS - MAX_PER_REQUEST) {
            ids.push(
                s.add_or_existing(new(None, &format!("SELECT {i}"), &[]))
                    .unwrap()
                    .0
                    .id,
            );
        }
        assert_eq!(s.list(&ProposalFilter::default()).len(), MAX_PROPOSALS);
        let answered = ids[10].clone();
        s.answer(&answered, ProposalStatus::Accepted, None).unwrap();
        s.add_or_existing(new(None, "SELECT 1000", &[])).unwrap();
        assert_eq!(s.get(&answered), None, "старое с ответом вытеснено первым");
        assert!(
            s.list(&ProposalFilter::default())
                .iter()
                .any(|p| p.request_id.as_deref() == Some("r1"))
        );
        s.add_or_existing(new(None, "SELECT 1001", &[])).unwrap();
        assert_eq!(s.list(&ProposalFilter::default()).len(), MAX_PROPOSALS);
        assert!(
            s.list(&ProposalFilter {
                request_ids: Some(vec!["r1".into()]),
                ..Default::default()
            })
            .len()
                == MAX_PER_REQUEST - 1,
            "затем — самое старое новое (первое предложение r1)"
        );
    }

    #[test]
    fn answers_and_comparison() {
        let dir = tempfile::tempdir().unwrap();
        let s = ProposalStore::open(dir.path());
        let (p, _) = s.add_or_existing(new(Some("r1"), "SELECT 1", &[])).unwrap();
        let r = s
            .answer(
                &p.id,
                ProposalStatus::Rejected,
                Some("  slower on prod\u{7} ".into()),
            )
            .unwrap();
        assert_eq!(
            (r.status, r.reason.as_deref()),
            (ProposalStatus::Rejected, Some("slower on prod"))
        );
        assert!(r.answered_at_ms.is_some());
        let a = s
            .answer(&p.id, ProposalStatus::Accepted, Some("ignored".into()))
            .unwrap();
        assert_eq!((a.status, a.reason), (ProposalStatus::Accepted, None));
        assert_eq!(
            s.answer(&p.id, ProposalStatus::New, None).unwrap_err().key,
            "errors.llm.badStatus"
        );
        let long = "x".repeat(MAX_REASON_CHARS + 1);
        assert_eq!(
            s.answer(&p.id, ProposalStatus::Rejected, Some(long))
                .unwrap_err()
                .key,
            "errors.llm.reasonTooLong"
        );
        assert_eq!(
            s.answer("p000000", ProposalStatus::Accepted, None)
                .unwrap_err()
                .key,
            "errors.llm.proposalNotFound"
        );
        let c = s.set_comparison(&p.id, comparison()).unwrap();
        assert_eq!(c.comparison.as_ref().unwrap().cost_after, 40.0);
        assert_eq!(
            ProposalStore::open(dir.path())
                .get(&p.id)
                .unwrap()
                .comparison,
            Some(comparison())
        );
    }

    #[test]
    fn comparison_for_the_model_has_no_plans() {
        let v = comparison().for_model();
        assert_eq!(
            v,
            json!({
                "costBefore": 12000.0, "costAfter": 40.0,
                "indexesUsed": [{ "ddl": "CREATE INDEX ON orders (status)", "used": true }],
                "member": "pg-1", "atMs": 5, "withoutHypopg": false
            })
        );
    }

    #[test]
    fn filter_of_the_console_and_of_the_card() {
        let p = |request: Option<&str>, database: &str, query_id: Option<&str>| LlmProposal {
            id: "p1".into(),
            source: ProposalSource::Mcp,
            client: None,
            request_id: request.map(String::from),
            connection_id: "c1".into(),
            database: database.into(),
            member: None,
            original_sql: "SELECT 1".into(),
            original_query_id: query_id.map(String::from),
            rewritten_sql: "SELECT 1".into(),
            indexes: vec![],
            rationale: "r".into(),
            expected_effect: None,
            status: ProposalStatus::New,
            reason: None,
            comparison: None,
            created_at_ms: 1,
            answered_at_ms: None,
        };
        let console = ProposalFilter {
            connection_id: Some("c1".into()),
            database: Some("shop".into()),
            request_ids: Some(vec!["r1".into()]),
            query_id: None,
        };
        assert!(
            console.matches(&p(Some("r1"), "other", None)),
            "по запросу консоли"
        );
        assert!(
            console.matches(&p(None, "shop", None)),
            "без запроса — по базе"
        );
        assert!(
            !console.matches(&p(Some("r2"), "shop", None)),
            "чужой запрос"
        );
        let card = ProposalFilter {
            connection_id: Some("c1".into()),
            query_id: Some("-5".into()),
            ..Default::default()
        };
        assert!(card.matches(&p(None, "shop", Some("-5"))));
        assert!(!card.matches(&p(None, "shop", Some("7"))));
        let all_of_c2 = ProposalFilter {
            connection_id: Some("c2".into()),
            ..Default::default()
        };
        assert!(!all_of_c2.matches(&p(None, "shop", None)));
        assert!(ProposalFilter::default().matches(&p(None, "shop", None)));
        let f: ProposalFilter =
            serde_json::from_value(json!({ "connectionId": "c1", "requestIds": ["r1"] })).unwrap();
        assert_eq!(f.request_ids, Some(vec!["r1".to_string()]));
    }

    #[test]
    fn json_shape_of_a_proposal() {
        let dir = tempfile::tempdir().unwrap();
        let s = ProposalStore::open(dir.path());
        let (p, _) = s.add_or_existing(new(None, "SELECT 1", &[])).unwrap();
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["source"], "mcp");
        assert_eq!(v["status"], "new");
        assert_eq!(v["rewrittenSql"], "SELECT 1");
        assert_eq!(v["requestId"], serde_json::Value::Null);
        assert_eq!(ProposalStatus::Rejected.as_str(), "rejected");
    }
}
