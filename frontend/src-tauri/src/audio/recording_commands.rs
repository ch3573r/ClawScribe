// audio/recording_commands.rs
//
// Slim Tauri command layer for recording functionality.
// Delegates to transcription and recording modules for actual implementation.

use anyhow::Result;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::{
    default_input_device,  // Get default microphone
    default_output_device, // Get default system audio
    parse_audio_device,
    DeviceEvent,
    DeviceMonitorType,
    RecordingManager,
};

// Import transcription modules
use super::transcription::{self, reset_speech_detected_flag};

// Re-export TranscriptUpdate for backward compatibility
pub use super::transcription::TranscriptUpdate;

// ============================================================================
// GLOBAL STATE
// ============================================================================

// Simple recording state tracking
static IS_RECORDING: AtomicBool = AtomicBool::new(false);
static STOP_OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod stop_tests {
    use super::acquire_stop_owner;
    #[tokio::test]
    async fn duplicate_stop_waits_without_taking_ownership_of_the_next_session() {
        let lock = tokio::sync::Mutex::new(());
        let first = acquire_stop_owner(&lock).await.unwrap();
        let duplicate = acquire_stop_owner(&lock);
        tokio::pin!(duplicate);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut duplicate)
                .await
                .is_err()
        );
        drop(first);
        assert!(duplicate.await.is_none());
        assert!(acquire_stop_owner(&lock).await.is_some());
    }
    #[tokio::test]
    async fn shared_start_preparation_binds_fresh_producer_ids_and_owned_failure_cleanup() {
        use super::*;
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _admission = crate::audio::inference::claim_job().unwrap();
        let runtime = crate::knowledge::KnowledgeState::default();
        let mut first_manager = RecordingManager::new();
        let (first_start, first_sink) =
            prepare_live_recording(&runtime, &mut first_manager, true).unwrap();
        let first = first_start.session_id.clone();
        assert_eq!(first_manager.live_session_id(), Some(first.as_str()));
        let mut update = TranscriptUpdate {
            session_id: Some(first.clone()),
            text: "Ja".into(),
            timestamp: "12:00:00".into(),
            source: "Me".into(),
            sequence_id: 1,
            chunk_start_time: 6.,
            is_partial: true,
            confidence: Some(0.01),
            audio_start_time: 6.,
            audio_end_time: 7.,
            duration: 1.,
            word_timestamps: None,
        };
        first_sink.ingest(&update);
        assert!(runtime.live.snapshot(&first).unwrap().segments.is_empty());
        update.is_partial = false;
        first_sink.ingest(&update);
        assert_eq!(
            runtime.live.snapshot(&first).unwrap().segments[0].text,
            "Ja"
        );
        let mut second_manager = RecordingManager::new();
        let (second_start, second_sink) =
            prepare_live_recording(&runtime, &mut second_manager, true).unwrap();
        let second = second_start.commit();
        assert_ne!(first, second);
        first_manager.mark_detected_call(&first);
        assert!(first_manager.owns_detected_call(&first));
        assert!(
            !second_manager.owns_detected_call(&second),
            "Manual starts have no detector owner"
        );
        second_manager.mark_detected_call(&first);
        assert!(
            !second_manager.owns_detected_call(&first),
            "A late response cannot claim a replacement session"
        );
        assert!(!second_manager.owns_detected_call(&second));
        assert!(uuid::Uuid::parse_str(&second).is_ok());
        drop(first_start); // A late failed-start guard must not clear the newer session.
        first_sink.ingest(&update); // The old producer retains its first ID.
        assert!(runtime.live.snapshot(&second).unwrap().segments.is_empty());
        update.session_id = Some(second.clone());
        second_sink.ingest(&update);
        assert_eq!(
            runtime.live.snapshot(&second).unwrap().segments[0].text,
            "Ja"
        );
        assert_eq!(
            stop_live_assistance(&runtime).map(|identity| identity.session_id),
            Some(second.clone())
        );
        assert!(runtime.live.snapshot(&second).is_err());
    }

    fn generation(wire: &serde_json::Value) -> u64 {
        let decimal = wire["recording_generation"]
            .as_str()
            .expect("producer metadata must carry a decimal generation string");
        let value = decimal.parse::<u64>().expect("generation must fit u64");
        assert!(value > 0, "zero is not an admitted recording generation");
        assert_eq!(decimal, value.to_string(), "generation must be canonical");
        value
    }

    #[tokio::test]
    async fn shared_preparation_pairs_ordered_generations_with_each_live_or_audio_only_producer() {
        use super::*;
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _admission = crate::audio::inference::claim_job().unwrap();
        let runtime = crate::knowledge::KnowledgeState::default();
        let mut previous = 0;
        for transcribes in [true, false, true] {
            let mut manager = RecordingManager::new();
            let (startup, _sink) =
                prepare_live_recording(&runtime, &mut manager, transcribes).unwrap();
            let id = startup.commit();
            let canonical = serde_json::to_value(runtime.live.snapshot(&id).unwrap()).unwrap();
            let producer = serde_json::to_value(manager.live_snapshot().unwrap()).unwrap();
            assert_eq!(canonical["session_id"], id);
            assert_eq!(producer["session_id"], id);
            assert_eq!(producer["transcription_available"], transcribes);
            let current = generation(&canonical);
            assert!(
                current > previous,
                "native generations must order distinct recordings"
            );
            assert_eq!(
                generation(&producer),
                current,
                "the manager must retain its producer's generation"
            );
            previous = current;
        }
    }

    #[tokio::test]
    async fn captured_stop_identity_retains_its_producer_generation_after_a_new_recording_starts() {
        use super::*;
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _admission = crate::audio::inference::claim_job().unwrap();
        let runtime = crate::knowledge::KnowledgeState::default();
        let mut first_manager = RecordingManager::new();
        let (first_start, _first_sink) =
            prepare_live_recording(&runtime, &mut first_manager, true).unwrap();
        let first = first_start.commit();
        let first_wire = serde_json::to_value(runtime.live.snapshot(&first).unwrap()).unwrap();
        let stopped = stop_live_assistance(&runtime);
        assert!(runtime.live.snapshot(&first).is_err());
        let mut next_manager = RecordingManager::new();
        let (next_start, _next_sink) =
            prepare_live_recording(&runtime, &mut next_manager, false).unwrap();
        let next = next_start.commit();
        let next_wire = serde_json::to_value(runtime.live.snapshot(&next).unwrap()).unwrap();
        // Serialize the captured shutdown result only after the replacement
        // starts, exactly when a delayed stopped event could be emitted.
        let stopped_wire = serde_json::to_value(stopped).unwrap();
        assert_eq!(
            stopped_wire["session_id"], first,
            "Stop must retain the old producer UUID"
        );
        assert_eq!(generation(&stopped_wire), generation(&first_wire));
        assert!(generation(&next_wire) > generation(&stopped_wire));
        assert_eq!(runtime.live.current_session_id(), Some(next));
    }

    #[tokio::test]
    async fn late_failed_start_cannot_retire_the_newer_producers_generation() {
        use super::*;
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _admission = crate::audio::inference::claim_job().unwrap();
        let runtime = crate::knowledge::KnowledgeState::default();
        let mut failed_manager = RecordingManager::new();
        let (failed_start, _failed_sink) =
            prepare_live_recording(&runtime, &mut failed_manager, true).unwrap();
        let failed = failed_start.session_id.clone();
        let failed_wire = serde_json::to_value(runtime.live.snapshot(&failed).unwrap()).unwrap();
        let mut current_manager = RecordingManager::new();
        let (current_start, _current_sink) =
            prepare_live_recording(&runtime, &mut current_manager, true).unwrap();
        let current = current_start.commit();
        let before = serde_json::to_value(runtime.live.snapshot(&current).unwrap()).unwrap();
        drop(failed_start);
        let after = serde_json::to_value(runtime.live.snapshot(&current).unwrap()).unwrap();
        assert_eq!(after["session_id"], current);
        assert_eq!(
            generation(&after),
            generation(&before),
            "failed startup cleanup is producer-owned"
        );
        assert!(generation(&after) > generation(&failed_wire));
        assert_eq!(runtime.live.current_session_id(), Some(current));
    }

    #[tokio::test]
    async fn canonical_reload_metadata_reports_the_same_bound_producer_identity_and_generation() {
        use super::*;
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _admission = crate::audio::inference::claim_job().unwrap();
        // Restore all process globals even when an expected RED assertion panics.
        struct Restore {
            manager: Option<RecordingManager>,
            recording: bool,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                *RECORDING_MANAGER.lock().unwrap() = self.manager.take();
                IS_RECORDING.store(self.recording, Ordering::SeqCst);
            }
        }
        let restore = Restore {
            manager: RECORDING_MANAGER.lock().unwrap().take(),
            recording: IS_RECORDING.swap(true, Ordering::SeqCst),
        };
        let runtime = crate::knowledge::KnowledgeState::default();
        let mut manager = RecordingManager::new();
        let (startup, _sink) = prepare_live_recording(&runtime, &mut manager, true).unwrap();
        let id = startup.commit();
        let snapshot = serde_json::to_value(runtime.live.snapshot(&id).unwrap()).unwrap();
        *RECORDING_MANAGER.lock().unwrap() = Some(manager);
        let metadata = get_recording_state().await;
        assert_eq!(
            metadata["session_id"], id,
            "reload must bind the actual recording UUID"
        );
        assert_eq!(generation(&metadata), generation(&snapshot));
        assert_eq!(metadata["is_recording"], true);
        drop(restore);
    }
}
static IS_STOPPING: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_stopping() -> bool {
    IS_STOPPING.load(Ordering::Acquire)
}
pub(crate) fn capture_active() -> bool {
    IS_RECORDING.load(Ordering::Acquire) || is_stopping()
}

