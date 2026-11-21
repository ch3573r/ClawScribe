use super::commands::ProcessTranscriptResponse;
pub(crate) use super::request_config::AuthorizedRequest;
use super::request_config::{AutomaticRequest, DestinationReview, DispatchCheck};
use super::service::{SummaryProviderRequest, SummaryService};
use crate::{
    behavior_settings::{self, Backing},
    database::repositories::summary::{SummaryProcessesRepository, NO_TRANSCRIPT, RECOVERY_NEEDED},
    state::AppState,
};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_store::StoreExt;

pub(crate) mod publication;

const LEGACY_AUTO_SUMMARY: &str = "automatic_summary";
const AUTO_SUMMARY: &str = "automatic_summary_v2";
pub(super) const DISABLED: &str = "Automatic summaries are off. You can generate notes here.";
pub(super) const REVIEW_REQUIRED: &str =
    "Review the summary provider in Settings to allow automatic notes at this destination.";
pub(super) const DESTINATION_CHANGED: &str =
    "The summary destination changed. Review it and try again.";
pub(crate) const PAUSED_FOR_INFERENCE: &str =
    "Automatic notes are paused while recording or transcription uses local inference.";
pub(crate) const CHANGED_EVENT: &str = "summary-job-changed";
pub(crate) static ROUTE_CHANGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static PREFERENCE_EPOCH: AtomicU64 = AtomicU64::new(0);
static ROUTE_EPOCH: AtomicU64 = AtomicU64::new(0);
static PREFERENCE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Default, Serialize, Deserialize)]
struct Preference {
    enabled: bool,
    external_fingerprint: Option<String>,
}

impl Preference {
    fn read(backing: &dyn Backing) -> Self {
        if let Some(stored) = backing.read(AUTO_SUMMARY) {
            return serde_json::from_value(stored).unwrap_or_default();
        }
        Self {
            enabled: backing.read(LEGACY_AUTO_SUMMARY).and_then(|v| v.as_bool()) == Some(true),
            external_fingerprint: None,
        }
    }

    fn allows(&self, review: &DestinationReview) -> bool {
        self.enabled
            && (!review.external
                || self.external_fingerprint.as_deref() == Some(review.fingerprint.as_str()))
    }

    fn migrate(backing: &dyn Backing, legacy_enabled: Option<bool>) -> Result<Self, String> {
        let mut preference = Self::read(backing);
        if backing.read(AUTO_SUMMARY).is_none() {
            if let Some(enabled) = backing
                .read(LEGACY_AUTO_SUMMARY)
                .and_then(|v| v.as_bool())
                .or(legacy_enabled)
            {
                preference.enabled = enabled;
            }
            preference.persist(backing)?;
        }
        Ok(preference)
    }

    fn persist(&self, backing: &dyn Backing) -> Result<(), String> {
        behavior_settings::persist(backing, AUTO_SUMMARY, serde_json::json!(self))
    }
}

fn with_preference<R: Runtime, T>(
    app: &AppHandle<R>,
    action: impl FnOnce(&dyn Backing) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = PREFERENCE_LOCK
        .lock()
        .map_err(|_| "Automatic summary settings are unavailable")?;
    let store = app
        .store(behavior_settings::STORE_FILE)
        .map_err(|_| "Could not open automatic summary settings")?;
    action(store.as_ref())
}

/// Called under ROUTE_CHANGE_LOCK before a saved route changes. The request
/// gate holds only the approved identity, so it can notice revocation without
/// consulting any mutable routing settings.
pub(crate) fn invalidate_external_review<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    with_preference(app, |store| {
        let mut preference = Preference::read(store);
        if preference.external_fingerprint.take().is_some() {
            preference.persist(store)?;
        }
        Ok(())
    })?;
    ROUTE_EPOCH.fetch_add(1, Ordering::SeqCst);
    let _ = app.emit("summary-destination-changed", ());
    Ok(())
}

pub(crate) fn same_endpoint(old: &str, new: &str) -> bool {
    let normalize = |value: &str| {
        let value = value.trim().trim_end_matches('/');
        url::Url::parse(value)
            .map(|url| url.as_str().trim_end_matches('/').to_owned())
            .unwrap_or_else(|_| value.to_owned())
    };
    normalize(old) == normalize(new)
}

