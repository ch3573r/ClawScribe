# ClawScribe Notices

ClawScribe is an OpenClaw productization fork based on Meetily Community Edition `0.4.0`.

Upstream Meetily copyright:

- Copyright (c) 2024 Zackriya Solutions
- Meetily contributors

Upstream Meetily is distributed under the MIT License. The upstream license text is preserved in [LICENSE.md](LICENSE.md).

ClawScribe-specific changes are copyright OpenClaw contributors and are distributed under the same MIT License unless a file states otherwise.

Names and attribution:

- "Meetily" identifies the upstream project and some compatibility formats, paths, and migration flows.
- "ClawScribe" identifies this fork/product.
- This fork is not presented as an official Meetily release.

Additional upstream acknowledgments retained from the original project:

- Code or implementation ideas from [Whisper.cpp](https://github.com/ggerganov/whisper.cpp)
- Code or implementation ideas from [Screenpipe](https://github.com/mediar-ai/screenpipe)
- Code or implementation ideas from [transcribe-rs](https://crates.io/crates/transcribe-rs)
- NVIDIA for the Parakeet model
- [istupakov](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) for the ONNX conversion of the Parakeet model
- Moondream, for the Parakeet TDT 0.6B v3 fine-tune behind the Parakeet Ultra option (CC-BY-4.0), and [Masterx](https://huggingface.co/Masterx/parakeet-tdt-0.6b-ultra-onnx) for its ONNX export
- NVIDIA for the Nemotron 3.5 ASR Streaming model
- [soniqo](https://huggingface.co/soniqo) for the ONNX (INT8/INT4) conversions of the Nemotron model
- [Silero VAD](https://github.com/snakers4/silero-vad) (MIT) for voice-activity detection
- [ONNX Runtime](https://github.com/microsoft/onnxruntime) (MIT), used via the [`ort`](https://crates.io/crates/ort) crate, for Parakeet/Nemotron inference
- [OpenAI Whisper](https://github.com/openai/whisper) (MIT) models, run via [`whisper-rs`](https://crates.io/crates/whisper-rs)

Windows installers also include Microsoft Visual C++ x64 runtime and OpenMP
DLLs, copyright Microsoft Corporation, distributed app-local under the
[Visual Studio terms for distributable code](https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution).

NVIDIA Parakeet and Nemotron models are governed by NVIDIA's model license terms;
ClawScribe downloads them at runtime from their published sources rather than
bundling them in the installer.
