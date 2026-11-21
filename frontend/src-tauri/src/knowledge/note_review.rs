//! One-use local review for personal excerpts on a single library question.
use super::store::saved_search::{self, SavedReference, SavedSource};
use super::types::{AskRequest, ConversationOwner, KnowledgeScope};
use super::{answers, conversations, retrieval, KnowledgeState};
use crate::state::AppState;
use crate::summary::llm_client::{self, NoteDestination, ResolvedText, TextEnvironment};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::time::{Duration, Instant};
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

const EXPIRED: &str = "This review changed or expired. Review the question and excerpts again.";
#[derive(Default)]
pub(super) struct Reviews(std::sync::Mutex<Option<Grant>>);
struct Grant {
    token: String,
    identity: Vec<u8>,
    destination: NoteDestination,
    expires: Instant,
}
#[derive(Serialize)]
pub(crate) struct Excerpt {
    source: SavedSource,
    text: String,
}
#[derive(Serialize)]
pub(crate) struct Preview {
    token: String,
    question: String,
    excerpts: Vec<Excerpt>,
    destination: NoteDestination,
}
fn identity(request: &AskRequest, refs: &[SavedReference]) -> Result<Vec<u8>, String> {
    conversations::validate_request(request)?;
    if refs.is_empty()
        || refs.len() > 8
        || refs
            .iter()
            .any(|r| r.id.len() > 512 || r.meeting_id.len() > 512)
    {
        return Err(EXPIRED.into());
    }
    Ok(Sha256::digest(serde_json::to_vec(&(request, refs)).map_err(|_| EXPIRED)?).to_vec())
}
impl Reviews {
    fn put(&self, identity: Vec<u8>, destination: NoteDestination) -> Result<String, String> {
        let token = uuid::Uuid::new_v4().to_string();
        *self.0.lock().map_err(|_| EXPIRED)? = Some(Grant {
            token: token.clone(),
            identity,
            destination,
            expires: Instant::now() + Duration::from_secs(300),
        });
        Ok(token)
    }
    fn take(&self, token: &str, identity: &[u8]) -> Result<NoteDestination, String> {
        let mut slot = self.0.lock().map_err(|_| EXPIRED)?;
        if !slot.as_ref().is_some_and(|p| p.token == token) {
            return Err(EXPIRED.into());
        }
        let grant = slot.take().ok_or(EXPIRED)?;
        if Instant::now() >= grant.expires || grant.identity != identity {
            return Err(EXPIRED.into());
        }
        Ok(grant.destination)
    }
    fn discard(&self, token: &str) {
        if let Ok(mut slot) = self.0.lock() {
            if slot.as_ref().is_some_and(|p| p.token == token) {
                *slot = None;
            }
        }
    }
}

pub(super) async fn resolve(
    pool: &SqlitePool,
    request: &AskRequest,
    refs: &[SavedReference],
) -> Result<Vec<Excerpt>, String> {
    conversations::validate_request(request)?;
    let KnowledgeScope::Library { filter } = &request.search.scope else {
        return Err(EXPIRED.into());
    };
    if !matches!(&request.owner, ConversationOwner::Library(_))
        || filter.all_meetings
        || filter.meeting_ids.is_empty()
        || filter.meeting_ids.len() > 256
        || refs.is_empty()
        || refs.len() > 8
    {
        return Err("Choose up to eight personal excerpts within the matched meetings.".into());
    }
    let scope = retrieval::freeze_scope(pool, &request.search.scope)
        .await
        .map_err(|e| e.to_string())?;
    if scope.meeting_ids.len() != filter.meeting_ids.len() {
        return Err(EXPIRED.into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut excerpts = Vec::new();
    for reference in refs {
        if reference.source == SavedSource::Summary
            || !scope.meeting_ids.contains(&reference.meeting_id)
            || !seen.insert((&reference.meeting_id, &reference.id, reference.source as u8))
        {
            return Err(EXPIRED.into());
        }
        let hit = saved_search::resolve(pool, reference.clone())
            .await
            .map_err(|_| EXPIRED)?
            .ok_or(EXPIRED)?;
        excerpts.push(Excerpt {
            source: reference.source,
            text: hit.text,
        });
    }
    Ok(excerpts)
}
pub(super) async fn configured(app: &AppHandle, pool: &SqlitePool) -> Result<ResolvedText, String> {
    let settings =
        crate::database::repositories::setting::SettingsRepository::get_model_config(pool)
            .await
            .map_err(|_| "Provider settings unavailable")?
            .ok_or("Choose a summary provider in Settings.")?;
    let provider = llm_client::LLMProvider::from_str(&settings.provider)?;
    if matches!(
        provider,
        llm_client::LLMProvider::ClaudeCode | llm_client::LLMProvider::Codex
    ) {
        return Err("This provider manages its own connection. Deselect personal excerpts or choose a provider with a verifiable endpoint.".into());
    }
    llm_client::resolve_configured_text(
        pool,
        TextEnvironment::from_app(app, &provider)?,
        &settings.provider,
        &settings.model,
        tokio::time::Instant::now(),
        &CancellationToken::new(),
    )
    .await
}

#[tauri::command]
pub(crate) async fn knowledge_preview_notes(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    request: AskRequest,
    references: Vec<SavedReference>,
) -> Result<Preview, String> {
    let pool = state.db_manager.pool();
    let excerpts = resolve(pool, &request, &references).await?;
    let destination = configured(&app, pool).await?.note_destination()?;
    let token = runtime
        .note_reviews
        .put(identity(&request, &references)?, destination.clone())?;
    Ok(Preview {
        token,
        question: request.search.query,
        excerpts,
        destination,
    })
}
#[tauri::command]
pub(crate) fn knowledge_discard_note_review(runtime: State<'_, KnowledgeState>, token: String) {
    runtime.note_reviews.discard(&token);
}
#[tauri::command]
pub(crate) async fn knowledge_ask_with_notes(
    app: AppHandle,
    state: State<'_, AppState>,
    runtime: State<'_, KnowledgeState>,
    request: AskRequest,
    references: Vec<SavedReference>,
    token: String,
) -> Result<super::types::AssistantReply, String> {
    let destination = runtime
        .note_reviews
        .take(&token, &identity(&request, &references)?)?;
    answers::ask_notes(
        app,
        state.db_manager.pool(),
        &runtime,
        request,
        references,
        destination,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reviews_bind_inputs_expire_and_cannot_be_consumed_twice() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let route = llm_client::resolve_configured_text(
            &pool,
            TextEnvironment::local(dir.path().into()),
            "builtin-ai",
            "qwen3.5:4b",
            tokio::time::Instant::now(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .note_destination()
        .unwrap();
        let registry = Reviews::default();
        let key = registry.put(vec![1], route.clone()).unwrap();
        assert!(registry.take(&key, &[2]).is_err());
        assert!(registry.take(&key, &[1]).is_err());
        let key = registry.put(vec![1], route.clone()).unwrap();
        assert_eq!(registry.take(&key, &[1]).unwrap(), route);
        assert!(registry.take(&key, &[1]).is_err());
        let key = registry.put(vec![1], route.clone()).unwrap();
        registry.discard(&key);
        assert!(registry.take(&key, &[1]).is_err());
        let key = registry.put(vec![1], route).unwrap();
        registry.0.lock().unwrap().as_mut().unwrap().expires = Instant::now();
        assert!(registry.take(&key, &[1]).is_err());
    }
}
