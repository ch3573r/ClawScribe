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

pub(crate) fn resolve_requested_language(
    language: Option<&str>,
    system_locale: Option<&str>,
) -> std::result::Result<String, TranscriptionError> {
    match language.map(str::trim).filter(|language| !language.is_empty()) {
        Some("auto-translate") => Err(TranscriptionError::UnsupportedLanguage(
            "auto-translate (Nemotron transcribes but does not translate)".to_string(),
        )),
        Some("auto") | None => system_locale
            .map(|locale| locale.replace('_', "-").to_ascii_lowercase())
            .filter(|locale| !locale.is_empty())
            .ok_or_else(|| {
                TranscriptionError::EngineFailed(
                    "Nemotron needs an explicit transcription language because the system locale could not be detected"
                        .to_string(),
                )
            }),
        Some(language) => Ok(language.replace('_', "-").to_ascii_lowercase()),
    }
}

#[async_trait]
impl TranscriptionProvider for NemotronProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError> {
        // Nemotron has prompt-conditioned language slots, not language
        // detection. Resolve Auto from the OS locale rather than silently using
        // the English slot for every multilingual meeting.
        let language =
            resolve_requested_language(language.as_deref(), sys_locale::get_locale().as_deref())?;
        match self.engine.transcribe_audio(audio, Some(language)).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_system_locale_instead_of_english_fallback() {
        assert_eq!(
            resolve_requested_language(Some("auto"), Some("de-DE")).unwrap(),
            "de-de"
        );
        assert_eq!(
            resolve_requested_language(None, Some("de_AT")).unwrap(),
            "de-at"
        );
    }

    #[test]
    fn translation_mode_is_rejected_for_nemotron() {
        assert!(matches!(
            resolve_requested_language(Some("auto-translate"), Some("de-DE")),
            Err(TranscriptionError::UnsupportedLanguage(_))
        ));
    }
}
