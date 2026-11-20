
use super::commands::ProcessTranscriptResponse;
pub(crate) use super::request_config::AuthorizedRequest;
use super::request_config::{AutomaticRequest, DestinationReview, DispatchCheck};
use super::service::{SummaryProviderRequest, SummaryService};
use crate::{
    behavior_settings::{self, Backing},
    database::repositories::summary::{SummaryProcessesRepository, RECOVERY_NEEDED},
    state::AppState,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_store::StoreExt;

const LEGACY_AUTO_SUMMARY: &str = "automatic_summary";
const AUTO_SUMMARY: &str = "automatic_summary_v2";
pub(super) const DISABLED: &str = "Automatic summaries are off. You can generate notes here.";
pub(super) const REVIEW_REQUIRED: &str =
    "Review the summary provider in Settings to allow automatic notes at this destination.";
pub(super) const DESTINATION_CHANGED: &str = "The summary destination changed. Review it and try again.";
pub(crate) const CHANGED_EVENT: &str = "summary-job-changed";
pub(crate) static ROUTE_CHANGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
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
        self.enabled && (!review.external
            || self.external_fingerprint.as_deref() == Some(review.fingerprint.as_str()))
    }

    fn migrate(backing: &dyn Backing, legacy_enabled: Option<bool>) -> Result<Self, String> {
        let mut preference = Self::read(backing);
        if backing.read(AUTO_SUMMARY).is_none() {
            if let Some(enabled) = backing.read(LEGACY_AUTO_SUMMARY)
                .and_then(|v| v.as_bool()).or(legacy_enabled)
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
    let _guard = PREFERENCE_LOCK.lock().map_err(|_| "Automatic summary settings are unavailable")?;
    let store = app.store(behavior_settings::STORE_FILE)
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
        url::Url::parse(value).map(|url| url.as_str().trim_end_matches('/').to_owned())
            .unwrap_or_else(|_| value.to_owned())
    };
    normalize(old) == normalize(new)
}

#[tauri::command]
pub async fn get_automatic_summary_preference<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    with_preference(&app, |store| Ok(Preference::read(store).enabled))
}

#[tauri::command]
pub async fn migrate_automatic_summary_preference<R: Runtime>(
    app: AppHandle<R>,
    legacy_enabled: Option<bool>,
) -> Result<bool, String> {
    let _guard = ROUTE_CHANGE_LOCK.lock().await;
    with_preference(&app, |store| Ok(Preference::migrate(store, legacy_enabled)?.enabled))
}

#[tauri::command]
pub async fn get_automatic_summary_destination<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<DestinationReview, String> {
    let _guard = ROUTE_CHANGE_LOCK.lock().await;
    let mut review = AutomaticRequest::resolve(&app, state.db_manager.pool()).await?.review();
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
        let review = AutomaticRequest::resolve(&app, state.db_manager.pool()).await?.review();
        if fingerprint.as_ref().is_some_and(|value| *value != review.fingerprint) {
            return Err(DESTINATION_CHANGED.into());
        }
        if review.external && fingerprint.is_none() {
            return Err(REVIEW_REQUIRED.into());
        }
        review.external.then_some(review.fingerprint)
    } else {
        None
    };
    with_preference(&app, |store| Preference { enabled, external_fingerprint }.persist(store))
}

