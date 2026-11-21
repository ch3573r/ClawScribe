use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use silero_rs::{VadConfig, VadSession};
use std::collections::VecDeque;
use std::time::Duration;

/// Silero VAD requires 16 kHz mono.
const VAD_SAMPLE_RATE: u32 = 16000;
const VAD_FRAME_MS: u64 = 30;
const PRE_SPEECH_PAD_MS: u64 = 300;
const MIN_SPEECH_TIME_MS: u64 = 250;
// Keep the pre-roll and trim in batches during silence.
const IDLE_HISTORY_SAMPLES: usize = ms_to_samples(PRE_SPEECH_PAD_MS as usize);
const IDLE_TRIM_INTERVAL_SAMPLES: usize = ms_to_samples(1_000);
/// Fixed input frame size for the rubato resampler (variable input is buffered
/// up to this size, like the recording pipeline's resampler).
const VAD_RESAMPLE_CHUNK: usize = 512;

const fn ms_to_samples(ms: usize) -> usize {
    ms * VAD_SAMPLE_RATE as usize / 1000
}

fn samples_to_ms(samples: usize) -> f64 {
    samples as f64 / (VAD_SAMPLE_RATE / 1000) as f64
}

/// Resamples streaming audio to 16 kHz for the VAD using a persistent rubato
/// sinc resampler. A persistent resampler with input buffering is required:
/// constructing a fresh resampler per chunk amplifies energy and mis-sizes the
/// output, and the previous moving-average + linear-interpolation downsampler
/// aliased badly, which degraded speech detection. Falls back to a simple
/// downsampler only if rubato fails to initialise.
struct VadResampler {
    input_rate: u32,
    resampler: Option<SincFixedIn<f32>>,
    input_buffer: Vec<f32>,
    input_samples: u64,
    output_samples: u64,
}

impl VadResampler {
    fn new(input_rate: u32) -> Self {
        let resampler = if input_rate == VAD_SAMPLE_RATE {
            None
        } else {
            let ratio = VAD_SAMPLE_RATE as f64 / input_rate as f64;
            // Match the recording pipeline's quality/ratio heuristic.
            let (sinc_len, interpolation, oversampling_factor) = if ratio <= 0.5 {
                (512, SincInterpolationType::Cubic, 512)
            } else if ratio <= 1.0 {
                (384, SincInterpolationType::Linear, 384)
            } else {
                (256, SincInterpolationType::Linear, 256)
            };
            let params = SincInterpolationParameters {
                sinc_len,
                f_cutoff: 0.95,
                interpolation,
                oversampling_factor,
                window: WindowFunction::BlackmanHarris2,
            };
            match SincFixedIn::<f32>::new(ratio, 2.0, params, VAD_RESAMPLE_CHUNK, 1) {
                Ok(resampler) => {
                    info!(
                        "VAD resampler: rubato sinc {}Hz → {}Hz (chunk {})",
                        input_rate, VAD_SAMPLE_RATE, VAD_RESAMPLE_CHUNK
                    );
                    Some(resampler)
                }
                Err(e) => {
                    warn!("VAD resampler: rubato init failed ({e}); using fallback downsampler");
                    None
                }
            }
        };

        Self {
            input_rate,
            resampler,
            input_buffer: Vec::with_capacity(VAD_RESAMPLE_CHUNK * 2),
            input_samples: 0,
            output_samples: 0,
        }
    }

    /// Resample `samples` to 16 kHz. A trailing partial frame is buffered across
    /// calls, so chunked streaming input resamples with energy preserved.
    fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.input_rate == VAD_SAMPLE_RATE {
            return samples.to_vec();
        }

        if self.resampler.is_none() {
            return legacy_downsample(samples, self.input_rate);
        }

        self.input_samples += samples.len() as u64;
        self.input_buffer.extend_from_slice(samples);
        let mut output = Vec::new();
        let resampler = self.resampler.as_mut().expect("checked above");
        while self.input_buffer.len() >= VAD_RESAMPLE_CHUNK {
            let frame: Vec<f32> = self.input_buffer.drain(0..VAD_RESAMPLE_CHUNK).collect();
            match resampler.process(&[frame], None) {
                Ok(mut waves) => {
                    if let Some(channel) = waves.pop() {
                        output.extend_from_slice(&channel);
                    }
                }
                Err(e) => {
                    warn!("VAD resampler: rubato process failed ({e}); skipping frame");
                    break;
                }
            }
        }
        self.output_samples += output.len() as u64;
        output
    }

    fn flush(&mut self) -> Result<Vec<f32>> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let expected_samples =
            self.input_samples * u64::from(VAD_SAMPLE_RATE) / u64::from(self.input_rate);
        let remaining = expected_samples.saturating_sub(self.output_samples) as usize;
        let mut tail = Vec::new();
        // Drain each remaining input block, then pad as needed up to the input
        // clock's output length. A previous processing error can leave full blocks.
        for _ in 0..self.input_buffer.len().div_ceil(VAD_RESAMPLE_CHUNK) + 2 {
            if tail.len() == remaining {
                break;
            }
            let count = self.input_buffer.len().min(VAD_RESAMPLE_CHUNK);
            let mut frame = [0.0; VAD_RESAMPLE_CHUNK];
            frame[..count].copy_from_slice(&self.input_buffer[..count]);
            let waves = resampler.process(&[&frame[..]], None)?;
            self.input_buffer.drain(..count);
            tail.extend(waves[0].iter().take(remaining - tail.len()));
        }
        anyhow::ensure!(
            tail.len() == remaining,
            "VAD resampler did not drain its input"
        );
        self.output_samples += tail.len() as u64;
        Ok(tail)
    }
}

/// Simple anti-aliased downsampler used only when rubato is unavailable.
fn legacy_downsample(samples: &[f32], input_rate: u32) -> Vec<f32> {
    if input_rate == VAD_SAMPLE_RATE {
        return samples.to_vec();
    }
    let ratio = input_rate as f64 / VAD_SAMPLE_RATE as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut resampled = Vec::with_capacity(output_len);

    // Small moving-average low-pass before decimation.
    let filter_size = 3usize;
    let mut filtered = Vec::with_capacity(samples.len());
    for i in 0..samples.len() {
        let start = i.saturating_sub(filter_size);
        let end = std::cmp::min(i + filter_size + 1, samples.len());
        let sum: f32 = samples[start..end].iter().sum();
        filtered.push(sum / (end - start) as f32);
    }

    for i in 0..output_len {
        let source_pos = i as f64 * ratio;
        let source_index = source_pos as usize;
        let fraction = (source_pos - source_index as f64) as f32;
        if source_index + 1 < filtered.len() {
            let a = filtered[source_index];
            let b = filtered[source_index + 1];
            resampled.push(a + (b - a) * fraction);
        } else if source_index < filtered.len() {
            resampled.push(filtered[source_index]);
        }
    }
    resampled
}

