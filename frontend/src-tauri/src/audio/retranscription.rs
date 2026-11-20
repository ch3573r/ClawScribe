// Retranscription module - allows re-processing stored audio with different settings

use super::common::{
    create_readable_transcript_segments_with_words, split_transcripts_to_timing_grid,
    transcript_words_from_token_timestamps, TranscribedSegment,
};
use crate::config::{DEFAULT_PARAKEET_MODEL, DEFAULT_WHISPER_MODEL};
use crate::database::transcript_edits::RetranscriptionFileUpdate;
use crate::parakeet_engine::ParakeetEngine;
use crate::state::AppState;
use crate::whisper_engine::WhisperEngine;
use anyhow::{anyhow, Result};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Global flag to track if retranscription is in progress
static RETRANSCRIPTION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Global flag to signal cancellation
static RETRANSCRIPTION_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Where the job is, as far as a cancel from a starting recording is
/// concerned. The lock makes "cancel accepted" and "save begins" mutually
/// exclusive, so a cancel reported as accepted always stops the save.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Before the save begins: a cancel takes effect.
    Open,
    /// The save has begun and runs to completion.
    Saving,
    /// The job is over, successfully or not.
    Finished,
}

static RETRANSCRIPTION_PHASE: std::sync::Mutex<Phase> = std::sync::Mutex::new(Phase::Finished);

fn set_retranscription_phase(phase: Phase) {
    *RETRANSCRIPTION_PHASE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = phase;
}

/// Opens the cancel window for one job and closes it when dropped. Declared
/// after the job registration, so it is dropped (and cancel starts reporting
/// false) before the registration is cleared.
struct PhaseGuard;

impl PhaseGuard {
    fn open() -> Self {
        set_retranscription_phase(Phase::Open);
        PhaseGuard
    }
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        set_retranscription_phase(Phase::Finished);
    }
}

/// Cancels for a starting recording. Returns false when the job is already
/// saving, or over, and a cancel cannot take effect.
fn cancel_retranscription_if_possible() -> bool {
    let phase = RETRANSCRIPTION_PHASE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if *phase != Phase::Open {
        return false;
    }
    RETRANSCRIPTION_CANCELLED.store(true, Ordering::SeqCst);
    true
}

/// The last cancel check: after this the save runs to completion.
fn begin_retranscription_save() -> Result<()> {
    let mut phase = RETRANSCRIPTION_PHASE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }
    *phase = Phase::Saving;
    Ok(())
}

/// RAII guard for RETRANSCRIPTION_IN_PROGRESS flag
/// Ensures flag is cleared even if retranscription panics or returns early
struct RetranscriptionGuard;

impl RetranscriptionGuard {
    /// Create guard and set flag atomically
    fn acquire() -> Result<Self, String> {
        if RETRANSCRIPTION_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("Retranscription already in progress".to_string());
        }
        Ok(RetranscriptionGuard)
    }
}

impl Drop for RetranscriptionGuard {
    fn drop(&mut self) {
        RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// VAD redemption time in milliseconds. Keep this close to the live pipeline so
/// retranscription rows do not bridge normal speaker handoffs into long
/// multi-speaker transcript rows that diarization then has to split later.
const VAD_REDEMPTION_TIME_MS: u32 = 500;
const MAX_TRANSCRIPTION_SEGMENT_SAMPLES: usize = 25 * 16000;

/// Progress update emitted during retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionProgress {
    pub meeting_id: String,
    pub stage: String, // "decoding", "transcribing", "saving"
    pub progress_percentage: u32,
    pub message: String,
}

/// Result of retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionResult {
    pub meeting_id: String,
    pub segments_count: usize,
    pub duration_seconds: f64,
    pub language: Option<String>,
    pub file_warning: bool,
}

/// Error during retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionError {
    pub meeting_id: String,
    pub error: String,
}

/// Check if retranscription is currently in progress
pub fn is_retranscription_in_progress() -> bool {
    RETRANSCRIPTION_IN_PROGRESS.load(Ordering::SeqCst)
}

/// Start retranscription of a meeting's audio
pub async fn start_retranscription<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
) -> Result<RetranscriptionResult> {
    // Registered so a starting recording can cancel this job. A cancel before
    // the transcript is saved leaves the meeting's transcript untouched: the
    // delete and insert happen in one transaction after the last cancel check.
    let cancel: super::inference::CancelBatchJob = Arc::new(cancel_retranscription_if_possible);
    let _job = super::inference::claim_batch_job(
        super::inference::BatchJob::Retranscription,
        cancel,
        "Local summary stopped because retranscription started. Generate it again afterwards.",
    )
    .await
    .map_err(anyhow::Error::msg)?;
    // Acquire guard - ensures flag is cleared even on panic/early return
    let _guard = RetranscriptionGuard::acquire().map_err(|e| anyhow!(e))?;
    RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);
    // Open the cancellation window after resetting its flag.
    let _phase = PhaseGuard::open();
    let _ = crate::summary::summary_engine::force_shutdown_sidecar().await;

    let use_parakeet = provider.as_deref() == Some("parakeet")
        || super::transcription::cloud::is_cloud_provider(provider.as_deref());
    let use_nemotron = provider.as_deref() == Some("nemotron");
    let result = run_retranscription(
        app.clone(),
        meeting_id.clone(),
        meeting_folder_path,
        language,
        model,
        provider,
    )
    .await;

    // Unload the engine after the batch job (success, failure, or cancellation)
    super::common::unload_engine_after_batch_for(use_parakeet, use_nemotron).await;

    // Guard will automatically clear flag on drop
    // No need for manual: RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);

    match &result {
        Ok(res) => {
            let _ = app.emit("retranscription-complete", res);
        }
        Err(e) => {
            let _ = app.emit(
                "retranscription-error",
                RetranscriptionError {
                    meeting_id: meeting_id.clone(),
                    error: e.to_string(),
                },
            );
        }
    }

    result
}

