use anyhow::{Context, Result};
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
use super::recording_state::{
    AudioChunk, AudioError, DeviceType, RecordingState, CAPTURE_HANDOFF_MS,
};
use super::transcription::queue::{BufferedSpool, TranscriptionQueueSender};
use super::vad::ContinuousVadProcessor;

#[cfg(test)]
#[path = "capture_bench.rs"]
mod capture_bench;

/// Live VAD max-segment cap (ms). Set per active transcription provider at
/// recording start; the pipeline reads it when creating the VAD. Default 6s.
static LIVE_MAX_SEGMENT_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(6_000);
const LIVE_VAD_REDEMPTION_MS: u32 = 400;
pub(super) const CAPTURE_SAMPLE_RATE: u32 = 48_000;
const RESAMPLER_CHUNK_SIZE: usize = 512;
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

struct TimedSamples {
    start: u64,
    samples: VecDeque<f32>,
}

#[derive(Default)]
struct SourceTimeline {
    pending: VecDeque<TimedSamples>,
    buffered: usize,
    end: u64,
}

impl SourceTimeline {
    fn add(&mut self, cursor: u64, start: u64, data: Vec<f32>, capacity: usize) -> usize {
        // Source-local overlap can be introduced by correcting device-clock
        // drift. Keep the earlier samples at those overlapping positions.
        let unique_start = start.max(self.end);
        let skip = unique_start.max(cursor).saturating_sub(start) as usize;
        let late = cursor
            .saturating_sub(unique_start)
            .min((start + data.len() as u64).saturating_sub(unique_start))
            as usize;
        if skip >= data.len() {
            return late;
        }
        self.end = start + data.len() as u64;
        let mut samples = VecDeque::from(data);
        samples.drain(..skip);
        self.buffered += samples.len();
        self.pending.push_back(TimedSamples {
            start: start + skip as u64,
            samples,
        });
        let overflow = self.buffered.saturating_sub(capacity);
        self.discard(overflow);
        late + overflow
    }

    fn discard(&mut self, mut count: usize) {
        self.buffered -= count;
        while count > 0 {
            let block = self.pending.front_mut().expect("buffer has samples");
            let removed = count.min(block.samples.len());
            block.samples.drain(..removed);
            block.start += removed as u64;
            count -= removed;
            if block.samples.is_empty() {
                self.pending.pop_front();
            }
        }
    }

    fn window(&mut self, start: u64, len: usize) -> Vec<f32> {
        let mut pcm = vec![0.0; len];
        let end = start + len as u64;
        while let Some(block) = self.pending.front_mut() {
            if block.start >= end {
                break;
            }
            let count = block.samples.len().min((end - block.start) as usize);
            let offset = (block.start - start) as usize;
            for (output, sample) in pcm[offset..offset + count]
                .iter_mut()
                .zip(block.samples.drain(..count))
            {
                *output = sample;
            }
            block.start += count as u64;
            self.buffered -= count;
            if block.samples.is_empty() {
                self.pending.pop_front();
            }
        }
        pcm
    }
}

/// The cursor is the number of samples already emitted to both VAD and saving.
/// The two sparse source queues share that sample clock and the same deadline.
struct AudioMixerRingBuffer {
    mic: SourceTimeline,
    system: SourceTimeline,
    cursor: u64,
    window_size_samples: usize,
    source_grace_samples: u64,
    max_buffer_size: usize,
}

impl AudioMixerRingBuffer {
    fn new(rate: u32) -> Self {
        let window_size_samples = rate as usize * 600 / 1_000;
        Self {
            mic: SourceTimeline::default(),
            system: SourceTimeline::default(),
            cursor: 0,
            window_size_samples,
            source_grace_samples: (u64::from(rate) * u64::from(CAPTURE_HANDOFF_MS)).div_ceil(1_000),
            max_buffer_size: window_size_samples * 8,
        }
    }

