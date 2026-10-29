//! Команды инструмента «Запросы» и карточки запроса (спец. P3 §10).

use std::sync::Arc;

use pg_advisor::queries::QueryKey;
use tauri::State;
use tauri::ipc::Channel;

use crate::diag::queries::QueryText;
use crate::error::AppResult;
use crate::sampler::registry::SamplerSink;
use crate::sampler::statements::StatementsEvent;
use crate::state::AppState;

/// Тексты запросов по ключам (ленивая подгрузка видимых строк).
#[tauri::command]
pub async fn query_texts(
    state: State<'_, AppState>,
    id: String,
    member: Option<String>,
    keys: Vec<QueryKey>,
) -> AppResult<Vec<QueryText>> {
    crate::diag::queries::texts(&state, &id, member.as_deref(), keys).await
}

struct StatementsChannelSink(Channel<StatementsEvent>);

impl SamplerSink<StatementsEvent> for StatementsChannelSink {
    fn send(&self, e: StatementsEvent) -> bool {
        self.0.send(e).is_ok()
    }
}

/// Подписка вкладки на сэмплер pg_stat_statements узла; итог — id сессии для `stop_session`.
#[tauri::command]
pub async fn start_statements_sampler(
    state: State<'_, AppState>,
    id: String,
    member: Option<String>,
    channel: Channel<StatementsEvent>,
) -> AppResult<String> {
    crate::sampler::statements::start(
        &state,
        &id,
        member.as_deref(),
        Arc::new(StatementsChannelSink(channel)),
    )
}
