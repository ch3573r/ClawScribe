//! Native saved-meeting and library conversation boundary.
use super::{answers, conversations, evidence, types::*, KnowledgeState};
use crate::state::AppState;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn knowledge_ask(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    request: AskRequest,
) -> Result<AssistantReply, String> {
    answers::ask(state.db_manager.pool(), &runtime, request, |provider| {
        crate::summary::llm_client::TextEnvironment::from_app(&app, provider)
    })
    .await
}
#[tauri::command]
pub async fn knowledge_cancel_request(
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    request_id: String,
) -> Result<(), String> {
    conversations::cancel(state.db_manager.pool(), &request_id).await?;
    runtime.answers.cancel(&request_id);
    Ok(())
}
#[tauri::command]
pub async fn knowledge_history(
    state: State<'_, AppState>,
    owner: ConversationOwner,
) -> Result<Vec<HistoryMessage>, String> {
    conversations::history(state.db_manager.pool(), &owner).await
}
#[tauri::command]
pub async fn knowledge_resolve_evidence(
    state: State<'_, AppState>,
    reference: EvidenceRef,
) -> Result<evidence::ResolvedEvidence, String> {
    evidence::resolve(state.db_manager.pool(), &reference)
        .await
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub async fn knowledge_create_library_conversation(
    state: State<'_, AppState>,
) -> Result<ConversationOwner, String> {
    conversations::create_library(state.db_manager.pool()).await
}
#[tauri::command]
pub async fn knowledge_list_library_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<ConversationOwner>, String> {
    conversations::list_libraries(state.db_manager.pool()).await
}
#[tauri::command]
pub async fn knowledge_clear_history(
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    owner: ConversationOwner,
) -> Result<u64, String> {
    runtime.answers.cancel_owner(&owner)?;
    conversations::clear(state.db_manager.pool(), &owner).await
}