/// Internal function to run retranscription
async fn run_retranscription<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    mut model: Option<String>,
    mut provider: Option<String>,
) -> Result<RetranscriptionResult> {
    // The folder is the one the core saved for this meeting; the path the
    // webview sends is not trusted.
    let _ = &meeting_folder_path;
    let pool = app
        .try_state::<crate::state::AppState>()
        .ok_or_else(|| anyhow!("Application database is not initialized"))?
        .db_manager
        .pool()
        .clone();
    let roots = crate::audio::recording_preferences::recording_roots(&app).await;
    let folder_path = crate::database::meeting_files::registered_meeting_folder(&pool, &meeting_id, &roots)
        .await
        .map_err(|message| anyhow!(message))?;
    let (audio_path, capture_gaps) =
        crate::audio::incremental_saver::find_or_recover_audio_file_with_status(&folder_path)
            .await?;

    info!(
        "Starting retranscription for meeting {} with language {:?}, model {:?}, provider {:?}",
        meeting_id, language, model, provider
    );

    let mut cloud_fallback_error = None;
    if super::transcription::cloud::is_cloud_provider(provider.as_deref()) {
        if super::recording_commands::cloud_transcription_enabled() {
            let provider_id = provider.as_deref().unwrap_or_default().to_string();
            emit_progress(
                &app,
                &meeting_id,
                "transcribing",
                5,
                "Uploading audio for cloud transcription...",
            );
            let outcome = super::transcription::cloud::transcribe_whole_file(
                &app,
                &provider_id,
                model.as_deref(),
                &audio_path,
                language.as_deref(),
                &RETRANSCRIPTION_CANCELLED,
            )
            .await;
            if RETRANSCRIPTION_CANCELLED.load(Ordering::Acquire) {
                return Err(anyhow!("Retranscription cancelled"));
            }
            match outcome {
                Ok(outcome) => {
                    let mut duration_seconds =
                        crate::audio::import::validate_audio_file(&audio_path)
                            .map(|info| info.duration_seconds)
                            .unwrap_or_else(|_| {
                                outcome
                                    .segments
                                    .iter()
                                    .map(|segment| segment.end_ms)
                                    .fold(0.0_f64, f64::max)
                                    / 1000.0
                            });
                    let cloud_segments = if outcome.requires_local_timing_grid {
                        let (decoded_duration_seconds, timed_segments) =
                            prepare_cloud_retranscription_timing_grid(
                                &app,
                                &meeting_id,
                                &audio_path,
                                &outcome.segments,
                            )
                            .await?;
                        duration_seconds = decoded_duration_seconds;
                        timed_segments
                    } else {
                        outcome.segments
                    };
                    let source_language =
                        super::common::transcription_source_language_hint(language.as_deref());
                    begin_retranscription_save()?;
                    return save_retranscription_transcripts(
                        &app,
                        &meeting_id,
                        &audio_path,
                        duration_seconds,
                        &cloud_segments,
                        &outcome.provider,
                        &outcome.model,
                        source_language.as_deref(),
                        capture_gaps,
                    )
                    .await;
                }
                Err(error) => {
                    warn!("Cloud retranscription failed; using the configured local fallback");
                    super::transcription::cloud::emit_fallback_event(
                        &app,
                        Some(&meeting_id),
                        &provider_id,
                        &error,
                    );
                    cloud_fallback_error = Some(error);
                }
            }
        } else {
            info!("Cloud transcription provider requested while Beta toggle is disabled; using local fallback");
        }

        provider = Some("parakeet".to_string());
        model = Some(DEFAULT_PARAKEET_MODEL.to_string());
    }

    // Determine which local provider to use after any cloud fallback.
    let use_parakeet = provider.as_deref() == Some("parakeet");
    let use_nemotron = provider.as_deref() == Some("nemotron");

    // Emit progress: decoding
    emit_progress(&app, &meeting_id, "decoding", 5, "Decoding audio file...");

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    let prepared_audio = super::batch_audio::prepare(
        &audio_path,
        VAD_REDEMPTION_TIME_MS,
        MAX_TRANSCRIPTION_SEGMENT_SAMPLES,
        &RETRANSCRIPTION_CANCELLED,
    )
    .await?;
    let duration_seconds = prepared_audio.duration_seconds;
    let speech_segments = &prepared_audio.segments;

    let total_segments = speech_segments.len();
    info!(
        "VAD detected {} speech segments (redemption_time={}ms)",
        total_segments, VAD_REDEMPTION_TIME_MS
    );

    // Diagnostic: log segment duration distribution
    if !speech_segments.is_empty() {
        let durations_ms: Vec<f64> = speech_segments
            .iter()
            .map(|s| s.end_timestamp_ms - s.start_timestamp_ms)
            .collect();
        let total_speech_ms: f64 = durations_ms.iter().sum();
        let avg_duration = total_speech_ms / durations_ms.len() as f64;
        let min_duration = durations_ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_duration = durations_ms
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        info!(
            "VAD segment stats: avg={:.0}ms, min={:.0}ms, max={:.0}ms, total_speech={:.1}s/{:.1}s ({:.0}%)",
            avg_duration, min_duration, max_duration,
            total_speech_ms / 1000.0, duration_seconds,
            (total_speech_ms / 1000.0 / duration_seconds) * 100.0
        );
        // Log first 10 segments for detailed inspection
        for (i, seg) in speech_segments.iter().take(10).enumerate() {
            let dur = seg.end_timestamp_ms - seg.start_timestamp_ms;
            debug!(
                "  Segment {}: {:.0}ms-{:.0}ms ({:.0}ms, {} samples)",
                i, seg.start_timestamp_ms, seg.end_timestamp_ms, dur, seg.sample_count
            );
        }
        if total_segments > 10 {
            debug!("  ... and {} more segments", total_segments - 10);
        }
    }

    if total_segments == 0 {
        warn!("No speech detected in audio");
        return Err(anyhow!("No speech detected in audio file"));
    }

    emit_progress(
        &app,
        &meeting_id,
        "transcribing",
        25,
        "Loading transcription engine...",
    );

    // Initialize the appropriate engine once (not per-segment)
    let whisper_engine = if !use_parakeet && !use_nemotron {
        match get_or_init_whisper(&app, model.as_deref()).await {
            Ok(engine) => Some(engine),
            Err(error) => {
                return Err(match &cloud_fallback_error {
                    Some(cloud_error) => super::transcription::cloud::local_fallback_error_context(
                        cloud_error,
                        error,
                    ),
                    None => error,
                });
            }
        }
    } else {
        None
    };
    let parakeet_engine = if use_parakeet {
        match get_or_init_parakeet(&app, model.as_deref()).await {
            Ok(engine) => Some(engine),
            Err(error) => {
                return Err(match &cloud_fallback_error {
                    Some(cloud_error) => super::transcription::cloud::local_fallback_error_context(
                        cloud_error,
                        error,
                    ),
                    None => error,
                });
            }
        }
    } else {
        None
    };
    let nemotron_engine = if use_nemotron {
        match get_or_init_nemotron(&app, model.as_deref()).await {
            Ok(engine) => Some(engine),
            Err(error) => {
                return Err(match &cloud_fallback_error {
                    Some(cloud_error) => super::transcription::cloud::local_fallback_error_context(
                        cloud_error,
                        error,
                    ),
                    None => error,
                });
            }
        }
    } else {
        None
    };

    // Split very long segments at silence boundaries for better transcription quality.
    // Hard cuts at arbitrary sample positions lose words at boundaries. Instead, scan
    // for the lowest-energy window near the target split point and cut there.
    // Consume the VAD segments by value: cloning them would double the resident
    // memory of the meeting's entire speech audio.
    let processable_segments = &prepared_audio.segments;

    let processable_count = processable_segments.len();
    info!(
        "Processing {} segments (after splitting)",
        processable_count
    );

    // Process each speech segment with progress updates
    let mut all_transcripts: Vec<TranscribedSegment> = Vec::new();
    let mut total_confidence = 0.0f32;
    let mut confidence_count = 0usize;

    for (i, segment) in processable_segments.iter().enumerate() {
        let segment = segment.load().await?;
        // Check for cancellation before each segment
        if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
            return Err(anyhow!("Retranscription cancelled"));
        }

        // Calculate progress (25% to 80% range for transcription)
        let progress = 25 + ((i as f32 / processable_count as f32) * 55.0) as u32;
        let segment_duration_sec = (segment.end_timestamp_ms - segment.start_timestamp_ms) / 1000.0;
        emit_progress(
            &app,
            &meeting_id,
            "transcribing",
            progress,
            &format!(
                "Transcribing segment {} of {} ({:.1}s)...",
                i + 1,
                processable_count,
                segment_duration_sec
            ),
        );

        // Skip very short segments (< 100ms of audio = 1600 samples at 16kHz)
        if segment.samples.len() < 1600 {
            debug!(
                "Skipping short segment {} with {} samples",
                i,
                segment.samples.len()
            );
            continue;
        }

        // Transcribe this segment
        let segment_result =
            super::batch_audio::retry_segment(&RETRANSCRIPTION_CANCELLED, || async {
                let value = if use_nemotron {
                    let engine = nemotron_engine.as_ref().unwrap();
                    let text = engine
                        .transcribe_audio(segment.samples.clone(), language.clone())
                        .await
                        .map_err(|e| {
                            anyhow!("Nemotron transcription failed on segment {}: {}", i, e)
                        })?;
                    (text, None, None)
                } else if use_parakeet {
                    let engine = parakeet_engine.as_ref().unwrap();
                    let result = engine
                        .transcribe_audio_timestamped(segment.samples.clone())
                        .await
                        .map_err(|e| {
                            anyhow!("Parakeet transcription failed on segment {}: {}", i, e)
                        })?;
                    let text = result.text;
                    let word_timestamps = transcript_words_from_token_timestamps(
                        &text,
                        &result.tokens,
                        &result.timestamps,
                        segment.start_timestamp_ms / 1000.0,
                        segment.end_timestamp_ms / 1000.0,
                        None,
                        None,
                    );
                    (text, None, word_timestamps)
                } else {
                    let engine = whisper_engine.as_ref().unwrap();
                    let (text, conf, partial) = engine
                        .transcribe_audio_with_confidence(segment.samples.clone(), language.clone())
                        .await
                        .map_err(|e| {
                            anyhow!("Whisper transcription failed on segment {}: {}", i, e)
                        })?;
                    if partial {
                        return Err(anyhow!("The speech engine returned incomplete text"));
                    }
                    (text, conf, None)
                };
                Ok(value)
            })
            .await;
        let (text, conf, word_timestamps) = segment_result?;

        // Skip empty transcripts
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            debug!(
                "Segment {}/{}: audio={:.1}s, confidence={:?}, chars={}",
                i + 1,
                processable_count,
                segment_duration_sec,
                conf,
                trimmed.chars().count()
            );
            all_transcripts.push(TranscribedSegment {
                text,
                start_ms: segment.start_timestamp_ms,
                end_ms: segment.end_timestamp_ms,
                word_timestamps,
            });
            if let Some(confidence) = conf {
                total_confidence += confidence;
                confidence_count += 1;
            }
        } else {
            debug!(
                "Segment {}/{}: {:.1}s — empty transcription",
                i + 1,
                processable_count,
                segment_duration_sec
            );
        }
    }

    let transcribed_count = all_transcripts.len();
    let avg_confidence = (confidence_count > 0).then(|| total_confidence / confidence_count as f32);

    info!(
        "Transcription complete: {} segments transcribed out of {}, avg reported confidence: {:?}",
        transcribed_count, processable_count, avg_confidence
    );

    // Last cancel check; the save below cannot be cancelled.
    begin_retranscription_save()?;

    let used_provider = if use_nemotron {
        "nemotron"
    } else if use_parakeet {
        "parakeet"
    } else {
        "localWhisper"
    };
    let used_model = if let Some(e) = &nemotron_engine {
        e.get_current_model().await
    } else if let Some(e) = &parakeet_engine {
        e.get_current_model().await
    } else if let Some(e) = &whisper_engine {
        e.get_current_model().await
    } else {
        None
    }
    .or_else(|| model.clone())
    .unwrap_or_default();
    let source_language = super::common::transcription_source_language_hint(language.as_deref());

    save_retranscription_transcripts(
        &app,
        &meeting_id,
        &audio_path,
        duration_seconds,
        &all_transcripts,
        used_provider,
        &used_model,
        source_language.as_deref(),
        capture_gaps,
    )
    .await
}

