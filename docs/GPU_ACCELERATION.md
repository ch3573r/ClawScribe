# GPU Acceleration

ClawScribe has two separate acceleration families:

- Whisper acceleration through `whisper-rs` / whisper.cpp feature backends.
- ONNX acceleration through ONNX Runtime execution providers, primarily
  DirectML on Windows for Parakeet and Nemotron.

The app should always remain correct on CPU-capable models. GPU paths are
performance paths and must be validated per engine/model variant.

## Windows DirectML

DirectML is the main Windows GPU path for ONNX transcription engines.

Current behavior:

- Parakeet int8 models are the default fast path and can use DirectML in GPU
  builds.
- Parakeet SmoothQuant int8 is available as an experimental accuracy/speed
  tradeoff.
- Nemotron fp16 and int8 are beta paths. They use DirectML probes/self-tests
  before trusting GPU output.
- Nemotron fp16 has a CPU-capable fallback. Nemotron int8 is intended for
  DirectML-capable GPU builds because the exported encoder uses int8 ops that
  may not have a usable CPU kernel in the bundled ONNX Runtime path.

Nemotron DirectML loading deliberately tests graph optimization levels. On some
hardware, optimized DirectML graphs can produce plausible but wrong encoder
activations. The loader should accept DirectML only after output checks pass and
should fall back where the model variant supports it.

## Whisper Backends

Whisper acceleration is controlled by Rust features exposed by the Tauri app:

```text
cuda
vulkan
openblas
metal
coreml
hipblas
```

Use the explicit frontend scripts when validating a backend:

```powershell
cd frontend
pnpm run tauri:dev:vulkan
pnpm run tauri:dev:cuda
pnpm run tauri:dev:openblas
```

The automatic scripts are useful for normal development, but explicit scripts
make performance regressions easier to compare.

## ONNX Model Variants

| Engine | Variants | Notes |
| --- | --- | --- |
| Parakeet | v3 int8, v3 SmoothQuant int8, v2 int8 | Fast default. Best current realtime path on DirectML-capable Windows systems. |
| Nemotron | fp16, int8 | Multilingual beta path. Requires per-hardware validation because DirectML graph behavior varies. |
| Whisper | local whisper.cpp models | Compatibility path with backend-specific acceleration features. |

Model downloads are managed in app settings. Large-file sizes are checked so CDN
errors, interrupted downloads, and Git LFS pointer stubs are rejected instead of
being treated as valid models.

## Performance Validation

Use the same audio clip when comparing engines. Capture at least:

- engine and model variant
- execution provider used by logs
- graph optimization level if logged
- total realtime factor
- encoder/decode timing when available
- transcript sanity, not just speed

Expected log lines to check:

```text
Parakeet ...
Nemotron encoder: ...
Nemotron decoder/joint: ...
Nemotron segment: ...
```

A GPU run that is faster but produces blank or obviously wrong text is a failed
run. Correctness beats provider selection.

## Environment Overrides

Developer-only overrides used for diagnosis include:

```text
NEMOTRON_FORCE_CPU=1
NEMOTRON_DECODE_EP=cpu|dml
NEMOTRON_DECODE_THREADS=auto|1|2|4|8
```

These are benchmarking levers, not user-facing configuration. Do not bake a
temporary override into release behavior without documenting why.

## Release Boundary

Windows release artifacts should state which feature set they were built with.
When publishing a GPU build, smoke test:

1. app startup
2. model discovery/download validation
3. one short live recording
4. one import transcription
5. summary generation
6. updater metadata

Keep GPU-specific regressions out of generic UI or docs commits unless the docs
change is intentionally documenting the new behavior.
