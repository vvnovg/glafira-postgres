//! Соединение с узлом для опроса топологии: разовое (проверка подключения) или из пула (Task 12).

use std::sync::Arc;

use crate::conn::pg::ConnectSpec;
use crate::conn::profile::ConnectionProfile;
use crate::conn::run_blocking;
use crate::error::AppResult;
use crate::secrets::SecretStore;
use crate::topology::BoxFuture;
use crate::topology::probe::{NodeFacts, collect};

pub trait NodeConnector: Send + Sync {
    fn facts<'a>(
        &'a self,
        member: &'a str,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, AppResult<NodeFacts>>;
}

/// Новое соединение на каждый опрос; секреты — как переданы (для проверки — форма поверх keychain).
pub struct AdhocConnector {
    pub profile: ConnectionProfile,
    pub secrets: Arc<dyn SecretStore>,
}

impl NodeConnector for AdhocConnector {
    fn facts<'a>(
        &'a self,
        _member: &'a str,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, AppResult<NodeFacts>> {
        Box::pin(async move {
            let (profile, secrets, h) =
                (self.profile.clone(), self.secrets.clone(), host.to_string());
            let db = profile.pg.database.clone();
            let spec =
                run_blocking(move || ConnectSpec::build(&profile, &h, port, &db, secrets.as_ref()))
                    .await?;
            let client = spec.connect().await?;
            collect(&client).await
        })
    }
}