async fn prepare_cloud_retranscription_timing_grid<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    audio_path: &Path,
    cloud_segments: &[TranscribedSegment],
) -> Result<(f64, Vec<TranscribedSegment>)> {
    emit_progress(
        app,
        meeting_id,
        "decoding",
        10,
        "Preparing cloud transcript timing...",
    );

    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    let prepared_audio = super::batch_audio::prepare(
        audio_path,
        VAD_REDEMPTION_TIME_MS,
        MAX_TRANSCRIPTION_SEGMENT_SAMPLES,
        &RETRANSCRIPTION_CANCELLED,
    )
    .await?;
    let duration_seconds = prepared_audio.duration_seconds;
    let timing_grid = prepared_audio.timing_grid();
    if timing_grid.is_empty() {
        warn!("Cloud provider returned collapsed transcript output, but local VAD found no timing grid; preserving cloud transcript as returned");
        return Ok((duration_seconds, cloud_segments.to_vec()));
    }

    let timed_segments = split_transcripts_to_timing_grid(cloud_segments, &timing_grid);
    if timed_segments.is_empty() {
        warn!("Cloud provider returned collapsed transcript output, but text could not be split onto local timing grid; preserving cloud transcript as returned");
        return Ok((duration_seconds, cloud_segments.to_vec()));
    }

    info!("Applied local VAD timing to cloud retranscription");

    Ok((duration_seconds, timed_segments))
}

