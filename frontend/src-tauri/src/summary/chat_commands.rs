// Transcript-grounded "chat with your meeting".
//
// Reuses the existing multi-provider `generate_summary` chokepoint (OpenAI /
// Claude / Groq / Ollama / OpenRouter / OpenAI-compatible / OpenClaw / Codex /
// built-in) rather than adding a parallel message-array API: each turn collapses
// to one system+user call whose user prompt carries the labeled transcript, the
// prior conversation, and the new question. History is persisted in SQLite.

use crate::database::models::AiChatMessage;
use crate::database::repositories::ai_chat::AiChatRepository;
use crate::database::repositories::setting::SettingsRepository;
use crate::state::AppState;
use crate::summary::llm_client::{generate_summary, LLMProvider};
use tauri::{AppHandle, Manager, Runtime};

/// Keep the transcript context bounded so very long meetings don't blow the
/// model's context window. Head + tail are kept (intro + most recent), which is
/// where Q&A grounding usually lives; the middle is elided.
const MAX_TRANSCRIPT_CHARS: usize = 48_000;
/// Most recent turns to replay as conversation context.
const MAX_HISTORY_TURNS: usize = 20;

fn build_transcript_context(transcripts: &[crate::database::models::Transcript]) -> String {
    let mut full = String::new();
    for t in transcripts {
        let text = t.transcript.trim();
        if text.is_empty() {
            continue;
        }
        let who = t.speaker.as_deref().unwrap_or("Speaker");
        full.push_str(&format!("[{who}] {text}\n"));
    }
    if full.len() <= MAX_TRANSCRIPT_CHARS {
        return full;
    }
    // Trim the middle: keep ~two-thirds head, one-third tail on a char boundary.
    let head = MAX_TRANSCRIPT_CHARS * 2 / 3;
    let tail = MAX_TRANSCRIPT_CHARS - head;
    let head_end = full.char_indices().nth(head).map(|(i, _)| i).unwrap_or(full.len());
    let tail_start = full
        .char_indices()
        .nth(full.chars().count().saturating_sub(tail))
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!(
        "{}\n…(transcript trimmed for length)…\n{}",
        &full[..head_end],
        &full[tail_start..]
    )
}

/// Resolve the configured provider's key/endpoint and run one system+user turn.
/// This is the same resolution `polish_planner_tasks` uses, kept local so the
/// chat path stays self-contained.
async fn run_turn<R: Runtime>(
    app: &AppHandle<R>,
    state: &tauri::State<'_, AppState>,
    model: &str,
    model_name: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let provider = LLMProvider::from_str(model)?;

    if matches!(provider, LLMProvider::Codex) {
        let codex = crate::summary::codex_provider::provider_from_app(app)
            .map_err(|e| format!("Codex app-server unavailable: {e}"))?;
        return codex.run_text_prompt(&format!("{system}\n\n{user}")).await;
    }

    let pool = state.db_manager.pool().clone();
    let mut api_key = String::new();
    let mut ollama_endpoint: Option<String> = None;
    let mut custom_openai_endpoint: Option<String> = None;
    let mut max_tokens: Option<u32> = None;
    let mut temperature: Option<f32> = None;
    let mut top_p: Option<f32> = None;

    match provider {
        LLMProvider::Ollama | LLMProvider::BuiltInAI => {}
        LLMProvider::CustomOpenAI => {
            let cfg = SettingsRepository::get_custom_openai_config(&pool)
                .await
                .map_err(|e| format!("Failed to read OpenAI-compatible config: {e}"))?
                .ok_or("No OpenAI-compatible configuration found")?;
            custom_openai_endpoint = Some(cfg.endpoint);
            api_key = cfg.api_key.unwrap_or_default();
            max_tokens = cfg.max_tokens.map(|t| t as u32);
            temperature = cfg.temperature;
            top_p = cfg.top_p;
        }
        LLMProvider::OpenClaw => {
            let cfg = crate::openclaw::load_config(app)
                .map_err(|e| format!("Failed to load OpenClaw config: {e}"))?;
            if !cfg.enabled || cfg.bearer_token.trim().is_empty() {
                return Err("OpenClaw handoff is disabled or missing a bearer token.".to_string());
            }
            custom_openai_endpoint = Some(cfg.model_endpoint);
            api_key = cfg.bearer_token;
        }
        _ => {
            api_key = SettingsRepository::get_api_key(&pool, model)
                .await
                .map_err(|e| format!("Failed to read API key: {e}"))?
                .filter(|k| !k.is_empty())
                .ok_or_else(|| format!("API key not found for {model}"))?;
        }
    }

    if provider == LLMProvider::Ollama {
        ollama_endpoint = SettingsRepository::get_model_config(&pool)
            .await
            .ok()
            .flatten()
            .and_then(|c| c.ollama_endpoint);
    }

    let app_data_dir = app.path().app_data_dir().ok();
    let client = reqwest::Client::new();
    generate_summary(
        &client,
        &provider,
        model_name,
        &api_key,
        system,
        user,
        ollama_endpoint.as_deref(),
        custom_openai_endpoint.as_deref(),
        max_tokens,
        temperature,
        top_p,
        app_data_dir.as_ref(),
        None,
    )
    .await
}

