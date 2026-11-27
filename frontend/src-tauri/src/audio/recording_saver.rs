use crate::api::TranscriptWord;
use anyhow::Result;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Mutex as AsyncMutex;

use super::audio_processing::create_meeting_folder;
use super::incremental_saver::IncrementalAudioSaver;
use super::outcome::RecordingOutcome;
use super::recording_state::RecordingState;
use super::transcription::TranscriptUpdate;

/// Structured transcript segment for JSON export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub audio_start_time: f64, // Seconds from recording start
    pub audio_end_time: f64,   // Seconds from recording start
    pub duration: f64,         // Segment duration in seconds
    pub display_time: String,  // Formatted time for display like "[02:15]"
    #[serde(default)]
    pub confidence: Option<f32>,
    pub sequence_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_timestamps: Option<Vec<TranscriptWord>>,
}

/// Meeting metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMetadata {
    #[serde(default)]
    pub recording_mode: super::recording_mode::RecordingMode,
    pub version: String,
    pub meeting_id: Option<String>,
    #[serde(default)]
    pub recording_id: Option<String>,
    pub meeting_name: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub duration_seconds: Option<f64>,
    #[serde(default)]
    pub duration_measured: bool,
    pub devices: DeviceInfo,
    pub audio_file: String,
    pub transcript_file: String,
    pub sample_rate: u32,
    pub status: String, // "recording", "completed", "error"
    // Which transcription engine + model ran for this meeting (no silent fallback).
    #[serde(default)]
    pub transcription_provider: Option<String>,
    #[serde(default)]
    pub transcription_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcription_source_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub microphone: Option<String>,
    pub system_audio: Option<String>,
}

#[derive(Default)]
pub struct RecordingSaveReport {
    pub outcome: RecordingOutcome,
    pub audio_path: Option<String>,
    raw_fully_encoded: bool,
    warnings: Vec<&'static str>,
}

#[derive(Default)]
struct TranscriptState {
    session_id: Option<String>,
    segments: Vec<Arc<TranscriptSegment>>,
}

/// Shared with the transcription producer. Disk writers take a shallow copy
/// of the rows and do their work after releasing the state lock.
#[derive(Default)]
pub(super) struct RecordingTranscripts {
    state: Mutex<TranscriptState>,
    snapshot_notify: tokio::sync::Notify,
    updates_since_flush: AtomicUsize,
    snapshot_written: AtomicBool,
}

impl RecordingTranscripts {
    fn lock(&self) -> MutexGuard<'_, TranscriptState> {
        // Retain the poison flag so failed-start cleanup cannot infer emptiness.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn snapshot(&self) -> Vec<Arc<TranscriptSegment>> {
        self.lock().segments.clone()
    }

    pub(super) fn segments(&self) -> Vec<TranscriptSegment> {
        self.snapshot()
            .iter()
            .map(|segment| (**segment).clone())
            .collect()
    }

    pub(super) fn record_update(&self, session_id: &str, update: &TranscriptUpdate) -> bool {
        if update.session_id.as_deref() != Some(session_id) || update.text.trim().is_empty() {
            return false;
        }
        let segment = TranscriptSegment {
            id: format!("seg_{}", update.sequence_id),
            text: update.text.clone(),
            speaker: match update.source.as_str() {
                "Me" | "Participants" => Some(update.source.clone()),
                _ => None,
            },
            audio_start_time: update.audio_start_time,
            audio_end_time: update.audio_end_time,
            duration: update.duration,
            display_time: update.timestamp.clone(),
            confidence: update.confidence,
            sequence_id: update.sequence_id,
            word_timestamps: update.word_timestamps.clone(),
        };
        let mut state = self.lock();
        if state.session_id.as_deref() != Some(session_id) {
            return false;
        }
        Self::upsert(&mut state.segments, segment);
        drop(state);
        self.notify_snapshot_worker();
        true
    }

    fn add_segment(&self, segment: TranscriptSegment) {
        Self::upsert(&mut self.lock().segments, segment);
        self.notify_snapshot_worker();
    }

    fn upsert(segments: &mut Vec<Arc<TranscriptSegment>>, segment: TranscriptSegment) {
        if let Some(existing) = segments
            .iter_mut()
            .find(|s| s.sequence_id == segment.sequence_id)
        {
            *existing = Arc::new(segment);
        } else {
            segments.push(Arc::new(segment));
        }
    }

    fn notify_snapshot_worker(&self) {
        let pending = self.updates_since_flush.fetch_add(1, Ordering::AcqRel) + 1;
        if (pending == 1 && !self.snapshot_written.load(Ordering::Acquire))
            || pending == TRANSCRIPT_SNAPSHOT_MAX_UPDATES
        {
            self.snapshot_notify.notify_one();
        }
    }
}

/// New recording saver using incremental saving strategy
pub struct RecordingSaver {
    mode: super::recording_mode::RecordingMode,
    incremental_saver: Option<Arc<AsyncMutex<IncrementalAudioSaver>>>,
    base_recordings_folder: Option<PathBuf>,
    meeting_folder: Option<PathBuf>,
    meeting_name: Option<String>,
    metadata: Option<MeetingMetadata>,
    transcripts: Arc<RecordingTranscripts>,
    snapshot_write_lock: Arc<Mutex<()>>,
    metadata_write_lock: Arc<Mutex<()>>,
    snapshot_task: Option<tokio::task::JoinHandle<()>>,
    capture_state: Option<Arc<RecordingState>>,
    transcription_provider: Option<String>,
    transcription_model: Option<String>,
    transcription_source_language: Option<String>,
    summary_language: Option<String>,
}

impl RecordingSaver {
    pub fn new() -> Self {
        Self {
            mode: super::recording_mode::RecordingMode::Live,
            incremental_saver: None,
            base_recordings_folder: None,
            meeting_folder: None,
            meeting_name: None,
            metadata: None,
            transcripts: Arc::default(),
            snapshot_write_lock: Arc::new(Mutex::new(())),
            metadata_write_lock: Arc::new(Mutex::new(())),
            snapshot_task: None,
            capture_state: None,
            transcription_provider: None,
            transcription_model: None,
            transcription_source_language: None,
            summary_language: None,
        }
    }

    /// Snapshot the selected capture mode before initializing meeting metadata.
    pub fn set_mode(&mut self, mode: super::recording_mode::RecordingMode) {
        self.mode = mode;
    }

