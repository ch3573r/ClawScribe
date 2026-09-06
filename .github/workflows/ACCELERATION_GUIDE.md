# Windows Build Acceleration

The supported installer workflow is **ClawScribe Windows Release**, running on
the designated local Windows x64 machine. The former hosted cross-platform
installer workflows have been retired.

The normal feature is `windows-gpu`: Whisper uses Vulkan, while the supported
ONNX/sherpa transcription paths use DirectML. These are separate acceleration
backends. CPU, Vulkan-only, DirectML-only, CUDA, and OpenBLAS builds are explicit
alternatives with their own prerequisites.

The Vulkan SDK enables compilation. Installing the Vulkan runtime supplies the
loader but does not prove that a model used a GPU. Verify the built feature set,
active engine, model, language, and startup/inference logs on the target machine.
Use measured release-like transcription runs for performance claims.

See [Windows releases](../../docs/windows-release.md) for pinned toolchains,
local runner configuration, native tests, and real-device acceptance, and
[Building ClawScribe](../../docs/BUILDING.md) for development commands.
