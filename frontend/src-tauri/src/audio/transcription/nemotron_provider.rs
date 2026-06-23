// audio/transcription/nemotron_provider.rs
//
// Nemotron transcription provider (wraps NemotronEngine) — the trait-based
// integration path used by TranscriptionEngine::Provider.

use super::provider::{TranscriptResult, TranscriptionError, TranscriptionProvider};
use async_trait::async_trait;
use std::sync::Arc;

pub struct NemotronProvider {
    engine: Arc<crate::nemotron_engine::nemotron_engine::NemotronEngine>,
}

impl NemotronProvider {
    pub fn new(engine: Arc<crate::nemotron_engine::nemotron_engine::NemotronEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl TranscriptionProvider for NemotronProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError> {
        // Language selects the encoder prompt slot (one-hot language_mask) via
        // languages.json; "auto"/unknown falls back to English.
        match self.engine.transcribe_audio(audio, language).await {
            Ok(text) => Ok(TranscriptResult {
                text: text.trim().to_string(),
                confidence: None,  // RNN-T greedy decode provides no confidence
                is_partial: false, // offline-per-segment, no partials
                word_timestamps: None,
            }),
            Err(e) => Err(TranscriptionError::EngineFailed(e.to_string())),
        }
    }

    async fn is_model_loaded(&self) -> bool {
        self.engine.is_model_loaded().await
    }

    async fn get_current_model(&self) -> Option<String> {
        self.engine.get_current_model().await
    }

    fn provider_name(&self) -> &'static str {
        "Nemotron"
    }
}