#[tauri::command]
pub async fn get_automatic_summary_preference<R: Runtime>(
    app: AppHandle<R>,
) -> Result<bool, String> {
    with_preference(&app, |store| Ok(Preference::read(store).enabled))
}

#[tauri::command]
pub async fn migrate_automatic_summary_preference<R: Runtime>(
    app: AppHandle<R>,
    legacy_enabled: Option<bool>,
) -> Result<bool, String> {
    let _guard = ROUTE_CHANGE_LOCK.lock().await;
    with_preference(&app, |store| {
        Ok(Preference::migrate(store, legacy_enabled)?.enabled)
    })
}

#[tauri::command]
pub async fn get_automatic_summary_destination<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<DestinationReview, String> {
    let _guard = ROUTE_CHANGE_LOCK.lock().await;
    let mut review = AutomaticRequest::resolve(&app, state.db_manager.pool())
        .await?
        .review();
    review.approved = with_preference(&app, |store| Ok(Preference::read(store).allows(&review)))?;
    Ok(review)
}

#[tauri::command]
pub async fn set_automatic_summary_preference<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    enabled: bool,
    fingerprint: Option<String>,
) -> Result<(), String> {
    let _guard = ROUTE_CHANGE_LOCK.lock().await;
    let external_fingerprint = if enabled {
        let review = AutomaticRequest::resolve(&app, state.db_manager.pool())
            .await?
            .review();
        if fingerprint
            .as_ref()
            .is_some_and(|value| *value != review.fingerprint)
        {
            return Err(DESTINATION_CHANGED.into());
        }
        if review.external && fingerprint.is_none() {
            return Err(REVIEW_REQUIRED.into());
        }
        review.external.then_some(review.fingerprint)
    } else {
        None
    };
    with_preference(&app, |store| {
        Preference {
            enabled,
            external_fingerprint,
        }
        .persist(store)?;
        if !enabled {
            PREFERENCE_EPOCH.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    })
}

fn dispatch_check<R: Runtime>(app: AppHandle<R>, review: DestinationReview) -> DispatchCheck {
    let epoch = ROUTE_EPOCH.load(Ordering::SeqCst);
    let preference_epoch = PREFERENCE_EPOCH.load(Ordering::SeqCst);
    Arc::new(move || {
        with_preference(&app, |store| {
            let preference = Preference::read(store);
            if !preference.enabled || PREFERENCE_EPOCH.load(Ordering::SeqCst) != preference_epoch {
                return Err(DISABLED.into());
            }
            if !preference.allows(&review) {
                return Err(REVIEW_REQUIRED.into());
            }
            if ROUTE_EPOCH.load(Ordering::SeqCst) != epoch {
                return Err(DESTINATION_CHANGED.into());
            }
            Ok(())
        })
    })
}

pub(crate) async fn prepare_import<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    fingerprint: Option<&str>,
    use_automatic_preference: bool,
) -> Result<AuthorizedRequest, String> {
    let _guard = ROUTE_CHANGE_LOCK.lock().await;
    let request = AutomaticRequest::resolve(app, pool).await?;
    let review = request.review();
    // Older import clients only offered explicit Built-in AI notes. A missing
    // review token must never authorize another provider on their behalf.
    let legacy_builtin = !use_automatic_preference
        && fingerprint.is_none()
        && request.provider == super::llm_client::LLMProvider::BuiltInAI;
    if !legacy_builtin && fingerprint != Some(review.fingerprint.as_str()) {
        return Err(DESTINATION_CHANGED.into());
    }
    let check: DispatchCheck = if use_automatic_preference {
        dispatch_check(app.clone(), review)
    } else {
        let epoch = ROUTE_EPOCH.load(Ordering::SeqCst);
        Arc::new(move || {
            if ROUTE_EPOCH.load(Ordering::SeqCst) == epoch {
                Ok(())
            } else {
                Err(DESTINATION_CHANGED.into())
            }
        })
    };
    check()?;
    Ok(AuthorizedRequest {
        request,
        check: Some(check),
    })
}

#[tauri::command]
pub async fn start_recording_summary<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<ProcessTranscriptResponse, String> {
    start(app, state.db_manager.pool().clone(), meeting_id.clone()).await?;
    Ok(ProcessTranscriptResponse {
        message: "Check the meeting for automatic notes status".into(),
        process_id: meeting_id,
    })
}

