// nemotron_engine/
//
// Nemotron 3.5 ASR streaming 0.6B (ONNX INT4) — a third local transcription
// engine alongside Whisper and Parakeet. See NEMOTRON_IMPLEMENTATION_PLAN.md
// for the full design and the resolved tensor I/O contract.
//
// Scaffolding in progress. `features.rs` (log-mel preprocessing) is the first
// landed piece; `model.rs` (streaming RNN-T inference), `nemotron_engine.rs`
// (catalog/download/load), `commands.rs`, and the provider wrapper follow.

pub mod features;
pub mod model;
