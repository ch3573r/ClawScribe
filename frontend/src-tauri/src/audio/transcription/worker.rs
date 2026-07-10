// audio/transcription/worker.rs
//
// Parallel transcription worker pool and chunk processing logic.

use super::engine::TranscriptionEngine;
use super::provider::TranscriptionError;
use super::queue::{
    QueuedAudioChunk, TranscriptionMetrics, TranscriptionMetricsSnapshot,
    TranscriptionQueueReceiver,
};
use crate::api::TranscriptWord;
use crate::audio::AudioChunk;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Runtime};
use tokio_util::sync::CancellationToken;

// Sequence counter for transcript updates
static SEQUENCE_COUNTER: AtomicU64 = AtomicU64::new(0);

// Speech detection flag - reset per recording session
static SPEECH_DETECTED_EMITTED: AtomicBool = AtomicBool::new(false);

/// Beta (opt-in, default off): energy-based "Me"/"Participants" source
/// attribution. Set via `set_source_attribution_enabled`. When off, segments
/// carry no speaker label so nothing mislabels who spoke.
pub static SOURCE_ATTRIBUTION_ENABLED: AtomicBool = AtomicBool::new(false);

static ACTIVE_TRANSCRIPTION_METRICS: Lazy<StdMutex<Option<Arc<TranscriptionMetrics>>>> =
    Lazy::new(|| StdMutex::new(None));

pub struct TranscriptionTask {
    pub handle: tokio::task::JoinHandle<()>,
    cancellation: CancellationToken,
    metrics: Arc<TranscriptionMetrics>,
}

impl TranscriptionTask {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn mark_stopped(&self) {
        self.metrics.set_worker_active(false);
        if let Ok(mut active_metrics) = ACTIVE_TRANSCRIPTION_METRICS.lock() {
            clear_metrics_if_current(&mut active_metrics, &self.metrics);
        }
    }
}

fn clear_metrics_if_current(
    active_metrics: &mut Option<Arc<TranscriptionMetrics>>,
    completed_metrics: &Arc<TranscriptionMetrics>,
) -> bool {
    if active_metrics
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, completed_metrics))
    {
        *active_metrics = None;
        true
    } else {
        false
    }
}

pub fn current_transcription_metrics() -> Option<TranscriptionMetricsSnapshot> {
    ACTIVE_TRANSCRIPTION_METRICS
        .lock()
        .ok()
        .and_then(|metrics| metrics.as_ref().map(|metrics| metrics.snapshot()))
}

struct ProcessingMetricsGuard {
    metrics: Arc<TranscriptionMetrics>,
    audio_ms: u64,
    spool_bytes: u64,
    started: Instant,
    completed: bool,
}

impl ProcessingMetricsGuard {
    fn new(metrics: Arc<TranscriptionMetrics>, item: &QueuedAudioChunk) -> Self {
        metrics.mark_processing_started();
        Self {
            metrics,
            audio_ms: item.audio_ms(),
            spool_bytes: item.spool_bytes(),
            started: Instant::now(),
            completed: false,
        }
    }

    fn complete(&mut self) {
        if !self.completed {
            self.metrics
                .mark_completed(self.audio_ms, self.spool_bytes, self.started.elapsed());
            self.completed = true;
        }
    }
}

impl Drop for ProcessingMetricsGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.metrics.mark_processing_stopped();
        }
    }
}

