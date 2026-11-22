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

use super::transcription::engine::LiveModelFailure;
use super::{
    default_input_device,  // Get default microphone
    default_output_device, // Get default system audio
    parse_audio_device,
    DeviceEvent,
    DeviceMonitorType,
    RecordingManager,
};

// Import transcription modules
use super::transcription;

pub(crate) mod floating;

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
    use super::{acquire_stop_owner, stop_should_continue};
    #[tokio::test]
    async fn duplicate_stop_waits_without_taking_ownership_of_the_next_session() {
        let lock = tokio::sync::Mutex::new(());
        let (first, first_waited) = acquire_stop_owner(&lock).await;
        assert!(!first_waited);
        let duplicate = acquire_stop_owner(&lock);
        tokio::pin!(duplicate);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut duplicate)
                .await
                .is_err()
        );
        drop(first);
        assert!(duplicate.await.1, "the duplicate reports that it waited");
        assert!(!acquire_stop_owner(&lock).await.1);
    }

    #[test]
    fn a_stop_that_waited_behind_a_stop_that_gave_up_takes_over() {
        // Did not wait: always goes on to its own checks.
        assert!(stop_should_continue(false, false, 3, 3));
        assert!(stop_should_continue(false, true, 3, 3));
        // Waited and the first Stop finished the recording: nothing to do.
        assert!(!stop_should_continue(true, false, 3, 3));
        // Waited and the first Stop gave up: the same recording still runs, so this Stop takes over.
        assert!(stop_should_continue(true, true, 3, 3));
        // Waited and a different recording has started since: never touched.
        assert!(!stop_should_continue(true, true, 3, 4));
        assert!(!stop_should_continue(true, false, 3, 4));
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
        first_sink.ingest(&update, true);
        assert!(runtime.live.snapshot(&first).unwrap().segments.is_empty());
        update.is_partial = false;
        first_sink.ingest(&update, true);
        assert_eq!(
            runtime.live.snapshot(&first).unwrap().segments[0].text,
            "Ja"
        );
        let mut second_manager = RecordingManager::new();
        let (second_start, second_sink) =
            prepare_live_recording(&runtime, &mut second_manager, true).unwrap();
        let second = second_start.commit();
        assert_ne!(first, second);
        assert!(StopTarget::Session(&first).matches(&first_manager));
        assert!(!StopTarget::Session(&first).matches(&second_manager));
        assert!(!StopTarget::DetectedCall(&first).matches(&first_manager));
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
        first_sink.ingest(&update, true); // The old producer retains its first ID.
        assert!(runtime.live.snapshot(&second).unwrap().segments.is_empty());
        update.session_id = Some(second.clone());
        second_sink.ingest(&update, true);
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
            active: Option<ActiveRecording>,
            recording: bool,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                *RECORDING_MANAGER.lock().unwrap() = self.manager.take();
                *ACTIVE_CAPTURE.lock().unwrap() = self.active.take();
                IS_RECORDING.store(self.recording, Ordering::SeqCst);
            }
        }
        let restore = Restore {
            manager: RECORDING_MANAGER.lock().unwrap().take(),
            active: ACTIVE_CAPTURE.lock().unwrap().take(),
            recording: IS_RECORDING.swap(true, Ordering::SeqCst),
        };
        let runtime = crate::knowledge::KnowledgeState::default();
        let mut manager = RecordingManager::new();
        let (startup, sink) = prepare_live_recording(&runtime, &mut manager, true).unwrap();
        let id = startup.commit();
        let snapshot = serde_json::to_value(runtime.live.snapshot(&id).unwrap()).unwrap();
        *RECORDING_MANAGER.lock().unwrap() = Some(manager);
        let _ = publish_active_recording(&sink);
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
static IS_STARTING: AtomicBool = AtomicBool::new(false);
static IS_STOPPING: AtomicBool = AtomicBool::new(false);

struct StartingState;
impl StartingState {
    fn claim() -> Result<Self, String> {
        if capture_active() || IS_STARTING.swap(true, Ordering::AcqRel) {
            return Err("Recording already in progress".into());
        }
        let guard = Self;
        #[cfg(windows)]
        crate::windows_lifecycle::begin_start()?;
        Ok(guard)
    }
}
impl Drop for StartingState {
    fn drop(&mut self) {
        IS_STARTING.store(false, Ordering::Release);
        #[cfg(windows)]
        crate::windows_lifecycle::start_settled();
    }
}

pub(crate) fn is_stopping() -> bool {
    IS_STOPPING.load(Ordering::Acquire)
}
pub(crate) fn capture_active() -> bool {
    IS_RECORDING.load(Ordering::Acquire) || is_stopping()
}

pub(crate) fn is_starting() -> bool {
    IS_STARTING.load(Ordering::Acquire)
}

pub(crate) fn calendar_recording_id(session_id: &str) -> Result<String, String> {
    if is_stopping() || !IS_RECORDING.load(Ordering::Acquire) {
        return Err("This recording has stopped.".into());
    }
    ACTIVE_CAPTURE
        .lock()
        .ok()
        .and_then(|active| {
            active
                .as_ref()
                .filter(|active| active.sink.session_id == session_id)
                .and_then(|active| active.recording_id.clone())
        })
        .ok_or_else(|| "This recording is unavailable.".into())
}

struct StopState;
// The Stop owner opens admission after it has validated the session and closes
// it under the same lock when the save commits or the Stop unwinds.
struct SavingRecording {
    active_seconds: f64,
    progress: u8,
    message: &'static str,
    session_id: Option<String>,
    start_requested: Option<Instant>,
}

impl SavingRecording {
    fn update(
        &mut self,
        want_recording: Option<bool>,
        session_id: Option<&str>,
        now: Instant,
    ) -> bool {
        if session_id.is_some() && self.session_id.as_deref() != session_id {
            return false;
        }
        match want_recording {
            Some(true) => self.start_requested = Some(now),
            Some(false) => self.start_requested = None,
            None => {}
        }
        true
    }
}

static SAVING_RECORDING: Mutex<Option<SavingRecording>> = Mutex::new(None);

pub(crate) fn set_start_after_save(want_recording: Option<bool>, session_id: Option<&str>) -> bool {
    let mut saving = SAVING_RECORDING.lock().unwrap();
    saving
        .as_mut()
        .is_some_and(|owner| owner.update(want_recording, session_id, Instant::now()))
}

#[tauri::command]
pub fn queue_recording_after_save(session_id: String, enabled: bool) -> bool {
    set_start_after_save(Some(enabled), Some(&session_id))
}

pub(crate) fn same_recording_epoch(epoch: u64) -> bool {
    recording_epoch() == epoch
}

pub(crate) fn recording_epoch() -> u64 {
    RECORDING_EPOCH.load(Ordering::Acquire)
}

#[cfg(test)]
mod save_intent_tests {
    use super::*;

    #[test]
    fn explicit_requests_coalesce_for_one_save_and_can_be_canceled() {
        let now = Instant::now();
        let mut saving = SavingRecording {
            session_id: Some("current".into()),
            active_seconds: 12.0,
            progress: 20,
            message: "Stopping audio capture…",
            start_requested: None,
        };
        assert!(!saving.update(Some(true), Some("old"), now));
        assert!(saving.update(None, None, now)); // shortcut toggle
        assert!(saving.start_requested.is_none());
        assert!(saving.update(Some(true), Some("current"), now));
        assert!(saving.update(Some(true), None, now)); // a second explicit CLI start
        assert_eq!(saving.start_requested, Some(now));
        assert!(!saving.update(Some(false), Some("old"), now));
        assert_eq!(saving.start_requested, Some(now));
        assert!(saving.update(Some(false), Some("current"), now));
        assert!(saving.start_requested.is_none());
    }
}

