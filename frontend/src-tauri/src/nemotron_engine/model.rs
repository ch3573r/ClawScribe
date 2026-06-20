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
        // Variant-aware decoder/joint CPU threading. The joint is a large
        // 640x13088 matmul called hundreds of times per segment: int8 weights run
        // fast single-threaded, but fp16 weights upconvert to fp32 and a single
        // thread serializes the matmul. So int8 keeps the lean 1-thread session;
        // fp16 uses ORT's default (auto) intra-op threads. Override either with
        // NEMOTRON_DECODE_THREADS=auto|1|2|4|8.
        let default_decode_threads = if cpu_capable { None } else { Some(1) };
        let decode_threads: Option<usize> =
            match std::env::var("NEMOTRON_DECODE_THREADS").ok().as_deref() {
                Some("auto") => None,
                Some(n) => n
                    .parse::<usize>()
                    .ok()
                    .filter(|&t| t >= 1)
                    .or(default_decode_threads),
                None => default_decode_threads,
            };
        let decode_mode = match decode_threads {
            None => "auto threads".to_string(),
            Some(1) => "1 thread".to_string(),
            Some(n) => format!("{n} threads"),
        };
        log::info!(
            "Nemotron decoder/joint: CPU {decode_mode} ({})",
            if cpu_capable { "fp16" } else { "int8" }
        );
        let decoder = Self::build_decode_session(&dir.join("decoder.onnx"), decode_threads)?;
        let joint = Self::build_decode_session(&dir.join("joint.onnx"), decode_threads)?;
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

    /// Load the encoder. fp16 builds a CPU baseline and uses DirectML only when a
    /// graph-opt level is both correct and measurably faster, falling back to
    /// CPU. int8 prefers DirectML (its `ConvInteger` encoder hits a graph-opt
    /// fusion bug at full optimization, so levels are probed high→low), but is no
    /// longer assumed GPU-only: when DirectML is unavailable or every level
    /// fails, it falls back to a CPU probe, reporting `CpuUnsupported` only when
    /// neither path can run the encoder.
    fn load_encoder(dir: &Path, cpu_capable: bool) -> Result<Session, NemotronError> {
        let path = dir.join("encoder.onnx");

        // Benchmark lever: NEMOTRON_FORCE_CPU=1 skips DirectML so the same audio
        // can be timed CPU-only vs GPU for an honest RTF comparison. It now
        // applies to int8 too (which has a CPU probe path); if int8 can't run on
        // the CPU EP the load fails — the intended "is CPU viable?" signal here.
        if std::env::var("NEMOTRON_FORCE_CPU").is_ok_and(|v| !v.is_empty() && v != "0") {
            log::info!("Nemotron encoder: NEMOTRON_FORCE_CPU set — forcing CPU execution");
            if cpu_capable {
                return Self::init_session(dir, "encoder.onnx");
            }
            return Self::try_cpu_encoder(&path).ok_or(NemotronError::CpuUnsupported);
        }

        #[cfg(feature = "directml")]
        {
            use GraphOptimizationLevel as G;
            // int8: prefer DirectML — its ConvInteger encoder hits a graph-opt
            // fusion bug at full optimization, so probe levels high→low and accept
            // the first whose ramp-probe output isn't collapsed (a correct encoder
            // peaks ~8; the bug collapses it to ~0.3). No longer GPU-only: if every
            // level fails, fall back to a CPU probe before giving up.
            if !cpu_capable {
                for opt in [G::Level3, G::Level2, G::Level1, G::Disable] {
                    let lvl = format!("{opt:?}");
                    match Self::build_dml_session(&path, opt) {
                        Ok(mut session) => match Self::encoder_probe_output(&mut session) {
                            Ok(probe) => {
                                let pmax = probe.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
                                if pmax > 2.0 {
                                    log::info!(
                                        "Nemotron encoder: DirectML (GPU) @ graph_opt={lvl}, probe |max|={pmax:.3} (int8)"
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
                log::warn!("Nemotron int8 encoder: no DirectML graph_opt level produced valid output; trying CPU");
                if let Some(session) = Self::try_cpu_encoder(&path) {
                    return Ok(session);
                }
                log::error!("Nemotron int8 encoder: neither DirectML nor the CPU EP could run this encoder");
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
        // Built without DirectML: fp16 runs on CPU; int8 falls back to a CPU probe
        // (some ORT builds can run its ConvInteger encoder on the CPU EP).
        #[cfg(not(feature = "directml"))]
        {
            if !cpu_capable {
                log::info!("Nemotron int8 encoder: DirectML not in this build — probing CPU");
                if let Some(session) = Self::try_cpu_encoder(&path) {
                    return Ok(session);
                }
                log::error!("Nemotron int8 encoder: CPU EP cannot run this encoder and DirectML isn't in this build");
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

    // Not DirectML-gated: also used by the CPU int8 encoder probe, which runs in
    // both DirectML and CPU-only builds.
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

    /// CPU session: full graph optimization + parallel execution. Used for the
    /// fp16 encoder, whose single per-window call is heavy enough to benefit.
    fn build_cpu_session(path: &Path) -> Result<Session, NemotronError> {
        let builder = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_execution_providers([CPUExecutionProvider::default().build()])?
            .with_parallel_execution(true)?;
        Ok(builder.commit_from_file(path)?)
    }

    /// CPU session for the decoder/joint graphs with a caller-chosen thread count.
    /// `None` leaves ORT's defaults (auto ≈ cores), which the large fp16 joint
    /// matmul needs (fp16 weights upconvert to fp32 on CPU, so a single thread
    /// serializes it); `Some(n)` pins intra/inter threads (int8 uses 1 — its
    /// weights run fast single-threaded via Zen VNNI/AVX). Sequential execution
    /// either way — the graphs are linear, so inter-op parallelism only adds
    /// overhead.
    fn build_decode_session(path: &Path, threads: Option<usize>) -> Result<Session, NemotronError> {
        let mut builder = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_execution_providers([CPUExecutionProvider::default().build()])?
            .with_parallel_execution(false)?;
        if let Some(t) = threads {
            builder = builder.with_intra_threads(t)?.with_inter_threads(t)?;
        }
        Ok(builder.commit_from_file(path)?)
    }

    /// Try to run the int8 encoder on the CPU EP. The int8 export uses
    /// `ConvInteger`, which historically had no CPU kernel in ORT — but that is a
    /// property of the specific ORT build, not a law, so probe it rather than
    /// assume. Builds a CPU session and runs the deterministic encoder probe;
    /// accepts only if the output has a valid (non-empty) shape and a sane peak
    /// activation (a correct encoder peaks well above 2.0; a collapsed/garbage
    /// run does not). Returns the validated CPU session, or `None` if the CPU EP
    /// can't build or run it — the raw ORT error (e.g. a `ConvInteger` "no
    /// kernel" message) is logged so the failure is diagnosable.
    fn try_cpu_encoder(path: &Path) -> Option<Session> {
        let mut session = match Self::build_cpu_session(path) {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "Nemotron int8 encoder: CPU session build failed: {e} \
                     (a ConvInteger 'no kernel' error here means this ORT build has no CPU int8 path)"
                );
                return None;
            }
        };
        match Self::encoder_probe_output(&mut session) {
            Ok(probe) => {
                let pmax = probe.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
                if !probe.is_empty() && pmax > 2.0 {
                    log::info!("Nemotron int8 encoder: CPU self-test passed (probe |max|={pmax:.3})");
                    Some(session)
                } else {
                    log::warn!(
                        "Nemotron int8 encoder: CPU probe output invalid (len={}, |max|={pmax:.3}); not using CPU",
                        probe.len()
                    );
                    None
                }
            }
            Err(e) => {
                log::warn!("Nemotron int8 encoder: CPU probe run failed: {e}");
                None
            }
        }
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
        let mut stats = SegmentStats::default();
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
                &mut stats,
            )?);
        }
        let secs = (padded.len() as f32 / 16_000.0).max(0.001);
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let avg_enc_ms = stats.enc_ms / (total_windows.max(1) as f64);
        let avg_dec_ms_per_frame = stats.dec_ms / (stats.encoded_frames.max(1) as f64);
        log::info!(
            "Nemotron segment: {secs:.1}s audio, {windows} windows, {frames} enc frames, \
             {ms:.0}ms compute (encoder {enc:.0}ms, decode {dec:.0}ms), \
             {jc} joint_calls, {dc} decoder_calls, {tok} tokens, \
             avg enc {avg_enc:.1}ms/window, avg decode {avg_dec:.2}ms/frame, RTF {rtf:.2} (lower=faster)",
            windows = total_windows,
            frames = stats.encoded_frames,
            enc = stats.enc_ms,
            dec = stats.dec_ms,
            jc = stats.joint_calls,
            dc = stats.decoder_calls,
            tok = stats.emitted_tokens,
            avg_enc = avg_enc_ms,
            avg_dec = avg_dec_ms_per_frame,
            rtf = (ms / 1000.0) / secs,
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
        stats: &mut SegmentStats,
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
        stats.enc_ms += t_enc.elapsed().as_secs_f64() * 1000.0;

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

        // Greedy RNN-T over the committed encoder frames. Reuse one [1,1,1024]
        // scratch tensor for the per-frame encoder slice instead of allocating a
        // fresh Vec+Array each frame, and argmax the joint logits in place.
        stats.encoded_frames += t_out as u64;
        let t_dec = std::time::Instant::now();
        let mut emitted = String::new();
        let mut enc_frame = Array::zeros((1, 1, ENCODER_HIDDEN)).into_dyn();
        for frame in 0..t_out {
            for k in 0..ENCODER_HIDDEN {
                enc_frame[[0, 0, k]] = enc[[0, frame, k]];
            }
            for _ in 0..MAX_SYMBOLS {
                stats.joint_calls += 1;
                let best = self.joint_argmax(&enc_frame, dec_hidden)?;
                if best == BLANK_ID {
                    break;
                }
                emitted.push_str(&self.token_to_text(best));
                stats.emitted_tokens += 1;
                stats.decoder_calls += 1;
                self.decoder_step(best as i64, dec_h, dec_c, dec_hidden)?;
            }
        }
        stats.dec_ms += t_dec.elapsed().as_secs_f64() * 1000.0;
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

    /// Run the joint network and return the argmax token id directly, without
    /// copying all `N_LOGITS` (~13k) logits into a Vec first. Tie-breaking matches
    /// the previous `argmax(&Vec)` (`max_by` keeps the last maximum).
    fn joint_argmax(
        &mut self,
        enc_frame: &ArrayD<f32>,
        dec_hidden: &ArrayD<f32>,
    ) -> Result<i32, NemotronError> {
        let outputs = self.joint.run(inputs![
            "encoder_output" => TensorRef::from_array_view(enc_frame.view())?,
            "decoder_output" => TensorRef::from_array_view(dec_hidden.view())?,
        ])?;
        let logits = outputs
            .get("logits")
            .ok_or_else(|| NemotronError::OutputNotFound("logits".into()))?
            .try_extract_array::<f32>()?;
        let best = logits
            .iter()
            .take(N_LOGITS)
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i as i32)
            .unwrap_or(BLANK_ID);
        Ok(best)
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

/// Per-segment decode metrics, accumulated across windows for one summary log
/// line so we can see whether encoder session time or the RNN-T decode/joint
/// loop dominates.
#[derive(Default)]
struct SegmentStats {
    enc_ms: f64,
    dec_ms: f64,
    encoded_frames: u64,
    joint_calls: u64,
    decoder_calls: u64,
    emitted_tokens: u64,
}
