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
    Session {
        pid: i32,
        backend_start_us: i64,
    },
    Slot {
        name: String,
    },
    Member {
        name: String,
    },
    Relation {
        name: String,
    },
    Database {
        name: String,
    },
}

/// Куда ведёт кнопка находки. `tool` — id вкладки инструмента во фронтенде (`sessions`, `idleTx`, `locks`…).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FindingLink {
    OpenTool {
        tool: String,
        member: Option<String>,
    },
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
        Self {
            id: id.to_string(),
            severity,
            member: None,
            target: None,
            params: Map::new(),
            links: Vec::new(),
        }
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
        self.links.push(FindingLink::OpenTool {
            tool: tool.to_string(),
            member: member.map(str::to_string),
        });
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
    /// Доля `Lock` оценивается, если в окне не меньше сэмплов.
    pub const LOCK_SHARE_MIN_SAMPLES: u64 = 30;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grade_picks_the_highest_reached_level() {
        assert_eq!(grade(10.0, 30.0, 300.0), None);
        assert_eq!(grade(30.0, 30.0, 300.0), Some(Severity::Warning));
        assert_eq!(grade(299.9, 30.0, 300.0), Some(Severity::Warning));
        assert_eq!(grade(300.0, 30.0, 300.0), Some(Severity::Critical));
        assert_eq!(grade(f64::NAN, 30.0, 300.0), None);
        assert_eq!(
            worse(Some(Severity::Warning), None),
            Some(Severity::Warning)
        );
        assert_eq!(
            worse(Some(Severity::Warning), Some(Severity::Critical)),
            Some(Severity::Critical)
        );
    }

    #[test]
    fn finding_json_is_camel_case_with_tagged_target_and_links() {
        let f = Finding::new("idleInTransaction", Severity::Critical)
            .member(Some("pg-1"))
            .target(FindingTarget::Session {
                pid: 7,
                backend_start_us: 1_700_000_000_000_001,
            })
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
        let slot = Finding::new("slotLost", Severity::Critical)
            .target(FindingTarget::Slot { name: "s".into() });
        assert_eq!(
            serde_json::to_value(&slot).unwrap()["target"],
            json!({"kind": "slot", "name": "s"})
        );
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
        let order: Vec<(&str, Option<&str>)> = v
            .iter()
            .map(|f| (f.id.as_str(), f.member.as_deref()))
            .collect();
        assert_eq!(
            order,
            [
                ("a", Some("x")),
                ("c", Some("x")),
                ("a", Some("y")),
                ("z", None),
                ("b", Some("x"))
            ]
        );
    }
}
