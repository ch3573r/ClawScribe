# Nemotron 3.5 ASR Streaming 0.6B — Implementation Map

Status: **wired end-to-end, pending on-device validation.** Nemotron is a third
local transcription engine alongside Whisper and Parakeet — selectable in the
picker + Settings, downloadable, labelled BETA, and routed through live
recording, import, and retranscription. The inference tensor shapes
(caches/LSTM-states/joint frames) and the `lang_id` table still need validation
on a real Windows box with the 790 MB INT4 model (can't run in CI).

Done: §3 module (`features.rs` mel + tests, `model.rs` streaming RNN-T,
`nemotron_engine.rs` catalog/download/load, `commands.rs`), §4 wiring (provider,
engine.rs dispatch, lib.rs registration, import.rs/retranscription.rs,
`useTranscriptionModels`, `NemotronModelManager`, `TranscriptSettings`,
`LanguageSelection`). Remaining: §6 open items (on-device shape/mel/lang_id
validation), and benchmark vs Parakeet int8 on the i5-1235u.

Model: `onnx-community/nemotron-3.5-asr-streaming-0.6b-onnx-int4`
(HF, INT4, ~790 MB). NeMo FastConformer **streaming RNN-T**, multilingual
(~40 locales incl. German), punctuation/caps.

## 1. What the model actually is (resolved from its configs)

Architecture is a cache-based streaming RNN-T (encoder / decoder / joint), NOT
the fused TDT layout Parakeet uses. Confirmed from `genai_config.json`:

```
vocab_size 13088   blank_id 13087   num_mels 128   sample_rate 16000
fft_size 512  hop_length 160  win_length 400  preemph 0.97  mag_power 2.0
subsampling_factor 8  chunk_samples 8960 (=560 ms)  max_symbols_per_step 10
pre_encode_cache_size 9  left_context 56  conv_context 8
```

Files in the repo:
- `encoder.onnx` + **`encoder.onnx.data`** (~690 MB, ONNX external data)
- `decoder.onnx` + `decoder.onnx.data` (~60 MB)   ← LSTM prediction net (2 layers, hidden 640)
- `joint.onnx` + `joint.onnx.data` (~38 MB)
- `silero_vad.onnx` (~2.2 MB) — bundled VAD, **we ignore it** (pipeline already runs Silero)
- `tokenizer.json`, `tokenizer_config.json` (T5Tokenizer), `vocab.txt` (sentencepiece, ▁)
- `model_config.json`, `audio_processor_config.json`, `genai_config.json`

### Tensor I/O contract (from genai_config.json)

Encoder (`encoder.onnx`):
- in:  `audio_signal` [B,128,T], `length` [B], `cache_last_channel`,
       `cache_last_time`, `cache_last_channel_len`, `lang_id`
- out: `outputs` [B,D,T'], `encoded_lengths`, `cache_last_channel_next`,
       `cache_last_time_next`, `cache_last_channel_len_next`

Decoder (`decoder.onnx`, LSTM):
- in:  `targets` [B,U], `h_in`, `c_in`
- out: `decoder_output`, `h_out`, `c_out`

Joint (`joint.onnx`):
- in:  `encoder_output`, `decoder_output`
- out: `joint_output` [.., 13088 logits]

Decode: standard RNN-T greedy, `blank_id 13087`, cap `max_symbols_per_step 10`.
(No TDT duration head — logits width == vocab_size, so no split like Parakeet.)

## 2. Key decisions (defaults chosen — change before deep impl if needed)

1. **Preprocessing → pure-Rust log-mel** (NOT reuse Parakeet's `nemo128.onnx`).
   Nemotron ships no preprocessor onnx, and its mel params include `preemph 0.97`
   + `dither 1e-5` which may differ from nemo128's. We implement log-mel exactly
   from `audio_processor_config.json` using `realfft` (already a dependency):
   preemphasis → frame (win 400, hop 160, center) → Hann → rfft(512) →
   power(mag^2) → 128 mel filterbank (fmin 0, fmax 8000) → log(x + 1e-10).
   Lives in `nemotron_engine/features.rs`, unit-tested against a known vector.

2. **Per-VAD-segment, cache-reset (offline-within-segment)**, not whole-meeting
   streaming. The pipeline already hands us VAD speech segments; we feed each
   segment through the encoder in 560 ms (8960-sample) chunks, threading the
   cache tensors **within** the segment and resetting (zero caches) **between**
   segments. True persistent streaming across the meeting is a later optimization.

3. **`lang_id`**: the public configs don't publish the language→id table (NeMo
   convention). v1 sends a single default id (English) and exposes no per-line
   language switch; resolve the full table by probing the encoder input / NeMo
   manifest before promoting out of BETA. German still works (it's in the
   transcription-ready tier and the multilingual encoder handles it).

4. **Tokenizer**: reuse Parakeet's `vocab.txt` loader + ▁→space + the
   `DECODE_SPACE_RE` detokenizer. `blank_id 13087`. No `tokenizers` crate needed.

5. **Integration as `TranscriptionEngine::Provider`** (trait-based, the
   "preferred for new code" path) — a `NemotronProvider` implementing
   `TranscriptionProvider`, not a new enum variant.

6. **External data**: download `encoder.onnx` AND `encoder.onnx.data` (etc.)
   into the same dir; `ort`/onnxruntime loads `.data` relative to the `.onnx`
   automatically.

## 3. Module layout (mirrors parakeet_engine/)

```
src/nemotron_engine/
  mod.rs              exports + ModelInfo/ModelStatus reuse
  features.rs         pure-Rust log-mel (NEW, the novel DSP core)
  model.rs            ort sessions (encoder/decoder/joint) + streaming RNN-T greedy
  nemotron_engine.rs  catalog/discover_models/download/load/unload (mirror Parakeet)
  commands.rs         nemotron_init / _get_available_models / _download_model /
                      _cancel_download / _validate_model_ready_with_config
src/audio/transcription/nemotron_provider.rs   TranscriptionProvider impl
```

Download catalog (single model for v1):
- name `nemotron-streaming-0.6b-int4`, ~790 MB, BETA
- base url `https://huggingface.co/onnx-community/nemotron-3.5-asr-streaming-0.6b-onnx-int4/resolve/main`
- files: encoder.onnx(+.data), decoder.onnx(+.data), joint.onnx(+.data),
  tokenizer.json, vocab.txt, model_config.json, audio_processor_config.json,
  genai_config.json   (skip silero_vad.onnx)

## 4. Wiring for parity (the non-inference glue)

- `audio/transcription/engine.rs`: add `"nemotron"` arms to
  `validate_transcription_model_ready` and `get_or_init_transcription_engine`
  (build `TranscriptionEngine::Provider(Arc<NemotronProvider>)`).
- `audio/transcription/worker.rs`: confidence threshold — RNN-T has no
  confidence, so treat Nemotron like Parakeet (accept all / 0.0). Provider
  currently maps to 0.3; add a Nemotron-aware case (or have the provider return
  `confidence: None` and key the threshold off that).
- `lib.rs`: register the five `nemotron_*` commands.
- `config.rs`: optional `DEFAULT_NEMOTRON_MODEL` const.
- Frontend:
  - `useTranscriptionModels.ts`: add `'nemotron'` to the provider union + a
    `nemotron_get_available_models` fetch block (display `🌊 Nemotron: …`).
  - Settings model-download UI: add a Nemotron section (download/progress/delete)
    mirroring the Parakeet section, with a BETA chip.
  - Transcript config save path: allow provider `"nemotron"`.
  - Parakeet-only UI guards (e.g. ImportAudioDialog language lock) generalize to
    "engine doesn't take a language hint" for Nemotron too.

## 5. Build / validation

- No new crates (`ort`, `ndarray`, `realfft`, `regex`, `once_cell`, `reqwest`
  all present). INT4 weights load via the same `ort 2.0.0-rc.10` path.
- Local Linux: `cargo check`, unit-test `features.rs` mel output.
- On-device (self-hosted Windows runner / target box): download, load, transcribe
  a known clip; verify text, measure RTF on the i5-1235u, then benchmark
  Nemotron-INT4 vs Parakeet-int8 (this is the comparison the roadmap calls for).
  DirectML EP can be layered on later (the `directml` feature already exists).

## 6. Open items before leaving BETA
- Resolve the `lang_id` table (probe encoder / NeMo manifest) for true multilingual selection.
- Validate mel features match NeMo (compare a frame against a reference).
- Decide persistent-streaming vs per-segment after measuring latency/quality.
