use ndarray::{Array, Array1, Array2, Array3, ArrayD, ArrayViewD, IxDyn};
use once_cell::sync::Lazy;
use ort::execution_providers::CPUExecutionProvider;
#[cfg(feature = "directml")]
use ort::execution_providers::{DirectMLExecutionProvider, ExecutionProvider};
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use regex::Regex;

use std::fs;
use std::path::Path;

pub type DecoderState = (Array3<f32>, Array3<f32>);

const SUBSAMPLING_FACTOR: usize = 8;
const WINDOW_SIZE: f32 = 0.01;
const MAX_TOKENS_PER_STEP: usize = 3;
const TDT_DURATIONS: [usize; 5] = [0, 1, 2, 3, 4];
const SAMPLE_RATE: usize = 16_000;

static DECODE_SPACE_RE: Lazy<Result<Regex, regex::Error>> =
    Lazy::new(|| Regex::new(r"\A\s|\s\B|(\s)\b"));

#[derive(Debug, Clone)]
pub struct TimestampedResult {
    pub text: String,
    pub timestamps: Vec<f32>,
    pub tokens: Vec<String>,
}

/// Segments quieter than this (about -26 dBFS) are raised before decoding.
const QUIET_SEGMENT_RMS: f32 = 0.05;
/// Level a quiet segment is raised to (-20 dBFS).
const TARGET_SEGMENT_RMS: f32 = 0.1;
/// Below this (-60 dBFS) a segment is silence or noise and is left alone.
const SILENT_SEGMENT_RMS: f32 = 0.001;
/// Never amplify by more than this (about +30 dB).
const MAX_SEGMENT_GAIN: f32 = 32.0;
/// The gain keeps all but the loudest 0.1% of samples at or below this level.
const LOUD_PART_CEILING: f32 = 0.9;

/// Raises a quiet segment to a normal speech level.
///
/// The models drop speech from quiet audio: on a recording at about -37 dBFS
/// the same clear sentences came back complete, cut in half or empty depending
/// on where the segment began, and complete once the level was raised. A
/// segment at a normal average level is not touched, so its results do not
/// change.
///
/// A segment can be quiet on average and still hold louder sound, such as one
/// loud remark next to a quiet speaker. The gain is limited so that at most
/// 0.1% of the samples reach full scale and are clipped. That bounds the damage
/// to the louder part without ruling it out: speech peaks well above its
/// typical level, so the clipped samples can all fall inside one remark. The
/// same limit lets a click or tone longer than 0.1% of the segment hold the
/// gain back for the quiet speech around it. Neither effect has been measured
/// with the model.
fn raise_quiet_segment(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }
    let rms =
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt();
    // A NaN or infinite level fails this check too, so every sample below is finite.
    if !(SILENT_SEGMENT_RMS..QUIET_SEGMENT_RMS).contains(&rms) {
        return;
    }
    let mut magnitudes: Vec<f32> = samples.iter().map(|sample| sample.abs()).collect();
    let loud_index = (magnitudes.len() * 999 / 1000).min(magnitudes.len() - 1);
    let (_, loud_part, _) = magnitudes.select_nth_unstable_by(loud_index, f32::total_cmp);
    let gain = (TARGET_SEGMENT_RMS / rms)
        .min(MAX_SEGMENT_GAIN)
        .min(LOUD_PART_CEILING / *loud_part);
    if gain <= 1.0 {
        return;
    }
    for sample in samples.iter_mut() {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

fn shift_timestamps(timestamps: &mut [f32], padding_seconds: f32) {
    for timestamp in timestamps {
        *timestamp = (*timestamp - padding_seconds).max(0.0);
    }
}

fn transcribe_with_empty_retry<F>(
    samples: Vec<f32>,
    is_cancelled: &dyn Fn() -> bool,
    mut decode: F,
) -> Result<TimestampedResult, ParakeetError>
where
    F: FnMut(Vec<f32>) -> Result<TimestampedResult, ParakeetError>,
{
    let sample_count = samples.len();
    let duration_seconds = sample_count as f64 / SAMPLE_RATE as f64;
    let rms = if sample_count == 0 {
        0.0
    } else {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / sample_count as f32).sqrt()
    };
    // The first decode consumes the original buffer. Keep a retry copy only
    // for audio long and audible enough for the empty-result retry gate.
    let retry_samples = if sample_count >= SAMPLE_RATE && rms >= 0.005 {
        Some(samples.clone())
    } else {
        None
    };
    let result = decode(samples)?;
    if !result.text.trim().is_empty() {
        return Ok(result);
    }
    let Some(samples) = retry_samples else {
        return Ok(result);
    };

    for padding_ms in [300usize, 1000] {
        if is_cancelled() {
            return Ok(result);
        }
        let padding_samples = padding_ms * SAMPLE_RATE / 1000;
        let mut padded = Vec::with_capacity(padding_samples + samples.len());
        padded.resize(padding_samples, 0.0);
        padded.extend_from_slice(&samples);
        let mut retry = match decode(padded) {
            Ok(retry) => retry,
            Err(_) => {
                log::warn!(
                    "Parakeet empty-result retry decode failed: audio={:.2}s padding={}ms",
                    duration_seconds,
                    padding_ms
                );
                return Ok(result);
            }
        };
        // Any recovered words are kept, however few: a short answer is still
        // an answer.
        if !retry.text.trim().is_empty() {
            shift_timestamps(&mut retry.timestamps, padding_ms as f32 / 1000.0);
            log::info!(
                "Parakeet empty-result retry recovered text for segment: audio={:.2}s",
                duration_seconds
            );
            return Ok(retry);
        }
    }
    log::debug!(
        "Parakeet empty-result retries returned no text for segment: audio={:.2}s",
        duration_seconds
    );
    Ok(result)
}