    pub(crate) fn set_summary_language(&mut self, language: Option<&str>) {
        self.summary_language = language.and_then(|value| {
            crate::summary::metadata::normalise_supported_summary_language(value).ok()
        });
    }

    pub(crate) fn continue_audio_only(&mut self) {
        self.mode = super::recording_mode::RecordingMode::AudioOnly;
        self.set_transcription_info(None, None, None);
        if let Some(metadata) = &mut self.metadata {
            metadata.recording_mode = self.mode;
            metadata.transcription_provider = None;
            metadata.transcription_model = None;
            metadata.transcription_source_language = None;
        }
    }

    /// Provider captured for this recording, without loading saved credentials.
    pub fn transcription_provider(&self) -> Option<&str> {
        self.transcription_provider.as_deref()
    }

    /// Record which transcription engine + model this recording uses. Set before
    /// the meeting folder is initialized so it lands in metadata.json.
    pub fn set_transcription_info(
        &mut self,
        provider: Option<String>,
        model: Option<String>,
        source_language: Option<String>,
    ) {
        self.transcription_provider = provider;
        self.transcription_model = model;
        self.transcription_source_language = source_language;
    }

    /// Set the meeting name for this recording session
    pub fn set_meeting_name(&mut self, name: Option<String>) {
        self.meeting_name = name;
    }

    /// Set the base folder where meeting folders should be created.
    pub fn set_recordings_folder(&mut self, folder: PathBuf) {
        self.base_recordings_folder = Some(folder);
    }

    /// Set device information in metadata
    pub(super) fn mark_capture_started(
        &mut self,
        started: chrono::DateTime<chrono::Utc>,
        mic_name: Option<String>,
        sys_name: Option<String>,
    ) {
        let _writer = self
            .metadata_write_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let started = self.capture_started_at().unwrap_or(started);
        if let Some(ref mut metadata) = self.metadata {
            metadata.created_at = started.to_rfc3339();
            metadata.devices.microphone = mic_name;
            metadata.devices.system_audio = sys_name;

            // Record the device-open fallback until the first frame is available.
            if let Some(folder) = &self.meeting_folder {
                let metadata_clone = metadata.clone();
                if let Err(_e) = write_metadata(folder, &metadata_clone) {
                    warn!("Failed to update metadata with device info");
                }
            }
        }
    }

