//! Вкладка функции и процедуры (спец. P1 §9): сигнатура, результат, язык, изменчивость, определение.

use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::{AppError, AppResult, ErrorCode};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDetails {
    pub oid: i64,
    pub schema: String,
    pub name: String,
    /// `prokind`: `f` — функция, `p` — процедура, `a` — агрегат, `w` — оконная.
    pub kind: String,
    /// Аргументы (`pg_get_function_identity_arguments`).
    pub signature: String,
    /// Тип результата; у процедуры и агрегата — `None`.
    pub returns: Option<String>,
    pub language: String,
    /// `provolatile`: `i`, `s`, `v`.
    pub volatility: String,
    pub security_definer: bool,
    pub owner: String,
    pub comment: Option<String>,
    /// `CREATE OR REPLACE …` (`pg_get_functiondef`); у агрегата — `None` (функция его не строит).
    pub definition: Option<String>,
}

const SQL: &str = "SELECT n.nspname::text, p.proname::text, p.prokind::text, \
    COALESCE(pg_get_function_identity_arguments(p.oid), ''), \
    CASE WHEN p.prokind IN ('f', 'w') THEN pg_get_function_result(p.oid) END, l.lanname::text, \
    p.provolatile::text, p.prosecdef, pg_get_userbyid(p.proowner)::text, obj_description(p.oid, 'pg_proc'), \
    CASE WHEN p.prokind = 'a' THEN NULL ELSE pg_get_functiondef(p.oid) END \
  FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace JOIN pg_language l ON l.oid = p.prolang \
  WHERE p.oid = $1";

/// Функции с таким OID нет (удалена после того, как её показало дерево, или число не OID). Своё сообщение: схемы
/// у запроса нет, и `object_not_found("", oid)` дало бы «Объект .16401 не найден». Код тот же, что у `objectNotFound`.
fn function_not_found(oid: i64) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.catalog.functionNotFound",
        "Function with oid {{oid}} was not found",
        oid = oid
    )
}

/// Функция по OID (имена перегружены, в дереве у функции — OID).
pub async fn get(c: &Client, oid: i64) -> AppResult<FunctionDetails> {
    let not_found = || function_not_found(oid);
    let Ok(pg_oid) = u32::try_from(oid) else {
        return Err(not_found());
    };
    let r = c
        .query_opt(SQL, &[&pg_oid])
        .await
        .map_err(|e| from_pg(&e))?
        .ok_or_else(not_found)?;
    Ok(FunctionDetails {
        oid,
        schema: r.get(0),
        name: r.get(1),
        kind: r.get(2),
        signature: r.get(3),
        returns: r.get(4),
        language: r.get(5),
        volatility: r.get(6),
        security_definer: r.get(7),
        owner: r.get(8),
        comment: r.get(9),
        definition: r.get(10),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_not_found_names_the_oid_and_is_not_found_like_other_objects() {
        let e = function_not_found(16401);
        assert_eq!(e.key, "errors.catalog.functionNotFound");
        assert_eq!(e.message, "Function with oid 16401 was not found");
        assert_eq!(e.params.get("oid").map(String::as_str), Some("16401"));
        // Тот же код, что у «объекта нет» таблицы, представления и последовательности: интерфейс различает их по коду.
        assert_eq!(e.code, crate::catalog::object_not_found("shop", "t").code);
        assert_eq!(e.code, ErrorCode::NotFound);
        assert!(!e.retryable);
    }
}