struct StopState;
impl Drop for StopState {
    fn drop(&mut self) {
        IS_STOPPING.store(false, Ordering::Release);
    }
}

async fn acquire_stop_owner(
    lock: &tokio::sync::Mutex<()>,
) -> Option<tokio::sync::MutexGuard<'_, ()>> {
    match lock.try_lock() {
        Ok(owner) => Some(owner),
        Err(_) => {
            let _completed = lock.lock().await;
            None
        }
    }
}
static LIVE_TRANSCRIPTION: AtomicBool = AtomicBool::new(true);
static RECORDING_JOB: Mutex<Option<tokio::sync::OwnedSemaphorePermit>> = Mutex::new(None);

// Global recording manager and transcription task to keep them alive during recording
static RECORDING_MANAGER: Mutex<Option<RecordingManager>> = Mutex::new(None);

/// Snapshot the active recording's folder and pause-adjusted time without disk work.
pub(crate) fn bookmark_position() -> Result<(String, f64), String> {
    let guard = RECORDING_MANAGER
        .lock()
        .map_err(|_| "Recording state unavailable.")?;
    let manager = guard
        .as_ref()
        .filter(|m| m.is_recording())
        .ok_or("No active recording.")?;
    let folder = manager
        .get_meeting_folder()
        .ok_or("Recording folder is not ready.")?;
    let seconds = manager
        .get_active_recording_duration()
        .ok_or("Recording time is unavailable.")?;
    Ok((folder.to_string_lossy().into_owned(), seconds))
}
static TRANSCRIPTION_TASK: Mutex<Option<transcription::TranscriptionTask>> = Mutex::new(None);
static PENDING_TRANSCRIPT_SEGMENTS: Mutex<
    Vec<(
        Option<String>,
        crate::audio::recording_saver::TranscriptSegment,
    )>,
> = Mutex::new(Vec::new());

// Listener ID for proper cleanup - prevents microphone from staying active after recording stops
static TRANSCRIPT_LISTENER_ID: Mutex<Option<tauri::EventId>> = Mutex::new(None);

fn transcript_segment_from_update(
    update: TranscriptUpdate,
) -> crate::audio::recording_saver::TranscriptSegment {
    crate::audio::recording_saver::TranscriptSegment {
        id: format!("seg_{}", update.sequence_id),
        text: update.text,
        speaker: match update.source.as_str() {
            "Me" | "Participants" => Some(update.source),
            _ => None,
        },
        audio_start_time: update.audio_start_time,
        audio_end_time: update.audio_end_time,
        duration: update.duration,
        display_time: update.timestamp,
        confidence: update.confidence,
        sequence_id: update.sequence_id,
        word_timestamps: update.word_timestamps,
    }
}

fn persist_transcript_update(update: TranscriptUpdate) {
    let session_id = update.session_id.clone();
    let segment = transcript_segment_from_update(update);
    if let Ok(manager_guard) = RECORDING_MANAGER.lock() {
        if let Some(manager) = manager_guard.as_ref() {
            if session_id
                .as_deref()
                .is_some_and(|id| Some(id) != manager.live_session_id())
            {
                return;
            }
            manager.add_transcript_segment(segment);
        } else if let Ok(mut pending) = PENDING_TRANSCRIPT_SEGMENTS.lock() {
            pending.push((session_id, segment));
        }
    }
}
type ModelValidation = tokio::task::JoinHandle<Result<(), String>>;

/// How long a start waits for the model check before it opens the streams. A
/// missing model fails within this time; a model that has to be loaded does not.
const EARLY_MODEL_VALIDATION_WAIT: std::time::Duration = std::time::Duration::from_millis(250);
const MODEL_VALIDATION_STOPPED: &str = "Failed to initialize speech recognition";

/// Starts checking and loading the speech model. Loading takes seconds, and
/// audio spoken meanwhile used to be lost because capture began only
/// afterwards. The check now runs beside the stream start. `Ok(None)` means
/// the model is ready or not needed; `Ok(Some(_))` is a load still in
/// progress, to be awaited with `finish_model_validation` once the streams
/// are open; `Err` is a failure found before the microphone was opened.
async fn begin_model_validation<R: Runtime>(
    app: &AppHandle<R>,
    transcribes: bool,
) -> Result<Option<ModelValidation>, String> {
    if !transcribes {
        return Ok(None);
    }
    let task_app = app.clone();
    let mut task =
        tokio::spawn(
            async move { transcription::validate_transcription_model_ready(&task_app).await },
        );
    match tokio::time::timeout(EARLY_MODEL_VALIDATION_WAIT, &mut task).await {
        Ok(finished) => finished
            .map_err(|_| MODEL_VALIDATION_STOPPED.to_string())?
            .map(|()| None),
        Err(_) => Ok(Some(task)),
    }
}

async fn finish_model_validation(pending: Option<ModelValidation>) -> Result<(), String> {
    match pending {
        Some(task) => task
            .await
            .map_err(|_| MODEL_VALIDATION_STOPPED.to_string())?,
        None => Ok(()),
    }
}

const CAPTURE_HALTED_DURING_START: &str =
    "Recording could not start because audio capture stopped. Check your microphone and try again.";

/// True when capture stopped itself after the streams opened: a fatal error, or
/// a burst of recoverable ones. The capture state is marked recording by a
/// successful `start_recording`, so a cleared flag means it was halted since.
fn capture_halted_during_start(state: &super::recording_state::RecordingState) -> bool {
    state.has_fatal_error() || !state.is_recording()
}

fn report_model_validation_failure<R: Runtime>(app: &AppHandle<R>, validation_error: &str) {
    error!("Model validation failed");
    // actionable: false shows a toast instead of a modal; download progress
    // already has its own toast.
    let _ = app.emit("transcription-error", serde_json::json!({
        "error": validation_error,
        "userMessage": "Recording cannot start: Transcription model is still downloading. Please wait for the download to complete.",
        "actionable": false
    }));
}

/// Called after recording admission, while the speech model may still be
/// loading; both start paths share it.
fn prepare_live_recording(
    runtime: &crate::knowledge::KnowledgeState,
    manager: &mut RecordingManager,
    transcribes: bool,
) -> Result<
    (
        crate::knowledge::live::LiveStartup,
        super::recording_manager::LiveTranscriptSink,
    ),
    String,
> {
    let startup = runtime.live.prepare(transcribes)?;
    let sink = super::recording_manager::LiveTranscriptSink::new(
        runtime.live.clone(),
        startup.session_id.clone(),
        startup.recording_generation,
    );
    manager.bind_live_session(sink.clone());
    Ok((startup, sink))
}
pub(crate) fn live_snapshot() -> Result<crate::knowledge::live::LiveSnapshot, String> {
    if is_stopping() {
        return Err("This recording session is stopping".into());
    }
    let guard = RECORDING_MANAGER
        .lock()
        .map_err(|_| "Recording state unavailable")?;
    guard
        .as_ref()
        .filter(|manager| manager.is_recording())
        .ok_or("No active recording")?
        .live_snapshot()
}
/// Stop never waits for provider/configuration/index locks or assistance cleanup.
pub(crate) fn stop_live_assistance(
    runtime: &crate::knowledge::KnowledgeState,
) -> Option<crate::knowledge::live::RecordingIdentity> {
    // Capture the same producer's pair atomically before clearing its memory;
    // delayed event emission must never consult a newer global identity.
    runtime.live.stop()
}

fn store_recording_manager(manager: RecordingManager) {
    let mut manager_guard = RECORDING_MANAGER.lock().unwrap();
    *manager_guard = Some(manager);

    // Keep the manager lock while draining so a listener cannot observe None,
    // then enqueue a segment after this flush has already completed.
    if let (Some(manager), Ok(mut pending)) =
        (manager_guard.as_ref(), PENDING_TRANSCRIPT_SEGMENTS.lock())
    {
        for (session_id, segment) in pending.drain(..) {
            if session_id
                .as_deref()
                .is_some_and(|id| Some(id) != manager.live_session_id())
            {
                continue;
            }
            manager.add_transcript_segment(segment);
        }
    }
}

/// Test utility for the actual manager-absent ingestion and Stop restore path.
#[cfg(test)]
pub(crate) fn test_queue_and_restore(
    manager: RecordingManager,
    updates: Vec<TranscriptUpdate>,
) -> Vec<crate::audio::recording_saver::TranscriptSegment> {
    *RECORDING_MANAGER.lock().unwrap() = None;
    PENDING_TRANSCRIPT_SEGMENTS.lock().unwrap().clear();
    for update in updates {
        persist_transcript_update(update);
    }
    store_recording_manager(manager);
    let manager = RECORDING_MANAGER.lock().unwrap().take().unwrap();
    let rows = manager.get_transcript_segments();
    PENDING_TRANSCRIPT_SEGMENTS.lock().unwrap().clear();
    rows
}

