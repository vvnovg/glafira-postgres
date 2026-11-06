//! Запросы на оптимизацию (спец. P4 §8.1): снимок `LlmBundle` P3, статус, предложения модели. Файл `llm-requests.json`
//! в каталоге данных (0600, атомарная запись), последние `MAX_REQUESTS`; порядок в файле — порядок создания.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::list_file::ListFile;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::topology::model::now_ms;

pub const REQUESTS_FILE: &str = "llm-requests.json";
pub const MAX_REQUESTS: usize = 50;

/// Откуда запрос: консоль EXPLAIN или карточка запроса (`queryId` pgss строкой, как в интерфейсе).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RequestSource {
    Console,
    #[serde(rename_all = "camelCase")]
    Query {
        query_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    Waiting,
    Answered,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationRequest {
    /// `r` + 6 hex.
    pub id: String,
    pub connection_id: String,
    pub member: String,
    pub database: String,
    pub source: RequestSource,
    pub sql: String,
    /// Значения `$N` консоли (`None` — NULL); у карточки — пусто.
    #[serde(default)]
    pub params: Vec<Option<String>>,
    /// `LlmBundle` P3 как JSON: среда, текст, статистика, текст плана, DDL таблиц. Не маскируется (решение владельца 8).
    pub bundle: Value,
    pub created_at_ms: i64,
    pub status: RequestStatus,
    #[serde(default)]
    pub proposal_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NewRequest {
    pub connection_id: String,
    pub member: String,
    pub database: String,
    pub source: RequestSource,
    pub sql: String,
    pub params: Vec<Option<String>>,
    pub bundle: Value,
}

pub fn request_not_found(id: &str) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.llm.requestNotFound",
        "Optimization request {{id}} not found",
        id = id
    )
}

pub fn request_closed(id: &str) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.llm.requestClosed",
        "Optimization request {{id}} is cancelled",
        id = id
    )
}

pub struct RequestStore(ListFile<OptimizationRequest>);

impl RequestStore {
    pub fn open(dir: &Path) -> Self {
        Self(ListFile::open(dir, REQUESTS_FILE, "requests"))
    }

    fn update<T>(
        &self,
        f: impl FnOnce(&mut Vec<OptimizationRequest>) -> AppResult<T>,
    ) -> AppResult<T> {
        self.0.update(f)
    }
    pub fn create(&self, r: NewRequest) -> AppResult<OptimizationRequest> {
        self.update(|items| {
            let id = super::new_id('r', |id| items.iter().any(|x| x.id == id));
            let req = OptimizationRequest {
                id,
                connection_id: r.connection_id,
                member: r.member,
                database: r.database,
                source: r.source,
                sql: r.sql,
                params: r.params,
                bundle: r.bundle,
                created_at_ms: now_ms(),
                status: RequestStatus::Waiting,
                proposal_ids: Vec::new(),
            };
            items.push(req.clone());
            let extra = items.len().saturating_sub(MAX_REQUESTS);
            items.drain(..extra);
            Ok(req)
        })
    }

    /// «Отменить» в консоли и карточке: модель получит `request_closed`. Повторная отмена ничего не меняет.
    pub fn cancel(&self, id: &str) -> AppResult<OptimizationRequest> {
        self.update(|items| {
            let r = items
                .iter_mut()
                .find(|x| x.id == id)
                .ok_or_else(|| request_not_found(id))?;
            r.status = RequestStatus::Cancelled;
            Ok(r.clone())
        })
    }

    pub fn get(&self, id: &str) -> Option<OptimizationRequest> {
        self.0
            .read(|items| items.iter().find(|x| x.id == id).cloned())
    }

    /// Последний `waiting` среди подключений, открытых для модели (`allowed`): `get_optimization_request` без id.
    pub fn latest_waiting(&self, allowed: &dyn Fn(&str) -> bool) -> Option<OptimizationRequest> {
        self.0.read(|items| {
            items
                .iter()
                .rev()
                .find(|x| x.status == RequestStatus::Waiting && allowed(&x.connection_id))
                .cloned()
        })
    }

    /// Новое предложение модели: id в списке, статус `answered`. Отменённый запрос предложений не принимает.
    pub fn attach_proposal(&self, id: &str, proposal_id: &str) -> AppResult<()> {
        self.update(|items| {
            let r = items
                .iter_mut()
                .find(|x| x.id == id)
                .ok_or_else(|| request_not_found(id))?;
            if r.status == RequestStatus::Cancelled {
                return Err(request_closed(id));
            }
            if !r.proposal_ids.iter().any(|p| p == proposal_id) {
                r.proposal_ids.push(proposal_id.to_string());
            }
            r.status = RequestStatus::Answered;
            Ok(())
        })
    }