/// A forced cut may move back this far from the cap to land in a pause.
const FORCED_CUT_SEARCH_MS: usize = 2_000;
/// At most this share of the cap is searched, so a short cap keeps most of
/// its length.
const FORCED_CUT_SEARCH_DIVISOR: usize = 3;
/// Length of the window whose level decides whether it is a pause.
const FORCED_CUT_WINDOW_MS: usize = 100;
const FORCED_CUT_STEP_MS: usize = 10;
/// A window is a pause only when its level is below this share of the level
/// of the searched audio. Steady speech or noise has no such dip.
const FORCED_CUT_PAUSE_RATIO: f32 = 0.5;

fn rms_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Where to end a segment that has to be cut because uninterrupted speech
/// reached the cap. `speech` is the audio up to the cap; the result is how
/// much of it the segment keeps.
///
/// A cut at the cap usually lands inside a word, and both halves of that word
/// are then transcribed wrongly. So the cut moves back to the quietest moment
/// in the last stretch before the cap, when that moment is a real dip in
/// level. The rest stays in the VAD as the start of the next segment. Without
/// a dip the segment keeps its full length.
fn forced_cut_len(speech: &[f32]) -> usize {
    let cap = speech.len();
    let window = ms_to_samples(FORCED_CUT_WINDOW_MS);
    let step = ms_to_samples(FORCED_CUT_STEP_MS);
    let search = ms_to_samples(FORCED_CUT_SEARCH_MS).min(cap / FORCED_CUT_SEARCH_DIVISOR);
    if search < 2 * window {
        return cap;
    }

    let searched = &speech[cap - search..];
    // Latest window first: of equally quiet windows the latest wins, which
    // keeps the segment as long as possible.
    let mut quietest = (0usize, f32::MAX);
    let mut start = searched.len() - window;
    loop {
        let level = rms_level(&searched[start..start + window]);
        if level < quietest.1 {
            quietest = (start, level);
        }
        if start < step {
            break;
        }
        start -= step;
    }

    if quietest.1 < rms_level(searched) * FORCED_CUT_PAUSE_RATIO {
        cap - search + quietest.0 + window / 2
    } else {
        cap
    }
}

/// Represents a complete speech segment detected by VAD
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    pub samples: Vec<f32>,
    pub start_timestamp_ms: f64,
    pub end_timestamp_ms: f64,
    pub confidence: f32,
}

struct Utterance {
    voiced_start_sample: usize,
    // Absolute start of the PCM that has yet to be emitted.
    next_sample: usize,
    confirmed: bool,
}

/// Processes audio in 30ms chunks but returns complete speech segments
pub struct ContinuousVadProcessor {
    session: VadSession,
    chunk_size: usize,
    _sample_rate: u32,
    resampler: VadResampler,
    buffer: Vec<f32>,
    speech_segments: VecDeque<SpeechSegment>,
    pcm: Vec<f32>,
    pcm_start_sample: usize,
    speech: Option<Utterance>,
    processed_samples: usize,
    silent_samples: usize,
    max_segment_samples: Option<usize>,
}

impl ContinuousVadProcessor {
    pub fn new(input_sample_rate: u32, redemption_time_ms: u32) -> Result<Self> {
        Self::new_with_max_segment_duration(input_sample_rate, redemption_time_ms, None)
    }

    pub fn new_with_max_segment_duration(
        input_sample_rate: u32,
        redemption_time_ms: u32,
        max_segment_duration_ms: Option<u32>,
    ) -> Result<Self> {
        // Use STRICT settings to prevent silence from reaching Whisper
        let mut config = VadConfig::default();
        config.sample_rate = VAD_SAMPLE_RATE as usize;

        // CONTINUOUS SPEECH FIX: Tuned for capturing complete 5+ second utterances
        // Previous: 0.55/0.40 with 400ms redemption was fragmenting speech into 40ms segments
        // New: More lenient thresholds + longer redemption for continuous speech
        config.positive_speech_threshold = 0.50; // Silero default - good for continuous speech
        config.negative_speech_threshold = 0.35; // Silero default - allows natural pauses

        // Use the caller-provided pause bridge. Live recording keeps this short
        // for responsiveness; batch import can pass a longer value.
        config.redemption_time = Duration::from_millis(redemption_time_ms as u64);
        config.pre_speech_pad = Duration::from_millis(PRE_SPEECH_PAD_MS);
        config.post_speech_pad = Duration::from_millis(400); // Increased: more context at end

        // CRITICAL FIX: Increased min_speech_time to prevent tiny 40ms fragments
        // Previous: 100ms allowed too-short segments that Whisper rejects
        // New: 250ms ensures segments are substantial enough for Whisper (>100ms requirement)
        config.min_speech_time = Duration::from_millis(MIN_SPEECH_TIME_MS);

        debug!("Creating VAD session with: sample_rate={}Hz, redemption={}ms, min_speech={}ms, input_rate={}Hz",
               VAD_SAMPLE_RATE, redemption_time_ms, MIN_SPEECH_TIME_MS, input_sample_rate);

        let session = VadSession::new(config)
            .map_err(|e| anyhow!("Failed to create VAD session: {:?}", e))?;

        // VAD uses 30ms chunks at 16kHz (480 samples)
        let vad_chunk_size = ms_to_samples(VAD_FRAME_MS as usize);
        let max_segment_samples = max_segment_duration_ms
            .map(|ms| ((ms as usize * VAD_SAMPLE_RATE as usize) / 1000).max(vad_chunk_size));

        info!(
            "VAD processor created: input={}Hz, vad={}Hz, chunk_size={} samples, max_segment_ms={:?}",
            input_sample_rate, VAD_SAMPLE_RATE, vad_chunk_size, max_segment_duration_ms
        );

        Ok(Self {
            session,
            chunk_size: vad_chunk_size,
            _sample_rate: input_sample_rate,
            resampler: VadResampler::new(input_sample_rate),
            buffer: Vec::with_capacity(vad_chunk_size * 2),
            speech_segments: VecDeque::new(),
            pcm: Vec::new(),
            pcm_start_sample: 0,
            speech: None,
            processed_samples: 0,
            silent_samples: 0,
            max_segment_samples,
        })
    }

    #[cfg(test)]
    pub(super) fn set_min_speech_for_benchmark(&mut self, millis: u64) {
        assert!(millis <= MIN_SPEECH_TIME_MS);
        self.session.config_mut().min_speech_time = Duration::from_millis(millis);
    }

    /// Process incoming audio samples and return any complete speech segments
    /// Handles resampling from input sample rate to 16kHz for VAD processing
    pub fn process_audio(&mut self, samples: &[f32]) -> Result<Vec<SpeechSegment>> {
        // Resample to 16kHz for the VAD (rubato sinc, energy-preserving).
        let resampled_audio = self.resampler.process(samples);

        self.buffer.extend_from_slice(&resampled_audio);
        self.process_buffered_frames()
    }

