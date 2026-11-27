use anyhow::Result;
use log::{debug, error, info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::devices::{list_audio_devices, AudioDevice};

#[cfg(target_os = "macos")]
use super::devices::get_safe_recording_devices_macos;

use super::device_monitor::{AudioDeviceMonitor, DeviceEvent, DeviceMonitorType};
#[cfg(not(target_os = "macos"))]
use super::devices::{default_input_device, default_output_device};
use super::pipeline::AudioPipelineManager;
use super::recording_saver::{RecordingSaver, RecordingTranscripts};
use super::recording_state::{DeviceType as RecordingDeviceType, RecordingState};
use super::stream::AudioStreamManager;
use super::transcription::queue::{transcription_queue, TranscriptionQueueReceiver};

/// Stream manager type enumeration
pub enum StreamManagerType {
    Standard(AudioStreamManager),
}
/// Captured at producer startup; never resolves a session ID from global state.
#[derive(Clone)]
pub struct LiveTranscriptSink {
    pub session_id: String,
    pub recording_generation: crate::knowledge::live::RecordingGeneration,
    state: Arc<crate::knowledge::live::LiveState>,
    transcripts: Arc<RecordingTranscripts>,
    pub(super) conversation: Arc<super::live_conversation::LiveConversation>,
    pub(crate) capture: Arc<RecordingState>,
}
impl LiveTranscriptSink {
    pub(crate) fn ingest(
        &self,
        update: &super::transcription::TranscriptUpdate,
        clear_source: bool,
    ) -> bool {
        // A finished decode can be marked partial when some text was unreadable.
        // Keep the readable portion in the recording; live assistance filters it.
        if !self.transcripts.record_update(&self.session_id, update) {
            return false;
        }
        self.state.ingest(&self.session_id, update);
        self.conversation.ingest(update, clear_source);
        true
    }
    pub(crate) fn snapshot(&self) -> Result<crate::knowledge::live::LiveSnapshot, String> {
        self.state.snapshot(&self.session_id)
    }
    pub(super) fn transcript_segments(&self) -> Vec<super::recording_saver::TranscriptSegment> {
        self.transcripts.segments()
    }
}

/// Simplified recording manager that coordinates all audio components
pub struct RecordingManager {
    detected_call: bool,
    live: Option<LiveTranscriptSink>,
    mode: super::recording_mode::RecordingMode,
    state: Arc<RecordingState>,
    stream_manager: AudioStreamManager,
    pipeline_manager: AudioPipelineManager,
    recording_saver: RecordingSaver,
    device_monitor: Option<AudioDeviceMonitor>,
    device_event_receiver: Option<mpsc::UnboundedReceiver<DeviceEvent>>,
}

// SAFETY: RecordingManager contains types that we've marked as Send
unsafe impl Send for RecordingManager {}

impl RecordingManager {
    /// Create a new recording manager
    pub fn new() -> Self {
        let state = RecordingState::new();
        let stream_manager = AudioStreamManager::new(state.clone());
        let pipeline_manager = AudioPipelineManager::new();
        let (device_monitor, device_event_receiver) = AudioDeviceMonitor::new();

        Self {
            live: None,
            detected_call: false,
            mode: super::recording_mode::RecordingMode::Live,
            state,
            stream_manager,
            pipeline_manager,
            recording_saver: RecordingSaver::new(),
            device_monitor: Some(device_monitor),
            device_event_receiver: Some(device_event_receiver),
        }
    }

    // Remove app handle storage for now - will be passed directly when saving
    pub fn set_mode(&mut self, mode: super::recording_mode::RecordingMode) {
        self.mode = mode;
        self.recording_saver.set_mode(mode);
    }

    pub(crate) fn set_summary_language(&mut self, language: Option<&str>) {
        self.recording_saver.set_summary_language(language);
    }
    pub(crate) fn bind_live_session(
        &mut self,
        state: Arc<crate::knowledge::live::LiveState>,
        session_id: String,
        recording_generation: crate::knowledge::live::RecordingGeneration,
    ) -> LiveTranscriptSink {
        let sink = LiveTranscriptSink {
            transcripts: self.recording_saver.bind_transcript_session(&session_id),
            conversation: Arc::new(super::live_conversation::LiveConversation::new(&session_id)),
            session_id,
            recording_generation,
            state,
            capture: self.state.clone(),
        };
        self.live = Some(sink.clone());
        sink
    }

    pub(crate) fn close_transcript_input(&self) {
        self.recording_saver.close_transcript_input();
    }
    pub(crate) fn live_session_id(&self) -> Option<&str> {
        self.live.as_ref().map(|sink| sink.session_id.as_str())
    }
    pub(super) fn mark_detected_call(&mut self, session: &str) {
        if self.live_session_id() == Some(session) {
            self.detected_call = true;
        }
    }
    pub(super) fn owns_detected_call(&self, session: &str) -> bool {
        self.detected_call && self.live_session_id() == Some(session)
    }
    pub(crate) fn live_recording_generation(
        &self,
    ) -> Option<crate::knowledge::live::RecordingGeneration> {
        self.live.as_ref().map(|sink| sink.recording_generation)
    }
    pub(crate) fn live_snapshot(&self) -> Result<crate::knowledge::live::LiveSnapshot, String> {
        let mut snapshot = self
            .live
            .as_ref()
            .ok_or("No active live session")?
            .snapshot()?;
        snapshot.transcription_incomplete |= self.state.capture_incomplete();
        if self.mode.transcribes() {
            snapshot.transcription_incomplete |=
                super::transcription::current_transcription_metrics().is_none_or(|metrics| {
                    metrics.failed_chunks > 0
                        || metrics.chunks_in_queue > 0
                        || metrics.is_processing
                });
        }
        Ok(snapshot)
    }

    /// Start recording with specified devices
    ///
    /// # Arguments
    /// * `microphone_device` - Optional microphone device to use
    /// * `system_device` - Optional system audio device to use
    /// * `auto_save` - Whether to save raw audio (true) or just transcripts/metadata (false)
    pub async fn start_recording(
        &mut self,
        microphone_device: Option<Arc<AudioDevice>>,
        system_device: Option<Arc<AudioDevice>>,
        auto_save: bool,
    ) -> Result<TranscriptionQueueReceiver> {
        self.start_recording_with_preparation(
            microphone_device,
            system_device,
            auto_save,
            |_| async { Ok(()) },
        )
        .await
    }

    pub(crate) async fn start_recording_with_preparation<F, Fut>(
        &mut self,
        microphone_device: Option<Arc<AudioDevice>>,
        system_device: Option<Arc<AudioDevice>>,
        auto_save: bool,
        prepare: F,
    ) -> Result<TranscriptionQueueReceiver>
    where
        F: FnOnce(Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let result = self
            .start_session(microphone_device, system_device, auto_save, prepare)
            .await;
        if result.is_err() {
            self.state.stop_recording();
            let streams_stopped = self.stream_manager.stop_streams().is_ok();
            let pipeline_drained = self.pipeline_manager.stop().await.is_ok();
            self.recording_saver
                .failed_start(streams_stopped && pipeline_drained)
                .await;
            self.state.cleanup();
        }
        result
    }

    async fn start_session<F, Fut>(
        &mut self,
        microphone_device: Option<Arc<AudioDevice>>,
        system_device: Option<Arc<AudioDevice>>,
        auto_save: bool,
        prepare: F,
    ) -> Result<TranscriptionQueueReceiver>
    where
        F: FnOnce(Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        info!("Starting recording manager (auto_save: {})", auto_save);

        // Stage VAD segments on disk. Slow inference must not retain an
        // unbounded number of large f32 buffers in memory or stall capture.
        let (transcription_sender, transcription_receiver, _) = transcription_queue()?;

        // CRITICAL FIX: Create recording sender for pre-mixed audio from pipeline
        // Pipeline will mix mic + system audio professionally and send to this channel
        // Pass auto_save to control whether raw audio is saved
        let recording_sender = self
            .recording_saver
            .start_accumulation(auto_save, self.state.clone())?;
        prepare(self.recording_id().map(str::to_owned)).await?;

        // Start recording state first
        self.state.start_recording()?;

        // Get device information for adaptive mixing
        // The pipeline uses device kind (Bluetooth vs Wired) to apply adaptive buffering:
        // - Bluetooth: Larger buffers (80-200ms) to handle jitter
        // - Wired: Smaller buffers (20-50ms) for low latency
        let (mic_name, mic_kind) = if let Some(ref mic) = microphone_device {
            let device_kind =
                super::device_detection::InputDeviceKind::detect(&mic.name, 512, 48000);
            (mic.name.clone(), device_kind)
        } else {
            (
                "No Microphone".to_string(),
                super::device_detection::InputDeviceKind::Unknown,
            )
        };

        let (sys_name, sys_kind) = if let Some(ref sys) = system_device {
            let device_kind =
                super::device_detection::InputDeviceKind::detect(&sys.name, 512, 48000);
            (sys.name.clone(), device_kind)
        } else {
            (
                "No System Audio".to_string(),
                super::device_detection::InputDeviceKind::Unknown,
            )
        };

        // Start the audio processing pipeline with FFmpeg adaptive mixer
        // Pipeline will: 1) Mix mic+system audio with adaptive buffering, 2) Send mixed to recording_sender,
        // 3) Apply VAD and send speech segments to transcription
        self.pipeline_manager.start(
            self.state.clone(),
            transcription_sender,
            0,     // Ignored - using dynamic sizing internally
            48000, // 48kHz sample rate
            recording_sender,
            mic_name,
            mic_kind,
            sys_name,
            sys_kind,
            self.mode.transcribes(),
        )?;

        // Give the pipeline a moment to fully initialize before starting streams
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Start audio streams - they send RAW unmixed chunks to pipeline for mixing
        // Pipeline handles mixing and distribution to both recording and transcription
        // Record the first device-open time until capture supplies sample zero.
        let saver = &mut self.recording_saver;
        self.stream_manager
            .start_streams(
                microphone_device.clone(),
                system_device.clone(),
                None,
                || {
                    saver.mark_capture_started(
                        chrono::Utc::now(),
                        microphone_device.as_ref().map(|d| d.name.clone()),
                        system_device.as_ref().map(|d| d.name.clone()),
                    );
                },
            )
            .await?;

        // Start device monitoring to detect disconnects
        if let Some(ref mut monitor) = self.device_monitor {
            if let Err(_e) = monitor.start_monitoring(microphone_device, system_device) {
                warn!("Failed to start device monitoring");
                // Non-fatal - continue without monitoring
            } else {
                info!("✅ Device monitoring started");
            }
        }

        info!(
            "Recording manager started successfully with {} active streams",
            self.stream_manager.active_stream_count()
        );

        Ok(transcription_receiver)
    }

    /// Start recording with default devices and auto_save setting
    ///
    /// # Arguments
    /// * `auto_save` - Whether to save raw audio (true) or just transcripts/metadata (false)
    ///
    /// # Platform-Specific Behavior
    ///
    /// **macOS**: Uses smart device selection that automatically overrides
    /// Bluetooth devices to built-in wired devices for stable, consistent sample rates.
    /// This prevents Core Audio/ScreenCaptureKit from delivering variable sample rate
    /// streams that cause sync issues when mixing mic + system audio.
    ///
    /// **Windows/Linux**: Uses system default devices directly without override.
    ///
    /// # macOS Bluetooth Override Strategy
    ///
    /// - Microphone: If Bluetooth → Use built-in MacBook mic
    /// - Speaker: If Bluetooth → Use built-in MacBook speaker (for ScreenCaptureKit)
    /// - Each device is checked INDEPENDENTLY
    ///
    /// Rationale: Bluetooth devices on macOS can have variable sample rates as Core Audio
    /// and the Bluetooth stack may resample dynamically. Built-in devices provide
    /// fixed, consistent sample rates for reliable audio mixing.
    ///
    /// User still hears audio via Bluetooth (playback), but recording captures
    /// via stable wired path for best quality.
    pub async fn start_recording_with_defaults_and_auto_save(
        &mut self,
        auto_save: bool,
    ) -> Result<TranscriptionQueueReceiver> {
        #[cfg(target_os = "macos")]
        {
            info!("🎙️ [macOS] Starting recording with smart device selection (Bluetooth override enabled)");

            // Get safe recording devices with automatic Bluetooth fallback
            // This function handles all the detection and override logic for macOS
            let (microphone_device, system_device) = get_safe_recording_devices_macos()?;

            // Wrap in Arc for sharing across threads
            let microphone_device = microphone_device.map(Arc::new);
            let system_device = system_device.map(Arc::new);

            // Ensure at least microphone is available
            if microphone_device.is_none() {
                return Err(anyhow::anyhow!(
                    "❌ No microphone device available for recording"
                ));
            }

            // Start recording with selected devices and auto_save setting
            self.start_recording(microphone_device, system_device, auto_save)
                .await
        }

        #[cfg(not(target_os = "macos"))]
        {
            info!("Starting recording with default devices");

            // Get default devices (no Bluetooth override on Windows/Linux)
            let microphone_device = match default_input_device() {
                Ok(device) => {
                    info!("Using default microphone: {}", device.name);
                    Some(Arc::new(device))
                }
                Err(_e) => {
                    warn!("No default microphone available");
                    None
                }
            };

            let system_device = match default_output_device() {
                Ok(device) => {
                    info!("Using default system audio: {}", device.name);
                    Some(Arc::new(device))
                }
                Err(_e) => {
                    warn!("No default system audio available");
                    None
                }
            };

            // Ensure at least microphone is available
            if microphone_device.is_none() {
                return Err(anyhow::anyhow!("No microphone device available"));
            }

            self.start_recording(microphone_device, system_device, auto_save)
                .await
        }
    }

    pub fn continue_audio_only(&mut self) {
        self.state.disable_live_transcription();
        self.mode = super::recording_mode::RecordingMode::AudioOnly;
        self.recording_saver.continue_audio_only();
    }

    pub async fn abort_start(&mut self, preserve_audio: bool) {
        if preserve_audio {
            self.continue_audio_only();
            let drained = self.stop_streams_and_force_flush().await.is_ok();
            self.recording_saver.failed_start(drained).await;
            self.state.cleanup();
            return;
        }
        if let Some(ref mut monitor) = self.device_monitor {
            monitor.stop_monitoring().await;
        }
        self.state.stop_recording();
        let streams_stopped = self.stream_manager.stop_streams().is_ok();
        let pipeline_drained = self.pipeline_manager.stop().await.is_ok();
        self.recording_saver
            .failed_start(streams_stopped && pipeline_drained)
            .await;
        self.state.cleanup();
    }

    /// Stop recording streams without saving (for use when waiting for transcription)
    pub async fn stop_streams_only(&mut self) -> Result<()> {
        info!("Stopping recording streams only");

        // Stop device monitoring
        if let Some(ref mut monitor) = self.device_monitor {
            monitor.stop_monitoring().await;
        }

        // Stop recording state first
        self.state.stop_recording();

        // Stop audio streams
        if let Err(_e) = self.stream_manager.stop_streams() {
            error!("Error stopping audio streams");
        }

        // Stop audio pipeline
        if let Err(_e) = self.pipeline_manager.stop().await {
            error!("Error stopping audio pipeline");
        }

        debug!("Recording streams stopped successfully");
        Ok(())
    }

    /// Stop streams and force immediate pipeline flush to process all accumulated audio
    pub async fn stop_streams_and_force_flush(&mut self) -> Result<()> {
        info!("🚀 Stopping recording streams with IMMEDIATE pipeline flush");

        // Stop recording state first - this clears device references
        self.state.stop_recording();

        // A native driver can block while pausing or destroying a stream.
        // Freeze this session first, then bound how long Stop waits for release.
        let mut streams = std::mem::replace(
            &mut self.stream_manager,
            AudioStreamManager::new(self.state.clone()),
        );
        let release = tokio::task::spawn_blocking(move || streams.stop_streams());

        if let Some(ref mut monitor) = self.device_monitor {
            monitor.stop_monitoring().await;
        }

        // CRITICAL: Force pipeline to flush ALL accumulated audio before stopping
        debug!("💨 Forcing pipeline to flush accumulated audio immediately");
        let flush_result = self.pipeline_manager.force_flush_and_stop().await;
        if !matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), release).await,
            Ok(Ok(Ok(())))
        ) {
            warn!("An audio device did not release cleanly after capture stopped");
        }
        if flush_result.is_err() {
            self.state.mark_capture_incomplete();
        }

        // stop_recording already releases capture devices. Retain frozen timing
        // and failure statistics until metadata and the final outcome are saved.

        info!("Recording capture stopped");
        flush_result
    }

    /// Save recording after transcription is complete
    pub async fn save_recording_only<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        outcome: super::outcome::RecordingOutcome,
    ) -> Result<super::recording_saver::RecordingSaveReport> {
        self.recording_saver
            .stop_and_save(app, self.state.get_active_recording_duration(), outcome)
            .await
            .map_err(anyhow::Error::msg)
    }

    pub(crate) fn recording_id(&self) -> Option<&str> {
        self.recording_saver.recording_id()
    }

    pub(super) fn recording_provenance(&self) -> super::provenance::RecordingProvenance {
        let mut provenance = super::provenance::RecordingProvenance::recording(
            self.get_active_recording_duration(),
            self.recording_saver.started_at(),
        );
        provenance.started_at = self.state.capture_started_at().or(provenance.started_at);
        provenance
    }

    pub(crate) fn library_transcripts(&self) -> Vec<crate::api::TranscriptSegment> {
        self.recording_saver
            .get_transcript_segments()
            .into_iter()
            .map(|segment| crate::api::TranscriptSegment {
                id: segment.id,
                text: segment.text,
                timestamp: segment.display_time,
                audio_start_time: Some(segment.audio_start_time),
                audio_end_time: Some(segment.audio_end_time),
                duration: Some(segment.duration),
                speaker: segment.speaker,
                word_timestamps: segment.word_timestamps,
            })
            .collect()
    }

    /// Stop recording and save audio (legacy method)
    pub async fn stop_recording<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
    ) -> Result<()> {
        info!("Stopping recording manager");

        // Get recording duration BEFORE stopping (important!)
        let recording_duration = self.state.get_active_recording_duration();
        info!("Recording duration before stop: {:?}s", recording_duration);

        // Stop recording state first
        self.state.stop_recording();

        // Stop audio streams
        if let Err(_e) = self.stream_manager.stop_streams() {
            error!("Error stopping audio streams");
        }

        // Stop audio pipeline
        if let Err(_e) = self.pipeline_manager.stop().await {
            error!("Error stopping audio pipeline");
        }

        // Save the recording with actual duration
        match self
            .recording_saver
            .stop_and_save(
                app,
                recording_duration,
                super::outcome::RecordingOutcome {
                    capture_incomplete: self.state.capture_incomplete(),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(report) if report.audio_path.is_some() => {
                info!("Recording audio saved successfully");
            }
            Ok(_) => {
                info!("Recording not saved (auto-save disabled or no audio data)");
            }
            Err(_e) => {
                error!("Failed to save recording");
                // Don't fail the stop operation if saving fails
            }
        }

        info!("Recording manager stopped");
        Ok(())
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.state.is_recording()
    }

    /// Pause the current recording session
    pub fn pause_recording(&self) -> Result<()> {
        info!("Pausing recording");
        self.state.pause_recording()
    }

    /// Resume the current recording session
    pub fn resume_recording(&self) -> Result<()> {
        info!("Resuming recording");
        self.state.resume_recording()
    }

    /// Check if recording is currently paused
    pub fn is_paused(&self) -> bool {
        self.state.is_paused()
    }

    /// Check if recording is active (recording and not paused)
    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }

    /// Get recording statistics
    pub fn get_stats(&self) -> super::recording_state::RecordingStats {
        self.state.get_stats()
    }

    /// Get recording duration
    pub fn get_recording_duration(&self) -> Option<f64> {
        self.state.get_recording_duration()
    }

    /// Get active recording duration (excluding pauses)
    pub fn get_active_recording_duration(&self) -> Option<f64> {
        self.state.get_active_recording_duration()
    }

    /// Get total pause duration
    pub fn get_total_pause_duration(&self) -> f64 {
        self.state.get_total_pause_duration()
    }

    /// Get current pause duration if paused
    pub fn get_current_pause_duration(&self) -> Option<f64> {
        self.state.get_current_pause_duration()
    }

    /// Get error information
    pub fn get_error_info(&self) -> (u32, Option<super::recording_state::AudioError>) {
        (self.state.get_error_count(), self.state.get_last_error())
    }

    /// Get active stream count
    pub fn active_stream_count(&self) -> usize {
        self.stream_manager.active_stream_count()
    }

    /// Set error callback for handling errors
    pub fn set_error_callback<F>(&self, callback: F)
    where
        F: Fn(&super::recording_state::AudioError) + Send + Sync + 'static,
    {
        self.state.set_error_callback(callback);
    }

    /// Set a callback for non-fatal warnings surfaced to the UI.
    pub fn set_warning_callback<F>(&self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.state.set_warning_callback(callback);
    }

    /// Set a callback for the live input levels (about ten calls a second).
    pub fn set_levels_callback<F>(&self, callback: F)
    where
        F: Fn(&super::recording_levels::RecordingLevels) + Send + Sync + 'static,
    {
        self.state.set_levels_callback(callback);
    }

    /// Set a callback for the recordings volume running low on space.
    pub fn set_disk_space_callback<F>(&self, callback: F)
    where
        F: Fn(u64, &std::path::Path) + Send + Sync + 'static,
    {
        self.state.set_disk_space_callback(callback);
    }

    pub fn set_warning_cleared_callback<F>(&self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.state.set_warning_cleared_callback(callback);
    }

    /// Check if there's a fatal error
    pub fn has_fatal_error(&self) -> bool {
        self.state.has_fatal_error()
    }

    /// Set the meeting name for this recording session
    pub fn set_meeting_name(&mut self, name: Option<String>) {
        self.recording_saver.set_meeting_name(name);
    }

    /// Set the base folder where new recording meeting folders are created.
    pub fn set_recordings_folder(&mut self, folder: PathBuf) {
        self.recording_saver.set_recordings_folder(folder);
    }

    /// Record which transcription engine + model this session uses.
    pub fn set_transcription_info(
        &mut self,
        provider: Option<String>,
        model: Option<String>,
        source_language: Option<String>,
    ) {
        self.recording_saver
            .set_transcription_info(provider, model, source_language);
    }

    pub fn transcription_provider(&self) -> Option<&str> {
        self.recording_saver.transcription_provider()
    }

    /// Add a structured transcript segment to be saved later
    pub fn add_transcript_segment(&self, segment: super::recording_saver::TranscriptSegment) {
        self.recording_saver.add_transcript_segment(segment);
    }

    /// Add a transcript chunk to be saved later (legacy method)
    pub fn add_transcript_chunk(&self, text: String) {
        self.recording_saver.add_transcript_chunk(text);
    }

    /// Get accumulated transcript segments from current recording session
    /// Used for syncing frontend state after page reload during active recording
    pub fn get_transcript_segments(&self) -> Vec<super::recording_saver::TranscriptSegment> {
        self.recording_saver.get_transcript_segments()
    }

    /// Get meeting name from current recording session
    /// Used for syncing frontend state after page reload during active recording
    pub fn get_meeting_name(&self) -> Option<String> {
        self.recording_saver.get_meeting_name()
    }

    /// Cleanup all resources without saving
    pub async fn cleanup_without_save(&mut self) {
        if self.is_recording() {
            debug!("Stopping recording without saving during cleanup");

            // Stop recording state first
            self.state.stop_recording();

            // Stop audio streams
            if let Err(_e) = self.stream_manager.stop_streams() {
                error!("Error stopping audio streams during cleanup");
            }

            // Stop audio pipeline
            if let Err(_e) = self.pipeline_manager.stop().await {
                error!("Error stopping audio pipeline during cleanup");
            }
        }
        self.state.cleanup();
    }

    /// Get the meeting folder path (if available)
    /// Returns None if no meeting name was set or folder structure not initialized
    pub fn get_meeting_folder(&self) -> Option<std::path::PathBuf> {
        self.recording_saver.get_meeting_folder().map(|p| p.clone())
    }

    /// Check for device events (disconnects/reconnects)
    /// Returns Some(DeviceEvent) if an event occurred, None otherwise
    pub fn poll_device_events(&mut self) -> Option<DeviceEvent> {
        if let Some(ref mut receiver) = self.device_event_receiver {
            receiver.try_recv().ok()
        } else {
            None
        }
    }

    pub fn handle_device_disconnect_event(
        &mut self,
        device_name: String,
        device_type: DeviceMonitorType,
    ) {
        let (source, kind) = source_types(device_type);
        let device = Arc::new(AudioDevice::new(device_name, kind));
        self.state.mark_source_missing(device, source);
    }

    pub async fn attempt_device_reconnect(
        &mut self,
        device_name: &str,
        device_type: DeviceMonitorType,
    ) -> Result<ReconnectResult> {
        let (source, kind) = source_types(device_type);
        let Some(revision) = self.state.source_failure_revision(device_name, &source) else {
            return Ok(self.reconnect_result(false));
        };

        let device = list_audio_devices()
            .await?
            .into_iter()
            .find(|device| device.name == device_name && device.device_type == kind);
        let Some(device) = device.map(Arc::new) else {
            return Ok(self.reconnect_result(false));
        };
        if self
            .stream_manager
            .reconnect_source(device, source.clone())
            .await
            .is_err()
        {
            warn!("Requested audio source could not be reopened");
        } else {
            // Keep the source visibly unavailable until it opens, and never
            // clear a newer error reported by a callback during that open.
            self.state.mark_source_reconnected(&source, revision);
        }
        // An error callback can report a disconnect even while the stream is opening.
        Ok(self.reconnect_result(self.source_active(&source)))
    }

    fn source_active(&self, source: &RecordingDeviceType) -> bool {
        self.stream_manager.source_active(source) && !self.state.source_is_missing(source)
    }

    fn reconnect_result(&self, reconnected: bool) -> ReconnectResult {
        ReconnectResult {
            reconnected,
            microphone_active: self.source_active(&RecordingDeviceType::Microphone),
            system_audio_active: self.source_active(&RecordingDeviceType::System),
        }
    }

    /// Get reference to recording state for external access
    pub fn get_state(&self) -> &Arc<RecordingState> {
        &self.state
    }
}

