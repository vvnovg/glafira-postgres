//! Вкладка функции и процедуры (спец. P1 §9): сигнатура, результат, язык, изменчивость, определение.

use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::catalog::object_not_found;
use crate::conn::errors::from_pg;
use crate::error::AppResult;

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

/// Функция по OID (имена перегружены, в дереве у функции — OID).
pub async fn get(c: &Client, oid: i64) -> AppResult<FunctionDetails> {
    let not_found = || object_not_found("", &oid.to_string());
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