impl StopState {
    fn report<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        identity: &Option<crate::knowledge::live::RecordingIdentity>,
        stage: &str,
        progress: u8,
        message: &'static str,
    ) {
        if let Some(owner) = SAVING_RECORDING.lock().unwrap().as_mut() {
            owner.progress = progress;
            owner.message = message;
        }
        let _ = app.emit(
            "recording-shutdown-progress",
            serde_json::json!({
                "stage": stage, "identity": identity, "message": message, "progress": progress
            }),
        );
    }

    fn close(&self) -> Option<Instant> {
        let mut saving = SAVING_RECORDING.lock().unwrap();
        IS_STOPPING.store(false, Ordering::Release);
        saving.take().and_then(|owner| owner.start_requested)
    }
}
impl Drop for StopState {
    fn drop(&mut self) {
        self.close();
    }
}

/// Counts recordings: bumped each time one becomes active. A Stop that waited
/// behind another Stop compares it to know whether it is still looking at the
/// recording it was asked to stop.
static RECORDING_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Takes the stop owner lock. The flag is true when another Stop held it first
/// and this one waited for it.
async fn acquire_stop_owner(
    lock: &tokio::sync::Mutex<()>,
) -> (tokio::sync::MutexGuard<'_, ()>, bool) {
    match lock.try_lock() {
        Ok(owner) => (owner, false),
        Err(_) => (lock.lock().await, true),
    }
}

/// Whether a Stop that has the lock should go on. One that did not wait always
/// does. One that waited goes on only if the recording it was asked to stop is
/// still running: the Stop it waited for may have finished it (nothing left to
/// do, so report "not stopped by this call") or may have given up (the
/// recording is still running, so this Stop takes over). A recording that
/// started in between is never touched.
fn stop_should_continue(
    waited: bool,
    recording_now: bool,
    epoch_at_request: u64,
    epoch_now: u64,
) -> bool {
    !waited || (recording_now && epoch_at_request == epoch_now)
}
static LIVE_TRANSCRIPTION: AtomicBool = AtomicBool::new(true);
static RECORDING_JOB: Mutex<Option<tokio::sync::OwnedSemaphorePermit>> = Mutex::new(None);

// Global recording manager and transcription task to keep them alive during recording
static RECORDING_MANAGER: Mutex<Option<RecordingManager>> = Mutex::new(None);

pub(crate) fn active_bookmark_session() -> Option<String> {
    let guard = ACTIVE_CAPTURE.lock().ok()?;
    let active = guard
        .as_ref()
        .filter(|active| active.sink.capture.is_recording() && !is_stopping())?;
    Some(active.sink.session_id.clone())
}

/// Snapshot the requested session's durable ID, folder and pause-adjusted time
/// without disk work. The UI session ID prevents a stale request from writing
/// into a later recording.
pub(crate) fn bookmark_position(session_id: &str) -> Result<(String, String, f64), String> {
    let guard = ACTIVE_CAPTURE
        .lock()
        .map_err(|_| "Recording state unavailable.")?;
    let active = guard
        .as_ref()
        .filter(|active| {
            active.sink.capture.is_recording()
                && !is_stopping()
                && active.sink.session_id == session_id
        })
        .ok_or("This recording has ended.")?;
    let recording_id = active
        .recording_id
        .clone()
        .ok_or("Recording identity is unavailable.")?;
    let folder = active
        .folder
        .as_ref()
        .ok_or("Recording folder is not ready.")?;
    let seconds = active
        .sink
        .capture
        .get_active_recording_duration()
        .ok_or("Recording time is unavailable.")?;
    Ok((recording_id, folder.to_string_lossy().into_owned(), seconds))
}
static TRANSCRIPTION_TASK: Mutex<Option<transcription::TranscriptionTask>> = Mutex::new(None);
type ModelValidation = tokio::task::JoinHandle<Result<(), LiveModelFailure>>;

/// How long a start waits for the model check before it opens the streams. A
/// missing model fails within this time; a model that has to be loaded does not.
const EARLY_MODEL_VALIDATION_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// Starts checking and loading the speech model. Loading takes seconds, and
/// audio spoken meanwhile used to be lost because capture began only
/// afterwards. The check now runs beside the stream start. `Ok(None)` means
/// the model is ready or not needed; `Ok(Some(_))` is a load still in
/// progress, to be awaited with `finish_model_validation` once the streams
/// are open; `Err` is a failure found before the microphone was opened.
async fn begin_model_validation<R: Runtime>(
    app: &AppHandle<R>,
    transcribes: bool,
) -> Result<Option<ModelValidation>, LiveModelFailure> {
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
            .map_err(|_| LiveModelFailure::LoadFailed)?
            .map(|()| None),
        Err(_) => Ok(Some(task)),
    }
}