// ============================================================================
// PUBLIC TYPES
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RecordingArgs {
    pub save_path: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct TranscriptionStatus {
    pub chunks_in_queue: usize,
    pub is_processing: bool,
    pub last_activity_ms: u64,
    pub queued_audio_seconds: f64,
    pub processing_realtime_factor: Option<f64>,
    pub estimated_seconds_remaining: Option<f64>,
    pub spool_bytes: u64,
    pub total_chunks_queued: u64,
    pub total_chunks_completed: u64,
}

// ============================================================================
// RECORDING COMMANDS
// ============================================================================

/// Start recording with default devices
pub async fn start_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    start_recording_with_meeting_name(app, None).await.map(drop)
}

/// Start recording with default devices and optional meeting name
pub async fn start_recording_with_meeting_name<R: Runtime>(
    app: AppHandle<R>,
    meeting_name: Option<String>,
) -> Result<String, String> {
    let start_timer = Instant::now();
    info!("Starting recording with default devices");

    let mode = super::recording_mode::load(&app)?;
    // A recording pre-empts a local summary or a cancellable batch job
    // (retranscription, import, speaker detection, PC test). A running
    // recording holds an unregistered permit and is never pre-empted.
    let job = super::inference::claim_job_for_recording(
        &app,
        "Local summary stopped because a recording started. Generate it again afterwards.",
    )
    .await?;
    let _ = crate::summary::summary_engine::force_shutdown_sidecar().await;
    let engine_lifecycle_guard = super::common::acquire_engine_lifecycle_lock().await;

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Loading the speech model takes seconds, so it runs while the streams
    // open. A missing model is still reported before the microphone opens.
    info!("🔍 Validating transcription model availability before starting recording...");
    let validation_timer = Instant::now();
    let model_validation = match begin_model_validation(&app, mode.transcribes()).await {
        Ok(pending) => pending,
        Err(validation_error) => {
            report_model_validation_failure(&app, &validation_error);
            return Err(validation_error);
        }
    };

    // Async-first approach - no more blocking operations!
    info!("🚀 Starting async recording initialization");

    // Create new recording manager
    let mut manager = RecordingManager::new();
    manager.set_mode(mode);

    // Load recording preferences to get save path, auto_save, and device preferences.
    let (auto_save, preferred_mic_name, preferred_system_name, save_folder) =
        match super::recording_preferences::load_recording_preferences(&app).await {
            Ok(prefs) => {
                info!("📋 Loaded recording preferences: save_folder={:?}, auto_save={}, preferred_mic={:?}, preferred_system={:?}",
                      prefs.save_folder, prefs.auto_save, prefs.preferred_mic_device, prefs.preferred_system_device);
                (
                    prefs.auto_save,
                    prefs.preferred_mic_device,
                    prefs.preferred_system_device,
                    prefs.save_folder,
                )
            }
            Err(_e) => {
                warn!("Failed to load recording preferences, using defaults");
                (
                    true,
                    None,
                    None,
                    super::recording_preferences::get_default_recordings_folder(),
                )
            }
        };

    // Refuse before opening any device when a minimum meeting cannot fit.
    if mode.saves_audio(auto_save) {
        super::disk_space::ensure_space_to_record(&save_folder).await?;
    }

    // ============================================================================
    // MICROPHONE DEVICE RESOLUTION: Preference → Default → Error
    // ============================================================================
    let microphone_device = match preferred_mic_name {
        Some(pref_name) => {
            info!("🎤 Attempting to use preferred microphone: '{}'", pref_name);
            match parse_audio_device(&pref_name) {
                Ok(device) => {
                    info!("✅ Using preferred microphone: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(_e) => {
                    warn!("Preferred microphone unavailable");
                    warn!("   Falling back to system default microphone...");
                    match default_input_device() {
                        Ok(device) => {
                            info!("✅ Using default microphone: '{}'", device.name);
                            Some(Arc::new(device))
                        }
                        Err(default_err) => {
                            error!(
                                "❌ No microphone available (preferred and default both failed)"
                            );
                            return Err(format!(
                                "No microphone device available. Preferred device '{}' not found, and default microphone unavailable: {}",
                                pref_name, default_err
                            ));
                        }
                    }
                }
            }
        }
        None => {
            info!("🎤 No microphone preference set, using system default");
            match default_input_device() {
                Ok(device) => {
                    info!("✅ Using default microphone: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    error!("❌ No default microphone available");
                    return Err(format!("No microphone device available: {}", e));
                }
            }
        }
    };

    // ============================================================================
    // SYSTEM AUDIO DEVICE RESOLUTION: Preference → Default → None (optional)
    // ============================================================================
    let system_device = match preferred_system_name {
        Some(pref_name) => {
            info!(
                "🔊 Attempting to use preferred system audio: '{}'",
                pref_name
            );
            match parse_audio_device(&pref_name) {
                Ok(device) => {
                    info!("✅ Using preferred system audio: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(_e) => {
                    warn!("Preferred system audio unavailable");
                    warn!("   Falling back to system default...");
                    match default_output_device() {
                        Ok(device) => {
                            info!("✅ Using default system audio: '{}'", device.name);
                            Some(Arc::new(device))
                        }
                        Err(_default_err) => {
                            warn!(
                                "⚠️ No system audio available (preferred and default both failed)"
                            );
                            warn!("   Recording will continue with microphone only");
                            None // System audio is optional
                        }
                    }
                }
            }
        }
        None => {
            info!("🔊 No system audio preference set, using system default");
            match default_output_device() {
                Ok(device) => {
                    info!("✅ Using default system audio: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(_e) => {
                    warn!("⚠️ No default system audio available");
                    warn!("   Recording will continue with microphone only");
                    None // System audio is optional
                }
            }
        }
    };

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        // Example: Meeting 2025-10-03_08-25-23
        let now = chrono::Local::now();
        format!("Meeting {}", now.format("%Y-%m-%d_%H-%M-%S"))
    });
    manager.set_meeting_name(Some(effective_meeting_name));
    manager.set_recordings_folder(save_folder);

    // Record which transcription engine + model this session uses.
    let (tp, tm, source_language) = if mode.transcribes() {
        resolve_transcription_info(&app).await
    } else {
        (None, None, None)
    };
    // Provider-specific live VAD cap: Nemotron is slower per segment, so cap its
    // live segments shorter to reduce perceived latency; others keep the default.
    crate::audio::pipeline::set_live_max_segment_ms_for_provider(tp.as_deref().unwrap_or(""));
    manager.set_transcription_info(tp, tm, source_language);

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Surface non-fatal recording warnings (for example silent system audio or
    // a transcription spool that cannot write to disk).
    let app_for_warning = app.clone();
    manager.set_warning_callback(move |message| {
        let _ = app_for_warning.emit("recording-warning", message);
    });
    let app_for_warning_cleared = app.clone();
    manager.set_warning_cleared_callback(move |message| {
        let _ = app_for_warning_cleared.emit("recording-warning-cleared", message);
    });
    // Live input levels for the meters, about ten a second.
    let app_for_levels = app.clone();
    manager.set_levels_callback(move |levels| {
        let _ = app_for_levels.emit("recording-levels", levels);
    });
    // One warning per recording when free space for the audio runs low.
    let app_for_disk_space = app.clone();
    manager.set_disk_space_callback(move |free_bytes, folder| {
        let _ = app_for_disk_space.emit(
            "recording-disk-space-low",
            serde_json::json!({ "freeBytes": free_bytes, "folder": folder.to_string_lossy() }),
        );
    });

    // Start recording with resolved devices (replaces start_recording_with_defaults_and_auto_save call)
    let (live_startup, live_sink) = prepare_live_recording(
        &app.state::<crate::knowledge::KnowledgeState>(),
        &mut manager,
        mode.transcribes(),
    )?;
    let manager_timer = Instant::now();
    let transcription_receiver = manager
        .start_recording(
            microphone_device,
            system_device,
            mode.saves_audio(auto_save),
        )
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;
    // Audio is being captured from here on. Speech during the rest of the
    // model load waits in the transcription queue.
    if let Err(validation_error) = finish_model_validation(model_validation).await {
        manager.abort_start().await;
        report_model_validation_failure(&app, &validation_error);
        return Err(validation_error);
    }
    info!(
        "✅ Transcription model validation passed in {:?}",
        validation_timer.elapsed()
    );
    // The streams have been open since `start_recording`. A fatal capture error
    // during the model load has already emitted `recording-error` and halted
    // the capture state; committing now would mark a session that takes no
    // audio as recording. Discard it like a failed model load.
    if capture_halted_during_start(manager.get_state()) {
        manager.abort_start().await;
        return Err(CAPTURE_HALTED_DURING_START.to_string());
    }
    let recording_generation = live_startup.recording_generation;
    let live_session_id = live_startup.commit();
    info!(
        "✅ Recording manager opened streams in {:?}",
        manager_timer.elapsed()
    );

    // Store the manager globally to keep it alive
    {
        PENDING_TRANSCRIPT_SEGMENTS.lock().unwrap().clear();
        store_recording_manager(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    LIVE_TRANSCRIPTION.store(mode.transcribes(), Ordering::SeqCst);
    IS_RECORDING.store(true, Ordering::SeqCst);
    *RECORDING_JOB.lock().unwrap() = Some(job);
    drop(engine_lifecycle_guard);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    if mode.transcribes() {
        // CRITICAL: Listen for transcript-update events and save to recording manager
        // This enables transcript history persistence for page reload sync
        // Store listener ID for cleanup during stop_recording to ensure microphone is released
        // The listener is registered before the worker starts: speech captured
        // while the model loaded is already queued and is transcribed at once.
        {
            use tauri::Listener;
            let listener_id = app.listen("transcript-update", move |event: tauri::Event| {
                if let Ok(update) = serde_json::from_str::<TranscriptUpdate>(event.payload()) {
                    persist_transcript_update(update);
                }
            });
            let mut global_listener = TRANSCRIPT_LISTENER_ID.lock().unwrap();
            *global_listener = Some(listener_id);
            info!("✅ Transcript-update event listener registered for history persistence");
        }

        let task_handle =
            transcription::start_transcription_task(app.clone(), transcription_receiver, live_sink);
        {
            let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
            *global_task = Some(task_handle);
        }
    } // Audio-only sessions have no worker or transcript listener.

    // Emit success event
    app.emit(
        "recording-started",
        serde_json::json!({
            "message": "Recording started",
            "session_id": live_session_id,
            "recording_generation": recording_generation,
            "recording_mode": mode,
            "devices": ["Default Microphone", "Default System Audio"],
            "workers": if mode.transcribes() { 1 } else { 0 }
        }),
    )
    .map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!(
        "✅ Recording started successfully with async-first approach in {:?}",
        start_timer.elapsed()
    );

    Ok(live_session_id)
}

/// Start recording with specific devices
pub async fn start_recording_with_devices<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_devices_and_meeting(app, mic_device_name, system_device_name, None)
        .await
        .map(drop)
}

/// Start recording with specific devices and optional meeting name
pub async fn start_recording_with_devices_and_meeting<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<String, String> {
    let start_timer = Instant::now();
    info!("Starting recording with selected devices");

    let mode = super::recording_mode::load(&app)?;
    // A recording pre-empts a local summary or a cancellable batch job
    // (retranscription, import, speaker detection, PC test). A running
    // recording holds an unregistered permit and is never pre-empted.
    let job = super::inference::claim_job_for_recording(
        &app,
        "Local summary stopped because a recording started. Generate it again afterwards.",
    )
    .await?;
    let _ = crate::summary::summary_engine::force_shutdown_sidecar().await;
    let engine_lifecycle_guard = super::common::acquire_engine_lifecycle_lock().await;

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Loading the speech model takes seconds, so it runs while the streams
    // open. A missing model is still reported before the microphone opens.
    info!("🔍 Validating transcription model availability before starting recording...");
    let validation_timer = Instant::now();
    let model_validation = match begin_model_validation(&app, mode.transcribes()).await {
        Ok(pending) => pending,
        Err(validation_error) => {
            report_model_validation_failure(&app, &validation_error);
            return Err(validation_error);
        }
    };

    // Parse devices
    let mic_device = if let Some(ref name) = mic_device_name {
        Some(Arc::new(parse_audio_device(name).map_err(|e| {
            format!("Invalid microphone device '{}': {}", name, e)
        })?))
    } else {
        None
    };

    let system_device = if let Some(ref name) = system_device_name {
        Some(Arc::new(parse_audio_device(name).map_err(|e| {
            format!("Invalid system device '{}': {}", name, e)
        })?))
    } else {
        None
    };

    // Async-first approach for custom devices - no more blocking operations!
    info!("🚀 Starting async recording initialization with custom devices");

    // Create new recording manager
    let mut manager = RecordingManager::new();
    manager.set_mode(mode);

    // Load recording preferences to check save path and auto_save setting.
    let (auto_save, save_folder) =
        match super::recording_preferences::load_recording_preferences(&app).await {
            Ok(prefs) => {
                info!(
                    "📋 Loaded recording preferences: save_folder={:?}, auto_save={}",
                    prefs.save_folder, prefs.auto_save
                );
                (prefs.auto_save, prefs.save_folder)
            }
            Err(_e) => {
                warn!("Failed to load recording preferences, defaulting to auto_save=true");
                (
                    true,
                    super::recording_preferences::get_default_recordings_folder(),
                ) // Default to saving if preferences can't be loaded
            }
        };

    // Refuse before opening any stream when a minimum meeting cannot fit.
    if mode.saves_audio(auto_save) {
        super::disk_space::ensure_space_to_record(&save_folder).await?;
    }

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!("Meeting {}", now.format("%Y-%m-%d_%H-%M-%S"))
    });
    manager.set_meeting_name(Some(effective_meeting_name));
    manager.set_recordings_folder(save_folder);

    // Record which transcription engine + model this session uses.
    let (tp, tm, source_language) = if mode.transcribes() {
        resolve_transcription_info(&app).await
    } else {
        (None, None, None)
    };
    // Provider-specific live VAD cap: Nemotron is slower per segment, so cap its
    // live segments shorter to reduce perceived latency; others keep the default.
    crate::audio::pipeline::set_live_max_segment_ms_for_provider(tp.as_deref().unwrap_or(""));
    manager.set_transcription_info(tp, tm, source_language);

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Surface non-fatal recording warnings (for example silent system audio or
    // a transcription spool that cannot write to disk).
    let app_for_warning = app.clone();
    manager.set_warning_callback(move |message| {
        let _ = app_for_warning.emit("recording-warning", message);
    });
    let app_for_warning_cleared = app.clone();
    manager.set_warning_cleared_callback(move |message| {
        let _ = app_for_warning_cleared.emit("recording-warning-cleared", message);
    });
    // Live input levels for the meters, about ten a second.
    let app_for_levels = app.clone();
    manager.set_levels_callback(move |levels| {
        let _ = app_for_levels.emit("recording-levels", levels);
    });
    // One warning per recording when free space for the audio runs low.
    let app_for_disk_space = app.clone();
    manager.set_disk_space_callback(move |free_bytes, folder| {
        let _ = app_for_disk_space.emit(
            "recording-disk-space-low",
            serde_json::json!({ "freeBytes": free_bytes, "folder": folder.to_string_lossy() }),
        );
    });

    // Start recording with specified devices and auto_save setting
    let (live_startup, live_sink) = prepare_live_recording(
        &app.state::<crate::knowledge::KnowledgeState>(),
        &mut manager,
        mode.transcribes(),
    )?;
    let manager_timer = Instant::now();
    let transcription_receiver = manager
        .start_recording(mic_device, system_device, mode.saves_audio(auto_save))
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;
    // Audio is being captured from here on. Speech during the rest of the
    // model load waits in the transcription queue.
    if let Err(validation_error) = finish_model_validation(model_validation).await {
        manager.abort_start().await;
        report_model_validation_failure(&app, &validation_error);
        return Err(validation_error);
    }
    info!(
        "✅ Transcription model validation passed in {:?}",
        validation_timer.elapsed()
    );
    // The streams have been open since `start_recording`. A fatal capture error
    // during the model load has already emitted `recording-error` and halted
    // the capture state; committing now would mark a session that takes no
    // audio as recording. Discard it like a failed model load.
    if capture_halted_during_start(manager.get_state()) {
        manager.abort_start().await;
        return Err(CAPTURE_HALTED_DURING_START.to_string());
    }
    let recording_generation = live_startup.recording_generation;
    let live_session_id = live_startup.commit();
    info!(
        "✅ Recording manager opened streams in {:?}",
        manager_timer.elapsed()
    );

    // Store the manager globally to keep it alive
    {
        PENDING_TRANSCRIPT_SEGMENTS.lock().unwrap().clear();
        store_recording_manager(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    LIVE_TRANSCRIPTION.store(mode.transcribes(), Ordering::SeqCst);
    IS_RECORDING.store(true, Ordering::SeqCst);
    *RECORDING_JOB.lock().unwrap() = Some(job);
    drop(engine_lifecycle_guard);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    if mode.transcribes() {
        // CRITICAL: Listen for transcript-update events and save to recording manager
        // This enables transcript history persistence for page reload sync
        // Store listener ID for cleanup during stop_recording to ensure microphone is released
        // The listener is registered before the worker starts: speech captured
        // while the model loaded is already queued and is transcribed at once.
        {
            use tauri::Listener;
            let listener_id = app.listen("transcript-update", move |event: tauri::Event| {
                if let Ok(update) = serde_json::from_str::<TranscriptUpdate>(event.payload()) {
                    persist_transcript_update(update);
                }
            });
            let mut global_listener = TRANSCRIPT_LISTENER_ID.lock().unwrap();
            *global_listener = Some(listener_id);
            info!("✅ Transcript-update event listener registered for history persistence");
        }

        let task_handle =
            transcription::start_transcription_task(app.clone(), transcription_receiver, live_sink);
        {
            let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
            *global_task = Some(task_handle);
        }
    } // Audio-only sessions have no worker or transcript listener.

    // Emit success event
    app.emit(
        "recording-started",
        serde_json::json!({
            "message": "Recording started",
            "session_id": live_session_id,
            "recording_generation": recording_generation,
            "recording_mode": mode,
            "devices": [
                mic_device_name.unwrap_or_else(|| "Default Microphone".to_string()),
                system_device_name.unwrap_or_else(|| "Default System Audio".to_string())
            ],
            "workers": if mode.transcribes() { 1 } else { 0 }
        }),
    )
    .map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!(
        "✅ Recording started with custom devices using async-first approach in {:?}",
        start_timer.elapsed()
    );

    Ok(live_session_id)
}

/// Stop recording with optimized graceful shutdown ensuring NO transcript chunks are lost
pub async fn stop_recording<R: Runtime>(
    app: AppHandle<R>,
    _args: RecordingArgs,
) -> Result<bool, String> {
    stop_recording_session(app, None).await
}

#[tauri::command]
pub async fn stop_detected_call_recording<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    let stopped = stop_recording_session(app.clone(), Some(&session_id)).await?;
    if stopped {
        app.emit("recording-stop-complete", true)
            .map_err(|_| "Recording stopped. Open ClawScribe to finish saving.".to_string())?;
    }
    Ok(stopped)
}

pub(crate) fn mark_detected_call(session: &str) {
    if let Ok(mut manager) = RECORDING_MANAGER.lock() {
        if let Some(manager) = manager.as_mut() {
            manager.mark_detected_call(session);
        }
    }
}

async fn stop_recording_session<R: Runtime>(
    app: AppHandle<R>,
    expected_session: Option<&str>,
) -> Result<bool, String> {
    // All entry points share one owner through drain, save and completion.
    // Concurrent callers wait for that owner rather than taking its manager.
    let Some(_owner) = acquire_stop_owner(&STOP_OWNER).await else {
        return Ok(false);
    };
    info!(
        "🛑 Starting optimized recording shutdown - ensuring ALL transcript chunks are preserved"
    );

    // A reconnection in flight owns the recording manager, so every check
    // below would see none. Wait for it first, but not forever: a driver that
    // never returns must not make Stop hang. Then nothing has changed: the
    // recording is still running (and everything captured is already on disk),
    // and the next Stop, or a Quit, finishes it once the device call returns.
    let reconnecting = match wait_for_reconnect(STOP_RECONNECT_WAIT).await {
        Ok(guard) => guard,
        Err(()) => {
            warn!("Stop is waiting on an audio device call that has not returned");
            return Err("The audio device is not responding, so the recording cannot be stopped yet. Everything recorded so far is saved on disk. Try Stop again in a moment.".to_string());
        }
    };

    // Check if recording is active
    if !IS_RECORDING.load(Ordering::SeqCst) {
        info!("Recording was not active");
        return Ok(false);
    }
    // Validate inside the shared stop-owner guard. A delayed detector request
    // cannot take ownership of a subsequent recording after a manual stop.
    if let Some(expected) = expected_session {
        let manager = RECORDING_MANAGER
            .lock()
            .map_err(|_| "Recording state unavailable")?;
        if !manager
            .as_ref()
            .is_some_and(|m| m.owns_detected_call(expected))
        {
            return Ok(false);
        }
    }
    IS_STOPPING.store(true, Ordering::Release);
    let stopped_live_identity =
        stop_live_assistance(&app.state::<crate::knowledge::KnowledgeState>());
    let _stop_state = StopState;
    crate::tray::set_tray_state(&app, crate::tray::RecordingState::Stopping);

    // Emit shutdown progress to frontend
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "stopping_audio",
            "message": "Stopping audio capture...",
            "progress": 20
        }),
    );

    let mut audio_save_failed = false;
    let mut capture_incomplete = false;
    let mut recording_files_incomplete = false;
    // Step 1: Stop audio capture immediately (no more new chunks) with proper error handling
    let manager_for_cleanup = {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        global_manager.take()
    };
    // The manager is ours now; a reconnection can no longer take it.
    drop(reconnecting);

    let stop_result = if let Some(mut manager) = manager_for_cleanup {
        // Use FORCE FLUSH to immediately process all accumulated audio - eliminates 30s delay!
        info!("🚀 Using FORCE FLUSH to eliminate pipeline accumulation delays");
        let result = manager.stop_streams_and_force_flush().await;
        capture_incomplete |= manager.get_state().capture_incomplete();
        // Store manager back for later cleanup
        let manager_for_cleanup = Some(manager);
        (result, manager_for_cleanup)
    } else {
        warn!("No recording manager found to stop");
        (Ok(()), None)
    };

    let (stop_result, mut manager_for_cleanup) = stop_result;

    match stop_result {
        Ok(_) => {
            info!("✅ Audio streams stopped successfully - no more chunks will be created");
        }
        Err(_) => {
            capture_incomplete = true;
            warn!("Audio pipeline did not finish cleanly; preserving recovery data");
        }
    }

    // Put the stopped manager back while queued transcript updates drain. The
    // listener persists into this manager, so taking it out here used to discard
    // every segment produced during post-meeting catch-up.
    if let Some(manager) = manager_for_cleanup.take() {
        store_recording_manager(manager);
    }

    let transcribes = LIVE_TRANSCRIPTION.load(Ordering::SeqCst);
    if transcribes {
        // Step 2: Signal transcription workers to finish processing ALL queued chunks
        let _ = app.emit(
            "recording-shutdown-progress",
            serde_json::json!({
                "stage": "processing_transcripts",
                "message": "Processing remaining transcript chunks...",
                "progress": 40
            }),
        );
    }

    // Drain a bounded amount of recognition work; retained audio supports recovery.
    let transcription_task = {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        global_task.take()
    };

    let mut transcription_incomplete = false;
    if let Some(mut task) = transcription_task {
        const TRANSCRIPTION_DRAIN_TIMEOUT: tokio::time::Duration =
            tokio::time::Duration::from_secs(120);
        const TRANSCRIPTION_CANCEL_GRACE: tokio::time::Duration =
            tokio::time::Duration::from_secs(5);
        info!(
            "⏳ Draining transcription queue (maximum {:?})",
            TRANSCRIPTION_DRAIN_TIMEOUT
        );

        // Enhanced progress monitoring during shutdown
        let progress_app = app.clone();
        let progress_task = tokio::spawn(async move {
            let last_update = std::time::Instant::now();

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                let status = transcription::current_transcription_metrics();
                let remaining = status
                    .as_ref()
                    .map(|status| status.chunks_in_queue)
                    .unwrap_or(0);
                let estimated_seconds = status
                    .as_ref()
                    .and_then(|status| status.estimated_seconds_remaining);

                // Emit periodic progress updates during shutdown
                let elapsed = last_update.elapsed().as_secs();
                let _ = progress_app.emit(
                    "recording-shutdown-progress",
                    serde_json::json!({
                        "stage": "processing_transcripts",
                        "message": format!("Processing {} remaining transcript chunks...", remaining),
                        "progress": 40,
                        "detailed": true,
                        "elapsed_seconds": elapsed,
                        "chunks_remaining": remaining,
                        "estimated_seconds_remaining": estimated_seconds
                    }),
                );
            }
        });

        // Borrow the handle through the timeout. If it expires, retaining the
        // handle lets us cancel and join/abort instead of detaching the task.
        match tokio::time::timeout(TRANSCRIPTION_DRAIN_TIMEOUT, &mut task.handle).await {
            Ok(Ok(())) => {
                info!("Transcription worker finished; checking completion metrics");
            }
            Ok(Err(_e)) => {
                warn!("⚠️ Transcription task completed with error");
                transcription_incomplete = true;
            }
            Err(_) => {
                transcription_incomplete = true;
                let status = transcription::current_transcription_metrics();
                let remaining = status
                    .as_ref()
                    .map(|status| status.chunks_in_queue)
                    .unwrap_or(0);
                warn!("Transcription drain exceeded 120 seconds with chunks remaining; cancelling worker");
                let _ = app.emit("transcription-warning", format!(
                    "Stopped transcription after two minutes with {remaining} audio chunks remaining. The recorded audio is preserved and can be retranscribed."
                ));
                task.cancel();

                if tokio::time::timeout(TRANSCRIPTION_CANCEL_GRACE, &mut task.handle)
                    .await
                    .is_err()
                {
                    warn!("Transcription worker did not stop after cancellation; aborting it");
                    task.handle.abort();
                    let _ = (&mut task.handle).await;
                }
            }
        }
        let outcome = task.mark_stopped();
        if outcome.chunks_in_queue > 0 || outcome.failed_chunks > 0 {
            transcription_incomplete = true;
        }

        // Stop progress monitoring
        progress_task.abort();
    } else if transcribes {
        info!("ℹ️ No transcription task found to wait for");
        if transcription::current_transcription_metrics()
            .is_some_and(|status| status.chunks_in_queue > 0)
        {
            transcription_incomplete = true;
        }
    }

    // Remove the listener only after the worker has exited so every emitted
    // catch-up segment has reached the recording saver.
    {
        use tauri::Listener;
        if let Some(listener_id) = TRANSCRIPT_LISTENER_ID.lock().unwrap().take() {
            app.unlisten(listener_id);
            info!("✅ Transcript-update listener removed after transcription drain");
        }
    }

    let manager_for_cleanup = RECORDING_MANAGER.lock().unwrap().take();

    if transcribes {
        // Step 3: Now safely unload Whisper model after ALL chunks are processed
        let _ = app.emit(
            "recording-shutdown-progress",
            serde_json::json!({
                "stage": "unloading_model",
                "message": "Unloading speech recognition model...",
                "progress": 70
            }),
        );

        info!("🧠 All transcript chunks processed. Now safely unloading transcription model...");

        // Use the session's provider snapshot; stopping never loads credentials or
        // settings that may have changed since recording started.
        let config = manager_for_cleanup
            .as_ref()
            .and_then(|manager| manager.transcription_provider())
            .map(|provider| {
                if crate::audio::transcription::cloud::is_cloud_provider(Some(provider)) {
                    "parakeet"
                } else {
                    provider
                }
            });

        if tokio::time::timeout(std::time::Duration::from_secs(3), async {
            match config {
                Some("parakeet") => {
                    info!("🦜 Unloading Parakeet model...");
                    let engine_clone = {
                        let engine_guard = crate::parakeet_engine::commands::PARAKEET_ENGINE
                            .lock()
                            .unwrap();
                        engine_guard.as_ref().cloned()
                    };

                    if let Some(engine) = engine_clone {
                        let current_model = engine
                            .get_current_model()
                            .await
                            .unwrap_or_else(|| "unknown".to_string());
                        info!("Current Parakeet model before unload: '{}'", current_model);

                        if engine.unload_model().await {
                            info!(
                                "✅ Parakeet model '{}' unloaded successfully",
                                current_model
                            );
                        } else {
                            warn!("Failed to unload Parakeet model");
                        }
                    } else {
                        warn!("⚠️ No Parakeet engine found to unload model");
                    }
                }
                Some("nemotron") => {
                    info!("🌊 Unloading Nemotron model...");
                    let engine_clone = {
                        let engine_guard = crate::nemotron_engine::commands::NEMOTRON_ENGINE
                            .lock()
                            .unwrap();
                        engine_guard.as_ref().cloned()
                    };

                    if let Some(engine) = engine_clone {
                        let current_model = engine
                            .get_current_model()
                            .await
                            .unwrap_or_else(|| "unknown".to_string());
                        if engine.unload_model().await {
                            info!(
                                "✅ Nemotron model '{}' unloaded successfully",
                                current_model
                            );
                        } else {
                            warn!("Failed to unload Nemotron model");
                        }
                    } else {
                        warn!("⚠️ No Nemotron engine found to unload model");
                    }
                }
                _ => {
                    // Default to Whisper
                    info!("🎤 Unloading Whisper model...");
                    let engine_clone = {
                        let engine_guard = crate::whisper_engine::commands::WHISPER_ENGINE
                            .lock()
                            .unwrap();
                        engine_guard.as_ref().cloned()
                    };

                    if let Some(engine) = engine_clone {
                        let current_model = engine
                            .get_current_model()
                            .await
                            .unwrap_or_else(|| "unknown".to_string());
                        info!("Current Whisper model before unload: '{}'", current_model);

                        if engine.unload_model().await {
                            info!("✅ Whisper model '{}' unloaded successfully", current_model);
                        } else {
                            warn!("Failed to unload Whisper model");
                        }
                    } else {
                        warn!("⚠️ No Whisper engine found to unload model");
                    }
                }
            }
        })
        .await
        .is_err()
        {
            warn!("Speech engine still finishing a native call; model retained until it returns");
        }
    }

    // Step 4: Finalize recording state and cleanup resources safely
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "finalizing",
            "message": "Finalizing recording and cleaning up resources...",
            "progress": 90
        }),
    );

    // Perform final cleanup with the manager if available
    let (meeting_folder, meeting_name) = if let Some(mut manager) = manager_for_cleanup {
        info!("🧹 Performing final cleanup and saving recording data");

        // Extract meeting info BEFORE async operations
        let meeting_folder = manager.get_meeting_folder();
        let meeting_name = manager.get_meeting_name();

        match tokio::time::timeout(
            super::audio_spool::encode_timeout(
                manager
                    .get_state()
                    .get_active_recording_duration()
                    .unwrap_or(0.0),
            ) + tokio::time::Duration::from_secs(120), // encoding plus bounded publication/fallback time
            manager.save_recording_only(&app),
        )
        .await
        {
            Ok(Ok(report)) => {
                audio_save_failed |= report.outcome.audio_save_failed;
                capture_incomplete |= report.outcome.capture_incomplete;
                recording_files_incomplete |= report.outcome.recording_files_incomplete;
                info!("Recording file save attempts completed");
            }
            Ok(Err(_e)) => {
                audio_save_failed = true;
                warn!("⚠️ Error during recording cleanup (transcripts preserved)");
                // Don't fail shutdown - transcripts are already preserved
            }
            Err(_) => {
                audio_save_failed = true;
                warn!("Recording finalization deadline reached; originals retained");
                // Don't fail shutdown - transcripts are already preserved
            }
        }

        (meeting_folder, meeting_name)
    } else {
        info!("ℹ️ No recording manager available for cleanup");
        (None, None)
    };

    // The frontend initiates the library save using this folder's authoritative
    // transcript snapshot after the stop operation completes.
    let (folder_path_str, meeting_name_str) = match (&meeting_folder, &meeting_name) {
        (Some(path), Some(name)) => (Some(path.to_string_lossy().to_string()), Some(name.clone())),
        _ => (None, None),
    };

    info!("📤 Preparing recording metadata for frontend save");

    if let Some(folder) = meeting_folder.clone() {
        let outcome = super::outcome::RecordingOutcome {
            audio_save_failed,
            transcription_incomplete,
            capture_incomplete,
            recording_files_incomplete,
            recovery_files_elsewhere: false,
        };
        if !matches!(
            tokio::task::spawn_blocking(move || outcome.write(&folder)).await,
            Ok(Ok(()))
        ) {
            recording_files_incomplete = true;
        }
    }

    // Release the recording job only after the outcome is persisted.
    IS_RECORDING.store(false, Ordering::SeqCst);
    RECORDING_JOB.lock().unwrap().take();
    IS_STOPPING.store(false, Ordering::Release);
    crate::tray::update_tray_menu(&app);

    // Step 5: Complete shutdown
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "complete",
            "message": if audio_save_failed || transcription_incomplete || capture_incomplete || recording_files_incomplete { "Recording stopped; review needed" } else { "Recording stopped" },
            "progress": 100
        }),
    );

    // Emit final stop event with folder_path and meeting_name for frontend to save
    app.emit(
        "recording-stopped",
        serde_json::json!({
            "message": if audio_save_failed {
                "Recording stopped; audio recovery is needed"
            } else if capture_incomplete {
                "Recording stopped; available audio was saved with capture gaps"
            } else if transcription_incomplete {
                "Recording stopped; some queued audio still needs retranscription"
            } else {
                "Recording stopped - frontend will save after all transcripts received"
            },
            "folder_path": folder_path_str,
            "session_id": stopped_live_identity.as_ref().map(|identity| &identity.session_id),
            "recording_generation": stopped_live_identity.as_ref().map(|identity| identity.recording_generation),
            "meeting_name": meeting_name_str,
            "recording_mode": if transcribes { "live" } else { "audio_only" },
            "transcription_incomplete": transcription_incomplete,
            "audio_save_failed": audio_save_failed,
            "capture_incomplete": capture_incomplete,
            "recording_files_incomplete": recording_files_incomplete
        }),
    )
    .map_err(|e| e.to_string())?;

    if transcribes
        && !transcription_incomplete
        && !audio_save_failed
        && !capture_incomplete
        && !recording_files_incomplete
    {
        crate::openclaw::submit_completed_recording(
            app.clone(),
            folder_path_str.clone(),
            meeting_name_str.clone(),
        );
    }

    // Update tray menu to reflect stopped state
    crate::tray::update_tray_menu(&app);

    if transcription_incomplete {
        warn!("Recording stopped with an incomplete live transcript; audio was preserved");
    } else {
        info!("🎉 Recording stopped successfully with all transcript chunks processed");
    }
    Ok(true)
}

