//! One durable worker for the installed application pool. Notifications only wake it.
use super::{chunking, model, retrieval, scheduler::Scheduler, store, types::*, KnowledgeState};
use sqlx::SqlitePool;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{Manager, State};

#[derive(Default)]
pub struct IndexWorker {
    started: AtomicBool,
    wake: Arc<tokio::sync::Notify>,
}
impl IndexWorker {
    pub fn wake(&self) {
        self.wake.notify_one();
    }
    pub fn start(&self, pool: SqlitePool, scheduler: Arc<Scheduler>) -> bool {
        if self.started.swap(true, Ordering::AcqRel) {
            return false;
        }
        let wake = self.wake.clone();
        let scheduler = Arc::downgrade(&scheduler);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _=wake.notified()=>{},
                    _=tokio::time::sleep(std::time::Duration::from_secs(1))=>{},
                }
                let Some(runtime) = scheduler.upgrade() else {
                    break;
                };
                if pool.is_closed() {
                    break;
                }
                if !runtime.is_enabled() {
                    continue;
                }
                while runtime.notifications.lock().unwrap().next().is_some() {}
                if store::invalidate_other_spaces(&pool, &model::PINS.space().id)
                    .await
                    .is_err()
                {
                    continue;
                }
                match store::next_job(&pool).await {
                    Ok(Some(job)) => {
                        if let Err(error) = index_source(&pool, &runtime, &job).await {
                            let _ = store::record_failure(&pool, &job, &error).await;
                        } else {
                            wake.notify_one();
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            }
        });
        true
    }
}

pub async fn index_source(
    pool: &SqlitePool,
    runtime: &Arc<Scheduler>,
    job: &store::SourceJob,
) -> Result<(), KnowledgeError> {
    let mut after: Option<(String, String)> = None;
    let mut ordinal = 0;
    let space = model::PINS.space();
    loop {
        let rows = store::rows_page(
            pool,
            job,
            after
                .as_ref()
                .map(|(time, id)| (time.as_str(), id.as_str())),
        )
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let mut base = 0;
            while base < row.transcript.len() {
                if !store::current(pool, job).await? {
                    return Err(KnowledgeError::Superseded);
                }
                let end = chunking::floor_boundary(&row.transcript, base + 16 * 1024);
                let window = &row.transcript[base..end];
                if window.trim().is_empty() {
                    base = end;
                    continue;
                }
                let spans = runtime.spans(row.id.clone(), window.into()).await?;
                if spans.is_empty() {
                    base = end;
                    continue;
                }
                // Retain the last partial chunk as the next window's beginning
                // when more source bytes remain, so token overlap crosses windows.
                let partial = end < row.transcript.len() && spans.len() > 1;
                let count = spans.len() - usize::from(partial);
                for span in spans.iter().take(count) {
                    let span = TextSpan {
                        transcript_id: row.id.clone(),
                        start_byte: base + span.start_byte,
                        end_byte: base + span.end_byte,
                    };
                    let vector = runtime
                        .embed(
                            row.transcript[span.start_byte..span.end_byte].to_owned(),
                            EmbeddingPurpose::Passage,
                        )
                        .await?;
                    store::stage(pool, job, &row, &span, ordinal, &vector, &space.id).await?;
                    ordinal += 1;
                }
                base = if partial {
                    base + spans.last().unwrap().start_byte
                } else {
                    end
                };
            }
            after = Some((row.timestamp, row.id));
        }
    }
    store::publish(pool, job, &space.id).await
}

