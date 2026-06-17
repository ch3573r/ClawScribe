// nemotron_engine/model.rs
//
// Streaming RNN-T inference for Nemotron 3.5 ASR (encoder / decoder / joint).
//
// Architecture (resolved from genai_config.json — see
// NEMOTRON_IMPLEMENTATION_PLAN.md):
//   encoder.onnx  audio_signal[B,128,T] + length + cache_last_channel +
//                 cache_last_time + cache_last_channel_len + lang_id
//                 -> outputs[B,D,T'] + encoded_lengths + *_next caches
//   decoder.onnx  targets[B,U] + h_in + c_in -> decoder_output + h_out + c_out  (LSTM, 2 layers)
//   joint.onnx    encoder_output + decoder_output -> joint_output[..,13088]
//   blank_id 13087  vocab 13088  subsampling 8  chunk_samples 8960  max_symbols 10
//
// We run per-VAD-segment (caller hands us a speech segment): mel features for
// the whole segment, fed through the encoder in 560 ms (8960-sample / 56-frame)
// chunks with the cache tensors threaded WITHIN the segment, then a greedy
// RNN-T decode over the accumulated encoder frames. Caches reset per segment.
//
// IMPORTANT: the exact ranks/layouts of the cache, LSTM-state, and joint frame
// tensors are not published in the model's configs; the shapes below follow
// NeMo's standard cache-aware streaming RNN-T ONNX export and MUST be validated
// on-device (the 790 MB INT4 model can't run in CI). Session inputs are logged
// at load to make the first on-device bring-up a quick fix if anything differs.

use ndarray::{Array, Array1, Array2, ArrayD, IxDyn};
use once_cell::sync::Lazy;
use ort::execution_providers::CPUExecutionProvider;
#[cfg(feature = "directml")]
use ort::execution_providers::DirectMLExecutionProvider;
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use regex::Regex;

use std::fs;
use std::path::Path;

use super::features::{MelExtractor, HOP_LENGTH, N_MELS};

// From genai_config.json.
const BLANK_ID: i32 = 13087;
const VOCAB_SIZE: usize = 13088;
const CHUNK_SAMPLES: usize = 8960; // 560 ms @ 16 kHz
const MAX_SYMBOLS_PER_STEP: usize = 10;
/// Default language id for the encoder `lang_id` input. The language→id table
/// isn't published; 0 is the conventional first/primary (English) slot. Revisit
/// when the table is resolved (plan §6).
const DEFAULT_LANG_ID: i64 = 0;

static DECODE_SPACE_RE: Lazy<Result<Regex, regex::Error>> =
    Lazy::new(|| Regex::new(r"\A\s|\s\B|(\s)\b"));

#[derive(thiserror::Error, Debug)]
pub enum NemotronError {
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
}

pub struct NemotronModel {
    encoder: Session,
    decoder: Session,
    joint: Session,
    mel: MelExtractor,
    vocab: Vec<String>,
    lang_id: i64,
}

impl NemotronModel {
    pub fn new<P: AsRef<Path>>(model_dir: P, use_directml: bool) -> Result<Self, NemotronError> {
        let dir = model_dir.as_ref();
        // ort loads the external-data `.onnx.data` files automatically as long
        // as they sit next to the `.onnx` graph (they're downloaded together).
        let encoder = Self::init_session(dir, "encoder.onnx", use_directml)?;
        let decoder = Self::init_session(dir, "decoder.onnx", use_directml)?;
        let joint = Self::init_session(dir, "joint.onnx", use_directml)?;

        let vocab = Self::load_vocab(dir)?;
        log::info!(
            "Loaded Nemotron vocabulary with {} tokens (expected {}), blank_id={}",
            vocab.len(),
            VOCAB_SIZE,
            BLANK_ID
        );

        Ok(Self {
            encoder,
            decoder,
            joint,
            mel: MelExtractor::new(),
            vocab,
            lang_id: DEFAULT_LANG_ID,
        })
    }