/// Reset the speech detected flag for a new recording session
pub fn reset_speech_detected_flag() {
    SPEECH_DETECTED_EMITTED.store(false, Ordering::SeqCst);
    info!(
        "🔍 SPEECH_DETECTED_EMITTED reset to: {}",
        SPEECH_DETECTED_EMITTED.load(Ordering::SeqCst)
    );
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranscriptUpdate {
    pub text: String,
    pub timestamp: String, // Wall-clock time for reference (e.g., "14:30:05")
    pub source: String,
    pub sequence_id: u64,
    pub chunk_start_time: f64, // Legacy field, kept for compatibility
    pub is_partial: bool,
    pub confidence: f32,
    // NEW: Recording-relative timestamps for playback sync
    pub audio_start_time: f64, // Seconds from recording start (e.g., 125.3)
    pub audio_end_time: f64,   // Seconds from recording start (e.g., 128.6)
    pub duration: f64,         // Segment duration in seconds (e.g., 3.3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_timestamps: Option<Vec<TranscriptWord>>,
}

// NOTE: get_transcript_history and get_recording_meeting_name functions
// have been moved to recording_commands.rs where they have access to RECORDING_MANAGER

/// Start the serial, memory-bounded transcription task.
pub fn start_transcription_task<R: Runtime>(
    app: AppHandle<R>,
    transcription_receiver: TranscriptionQueueReceiver,
) -> TranscriptionTask {
    let metrics = transcription_receiver.metrics();
    metrics.set_worker_active(true);
    if let Ok(mut active_metrics) = ACTIVE_TRANSCRIPTION_METRICS.lock() {
        *active_metrics = Some(metrics.clone());
    }
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task_metrics = metrics.clone();
    let handle = tokio::spawn(async move {
        info!("Starting memory-bounded transcription task");

        // Initialize transcription engine (Whisper or Parakeet based on config)
        let transcription_engine = match super::engine::get_or_init_transcription_engine(&app).await
        {
            Ok(engine) => engine,
            Err(e) => {
                error!("Failed to initialize transcription engine: {}", e);
                let _ = app.emit("transcription-error", serde_json::json!({
                    "error": e,
                    "userMessage": "Recording failed: Unable to initialize speech recognition. Please check your model settings.",
                    "actionable": true
                }));
                task_metrics.set_worker_active(false);
                let _ = app.emit(
                    "transcription-complete",
                    serde_json::json!({
                        "cancelled": false,
                        "failed": true,
                        "error": e
                    }),
                );
                return;
            }
        };

        // Serial processing keeps transcript emission in chronological order.
        const NUM_WORKERS: usize = 1;
        // A single buffered item plus the item currently in inference bounds the
        // worker-side memory. The remaining backlog stays in the disk queue.
        let (work_sender, work_receiver) = tokio::sync::mpsc::channel::<QueuedAudioChunk>(1);
        let work_receiver = Arc::new(tokio::sync::Mutex::new(work_receiver));

        // Track completion: AtomicU64 for chunks queued, AtomicU64 for chunks completed
        let chunks_queued = Arc::new(AtomicU64::new(0));
        let chunks_completed = Arc::new(AtomicU64::new(0));
        let input_finished = Arc::new(AtomicBool::new(false));

        info!(
            "📊 Starting {} transcription worker{} (serial mode for ordered emission)",
            NUM_WORKERS,
            if NUM_WORKERS == 1 { "" } else { "s" }
        );

        // Spawn worker tasks
        let mut worker_handles = tokio::task::JoinSet::new();
        for worker_id in 0..NUM_WORKERS {
            let engine_clone = match &transcription_engine {
                TranscriptionEngine::Whisper(e) => TranscriptionEngine::Whisper(e.clone()),
                TranscriptionEngine::Parakeet(e) => TranscriptionEngine::Parakeet(e.clone()),
                TranscriptionEngine::Provider(p) => TranscriptionEngine::Provider(p.clone()),
            };
            let app_clone = app.clone();
            let work_receiver_clone = work_receiver.clone();
            let chunks_completed_clone = chunks_completed.clone();
            let input_finished_clone = input_finished.clone();
            let chunks_queued_clone = chunks_queued.clone();
            let metrics_clone = task_metrics.clone();

            worker_handles.spawn(async move {
                info!("👷 Worker {} started", worker_id);

                // PRE-VALIDATE model state to avoid repeated async calls per chunk
                let initial_model_loaded = engine_clone.is_model_loaded().await;
                let current_model = engine_clone
                    .get_current_model()
                    .await
                    .unwrap_or_else(|| "unknown".to_string());

                let engine_name = engine_clone.provider_name();

                if initial_model_loaded {
                    info!(
                        "✅ Worker {} pre-validation: {} model '{}' is loaded and ready",
                        worker_id, engine_name, current_model
                    );
                } else {
                    warn!(
                        "⚠️ Worker {} pre-validation: {} model not loaded - chunks may be skipped",
                        worker_id, engine_name
                    );
                }

                loop {
                    // Try to get a chunk to process
                    let chunk = {
                        let mut receiver = work_receiver_clone.lock().await;
                        receiver.recv().await
                    };

                    match chunk {
                        Some(queued_chunk) => {
                            let mut metrics_guard =
                                ProcessingMetricsGuard::new(metrics_clone.clone(), &queued_chunk);
                            let chunk = queued_chunk.chunk;
                            // PERFORMANCE OPTIMIZATION: Reduce logging in hot path
                            // Only log every 10th chunk per worker to reduce I/O overhead
                            let should_log_this_chunk = chunk.chunk_id % 10 == 0;

                            if should_log_this_chunk {
                                info!(
                                    "👷 Worker {} processing chunk {} with {} samples",
                                    worker_id,
                                    chunk.chunk_id,
                                    chunk.data.len()
                                );
                            }

                            // Check if model is still loaded before processing
                            if !engine_clone.is_model_loaded().await {
                                warn!("⚠️ Worker {}: Model unloaded, but continuing to preserve chunk {}", worker_id, chunk.chunk_id);
                                // Still count as completed even if we can't process
                                chunks_completed_clone.fetch_add(1, Ordering::SeqCst);
                                metrics_guard.complete();
                                continue;
                            }

                            let chunk_timestamp = chunk.timestamp;
                            let chunk_duration = chunk.data.len() as f64 / chunk.sample_rate as f64;

                            // Speaker label from the segment's dominant source (set
                            // by the pipeline): mic = "Me", system audio = "Participants".
                            // Gated behind a Beta toggle (default off) — the energy
                            // heuristic isn't reliable yet, so when disabled we emit
                            // no label rather than a wrong one.
                            let source_label = if SOURCE_ATTRIBUTION_ENABLED.load(Ordering::Relaxed)
                            {
                                match &chunk.device_type {
                                    crate::audio::recording_state::DeviceType::System => {
                                        "Participants"
                                    }
                                    _ => "Me",
                                }
                            } else {
                                ""
                            };

                            // Transcribe with provider-agnostic approach
                            match transcribe_chunk_with_provider(&engine_clone, chunk, &app_clone)
                                .await
                            {
                                Ok((
                                    transcript,
                                    confidence_opt,
                                    is_partial,
                                    provider_word_timestamps,
                                )) => {
                                    // Provider-aware confidence threshold
                                    let confidence_threshold = match &engine_clone {
                                        TranscriptionEngine::Whisper(_)
                                        | TranscriptionEngine::Provider(_) => 0.3,
                                        TranscriptionEngine::Parakeet(_) => 0.0, // Parakeet has no confidence, accept all
                                    };

                                    let confidence_str = match confidence_opt {
                                        Some(c) => format!("{:.2}", c),
                                        None => "N/A".to_string(),
                                    };

                                    debug!("Worker {} transcription result: text='{}', confidence={}, partial={}, threshold={:.2}",
                                          worker_id, transcript, confidence_str, is_partial, confidence_threshold);

                                    // Check confidence threshold (or accept if no confidence provided)
                                    let meets_threshold =
                                        confidence_opt.map_or(true, |c| c >= confidence_threshold);

                                    if !transcript.trim().is_empty() && meets_threshold {
                                        // PERFORMANCE: Only log transcription results, not every processing step
                                        debug!("Worker {} transcribed: {} (confidence: {}, partial: {})",
                                              worker_id, transcript, confidence_str, is_partial);

                                        // Emit speech-detected event for frontend UX (only on first detection per session)
                                        // This is lightweight and provides better user feedback
                                        let current_flag =
                                            SPEECH_DETECTED_EMITTED.load(Ordering::SeqCst);
                                        debug!("Checking speech-detected flag: current={}, will_emit={}", current_flag, !current_flag);

                                        if !current_flag {
                                            SPEECH_DETECTED_EMITTED.store(true, Ordering::SeqCst);
                                            match app_clone.emit("speech-detected", serde_json::json!({
                                                "message": "Speech activity detected"
                                            })) {
                                                Ok(_) => info!("🎤 ✅ First speech detected - successfully emitted speech-detected event"),
                                                Err(e) => error!("🎤 ❌ Failed to emit speech-detected event: {}", e),
                                            }
                                        }

                                        // Generate sequence ID and calculate timestamps FIRST
                                        let sequence_id =
                                            SEQUENCE_COUNTER.fetch_add(1, Ordering::SeqCst);
                                        let audio_start_time = chunk_timestamp; // Already in seconds from recording start
                                        let audio_end_time = chunk_timestamp + chunk_duration;
                                        let source_label_opt = if source_label.is_empty() {
                                            None
                                        } else {
                                            Some(source_label)
                                        };
                                        let word_timestamps = provider_word_timestamps
                                            .map(|words| with_word_speaker(words, source_label_opt))
                                            .or_else(|| {
                                                crate::audio::common::estimate_word_timestamps(
                                                    &transcript,
                                                    audio_start_time,
                                                    audio_end_time,
                                                    confidence_opt,
                                                    source_label_opt,
                                                )
                                            });

                                        // Save structured transcript segment to recording manager (only final results)
                                        // Save ALL segments (partial and final) to ensure complete JSON
                                        // Create structured segment with full timestamp data
                                        // NOTE: This is now handled via the transcript-update event emission below
                                        // The recording_commands module listens to these events and saves them
                                        // This decouples the transcription worker from direct RECORDING_MANAGER access

                                        // Emit transcript update with NEW recording-relative timestamps

                                        let update = TranscriptUpdate {
                                            text: transcript,
                                            timestamp: format_current_timestamp(), // Wall-clock for reference
                                            source: source_label.to_string(),
                                            sequence_id,
                                            chunk_start_time: chunk_timestamp, // Legacy compatibility
                                            is_partial,
                                            confidence: confidence_opt.unwrap_or(0.85), // Default for providers without confidence
                                            // NEW: Recording-relative timestamps for sync
                                            audio_start_time,
                                            audio_end_time,
                                            duration: chunk_duration,
                                            word_timestamps,
                                        };

                                        if let Err(e) = app_clone.emit("transcript-update", &update)
                                        {
                                            error!(
                                                "Worker {}: Failed to emit transcript update: {}",
                                                worker_id, e
                                            );
                                        }
                                        // PERFORMANCE: Removed verbose logging of every emission
                                    } else if !transcript.trim().is_empty() && should_log_this_chunk
                                    {
                                        // PERFORMANCE: Only log low-confidence results occasionally
                                        if let Some(c) = confidence_opt {
                                            info!("Worker {} low-confidence transcription (confidence: {:.2}), skipping", worker_id, c);
                                        }
                                    }
                                }
                                Err(e) => {
                                    // Improved error handling with specific cases
                                    match e {
                                        TranscriptionError::AudioTooShort { .. } => {
                                            // Skip silently, this is expected for very short chunks
                                            info!("Worker {}: {}", worker_id, e);
                                            chunks_completed_clone.fetch_add(1, Ordering::SeqCst);
                                            metrics_guard.complete();
                                            continue;
                                        }
                                        TranscriptionError::ModelNotLoaded => {
                                            warn!(
                                                "Worker {}: Model unloaded during transcription",
                                                worker_id
                                            );
                                            chunks_completed_clone.fetch_add(1, Ordering::SeqCst);
                                            metrics_guard.complete();
                                            continue;
                                        }
                                        _ => {
                                            warn!(
                                                "Worker {}: Transcription failed: {}",
                                                worker_id, e
                                            );
                                            let _ = app_clone
                                                .emit("transcription-warning", e.to_string());
                                        }
                                    }
                                }
                            }

                            // Mark chunk as completed
                            let completed =
                                chunks_completed_clone.fetch_add(1, Ordering::SeqCst) + 1;
                            metrics_guard.complete();
                            let queued = chunks_queued_clone.load(Ordering::SeqCst);

                            // PERFORMANCE: Only log progress every 5th chunk to reduce I/O overhead
                            if completed % 5 == 0 || should_log_this_chunk {
                                info!(
                                    "Worker {}: Progress {}/{} chunks ({:.1}%)",
                                    worker_id,
                                    completed,
                                    queued,
                                    (completed as f64 / queued.max(1) as f64 * 100.0)
                                );
                            }

                            // Avoid a WebView event and serialization round-trip
                            // for every segment. Exact status remains available
                            // through get_transcription_status.
                            if completed % 5 == 0 || should_log_this_chunk {
                                let progress_percentage = if queued > 0 {
                                    (completed as f64 / queued as f64 * 100.0) as u32
                                } else {
                                    100
                                };

                                let _ = app_clone.emit("transcription-progress", serde_json::json!({
                                    "worker_id": worker_id,
                                    "chunks_completed": completed,
                                    "chunks_queued": queued,
                                    "progress_percentage": progress_percentage,
                                    "message": format!("Worker {} processing... ({}/{})", worker_id, completed, queued)
                                }));
                            }
                        }
                        None => {
                            // No more chunks available
                            if input_finished_clone.load(Ordering::SeqCst) {
                                // Double-check that all queued chunks are actually completed
                                let final_queued = chunks_queued_clone.load(Ordering::SeqCst);
                                let final_completed = chunks_completed_clone.load(Ordering::SeqCst);

                                if final_completed >= final_queued {
                                    info!(
                                        "👷 Worker {} finishing - all {}/{} chunks processed",
                                        worker_id, final_completed, final_queued
                                    );
                                    break;
                                } else {
                                    warn!("👷 Worker {} detected potential chunk loss: {}/{} completed, waiting...", worker_id, final_completed, final_queued);
                                    // AGGRESSIVE POLLING: Reduced from 50ms to 5ms for faster chunk detection during shutdown
                                    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                                }
                            } else {
                                // AGGRESSIVE POLLING: Reduced from 10ms to 1ms for faster response during shutdown
                                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
                            }
                        }
                    }
                }

                info!("👷 Worker {} completed", worker_id);
            });
        }

        // Main dispatcher: receive chunks and distribute to workers
        let mut receiver = transcription_receiver;
        loop {
            let next_item = tokio::select! {
                _ = task_cancellation.cancelled() => break,
                item = receiver.recv() => item,
            };

            match next_item {
                Ok(Some(item)) => {
                    let queued = chunks_queued.fetch_add(1, Ordering::SeqCst) + 1;
                    debug!(
                        "Dispatching chunk {} to transcription worker (total queued: {})",
                        item.chunk.chunk_id, queued
                    );
                    let send_result = tokio::select! {
                        _ = task_cancellation.cancelled() => break,
                        result = work_sender.send(item) => result,
                    };
                    if send_result.is_err() {
                        error!("Failed to send chunk to transcription worker");
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    error!("Failed to read transcription queue: {}", error);
                    let _ = app.emit("transcription-error", serde_json::json!({
                        "error": error.to_string(),
                        "userMessage": "A staged audio segment could not be read. The recording audio is still preserved.",
                        "actionable": false
                    }));
                    break;
                }
            }
        }

        // Signal that input is finished
        input_finished.store(true, Ordering::SeqCst);
        drop(work_sender); // Close the channel to signal workers

        let total_chunks_queued = chunks_queued.load(Ordering::SeqCst);
        info!("📭 Input finished with {} total chunks queued. Waiting for all {} workers to complete...",
              total_chunks_queued, NUM_WORKERS);

        // Emit final chunk count to frontend
        let _ = app.emit("transcription-queue-complete", serde_json::json!({
            "total_chunks": total_chunks_queued,
            "message": format!("{} chunks queued for processing - waiting for completion", total_chunks_queued)
        }));

        // Wait for all workers to complete
        let mut finished_worker_id = 0usize;
        while let Some(result) = worker_handles.join_next().await {
            if let Err(e) = result {
                error!("❌ Worker {} panicked: {:?}", finished_worker_id, e);
            } else {
                info!("✅ Worker {} completed successfully", finished_worker_id);
            }
            finished_worker_id += 1;
        }

        // Final verification with retry logic to catch any stragglers
        let mut verification_attempts = 0;
        const MAX_VERIFICATION_ATTEMPTS: u32 = 10;

        loop {
            let final_queued = chunks_queued.load(Ordering::SeqCst);
            let final_completed = chunks_completed.load(Ordering::SeqCst);

            if task_cancellation.is_cancelled() {
                warn!(
                    "Transcription cancelled with {}/{} dispatched chunks completed",
                    final_completed, final_queued
                );
                break;
            } else if final_queued == final_completed {
                info!(
                    "🎉 ALL {} chunks processed successfully - ZERO chunks lost!",
                    final_completed
                );
                break;
            } else if verification_attempts < MAX_VERIFICATION_ATTEMPTS {
                verification_attempts += 1;
                warn!("⚠️ Chunk count mismatch (attempt {}): {} queued, {} completed - waiting for stragglers...",
                     verification_attempts, final_queued, final_completed);

                // Wait a bit for any remaining chunks to be processed
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            } else {
                error!(
                    "❌ CRITICAL: After {} attempts, chunk loss detected: {} queued, {} completed",
                    MAX_VERIFICATION_ATTEMPTS, final_queued, final_completed
                );

                // Emit critical error event
                let _ = app.emit(
                    "transcript-chunk-loss-detected",
                    serde_json::json!({
                        "chunks_queued": final_queued,
                        "chunks_completed": final_completed,
                        "chunks_lost": final_queued - final_completed,
                        "message": "Some transcript chunks may have been lost during shutdown"
                    }),
                );
                break;
            }
        }

        task_metrics.set_worker_active(false);
        let status = task_metrics.snapshot();
        let _ = app.emit(
            "transcription-complete",
            serde_json::json!({
                "cancelled": task_cancellation.is_cancelled(),
                "failed": false,
                "chunks_remaining": status.chunks_in_queue,
                "chunks_completed": status.total_chunks_completed
            }),
        );
        info!("✅ Transcription task completed - worker stopped, ready for model unload");
    });

    TranscriptionTask {
        handle,
        cancellation,
        metrics,
    }
}

/// Transcribe audio chunk using the appropriate provider (Whisper, Parakeet, or trait-based)
/// Returns: (text, confidence Option, is_partial)
async fn transcribe_chunk_with_provider<R: Runtime>(
    engine: &TranscriptionEngine,
    chunk: AudioChunk,
    app: &AppHandle<R>,
) -> std::result::Result<(String, Option<f32>, bool, Option<Vec<TranscriptWord>>), TranscriptionError>
{
    let chunk_start_time = chunk.timestamp;
    let chunk_duration = chunk.data.len() as f64 / chunk.sample_rate as f64;
    let chunk_end_time = chunk_start_time + chunk_duration;

    // Convert to 16kHz mono for transcription
    let transcription_data = if chunk.sample_rate != 16000 {
        crate::audio::audio_processing::resample_audio(&chunk.data, chunk.sample_rate, 16000)
    } else {
        chunk.data
    };

    // Skip VAD processing here since the pipeline already extracted speech using VAD
    let speech_samples = transcription_data;

    // Check for empty samples - improved error handling
    if speech_samples.is_empty() {
        warn!(
            "Audio chunk {} is empty, skipping transcription",
            chunk.chunk_id
        );
        return Err(TranscriptionError::AudioTooShort {
            samples: 0,
            minimum: 1600, // 100ms at 16kHz
        });
    }

    // Calculate energy for logging/monitoring only
    let energy: f32 =
        speech_samples.iter().map(|&x| x * x).sum::<f32>() / speech_samples.len() as f32;
    debug!(
        "Processing speech audio chunk {} with {} samples (energy: {:.6})",
        chunk.chunk_id,
        speech_samples.len(),
        energy
    );

    // Transcribe using the appropriate engine (with improved error handling)
    match engine {
        TranscriptionEngine::Whisper(whisper_engine) => {
            // Get language preference from global state
            let language = crate::get_language_preference_internal();

            match whisper_engine
                .transcribe_audio_with_confidence(speech_samples, language)
                .await
            {
                Ok((text, confidence, is_partial)) => {
                    let cleaned_text = text.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((String::new(), Some(confidence), is_partial, None));
                    }

                    debug!(
                        "Whisper transcription complete for chunk {}: '{}' (confidence: {:.2}, partial: {})",
                        chunk.chunk_id, cleaned_text, confidence, is_partial
                    );

                    Ok((cleaned_text, Some(confidence), is_partial, None))
                }
                Err(e) => {
                    error!(
                        "Whisper transcription failed for chunk {}: {}",
                        chunk.chunk_id, e
                    );

                    let transcription_error = TranscriptionError::EngineFailed(e.to_string());
                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": transcription_error.to_string(),
                            "userMessage": format!("Transcription failed: {}", transcription_error),
                            "actionable": false
                        }),
                    );

                    Err(transcription_error)
                }
            }
        }
        TranscriptionEngine::Parakeet(parakeet_engine) => {
            match parakeet_engine
                .transcribe_audio_timestamped(speech_samples)
                .await
            {
                Ok(result) => {
                    let cleaned_text = result.text.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((String::new(), None, false, None));
                    }

                    debug!(
                        "Parakeet transcription complete for chunk {}: '{}'",
                        chunk.chunk_id, cleaned_text
                    );

                    let word_timestamps =
                        crate::audio::common::transcript_words_from_token_timestamps(
                            &cleaned_text,
                            &result.tokens,
                            &result.timestamps,
                            chunk_start_time,
                            chunk_end_time,
                            None,
                            None,
                        );

                    Ok((cleaned_text, None, false, word_timestamps))
                }
                Err(e) => {
                    error!(
                        "Parakeet transcription failed for chunk {}: {}",
                        chunk.chunk_id, e
                    );

                    let transcription_error = TranscriptionError::EngineFailed(e.to_string());
                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": transcription_error.to_string(),
                            "userMessage": format!("Transcription failed: {}", transcription_error),
                            "actionable": false
                        }),
                    );

                    Err(transcription_error)
                }
            }
        }
        TranscriptionEngine::Provider(provider) => {
            // NEW: Trait-based provider (clean, unified interface)
            let language = crate::get_language_preference_internal();

            match provider.transcribe(speech_samples, language).await {
                Ok(result) => {
                    let cleaned_text = result.text.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((String::new(), result.confidence, result.is_partial, None));
                    }

                    let confidence_str = match result.confidence {
                        Some(c) => format!("confidence: {:.2}", c),
                        None => "no confidence".to_string(),
                    };

                    debug!(
                        "{} transcription complete for chunk {}: '{}' ({}, partial: {})",
                        provider.provider_name(),
                        chunk.chunk_id,
                        cleaned_text,
                        confidence_str,
                        result.is_partial
                    );

                    Ok((
                        cleaned_text,
                        result.confidence,
                        result.is_partial,
                        result.word_timestamps,
                    ))
                }
                Err(e) => {
                    error!(
                        "{} transcription failed for chunk {}: {}",
                        provider.provider_name(),
                        chunk.chunk_id,
                        e
                    );

                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": e.to_string(),
                            "userMessage": format!("Transcription failed: {}", e),
                            "actionable": false
                        }),
                    );

                    Err(e)
                }
            }
        }
    }
}

