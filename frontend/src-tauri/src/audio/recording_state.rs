use anyhow::Result;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use super::buffer_pool::AudioBufferPool;
use super::devices::AudioDevice;
use super::recording_levels::RecordingLevels;

#[derive(Default)]
struct LiveLevels {
    latest: Option<(Instant, RecordingLevels)>,
    quiet_mic_since: Option<Instant>,
    mic_heard: bool,
}

impl LiveLevels {
    fn observe(&mut self, levels: RecordingLevels, now: Instant) {
        self.latest = Some((now, levels));
        // Match the in-app meter's -60 dB floor. Once we hear any input,
        // subsequent silence is normal until a pause/resume resets the meter.
        if levels.mic.is_some_and(|mic| mic.rms.max(mic.peak) >= 0.001) {
            self.mic_heard = true;
        }
        if levels.mic.is_some() && !self.mic_heard {
            self.quiet_mic_since.get_or_insert(now);
        } else {
            self.quiet_mic_since = None;
        }
    }

    fn mic_no_signal(&self, now: Instant) -> bool {
        self.quiet_mic_since.is_some_and(|started| {
            now.saturating_duration_since(started) >= Duration::from_secs(10)
        })
    }
}

/// Device type for audio chunks
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceType {
    Microphone,
    System,
}

/// Audio chunk with metadata for processing
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub timestamp: f64,
    pub chunk_id: u64,
    pub device_type: DeviceType,
}

pub(super) const CAPTURE_HANDOFF_MS: u32 = 400;

#[derive(Default)]
pub(super) struct CaptureProgress {
    pub admitted_end: AtomicU64,
    pub pending_packets: AtomicU64,
}

pub(super) struct CaptureStamp {
    pub resume_revision: u64,
    pub seconds: f64,
    pub skipped_frames: usize,
}

const CAPTURE_NOT_STARTED: u64 = u64::MAX;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Processed audio chunk (post-VAD) for recording
#[derive(Debug, Clone)]
pub struct ProcessedAudioChunk {
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub timestamp: f64,
    pub device_type: DeviceType,
}

/// Comprehensive error types for audio system
#[derive(Debug, Clone)]
pub enum AudioError {
    DeviceDisconnected,
    StreamFailed,
    ProcessingFailed,
    TranscriptionFailed,
    ChannelClosed,
    InitializationFailed,
    ConfigurationError,
    PermissionDenied,
    BufferOverflow,
    SampleRateUnsupported,
}

impl AudioError {
    /// Check if error is recoverable (can attempt reconnection)
    pub fn is_recoverable(&self) -> bool {
        match self {
            // Device disconnect is now recoverable - we can attempt reconnection
            AudioError::DeviceDisconnected => true,
            AudioError::StreamFailed => true,
            AudioError::ProcessingFailed => true,
            AudioError::TranscriptionFailed => true,
            AudioError::ChannelClosed => false,
            AudioError::InitializationFailed => false,
            AudioError::ConfigurationError => false,
            AudioError::PermissionDenied => false,
            AudioError::BufferOverflow => true,
            AudioError::SampleRateUnsupported => false,
        }
    }

    /// Get user-friendly error message
    pub fn user_message(&self) -> &'static str {
        match self {
            AudioError::DeviceDisconnected => "Audio device was disconnected",
            AudioError::StreamFailed => "Audio stream encountered an error",
            AudioError::ProcessingFailed => "Audio processing failed",
            AudioError::TranscriptionFailed => "Speech transcription failed",
            AudioError::ChannelClosed => "Audio channel was closed unexpectedly",
            AudioError::InitializationFailed => "Failed to initialize audio system",
            AudioError::ConfigurationError => "Audio configuration error",
            AudioError::PermissionDenied => "Microphone permission denied",
            AudioError::BufferOverflow => "Audio buffer overflow",
            AudioError::SampleRateUnsupported => "Audio sample rate not supported",
        }
    }
}

/// Recording statistics
#[derive(Debug, Default)]
pub struct RecordingStats {
    pub chunks_processed: u64,
    pub total_duration: f64,
    pub last_activity: Option<Instant>,
}

/// Unified state management for audio recording
#[derive(Clone, Default, serde::Serialize)]
pub(crate) struct CaptureHealth {
    pub warnings: Vec<String>,
    pub destination_free_bytes: Option<u64>,
    pub scratch_free_bytes: Option<u64>,
    pub details: super::outcome::CaptureDetails,
}