async fn finish_model_validation(pending: Option<ModelValidation>) -> Result<(), LiveModelFailure> {
    match pending {
        Some(task) => task.await.map_err(|_| LiveModelFailure::LoadFailed)?,
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

fn report_model_validation_failure<R: Runtime>(app: &AppHandle<R>, failure: LiveModelFailure) {
    error!("Live model validation failed: {failure:?}");
    let _ = app.emit("live-model-start-failed", failure);
}

/// Once capture is running, a late recognition failure can switch this
/// session to audio-only without reopening its devices or losing the prefix.
async fn finish_recording_start<R: Runtime>(
    app: &AppHandle<R>,
    manager: &mut RecordingManager,
    pending: Option<ModelValidation>,
    mode: super::recording_mode::RecordingMode,
    auto_save: bool,
) -> Result<super::recording_mode::RecordingMode, String> {
    let validation = finish_model_validation(pending).await;
    let saves_audio = mode.saves_audio(auto_save);
    if capture_halted_during_start(manager.get_state()) {
        manager.abort_start(saves_audio).await;
        return Err(CAPTURE_HALTED_DURING_START.to_string());
    }
    if let Err(error) = validation {
        if saves_audio {
            manager.continue_audio_only();
            return Ok(super::recording_mode::RecordingMode::AudioOnly);
        }
        manager.abort_start(false).await;
        report_model_validation_failure(app, error);
        return Err(error.code());
    }
    Ok(mode)
}

/// Bind the final mode to the new producer before it emits any transcript.
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
    let sink = manager.bind_live_session(
        runtime.live.clone(),
        startup.session_id.clone(),
        startup.recording_generation,
    );
    Ok((startup, sink))
}
pub(crate) fn live_snapshot() -> Result<crate::knowledge::live::LiveSnapshot, String> {
    if is_stopping() {
        return Err("This recording session is stopping".into());
    }
    let sink = ACTIVE_CAPTURE
        .lock()
        .map_err(|_| "Recording state unavailable")?
        .as_ref()
        .map(|active| &active.sink)
        .filter(|sink| sink.capture.is_recording())
        .cloned()
        .ok_or("No active recording")?;
    sink.snapshot()
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
    *RECORDING_MANAGER.lock().unwrap() = Some(manager);
}

/// Publish the active state and worker together so Stop cannot miss the task.
fn activate_recording<R: Runtime>(
    app: &AppHandle<R>,
    transcribes: bool,
    auto_open_floating_controls: bool,
    receiver: transcription::queue::TranscriptionQueueReceiver,
    sink: super::recording_manager::LiveTranscriptSink,
) {
    let controls_session = auto_open_floating_controls.then(|| sink.session_id.clone());
    let folder = publish_active_recording(&sink);
    start_health_watch(sink.session_id.clone(), folder, transcribes);
    LIVE_TRANSCRIPTION.store(transcribes, Ordering::SeqCst);
    let mut task = TRANSCRIPTION_TASK.lock().unwrap();
    RECORDING_EPOCH.fetch_add(1, Ordering::AcqRel);
    IS_RECORDING.store(true, Ordering::SeqCst);
    if transcribes {
        sink.conversation
            .clone()
            .watch(app.clone(), sink.capture.clone());
    }
    *task =
        transcribes.then(|| transcription::start_transcription_task(app.clone(), receiver, sink));
    drop(task);
    if let Some(id) = controls_session {
        floating::open_on_start(app.clone(), id);
    }
    crate::teams_detection::monitor::capture_started(app);
    crate::teams_detection::monitor::join::cancel_pending(app);
}

struct ActiveRecording {
    sink: super::recording_manager::LiveTranscriptSink,
    recording_id: Option<String>,
    folder: Option<std::path::PathBuf>,
    meeting_name: Option<String>,
}

static ACTIVE_CAPTURE: Mutex<Option<ActiveRecording>> = Mutex::new(None);

pub(crate) fn active_capture(
    session_id: &str,
) -> Option<Arc<super::recording_state::RecordingState>> {
    ACTIVE_CAPTURE
        .lock()
        .ok()?
        .as_ref()
        .map(|active| &active.sink)
        .filter(|sink| sink.session_id == session_id)
        .map(|sink| sink.capture.clone())
}

#[derive(Serialize)]
pub(crate) struct CaptureSnapshot {
    #[serde(flatten)]
    health: super::recording_state::CaptureHealth,
    disconnected_devices: Vec<DisconnectedDeviceInfo>,
    reconnecting: bool,
    capture_incomplete: bool,
    transcription: Option<transcription::queue::TranscriptionMetricsSnapshot>,
}

fn capture_snapshot(session_id: &str) -> Option<CaptureSnapshot> {
    let active = ACTIVE_CAPTURE.lock().ok()?;
    let state = &active
        .as_ref()
        .map(|active| &active.sink)
        .filter(|sink| sink.session_id == session_id)?
        .capture;
    let mut health = state.health.lock().ok()?.clone();
    let check_age = RECONNECT_STARTED
        .lock()
        .ok()?
        .as_ref()
        .map(Instant::elapsed);
    if check_age.is_some_and(|age| age >= RECONNECT_TIMEOUT) && !is_stopping() {
        health.warnings.push("The audio device is not responding. Check its connection. If Stop cannot finish, try again after the device responds.".into());
    }
    Some(CaptureSnapshot {
        health,
        disconnected_devices: disconnected_devices(state),
        reconnecting: check_age.is_some_and(|age| age >= std::time::Duration::from_secs(1))
            && !is_stopping(),
        capture_incomplete: state.capture_incomplete(),
        transcription: transcription::current_transcription_metrics(),
    })
}

#[tauri::command]
pub(crate) fn get_live_conversation_metrics(
    session_id: String,
) -> Option<super::live_conversation::Readout> {
    let active = ACTIVE_CAPTURE.lock().ok()?;
    let sink = &active.as_ref()?.sink;
    if sink.session_id != session_id || !sink.capture.is_recording() {
        return None;
    }
    sink.conversation.read()
}

#[tauri::command]
pub(crate) fn get_recording_health(session_id: String) -> Option<CaptureSnapshot> {
    capture_snapshot(&session_id)
}

fn publish_active_recording(
    sink: &super::recording_manager::LiveTranscriptSink,
) -> Option<std::path::PathBuf> {
    let active = {
        let manager = RECORDING_MANAGER.lock().unwrap();
        let manager = manager
            .as_ref()
            .expect("recording manager stored before activation");
        ActiveRecording {
            sink: sink.clone(),
            folder: manager.get_meeting_folder(),
            meeting_name: manager.get_meeting_name(),
            recording_id: manager.recording_id().map(str::to_owned),
        }
    };
    let folder = active.folder.clone();
    *ACTIVE_CAPTURE.lock().unwrap() = Some(active);
    folder
}

fn start_health_watch(session_id: String, folder: Option<std::path::PathBuf>, live: bool) {
    let disk_session = session_id.clone();
    tokio::spawn(async move {
        let scratch = live.then(transcription::queue::transcription_spool_root);
        let capacity = |path: Option<std::path::PathBuf>| async move {
            match path {
                Some(path) => super::disk_space::free_space_bounded(&path).await,
                None => None,
            }
        };
        while active_capture(&disk_session).is_some() {
            let (destination, scratch) =
                tokio::join!(capacity(folder.clone()), capacity(scratch.clone()));
            {
                let Some(state) = active_capture(&disk_session) else {
                    break;
                };
                let mut health = state.health.lock().unwrap();
                health.destination_free_bytes = destination;
                health.scratch_free_bytes = scratch;
            }
            tokio::time::sleep(super::disk_space::CHECK_INTERVAL).await;
        }
    });
    tokio::spawn(async move {
        let mut schedules = std::collections::HashMap::new();
        while let Some(state) = active_capture(&session_id) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if is_stopping() || !state.is_recording() {
                continue;
            }
            let available = {
                let mut guard = RECORDING_MANAGER.lock().unwrap();
                let Some(manager) = guard
                    .as_mut()
                    .filter(|m| m.live_session_id() == Some(session_id.as_str()))
                else {
                    continue;
                };
                let mut available = std::collections::HashSet::new();
                while let Some(event) = manager.poll_device_events() {
                    match event {
                        DeviceEvent::DeviceDisconnected {
                            device_name,
                            device_type,
                        } => {
                            available.remove(&(device_name.clone(), device_type));
                            manager.handle_device_disconnect_event(device_name, device_type);
                        }
                        DeviceEvent::DeviceReconnected {
                            device_name,
                            device_type,
                        } => {
                            available.insert((device_name, device_type));
                        }
                        DeviceEvent::DeviceListChanged => {}
                    }
                }
                available
            };
            use super::recording_state::DeviceType;
            let missing: Vec<_> = state
                .disconnected_devices()
                .into_iter()
                .map(|(device, source)| {
                    let kind = match source {
                        DeviceType::Microphone => DeviceMonitorType::Microphone,
                        DeviceType::System => DeviceMonitorType::SystemAudio,
                    };
                    (device.name.clone(), kind)
                })
                .collect();
            schedules.retain(|key, _| missing.contains(key));
            for key in missing {
                let now = Instant::now();
                let schedule = schedules
                    .entry(key.clone())
                    .or_insert_with(|| ReconnectBackoff::new(now));
                if available.contains(&key) || now >= schedule.next_check {
                    let (name, kind) = &key;
                    let result = reconnect_device(name.clone(), *kind, session_id.clone()).await;
                    if result.is_ok_and(|result| result.reconnected) {
                        schedules.remove(&key);
                    } else {
                        schedule.retry_later(Instant::now());
                    }
                }
            }
        }
    });
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

/// Without the meeting database nothing may start a recording: the app is only
/// waiting for its startup error dialog to be closed.
pub(crate) fn start_allowed() -> Result<(), String> {
    if crate::database::setup::startup_failed() {
        return Err(
            "ClawScribe cannot record because its meeting database could not be opened.".into(),
        );
    }
    Ok(())
}

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
    start_allowed()?;
    let _starting = StartingState::claim()?;
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
    let model_validation = match begin_model_validation(&app, mode.transcribes()).await {
        Ok(pending) => pending,
        Err(validation_error) => {
            report_model_validation_failure(&app, validation_error);
            return Err(validation_error.code());
        }
    };

    // Async-first approach - no more blocking operations!
    info!("🚀 Starting async recording initialization");

    // Create new recording manager
    let mut manager = RecordingManager::new();
    manager.set_mode(mode);

    // Load recording preferences to get save path, auto_save, and device preferences.
    let preferences = super::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|_| "recording_device_preferences_unavailable")?;
    let auto_save = preferences.auto_save;
    let preferred_mic_name = preferences.preferred_mic_device;
    let preferred_system_name = preferences.preferred_system_device;
    let save_folder = preferences.save_folder;

    // Refuse before opening any device when a minimum meeting cannot fit.
    if mode.saves_audio(auto_save) {
        super::disk_space::ensure_space_to_record(&save_folder).await?;
    }

    let microphone_device = Some(Arc::new(match preferred_mic_name {
        Some(name) => {
            parse_audio_device(&name).map_err(|_| "Select a valid microphone in Settings.")?
        }
        None => default_input_device().map_err(|_| "No default microphone is available.")?,
    }));
    let system_device = match preferred_system_name {
        Some(name) => Some(
            parse_audio_device(&name)
                .map_err(|_| "Select a valid system audio device in Settings.")?,
        ),
        None => default_output_device().ok(),
    }
    .map(Arc::new);

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

    // Surface actionable recording warnings, such as a failed spool write.
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
    let manager_timer = Instant::now();
    let transcription_receiver = manager
        .start_recording(
            microphone_device,
            system_device,
            mode.saves_audio(auto_save),
        )
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;
    let requested_mode = mode;
    let mode =
        finish_recording_start(&app, &mut manager, model_validation, mode, auto_save).await?;
    let recording_id = manager.recording_id().map(str::to_owned);
    let recording_folder = manager
        .get_meeting_folder()
        .map(|folder| folder.to_string_lossy().into_owned());
    let recording_name = manager.get_meeting_name();
    let (live_startup, live_sink) = match prepare_live_recording(
        &app.state::<crate::knowledge::KnowledgeState>(),
        &mut manager,
        mode.transcribes(),
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            manager.abort_start(mode.saves_audio(auto_save)).await;
            return Err(error);
        }
    };
    let recording_generation = live_startup.recording_generation;
    let live_session_id = live_startup.commit();
    info!(
        "✅ Recording manager opened streams in {:?}",
        manager_timer.elapsed()
    );

    store_recording_manager(manager);
    *RECORDING_JOB.lock().unwrap() = Some(job);
    drop(engine_lifecycle_guard);
    activate_recording(
        &app,
        mode.transcribes(),
        preferences.auto_open_floating_controls,
        transcription_receiver,
        live_sink,
    );

    // Emit success event
    app.emit(
        "recording-started",
        serde_json::json!({
            "message": "Recording started",
            "session_id": live_session_id,
            "recording_generation": recording_generation,
            "recording_mode": mode,
            "recording_id": recording_id,
            "folder_path": recording_folder,
            "meeting_name": recording_name,
            "devices": ["Default Microphone", "Default System Audio"],
            "workers": if mode.transcribes() { 1 } else { 0 }
        }),
    )
    .map_err(|e| e.to_string())?;

    if mode != requested_mode {
        if let Some(state) = active_capture(&live_session_id) {
            state.report_warning("Speech recognition could not start. Audio is still recording. Choose Transcribe after saving the meeting.");
        }
    }
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
    start_selected_session(
        app,
        mic_device_name,
        system_device_name,
        meeting_name,
        SelectedStart::default(),
    )
    .await
}