/// Full chat history for a meeting, oldest first.
#[tauri::command]
pub async fn api_chat_history(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<AiChatMessage>, String> {
    let pool = state.db_manager.pool().clone();
    AiChatRepository::list(&pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {e}"))
}

/// Erase a meeting's chat history.
#[tauri::command]
pub async fn api_chat_clear(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<u64, String> {
    let pool = state.db_manager.pool().clone();
    AiChatRepository::clear(&pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to clear chat history: {e}"))
}

/// Send a question about a meeting and get the assistant's reply, grounded in
/// the meeting transcript and the prior conversation. Persists both turns.
#[tauri::command]
pub async fn api_chat_send<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    model: String,
    model_name: String,
    question: String,
) -> Result<AiChatMessage, String> {
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err("Question is empty".to_string());
    }

    let pool = state.db_manager.pool().clone();

    let transcripts = AiChatRepository::transcripts(&pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to load transcript: {e}"))?;
    if transcripts.is_empty() {
        return Err("This meeting has no transcript to chat about yet.".to_string());
    }
    let transcript_context = build_transcript_context(&transcripts);

    let history = AiChatRepository::list(&pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {e}"))?;

    let system = "You are a helpful assistant answering questions about a single meeting. \
Base your answers ONLY on the meeting transcript provided. Speaker labels are \"Me\" \
(the user's microphone) and \"Participants\" (everyone else on the call). If the transcript \
does not contain the answer, say so plainly instead of guessing. Be concise; use Markdown \
when it helps (lists, bold). Do not invent attendees, decisions, or action items.";

    let mut user = String::with_capacity(transcript_context.len() + 2048);
    user.push_str("<transcript>\n");
    user.push_str(&transcript_context);
    user.push_str("</transcript>\n\n");

    let recent: &[AiChatMessage] = if history.len() > MAX_HISTORY_TURNS {
        &history[history.len() - MAX_HISTORY_TURNS..]
    } else {
        &history
    };
    if !recent.is_empty() {
        user.push_str("Conversation so far:\n");
        for m in recent {
            let who = if m.role == "assistant" { "Assistant" } else { "User" };
            user.push_str(&format!("{who}: {}\n", m.content.trim()));
        }
        user.push('\n');
    }
    user.push_str("User: ");
    user.push_str(&question);
    user.push_str("\nAssistant:");

    // Persist the user turn first so a failed reply still leaves the question.
    AiChatRepository::insert(&pool, &meeting_id, "user", &question)
        .await
        .map_err(|e| format!("Failed to save question: {e}"))?;

    let answer = run_turn(&app, &state, &model, &model_name, system, &user)
        .await?
        .trim()
        .to_string();
    if answer.is_empty() {
        return Err("The model returned an empty response.".to_string());
    }

    AiChatRepository::insert(&pool, &meeting_id, "assistant", &answer)
        .await
        .map_err(|e| format!("Failed to save reply: {e}"))
}