pub(crate) async fn start<R: Runtime>(
    app: AppHandle<R>,
    pool: SqlitePool,
    meeting_id: String,
) -> Result<(), String> {
    launch_recording(app, pool, meeting_id).await?;
    Ok(())
}

async fn launch_recording<R: Runtime>(
    app: AppHandle<R>,
    pool: SqlitePool,
    meeting_id: String,
) -> Result<Launch, String> {
    if !recording_allowed(&app, &pool, &meeting_id).await? {
        return Ok(Launch::Handled);
    }
    launch(app, pool, meeting_id, Start::Recording).await
}

async fn recording_allowed<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<bool, String> {
    let restored = app
        .path()
        .app_data_dir()
        .map_err(|_| "Could not check the recording location")?
        .join("restored-recordings");
    let roots = crate::audio::recording_preferences::recording_roots(app).await;
    is_local_recording(pool, meeting_id, &restored, &roots).await
}

async fn is_local_recording(
    pool: &SqlitePool,
    meeting: &str,
    restored_root: &std::path::Path,
    roots: &[std::path::PathBuf],
) -> Result<bool, String> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT recording_source, folder_path FROM meetings WHERE id = ?")
            .bind(meeting)
            .fetch_optional(pool)
            .await
            .map_err(|_| "Could not check the recording source")?;
    let Some((source, Some(folder))) = row else {
        return Ok(false);
    };
    // Archive restore preserves the source field but does not transfer automatic consent.
    if source.as_deref() == Some("import")
        || crate::local_path::lexically_inside(&folder, restored_root)
    {
        return Ok(false);
    }
    // Before recording_source existed, only an owned native recording marker
    // can establish origin. A folder or a recording_outcomes row alone cannot.
    let recorded_here = source.as_deref() == Some("recording");
    let meeting = meeting.to_owned();
    let roots = roots.to_vec();
    let restored_root = restored_root.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        use crate::database::meeting_files;
        let Ok(folder) = crate::trusted_roots::resolve_allowed(&folder, &roots) else {
            return recorded_here;
        };
        if crate::local_path::lexically_inside(&folder.to_string_lossy(), &restored_root) {
            return false;
        }
        let Some(marker) = meeting_files::read_marker(&folder) else {
            return recorded_here;
        };
        let owner = meeting_files::stamped_meeting_id(&marker)
            .or_else(|| meeting_files::recording_id(&marker));
        if meeting_files::has_invalid_marker_id(&marker)
            || marker["source"] == "import"
            || owner.is_some_and(|id| id != meeting)
        {
            return false;
        }
        recorded_here
            || (owner.is_some() && crate::audio::interrupted::is_recording_marker(&marker))
    })
    .await
    .map_err(|_| "Could not check the recording source".into())
}

pub(crate) async fn start_import<R: Runtime>(
    app: AppHandle<R>,
    pool: SqlitePool,
    meeting_id: String,
    request: AuthorizedRequest,
) -> Result<(), String> {
    launch(app, pool, meeting_id, Start::Import(request)).await?;
    Ok(())
}

pub(crate) async fn resume_at_startup<R: Runtime>(app: AppHandle<R>, pool: SqlitePool) {
    publication::resume(&app, &pool).await;
    if get_automatic_summary_preference(app.clone()).await != Ok(true) {
        return;
    }
    let route_guard = ROUTE_CHANGE_LOCK.lock().await;
    let Ok(request) = AutomaticRequest::resolve(&app, &pool).await else {
        return;
    };
    let review = request.review();
    let fingerprint = review.fingerprint.clone();
    let check = dispatch_check(app.clone(), review);
    if check().is_err() {
        return;
    }
    let request = AuthorizedRequest {
        request,
        check: Some(check),
    };
    drop(route_guard);

    let profile = publication::stored_profile(&app).ok().flatten();
    let local = request.request.provider == super::llm_client::LLMProvider::BuiltInAI;
    let mut after = String::new();
    loop {
        let Ok(candidates) = SummaryProcessesRepository::automatic_resume_candidates_after(
            &pool,
            &fingerprint,
            profile.as_deref(),
            &after,
            local,
        )
        .await
        else {
            log::warn!("Could not read interrupted automatic notes");
            return;
        };
        if candidates.is_empty() {
            return;
        }
        for (meeting, stored_fingerprint) in candidates {
            after.clone_from(&meeting);
            if stored_fingerprint != fingerprint
                && recording_allowed(&app, &pool, &meeting).await != Ok(true)
            {
                continue;
            }
            match launch(
                app.clone(),
                pool.clone(),
                meeting,
                Start::Resume(request.clone(), stored_fingerprint),
            )
            .await
            {
                Ok(Launch::Started(worker)) => {
                    let _ = worker.await;
                }
                Ok(_) => {}
                Err(_) => log::warn!("Could not resume automatic notes"),
            }
        }
        if !local {
            // Retain the existing bound on automatically retried external requests.
            return;
        }
    }
}