    /// Запросы подключения, сначала новые.
    pub fn list(&self, connection_id: &str) -> Vec<OptimizationRequest> {
        self.0.read(|items| {
            items
                .iter()
                .rev()
                .filter(|x| x.connection_id == connection_id)
                .cloned()
                .collect()
        })
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    pub(crate) fn new(conn: &str) -> NewRequest {
        NewRequest {
            connection_id: conn.into(),
            member: "pg-1".into(),
            database: "shop".into(),
            source: RequestSource::Console,
            sql: "SELECT * FROM orders WHERE status = $1".into(),
            params: vec![Some("new".into())],
            bundle: json!({ "planText": "Seq Scan on orders", "tables": [] }),
        }
    }

    #[test]
    fn requests_get_short_ids_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let s = RequestStore::open(dir.path());
        let r = s.create(new("c1")).unwrap();
        assert!(
            r.id.len() == 7
                && r.id.starts_with('r')
                && r.id[1..].chars().all(|c| c.is_ascii_hexdigit()),
            "{}",
            r.id
        );
        assert_eq!(r.status, RequestStatus::Waiting);
        assert_eq!(RequestStore::open(dir.path()).get(&r.id), Some(r.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(REQUESTS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn only_the_latest_fifty_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let s = RequestStore::open(dir.path());
        let first = s.create(new("c1")).unwrap();
        for _ in 0..MAX_REQUESTS {
            s.create(new("c1")).unwrap();
        }
        assert_eq!(s.list("c1").len(), MAX_REQUESTS);
        assert_eq!(s.get(&first.id), None, "самый старый вытеснен");
    }

    #[test]
    fn statuses_follow_proposals_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let s = RequestStore::open(dir.path());
        let r = s.create(new("c1")).unwrap();
        s.attach_proposal(&r.id, "p000001").unwrap();
        s.attach_proposal(&r.id, "p000001").unwrap();
        let got = s.get(&r.id).unwrap();
        assert_eq!(got.status, RequestStatus::Answered);
        assert_eq!(got.proposal_ids, ["p000001"]);
        assert_eq!(s.cancel(&r.id).unwrap().status, RequestStatus::Cancelled);
        assert_eq!(
            s.attach_proposal(&r.id, "p000002").unwrap_err().key,
            "errors.llm.requestClosed"
        );
        assert_eq!(
            s.cancel("r000000").unwrap_err().key,
            "errors.llm.requestNotFound"
        );
    }

    #[test]
    fn latest_waiting_skips_closed_requests_and_hidden_connections() {
        let dir = tempfile::tempdir().unwrap();
        let s = RequestStore::open(dir.path());
        let a = s.create(new("c1")).unwrap();
        let b = s.create(new("c2")).unwrap();
        let c = s.create(new("c1")).unwrap();
        s.cancel(&c.id).unwrap();
        assert_eq!(
            s.latest_waiting(&|_| true).map(|r| r.id),
            Some(b.id.clone())
        );
        assert_eq!(
            s.latest_waiting(&|id| id == "c1").map(|r| r.id),
            Some(a.id.clone())
        );
        assert_eq!(s.latest_waiting(&|_| false), None);
        let ids: Vec<String> = s.list("c1").into_iter().map(|r| r.id).collect();
        assert_eq!(ids, [c.id, a.id], "сначала новые");
    }

    #[test]
    fn json_shape_of_a_request() {
        let r = OptimizationRequest {
            id: "r3a9f1c".into(),
            connection_id: "c1".into(),
            member: "pg-1".into(),
            database: "shop".into(),
            source: RequestSource::Query {
                query_id: "-5".into(),
            },
            sql: "SELECT $1".into(),
            params: vec![None],
            bundle: json!({}),
            created_at_ms: 1,
            status: RequestStatus::Waiting,
            proposal_ids: vec![],
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["source"], json!({ "kind": "query", "queryId": "-5" }));
        assert_eq!(v["status"], "waiting");
        assert_eq!(v["connectionId"], "c1");
        assert_eq!(v["createdAtMs"], 1);
        assert_eq!(v["proposalIds"], json!([]));
        assert_eq!(
            serde_json::from_value::<RequestSource>(json!({ "kind": "console" })).unwrap(),
            RequestSource::Console
        );
    }

    #[test]
    fn a_file_from_a_newer_version_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REQUESTS_FILE);
        std::fs::write(&path, r#"{"schemaVersion": 9, "requests": []}"#).unwrap();
        let s = RequestStore::open(dir.path());
        assert_eq!(
            s.create(new("c1")).unwrap_err().key,
            "errors.storage.newerSchema"
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"schemaVersion\": 9")
        );
    }
}
