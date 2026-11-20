use super::{
    commands::ProcessTranscriptResponse, llm_client::LLMProvider, service::SummaryService,
};
use crate::{
    database::repositories::{setting::SettingsRepository, summary::SummaryProcessesRepository},
    state::AppState,
};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_store::StoreExt;

const AUTO_SUMMARY: &str = "automatic_summary";
const LOCAL_MODEL_REQUIRED: &str =
    "Choose a Built-in AI model in Settings to generate notes automatically.";
pub(crate) const CHANGED_EVENT: &str = "summary-job-changed";

#[tauri::command]
pub async fn get_automatic_summary_preference<R: Runtime>(
    app: AppHandle<R>,
) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        let store = app
            .store(crate::behavior_settings::STORE_FILE)
            .map_err(|_| "Could not read the automatic summary setting")?;
        Ok(store.get(AUTO_SUMMARY).and_then(|value| value.as_bool()) == Some(true))
    })
    .await
    .map_err(|_| "Could not read the automatic summary setting")?
}

#[tauri::command]
pub async fn set_automatic_summary_preference<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let _guard = crate::behavior_settings::CHANGE_LOCK.lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        crate::behavior_settings::save_bool(&app, AUTO_SUMMARY, enabled)
    })
    .await
    .map_err(|_| "Could not save the automatic summary setting")?
}

#[tauri::command]
pub async fn start_recording_summary<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<ProcessTranscriptResponse, String> {
    let enabled = get_automatic_summary_preference(app.clone()).await;
    start(app, state.db_manager.pool().clone(), meeting_id, enabled).await
}

pub(crate) async fn start<R: Runtime>(
    app: AppHandle<R>,
    pool: SqlitePool,
    meeting_id: String,
    enabled: Result<bool, String>,
) -> Result<ProcessTranscriptResponse, String> {
    if SummaryProcessesRepository::get_summary_data(&pool, &meeting_id)
        .await
        .map_err(|_| "Could not read notes status")?
        .is_some()
    {
        return Ok(response(meeting_id));
    }

    // The guarded job claim covers setup as well as generation. Repeated stop
    // events, navigation, and a simultaneous manual start share this slot.
    let Some(job) = SummaryService::try_register_job(&meeting_id, "builtin-ai")? else {
        return Ok(response(meeting_id));
    };
    let Some(model) = setup(&app, &pool, &meeting_id, enabled).await? else {
        return Ok(response(meeting_id));
    };
    if !SummaryProcessesRepository::create_automatic_process(&pool, &meeting_id)
        .await
        .map_err(|_| "Could not start automatic notes")?
    {
        return Ok(response(meeting_id));
    }
    let _ = app.emit(CHANGED_EVENT, &meeting_id);

    let worker_id = meeting_id.clone();
    tauri::async_runtime::spawn(async move {
        let settings = prepare(&app, &pool, &worker_id).await;
        match settings {
            Ok((context, language)) => {
                SummaryService::process_transcript_background(
                    app.clone(),
                    pool.clone(),
                    worker_id.clone(),
                    String::new(),
                    model.provider,
                    model.model,
                    context.prompt,
                    context.title,
                    context.template,
                    language,
                    true,
                    &job,
                )
                .await;
            }
            Err(error) => {
                let _ =
                    SummaryProcessesRepository::update_process_failed(&pool, &worker_id, &error)
                        .await;
            }
        }
        let ready = SummaryProcessesRepository::get_summary_data(&pool, &worker_id)
            .await
            .is_ok_and(|row| row.is_some_and(|row| row.status == "completed"));
        drop(job);
        let _ = app.emit(CHANGED_EVENT, &worker_id);
        if ready {
            notify_ready(&app).await;
        }
    });

    Ok(response(meeting_id))
}

fn response(meeting_id: String) -> ProcessTranscriptResponse {
    ProcessTranscriptResponse {
        message: "Check the meeting for automatic notes status".into(),
        process_id: meeting_id,
    }
}

async fn setup<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
    enabled: Result<bool, String>,
) -> Result<Option<crate::database::models::Setting>, String> {
    let decision: Result<Result<crate::database::models::Setting, &str>, String> = async {
        if !enabled? {
            return Ok(Err("Automatic summaries are off. You can generate notes here."));
        }
        let needs_recovery = sqlx::query_as::<_, crate::audio::outcome::RecordingOutcome>(
            "SELECT * FROM recording_outcomes WHERE meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| "Could not check the transcript recovery status")?
        .is_some_and(|outcome| outcome.needs_recovery());
        if needs_recovery {
            return Ok(Err("Review or retranscribe this meeting before generating notes."));
        }
        let has_transcript: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM transcripts WHERE meeting_id = ? AND TRIM(transcript) <> '')",
        )
        .bind(meeting_id)
        .fetch_one(pool)
        .await
        .map_err(|_| "Could not check the saved transcript")?;
        if !has_transcript {
            return Ok(Err("There is no saved transcript to summarize."));
        }
        let model = SettingsRepository::get_model_config(pool)
            .await
            .map_err(|_| "Could not read the saved summary model")?;
        Ok(match model {
            Some(model)
                if !model.model.trim().is_empty()
                    && LLMProvider::from_str(&model.provider) == Ok(LLMProvider::BuiltInAI) => Ok(model),
            _ => Err(LOCAL_MODEL_REQUIRED),
        })
    }.await;

    let (status, reason) = match decision {
        Ok(Ok(model)) => return Ok(Some(model)),
        Ok(Err(reason)) => ("skipped", reason.to_owned()),
        Err(error) => ("failed", error),
    };
    SummaryProcessesRepository::record_automatic_outcome(pool, meeting_id, status, &reason)
        .await
        .map_err(|_| "Could not save automatic notes status")?;
    let _ = app.emit(CHANGED_EVENT, meeting_id);
    Ok(None)
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

    #[tokio::test]
    async fn automatic_attempt_cannot_replace_an_existing_summary_or_skip() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            "Automatic summaries are off.",
        )
        .await
        .unwrap();
        assert!(
            !SummaryProcessesRepository::create_automatic_process(&pool, meeting)
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
            !SummaryProcessesRepository::create_automatic_process(&pool, meeting)
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
            SummaryProcessesRepository::create_automatic_process(&pool, meeting)
                .await
                .unwrap()
        );
        assert!(
            !SummaryProcessesRepository::create_automatic_process(&pool, meeting)
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