    fn process_buffered_frames(&mut self) -> Result<Vec<SpeechSegment>> {
        let mut completed_segments = Vec::new();

        // Process complete 30ms chunks (480 samples at 16kHz)
        while self.buffer.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.buffer.drain(..self.chunk_size).collect();
            self.process_chunk(&chunk)?;

            // Extract any completed speech segments
            while let Some(segment) = self.speech_segments.pop_front() {
                completed_segments.push(segment);
            }
        }

        Ok(completed_segments)
    }

    /// Finish the stream and return final speech segments. Start the next stream
    /// with a new processor so it has its own sample clock and model state.
    pub fn flush(&mut self) -> Result<Vec<SpeechSegment>> {
        self.drain_resampler()?;
        self.finish_buffered_speech()
    }

    pub(super) fn drain_resampler(&mut self) -> Result<()> {
        self.buffer.extend(self.resampler.flush()?);
        Ok(())
    }

    pub(super) fn finish_buffered_speech(&mut self) -> Result<Vec<SpeechSegment>> {
        let mut completed_segments = self.process_buffered_frames()?;
        // Bound every segment emitted by the padded terminal frame to real PCM.
        let real_end_sample = self.processed_samples + self.buffer.len();

        // Process any remaining buffered audio
        if !self.buffer.is_empty() {
            let remaining = self.buffer.clone();
            self.buffer.clear();

            // Pad to chunk size if needed
            let mut padded_chunk = remaining;
            if padded_chunk.len() < self.chunk_size {
                padded_chunk.resize(self.chunk_size, 0.0);
            }

            self.process_chunk(&padded_chunk)?;
        }

        if let Some(speech) = self.speech.take().filter(|speech| speech.confirmed) {
            self.emit_segment(speech.next_sample, real_end_sample, 0.8);
        }
        self.discard_before(self.processed_samples);

        // Extract all remaining segments
        while let Some(segment) = self.speech_segments.pop_front() {
            let mut segment = segment;
            let start_sample =
                (segment.start_timestamp_ms * VAD_SAMPLE_RATE as f64 / 1000.0).round() as usize;
            let real_len = real_end_sample.saturating_sub(start_sample);
            segment.samples.truncate(real_len);
            if !segment.samples.is_empty() {
                segment.end_timestamp_ms = samples_to_ms(start_sample + segment.samples.len());
                completed_segments.push(segment);
            }
        }

        Ok(completed_segments)
    }

    fn process_chunk(&mut self, chunk: &[f32]) -> Result<()> {
        // Use the pinned model and its recurrent state without the session's
        // audio buffer. Its SpeechEnd still indexes the original start after
        // take_until moves the buffer, so it cannot trim an active utterance.
        #[cfg(debug_assertions)]
        self.session.validate_input(chunk)?;
        let output = self.session.forward(chunk.to_vec())?;
        let probability = *output
            .try_extract_array::<f32>()?
            .first()
            .ok_or_else(|| anyhow!("Silero returned no speech probability"))?;

        let config = *self.session.config_mut();
        let frame_start = self.processed_samples;
        self.processed_samples += chunk.len();
        self.pcm.extend_from_slice(chunk);
        let quiet = probability < config.negative_speech_threshold;
        if quiet {
            self.silent_samples += chunk.len();
        } else {
            self.silent_samples = 0;
        }

        let pre_roll = ms_to_samples(config.pre_speech_pad.as_millis() as usize);
        let Some(mut speech) = self.speech.take() else {
            if probability > config.positive_speech_threshold {
                let start = frame_start
                    .saturating_sub(pre_roll)
                    .max(self.pcm_start_sample);
                self.discard_before(start);
                self.speech = Some(Utterance {
                    voiced_start_sample: frame_start,
                    next_sample: start,
                    confirmed: false,
                });
            } else {
                self.trim_idle_history();
            }
            return Ok(());
        };

        if !speech.confirmed {
            let min_speech = ms_to_samples(config.min_speech_time.as_millis() as usize);
            // Match Silero's strict duration check, excluding the trigger frame.
            if frame_start - speech.voiced_start_sample > min_speech {
                speech.confirmed = true;
                debug!(
                    "VAD: Speech started at {}ms",
                    samples_to_ms(speech.next_sample)
                );
            } else {
                if quiet {
                    self.trim_idle_history();
                } else {
                    self.speech = Some(speech);
                }
                return Ok(());
            }
        }

        let redemption_samples = ms_to_samples(config.redemption_time.as_millis() as usize);
        if quiet && self.silent_samples > redemption_samples {
            let speech_end = self.processed_samples - self.silent_samples;
            let post_roll = ms_to_samples(config.post_speech_pad.as_millis() as usize);
            let end = (speech_end + post_roll).min(self.processed_samples);
            debug!("VAD: Speech ended at {}ms", samples_to_ms(end));
            self.emit_segment(speech.next_sample, end, 0.9);
            // Match the pinned session's inclusive trim at the unpadded end.
            // The next utterance can use the retained silence as pre-roll.
            self.discard_before(speech_end + 1);
            self.trim_idle_history();
        } else {
            speech.next_sample = self.emit_forced_segments_if_needed(speech.next_sample);
            // Keep the uncommitted tail plus enough history for a new utterance
            // if the next frame ends this one, even if we cut during a pause.
            let history_start = self.processed_samples.saturating_sub(pre_roll);
            self.discard_before(speech.next_sample.min(history_start));
            self.speech = Some(speech);
        }
        Ok(())
    }

    fn emit_forced_segments_if_needed(&mut self, mut start: usize) -> usize {
        let Some(max_segment_samples) = self.max_segment_samples else {
            return start;
        };

        while self.processed_samples - start >= max_segment_samples {
            let offset = start - self.pcm_start_sample;
            let cut = forced_cut_len(&self.pcm[offset..offset + max_segment_samples]);
            self.emit_segment(start, start + cut, 0.85);
            debug!("VAD: Forced segment of {} samples", cut);
            start += cut;
        }
        start
    }

    fn emit_segment(&mut self, start: usize, end: usize, confidence: f32) {
        if start >= end {
            return;
        }
        let samples = self.pcm[start - self.pcm_start_sample..end - self.pcm_start_sample].to_vec();
        self.speech_segments.push_back(SpeechSegment {
            samples,
            start_timestamp_ms: samples_to_ms(start),
            end_timestamp_ms: samples_to_ms(end),
            confidence,
        });
    }

    fn discard_before(&mut self, sample: usize) {
        if sample > self.pcm_start_sample {
            self.pcm.drain(..sample - self.pcm_start_sample);
            self.pcm_start_sample = sample;
        }
    }

    fn trim_idle_history(&mut self) {
        if self.pcm.len() >= IDLE_HISTORY_SAMPLES + IDLE_TRIM_INTERVAL_SAMPLES {
            self.discard_before(self.processed_samples - IDLE_HISTORY_SAMPLES);
        }
    }
}