    fn init_session<P: AsRef<Path>>(
        model_dir: P,
        filename: &str,
        use_directml: bool,
    ) -> Result<Session, NemotronError> {
        let mut providers = Vec::new();
        #[cfg(feature = "directml")]
        if use_directml {
            log::info!("Nemotron: registering DirectML execution provider for {filename}");
            providers.push(DirectMLExecutionProvider::default().build());
        }
        #[cfg(not(feature = "directml"))]
        let _ = use_directml;
        providers.push(CPUExecutionProvider::default().build());

        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_execution_providers(providers)?
            .with_parallel_execution(true)?
            .commit_from_file(model_dir.as_ref().join(filename))?;

        // Log I/O so on-device shape mismatches are obvious on first bring-up.
        for input in &session.inputs {
            log::info!(
                "Nemotron '{}' input: name={}, type={:?}",
                filename,
                input.name,
                input.input_type
            );
        }
        for output in &session.outputs {
            log::info!(
                "Nemotron '{}' output: name={}, type={:?}",
                filename,
                output.name,
                output.output_type
            );
        }
        Ok(session)
    }

    /// Load the sentencepiece vocab. Accepts either "token id" (space-separated,
    /// like Parakeet) or one-token-per-line (id = line index). `▁` → space.
    fn load_vocab<P: AsRef<Path>>(model_dir: P) -> Result<Vec<String>, NemotronError> {
        let content = fs::read_to_string(model_dir.as_ref().join("vocab.txt"))?;

        // Detect the format from the first non-empty line.
        let explicit_ids = content
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| {
                let parts: Vec<&str> = l.trim_end().rsplitn(2, char::is_whitespace).collect();
                parts.len() == 2 && parts[0].parse::<usize>().is_ok()
            })
            .unwrap_or(false);