pub struct RecordingState {
    // Core recording state
    is_recording: AtomicBool,
    capture_incomplete: AtomicBool,
    recovery_signal: tokio::sync::Notify,
    live_transcription_disabled: AtomicBool,
    speech_clock_ms: AtomicU64,
    is_paused: AtomicBool,
    resume_revision: AtomicU64,
    capture_epoch: Instant,
    capture_epoch_utc: chrono::DateTime<chrono::Utc>,
    capture_origin_ns: AtomicU64,
    capture_started_ns: AtomicU64,
    capture_pause_ns: AtomicU64,
    capture_cutoff_ns: AtomicU64,
    capture_start_notified: AtomicBool,
    capture_lost_through: AtomicU64,
    capture_progress: Mutex<Vec<(DeviceType, std::sync::Weak<CaptureProgress>)>>,

    // Audio devices
    microphone_device: Mutex<Option<Arc<AudioDevice>>>,
    system_device: Mutex<Option<Arc<AudioDevice>>>,
    // Track which device is disconnected for reconnection attempts
    disconnected_devices: Mutex<Vec<(Arc<AudioDevice>, DeviceType, u64)>>,
    source_failure_revision: AtomicU64,

    // Audio pipeline
    audio_sender: Mutex<Option<mpsc::Sender<AudioChunk>>>,

    // Memory optimization
    buffer_pool: AudioBufferPool,

    // Error handling
    error_count: AtomicU32,
    recoverable_error_count: AtomicU32,
    recent_errors: Mutex<VecDeque<Instant>>,
    last_error: Mutex<Option<AudioError>>,
    error_callback: Mutex<Option<Box<dyn Fn(&AudioError) + Send + Sync>>>,
    // Non-fatal warnings surfaced to the UI (e.g. system audio is silent).
    pub(crate) health: Mutex<CaptureHealth>,
    warning_callback: Mutex<Option<Box<dyn Fn(&str) + Send + Sync>>>,
    warning_cleared_callback: Mutex<Option<Box<dyn Fn(&str) + Send + Sync>>>,
    // Called once per recording when the recordings volume runs low.
    disk_space_callback: Mutex<Option<Box<dyn Fn(u64, &std::path::Path) + Send + Sync>>>,
    latest_levels: Mutex<LiveLevels>,
    // Called about ten times a second with the live input levels.
    levels_callback:
        Mutex<Option<Box<dyn Fn(&super::recording_levels::RecordingLevels) + Send + Sync>>>,

    // Statistics
    stats: Mutex<RecordingStats>,

    // Recording start time for accurate timestamps
    recording_start: Mutex<Option<Instant>>,
    recording_end: Mutex<Option<Instant>>,
    // Pause time tracking
    pause_start: Mutex<Option<Instant>>,
    total_pause_duration: Mutex<std::time::Duration>,
}