/// Legacy function for backward compatibility - now uses the optimized approach
pub fn extract_speech_16k(samples_mono_16k: &[f32]) -> Result<Vec<f32>> {
    let mut processor = ContinuousVadProcessor::new(16000, 400)?;

    // Process all audio
    let mut all_segments = processor.process_audio(samples_mono_16k)?;
    let final_segments = processor.flush()?;
    all_segments.extend(final_segments);

    // Concatenate all speech segments
    let mut result = Vec::new();
    let num_segments = all_segments.len();
    for segment in &all_segments {
        result.extend_from_slice(&segment.samples);
    }

    // Apply balanced energy filtering for very short segments
    if result.len() < 1600 {
        // Less than 100ms at 16kHz
        let input_energy: f32 =
            samples_mono_16k.iter().map(|&x| x * x).sum::<f32>() / samples_mono_16k.len() as f32;
        let rms = input_energy.sqrt();
        let peak = samples_mono_16k
            .iter()
            .map(|&x| x.abs())
            .fold(0.0f32, f32::max);

        // BALANCED FIX: Lowered thresholds to preserve quiet speech while still filtering silence
        // Previous aggressive values (0.08/0.15) were discarding valid quiet speech
        // New values (0.03/0.08) are more balanced - catch quiet speech, reject pure silence
        if rms < 0.2 || peak < 0.20 {
            info!("-----VAD detected silence/noise (RMS: {:.6}, Peak: {:.6}), skipping to prevent hallucinations-----", rms, peak);
            return Ok(Vec::new());
        } else {
            info!(
                "VAD detected speech with sufficient energy (RMS: {:.6}, Peak: {:.6})",
                rms, peak
            );
            return Ok(samples_mono_16k.to_vec());
        }
    }

    debug!(
        "VAD: Processed {} samples, extracted {} speech samples from {} segments",
        samples_mono_16k.len(),
        result.len(),
        num_segments
    );

    Ok(result)
}

/// Simple convenience function to get speech chunks from audio
/// Uses the optimized ContinuousVadProcessor with configurable redemption time
pub fn get_speech_chunks(
    samples_mono_16k: &[f32],
    redemption_time_ms: u32,
) -> Result<Vec<SpeechSegment>> {
    get_speech_chunks_with_progress(samples_mono_16k, redemption_time_ms, |_, _| true)
}