enum Start {
    Recording,
    Import(AuthorizedRequest),
    Resume(AuthorizedRequest, String),
}

enum Launch {
    Handled,
    Busy,
    Started(tauri::async_runtime::JoinHandle<()>),
}

async fn launch<R: Runtime>(
    app: AppHandle<R>,
    pool: SqlitePool,
    meeting: String,
    start: Start,
) -> Result<Launch, String> {
    let (origin, resume, prepared) = match start {
        Start::Recording => ("recording", None, None),
        Start::Import(request) => ("import", None, Some(request)),
        Start::Resume(request, fingerprint) => ("", Some(fingerprint), Some(request)),
    };
    // Stop and repair can both wake this path. Avoid even resolving a provider
    // for rows that cannot be claimed. The write repeats this eligibility check.
    if resume.is_none()
        && !SummaryProcessesRepository::can_start_automatic_process(&pool, &meeting, origin)
            .await
            .map_err(|_| "Could not read automatic notes status")?
    {
        return Ok(Launch::Handled);
    }
    if origin != "import" && !get_automatic_summary_preference(app.clone()).await? {
        record_outcome(&app, &pool, &meeting, origin, DISABLED).await?;
        return Ok(Launch::Handled);
    }
    if let Some(reason) = transcript_unavailable(&pool, &meeting).await? {
        record_outcome(&app, &pool, &meeting, origin, reason).await?;
        return Ok(Launch::Handled);
    }

    let _route = ROUTE_CHANGE_LOCK.lock().await;
    let (request, check) = if let Some(authorized) = prepared {
        (authorized.request, authorized.check)
    } else {
        let request = match AutomaticRequest::resolve(&app, &pool).await {
            Ok(request) => request,
            Err(reason) => {
                record_outcome(&app, &pool, &meeting, origin, &reason).await?;
                return Ok(Launch::Handled);
            }
        };
        let check = dispatch_check(app.clone(), request.review());
        (request, Some(check))
    };
    if let Some(check) = &check {
        if let Err(reason) = check() {
            record_outcome(&app, &pool, &meeting, origin, &reason).await?;
            return Ok(Launch::Handled);
        }
    }
    let fingerprint = request.review().fingerprint;
    let provider = super::llm_client::canonical_provider(&request.provider);
    let local = request.provider == super::llm_client::LLMProvider::BuiltInAI;
    let Some(mut job) = SummaryService::try_register_job(&meeting, provider, true)? else {
        return Ok(Launch::Busy);
    };

    let claimed = if let Some(fingerprint) = resume {
        SummaryProcessesRepository::claim_automatic_resume(&pool, &meeting, &fingerprint).await
    } else {
        SummaryProcessesRepository::create_automatic_process(&pool, &meeting, origin, &fingerprint)
            .await
    }
    .map_err(|_| "Could not start automatic notes")?;
    if !claimed {
        return Ok(Launch::Handled);
    }
    let _ = app.emit(CHANGED_EVENT, &meeting);
    drop(_route);

    Ok(Launch::Started(tauri::async_runtime::spawn(async move {
        let authorized = AuthorizedRequest { request, check };
        loop {
            let prepared = super::llm_client::with_cancellation(
                Some(job.token()),
                prepare(&app, &pool, &meeting),
            )
            .await;
            match prepared {
                Ok((context, language)) => {
                    SummaryService::process_transcript_background(
                        app.clone(),
                        pool.clone(),
                        meeting.clone(),
                        String::new(),
                        SummaryProviderRequest::Automatic(authorized.clone()),
                        context.prompt,
                        context.template,
                        language,
                        true,
                        &job,
                    )
                    .await;
                }
                Err(reason) => {
                    super::service::finish_provider_error(&pool, &meeting, job.token(), &reason)
                        .await;
                }
            }
            if !local || !resume_after_preemption(&app, &pool, &meeting, &mut job).await {
                break;
            }
        }
        let ready = SummaryProcessesRepository::get_summary_data(&pool, &meeting)
            .await
            .is_ok_and(|row| row.is_some_and(|row| row.status == "completed"));
        drop(job);
        let _ = app.emit(CHANGED_EVENT, &meeting);
        if ready {
            notify_ready(&app, &pool, &meeting).await;
        }
    })))
}

