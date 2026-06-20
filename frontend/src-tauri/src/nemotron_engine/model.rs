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
    pub fn new<P: AsRef<Path>>(model_dir: P, cpu_capable: bool) -> Result<Self, NemotronError> {
        let dir = model_dir.as_ref();
        // The encoder is the heavy part. fp16 (cpu_capable) tries DirectML with a
        // CPU-vs-DML self-test and CPU fallback; int8 is DirectML(GPU)-only. The
        // decoder/joint are tiny and run on CPU for both (int8's are MatMul-based,
        // not ConvInteger, so the CPU EP handles them).
        let encoder = Self::load_encoder(dir, cpu_capable)?;
        let decoder = Self::init_session(dir, "decoder.onnx")?;
        let joint = Self::init_session(dir, "joint.onnx")?;
        let vocab = Self::load_vocab(dir)?;
        // Degrading to an empty slot map is survivable (every language falls back
        // to DEFAULT_LANG_SLOT), but it silently ignores the user's language
        // selection — so make the failure visible rather than swallowing it.
        let lang_slots = match Self::load_lang_slots(dir) {
            Ok(slots) if !slots.is_empty() => slots,
            Ok(_) => {
                log::warn!(
                    "Nemotron: languages.json had no usable promptDictionary entries; \
                     language selection will be ignored (all audio uses default slot {DEFAULT_LANG_SLOT})"
                );
                HashMap::new()
            }
            Err(e) => {
                log::warn!(
                    "Nemotron: failed to load languages.json ({e}); language selection \
                     will be ignored (all audio uses default slot {DEFAULT_LANG_SLOT})"
                );
                HashMap::new()
            }
        };
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
        Self::build_cpu_session(&path).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("ConvInteger") {
                NemotronError::CpuUnsupported
            } else {
                e
            }
        })
    }

    /// Load the encoder on DirectML (GPU), probing graph-optimization levels
    /// from fastest (Level3) to slowest (Disable) and using the FIRST one whose
    /// output matches CPU (Cipher's self-test). Full optimization collapses this
    /// encoder on DML, but a lower level is usually both correct AND much faster
    /// than fully-disabled (fewer, fused kernels → real GPU utilization). Falls
    /// back to CPU if no level matches.
    fn load_encoder(dir: &Path, cpu_capable: bool) -> Result<Session, NemotronError> {
        // Benchmark lever (fp16 only): NEMOTRON_FORCE_CPU=1 skips the DirectML
        // probe so the same audio can be timed CPU-only vs GPU for an honest RTF
        // comparison. int8's ConvInteger encoder has no CPU kernel, so the lever
        // doesn't apply to it.
        if cpu_capable
            && std::env::var("NEMOTRON_FORCE_CPU").is_ok_and(|v| !v.is_empty() && v != "0")
        {
            log::info!("Nemotron encoder: NEMOTRON_FORCE_CPU set — forcing CPU execution");
            return Self::init_session(dir, "encoder.onnx");
        }
        #[cfg(feature = "directml")]
        {
            let path = dir.join("encoder.onnx");
            use GraphOptimizationLevel as G;
            // int8: the ConvInteger encoder can't build on the CPU EP, so there's
            // no CPU baseline to diff against and no CPU fallback. Probe DirectML
            // graph-opt levels and accept the first whose ramp-probe output isn't
            // collapsed — a correct encoder peaks ~8, the graph-opt fusion bug
            // collapses it to ~0.3. GPU-only: error out if every level collapses.
            if !cpu_capable {
                for opt in [G::Level3, G::Level2, G::Level1, G::Disable] {
                    let lvl = format!("{opt:?}");
                    match Self::build_dml_session(&path, opt) {
                        Ok(mut session) => match Self::encoder_probe_output(&mut session) {
                            Ok(probe) => {
                                let pmax = probe.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
                                if pmax > 2.0 {
                                    log::info!(
                                        "Nemotron encoder: DirectML (GPU) @ graph_opt={lvl}, probe |max|={pmax:.3} (int8, GPU-only)"
                                    );
                                    return Ok(session);
                                }
                                log::warn!(
                                    "Nemotron encoder: DirectML @ graph_opt={lvl} probe collapsed (|max|={pmax:.3}); trying a lower level"
                                );
                            }
                            Err(e) => log::warn!(
                                "Nemotron encoder: DirectML probe @ graph_opt={lvl} failed ({e})"
                            ),
                        },
                        Err(e) => log::warn!(
                            "Nemotron encoder: DirectML init @ graph_opt={lvl} failed ({e})"
                        ),
                    }
                }
                log::error!("Nemotron int8 encoder: no DirectML graph_opt level produced valid output (int8 is GPU-only, no CPU fallback)");
                return Err(NemotronError::CpuUnsupported);
            }

            // fp16: build a CPU baseline, then accept the first DirectML level
            // whose output matches it; fall back to CPU if none do.
            let mut cpu_session = Self::init_session(dir, "encoder.onnx")?;
            for opt in [G::Level3, G::Level2, G::Level1, G::Disable] {
                let lvl = format!("{opt:?}");
                match Self::build_dml_session(&path, opt) {
                    Ok(mut session) => {
                        if Self::encoder_self_test(&mut session, &mut cpu_session) {
                            // The self-test only proves the DML output is correct,
                            // not that the heavy work runs on the GPU: ORT silently
                            // places unsupported nodes on the CPU EP, so a "passing"
                            // DML session can still be mostly CPU. A correct session
                            // is not enough — require a measured speedup over the CPU
                            // baseline before trusting DirectML, otherwise the DML
                            // session only adds overhead over the CPU one we already
                            // have. Below threshold (or an inconclusive probe), keep
                            // probing lower graph-opt levels and fall back to CPU.
                            match Self::encoder_speed_ratio(&mut session, &mut cpu_session) {
                                Some(ratio) if ratio >= 1.15 => {
                                    log::info!(
                                        "Nemotron encoder: DirectML GPU-accelerated @ graph_opt={lvl} (self-test passed, ~{ratio:.2}x CPU)"
                                    );
                                    return Ok(session);
                                }
                                Some(ratio) => log::warn!(
                                    "Nemotron encoder: DirectML @ graph_opt={lvl} self-test passed but only ~{ratio:.2}x CPU — ORT likely placed the heavy ops on the CPU EP; not using DirectML at this level"
                                ),
                                None => log::warn!(
                                    "Nemotron encoder: DirectML @ graph_opt={lvl} self-test passed but the speed probe was inconclusive; can't confirm GPU acceleration, not using DirectML at this level"
                                ),
                            }
                        } else {
                            log::warn!(
                                "Nemotron encoder: DirectML @ graph_opt={lvl} mismatched CPU; trying a lower level"
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("Nemotron encoder: DirectML init @ graph_opt={lvl} failed ({e})")
                    }
                }
            }
            log::warn!("Nemotron encoder: no DirectML graph_opt level was both correct and measurably GPU-accelerated; using CPU");
            return Ok(cpu_session);
        }
        // Built without DirectML: fp16 runs on CPU; int8 cannot.
        #[cfg(not(feature = "directml"))]
        {
            if !cpu_capable {
                return Err(NemotronError::CpuUnsupported);
            }
            let session = Self::init_session(dir, "encoder.onnx")?;
            log::info!("Nemotron encoder: CPU");
            Ok(session)
        }
    }

    /// Feed a deterministic mel-shaped probe through CPU and DML and require the
    /// DML output to closely match CPU. An absolute activation threshold is too
    /// brittle: some synthetic probes produce small-but-valid CPU activations.
    #[cfg(feature = "directml")]
    fn encoder_self_test(candidate: &mut Session, baseline: &mut Session) -> bool {
        let base = match Self::encoder_probe_output(baseline) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Nemotron encoder CPU self-test run failed: {e}");
                return false;
            }
        };
        let cand = match Self::encoder_probe_output(candidate) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Nemotron encoder DirectML self-test run failed: {e}");
                return false;
            }
        };
        if base.len() != cand.len() || base.is_empty() {
            log::warn!(
                "Nemotron encoder self-test shape mismatch: CPU={} DML={}",
                base.len(),
                cand.len()
            );
            return false;
        }

        let mut max_abs_err = 0.0f32;
        let mut base_max = 0.0f32;
        let mut cand_max = 0.0f32;
        let mut dot = 0.0f64;
        let mut base_norm = 0.0f64;
        let mut cand_norm = 0.0f64;
        for (&a, &b) in base.iter().zip(cand.iter()) {
            max_abs_err = max_abs_err.max((a - b).abs());
            base_max = base_max.max(a.abs());
            cand_max = cand_max.max(b.abs());
            let af = a as f64;
            let bf = b as f64;
            dot += af * bf;
            base_norm += af * af;
            cand_norm += bf * bf;
        }
        let cosine = if base_norm > 0.0 && cand_norm > 0.0 {
            dot / (base_norm.sqrt() * cand_norm.sqrt())
        } else {
            0.0
        };
        log::info!(
            "Nemotron encoder self-test: CPU|max|={base_max:.3} DML|max|={cand_max:.3} max_abs_err={max_abs_err:.4} cosine={cosine:.5}"
        );
        max_abs_err <= 0.1 && cosine >= 0.995 && cand_max > 0.5
    }

    /// Wall-time ratio cpu/dml of repeated encoder probe runs. >1 means the DML
    /// session is genuinely faster than CPU (real GPU offload); ~1 (or <1) means
    /// ORT placed most ops on the CPU EP and the "GPU" session isn't accelerating.
    /// Returns None if either side errors. A few iterations is enough to clear
    /// one-shot init noise without slowing model load meaningfully.
    #[cfg(feature = "directml")]
    fn encoder_speed_ratio(dml: &mut Session, cpu: &mut Session) -> Option<f32> {
        use std::time::Instant;
        const ITERS: u32 = 3;
        // One warm-up each so first-run graph/kernel init isn't timed.
        Self::encoder_probe_output(dml).ok()?;
        Self::encoder_probe_output(cpu).ok()?;

        let cpu_start = Instant::now();
        for _ in 0..ITERS {
            Self::encoder_probe_output(cpu).ok()?;
        }
        let cpu_time = cpu_start.elapsed().as_secs_f32();

        let dml_start = Instant::now();
        for _ in 0..ITERS {
            Self::encoder_probe_output(dml).ok()?;
        }
        let dml_time = dml_start.elapsed().as_secs_f32();

        if dml_time > 0.0 {
            Some(cpu_time / dml_time)
        } else {
            None
        }
    }

    #[cfg(feature = "directml")]
    fn encoder_probe_output(enc: &mut Session) -> Result<Vec<f32>, NemotronError> {
        let mut audio = Array::zeros((1, MEL_BINS, CHUNK_MEL_FRAMES));
        for b in 0..MEL_BINS {
            for t in 0..CHUNK_MEL_FRAMES {
                audio[[0, b, t]] =
                    (b as f32 / (MEL_BINS - 1) as f32) * -14.0
                        + (t as f32 / (CHUNK_MEL_FRAMES - 1) as f32) * 2.0;
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
        Ok(e.iter().copied().collect())
    }

    /// CPU session: full graph optimization + parallel execution.
    fn build_cpu_session(path: &Path) -> Result<Session, NemotronError> {
        let builder = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_execution_providers([CPUExecutionProvider::default().build()])?
            .with_parallel_execution(true)?;
        Ok(builder.commit_from_file(path)?)
    }

    /// DirectML session at a given graph-optimization level. Full optimization
    /// (Level3) collapses this encoder on DML (a fusion/layout bug), but lower
    /// levels may be both correct AND faster than fully-disabled — load_encoder
    /// probes levels high→low and self-tests each. DirectML requires sequential
    /// execution + no memory pattern.
    #[cfg(feature = "directml")]
    fn build_dml_session(
        path: &Path,
        opt: GraphOptimizationLevel,
    ) -> Result<Session, NemotronError> {
        let providers = [
            DirectMLExecutionProvider::default().build(),
            CPUExecutionProvider::default().build(),
        ];
        let builder = Session::builder()?
            .with_optimization_level(opt)?
            .with_execution_providers(providers)?
            .with_parallel_execution(false)?
            .with_memory_pattern(false)?;
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

        let t0 = std::time::Instant::now();
        let mut enc_ms = 0.0f64;
        let mut dec_ms = 0.0f64;
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
                &mut enc_ms,
                &mut dec_ms,
            )?);
        }
        let secs = (padded.len() as f32 / 16_000.0).max(0.001);
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        log::info!(
            "Nemotron segment: {secs:.1}s audio, {windows} windows, {ms:.0}ms compute (encoder {enc_ms:.0}ms, decode {dec_ms:.0}ms), RTF {:.2} (lower=faster)",
            (ms / 1000.0) / secs,
            windows = total_windows
        );
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
        enc_ms: &mut f64,
        dec_ms: &mut f64,
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

        let t_enc = std::time::Instant::now();
        let outputs = self.encoder.run(inputs![
            "audio_signal" => TensorRef::from_array_view(audio.view())?,
            "audio_length" => TensorRef::from_array_view(audio_length.view())?,
            "language_mask" => TensorRef::from_array_view(lang_mask.view())?,
            "pre_cache" => TensorRef::from_array_view(pre_cache.view())?,
            "cache_last_channel" => TensorRef::from_array_view(clc.view())?,
            "cache_last_time" => TensorRef::from_array_view(clt.view())?,
            "cache_last_channel_len" => TensorRef::from_array_view(chl.view())?,
        ])?;
        *enc_ms += t_enc.elapsed().as_secs_f64() * 1000.0;

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
        let t_dec = std::time::Instant::now();
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
        *dec_ms += t_dec.elapsed().as_secs_f64() * 1000.0;
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
