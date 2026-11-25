//! One durable worker for the installed application pool. Notifications only wake it.
use super::{
    model, retrieval,
    scheduler::{Scheduler, VerificationStatus},
    store,
    types::*,
    KnowledgeState,
};
use sqlx::SqlitePool;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::{Listener, Manager, State};

// SQL triggers queue work even on paths that have no application event.
const DURABLE_QUEUE_RECHECK: Duration = Duration::from_secs(30);
const FAILED_JOB_RECHECK: Duration = Duration::from_secs(1);

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
                // Register before reading the durable queue or its admission
                // gates. A concurrent commit can then wake this idle pass.
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let Some(runtime) = scheduler.upgrade() else {
                    break;
                };
                if pool.is_closed() {
                    break;
                }
                let delay = match run_queued_job(&pool, &runtime).await {
                    QueuePass::More => continue,
                    QueuePass::Idle => DURABLE_QUEUE_RECHECK,
                    QueuePass::Retry => FAILED_JOB_RECHECK,
                };
                drop(runtime);
                tokio::select! {
                    _ = notified => {},
                    _ = tokio::time::sleep(delay) => {},
                    _ = pool.close_event() => break,
                }
            }
        });
        true
    }
}

enum QueuePass {
    More,
    Idle,
    Retry,
}

async fn run_queued_job(pool: &SqlitePool, runtime: &Arc<Scheduler>) -> QueuePass {
    if !runtime.accepts_work() {
        return QueuePass::Idle;
    }
    // The recording-stopped event precedes release of the save guard. If the
    // wake sees that guard, recheck while the Stop transition is settling.
    if crate::audio::recording_commands::is_stopping() {
        return QueuePass::Retry;
    }
    if crate::audio::recording_commands::capture_active() || super::scheduler::foreground_waiting()
    {
        return QueuePass::Idle;
    }
    while runtime.notifications.lock().unwrap().next().is_some() {}
    if store::invalidate_other_spaces(pool, &model::PINS.space().id)
        .await
        .is_err()
    {
        return QueuePass::Idle;
    }
    let Ok(Some(job)) = store::next_job(pool).await else {
        return QueuePass::Idle;
    };
    if let Err(error) = index_source(pool, runtime, &job).await {
        // A failed model check blocks the worker globally. Leave this source
        // queued for when the files are repaired.
        if error != KnowledgeError::ModelUnavailable || runtime.accepts_work() {
            let _ = store::record_failure(pool, &job, &error).await;
        }
        return QueuePass::Retry;
    }
    QueuePass::More
}