    fn capture_started_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.capture_state.as_ref()?.capture_started_at()
    }

    pub(super) fn bind_transcript_session(&self, session_id: &str) -> Arc<RecordingTranscripts> {
        self.transcripts.lock().session_id = Some(session_id.to_owned());
        self.transcripts.clone()
    }

    pub(super) fn close_transcript_input(&self) {
        self.transcripts.lock().session_id = None;
    }

    /// Add or update a transcript segment and schedule a recovery snapshot.
    pub fn add_transcript_segment(&self, segment: TranscriptSegment) {
        self.transcripts.add_segment(segment);
    }

    /// Legacy method for backward compatibility - converts text to basic segment
    pub fn add_transcript_chunk(&self, text: String) {
        let segment = TranscriptSegment {
            id: format!("seg_{}", chrono::Utc::now().timestamp_millis()),
            text,
            speaker: None,
            audio_start_time: 0.0,
            audio_end_time: 0.0,
            duration: 0.0,
            display_time: "[00:00]".to_string(),
            confidence: None,
            sequence_id: 0,
            word_timestamps: None,
        };
        self.add_transcript_segment(segment);
    }

    /// Start accumulation with optional incremental saving
    ///
    /// # Arguments
    /// * `auto_save` - If true, enables raw audio saving. If false, audio chunks are discarded.
    pub fn start_accumulation(
        &mut self,
        auto_save: bool,
        state: Arc<RecordingState>,
    ) -> Result<Option<super::transcription::queue::TranscriptionQueueSender>> {
        let name = self
            .meeting_name
            .clone()
            .unwrap_or_else(|| "Meeting".into());
        self.initialize_meeting_folder(&name, auto_save)?;
        self.capture_state = Some(state.clone());
        // Only a recording that writes audio can run out of space for it.
        self.start_snapshot_timer(TRANSCRIPT_SNAPSHOT_INTERVAL, Some((state, auto_save)));
        if !auto_save {
            return Ok(None);
        }
        let folder = self
            .meeting_folder
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Recording folder unavailable"))?;
        // The pipeline drains its bounded disk writer before finalization.
        // Raw chunks already provide recovery; no periodic encoder is needed.
        let (sender, _receiver, _metrics) =
            super::transcription::queue::recording_audio_queue(folder)?;
        Ok(Some(sender))
    }

    /// Handle periodic snapshots and recovery requests. The free-space check
    /// also runs once a minute for a recording that saves audio.
    fn start_snapshot_timer(
        &mut self,
        interval_duration: Duration,
        state: Option<(Arc<RecordingState>, bool)>,
    ) {
        let Some(folder) = self.meeting_folder.clone() else {
            return;
        };
        let transcripts = self.transcripts.clone();
        let write_lock = self.snapshot_write_lock.clone();
        let metadata_lock = self.metadata_write_lock.clone();
        self.snapshot_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval_duration);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            let mut disk_watch = super::disk_space::DiskWatch::new(Instant::now());
            let mut gap_saved = false;
            let mut start_saved = false;
            loop {
                tokio::select! {
                    _ = interval.tick() => {},
                    _ = transcripts.snapshot_notify.notified() => {},
                    _ = async {
                        match &state {
                            Some((state, _)) => state.wait_for_recovery_work().await,
                            None => std::future::pending().await,
                        }
                    } => {},
                }
                if !start_saved {
                    if let Some(started) = state
                        .as_ref()
                        .and_then(|(state, _)| state.capture_started_at())
                    {
                        let folder = folder.clone();
                        let lock = metadata_lock.clone();
                        start_saved = tokio::task::spawn_blocking(move || {
                            let _writer = lock.lock().unwrap_or_else(PoisonError::into_inner);
                            let mut marker = crate::database::meeting_files::read_marker(&folder)
                                .ok_or_else(|| {
                                anyhow::anyhow!("Recording metadata is unavailable")
                            })?;
                            if marker.get("status").and_then(serde_json::Value::as_str)
                                == Some("recording")
                            {
                                marker["created_at"] = started.to_rfc3339().into();
                                let json = serde_json::to_vec_pretty(&marker)?;
                                crate::library::atomic_write(&folder.join("metadata.json"), &json)
                                    .map_err(anyhow::Error::msg)?;
                            }
                            Ok::<_, anyhow::Error>(())
                        })
                        .await
                        .is_ok_and(|result| result.is_ok());
                    }
                }
                if let Some((state, true)) = &state {
                    if !gap_saved && state.capture_incomplete() {
                        let gap = folder.join(".audio-spool/.incomplete");
                        gap_saved = tokio::task::spawn_blocking(move || {
                            crate::library::atomic_write(&gap, b"capture incomplete")
                        })
                        .await
                        .is_ok_and(|result| result.is_ok());
                    }
                    if disk_watch.due(Instant::now()) {
                        let free = super::disk_space::free_space_bounded(&folder).await;
                        if let Some(free) = disk_watch.evaluate(free) {
                            state.report_disk_space_low(free, &folder);
                        }
                    }
                }
                let count = transcripts.updates_since_flush.swap(0, Ordering::AcqRel);
                if count == 0 {
                    continue;
                }
                if save_snapshot(folder.clone(), transcripts.clone(), write_lock.clone())
                    .await
                    .is_ok()
                {
                    transcripts.snapshot_written.store(true, Ordering::Release);
                } else {
                    // Keep the snapshot dirty even if speech stops before the next tick.
                    transcripts
                        .updates_since_flush
                        .fetch_add(1, Ordering::AcqRel);
                    warn!("Transcript recovery snapshot could not be saved; retrying");
                }
            }
        }));
    }

    pub async fn failed_start(&mut self, capture_drained: bool) {
        self.close_transcript_input();
        if let Some(mut task) = self.snapshot_task.take() {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
        }
        let Some(folder) = &self.meeting_folder else {
            return;
        };
        let owned = std::fs::symlink_metadata(folder)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            && crate::database::meeting_files::read_marker(folder).is_some_and(|marker| {
                self.recording_id().is_some_and(|id| {
                    crate::database::meeting_files::recording_id(&marker) == Some(id)
                        && marker
                            .get("meeting_id")
                            .is_none_or(serde_json::Value::is_null)
                })
            });
        if !owned {
            return;
        }

        let definitely_empty = self
            .transcripts
            .state
            .lock()
            .is_ok_and(|state| state.segments.is_empty());
        if capture_drained && definitely_empty {
            const CAPTURE_DIRS: [&str; 2] = [".audio-spool", ".checkpoints"];
            let only_start_files = std::fs::read_dir(folder).is_ok_and(|mut entries| {
                entries.all(|entry| {
                    let Ok(entry) = entry else {
                        return false;
                    };
                    entry.file_name() == "metadata.json"
                        || (CAPTURE_DIRS.iter().any(|name| entry.file_name() == *name)
                            && entry.file_type().is_ok_and(|kind| kind.is_dir()))
                })
            });
            // Non-recursive removal also protects a late capture write that
            // lands after enumeration.
            if only_start_files
                && CAPTURE_DIRS
                    .iter()
                    .all(|name| match std::fs::remove_dir(folder.join(name)) {
                        Ok(()) => true,
                        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                    })
                && std::fs::remove_file(folder.join("metadata.json")).is_ok()
                && std::fs::remove_dir(folder).is_ok()
            {
                self.meeting_folder = None;
                self.metadata = None;
                self.incremental_saver = None;
                return;
            }
        }
        // Never delete possible capture from a partially opened device. Mark
        // the folder interrupted so recovery can distinguish it from a live session.
        let started = self.capture_started_at();
        if let (Some(folder), Some(metadata)) = (&self.meeting_folder, &mut self.metadata) {
            if let Some(started) = started {
                metadata.created_at = started.to_rfc3339();
            }
            metadata.status = "error".into();
            let metadata = metadata.clone();
            let folder = folder.clone();
            let _ = save_metadata(
                folder.clone(),
                metadata,
                false,
                self.metadata_write_lock.clone(),
            )
            .await;
            let _ = self.write_transcripts_json(&folder).await;
        }
    }

    /// Initialize meeting folder structure and metadata
    ///
    /// # Arguments
    /// * `meeting_name` - Name of the meeting
    /// * `save_audio` - Whether to enable final audio encoding
    fn initialize_meeting_folder(&mut self, meeting_name: &str, save_audio: bool) -> Result<()> {
        let base_folder = self
            .base_recordings_folder
            .clone()
            .unwrap_or_else(super::recording_preferences::get_default_recordings_folder);

        // Create a new folder owned by this recording
        let meeting_folder = create_meeting_folder(&base_folder, meeting_name)?;
        self.meeting_folder = Some(meeting_folder.clone());

        // Initialize final audio encoding when auto_save is true
        if save_audio {
            let incremental_saver = IncrementalAudioSaver::new(meeting_folder.clone());
            self.incremental_saver = Some(Arc::new(AsyncMutex::new(incremental_saver)));
            info!("✅ Incremental audio saver initialized for meeting");
        } else {
            info!("⚠️  Skipped incremental audio saver (auto-save disabled)");
        }

        // Create initial metadata
        let metadata = MeetingMetadata {
            recording_mode: self.mode,
            version: "1.0".to_string(),
            meeting_id: None, // Will be set by backend
            recording_id: Some(format!("meeting-{}", uuid::Uuid::new_v4())),
            meeting_name: Some(meeting_name.to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            duration_seconds: None,
            duration_measured: false,
            devices: DeviceInfo {
                microphone: None, // Could be enhanced to store actual device names
                system_audio: None,
            },
            audio_file: if save_audio {
                "audio.mp4".to_string()
            } else {
                "".to_string()
            },
            transcript_file: "transcripts.json".to_string(),
            sample_rate: 48000,
            status: "recording".to_string(),
            transcription_provider: self.transcription_provider.clone(),
            transcription_model: self.transcription_model.clone(),
            transcription_source_language: self.transcription_source_language.clone(),
            summary_language: self.summary_language.clone(),
        };

        // Write initial metadata.json
        self.metadata = Some(metadata.clone());
        write_metadata(&meeting_folder, &metadata)?;

        self.meeting_folder = Some(meeting_folder);
        self.metadata = Some(metadata);

        Ok(())
    }

    /// Write transcripts.json to disk (atomic write with temp file and validation)
    async fn write_transcripts_json(&self, folder: &Path) -> Result<()> {
        save_snapshot(
            folder.to_path_buf(),
            self.transcripts.clone(),
            self.snapshot_write_lock.clone(),
        )
        .await
    }

    /// Stop and save using incremental saving approach
    ///
    /// # Arguments
    /// * `app` - Tauri app handle for emitting events
    /// * `recording_duration` - Actual recording duration in seconds (from RecordingState)
    pub async fn stop_and_save<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        recording_duration: Option<f64>,
        outcome: RecordingOutcome,
    ) -> Result<RecordingSaveReport, String> {
        let report = self.finalize_files(recording_duration, outcome).await?;
        if report.audio_path.is_some() {
            let _ = app.emit("recording-saved", serde_json::json!({
                "audio_file": report.audio_path,
                "transcript_file": self.meeting_folder.as_ref().map(|f| f.join("transcripts.json")),
                "meeting_name": self.meeting_name,
                "meeting_folder": self.meeting_folder,
            }));
        }
        if !report.warnings.is_empty() {
            let _ = app.emit("recording-warning", report.warnings.join(" "));
        }
        Ok(report)
    }

    async fn finalize_files(
        &mut self,
        recording_duration: Option<f64>,
        outcome: RecordingOutcome,
    ) -> Result<RecordingSaveReport, String> {
        info!("Stopping recording saver");
        self.close_transcript_input();
        if let Some(mut task) = self.snapshot_task.take() {
            task.abort();
            let _ = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
        }
        let mut report = RecordingSaveReport {
            outcome,
            ..Default::default()
        };
        let capture_started_at = self.capture_started_at();
        // Attempt every artifact independently, then report all failures.
        if let Some(folder) = &self.meeting_folder {
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), self.write_transcripts_json(folder))
                    .await,
                Ok(Ok(()))
            ) {
                report.outcome.recording_files_incomplete = true;
                report
                    .warnings
                    .push("The final transcript snapshot could not be saved.");
            }
            if let Some(metadata) = &mut self.metadata {
                if let Some(started) = capture_started_at {
                    metadata.created_at = started.to_rfc3339();
                }
                metadata.duration_seconds =
                    super::provenance::measured_duration(recording_duration);
                metadata.duration_measured = metadata.duration_seconds.is_some();
                metadata.completed_at = Some(chrono::Utc::now().to_rfc3339());
                metadata.status = "error".into();
            }
            // Persist the stopped duration before potentially lengthy encoding.
            // Reuse the existing error status until final audio is published.
            if let Some(metadata) = &self.metadata {
                if save_metadata(
                    folder.clone(),
                    metadata.clone(),
                    report.outcome.capture_incomplete,
                    self.metadata_write_lock.clone(),
                )
                .await
                .is_err()
                {
                    // The final write below may repair this transient failure.
                    // Only its result determines the persistent file warning.
                    debug!("Stopped metadata write failed; final metadata will retry");
                }
            }
        }

        if let Some(saver_arc) = &self.incremental_saver {
            let saver_arc = saver_arc.clone();
            let finalized = tokio::task::spawn_blocking(move || {
                let mut saver = saver_arc.blocking_lock();
                let result = tokio::runtime::Handle::current().block_on(saver.finalize());
                (result, saver.capture_incomplete, saver.raw_fully_encoded)
            })
            .await;
            match finalized {
                Ok((Ok(path), gaps, raw_fully_encoded)) => {
                    report.outcome.capture_incomplete |= gaps;
                    report.audio_path = Some(path.to_string_lossy().into_owned());
                    report.raw_fully_encoded = raw_fully_encoded;
                }
                Ok((Err(error), _, _)) => {
                    report.outcome.audio_save_failed = true;
                    report
                        .warnings
                        .push(final_audio_failure_message(&error.to_string()));
                }
                Err(_) => {
                    report.outcome.audio_save_failed = true;
                    report.warnings.push("The audio finalization worker failed. Keep the meeting recovery files and retry recovery.");
                }
            }
        }

        if let (Some(folder), Some(mut metadata)) = (&self.meeting_folder, self.metadata.clone()) {
            metadata.status = if report.outcome.audio_save_failed {
                "error"
            } else {
                "completed"
            }
            .into();
            if save_metadata(
                folder.clone(),
                metadata.clone(),
                false,
                self.metadata_write_lock.clone(),
            )
            .await
            .is_err()
            {
                report.outcome.recording_files_incomplete = true;
                report
                    .warnings
                    .push("The recording details could not be saved.");
            }
            self.metadata = Some(metadata);
        }
        // Persist gaps before discarding redundant originals. Missing samples
        // cannot be reconstructed by retaining an otherwise encoded raw spool.
        if let Some(folder) = &self.meeting_folder {
            let outcome = report.outcome.clone();
            let folder = folder.clone();
            let clean_spool = report.raw_fully_encoded && report.audio_path.is_some();
            let outcome_saved = tokio::task::spawn_blocking(move || {
                outcome.write(&folder)?;
                if clean_spool {
                    let _ = std::fs::remove_dir_all(folder.join(".audio-spool"));
                }
                Ok::<_, String>(())
            })
            .await
            .is_ok_and(|result| result.is_ok());
            if !outcome_saved {
                report.outcome.recording_files_incomplete = true;
                report
                    .warnings
                    .push("The recording warning status could not be saved.");
            }
        }
        Ok(report)
    }

    /// Get the meeting folder path (for passing to backend)
    pub fn get_meeting_folder(&self) -> Option<&PathBuf> {
        self.meeting_folder.as_ref()
    }

    pub(crate) fn recording_id(&self) -> Option<&str> {
        self.metadata.as_ref()?.recording_id.as_deref()
    }

    pub(super) fn started_at(&self) -> Option<&str> {
        Some(&self.metadata.as_ref()?.created_at)
    }

    /// Get accumulated transcript segments (for reload sync)
    pub fn get_transcript_segments(&self) -> Vec<TranscriptSegment> {
        self.transcripts.segments()
    }

    /// Get meeting name (for reload sync)
    pub fn get_meeting_name(&self) -> Option<String> {
        self.meeting_name.clone()
    }
}

