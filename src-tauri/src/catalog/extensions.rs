use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub name: String,
    pub version: String,
    pub schema: String,
    pub default_version: Option<String>,
    pub update_available: bool,
}

pub async fn list(c: &Client) -> AppResult<Vec<ExtensionInfo>> {
    let rows = c
        .query(
            "SELECT e.extname::text, e.extversion::text, n.nspname::text, a.default_version::text \
             FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace \
             LEFT JOIN pg_available_extensions a ON a.name = e.extname ORDER BY 1",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| {
            let version: String = r.get(1);
            let default_version: Option<String> = r.get(3);
            ExtensionInfo {
                name: r.get(0),
                update_available: default_version.as_ref().is_some_and(|d| *d != version),
                version,
                schema: r.get(2),
                default_version,
            }
        })
        .collect())
}