/// Format current timestamp (wall-clock time)
fn format_current_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    let hours = (now.as_secs() / 3600) % 24;
    let minutes = (now.as_secs() / 60) % 60;
    let seconds = now.as_secs() % 60;

    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

fn with_word_speaker(words: Vec<TranscriptWord>, speaker: Option<&str>) -> Vec<TranscriptWord> {
    let Some(speaker) = speaker.map(str::to_string) else {
        return words;
    };

    words
        .into_iter()
        .map(|mut word| {
            word.speaker = Some(speaker.clone());
            word
        })
        .collect()
}

/// Format recording-relative time as [MM:SS]
#[allow(dead_code)]
fn format_recording_time(seconds: f64) -> String {
    let total_seconds = seconds.floor() as u64;
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;

    format!("[{:02}:{:02}]", minutes, secs)
}

#[cfg(test)]
mod metrics_lifecycle_tests {
    use super::*;

    #[test]
    fn completed_session_only_clears_its_own_metrics() {
        let completed = Arc::new(TranscriptionMetrics::default());
        let replacement = Arc::new(TranscriptionMetrics::default());
        let mut active = Some(replacement.clone());

        assert!(!clear_metrics_if_current(&mut active, &completed));
        assert!(active
            .as_ref()
            .is_some_and(|metrics| Arc::ptr_eq(metrics, &replacement)));
        assert!(clear_metrics_if_current(&mut active, &replacement));
        assert!(active.is_none());
    }
}
