use anyhow::Result;
use log::{debug, error, info, warn};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::audio_processing::{
    audio_to_mono, HighPassFilter, LoudnessNormalizer, NoiseSuppressionProcessor,
};
use super::devices::AudioDevice;
use super::recording_state::{AudioChunk, AudioError, DeviceType, RecordingState};
use super::transcription::queue::{BufferedSpool, TranscriptionQueueSender};
use super::vad::ContinuousVadProcessor;

#[cfg(test)]
#[path = "capture_bench.rs"]
mod capture_bench;

/// Live VAD max-segment cap (ms). Set per active transcription provider at
/// recording start; the pipeline reads it when creating the VAD. Default 6s.
static LIVE_MAX_SEGMENT_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(6_000);
const LIVE_VAD_REDEMPTION_MS: u32 = 400;
const TRANSCRIPTION_QUEUE_WARNING_THRESHOLD: u32 = 3;
const MICROPHONE_STREAM_WARNING: &str =
    "The selected microphone is not sending audio. Check the microphone connection and recording devices.";

#[derive(Default)]
struct MicrophoneStreamWatch {
    last_samples: Option<std::time::Instant>,
    warned: bool,
}

impl MicrophoneStreamWatch {
    /// A silent microphone still delivers samples. Judge delivery rather than amplitude.
    fn update(&mut self, now: std::time::Instant, active: bool, received: bool) -> Option<bool> {
        if !active {
            self.last_samples = None;
        } else if received || self.last_samples.is_none() {
            self.last_samples = Some(now);
        }
        let stalled = self.last_samples.is_some_and(|last| {
            now.saturating_duration_since(last) >= std::time::Duration::from_secs(15)
        });
        if self.warned == stalled {
            return None;
        }
        self.warned = stalled;
        Some(stalled)
    }
}

#[derive(Default)]
struct QueueWriteFailureTracker {
    consecutive: u32,
}

impl QueueWriteFailureTracker {
    fn record_failure(&mut self) -> bool {
        self.consecutive = self.consecutive.saturating_add(1);
        self.consecutive == TRANSCRIPTION_QUEUE_WARNING_THRESHOLD
    }

    fn record_success(&mut self) {
        self.consecutive = 0;
    }
}

#[derive(Default)]
struct SystemAudioSilenceTracker {
    last_signal: Option<std::time::Instant>,
    quiet: bool,
}

impl SystemAudioSilenceTracker {
    /// Returns true once per quiet period. WASAPI loopback can stop delivering
    /// packets during normal silence, so this observation is diagnostic only.
    fn update(&mut self, now: std::time::Instant, active: bool, audible: bool) -> bool {
        if !active {
            self.last_signal = None;
        } else if audible || self.last_signal.is_none() {
            self.last_signal = Some(now);
        }
        let silent = self.last_signal.is_some_and(|last| {
            now.saturating_duration_since(last) >= std::time::Duration::from_secs(15)
        });
        let began = silent && !self.quiet;
        self.quiet = silent;
        began
    }
}

fn audible_system_audio(device_type: &DeviceType, samples: &[f32]) -> bool {
    *device_type == DeviceType::System && samples.iter().any(|sample| sample.abs() > 0.001)
}

/// Choose the live VAD segment cap for the active transcription provider.
/// Nemotron transcribes each VAD segment offline at only ~2.5x realtime, so a 6s
/// cap can leave continuous speech several seconds behind; cap its live segments
/// shorter (3s) to cut perceived latency. Whisper and cloud providers keep 6s.
/// Import/retranscription is unaffected — it uses silence-aware splitting, not
/// this cap.
///
/// Parakeet gets a longer cap (12s). It decodes far faster than realtime, and
/// it is unreliable on short pieces of a sentence: in `live_cut_bench` some
/// segments under 6s decoded to no text at all, while none of 8s or more did.
/// The price is that uninterrupted speech can appear up to 12s late.
/// Cutting earlier at a pause was measured with `live_cut_bench` and made Parakeet
/// less accurate, so the cap is the only limit.
pub fn set_live_max_segment_ms_for_provider(provider: &str) {
    LIVE_MAX_SEGMENT_MS.store(
        live_max_segment_ms_for_provider(provider),
        std::sync::atomic::Ordering::Relaxed,
    );
}

fn live_max_segment_ms_for_provider(provider: &str) -> u32 {
    if provider.eq_ignore_ascii_case("nemotron") {
        3_000
    } else if provider.eq_ignore_ascii_case("parakeet") {
        12_000
    } else {
        6_000
    }
}

struct SourceAttributionWindow {
    start_timestamp_ms: f64,
    end_timestamp_ms: f64,
    mic_snr: f32,
    sys_snr: f32,
}

/// Mixer-window ratios on the VAD's sample-count timeline. Retain the tail
/// after a cut, but bound storage even during long stretches without speech.
#[derive(Default)]
struct SourceAttributionHistory {
    windows: VecDeque<SourceAttributionWindow>,
    input_samples: u64,
}

impl SourceAttributionHistory {
    // A language hint may affect later words, so require consistent speech on
    // one input. The optional speaker labels use their existing, looser rule.
    fn confidently_matches(&self, source: &DeviceType, start_ms: f64, end_ms: f64) -> bool {
        let duration = end_ms - start_ms;
        if duration <= 0.0 {
            return false;
        }
        let mut clear_ms = 0.0;
        let mut covered_ms = 0.0;
        for window in &self.windows {
            let overlap =
                end_ms.min(window.end_timestamp_ms) - start_ms.max(window.start_timestamp_ms);
            if overlap <= 0.0 {
                continue;
            }
            covered_ms += overlap;
            let (signal, other) = match source {
                DeviceType::Microphone => (window.mic_snr, window.sys_snr),
                DeviceType::System => (window.sys_snr, window.mic_snr),
            };
            if !signal.is_finite() || !other.is_finite() || other >= 3.0 {
                return false;
            }
            if signal >= 3.0 && signal >= other * 4.0 {
                clear_ms += overlap;
            }
        }
        covered_ms >= 0.9 * duration && clear_ms >= 0.5 * duration
    }

    fn record_window(&mut self, sample_count: usize, sample_rate: u32, mic_snr: f32, sys_snr: f32) {
        if sample_count == 0 {
            return;
        }
        let start_timestamp_ms = self.input_samples as f64 / sample_rate as f64 * 1000.0;
        self.input_samples += sample_count as u64;
        let end_timestamp_ms = self.input_samples as f64 / sample_rate as f64 * 1000.0;
        self.windows.push_back(SourceAttributionWindow {
            start_timestamp_ms,
            end_timestamp_ms,
            mic_snr,
            sys_snr,
        });
        let oldest_ms = end_timestamp_ms - 150_000.0;
        while self
            .windows
            .front()
            .map_or(false, |window| window.end_timestamp_ms <= oldest_ms)
        {
            self.windows.pop_front();
        }
    }

    fn source_for_segment(&self, start_ms: f64, end_ms: f64) -> (DeviceType, f32, f32) {
        let mut mic_snr = 0.0;
        let mut sys_snr = 0.0;
        for window in &self.windows {
            let window_duration = window.end_timestamp_ms - window.start_timestamp_ms;
            let overlap = (end_ms.min(window.end_timestamp_ms)
                - start_ms.max(window.start_timestamp_ms))
            .max(0.0);
            // Half-open ranges give no weight to windows touching only an edge.
            let weight = if window_duration > 0.0 {
                (overlap / window_duration) as f32
            } else {
                0.0
            };
            if weight > 0.0 {
                mic_snr += window.mic_snr * weight;
                sys_snr += window.sys_snr * weight;
            }
        }
        // Normalizing by the total weight cancels in the ratio. Preserve the
        // microphone fallback when both sums are zero. The threshold may need calibration.
        const MIC_DOMINANCE: f32 = 1.5;
        let source = if mic_snr >= sys_snr * MIC_DOMINANCE {
            DeviceType::Microphone
        } else {
            DeviceType::System
        };
        (source, mic_snr, sys_snr)
    }
}

/// Ring buffer for synchronized audio mixing.
/// Accumulates mic and system samples until we have aligned windows.
struct AudioMixerRingBuffer {
    mic_buffer: VecDeque<f32>,
    system_buffer: VecDeque<f32>,
    window_size_samples: usize,
    source_grace_samples: usize,
    max_buffer_size: usize,
}