async fn save_retranscription_transcripts<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    audio_path: &Path,
    duration_seconds: f64,
    all_transcripts: &[TranscribedSegment],
    used_provider: &str,
    used_model: &str,
    source_language: Option<&str>,
    capture_gaps: bool,
) -> Result<RetranscriptionResult> {
    emit_progress(app, meeting_id, "saving", 80, "Saving transcripts...");

    // Create transcript segments with proper timestamps from VAD, then stitch
    // obvious VAD fragments so the saved transcript reads like prose.
    let mut segments = create_readable_transcript_segments_with_words(all_transcripts);
    let vocabulary = super::vocabulary_settings::load_vocabulary(app);
    super::vocabulary_settings::correct_segments(&vocabulary, &mut segments);
    validate_replacement_transcripts(&segments)?;

    // Save to database
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("App state not available"))?;
    let pool = app_state.db_manager.pool();
    let publication = RetranscriptionFileUpdate {
        retranscribed_at: chrono::Utc::now().to_rfc3339(),
        duration_seconds,
        audio_filename: audio_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audio.mp4")
            .to_string(),
        provider: used_provider.to_string(),
        model: used_model.to_string(),
        source_language: source_language.map(str::to_string),
    };
    let roots = super::recording_preferences::recording_roots(app).await;
    // What the recording folder says produced the text about to be archived.
    let previous_provenance =
        crate::database::transcript_edits::file_provenance(pool, meeting_id, &roots).await;
    commit_retranscription(
        pool,
        meeting_id,
        &segments,
        &publication,
        capture_gaps,
        previous_provenance,
    )
    .await?;

    emit_progress(app, meeting_id, "saving", 90, "Updating recording files...");
    let file_warning = crate::database::transcript_edits::sync_file(pool, meeting_id, &roots)
        .await
        .is_err();
    if file_warning {
        warn!("Retranscription saved; recording files need updating");
    }

    emit_progress(app, meeting_id, "complete", 100, "Retranscription complete");
    Ok(RetranscriptionResult {
        meeting_id: meeting_id.to_string(),
        segments_count: segments.len(),
        duration_seconds,
        language: publication.source_language,
        file_warning,
    })
}

async fn commit_retranscription(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    segments: &[crate::api::TranscriptSegment],
    publication: &RetranscriptionFileUpdate,
    capture_gaps: bool,
    previous_provenance: Option<crate::database::transcript_edits::Provenance>,
) -> Result<()> {
    let mut tx = pool.begin().await?;

    let previous = sqlx::query_as::<_, crate::database::models::Transcript>(
        "SELECT * FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time, timestamp",
    )
    .bind(meeting_id)
    .fetch_all(&mut *tx)
    .await?;
    if !previous.is_empty() {
        // The archived text keeps the settings that produced it, so restoring it
        // later never labels it with this retranscription's provider or model.
        let provenance = crate::database::transcript_edits::revision_provenance(
            &mut tx,
            meeting_id,
            previous_provenance,
        )
        .await
        .map_err(|e| anyhow!(e))?;
        sqlx::query("INSERT INTO transcript_revisions (id, meeting_id, created_at, transcripts_json, provenance_json) VALUES (?, ?, ?, ?, ?)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(meeting_id)
            .bind(chrono::Utc::now().to_rfc3339()).bind(serde_json::to_string(&previous)?)
            .bind(provenance)
            .execute(&mut *tx).await?;
    }

    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to delete existing transcripts: {}", e))?;

    // Previous corrections remain in the archived transcript revision. They
    // cannot be applied to the new recognition result by the correction undo UI.
    sqlx::query("UPDATE transcript_edit_batches SET undone = 1 WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *tx)
        .await?;

    for segment in segments {
        let word_timestamps_json = segment
            .word_timestamps
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| anyhow!("Invalid word timestamps: {}", e))?;
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker, word_timestamps_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&segment.id)
        .bind(meeting_id)
        .bind(&segment.text)
        .bind(&segment.timestamp)
        .bind(segment.audio_start_time)
        .bind(segment.audio_end_time)
        .bind(segment.duration)
        .bind(&segment.speaker)
        .bind(word_timestamps_json)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to insert transcript: {}", e))?;
    }

    // Commit the revision, current text, and file retry together. A restart
    // before publication can reconstruct the files from these committed rows.
    update_retranscription_outcome(&mut tx, meeting_id, capture_gaps).await?;
    sqlx::query("INSERT INTO transcript_file_sync (meeting_id, retranscription_json) VALUES (?, ?)
                 ON CONFLICT(meeting_id) DO UPDATE SET retranscription_json = excluded.retranscription_json")
        .bind(meeting_id)
        .bind(serde_json::to_string(publication)?)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
        .bind(chrono::Utc::now())
        .bind(meeting_id)
        .execute(&mut *tx)
        .await?;
    tx.commit()
        .await
        .map_err(|e| anyhow!("Failed to commit transaction: {}", e))?;
    Ok(())
}

async fn update_retranscription_outcome(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    meeting_id: &str,
    capture_gaps: bool,
) -> Result<()> {
    sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete, recording_files_incomplete, capture_incomplete)
        VALUES (?, 0, 0, 1, ?) ON CONFLICT(meeting_id) DO UPDATE SET
        audio_save_failed = 0, transcription_incomplete = 0, recording_files_incomplete = 1,
        capture_incomplete = recording_outcomes.capture_incomplete OR excluded.capture_incomplete")
        .bind(meeting_id).bind(capture_gaps).execute(&mut **tx).await?;
    Ok(())
}

