// nemotron_engine/model.rs
//
// Streaming RNN-T inference for the soniqo Nemotron 3.5 ASR ONNX export
// (int8 / fp16). Interface reverse-engineered from the model graphs + soniqo's
// open-source speech-core decoder:
//
//   encoder.onnx
//     in:  audio_signal[1,128,32] (feature-major mel), audio_length:i32[1],
//          language_mask[1,128] (one-hot at the language's prompt slot),
//          pre_cache[1,128,9], cache_last_channel[24,1,56,1024],
//          cache_last_time[24,1,1024,8], cache_last_channel_len:i32[1]
//     out: encoded_output[1,T,1024], encoded_length, new_pre_cache,
//          new_cache_last_channel, new_cache_last_time, new_cache_last_channel_len
//   decoder.onnx  token:i64[1,1] + h[2,1,640] + c[2,1,640]
//                 -> decoder_output[1,1,640] + h_out + c_out
//   joint.onnx    encoder_output[1,1,1024] + decoder_output[1,1,640]
//                 -> logits[1,1,13088]   (blank id 13087 = vocab_size)
//
// Per VAD segment: zero the caches, prime the predictor with the blank token,
// then stream 320 ms (5120-sample / 32-mel-frame) windows, threading the
// pre_cache + conformer caches across windows, and run a greedy RNN-T over each
// window's encoder frames. `language_mask` as one-hot is the one unverified
// assumption (the prompt-conditioning mask is undocumented) — validate de/en
// on-device.

use ndarray::{Array, Array1, Array2, ArrayD, IxDyn};
use ort::execution_providers::CPUExecutionProvider;
#[cfg(feature = "directml")]
use ort::execution_providers::DirectMLExecutionProvider;
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::features::MelExtractor;

// Fixed model geometry (identical across the int8/fp16 exports — config.json).
const MEL_BINS: usize = 128;
const HOP: usize = 160;
const CHUNK_MEL_FRAMES: usize = 32; // 320 ms
const WIN_SAMPLES: usize = CHUNK_MEL_FRAMES * HOP; // 5120
const PRE_CACHE_SIZE: usize = 9;
const ENCODER_LAYERS: usize = 24;
const ATTN_LEFT_CONTEXT: usize = 56;
const ENCODER_HIDDEN: usize = 1024;
const CONV_CACHE_SIZE: usize = 8;
const DECODER_LAYERS: usize = 2;
const DECODER_HIDDEN: usize = 640;
const NUM_PROMPTS: usize = 128;
const BLANK_ID: i32 = 13087; // == vocab_size; the extra logit
const N_LOGITS: usize = 13088;
const MAX_SYMBOLS: usize = 10;
/// soniqo's log-mel guard: ln(x + 2^-24).
const LOG_FLOOR: f32 = 1.0 / (1u32 << 24) as f32;
/// Fallback language prompt slot (en-US) when the requested code isn't known.
const DEFAULT_LANG_SLOT: i64 = 0;

#[derive(thiserror::Error, Debug)]
pub enum NemotronError {
    #[error("ORT error: {0}")]
    Ort(#[from] ort::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ndarray shape error: {0}")]
    Shape(#[from] ndarray::ShapeError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Model output not found: {0}")]
    OutputNotFound(String),
    #[error("This Nemotron model needs a DirectML-capable GPU (its int8 ops have no CPU implementation). Use a DirectML build/GPU, or pick the fp16 Nemotron or another engine.")]
    CpuUnsupported,
}

pub struct NemotronModel {
    encoder: Session,
    decoder: Session,
    joint: Session,
    mel: MelExtractor,
    vocab: Vec<String>,
    lang_slots: HashMap<String, i64>,
}

impl NemotronModel {
    pub fn new<P: AsRef<Path>>(model_dir: P) -> Result<Self, NemotronError> {
        let dir = model_dir.as_ref();
        // The encoder is the heavy part AND the one DirectML mishandles. Try it
        // on DirectML (graph optimizations disabled — a DML fusion/layout rewrite
        // is the prime suspect for the encoder collapse) and self-test the output;
        // fall back to CPU if it's still collapsed. decoder/joint are tiny and
        // correct on CPU, so keep them there.
        let encoder = Self::load_encoder(dir)?;
        let decoder = Self::init_session(dir, "decoder.onnx")?;
        let joint = Self::init_session(dir, "joint.onnx")?;
        let vocab = Self::load_vocab(dir)?;
        let lang_slots = Self::load_lang_slots(dir).unwrap_or_default();
        log::info!(
            "Loaded Nemotron: {} vocab tokens, {} language slots, blank_id={}",
            vocab.len(),
            lang_slots.len(),
            BLANK_ID
        );
        Ok(Self {
            encoder,
            decoder,
            joint,
            mel: MelExtractor::new(),
            vocab,
            lang_slots,
        })
    }

    /// CPU session (decoder/joint, and the encoder's fallback). fp16 runs
    /// correctly here; int8's ConvInteger has no CPU kernel → clear error.
    fn init_session<P: AsRef<Path>>(
        model_dir: P,
        filename: &str,
    ) -> Result<Session, NemotronError> {
        let path = model_dir.as_ref().join(filename);
        Self::build_session(&path, false).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("ConvInteger") {
                NemotronError::CpuUnsupported
            } else {
                e
            }
        })
    }

