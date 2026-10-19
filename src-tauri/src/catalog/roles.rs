use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleInfo {
    pub name: String,
    pub superuser: bool,
    pub login: bool,
    pub create_db: bool,
    pub create_role: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    pub conn_limit: i32,
    pub valid_until: Option<String>,
    pub member_of: Vec<String>,
}

/// Роли кластера; встроенные `pg_*` — только при `include_system`.
pub async fn list(c: &Client, include_system: bool) -> AppResult<Vec<RoleInfo>> {
    let rows = c
        .query(
            "SELECT r.rolname::text, r.rolsuper, r.rolcanlogin, r.rolcreatedb, r.rolcreaterole, \
               r.rolreplication, r.rolbypassrls, r.rolconnlimit, r.rolvaliduntil::text, \
               ARRAY(SELECT b.rolname::text FROM pg_auth_members m JOIN pg_roles b ON b.oid = m.roleid \
                     WHERE m.member = r.oid ORDER BY 1) \
             FROM pg_roles r WHERE $1 OR r.rolname !~ '^pg_' ORDER BY r.rolname",
            &[&include_system],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| RoleInfo {
            name: r.get(0),
            superuser: r.get(1),
            login: r.get(2),
            create_db: r.get(3),
            create_role: r.get(4),
            replication: r.get(5),
            bypass_rls: r.get(6),
            conn_limit: r.get(7),
            valid_until: r.get(8),
            member_of: r.get(9),
        })
        .collect())
}