fn model_root(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("knowledge-models").join("multilingual-e5-small"))
        .map_err(|_| "Local model directory unavailable".into())
}
pub fn start_installed_pool(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<crate::state::AppState>() else {
        return;
    };
    let knowledge = app.state::<KnowledgeState>();
    if !knowledge
        .index_worker
        .start(state.db_manager.pool().clone(), knowledge.scheduler.clone())
    {
        return;
    }
    let pool = state.db_manager.pool().clone();
    let scheduler = knowledge.scheduler.clone();
    let root = model_root(app);
    let configuration = knowledge.configuration.clone();
    tauri::async_runtime::spawn(async move {
        let _configuration = configuration.lock().await;
        let enabled = sqlx::query_scalar::<_, i64>(
            "SELECT enabled FROM knowledge_settings WHERE singleton=1",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(0)
            == 1;
        if enabled {
            if let Ok(root) = root {
                if let Ok(verified) = model::VerifiedModel::verify(&root).await {
                    let _ = scheduler.enable(verified);
                }
            }
        }
    });
}

#[tauri::command]
pub async fn knowledge_search(
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
    request: SearchRequest,
) -> Result<SearchResponse, String> {
    retrieval::retrieve(state.db_manager.pool(), &runtime, &request)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn knowledge_index_status(
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
) -> Result<IndexStatus, String> {
    store::status(
        state.db_manager.pool(),
        runtime.scheduler.is_enabled(),
        &model::PINS.space().id,
    )
    .await
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn knowledge_reindex(
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
    scope: KnowledgeScope,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    let allowed = retrieval::freeze_scope(pool, &scope)
        .await
        .map_err(|e| e.to_string())?;
    store::requeue(pool, &allowed.meeting_ids)
        .await
        .map_err(|e| e.to_string())?;
    runtime.index_worker.wake();
    Ok(())
}
#[tauri::command]
pub async fn knowledge_cancel_index(
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
    scope: KnowledgeScope,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    let allowed = retrieval::freeze_scope(pool, &scope)
        .await
        .map_err(|e| e.to_string())?;
    let ids = serde_json::to_string(&allowed.meeting_ids).map_err(|_| "Invalid scope")?;
    sqlx::query("UPDATE knowledge_index_jobs SET paused=1 WHERE source_id IN(SELECT id FROM knowledge_sources WHERE meeting_id IN(SELECT value FROM json_each(?)))").bind(ids).execute(pool).await.map_err(|_|"Knowledge storage failed")?;
    runtime.cancellation.cancel();
    Ok(())
}
#[tauri::command]
pub async fn knowledge_model_enable(
    app: tauri::AppHandle,
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
    enabled: bool,
) -> Result<(), String> {
    let _configuration = runtime.configuration.lock().await;
    sqlx::query("UPDATE knowledge_settings SET enabled=? WHERE singleton=1")
        .bind(enabled)
        .execute(state.db_manager.pool())
        .await
        .map_err(|_| "Knowledge storage failed")?;
    if enabled {
        if let Ok(verified) = model::VerifiedModel::verify(&model_root(&app)?).await {
            runtime
                .scheduler
                .enable(verified)
                .map_err(|e| e.to_string())?;
        }
    } else {
        runtime.scheduler.disable().await;
    }
    runtime.index_worker.wake();
    Ok(())
}
#[derive(serde::Serialize)]
pub struct ModelStatus {
    pub enabled: bool,
    pub ready: bool,
    pub model: String,
}
#[tauri::command]
pub async fn knowledge_model_status(
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
) -> Result<ModelStatus, String> {
    let enabled =
        sqlx::query_scalar::<_, i64>("SELECT enabled FROM knowledge_settings WHERE singleton=1")
            .fetch_one(state.db_manager.pool())
            .await
            .map_err(|_| "Knowledge storage failed")?
            == 1;
    Ok(ModelStatus {
        enabled,
        ready: runtime.scheduler.is_enabled(),
        model: model::PINS.model.clone(),
    })
}
#[tauri::command]
pub async fn knowledge_model_download(
    app: tauri::AppHandle,
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
) -> Result<(), String> {
    let verified = runtime
        .downloads
        .download(
            &model_root(&app)?,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .map_err(|e| e.to_string())?;
    let _configuration = runtime.configuration.lock().await;
    let enabled =
        sqlx::query_scalar::<_, i64>("SELECT enabled FROM knowledge_settings WHERE singleton=1")
            .fetch_one(state.db_manager.pool())
            .await
            .map_err(|_| "Knowledge storage failed")?
            == 1;
    if enabled {
        runtime
            .scheduler
            .enable(verified)
            .map_err(|e| e.to_string())?;
    }
    runtime.index_worker.wake();
    Ok(())
}
#[tauri::command]
pub async fn knowledge_model_cancel_download(
    runtime: State<'_, KnowledgeState>,
) -> Result<(), String> {
    runtime.downloads.cancel().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn installed_pool_has_one_worker_and_disabled_jobs_remain_durable() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES('fixture','Public','2026-01-01','2026-01-01')").execute(&pool).await.unwrap();
        let runtime = KnowledgeState::default();
        assert!(runtime
            .index_worker
            .start(pool.clone(), runtime.scheduler.clone()));
        assert!(!runtime
            .index_worker
            .start(pool.clone(), runtime.scheduler.clone()));
        runtime.index_worker.wake();
        tokio::task::yield_now().await;
        assert!(store::next_job(&pool).await.unwrap().is_some());
        pool.close().await;
    }
}