/// Check if recording is active
pub async fn is_recording() -> bool {
    IS_RECORDING.load(Ordering::SeqCst)
}

/// Get recording statistics
pub async fn get_transcription_status() -> TranscriptionStatus {
    if !LIVE_TRANSCRIPTION.load(Ordering::SeqCst) {
        return TranscriptionStatus {
            chunks_in_queue: 0,
            is_processing: false,
            last_activity_ms: 0,
            queued_audio_seconds: 0.0,
            processing_realtime_factor: None,
            estimated_seconds_remaining: None,
            spool_bytes: 0,
            total_chunks_queued: 0,
            total_chunks_completed: 0,
        };
    }
    match transcription::current_transcription_metrics() {
        Some(status) => TranscriptionStatus {
            chunks_in_queue: status.chunks_in_queue,
            is_processing: status.is_processing,
            last_activity_ms: status.last_activity_ms,
            queued_audio_seconds: status.queued_audio_seconds,
            processing_realtime_factor: status.processing_realtime_factor,
            estimated_seconds_remaining: status.estimated_seconds_remaining,
            spool_bytes: status.spool_bytes,
            total_chunks_queued: status.total_chunks_queued,
            total_chunks_completed: status.total_chunks_completed,
        },
        None => TranscriptionStatus {
            chunks_in_queue: 0,
            is_processing: false,
            last_activity_ms: 0,
            queued_audio_seconds: 0.0,
            processing_realtime_factor: None,
            estimated_seconds_remaining: None,
            spool_bytes: 0,
            total_chunks_queued: 0,
            total_chunks_completed: 0,
        },
    }
}