impl Default for RecordingManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RecordingManager {
    fn drop(&mut self) {
        // Note: Can't call async cleanup in Drop, but streams have their own Drop implementations
        self.state.cleanup();
    }
}

fn source_types(device: DeviceMonitorType) -> (RecordingDeviceType, super::devices::DeviceType) {
    use super::devices::DeviceType;
    match device {
        DeviceMonitorType::Microphone => (RecordingDeviceType::Microphone, DeviceType::Input),
        DeviceMonitorType::SystemAudio => (RecordingDeviceType::System, DeviceType::Output),
    }
}

#[derive(serde::Serialize)]
pub struct ReconnectResult {
    pub reconnected: bool,
    pub microphone_active: bool,
    pub system_audio_active: bool,
}

#[cfg(test)]
mod reconnect_recovery_tests {
    use super::*;

    #[test]
    fn failed_loopback_stays_pending_when_mic_returns_and_gaps_survive_retry() {
        let manager = RecordingManager::new();
        manager.state.start_recording().unwrap();
        let (mic, mic_kind) = source_types(DeviceMonitorType::Microphone);
        let (system, system_kind) = source_types(DeviceMonitorType::SystemAudio);
        manager.state.mark_source_missing(
            Arc::new(AudioDevice::new("synthetic output".into(), system_kind)),
            system.clone(),
        );
        manager.state.mark_source_missing(
            Arc::new(AudioDevice::new("synthetic microphone".into(), mic_kind)),
            mic.clone(),
        );
        let mic_failure = manager
            .state
            .source_failure_revision("synthetic microphone", &mic)
            .unwrap();
        manager.state.mark_source_reconnected(&mic, mic_failure);
        let pending = manager.state.disconnected_devices();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, system);
        assert!(!manager.reconnect_result(false).reconnected);
        let system_failure = manager
            .state
            .source_failure_revision("synthetic output", &system)
            .unwrap();
        manager
            .state
            .mark_source_reconnected(&system, system_failure);
        assert!(manager.state.disconnected_devices().is_empty());
        assert!(manager.state.capture_incomplete());
    }

    #[test]
    fn a_new_source_error_during_open_stays_pending() {
        let manager = RecordingManager::new();
        manager.state.start_recording().unwrap();
        let (source, kind) = source_types(DeviceMonitorType::SystemAudio);
        let device = Arc::new(AudioDevice::new("synthetic output".into(), kind));
        manager
            .state
            .mark_source_missing(device.clone(), source.clone());
        let opening_revision = manager
            .state
            .source_failure_revision(&device.name, &source)
            .unwrap();
        manager
            .state
            .mark_source_missing(device.clone(), source.clone());
        manager
            .state
            .mark_source_reconnected(&source, opening_revision);
        assert!(manager
            .state
            .source_failure_revision(&device.name, &source)
            .is_some());
        assert!(manager.state.capture_incomplete());
    }

    #[test]
    fn a_late_disconnect_from_stream_release_does_not_create_a_capture_gap() {
        let manager = RecordingManager::new();
        manager.state.start_recording().unwrap();
        manager.state.stop_recording();
        let (source, kind) = source_types(DeviceMonitorType::SystemAudio);
        manager.state.mark_source_missing(
            Arc::new(AudioDevice::new("synthetic output".into(), kind)),
            source,
        );
        assert!(manager.state.disconnected_devices().is_empty());
        assert!(!manager.state.capture_incomplete());
    }
}