fn validate_replacement_transcripts(segments: &[crate::api::TranscriptSegment]) -> Result<()> {
    if !segments
        .iter()
        .any(|segment| !segment.text.trim().is_empty())
    {
        return Err(anyhow!(
            "Retranscription produced no text. Your existing transcript was preserved. Try another model or check the recording audio."
        ));
    }
    Ok(())
}

/// Emit progress event
fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    stage: &str,
    progress: u32,
    message: &str,
) {
    let _ = app.emit(
        "retranscription-progress",
        RetranscriptionProgress {
            meeting_id: meeting_id.to_string(),
            stage: stage.to_string(),
            progress_percentage: progress,
            message: message.to_string(),
        },
    );
}

/// Get or initialize the Whisper engine, auto-loading the model if needed
/// If `requested_model` is provided, ensures that specific model is loaded
async fn get_or_init_whisper<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<WhisperEngine>> {
    use crate::whisper_engine::commands::WHISPER_ENGINE;

    let engine = {
        let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            // Determine which model to use
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_whisper_model(app).await?,
            };

            // Check if the correct model is already loaded
            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Whisper model '{}' (current: {:?})",
                    target_model, current_model
                );

                // Discover available models first (populates the internal cache)
                info!("Discovering available Whisper models...");
                if let Err(_discover_err) = e.discover_models().await {
                    warn!("Error during model discovery (continuing anyway)");
                }

                match e.load_model(&target_model).await {
                    Ok(_) => {
                        info!("Whisper model '{}' loaded successfully", target_model);
                        Ok(e)
                    }
                    Err(load_err) => {
                        error!("Failed to load Whisper model");
                        Err(anyhow!(
                            "Failed to load Whisper model '{}': {}",
                            target_model,
                            load_err
                        ))
                    }
                }
            } else {
                info!("Whisper model '{}' already loaded", target_model);
                Ok(e)
            }
        }
        None => Err(anyhow!("Whisper engine not initialized")),
    }
}

/// Get the configured Whisper model name from the database
async fn get_configured_whisper_model<R: Runtime>(app: &AppHandle<R>) -> Result<String> {
    debug!("Getting configured Whisper model from database...");

    let app_state = app.try_state::<AppState>().ok_or_else(|| {
        error!("App state not available");
        anyhow!("App state not available")
    })?;

    debug!("Querying transcript_settings table...");

    // Query the transcript settings from the database - get both provider and model
    let result: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
            .fetch_optional(app_state.db_manager.pool())
            .await
            .map_err(|e| {
                error!("Failed to query transcript config");
                anyhow!("Failed to query transcript config: {}", e)
            })?;

    match result {
        Some((provider, model)) => {
            info!("Loaded transcription configuration");

            // Check if provider is Whisper-based
            if provider == "localWhisper" || provider == "whisper" {
                Ok(model)
            } else {
                error!("Retranscription requires a configured Whisper provider");
                Err(anyhow!("Retranscription requires Whisper. Current provider '{}' does not support retranscription with language selection.", provider))
            }
        }
        None => {
            // Default to configured Whisper model if no config exists
            warn!("No transcription configuration found; using default model");
            Ok(DEFAULT_WHISPER_MODEL.to_string())
        }
    }
}

/// Get or initialize the Parakeet engine, auto-loading the model if needed
async fn get_or_init_parakeet<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<ParakeetEngine>> {
    use crate::parakeet_engine::commands::PARAKEET_ENGINE;

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            // Determine which model to use
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_parakeet_model(app).await?,
            };

            // Check if the correct model is already loaded
            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Parakeet model '{}' (current: {:?})",
                    target_model, current_model
                );

                // Discover available models first
                info!("Discovering available Parakeet models...");
                if let Err(_discover_err) = e.discover_models().await {
                    warn!("Error during Parakeet model discovery (continuing anyway)");
                }

                match e.load_model(&target_model).await {
                    Ok(_) => {
                        info!("Parakeet model '{}' loaded successfully", target_model);
                        Ok(e)
                    }
                    Err(load_err) => {
                        error!("Failed to load Parakeet model");
                        Err(anyhow!(
                            "Failed to load Parakeet model '{}': {}",
                            target_model,
                            load_err
                        ))
                    }
                }
            } else {
                info!("Parakeet model '{}' already loaded", target_model);
                Ok(e)
            }
        }
        None => Err(anyhow!("Parakeet engine not initialized")),
    }
}

/// Get or initialize the Nemotron engine, auto-loading the model if needed.
async fn get_or_init_nemotron<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<crate::nemotron_engine::nemotron_engine::NemotronEngine>> {
    use crate::nemotron_engine::commands::{nemotron_init, NEMOTRON_ENGINE};
    use crate::nemotron_engine::nemotron_engine::NEMOTRON_MODEL;

    nemotron_init(app.clone())
        .await
        .map_err(|e| anyhow!("Failed to initialize Nemotron engine: {}", e))?;

    let engine = {
        let guard = NEMOTRON_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            let target_model = requested_model.unwrap_or(NEMOTRON_MODEL).to_string();
            let current_model = e.get_current_model().await;
            if current_model.as_deref() != Some(target_model.as_str()) {
                if let Err(_discover_err) = e.discover_models().await {
                    warn!("Nemotron model discovery error (continuing)");
                }
                e.load_model(&target_model).await.map_err(|load_err| {
                    anyhow!(
                        "Failed to load Nemotron model '{}': {}",
                        target_model,
                        load_err
                    )
                })?;
            }
            Ok(e)
        }
        None => Err(anyhow!("Nemotron engine not initialized")),
    }
}