#[derive(thiserror::Error, Debug)]
pub enum ParakeetError {
    #[error("ORT error")]
    Ort(#[from] ort::Error),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("ndarray shape error")]
    Shape(#[from] ndarray::ShapeError),
    #[error("Model input not found: {0}")]
    InputNotFound(String),
    #[error("Model output not found: {0}")]
    OutputNotFound(String),
    #[error("Failed to get tensor shape for input: {0}")]
    TensorShape(String),
}

pub struct ParakeetModel {
    encoder: Session,
    decoder_joint: Session,
    preprocessor: Session,
    vocab: Vec<String>,
    blank_idx: i32,
    vocab_size: usize,
    pub(crate) directml_registered: bool,
}

impl Drop for ParakeetModel {
    fn drop(&mut self) {
        log::debug!(
            "Dropping ParakeetModel with {} vocab tokens",
            self.vocab.len()
        );
    }
}

impl ParakeetModel {
    pub fn new<P: AsRef<Path>>(
        model_dir: P,
        precision_suffix: Option<&str>,
        use_directml: bool,
    ) -> Result<Self, ParakeetError> {
        let decoder_use_directml = Self::decoder_directml_enabled(precision_suffix, use_directml);

        let (encoder, encoder_directml) = Self::init_session(
            &model_dir,
            "encoder-model",
            None,
            precision_suffix,
            use_directml,
        )?;
        let (decoder_joint, decoder_directml) = Self::init_session(
            &model_dir,
            "decoder_joint-model",
            None,
            precision_suffix,
            decoder_use_directml,
        )?;
        // Preprocessor is tiny — keep it on CPU to avoid DirectML transfer overhead.
        let (preprocessor, _) = Self::init_session(&model_dir, "nemo128", None, None, false)?;

        let (vocab, blank_idx) = Self::load_vocab(&model_dir)?;
        let vocab_size = vocab.len();

        log::info!(
            "Loaded Parakeet vocabulary with {} tokens, blank_idx={}",
            vocab_size,
            blank_idx
        );

        Ok(Self {
            encoder,
            decoder_joint,
            preprocessor,
            vocab,
            blank_idx,
            vocab_size,
            directml_registered: encoder_directml || decoder_directml,
        })
    }

    fn decoder_directml_enabled(precision_suffix: Option<&str>, use_directml: bool) -> bool {
        if !use_directml {
            return false;
        }

        match std::env::var("PARAKEET_DECODER_EP") {
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "dml" | "directml" | "gpu" => {
                    log::info!("Parakeet decoder/joint EP override: DirectML");
                    true
                }
                "cpu" => {
                    log::info!("Parakeet decoder/joint EP override: CPU");
                    false
                }
                other => {
                    log::warn!(
                        "Ignoring invalid PARAKEET_DECODER_EP='{}' (expected cpu or dml)",
                        other
                    );
                    Self::default_decoder_directml_enabled(precision_suffix)
                }
            },
            Err(_) => Self::default_decoder_directml_enabled(precision_suffix),
        }
    }

    fn default_decoder_directml_enabled(precision_suffix: Option<&str>) -> bool {
        if precision_suffix == Some("fp16") {
            log::info!(
                "Parakeet: keeping fp16 decoder_joint-model on CPU; DirectML decoder is too fine-grained for the TDT loop"
            );
            false
        } else {
            true
        }
    }

    fn init_session<P: AsRef<Path>>(
        model_dir: P,
        model_name: &str,
        intra_threads: Option<usize>,
        precision_suffix: Option<&str>,
        use_directml: bool,
    ) -> Result<(Session, bool), ParakeetError> {
        let mut builder =
            Session::builder()?.with_optimization_level(GraphOptimizationLevel::Level3)?;
        // ORT normally hides registration failures and proceeds with CPU.
        // Keep that fallback but retain the actual session providers.
        #[cfg(feature = "directml")]
        let using_directml = use_directml
            && DirectMLExecutionProvider::default()
                .register(&mut builder)
                .is_ok();
        #[cfg(not(feature = "directml"))]
        let using_directml = false;
        if use_directml && !using_directml {
            log::warn!("Parakeet: DirectML unavailable for {model_name}, using CPU");
        }

        let model_filename = if let Some(suffix) = precision_suffix {
            let precision_name = format!("{model_name}.{suffix}.onnx");
            let precision_path = model_dir.as_ref().join(&precision_name);
            if precision_path.exists() {
                log::info!("Loading {suffix} Parakeet model from {}...", precision_name);
                precision_name
            } else {
                return Err(ParakeetError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{} not found", precision_name),
                )));
            }
        } else {
            let regular_name = format!("{}.onnx", model_name);
            log::info!("Loading Parakeet model from {}...", regular_name);
            regular_name
        };