#[derive(Default)]
pub(crate) struct SelectedStart<'a> {
    pub(crate) use_default_microphone: bool,
    pub(crate) calendar_link: Option<crate::library::calendar::CalendarLink>,
    pub(crate) summary_language: Option<String>,
    pub(crate) request: Option<&'a crate::start_request::ClaimedStart>,
}

pub(crate) async fn start_selected_session<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
    options: SelectedStart<'_>,
) -> Result<String, String> {
    let start_timer = Instant::now();
    start_allowed()?;
    let _starting = StartingState::claim()?;
    info!("Starting recording with selected devices");
    if let Some(request) = options.request {
        request.check_call()?;
    }

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
    let model_validation = match begin_model_validation(&app, mode.transcribes()).await {
        Ok(pending) => pending,
        Err(validation_error) => {
            report_model_validation_failure(&app, validation_error);
            return Err(validation_error.code());
        }
    };

    // Existing IPC callers can select system audio alone. The product's
    // saved null microphone means system default, even with a named output.
    let mic_device = match mic_device_name.as_deref() {
        Some(name) => {
            Some(parse_audio_device(name).map_err(|e| format!("Invalid microphone device: {e}"))?)
        }
        None if system_device_name.is_some() && !options.use_default_microphone => None,
        None => Some(
            default_input_device()
                .map_err(|e| format!("No default microphone is available: {e}"))?,
        ),
    }
    .map(Arc::new);
    let system_device = match system_device_name.as_deref() {
        Some(name) => Some(
            parse_audio_device(name).map_err(|e| format!("Invalid system audio device: {e}"))?,
        ),
        None => default_output_device().ok(),
    }
    .map(Arc::new);

    // Async-first approach for custom devices - no more blocking operations!
    info!("🚀 Starting async recording initialization with custom devices");

    // Create new recording manager
    let mut manager = RecordingManager::new();
    manager.set_mode(mode);
    manager.set_summary_language(options.summary_language.as_deref());

    // Load recording preferences to check save path and auto_save setting.
    let preferences = super::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|_| "recording_device_preferences_unavailable")?;
    let (auto_save, save_folder) = (preferences.auto_save, preferences.save_folder);

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
    let devices = [mic_device.as_ref(), system_device.as_ref()]
        .into_iter()
        .flatten()
        .map(|device| device.name.clone())
        .collect::<Vec<_>>();
    let manager_timer = Instant::now();
    let mut staged_calendar_id = None;
    // The durable fallback title stays neutral. A calendar subject is only
    // available for live display after staging succeeded, and the library
    // derives its saved title from the still-current draft.
    let calendar_title = options
        .calendar_link
        .as_ref()
        .and_then(|link| link.clone().normalized().ok())
        .and_then(|link| link.subject().map(str::to_owned));
    let requested_mode = mode;
    let startup = async {
        let staged_id = &mut staged_calendar_id;
        let transcription_receiver = manager
            .start_recording_with_preparation(
                mic_device,
                system_device,
                mode.saves_audio(auto_save),
                |recording_id| {
                    let app = &app;
                    let request = options.request;
                    async move {
                        if let Some(request) = request {
                            request.verify_call().await.map_err(anyhow::Error::msg)?;
                        }
                        match crate::library::recording_calendar::stage_initial(
                            app,
                            recording_id.as_deref(),
                            options.calendar_link,
                        )
                        .await
                        {
                            Ok(id) => *staged_id = id,
                            Err(_) if request.is_some_and(|claim| claim.calendar().is_some()) => {
                                anyhow::bail!("Could not link the selected calendar event.");
                            }
                            Err(_) => {
                                let _ = app.emit(
                                    "recording-warning",
                                    "Could not link the selected calendar event. Audio capture continues.",
                                );
                            }
                        }
                        if let Some(request) = request {
                            request.admit(app).map_err(anyhow::Error::msg)?;
                        }
                        Ok(())
                    }
                },
            )
            .await
            .map_err(|error| {
                let message = error.to_string();
                if message == crate::start_request::EXPIRED {
                    message
                } else {
                    format!("Failed to start recording: {message}")
                }
            })?;
        let mode =
            finish_recording_start(&app, &mut manager, model_validation, mode, auto_save).await?;
        let (live_startup, live_sink) = match prepare_live_recording(
            &app.state::<crate::knowledge::KnowledgeState>(),
            &mut manager,
            mode.transcribes(),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                manager.abort_start(mode.saves_audio(auto_save)).await;
                return Err(error);
            }
        };
        Ok((transcription_receiver, mode, live_startup, live_sink))
    }
    .await;
    let (transcription_receiver, mode, live_startup, live_sink) = match startup {
        Ok(started) => started,
        Err(error) => {
            let state = app.state::<crate::state::AppState>();
            // After preparation supplied an ID, the saver clears its folder
            // only when it has removed this start's verified empty directory.
            if crate::library::recording_calendar::discard_failed_start_draft(
                state.db_manager.pool(),
                staged_calendar_id.as_deref(),
                manager.get_meeting_folder().is_none(),
            )
            .await
            .is_err()
            {
                warn!("Could not remove the calendar draft for an empty failed start");
            }
            return Err(error);
        }
    };
    let recording_id = manager.recording_id().map(str::to_owned);
    let recording_folder = manager
        .get_meeting_folder()
        .map(|folder| folder.to_string_lossy().into_owned());
    let recording_name = staged_calendar_id
        .as_ref()
        .and(calendar_title)
        .or_else(|| manager.get_meeting_name());
    let recording_generation = live_startup.recording_generation;
    let live_session_id = live_startup.commit();
    if options.request.is_some_and(|request| request.is_call()) {
        manager.mark_detected_call(&live_session_id);
    }
    info!(
        "✅ Recording manager opened streams in {:?}",
        manager_timer.elapsed()
    );

    store_recording_manager(manager);
    *RECORDING_JOB.lock().unwrap() = Some(job);
    drop(engine_lifecycle_guard);
    activate_recording(
        &app,
        mode.transcribes(),
        preferences.auto_open_floating_controls,
        transcription_receiver,
        live_sink,
    );

    // Emit success event
    app.emit(
        "recording-started",
        serde_json::json!({
            "message": "Recording started",
            "session_id": live_session_id,
            "recording_generation": recording_generation,
            "recording_mode": mode,
            "recording_id": recording_id,
            "folder_path": recording_folder,
            "meeting_name": recording_name,
            "devices": devices,
            "workers": if mode.transcribes() { 1 } else { 0 }
        }),
    )
    .unwrap_or_else(|_| warn!("Could not announce the started recording to the window"));

    if mode != requested_mode {
        if let Some(state) = active_capture(&live_session_id) {
            state.report_warning("Speech recognition could not start. Audio is still recording. Choose Transcribe after saving the meeting.");
        }
    }
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
    stop_recording_session(app, None, None).await
}

