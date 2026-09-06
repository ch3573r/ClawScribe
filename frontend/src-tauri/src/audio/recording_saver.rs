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

/// New recording saver using incremental saving strategy
pub struct RecordingSaver {
    mode: super::recording_mode::RecordingMode,
    incremental_saver: Option<Arc<AsyncMutex<IncrementalAudioSaver>>>,
    base_recordings_folder: Option<PathBuf>,
    meeting_folder: Option<PathBuf>,
    meeting_name: Option<String>,
    metadata: Option<MeetingMetadata>,
    transcript_segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    transcript_updates_since_flush: AtomicUsize,
    transcript_snapshot_written: AtomicBool,
    last_transcript_flush: Mutex<Instant>,
    accumulation_task: Option<tokio::task::JoinHandle<Result<(), String>>>,
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
            transcript_updates_since_flush: AtomicUsize::new(0),
            transcript_snapshot_written: AtomicBool::new(false),
            last_transcript_flush: Mutex::new(Instant::now()),
            accumulation_task: None,
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
        state: Arc<super::recording_state::RecordingState>,
    ) -> Result<Option<super::transcription::queue::TranscriptionQueueSender>> {
        let name = self
            .meeting_name
            .clone()
            .unwrap_or_else(|| "Meeting".into());
        self.initialize_meeting_folder(&name, auto_save)?;
        if !auto_save {
            return Ok(None);
        }
        let folder = self
            .meeting_folder
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Recording folder unavailable"))?;
        let (sender, mut receiver, metrics) =
            super::transcription::queue::recording_audio_queue(folder)?;
        let saver = self
            .incremental_saver
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Audio saver unavailable"))?;
        self.accumulation_task = Some(tokio::spawn(async move {
            let result: Result<(), String> = async {
                while let Some(item) = receiver
                    .recv()
                    .await
                    .map_err(|_| "Recorded audio spool could not be read".to_string())?
                {
                    let saver = saver.clone();
                    tokio::task::spawn_blocking(move || {
                        saver.blocking_lock().add_chunk(item.chunk)
                    })
                    .await
                    .map_err(|_| "Audio encoder task failed".to_string())?
                    .map_err(|_| "Audio checkpoint could not be saved".to_string())?;
                }
                if metrics.snapshot().failed_chunks > 0 {
                    return Err(
                        "Some captured audio could not be written; recovery files preserved".into(),
                    );
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                state.report_warning("Audio encoding failed. Capture is being preserved in the meeting recovery files. Check disk space and recover the audio after stopping.");
            }
            result
        }));
        Ok(Some(sender))
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
        // Clone segments to avoid holding lock during I/O
        let segments_clone = if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            error!("Failed to lock transcript segments for writing");
            return Err(anyhow::anyhow!("Failed to lock transcript segments"));
        };

        debug!("Writing transcript segments to JSON");

        let transcript_path = folder.join("transcripts.json");
        let temp_path = folder.join(".transcripts.json.tmp");

        // Create JSON structure
        let json = serde_json::json!({
            "version": "1.0",
            "segments": segments_clone,
            "last_updated": chrono::Utc::now().to_rfc3339(),
            "total_segments": segments_clone.len()
        });

        // Serialize to pretty JSON string
        let json_string = serde_json::to_string(&json).map_err(|e| {
            error!("Failed to serialize transcripts to JSON");
            anyhow::anyhow!("JSON serialization failed: {}", e)
        })?;

        // Write to temp file with error handling
        std::fs::write(&temp_path, &json_string).map_err(|e| {
            error!("Failed to write temporary transcript file");
            anyhow::anyhow!("Failed to write temp file: {}", e)
        })?;

        // Verify temp file was written correctly
        if !temp_path.exists() {
            error!("Temp transcript file does not exist after write");
            return Err(anyhow::anyhow!("Temp file verification failed"));
        }

        // Atomic rename
        std::fs::rename(&temp_path, &transcript_path).map_err(|e| {
            error!("Failed to finalize transcript file");
            anyhow::anyhow!("Failed to rename transcript file: {}", e)
        })?;

        debug!(
            "✅ Successfully wrote transcripts.json with {} segments",
            segments_clone.len()
        );
        Ok(())
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
    ) -> Result<Option<String>, String> {
        info!("Stopping recording saver");

        // The stopped pipeline drops the producer. Join the consumer after it
        // drains every accepted chunk; never drop a tail based on a sleep/flag.
        if let Some(mut task) = self.accumulation_task.take() {
            match tokio::time::timeout(std::time::Duration::from_secs(60), &mut task).await {
                Ok(result) => result.map_err(|_| {
                    "Audio saver task failed; recovery files preserved".to_string()
                })??,
                Err(_) => {
                    task.abort();
                    return Err("Audio saving timed out; recovery files were preserved. Recover the recording after stopping.".into());
                }
            }
        }

        // Check if incremental saver exists (indicates auto_save was enabled)
        let should_save_audio = self.incremental_saver.is_some();

        if !should_save_audio {
            info!("⚠️  No audio saver initialized (auto-save was disabled) - skipping audio finalization");
            info!("✅ Transcripts and metadata already saved incrementally");
            return Ok(None);
        }

        // Finalize incremental saver (merge checkpoints into final audio.mp4)
        let final_audio_path = if let Some(saver_arc) = &self.incremental_saver {
            let saver_arc = saver_arc.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut saver = saver_arc.blocking_lock();
                tokio::runtime::Handle::current().block_on(saver.finalize())
            })
            .await
            .map_err(|_| "Audio finalization task failed; recovery files preserved".to_string())?;
            match result {
                Ok(path) => {
                    info!("✅ Successfully finalized audio");
                    path
                }
                Err(e) => {
                    error!("❌ Failed to finalize incremental saver");
                    return Err(format!("Failed to finalize audio: {}", e));
                }
            }
        } else {
            error!("No incremental saver initialized - cannot save recording");
            return Err("No incremental saver initialized".to_string());
        };

        // Save final transcripts.json with validation
        if let Some(folder) = &self.meeting_folder {
            if let Err(e) = self.write_transcripts_json(folder) {
                error!("❌ Failed to write final transcripts");
                return Err(format!("Failed to save transcripts: {}", e));
            }

            // Verify transcripts were written correctly
            let transcript_path = folder.join("transcripts.json");
            if !transcript_path.exists() {
                error!("Transcript file was not created");
                return Err("Transcript file verification failed".to_string());
            }
            info!("Transcripts saved and verified");
        }

        // Update metadata to completed status with actual recording duration
        if let (Some(folder), Some(mut metadata)) = (&self.meeting_folder, self.metadata.clone()) {
            metadata.status = "completed".to_string();
            metadata.completed_at = Some(chrono::Utc::now().to_rfc3339());

            // Use actual recording duration from RecordingState (more accurate than transcript segments)
            // Falls back to last transcript segment if duration not provided
            metadata.duration_seconds = recording_duration.or_else(|| {
                if let Ok(segments) = self.transcript_segments.lock() {
                    segments.last().map(|seg| seg.audio_end_time)
                } else {
                    None
                }
            });

            if let Err(e) = self.write_metadata(folder, &metadata) {
                error!("❌ Failed to update metadata to completed");
                return Err(format!("Failed to update metadata: {}", e));
            }

            info!(
                "✅ Metadata updated with duration: {:?}s",
                metadata.duration_seconds
            );
        }

        // Emit save event with audio and transcript paths
        let save_event = serde_json::json!({
            "audio_file": final_audio_path.to_string_lossy(),
            "transcript_file": self.meeting_folder.as_ref()
                .map(|f| f.join("transcripts.json").to_string_lossy().to_string()),
            "meeting_name": self.meeting_name,
            "meeting_folder": self.meeting_folder.as_ref()
                .map(|f| f.to_string_lossy().to_string())
        });

        if let Err(_e) = app.emit("recording-saved", &save_event) {
            warn!("Failed to emit recording-saved event");
        }

        // The durable spool is redundant only after audio and metadata succeed.
        if !capture_incomplete {
            if let Some(folder) = &self.meeting_folder {
                let _ = std::fs::remove_dir_all(folder.join(".audio-spool"));
            }
        }

        // Clean up transcript segments
        if let Ok(mut segments) = self.transcript_segments.lock() {
            segments.clear();
        }

        Ok(Some(final_audio_path.to_string_lossy().to_string()))
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
