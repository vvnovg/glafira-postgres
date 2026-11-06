//! Команды запросов на оптимизацию и предложений LLM (спец. P4 §8, §11). Логика — `mcp::llm`.

use tauri::State;

use crate::error::AppResult;
use crate::mcp::llm::proposals::{LlmProposal, ProposalFilter};
use crate::mcp::llm::requests::OptimizationRequest;
use crate::mcp::llm::{self, CreateRequestArgs};
use crate::state::AppState;

/// «Передать LLM-ассистенту» в консоли EXPLAIN и карточке запроса.
#[tauri::command]
pub async fn create_optimization_request(
    state: State<'_, AppState>,
    id: String,
    req: CreateRequestArgs,
) -> AppResult<OptimizationRequest> {
    llm::create_request(&state, &id, req).await
}

#[tauri::command]
pub async fn cancel_optimization_request(
    state: State<'_, AppState>,
    request_id: String,
) -> AppResult<OptimizationRequest> {
    llm::cancel_request(&state, &request_id).await
}

/// Запросы подключения, сначала новые: статус «Ждёт ответа LLM» в консоли и карточке.
#[tauri::command]
pub fn list_optimization_requests(
    state: State<'_, AppState>,
    connection_id: String,
) -> Vec<OptimizationRequest> {
    state.llm.requests.list(&connection_id)
}

/// Предложения для панели консоли и значка карточки, сначала новые.
#[tauri::command]
pub fn list_llm_proposals(state: State<'_, AppState>, filter: ProposalFilter) -> Vec<LlmProposal> {
    state.llm.proposals.list(&filter)
}

#[tauri::command]
pub async fn answer_llm_proposal(
    state: State<'_, AppState>,
    id: String,
    status: String,
    reason: Option<String>,
) -> AppResult<LlmProposal> {
    llm::answer_proposal(&state, &id, &status, reason).await
}