const TRANSCRIPT_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(15);
const TRANSCRIPT_SNAPSHOT_MAX_UPDATES: usize = 32;

fn final_audio_failure_message(error: &str) -> &'static str {
    if error.contains("insufficient disk space") {
        "Audio could not be saved because disk space ran out. Keep the meeting recovery files."
    } else if error.contains("output access denied") {
        "Audio could not be saved because file access was denied. Check folder permissions and keep the meeting recovery files."
    } else if error.contains("timed out") {
        "Audio encoding exceeded its time limit. Keep the meeting recovery files and retry recovery."
    } else if error.contains("FFmpeg not found") || error.contains("required encoder unavailable") {
        "The audio encoder is unavailable. Repair the app installation and keep the meeting recovery files."
    } else {
        "Audio could not be finalized. Check disk space and keep the meeting recovery files."
    }
}

fn write_transcript_snapshot(folder: &Path, segments: &[Arc<TranscriptSegment>]) -> Result<()> {
    use std::io::{BufWriter, Write};
    #[derive(Serialize)]
    struct Snapshot<'a> {
        version: &'static str,
        segments: Vec<&'a TranscriptSegment>,
        total_segments: usize,
        last_updated: String,
    }
    let mut staged = tempfile::NamedTempFile::new_in(folder)?;
    let snapshot = Snapshot {
        version: "1.0",
        segments: segments.iter().map(Arc::as_ref).collect(),
        total_segments: segments.len(),
        last_updated: chrono::Utc::now().to_rfc3339(),
    };
    {
        let mut writer = BufWriter::new(staged.as_file_mut());
        serde_json::to_writer(&mut writer, &snapshot)?;
        writer.flush()?;
    }
    staged.as_file().sync_all()?;
    staged.persist(folder.join("transcripts.json"))?;
    Ok(())
}

