use crate::database::repositories::{
    meeting::MeetingsRepository, summary::SummaryProcessesRepository,
    transcript_chunk::TranscriptChunksRepository,
};
use crate::state::AppState;
use crate::summary::language_detection::{detect_summary_language, SummaryLanguageDetection};
use crate::summary::llm_client::generate_configured_text;
use crate::summary::metadata::{
    read_detected_summary_language_from_metadata, read_summary_language_from_metadata,
    write_detected_summary_language_to_metadata, write_summary_language_to_metadata,
};
use crate::summary::service::SummaryService;
use log::{error as log_error, info as log_info, warn as log_warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, Runtime};

#[derive(Debug, Serialize, Deserialize)]
pub struct SummaryResponse {
    pub status: String,
    #[serde(rename = "meetingName")]
    pub meeting_name: Option<String>,
    pub meeting_id: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
    pub has_previous_result: bool,
    pub(crate) progress: Option<super::context_budget::ReductionProgress>,
    pub partial_notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessTranscriptResponse {
    pub message: String,
    pub process_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SummaryLanguageStorage {
    Metadata,
    LocalFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummaryLanguagePreference {
    pub language: Option<String>,
    pub storage: SummaryLanguageStorage,
}

impl MeetingSummaryLanguagePreference {
    fn metadata(language: Option<String>) -> Self {
        Self {
            language,
            storage: SummaryLanguageStorage::Metadata,
        }
    }

    fn local_fallback() -> Self {
        Self {
            language: None,
            storage: SummaryLanguageStorage::LocalFallback,
        }
    }
}

enum MeetingFolderResolution {
    Folder(PathBuf),
    NoFolder,
}

/// Saves a meeting summary (Native SQLx implementation)
///
/// Expected format: { "markdown": "...", "summary_json": [...BlockNote blocks...] }
#[tauri::command]
pub async fn api_save_meeting_summary<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    mut summary: serde_json::Value,
    _auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "api_save_meeting_summary (native) called for meeting_id: {}",
        meeting_id
    );
    let pool = state.db_manager.pool();

    super::processor::validate_saved_summary(&summary)?;
    summary
        .as_object_mut()
        .ok_or("Invalid summary format")?
        .insert(
            "user_edited_at".into(),
            serde_json::json!(chrono::Utc::now().to_rfc3339()),
        );
    match SummaryProcessesRepository::update_meeting_summary(pool, &meeting_id, &summary).await {
        Ok(true) => {
            log_info!("Summary saved successfully for meeting_id: {}", meeting_id);
            Ok(serde_json::json!({
                "message": "Meeting summary saved successfully"
            }))
        }
        Ok(false) => {
            log_warn!(
                "Meeting not found or invalid JSON for meeting_id: {}",
                meeting_id
            );
            Err("Meeting not found or can't convert the json".into())
        }
        Err(e) => {
            log_error!("Failed to save meeting summary for {}: {}", meeting_id, e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_restore_previous_summary<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<SummaryResponse, String> {
    SummaryProcessesRepository::restore_previous_summary(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|error| error.to_string())?;
    api_get_summary(app, state, meeting_id, None).await
}

/// Gets the per-meeting summary language override from metadata.json.
#[tauri::command]
pub async fn api_get_meeting_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_get_meeting_summary_language called for meeting_id: {}",
        meeting_id
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => read_summary_language_from_metadata(&folder)
            .map(MeetingSummaryLanguagePreference::metadata)
            .map_err(|e| e.to_string()),
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Saves or clears the per-meeting summary language override in metadata.json.
#[tauri::command]
pub async fn api_save_meeting_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    summary_language: Option<String>,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_save_meeting_summary_language called for meeting_id: {}, language: {:?}",
        meeting_id,
        summary_language
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => {
            write_summary_language_to_metadata(&folder, summary_language.as_deref())
                .map_err(|e| e.to_string())?;
            read_summary_language_from_metadata(&folder)
                .map(MeetingSummaryLanguagePreference::metadata)
                .map_err(|e| e.to_string())
        }
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Gets the cached Auto-detected summary language from metadata.json.
#[tauri::command]
pub async fn api_get_meeting_detected_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_get_meeting_detected_summary_language called for meeting_id: {}",
        meeting_id
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => {
            read_detected_summary_language_from_metadata(&folder)
                .map(MeetingSummaryLanguagePreference::metadata)
                .map_err(|e| e.to_string())
        }
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Saves or clears the cached Auto-detected summary language in metadata.json.
#[tauri::command]
pub async fn api_save_meeting_detected_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    detected_summary_language: Option<String>,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_save_meeting_detected_summary_language called for meeting_id: {}, language: {:?}",
        meeting_id,
        detected_summary_language
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => {
            write_detected_summary_language_to_metadata(
                &folder,
                detected_summary_language.as_deref(),
            )
            .map_err(|e| e.to_string())?;
            read_detected_summary_language_from_metadata(&folder)
                .map(MeetingSummaryLanguagePreference::metadata)
                .map_err(|e| e.to_string())
        }
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Detects the dominant supported summary language from transcript segments.
#[tauri::command]
pub async fn api_detect_transcript_summary_language(
    transcript_texts: Vec<String>,
) -> Result<SummaryLanguageDetection, String> {
    Ok(detect_summary_language(&transcript_texts))
}

async fn resolve_meeting_folder(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<MeetingFolderResolution, String> {
    let meeting = MeetingsRepository::get_meeting_metadata(pool, meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting metadata: {}", e))?
        .ok_or_else(|| format!("Meeting not found: {}", meeting_id))?;

    let Some(folder_path) = meeting.folder_path.filter(|p| !p.trim().is_empty()) else {
        return Ok(MeetingFolderResolution::NoFolder);
    };

    Ok(MeetingFolderResolution::Folder(PathBuf::from(folder_path)))
}

/// Gets summary status and data (Native SQLx implementation)
///
/// Returns summary status (pending/processing/completed/failed) and parsed result data
#[tauri::command]
pub async fn api_get_summary<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    _auth_token: Option<String>,
) -> Result<SummaryResponse, String> {
    log_info!(
        "api_get_summary (native) called for meeting_id: {}",
        meeting_id
    );
    let pool = state.db_manager.pool();

    match SummaryProcessesRepository::get_summary_data_for_meeting(pool, &meeting_id).await {
        Ok(Some(process)) => {
            let mut status = process.status.to_lowercase();
            let mut error = process.error;

            // Parse result data if it exists (regardless of status)
            // This allows displaying restored summaries after cancellation or failure
            let data = if let Some(result_str) = process.result {
                match serde_json::from_str::<serde_json::Value>(&result_str) {
                    Ok(parsed) => match super::processor::validate_saved_summary(&parsed) {
                        Ok(()) => Some(parsed),
                        Err(reason) => {
                            // Hide unsafe legacy output without altering its stored copy.
                            if status == "completed" {
                                status = "failed".into();
                            }
                            error = Some(reason);
                            None
                        }
                    },
                    Err(e) => {
                        log_error!("Failed to parse summary result JSON: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            // Fetch meeting title from database
            let meeting_name =
                match MeetingsRepository::get_meeting_metadata(pool, &meeting_id).await {
                    Ok(Some(meeting_details)) => {
                        log_info!("Fetched meeting metadata for {}", meeting_id);
                        Some(meeting_details.title)
                    }
                    Ok(None) => {
                        log_warn!("Meeting not found for meeting_id: {}", meeting_id);
                        None
                    }
                    Err(e) => {
                        log_error!("Failed to fetch meeting title: {}", e);
                        None
                    }
                };

            let partial = process
                .metadata
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|value| {
                    Some((
                        value.get("partial_notes")?.as_str()?.to_owned(),
                        value.get("transcript_revision")?.as_i64()?,
                    ))
                });
            let partial_notes = if let Some((notes, revision)) = partial {
                let still_current: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM knowledge_sources WHERE meeting_id = ? AND kind = 'meeting' AND revision = ?)",
                )
                .bind(&meeting_id)
                .bind(revision)
                .fetch_one(pool)
                .await
                .unwrap_or(false);
                still_current.then_some(notes)
            } else {
                None
            };
            let response = SummaryResponse {
                progress: SummaryService::progress(&meeting_id),
                partial_notes,
                has_previous_result: process.previous_result.is_some(),
                status: status.clone(),
                meeting_name,
                meeting_id: meeting_id.clone(),
                start: process.start_time.map(|t| t.to_rfc3339()),
                end: process.end_time.map(|t| t.to_rfc3339()),
                data,
                error,
            };

            log_info!(
                "Summary status for {}: {}, has_data: {}",
                meeting_id,
                status,
                response.data.is_some()
            );
            Ok(response)
        }
        Ok(None) => {
            log_info!("No summary process found for meeting_id: {}", meeting_id);

            // Still fetch meeting title for idle state
            let meeting_name =
                match MeetingsRepository::get_meeting_metadata(pool, &meeting_id).await {
                    Ok(Some(meeting_details)) => Some(meeting_details.title),
                    _ => None,
                };

            Ok(SummaryResponse {
                has_previous_result: false,
                status: "idle".to_string(),
                progress: None,
                partial_notes: None,
                meeting_name,
                meeting_id,
                start: None,
                end: None,
                data: None,
                error: None,
            })
        }
        Err(e) => {
            log_error!("Error retrieving summary for {}: {}", meeting_id, e);
            Err(format!("Failed to retrieve summary: {}", e))
        }
    }
}

pub(super) struct MeetingSummaryContext {
    pub(super) prompt: String,
    pub(super) title: crate::library::calendar::SummaryTitle,
    pub(super) template: String,
}

pub(super) async fn meeting_summary_context<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    base_prompt: String,
    template_id: Option<String>,
) -> Result<MeetingSummaryContext, String> {
    let (calendar, summary_title) = crate::library::calendar::read_snapshot(pool, meeting_id).await?;
    let final_prompt = match calendar.as_ref() {
        Some(link) => link.with_attendance(base_prompt),
        None => base_prompt,
    };
    let final_template_id = match template_id.filter(|id| !id.is_empty()) {
        Some(id) => id,
        None => {
            let remembered = match calendar.as_ref() {
                Some(link) => crate::library::calendar::series_template(pool, link).await?,
                None => None,
            };
            match remembered {
                Some(id) => id,
                None => super::template_commands::default_template(app)?,
            }
        }
    };

    Ok(MeetingSummaryContext {
        prompt: final_prompt,
        title: summary_title,
        template: final_template_id,
    })
}

/// Processes transcript and generates summary (Native SQLx implementation)
///
/// Spawns a background task and returns immediately with process_id
#[tauri::command]
pub async fn api_process_transcript<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    text: String,
    model: String,
    model_name: String,
    meeting_id: Option<String>,
    _chunk_size: Option<i32>,
    _overlap: Option<i32>,
    custom_prompt: Option<String>,
    template_id: Option<String>,
    summary_language: Option<String>,
    _auth_token: Option<String>,
) -> Result<ProcessTranscriptResponse, String> {
    use uuid::Uuid;

    let m_id = meeting_id.unwrap_or_else(|| format!("meeting-{}", Uuid::new_v4()));
    log_info!(
        "api_process_transcript (native) called for meeting_id: {}, model: {}",
        &m_id,
        &model
    );

    let pool = state.db_manager.pool().clone();
    let context = meeting_summary_context(
        &app, &pool, &m_id, custom_prompt.unwrap_or_default(), template_id,
    ).await?;

    // Normalise empty / whitespace-only to None so "" and null behave identically
    let summary_language = summary_language.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });

    let job = SummaryService::register_job(&m_id, &model)?;

    // Create or reset the process entry in the database
    SummaryProcessesRepository::create_or_reset_process(&pool, &m_id)
        .await
        .map_err(|e| format!("Failed to initialize process: {}", e))?;

    log_info!("✓ Summary process initialized for meeting_id: {}", &m_id);

    // Save transcript chunks data (matching Python backend behavior)
    let chunk_size = _chunk_size.unwrap_or(40000);
    let overlap = _overlap.unwrap_or(1000);

    if TranscriptChunksRepository::save_transcript_data(
        &pool,
        &m_id,
        &text,
        &model,
        &model_name,
        chunk_size,
        overlap,
    )
    .await
    .is_err()
    {
        let reason = "Could not save transcript data for summary generation";
        let _ = SummaryProcessesRepository::update_process_failed(&pool, &m_id, reason).await;
        return Err(reason.into());
    }

    log_info!("✓ Transcript chunks saved for meeting_id: {}", &m_id);

    Ok(spawn_summary(
        app,
        pool,
        m_id,
        text,
        model,
        model_name,
        context,
        summary_language,
        true,
        job,
    ))
}

/// Starts a manual summary from the canonical saved transcript. The legacy
/// raw-text command remains available for compatible clients.
#[tauri::command]
pub async fn api_generate_meeting_summary<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    model: String,
    model_name: String,
    custom_prompt: Option<String>,
    template_id: Option<String>,
    summary_language: Option<String>,
    include_speaker_labels: Option<bool>,
) -> Result<ProcessTranscriptResponse, String> {
    let pool = state.db_manager.pool().clone();
    let job = SummaryService::register_job(&meeting_id, &model)?;
    let has_transcript: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM transcripts WHERE meeting_id = ? AND TRIM(transcript) <> '')",
    )
    .bind(&meeting_id)
    .fetch_one(&pool)
    .await
    .map_err(|_| "Could not read the saved transcript")?;
    if !has_transcript {
        return Err("No saved transcript is available to summarize.".into());
    }
    let context = match custom_prompt {
        Some(context) => context,
        None => crate::library::context::read(&pool, &meeting_id).await?,
    };
    let context = meeting_summary_context(&app, &pool, &meeting_id, context, template_id).await?;
    SummaryProcessesRepository::create_or_reset_process(&pool, &meeting_id)
        .await
        .map_err(|_| "Could not start summary generation")?;
    Ok(spawn_summary(
        app,
        pool,
        meeting_id,
        String::new(),
        model,
        model_name,
        context,
        summary_language,
        include_speaker_labels.unwrap_or(true),
        job,
    ))
}