    /// Load the encoder, preferring DirectML (GPU) with graph optimizations
    /// DISABLED — the encoder collapses on DML at full optimization, and a buggy
    /// DML fusion/layout rewrite is the prime suspect. We self-test the loaded
    /// session and fall back to CPU if the output is still collapsed, so the
    /// model is always correct (worst case: slower CPU).
    fn load_encoder(dir: &Path) -> Result<Session, NemotronError> {
        let path = dir.join("encoder.onnx");
        #[cfg(feature = "directml")]
        {
            match Self::build_session(&path, true) {
                Ok(mut session) => {
                    if Self::encoder_self_test(&mut session) {
                        log::info!("Nemotron encoder: DirectML (GPU), self-test passed");
                        return Ok(session);
                    }
                    log::warn!(
                        "Nemotron encoder: DirectML output collapsed (self-test failed); using CPU"
                    );
                }
                Err(e) => log::warn!("Nemotron encoder: DirectML init failed ({e}); using CPU"),
            }
        }
        let session = Self::init_session(dir, "encoder.onnx")?;
        log::info!("Nemotron encoder: CPU");
        Ok(session)
    }

    /// Feed a synthetic energetic mel through the encoder and check the output
    /// isn't collapsed. A correct encoder yields activations well above ±1; the
    /// broken-DML path collapses to ~±0.25.
    #[cfg(feature = "directml")]
    fn encoder_self_test(enc: &mut Session) -> bool {
        let mut audio = Array::zeros((1, MEL_BINS, CHUNK_MEL_FRAMES));
        for b in 0..MEL_BINS {
            for t in 0..CHUNK_MEL_FRAMES {
                audio[[0, b, t]] = (b as f32 * 0.13 + t as f32 * 0.31).sin() * 3.0;
            }
        }
        let audio = audio.into_dyn();
        let length = Array1::<i32>::from_vec(vec![CHUNK_MEL_FRAMES as i32]);
        let chl = Array1::<i32>::from_vec(vec![0]);
        let mut mask = Array2::<f32>::zeros((1, NUM_PROMPTS));
        mask[[0, 0]] = 1.0;
        let mask = mask.into_dyn();
        let pre = ArrayD::<f32>::zeros(IxDyn(&[1, MEL_BINS, PRE_CACHE_SIZE]));
        let clc =
            ArrayD::<f32>::zeros(IxDyn(&[ENCODER_LAYERS, 1, ATTN_LEFT_CONTEXT, ENCODER_HIDDEN]));
        let clt =
            ArrayD::<f32>::zeros(IxDyn(&[ENCODER_LAYERS, 1, ENCODER_HIDDEN, CONV_CACHE_SIZE]));
        let run = (|| -> Result<f32, NemotronError> {
            let out = enc.run(inputs![
                "audio_signal" => TensorRef::from_array_view(audio.view())?,
                "audio_length" => TensorRef::from_array_view(length.view())?,
                "language_mask" => TensorRef::from_array_view(mask.view())?,
                "pre_cache" => TensorRef::from_array_view(pre.view())?,
                "cache_last_channel" => TensorRef::from_array_view(clc.view())?,
                "cache_last_time" => TensorRef::from_array_view(clt.view())?,
                "cache_last_channel_len" => TensorRef::from_array_view(chl.view())?,
            ])?;
            let e = out
                .get("encoded_output")
                .ok_or_else(|| NemotronError::OutputNotFound("encoded_output".into()))?
                .try_extract_array::<f32>()?;
            Ok(e.iter().fold(0.0f32, |m, &x| m.max(x.abs())))
        })();
        match run {
            Ok(max_abs) => {
                log::info!("Nemotron encoder self-test: max|out|={max_abs:.3} (collapsed if <1)");
                max_abs > 1.0
            }
            Err(e) => {
                log::warn!("Nemotron encoder self-test run failed: {e}");
                false
            }
        }
    }

