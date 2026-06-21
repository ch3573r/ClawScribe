// nemotron_engine/
//
// Nemotron 3.5 ASR streaming 0.6B ONNX integration. It provides the local
// multilingual transcription path alongside Whisper and Parakeet. Runtime
// behavior, model variants, DirectML validation, and fallback notes are covered
// in docs/GPU_ACCELERATION.md.

pub mod commands;
pub mod features;
pub mod model;
pub mod nemotron_engine;