#[allow(clippy::too_many_arguments)]
fn spawn_summary<R: Runtime>(
    app: AppHandle<R>,
    pool: sqlx::SqlitePool,
    m_id: String,
    text: String,
    model: String,
    model_name: String,
    context: MeetingSummaryContext,
    summary_language: Option<String>,
    include_speaker_labels: bool,
    job: super::service::SummaryJob,
) -> ProcessTranscriptResponse {
    let meeting_id_clone = m_id.clone();
    let _ = app.emit(super::automatic::CHANGED_EVENT, &m_id);
    tauri::async_runtime::spawn(async move {
        SummaryService::process_transcript_background(
            app.clone(),
            pool,
            meeting_id_clone.clone(),
            text,
            model,
            model_name,
            context.prompt,
            context.title,
            context.template,
            summary_language,
            include_speaker_labels,
            &job,
        )
        .await;
        let _ = app.emit(super::automatic::CHANGED_EVENT, &meeting_id_clone);
    });

    log_info!("🚀 Background task spawned for meeting_id: {}", &m_id);

    ProcessTranscriptResponse {
        message: "Summary generation started".to_string(),
        process_id: m_id,
    }
}

/// Cancels an ongoing summary generation process
///
/// This command triggers the cancellation token for the specified meeting,
/// stopping the summary generation gracefully.
#[tauri::command]
pub async fn api_cancel_summary(meeting_id: String) -> Result<serde_json::Value, String> {
    log_info!("api_cancel_summary called for meeting_id: {}", meeting_id);

    // The background job owns the final database status. A second async write
    // here could arrive after that job exits and mark its replacement cancelled.
    let cancelled = SummaryService::cancel_summary(&meeting_id);
    Ok(serde_json::json!({
        "message": if cancelled {
            "Summary cancellation requested"
        } else {
            "No active summary generation to cancel"
        },
        "meeting_id": meeting_id,
    }))
}