/// Pause the current recording
#[tauri::command]
pub async fn pause_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Pausing recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // A reconnection in flight owns the manager; wait for it briefly.
    let _reconnecting = wait_for_reconnect(PAUSE_RECONNECT_WAIT)
        .await
        .map_err(|()| AUDIO_DEVICE_NOT_RESPONDING.to_string())?;

    // Access the recording manager and pause it
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.pause_recording().map_err(|e| e.to_string())?;

        // Emit pause event to frontend
        app.emit(
            "recording-paused",
            serde_json::json!({
                "message": "Recording paused"
            }),
        )
        .map_err(|e| e.to_string())?;

        // Update tray menu to reflect paused state
        crate::tray::update_tray_menu(&app);

        info!("Recording paused successfully");
        Ok(())
    } else {
        Err("No recording manager found".to_string())
    }
}

/// Resume the current recording
#[tauri::command]
pub async fn resume_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Resuming recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // A reconnection in flight owns the manager; wait for it briefly.
    let _reconnecting = wait_for_reconnect(PAUSE_RECONNECT_WAIT)
        .await
        .map_err(|()| AUDIO_DEVICE_NOT_RESPONDING.to_string())?;

    // Access the recording manager and resume it
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.resume_recording().map_err(|e| e.to_string())?;

        // Emit resume event to frontend
        app.emit(
            "recording-resumed",
            serde_json::json!({
                "message": "Recording resumed"
            }),
        )
        .map_err(|e| e.to_string())?;

        // Update tray menu to reflect resumed state
        crate::tray::update_tray_menu(&app);

        info!("Recording resumed successfully");
        Ok(())
    } else {
        Err("No recording manager found".to_string())
    }
}