fn dispatch_check<R: Runtime>(app: AppHandle<R>, review: DestinationReview) -> DispatchCheck {
    let epoch = ROUTE_EPOCH.load(Ordering::SeqCst);
    Arc::new(move || with_preference(&app, |store| {
        let preference = Preference::read(store);
        if !preference.enabled {
            return Err(DISABLED.into());
        }
        if !preference.allows(&review) {
            return Err(REVIEW_REQUIRED.into());
        }
        if ROUTE_EPOCH.load(Ordering::SeqCst) != epoch {
            return Err(DESTINATION_CHANGED.into());
        }
        Ok(())
    }))
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
    let legacy_builtin = !use_automatic_preference && fingerprint.is_none()
        && request.provider == super::llm_client::LLMProvider::BuiltInAI;
    if !legacy_builtin && fingerprint != Some(review.fingerprint.as_str()) {
        return Err(DESTINATION_CHANGED.into());
    }
    let check: DispatchCheck = if use_automatic_preference {
        dispatch_check(app.clone(), review)
    } else {
        let epoch = ROUTE_EPOCH.load(Ordering::SeqCst);
        Arc::new(move || {
            if ROUTE_EPOCH.load(Ordering::SeqCst) == epoch { Ok(()) }
            else { Err(DESTINATION_CHANGED.into()) }
        })
    };
    check()?;
    Ok(AuthorizedRequest { request, check: Some(check) })
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
    launch(app, pool, meeting_id, Start::Recording).await?;
    Ok(())
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
    if get_automatic_summary_preference(app.clone()).await != Ok(true) {
        return;
    }
    let route_guard = ROUTE_CHANGE_LOCK.lock().await;
    let Ok(request) = AutomaticRequest::resolve(&app, &pool).await else { return; };
    let review = request.review();
    let fingerprint = review.fingerprint.clone();
    let check = dispatch_check(app.clone(), review);
    if check().is_err() {
        return;
    }
    let request = AuthorizedRequest { request, check: Some(check) };
    drop(route_guard);

    let Ok(candidates) = SummaryProcessesRepository::automatic_resume_candidates(&pool, &fingerprint).await else {
        log::warn!("Could not read interrupted automatic notes");
        return;
    };
    for meeting in candidates {
        match launch(app.clone(), pool.clone(), meeting, Start::Resume(request.clone())).await {
            Ok(Some(worker)) => { let _ = worker.await; }
            Ok(None) => {}
            Err(_) => log::warn!("Could not resume automatic notes"),
        }
    }
}

enum Start {
    Recording,
    Import(AuthorizedRequest),
    Resume(AuthorizedRequest),
}

async fn launch<R: Runtime>(
    app: AppHandle<R>,
    pool: SqlitePool,
    meeting: String,
    start: Start,
) -> Result<Option<tauri::async_runtime::JoinHandle<()>>, String> {
    let (origin, resume, prepared) = match start {
        Start::Recording => ("recording", false, None),
        Start::Import(request) => ("import", false, Some(request)),
        Start::Resume(request) => ("", true, Some(request)),
    };
    // Stop and repair can both wake this path. Avoid even resolving a provider
    // for rows that cannot be claimed. The write repeats this eligibility check.
    if !resume && !SummaryProcessesRepository::can_start_automatic_process(&pool, &meeting, origin)
        .await.map_err(|_| "Could not read automatic notes status")?
    {
        return Ok(None);
    }
    if origin != "import" && !get_automatic_summary_preference(app.clone()).await? {
        record_outcome(&app, &pool, &meeting, origin, DISABLED).await?;
        return Ok(None);
    }
    if let Some(reason) = transcript_unavailable(&pool, &meeting).await? {
        record_outcome(&app, &pool, &meeting, origin, reason).await?;
        return Ok(None);
    }

    let _route = ROUTE_CHANGE_LOCK.lock().await;
    let (request, check) = if let Some(authorized) = prepared {
        (authorized.request, authorized.check)
    } else {
        let request = match AutomaticRequest::resolve(&app, &pool).await {
            Ok(request) => request,
            Err(reason) => {
                record_outcome(&app, &pool, &meeting, origin, &reason).await?;
                return Ok(None);
            }
        };
        let check = dispatch_check(app.clone(), request.review());
        (request, Some(check))
    };
    if let Some(check) = &check {
        if let Err(reason) = check() {
            record_outcome(&app, &pool, &meeting, origin, &reason).await?;
            return Ok(None);
        }
    }
    let fingerprint = request.review().fingerprint;
    let provider = super::llm_client::canonical_provider(&request.provider);
    let Some(job) = SummaryService::try_register_job(&meeting, provider)? else {
        return Ok(None);
    };

    let claimed = if resume {
        SummaryProcessesRepository::claim_automatic_resume(&pool, &meeting, &fingerprint).await
    } else {
        SummaryProcessesRepository::create_automatic_process(&pool, &meeting, origin, &fingerprint).await
    }.map_err(|_| "Could not start automatic notes")?;
    if !claimed {
        return Ok(None);
    }
    let _ = app.emit(CHANGED_EVENT, &meeting);
    drop(_route);

    Ok(Some(tauri::async_runtime::spawn(async move {
        match prepare(&app, &pool, &meeting).await {
            Ok((context, language)) => {
                SummaryService::process_transcript_background(
                    app.clone(), pool.clone(), meeting.clone(), String::new(),
                    SummaryProviderRequest::Automatic(AuthorizedRequest { request, check }),
                    context.prompt, context.title, context.template, language, true,
                    &job,
                ).await;
            }
            Err(reason) => {
                let _ = SummaryProcessesRepository::update_process_failed(&pool, &meeting, &reason).await;
            }
        }
        let ready = SummaryProcessesRepository::get_summary_data(&pool, &meeting)
            .await.is_ok_and(|row| row.is_some_and(|row| row.status == "completed"));
        drop(job);
        let _ = app.emit(CHANGED_EVENT, &meeting);
        if ready {
            notify_ready(&app).await;
        }
    })))
}

