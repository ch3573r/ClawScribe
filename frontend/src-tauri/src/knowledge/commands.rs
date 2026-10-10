//! Native saved-meeting and library conversation boundary.
use super::{answers, conversations, evidence, types::*, KnowledgeState};
use crate::state::AppState;
use tauri::{AppHandle, Manager, State};

fn document_root(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|p| p.join("reference-documents"))
        .map_err(|_| "Reference document storage is unavailable".into())
}
#[tauri::command]
pub async fn knowledge_import_document(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    request_id: String,
    meeting_id: String,
    path: String,
) -> Result<super::documents::DocumentAttachment, String> {
    let lease = runtime
        .documents
        .begin(&request_id)
        .map_err(|e| e.to_string())?;
    let cancel = lease.cancel.clone();
    let preempt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let admission = super::scheduler::claim_snapshot(preempt.clone()).map_err(|e| e.to_string())?;
    let attachment = super::documents::import::import_document(
        state.db_manager.pool().clone(),
        document_root(&app)?,
        meeting_id,
        path.into(),
        cancel,
        preempt,
        (admission, lease),
    )
    .await
    .map_err(|e| e.to_string())?;
    runtime.index_worker.wake();
    Ok(attachment)
}
#[tauri::command]
pub fn knowledge_cancel_document_import(runtime: State<'_, KnowledgeState>, request_id: String) {
    runtime.documents.cancel(&request_id);
}
#[tauri::command]
pub async fn knowledge_list_documents(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<super::documents::DocumentAttachment>, String> {
    super::documents::store::list(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn knowledge_list_scope_documents(
    state: State<'_, AppState>,
    scope: KnowledgeScope,
) -> Result<Vec<super::document_context::ScopedDocument>, String> {
    super::document_context::scoped_documents(state.db_manager.pool(), &scope)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn knowledge_document_sharing(
    state: State<'_, AppState>,
    owner: ConversationOwner,
) -> Result<bool, String> {
    super::document_context::sharing(state.db_manager.pool(), &owner).await
}
#[tauri::command]
pub async fn knowledge_set_document_sharing(
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    owner: ConversationOwner,
    enabled: bool,
) -> Result<(), String> {
    if !enabled {
        runtime.answers.cancel_owner(&owner)?;
    }
    super::document_context::set_sharing(state.db_manager.pool(), &owner, enabled).await
}
#[tauri::command]
pub async fn knowledge_retry_document_index(
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    meeting_id: String,
    document_id: String,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    super::documents::store::get(pool, &meeting_id, &document_id)
        .await
        .map_err(|e| e.to_string())?;
    super::store::requeue_sources(pool, &[format!("document:{document_id}")])
        .await
        .map_err(|e| e.to_string())?;
    runtime.index_worker.wake();
    Ok(())
}
#[tauri::command]
pub async fn knowledge_get_document_blocks(
    state: State<'_, AppState>,
    meeting_id: String,
    document_id: String,
) -> Result<Vec<super::documents::DocumentBlock>, String> {
    super::documents::store::blocks(state.db_manager.pool(), &meeting_id, &document_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn knowledge_remove_attachment(
    state: State<'_, AppState>,
    meeting_id: String,
    document_id: String,
) -> Result<(), String> {
    super::documents::store::detach(state.db_manager.pool(), &meeting_id, &document_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn knowledge_delete_document(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    document_id: String,
) -> Result<(), String> {
    let root = document_root(&app)?;
    let name = super::documents::store::delete(state.db_manager.pool(), &meeting_id, &document_id)
        .await
        .map_err(|e| e.to_string())?;
    let path = super::documents::original_path(&root, &name).map_err(|e| e.to_string())?;
    match tokio::fs::remove_file(path).await {
        Ok(())=>Ok(()), Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(()),
        Err(_)=>Err("The document was deleted, but its original file could not be removed. Check storage permissions.".into()),
    }
}

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
    cancel_request(state.db_manager.pool(), &runtime, &request_id).await
}
/// Shared actual cancellation boundary, independently testable without a window.
pub(crate) async fn cancel_request(
    pool: &sqlx::SqlitePool,
    runtime: &KnowledgeState,
    request_id: &str,
) -> Result<(), String> {
    let deadline = runtime.answers.cancel_with_deadline(request_id);
    tokio::time::timeout_at(deadline, conversations::cancel(pool, request_id))
        .await
        .map_err(|_| {
            "Active answer cancelled; durable cancellation cleanup timed out".to_string()
        })?
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