async fn resume_after_preemption<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting: &str,
    job: &mut super::service::SummaryJob,
) -> bool {
    let Ok(Some(paused)) = SummaryProcessesRepository::get_summary_data(pool, meeting).await else {
        return false;
    };
    if paused.status != "cancelled" || paused.error.as_deref() != Some(PAUSED_FOR_INFERENCE) {
        return false;
    }
    loop {
        if !job.renew_after_preemption() {
            super::service::finish_provider_error(pool, meeting, job.token(), PAUSED_FOR_INFERENCE)
                .await;
            return false;
        }
        let _ = app.emit(CHANGED_EVENT, meeting);
        // Each native request takes its own permit and rechecks consent at dispatch.
        if crate::audio::inference::wait_for_job(job.token())
            .await
            .map(drop)
            .is_ok()
        {
            break;
        }
    }
    let Ok(Some(fingerprint)) = sqlx::query_scalar::<_, Option<String>>(
        "SELECT automatic_request_fingerprint FROM summary_processes WHERE meeting_id = ?",
    )
    .bind(meeting)
    .fetch_one(pool)
    .await
    else {
        return false;
    };
    SummaryProcessesRepository::claim_automatic_resume(pool, meeting, &fingerprint)
        .await
        .unwrap_or(false)
}

async fn record_outcome<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting: &str,
    origin: &str,
    reason: &str,
) -> Result<(), String> {
    if !origin.is_empty() {
        SummaryProcessesRepository::record_automatic_outcome(
            pool, meeting, "skipped", reason, origin,
        )
        .await
        .map_err(|_| "Could not save automatic notes status")?;
        let _ = app.emit(CHANGED_EVENT, meeting);
    }
    Ok(())
}

async fn transcript_unavailable(
    pool: &SqlitePool,
    meeting: &str,
) -> Result<Option<&'static str>, String> {
    let needs_recovery: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM recording_outcomes WHERE meeting_id = ?1
            AND (audio_save_failed OR transcription_incomplete))
         OR EXISTS(SELECT 1 FROM transcript_file_sync WHERE meeting_id = ?1
            AND retranscription_json IS NOT NULL)",
    )
    .bind(meeting)
    .fetch_one(pool)
    .await
    .map_err(|_| "Could not check the transcript recovery status")?;
    if needs_recovery {
        return Ok(Some(RECOVERY_NEEDED));
    }
    // SQLite TRIM only removes spaces, and SQLite permits blobs in a TEXT column.
    // Validate all saved passages, including those following a usable short reply.
    let mut rows = sqlx::query("SELECT transcript FROM transcripts WHERE meeting_id = ?")
        .bind(meeting)
        .fetch(pool);
    let mut has_text = false;
    while let Some(row) = rows
        .try_next()
        .await
        .map_err(|_| "Could not check the saved transcript")?
    {
        let Ok(text) = row.try_get::<&str, _>("transcript") else {
            return Ok(Some(RECOVERY_NEEDED));
        };
        if text.contains('\0') {
            return Ok(Some(RECOVERY_NEEDED));
        }
        has_text |= !text.trim().is_empty();
    }
    Ok((!has_text).then_some(NO_TRANSCRIPT))
}

async fn prepare<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<(super::commands::MeetingSummaryContext, Option<String>), String> {
    let context = crate::library::context::read(pool, meeting_id).await?;
    let folder: Option<String> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_one(pool)
            .await
            .map_err(|_| "Could not read the saved meeting")?;
    let context =
        super::commands::meeting_summary_context(app, pool, meeting_id, context, None).await?;
    let language = tauri::async_runtime::spawn_blocking(move || {
        let language = folder
            .filter(|path| !path.is_empty())
            .map(|path| {
                super::metadata::read_summary_language_from_metadata(std::path::Path::new(&path))
            })
            .transpose()
            .map_err(|_| "Could not read the meeting's summary language")?
            .flatten();
        Ok::<_, String>(language)
    })
    .await
    .map_err(|_| "Could not read automatic summary preferences")??;
    Ok((context, language))
}