pub async fn index_source(
    pool: &SqlitePool,
    runtime: &Arc<Scheduler>,
    job: &store::SourceJob,
) -> Result<(), KnowledgeError> {
    let mut after: Option<String> = None;
    let mut ordinal = 0;
    let space = model::PINS.space();
    const SHORT_ROW_BYTES: usize = 96;
    const SHORT_ROW_CACHE: usize = 64;
    let mut short_rows = VecDeque::<(String, String)>::with_capacity(SHORT_ROW_CACHE);
    loop {
        runtime.check_available()?;
        drop(super::scheduler::claim_snapshot(Arc::new(
            AtomicBool::new(false),
        ))?);
        let rows = store::row_ids_page(pool, job, after.as_deref()).await?;
        if rows.is_empty() {
            break;
        }
        for id in rows {
            let selected = store::SelectedRow::for_job(job, id.clone());
            let mut base = 0;
            loop {
                runtime.check_available()?;
                if !store::current(pool, job).await? {
                    return Err(KnowledgeError::Superseded);
                }
                let (window, total) = store::body_window(pool, &selected, base).await?;
                let end = base + window.len();
                if base == total {
                    break;
                }
                if window.trim().is_empty() {
                    base = end;
                    continue;
                }
                let spans = runtime.spans(id.clone(), window).await?;
                if spans.is_empty() {
                    base = end;
                    continue;
                }
                // Retain the last partial chunk as the next window's beginning
                // when more source bytes remain, so token overlap crosses windows.
                let partial = end < total && spans.len() > 1;
                let count = spans.len() - usize::from(partial);
                for span in spans.iter().take(count) {
                    let span = TextSpan {
                        transcript_id: id.clone(),
                        start_byte: base + span.start_byte,
                        end_byte: base + span.end_byte,
                    };
                    let passage = store::materialize(pool, &selected, span.clone(), true).await?;
                    let can_share = !job.meeting_id.is_empty()
                        && span.start_byte == 0
                        && span.end_byte == total
                        && passage.text.len() <= SHORT_ROW_BYTES;
                    let shared = can_share
                        .then(|| {
                            short_rows
                                .iter()
                                .position(|(text, _)| *text == passage.text)
                        })
                        .flatten();
                    if let Some(entry) = shared.and_then(|index| short_rows.remove(index)) {
                        store::stage_index_chunk(
                            pool,
                            job,
                            &passage.evidence,
                            &passage.text,
                            ordinal,
                            store::StagedVector::Shared(&entry.1),
                            &space.id,
                        )
                        .await?;
                        short_rows.push_back(entry);
                        ordinal += 1;
                        continue;
                    }
                    let vector =
                        match store::reusable_vector(pool, job, &span, &passage.text, &space.id)
                            .await?
                        {
                            Some(vector) => vector,
                            None => {
                                runtime
                                    .embed(passage.text.clone(), EmbeddingPurpose::Passage)
                                    .await?
                            }
                        };
                    store::stage_evidence(
                        pool,
                        job,
                        &passage.evidence,
                        &passage.text,
                        ordinal,
                        &vector,
                        &space.id,
                    )
                    .await?;
                    if can_share {
                        if short_rows.len() == SHORT_ROW_CACHE {
                            short_rows.pop_front();
                        }
                        short_rows.push_back((passage.text, passage.evidence.chunk_id));
                    }
                    ordinal += 1;
                }
                base = if partial {
                    base + spans.last().unwrap().start_byte
                } else {
                    end
                };
            }
            after = Some(id);
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
    for event in [
        "library-changed",
        "recording-stopped",
        "retranscription-complete",
    ] {
        let wake = knowledge.index_worker.wake.clone();
        app.listen(event, move |_| wake.notify_one());
    }
    let pool = state.db_manager.pool().clone();
    let scheduler = knowledge.scheduler.clone();
    let root = model_root(app);
    let configuration = knowledge.configuration.clone();
    let wake = knowledge.index_worker.wake.clone();
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
                scheduler.enable(root);
                wake.notify_one();
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
        runtime.scheduler.verification_status(),
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
    sqlx::query("WITH allowed AS(SELECT value FROM json_each(?)) UPDATE knowledge_index_jobs SET paused=1 WHERE source_id IN(SELECT id FROM knowledge_sources WHERE meeting_id IN(SELECT value FROM allowed) OR id IN(SELECT d.source_id FROM knowledge_documents d JOIN knowledge_document_attachments a ON a.document_id=d.id WHERE a.meeting_id IN(SELECT value FROM allowed)))").bind(ids).execute(pool).await.map_err(|_|"Knowledge storage failed")?;
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
    let root = enabled.then(|| model_root(&app)).transpose()?;
    sqlx::query("UPDATE knowledge_settings SET enabled=? WHERE singleton=1")
        .bind(enabled)
        .execute(state.db_manager.pool())
        .await
        .map_err(|_| "Knowledge storage failed")?;
    if let Some(root) = root {
        runtime.scheduler.enable(root);
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
    /// Files exist with pinned sizes; this is not an integrity or inference guarantee.
    pub installed: bool,
    verification: VerificationStatus,
    pub model: String,
    pub download: model::DownloadStatus,
}
#[tauri::command]
pub async fn knowledge_model_status(
    app: tauri::AppHandle,
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
) -> Result<ModelStatus, String> {
    let enabled =
        sqlx::query_scalar::<_, i64>("SELECT enabled FROM knowledge_settings WHERE singleton=1")
            .fetch_one(state.db_manager.pool())
            .await
            .map_err(|_| "Knowledge storage failed")?
            == 1;
    let installed = model::files_present(&model_root(&app)?).await;
    let verification = runtime.scheduler.verification_status();
    Ok(ModelStatus {
        enabled,
        ready: enabled && installed && verification == VerificationStatus::Verified,
        installed,
        verification,
        model: model::PINS.model.clone(),
        download: runtime.downloads.snapshot(),
    })
}
#[tauri::command]
pub async fn knowledge_model_download(
    app: tauri::AppHandle,
    state: State<'_, crate::state::AppState>,
    runtime: State<'_, KnowledgeState>,
) -> Result<(), String> {
    let root = model_root(&app)?;
    let _configuration = runtime
        .configuration
        .try_lock()
        .map_err(|_| KnowledgeError::Busy.to_string())?;
    let enabled =
        sqlx::query_scalar::<_, i64>("SELECT enabled FROM knowledge_settings WHERE singleton=1")
            .fetch_one(state.db_manager.pool())
            .await
            .map_err(|_| "Knowledge storage failed")?
            == 1;
    let update = runtime.scheduler.begin_model_update(enabled, root.clone());
    let result = runtime
        .downloads
        .download_after(
            &root,
            tokio_util::sync::CancellationToken::new(),
            update.drain(),
        )
        .await;
    update.finish(&result);
    runtime.index_worker.wake();
    result.map(|_| ()).map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn knowledge_model_cancel_download(
    runtime: State<'_, KnowledgeState>,
) -> Result<(), String> {
    runtime.downloads.cancel().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn repeated_short_rows_share_vectors_and_keep_citations_after_reindexing() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('repeat','Fixture','2026-01-01','2026-01-01');
             INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time) VALUES
                ('first','repeat','Ja.','00:01',1), ('second','repeat','Ja.','00:03',3),
                ('third','repeat','Genau.','00:04',4);",
        ).execute(&pool).await.unwrap();
        let job = store::next_job(&pool).await.unwrap().unwrap();
        let runtime = super::super::scheduler::synthetic_query_scheduler();
        index_source(&pool, &runtime, &job).await.unwrap();
        let stored_vectors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_vectors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored_vectors, 2);
        let scope = retrieval::freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "repeat".into(),
            },
        )
        .await
        .unwrap();
        let space = model::PINS.space();
        let mut query_vector = [0.; 384];
        query_vector[0] = 1.;
        let candidates = retrieval::search_channels(
            &pool,
            &scope,
            "unmatched query",
            Some((&space.id, &query_vector)),
        )
        .await
        .unwrap();
        assert_eq!(
            candidates.len(),
            3,
            "shared vectors still rank each canonical row"
        );
        let selected = store::SelectedRow::for_job(&job, "second".into());
        let evidence = store::materialize(
            &pool,
            &selected,
            TextSpan {
                transcript_id: "second".into(),
                start_byte: 0,
                end_byte: 3,
            },
            false,
        )
        .await
        .unwrap()
        .evidence;
        store::requeue_sources(&pool, &[job.source_id])
            .await
            .unwrap();
        let rebuilt = store::next_job(&pool).await.unwrap().unwrap();
        index_source(&pool, &runtime, &rebuilt).await.unwrap();
        let resolved = super::super::evidence::resolve(&pool, &evidence)
            .await
            .unwrap();
        assert_eq!(
            resolved.status,
            super::super::evidence::EvidenceStatus::Current
        );
        assert_eq!(resolved.navigation.unwrap().start_seconds, Some(3.));
    }

    #[tokio::test]
    async fn large_unicode_indexing_is_complete_bounded_and_defers_reads_during_capture() {
        use super::*;
        use std::sync::atomic::Ordering;
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES('large','Synthetic','2026-10-07','2026-10-07')").execute(&pool).await.unwrap();
        let text = "Äpfel🙂 e\u{301} canonical word ".repeat(12000);
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES('large-row','large',?,'00:01')").bind(&text).execute(&pool).await.unwrap();
        let job = store::next_job(&pool).await.unwrap().unwrap();
        let runtime = super::super::scheduler::synthetic_query_scheduler();
        store::BODY_PEAK.store(0, Ordering::Relaxed);
        let recording = crate::audio::inference::claim_job().unwrap();
        assert!(matches!(
            index_source(&pool, &runtime, &job).await,
            Err(KnowledgeError::Busy)
        ));
        let capture_reads = store::BODY_PEAK.load(Ordering::Relaxed);
        drop(recording);
        assert_eq!(
            capture_reads, 0,
            "capture contention must be checked before canonical body loading"
        );
        store::BODY_PEAK.store(0, Ordering::Relaxed);
        index_source(&pool, &runtime, &job).await.unwrap();
        let spans: Vec<(i64, i64)> =
            sqlx::query_as("SELECT start_byte,end_byte FROM knowledge_chunks ORDER BY ordinal")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(spans.len() > 10);
        assert_eq!(spans[0].0, 0);
        assert_eq!(spans.last().unwrap().1 as usize, text.len());
        for span in &spans {
            assert!(text.get(span.0 as usize..span.1 as usize).is_some());
        }
        for pair in spans.windows(2) {
            assert!(
                pair[1].0 > pair[0].0 && pair[1].0 < pair[0].1,
                "semantic overlap must cover every byte across read windows"
            );
        }
        let peak = store::BODY_PEAK.load(Ordering::Relaxed);
        println!(
            "KNOWLEDGE_LARGE_INDEX canonical_bytes={} chunks={} retained_body_bytes={peak}",
            text.len(),
            spans.len()
        );
        assert!(peak <= 16384, "index input must remain bounded");
    }
    use super::*;

    #[tokio::test]
    async fn installed_worker_drains_startup_work_then_waits_for_notifications() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let enqueue = |id: &'static str| {
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES(?,'Synthetic','2026-01-01','2026-01-01')")
                .bind(id)
                .execute(&pool)
        };
        let wait_for_drain = || async {
            tokio::time::timeout(Duration::from_millis(750), async {
                while store::next_job(&pool).await.unwrap().is_some() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        };

        enqueue("first").await.unwrap();
        enqueue("second").await.unwrap();
        let scheduler = super::super::scheduler::synthetic_query_scheduler();
        let worker = IndexWorker::default();
        assert!(worker.start(pool.clone(), scheduler.clone()));
        wait_for_drain().await;

        enqueue("third").await.unwrap();
        worker.wake();
        wait_for_drain().await;
        pool.close().await;
    }

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