/// Get speech chunks with progress callback and cancellation support
/// The callback receives (progress_percent, segments_found) and returns false to cancel
pub fn get_speech_chunks_with_progress<F>(
    samples_mono_16k: &[f32],
    redemption_time_ms: u32,
    mut progress_callback: F,
) -> Result<Vec<SpeechSegment>>
where
    F: FnMut(u32, usize) -> bool,
{
    let mut processor = ContinuousVadProcessor::new(16000, redemption_time_ms)?;

    let total_samples = samples_mono_16k.len();

    // For large files (>1 minute at 16kHz = 960,000 samples), process in chunks with progress logging
    const LARGE_FILE_THRESHOLD: usize = 960_000;
    const CHUNK_SIZE: usize = 160_000; // 10 seconds at 16kHz

    let mut all_segments = Vec::new();

    if total_samples > LARGE_FILE_THRESHOLD {
        info!(
            "VAD: Processing large file ({} samples = {:.1}s), will log progress...",
            total_samples,
            total_samples as f64 / 16000.0
        );

        let mut processed = 0;
        let mut last_progress = 0u32;
        let mut chunk_count = 0;
        let total_chunks = (total_samples + CHUNK_SIZE - 1) / CHUNK_SIZE;

        for chunk in samples_mono_16k.chunks(CHUNK_SIZE) {
            chunk_count += 1;

            let start_time = std::time::Instant::now();
            let segments = processor.process_audio(chunk)?;
            let elapsed = start_time.elapsed();

            // Debug log for chunk processing details
            debug!(
                "VAD: Chunk {}/{} processed in {:?}, found {} segments",
                chunk_count,
                total_chunks,
                elapsed,
                segments.len()
            );

            // Warn if chunk processing took too long (>1 second)
            if elapsed.as_secs() > 1 {
                warn!(
                    "VAD: Chunk {} took {:?} - possible performance issue",
                    chunk_count, elapsed
                );
            }

            all_segments.extend(segments);

            processed += chunk.len();
            let progress = ((processed * 100) / total_samples) as u32;

            // Call progress callback every 5%
            if progress >= last_progress + 5 {
                debug!(
                    "VAD: Progress {}% ({} segments found so far)",
                    progress,
                    all_segments.len()
                );

                // Check for cancellation
                if !progress_callback(progress, all_segments.len()) {
                    info!("VAD: Cancelled by callback at {}%", progress);
                    return Err(anyhow!("VAD processing cancelled"));
                }

                last_progress = progress;
            }
        }

        let final_segments = processor.flush()?;
        all_segments.extend(final_segments);

        info!(
            "VAD: Complete! Found {} speech segments",
            all_segments.len()
        );
    } else {
        // Small file - process all at once
        all_segments = processor.process_audio(samples_mono_16k)?;
        let final_segments = processor.flush()?;
        all_segments.extend(final_segments);
    }

    Ok(all_segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use silero_rs::VadTransition;

    /// Generate synthetic speech-like audio with alternating speech/silence
    fn generate_test_audio_with_speech(duration_seconds: f32, sample_rate: u32) -> Vec<f32> {
        let total_samples = (duration_seconds * sample_rate as f32) as usize;
        let mut samples = vec![0.0f32; total_samples];

        // Create speech-like patterns: bursts of sine waves with varying amplitude
        // Speech every 10 seconds for 5 seconds
        let speech_interval = 10.0; // seconds between speech starts
        let speech_duration = 5.0; // seconds of speech

        for i in 0..total_samples {
            let time = i as f32 / sample_rate as f32;
            let cycle_time = time % speech_interval;

            // Speech occurs in the first `speech_duration` seconds of each cycle
            if cycle_time < speech_duration {
                // Generate speech-like signal: multiple frequencies with amplitude modulation
                let freq1 = 200.0 + (time * 50.0).sin() * 100.0; // Varying fundamental
                let freq2 = freq1 * 2.0; // Harmonic
                let freq3 = freq1 * 3.0; // Another harmonic

                let amplitude = 0.3 + 0.1 * (time * 5.0).sin(); // Amplitude modulation
                samples[i] = amplitude
                    * (0.5 * (2.0 * std::f32::consts::PI * freq1 * time).sin()
                        + 0.3 * (2.0 * std::f32::consts::PI * freq2 * time).sin()
                        + 0.2 * (2.0 * std::f32::consts::PI * freq3 * time).sin());
            }
            // else: silence (already 0.0)
        }

        samples
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn sine(freq: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn resampler_passthrough_at_16k() {
        let mut r = VadResampler::new(16000);
        let input = sine(440.0, 16000, 1000);
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn resampler_48k_to_16k_length_and_energy() {
        let mut r = VadResampler::new(48000);
        let total = 48000; // 1 second
        let signal = sine(1000.0, 48000, total);
        // Stream in irregular chunk sizes to exercise the cross-call buffering.
        let mut out = Vec::new();
        for chunk in signal.chunks(317) {
            out.extend(r.process(chunk));
        }
        let expected = total / 3;
        let diff = (out.len() as i64 - expected as i64).abs();
        assert!(
            diff < 1200,
            "got {} samples, expected ~{}",
            out.len(),
            expected
        );
        // 1 kHz is well below the 8 kHz Nyquist → energy preserved.
        let (r_in, r_out) = (rms(&signal), rms(&out));
        assert!(
            (r_out / r_in - 1.0).abs() < 0.15,
            "rms not preserved: in {r_in} out {r_out}"
        );
    }

    #[test]
    fn resampler_attenuates_above_nyquist() {
        // A 12 kHz tone at 48k must be removed when downsampling to 16k (Nyquist
        // 8 kHz), or it aliases into the speech band. The old moving-average
        // filter barely touched it; the rubato sinc removes it.
        let mut r = VadResampler::new(48000);
        let signal = sine(12000.0, 48000, 48000);
        let mut out = Vec::new();
        for chunk in signal.chunks(512) {
            out.extend(r.process(chunk));
        }
        let (r_in, r_out) = (rms(&signal), rms(&out));
        assert!(
            r_out < r_in * 0.25,
            "above-Nyquist tone not attenuated: in {r_in} out {r_out}"
        );
    }

    #[test]
    fn sustained_silence_keeps_bounded_history_and_preserves_following_speech() {
        for silence_frames in [2, 50, 4_000] {
            check_speech_after_silence(silence_frames);
        }
    }

    fn check_speech_after_silence(silence_frames: usize) {
        let mut processor = ContinuousVadProcessor::new_with_max_segment_duration(
            VAD_SAMPLE_RATE,
            400,
            Some(1_000),
        )
        .unwrap();
        let silence_frame = [0.0; 480];
        for _ in 0..silence_frames {
            assert!(processor.process_audio(&silence_frame).unwrap().is_empty());
            assert!(processor.pcm.len() <= ms_to_samples(1_300));
            assert_eq!(processor.session.session_audio_samples(), 0);
        }
        let silence_samples = silence_frames * silence_frame.len();
        assert_eq!(processor.processed_samples, silence_samples);

        // Compare absolute segment boundaries and PCM against the pinned session
        // with the same model state and no history trimming or forced cuts.
        let mut reference = VadSession::new(*processor.session.config_mut()).unwrap();
        for _ in 0..silence_frames {
            reference.process(&silence_frame).unwrap();
        }
        let mut speech: Vec<_> = include_bytes!("pc_test_samples/de.s16le")
            .chunks_exact(2)
            .take(ms_to_samples(2_400))
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
            .collect();
        speech.resize(ms_to_samples(3_900), 0.0);
        let mut actual = Vec::new();
        let mut expected = Vec::new();
        for frame in speech.chunks_exact(480) {
            actual.extend(processor.process_audio(frame).unwrap());
            for transition in reference.process(frame).unwrap() {
                if let VadTransition::SpeechEnd {
                    start_timestamp_ms,
                    end_timestamp_ms,
                    samples,
                } = transition
                {
                    expected.push((start_timestamp_ms, end_timestamp_ms, samples));
                }
            }
        }
        assert!(processor.flush().unwrap().is_empty());
        assert_eq!(expected.len(), 1, "expected one reference utterance");
        let (start, end, samples) = &expected[0];
        assert_eq!(actual.first().unwrap().start_timestamp_ms, *start as f64);
        assert_eq!(actual.last().unwrap().end_timestamp_ms, *end as f64);
        assert!(actual.iter().any(|segment| segment.confidence == 0.85));
        for pair in actual.windows(2) {
            assert_eq!(pair[0].end_timestamp_ms, pair[1].start_timestamp_ms);
        }
        assert_eq!(
            actual
                .into_iter()
                .flat_map(|segment| segment.samples)
                .collect::<Vec<_>>(),
            *samples
        );
    }

    #[test]
    fn sustained_voice_bounds_retained_pcm_and_flushes_the_uncommitted_tail() {
        // Tile the same voiced signal so it remains comparable across the
        // whole stream. Exercise both repeated short cuts and the batch cap.
        let voiced = repeating_voice();
        let mut audio = vec![0.0; ms_to_samples(600)];
        audio.extend(voiced.iter().copied().cycle().take(ms_to_samples(129_001)));

        for (redemption_ms, cap_ms) in [(400, 2_007), (500, 60_000)] {
            let mut processor = ContinuousVadProcessor::new_with_max_segment_duration(
                VAD_SAMPLE_RATE,
                redemption_ms,
                Some(cap_ms),
            )
            .unwrap();
            let mut segments = Vec::new();
            let mut peak_app = 0;
            let mut peak_session = 0;
            let mut peak_capacity = 0;
            for chunk in audio.chunks(997) {
                segments.extend(processor.process_audio(chunk).unwrap());
                peak_app = peak_app.max(processor.pcm.len());
                peak_session = peak_session.max(processor.session.session_audio_samples());
                peak_capacity = peak_capacity.max(processor.pcm.capacity());
            }
            assert!(segments.len() >= 2, "fixture must force repeated cuts");
            let bound = ms_to_samples(cap_ms as usize);
            assert!(
                peak_app <= bound && peak_session == 0,
                "cap {cap_ms} ms retained {peak_app} app + {peak_session} Silero samples"
            );
            println!(
                "cap {cap_ms} ms: {peak_app} retained samples, capacity {peak_capacity}, Silero PCM {peak_session}"
            );
            segments.extend(processor.flush().unwrap());
            assert!(processor.flush().unwrap().is_empty());

            let config = *processor.session.config_mut();
            let (expected, speech_ends) = untrimmed_reference(config, cap_ms as usize, &audio);
            assert_eq!(speech_ends, 0, "fixture must stay continuously voiced");
            assert_eq!(expected.last().unwrap().end_timestamp_ms, 129_601.0);
            assert!(expected
                .windows(2)
                .all(|pair| pair[0].end_timestamp_ms == pair[1].start_timestamp_ms));
            assert_segments_match(&segments, &expected);
        }
    }

    fn repeating_voice() -> Vec<f32> {
        // A slice of the bundled synthetic German narration with no long pause.
        include_bytes!("pc_test_samples/de.s16le")[2 * ms_to_samples(300)..2 * ms_to_samples(2_100)]
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
            .collect()
    }

    #[test]
    fn forced_cuts_match_the_untrimmed_reference_through_speech_end_and_restart() {
        let voiced = repeating_voice();
        let mut audio = vec![0.0; ms_to_samples(3_000)];
        audio.extend(voiced.iter().copied().cycle().take(ms_to_samples(5_000)));
        audio.resize(ms_to_samples(12_000), 0.0);
        audio.extend(voiced.iter().copied().cycle().take(ms_to_samples(5_001)));

        // The long redemption permits a cut during the pause past the eventual
        // padded SpeechEnd. With the shorter redemption a natural tail remains.
        for (redemption_ms, cap_ms, has_natural_tail) in
            [(400, 2_007, true), (500, 2_007, true), (2000, 137, false)]
        {
            let mut processor = ContinuousVadProcessor::new_with_max_segment_duration(
                VAD_SAMPLE_RATE,
                redemption_ms,
                Some(cap_ms),
            )
            .unwrap();
            let mut actual = Vec::new();
            for chunk in audio.chunks(16_000) {
                actual.extend(processor.process_audio(chunk).unwrap());
            }
            actual.extend(processor.flush().unwrap());
            assert!(processor.flush().unwrap().is_empty());

            let (expected, speech_ends) =
                untrimmed_reference(*processor.session.config_mut(), cap_ms as usize, &audio);
            assert_eq!(speech_ends, 1, "fixture must end and restart speech");
            assert_eq!(
                expected.iter().any(|segment| segment.confidence == 0.9),
                has_natural_tail,
                "fixture must exercise the intended natural tail"
            );
            assert_eq!(expected.last().unwrap().end_timestamp_ms, 17_001.0);
            assert!(expected
                .windows(2)
                .any(|pair| { pair[0].end_timestamp_ms + 1_000.0 < pair[1].start_timestamp_ms }));
            assert_segments_match(&actual, &expected);
        }
    }

    // The pinned session keeps the whole utterance. Advance a read cursor over
    // its output using the existing cut heuristic, without compacting any PCM.
    fn untrimmed_reference(
        config: VadConfig,
        cap_ms: usize,
        audio: &[f32],
    ) -> (Vec<SpeechSegment>, usize) {
        let mut session = VadSession::new(config).unwrap();
        let mut active = None;
        let mut speech_ends = 0;
        let mut segments = Vec::new();
        let mut emit = |start: usize, samples: &[f32], confidence| {
            let end = (start + samples.len()).min(audio.len());
            if start < end {
                segments.push(SpeechSegment {
                    samples: samples[..end - start].to_vec(),
                    start_timestamp_ms: samples_to_ms(start),
                    end_timestamp_ms: samples_to_ms(end),
                    confidence,
                });
            }
        };
        for chunk in audio.chunks(480) {
            let mut frame = [0.0; 480];
            frame[..chunk.len()].copy_from_slice(chunk);
            for transition in session.process(&frame).unwrap() {
                match transition {
                    VadTransition::SpeechStart { timestamp_ms } => {
                        active = Some((ms_to_samples(timestamp_ms), 0));
                    }
                    VadTransition::SpeechEnd {
                        start_timestamp_ms,
                        samples,
                        ..
                    } => {
                        speech_ends += 1;
                        let start = ms_to_samples(start_timestamp_ms);
                        let (_, emitted) = active.take().unwrap();
                        if let Some(tail) = samples.get(emitted..) {
                            emit(start + emitted, tail, 0.9);
                        }
                    }
                }
            }
            if let Some((start, emitted)) = &mut active {
                let pcm = session.get_current_speech();
                let cap = ms_to_samples(cap_ms);
                while pcm.len() - *emitted >= cap {
                    let cut = forced_cut_len(&pcm[*emitted..*emitted + cap]);
                    emit(*start + *emitted, &pcm[*emitted..*emitted + cut], 0.85);
                    *emitted += cut;
                }
            }
        }
        if let Some((start, emitted)) = active {
            emit(
                start + emitted,
                &session.get_current_speech()[emitted..],
                0.8,
            );
        }
        (segments, speech_ends)
    }

    fn assert_segments_match(actual: &[SpeechSegment], expected: &[SpeechSegment]) {
        assert_eq!(actual.len(), expected.len(), "segment count");
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(
                actual.start_timestamp_ms, expected.start_timestamp_ms,
                "segment {index} start"
            );
            assert_eq!(
                actual.end_timestamp_ms, expected.end_timestamp_ms,
                "segment {index} end"
            );
            assert_eq!(
                actual.confidence, expected.confidence,
                "segment {index} confidence"
            );
            assert!(
                actual.samples == expected.samples,
                "segment {index} PCM differs"
            );
        }
    }

    /// Six seconds of tone with silence from `gap_start_ms` for `gap_ms`.
    fn tone_with_gap(gap_start_ms: usize, gap_ms: usize) -> Vec<f32> {
        let mut speech = sine(220.0, 16_000, 6 * 16_000);
        let gap = ms_to_samples(gap_start_ms)..ms_to_samples(gap_start_ms + gap_ms);
        speech[gap].fill(0.0);
        speech
    }

    #[test]
    fn forced_cut_moves_back_into_a_pause_near_the_cap() {
        let cut = forced_cut_len(&tone_with_gap(5_000, 200));
        assert!(
            (ms_to_samples(5_000)..=ms_to_samples(5_200)).contains(&cut),
            "cut at {:.0} ms is outside the pause",
            samples_to_ms(cut)
        );
    }

    #[test]
    fn forced_cut_prefers_the_latest_of_equal_pauses() {
        let mut speech = tone_with_gap(4_400, 200);
        speech[ms_to_samples(5_300)..ms_to_samples(5_500)].fill(0.0);
        let cut = forced_cut_len(&speech);
        assert!(
            (ms_to_samples(5_300)..=ms_to_samples(5_500)).contains(&cut),
            "cut at {:.0} ms is not in the later pause",
            samples_to_ms(cut)
        );
    }

    #[test]
    fn forced_cut_keeps_the_cap_without_a_usable_pause() {
        // Steady level: nothing to prefer.
        let steady = sine(220.0, 16_000, 6 * 16_000);
        assert_eq!(forced_cut_len(&steady), steady.len());
        // A pause more than two seconds before the cap is out of reach.
        let early = tone_with_gap(2_000, 300);
        assert_eq!(forced_cut_len(&early), early.len());
        // A dip that only halves the amplitude for 30 ms is not a pause.
        let mut shallow = sine(220.0, 16_000, 6 * 16_000);
        for sample in &mut shallow[ms_to_samples(5_000)..ms_to_samples(5_030)] {
            *sample *= 0.5;
        }
        assert_eq!(forced_cut_len(&shallow), shallow.len());
        // A cap too short to search keeps its length even over silence.
        let short = vec![0.0; ms_to_samples(500)];
        assert_eq!(forced_cut_len(&short), short.len());
    }

    #[test]
    fn forced_cut_never_keeps_less_than_two_thirds_of_a_short_cap() {
        // Three-second cap (Nemotron): only the last second is searched.
        let mut speech = sine(220.0, 16_000, 3 * 16_000);
        speech[ms_to_samples(1_500)..ms_to_samples(1_800)].fill(0.0);
        assert_eq!(forced_cut_len(&speech), speech.len());
        speech[ms_to_samples(2_300)..ms_to_samples(2_500)].fill(0.0);
        let cut = forced_cut_len(&speech);
        assert!((ms_to_samples(2_300)..=ms_to_samples(2_500)).contains(&cut));
    }

    #[test]
    fn forced_segments_cut_in_pauses_stay_contiguous_and_within_the_cap() {
        // Speech-like bursts with a 150 ms gap every 1.3 s: shorter than the
        // redemption time, so the VAD keeps the speech open and the cap cuts.
        let mut audio = vec![0.0; 16_000];
        audio.extend(generate_test_audio_with_speech(5.0, 16_000));
        let speech_start = 16_000;
        let mut gaps = Vec::new();
        let mut gap_start = speech_start + ms_to_samples(1_300);
        while gap_start + ms_to_samples(150) < audio.len() {
            let gap = gap_start..gap_start + ms_to_samples(150);
            audio[gap.clone()].fill(0.0);
            gaps.push(gap);
            gap_start += ms_to_samples(1_300);
        }

        let cap = ms_to_samples(2_000);
        let mut processor =
            ContinuousVadProcessor::new_with_max_segment_duration(16_000, 2000, Some(2_000))
                .unwrap();
        let mut segments = processor.process_audio(&audio).unwrap();
        segments.extend(processor.flush().unwrap());
        assert!(segments.len() >= 2, "fixture must force at least one cut");

        let mut last_end = 0;
        let mut cuts_in_a_gap = 0;
        for segment in &segments {
            let start = (segment.start_timestamp_ms * 16.0).round() as usize;
            let end = (segment.end_timestamp_ms * 16.0).round() as usize;
            assert!(start >= last_end, "segments must not duplicate audio");
            if last_end > 0 {
                assert_eq!(start, last_end, "forced segments must not drop audio");
            }
            assert!(segment.samples.len() <= cap, "segment exceeds the cap");
            assert_eq!(end - start, segment.samples.len());
            assert_eq!(segment.samples.as_slice(), &audio[start..end]);
            if gaps.iter().any(|gap| gap.contains(&end)) {
                cuts_in_a_gap += 1;
            }
            last_end = end;
        }
        assert!(cuts_in_a_gap >= 1, "no forced cut landed in a pause");
    }

    #[test]
    fn speech_that_ends_after_a_pause_cut_keeps_every_sample_once() {
        // Same bursts, followed by silence longer than the redemption time, so
        // the last segment comes from the VAD ending the speech, not from flush.
        let mut audio = vec![0.0; 16_000];
        audio.extend(generate_test_audio_with_speech(5.0, 16_000));
        let mut gap_start = 16_000 + ms_to_samples(1_300);
        while gap_start + ms_to_samples(150) < audio.len() {
            audio[gap_start..gap_start + ms_to_samples(150)].fill(0.0);
            gap_start += ms_to_samples(1_300);
        }
        audio.extend(vec![0.0; 3 * 16_000]);

        let mut processor =
            ContinuousVadProcessor::new_with_max_segment_duration(16_000, 2000, Some(2_000))
                .unwrap();
        let segments = processor.process_audio(&audio).unwrap();
        assert!(
            processor.speech.is_none(),
            "fixture must end the speech before flush"
        );
        assert!(segments.len() >= 2, "fixture must force at least one cut");
        assert!(
            segments
                .iter()
                .any(|s| s.samples.len() < ms_to_samples(2_000)
                    && s.samples.len() > ms_to_samples(1_300)),
            "fixture must shorten at least one forced cut"
        );

        let mut last_end = 0;
        for segment in &segments {
            let start = (segment.start_timestamp_ms * 16.0).round() as usize;
            let end = start + segment.samples.len();
            if last_end > 0 {
                assert_eq!(
                    start, last_end,
                    "segments must neither drop nor repeat audio"
                );
            }
            assert!(end <= audio.len());
            assert_eq!(segment.samples.as_slice(), &audio[start..end]);
            last_end = end;
        }
        assert!(processor.flush().unwrap().is_empty());
    }

    #[test]
    fn flush_trims_padding_after_late_speech_with_and_without_forced_segments() {
        for max_ms in [None, Some(500)] {
            let mut audio = vec![0.0; 20 * 16_000];
            audio.extend(generate_test_audio_with_speech(3.0, 16_000));
            assert_eq!(audio.len() % 480, 320);
            let mut processor =
                ContinuousVadProcessor::new_with_max_segment_duration(16_000, 2000, max_ms)
                    .unwrap();
            let mut segments = processor.process_audio(&audio).unwrap();
            assert!(
                processor
                    .speech
                    .as_ref()
                    .is_some_and(|speech| speech.confirmed),
                "fixture must exercise the forced tail"
            );
            segments.extend(processor.flush().unwrap());
            assert!(!segments.is_empty());
            let mut last_end = 0;
            for segment in &segments {
                let start = (segment.start_timestamp_ms * 16.0).round() as usize;
                let end = (segment.end_timestamp_ms * 16.0).round() as usize;
                assert!(start >= last_end, "segments must not duplicate audio");
                assert!(end <= audio.len(), "padding must not become speech");
                assert_eq!(end - start, segment.samples.len());
                assert_eq!(segment.samples.as_slice(), &audio[start..end]);
                last_end = end;
            }
            assert_eq!(last_end, audio.len());
            assert!(processor.flush().unwrap().is_empty());
        }
    }

    #[test]
    fn flush_preserves_the_resampled_end_of_active_speech() {
        let speech: Vec<_> = include_bytes!("pc_test_samples/de.s16le")
            .chunks_exact(2)
            .take(23_365)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0)
            .collect();
        let mut audio = vec![0.0; 48_000];
        audio.extend(speech.windows(2).flat_map(|pair| {
            let [first, next] = [pair[0], pair[1]];
            [
                first,
                first + (next - first) / 3.0,
                first + (next - first) * (2.0 / 3.0),
            ]
        }));
        // Stop in voiced audio, with a partial 512-sample resampler input block.
        audio.truncate(114_090);
        let mut vad = ContinuousVadProcessor::new(48_000, 400).unwrap();
        for chunk in audio.chunks(317) {
            assert!(vad.process_audio(chunk).unwrap().is_empty());
        }
        let segments = vad.flush().unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].end_timestamp_ms, samples_to_ms(38_030));
        assert!(rms(&segments[0].samples[segments[0].samples.len() - 80..]) > 0.05);
        assert!(vad.flush().unwrap().is_empty());

        let mut capped =
            ContinuousVadProcessor::new_with_max_segment_duration(48_000, 400, Some(137)).unwrap();
        let mut cuts = Vec::new();
        for chunk in audio.chunks(317) {
            cuts.extend(capped.process_audio(chunk).unwrap());
        }
        cuts.extend(capped.flush().unwrap());
        assert!(cuts.len() > 1);
        assert_eq!(cuts[0].start_timestamp_ms, segments[0].start_timestamp_ms);
        assert_eq!(
            cuts.last().unwrap().end_timestamp_ms,
            segments[0].end_timestamp_ms
        );
        assert!(cuts
            .windows(2)
            .all(|pair| pair[0].end_timestamp_ms == pair[1].start_timestamp_ms));
        let joined: Vec<_> = cuts.into_iter().flat_map(|cut| cut.samples).collect();
        assert_eq!(joined, segments[0].samples);

        let mut interrupted = ContinuousVadProcessor::new(48_000, 400).unwrap();
        for chunk in audio.chunks(317) {
            assert!(interrupted.process_audio(chunk).unwrap().is_empty());
        }
        // Simulate a gap in resampler output that the terminal blocks cannot fill.
        interrupted.resampler.output_samples -= VAD_SAMPLE_RATE as u64;
        assert!(interrupted.drain_resampler().is_err());
        let recovered = interrupted.finish_buffered_speech().unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(!recovered[0].samples.is_empty());
        assert!(segments[0].samples.starts_with(&recovered[0].samples));
        assert!(interrupted.finish_buffered_speech().unwrap().is_empty());
    }

    #[test]
    fn test_vad_chunked_vs_single_processing() {
        // Generate 60 seconds of audio with speech patterns at 16kHz
        let audio = generate_test_audio_with_speech(60.0, 16000);
        println!(
            "Generated {} samples ({:.1}s)",
            audio.len(),
            audio.len() as f32 / 16000.0
        );

        // Process all at once (like small files)
        let segments_single = get_speech_chunks(&audio, 2000).expect("Single processing failed");
        println!("Single processing found {} segments", segments_single.len());

        // Process in chunks (like large files)
        let segments_chunked =
            get_speech_chunks_with_progress(&audio, 2000, |progress, segments| {
                println!("Chunked progress: {}%, {} segments", progress, segments);
                true // Don't cancel
            })
            .expect("Chunked processing failed");
        println!(
            "Chunked processing found {} segments",
            segments_chunked.len()
        );

        // Both should find the same number of segments (approximately)
        // Allow some variance due to chunk boundary effects
        let diff = (segments_single.len() as i32 - segments_chunked.len() as i32).abs();
        assert!(
            diff <= 1,
            "Chunked and single processing found different segment counts: {} vs {} (diff: {})",
            segments_single.len(),
            segments_chunked.len(),
            diff
        );
    }

    #[test]
    fn test_vad_large_file_progress() {
        // Generate 120 seconds (2 minutes) of audio - triggers large file threshold
        let audio = generate_test_audio_with_speech(120.0, 16000);
        let total_samples = audio.len();
        println!(
            "Generated {} samples ({:.1}s)",
            total_samples,
            total_samples as f32 / 16000.0
        );

        // This should trigger the large file path (>960,000 samples)
        assert!(
            total_samples > 960_000,
            "Audio should be large enough to trigger chunked processing"
        );

        let mut progress_updates = Vec::new();
        let segments = get_speech_chunks_with_progress(&audio, 2000, |progress, segments| {
            progress_updates.push((progress, segments));
            true // Don't cancel
        })
        .expect("Processing failed");

        println!(
            "Found {} segments with {} progress updates",
            segments.len(),
            progress_updates.len()
        );

        // The synthetic signal is not real speech, so Silero may merge it into
        // one long segment. This test is specifically for the large-file path:
        // it must still emit speech and report monotonic progress through 100%.
        assert!(!segments.is_empty(), "Expected at least one speech segment");
        assert!(
            segments.iter().all(|segment| !segment.samples.is_empty()
                && segment.end_timestamp_ms > segment.start_timestamp_ms),
            "Expected all speech segments to contain audio with positive duration"
        );

        // Should have received progress updates
        assert!(
            !progress_updates.is_empty(),
            "Expected progress updates for large file"
        );
        assert_eq!(
            progress_updates.last().map(|(progress, _)| *progress),
            Some(100),
            "Expected progress to reach 100%"
        );
        assert!(
            progress_updates
                .windows(2)
                .all(|pair| pair[0].0 < pair[1].0),
            "Expected progress updates to increase monotonically: {:?}",
            progress_updates
        );
    }

    #[test]
    fn test_vad_cancellation() {
        let audio = generate_test_audio_with_speech(120.0, 16000);

        // Cancel at 50%
        let result = get_speech_chunks_with_progress(&audio, 2000, |progress, _| {
            progress < 50 // Cancel when reaching 50%
        });

        // Should return error due to cancellation
        assert!(result.is_err(), "Expected cancellation error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("cancelled"),
            "Error should mention cancellation: {}",
            err_msg
        );
    }

    #[test]
    fn test_vad_continuous_processor_state_across_chunks() {
        // Test that VAD state is correctly maintained across chunk boundaries
        let mut processor =
            ContinuousVadProcessor::new(16000, 2000).expect("Failed to create processor");

        // Generate audio with a speech segment that spans a chunk boundary
        let chunk_size = 160_000; // 10 seconds
        let audio = generate_test_audio_with_speech(30.0, 16000); // 30 seconds

        // Process in 10-second chunks
        let mut all_segments = Vec::new();
        for (i, chunk) in audio.chunks(chunk_size).enumerate() {
            let segments = processor.process_audio(chunk).expect("Processing failed");
            println!(
                "Chunk {}: processed {} samples, found {} segments",
                i,
                chunk.len(),
                segments.len()
            );
            all_segments.extend(segments);
        }

        // Flush remaining
        let final_segments = processor.flush().expect("Flush failed");
        all_segments.extend(final_segments);

        println!("Total segments found: {}", all_segments.len());

        // Should find speech segments
        assert!(
            all_segments.len() >= 1,
            "Expected at least 1 speech segment"
        );
    }

    #[test]
    fn test_vad_400ms_vs_2000ms_segmentation() {
        // Demonstrates why 2000ms redemption is needed for batch processing:
        // 400ms creates excessive fragmentation, 2000ms bridges natural pauses.
        //
        // Audio pattern: 60s with 5s speech / 5s silence cycles
        // Natural pauses within speech (sentence gaps) are 500ms-1.5s
        let audio = generate_test_audio_with_speech(60.0, 16000);

        let segments_400 = get_speech_chunks(&audio, 400).expect("400ms processing failed");
        let segments_2000 = get_speech_chunks(&audio, 2000).expect("2000ms processing failed");

        println!(
            "400ms redemption: {} segments, 2000ms redemption: {} segments",
            segments_400.len(),
            segments_2000.len()
        );

        // 2000ms should produce fewer or equal segments (bridges more pauses)
        assert!(
            segments_2000.len() <= segments_400.len(),
            "2000ms redemption ({} segments) should not produce more segments than 400ms ({} segments)",
            segments_2000.len(),
            segments_400.len()
        );

        // Verify segments have reasonable durations with 2000ms
        for (i, seg) in segments_2000.iter().enumerate() {
            let duration_ms = seg.end_timestamp_ms - seg.start_timestamp_ms;
            println!("2000ms segment {}: {:.0}ms duration", i, duration_ms);
            // Each segment should be at least 250ms (min_speech_time)
            assert!(
                duration_ms >= 200.0,
                "Segment {} too short: {:.0}ms",
                i,
                duration_ms
            );
        }
    }
}