        let mut vocab = vec![String::new(); VOCAB_SIZE];
        if explicit_ids {
            for line in content.lines() {
                let mut it = line.trim_end().rsplitn(2, char::is_whitespace);
                if let (Some(id_str), Some(token)) = (it.next(), it.next()) {
                    if let Ok(id) = id_str.parse::<usize>() {
                        if id < vocab.len() {
                            vocab[id] = token.replace('\u{2581}', " ");
                        }
                    }
                }
            }
        } else {
            for (id, line) in content.lines().enumerate() {
                if id < vocab.len() {
                    // first whitespace-delimited field is the token
                    let token = line.split_whitespace().next().unwrap_or("");
                    vocab[id] = token.replace('\u{2581}', " ");
                }
            }
        }
        Ok(vocab)
    }

    pub fn set_lang_id(&mut self, lang_id: i64) {
        self.lang_id = lang_id;
    }

    /// Build a zero-filled f32 tensor matching a session input's declared shape,
    /// substituting batch=1 for any dynamic (≤0) dimension.
    fn zeros_for_input(session: &Session, name: &str) -> Result<ArrayD<f32>, NemotronError> {
        let shape = session
            .inputs
            .iter()
            .find(|i| i.name == name)
            .and_then(|i| i.input_type.tensor_shape().cloned())
            .ok_or_else(|| NemotronError::InputNotFound(name.to_string()))?;
        let dims: Vec<usize> = shape
            .iter()
            .map(|&d| if d <= 0 { 1 } else { d as usize })
            .collect();
        Ok(ArrayD::zeros(IxDyn(&dims)))
    }

    /// Transcribe a mono 16 kHz speech segment to text.
    pub fn transcribe_samples(&mut self, samples: Vec<f32>) -> Result<String, NemotronError> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        // 1. Log-mel features for the whole segment → [N_MELS, T_total].
        let mel = self.mel.compute(&samples); // mel-major
        let total_frames = mel.first().map(|r| r.len()).unwrap_or(0);
        if total_frames == 0 {
            return Ok(String::new());
        }

        // 2. Stream the features through the encoder in fixed chunks, threading
        //    the caches. Chunk size in feature frames = CHUNK_SAMPLES / hop.
        let chunk_frames = (CHUNK_SAMPLES / HOP_LENGTH).max(1); // 56
        let mut cache_channel = Self::zeros_for_input(&self.encoder, "cache_last_channel")?;
        let mut cache_time = Self::zeros_for_input(&self.encoder, "cache_last_time")?;
        let mut cache_len: Array1<i64> = Array1::zeros(1);

        // Accumulated encoder frames: each row is a D-dim encoder output vector.
        let mut enc_frames: Vec<Vec<f32>> = Vec::new();

        let mut start = 0usize;
        while start < total_frames {
            let end = (start + chunk_frames).min(total_frames);
            let t = end - start;

            // audio_signal: [1, N_MELS, t]
            let mut audio = Array::zeros((1, N_MELS, t));
            for m in 0..N_MELS {
                for (j, frame) in (start..end).enumerate() {
                    audio[[0, m, j]] = mel[m][frame];
                }
            }
            let audio = audio.into_dyn();
            let length: Array1<i64> = Array1::from_vec(vec![t as i64]);
            let lang: Array1<i64> = Array1::from_vec(vec![self.lang_id]);

            let outputs = self.encoder.run(inputs![
                "audio_signal" => TensorRef::from_array_view(audio.view())?,
                "length" => TensorRef::from_array_view(length.view())?,
                "cache_last_channel" => TensorRef::from_array_view(cache_channel.view())?,
                "cache_last_time" => TensorRef::from_array_view(cache_time.view())?,
                "cache_last_channel_len" => TensorRef::from_array_view(cache_len.view())?,
                "lang_id" => TensorRef::from_array_view(lang.view())?,
            ])?;

            // outputs: [B, D, T'] → push each time-step as a D-vector.
            let enc = outputs
                .get("outputs")
                .ok_or_else(|| NemotronError::OutputNotFound("outputs".into()))?
                .try_extract_array::<f32>()?;
            let enc = enc.into_dimensionality::<ndarray::Ix3>()?; // [1, D, T']
            let d = enc.shape()[1];
            let tp = enc.shape()[2];
            for ti in 0..tp {
                let mut v = Vec::with_capacity(d);
                for di in 0..d {
                    v.push(enc[[0, di, ti]]);
                }
                enc_frames.push(v);
            }

            // Thread caches forward.
            cache_channel = outputs
                .get("cache_last_channel_next")
                .ok_or_else(|| NemotronError::OutputNotFound("cache_last_channel_next".into()))?
                .try_extract_array::<f32>()?
                .to_owned();
            cache_time = outputs
                .get("cache_last_time_next")
                .ok_or_else(|| NemotronError::OutputNotFound("cache_last_time_next".into()))?
                .try_extract_array::<f32>()?
                .to_owned();
            cache_len = outputs
                .get("cache_last_channel_len_next")
                .ok_or_else(|| NemotronError::OutputNotFound("cache_last_channel_len_next".into()))?
                .try_extract_array::<i64>()?
                .into_dimensionality::<ndarray::Ix1>()?
                .to_owned();

            start = end;
        }

        // 3. Greedy RNN-T decode over the accumulated encoder frames.
        let ids = self.greedy_decode(&enc_frames)?;
        Ok(self.decode_tokens(&ids))
    }

    fn greedy_decode(&mut self, enc_frames: &[Vec<f32>]) -> Result<Vec<i32>, NemotronError> {
        if enc_frames.is_empty() {
            return Ok(Vec::new());
        }
        let d_enc = enc_frames[0].len();

        // LSTM states (zeros) + initial decoder step with the blank/SOS target.
        let mut h = Self::zeros_for_input(&self.decoder, "h_in")?;
        let mut c = Self::zeros_for_input(&self.decoder, "c_in")?;
        let (mut dec_out, nh, nc) = self.run_decoder(BLANK_ID, &h, &c)?;
        h = nh;
        c = nc;

        let mut tokens: Vec<i32> = Vec::new();
        for frame in enc_frames {
            let mut emitted = 0usize;
            loop {
                let logits = self.run_joint(frame, d_enc, &dec_out)?;
                let token = argmax(&logits);
                if token == BLANK_ID || emitted >= MAX_SYMBOLS_PER_STEP {
                    break;
                }
                tokens.push(token);
                let (nd, nh, nc) = self.run_decoder(token, &h, &c)?;
                dec_out = nd;
                h = nh;
                c = nc;
                emitted += 1;
            }
        }

        if tokens.is_empty() {
            log::debug!(
                "Nemotron decoded zero tokens (all blank) over {} encoder frames",
                enc_frames.len()
            );
        }
        Ok(tokens)
    }

    /// Run the LSTM prediction network for one target token. Returns
    /// (decoder_output, h_out, c_out).
    fn run_decoder(
        &mut self,
        token: i32,
        h_in: &ArrayD<f32>,
        c_in: &ArrayD<f32>,
    ) -> Result<(ArrayD<f32>, ArrayD<f32>, ArrayD<f32>), NemotronError> {
        let targets = Array2::from_shape_vec((1, 1), vec![token])?.into_dyn();
        let outputs = self.decoder.run(inputs![
            "targets" => TensorRef::from_array_view(targets.view())?,
            "h_in" => TensorRef::from_array_view(h_in.view())?,
            "c_in" => TensorRef::from_array_view(c_in.view())?,
        ])?;
        let dec_out = outputs
            .get("decoder_output")
            .ok_or_else(|| NemotronError::OutputNotFound("decoder_output".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        let h_out = outputs
            .get("h_out")
            .ok_or_else(|| NemotronError::OutputNotFound("h_out".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        let c_out = outputs
            .get("c_out")
            .ok_or_else(|| NemotronError::OutputNotFound("c_out".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        Ok((dec_out, h_out, c_out))
    }

    /// Run the joint network for one encoder frame + the current decoder output.
    /// Encoder frame is shaped [1, 1, D] (batch, time=1, feat); the decoder
    /// output is passed through as the export produced it.
    fn run_joint(
        &mut self,
        enc_frame: &[f32],
        d_enc: usize,
        dec_out: &ArrayD<f32>,
    ) -> Result<Vec<f32>, NemotronError> {
        let enc = Array::from_shape_vec((1, 1, d_enc), enc_frame.to_vec())?.into_dyn();
        let outputs = self.joint.run(inputs![
            "encoder_output" => TensorRef::from_array_view(enc.view())?,
            "decoder_output" => TensorRef::from_array_view(dec_out.view())?,
        ])?;
        let logits = outputs
            .get("joint_output")
            .ok_or_else(|| NemotronError::OutputNotFound("joint_output".into()))?
            .try_extract_array::<f32>()?;
        // Squeeze to a flat vocab vector regardless of [B,1,1,V]-style shape.
        Ok(logits.iter().copied().collect())
    }

    fn decode_tokens(&self, ids: &[i32]) -> String {
        let tokens: Vec<String> = ids
            .iter()
            .filter_map(|&id| self.vocab.get(id as usize).cloned())
            .collect();
        match &*DECODE_SPACE_RE {
            Ok(re) => re
                .replace_all(&tokens.join(""), |caps: &regex::Captures| {
                    if caps.get(1).is_some() {
                        " "
                    } else {
                        ""
                    }
                })
                .to_string(),
            Err(_) => tokens.join(""),
        }
    }
}

fn argmax(logits: &[f32]) -> i32 {
    logits
        .iter()
        .take(VOCAB_SIZE)
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i as i32)
        .unwrap_or(BLANK_ID)
}