#[tauri::command]
pub async fn stop_detected_call_recording<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    stop_recording_session(app, Some(StopTarget::DetectedCall(&session_id)), None).await
}

/// One nudge to the existing snapshot worker. The PCM spool is published
/// independently; neither the signal handler nor this request waits on disk.
#[cfg(windows)]
pub(crate) fn request_recovery_snapshot(attempt: u64) {
    if let Ok(active) = ACTIVE_CAPTURE.try_lock() {
        if let Some(active) = active
            .as_ref()
            .filter(|_| crate::windows_lifecycle::is_current(attempt))
        {
            let state = &active.sink.capture;
            if crate::windows_lifecycle::suspend_requested() && state.is_active() {
                state.mark_capture_incomplete();
            }
            state.request_recovery_snapshot();
        }
    }
}

#[cfg(windows)]
pub(crate) async fn stop_after_windows_suspend<R: Runtime>(app: AppHandle<R>, attempt: u64) {
    if stop_recording_session(app.clone(), None, Some(attempt))
        .await
        .is_err()
        && crate::windows_lifecycle::is_current(attempt)
    {
        let _ = app.emit(
            "recording-stop-failed",
            serde_json::json!({
                "message": "Windows sleep may have interrupted the audio. Press Stop again to save the available recording."
            }),
        );
    }
}

enum StopTarget<'a> {
    Session(&'a str),
    DetectedCall(&'a str),
}

impl StopTarget<'_> {
    fn matches(&self, manager: &RecordingManager) -> bool {
        match *self {
            Self::Session(id) => manager.live_session_id() == Some(id),
            Self::DetectedCall(id) => manager.owns_detected_call(id),
        }
    }
}