/// Get the configured Parakeet model name from the database
async fn get_configured_parakeet_model<R: Runtime>(app: &AppHandle<R>) -> Result<String> {
    debug!("Getting configured Parakeet model from database...");

    let app_state = app.try_state::<AppState>().ok_or_else(|| {
        error!("App state not available");
        anyhow!("App state not available")
    })?;

    // Query the transcript settings from the database
    let result: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
            .fetch_optional(app_state.db_manager.pool())
            .await
            .map_err(|e| {
                error!("Failed to query transcript config");
                anyhow!("Failed to query transcript config: {}", e)
            })?;

    match result {
        Some((provider, model)) => {
            info!("Loaded transcription configuration");

            if provider == "parakeet" {
                Ok(model)
            } else {
                // Default to configured Parakeet model
                warn!("Configured provider is not Parakeet, using default model");
                Ok(DEFAULT_PARAKEET_MODEL.to_string())
            }
        }
        None => {
            // Default to configured Parakeet model if no config exists
            warn!("No transcript config found, using default Parakeet model");
            Ok(DEFAULT_PARAKEET_MODEL.to_string())
        }
    }
}

// Tauri commands

/// Response when retranscription is started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionStarted {
    pub meeting_id: String,
    pub message: String,
}

// Start retranscription.
#[tauri::command]
pub async fn start_retranscription_command<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
) -> Result<RetranscriptionStarted, String> {
    // Check if retranscription is already in progress (guard will be acquired in start_retranscription)
    if is_retranscription_in_progress() {
        return Err("Retranscription already in progress".to_string());
    }

    // Clone values for the spawned task
    let meeting_id_clone = meeting_id.clone();

    // Spawn the retranscription in a background task
    tauri::async_runtime::spawn(async move {
        let result = start_retranscription(
            app,
            meeting_id_clone,
            meeting_folder_path,
            language,
            model,
            provider,
        )
        .await;

        // Errors are already emitted as events in start_retranscription
        // so we just log here for debugging
        if let Err(_e) = result {
            error!("Retranscription failed");
        }
    });

    Ok(RetranscriptionStarted {
        meeting_id,
        message: "Retranscription started".to_string(),
    })
}

#[tauri::command]
pub async fn cancel_retranscription_command() -> bool {
    cancel_retranscription_if_possible()
}

#[cfg(test)]
mod tests {
    use super::super::common::create_transcript_segments;
    use super::*;
    use crate::audio::constants::AUDIO_EXTENSIONS;
    use crate::audio::incremental_saver::find_existing_audio_file;
    use std::path::Path;