impl AudioMixerRingBuffer {
    fn new(sample_rate: u32) -> Self {
        let window_size_samples = sample_rate as usize * 600 / 1_000;
        let source_grace_samples = sample_rate as usize / 10;
        let max_buffer_size = window_size_samples * 8;

        Self {
            mic_buffer: VecDeque::with_capacity(max_buffer_size),
            system_buffer: VecDeque::with_capacity(max_buffer_size),
            window_size_samples,
            source_grace_samples,
            max_buffer_size,
        }
    }

    fn add_samples(&mut self, device_type: DeviceType, samples: Vec<f32>) -> usize {
        let buffer = match device_type {
            DeviceType::Microphone => &mut self.mic_buffer,
            DeviceType::System => &mut self.system_buffer,
        };
        buffer.extend(samples);
        let dropped = buffer.len().saturating_sub(self.max_buffer_size);
        buffer.drain(..dropped);
        dropped
    }

    fn can_mix(&self) -> bool {
        let mic = self.mic_buffer.len();
        let system = self.system_buffer.len();
        // A callback arriving just before its counterpart must not advance the
        // mix clock with padding. Allow bounded sample lookahead so a missing
        // or stalled source cannot block the active source.
        mic.min(system) >= self.window_size_samples
            || mic.max(system) >= self.window_size_samples + self.source_grace_samples
    }

    fn extract_window(&mut self) -> Option<(Vec<f32>, Vec<f32>)> {
        if !self.can_mix() {
            return None;
        }

        // Extract mic window with zero-padding for incomplete buffers
        // Zero-padding (silence) is preferred over last-sample-hold to prevent artifacts

        // Extract mic window (or pad with zeros if insufficient data)
        let mic_window = if self.mic_buffer.len() >= self.window_size_samples {
            // Enough mic data - drain window
            self.mic_buffer.drain(0..self.window_size_samples).collect()
        } else if !self.mic_buffer.is_empty() {
            // Some mic data but not enough - consume all + pad with zeros
            let available: Vec<f32> = self.mic_buffer.drain(..).collect();
            let mut padded = Vec::with_capacity(self.window_size_samples);
            padded.extend_from_slice(&available);

            // Use zero-padding (silence) to prevent repetition artifacts
            // Zero-padding is inaudible at 48kHz sample rate
            padded.resize(self.window_size_samples, 0.0);

            padded
        } else {
            // No mic data - return silence
            vec![0.0; self.window_size_samples]
        };

        // Extract system window (or pad with zeros if insufficient data)
        let sys_window = if self.system_buffer.len() >= self.window_size_samples {
            // Enough system data - drain window
            self.system_buffer
                .drain(0..self.window_size_samples)
                .collect()
        } else if !self.system_buffer.is_empty() {
            // Some system data but not enough - consume all + pad with zeros
            let available: Vec<f32> = self.system_buffer.drain(..).collect();
            let mut padded = Vec::with_capacity(self.window_size_samples);
            padded.extend_from_slice(&available);

            // Use zero-padding (silence) to prevent repetition artifacts
            // Zero-padding is inaudible at 48kHz sample rate
            padded.resize(self.window_size_samples, 0.0);

            padded
        } else {
            // No system data - return silence
            vec![0.0; self.window_size_samples]
        };

        Some((mic_window, sys_window))
    }
}

/// Simple audio mixer without aggressive ducking
/// Combines mic + system audio with basic clipping prevention
struct ProfessionalAudioMixer;

impl ProfessionalAudioMixer {
    fn new(_sample_rate: u32) -> Self {
        Self
    }

    fn mix_window(&mut self, mic_window: &[f32], sys_window: &[f32]) -> Vec<f32> {
        // Handle different lengths (already padded by extract_window, but defensive)
        let max_len = mic_window.len().max(sys_window.len());
        let mut mixed = Vec::with_capacity(max_len);

        // Professional mixing with soft scaling to prevent distortion
        // Uses proportional scaling instead of hard clamping to avoid artifacts
        for i in 0..max_len {
            let mic = mic_window.get(i).copied().unwrap_or(0.0);
            let sys = sys_window.get(i).copied().unwrap_or(0.0);

            // Pre-scale system audio to 70% to leave headroom
            // This prevents constant soft scaling which can cause pumping artifacts
            // Mic is normalized to -23 LUFS (already optimal), system needs reduction
            let sys_scaled = sys * 1.0;
            let _mic_scaled = mic * 0.8; // Reserved for future mic scaling

            // Sum without ducking - mic stays at full volume, system slightly reduced
            let sum = mic + sys_scaled;

            // CRITICAL FIX: Soft scaling prevents distortion artifacts
            // If the sum would exceed ±1.0, scale down PROPORTIONALLY
            // This avoids hard clipping distortion that sounds like "radio breaks"
            let sum_abs = sum.abs();
            let mixed_sample = if sum_abs > 1.0 {
                // Scale down to fit within ±1.0
                sum / sum_abs
            } else {
                sum
            };

            mixed.push(mixed_sample);
        }

        mixed
    }
}

/// Simplified audio capture without broadcast channels
#[derive(Clone)]
pub struct AudioCapture {
    device: Arc<AudioDevice>,
    state: Arc<RecordingState>,
    sample_rate: u32, // Original device sample rate
    channels: u16,
    chunk_counter: Arc<std::sync::atomic::AtomicU64>,
    device_type: DeviceType,
    needs_resampling: bool, // Flag if resampling is required
    // CRITICAL FIX: Persistent resampler to preserve energy across chunks
    resampler: Arc<std::sync::Mutex<Option<SincFixedIn<f32>>>>,
    // Buffering for variable-size chunks → fixed-size resampler input
    resampler_input_buffer: Arc<std::sync::Mutex<Vec<f32>>>,
    resampler_chunk_size: usize, // Fixed chunk size for resampler (512 samples)
    // Audio enhancement processors (microphone only)
    noise_suppressor: Arc<std::sync::Mutex<Option<NoiseSuppressionProcessor>>>,
    high_pass_filter: Arc<std::sync::Mutex<Option<HighPassFilter>>>,
    // EBU R128 normalizer for microphone audio (per-device, stateful)
    normalizer: Arc<std::sync::Mutex<Option<LoudnessNormalizer>>>,
    // Note: Using global recording timestamp for synchronization
}

impl AudioCapture {
    pub fn new(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        sample_rate: u32,
        channels: u16,
        device_type: DeviceType,
    ) -> Self {
        // CRITICAL FIX: Detect if resampling is needed
        // Pipeline expects 48kHz, but Bluetooth devices often report 8kHz, 16kHz, or 44.1kHz
        const TARGET_SAMPLE_RATE: u32 = 48000;
        let needs_resampling = sample_rate != TARGET_SAMPLE_RATE;

        // Detect device kind (Bluetooth vs Wired) for adaptive processing
        // Use reasonable defaults for buffer size (512 samples is typical)
        let device_kind =
            super::device_detection::InputDeviceKind::detect(&device.name, 512, sample_rate);

        if needs_resampling {
            warn!("⚠️ SAMPLE RATE MISMATCH DETECTED ⚠️");
            warn!(
                "🔄 [{:?}] Audio device '{}' ({:?}) reports {} Hz (pipeline expects {} Hz)",
                device_type, device.name, device_kind, sample_rate, TARGET_SAMPLE_RATE
            );
            warn!(
                "🔄 Automatic resampling will be applied: {} Hz → {} Hz",
                sample_rate, TARGET_SAMPLE_RATE
            );

            // Log which resampling strategy will be used
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
            let strategy = if ratio >= 2.0 {
                "High-quality upsampling (sinc_len=512, Cubic interpolation)"
            } else if ratio >= 1.5 {
                "Moderate upsampling (sinc_len=384, Cubic)"
            } else if ratio > 1.0 {
                "Small upsampling (sinc_len=256, Linear)"
            } else if ratio <= 0.5 {
                "Anti-aliased downsampling (sinc_len=512, Cubic)"
            } else {
                "Moderate downsampling (sinc_len=384, Linear)"
            };
            info!("   Resampling strategy: {}", strategy);
        } else {
            info!(
                "✅ [{:?}] Audio device '{}' ({:?}) uses {} Hz (matches pipeline)",
                device_type, device.name, device_kind, sample_rate
            );
        }

        // Initialize audio enhancement processors for MICROPHONE ONLY
        // System audio doesn't need enhancement (already clean)
        let (noise_suppressor, high_pass_filter, normalizer) = if matches!(
            device_type,
            DeviceType::Microphone
        ) {
            // Initialize noise suppression (RNNoise) at 48kHz - CONDITIONAL based on flag
            let ns = if super::ffmpeg_mixer::RNNOISE_APPLY_ENABLED {
                match NoiseSuppressionProcessor::new(TARGET_SAMPLE_RATE) {
                    Ok(processor) => {
                        info!("✅ RNNoise noise suppression ENABLED for microphone '{}' (10-15 dB reduction)", device.name);
                        Some(processor)
                    }
                    Err(e) => {
                        warn!("⚠️ Failed to create noise suppressor: {}, continuing without noise suppression", e);
                        None
                    }
                }
            } else {
                info!("ℹ️ RNNoise noise suppression DISABLED for microphone '{}' (flag: RNNOISE_APPLY_ENABLED=false)", device.name);
                info!("   Whisper handles noise well internally - RNNoise is optional");
                None
            };

            // Initialize high-pass filter (removes rumble below 80 Hz)
            let hpf = {
                let filter = HighPassFilter::new(TARGET_SAMPLE_RATE, 80.0);
                info!(
                    "✅ High-pass filter initialized for microphone '{}' (cutoff: 80 Hz)",
                    device.name
                );
                Some(filter)
            };

            // Initialize EBU R128 normalizer (professional loudness standard)
            let norm = match LoudnessNormalizer::new(1, TARGET_SAMPLE_RATE) {
                Ok(normalizer) => {
                    info!(
                        "✅ EBU R128 normalizer initialized for microphone '{}' (target: -23 LUFS)",
                        device.name
                    );
                    Some(normalizer)
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to create normalizer for microphone: {}, normalization disabled",
                        e
                    );
                    None
                }
            };

            (ns, hpf, norm)
        } else {
            // System audio: no enhancement needed
            info!(
                "ℹ️ System audio '{}' captured raw (no enhancement)",
                device.name
            );
            (None, None, None)
        };