async fn stop_recording_session<R: Runtime>(
    app: AppHandle<R>,
    expected_session: Option<StopTarget<'_>>,
    suspend_attempt: Option<u64>,
) -> Result<bool, String> {
    // All entry points share one owner through drain, save and completion.
    // A caller that waited takes over only if the Stop it waited for gave up
    // and left the same recording running.
    let epoch_at_request = RECORDING_EPOCH.load(Ordering::Acquire);
    let (_owner, waited) = acquire_stop_owner(&STOP_OWNER).await;
    if !stop_should_continue(
        waited,
        IS_RECORDING.load(Ordering::SeqCst),
        epoch_at_request,
        RECORDING_EPOCH.load(Ordering::Acquire),
    ) {
        return Ok(false);
    }
    #[cfg(windows)]
    if let Some(attempt) = suspend_attempt {
        if !IS_RECORDING.load(Ordering::Acquire) || !crate::windows_lifecycle::is_current(attempt) {
            return Ok(false);
        }
        let _ = app.emit(
            "recording-warning",
            "Windows sleep interrupted this session. ClawScribe is saving the recording.",
        );
    }
    if waited {
        info!("The Stop this one waited for did not finish; stopping the recording now");
    }
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
            return Err("The audio device is still reconnecting, so Stop could not finish. Try Stop again in a moment.".to_string());
        }
    };

    // Check if recording is active
    if !IS_RECORDING.load(Ordering::SeqCst) {
        info!("Recording was not active");
        return Ok(false);
    }
    // Validate inside the shared stop-owner guard. A delayed call detector or
    // floating window request cannot take ownership of a later recording.
    let (active_seconds, session_id) = {
        let manager = RECORDING_MANAGER
            .lock()
            .map_err(|_| "Recording state unavailable")?;
        if expected_session
            .is_some_and(|expected| !manager.as_ref().is_some_and(|m| expected.matches(m)))
        {
            return Ok(false);
        }
        let active_seconds = manager
            .as_ref()
            .and_then(|m| m.get_active_recording_duration())
            .unwrap_or_default();
        let session_id = manager
            .as_ref()
            .and_then(|m| m.live_session_id())
            .map(str::to_owned);
        (active_seconds, session_id)
    };
    let stopped_live_identity = {
        let mut saving = SAVING_RECORDING.lock().unwrap();
        IS_STOPPING.store(true, Ordering::Release);
        let identity = stop_live_assistance(&app.state::<crate::knowledge::KnowledgeState>());
        *saving = Some(SavingRecording {
            active_seconds,
            progress: 20,
            message: "Stopping audio capture…",
            session_id: identity
                .as_ref()
                .map(|identity| identity.session_id.clone())
                .or_else(|| session_id.clone()),
            start_requested: None,
        });
        identity
    };
    let _stop_state = StopState;
    crate::tray::set_tray_state(&app, crate::tray::RecordingState::Stopping);

    // Emit shutdown progress to frontend
    _stop_state.report(
        &app,
        &stopped_live_identity,
        "stopping_audio",
        20,
        "Stopping audio capture...",
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
        #[cfg(windows)]
        if crate::windows_lifecycle::suspend_requested() && manager.get_state().is_active() {
            manager.get_state().mark_capture_incomplete();
        }
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
            info!("Audio pipeline drained; capture is closed");
        }
        Err(_) => {
            capture_incomplete = true;
            warn!("Audio pipeline did not finish cleanly; preserving recovery data");
        }
    }

    // Keep transcript history available to the UI during the drain. The worker
    // saves into its own recording even while Stop has the manager out.
    if let Some(manager) = manager_for_cleanup.take() {
        store_recording_manager(manager);
    }

    let transcribes = LIVE_TRANSCRIPTION.load(Ordering::SeqCst);
    if transcribes {
        // Step 2: Signal transcription workers to finish processing ALL queued chunks
        _stop_state.report(
            &app,
            &stopped_live_identity,
            "processing_transcripts",
            40,
            "Processing remaining transcript chunks...",
        );
    }

    // Drain a bounded amount of recognition work; retained audio supports recovery.
    let transcription_task = {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        global_task.take()
    };

    let mut transcription_incomplete = false;
    let mut queued_segments = 0;
    let mut unfinished_segments = 0;
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
        let progress_identity = stopped_live_identity.clone();
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
                        "identity": progress_identity,
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
                }
            }
        }
        let outcome = task.mark_stopped();
        queued_segments = outcome.total_chunks_queued;
        unfinished_segments = outcome.chunks_in_queue;
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

    let manager_for_cleanup = RECORDING_MANAGER.lock().unwrap().take();
    if let Some(manager) = &manager_for_cleanup {
        // If the dispatcher was aborted, a child worker may still be exiting.
        // Serialize closing with ingestion before taking the final snapshots.
        manager.close_transcript_input();
    }
    let mut handoff = manager_for_cleanup.as_ref().and_then(|manager| {
        Some((
            manager.recording_id()?.to_owned(),
            manager.library_transcripts(),
            manager.recording_provenance(),
        ))
    });
    let capture_details = manager_for_cleanup.as_ref().map(|manager| {
        let mut details = manager.get_state().health.lock().unwrap().details.clone();
        details.unfinished_segments = unfinished_segments;
        details.empty_live_transcript = super::outcome::EmptyLiveTranscript::from_live_result(
            transcribes,
            handoff
                .as_ref()
                .is_some_and(|(_, transcripts, _)| !transcripts.is_empty()),
            transcription_incomplete || capture_incomplete,
            queued_segments,
        );
        details
    });

    if transcribes {
        // Step 3: Now safely unload Whisper model after ALL chunks are processed
        _stop_state.report(
            &app,
            &stopped_live_identity,
            "unloading_model",
            70,
            "Unloading speech recognition model...",
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
    _stop_state.report(
        &app,
        &stopped_live_identity,
        "finalizing",
        90,
        "Finalizing recording and cleaning up resources...",
    );

    // Perform final cleanup with the manager if available
    let mut finalization_returned = false;
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
            manager.save_recording_only(
                &app,
                super::outcome::RecordingOutcome {
                    transcription_incomplete,
                    capture_incomplete,
                    capture_details: capture_details.clone(),
                    ..Default::default()
                },
            ),
        )
        .await
        {
            Ok(Ok(report)) => {
                finalization_returned = true;
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

        if let Some((_, _, provenance)) = handoff.as_mut() {
            *provenance = manager.recording_provenance();
        }
        (meeting_folder, meeting_name)
    } else {
        info!("ℹ️ No recording manager available for cleanup");
        (None, None)
    };

    // Commit before announcing completion. The folder's stable ID also lets
    // recovery finish a handoff interrupted before the database commit.
    let (folder_path_str, meeting_name_str) = match (&meeting_folder, &meeting_name) {
        (Some(path), Some(name)) => (Some(path.to_string_lossy().to_string()), Some(name.clone())),
        _ => (None, None),
    };

    let mut outcome = super::outcome::RecordingOutcome {
        audio_save_failed,
        transcription_incomplete,
        capture_incomplete,
        recording_files_incomplete,
        recovery_files_elsewhere: false,
        capture_details,
    };
    if let (false, Some(folder)) = (finalization_returned, meeting_folder) {
        let disk_outcome = outcome.clone();
        if !matches!(
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::task::spawn_blocking(move || disk_outcome.write(&folder))
            )
            .await,
            Ok(Ok(Ok(())))
        ) {
            outcome.recording_files_incomplete = true;
        }
    }
    let recording_id = handoff.as_ref().map(|(id, _, _)| id.clone());
    let (meeting_id, transcript_count) = match (handoff, &folder_path_str, &meeting_name_str) {
        (Some((recording_id, transcripts, provenance)), Some(folder), Some(title)) => {
            let _calendar_edits = crate::library::recording_calendar::EDIT.lock().await;
            let state = app.state::<crate::state::AppState>();
            let saved = crate::database::repositories::transcript::TranscriptsRepository::save_recording_handoff(
                state.db_manager.pool(), &recording_id, title, &transcripts, folder.clone(), &outcome, provenance,
            ).await;
            match saved {
                Ok(id) => {
                    // The database commit is authoritative. A failed stamp
                    // leaves the stable recording ID in the original marker.
                    if finalization_returned && id == recording_id {
                        let path = std::path::PathBuf::from(folder);
                        let stamp = id.clone();
                        let stamped = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            tokio::task::spawn_blocking(move || {
                                crate::database::meeting_files::stamp_meeting_id(&path, &stamp)
                            }),
                        )
                        .await;
                        if !matches!(stamped, Ok(Ok(Ok(())))) {
                            warn!("Could not stamp the recording folder after library save");
                        }
                    }
                    (Some(id), transcripts.len())
                }
                Err(_) => {
                    warn!("Recording library handoff failed; the recording folder remains available for recovery");
                    (None, transcripts.len())
                }
            }
        }
        _ => (None, 0),
    };
    let library_save_failed = meeting_id.is_none();
    let floating_session = stopped_live_identity
        .as_ref()
        .map(|identity| identity.session_id.as_str())
        .or(session_id.as_deref());
    let floating_outcome = floating::Status::from_outcome(&outcome, library_save_failed);
    floating_outcome.remember_outcome(floating_session);

    // Release the recording job only after the outcome is persisted.
    ACTIVE_CAPTURE.lock().unwrap().take();
    IS_RECORDING.store(false, Ordering::SeqCst);

    // Step 5: Complete shutdown
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "complete",
            "identity": stopped_live_identity,
            "message": if outcome.needs_recovery() || library_save_failed { "Recording stopped; review needed" } else { "Recording stopped" },
            "progress": 100
        }),
    );

    let emitted = app.emit(
        "recording-stopped",
        serde_json::json!({
            "message": if library_save_failed {
                "Recording stopped. Open Interrupted recordings to recover the meeting."
            } else if audio_save_failed {
                "Recording stopped; audio recovery is needed"
            } else if capture_incomplete {
                "Recording stopped; available audio was saved with capture gaps"
            } else if transcription_incomplete {
                "Recording stopped; some queued audio still needs retranscription"
            } else {
                "Recording saved to the library"
            },
            "folder_path": folder_path_str,
            "session_id": stopped_live_identity.as_ref().map(|identity| &identity.session_id),
            "recording_generation": stopped_live_identity.as_ref().map(|identity| identity.recording_generation),
            "meeting_name": meeting_name_str,
            "meeting_id": meeting_id,
            "recording_id": recording_id,
            "transcript_count": transcript_count,
            "library_save_failed": library_save_failed,
            "recording_mode": if transcribes { "live" } else { "audio_only" },
            "transcription_incomplete": transcription_incomplete,
            "audio_save_failed": audio_save_failed,
            "capture_incomplete": capture_incomplete,
            "recording_files_incomplete": outcome.recording_files_incomplete
        }),
    );
    if emitted.is_err() {
        warn!("Could not notify the webview that recording stopped");
    }

    // Release the old job before admitting another start. Take the request
    // under the admission lock so a request racing the commit cannot vanish.
    RECORDING_JOB.lock().unwrap().take();
    let queued = _stop_state.close();
    floating_outcome.finish(&app, floating_session);
    if let Some(requested) = queued {
        if !library_save_failed
            && suspend_attempt.is_none()
            && !crate::exit_guard::exit_in_progress()
            && requested.elapsed() < crate::start_request::START_TIMEOUT
        {
            let next_app = app.clone();
            tauri::async_runtime::spawn(async move {
                crate::tray::start_after_save(&next_app, epoch_at_request, requested).await;
            });
        } else {
            crate::tray::queued_start_failed(&app);
        }
    }
    crate::tray::update_tray_menu(&app);

    if let Some(id) = meeting_id {
        if suspend_attempt.is_none() {
            if !audio_save_failed {
                crate::webhook::recording_saved(&app, &id);
            }
            if transcribes {
                wake_saved_recording_summary(&app, id);
            }
        }
        info!("Recording stop committed to the library");
    }
    Ok(true)
}

/// Dispatch only after library registration or repair commits. The summary
/// entry point owns eligibility, consent and duplicate protection.
pub(crate) fn wake_saved_recording_summary<R: Runtime>(app: &AppHandle<R>, meeting_id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let pool = app
            .state::<crate::state::AppState>()
            .db_manager
            .pool()
            .clone();
        if crate::summary::automatic::start(app, pool, meeting_id)
            .await
            .is_err()
        {
            warn!("Could not dispatch automatic notes for the saved recording");
        }
    });
}

/// Check if recording is active
pub async fn is_recording() -> bool {
    IS_RECORDING.load(Ordering::SeqCst)
}

/// The folder a running (or stopping) recording writes to.
pub(crate) fn active_recording_folder() -> super::interrupted::ActiveFolder {
    active_recording_recovery().0
}