    fn add_samples(&mut self, device: DeviceType, start: u64, samples: Vec<f32>) -> usize {
        let source = match device {
            DeviceType::Microphone => &mut self.mic,
            DeviceType::System => &mut self.system,
        };
        source.add(self.cursor, start, samples, self.max_buffer_size)
    }

    fn extract_window(
        &mut self,
        idle_clock: u64,
        allow_padding: bool,
        protected: [bool; 2],
    ) -> Option<(u64, Vec<f32>, Vec<f32>)> {
        let end = self.cursor + self.window_size_samples as u64;
        let ready = self.mic.end >= end && self.system.end >= end;
        let can_pad = allow_padding
            && (!protected[0] || self.mic.end >= end)
            && (!protected[1] || self.system.end >= end)
            && self.mic.end.max(self.system.end).max(idle_clock) >= end + self.source_grace_samples;
        (ready || can_pad).then(|| self.take_window(self.window_size_samples))
    }

    fn extract_tail(&mut self, lost_through: u64) -> Option<(u64, Vec<f32>, Vec<f32>)> {
        let end = self.mic.end.max(self.system.end).max(lost_through);
        let count = end
            .saturating_sub(self.cursor)
            .min(self.window_size_samples as u64);
        (count > 0).then(|| self.take_window(count as usize))
    }

    fn take_window(&mut self, samples: usize) -> (u64, Vec<f32>, Vec<f32>) {
        let start = self.cursor;
        self.cursor += samples as u64;
        (
            start,
            self.mic.window(start, samples),
            self.system.window(start, samples),
        )
    }
}

/// Adds both sources at unity gain and hard-clips peaks to the PCM range.
struct ProfessionalAudioMixer;

impl ProfessionalAudioMixer {
    fn new(_sample_rate: u32) -> Self {
        Self
    }

    fn mix_window(&mut self, mic_window: &[f32], sys_window: &[f32]) -> Vec<f32> {
        // Handle different lengths (already padded by extract_window, but defensive)
        let max_len = mic_window.len().max(sys_window.len());
        let mut mixed = Vec::with_capacity(max_len);

        for i in 0..max_len {
            let mic = mic_window.get(i).copied().unwrap_or(0.0);
            let sys = sys_window.get(i).copied().unwrap_or(0.0);
            mixed.push((mic + sys).clamp(-1.0, 1.0));
        }

        mixed
    }
}

type TimedPcm = (f64, Vec<f32>);

#[derive(Default)]
struct CaptureClockSlew {
    error_samples: f64,
    applied_samples: i64,
    direction: i8,
    since_edit: usize,
}

impl CaptureClockSlew {
    fn observe(&mut self, timestamp_error: f64, packet_seconds: f64) {
        let error = timestamp_error * f64::from(CAPTURE_SAMPLE_RATE) - self.applied_samples as f64;
        let weight = packet_seconds / (0.5 + packet_seconds);
        self.error_samples += weight * (error - self.error_samples);
        // A 12 ms entry threshold and a 4 ms exit threshold keep jitter from
        // triggering edits and keep the correction moving once drift is real.
        if self.direction != 0 && self.error_samples * f64::from(self.direction) <= 192.0 {
            self.direction = 0;
        }
        if self.direction == 0 && self.error_samples.abs() > 576.0 {
            self.direction = if self.error_samples > 0.0 { 1 } else { -1 };
        }
    }

    fn apply(&mut self, samples: &mut Vec<f32>) {
        // At most one edit per 2,400 output samples, about 420 ppm. Interpolate
        // the insertion or the splice to limit single-sample discontinuities.
        const SLEW_INTERVAL: usize = 2_400;
        self.since_edit = (self.since_edit + samples.len()).min(SLEW_INTERVAL);
        if self.direction == 0 || self.since_edit < SLEW_INTERVAL || samples.len() < 3 {
            return;
        }
        let middle = samples.len() / 2;
        if self.direction > 0 {
            let between = (samples[middle - 1] + samples[middle]) * 0.5;
            samples.insert(middle, between);
        } else {
            samples[middle + 1] = (samples[middle] + samples[middle + 1]) * 0.5;
            samples.remove(middle);
        }
        self.applied_samples += i64::from(self.direction);
        self.since_edit = 0;
    }
}