impl RecordingState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    // Recording control
    pub fn start_recording(&self) -> Result<()> {
        *self.latest_levels.lock().unwrap() = LiveLevels::default();
        self.capture_incomplete.store(false, Ordering::Release);
        let start = Instant::now();
        let start_ns = self.capture_nanos(start);
        self.capture_origin_ns
            .store(CAPTURE_NOT_STARTED, Ordering::SeqCst);
        self.capture_started_ns
            .store(CAPTURE_NOT_STARTED, Ordering::SeqCst);
        self.capture_pause_ns.store(0, Ordering::SeqCst);
        self.capture_cutoff_ns.store(start_ns, Ordering::SeqCst);
        self.capture_start_notified.store(false, Ordering::Relaxed);
        self.capture_lost_through.store(0, Ordering::Relaxed);
        *self.recording_start.lock().unwrap() = Some(start);
        *self.recording_end.lock().unwrap() = None;
        self.recent_errors.lock().unwrap().clear();
        self.error_count.store(0, Ordering::SeqCst);
        self.recoverable_error_count.store(0, Ordering::SeqCst);
        *self.last_error.lock().unwrap() = None;
        self.is_recording.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn stop_recording(&self) {
        self.recording_end
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
        if let Some(start) = self.pause_start.lock().unwrap().take() {
            *self.total_pause_duration.lock().unwrap() += start.elapsed();
        }
        self.is_recording.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        // Clear pause tracking when stopping
        *self.pause_start.lock().unwrap() = None;
        // CRITICAL: Clear audio sender to close the pipeline channel
        // This ensures the pipeline loop exits properly after processing all chunks
        *self.audio_sender.lock().unwrap() = None;
        // CRITICAL: Clear device references to release microphone/speaker
        // Without this, Arc<AudioDevice> references persist and keep the mic active
        *self.microphone_device.lock().unwrap() = None;
        *self.system_device.lock().unwrap() = None;
        self.disconnected_devices.lock().unwrap().clear();
        log::info!("Recording stopped, device references cleared");
    }

    pub fn pause_recording(&self) -> Result<()> {
        if !self.is_recording() {
            return Err(anyhow::anyhow!("Cannot pause when not recording"));
        }
        if self.is_paused() {
            return Err(anyhow::anyhow!("Recording is already paused"));
        }

        self.is_paused.store(true, Ordering::SeqCst);
        *self.pause_start.lock().unwrap() = Some(Instant::now());
        log::info!("Recording paused");
        Ok(())
    }

    pub fn resume_recording(&self) -> Result<()> {
        if !self.is_recording() {
            return Err(anyhow::anyhow!("Cannot resume when not recording"));
        }
        if !self.is_paused() {
            return Err(anyhow::anyhow!("Recording is not paused"));
        }

        // Calculate pause duration and add to total
        if let Some(pause_start) = self.pause_start.lock().unwrap().take() {
            let resumed = Instant::now();
            let pause_duration = resumed.duration_since(pause_start);
            *self.total_pause_duration.lock().unwrap() += pause_duration;
            self.capture_pause_ns
                .fetch_add(pause_duration.as_nanos() as u64, Ordering::SeqCst);
            self.capture_cutoff_ns
                .store(self.capture_nanos(resumed), Ordering::SeqCst);
            log::info!(
                "Recording resumed after pause of {:.2}s",
                pause_duration.as_secs_f64()
            );
        }

        *self.latest_levels.lock().unwrap() = LiveLevels::default();
        self.resume_revision.fetch_add(1, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::SeqCst)
    }

    pub fn is_active(&self) -> bool {
        self.is_recording() && !self.is_paused()
    }

    pub(super) fn resume_revision(&self) -> u64 {
        self.resume_revision.load(Ordering::SeqCst)
    }

    fn capture_nanos(&self, instant: Instant) -> u64 {
        instant
            .saturating_duration_since(self.capture_epoch)
            .as_nanos() as u64
    }

    fn active_capture_clock(&self) -> Option<(u64, u64, u64)> {
        let resume_revision = self.resume_revision();
        let paused = self.capture_pause_ns.load(Ordering::SeqCst);
        let cutoff = self.capture_cutoff_ns.load(Ordering::SeqCst);
        (self.is_active() && resume_revision == self.resume_revision()).then_some((
            resume_revision,
            paused,
            cutoff,
        ))
    }

    /// The first packet with admissible frames claims the common sample zero.
    /// The callback only uses atomics and the monotonic clock.
    pub(super) fn admit_capture(
        &self,
        captured_at: Instant,
        frames: usize,
        rate: u32,
    ) -> Option<CaptureStamp> {
        let (resume_revision, paused, cutoff) = self.active_capture_clock()?;
        let raw = if captured_at >= self.capture_epoch {
            i128::from(self.capture_nanos(captured_at))
        } else {
            -(self.capture_epoch.duration_since(captured_at).as_nanos() as i128)
        };
        let frames_before = |boundary: u64| {
            ((i128::from(boundary) - raw).max(0) as u128 * u128::from(rate))
                .div_ceil(u128::from(NANOS_PER_SECOND)) as usize
        };
        let after_frames = |skip: usize| {
            (raw + (skip as i128 * i128::from(NANOS_PER_SECOND)) / i128::from(rate)).max(0) as u64
        };
        let mut origin = self.capture_origin_ns.load(Ordering::Acquire);
        let skipped = frames_before(cutoff);
        if skipped >= frames {
            return None;
        }
        if origin == CAPTURE_NOT_STARTED {
            let first = after_frames(skipped);
            let effective_origin = first.saturating_sub(paused);
            origin = match self.capture_origin_ns.compare_exchange(
                CAPTURE_NOT_STARTED,
                effective_origin,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.capture_started_ns.store(first, Ordering::Release);
                    effective_origin
                }
                Err(existing) => existing,
            };
        }
        let skipped_frames = frames_before(cutoff.max(origin.saturating_add(paused)));
        if skipped_frames >= frames {
            return None;
        }
        Some(CaptureStamp {
            resume_revision,
            seconds: after_frames(skipped_frames).saturating_sub(origin + paused) as f64
                / NANOS_PER_SECOND as f64,
            skipped_frames,
        })
    }

    pub(super) fn capture_seconds(&self, instant: Instant) -> Option<f64> {
        let (_, paused, _) = self.active_capture_clock()?;
        let origin = self.capture_origin_ns.load(Ordering::Acquire);
        (origin != CAPTURE_NOT_STARTED).then(|| {
            self.capture_nanos(instant).saturating_sub(origin + paused) as f64
                / NANOS_PER_SECOND as f64
        })
    }

    pub(super) fn capture_started_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        let started = self.capture_started_ns.load(Ordering::Acquire);
        (started != CAPTURE_NOT_STARTED)
            .then(|| self.capture_epoch_utc + chrono::Duration::nanoseconds(started as i64))
    }

    pub(super) fn record_capture_gap_end(&self, end_sample: u64) {
        self.capture_lost_through
            .fetch_max(end_sample, Ordering::Relaxed);
    }

    pub(super) fn capture_gap_end(&self) -> u64 {
        self.capture_lost_through.load(Ordering::Relaxed)
    }

    pub(super) fn register_capture_progress(
        &self,
        source: DeviceType,
        progress: &Arc<CaptureProgress>,
    ) {
        let mut sources = self.capture_progress.lock().unwrap();
        sources.retain(|(_, source)| source.strong_count() > 0);
        sources.push((source, Arc::downgrade(progress)));
    }

    pub(super) fn sources_with_pending_capture(&self, clock: u64, grace: u64) -> [bool; 2] {
        let mut active = [false; 2];
        for (device, progress) in self.capture_progress.lock().unwrap().iter() {
            if let Some(progress) = progress.upgrade() {
                let end = progress.admitted_end.load(Ordering::Acquire);
                let pending = progress.pending_packets.load(Ordering::Acquire);
                let index = usize::from(*device == DeviceType::System);
                active[index] |= pending > 0 || (end > 0 && clock < end.saturating_add(grace));
            }
        }
        active
    }

    pub fn mark_source_missing(&self, device: Arc<AudioDevice>, device_type: DeviceType) {
        let mut pending = self.disconnected_devices.lock().unwrap();
        if !self.is_recording() {
            return;
        }
        pending.retain(|(_, source, _)| *source != device_type);
        let revision = self.source_failure_revision.fetch_add(1, Ordering::Relaxed);
        pending.push((device, device_type, revision));
        self.mark_capture_incomplete();
    }

    pub(super) fn source_failure_revision(&self, name: &str, kind: &DeviceType) -> Option<u64> {
        self.disconnected_devices
            .lock()
            .unwrap()
            .iter()
            .find(|(device, source, _)| device.name == name && source == kind)
            .map(|(_, _, revision)| *revision)
    }

    pub(super) fn mark_source_reconnected(&self, device_type: &DeviceType, reopened_revision: u64) {
        self.disconnected_devices
            .lock()
            .unwrap()
            .retain(|(_, source, revision)| {
                source != device_type || *revision != reopened_revision
            });
    }

    pub fn disconnected_devices(&self) -> Vec<(Arc<AudioDevice>, DeviceType)> {
        self.disconnected_devices
            .lock()
            .unwrap()
            .iter()
            .map(|(device, source, _)| (device.clone(), source.clone()))
            .collect()
    }

    pub(super) fn source_is_missing(&self, kind: &DeviceType) -> bool {
        self.disconnected_devices
            .lock()
            .unwrap()
            .iter()
            .any(|(_, source, _)| source == kind)
    }

    pub fn clear_active_device(&self, device_type: &DeviceType) {
        let device = match device_type {
            DeviceType::Microphone => &self.microphone_device,
            DeviceType::System => &self.system_device,
        };
        *device.lock().unwrap() = None;
    }

    // Device management
    pub fn set_microphone_device(&self, device: Arc<AudioDevice>) {
        *self.microphone_device.lock().unwrap() = Some(device);
    }

    pub fn set_system_device(&self, device: Arc<AudioDevice>) {
        *self.system_device.lock().unwrap() = Some(device);
    }

    pub fn get_microphone_device(&self) -> Option<Arc<AudioDevice>> {
        self.microphone_device.lock().unwrap().clone()
    }

    pub fn get_system_device(&self) -> Option<Arc<AudioDevice>> {
        self.system_device.lock().unwrap().clone()
    }

    // Audio pipeline management
    pub fn set_audio_sender(&self, sender: mpsc::Sender<AudioChunk>) {
        *self.audio_sender.lock().unwrap() = Some(sender);
    }

    pub(crate) fn disable_live_transcription(&self) {
        self.live_transcription_disabled
            .store(true, Ordering::Release);
    }

    pub(crate) fn live_transcription_disabled(&self) -> bool {
        self.live_transcription_disabled.load(Ordering::Acquire)
    }

    pub fn capture_incomplete(&self) -> bool {
        self.capture_incomplete.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_for_recovery_work(&self) {
        self.recovery_signal.notified().await;
    }

    pub(crate) fn request_recovery_snapshot(&self) {
        self.recovery_signal.notify_one();
    }

    pub fn mark_capture_incomplete(&self) {
        if !self.capture_incomplete.swap(true, Ordering::AcqRel) {
            self.request_recovery_snapshot();
        }
    }

    pub(crate) fn record_sample_loss(&self, samples: usize) {
        let mut health = self.health.lock().unwrap();
        let first = health.details.dropped_samples == 0;
        health.details.dropped_samples = health
            .details
            .dropped_samples
            .saturating_add(samples as u64);
        drop(health);
        self.mark_capture_incomplete();
        if first {
            self.report_warning("Some audio was lost while processing was busy. Recording continues; the saved meeting will show a capture gap.");
        }
    }

    pub(crate) fn record_system_silence(&self) {
        let mut health = self.health.lock().unwrap();
        health.details.system_silence_periods =
            health.details.system_silence_periods.saturating_add(1);
    }

    pub(super) fn observe_transcription_backend(&self, backend: String) {
        self.health.lock().unwrap().details.backend = Some(backend);
    }

    pub fn send_audio_chunk(&self, chunk: AudioChunk) -> Result<()> {
        if self.is_paused() || !self.is_recording() {
            return Ok(());
        }
        let sender = self
            .capture_sender()
            .ok_or_else(|| anyhow::anyhow!("Audio pipeline not ready"))?;
        self.send_captured_chunk(&sender, chunk)
    }

    pub(super) fn capture_sender(&self) -> Option<mpsc::Sender<AudioChunk>> {
        self.audio_sender.lock().unwrap().clone()
    }

    pub(super) fn send_captured_chunk(
        &self,
        sender: &mpsc::Sender<AudioChunk>,
        chunk: AudioChunk,
    ) -> Result<()> {
        match sender.try_send(chunk) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(chunk)) => {
                let end = (chunk.timestamp * f64::from(chunk.sample_rate)).round() as u64
                    + chunk.data.len() as u64;
                self.record_capture_gap_end(end);
                self.record_sample_loss(chunk.data.len());
                return Ok(());
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.mark_capture_incomplete();
                self.stop_recording();
                self.report_warning("The audio processor stopped unexpectedly. Press Stop to save the available recording.");
                return Err(anyhow::anyhow!("Audio capture queue closed"));
            }
        }
        let mut stats = self.stats.lock().unwrap();
        stats.chunks_processed += 1;
        stats.last_activity = Some(Instant::now());
        if self.capture_started_ns.load(Ordering::Acquire) != CAPTURE_NOT_STARTED
            && !self.capture_start_notified.swap(true, Ordering::AcqRel)
        {
            self.request_recovery_snapshot();
        }
        Ok(())
    }

    // Error handling
    pub fn set_error_callback<F>(&self, callback: F)
    where
        F: Fn(&AudioError) + Send + Sync + 'static,
    {
        *self.error_callback.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn set_warning_callback<F>(&self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        *self.warning_callback.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn set_warning_cleared_callback<F>(&self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        *self.warning_cleared_callback.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn set_disk_space_callback<F>(&self, callback: F)
    where
        F: Fn(u64, &std::path::Path) + Send + Sync + 'static,
    {
        *self.disk_space_callback.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn set_levels_callback<F>(&self, callback: F)
    where
        F: Fn(&super::recording_levels::RecordingLevels) + Send + Sync + 'static,
    {
        *self.levels_callback.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn report_levels(&self, levels: &super::recording_levels::RecordingLevels) {
        // Sampled by the optional controls window. Metering must never delay the pipeline.
        if let Ok(mut latest) = self.latest_levels.try_lock() {
            if !self.is_paused() {
                latest.observe(*levels, Instant::now());
            }
        }
        if let Some(callback) = self.levels_callback.lock().unwrap().as_ref() {
            callback(levels);
        }
    }

    pub(crate) fn live_levels(&self) -> (Option<RecordingLevels>, bool) {
        let Ok(readings) = self.latest_levels.lock() else {
            return (None, false);
        };
        let now = Instant::now();
        let levels = readings.latest.map(|(at, mut levels)| {
            if now.duration_since(at) >= Duration::from_millis(500) {
                for source in [&mut levels.mic, &mut levels.system].into_iter().flatten() {
                    source.rms = 0.0;
                    source.peak = 0.0;
                }
            }
            levels
        });
        (levels, !self.is_paused() && readings.mic_no_signal(now))
    }

    /// Tell the UI the recordings volume is running low. Does not affect
    /// recording state.
    pub fn report_disk_space_low(&self, free_bytes: u64, folder: &std::path::Path) {
        log::warn!("Free disk space for the recording is low: {free_bytes} bytes");
        if let Some(callback) = self.disk_space_callback.lock().unwrap().as_ref() {
            callback(free_bytes, folder);
        }
    }

    /// Resolve one recoverable warning without clearing unrelated recording failures.
    pub fn clear_warning(&self, message: &str) {
        self.health
            .lock()
            .unwrap()
            .warnings
            .retain(|warning| warning != message);
        if let Some(callback) = self.warning_cleared_callback.lock().unwrap().as_ref() {
            callback(message);
        }
    }

    /// Surface a non-fatal warning to the UI. Does not affect recording state.
    pub fn report_warning(&self, message: &str) {
        let mut health = self.health.lock().unwrap();
        if health.warnings.iter().any(|warning| warning == message) {
            return;
        }
        health.warnings.push(message.into());
        drop(health);
        log::warn!("Recording warning: {message}");
        if let Some(callback) = self.warning_callback.lock().unwrap().as_ref() {
            callback(message);
        }
    }

    pub fn report_error(&self, error: AudioError) {
        self.error_count.fetch_add(1, Ordering::SeqCst);

        // Track recoverable vs non-recoverable errors separately
        if error.is_recoverable() {
            let recoverable_count = self.recoverable_error_count.fetch_add(1, Ordering::SeqCst) + 1;
            log::warn!(
                "Recoverable audio error ({}): {:?}",
                recoverable_count,
                error
            );

            // Isolated glitches over a long meeting must not accumulate into
            // a fatal condition. Only a burst in the last 30 seconds stops capture.
            let recent_count = self.record_recent_error(Instant::now());
            if recent_count >= 10 {
                log::error!(
                    "Too many recoverable errors ({}), stopping recording",
                    recent_count
                );
                self.mark_capture_incomplete();
                self.stop_recording();
            }
        } else {
            log::error!("Non-recoverable audio error: {:?}", error);
            // Stop immediately for non-recoverable errors
            self.mark_capture_incomplete();
            self.stop_recording();
        }

        *self.last_error.lock().unwrap() = Some(error.clone());

        // Call error callback if set
        if let Some(callback) = self.error_callback.lock().unwrap().as_ref() {
            callback(&error);
        }
    }

    fn record_recent_error(&self, now: Instant) -> usize {
        let mut recent = self.recent_errors.lock().unwrap();
        while recent
            .front()
            .is_some_and(|time| now.duration_since(*time).as_secs() >= 30)
        {
            recent.pop_front();
        }
        recent.push_back(now);
        // Bounded even if a disconnected device keeps reporting errors.
        if recent.len() > 10 {
            recent.pop_front();
        }
        recent.len()
    }

    pub fn get_error_count(&self) -> u32 {
        self.error_count.load(Ordering::SeqCst)
    }

    pub fn get_recoverable_error_count(&self) -> u32 {
        self.recoverable_error_count.load(Ordering::SeqCst)
    }

    pub fn get_last_error(&self) -> Option<AudioError> {
        self.last_error.lock().unwrap().clone()
    }

    pub fn has_fatal_error(&self) -> bool {
        if let Some(error) = &*self.last_error.lock().unwrap() {
            !error.is_recoverable() && self.error_count.load(Ordering::SeqCst) > 0
        } else {
            false
        }
    }

    // Statistics
    pub fn get_stats(&self) -> RecordingStats {
        self.stats.lock().unwrap().clone()
    }

    // The mixer counts only samples sent to the speech detector. Paused
    // wall time never enters this timeline.
    pub(super) fn set_speech_clock_ms(&self, position_ms: u64) {
        self.speech_clock_ms.store(position_ms, Ordering::Relaxed);
    }

    pub(super) fn speech_clock_seconds(&self) -> f64 {
        self.speech_clock_ms.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub fn get_recording_duration(&self) -> Option<f64> {
        self.recording_start.lock().unwrap().map(|start| {
            self.recording_end
                .lock()
                .unwrap()
                .unwrap_or_else(Instant::now)
                .duration_since(start)
                .as_secs_f64()
        })
    }

    pub fn get_active_recording_duration(&self) -> Option<f64> {
        self.recording_start.lock().unwrap().map(|start| {
            let end = self
                .recording_end
                .lock()
                .unwrap()
                .unwrap_or_else(Instant::now);
            let pause_duration = self.get_total_pause_duration();
            let current_pause = if self.is_paused() {
                self.pause_start
                    .lock()
                    .unwrap()
                    .map(|p| p.elapsed().as_secs_f64())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let origin = self.capture_origin_ns.load(Ordering::Acquire);
            let elapsed = if origin == CAPTURE_NOT_STARTED {
                end.saturating_duration_since(start).as_secs_f64()
            } else {
                self.capture_nanos(end).saturating_sub(origin) as f64 / NANOS_PER_SECOND as f64
            };
            (elapsed - pause_duration - current_pause).max(0.0)
        })
    }

    pub fn get_total_pause_duration(&self) -> f64 {
        self.total_pause_duration.lock().unwrap().as_secs_f64()
    }

    pub fn get_current_pause_duration(&self) -> Option<f64> {
        if self.is_paused() {
            self.pause_start
                .lock()
                .unwrap()
                .map(|start| start.elapsed().as_secs_f64())
        } else {
            None
        }
    }

    // Memory management
    pub fn get_buffer_pool(&self) -> AudioBufferPool {
        self.buffer_pool.clone()
    }

    // Cleanup
    pub fn cleanup(&self) {
        self.stop_recording();
        *self.microphone_device.lock().unwrap() = None;
        *self.system_device.lock().unwrap() = None;
        self.disconnected_devices.lock().unwrap().clear();
        *self.audio_sender.lock().unwrap() = None;
        *self.last_error.lock().unwrap() = None;
        *self.error_callback.lock().unwrap() = None;
        *self.stats.lock().unwrap() = RecordingStats::default();
        *self.recording_start.lock().unwrap() = None;
        *self.recording_end.lock().unwrap() = None;
        self.recent_errors.lock().unwrap().clear();
        *self.pause_start.lock().unwrap() = None;
        *self.total_pause_duration.lock().unwrap() = std::time::Duration::ZERO;
        self.error_count.store(0, Ordering::SeqCst);
        self.recoverable_error_count.store(0, Ordering::SeqCst);

        // Clear buffer pool to free memory
        self.buffer_pool.clear();
    }
}

impl Default for RecordingState {
    fn default() -> Self {
        Self {
            capture_incomplete: AtomicBool::new(false),
            recovery_signal: tokio::sync::Notify::new(),
            live_transcription_disabled: AtomicBool::new(false),
            speech_clock_ms: AtomicU64::new(0),
            is_recording: AtomicBool::new(false),
            is_paused: AtomicBool::new(false),
            resume_revision: AtomicU64::new(0),
            capture_epoch: Instant::now(),
            capture_epoch_utc: chrono::Utc::now(),
            capture_origin_ns: AtomicU64::new(CAPTURE_NOT_STARTED),
            capture_started_ns: AtomicU64::new(CAPTURE_NOT_STARTED),
            capture_pause_ns: AtomicU64::new(0),
            capture_cutoff_ns: AtomicU64::new(0),
            capture_start_notified: AtomicBool::new(false),
            capture_lost_through: AtomicU64::new(0),
            capture_progress: Mutex::new(Vec::new()),
            microphone_device: Mutex::new(None),
            system_device: Mutex::new(None),
            disconnected_devices: Mutex::new(Vec::new()),
            source_failure_revision: AtomicU64::new(0),
            audio_sender: Mutex::new(None),
            buffer_pool: AudioBufferPool::new(16, 48000), // Pool of 16 buffers with 48kHz samples capacity
            error_count: AtomicU32::new(0),
            recoverable_error_count: AtomicU32::new(0),
            recent_errors: Mutex::new(VecDeque::new()),
            last_error: Mutex::new(None),
            error_callback: Mutex::new(None),
            warning_callback: Mutex::new(None),
            health: Mutex::new(Default::default()),
            warning_cleared_callback: Mutex::new(None),
            disk_space_callback: Mutex::new(None),
            levels_callback: Mutex::new(None),
            latest_levels: Mutex::new(LiveLevels::default()),
            stats: Mutex::new(RecordingStats::default()),
            recording_start: Mutex::new(None),
            recording_end: Mutex::new(None),
            pause_start: Mutex::new(None),
            total_pause_duration: Mutex::new(std::time::Duration::ZERO),
        }
    }
}

// Thread-safe cloning for RecordingStats
impl Clone for RecordingStats {
    fn clone(&self) -> Self {
        Self {
            chunks_processed: self.chunks_processed,
            total_duration: self.total_duration,
            last_activity: self.last_activity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_admitted_frame_sets_sample_zero_and_excludes_device_open_time() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        let requested = state.recording_start.lock().unwrap().unwrap();
        let first = requested + std::time::Duration::from_millis(750);
        assert!(state.capture_seconds(first).is_none());
        assert!(state
            .admit_capture(
                requested - std::time::Duration::from_millis(10),
                240,
                48_000
            )
            .is_none());
        assert!(state.capture_started_at().is_none());

        let stamp = state.admit_capture(first, 480, 48_000).unwrap();
        assert_eq!((stamp.seconds, stamp.skipped_frames), (0.0, 0));
        let late_other_source = state
            .admit_capture(first - std::time::Duration::from_millis(5), 480, 48_000)
            .unwrap();
        assert_eq!(
            (late_other_source.seconds, late_other_source.skipped_frames),
            (0.0, 240)
        );

        let end = first + std::time::Duration::from_millis(250);
        assert_eq!(state.capture_seconds(end), Some(0.25));
        assert_eq!(
            state.capture_started_at(),
            Some(
                state.capture_epoch_utc
                    + chrono::Duration::nanoseconds(state.capture_nanos(first) as i64)
            )
        );
        *state.recording_end.lock().unwrap() = Some(end);
        state.stop_recording();
        assert_eq!(state.get_active_recording_duration(), Some(0.25));
    }

    #[test]
    fn recording_quiet_loopback_updates_diagnostics_only() {
        let state = RecordingState::new();
        state.record_system_silence();
        state.record_system_silence();
        let health = state.health.lock().unwrap();
        assert_eq!(health.details.system_silence_periods, 2);
        assert_eq!(health.details.dropped_samples, 0);
        assert!(health.warnings.is_empty());
        assert!(!state.capture_incomplete());
    }

    #[test]
    fn controls_levels_expire_without_changing_which_sources_are_captured() {
        use crate::audio::recording_levels::{RecordingLevels, SourceLevel};
        let state = RecordingState::new();
        let levels = RecordingLevels {
            mic: Some(SourceLevel {
                rms: 0.2,
                peak: 0.4,
            }),
            system: None,
        };
        assert_eq!(state.live_levels().0, None);
        state.report_levels(&levels);
        assert_eq!(state.live_levels().0, Some(levels));
        state.latest_levels.lock().unwrap().latest =
            Some((Instant::now() - Duration::from_secs(1), levels));
        let stale = state.live_levels().0.unwrap();
        assert_eq!(
            stale.mic,
            Some(SourceLevel {
                rms: 0.0,
                peak: 0.0
            })
        );
        assert_eq!(stale.system, None);
        state.start_recording().unwrap();
        assert_eq!(state.live_levels().0, None);
    }

    #[test]
    fn floating_microphone_warning_follows_capture_even_before_the_window_opens() {
        use crate::audio::recording_levels::SourceLevel;
        let state = RecordingState::new();
        state.start_recording().unwrap();
        let now = Instant::now();
        let quiet = RecordingLevels {
            mic: Some(SourceLevel {
                rms: 0.0,
                peak: 0.0,
            }),
            system: None,
        };
        {
            let mut readings = state.latest_levels.lock().unwrap();
            readings.observe(quiet, now);
            assert!(!readings.mic_no_signal(now + Duration::from_millis(9_999)));
            assert!(readings.mic_no_signal(now + Duration::from_secs(10)));
            readings.observe(
                RecordingLevels {
                    mic: Some(SourceLevel {
                        rms: 0.0,
                        peak: 0.1,
                    }),
                    system: None,
                },
                now + Duration::from_secs(11),
            );
            readings.observe(quiet, now + Duration::from_secs(12));
            assert!(!readings.mic_no_signal(now + Duration::from_secs(30)));
        }
        state.pause_recording().unwrap();
        assert!(!state.live_levels().1);
        state.resume_recording().unwrap();
        let mut readings = state.latest_levels.lock().unwrap();
        readings.observe(quiet, now + Duration::from_secs(40));
        assert!(!readings.mic_no_signal(now + Duration::from_millis(49_999)));
        assert!(readings.mic_no_signal(now + Duration::from_secs(50)));
    }

    #[tokio::test]
    async fn capture_overrun_preserves_accepted_audio_and_allows_a_new_session() {
        let state = RecordingState::new();
        let (sender, mut receiver) = mpsc::channel(1);
        state.set_audio_sender(sender);
        state.start_recording().unwrap();
        let chunk = AudioChunk {
            data: vec![0.25; 480],
            sample_rate: 48000,
            timestamp: 0.0,
            chunk_id: 0,
            device_type: DeviceType::System,
        };
        state.send_audio_chunk(chunk.clone()).unwrap();
        state.send_audio_chunk(chunk.clone()).unwrap();
        assert!(state.is_recording());
        assert!(state.capture_incomplete());
        assert_eq!(receiver.recv().await.unwrap().data, chunk.data);
        state.send_audio_chunk(chunk.clone()).unwrap();
        assert_eq!(receiver.recv().await.unwrap().data, chunk.data);
        state.stop_recording();
        assert!(receiver.recv().await.is_none());

        let (sender, mut receiver) = mpsc::channel(1);
        state.set_audio_sender(sender);
        state.start_recording().unwrap();
        assert!(!state.capture_incomplete());
        state.send_audio_chunk(chunk).unwrap();
        state.stop_recording();
        assert!(receiver.recv().await.is_some());
        assert!(receiver.recv().await.is_none());
    }

    #[test]
    fn isolated_errors_do_not_accumulate_but_recent_bursts_do() {
        let state = RecordingState::new();
        let now = Instant::now();
        for index in 0..100 {
            assert_eq!(
                state.record_recent_error(now + std::time::Duration::from_secs(index * 31)),
                1
            );
        }
        let later = now + std::time::Duration::from_secs(3200);
        for count in 1..=10 {
            assert_eq!(state.record_recent_error(later), count);
        }
    }

    #[test]
    fn stopped_duration_is_frozen_until_cleanup() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        *state.recording_start.lock().unwrap() =
            Some(Instant::now() - std::time::Duration::from_secs(90));
        state.pause_recording().unwrap();
        *state.pause_start.lock().unwrap() =
            Some(Instant::now() - std::time::Duration::from_secs(10));
        state.stop_recording();
        let active = state.get_active_recording_duration().unwrap();
        assert!((active - 80.0).abs() < 0.1);
        state.stop_recording();
        assert_eq!(state.get_active_recording_duration().unwrap(), active);
        state.cleanup();
        assert_eq!(state.get_active_recording_duration(), None);
    }
}