pub(crate) fn active_recording_recovery() -> (super::interrupted::ActiveFolder, Option<String>) {
    use super::interrupted::ActiveFolder;
    if IS_STARTING.load(Ordering::Acquire) {
        return (ActiveFolder::Unknown, None);
    }
    if !capture_active() {
        return (ActiveFolder::None, None);
    }
    let Ok(active) = ACTIVE_CAPTURE.try_lock() else {
        return (ActiveFolder::Unknown, None);
    };
    let Some(active) = active.as_ref() else {
        return (ActiveFolder::Unknown, None);
    };
    (
        active
            .folder
            .clone()
            .map_or(ActiveFolder::Unknown, ActiveFolder::Folder),
        active.recording_id.clone(),
    )
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

/// Pause the current recording.
#[tauri::command]
pub async fn pause_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    set_recording_paused(&app, None, true).await
}

#[tauri::command]
pub async fn resume_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    set_recording_paused(&app, None, false).await
}

async fn set_recording_paused<R: Runtime>(
    app: &AppHandle<R>,
    expected_session: Option<&str>,
    paused: bool,
) -> Result<(), String> {
    if !IS_RECORDING.load(Ordering::Acquire) {
        return Err("No recording is currently active.".into());
    }
    let epoch = recording_epoch();
    {
        // Pause changes capture state only. Serialize with Stop admission,
        // which can run independently of a driver call.
        let _saving = SAVING_RECORDING.lock().unwrap();
        let guard = ACTIVE_CAPTURE.lock().unwrap();
        let sink = guard
            .as_ref()
            .map(|active| &active.sink)
            .filter(|sink| {
                !is_stopping() && recording_epoch() == epoch && sink.capture.is_recording()
            })
            .filter(|sink| expected_session.is_none_or(|id| sink.session_id == id))
            .ok_or("This recording is no longer active.")?;
        if paused {
            sink.capture.pause_recording()
        } else {
            sink.capture.resume_recording()
        }
        .map_err(|error| error.to_string())?;
    }
    let (event, message) = if paused {
        ("recording-paused", "Recording paused")
    } else {
        ("recording-resumed", "Recording resumed")
    };
    let _ = app.emit(event, serde_json::json!({ "message": message }));
    crate::tray::update_tray_menu(app);
    Ok(())
}

/// Check if recording is currently paused
pub async fn is_recording_paused() -> bool {
    ACTIVE_CAPTURE
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|active| active.sink.capture.is_paused())
}

/// Get detailed recording state
#[tauri::command]
pub async fn get_recording_state() -> serde_json::Value {
    let is_recording = IS_RECORDING.load(Ordering::SeqCst);
    let active = ACTIVE_CAPTURE.lock().unwrap();
    if let Some(sink) = active.as_ref().map(|active| &active.sink) {
        let state = &sink.capture;
        serde_json::json!({
            "is_recording": is_recording,
            "session_id": sink.session_id,
            "recording_generation": sink.recording_generation,
            "recording_mode": if LIVE_TRANSCRIPTION.load(Ordering::SeqCst) { "live" } else { "audio_only" },
            "is_paused": state.is_paused(),
            "is_active": state.is_active(),
            "recording_duration": state.get_recording_duration(),
            "active_duration": state.get_active_recording_duration(),
            "total_pause_duration": state.get_total_pause_duration(),
            "current_pause_duration": state.get_current_pause_duration()
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
    Ok(ACTIVE_CAPTURE.lock().unwrap().as_ref().and_then(|active| {
        active
            .folder
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
    }))
}

/// Get accumulated transcript segments from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_transcript_history(
) -> Result<Vec<crate::audio::recording_saver::TranscriptSegment>, String> {
    let sink = ACTIVE_CAPTURE
        .lock()
        .unwrap()
        .as_ref()
        .map(|active| active.sink.clone());
    Ok(sink.map_or_else(Vec::new, |sink| sink.transcript_segments()))
}

/// Get meeting name from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_recording_meeting_name() -> Result<Option<String>, String> {
    Ok(ACTIVE_CAPTURE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|active| active.meeting_name.clone()))
}

// ============================================================================
// DEVICE MONITORING COMMANDS (AirPods/Bluetooth disconnect/reconnect support)
// ============================================================================

#[derive(Debug, Serialize, Clone)]
pub struct DisconnectedDeviceInfo {
    pub name: String,
    pub device_type: String,
}

fn disconnected_devices(
    state: &super::recording_state::RecordingState,
) -> Vec<DisconnectedDeviceInfo> {
    use super::recording_state::DeviceType;
    state
        .disconnected_devices()
        .into_iter()
        .map(|(device, kind)| DisconnectedDeviceInfo {
            name: device.name.clone(),
            device_type: match kind {
                DeviceType::Microphone => "Microphone",
                DeviceType::System => "SystemAudio",
            }
            .into(),
        })
        .collect()
}

/// Manually trigger device reconnection attempt
/// Useful for UI "Retry" button
#[tauri::command]
pub async fn attempt_device_reconnect(
    device_name: String,
    device_type: String,
    session_id: String,
) -> Result<super::recording_manager::ReconnectResult, String> {
    // Parse device type first
    let monitor_type = match device_type.as_str() {
        "Microphone" => DeviceMonitorType::Microphone,
        "SystemAudio" => DeviceMonitorType::SystemAudio,
        _ => return Err(format!("Invalid device type: {}", device_type)),
    };

    reconnect_device(device_name, monitor_type, session_id).await
}

async fn reconnect_device(
    device_name: String,
    monitor_type: DeviceMonitorType,
    session_id: String,
) -> Result<super::recording_manager::ReconnectResult, String> {
    if active_capture(&session_id).is_none() {
        return Err("The recording session has ended.".into());
    }
    super::com_anchor::ensure_device_enumerator();
    let handle = tokio::runtime::Handle::current();
    let outcome = with_recording_manager_out(RECONNECT_TIMEOUT, move |manager| {
        if manager.live_session_id() != Some(session_id.as_str()) {
            return Err(anyhow::anyhow!("The recording session has ended"));
        }
        // No timeout inside: cancelling between `stop_streams` and
        // `start_streams` would leave the streams stopped. The work runs to
        // its end; only the caller stops waiting for it.
        handle.block_on(manager.attempt_device_reconnect(&device_name, monitor_type))
    })
    .await?;
    outcome.map_err(|_| "Could not check the audio device. Try reconnecting again.".into())
}

/// Serialises device reconnection with itself, Stop, and Pause/Resume. It is
/// held for as long as a reconnection owns the recording manager, which is
/// until the (possibly blocked) device call has actually returned.
static RECONNECT_OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static RECONNECT_STARTED: Mutex<Option<Instant>> = Mutex::new(None);

struct ReconnectBackoff {
    next_check: Instant,
    delay: std::time::Duration,
}

impl ReconnectBackoff {
    fn new(now: Instant) -> Self {
        let delay = std::time::Duration::from_secs(12);
        Self {
            next_check: now + delay,
            delay,
        }
    }

    fn retry_later(&mut self, now: Instant) {
        self.delay = (self.delay * 2).min(std::time::Duration::from_secs(60));
        self.next_check = now + self.delay;
    }
}

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
/// Session commands use the shared active recording state while the manager
/// is out. Stop waits on `RECONNECT_OWNER` before taking the manager.
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
    *RECONNECT_STARTED.lock().unwrap() = Some(Instant::now());
    let task = tokio::task::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut manager)));
        if result.is_err() {
            // The work may have stopped the streams before it failed: say so
            // before the manager goes back, so the recording is not silently
            // incomplete.
            let state = manager.get_state();
            state.mark_capture_incomplete();
            state.report_warning(
                "An audio device operation failed unexpectedly and some audio may be missing. Try reconnecting the device.",
            );
        }
        store_recording_manager(manager);
        *RECONNECT_STARTED.lock().unwrap() = None;
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
    let Ok(route) = transcription::engine::live_transcription_route(app).await else {
        return (None, None, None);
    };
    let Some(provider) = route.provider else {
        return (None, None, None);
    };
    let model = route.configured_model.or_else(|| match provider {
        "parakeet" => Some(crate::config::DEFAULT_PARAKEET_MODEL.to_string()),
        "nemotron" => Some(crate::nemotron_engine::nemotron_engine::NEMOTRON_MODEL.to_string()),
        _ => None,
    });
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

    (Some(provider.to_string()), model, source_language)
}