async fn notify_ready<R: Runtime>(app: &AppHandle<R>, pool: &SqlitePool, meeting: &str) {
    use crate::notifications::{
        commands::NotificationManagerState,
        types::{Notification, NotificationType},
    };
    if let Some(state) = app.try_state::<NotificationManagerState<R>>() {
        let manager = state.read().await;
        if let Some(manager) = manager.as_ref() {
            let title: Option<String> =
                sqlx::query_scalar("SELECT title FROM meetings WHERE id = ?")
                    .bind(meeting)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or_default();
            let notification = Notification::new(
                "Meeting notes are ready",
                title.unwrap_or_else(|| "Open the saved meeting in ClawScribe.".into()),
                NotificationType::TranscriptionComplete,
            );
            let _ = manager.show_notification(notification).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gap_allows_usable_text_but_incomplete_publication_and_damaged_text_wait() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, capture_incomplete, recording_files_incomplete) VALUES (?, 1, 1)")
            .bind(meeting).execute(&pool).await.unwrap();
        sqlx::query("UPDATE transcripts SET transcript = 'Ja.'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(transcript_unavailable(&pool, meeting).await.unwrap(), None);
        for field in ["audio_save_failed", "transcription_incomplete"] {
            sqlx::query(&format!("UPDATE recording_outcomes SET {field} = 1"))
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(
                transcript_unavailable(&pool, meeting).await.unwrap(),
                Some(RECOVERY_NEEDED)
            );
            sqlx::query(&format!("UPDATE recording_outcomes SET {field} = 0"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO transcript_file_sync (meeting_id, retranscription_json) VALUES (?, 'broken')")
            .bind(meeting).execute(&pool).await.unwrap();
        assert_eq!(
            transcript_unavailable(&pool, meeting).await.unwrap(),
            Some(RECOVERY_NEEDED)
        );
        sqlx::query("DELETE FROM transcript_file_sync")
            .execute(&pool)
            .await
            .unwrap();
        for expression in ["x'4142'", "CAST(x'FF' AS TEXT)", "'OK' || char(0)"] {
            sqlx::query(&format!(
                "UPDATE transcripts SET transcript = {expression} WHERE id = 'b'"
            ))
            .execute(&pool)
            .await
            .unwrap();
            assert_eq!(
                transcript_unavailable(&pool, meeting).await.unwrap(),
                Some(RECOVERY_NEEDED)
            );
        }
        sqlx::query("UPDATE transcripts SET transcript = ?")
            .bind(" \t\n\u{a0}")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            transcript_unavailable(&pool, meeting).await.unwrap(),
            Some(NO_TRANSCRIPT)
        );
    }

    #[tokio::test]
    async fn repairing_an_empty_transcript_claims_one_job_and_preserves_prior_results() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            NO_TRANSCRIPT,
            "recording",
        )
        .await
        .unwrap();
        for column in ["result", "previous_result", "result_backup", "metadata"] {
            sqlx::query(&format!("UPDATE summary_processes SET {column} = 'saved'"))
                .execute(&pool)
                .await
                .unwrap();
            assert!(!SummaryProcessesRepository::can_start_automatic_process(
                &pool,
                meeting,
                "recording"
            )
            .await
            .unwrap());
            assert!(!SummaryProcessesRepository::create_automatic_process(
                &pool,
                meeting,
                "recording",
                "approved"
            )
            .await
            .unwrap());
            sqlx::query(&format!("UPDATE summary_processes SET {column} = NULL"))
                .execute(&pool)
                .await
                .unwrap();
        }
        assert!(SummaryProcessesRepository::can_start_automatic_process(
            &pool,
            meeting,
            "recording"
        )
        .await
        .unwrap());
        assert!(SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "approved"
        )
        .await
        .unwrap());
        assert!(!SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "approved"
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn retranscription_requires_a_local_recording_or_matching_legacy_marker() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let folder = tempfile::tempdir().unwrap();
        let id = format!("meeting-{}", uuid::Uuid::new_v4());
        let restored = folder.path().join("restored-recordings");
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, 'Synthetic', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
            .bind(&id).bind(folder.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let local = || is_local_recording(&pool, &id, &restored, &[]);
        assert!(!local().await.unwrap());
        for (owner, mode, source, allowed) in [
            (id.as_str(), "audio_only", "recording", true),
            (id.as_str(), "live", "recording", true),
            (id.as_str(), "live", "import", false),
            ("foreign", "live", "recording", false),
        ] {
            std::fs::write(
                folder.path().join("metadata.json"),
                serde_json::json!({
                    "version": "1.0", "transcript_file": "transcripts.json", "meeting_id": owner,
                    "source": source, "recording_mode": mode
                })
                .to_string(),
            )
            .unwrap();
            assert_eq!(local().await.unwrap(), allowed);
        }
        sqlx::query("UPDATE meetings SET recording_source = 'import' WHERE id = ?")
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!local().await.unwrap());
        sqlx::query(
            "UPDATE meetings SET recording_source = 'recording', folder_path = ? WHERE id = ?",
        )
        .bind(restored.join("copy").to_string_lossy().as_ref())
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!local().await.unwrap());
    }

    #[test]
    fn upgrades_keep_local_preference_once_and_require_explicit_external_consent() {
        use crate::behavior_settings::test_memory::Memory;
        let memory = Memory::default();
        let preference = Preference::migrate(&memory, Some(true)).unwrap();
        let mut review = DestinationReview {
            provider: "ollama",
            model: "synthetic".into(),
            endpoint: None,
            external: false,
            unencrypted: false,
            fingerprint: "first".into(),
            approved: false,
        };
        assert!(preference.allows(&review));
        review.external = true;
        assert!(!preference.allows(&review));
        let approval = Preference {
            enabled: true,
            external_fingerprint: Some(review.fingerprint.clone()),
        };
        approval.persist(&memory).unwrap();
        assert!(Preference::migrate(&memory, Some(false))
            .unwrap()
            .allows(&review));
        review.fingerprint = "second".into();
        assert!(!Preference::read(&memory).allows(&review));

        let disabled = Memory::with(&[(LEGACY_AUTO_SUMMARY, serde_json::json!(false))]);
        assert!(!Preference::migrate(&disabled, Some(true)).unwrap().enabled);
    }

    #[tokio::test]
    async fn automatic_attempt_cannot_replace_an_existing_summary_or_skip() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            "Automatic summaries are off.",
            "recording",
        )
        .await
        .unwrap();
        assert!(!SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "reviewed"
        )
        .await
        .unwrap());
        let skipped = SummaryProcessesRepository::get_summary_data_for_meeting(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(skipped.status, "skipped");
        assert_eq!(
            skipped.error.as_deref(),
            Some("Automatic summaries are off.")
        );
        SummaryProcessesRepository::create_or_reset_process(&pool, meeting)
            .await
            .unwrap();
        SummaryProcessesRepository::update_process_completed(
            &pool,
            meeting,
            serde_json::json!({"markdown": "Reviewed notes"}),
            1,
            0.0,
        )
        .await
        .unwrap();
        assert!(!SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "reviewed"
        )
        .await
        .unwrap());
        let saved = SummaryProcessesRepository::get_summary_data_for_meeting(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        assert!(saved.result.unwrap().contains("Reviewed notes"));
    }

    #[tokio::test]
    async fn publishing_old_notes_cannot_overwrite_a_changed_transcript() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        assert!(SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "reviewed"
        )
        .await
        .unwrap());
        assert!(!SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "reviewed"
        )
        .await
        .unwrap());
        let snapshot = super::super::sources::prepare_snapshot(
            &pool,
            meeting,
            "Untrusted payload".into(),
            true,
        )
        .await
        .unwrap();
        assert!(snapshot.text.contains("Äpfel project"));
        assert!(snapshot.text.contains("Project confirmed"));
        assert!(!snapshot.text.contains("Untrusted payload"));
        sqlx::query("UPDATE transcripts SET transcript = 'Synthetic correction' WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(snapshot.revision.is_some());
        let saved = SummaryProcessesRepository::complete_for_revision(
            &pool,
            meeting,
            serde_json::json!({"markdown": "Stale notes"}),
            1,
            0.0,
            snapshot.revision,
        )
        .await
        .unwrap();
        assert!(!saved);
        assert!(
            SummaryProcessesRepository::get_summary_data_for_meeting(&pool, meeting)
                .await
                .unwrap()
                .unwrap()
                .result
                .is_none()
        );
    }
}