/// The first sample of a continuous source run anchors the resampler output.
/// Ordinary clock drift adjusts output gradually while the filter stays warm.
struct CaptureResampler {
    engine: Option<SincFixedIn<f32>>,
    pending: Vec<f32>,
    start: Option<f64>,
    input_frames: usize,
    resampled_frames: usize,
    output_frames: usize,
    revision: u64,
    slew: CaptureClockSlew,
}

impl CaptureResampler {
    fn new(engine: Option<SincFixedIn<f32>>) -> Self {
        Self {
            engine,
            pending: Vec::with_capacity(RESAMPLER_CHUNK_SIZE * 2),
            start: None,
            input_frames: 0,
            resampled_frames: 0,
            output_frames: 0,
            revision: 0,
            slew: CaptureClockSlew::default(),
        }
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset();
        }
        self.pending.clear();
        self.start = None;
        self.input_frames = 0;
        self.resampled_frames = 0;
        self.output_frames = 0;
        self.slew = CaptureClockSlew::default();
    }

    fn process(
        &mut self,
        mono: Vec<f32>,
        rate: u32,
        mut timestamp: f64,
        revision: u64,
        missing_frames: usize,
    ) -> [Option<TimedPcm>; 2] {
        let mut tail = None;
        if let Some(start) = self.start {
            let expected = start + self.input_frames as f64 / f64::from(rate);
            let gap = missing_frames > 0 && revision == self.revision;
            if gap || revision != self.revision {
                tail = self.finish(rate);
                let placed_end = start + self.output_frames as f64 / f64::from(CAPTURE_SAMPLE_RATE);
                self.reset();
                if gap {
                    timestamp = timestamp.max(placed_end + missing_frames as f64 / f64::from(rate));
                } else {
                    timestamp = timestamp.max(placed_end);
                }
            } else {
                self.slew
                    .observe(timestamp - expected, mono.len() as f64 / f64::from(rate));
            }
        }

        self.start.get_or_insert(timestamp);
        self.revision = revision;
        self.input_frames += mono.len();
        let mut data = if let Some(engine) = &mut self.engine {
            self.pending.extend(mono);
            let mut output = Vec::new();
            let mut processed = 0;
            for frame in self.pending.chunks_exact(RESAMPLER_CHUNK_SIZE) {
                match engine.process(&[frame], None) {
                    Ok(waves) => output.extend_from_slice(&waves[0]),
                    Err(error) => warn!("Capture resampler skipped an input block: {error}"),
                }
                processed += frame.len();
            }
            self.pending.drain(..processed);
            output
        } else if rate == CAPTURE_SAMPLE_RATE {
            mono
        } else {
            super::audio_processing::resample_audio(&mono, rate, CAPTURE_SAMPLE_RATE)
        };
        self.resampled_frames += data.len();
        self.slew.apply(&mut data);
        [tail, self.place(data)]
    }

    fn finish(&mut self, rate: u32) -> Option<TimedPcm> {
        let engine = self.engine.as_mut()?;
        let expected = self.input_frames * CAPTURE_SAMPLE_RATE as usize / rate as usize;
        let mut remaining = expected.saturating_sub(self.resampled_frames);
        // At most one partial input block and the sinc filter's tail remain.
        let mut tail = Vec::new();
        let mut frame = [0.0; RESAMPLER_CHUNK_SIZE];
        frame[..self.pending.len()].copy_from_slice(&self.pending);
        for _ in 0..3 {
            if remaining == 0 {
                break;
            }
            let Ok(waves) = engine.process(&[&frame[..]], None) else {
                break;
            };
            let count = remaining.min(waves[0].len());
            tail.extend_from_slice(&waves[0][..count]);
            remaining -= count;
            frame.fill(0.0);
        }
        self.resampled_frames += tail.len();
        self.place(tail)
    }

    fn place(&mut self, data: Vec<f32>) -> Option<TimedPcm> {
        if data.is_empty() {
            return None;
        }
        let start = self.start? + self.output_frames as f64 / f64::from(CAPTURE_SAMPLE_RATE);
        self.output_frames += data.len();
        Some((start, data))
    }
}