    #[tokio::test]
    async fn failed_commit_keeps_the_previous_revision_and_recording_files() {
        let (pool, folder) = publication_fixture().await;
        let segment = create_transcript_segments(&[("new transcript".into(), 0.0, 2000.0)]);
        // A deferred FK violation fails COMMIT after all replacement statements succeeded.
        sqlx::raw_sql(
            "CREATE TABLE commit_guard (id TEXT REFERENCES meetings(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TRIGGER reject_replacement AFTER INSERT ON transcripts
             BEGIN INSERT INTO commit_guard VALUES ('missing'); END;",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(commit_retranscription(
            &pool,
            "review-test",
            &segment,
            &publication(),
            false,
            None
        )
        .await
        .is_err());
        let text: String = sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id = 'a'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(text, "Äpfel project");
        let revisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_revisions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((revisions, pending), (0, 0));
        assert_eq!(
            std::fs::read(folder.path().join("transcripts.json")).unwrap(),
            b"previous copy"
        );
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.path().join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(original["transcription_model"], "old-model");
        assert!(!folder.path().join("recording-outcome.json").exists());
        pool.close().await;
    }

    #[tokio::test]
    async fn committed_replacement_retries_each_folder_file_after_database_reopen() {
        for blocked in [
            "transcripts.json",
            "metadata.json",
            "recording-outcome.json",
        ] {
            let (pool, folder) = publication_fixture().await;
            let segments = create_transcript_segments(&[("new transcript".into(), 0.0, 2000.0)]);
            let blocked_path = folder.path().join(blocked);
            if blocked_path.exists() {
                std::fs::remove_file(&blocked_path).unwrap();
            }
            std::fs::create_dir(&blocked_path).unwrap();
            commit_retranscription(&pool, "review-test", &segments, &publication(), true, None)
                .await
                .unwrap();
            let text: String = sqlx::query_scalar("SELECT transcript FROM transcripts")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(text, "new transcript");
            let revision: String =
                sqlx::query_scalar("SELECT transcripts_json FROM transcript_revisions")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(revision.contains("Äpfel project"));
            let sync = crate::database::transcript_edits::sync_file;
            assert!(sync(&pool, "review-test", &[]).await.is_err(), "{blocked}");
            let pending: bool =
                sqlx::query_scalar("SELECT recording_files_incomplete FROM recording_outcomes")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(pending);

            pool.close().await;
            let pool = sqlx::SqlitePool::connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(folder.path().join("library.sqlite")),
            )
            .await
            .unwrap();
            std::fs::remove_dir(&blocked_path).unwrap();
            sync(&pool, "review-test", &[]).await.unwrap();
            sync(&pool, "review-test", &[]).await.unwrap();

            let metadata: serde_json::Value = serde_json::from_slice(
                &std::fs::read(folder.path().join("metadata.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(metadata["transcription_model"], "tiny");
            assert_eq!(metadata["transcription_source_language"], "de");
            if blocked != "metadata.json" {
                assert_eq!(metadata["meeting_name"], "Synthetic review");
                assert!(metadata.get("detected_summary_language").is_none());
            }
            let outcome = super::super::outcome::RecordingOutcome::read(folder.path())
                .unwrap()
                .unwrap();
            assert!(outcome.capture_incomplete);
            assert!(!outcome.recording_files_incomplete);
            let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(pending, 0);
            pool.close().await;
        }
    }

    fn recorded_metadata(folder: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(folder.join("metadata.json")).unwrap()).unwrap()
    }

    async fn pending_files(pool: &sqlx::SqlitePool) -> (i64, bool, bool) {
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync")
            .fetch_one(pool)
            .await
            .unwrap();
        let (files, capture): (bool, bool) = sqlx::query_as(
            "SELECT recording_files_incomplete, capture_incomplete FROM recording_outcomes",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        (rows, files, capture)
    }

    #[tokio::test]
    async fn restore_after_a_completed_publication_returns_the_previous_provenance() {
        let (pool, folder) = publication_fixture().await;
        std::fs::write(
            folder.path().join("metadata.json"),
            serde_json::json!({
                "meeting_name": "Synthetic review",
                "transcription_provider": "parakeet",
                "transcription_model": "old-model",
                "transcription_source_language": "en",
                "retranscribed_at": "2025-12-31T00:00:00Z",
                "detected_summary_language": "en",
            })
            .to_string(),
        )
        .unwrap();
        let roots = [folder.path().to_path_buf()];
        let sync = crate::database::transcript_edits::sync_file;
        let restore = crate::database::transcript_edits::restore_previous;
        let previous =
            crate::database::transcript_edits::file_provenance(&pool, "review-test", &roots).await;
        assert_eq!(
            previous.as_ref().map(|p| p.model.as_str()),
            Some("old-model")
        );
        let segments = create_transcript_segments(&[("new transcript".into(), 0.0, 2000.0)]);
        commit_retranscription(
            &pool,
            "review-test",
            &segments,
            &publication(),
            true,
            previous,
        )
        .await
        .unwrap();
        sync(&pool, "review-test", &roots).await.unwrap();
        assert_eq!(
            recorded_metadata(folder.path())["transcription_model"],
            "tiny"
        );
        assert_eq!(pending_files(&pool).await, (0, false, true));

        // Restoring after the publication finished rewrites the metadata too.
        restore(&pool, "review-test", &roots).await.unwrap();
        assert_eq!(pending_files(&pool).await, (1, true, true));
        sync(&pool, "review-test", &roots).await.unwrap();
        let metadata = recorded_metadata(folder.path());
        assert_eq!(metadata["transcription_provider"], "parakeet");
        assert_eq!(metadata["transcription_model"], "old-model");
        assert_eq!(metadata["transcription_source_language"], "en");
        assert_eq!(metadata["retranscribed_at"], "2025-12-31T00:00:00Z");
        assert!(metadata.get("transcription_provenance").is_none());
        // Capture gaps survive, and the retry flag clears only after publication.
        assert_eq!(pending_files(&pool).await, (0, false, true));
        let outcome = super::super::outcome::RecordingOutcome::read(folder.path())
            .unwrap()
            .unwrap();
        assert!(outcome.capture_incomplete && !outcome.recording_files_incomplete);
        let text: String = sqlx::query_scalar("SELECT transcript FROM transcripts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(text, "Äpfel project");

        // Restoring again returns to the newer settings, not to a guess.
        restore(&pool, "review-test", &roots).await.unwrap();
        sync(&pool, "review-test", &roots).await.unwrap();
        let metadata = recorded_metadata(folder.path());
        assert_eq!(metadata["transcription_provider"], "localWhisper");
        assert_eq!(metadata["transcription_model"], "tiny");
        assert_eq!(metadata["transcription_source_language"], "de");
        pool.close().await;
    }

    #[tokio::test]
    async fn restore_of_unrecorded_text_marks_provenance_unknown_and_replaces_pending_metadata() {
        let (pool, folder) = publication_fixture().await;
        let roots = [folder.path().to_path_buf()];
        let sync = crate::database::transcript_edits::sync_file;
        let restore = crate::database::transcript_edits::restore_previous;
        // The replaced text never recorded a provider (for example a live transcript).
        let previous =
            crate::database::transcript_edits::file_provenance(&pool, "review-test", &roots).await;
        assert!(previous.is_none());
        let segments = create_transcript_segments(&[("new transcript".into(), 0.0, 2000.0)]);
        commit_retranscription(
            &pool,
            "review-test",
            &segments,
            &publication(),
            true,
            previous,
        )
        .await
        .unwrap();

        // Restore before the retranscription's files were published.
        restore(&pool, "review-test", &roots).await.unwrap();
        let payload: String =
            sqlx::query_scalar("SELECT retranscription_json FROM transcript_file_sync")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(payload.contains("Unknown") && !payload.contains("tiny"));
        assert_eq!(pending_files(&pool).await, (1, true, true));
        sync(&pool, "review-test", &roots).await.unwrap();
        let metadata = recorded_metadata(folder.path());
        for key in [
            "transcription_provider",
            "transcription_model",
            "transcription_source_language",
            "retranscribed_at",
        ] {
            assert!(metadata.get(key).is_none(), "{key}");
        }
        assert_eq!(metadata["transcription_provenance"], "unknown");
        assert_eq!(pending_files(&pool).await, (0, false, true));

        // Restoring back to the newer text recovers its recorded settings.
        restore(&pool, "review-test", &roots).await.unwrap();
        sync(&pool, "review-test", &roots).await.unwrap();
        let metadata = recorded_metadata(folder.path());
        assert_eq!(metadata["transcription_model"], "tiny");
        assert_eq!(metadata["transcription_source_language"], "de");
        assert!(metadata.get("transcription_provenance").is_none());
        pool.close().await;
    }

    fn publication() -> RetranscriptionFileUpdate {
        RetranscriptionFileUpdate {
            retranscribed_at: "2026-01-01T00:00:00Z".into(),
            duration_seconds: 2.0,
            audio_filename: "audio.wav".into(),
            provider: "localWhisper".into(),
            model: "tiny".into(),
            source_language: Some("de".into()),
        }
    }

    async fn publication_fixture() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let folder = tempfile::tempdir().unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(folder.path().join("library.sqlite"))
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path)
                     VALUES ('review-test', 'Synthetic review', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
            .bind(folder.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp)
                     VALUES ('a', 'review-test', 'Äpfel project', '00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();
        std::fs::write(folder.path().join("transcripts.json"), b"previous copy").unwrap();
        std::fs::write(
            folder.path().join("metadata.json"),
            br#"{"meeting_name":"Synthetic review","transcription_model":"old-model","detected_summary_language":"en"}"#,
        ).unwrap();
        (pool, folder)
    }

    #[test]
    fn empty_retranscription_cannot_replace_existing_text() {
        assert!(validate_replacement_transcripts(&[]).is_err());
        let short_reply = create_transcript_segments(&[("Ja.".into(), 0.0, 300.0)]);
        assert!(validate_replacement_transcripts(&short_reply).is_ok());
    }

    #[test]
    fn test_create_transcript_segments_empty() {
        let transcripts: Vec<(String, f64, f64)> = vec![];
        let segments = create_transcript_segments(&transcripts);
        assert!(segments.is_empty());
    }

    #[test]
    fn test_create_transcript_segments_single() {
        let transcripts = vec![
            ("Hello world".to_string(), 0.0, 1500.0), // 0-1.5 seconds
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world");
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(1.5));
        assert_eq!(segments[0].duration, Some(1.5));
    }

    #[test]
    fn test_create_transcript_segments_multiple() {
        let transcripts = vec![
            ("First segment".to_string(), 0.0, 2000.0), // 0-2 seconds
            ("Second segment".to_string(), 3000.0, 5000.0), // 3-5 seconds
            ("Third segment".to_string(), 6500.0, 8000.0), // 6.5-8 seconds
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 3);

        // First segment
        assert_eq!(segments[0].text, "First segment");
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(2.0));
        assert_eq!(segments[0].duration, Some(2.0));

        // Second segment
        assert_eq!(segments[1].text, "Second segment");
        assert_eq!(segments[1].audio_start_time, Some(3.0));
        assert_eq!(segments[1].audio_end_time, Some(5.0));
        assert_eq!(segments[1].duration, Some(2.0));

        // Third segment
        assert_eq!(segments[2].text, "Third segment");
        assert_eq!(segments[2].audio_start_time, Some(6.5));
        assert_eq!(segments[2].audio_end_time, Some(8.0));
        assert_eq!(segments[2].duration, Some(1.5));
    }

    #[test]
    fn test_create_transcript_segments_trims_whitespace() {
        let transcripts = vec![("  Hello with spaces  ".to_string(), 0.0, 1000.0)];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello with spaces");
    }

    #[test]
    fn test_create_transcript_segments_generates_unique_ids() {
        let transcripts = vec![
            ("Segment one".to_string(), 0.0, 1000.0),
            ("Segment two".to_string(), 1000.0, 2000.0),
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 2);
        assert_ne!(segments[0].id, segments[1].id);
        assert!(segments[0].id.starts_with("transcript-"));
        assert!(segments[1].id.starts_with("transcript-"));
    }

    #[test]
    fn test_vad_redemption_time_constant() {
        // Batch processing stays close to live VAD to avoid merging speaker turns.
        assert_eq!(VAD_REDEMPTION_TIME_MS, 500);
    }

    #[test]
    fn test_find_audio_file_common_candidates() {
        let dir = tempfile::tempdir().unwrap();

        // No audio file → error
        assert!(find_existing_audio_file(dir.path()).is_err());

        // Create audio.mp4 — should be found first
        std::fs::write(dir.path().join("audio.mp4"), b"fake").unwrap();
        let found = find_existing_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.mp4");
    }

    #[test]
    fn test_find_audio_file_non_mp4_extensions() {
        let dir = tempfile::tempdir().unwrap();

        // Create audio.wav (imported as .wav, not .mp4)
        std::fs::write(dir.path().join("audio.wav"), b"fake").unwrap();
        let found = find_existing_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.wav");
    }

    #[test]
    fn test_find_audio_file_fallback_scan() {
        let dir = tempfile::tempdir().unwrap();

        // Create a file with an audio extension but non-standard name
        std::fs::write(dir.path().join("my_recording.flac"), b"fake").unwrap();
        // Also add a non-audio file that should be ignored
        std::fs::write(dir.path().join("notes.txt"), b"text").unwrap();

        let found = find_existing_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "my_recording.flac");
    }

    #[test]
    fn test_find_audio_file_priority_order() {
        let dir = tempfile::tempdir().unwrap();

        // Create both audio.m4a and audio.mp4 — mp4 should win (listed first in candidates)
        std::fs::write(dir.path().join("audio.m4a"), b"fake").unwrap();
        std::fs::write(dir.path().join("audio.mp4"), b"fake").unwrap();
        let found = find_existing_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.mp4");
    }

    #[test]
    fn test_find_audio_file_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_existing_audio_file(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No audio file found"));
    }

    #[test]
    fn test_find_audio_file_nonexistent_folder() {
        let result = find_existing_audio_file(Path::new("/nonexistent/path/12345"));
        assert!(result.is_err());
    }

    #[test]
    fn test_audio_extensions_constant() {
        // Verify all expected formats are covered
        assert!(AUDIO_EXTENSIONS.contains(&"mp4"));
        assert!(AUDIO_EXTENSIONS.contains(&"m4a"));
        assert!(AUDIO_EXTENSIONS.contains(&"wav"));
        assert!(AUDIO_EXTENSIONS.contains(&"mp3"));
        assert!(AUDIO_EXTENSIONS.contains(&"flac"));
        assert!(AUDIO_EXTENSIONS.contains(&"ogg"));
        assert!(AUDIO_EXTENSIONS.contains(&"aac"));
        // FFmpeg-backed formats
        assert!(AUDIO_EXTENSIONS.contains(&"mkv"));
        assert!(AUDIO_EXTENSIONS.contains(&"webm"));
        assert!(AUDIO_EXTENSIONS.contains(&"wma"));
        // Non-audio formats
        assert!(!AUDIO_EXTENSIONS.contains(&"txt"));
        assert!(!AUDIO_EXTENSIONS.contains(&"pdf"));
    }

    /// A cancel from a starting recording is accepted only while the retranscription
    /// can still stop, and reports false once it is saving or over, so no
    /// "being stopped" notice is sent for a job that has already succeeded.
    #[test]
    fn cancel_is_accepted_only_before_the_save_and_never_after_the_job_ends() {
        set_retranscription_phase(Phase::Finished);
        assert!(!cancel_retranscription_if_possible(), "no job is running");
        {
            let _phase = PhaseGuard::open();
            assert!(cancel_retranscription_if_possible(), "before the save a cancel takes effect");
            assert!(cancel_retranscription_if_possible(), "repeats stay accepted");
            set_retranscription_phase(Phase::Saving);
            assert!(!cancel_retranscription_if_possible(), "a save in progress cannot be cancelled");
        }
        // Dropping the guard (declared after the job registration) ends the
        // window before the registration is cleared.
        assert!(!cancel_retranscription_if_possible(), "the finished job refuses a cancel");
    }
}
