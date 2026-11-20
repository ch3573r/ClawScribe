/// Summary module - handles all meeting summary generation functionality
///
/// This module contains:
/// - LLM client for communicating with various AI providers (OpenAI, Claude, Groq, Ollama, OpenRouter, CustomOpenAI)
/// - Processor for chunking transcripts and generating summaries
/// - Service layer for orchestrating summary generation
/// - Templates for structured meeting summary generation
/// - Tauri commands for frontend integration
use serde::{Deserialize, Serialize};

pub(crate) mod automatic;
mod request_config;

/// Custom OpenAI-compatible endpoint configuration
/// Stored as JSON in the database and used for connecting to any OpenAI-compatible API server
#[derive(Clone, Serialize, Deserialize)]
pub struct CustomOpenAIConfig {
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub destination_problem: Option<String>,
    #[serde(default)]
    pub allow_unencrypted: bool,
    /// Base URL of the OpenAI-compatible API endpoint (e.g., "http://localhost:8000/v1")
    pub endpoint: String,
    /// API key for authentication (optional if server doesn't require it)
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    /// Model identifier to use (e.g., "gpt-4", "llama-3-70b", "mistral-7b")
    pub model: String,
    /// Request timeout in seconds (optional)
    #[serde(rename = "timeoutSeconds")]
    pub timeout_seconds: Option<u64>,
    /// Optional OpenAI organization header
    pub organization: Option<String>,
    /// Optional OpenAI project header
    pub project: Option<String>,
    /// Maximum tokens for completion (optional)
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<i32>,
    /// Actual context limit advertised by the configured model.
    #[serde(rename = "contextWindow")]
    pub context_window: Option<usize>,
    /// Temperature parameter (0.0-2.0, optional)
    pub temperature: Option<f32>,
    /// Top-P sampling parameter (0.0-1.0, optional)
    #[serde(rename = "topP")]
    pub top_p: Option<f32>,
}

crate::credentials::debug_without_secrets!(CustomOpenAIConfig {
    destination_problem, allow_unencrypted, endpoint, model, timeout_seconds, organization, project,
    max_tokens, context_window, temperature, top_p;
    secret api_key
});

mod chunking;
pub mod claude_code_provider;
pub mod codex_provider;
pub mod commands;
pub mod edit_exit;
mod http_retry;
pub(crate) mod language_detection;
pub mod llm_client;
pub(crate) mod metadata;
pub mod openai_provider;
pub mod processor;
pub mod service;
pub mod sources;
pub mod summary_engine;
pub mod template_commands;
pub mod templates;

// Re-export Tauri commands (with their generated __cmd__ variants)
pub use commands::{
    __cmd__api_cancel_summary, __cmd__api_detect_transcript_summary_language,
    __cmd__api_get_meeting_detected_summary_language, __cmd__api_get_meeting_summary_language,
    __cmd__api_get_summary, __cmd__api_process_transcript,
    __cmd__api_save_meeting_detected_summary_language, __cmd__api_save_meeting_summary,
    __cmd__api_save_meeting_summary_language, __tauri_command_name_api_cancel_summary,
    __tauri_command_name_api_detect_transcript_summary_language,
    __tauri_command_name_api_get_meeting_detected_summary_language,
    __tauri_command_name_api_get_meeting_summary_language, __tauri_command_name_api_get_summary,
    __tauri_command_name_api_process_transcript,
    __tauri_command_name_api_save_meeting_detected_summary_language,
    __tauri_command_name_api_save_meeting_summary,
    __tauri_command_name_api_save_meeting_summary_language, api_cancel_summary,
    api_detect_transcript_summary_language, api_get_meeting_detected_summary_language,
    api_get_meeting_summary_language, api_get_summary, api_process_transcript,
    api_save_meeting_detected_summary_language, api_save_meeting_summary,
    api_save_meeting_summary_language,
};

pub use codex_provider::{
    __cmd__codex_check_installation, __cmd__codex_find_automatically, __cmd__codex_get_config,
    __cmd__codex_login_browser, __cmd__codex_login_device, __cmd__codex_logout,
    __cmd__codex_prepare_install_command, __cmd__codex_save_config, __cmd__codex_test_processing,
    __tauri_command_name_codex_check_installation, __tauri_command_name_codex_find_automatically,
    __tauri_command_name_codex_get_config, __tauri_command_name_codex_login_browser,
    __tauri_command_name_codex_login_device, __tauri_command_name_codex_logout,
    __tauri_command_name_codex_prepare_install_command, __tauri_command_name_codex_save_config,
    __tauri_command_name_codex_test_processing, codex_check_installation, codex_find_automatically,
    codex_get_config, codex_login_browser, codex_login_device, codex_logout,
    codex_prepare_install_command, codex_save_config, codex_test_processing,
};

// Re-export template commands
pub use template_commands::{
    __cmd__api_list_templates, __tauri_command_name_api_list_templates, api_list_templates,
};

// Re-export commonly used items
pub use llm_client::LLMProvider;
pub use processor::{
    chunk_text, clean_llm_markdown_output, extract_meeting_name_from_markdown, rough_token_count,
};
pub use service::SummaryService;

pub(crate) mod context_budget;