        // CRITICAL FIX: Initialize persistent resampler to preserve energy across chunks
        // Creating a new resampler per chunk causes energy amplification and incorrect output sizes
        // Use fixed chunk size of 512 samples with buffering for variable-size input
        const RESAMPLER_CHUNK_SIZE: usize = 512;

        let resampler = if needs_resampling {
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;

            // Adaptive parameters based on sample rate ratio (same logic as resample_audio)
            let (sinc_len, interpolation_type, oversampling) = if ratio >= 2.0 {
                (512, SincInterpolationType::Cubic, 512)
            } else if ratio >= 1.5 {
                (384, SincInterpolationType::Cubic, 384)
            } else if ratio > 1.0 {
                (256, SincInterpolationType::Linear, 256)
            } else if ratio <= 0.5 {
                (512, SincInterpolationType::Cubic, 512)
            } else {
                (384, SincInterpolationType::Linear, 384)
            };

            let params = SincInterpolationParameters {
                sinc_len,
                f_cutoff: 0.95,
                interpolation: interpolation_type,
                oversampling_factor: oversampling,
                window: WindowFunction::BlackmanHarris2,
            };

            match SincFixedIn::<f32>::new(
                ratio,
                2.0, // Maximum relative deviation
                params,
                RESAMPLER_CHUNK_SIZE,
                1, // Mono
            ) {
                Ok(resampler) => {
                    info!(
                        "✅ Persistent resampler initialized for '{}' ({}Hz → {}Hz, chunk_size={})",
                        device.name, sample_rate, TARGET_SAMPLE_RATE, RESAMPLER_CHUNK_SIZE
                    );
                    info!("   Buffering enabled for variable-size chunks (e.g., 320, 512, 1024, etc.)");
                    Some(resampler)
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to create persistent resampler: {}, will use fallback",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        Self {
            device,
            state,
            sample_rate,
            channels,
            chunk_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            device_type,
            needs_resampling,
            resampler: Arc::new(std::sync::Mutex::new(resampler)),
            resampler_input_buffer: Arc::new(std::sync::Mutex::new(Vec::with_capacity(
                RESAMPLER_CHUNK_SIZE * 2,
            ))),
            resampler_chunk_size: RESAMPLER_CHUNK_SIZE,
            noise_suppressor: Arc::new(std::sync::Mutex::new(noise_suppressor)),
            high_pass_filter: Arc::new(std::sync::Mutex::new(high_pass_filter)),
            normalizer: Arc::new(std::sync::Mutex::new(normalizer)),
            // Using global recording time for sync
        }
    }

    /// Process audio data directly from callback
    pub fn process_audio_data(&self, data: &[f32]) {
        // Check if still recording
        if !self.state.is_recording() {
            return;
        }

        // Convert to mono if needed
        let mut mono_data = if self.channels > 1 {
            audio_to_mono(data, self.channels)
        } else {
            data.to_vec()
        };

        // CRITICAL FIX: Resample to 48kHz if device uses different sample rate
        // This fixes Bluetooth devices (like Sony WH-1000XM4) that report 16kHz or 44.1kHz
        // Without this, audio is sped up 3x and VAD fails
        //
        // IMPORTANT: Uses PERSISTENT resampler with BUFFERING to preserve energy across chunks
        // Creating a new resampler per chunk causes energy amplification (173.5% RMS)
        // Buffering handles variable chunk sizes (320, 512, 1024, etc.) by accumulating to fixed 512-sample chunks
        const TARGET_SAMPLE_RATE: u32 = 48000;
        if self.needs_resampling {
            let before_len = mono_data.len();
            let before_rms = if !mono_data.is_empty() {
                (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
            } else {
                0.0
            };

            // Use persistent resampler with buffering to handle variable chunk sizes
            let mut resampled_output = Vec::new();
            let mut used_persistent_resampler = false;

            if let Ok(mut buffer_lock) = self.resampler_input_buffer.lock() {
                // Add new samples to buffer
                buffer_lock.extend_from_slice(&mono_data);

                // Process complete chunks through the resampler
                if let Ok(mut resampler_lock) = self.resampler.lock() {
                    if let Some(ref mut resampler) = *resampler_lock {
                        used_persistent_resampler = true;

                        // Process as many complete chunks as we have
                        while buffer_lock.len() >= self.resampler_chunk_size {
                            // Extract exactly chunk_size samples
                            let chunk: Vec<f32> =
                                buffer_lock.drain(0..self.resampler_chunk_size).collect();

                            // Rubato expects input as Vec<Vec<f32>> (one Vec per channel)
                            let waves_in = vec![chunk];

                            match resampler.process(&waves_in, None) {
                                Ok(mut waves_out) => {
                                    if let Some(output) = waves_out.pop() {
                                        resampled_output.extend_from_slice(&output);
                                    }
                                }
                                Err(e) => {
                                    warn!("⚠️ Persistent resampler processing failed: {}", e);
                                    used_persistent_resampler = false;
                                    break;
                                }
                            }
                        }
                        // Remaining samples in buffer will be processed in next iteration
                    }
                }
            }

            // CRITICAL: Only update mono_data if we got output from persistent resampler
            // If buffer is accumulating (< 512 samples), skip this chunk - data is safely buffered
            // and will be processed in next iteration with proper resampling
            let has_resampled_output = !resampled_output.is_empty();

            if has_resampled_output {
                mono_data = resampled_output;
            } else if !used_persistent_resampler {
                // Only fallback if persistent resampler is not available at all
                mono_data = super::audio_processing::resample_audio(
                    &mono_data,
                    self.sample_rate,
                    TARGET_SAMPLE_RATE,
                );
            } else {
                // Buffering: samples are accumulating in buffer, waiting for 512-sample chunk
                // Don't send partial/unprocessed data - return early
                // Audio is NOT lost - it's in the buffer and will be processed next iteration
                return;
            }

            // Log resampling only occasionally to avoid spam
            let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
            if chunk_id % 100 == 0 && has_resampled_output {
                let after_len = mono_data.len();
                let after_rms = if !mono_data.is_empty() {
                    (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
                } else {
                    0.0
                };
                let ratio = TARGET_SAMPLE_RATE as f64 / self.sample_rate as f64;
                let rms_preservation = if before_rms > 0.0 {
                    (after_rms / before_rms) * 100.0
                } else {
                    100.0
                };

                let buffer_size = if let Ok(buf) = self.resampler_input_buffer.lock() {
                    buf.len()
                } else {
                    0
                };

                info!(
                    "🔄 [{:?}] Persistent buffered resampler: {}Hz → {}Hz (ratio: {:.2}x)",
                    self.device_type, self.sample_rate, TARGET_SAMPLE_RATE, ratio
                );
                info!(
                    "   Chunk {}: {} → {} samples, RMS preservation: {:.1}%, buffer: {}",
                    chunk_id, before_len, after_len, rms_preservation, buffer_size
                );
            }
        }

        // AUDIO ENHANCEMENT PIPELINE (Microphone Only)
        // Processing order is critical: high-pass → noise suppression → normalization
        // This ensures noise is removed before being amplified by the normalizer
        if matches!(self.device_type, DeviceType::Microphone) {
            // STEP 1: Apply high-pass filter to remove low-frequency rumble (< 80 Hz)
            if let Ok(mut hpf_lock) = self.high_pass_filter.lock() {
                if let Some(ref mut filter) = *hpf_lock {
                    mono_data = filter.process(&mono_data);
                }
            }

            // STEP 2: Apply RNNoise noise suppression (10-15 dB reduction) - CONDITIONAL
            if super::ffmpeg_mixer::RNNOISE_APPLY_ENABLED {
                if let Ok(mut ns_lock) = self.noise_suppressor.lock() {
                    if let Some(ref mut suppressor) = *ns_lock {
                        let before_len = mono_data.len();
                        mono_data = suppressor.process(&mono_data);
                        let after_len = mono_data.len();

                        // CRITICAL MONITORING: Track buffer health
                        let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
                        if chunk_id % 100 == 0 {
                            let buffered = suppressor.buffered_samples();
                            let length_delta = (before_len as i32 - after_len as i32).abs();

                            debug!("🔇 Noise suppression health: in={}, out={}, delta={}, buffered={}, RMS={:.4}",
                                   before_len, after_len, length_delta, buffered,
                                   if !mono_data.is_empty() {
                                       (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
                                   } else { 0.0 });

                            // WARN if accumulating samples (potential latency buildup)
                            if buffered > 1000 {
                                warn!("⚠️ RNNoise accumulating samples: {} buffered (potential latency issue!)",
                                      buffered);
                            }

                            // WARN if significant length mismatch
                            if length_delta > 50 {
                                warn!(
                                    "⚠️ RNNoise length mismatch: input={} output={} (delta={})",
                                    before_len, after_len, length_delta
                                );
                            }
                        }
                    }
                }
            }

            // STEP 3: Apply EBU R128 normalization (professional loudness standard)
            if let Ok(mut normalizer_lock) = self.normalizer.lock() {
                if let Some(ref mut normalizer) = *normalizer_lock {
                    mono_data = normalizer.normalize_loudness(&mono_data);

                    // Log normalization occasionally for debugging
                    let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
                    if chunk_id % 200 == 0 && !mono_data.is_empty() {
                        let rms = (mono_data.iter().map(|&x| x * x).sum::<f32>()
                            / mono_data.len() as f32)
                            .sqrt();
                        let peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
                        debug!(
                            "🎤 After normalization chunk {}: RMS={:.4}, Peak={:.4}",
                            chunk_id, rms, peak
                        );
                    }
                }
            }
        }

        // Create audio chunk with stream-specific timestamp (get ID first for logging)
        let chunk_id = self
            .chunk_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // RAW AUDIO: No gain applied here - will be applied AFTER mixing
        // This prevents amplifying system audio bleed-through in the microphone

        // DIAGNOSTIC: Log audio levels for debugging (especially mic issues)
        // if chunk_id % 100 == 0 && !mono_data.is_empty() {
        //     let raw_rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
        //     let raw_peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);

        //         info!("🎙️ [{:?}] Chunk {} - Raw: RMS={:.6}, Peak={:.6}",
        //               self.device_type, chunk_id, raw_rms, raw_peak);

        //     // Warn if microphone is completely silent
        //     if matches!(self.device_type, DeviceType::Microphone) && raw_rms == 0.0 && raw_peak == 0.0 {
        //         warn!("⚠️ Microphone producing ZERO audio - check permissions or hardware!");
        //     }
        // }
        // else if chunk_id % 100 == 0 && matches!(self.device_type, DeviceType::System) {
        //     let raw_rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
        //     let raw_peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
        //     info!("🔊 [{:?}] Chunk {} - Raw: RMS={:.6}, Peak={:.6}",
        //       self.device_type, chunk_id, raw_rms, raw_peak);

        //     // Warn if system audio is completely silent
        //     if raw_rms == 0.0 && raw_peak == 0.0 {
        //         warn!("⚠️ System audio producing ZERO audio - check permissions or hardware!");
        //     }
        // }

        // Use global recording timestamp for proper synchronization
        let timestamp = self.state.get_recording_duration().unwrap_or(0.0);

        // RAW AUDIO CHUNK: No gain applied - will be mixed and gained downstream
        // Use 48kHz if we resampled, otherwise use original rate
        let audio_chunk = AudioChunk {
            data: mono_data, // Raw audio (resampled if needed), no gain yet
            sample_rate: if self.needs_resampling {
                48000
            } else {
                self.sample_rate
            },
            timestamp,
            chunk_id,
            device_type: self.device_type.clone(),
        };

        // NOTE: Raw audio is NOT sent to recording saver to prevent echo
        // Only the mixed audio (from AudioPipeline) is saved to file (see pipeline.rs:726-736)
        // This ensures we only record once: mic + system properly mixed
        // Individual raw streams go only to the transcription pipeline below

        // Send to processing pipeline for transcription
        if let Err(e) = self.state.send_audio_chunk(audio_chunk) {
            // Check if this is the "pipeline not ready" error
            if e.to_string().contains("Audio pipeline not ready") {
                // This is expected during initialization, just log it as debug
                debug!("Audio pipeline not ready yet, skipping chunk {}", chunk_id);
                return;
            }

            warn!("Failed to send audio chunk: {}", e);
            // More specific error handling based on failure reason
            let error = if e.to_string().contains("channel closed") {
                AudioError::ChannelClosed
            } else if e.to_string().contains("full") {
                AudioError::BufferOverflow
            } else {
                AudioError::ProcessingFailed
            };
            self.state.report_error(error);
        } else {
            debug!("Sent audio chunk {} ({} samples)", chunk_id, data.len());
        }
    }

    /// Handle stream errors with enhanced disconnect detection
    pub fn handle_stream_error(&self, error: cpal::StreamError) {
        if !self.state.is_recording() {
            return;
        }
        error!("Audio stream error for {}: {}", self.device.name, error);

        let error_str = error.to_string().to_lowercase();

        // Enhanced error detection for device disconnection
        let audio_error = if error_str.contains("device is no longer available")
            || error_str.contains("device not found")
            || error_str.contains("device disconnected")
            || error_str.contains("no such device")
            || error_str.contains("device unavailable")
            || error_str.contains("device removed")
        {
            warn!("🔌 Device disconnect detected for: {}", self.device.name);
            AudioError::DeviceDisconnected
        } else if error_str.contains("permission") || error_str.contains("access denied") {
            AudioError::PermissionDenied
        } else if error_str.contains("channel closed") {
            AudioError::ChannelClosed
        } else if error_str.contains("stream") && error_str.contains("failed") {
            AudioError::StreamFailed
        } else {
            warn!("Unknown audio error: {}", error);
            AudioError::StreamFailed
        };

        if matches!(audio_error, AudioError::DeviceDisconnected)
            || (self.device_type == DeviceType::System
                && self.state.get_microphone_device().is_some())
        {
            self.state
                .mark_source_missing(self.device.clone(), self.device_type.clone());
        } else {
            self.state.report_error(audio_error);
        }
    }
}

/// VAD-driven audio processing pipeline
/// Uses Voice Activity Detection to segment speech in real-time and send only speech to Whisper
pub struct AudioPipeline {
    receiver: mpsc::Receiver<AudioChunk>,
    transcription_sender: BufferedSpool,
    state: Arc<RecordingState>,
    vad_processor: Option<ContinuousVadProcessor>,
    attribution_history: SourceAttributionHistory,
    mic_floor: f32,
    sys_floor: f32,
    sample_rate: u32,
    chunk_id_counter: u64,
    // Performance optimization: reduce logging frequency
    last_summary_time: std::time::Instant,
    processed_chunks: u64,
    // Smart batching for audio metrics
    // PROFESSIONAL AUDIO MIXING: Ring buffer + RMS-based mixer
    ring_buffer: AudioMixerRingBuffer,
    mixer: ProfessionalAudioMixer,
    // Recording sender for pre-mixed audio
    recording_sender_for_mixed: Option<BufferedSpool>,
    recording_buffer: Vec<f32>,
    recording_buffer_timestamp: f64,
    queue_write_failures: QueueWriteFailureTracker,
    // Live input levels for the UI meters, folded from the chunks seen here.
    levels: super::recording_levels::LevelMeter,
}

impl AudioPipeline {
    pub fn new(
        receiver: mpsc::Receiver<AudioChunk>,
        transcription_sender: TranscriptionQueueSender,
        state: Arc<RecordingState>,
        target_chunk_duration_ms: u32,
        sample_rate: u32,
        mic_device_name: String,
        mic_device_kind: super::device_detection::InputDeviceKind,
        system_device_name: String,
        system_device_kind: super::device_detection::InputDeviceKind,
        transcribe: bool,
    ) -> Self {
        // Log device characteristics for adaptive buffering
        info!("🎛️ AudioPipeline initializing with device characteristics:");
        info!(
            "   Mic: '{}' ({:?}) - Buffer: {:?}",
            mic_device_name,
            mic_device_kind,
            mic_device_kind.buffer_timeout()
        );
        info!(
            "   System: '{}' ({:?}) - Buffer: {:?}",
            system_device_name,
            system_device_kind,
            system_device_kind.buffer_timeout()
        );

        // Device kind information can be used for adaptive buffering in the future
        // For now, we log it for monitoring and potential optimization
        let _ = (
            mic_device_name,
            mic_device_kind,
            system_device_name,
            system_device_kind,
        );

        // Create VAD processor with balanced redemption time for speech accumulation
        // The VAD processor now handles 48kHz->16kHz resampling internally
        // This bridges natural pauses without excessive fragmentation
        // For mac os core audio, 900ms, for windows 400ms seems good

        // Per-provider live cap, set at recording start (shorter for Nemotron).
        let live_max_segment_ms = LIVE_MAX_SEGMENT_MS.load(std::sync::atomic::Ordering::Relaxed);

        let vad_processor = if transcribe {
            Some(
                match ContinuousVadProcessor::new_with_max_segment_duration(
                    sample_rate,
                    LIVE_VAD_REDEMPTION_MS,
                    Some(live_max_segment_ms),
                ) {
                    Ok(processor) => {
                        info!(
                            "VAD-driven pipeline: live speech segments capped at {}ms for lower latency",
                            live_max_segment_ms
                        );
                        processor
                    }
                    Err(e) => {
                        error!("Failed to create VAD processor: {}", e);
                        panic!("VAD processor creation failed: {}", e);
                    }
                },
            )
        } else {
            None
        };

        // Initialize professional audio mixing components
        let ring_buffer = AudioMixerRingBuffer::new(sample_rate);
        let mixer = ProfessionalAudioMixer::new(sample_rate);

        // Note: target_chunk_duration_ms is ignored - VAD controls segmentation now
        let _ = target_chunk_duration_ms;

        Self {
            receiver,
            transcription_sender: BufferedSpool::new(transcription_sender, state.clone(), false),
            state,
            vad_processor,
            attribution_history: SourceAttributionHistory::default(),
            mic_floor: 1e-4,
            sys_floor: 1e-4,
            sample_rate,
            chunk_id_counter: 0,
            // Performance optimization: reduce logging frequency
            last_summary_time: std::time::Instant::now(),
            processed_chunks: 0,
            // Initialize professional audio mixing
            ring_buffer,
            mixer,
            recording_buffer: Vec::new(),
            recording_buffer_timestamp: 0.0,
            recording_sender_for_mixed: None, // Will be set by manager
            queue_write_failures: QueueWriteFailureTracker::default(),
            levels: super::recording_levels::LevelMeter::new(std::time::Instant::now()),
        }
    }

    async fn enqueue_transcription_chunk(&mut self, chunk: AudioChunk) -> bool {
        let start_ms = chunk.timestamp * 1000.0;
        let end_ms = start_ms + 1000.0 * chunk.data.len() as f64 / f64::from(chunk.sample_rate);
        let clear_source =
            self.attribution_history
                .confidently_matches(&chunk.device_type, start_ms, end_ms);
        match self
            .transcription_sender
            .send_with_source(chunk, clear_source)
            .await
        {
            Ok(()) => {
                self.queue_write_failures.record_success();
                true
            }
            Err(error) => {
                warn!("Failed to spool live transcription segment: {}", error);
                if self.queue_write_failures.record_failure() {
                    self.state.report_warning(
                        "Live transcription cannot write queued audio to disk. The recording is still being preserved, but the transcript will be incomplete and should be regenerated after the meeting. Check available disk space.",
                    );
                }
                false
            }
        }
    }

    /// Run the VAD-driven audio processing pipeline
    pub async fn run(mut self) -> Result<()> {
        info!("VAD-driven audio pipeline started - segments sent in real-time based on speech detection");

        let mut system_audio_silence = SystemAudioSilenceTracker::default();
        let mut microphone_stream = MicrophoneStreamWatch::default();

        // Speaker attribution ("Me" = mic, "Participants" = system). We compare
        // each source's energy *relative to its own rolling noise floor* (an
        // SNR-style measure), not raw level. This matters because the mic is
        // loudness-normalized to ~-23 LUFS at capture while system audio stays
        // raw: a raw-level comparison is structurally biased toward the mic, so
        // system-only playback (e.g. a YouTube video) mislabeled as "Me".
        // Measuring excess-over-floor cancels that bias. Transcription itself is
        // unchanged — it still runs on the mixed audio.

        // CRITICAL FIX: Continue processing until channel is closed, not based on recording state
        // This ensures ALL chunks are processed during shutdown, fixing premature meeting completion
        // Previous bug: Loop checked `while self.state.is_recording()` which caused early exit when
        // stop_recording() was called, losing flush signals and remaining chunks in the pipeline
        loop {
            if self.state.live_transcription_disabled() {
                self.vad_processor = None;
            }
            let mut microphone_samples = false;
            let mut system_audible = false;
            // Receive audio chunks with timeout
            match tokio::time::timeout(
                std::time::Duration::from_millis(50), // Shorter timeout for responsiveness
                self.receiver.recv(),
            )
            .await
            {
                Ok(Some(chunk)) => {
                    // PERFORMANCE OPTIMIZATION: Eliminate per-chunk logging overhead
                    // Logging in hot paths causes severe performance degradation
                    self.processed_chunks += 1;
                    microphone_samples =
                        chunk.device_type == DeviceType::Microphone && !chunk.data.is_empty();

                    // CRITICAL: Log summary only every 200 chunks OR every 60 seconds (99.5% reduction)
                    // This eliminates I/O overhead in the audio processing hot path
                    // Use performance-optimized debug macro that compiles to nothing in release builds
                    if self.processed_chunks % 200 == 0
                        || self.last_summary_time.elapsed().as_secs() >= 60
                    {
                        perf_debug!(
                            "Pipeline processed {} chunks, current chunk: {} ({} samples)",
                            self.processed_chunks,
                            chunk.chunk_id,
                            chunk.data.len()
                        );
                        self.last_summary_time = std::time::Instant::now();
                    }

                    // Measure quiet loopback periods independently of capture loss.
                    system_audible = audible_system_audio(&chunk.device_type, &chunk.data);

                    // Live meters: one snapshot per ~100 ms, none while paused
                    // or stopping. This is the pipeline task, not the capture
                    // callback, and the meter does not allocate.
                    let state = &self.state;
                    if let Some(levels) = self.levels.observe(
                        &chunk.device_type,
                        &chunk.data,
                        std::time::Instant::now(),
                        state.is_recording() && !state.is_paused(),
                        || {
                            (
                                state.get_microphone_device().is_some(),
                                state.get_system_device().is_some(),
                            )
                        },
                    ) {
                        state.report_levels(&levels);
                    }

                    // STEP 1: Add raw audio to ring buffer for mixing
                    // Microphone audio is already normalized at capture level (AudioCapture)
                    // System audio remains raw
                    let dropped = self
                        .ring_buffer
                        .add_samples(chunk.device_type.clone(), chunk.data);
                    if dropped > 0 {
                        self.state.record_sample_loss(dropped);
                    }

                    // Mix paired windows, allowing one source to continue after the grace period.
                    while self.ring_buffer.can_mix() {
                        if let Some((mic_window, sys_window)) = self.ring_buffer.extract_window() {
                            // Simple mixing without aggressive ducking
                            let mixed_clean = self.mixer.mix_window(&mic_window, &sys_window);

                            // NO POST-GAIN NEEDED: Microphone already normalized by EBU R128 to -23 LUFS
                            // This is broadcast-standard loudness (Netflix/YouTube/Spotify level)
                            // System audio at natural levels
                            // Previous 2x gain was causing excessive limiting/distortion
                            let mixed_with_gain = mixed_clean;

                            self.preserve_mixed_audio(&mixed_with_gain, chunk.timestamp)
                                .await?;

                            // STEP 3: Send mixed audio for transcription (VAD + Whisper)
                            if self.vad_processor.is_some() {
                                self.record_attribution_window(
                                    &mic_window,
                                    &sys_window,
                                    mixed_with_gain.len(),
                                );
                            }
                            match self
                                .vad_processor
                                .as_mut()
                                .map(|vad| vad.process_audio(&mixed_with_gain))
                                .transpose()
                            {
                                Ok(Some(speech_segments)) => {
                                    for segment in speech_segments {
                                        let segment_source = self.segment_source(
                                            segment.start_timestamp_ms,
                                            segment.end_timestamp_ms,
                                        );
                                        let duration_ms =
                                            segment.end_timestamp_ms - segment.start_timestamp_ms;

                                        if segment.samples.len() >= 800 {
                                            // Minimum 50ms at 16kHz - matches Parakeet capability
                                            debug!(
                                                "📤 Sending VAD segment: {:.1}ms, {} samples",
                                                duration_ms,
                                                segment.samples.len()
                                            );

                                            let transcription_chunk = AudioChunk {
                                                data: segment.samples,
                                                sample_rate: 16000,
                                                timestamp: segment.start_timestamp_ms / 1000.0,
                                                chunk_id: self.chunk_id_counter,
                                                device_type: segment_source,
                                            };

                                            if self
                                                .enqueue_transcription_chunk(transcription_chunk)
                                                .await
                                            {
                                                self.chunk_id_counter += 1;
                                            }
                                        } else {
                                            debug!("⏭️ Dropping short VAD segment: {:.1}ms ({} samples < 800)",
                                                   duration_ms, segment.samples.len());
                                        }
                                    }
                                }
                                Ok(None) => {}
                                Err(_) => {
                                    self.transcription_sender.mark_failed();
                                    self.state.report_warning("Speech detection failed. Recorded audio is retained; use Retranscribe after stopping.");
                                }
                            }
                        }
                    }
                }
                Ok(None) => {
                    info!(
                        "Audio pipeline: sender closed after processing {} chunks",
                        self.processed_chunks
                    );
                    break;
                }
                Err(_) => {
                    // No chunk this tick. The stream timers still advance.
                }
            }

            if let Some(stalled) = microphone_stream.update(
                std::time::Instant::now(),
                self.state.is_active() && self.state.get_microphone_device().is_some(),
                microphone_samples,
            ) {
                if stalled {
                    self.state.mark_capture_incomplete();
                    self.state.report_warning(MICROPHONE_STREAM_WARNING);
                } else {
                    self.state.clear_warning(MICROPHONE_STREAM_WARNING);
                }
            }

            if system_audio_silence.update(
                std::time::Instant::now(),
                self.state.is_active()
                    && self.state.get_system_device().is_some()
                    && !self.state.source_is_missing(&DeviceType::System),
                system_audible,
            ) {
                self.state.record_system_silence();
            }
        }

        // Flush any remaining VAD segments
        self.flush_remaining_audio().await?;

        // Drain independent disk workers after capture has closed. The bounded
        // deadlines mark omissions and retain recovery data on a stalled drive.
        tokio::join!(
            async {
                if let Some(recording) = &mut self.recording_sender_for_mixed {
                    recording.finish().await;
                }
            },
            self.transcription_sender.finish(),
        );

        info!("VAD-driven audio pipeline ended");
        Ok(())
    }

    fn record_attribution_window(&mut self, mic: &[f32], system: &[f32], sample_count: usize) {
        let rms = |window: &[f32]| -> f32 {
            if window.is_empty() {
                0.0
            } else {
                (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt()
            }
        };
        let mic_rms = rms(mic);
        let sys_rms = rms(system);
        // Floors persist across segments: fall fast toward quiet, rise slowly
        // so sustained speech does not inflate the baseline.
        let track = |floor: &mut f32, rms: f32| {
            let a = if rms < *floor { 0.10 } else { 0.001 };
            *floor += a * (rms - *floor);
            if *floor < 1e-6 {
                *floor = 1e-6;
            }
        };
        track(&mut self.mic_floor, mic_rms);
        track(&mut self.sys_floor, sys_rms);
        self.attribution_history.record_window(
            sample_count,
            self.sample_rate,
            mic_rms / self.mic_floor,
            sys_rms / self.sys_floor,
        );
        let position_ms =
            self.attribution_history.input_samples * 1000 / u64::from(self.sample_rate);
        self.state.set_speech_clock_ms(position_ms);
    }

    fn segment_source(&self, start_ms: f64, end_ms: f64) -> DeviceType {
        let attribution = self
            .attribution_history
            .source_for_segment(start_ms, end_ms);
        perf_debug!(
            "🗣️ attribution: mic_snr={:.1} sys_snr={:.1} floors(mic={:.5},sys={:.5}) -> {:?}",
            attribution.1,
            attribution.2,
            self.mic_floor,
            self.sys_floor,
            attribution.0
        );
        attribution.0
    }

    async fn preserve_mixed_audio(&mut self, samples: &[f32], timestamp: f64) -> Result<()> {
        if self.recording_sender_for_mixed.is_none() {
            return Ok(());
        }
        if self.recording_buffer.is_empty() {
            self.recording_buffer_timestamp = timestamp;
        }
        self.recording_buffer.extend_from_slice(samples);
        // Batch one second per durable write; syncing every mixer frame is too expensive.
        if self.recording_buffer.len() >= self.sample_rate as usize {
            self.flush_recording_buffer().await?;
        }
        Ok(())
    }

    async fn flush_recording_buffer(&mut self) -> Result<()> {
        if self.recording_buffer.is_empty() {
            return Ok(());
        }
        if let Some(sender) = &self.recording_sender_for_mixed {
            let chunk = AudioChunk {
                data: std::mem::take(&mut self.recording_buffer),
                sample_rate: self.sample_rate,
                timestamp: self.recording_buffer_timestamp,
                chunk_id: self.chunk_id_counter,
                device_type: DeviceType::Microphone,
            };
            if sender.send(chunk).await.is_err() {
                self.state.mark_capture_incomplete();
            }
        }
        Ok(())
    }

    async fn flush_remaining_audio(&mut self) -> Result<()> {
        if self.state.live_transcription_disabled() {
            self.vad_processor = None;
        }
        info!(
            "Flushing remaining audio from pipeline (processed {} chunks)",
            self.processed_chunks
        );

        let mic: Vec<f32> = self.ring_buffer.mic_buffer.drain(..).collect();
        let system: Vec<f32> = self.ring_buffer.system_buffer.drain(..).collect();
        if !mic.is_empty() || !system.is_empty() {
            let mixed = self.mixer.mix_window(&mic, &system);
            self.preserve_mixed_audio(&mixed, self.recording_buffer_timestamp)
                .await?;
            self.flush_recording_buffer().await?;
            if self.vad_processor.is_some() {
                self.record_attribution_window(&mic, &system, mixed.len());
            }
            let segments = self.vad_processor.as_mut().map(|vad| vad.process_audio(&mixed)).transpose().map(|segments| segments.unwrap_or_default()).unwrap_or_else(|_| {
                self.transcription_sender.mark_failed();
                self.state.report_warning("Final speech detection failed; use Retranscribe to recover speech from the saved audio.");
                Vec::new()
            });
            for segment in segments {
                let segment_source =
                    self.segment_source(segment.start_timestamp_ms, segment.end_timestamp_ms);
                let chunk = AudioChunk {
                    data: segment.samples,
                    sample_rate: 16000,
                    timestamp: segment.start_timestamp_ms / 1000.0,
                    chunk_id: self.chunk_id_counter,
                    device_type: segment_source,
                };
                if self.enqueue_transcription_chunk(chunk).await {
                    self.chunk_id_counter += 1;
                }
            }
        }
        self.flush_recording_buffer().await?;

        // Flush any remaining audio from VAD processor and send segments to transcription
        let Some(vad) = self.vad_processor.as_mut() else {
            return Ok(());
        };
        if vad.drain_resampler().is_err() {
            self.transcription_sender.mark_failed();
            self.state.report_warning("Final audio resampling failed. Retranscribe the saved audio to recover the end of the recording.");
        }
        match vad.finish_buffered_speech() {
            Ok(final_segments) => {
                for segment in final_segments {
                    let segment_source =
                        self.segment_source(segment.start_timestamp_ms, segment.end_timestamp_ms);
                    let duration_ms = segment.end_timestamp_ms - segment.start_timestamp_ms;

                    // Send segments >= 50ms (800 samples at 16kHz) - matches main pipeline filter
                    if segment.samples.len() >= 800 {
                        debug!(
                            "📤 Sending final VAD segment to Whisper: {:.1}ms duration, {} samples",
                            duration_ms,
                            segment.samples.len()
                        );

                        let transcription_chunk = AudioChunk {
                            data: segment.samples,
                            sample_rate: 16000,
                            timestamp: segment.start_timestamp_ms / 1000.0,
                            chunk_id: self.chunk_id_counter,
                            device_type: segment_source,
                        };

                        if self.enqueue_transcription_chunk(transcription_chunk).await {
                            self.chunk_id_counter += 1;
                        }
                    } else {
                        info!(
                            "⏭️ Skipping short final segment: {:.1}ms ({} samples < 800)",
                            duration_ms,
                            segment.samples.len()
                        );
                    }
                }
            }
            Err(_) => {
                self.transcription_sender.mark_failed();
                self.state.report_warning("Final speech detection failed; use Retranscribe to recover speech from the saved audio.");
            }
        }

        Ok(())
    }
}

/// Simple audio pipeline manager
pub struct AudioPipelineManager {
    pipeline_handle: Option<JoinHandle<Result<()>>>,
    audio_sender: Option<mpsc::Sender<AudioChunk>>,
}

impl AudioPipelineManager {
    pub fn new() -> Self {
        Self {
            pipeline_handle: None,
            audio_sender: None,
        }
    }

    /// Start the audio pipeline with device information for adaptive buffering
    pub fn start(
        &mut self,
        state: Arc<RecordingState>,
        transcription_sender: TranscriptionQueueSender,
        target_chunk_duration_ms: u32,
        sample_rate: u32,
        recording_sender: Option<super::transcription::queue::TranscriptionQueueSender>,
        mic_device_name: String,
        mic_device_kind: super::device_detection::InputDeviceKind,
        system_device_name: String,
        system_device_kind: super::device_detection::InputDeviceKind,
        transcribe: bool,
    ) -> Result<()> {
        // Log device information for adaptive buffering
        info!("🎙️ Starting pipeline with device info:");
        info!(
            "   Microphone: '{}' ({:?})",
            mic_device_name, mic_device_kind
        );
        info!(
            "   System Audio: '{}' ({:?})",
            system_device_name, system_device_kind
        );

        // Create audio processing channel
        let (audio_sender, audio_receiver) = mpsc::channel::<AudioChunk>(2048);

        // Set sender in state for audio captures to use
        state.set_audio_sender(audio_sender.clone());

        // Create and start pipeline with device information for adaptive mixing
        let mut pipeline = AudioPipeline::new(
            audio_receiver,
            transcription_sender,
            state.clone(),
            target_chunk_duration_ms,
            sample_rate,
            mic_device_name,
            mic_device_kind,
            system_device_name,
            system_device_kind,
            transcribe,
        );

        // CRITICAL FIX: Connect recording sender to receive pre-mixed audio
        // This ensures both mic AND system audio are captured in recordings
        pipeline.recording_sender_for_mixed =
            recording_sender.map(|sender| BufferedSpool::new(sender, state, true));

        let handle = tokio::spawn(async move { pipeline.run().await });

        self.pipeline_handle = Some(handle);
        self.audio_sender = Some(audio_sender);

        info!("Audio pipeline manager started with mixed audio recording");
        Ok(())
    }

    /// Stop the audio pipeline
    pub async fn stop(&mut self) -> Result<()> {
        // Drop the sender to close the pipeline
        self.audio_sender = None;

        // Wait for pipeline to finish
        if let Some(mut handle) = self.pipeline_handle.take() {
            match tokio::time::timeout(std::time::Duration::from_secs(45), &mut handle).await {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    error!("Pipeline task failed: {}", e);
                    Err(anyhow::anyhow!("Audio pipeline task failed"))
                }
                Err(_) => {
                    handle.abort();
                    Err(anyhow::anyhow!(
                        "Audio pipeline did not flush before the deadline"
                    ))
                }
            }
        } else {
            Ok(())
        }
    }

    /// Force immediate flush of accumulated audio and stop pipeline
    /// PERFORMANCE CRITICAL: Eliminates 30+ second shutdown delays
    pub async fn force_flush_and_stop(&mut self) -> Result<()> {
        self.stop().await
    }
}

impl Default for AudioPipelineManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod live_segment_cap_tests {
    use super::live_max_segment_ms_for_provider;

    #[test]
    fn each_provider_gets_its_live_segment_cap() {
        assert_eq!(live_max_segment_ms_for_provider("parakeet"), 12_000);
        assert_eq!(live_max_segment_ms_for_provider("Parakeet"), 12_000);
        assert_eq!(live_max_segment_ms_for_provider("nemotron"), 3_000);
        assert_eq!(live_max_segment_ms_for_provider("localWhisper"), 6_000);
        assert_eq!(live_max_segment_ms_for_provider(""), 6_000);
    }
}

#[cfg(test)]
mod source_attribution_tests {
    use super::{DeviceType, SourceAttributionHistory};

    #[test]
    fn language_attribution_requires_coverage_and_rejects_overlap_or_a_change_of_source() {
        let mut history = SourceAttributionHistory::default();
        for (mic, system) in [(10., 1.), (10., 1.), (1., 10.), (8., 7.), (1., 1.)] {
            history.record_window(16_000, 16_000, mic, system);
        }
        assert!(history.confidently_matches(&DeviceType::Microphone, 0., 2000.));
        assert!(history.confidently_matches(&DeviceType::System, 2000., 3000.));
        assert!(!history.confidently_matches(&DeviceType::Microphone, 0., 3000.));
        assert!(!history.confidently_matches(&DeviceType::System, 2000., 4000.));
        assert!(!history.confidently_matches(&DeviceType::Microphone, 4000., 5000.));
        assert!(!history.confidently_matches(&DeviceType::System, 5000., 6000.));
    }

    #[test]
    fn segments_emitted_together_use_their_own_source_windows() {
        let mut history = SourceAttributionHistory::default();
        history.record_window(48_000, 48_000, 10.0, 1.0);
        history.record_window(48_000, 48_000, 1.0, 10.0);
        let labels: Vec<_> = [(0.0, 1000.0), (1000.0, 2000.0)]
            .into_iter()
            .map(|(start, end)| history.source_for_segment(start, end).0)
            .collect();
        assert_eq!(labels, vec![DeviceType::Microphone, DeviceType::System]);
    }

    #[test]
    fn a_cut_retains_tail_windows_without_inheriting_the_previous_segment() {
        let mut history = SourceAttributionHistory::default();
        history.record_window(16_000, 16_000, 100.0, 1.0);
        history.record_window(16_000, 16_000, 1.0, 10.0);
        // The cut is inside the second window, whose tail remains in the VAD.
        assert_eq!(
            history.source_for_segment(0.0, 1500.0).0,
            DeviceType::Microphone
        );
        history.record_window(16_000, 16_000, 1.0, 10.0);
        let (source, mic, system) = history.source_for_segment(1500.0, 3000.0);
        assert_eq!(source, DeviceType::System);
        assert_eq!((mic, system), (1.5, 15.0));
    }

    #[test]
    fn short_microphone_segment_after_system_audio_weights_straddling_windows() {
        let mut history = SourceAttributionHistory::default();
        for _ in 0..20 {
            history.record_window(9_600, 16_000, 1.0, 40.0);
        }
        history.record_window(9_600, 16_000, 10.0, 1.0);
        history.record_window(9_600, 16_000, 10.0, 1.0);
        // Only 50 ms of the last system window and 550 ms of the last mic
        // window belong to this 1.2 s segment.
        let (source, mic, system) = history.source_for_segment(11_950.0, 13_150.0);
        assert_eq!(source, DeviceType::Microphone);
        assert!((mic - 19.25).abs() < 1e-5);
        assert!((system - 5.25).abs() < 1e-5);
    }

    #[test]
    fn zero_length_windows_and_non_overlapping_segments_have_zero_weight() {
        let mut history = SourceAttributionHistory::default();
        history.record_window(16_000, 16_000, 10.0, 1.0);
        history.windows.push_back(super::SourceAttributionWindow {
            start_timestamp_ms: 500.0,
            end_timestamp_ms: 500.0,
            mic_snr: 0.0,
            sys_snr: 1000.0,
        });
        assert_eq!(
            history.source_for_segment(0.0, 1000.0),
            (DeviceType::Microphone, 10.0, 1.0)
        );
        for (start, end) in [(1000.0, 2000.0), (500.0, 500.0), (700.0, 500.0)] {
            assert_eq!(
                history.source_for_segment(start, end),
                (DeviceType::Microphone, 0.0, 0.0)
            );
        }
    }

    #[test]
    fn attribution_keeps_the_existing_zero_sum_fallback() {
        let mut history = SourceAttributionHistory::default();
        assert_eq!(
            history.source_for_segment(0.0, 1000.0).0,
            DeviceType::Microphone
        );
        history.record_window(16_000, 16_000, 0.0, 0.0);
        assert_eq!(
            history.source_for_segment(0.0, 1000.0).0,
            DeviceType::Microphone
        );
        assert_eq!(
            history.source_for_segment(1000.0, 2000.0).0,
            DeviceType::Microphone
        );
    }

    #[test]
    fn attribution_history_discards_windows_older_than_150_seconds() {
        let mut history = SourceAttributionHistory::default();
        for _ in 0..200 {
            history.record_window(16_000, 16_000, 1.0, 10.0);
        }
        assert_eq!(history.windows.len(), 150);
        assert_eq!(history.source_for_segment(0.0, 50_000.0).1, 0.0);
        assert_eq!(
            history.source_for_segment(50_000.0, 51_000.0).0,
            DeviceType::System
        );
    }
}

#[cfg(test)]
mod system_audio_silence_tests {
    use super::{audible_system_audio, DeviceType, SystemAudioSilenceTracker};
    use std::time::Duration;

    #[test]
    fn a_stalled_microphone_warns_once_and_silent_samples_or_pause_clear_it() {
        let mut watch = super::MicrophoneStreamWatch::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        assert_eq!(watch.update(at(0), true, false), None);
        assert_eq!(watch.update(at(14), true, false), None);
        assert_eq!(watch.update(at(15), true, false), Some(true));
        assert_eq!(watch.update(at(30), true, false), None);
        assert_eq!(watch.update(at(31), true, true), Some(false));
        assert_eq!(watch.update(at(45), true, false), None);
        assert_eq!(watch.update(at(46), true, false), Some(true));
        assert_eq!(watch.update(at(47), true, false), None);
        assert_eq!(watch.update(at(48), true, true), Some(false));
        assert_eq!(watch.update(at(63), true, false), Some(true));
        assert_eq!(watch.update(at(64), false, false), Some(false));
        assert_eq!(watch.update(at(600), true, false), None);
        assert_eq!(watch.update(at(615), true, false), Some(true));
    }

    #[test]
    fn a_system_only_recording_does_not_start_the_microphone_timer() {
        let mut watch = super::MicrophoneStreamWatch::default();
        let start = std::time::Instant::now();
        assert_eq!(watch.update(start, false, false), None);
        assert_eq!(watch.update(start + Duration::from_secs(30), false, false), None);
    }

    #[test]
    fn only_audible_system_samples_reset_the_system_silence_timer() {
        assert!(!audible_system_audio(&DeviceType::Microphone, &[0.5]));
        assert!(!audible_system_audio(&DeviceType::System, &[0.0, 0.0005]));
        assert!(audible_system_audio(&DeviceType::System, &[0.0, -0.002]));
    }

    #[test]
    fn a_quiet_period_requires_a_selected_device_and_the_grace_period() {
        let mut tracker = SystemAudioSilenceTracker::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        assert!(!tracker.update(at(0), false, false));
        assert!(!tracker.update(at(15), true, false));
        assert!(!tracker.update(at(29), true, false));
        assert!(tracker.update(at(30), true, false));
        assert!(!tracker.update(at(31), true, false));
    }

    #[test]
    fn system_audio_can_go_quiet_again_and_pause_resets_the_clock() {
        let mut tracker = SystemAudioSilenceTracker::default();
        let start = std::time::Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        assert!(!tracker.update(at(0), true, true));
        assert!(tracker.update(at(15), true, false));
        assert!(!tracker.update(at(16), true, true));
        assert!(tracker.update(at(31), true, false));
        assert!(!tracker.update(at(32), false, false));
        assert!(!tracker.update(at(120), true, false));
        assert!(tracker.update(at(135), true, false));
    }
}

#[cfg(test)]
mod queue_failure_tests {
    use super::*;

    #[test]
    fn mixer_continues_when_only_one_source_delivers() {
        let mut mixer = AudioMixerRingBuffer::new(48_000);
        mixer.add_samples(DeviceType::System, vec![0.5; 28_800]);
        assert!(mixer.extract_window().is_none());
        mixer.add_samples(DeviceType::System, vec![0.5; 4_800]);
        let (mic, system) = mixer.extract_window().unwrap();
        assert_eq!(mic, vec![0.0; 28_800]);
        assert_eq!(system, vec![0.5; 28_800]);
        assert_eq!(mixer.system_buffer.len(), 4_800);
    }

    #[test]
    fn mixer_reports_discarded_samples_and_keeps_the_newest_audio() {
        let mut mixer = AudioMixerRingBuffer::new(48_000);
        let mut samples = vec![0.5; 7];
        samples.extend(vec![0.25; mixer.max_buffer_size]);
        assert_eq!(mixer.add_samples(DeviceType::System, samples), 7);
        assert_eq!(mixer.system_buffer.len(), mixer.max_buffer_size);
        assert_eq!(mixer.system_buffer.front(), Some(&0.25));
        assert!(mixer.mic_buffer.is_empty());
    }

    #[tokio::test]
    async fn audio_only_flush_preserves_audio_without_vad_or_transcription() {
        let (_capture, receiver) = mpsc::channel(1);
        let (sender, _transcripts, metrics) =
            super::super::transcription::queue::transcription_queue().unwrap();
        let (recording, mut saved, _) =
            super::super::transcription::queue::transcription_queue().unwrap();
        let kind = super::super::device_detection::InputDeviceKind::Unknown;
        let mut pipeline = AudioPipeline::new(
            receiver,
            sender,
            RecordingState::new(),
            0,
            48000,
            "Microphone".into(),
            kind,
            "System audio".into(),
            kind,
            false,
        );
        assert!(pipeline.vad_processor.is_none());
        pipeline.recording_sender_for_mixed =
            Some(BufferedSpool::new(recording, pipeline.state.clone(), true));
        pipeline
            .preserve_mixed_audio(&vec![0.1; 4800], 0.0)
            .await
            .unwrap();
        pipeline.flush_remaining_audio().await.unwrap();
        let chunk = saved.recv().await.unwrap().unwrap().chunk;
        assert_eq!(chunk.data.len(), 4800);
        assert_eq!(chunk.sample_rate, 48000);
        assert_eq!(metrics.snapshot().chunks_in_queue, 0);
        pipeline
            .recording_sender_for_mixed
            .as_mut()
            .unwrap()
            .finish()
            .await;
        pipeline.transcription_sender.finish().await;
    }

    #[test]
    fn queue_write_warning_is_throttled_and_resets_after_recovery() {
        let mut tracker = QueueWriteFailureTracker::default();
        assert!(!tracker.record_failure());
        assert!(!tracker.record_failure());
        assert!(tracker.record_failure());
        assert!(!tracker.record_failure());

        tracker.record_success();
        assert!(!tracker.record_failure());
        assert!(!tracker.record_failure());
        assert!(tracker.record_failure());
    }
}