impl Drop for RecordingSaver {
    fn drop(&mut self) {
        self.close_transcript_input();
        if let Some(task) = self.snapshot_task.take() {
            task.abort();
        }
    }
}

fn write_metadata(folder: &Path, metadata: &MeetingMetadata) -> Result<()> {
    let json = serde_json::to_vec_pretty(metadata)?;
    crate::library::atomic_write(&folder.join("metadata.json"), &json).map_err(anyhow::Error::msg)
}

async fn save_metadata(
    folder: PathBuf,
    metadata: MeetingMetadata,
    capture_gap: bool,
    write_lock: Arc<Mutex<()>>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let _writer = write_lock.lock().unwrap_or_else(PoisonError::into_inner);
        let result = write_metadata(&folder, &metadata);
        if capture_gap {
            let _ = crate::library::atomic_write(
                &folder.join(".audio-spool/.incomplete"),
                b"capture incomplete",
            );
        }
        result
    })
    .await?
}

async fn save_snapshot(
    folder: PathBuf,
    transcripts: Arc<RecordingTranscripts>,
    write_lock: Arc<Mutex<()>>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        // A failed writer only owned an unpublished temporary file.
        let _writer = write_lock.lock().unwrap_or_else(PoisonError::into_inner);
        let snapshot = transcripts.snapshot();
        write_transcript_snapshot(&folder, &snapshot)
    })
    .await?
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[cfg(any(windows, unix))]
    #[tokio::test]
    async fn selected_recording_target_survives_start_save_and_recovery_checks() {
        use crate::audio::{interrupted, recording_preferences, recording_state::RecordingState};
        use crate::database::meeting_files;
        let base = tempfile::tempdir().unwrap();
        let base_path = meeting_files::plain_path(&base.path().canonicalize().unwrap());
        let target = base_path.join("recordings");
        let alias = base_path.join("selected-link");
        std::fs::create_dir(&target).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(&alias)
                .arg(&target)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success());
        }

        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(alias.clone());
        let error = saver
            .start_accumulation(false, RecordingState::new())
            .err()
            .unwrap();
        assert!(error.to_string().contains("Settings → Recording"));
        assert!(saver.get_meeting_folder().is_none());
        assert!(std::fs::read_dir(&target).unwrap().next().is_none());

        let resolved = recording_preferences::validate_save_folder(
            &alias,
            &recording_preferences::ProtectedFolders::default(),
        )
        .unwrap();
        assert_eq!(resolved, target);
        let roots = [resolved.clone()];
        saver.set_recordings_folder(resolved);
        saver
            .start_accumulation(false, RecordingState::new())
            .unwrap();
        let folder = saver.get_meeting_folder().unwrap().clone();
        let stored =
            meeting_files::authorize_new_meeting_folder(&folder.to_string_lossy(), &roots).unwrap();
        assert_eq!(std::path::Path::new(&stored), folder.as_path());
        saver.add_transcript_chunk("Synthetic draft".into());
        saver.write_transcripts_json(&folder).await.unwrap();
        let scan = interrupted::scan(&interrupted::ScanInput {
            roots: &roots,
            kept: &[],
            registered: &Default::default(),
            registered_ids: &Default::default(),
            deleted: &Default::default(),
            active: interrupted::ActiveFolder::None,
            limits: interrupted::Limits::default(),
        });
        assert_eq!(scan.recordings.len(), 1);
        assert_eq!(
            std::path::Path::new(&scan.recordings[0].folder),
            folder.as_path()
        );
        assert_eq!(scan.recordings[0].transcript_lines, 1);
        let linked_folder = alias.join(folder.file_name().unwrap());
        assert!(meeting_files::resolve_recording_folder(&linked_folder, &roots).is_err());
        #[cfg(windows)]
        std::fs::remove_dir(&alias).unwrap();
    }

    fn poison<T>(lock: &Mutex<T>) {
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock.lock().unwrap();
            panic!("Synthetic lock poison");
        }));
        assert!(panic.is_err());
        assert!(lock.is_poisoned());
    }

    #[tokio::test]
    async fn poisoned_transcript_locks_preserve_rows_for_history_and_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        saver
            .initialize_meeting_folder("Synthetic meeting", false)
            .unwrap();
        let folder = saver.meeting_folder.clone().unwrap();
        saver.add_transcript_chunk("Before poison".into());
        poison(&saver.transcripts.state);
        poison(&saver.snapshot_write_lock);
        saver.add_transcript_segment(TranscriptSegment {
            id: "seg_1".into(),
            text: "After poison".into(),
            sequence_id: 1,
            ..saver.get_transcript_segments()[0].clone()
        });
        let rows = saver.get_transcript_segments();
        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["Before poison", "After poison"]
        );
        saver.write_transcripts_json(&folder).await.unwrap();
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("transcripts.json")).unwrap())
                .unwrap();
        assert_eq!(snapshot["total_segments"], 2);
        assert_eq!(snapshot["segments"][0]["text"], "Before poison");
        assert_eq!(snapshot["segments"][1]["text"], "After poison");
    }

    #[tokio::test]
    async fn first_source_activation_updates_the_start_for_stop_and_interrupted_recovery() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        saver.initialize_meeting_folder("Overnight", true).unwrap();
        let started = "2025-08-05T00:00:03Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        saver.mark_capture_started(started, Some("Test input".into()), None);
        let folder = saver.get_meeting_folder().unwrap();
        let marker = crate::database::meeting_files::read_marker(folder).unwrap();
        assert_eq!(saver.started_at(), marker["created_at"].as_str());
        assert_eq!(
            super::super::interrupted::recovered_provenance(folder, &marker, None).started_at,
            Some(started)
        );
    }

    #[tokio::test]
    async fn first_capture_sample_replaces_device_open_time_for_recovery_and_final_save() {
        use super::super::recording_state::{AudioChunk, DeviceType};

        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let state = RecordingState::new();
        let (sender, _audio) = tokio::sync::mpsc::channel(1);
        state.set_audio_sender(sender);
        saver.start_accumulation(false, state.clone()).unwrap();
        state.start_recording().unwrap();
        let fallback = chrono::Utc::now() - chrono::Duration::seconds(1);
        saver.mark_capture_started(fallback, Some("Test input".into()), None);
        let folder = saver.get_meeting_folder().unwrap().clone();

        let stamp = state.admit_capture(Instant::now(), 480, 48_000).unwrap();
        assert_eq!(stamp.seconds, 0.0);
        let started = state.capture_started_at().unwrap().to_rfc3339();
        state
            .send_audio_chunk(AudioChunk {
                data: vec![0.1; 480],
                sample_rate: 48_000,
                timestamp: stamp.seconds,
                chunk_id: 0,
                device_type: DeviceType::Microphone,
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if crate::database::meeting_files::read_marker(&folder)
                    .is_some_and(|marker| marker["created_at"].as_str() == Some(started.as_str()))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        state.stop_recording();
        saver
            .finalize_files(Some(0.5), RecordingOutcome::default())
            .await
            .unwrap();
        let marker = crate::database::meeting_files::read_marker(&folder).unwrap();
        assert_eq!(marker["created_at"].as_str(), Some(started.as_str()));
        assert_eq!(marker["devices"]["microphone"], "Test input");
        assert_eq!(saver.started_at(), Some(started.as_str()));
    }

    #[tokio::test]
    async fn failed_start_keeps_its_folder_when_transcript_emptiness_is_uncertain() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        saver
            .initialize_meeting_folder("Synthetic meeting", false)
            .unwrap();
        let folder = saver.meeting_folder.clone().unwrap();
        poison(&saver.transcripts.state);
        saver.failed_start(true).await;
        assert_eq!(saver.get_meeting_folder(), Some(&folder));
        let marker = crate::database::meeting_files::read_marker(&folder).unwrap();
        assert_eq!(marker["status"], "error");
    }

    #[tokio::test]
    async fn startup_fallback_keeps_the_original_capture_and_audio_only_metadata() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        saver.set_transcription_info(Some("whisper".into()), Some("synthetic-model".into()), None);
        let state = super::super::recording_state::RecordingState::new();
        let sender = saver.start_accumulation(true, state.clone()).unwrap();
        let folder = saver.meeting_folder.clone().unwrap();
        let id = saver.recording_id().unwrap().to_owned();
        drop(sender);
        let spool = folder.join(".audio-spool");
        std::fs::create_dir_all(&spool).unwrap();
        let capture = spool.join("synthetic-capture");
        std::fs::write(&capture, b"captured prefix").unwrap();

        saver.continue_audio_only();
        state.mark_capture_incomplete();
        let gap = spool.join(".incomplete");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !gap.is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        saver.failed_start(true).await;

        let marker = crate::database::meeting_files::read_marker(&folder).unwrap();
        assert_eq!(marker["recording_mode"], "audio_only");
        assert_eq!(marker["recording_id"], id);
        assert!(marker["transcription_provider"].is_null());
        assert_eq!(std::fs::read(capture).unwrap(), b"captured prefix");
    }

    #[tokio::test]
    async fn snapshot_and_metadata_failures_do_not_block_available_audio() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let sender = saver
            .start_accumulation(true, super::super::recording_state::RecordingState::new())
            .unwrap()
            .unwrap();
        let folder = saver.meeting_folder.clone().unwrap();
        sender
            .send(super::super::recording_state::AudioChunk {
                data: vec![0.05; 4800],
                sample_rate: 48000,
                timestamp: 0.0,
                chunk_id: 0,
                device_type: super::super::recording_state::DeviceType::System,
            })
            .await
            .unwrap();
        drop(sender);
        // Deterministic publication failures without changing process permissions.
        std::fs::create_dir(folder.join("transcripts.json")).unwrap();
        std::fs::remove_file(folder.join("metadata.json")).unwrap();
        std::fs::create_dir(folder.join("metadata.json")).unwrap();
        assert!(!folder.join(".checkpoints").exists());
        let report = saver
            .finalize_files(
                Some(0.1),
                RecordingOutcome {
                    capture_incomplete: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(report.audio_path.is_some());
        assert!(!report.outcome.audio_save_failed);
        assert!(report.outcome.capture_incomplete);
        assert!(report.outcome.recording_files_incomplete);
        assert_eq!(report.warnings.len(), 2);
        assert!(
            !folder.join(".audio-spool").exists(),
            "transcript/metadata failures cannot benefit from redundant raw audio"
        );
        super::super::incremental_saver::validate_recoverable_temp_audio_file(
            &folder.join("audio.mp4"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn transient_stopped_metadata_failure_is_repaired_by_final_write() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let sender = saver
            .start_accumulation(true, super::super::recording_state::RecordingState::new())
            .unwrap()
            .unwrap();
        sender
            .send(super::super::recording_state::AudioChunk {
                data: vec![0.05; 4800],
                sample_rate: 48000,
                timestamp: 0.0,
                chunk_id: 0,
                device_type: super::super::recording_state::DeviceType::System,
            })
            .await
            .unwrap();
        drop(sender);
        let folder = saver.meeting_folder.clone().unwrap();
        std::fs::remove_file(folder.join("metadata.json")).unwrap();
        std::fs::create_dir(folder.join("metadata.json")).unwrap();
        let encoder = saver.incremental_saver.as_ref().unwrap().clone();
        let guard = encoder.lock().await;
        let task = tokio::spawn(async move {
            saver
                .finalize_files(
                    Some(0.1),
                    RecordingOutcome {
                        capture_incomplete: true,
                        ..Default::default()
                    },
                )
                .await
        });
        // The gap marker is written after the stopped metadata attempt. Block
        // encoding until that failed attempt has happened, then repair the path.
        tokio::time::timeout(Duration::from_secs(2), async {
            while !folder.join(".audio-spool/.incomplete").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        std::fs::remove_dir(folder.join("metadata.json")).unwrap();
        drop(guard);
        let report = task.await.unwrap().unwrap();
        assert!(report.audio_path.is_some() && report.raw_fully_encoded);
        assert!(!report.outcome.recording_files_incomplete);
        assert!(report.warnings.is_empty());
        assert!(!folder.join(".audio-spool").exists());
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(metadata["status"], "completed");
        let outcome = super::super::outcome::RecordingOutcome::read(&folder)
            .unwrap()
            .unwrap();
        assert!(outcome.capture_incomplete);
        assert!(!outcome.recording_files_incomplete);
    }

    #[tokio::test]
    async fn final_file_failures_retain_spool_only_when_outcome_cannot_be_saved() {
        for failed_file in ["metadata.json", "recording-outcome.json"] {
            let root = tempfile::tempdir().unwrap();
            let mut saver = RecordingSaver::new();
            saver.set_recordings_folder(root.path().to_path_buf());
            let sender = saver
                .start_accumulation(true, super::super::recording_state::RecordingState::new())
                .unwrap()
                .unwrap();
            sender
                .send(super::super::recording_state::AudioChunk {
                    data: vec![0.05; 4800],
                    sample_rate: 48000,
                    timestamp: 0.0,
                    chunk_id: 0,
                    device_type: super::super::recording_state::DeviceType::System,
                })
                .await
                .unwrap();
            drop(sender);
            let folder = saver.meeting_folder.clone().unwrap();
            let failed_path = folder.join(failed_file);
            if failed_path.is_file() {
                std::fs::remove_file(&failed_path).unwrap();
            }
            std::fs::create_dir(failed_path).unwrap();
            let report = saver
                .finalize_files(Some(0.1), RecordingOutcome::default())
                .await
                .unwrap();
            assert!(report.audio_path.is_some() && report.raw_fully_encoded);
            assert!(!report.outcome.audio_save_failed);
            assert!(report.outcome.recording_files_incomplete);
            assert_eq!(
                report.warnings,
                vec![if failed_file == "metadata.json" {
                    "The recording details could not be saved."
                } else {
                    "The recording warning status could not be saved."
                }]
            );
            assert_eq!(
                folder.join(".audio-spool").exists(),
                failed_file == "recording-outcome.json"
            );
            if failed_file == "metadata.json" {
                assert!(
                    super::super::outcome::RecordingOutcome::read(&folder)
                        .unwrap()
                        .unwrap()
                        .recording_files_incomplete
                );
            }
        }
    }

    #[tokio::test]
    async fn successfully_encoded_gapped_capture_releases_redundant_spool() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let sender = saver
            .start_accumulation(true, super::super::recording_state::RecordingState::new())
            .unwrap()
            .unwrap();
        sender
            .send(super::super::recording_state::AudioChunk {
                data: vec![0.05; 4800],
                sample_rate: 48000,
                timestamp: 1.0,
                chunk_id: 1,
                device_type: super::super::recording_state::DeviceType::System,
            })
            .await
            .unwrap();
        drop(sender);
        let report = saver
            .finalize_files(
                Some(1.1),
                RecordingOutcome {
                    capture_incomplete: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let folder = saver.meeting_folder.as_ref().unwrap();
        assert!(report.audio_path.is_some() && !report.outcome.audio_save_failed);
        assert!(!folder.join(".audio-spool").exists());
        assert!(
            super::super::outcome::RecordingOutcome::read(folder)
                .unwrap()
                .unwrap()
                .capture_incomplete
        );
    }

    #[tokio::test]
    async fn metadata_is_stopped_before_audio_encoding_and_after_failure() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let sender = saver
            .start_accumulation(true, super::super::recording_state::RecordingState::new())
            .unwrap();
        drop(sender); // Empty capture makes encoding fail.
        let folder = saver.meeting_folder.clone().unwrap();
        // Hold the encoder lock so we can inspect metadata while finalization waits.
        let encoder = saver.incremental_saver.as_ref().unwrap().clone();
        let guard = encoder.lock().await;
        let task = tokio::spawn(async move {
            saver
                .finalize_files(Some(1.25), RecordingOutcome::default())
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let metadata: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(folder.join("metadata.json")).unwrap())
                        .unwrap();
                if metadata["status"] == "error" {
                    assert_eq!(metadata["duration_seconds"], 1.25);
                    assert!(metadata["completed_at"].is_string());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!task.is_finished());
        drop(guard);
        let report = task.await.unwrap().unwrap();
        assert!(report.outcome.audio_save_failed);
        assert!(!report.outcome.recording_files_incomplete);
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(metadata["status"], "error");
    }

    #[tokio::test]
    async fn temporary_capture_tail_is_encoded_or_reported_without_retaining_spool() {
        use super::super::recording_state::{AudioChunk, DeviceType};
        for (torn_temp, damaged_middle) in [(false, false), (true, false), (false, true)] {
            let root = tempfile::tempdir().unwrap();
            let mut saver = RecordingSaver::new();
            saver.set_recordings_folder(root.path().to_path_buf());
            let sender = saver
                .start_accumulation(true, super::super::recording_state::RecordingState::new())
                .unwrap()
                .unwrap();
            for id in 0..3 {
                sender
                    .send(AudioChunk {
                        data: vec![0.05; 4800],
                        sample_rate: 48000,
                        timestamp: id as f64 / 10.0,
                        chunk_id: id,
                        device_type: DeviceType::System,
                    })
                    .await
                    .unwrap();
            }
            drop(sender);
            let folder = saver.meeting_folder.clone().unwrap();
            let last = folder.join(".audio-spool/00000000000000000002.chunk");
            let temp = last.with_extension("tmp");
            std::fs::rename(last, &temp).unwrap();
            if torn_temp {
                std::fs::write(&temp, b"torn write").unwrap();
            }
            if damaged_middle {
                std::fs::write(
                    folder.join(".audio-spool/00000000000000000001.chunk"),
                    b"damaged",
                )
                .unwrap();
            }
            let report = saver
                .finalize_files(Some(0.3), RecordingOutcome::default())
                .await
                .unwrap();
            assert!(!report.outcome.audio_save_failed);
            assert!(!report.outcome.recording_files_incomplete);
            assert_eq!(
                report.outcome.capture_incomplete,
                torn_temp || damaged_middle
            );
            assert_eq!(report.raw_fully_encoded, !damaged_middle);
            assert_eq!(folder.join(".audio-spool").exists(), damaged_middle);
            let outcome = super::super::outcome::RecordingOutcome::read(&folder)
                .unwrap()
                .unwrap();
            assert_eq!(outcome.capture_incomplete, torn_temp || damaged_middle);
            let mut command =
                std::process::Command::new(super::super::ffmpeg::find_ffmpeg_path().unwrap());
            command
                .args(["-v", "error", "-i"])
                .arg(folder.join("audio.mp4"))
                .args(["-f", "f32le", "-ac", "1", "pipe:1"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            let decoded = command.output().unwrap();
            assert!(decoded.status.success());
            let expected = if torn_temp || damaged_middle {
                9600
            } else {
                14400
            };
            let samples = decoded.stdout.len() / 4;
            assert!(
                samples >= expected && samples - expected < 1024,
                "Unexpected decoded sample count: {samples}"
            );
        }
    }

    #[tokio::test]
    async fn all_artifact_failures_are_collected_together() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let sender = saver
            .start_accumulation(true, super::super::recording_state::RecordingState::new())
            .unwrap();
        drop(sender);
        let folder = saver.meeting_folder.clone().unwrap();
        std::fs::create_dir(folder.join("transcripts.json")).unwrap();
        std::fs::remove_file(folder.join("metadata.json")).unwrap();
        std::fs::create_dir(folder.join("metadata.json")).unwrap();
        let report = saver
            .finalize_files(Some(1.0), RecordingOutcome::default())
            .await
            .unwrap();
        assert!(report.outcome.audio_save_failed && report.outcome.recording_files_incomplete);
        assert_eq!(report.warnings.len(), 3);
        assert!(folder.join(".audio-spool").exists());
    }

    #[tokio::test]
    async fn silence_still_flushes_pending_transcript_updates() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        saver
            .initialize_meeting_folder("Synthetic meeting", false)
            .unwrap();
        saver.start_snapshot_timer(Duration::from_millis(20), None);
        saver.add_transcript_chunk("First synthetic sentence".into());
        saver.add_transcript_segment(TranscriptSegment {
            id: "second".into(),
            text: "Last sentence before silence".into(),
            speaker: None,
            audio_start_time: 1.0,
            audio_end_time: 2.0,
            duration: 1.0,
            display_time: "00:01".into(),
            confidence: None,
            sequence_id: 1,
            word_timestamps: None,
        });
        let path = saver
            .meeting_folder
            .as_ref()
            .unwrap()
            .join("transcripts.json");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let json: Option<serde_json::Value> = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                if json.is_some_and(|json| json["total_segments"] == 2) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        saver.failed_start(true).await;
    }

    #[tokio::test]
    async fn failed_empty_start_removes_its_folder_and_stops_workers() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let sender = saver
            .start_accumulation(true, super::super::recording_state::RecordingState::new())
            .unwrap();
        let folder = saver.meeting_folder.clone().unwrap();
        let id = saver.recording_id().unwrap().to_owned();
        let pool = stage_synthetic_calendar(&id).await;
        drop(sender);
        saver.failed_start(true).await;
        crate::library::recording_calendar::discard_failed_start_draft(
            &pool,
            Some(&id),
            saver.get_meeting_folder().is_none(),
        )
        .await
        .unwrap();
        assert!(!folder.exists());
        assert!(saver.snapshot_task.is_none());
        assert_eq!(draft_ids(&pool).await, ["another-start"]);
    }

    #[tokio::test]
    async fn failed_start_keeps_its_calendar_with_the_audio_captured_so_far() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        let sender = saver
            .start_accumulation(true, super::super::recording_state::RecordingState::new())
            .unwrap();
        let folder = saver.meeting_folder.clone().unwrap();
        let id = saver.recording_id().unwrap().to_owned();
        let pool = stage_synthetic_calendar(&id).await;
        drop(sender);
        let spool = folder.join(".audio-spool");
        std::fs::create_dir_all(&spool).unwrap();
        let prefix = spool.join("captured");
        std::fs::write(&prefix, b"first seconds").unwrap();
        saver.failed_start(true).await;
        crate::library::recording_calendar::discard_failed_start_draft(
            &pool,
            Some(&id),
            saver.get_meeting_folder().is_none(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(prefix).unwrap(), b"first seconds");
        assert_eq!(saver.get_meeting_folder(), Some(&folder));
        assert!(saver.snapshot_task.is_none());
        assert_eq!(draft_ids(&pool).await, ["another-start", id.as_str()]);
    }

    #[tokio::test]
    async fn uncertain_capture_shutdown_keeps_the_empty_folder_and_its_draft() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        saver
            .initialize_meeting_folder("Synthetic meeting", false)
            .unwrap();
        let id = saver.recording_id().unwrap().to_owned();
        let pool = stage_synthetic_calendar(&id).await;

        saver.failed_start(false).await;
        crate::library::recording_calendar::discard_failed_start_draft(
            &pool,
            Some(&id),
            saver.get_meeting_folder().is_none(),
        )
        .await
        .unwrap();
        assert!(saver.get_meeting_folder().unwrap().is_dir());
        assert_eq!(draft_ids(&pool).await, ["another-start", id.as_str()]);
    }

    async fn stage_synthetic_calendar(recording_id: &str) -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        for id in [recording_id, "another-start"] {
            sqlx::query("INSERT INTO recording_calendar_drafts VALUES (?, ?, 1)")
                .bind(id)
                .bind(r#"{"source":"microsoft","eventId":"fixture","subject":"Synthetic invitation"}"#)
                .execute(&pool)
                .await
                .unwrap();
        }
        pool
    }

    async fn draft_ids(pool: &sqlx::SqlitePool) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT recording_id FROM recording_calendar_drafts ORDER BY recording_id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_pending_disk_writer_does_not_block_transcript_ingestion() {
        let root = tempfile::tempdir().unwrap();
        let mut saver = RecordingSaver::new();
        saver.set_recordings_folder(root.path().to_path_buf());
        saver
            .initialize_meeting_folder("Synthetic meeting", false)
            .unwrap();
        saver.start_snapshot_timer(Duration::from_secs(60), None);
        let writer_lock = saver.snapshot_write_lock.clone();
        {
            let _writer = writer_lock.lock().unwrap();
            saver.add_transcript_chunk("First synthetic sentence".into());
            assert_eq!(saver.get_transcript_segments().len(), 1);
        }
        let path = saver
            .meeting_folder
            .as_ref()
            .unwrap()
            .join("transcripts.json");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !path.is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

impl Default for RecordingSaver {
    fn default() -> Self {
        Self::new()
    }
}