/// Check if recording is currently paused
pub async fn is_recording_paused() -> bool {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.is_paused()
    } else {
        false
    }
}

/// Get detailed recording state
#[tauri::command]
pub async fn get_recording_state() -> serde_json::Value {
    let is_recording = IS_RECORDING.load(Ordering::SeqCst);
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        serde_json::json!({
            "is_recording": is_recording,
            "session_id": manager.live_session_id(),
            "recording_generation": manager.live_recording_generation(),
            "recording_mode": if LIVE_TRANSCRIPTION.load(Ordering::SeqCst) { "live" } else { "audio_only" },
            "is_paused": manager.is_paused(),
            "is_active": manager.is_active(),
            "recording_duration": manager.get_recording_duration(),
            "active_duration": manager.get_active_recording_duration(),
            "total_pause_duration": manager.get_total_pause_duration(),
            "current_pause_duration": manager.get_current_pause_duration()
        })
    } else {
        serde_json::json!({
            "is_recording": is_recording,
            "session_id": null,
            "recording_generation": null,
            "recording_mode": if LIVE_TRANSCRIPTION.load(Ordering::SeqCst) { "live" } else { "audio_only" },
            "is_paused": false,
            "is_active": false,
            "recording_duration": null,
            "active_duration": null,
            "total_pause_duration": 0.0,
            "current_pause_duration": null
        })
    }
}

