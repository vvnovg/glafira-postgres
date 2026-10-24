//! Запрос сэмплера (спец. P2 §6): активные сессии узла на соединении пула `Sampler`.

use std::sync::OnceLock;

use futures_util::future::BoxFuture;

use crate::commands::catalog::on_catalog_node_within;
use crate::conn::errors::from_pg;
use crate::conn::pool::PoolPurpose;
use crate::diag::caps::{Caps, V14};
use crate::diag::sessions::QUERY_LIMIT;
use crate::error::AppResult;
use crate::sampler::buffer::RawSample;
use crate::sampler::registry::{SAMPLE_TIMEOUT, SampleSource};
use crate::state::AppState;

/// Активные сессии, кроме своей. Фоновые процессы с постоянным ожиданием-циклом (walsender, checkpointer и т. п.)
/// не сэмплируются: они искажали бы профиль. Текст запроса — не длиннее `QUERY_LIMIT` (уточнение 6 плана P2a).
pub fn sample_sql(caps: Caps) -> String {
    let query_id = caps.col(V14, "a.query_id", "bigint");
    format!(
        "SELECT a.pid, a.leader_pid, a.backend_type, a.datname::text, a.usename::text, a.application_name, \
                host(a.client_addr), a.wait_event_type, a.wait_event, {query_id}, left(a.query, {QUERY_LIMIT}) \
         FROM pg_stat_activity a \
         WHERE a.state = 'active' AND a.pid <> pg_backend_pid() \
           AND a.backend_type IN ('client backend', 'autovacuum worker', 'parallel worker')"
    )
}

/// Сэмпл с узла через пул `PoolPurpose::Sampler`; версия сервера — первым запросом, затем запомнена
/// (уточнение 10 плана P2a).
pub struct PoolSampleSource {
    st: AppState,
    connection_id: String,
    member: String,
    caps: OnceLock<Caps>,
}

impl PoolSampleSource {
    pub fn new(st: AppState, connection_id: &str, member: &str) -> Self {
        Self {
            st,
            connection_id: connection_id.to_string(),
            member: member.to_string(),
            caps: OnceLock::new(),
        }
    }
}

impl SampleSource for PoolSampleSource {
    fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<Vec<RawSample>>> {
        Box::pin(async move {
            let known = self.caps.get().copied();
            // Предел — `SAMPLE_TIMEOUT`, и по таймауту соединение закрывается, а не возвращается в пул.
            let (rows, caps) = on_catalog_node_within(
                SAMPLE_TIMEOUT,
                self.st.client_with(
                    &self.connection_id,
                    Some(&self.member),
                    None,
                    PoolPurpose::Sampler,
                ),
                move |c| {
                    Box::pin(async move {
                        let caps = match known {
                            Some(caps) => caps,
                            None => Caps::new(crate::diag::head(c).await?.version_num),
                        };
                        let rows = c
                            .query(&sample_sql(caps), &[])
                            .await
                            .map_err(|e| from_pg(&e))?;
                        Ok((rows, caps))
                    })
                },
            )
            .await?;
            let _ = self.caps.set(caps);
            Ok(rows
                .iter()
                .map(|r| RawSample {
                    pid: r.get(0),
                    leader_pid: r.get(1),
                    backend_type: r.get(2),
                    database: r.get(3),
                    user: r.get(4),
                    application: r.get(5),
                    client: r.get(6),
                    wait_type: r.get(7),
                    wait_event: r.get(8),
                    query_id: r.get(9),
                    query: r.get(10),
                })
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_id_only_from_14() {
        assert!(sample_sql(Caps::new(130_012)).contains("NULL::bigint, left(a.query, 16384)"));
        assert!(sample_sql(Caps::new(170_004)).contains("a.query_id, left(a.query, 16384)"));
    }
}