// ── Planner task AI polish ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerTaskDraft {
    pub title: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolishedPlannerTask {
    pub title: String,
    pub details: String,
}

/// Rewrite raw action items into clean Planner task titles + short notes using
/// the configured summary provider. Returns one polished task per input, in
/// order; the caller reviews/edits them in the export preview before anything is
/// created, so a poor rewrite is never committed silently. Errors (unsupported
/// provider, parse failure) surface so the UI can fall back to the raw titles.
#[tauri::command]
pub async fn polish_planner_tasks<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    model: String,
    model_name: String,
    tasks: Vec<PlannerTaskDraft>,
) -> Result<Vec<PolishedPlannerTask>, String> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }

    // Build the polish prompt (same instructions for every provider).
    let system = "You turn raw meeting action items into clean Microsoft Planner tasks. \
Respond with ONLY a JSON array — no prose, no code fences.";
    let mut user = String::from(
        "Rewrite each action item into a Planner task. Return a STRICT JSON array with one \
object per item, in the same order and the same count, each with keys \"title\" and \
\"details\". \"title\": a concise imperative task, at most 10 words, no trailing punctuation. \
\"details\": a 1-2 sentence description with relevant context. Do not add or remove items.\n\n\
Action items:\n",
    );
    for (i, t) in tasks.iter().enumerate() {
        user.push_str(&format!("{}. {}", i + 1, t.title.trim()));
        if let Some(owner) = t.owner.as_deref().filter(|s| !s.trim().is_empty()) {
            user.push_str(&format!(" (owner: {owner})"));
        }
        if let Some(due) = t.due_date.as_deref().filter(|s| !s.trim().is_empty()) {
            user.push_str(&format!(" (due: {due})"));
        }
        user.push('\n');
    }

    let raw = generate_configured_text(&app, &state, &model, &model_name, system, &user).await?;

    // Tolerantly extract the JSON array (some models wrap it in prose/fences).
    let json_slice = match (raw.find('['), raw.rfind(']')) {
        (Some(start), Some(end)) if end > start => &raw[start..=end],
        _ => return Err("AI polish returned no JSON array".to_string()),
    };
    let polished: Vec<PolishedPlannerTask> = serde_json::from_str(json_slice)
        .map_err(|e| format!("Failed to parse AI polish output: {e}"))?;
    if polished.len() != tasks.len() {
        return Err(format!(
            "AI polish returned {} items for {} tasks",
            polished.len(),
            tasks.len()
        ));
    }
    Ok(polished)
}