/// Get the meeting folder path for the current recording
/// Returns the path if a meeting name was set and folder structure initialized
#[tauri::command]
pub async fn get_meeting_folder_path() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager
            .get_meeting_folder()
            .map(|p| p.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

/// Get accumulated transcript segments from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_transcript_history(
) -> Result<Vec<crate::audio::recording_saver::TranscriptSegment>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_transcript_segments())
    } else {
        Ok(Vec::new()) // No recording active, return empty
    }
}

/// Get meeting name from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_recording_meeting_name() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_meeting_name())
    } else {
        Ok(None)
    }
}

// ============================================================================
// DEVICE MONITORING COMMANDS (AirPods/Bluetooth disconnect/reconnect support)
// ============================================================================

/// Response structure for device events
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
pub enum DeviceEventResponse {
    DeviceDisconnected {
        device_name: String,
        device_type: String,
    },
    DeviceReconnected {
        device_name: String,
        device_type: String,
    },
    DeviceListChanged,
}

impl From<DeviceEvent> for DeviceEventResponse {
    fn from(event: DeviceEvent) -> Self {
        match event {
            DeviceEvent::DeviceDisconnected {
                device_name,
                device_type,
            } => DeviceEventResponse::DeviceDisconnected {
                device_name,
                device_type: format!("{:?}", device_type),
            },
            DeviceEvent::DeviceReconnected {
                device_name,
                device_type,
            } => DeviceEventResponse::DeviceReconnected {
                device_name,
                device_type: format!("{:?}", device_type),
            },
            DeviceEvent::DeviceListChanged => DeviceEventResponse::DeviceListChanged,
        }
    }
}

/// Reconnection status information
#[derive(Debug, Serialize, Clone)]
pub struct ReconnectionStatus {
    pub is_reconnecting: bool,
    pub disconnected_device: Option<DisconnectedDeviceInfo>,
}

/// Information about a disconnected device
#[derive(Debug, Serialize, Clone)]
pub struct DisconnectedDeviceInfo {
    pub name: String,
    pub device_type: String,
}

/// Poll for audio device events (disconnect/reconnect)
/// Should be called periodically (every 1-2 seconds) by frontend during recording
#[tauri::command]
pub async fn poll_audio_device_events() -> Result<Option<DeviceEventResponse>, String> {
    let mut manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_mut() {
        if let Some(event) = manager.poll_device_events() {
            info!("📱 Device event polled: {:?}", event);
            if let DeviceEvent::DeviceDisconnected {
                device_name,
                device_type,
            } = &event
            {
                manager.handle_device_disconnect_event(device_name.clone(), device_type.clone());
            }
            Ok(Some(event.into()))
        } else {
            Ok(None)
        }
    } else {
        // Not recording, no events
        Ok(None)
    }
}

/// Get current reconnection status
/// Returns whether the system is attempting to reconnect and which device
#[tauri::command]
pub async fn get_reconnection_status() -> Result<ReconnectionStatus, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        let state = manager.get_state();
        let disconnected_device = state
            .get_disconnected_device()
            .map(|(device, device_type)| DisconnectedDeviceInfo {
                name: device.name.clone(),
                device_type: match device_type {
                    super::recording_state::DeviceType::Microphone => "Microphone",
                    super::recording_state::DeviceType::System => "SystemAudio",
                }
                .to_string(),
            });

        Ok(ReconnectionStatus {
            is_reconnecting: manager.is_reconnecting(),
            disconnected_device,
        })
    } else {
        // Not recording, no reconnection in progress
        Ok(ReconnectionStatus {
            is_reconnecting: false,
            disconnected_device: None,
        })
    }
}

/// Manually trigger device reconnection attempt
/// Useful for UI "Retry" button
#[tauri::command]
pub async fn attempt_device_reconnect(
    device_name: String,
    device_type: String,
) -> Result<bool, String> {
    // Parse device type first
    let monitor_type = match device_type.as_str() {
        "Microphone" => DeviceMonitorType::Microphone,
        "SystemAudio" => DeviceMonitorType::SystemAudio,
        _ => return Err(format!("Invalid device type: {}", device_type)),
    };

    super::com_anchor::ensure_device_enumerator();
    let handle = tokio::runtime::Handle::current();
    let outcome = with_recording_manager_out(RECONNECT_TIMEOUT, move |manager| {
        // No timeout inside: cancelling between `stop_streams` and
        // `start_streams` would leave the streams stopped. The work runs to
        // its end; only the caller stops waiting for it.
        handle.block_on(manager.attempt_device_reconnect(&device_name, monitor_type))
    })
    .await?;
    let result = outcome.map_err(|e| e.to_string())?;

    match result {
        true => info!("✅ Manual reconnection successful"),
        false => warn!("❌ Manual reconnection failed - device not available"),
    }
    Ok(result)
}

/// Serialises device reconnection with itself, Stop, and Pause/Resume. It is
/// held for as long as a reconnection owns the recording manager, which is
/// until the (possibly blocked) device call has actually returned.
static RECONNECT_OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long a reconnection command waits for its result. Listing devices and
/// reopening a stream take well under a second on a healthy driver. This
/// bounds only the wait: a cpal/WASAPI call that hangs inside the driver
/// cannot be interrupted, so it keeps the manager (and `RECONNECT_OWNER`) until
/// it returns, and then puts the manager back by itself.
const RECONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// How long Stop waits for a reconnection that owns the manager: the
/// reconnection wait plus a margin. Past it Stop fails with an error instead
/// of waiting forever; see `stop_recording_session`.
const STOP_RECONNECT_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// How long Pause and Resume wait for a reconnection before reporting that the
/// device is not responding.
const PAUSE_RECONNECT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

const AUDIO_DEVICE_NOT_RESPONDING: &str =
    "The audio device is not responding. Try again in a moment.";

/// Waits (bounded) until no reconnection owns the recording manager and holds
/// that ownership for the caller.
async fn wait_for_reconnect(
    limit: std::time::Duration,
) -> Result<tokio::sync::MutexGuard<'static, ()>, ()> {
    tokio::time::timeout(limit, RECONNECT_OWNER.lock())
        .await
        .map_err(|_| ())
}