    fn build_session(path: &Path, directml: bool) -> Result<Session, NemotronError> {
        let mut providers = Vec::new();
        #[cfg(feature = "directml")]
        if directml {
            providers.push(DirectMLExecutionProvider::default().build());
        }
        providers.push(CPUExecutionProvider::default().build());

        // DirectML: disable graph optimizations (suspected fusion bug) + use the
        // EP-required sequential execution / no memory pattern. CPU: full opt.
        let opt = if directml {
            GraphOptimizationLevel::Disable
        } else {
            GraphOptimizationLevel::Level3
        };
        let mut builder = Session::builder()?
            .with_optimization_level(opt)?
            .with_execution_providers(providers)?;
        builder = if directml {
            builder
                .with_parallel_execution(false)?
                .with_memory_pattern(false)?
        } else {
            builder.with_parallel_execution(true)?
        };
        Ok(builder.commit_from_file(path)?)
    }

    /// vocab.json is a flat `{ "id": "token" }` object.
    fn load_vocab<P: AsRef<Path>>(dir: P) -> Result<Vec<String>, NemotronError> {
        let text = fs::read_to_string(dir.as_ref().join("vocab.json"))?;
        let map: HashMap<String, String> = serde_json::from_str(&text)?;
        let max_id = map.keys().filter_map(|k| k.parse::<usize>().ok()).max().unwrap_or(0);
        let mut vocab = vec![String::new(); max_id + 1];
        for (k, v) in map {
            if let Ok(id) = k.parse::<usize>() {
                if id < vocab.len() {
                    vocab[id] = v;
                }
            }
        }
        Ok(vocab)
    }

    /// languages.json: `{ "promptDictionary": { "de-DE": 9, "de": 9, ... } }`.
    fn load_lang_slots<P: AsRef<Path>>(dir: P) -> Result<HashMap<String, i64>, NemotronError> {
        let text = fs::read_to_string(dir.as_ref().join("languages.json"))?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let mut slots = HashMap::new();
        if let Some(obj) = v.get("promptDictionary").and_then(|d| d.as_object()) {
            for (k, val) in obj {
                if let Some(n) = val.as_i64() {
                    slots.insert(k.to_ascii_lowercase(), n);
                }
            }
        }
        Ok(slots)
    }

    /// Map a language code (e.g. "de", "en", "de-DE") to its prompt slot.
    pub fn resolve_lang_slot(&self, code: Option<&str>) -> i64 {
        let code = match code {
            Some(c) if !c.is_empty() && c != "auto" => c.to_ascii_lowercase(),
            _ => return DEFAULT_LANG_SLOT,
        };
        self.lang_slots
            .get(&code)
            .or_else(|| self.lang_slots.get(code.split('-').next().unwrap_or(&code)))
            .copied()
            .unwrap_or(DEFAULT_LANG_SLOT)
    }

