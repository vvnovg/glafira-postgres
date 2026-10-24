//! Команды диагностики (спец. P2 §9).

use std::sync::Arc;

use tauri::State;
use tauri::ipc::Channel;

use crate::diag::{DiagReport, DiagRequest};
use crate::error::AppResult;
use crate::sampler::registry::{SamplerEvent, SamplerSink};
use crate::state::AppState;

#[tauri::command]
pub async fn run_diagnostic(
    state: State<'_, AppState>,
    id: String,
    request: DiagRequest,
) -> AppResult<DiagReport> {
    crate::diag::run(&state, &id, request).await
}

/// Канал вкладки: ошибка отправки — канал закрыт, подписка снимается.
struct ChannelSink(Channel<SamplerEvent>);

impl SamplerSink for ChannelSink {
    fn send(&self, e: SamplerEvent) -> bool {
        self.0.send(e).is_ok()
    }
}

/// Подписка вкладки на сэмплер узла; итог — id сессии для `stop_session`. `async`: задача сэмплера запускается
/// в рантайме Tauri (`tokio::spawn`), а синхронная команда идёт вне него.
#[tauri::command]
pub async fn start_sampler(
    state: State<'_, AppState>,
    id: String,
    member: Option<String>,
    channel: Channel<SamplerEvent>,
) -> AppResult<String> {
    crate::sampler::start(
        &state,
        &id,
        member.as_deref(),
        Arc::new(ChannelSink(channel)),
    )
}

/// Останавливает длительную сессию; `false` — её уже нет.
#[tauri::command]
pub fn stop_session(state: State<'_, AppState>, session_id: String) -> AppResult<bool> {
    Ok(state.sessions.stop(&session_id))
}
