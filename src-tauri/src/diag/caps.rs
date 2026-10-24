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
        if self.at_least(since) {
            expr.to_string()
        } else {
            format!("NULL::{ty}")
        }
    }

    /// Пометка «часть недоступна в этой версии»; `None` — доступна.
    pub fn missing(self, since: i32, part: &str) -> Option<Unavailable> {
        (!self.at_least(since)).then(|| Unavailable {
            part: part.to_string(),
            member: None,
            reason: ltext!(
                "diag.unavailable.version",
                "{{part}}: PostgreSQL {{version}}+",
                part = part,
                version = since / 10_000
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