async fn record_outcome<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting: &str,
    origin: &str,
    reason: &str,
) -> Result<(), String> {
    if !origin.is_empty() {
        SummaryProcessesRepository::record_automatic_outcome(pool, meeting, "skipped", reason, origin)
            .await.map_err(|_| "Could not save automatic notes status")?;
        let _ = app.emit(CHANGED_EVENT, meeting);
    }
    Ok(())
}

async fn transcript_unavailable(pool: &SqlitePool, meeting: &str) -> Result<Option<&'static str>, String> {
    let needs_recovery = sqlx::query_as::<_, crate::audio::outcome::RecordingOutcome>(
        "SELECT * FROM recording_outcomes WHERE meeting_id = ?",
    )
    .bind(meeting)
    .fetch_optional(pool)
    .await
    .map_err(|_| "Could not check the transcript recovery status")?
    .is_some_and(|outcome| outcome.needs_recovery());
    if needs_recovery {
        return Ok(Some(RECOVERY_NEEDED));
    }
    let has_transcript: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM transcripts WHERE meeting_id = ? AND TRIM(transcript) <> '')",
    )
    .bind(meeting)
    .fetch_one(pool)
    .await
    .map_err(|_| "Could not check the saved transcript")?;
    Ok((!has_transcript).then_some("There is no saved transcript to summarize."))
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
    let context = super::commands::meeting_summary_context(app, pool, meeting_id, context, None).await?;
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

async fn notify_ready<R: Runtime>(app: &AppHandle<R>) {
    use crate::notifications::{
        commands::NotificationManagerState,
        types::{Notification, NotificationType},
    };
    if let Some(state) = app.try_state::<NotificationManagerState<R>>() {
        let manager = state.read().await;
        if let Some(manager) = manager.as_ref() {
            let _ = manager
                .show_notification(Notification::new(
                    "ClawScribe",
                    "Meeting notes are ready.",
                    NotificationType::TranscriptionComplete,
                ))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_keep_local_preference_once_and_require_explicit_external_consent() {
        use crate::behavior_settings::test_memory::Memory;
        let memory = Memory::default();
        let preference = Preference::migrate(&memory, Some(true)).unwrap();
        let mut review = DestinationReview {
            provider: "ollama", model: "synthetic".into(), endpoint: None,
            external: false, unencrypted: false, fingerprint: "first".into(), approved: false,
        };
        assert!(preference.allows(&review));
        review.external = true;
        assert!(!preference.allows(&review));
        let approval = Preference { enabled: true, external_fingerprint: Some(review.fingerprint.clone()) };
        approval.persist(&memory).unwrap();
        assert!(Preference::migrate(&memory, Some(false)).unwrap().allows(&review));
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
        assert!(
            !SummaryProcessesRepository::create_automatic_process(&pool, meeting, "recording", "reviewed")
                .await
                .unwrap()
        );
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
        assert!(
            !SummaryProcessesRepository::create_automatic_process(&pool, meeting, "recording", "reviewed")
                .await
                .unwrap()
        );
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
        assert!(
            SummaryProcessesRepository::create_automatic_process(&pool, meeting, "recording", "reviewed")
                .await
                .unwrap()
        );
        assert!(
            !SummaryProcessesRepository::create_automatic_process(&pool, meeting, "recording", "reviewed")
                .await
                .unwrap()
        );
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