/// Capture processing and source-local DSP state.
#[derive(Clone)]
pub struct AudioCapture {
    device: Arc<AudioDevice>,
    state: Arc<RecordingState>,
    sample_rate: u32, // Original device sample rate
    channels: u16,
    chunk_counter: Arc<std::sync::atomic::AtomicU64>,
    device_type: DeviceType,
    resampler: Arc<std::sync::Mutex<CaptureResampler>>,
    // Audio enhancement processors (microphone only)
    noise_suppressor: Arc<std::sync::Mutex<Option<NoiseSuppressionProcessor>>>,
    high_pass_filter: Arc<std::sync::Mutex<Option<HighPassFilter>>>,
    // EBU R128 normalizer for microphone audio (per-device, stateful)
    normalizer: Arc<std::sync::Mutex<Option<LoudnessNormalizer>>>,
}

impl AudioCapture {
    pub(super) fn device_type(&self) -> DeviceType {
        self.device_type.clone()
    }

    pub fn new(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        sample_rate: u32,
        channels: u16,
        device_type: DeviceType,
    ) -> Self {
        // CRITICAL FIX: Detect if resampling is needed
        // Pipeline expects 48kHz, but Bluetooth devices often report 8kHz, 16kHz, or 44.1kHz
        const TARGET_SAMPLE_RATE: u32 = CAPTURE_SAMPLE_RATE;
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
            resampler: Arc::new(std::sync::Mutex::new(CaptureResampler::new(resampler))),
            noise_suppressor: Arc::new(std::sync::Mutex::new(noise_suppressor)),
            high_pass_filter: Arc::new(std::sync::Mutex::new(high_pass_filter)),
            normalizer: Arc::new(std::sync::Mutex::new(normalizer)),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn process_audio_data(&self, data: &[f32]) {
        let frames = data.len() / usize::from(self.channels);
        let duration =
            std::time::Duration::from_secs_f64(frames as f64 / f64::from(self.sample_rate));
        let Some(stamp) = self.state.admit_capture(
            std::time::Instant::now() - duration,
            frames,
            self.sample_rate,
        ) else {
            return;
        };
        let data = &data[stamp.skipped_frames * usize::from(self.channels)..];
        self.process_captured_data(data, stamp.resume_revision, stamp.seconds, 0, |chunk| {
            self.state.send_audio_chunk(chunk)
        });
    }

    #[cfg(test)]
    fn process_fixture_data(&self, data: &[f32], timestamp: f64) {
        if self.state.is_active() {
            self.process_captured_data(data, self.state.resume_revision(), timestamp, 0, |chunk| {
                self.state.send_audio_chunk(chunk)
            });
        }
    }

    /// Process a packet admitted by the capture callback, even during Stop.
    pub(super) fn process_captured_data(
        &self,
        data: &[f32],
        resume_revision: u64,
        timestamp: f64,
        missing_frames: usize,
        mut send: impl FnMut(AudioChunk) -> Result<()>,
    ) {
        if data.is_empty() {
            return;
        }

        let mono = if self.channels > 1 {
            audio_to_mono(data, self.channels)
        } else {
            data.to_vec()
        };
        let audio = self.resampler.lock().unwrap().process(
            mono,
            self.sample_rate,
            timestamp,
            resume_revision,
            missing_frames,
        );
        for span in audio.into_iter().flatten() {
            self.emit_processed(span, &mut send);
        }
    }

    pub(super) fn finish_capture(&self, send: impl FnOnce(AudioChunk) -> Result<()>) {
        let tail = self.resampler.lock().unwrap().finish(self.sample_rate);
        if let Some(span) = tail {
            self.emit_processed(span, send);
        }
    }

    fn emit_processed(
        &self,
        (timestamp, mut mono_data): TimedPcm,
        send: impl FnOnce(AudioChunk) -> Result<()>,
    ) {
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

        let audio_chunk = AudioChunk {
            data: mono_data,
            sample_rate: CAPTURE_SAMPLE_RATE,
            timestamp,
            chunk_id,
            device_type: self.device_type.clone(),
        };

        if let Err(e) = send(audio_chunk) {
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
            debug!("Sent audio chunk {}", chunk_id);
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
    recording_buffer_start: u64,
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
        vad_processor: Result<Option<ContinuousVadProcessor>>,
    ) -> Result<Self> {
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

        let vad_processor = vad_processor
            .context("Could not start speech detection. Try an audio-only recording.")?;

        // Initialize professional audio mixing components
        let ring_buffer = AudioMixerRingBuffer::new(sample_rate);
        let mixer = ProfessionalAudioMixer::new(sample_rate);

        // Note: target_chunk_duration_ms is ignored - VAD controls segmentation now
        let _ = target_chunk_duration_ms;

        Ok(Self {
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
            recording_buffer_start: 0,
            recording_sender_for_mixed: None, // Will be set by manager
            queue_write_failures: QueueWriteFailureTracker::default(),
            levels: super::recording_levels::LevelMeter::new(std::time::Instant::now()),
        })
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
            let idle = match tokio::time::timeout(
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

                    let start = (chunk.timestamp * f64::from(self.sample_rate)).round() as u64;
                    let dropped =
                        self.ring_buffer
                            .add_samples(chunk.device_type, start, chunk.data);
                    if dropped > 0 {
                        self.state.record_sample_loss(dropped);
                    }
                    false
                }
                Ok(None) => {
                    info!(
                        "Audio pipeline: sender closed after processing {} chunks",
                        self.processed_chunks
                    );
                    break;
                }
                Err(_) => true,
            };

            let now = std::time::Instant::now();
            let clock = self
                .state
                .capture_seconds(now)
                .map_or(0, |seconds| (seconds * f64::from(self.sample_rate)) as u64);
            // Packetless loopback can advance on idle ticks. A source with
            // admitted input or recent callbacks gets time to deliver its DSP
            // output. Take the queue check after reading worker progress.
            let idle_clock = if idle
                && (self.state.get_microphone_device().is_some()
                    || self.state.get_system_device().is_some())
            {
                clock
            } else {
                0
            };
            loop {
                let protected = self
                    .state
                    .sources_with_pending_capture(clock, self.ring_buffer.source_grace_samples);
                let Some((start, mic, system)) = self.ring_buffer.extract_window(
                    idle_clock,
                    self.receiver.is_empty(),
                    protected,
                ) else {
                    break;
                };
                self.process_mixed_window(start, &mic, &system).await?;
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

    async fn process_mixed_window(
        &mut self,
        start: u64,
        mic: &[f32],
        system: &[f32],
    ) -> Result<()> {
        let mixed = self.mixer.mix_window(mic, system);
        self.preserve_mixed_audio(&mixed, start).await?;
        if self.vad_processor.is_none() {
            return Ok(());
        }

        self.record_attribution_window(mic, system, mixed.len());
        let segments = match self.vad_processor.as_mut().unwrap().process_audio(&mixed) {
            Ok(segments) => segments,
            Err(_) => {
                self.transcription_sender.mark_failed();
                self.state.report_warning("Speech detection failed. Recorded audio is retained; use Retranscribe after stopping.");
                return Ok(());
            }
        };
        for segment in segments {
            if segment.samples.len() < 800 {
                continue;
            }
            let device_type =
                self.segment_source(segment.start_timestamp_ms, segment.end_timestamp_ms);
            let chunk = AudioChunk {
                data: segment.samples,
                sample_rate: 16_000,
                timestamp: segment.start_timestamp_ms / 1_000.0,
                chunk_id: self.chunk_id_counter,
                device_type,
            };
            if self.enqueue_transcription_chunk(chunk).await {
                self.chunk_id_counter += 1;
            }
        }
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

    async fn preserve_mixed_audio(&mut self, samples: &[f32], start: u64) -> Result<()> {
        if self.recording_sender_for_mixed.is_none() {
            return Ok(());
        }
        if self.recording_buffer.is_empty() {
            self.recording_buffer_start = start;
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
            let start = self.recording_buffer_start;
            let chunk = AudioChunk {
                data: std::mem::take(&mut self.recording_buffer),
                sample_rate: self.sample_rate,
                timestamp: start as f64 / f64::from(self.sample_rate),
                chunk_id: start,
                device_type: DeviceType::Microphone,
            };
            if sender.send_capture(chunk, start).await.is_err() {
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

        let lost_through = self.state.capture_gap_end();
        while let Some((start, mic, system)) = self.ring_buffer.extract_tail(lost_through) {
            self.process_mixed_window(start, &mic, &system).await?;
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

        let vad_processor = transcribe
            .then(|| {
                ContinuousVadProcessor::new_with_max_segment_duration(
                    sample_rate,
                    LIVE_VAD_REDEMPTION_MS,
                    Some(LIVE_MAX_SEGMENT_MS.load(std::sync::atomic::Ordering::Relaxed)),
                )
            })
            .transpose();

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
            vad_processor,
        )?;
        state.set_audio_sender(audio_sender.clone());

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
        assert_eq!(
            watch.update(start + Duration::from_secs(30), false, false),
            None
        );
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
    fn resuming_clears_pending_input_and_filter_history_from_the_resampler() {
        let state = RecordingState::new();
        let (sender, mut receiver) = mpsc::channel(8);
        state.set_audio_sender(sender);
        state.start_recording().unwrap();
        let capture = AudioCapture::new(
            Arc::new(AudioDevice::new(
                "Fixture output".into(),
                super::super::devices::DeviceType::Output,
            )),
            state.clone(),
            16_000,
            1,
            DeviceType::System,
        );
        capture.process_fixture_data(&[0.8; 768], 0.0);
        assert!(receiver.try_recv().is_ok());
        assert!(!capture.resampler.lock().unwrap().pending.is_empty());
        state.pause_recording().unwrap();
        capture.process_fixture_data(&[0.8; 128], 0.048);
        state.resume_recording().unwrap();
        capture.process_fixture_data(&[0.0; 512], 0.048);
        let before_pause = receiver.try_recv().unwrap();
        assert!(before_pause.timestamp < 0.048);
        let resumed = receiver.try_recv().unwrap();
        assert_eq!(resumed.timestamp, 0.048);
        assert!(!resumed.data.is_empty());
        assert!(resumed.data.iter().all(|&sample| sample == 0.0));
        assert!(capture.resampler.lock().unwrap().pending.is_empty());
    }

    #[test]
    fn mixing_preserves_unity_gain_and_clips_overlapping_peaks() {
        let mut mixer = ProfessionalAudioMixer::new(48_000);
        let mixed = mixer.mix_window(&[0.25, 0.75, -0.75], &[0.25, 0.75, -0.75, 0.5]);
        assert_eq!(mixed, vec![0.5, 1.0, -1.0, 0.5]);
    }

    #[tokio::test]
    async fn failed_vad_initialization_is_returned_to_recording_start() {
        let (_capture, receiver) = mpsc::channel(1);
        let (sender, mut transcripts, _) =
            super::super::transcription::queue::transcription_queue().unwrap();
        let kind = super::super::device_detection::InputDeviceKind::Unknown;
        let result = AudioPipeline::new(
            receiver,
            sender,
            RecordingState::new(),
            0,
            48000,
            "Microphone".into(),
            kind,
            "System audio".into(),
            kind,
            Err(anyhow::anyhow!("test VAD initialization failure")),
        );
        let error = result.err().expect("expected VAD initialization error");
        assert!(error.to_string().contains("speech detection"));
        assert!(transcripts.recv().await.unwrap().is_none());
    }

    #[test]
    fn mixer_continues_when_only_one_source_delivers() {
        let mut mixer = AudioMixerRingBuffer::new(48_000);
        mixer.add_samples(DeviceType::System, 0, vec![0.5; 28_800]);
        assert!(mixer.extract_window(0, true, [false; 2]).is_none());
        mixer.add_samples(DeviceType::System, 28_800, vec![0.5; 19_200]);
        let (start, mic, system) = mixer.extract_window(0, true, [false; 2]).unwrap();
        assert_eq!(start, 0);
        assert_eq!(mic, vec![0.0; 28_800]);
        assert_eq!(system, vec![0.5; 28_800]);
        assert_eq!(mixer.system.buffered, 19_200);
    }

    #[test]
    fn mixer_reports_discarded_samples_and_keeps_the_newest_audio() {
        let mut mixer = AudioMixerRingBuffer::new(48_000);
        let mut samples = vec![0.5; 7];
        samples.extend(vec![0.25; mixer.max_buffer_size]);
        assert_eq!(mixer.add_samples(DeviceType::System, 0, samples), 7);
        assert_eq!(mixer.system.buffered, mixer.max_buffer_size);
        let (_, _, system) = mixer.extract_window(0, true, [false; 2]).unwrap();
        assert_eq!(system[..7], [0.0; 7]);
        assert_eq!(system[7], 0.25);
        assert_eq!(mixer.mic.buffered, 0);
    }

    #[test]
    fn late_source_and_gaps_stay_at_their_capture_positions() {
        let mut mixer = AudioMixerRingBuffer::new(1_000);
        mixer.add_samples(DeviceType::System, 0, vec![0.1; 1_000]);
        assert_eq!(
            mixer.extract_window(0, true, [false; 2]).unwrap().1,
            vec![0.0; 600]
        );
        assert_eq!(
            mixer.add_samples(DeviceType::Microphone, 580, vec![0.2; 30]),
            20
        );
        mixer.add_samples(DeviceType::Microphone, 700, vec![0.3; 100]);
        mixer.add_samples(DeviceType::System, 1_000, vec![0.1; 600]);

        let (start, mic, system) = mixer.extract_window(0, true, [false; 2]).unwrap();
        assert_eq!(start, 600);
        assert_eq!(mic[..10], [0.2; 10]);
        assert_eq!(mic[10..100], [0.0; 90]);
        assert_eq!(mic[100..200], [0.3; 100]);
        assert_eq!(mic[200..], [0.0; 400]);
        assert_eq!(system, vec![0.1; 600]);
        let (start, _, system) = mixer.extract_tail(1_604).unwrap();
        assert_eq!(start, 1_200);
        assert_eq!(system[..400], [0.1; 400]);
        assert_eq!(system[400..], [0.0; 4]);
    }

    #[test]
    fn packetless_capture_advances_after_the_same_window_grace_period() {
        let mut mixer = AudioMixerRingBuffer::new(1_000);
        assert!(mixer.extract_window(999, true, [false; 2]).is_none());
        let (start, mic, system) = mixer.extract_window(1_000, true, [false; 2]).unwrap();
        assert_eq!(start, 0);
        assert_eq!(mic, vec![0.0; 600]);
        assert_eq!(mic, system);
        assert!(mixer.extract_window(1_000, true, [false; 2]).is_none());
    }

    #[test]
    fn a_worker_delayed_by_200_ms_still_contributes_its_admitted_audio() {
        use super::super::recording_state::CaptureProgress;
        use std::sync::atomic::Ordering;

        let state = RecordingState::new();
        let mic_progress = Arc::new(CaptureProgress::default());
        state.register_capture_progress(DeviceType::Microphone, &mic_progress);
        mic_progress.admitted_end.store(600, Ordering::Release);
        mic_progress.pending_packets.store(1, Ordering::Release);

        let mut mixer = AudioMixerRingBuffer::new(1_000);
        mixer.add_samples(DeviceType::System, 0, vec![0.25; 800]);
        let protected = state.sources_with_pending_capture(800, mixer.source_grace_samples);
        assert!(mixer.extract_window(800, true, protected).is_none());
        // A worker that has already admitted samples keeps the cursor behind
        // its pending output even if the wall clock runs past the grace period.
        let protected = state.sources_with_pending_capture(1_050, mixer.source_grace_samples);
        assert!(mixer.extract_window(1_050, true, protected).is_none());

        assert_eq!(
            mixer.add_samples(DeviceType::Microphone, 0, vec![0.5; 600]),
            0
        );
        mic_progress.pending_packets.store(0, Ordering::Release);
        let (start, mic, system) = mixer.extract_window(1_050, true, protected).unwrap();
        assert_eq!(start, 0);
        assert_eq!(mic, vec![0.5; 600]);
        assert_eq!(system, vec![0.25; 600]);
    }

    #[test]
    fn the_source_clock_keeps_a_known_dropped_block() {
        let mut capture = CaptureResampler::new(None);
        capture.process(vec![0.1; 480], 48_000, 0.0, 0, 0);
        let [_, span] = capture.process(vec![0.1; 480], 48_000, 0.01, 0, 48);
        assert!((span.unwrap().0 - 0.011).abs() < 1e-9);
    }

    #[test]
    fn resampled_capture_slews_100_ppm_drift_without_resetting_or_losing_input() {
        let rate = 8_000;
        let params = SincInterpolationParameters {
            sinc_len: 32,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 32,
            window: WindowFunction::BlackmanHarris2,
        };
        let engine = SincFixedIn::<f32>::new(6.0, 2.0, params, RESAMPLER_CHUNK_SIZE, 1).unwrap();
        let mut capture = CaptureResampler::new(Some(engine));
        let mut output = 0;
        let mut input = 0;
        for tick in 0..2_400 {
            let nominal = input as f64 / f64::from(rate);
            let jitter = if tick % 3 == 1 {
                0.003
            } else if tick % 3 == 2 {
                -0.003
            } else {
                0.0
            };
            let [tail, span] = capture.process(
                vec![0.25; RESAMPLER_CHUNK_SIZE],
                rate,
                nominal * 1.0001 + jitter,
                0,
                0,
            );
            assert!(tail.is_none(), "clock correction reset the resampler");
            let (start, audio) = span.unwrap();
            assert_eq!((start * 48_000.0).round() as usize, output);
            if tick > 0 {
                assert!(audio.iter().all(|sample| *sample > 0.20));
            }
            output += audio.len();
            input += RESAMPLER_CHUNK_SIZE;
        }
        let (start, tail) = capture.finish(rate).unwrap();
        assert_eq!((start * 48_000.0).round() as usize, output);
        output += tail.len();
        assert_eq!(capture.input_frames, input);
        assert_eq!(capture.resampled_frames, input * 6);
        assert!(capture.slew.applied_samples > 0);
        assert_eq!(output, input * 6 + capture.slew.applied_samples as usize);
        let target = input as f64 / f64::from(rate) * 1.0001;
        assert!((target - output as f64 / 48_000.0).abs() < 0.012);
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
            Ok(None),
        )
        .unwrap();
        assert!(pipeline.vad_processor.is_none());
        pipeline.recording_sender_for_mixed =
            Some(BufferedSpool::new(recording, pipeline.state.clone(), true));
        pipeline
            .preserve_mixed_audio(&vec![0.1; 4800], 0)
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