/// Beta (opt-in, default off): toggle energy-based "Me"/"Participants" source
/// attribution. Applies to segments transcribed after the change.
#[tauri::command]
pub async fn set_source_attribution_enabled<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let _change = crate::behavior_settings::CHANGE_LOCK.lock().await;
    apply_source_attribution(&app, enabled).await
}

/// Shared by the command and the hand-over of the webview's old values.
pub(crate) async fn apply_source_attribution<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let app = app.clone();
    // Stored first: a failed save leaves the running value as it is.
    tauri::async_runtime::spawn_blocking(move || {
        crate::behavior_settings::save_source_attribution(&app, enabled)
    })
    .await
    .map_err(|_| "Could not save the setting".to_string())??;
    crate::audio::transcription::worker::SOURCE_ATTRIBUTION_ENABLED
        .store(enabled, Ordering::Relaxed);
    info!("Source attribution set to {enabled}");
    Ok(())
}

static CLOUD_TRANSCRIPTION_ENABLED: AtomicBool = AtomicBool::new(false);

/// Beta (opt-in, default off): allow hosted whole-file transcription providers
/// for import and retranscription. Live recording remains local-only.
#[tauri::command]
pub async fn set_cloud_transcription_enabled<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let _change = crate::behavior_settings::CHANGE_LOCK.lock().await;
    apply_cloud_transcription(&app, enabled).await
}

/// Shared by the command and the hand-over of the webview's old values.
pub(crate) async fn apply_cloud_transcription<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let app = app.clone();
    // Stored first: a failed save leaves the running value as it is.
    tauri::async_runtime::spawn_blocking(move || {
        crate::behavior_settings::save_cloud_transcription(&app, enabled)
    })
    .await
    .map_err(|_| "Could not save the setting".to_string())??;
    CLOUD_TRANSCRIPTION_ENABLED.store(enabled, Ordering::Relaxed);
    info!("Cloud transcription set to {enabled}");
    Ok(())
}

/// Applies the stored value at start-up (no persistence).
pub(crate) fn set_cloud_transcription_runtime(enabled: bool) {
    CLOUD_TRANSCRIPTION_ENABLED.store(enabled, Ordering::Relaxed);
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
        let first = tokio::spawn(with_recording_manager_out(
            Duration::from_secs(5),
            move |_| {
                wait.recv().unwrap();
            },
        ));
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
        // A panic in the work: the capture is marked incomplete and a warning is
        // sent before the manager goes back.
        let warnings = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        {
            let guard = RECORDING_MANAGER.lock().unwrap();
            let sink = warnings.clone();
            guard
                .as_ref()
                .unwrap()
                .set_warning_callback(move |message| {
                    sink.lock().unwrap().push(message.to_string());
                });
            assert!(!guard.as_ref().unwrap().get_state().capture_incomplete());
        }
        let panicked = with_recording_manager_out(Duration::from_secs(5), |_| {
            panic!("driver crashed");
        })
        .await;
        assert_eq!(
            panicked.unwrap_err(),
            "The device reconnection failed unexpectedly."
        );
        assert!(RECORDING_MANAGER.lock().unwrap().is_some());
        assert!(RECORDING_MANAGER
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .get_state()
            .capture_incomplete());
        let seen = warnings.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains("some audio may be missing"));
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
        // Stop does not wait forever on it.
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
        let reconnect = tokio::spawn(with_recording_manager_out(
            Duration::from_secs(5),
            move |_| {
                wait.recv().unwrap();
            },
        ));
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
        assert!(
            waiter.await.unwrap(),
            "the manager was restored before the waiter ran"
        );
    }

    #[tokio::test]
    async fn session_metadata_and_notes_stay_available_during_a_reconnection() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let _restore = install_manager();
        struct RestoreCapture(Option<ActiveRecording>, bool);
        impl Drop for RestoreCapture {
            fn drop(&mut self) {
                *ACTIVE_CAPTURE.lock().unwrap() = self.0.take();
                IS_RECORDING.store(self.1, Ordering::SeqCst);
            }
        }
        let _capture = RestoreCapture(
            ACTIVE_CAPTURE.lock().unwrap().take(),
            IS_RECORDING.swap(true, Ordering::SeqCst),
        );
        let runtime = crate::knowledge::KnowledgeState::default();
        let (id, capture) = {
            let mut guard = RECORDING_MANAGER.lock().unwrap();
            let manager = guard.as_mut().unwrap();
            let (startup, sink) = prepare_live_recording(&runtime, manager, true).unwrap();
            let capture = sink.capture.clone();
            capture.start_recording().unwrap();
            capture.pause_recording().unwrap();
            capture.report_warning("Synthetic backend warning");
            *ACTIVE_CAPTURE.lock().unwrap() = Some(ActiveRecording {
                sink,
                recording_id: Some("synthetic-recording".into()),
                folder: Some("synthetic-folder".into()),
                meeting_name: Some("Synthetic meeting".into()),
            });
            (startup.commit(), capture)
        };
        let (release, wait) = mpsc::channel::<()>();
        let reconnect = tokio::spawn(with_recording_manager_out(
            Duration::from_secs(5),
            move |_| {
                wait.recv().unwrap();
            },
        ));
        until(|| RECORDING_MANAGER.lock().unwrap().is_none()).await;
        let state = get_recording_state().await;
        assert_eq!(state["session_id"], id.as_str());
        assert_eq!(state["is_paused"], true);
        assert_eq!(active_bookmark_session().as_deref(), Some(id.as_str()));
        assert_eq!(calendar_recording_id(&id).unwrap(), "synthetic-recording");
        let (recording_id, folder, seconds) = bookmark_position(&id).unwrap();
        assert_eq!(recording_id, "synthetic-recording");
        assert_eq!(folder, "synthetic-folder");
        assert!((seconds - capture.get_active_recording_duration().unwrap()).abs() < 0.05);
        assert_eq!(
            get_recording_meeting_name().await.unwrap().as_deref(),
            Some("Synthetic meeting")
        );
        assert_eq!(
            capture_snapshot(&id).unwrap().health.warnings,
            ["Synthetic backend warning"]
        );
        assert!(bookmark_position("stale-session").is_err());
        release.send(()).unwrap();
        reconnect.await.unwrap().unwrap();
    }

    #[test]
    fn automatic_reconnect_backs_off_after_failed_checks() {
        let start = Instant::now();
        let mut retry = ReconnectBackoff::new(start);
        assert_eq!(retry.next_check - start, Duration::from_secs(12));
        for seconds in [24, 48, 60, 60] {
            let completed = retry.next_check + Duration::from_secs(2);
            retry.retry_later(completed);
            assert_eq!(retry.next_check - completed, Duration::from_secs(seconds));
        }
    }
}

#[cfg(test)]
mod startup_guard_tests {
    use super::start_allowed;

    #[tokio::test]
    async fn starting_recording_keeps_recovery_away_from_the_spool() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let spool = root.path().join(".audio-spool");
        std::fs::create_dir(&spool).unwrap();
        let chunk = spool.join("00000000000000000000.chunk");
        std::fs::write(&chunk, b"synthetic capture prefix").unwrap();
        let _starting = super::StartingState::claim().unwrap();
        let result = crate::audio::incremental_saver::recover_audio_in_folder(
            root.path().to_string_lossy().into_owned(),
        )
        .await;
        assert!(matches!(result, Err(message) if message.contains("Stop recording")));
        assert_eq!(std::fs::read(&chunk).unwrap(), b"synthetic capture prefix");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn recording_cannot_start_after_the_database_failed_to_open() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        assert!(start_allowed().is_ok());
        crate::database::setup::set_startup_failed_for_tests(true);
        let refused = start_allowed();
        crate::database::setup::set_startup_failed_for_tests(false);
        assert!(refused.unwrap_err().contains("meeting database"));
        assert!(start_allowed().is_ok());
    }
}
