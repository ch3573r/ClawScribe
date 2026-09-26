use crate::api::TranscriptWord;
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Mutex as AsyncMutex;

use super::audio_processing::create_meeting_folder;
use super::incremental_saver::IncrementalAudioSaver;

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
    pub meeting_name: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub duration_seconds: Option<f64>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub microphone: Option<String>,
    pub system_audio: Option<String>,
}

#[derive(Default)]
pub struct RecordingSaveReport {
    pub outcome: super::outcome::RecordingOutcome,
    pub audio_path: Option<String>,
    raw_fully_encoded: bool,
    warnings: Vec<&'static str>,
}

/// New recording saver using incremental saving strategy
pub struct RecordingSaver {
    mode: super::recording_mode::RecordingMode,
    incremental_saver: Option<Arc<AsyncMutex<IncrementalAudioSaver>>>,
    base_recordings_folder: Option<PathBuf>,
    meeting_folder: Option<PathBuf>,
    meeting_name: Option<String>,
    metadata: Option<MeetingMetadata>,
    transcript_segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    snapshot_dirty: Arc<AtomicBool>,
    snapshot_write_lock: Arc<Mutex<()>>,
    snapshot_task: Option<tokio::task::JoinHandle<()>>,
    transcript_updates_since_flush: AtomicUsize,
    transcript_snapshot_written: AtomicBool,
    last_transcript_flush: Mutex<Instant>,
    transcription_provider: Option<String>,
    transcription_model: Option<String>,
    transcription_source_language: Option<String>,
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
            transcript_segments: Arc::new(Mutex::new(Vec::new())),
            snapshot_dirty: Arc::new(AtomicBool::new(false)),
            snapshot_write_lock: Arc::new(Mutex::new(())),
            snapshot_task: None,
            transcript_updates_since_flush: AtomicUsize::new(0),
            transcript_snapshot_written: AtomicBool::new(false),
            last_transcript_flush: Mutex::new(Instant::now()),
            transcription_provider: None,
            transcription_model: None,
            transcription_source_language: None,
        }
    }

    /// Snapshot the selected capture mode before initializing meeting metadata.
    pub fn set_mode(&mut self, mode: super::recording_mode::RecordingMode) {
        self.mode = mode;
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
    pub fn set_device_info(&mut self, mic_name: Option<String>, sys_name: Option<String>) {
        if let Some(ref mut metadata) = self.metadata {
            metadata.devices.microphone = mic_name;
            metadata.devices.system_audio = sys_name;

            // Write updated metadata to disk if folder exists
            if let Some(folder) = &self.meeting_folder {
                let metadata_clone = metadata.clone();
                if let Err(_e) = self.write_metadata(folder, &metadata_clone) {
                    warn!("Failed to update metadata with device info");
                }
            }
        }
    }

    /// Add or update a structured transcript segment (upserts based on sequence_id)
    /// Also saves incrementally to disk
    pub fn add_transcript_segment(&self, segment: TranscriptSegment) {
        if let Ok(mut segments) = self.transcript_segments.lock() {
            // Check if segment with same sequence_id exists (update it)
            if let Some(existing) = segments
                .iter_mut()
                .find(|s| s.sequence_id == segment.sequence_id)
            {
                *existing = segment.clone();
                debug!("Updated existing transcript segment");
            } else {
                // New segment, add it
                segments.push(segment.clone());
                debug!("Added transcript segment");
            }
        } else {
            error!("Failed to lock transcript segments for adding segment");
        }

        self.snapshot_dirty.store(true, Ordering::Release);

        let pending_updates = self
            .transcript_updates_since_flush
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        let elapsed = self
            .last_transcript_flush
            .lock()
            .map(|last_flush| last_flush.elapsed())
            .unwrap_or(Duration::MAX);
        let snapshot_due = transcript_snapshot_due(
            self.transcript_snapshot_written.load(Ordering::Acquire),
            pending_updates,
            elapsed,
        );

        // Keep a crash-recovery snapshot, but coalesce updates. Rewriting the
        // entire growing JSON document for every segment is cumulative O(n²)
        // work over a long meeting.
        if snapshot_due {
            if let Some(folder) = &self.meeting_folder {
                match self.write_transcripts_json(folder) {
                    Ok(()) => {
                        self.transcript_updates_since_flush
                            .store(0, Ordering::Release);
                        self.transcript_snapshot_written
                            .store(true, Ordering::Release);
                        if let Ok(mut last_flush) = self.last_transcript_flush.lock() {
                            *last_flush = Instant::now();
                        }
                    }
                    Err(_e) => warn!("Failed to write incremental transcript update"),
                }
            }
        }
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
    /// * `auto_save` - If true, creates checkpoints and enables saving. If false, audio chunks are discarded.
    pub fn start_accumulation(
        &mut self,
        auto_save: bool,
        _state: Arc<super::recording_state::RecordingState>,
    ) -> Result<Option<super::transcription::queue::TranscriptionQueueSender>> {
        let name = self
            .meeting_name
            .clone()
            .unwrap_or_else(|| "Meeting".into());
        self.initialize_meeting_folder(&name, auto_save)?;
        self.start_snapshot_timer(TRANSCRIPT_SNAPSHOT_INTERVAL);
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

    fn start_snapshot_timer(&mut self, interval_duration: Duration) {
        let Some(folder) = self.meeting_folder.clone() else {
            return;
        };
        let segments = self.transcript_segments.clone();
        let dirty = self.snapshot_dirty.clone();
        let write_lock = self.snapshot_write_lock.clone();
        self.snapshot_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval_duration);
            interval.tick().await;
            loop {
                interval.tick().await;
                if !dirty.load(Ordering::Acquire) {
                    continue;
                }
                let (folder, segments, dirty, write_lock) = (
                    folder.clone(),
                    segments.clone(),
                    dirty.clone(),
                    write_lock.clone(),
                );
                let _ = tokio::task::spawn_blocking(move || {
                    let _writer = write_lock.lock().unwrap();
                    // Take the dirty flag before cloning. Updates arriving during
                    // I/O remain dirty for the next tick.
                    if !dirty.swap(false, Ordering::AcqRel) {
                        return;
                    }
                    let snapshot = segments.lock().unwrap().clone();
                    if write_transcript_snapshot(&folder, &snapshot).is_err() {
                        dirty.store(true, Ordering::Release);
                        warn!("Transcript recovery snapshot could not be saved; retrying");
                    }
                })
                .await;
            }
        }));
    }

    pub async fn failed_start(&mut self) {
        if let Some(task) = self.snapshot_task.take() {
            task.abort();
            let _ = task.await;
        }
        // The recording manager has already stopped and drained the pipeline.
        if self.transcript_segments.lock().unwrap().is_empty() {
            if let Some(folder) = &self.meeting_folder {
                // Only remove a newly owned folder containing metadata and empty
                // capture directories. Unknown or possibly recoverable files stay.
                let empty = std::fs::read_dir(folder).is_ok_and(|mut entries| {
                    entries.all(|entry| {
                        let Ok(entry) = entry else {
                            return false;
                        };
                        entry.file_name() == "metadata.json"
                            || ([".audio-spool", ".checkpoints"]
                                .iter()
                                .any(|name| entry.file_name() == *name)
                                && std::fs::read_dir(entry.path())
                                    .is_ok_and(|mut files| files.next().is_none()))
                    })
                });
                if empty && std::fs::remove_dir_all(folder).is_ok() {
                    self.meeting_folder = None;
                    self.metadata = None;
                    self.incremental_saver = None;
                    return;
                }
            }
        }
        // Never delete possible capture from a partially opened device. Mark
        // the folder interrupted so recovery can distinguish it from a live session.
        if let (Some(folder), Some(metadata)) = (&self.meeting_folder, &mut self.metadata) {
            metadata.status = "error".into();
            let metadata = metadata.clone();
            let folder = folder.clone();
            let _ = self.write_metadata(&folder, &metadata);
            let _ = self.write_transcripts_json(&folder);
        }
    }

    /// Initialize meeting folder structure and metadata
    ///
    /// # Arguments
    /// * `meeting_name` - Name of the meeting
    /// * `create_checkpoints` - Whether to create .checkpoints/ directory and IncrementalAudioSaver
    fn initialize_meeting_folder(
        &mut self,
        meeting_name: &str,
        create_checkpoints: bool,
    ) -> Result<()> {
        let base_folder = self
            .base_recordings_folder
            .clone()
            .unwrap_or_else(super::recording_preferences::get_default_recordings_folder);

        // Create meeting folder structure (with or without .checkpoints/ subdirectory)
        let meeting_folder = create_meeting_folder(&base_folder, meeting_name, create_checkpoints)?;
        self.meeting_folder = Some(meeting_folder.clone());

        // Only initialize incremental saver if checkpoints are needed (auto_save is true)
        if create_checkpoints {
            let incremental_saver = IncrementalAudioSaver::new(meeting_folder.clone(), 48000)?;
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
            meeting_name: Some(meeting_name.to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            duration_seconds: None,
            devices: DeviceInfo {
                microphone: None, // Could be enhanced to store actual device names
                system_audio: None,
            },
            audio_file: if create_checkpoints {
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
        };

        // Write initial metadata.json
        self.metadata = Some(metadata.clone());
        self.write_metadata(&meeting_folder, &metadata)?;

        self.meeting_folder = Some(meeting_folder);
        self.metadata = Some(metadata);

        Ok(())
    }

    /// Write metadata.json to disk (atomic write with temp file)
    fn write_metadata(&self, folder: &PathBuf, metadata: &MeetingMetadata) -> Result<()> {
        let metadata_path = folder.join("metadata.json");
        let temp_path = folder.join(".metadata.json.tmp");

        let json_string = serde_json::to_string_pretty(metadata)?;
        std::fs::write(&temp_path, json_string)?;
        std::fs::rename(&temp_path, &metadata_path)?; // Atomic

        Ok(())
    }

    /// Write transcripts.json to disk (atomic write with temp file and validation)
    fn write_transcripts_json(&self, folder: &PathBuf) -> Result<()> {
        let _writer = self.snapshot_write_lock.lock().unwrap();
        self.snapshot_dirty.store(false, Ordering::Release);
        // Clone segments to avoid holding lock during I/O
        let segments_clone = if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            error!("Failed to lock transcript segments for writing");
            return Err(anyhow::anyhow!("Failed to lock transcript segments"));
        };

        let result = write_transcript_snapshot(folder, &segments_clone);
        if result.is_err() {
            self.snapshot_dirty.store(true, Ordering::Release);
        }
        result
    }

    // in frontend/src-tauri/src/audio/recording_saver.rs
    pub fn get_stats(&self) -> (usize, u32) {
        if let Some(ref saver) = self.incremental_saver {
            if let Ok(guard) = saver.try_lock() {
                (guard.get_checkpoint_count() as usize, 48000)
            } else {
                (0, 48000)
            }
        } else {
            (0, 48000)
        }
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
        capture_incomplete: bool,
    ) -> Result<RecordingSaveReport, String> {
        let report = self
            .finalize_files(recording_duration, capture_incomplete)
            .await?;
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
        capture_incomplete: bool,
    ) -> Result<RecordingSaveReport, String> {
        info!("Stopping recording saver");
        if let Some(task) = self.snapshot_task.take() {
            task.abort();
            let _ = task.await;
        }
        let mut report = RecordingSaveReport::default();
        report.outcome.capture_incomplete = capture_incomplete;
        // Attempt every artifact independently, then report all failures.
        if let Some(folder) = &self.meeting_folder {
            if self.write_transcripts_json(folder).is_err() {
                report.outcome.recording_files_incomplete = true;
                report
                    .warnings
                    .push("The final transcript snapshot could not be saved.");
            }
            if let Some(metadata) = &mut self.metadata {
                metadata.duration_seconds = recording_duration.or_else(|| {
                    self.transcript_segments
                        .lock()
                        .ok()
                        .and_then(|segments| segments.last().map(|segment| segment.audio_end_time))
                });
                metadata.completed_at = Some(chrono::Utc::now().to_rfc3339());
                metadata.status = "error".into();
            }
            if capture_incomplete {
                let _ = std::fs::write(
                    folder.join(".audio-spool/.incomplete"),
                    b"capture incomplete",
                );
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
            if self.write_metadata(folder, &metadata).is_err() {
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
            if report.outcome.write(folder).is_err() {
                report.outcome.recording_files_incomplete = true;
                report
                    .warnings
                    .push("The recording warning status could not be saved.");
            }
            if report.raw_fully_encoded && !report.outcome.recording_files_incomplete {
                let _ = std::fs::remove_dir_all(folder.join(".audio-spool"));
            }
        }
        Ok(report)
    }

    /// Get the meeting folder path (for passing to backend)
    pub fn get_meeting_folder(&self) -> Option<&PathBuf> {
        self.meeting_folder.as_ref()
    }

    /// Get accumulated transcript segments (for reload sync)
    pub fn get_transcript_segments(&self) -> Vec<TranscriptSegment> {
        if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            Vec::new()
        }
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

fn write_transcript_snapshot(
    folder: &std::path::Path,
    segments: &[TranscriptSegment],
) -> Result<()> {
    use std::io::Write;
    let mut staged = tempfile::NamedTempFile::new_in(folder)?;
    serde_json::to_writer(
        staged.as_file_mut(),
        &serde_json::json!({
            "version": "1.0", "segments": segments, "total_segments": segments.len(),
            "last_updated": chrono::Utc::now().to_rfc3339()
        }),
    )?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    staged.persist(folder.join("transcripts.json"))?;
    Ok(())
}

impl Drop for RecordingSaver {
    fn drop(&mut self) {
        if let Some(task) = self.snapshot_task.take() {
            task.abort();
        }
    }
}

fn transcript_snapshot_due(
    snapshot_written: bool,
    pending_updates: usize,
    elapsed: Duration,
) -> bool {
    !snapshot_written
        || pending_updates >= TRANSCRIPT_SNAPSHOT_MAX_UPDATES
        || elapsed >= TRANSCRIPT_SNAPSHOT_INTERVAL
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

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
        assert_eq!(
            std::fs::read_dir(folder.join(".checkpoints"))
                .unwrap()
                .count(),
            0
        );
        let report = saver.finalize_files(Some(0.1), true).await.unwrap();
        assert!(report.audio_path.is_some());
        assert!(!report.outcome.audio_save_failed);
        assert!(report.outcome.capture_incomplete);
        assert!(report.outcome.recording_files_incomplete);
        assert_eq!(report.warnings.len(), 2);
        assert!(
            folder.join(".audio-spool").exists(),
            "failed artifacts retain originals"
        );
        super::super::incremental_saver::validate_recoverable_temp_audio_file(
            &folder.join("audio.mp4"),
        )
        .unwrap();
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
        let report = saver.finalize_files(Some(1.1), true).await.unwrap();
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
        let report = saver.finalize_files(Some(1.0), false).await.unwrap();
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
        saver.start_snapshot_timer(Duration::from_millis(20));
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
                let json: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                if json["total_segments"] == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        saver.failed_start().await;
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
        drop(sender);
        saver.failed_start().await;
        assert!(!folder.exists());
        assert!(saver.snapshot_task.is_none());
    }

    #[test]
    fn transcript_snapshots_are_coalesced_but_first_update_is_durable() {
        assert!(transcript_snapshot_due(false, 1, Duration::ZERO));
        assert!(!transcript_snapshot_due(true, 1, Duration::from_secs(1)));
        assert!(transcript_snapshot_due(
            true,
            TRANSCRIPT_SNAPSHOT_MAX_UPDATES,
            Duration::from_secs(1)
        ));
        assert!(transcript_snapshot_due(
            true,
            1,
            TRANSCRIPT_SNAPSHOT_INTERVAL
        ));
    }
}

impl Default for RecordingSaver {
    fn default() -> Self {
        Self::new()
    }
}