/// Runs `work` on a blocking thread with the active recording manager taken out
/// of the global lock, and always puts the manager back: after success, after
/// an error value, after a panic, and after a call that outlives `limit`.
///
/// While the manager is out, the other recording commands see "no manager" (as
/// during Stop) and transcript updates are buffered, then replayed by
/// `store_recording_manager`. Stop, Pause and Resume wait on `RECONNECT_OWNER`.
/// If `work` has not finished after `limit`, this returns an error and leaves
/// it running; the thread keeps `RECONNECT_OWNER`, and releases it only after
/// the manager is back.
async fn with_recording_manager_out<T: Send + 'static>(
    limit: std::time::Duration,
    work: impl FnOnce(&mut RecordingManager) -> T + Send + 'static,
) -> Result<T, String> {
    let Ok(owner) = RECONNECT_OWNER.try_lock() else {
        return Err("A device reconnection is already in progress.".to_string());
    };
    if is_stopping() {
        return Err("The recording is stopping.".to_string());
    }
    let taken = RECORDING_MANAGER.lock().unwrap().take();
    let Some(mut manager) = taken else {
        return Err("Recording not active".to_string());
    };
    let task = tokio::task::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut manager)));
        store_recording_manager(manager);
        drop(owner);
        result
    });
    match tokio::time::timeout(limit, task).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(_panic))) => Err("The device reconnection failed unexpectedly.".to_string()),
        Ok(Err(error)) => Err(format!("Task join error: {error}")),
        Err(_) => {
            warn!("A device reconnection is taking longer than {limit:?}; it keeps running");
            Err(AUDIO_DEVICE_NOT_RESPONDING.to_string())
        }
    }
}

/// Resolve the transcription provider + model recorded in meeting metadata.
/// When no transcript config exists, recording defaults to Parakeet.
async fn resolve_transcription_info<R: Runtime>(
    app: &AppHandle<R>,
) -> (Option<String>, Option<String>, Option<String>) {
    let (provider, model) =
        match crate::api::api::api_get_transcript_config(app.clone(), app.state(), None).await {
            Ok(Some(c)) => (c.provider, c.model),
            _ => (
                "parakeet".to_string(),
                crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
            ),
        };
    let (provider, model) =
        if crate::audio::transcription::cloud::is_cloud_provider(Some(provider.as_str())) {
            (
                "parakeet".to_string(),
                crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
            )
        } else {
            (provider, model)
        };
    let language_preference = crate::get_language_preference_internal();
    let source_language = super::common::transcription_source_language_hint(
        language_preference.as_deref(),
    )
    .or_else(|| {
        (provider == "nemotron")
            .then(|| {
                super::transcription::nemotron_provider::resolve_requested_language(
                    language_preference.as_deref(),
                    sys_locale::get_locale().as_deref(),
                )
                .ok()
            })
            .flatten()
    });

    (Some(provider), Some(model), source_language)
}

/// Beta (opt-in, default off): toggle energy-based "Me"/"Participants" source
/// attribution. Applies to segments transcribed after the change.
#[tauri::command]
pub async fn set_source_attribution_enabled(enabled: bool) -> Result<(), String> {
    crate::audio::transcription::worker::SOURCE_ATTRIBUTION_ENABLED
        .store(enabled, Ordering::Relaxed);
    info!("Source attribution set to {enabled}");
    Ok(())
}

static CLOUD_TRANSCRIPTION_ENABLED: AtomicBool = AtomicBool::new(false);

/// Beta (opt-in, default off): allow hosted whole-file transcription providers
/// for import and retranscription. Live recording remains local-only.
#[tauri::command]
pub async fn set_cloud_transcription_enabled(enabled: bool) -> Result<(), String> {
    CLOUD_TRANSCRIPTION_ENABLED.store(enabled, Ordering::Relaxed);
    info!("Cloud transcription set to {enabled}");
    Ok(())
}

pub(crate) fn cloud_transcription_enabled() -> bool {
    CLOUD_TRANSCRIPTION_ENABLED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod start_halt_tests {
    use super::capture_halted_during_start;
    use crate::audio::recording_state::{AudioError, RecordingState};

    #[test]
    fn a_healthy_start_is_committed() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        assert!(!capture_halted_during_start(&state));
        // Recoverable glitches alone do not discard a start.
        state.report_error(AudioError::BufferOverflow);
        assert!(!capture_halted_during_start(&state));
    }

    #[test]
    fn a_fatal_capture_error_during_the_model_load_discards_the_start() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.report_error(AudioError::PermissionDenied);
        assert!(capture_halted_during_start(&state));
    }

    #[test]
    fn a_burst_of_recoverable_errors_that_halts_capture_discards_the_start() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        for _ in 0..10 {
            state.report_error(AudioError::StreamFailed);
        }
        // The last error is recoverable, but capture has stopped itself.
        assert!(!state.has_fatal_error());
        assert!(capture_halted_during_start(&state));
    }
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Restores the process-wide recording manager after a test.
    struct Restore(Option<RecordingManager>);
    impl Drop for Restore {
        fn drop(&mut self) {
            *RECORDING_MANAGER.lock().unwrap() = self.0.take();
        }
    }

    fn install_manager() -> Restore {
        let restore = Restore(RECORDING_MANAGER.lock().unwrap().take());
        *RECORDING_MANAGER.lock().unwrap() = Some(RecordingManager::new());
        restore
    }

    async fn until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("condition was not reached");
    }

    #[tokio::test]
    async fn the_manager_is_out_of_the_lock_during_the_work_and_restored_after() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _restore = install_manager();
        let value = with_recording_manager_out(Duration::from_secs(5), |_manager| {
            // No std guard is held while the work runs: the lock is free and
            // other commands see no manager, exactly as during Stop.
            let free = RECORDING_MANAGER.try_lock();
            assert!(matches!(free, Ok(ref guard) if guard.is_none()));
            7
        })
        .await
        .unwrap();
        assert_eq!(value, 7);
        assert!(RECORDING_MANAGER.lock().unwrap().is_some());
        assert!(RECONNECT_OWNER.try_lock().is_ok());
    }

    #[tokio::test]
    async fn a_second_reconnection_is_refused_while_one_runs() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _restore = install_manager();
        let (release, wait) = mpsc::channel::<()>();
        let first = tokio::spawn(with_recording_manager_out(Duration::from_secs(5), move |_| {
            wait.recv().unwrap();
        }));
        until(|| RECORDING_MANAGER.lock().unwrap().is_none()).await;
        let second = with_recording_manager_out(Duration::from_secs(5), |_| ()).await;
        assert_eq!(
            second.unwrap_err(),
            "A device reconnection is already in progress."
        );
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        assert!(RECORDING_MANAGER.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn without_an_active_recording_nothing_runs() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _restore = Restore(RECORDING_MANAGER.lock().unwrap().take());
        let result = with_recording_manager_out(Duration::from_secs(5), |_| {
            panic!("work must not run without a manager")
        })
        .await;
        assert_eq!(result.unwrap_err(), "Recording not active");
    }

    #[tokio::test]
    async fn a_failing_or_panicking_reconnection_still_returns_the_manager() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _restore = install_manager();
        // An error value from the work.
        let failed: Result<Result<bool, String>, String> =
            with_recording_manager_out(Duration::from_secs(5), |_| {
                Err("device unavailable".to_string())
            })
            .await;
        assert_eq!(failed.unwrap().unwrap_err(), "device unavailable");
        assert!(RECORDING_MANAGER.lock().unwrap().is_some());
        // A panic in the work.
        let panicked = with_recording_manager_out(Duration::from_secs(5), |_| {
            panic!("driver crashed");
        })
        .await;
        assert_eq!(
            panicked.unwrap_err(),
            "The device reconnection failed unexpectedly."
        );
        assert!(RECORDING_MANAGER.lock().unwrap().is_some());
        assert!(RECONNECT_OWNER.try_lock().is_ok());
    }

    #[tokio::test]
    async fn a_device_call_that_outlives_the_wait_keeps_the_manager_and_returns_it_later() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _restore = install_manager();
        let (release, wait) = mpsc::channel::<()>();
        let started = std::time::Instant::now();
        let result = with_recording_manager_out(Duration::from_millis(50), move |_| {
            wait.recv().unwrap();
        })
        .await;
        assert_eq!(result.unwrap_err(), AUDIO_DEVICE_NOT_RESPONDING);
        assert!(started.elapsed() < Duration::from_secs(2));
        // The hung call still owns the manager and the reconnect slot.
        assert!(RECORDING_MANAGER.lock().unwrap().is_none());
        assert!(RECONNECT_OWNER.try_lock().is_err());
        // Stop, Pause and Resume do not wait forever on it.
        assert!(wait_for_reconnect(Duration::from_millis(30)).await.is_err());
        // When the call finally returns, the manager is back and the slot free.
        release.send(()).unwrap();
        until(|| RECORDING_MANAGER.lock().unwrap().is_some()).await;
        until(|| RECONNECT_OWNER.try_lock().is_ok()).await;
        assert!(wait_for_reconnect(Duration::from_millis(30)).await.is_ok());
    }

    #[tokio::test]
    async fn stop_style_waiters_proceed_as_soon_as_the_reconnection_ends() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _restore = install_manager();
        let (release, wait) = mpsc::channel::<()>();
        let reconnect = tokio::spawn(with_recording_manager_out(Duration::from_secs(5), move |_| {
            wait.recv().unwrap();
        }));
        until(|| RECORDING_MANAGER.lock().unwrap().is_none()).await;
        let waiter = tokio::spawn(async {
            let _guard = wait_for_reconnect(Duration::from_secs(5)).await.unwrap();
            // Holding the slot, the manager is back: this is where the Stop
            // path checks the detected-call owner and takes the manager.
            RECORDING_MANAGER.lock().unwrap().is_some()
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "the waiter is held back");
        release.send(()).unwrap();
        reconnect.await.unwrap().unwrap();
        assert!(waiter.await.unwrap(), "the manager was restored before the waiter ran");
    }
}