    /// Transcribe a mono 16 kHz speech segment in the given language slot.
    pub fn transcribe_samples(
        &mut self,
        samples: Vec<f32>,
        lang_slot: i64,
    ) -> Result<String, NemotronError> {
        if samples.len() < HOP {
            return Ok(String::new());
        }

        // Compute ONE continuous log-mel over the whole (zero-padded to a whole
        // number of 320 ms windows) segment, then slice contiguous 32-frame
        // windows out of it — matching soniqo's push_chunk/end_stream. (Computing
        // mel per-window independently adds reflect-padding artifacts at every
        // boundary and leaves the model under-confident → mostly blank.)
        let total_windows = samples.len().div_ceil(WIN_SAMPLES);
        let mut padded = samples;
        padded.resize(total_windows * WIN_SAMPLES, 0.0);
        let mel = self.mel.compute(&padded, LOG_FLOOR); // [128][produced]
        let produced = mel.first().map(|r| r.len()).unwrap_or(0);

        // Per-segment streaming state (zeroed; predictor primed with blank).
        let mut pre_cache = ArrayD::<f32>::zeros(IxDyn(&[1, MEL_BINS, PRE_CACHE_SIZE]));
        let mut clc =
            ArrayD::<f32>::zeros(IxDyn(&[ENCODER_LAYERS, 1, ATTN_LEFT_CONTEXT, ENCODER_HIDDEN]));
        let mut clt =
            ArrayD::<f32>::zeros(IxDyn(&[ENCODER_LAYERS, 1, ENCODER_HIDDEN, CONV_CACHE_SIZE]));
        let mut ch_len: i32 = 0;
        let mut dec_h = ArrayD::<f32>::zeros(IxDyn(&[DECODER_LAYERS, 1, DECODER_HIDDEN]));
        let mut dec_c = ArrayD::<f32>::zeros(IxDyn(&[DECODER_LAYERS, 1, DECODER_HIDDEN]));
        let mut dec_hidden = ArrayD::<f32>::zeros(IxDyn(&[1, 1, DECODER_HIDDEN]));
        self.decoder_step(BLANK_ID as i64, &mut dec_h, &mut dec_c, &mut dec_hidden)?;

        // One-hot language prompt mask.
        let mut mask = Array2::<f32>::zeros((1, NUM_PROMPTS));
        if (lang_slot as usize) < NUM_PROMPTS {
            mask[[0, lang_slot as usize]] = 1.0;
        }
        let lang_mask = mask.into_dyn();

        let mut text = String::new();
        for k in 0..total_windows {
            text.push_str(&self.run_window(
                &mel,
                k * CHUNK_MEL_FRAMES,
                produced,
                &lang_mask,
                &mut pre_cache,
                &mut clc,
                &mut clt,
                &mut ch_len,
                &mut dec_h,
                &mut dec_c,
                &mut dec_hidden,
            )?);
        }
        Ok(text)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_window(
        &mut self,
        mel: &[Vec<f32>],
        f0: usize,
        produced: usize,
        lang_mask: &ArrayD<f32>,
        pre_cache: &mut ArrayD<f32>,
        clc: &mut ArrayD<f32>,
        clt: &mut ArrayD<f32>,
        ch_len: &mut i32,
        dec_h: &mut ArrayD<f32>,
        dec_c: &mut ArrayD<f32>,
        dec_hidden: &mut ArrayD<f32>,
    ) -> Result<String, NemotronError> {
        // Slice the contiguous 32-frame window [f0 .. f0+32] out of the
        // segment's continuous mel (zero-pad any frames past the end).
        let mut audio = Array::zeros((1, MEL_BINS, CHUNK_MEL_FRAMES));
        for b in 0..MEL_BINS {
            for i in 0..CHUNK_MEL_FRAMES {
                let f = f0 + i;
                audio[[0, b, i]] = if f < produced { mel[b][f] } else { 0.0 };
            }
        }
        let audio = audio.into_dyn();
        let audio_length = Array1::<i32>::from_vec(vec![CHUNK_MEL_FRAMES as i32]);
        let chl = Array1::<i32>::from_vec(vec![*ch_len]);

        let outputs = self.encoder.run(inputs![
            "audio_signal" => TensorRef::from_array_view(audio.view())?,
            "audio_length" => TensorRef::from_array_view(audio_length.view())?,
            "language_mask" => TensorRef::from_array_view(lang_mask.view())?,
            "pre_cache" => TensorRef::from_array_view(pre_cache.view())?,
            "cache_last_channel" => TensorRef::from_array_view(clc.view())?,
            "cache_last_time" => TensorRef::from_array_view(clt.view())?,
            "cache_last_channel_len" => TensorRef::from_array_view(chl.view())?,
        ])?;

        // Own the encoder output + roll caches, then drop `outputs` so the
        // borrow on `self.encoder` is released before the decode loop (which
        // needs `&mut self` for the joint/decoder sessions).
        let enc = outputs
            .get("encoded_output")
            .ok_or_else(|| NemotronError::OutputNotFound("encoded_output".into()))?
            .try_extract_array::<f32>()?
            .into_dimensionality::<ndarray::Ix3>()?
            .to_owned(); // [1, T, 1024]
        let t_out = enc.shape()[1];

        *pre_cache = outputs
            .get("new_pre_cache")
            .ok_or_else(|| NemotronError::OutputNotFound("new_pre_cache".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        *clc = outputs
            .get("new_cache_last_channel")
            .ok_or_else(|| NemotronError::OutputNotFound("new_cache_last_channel".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        *clt = outputs
            .get("new_cache_last_time")
            .ok_or_else(|| NemotronError::OutputNotFound("new_cache_last_time".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        if let Some(v) = outputs.get("new_cache_last_channel_len") {
            // Scalar; int32 on this export.
            if let Ok(a) = v.try_extract_array::<i32>() {
                if let Some(n) = a.iter().next() {
                    *ch_len = *n;
                }
            }
        }
        drop(outputs);

        // Greedy RNN-T over the committed encoder frames.
        let mut emitted = String::new();
        for frame in 0..t_out {
            let mut enc_vec = Vec::with_capacity(ENCODER_HIDDEN);
            for k in 0..ENCODER_HIDDEN {
                enc_vec.push(enc[[0, frame, k]]);
            }
            let enc_frame = Array::from_shape_vec((1, 1, ENCODER_HIDDEN), enc_vec)?.into_dyn();
            for _ in 0..MAX_SYMBOLS {
                let logits = self.joint_step(&enc_frame, dec_hidden)?;
                let best = argmax(&logits);
                if best == BLANK_ID {
                    break;
                }
                emitted.push_str(&self.token_to_text(best));
                self.decoder_step(best as i64, dec_h, dec_c, dec_hidden)?;
            }
        }
        Ok(emitted)
    }

    fn decoder_step(
        &mut self,
        token: i64,
        dec_h: &mut ArrayD<f32>,
        dec_c: &mut ArrayD<f32>,
        dec_hidden: &mut ArrayD<f32>,
    ) -> Result<(), NemotronError> {
        let tok = Array2::<i64>::from_shape_vec((1, 1), vec![token])?.into_dyn();
        let outputs = self.decoder.run(inputs![
            "token" => TensorRef::from_array_view(tok.view())?,
            "h" => TensorRef::from_array_view(dec_h.view())?,
            "c" => TensorRef::from_array_view(dec_c.view())?,
        ])?;
        *dec_hidden = outputs
            .get("decoder_output")
            .ok_or_else(|| NemotronError::OutputNotFound("decoder_output".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        *dec_h = outputs
            .get("h_out")
            .ok_or_else(|| NemotronError::OutputNotFound("h_out".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        *dec_c = outputs
            .get("c_out")
            .ok_or_else(|| NemotronError::OutputNotFound("c_out".into()))?
            .try_extract_array::<f32>()?
            .to_owned();
        Ok(())
    }

    fn joint_step(
        &mut self,
        enc_frame: &ArrayD<f32>,
        dec_hidden: &ArrayD<f32>,
    ) -> Result<Vec<f32>, NemotronError> {
        let outputs = self.joint.run(inputs![
            "encoder_output" => TensorRef::from_array_view(enc_frame.view())?,
            "decoder_output" => TensorRef::from_array_view(dec_hidden.view())?,
        ])?;
        let logits = outputs
            .get("logits")
            .ok_or_else(|| NemotronError::OutputNotFound("logits".into()))?
            .try_extract_array::<f32>()?;
        Ok(logits.iter().copied().collect())
    }

    fn token_to_text(&self, id: i32) -> String {
        let piece = match self.vocab.get(id as usize) {
            Some(p) => p.as_str(),
            None => return String::new(),
        };
        // Strip the SentencePiece word-boundary marker.
        let (lead, body) = match piece.strip_prefix('\u{2581}') {
            Some(rest) => (" ", rest),
            None => ("", piece),
        };
        // Drop special / language-prompt tokens like <en-US>, <unk>, <bg-BG>.
        if body.starts_with('<') && body.ends_with('>') {
            return String::new();
        }
        format!("{lead}{body}")
    }
}

fn argmax(logits: &[f32]) -> i32 {
    logits
        .iter()
        .take(N_LOGITS)
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i as i32)
        .unwrap_or(BLANK_ID)
}