        builder = builder.with_execution_providers([CPUExecutionProvider::default().build()])?;

        if using_directml {
            // DirectML requires sequential execution and disabled memory pattern
            // optimization. Keep CPU sessions parallel for throughput.
            builder = builder
                .with_parallel_execution(false)?
                .with_memory_pattern(false)?;
        } else {
            builder = builder.with_parallel_execution(true)?;
        }

        if let Some(threads) = intra_threads {
            builder = builder
                .with_intra_threads(threads)?
                .with_inter_threads(threads)?;
        }

        let session = builder.commit_from_file(model_dir.as_ref().join(&model_filename))?;

        for input in &session.inputs {
            log::info!(
                "Parakeet Model '{}' input: name={}, type={:?}",
                model_filename,
                input.name,
                input.input_type
            );
        }

        Ok((session, using_directml))
    }

    fn load_vocab<P: AsRef<Path>>(model_dir: P) -> Result<(Vec<String>, i32), ParakeetError> {
        let vocab_path = model_dir.as_ref().join("vocab.txt");
        let content = fs::read_to_string(vocab_path)?;

        let mut max_id = 0;
        let mut tokens_with_ids: Vec<(String, usize)> = Vec::new();
        let mut blank_idx: Option<usize> = None;

        for line in content.lines() {
            let parts: Vec<&str> = line.trim_end().split(' ').collect();
            if parts.len() >= 2 {
                let token = parts[0].to_string();
                if let Ok(id) = parts[1].parse::<usize>() {
                    if token == "<blk>" {
                        blank_idx = Some(id);
                    }
                    tokens_with_ids.push((token, id));
                    max_id = max_id.max(id);
                }
            }
        }

        // Create vocab vector with \u2581 replaced with space
        let mut vocab = vec![String::new(); max_id + 1];
        for (token, id) in tokens_with_ids {
            vocab[id] = token.replace('\u{2581}', " ");
        }

        let blank_idx = blank_idx.ok_or_else(|| {
            ParakeetError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Missing <blk> token in vocabulary",
            ))
        })? as i32;

        Ok((vocab, blank_idx))
    }

    pub fn preprocess(
        &mut self,
        waveforms: &ArrayViewD<f32>,
        waveforms_lens: &ArrayViewD<i64>,
    ) -> Result<(ArrayD<f32>, ArrayD<i64>), ParakeetError> {
        log::trace!("Running Parakeet preprocessor inference...");
        let inputs = inputs![
            "waveforms" => TensorRef::from_array_view(waveforms.view())?,
            "waveforms_lens" => TensorRef::from_array_view(waveforms_lens.view())?,
        ];
        let outputs = self.preprocessor.run(inputs)?;

        let features = outputs
            .get("features")
            .ok_or_else(|| ParakeetError::OutputNotFound("features".to_string()))?
            .try_extract_array()?;
        let features_lens = outputs
            .get("features_lens")
            .ok_or_else(|| ParakeetError::OutputNotFound("features_lens".to_string()))?
            .try_extract_array()?;

        Ok((features.to_owned(), features_lens.to_owned()))
    }

    pub fn encode(
        &mut self,
        audio_signal: &ArrayViewD<f32>,
        length: &ArrayViewD<i64>,
    ) -> Result<(ArrayD<f32>, ArrayD<i64>), ParakeetError> {
        log::trace!("Running Parakeet encoder inference...");
        let inputs = inputs![
            "audio_signal" => TensorRef::from_array_view(audio_signal.view())?,
            "length" => TensorRef::from_array_view(length.view())?,
        ];
        let outputs = self.encoder.run(inputs)?;

        let encoder_output = outputs
            .get("outputs")
            .ok_or_else(|| ParakeetError::OutputNotFound("outputs".to_string()))?
            .try_extract_array()?;
        let encoded_lengths = outputs
            .get("encoded_lengths")
            .ok_or_else(|| ParakeetError::OutputNotFound("encoded_lengths".to_string()))?
            .try_extract_array()?;

        let encoder_output = encoder_output.permuted_axes(IxDyn(&[0, 2, 1]));

        Ok((encoder_output.to_owned(), encoded_lengths.to_owned()))
    }

    pub fn create_decoder_state(&self) -> Result<DecoderState, ParakeetError> {
        // Get input shapes from decoder model
        let inputs = &self.decoder_joint.inputs;

        let state1_shape = inputs
            .iter()
            .find(|input| input.name == "input_states_1")
            .ok_or_else(|| ParakeetError::InputNotFound("input_states_1".to_string()))?
            .input_type
            .tensor_shape()
            .ok_or_else(|| ParakeetError::TensorShape("input_states_1".to_string()))?;

        let state2_shape = inputs
            .iter()
            .find(|input| input.name == "input_states_2")
            .ok_or_else(|| ParakeetError::InputNotFound("input_states_2".to_string()))?
            .input_type
            .tensor_shape()
            .ok_or_else(|| ParakeetError::TensorShape("input_states_2".to_string()))?;

        // Create zero states with batch_size=1
        // Shape is [2, -1, 640] so we use [2, 1, 640] for batch_size=1
        let state1 = Array::zeros((
            state1_shape[0] as usize,
            1, // batch_size = 1
            state1_shape[2] as usize,
        ));

        let state2 = Array::zeros((
            state2_shape[0] as usize,
            1, // batch_size = 1
            state2_shape[2] as usize,
        ));

        Ok((state1, state2))
    }

    pub fn decode_step(
        &mut self,
        prev_tokens: &[i32],
        prev_state: &DecoderState,
        encoder_out: &ArrayViewD<f32>, // [time_steps, 1024]
    ) -> Result<(ArrayD<f32>, DecoderState), ParakeetError> {
        log::trace!("Running Parakeet decoder inference...");

        // Get last token or blank_idx if empty
        let target_token = prev_tokens.last().copied().unwrap_or(self.blank_idx);

        // Prepare inputs matching Python: encoder_out[None, :, None] -> [1, time_steps, 1]
        let encoder_outputs = encoder_out
            .to_owned()
            .insert_axis(ndarray::Axis(0))
            .insert_axis(ndarray::Axis(2));
        let targets = Array2::from_shape_vec((1, 1), vec![target_token])?;
        let target_length = Array1::from_vec(vec![1]);

        let inputs = inputs![
            "encoder_outputs" => TensorRef::from_array_view(encoder_outputs.view())?,
            "targets" => TensorRef::from_array_view(targets.view())?,
            "target_length" => TensorRef::from_array_view(target_length.view())?,
            "input_states_1" => TensorRef::from_array_view(prev_state.0.view())?,
            "input_states_2" => TensorRef::from_array_view(prev_state.1.view())?,
        ];

        let outputs = self.decoder_joint.run(inputs)?;

        let logits = outputs
            .get("outputs")
            .ok_or_else(|| ParakeetError::OutputNotFound("outputs".to_string()))?
            .try_extract_array()?;
        log::trace!(
            "Parakeet Logits shape: {:?}, vocab_size: {}",
            logits.shape(),
            self.vocab_size
        );
        let state1 = outputs
            .get("output_states_1")
            .ok_or_else(|| ParakeetError::OutputNotFound("output_states_1".to_string()))?
            .try_extract_array()?;
        let state2 = outputs
            .get("output_states_2")
            .ok_or_else(|| ParakeetError::OutputNotFound("output_states_2".to_string()))?
            .try_extract_array()?;

        // Squeeze outputs like Python (remove batch dimension)
        let logits = logits.remove_axis(ndarray::Axis(0));

        // Convert ArrayD back to Array3 to match expected return type
        let state1_3d = state1.to_owned().into_dimensionality::<ndarray::Ix3>()?;
        let state2_3d = state2.to_owned().into_dimensionality::<ndarray::Ix3>()?;

        Ok((logits.to_owned(), (state1_3d, state2_3d)))
    }

    pub fn recognize_batch(
        &mut self,
        waveforms: &ArrayViewD<f32>,
        waveforms_len: &ArrayViewD<i64>,
    ) -> Result<Vec<TimestampedResult>, ParakeetError> {
        // Preprocess and encode
        let (features, features_lens) = self.preprocess(waveforms, waveforms_len)?;
        let (encoder_out, encoder_out_lens) =
            self.encode(&features.view(), &features_lens.view())?;

        // Decode for each batch item
        let mut results = Vec::new();
        for (encodings, &encodings_len) in encoder_out.outer_iter().zip(encoder_out_lens.iter()) {
            let (tokens, timestamps) =
                self.decode_sequence(&encodings.view(), encodings_len as usize)?;
            let result = self.decode_tokens(tokens, timestamps);
            results.push(result);
        }

        Ok(results)
    }

    fn decode_sequence(
        &mut self,
        encodings: &ArrayViewD<f32>, // [time_steps, 1024]
        encodings_len: usize,
    ) -> Result<(Vec<i32>, Vec<usize>), ParakeetError> {
        let mut prev_state = self.create_decoder_state()?;
        let mut tokens = Vec::new();
        let mut timestamps = Vec::new();

        let mut t = 0;
        let mut emitted_tokens = 0;

        while t < encodings_len {
            let encoder_step = encodings.slice(ndarray::s![t, ..]);
            // Convert to dynamic dimension to match decode_step parameter type
            let encoder_step_dyn = encoder_step.to_owned().into_dyn();
            let (probs, new_state) =
                self.decode_step(&tokens, &prev_state, &encoder_step_dyn.view())?;

            // For TDT models, split output into vocab logits and duration logits
            // output[:vocab_size] = vocabulary logits
            // output[vocab_size:] = duration logits
            let vocab_logits_slice = probs.as_slice().ok_or_else(|| {
                ParakeetError::Shape(ndarray::ShapeError::from_kind(
                    ndarray::ErrorKind::IncompatibleShape,
                ))
            })?;

            let is_tdt = probs.len() > self.vocab_size;
            let (vocab_logits, duration_logits) = if is_tdt {
                let (v, d) = vocab_logits_slice.split_at(self.vocab_size);
                (v, Some(d))
            } else {
                (vocab_logits_slice, None)
            };

            // Get argmax token from vocabulary logits only
            let token = vocab_logits
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(idx, _)| idx as i32)
                .unwrap_or(self.blank_idx);

            if token != self.blank_idx {
                prev_state = new_state;
                tokens.push(token);
                timestamps.push(t);
                emitted_tokens += 1;
            }

            if let Some(duration_logits) = duration_logits {
                // TDT: advance by the model's predicted duration (frames to skip).
                let dur_idx = duration_logits
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(idx, _)| idx)
                    .unwrap_or(0);
                let mut skip = TDT_DURATIONS.get(dur_idx).copied().unwrap_or(1);

                // Ensure forward progress on blank-with-zero-duration, and cap
                // same-frame emissions to avoid runaway repetition.
                if skip == 0 && (token == self.blank_idx || emitted_tokens >= MAX_TOKENS_PER_STEP) {
                    skip = 1;
                }
                if skip > 0 {
                    t += skip;
                    emitted_tokens = 0;
                }
            } else {
                // RNN-T greedy: advance one frame on blank or after emission cap.
                if token == self.blank_idx || emitted_tokens >= MAX_TOKENS_PER_STEP {
                    t += 1;
                    emitted_tokens = 0;
                }
            }
        }

        // NEW: Log if no tokens were decoded (helps debugging empty transcriptions)
        if tokens.is_empty() {
            log::debug!(
                "Parakeet decoded zero tokens (all blank) for audio with {} encoding timesteps - audio may be too short or low energy",
                encodings_len
            );
        }

        Ok((tokens, timestamps))
    }

    fn decode_tokens(&self, ids: Vec<i32>, timestamps: Vec<usize>) -> TimestampedResult {
        let tokens: Vec<String> = ids
            .iter()
            .filter_map(|&id| {
                let idx = id as usize;
                if idx < self.vocab.len() {
                    Some(self.vocab[idx].clone())
                } else {
                    None
                }
            })
            .collect();

        let text = match &*DECODE_SPACE_RE {
            Ok(regex) => regex
                .replace_all(&tokens.join(""), |caps: &regex::Captures| {
                    if caps.get(1).is_some() {
                        " "
                    } else {
                        ""
                    }
                })
                .to_string(),
            Err(_) => tokens.join(""), // Fallback if regex failed to compile
        };

        let float_timestamps: Vec<f32> = timestamps
            .iter()
            .map(|&t| WINDOW_SIZE * SUBSAMPLING_FACTOR as f32 * t as f32)
            .collect();

        TimestampedResult {
            text,
            timestamps: float_timestamps,
            tokens,
        }
    }

    /// Retry an audible segment that decoded to blank with a short silence lead.
    pub fn transcribe_samples_with_retry(
        &mut self,
        samples: Vec<f32>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TimestampedResult, ParakeetError> {
        transcribe_with_empty_retry(samples, is_cancelled, |s| self.transcribe_samples(s))
    }

    pub fn transcribe_samples(
        &mut self,
        mut samples: Vec<f32>,
    ) -> Result<TimestampedResult, ParakeetError> {
        raise_quiet_segment(&mut samples);
        let batch_size = 1;
        let samples_len = samples.len();

        // Create waveforms array [batch_size, samples_len]
        let waveforms = Array2::from_shape_vec((batch_size, samples_len), samples)?.into_dyn();

        // Create waveforms_lens array [batch_size] with the actual length
        let waveforms_lens = Array1::from_vec(vec![samples_len as i64]).into_dyn();

        // Run recognition to get detailed results
        let results = self.recognize_batch(&waveforms.view(), &waveforms_lens.view())?;

        // Extract the first (and only) result
        let timestamped_result = results.into_iter().next().ok_or_else(|| {
            ParakeetError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "No transcription result returned",
            ))
        })?;

        Ok(timestamped_result)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        raise_quiet_segment, shift_timestamps, transcribe_with_empty_retry, ParakeetError,
        TimestampedResult, SAMPLE_RATE,
    };

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn quiet_segments_are_raised_and_normal_or_silent_ones_are_left_alone() {
        // Quiet speech level (about -37 dBFS) is raised to -20 dBFS.
        let mut quiet: Vec<f32> = (0..SAMPLE_RATE)
            .map(|i| 0.02 * (i as f32 * 0.05).sin())
            .collect();
        raise_quiet_segment(&mut quiet);
        assert!((rms(&quiet) - 0.1).abs() < 0.005, "rms {}", rms(&quiet));

        // A normal level and near-silence are returned bit for bit.
        for amplitude in [0.2f32, 0.0005] {
            let original: Vec<f32> = (0..SAMPLE_RATE)
                .map(|i| amplitude * (i as f32 * 0.05).sin())
                .collect();
            let mut samples = original.clone();
            raise_quiet_segment(&mut samples);
            assert_eq!(samples, original);
        }

        // A loud click in a quiet segment is clipped, never wrapped or left above full scale.
        let mut click = vec![0.01f32; SAMPLE_RATE];
        click[100] = 0.9;
        raise_quiet_segment(&mut click);
        assert!(click.iter().all(|sample| sample.abs() <= 1.0));
        assert_eq!(click[100], 1.0);

        let mut empty: Vec<f32> = Vec::new();
        raise_quiet_segment(&mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn quiet_segment_gain_is_capped_and_spares_a_normal_level_stretch() {
        // Very quiet audio (-54 dBFS) would need a gain of 50; it gets 32.
        let mut faint = vec![0.002f32; SAMPLE_RATE];
        raise_quiet_segment(&mut faint);
        assert!((faint[0] - 0.064).abs() < 1e-5, "sample {}", faint[0]);

        // Ten seconds, quiet on average, with 0.2 s at a normal level: at most
        // 0.1% of the samples may reach full scale (none for this sine, whose
        // peak is close to its typical level), and the quiet part is still raised.
        let mut mixed: Vec<f32> = (0..SAMPLE_RATE * 10)
            .map(|i| {
                let amplitude = if i < SAMPLE_RATE / 5 { 0.4 } else { 0.005 };
                amplitude * (i as f32 * 0.05).sin()
            })
            .collect();
        let quiet_before = rms(&mixed[SAMPLE_RATE..]);
        raise_quiet_segment(&mut mixed);
        let loudest = mixed.iter().fold(0.0f32, |max, sample| max.max(sample.abs()));
        assert!(loudest < 1.0, "loudest {loudest}");
        let clipped = mixed.iter().filter(|sample| sample.abs() >= 1.0).count();
        assert!(clipped <= mixed.len() / 1000);
        assert!(rms(&mixed[SAMPLE_RATE..]) > quiet_before * 1.5);

        // Infinite or NaN input is left alone.
        let mut broken = vec![0.01f32, f32::NAN, f32::INFINITY, 0.01];
        raise_quiet_segment(&mut broken);
        assert_eq!(broken[0], 0.01);
        assert!(broken[1].is_nan());
        assert_eq!(broken[2], f32::INFINITY);
        assert_eq!(broken[3], 0.01);
    }

    fn result(text: &str) -> TimestampedResult {
        TimestampedResult {
            text: text.to_string(),
            timestamps: vec![0.1, 0.3, 0.8, 2.3],
            tokens: vec!["first".to_string(), "second".to_string()],
        }
    }

    fn assert_same_result(actual: &TimestampedResult, expected: &TimestampedResult) {
        assert_eq!(actual.text, expected.text);
        assert_eq!(actual.timestamps, expected.timestamps);
        assert_eq!(actual.tokens, expected.tokens);
    }

    #[test]
    fn non_empty_first_result_is_returned_untouched_after_one_decode() {
        let samples = vec![0.1; 4 * SAMPLE_RATE];
        let original_ptr = samples.as_ptr();
        let expected = result("one");
        let mut calls = 0;
        let actual = transcribe_with_empty_retry(samples, &|| false, |input| {
            calls += 1;
            assert_eq!(input.as_ptr(), original_ptr);
            Ok(expected.clone())
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_same_result(&actual, &expected);
    }

    #[test]
    fn empty_result_recovers_with_300_ms_padding_and_adjusted_timestamps() {
        let recovered = result("three recovered words");
        let mut responses = vec![Ok(result("")), Ok(recovered.clone())].into_iter();
        let mut inputs = Vec::new();
        let actual = transcribe_with_empty_retry(vec![0.1; 4 * SAMPLE_RATE], &|| false, |input| {
            inputs.push(input);
            responses.next().expect("unexpected decode")
        })
        .unwrap();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].len(), 4 * SAMPLE_RATE);
        assert_eq!(inputs[1].len(), 4 * SAMPLE_RATE + 4_800);
        assert!(inputs[1][..4_800].iter().all(|&sample| sample == 0.0));
        assert_eq!(&inputs[1][4_800..], inputs[0].as_slice());
        assert_eq!(actual.text, recovered.text);
        assert_eq!(actual.tokens, recovered.tokens);
        for (actual, expected) in actual.timestamps.iter().zip([0.0, 0.0, 0.5, 2.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        assert_eq!(actual.timestamps.len(), 4);
    }

    #[test]
    fn empty_results_recover_with_1000_ms_padding_on_the_third_decode() {
        let recovered = result("three recovered words");
        let mut responses = vec![Ok(result("")), Ok(result("")), Ok(recovered.clone())].into_iter();
        let mut inputs = Vec::new();
        let actual = transcribe_with_empty_retry(vec![0.1; 4 * SAMPLE_RATE], &|| false, |input| {
            inputs.push(input);
            responses.next().expect("unexpected decode")
        })
        .unwrap();
        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs[1].len(), 4 * SAMPLE_RATE + 4_800);
        assert_eq!(inputs[2].len(), 5 * SAMPLE_RATE);
        assert!(inputs[2][..SAMPLE_RATE].iter().all(|&sample| sample == 0.0));
        assert_eq!(&inputs[2][SAMPLE_RATE..], inputs[0].as_slice());
        assert_eq!(actual.text, recovered.text);
        assert_eq!(actual.tokens, recovered.tokens);
        for (actual, expected) in actual.timestamps.iter().zip([0.0, 0.0, 0.0, 1.3]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        assert_eq!(actual.timestamps.len(), 4);
    }

    #[test]
    fn all_empty_decodes_return_the_original_empty_result() {
        let original = result(" \n\t");
        let mut calls = 0;
        let actual = transcribe_with_empty_retry(vec![0.1; 4 * SAMPLE_RATE], &|| false, |_| {
            calls += 1;
            Ok(if calls == 1 {
                original.clone()
            } else {
                result("")
            })
        })
        .unwrap();
        assert_eq!(calls, 3);
        assert_same_result(&actual, &original);
    }

    #[test]
    fn retry_decode_error_preserves_the_original_successful_empty_result() {
        let original = result(" \n\t");
        let mut calls = 0;
        let actual = transcribe_with_empty_retry(vec![0.1; 4 * SAMPLE_RATE], &|| false, |_| {
            calls += 1;
            if calls == 1 {
                Ok(original.clone())
            } else {
                Err(ParakeetError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "fake decode failure",
                )))
            }
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_same_result(&actual, &original);
    }

    #[test]
    fn a_short_recovered_answer_is_kept() {
        let recovered = result("Ja");
        let mut responses = vec![Ok(result("")), Ok(recovered.clone())].into_iter();
        let mut calls = 0;
        let actual = transcribe_with_empty_retry(vec![0.1; 4 * SAMPLE_RATE], &|| false, |_| {
            calls += 1;
            responses.next().expect("unexpected decode")
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(actual.text, recovered.text);
        assert_eq!(actual.tokens, recovered.tokens);
    }

    #[test]
    fn cancellation_before_a_retry_stops_after_the_first_decode() {
        let original = result("");
        let mut calls = 0;
        let actual = transcribe_with_empty_retry(vec![0.1; 4 * SAMPLE_RATE], &|| true, |_| {
            calls += 1;
            Ok(original.clone())
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_same_result(&actual, &original);
    }

    #[test]
    fn cancellation_is_checked_before_each_retry() {
        let original = result("");
        let cancelled = std::cell::Cell::new(false);
        let mut calls = 0;
        let actual =
            transcribe_with_empty_retry(vec![0.1; 4 * SAMPLE_RATE], &|| cancelled.get(), |_| {
                calls += 1;
                if calls == 2 {
                    cancelled.set(true);
                }
                Ok(original.clone())
            })
            .unwrap();
        assert_eq!(calls, 2);
        assert_same_result(&actual, &original);
    }

    #[test]
    fn short_or_quiet_audio_is_not_retried() {
        let original = result("");
        for samples in [
            vec![],
            vec![0.1; SAMPLE_RATE - 1],
            vec![0.001; 2 * SAMPLE_RATE],
            vec![0.0; 2 * SAMPLE_RATE],
        ] {
            let mut calls = 0;
            let actual = transcribe_with_empty_retry(samples, &|| false, |_| {
                calls += 1;
                Ok(original.clone())
            })
            .unwrap();
            assert_eq!(calls, 1);
            assert_same_result(&actual, &original);
        }
    }

    #[test]
    fn retry_timestamps_subtract_padding_and_clamp_at_zero() {
        for padding in [0.3, 1.0] {
            let mut timestamps = vec![0.0, padding / 2.0, padding, padding + 0.5, padding + 2.0];
            shift_timestamps(&mut timestamps, padding);
            for (actual, expected) in timestamps.iter().zip([0.0, 0.0, 0.0, 0.5, 2.0]) {
                assert!((actual - expected).abs() < 1e-6);
            }
        }
        shift_timestamps(&mut [], 0.3);
    }
}
