use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};

const DEFAULT_CODEX_MODEL: &str = "gpt-5.1-codex";
const DEFAULT_CODEX_TIMEOUT_SECONDS: u64 = 600;
const CODEX_APP_SERVER_MISSING: &str = "Codex app-server runtime is not installed for ClawScribe. Normal processing still works through OpenAI API key or OpenClaw.";
const CODEX_APP_SERVER_STAGED: &str = "Codex app-server integration is staged, but no bundled/pinned app-server runtime is installed in this build.";
const CODEX_WINDOWSAPPS_REJECTED: &str = "Windows Store Codex app executables under WindowsApps are not supported for ClawScribe automation. Codex app-server mode requires a bundled/pinned ClawScribe runtime or a controlled ClawScribe runtime installer.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexHomeMode {
    ClawscribeIsolated,
    ExistingUserCodexSession,
}

#[cfg(test)]
mod app_server_tests {
    use super::*;

    fn make_executable(path: &Path) {
        fs::write(path, "# app-server placeholder\n").unwrap();
    }

    #[test]
    fn bundled_app_server_runtime_is_discovered() {
        let temp = tempfile::tempdir().unwrap();
        let resource_dir = temp.path().join("resources");
        let exe = if cfg!(target_os = "windows") {
            "codex-app-server.exe"
        } else {
            "codex-app-server"
        };
        let runtime = resource_dir.join("codex-app-server").join(exe);
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        make_executable(&runtime);

        let discovered = discover_codex_app_server_from(None, Some(&resource_dir)).unwrap();
        assert_eq!(discovered, runtime);
    }

    #[test]
    fn path_discovery_is_not_used_for_app_server_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let configured = temp.path().join("codex.exe");
        make_executable(&configured);

        let err =
            discover_codex_app_server_from(Some(configured.to_str().unwrap()), None).unwrap_err();
        assert!(err.contains("Configured global Codex executable paths are no longer supported"));
    }

    #[test]
    fn windowsapps_codex_path_is_rejected() {
        let err = discover_codex_app_server_from(
            Some(r"C:\Users\user\AppData\Local\Microsoft\WindowsApps\codex.exe"),
            None,
        )
        .unwrap_err();
        assert!(err.contains("WindowsApps"));
        assert!(err.contains("not supported"));
    }

    #[test]
    fn missing_runtime_is_non_blocking_status() {
        let err = discover_codex_app_server_from(None, None).unwrap_err();
        assert!(err.contains("Normal processing still works through OpenAI API key or OpenClaw"));
    }

    #[test]
    fn install_repair_plan_does_not_suggest_global_cli() {
        let plan = codex_install_repair_plan();
        assert!(plan.requires_confirmation);
        assert!(plan.alternatives.is_empty());
        assert!(!plan.recommended.command.contains("npm install"));
        assert!(!plan.recommended.command.contains("codex.exe"));
        assert!(plan.message.contains("WindowsApps"));
    }

    #[test]
    fn existing_user_codex_session_is_normalized_out() {
        let normalized = normalize_codex_config(CodexProviderConfig {
            codex_home_mode: CodexHomeMode::ExistingUserCodexSession,
            use_existing_user_codex_session: true,
            codex_home_path: None,
            ..CodexProviderConfig::default()
        });

        assert_eq!(
            normalized.effective_home_mode(),
            CodexHomeMode::ClawscribeIsolated
        );
        assert!(!normalized.use_existing_user_codex_session);
        assert!(normalized.effective_codex_home().is_some());
    }

    #[test]
    fn app_server_handshake_and_login_requests_use_expected_methods() {
        let initialize = app_server_initialize_request(1);
        let initialized = app_server_initialized_notification();
        let account_read = app_server_account_read_request(2);
        let browser_login = app_server_login_start_request(3, "chatgpt");
        let device_login = app_server_login_start_request(4, "chatgptDeviceCode");
        let logout = app_server_logout_request(5);

        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialized["method"], "notifications/initialized");
        assert_eq!(account_read["method"], "account/read");
        assert_eq!(browser_login["method"], "account/login/start");
        assert_eq!(browser_login["params"]["type"], "chatgpt");
        assert_eq!(device_login["params"]["type"], "chatgptDeviceCode");
        assert_eq!(logout["method"], "account/logout");
    }

    #[test]
    fn app_server_thread_and_turn_requests_carry_meeting_contract() {
        let thread = app_server_thread_start_request(10);
        let turn = app_server_turn_run_request(
            11,
            "thread-1",
            "gpt-5.1-codex",
            "[00:01] Alex: ship it",
            serde_json::json!({ "meeting_id": "m1" }),
        );

        assert_eq!(thread["method"], "thread/start");
        assert_eq!(turn["method"], "turn/run");
        assert_eq!(turn["params"]["threadId"], "thread-1");
        assert_eq!(turn["params"]["model"], "gpt-5.1-codex");
        assert!(turn["params"]["input"]["transcript"]
            .as_str()
            .unwrap()
            .contains("ship it"));
        assert!(turn["params"]["input"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Return only valid JSON"));
        assert_eq!(
            turn["params"]["input"]["outputSchema"]["required"][0],
            "executive_summary"
        );
    }

    #[tokio::test]
    async fn app_server_actions_do_not_fall_back_to_codex_exec() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("codex-app-server");
        make_executable(&runtime);
        let provider = CodexAppServerProvider::new(CodexProviderConfig::default(), runtime).unwrap();

        let status = provider.test_processing(Some(temp.path())).await.unwrap();
        assert!(!status.success);
        assert!(status.message.contains("codex exec is disabled"));
    }

    #[test]
    fn redacts_secret_like_strings() {
        let redacted = redact_secrets("stderr Authorization: Bearer abcdefghijklmnopqrstuvwxyz123 api_key = sk-proj-abcdefghijklmnopqrstuvwxyz123456 refresh_token = secretrefresh1234567890");
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!redacted.contains("secretrefresh1234567890"));
    }
}

impl Default for CodexHomeMode {
    fn default() -> Self {
        Self::ClawscribeIsolated
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProviderConfig {
    #[serde(default)]
    pub codex_home_mode: CodexHomeMode,
    pub codex_home_path: Option<String>,
    #[serde(default)]
    pub use_existing_user_codex_session: bool,
    pub codex_binary_path: Option<String>,
    #[serde(default = "default_codex_model")]
    pub model: String,
    #[serde(default = "default_codex_timeout_seconds")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingConfig {
    #[serde(default = "default_processing_provider")]
    pub provider: String,
    #[serde(default)]
    pub codex: CodexProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawScribeProcessingConfig {
    pub processing: ProcessingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexInstallationStatus {
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub runtime_kind: String,
    pub codex_home: String,
    pub codex_home_mode: CodexHomeMode,
    pub auth_status: Option<String>,
    pub account_email: Option<String>,
    pub plan_type: Option<String>,
    pub rate_limit_state: Option<String>,
    #[serde(default)]
    pub desktop_app_detected: bool,
    pub install_command: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCommandStatus {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexInstallCommand {
    pub label: String,
    pub shell: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexInstallRepairPlan {
    pub requires_confirmation: bool,
    pub docs_url: String,
    pub message: String,
    pub recommended: CodexInstallCommand,
    pub alternatives: Vec<CodexInstallCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProcessingResult {
    pub meeting_id: String,
    pub scratch_dir: String,
    pub output_json_path: String,
    pub notes_markdown_path: String,
    pub follow_up_email_path: String,
    pub processing_log_path: String,
    pub structured_output: MeetingNotesOutput,
    pub markdown: String,
}

#[derive(Debug, Clone)]
pub struct CodexAppServerProvider {
    pub config: CodexProviderConfig,
    pub app_server_binary: PathBuf,
    pub codex_home: Option<PathBuf>,
}

pub type CodexProcessingProvider = CodexAppServerProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingNotesOutput {
    pub executive_summary: String,
    pub decisions: Vec<DecisionItem>,
    pub risks_blockers: Vec<RiskBlockerItem>,
    pub open_questions: Vec<OpenQuestionItem>,
    pub action_items: Vec<ActionItem>,
    pub follow_up_email: FollowUpEmail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionItem {
    pub decision: String,
    pub owner: Option<String>,
    pub timestamp: Option<String>,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskBlockerItem {
    pub risk: String,
    pub impact: Option<String>,
    pub mitigation: Option<String>,
    pub owner: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenQuestionItem {
    pub question: String,
    pub owner: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionItem {
    pub task: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
    pub source_timestamp: Option<String>,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUpEmail {
    pub subject: String,
    pub body_markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

fn default_codex_model() -> String {
    DEFAULT_CODEX_MODEL.to_string()
}

fn default_codex_timeout_seconds() -> u64 {
    DEFAULT_CODEX_TIMEOUT_SECONDS
}

fn default_processing_provider() -> String {
    "openai-compatible".to_string()
}

impl Default for CodexProviderConfig {
    fn default() -> Self {
        Self {
            codex_home_mode: CodexHomeMode::ClawscribeIsolated,
            codex_home_path: Some(default_isolated_codex_home().to_string_lossy().to_string()),
            use_existing_user_codex_session: false,
            codex_binary_path: None,
            model: default_codex_model(),
            timeout_seconds: DEFAULT_CODEX_TIMEOUT_SECONDS,
        }
    }
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        Self {
            provider: default_processing_provider(),
            codex: CodexProviderConfig::default(),
        }
    }
}

impl Default for ClawScribeProcessingConfig {
    fn default() -> Self {
        Self {
            processing: ProcessingConfig::default(),
        }
    }
}

impl CodexProviderConfig {
    fn effective_home_mode(&self) -> CodexHomeMode {
        CodexHomeMode::ClawscribeIsolated
    }

    pub fn effective_codex_home(&self) -> Option<PathBuf> {
        match self.effective_home_mode() {
            CodexHomeMode::ClawscribeIsolated => self
                .codex_home_path
                .as_ref()
                .map(expand_windows_style_appdata)
                .or_else(|| Some(default_isolated_codex_home())),
            CodexHomeMode::ExistingUserCodexSession => None,
        }
    }
}

impl CodexAppServerProvider {
    pub fn new(config: CodexProviderConfig, app_server_binary: PathBuf) -> Result<Self, String> {
        let codex_home = config.effective_codex_home();
        if let Some(home) = &codex_home {
            prepare_isolated_codex_home(home, &config.model)?;
        }
        Ok(Self {
            config,
            app_server_binary,
            codex_home,
        })
    }

    pub async fn check_installation(&self) -> Result<CodexInstallationStatus, String> {
        let codex_home = self
            .codex_home
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| default_isolated_codex_home().to_string_lossy().to_string());

        Ok(CodexInstallationStatus {
            found: true,
            version: Some("bundled app-server runtime".to_string()),
            path: Some(self.app_server_binary.to_string_lossy().to_string()),
            runtime_kind: "codex-app-server".to_string(),
            codex_home,
            codex_home_mode: self.config.effective_home_mode(),
            auth_status: Some("unknown until account/read is wired".to_string()),
            account_email: None,
            plan_type: None,
            rate_limit_state: None,
            desktop_app_detected: false,
            install_command: Some(codex_install_repair_plan().recommended.command),
            message: "Bundled Codex app-server runtime found. JSON-RPC account/thread integration is the target path; CLI/codex exec fallback is disabled.".to_string(),
        })
    }

    pub async fn login_browser(&self) -> Result<CodexCommandStatus, String> {
        Ok(app_server_pending_status(
            "Sign in with ChatGPT will use account/login/start { \"type\": \"chatgpt\" } over the Codex app-server JSON-RPC transport.",
        ))
    }

    pub async fn login_device(&self) -> Result<CodexCommandStatus, String> {
        Ok(app_server_pending_status(
            "Device-code sign-in will use account/login/start { \"type\": \"chatgptDeviceCode\" } and surface verificationUrl plus userCode in the UI.",
        ))
    }

    pub async fn logout(&self) -> Result<CodexCommandStatus, String> {
        Ok(app_server_pending_status(
            "Logout will use account/logout over the Codex app-server JSON-RPC transport.",
        ))
    }

    pub async fn test_processing(
        &self,
        _scratch_parent: Option<&Path>,
    ) -> Result<CodexCommandStatus, String> {
        Ok(app_server_pending_status(
            "Test processing will use thread/start plus turn/run over Codex app-server. codex exec is disabled as an app integration path.",
        ))
    }

    pub async fn process_meeting(
        &self,
        request: CodexMeetingProcessRequest,
    ) -> Result<CodexProcessingResult, String> {
        let scratch_dir = request
            .scratch_root
            .unwrap_or_else(default_codex_runs_root)
            .join(sanitize_path_segment(&request.meeting_id));
        fs::create_dir_all(&scratch_dir)
            .map_err(|e| format!("Failed to create Codex run folder: {e}"))?;

        let transcript_path = scratch_dir.join("transcript.md");
        let metadata_path = scratch_dir.join("metadata.json");
        let schema_path = scratch_dir.join("output-schema.json");
        let prompt_path = scratch_dir.join("prompt.md");
        let output_path = scratch_dir.join("codex-output.json");
        let final_path = scratch_dir.join("codex-final.md");
        let events_path = scratch_dir.join("codex-events.jsonl");

        fs::write(
            &transcript_path,
            normalize_transcript_markdown(&request.transcript),
        )
        .map_err(|e| format!("Failed to write transcript.md: {e}"))?;
        fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "meeting_id": request.meeting_id,
                "meeting_title": request.meeting_title,
            }))
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("Failed to write metadata.json: {e}"))?;
        fs::write(&schema_path, output_schema_json())
            .map_err(|e| format!("Failed to write output-schema.json: {e}"))?;
        fs::write(&prompt_path, build_meeting_prompt())
            .map_err(|e| format!("Failed to write prompt.md: {e}"))?;
        let pending = app_server_pending_status(
            "Codex app-server meeting processing will use a fresh thread/start and turn/run. codex exec fallback is disabled.",
        );
        write_safe_events(&events_path, &pending)?;
        fs::write(&final_path, &pending.message)
            .map_err(|e| format!("Failed to write codex-final.md: {e}"))?;
        fs::write(&output_path, "{}").map_err(|e| format!("Failed to write codex-output.json: {e}"))?;
        write_processing_log(&scratch_dir, &pending, Duration::ZERO, "app-server runtime missing")?;
        Err(format!("{CODEX_APP_SERVER_STAGED} Use OpenAI API key or OpenClaw for normal processing until the pinned app-server runtime is packaged."))
    }
}

fn app_server_pending_status(detail: &str) -> CodexCommandStatus {
    CodexCommandStatus {
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        message: format!("{CODEX_APP_SERVER_STAGED} {detail}"),
    }
}

#[derive(Debug, Clone)]
pub struct CodexMeetingProcessRequest {
    pub meeting_id: String,
    pub meeting_title: Option<String>,
    pub transcript: String,
    pub output_dir: Option<PathBuf>,
    pub scratch_root: Option<PathBuf>,
}

#[tauri::command]
pub async fn codex_get_config<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ClawScribeProcessingConfig, String> {
    load_processing_config(&app)
}

#[tauri::command]
pub async fn codex_save_config<R: Runtime>(
    app: AppHandle<R>,
    config: CodexProviderConfig,
) -> Result<ClawScribeProcessingConfig, String> {
    let mut full = load_processing_config(&app).unwrap_or_default();
    full.processing.codex = normalize_codex_config(config);
    save_processing_config(&app, &full)?;
    Ok(full)
}

#[tauri::command]
pub async fn codex_check_installation<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CodexInstallationStatus, String> {
    let config = normalize_codex_config(
        load_processing_config(&app)
            .unwrap_or_default()
            .processing
            .codex,
    );
    codex_installation_status_for_config(&app, config).await
}

#[tauri::command]
pub async fn codex_find_automatically<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CodexInstallationStatus, String> {
    let mut config = normalize_codex_config(
        load_processing_config(&app)
            .unwrap_or_default()
            .processing
            .codex,
    );
    config.codex_binary_path = None;
    codex_installation_status_for_config(&app, config).await
}

#[tauri::command]
pub async fn codex_browse_for_binary<R: Runtime>(
    _app: AppHandle<R>,
) -> Result<Option<String>, String> {
    Err("Browsing to a global codex.exe is no longer supported. Codex app-server mode uses a bundled/pinned ClawScribe runtime or a controlled ClawScribe runtime installer.".to_string())
}

#[tauri::command]
pub fn codex_prepare_install_command() -> CodexInstallRepairPlan {
    codex_install_repair_plan()
}

#[tauri::command]
pub async fn codex_login_browser<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CodexCommandStatus, String> {
    let provider = provider_from_app(&app)?;
    let _ = app.emit("codex-auth-progress", "Starting Codex browser login");
    let status = provider.login_browser().await?;
    let _ = app.emit("codex-auth-progress", status.message.clone());
    Ok(status)
}

#[tauri::command]
pub async fn codex_login_device<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CodexCommandStatus, String> {
    let provider = provider_from_app(&app)?;
    let _ = app.emit("codex-auth-progress", "Starting Codex device-code login");
    let status = provider.login_device().await?;
    let _ = app.emit("codex-auth-progress", status.message.clone());
    Ok(status)
}

#[tauri::command]
pub async fn codex_logout<R: Runtime>(app: AppHandle<R>) -> Result<CodexCommandStatus, String> {
    let provider = provider_from_app(&app)?;
    provider.logout().await
}

#[tauri::command]
pub async fn codex_test_processing<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CodexCommandStatus, String> {
    let provider = provider_from_app(&app)?;
    provider.test_processing(None).await
}

#[tauri::command]
pub async fn codex_process_meeting<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
) -> Result<CodexProcessingResult, String> {
    let pool = state.db_manager.pool();
    let meeting =
        crate::database::repositories::meeting::MeetingsRepository::get_meeting(pool, &meeting_id)
            .await
            .map_err(|e| format!("Failed to load meeting: {e}"))?
            .ok_or_else(|| format!("Meeting not found: {meeting_id}"))?;
    let metadata =
        crate::database::repositories::meeting::MeetingsRepository::get_meeting_metadata(
            pool,
            &meeting_id,
        )
        .await
        .map_err(|e| format!("Failed to load meeting metadata: {e}"))?;
    let transcript = meeting
        .transcripts
        .iter()
        .map(|t| {
            let stamp = t
                .audio_start_time
                .map(format_seconds)
                .unwrap_or_else(|| t.timestamp.clone());
            format!("[{stamp}] {}", t.text)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let output_dir = metadata.and_then(|m| m.folder_path.map(PathBuf::from));
    let provider = provider_from_app(&app)?;
    provider
        .process_meeting(CodexMeetingProcessRequest {
            meeting_id,
            meeting_title: Some(meeting.title),
            transcript,
            output_dir,
            scratch_root: None,
        })
        .await
}

pub fn provider_from_app<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<CodexProcessingProvider, String> {
    let config = normalize_codex_config(
        load_processing_config(app)
            .unwrap_or_default()
            .processing
            .codex,
    );
    let runtime = discover_codex_app_server(app, config.codex_binary_path.as_deref())?;
    CodexAppServerProvider::new(config, runtime)
}

pub fn discover_codex_app_server<R: Runtime>(
    app: &AppHandle<R>,
    configured_path: Option<&str>,
) -> Result<PathBuf, String> {
    let resource_dir = app.path().resource_dir().ok();
    discover_codex_app_server_from(configured_path, resource_dir.as_deref())
}

pub fn discover_codex_binary<R: Runtime>(
    app: &AppHandle<R>,
    configured_path: Option<&str>,
) -> Result<PathBuf, String> {
    discover_codex_app_server(app, configured_path)
}

fn discover_codex_app_server_from(
    configured_path: Option<&str>,
    resource_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(path) = configured_path.filter(|s| !s.trim().is_empty()) {
        let candidate = PathBuf::from(path);
        if is_windowsapps_path(&candidate) {
            return Err(CODEX_WINDOWSAPPS_REJECTED.to_string());
        }
        return Err(format!(
            "Configured global Codex executable paths are no longer supported: {}. Codex app-server mode requires a bundled/pinned ClawScribe runtime or a controlled ClawScribe runtime installer.",
            candidate.display()
        ));
    }

    if let Some(resource_dir) = resource_dir {
        for candidate in bundled_codex_app_server_candidates(resource_dir) {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(CODEX_APP_SERVER_MISSING.to_string())
}

async fn codex_installation_status_for_config<R: Runtime>(
    app: &AppHandle<R>,
    config: CodexProviderConfig,
) -> Result<CodexInstallationStatus, String> {
    let codex_home = config
        .effective_codex_home()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| default_isolated_codex_home().to_string_lossy().to_string());
    match discover_codex_app_server(app, config.codex_binary_path.as_deref()) {
        Ok(runtime) => {
            CodexAppServerProvider::new(config, runtime)?
                .check_installation()
                .await
        }
        Err(message) => {
            Ok(CodexInstallationStatus {
                found: false,
                version: None,
                path: None,
                runtime_kind: "codex-app-server".to_string(),
                codex_home,
                codex_home_mode: config.effective_home_mode(),
                auth_status: None,
                account_email: None,
                plan_type: None,
                rate_limit_state: None,
                desktop_app_detected: false,
                install_command: Some(codex_install_repair_plan().recommended.command),
                message,
            })
        }
    }
}

fn is_windowsapps_path(path: &Path) -> bool {
    path.to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\")
        .contains("\\windowsapps\\")
}

fn codex_install_repair_plan() -> CodexInstallRepairPlan {
    let recommended = CodexInstallCommand {
        label: "Repair bundled Codex app-server runtime".to_string(),
        shell: "ClawScribe".to_string(),
        command: "Controlled Codex app-server runtime installer is not bundled in this build.".to_string(),
    };

    CodexInstallRepairPlan {
        requires_confirmation: true,
        docs_url: "docs/auth/codex-auth.md".to_string(),
        message: "Codex app-server mode needs a ClawScribe-bundled/pinned runtime or a controlled first-run installer. This build will not use global codex.exe, PATH, WindowsApps, or user-browsed Store app internals. OpenAI API key and OpenClaw continue to work without Codex.".to_string(),
        recommended,
        alternatives: vec![],
    }
}

pub fn codex_runtime_required_for_provider(provider: &str) -> bool {
    matches!(provider, "codex" | "codex-login" | "codex-chatgpt")
}

fn bundled_codex_app_server_candidates(resource_dir: &Path) -> Vec<PathBuf> {
    let exe = if cfg!(target_os = "windows") {
        "codex-app-server.exe"
    } else {
        "codex-app-server"
    };
    vec![
        resource_dir.join("codex-app-server").join(exe),
        resource_dir.join("codex").join("app-server").join(exe),
        resource_dir.join("bin").join("codex").join(exe),
        resource_dir.join(exe),
    ]
}

fn normalize_codex_config(mut config: CodexProviderConfig) -> CodexProviderConfig {
    if config.model.trim().is_empty() {
        config.model = default_codex_model();
    }
    if config.timeout_seconds == 0 {
        config.timeout_seconds = DEFAULT_CODEX_TIMEOUT_SECONDS;
    }
    config.use_existing_user_codex_session = false;
    config.codex_home_mode = CodexHomeMode::ClawscribeIsolated;
    if config
        .codex_home_path
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        config.codex_home_path = Some(default_isolated_codex_home().to_string_lossy().to_string());
    }
    config
}

fn load_processing_config<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<ClawScribeProcessingConfig, String> {
    let path = processing_config_path(app)?;
    if !path.exists() {
        return Ok(ClawScribeProcessingConfig::default());
    }
    let raw =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read processing config: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("Invalid processing config: {e}"))
}

fn save_processing_config<R: Runtime>(
    app: &AppHandle<R>,
    config: &ClawScribeProcessingConfig,
) -> Result<(), String> {
    let path = processing_config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config folder: {e}"))?;
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Failed to write processing config: {e}"))
}

fn processing_config_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to resolve app config directory: {e}"))?;
    Ok(dir.join("processing-config.json"))
}

fn default_isolated_codex_home() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("ClawScribe")
        .join("codex")
}

fn default_codex_runs_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("ClawScribe")
        .join("codex-runs")
}

fn expand_windows_style_appdata(value: &String) -> PathBuf {
    let appdata = std::env::var("APPDATA")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs::data_dir);
    if let Some(base) = appdata {
        if value.starts_with("%APPDATA%\\") {
            return base.join(value.trim_start_matches("%APPDATA%\\"));
        }
        if value.starts_with("%APPDATA%/") {
            return base.join(value.trim_start_matches("%APPDATA%/"));
        }
    }
    PathBuf::from(value)
}

fn prepare_isolated_codex_home(home: &Path, model: &str) -> Result<(), String> {
    fs::create_dir_all(home).map_err(|e| format!("Failed to create isolated CODEX_HOME: {e}"))?;
    let config_path = home.join("config.toml");
    if !config_path.exists() {
        let config = format!(
            "# ClawScribe-owned Codex profile. Do not paste secrets here.\nmodel = \"{}\"\nsandbox_mode = \"read-only\"\napproval_policy = \"never\"\n",
            model.replace('"', "")
        );
        fs::write(&config_path, config)
            .map_err(|e| format!("Failed to write isolated Codex config.toml: {e}"))?;
    }
    Ok(())
}

fn json_rpc_request(id: u64, method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

fn app_server_initialize_request(id: u64) -> Value {
    json_rpc_request(
        id,
        "initialize",
        serde_json::json!({
            "clientInfo": {
                "name": "ClawScribe",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "transport": "stdio-jsonl",
        }),
    )
}

fn app_server_initialized_notification() -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {},
    })
}

fn app_server_account_read_request(id: u64) -> Value {
    json_rpc_request(id, "account/read", serde_json::json!({}))
}

fn app_server_login_start_request(id: u64, login_type: &str) -> Value {
    json_rpc_request(
        id,
        "account/login/start",
        serde_json::json!({
            "type": login_type,
        }),
    )
}

fn app_server_logout_request(id: u64) -> Value {
    json_rpc_request(id, "account/logout", serde_json::json!({}))
}

fn app_server_thread_start_request(id: u64) -> Value {
    json_rpc_request(id, "thread/start", serde_json::json!({}))
}

fn app_server_turn_run_request(
    id: u64,
    thread_id: &str,
    model: &str,
    transcript: &str,
    metadata: Value,
) -> Value {
    json_rpc_request(
        id,
        "turn/run",
        serde_json::json!({
            "threadId": thread_id,
            "model": model,
            "input": {
                "transcript": normalize_transcript_markdown(transcript),
                "metadata": metadata,
                "instructions": build_meeting_prompt(),
                "outputSchema": serde_json::from_str::<Value>(&output_schema_json()).unwrap_or(Value::Null),
            },
        }),
    )
}

fn normalize_transcript_markdown(transcript: &str) -> String {
    let trimmed = transcript.trim();
    if trimmed.is_empty() {
        "# Transcript\n\nNo transcript text was provided.\n".to_string()
    } else {
        format!("# Transcript\n\n{trimmed}\n")
    }
}

pub(crate) fn build_meeting_prompt() -> String {
    r#"You are processing a meeting transcript for ClawScribe.

Return only valid JSON matching output-schema.json.

Extract:
- executive summary
- decisions
- risks/blockers
- open questions
- action items/todos
- optional follow-up email draft

Rules:
- Include source timestamps when available.
- Do not invent owners or due dates.
- Use null when unknown.
- If a section has no findings, return an empty array or a concise empty-state string as appropriate.
- Do not include Markdown fences, commentary, or fields outside the schema.
"#
    .to_string()
}

pub fn output_schema_json() -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "type": "object",
        "properties": {
            "executive_summary": { "type": "string" },
            "decisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "decision": { "type": "string" },
                        "owner": { "type": ["string", "null"] },
                        "timestamp": { "type": ["string", "null"] },
                        "confidence": { "type": "string", "enum": ["high", "medium", "low"] }
                    },
                    "required": ["decision", "owner", "timestamp", "confidence"],
                    "additionalProperties": false
                }
            },
            "risks_blockers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "risk": { "type": "string" },
                        "impact": { "type": ["string", "null"] },
                        "mitigation": { "type": ["string", "null"] },
                        "owner": { "type": ["string", "null"] },
                        "timestamp": { "type": ["string", "null"] }
                    },
                    "required": ["risk", "impact", "mitigation", "owner", "timestamp"],
                    "additionalProperties": false
                }
            },
            "open_questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "owner": { "type": ["string", "null"] },
                        "timestamp": { "type": ["string", "null"] }
                    },
                    "required": ["question", "owner", "timestamp"],
                    "additionalProperties": false
                }
            },
            "action_items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "task": { "type": "string" },
                        "owner": { "type": ["string", "null"] },
                        "due_date": { "type": ["string", "null"] },
                        "source_timestamp": { "type": ["string", "null"] },
                        "confidence": { "type": "string", "enum": ["high", "medium", "low"] }
                    },
                    "required": ["task", "owner", "due_date", "source_timestamp", "confidence"],
                    "additionalProperties": false
                }
            },
            "follow_up_email": {
                "type": "object",
                "properties": {
                    "subject": { "type": "string" },
                    "body_markdown": { "type": "string" }
                },
                "required": ["subject", "body_markdown"],
                "additionalProperties": false
            }
        },
        "required": [
            "executive_summary",
            "decisions",
            "risks_blockers",
            "open_questions",
            "action_items",
            "follow_up_email"
        ],
        "additionalProperties": false
    }))
    .unwrap()
}

pub(crate) fn parse_meeting_output(raw: &str) -> Result<MeetingNotesOutput, String> {
    let cleaned = strip_json_fence(raw);
    serde_json::from_str::<MeetingNotesOutput>(&cleaned)
        .map_err(|e| format!("Provider returned invalid meeting JSON: {e}"))
}

pub(crate) fn strip_json_fence(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_prefix = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    without_prefix
        .strip_suffix("```")
        .unwrap_or(without_prefix)
        .trim()
        .to_string()
}

pub fn render_meeting_notes_markdown(
    meeting_title: &Option<String>,
    output: &MeetingNotesOutput,
) -> String {
    let title = meeting_title
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Meeting Notes");
    let mut markdown = format!(
        "# {title}\n\n## Executive Summary\n\n{}\n",
        output.executive_summary.trim()
    );

    markdown.push_str("\n## Decisions\n\n");
    push_decisions(&mut markdown, &output.decisions);
    markdown.push_str("\n## Risks And Blockers\n\n");
    push_risks(&mut markdown, &output.risks_blockers);
    markdown.push_str("\n## Open Questions\n\n");
    push_questions(&mut markdown, &output.open_questions);
    markdown.push_str("\n## Action Items\n\n");
    push_actions(&mut markdown, &output.action_items);
    markdown.push_str("\n## Follow-Up Email Draft\n\n");
    markdown.push_str(&format!(
        "**Subject:** {}\n\n{}",
        output.follow_up_email.subject.trim(),
        output.follow_up_email.body_markdown.trim()
    ));
    markdown.push('\n');
    markdown
}

fn push_decisions(markdown: &mut String, items: &[DecisionItem]) {
    if items.is_empty() {
        markdown.push_str("None noted.\n");
        return;
    }
    for item in items {
        markdown.push_str(&format!(
            "- {}{}\n",
            item.decision.trim(),
            metadata_suffix([
                ("owner", item.owner.as_deref()),
                ("timestamp", item.timestamp.as_deref()),
                ("confidence", Some(confidence_str(&item.confidence))),
            ])
        ));
    }
}

fn push_risks(markdown: &mut String, items: &[RiskBlockerItem]) {
    if items.is_empty() {
        markdown.push_str("None noted.\n");
        return;
    }
    for item in items {
        markdown.push_str(&format!(
            "- {}{}\n",
            item.risk.trim(),
            metadata_suffix([
                ("impact", item.impact.as_deref()),
                ("mitigation", item.mitigation.as_deref()),
                ("owner", item.owner.as_deref()),
                ("timestamp", item.timestamp.as_deref()),
            ])
        ));
    }
}

fn push_questions(markdown: &mut String, items: &[OpenQuestionItem]) {
    if items.is_empty() {
        markdown.push_str("None noted.\n");
        return;
    }
    for item in items {
        markdown.push_str(&format!(
            "- {}{}\n",
            item.question.trim(),
            metadata_suffix([
                ("owner", item.owner.as_deref()),
                ("timestamp", item.timestamp.as_deref()),
            ])
        ));
    }
}

fn push_actions(markdown: &mut String, items: &[ActionItem]) {
    if items.is_empty() {
        markdown.push_str("None noted.\n");
        return;
    }
    for item in items {
        markdown.push_str(&format!(
            "- [ ] {}{}\n",
            item.task.trim(),
            metadata_suffix([
                ("owner", item.owner.as_deref()),
                ("due", item.due_date.as_deref()),
                ("timestamp", item.source_timestamp.as_deref()),
                ("confidence", Some(confidence_str(&item.confidence))),
            ])
        ));
    }
}

fn metadata_suffix<'a, I>(items: I) -> String
where
    I: IntoIterator<Item = (&'a str, Option<&'a str>)>,
{
    let parts = items
        .into_iter()
        .filter_map(|(label, value)| {
            let value = value?.trim();
            if value.is_empty() {
                None
            } else {
                Some(format!("{label}: {value}"))
            }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join("; "))
    }
}

fn confidence_str(confidence: &Confidence) -> &'static str {
    match confidence {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Low => "low",
    }
}

pub(crate) fn render_follow_up_email(email: &FollowUpEmail) -> String {
    format!(
        "# Follow-Up Email\n\n**Subject:** {}\n\n{}",
        email.subject, email.body_markdown
    )
}

fn write_safe_events(path: &Path, output: &CodexCommandStatus) -> Result<(), String> {
    let mut lines = Vec::new();
    if !output.stdout.trim().is_empty() {
        lines.push(serde_json::json!({ "stream": "stdout", "text": output.stdout }));
    }
    if !output.stderr.trim().is_empty() {
        lines.push(serde_json::json!({ "stream": "stderr", "text": output.stderr }));
    }
    let body = lines
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, body).map_err(|e| format!("Failed to write Codex events: {e}"))
}

fn write_processing_log(
    scratch_dir: &Path,
    output: &CodexCommandStatus,
    duration: Duration,
    status: &str,
) -> Result<(), String> {
    write_processing_log_at(
        &scratch_dir.join("processing-log.json"),
        output,
        duration,
        status,
        Some(scratch_dir),
    )
}

fn write_processing_log_at(
    path: &Path,
    output: &CodexCommandStatus,
    duration: Duration,
    status: &str,
    scratch_dir: Option<&Path>,
) -> Result<(), String> {
    let log = serde_json::json!({
        "provider": "codex",
        "status": status,
        "exit_code": output.exit_code,
        "duration_seconds": duration.as_secs_f64(),
        "stdout_excerpt": truncate_for_log(&output.stdout),
        "stderr_excerpt": truncate_for_log(&output.stderr),
        "scratch_dir": scratch_dir.map(|p| p.to_string_lossy().to_string()),
    });
    fs::write(
        path,
        serde_json::to_string_pretty(&log).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Failed to write processing-log.json: {e}"))
}

fn truncate_for_log(value: &str) -> String {
    let redacted = redact_secrets(value);
    if redacted.len() > 4000 {
        format!("{}…", &redacted[..4000])
    } else {
        redacted
    }
}

pub fn redact_secrets(value: &str) -> String {
    let patterns = [
        r"sk-[A-Za-z0-9_-]{20,}",
        r"sk-proj-[A-Za-z0-9_-]{20,}",
        r"sk-ant-[A-Za-z0-9_-]{20,}",
        r"Bearer\s+[A-Za-z0-9._~+/=-]{20,}",
        r#"(?i)(access_token|refresh_token|id_token|api_key|authorization)["'\s:=]+[A-Za-z0-9._~+/=-]{12,}"#,
    ];
    let mut redacted = value.to_string();
    for pattern in patterns {
        if let Ok(regex) = regex::Regex::new(pattern) {
            redacted = regex.replace_all(&redacted, "[REDACTED]").to_string();
        }
    }
    redacted
}

fn sanitize_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn format_seconds(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

#[cfg(all(test, any()))]
mod tests {
    use super::*;
    use std::io::Write;

    fn valid_meeting_json() -> String {
        serde_json::json!({
            "executive_summary": "The team agreed to ship the Codex provider.",
            "decisions": [{
                "decision": "Use Codex as the auth and runtime boundary.",
                "owner": null,
                "timestamp": "00:01",
                "confidence": "high"
            }],
            "risks_blockers": [{
                "risk": "Windows login still needs runtime verification.",
                "impact": "Release cannot claim verified Codex login yet.",
                "mitigation": "Run the Windows checklist.",
                "owner": null,
                "timestamp": null
            }],
            "open_questions": [{
                "question": "Which Codex model should be the release default?",
                "owner": null,
                "timestamp": null
            }],
            "action_items": [{
                "task": "Run fake-Codex tests.",
                "owner": "Nora",
                "due_date": null,
                "source_timestamp": "00:02",
                "confidence": "high"
            }],
            "follow_up_email": {
                "subject": "ClawScribe Codex provider",
                "body_markdown": "Codex provider implementation is ready for Windows verification."
            }
        })
        .to_string()
    }

    fn fake_codex(dir: &Path, scenario: &str) -> PathBuf {
        let path = dir.join("codex");
        let env_log = dir.join("env.log");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "${{CODEX_HOME:-}}" > "{}"
scenario="{}"
if [[ "${{1:-}}" == "--version" ]]; then
  if [[ "$scenario" == "version-fail" ]]; then
    echo "missing" >&2
    exit 12
  fi
  echo "codex-cli 999.0.0"
  exit 0
fi
if [[ "${{1:-}}" == "login" && "${{2:-}}" == "status" ]]; then
  echo "Logged in"
  exit 0
fi
if [[ "${{1:-}}" == "login" ]]; then
  if [[ "${{2:-}}" == "--device-auth" ]]; then
    echo "Use code ABCD-EFGH at https://example.test/device"
  else
    echo "Opening browser"
  fi
  exit 0
fi
if [[ "${{1:-}}" == "logout" ]]; then
  echo "Logged out"
  exit 0
fi
if [[ "${{1:-}}" == "exec" ]]; then
  out=""
  for ((i=1; i<=$#; i++)); do
    arg="${{!i}}"
    if [[ "$arg" == "--output-last-message" ]]; then
      j=$((i+1))
      out="${{!j}}"
    fi
  done
  if [[ "$scenario" == "exec-nonzero" ]]; then
    echo "Authorization: Bearer secret-token-value-1234567890" >&2
    exit 22
  fi
  if [[ "$*" == *"CLASCRIBE_CODEX_OK"* ]]; then
    echo "CLASCRIBE_CODEX_OK" > "$out"
    echo "CLASCRIBE_CODEX_OK"
    exit 0
  fi
  if [[ "$scenario" == "invalid-json" ]]; then
    echo "not json" > "$out"
    exit 0
  fi
  cat > "$out" <<'JSON'
            "#,
            env_log.display(),
            scenario,
        )
        .unwrap();
        file.write_all(valid_meeting_json().as_bytes()).unwrap();
        writeln!(
            file,
            r#"
JSON
  echo "done"
  exit 0
fi
exit 2
"#
        )
        .unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn provider_with_fake(temp: &tempfile::TempDir) -> CodexProcessingProvider {
        provider_with_fake_scenario(temp, "ok")
    }

    fn provider_with_fake_scenario(
        temp: &tempfile::TempDir,
        scenario: &str,
    ) -> CodexProcessingProvider {
        let fake = fake_codex(temp.path(), scenario);
        let home = temp.path().join("clawscribe-codex-home");
        CodexProcessingProvider::new(
            CodexProviderConfig {
                codex_home_path: Some(home.to_string_lossy().to_string()),
                codex_binary_path: Some(fake.to_string_lossy().to_string()),
                ..CodexProviderConfig::default()
            },
            fake,
        )
        .unwrap()
    }

    fn make_executable(path: &Path) {
        fs::write(path, "#!/usr/bin/env bash\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
    }

    #[test]
    fn configured_codex_path_wins_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let configured = temp.path().join("configured-codex");
        let path_dir = temp.path().join("path-bin");
        fs::create_dir_all(&path_dir).unwrap();
        make_executable(&configured);
        make_executable(&path_dir.join("codex"));
        let path_env = std::env::join_paths([path_dir]).unwrap();

        let discovered =
            discover_codex_binary_from(Some(configured.to_str().unwrap()), None, Some(&path_env))
                .unwrap();
        assert_eq!(discovered, configured);
    }

    #[test]
    fn path_discovery_finds_codex_command() {
        let temp = tempfile::tempdir().unwrap();
        let path_dir = temp.path().join("path-bin");
        fs::create_dir_all(&path_dir).unwrap();
        let codex = path_dir.join("codex");
        make_executable(&codex);
        let path_env = std::env::join_paths([path_dir]).unwrap();

        let discovered = discover_codex_binary_from(None, None, Some(&path_env)).unwrap();
        assert_eq!(discovered, codex);
    }

    #[test]
    fn windows_default_path_discovery_checks_localappdata_and_program_files() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("LocalAppData");
        let program_files = temp.path().join("ProgramFiles");
        let expected = local
            .join("Programs")
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("codex.exe");
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        make_executable(&expected);

        let candidates = windows_codex_cli_candidates_from(
            Some(local.as_os_str()),
            Some(program_files.as_os_str()),
        );
        assert_eq!(candidates.first(), Some(&expected));
        assert!(candidates.contains(&program_files.join("OpenAI").join("Codex").join("codex.exe")));
    }

    #[test]
    fn bundled_resource_path_is_last_resort_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let resource_dir = temp.path().join("resources");
        let bundled = resource_dir
            .join("codex")
            .join(if cfg!(target_os = "windows") {
                "codex.exe"
            } else {
                "codex"
            });
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        make_executable(&bundled);

        let empty_path = OsString::from("");
        let discovered =
            discover_codex_binary_from(None, Some(&resource_dir), Some(empty_path.as_os_str()))
                .unwrap();
        assert_eq!(discovered, bundled);
    }

    #[test]
    fn install_repair_plan_is_prepare_only() {
        let plan = codex_install_repair_plan();
        assert!(plan.requires_confirmation);
        assert!(plan.docs_url.contains("developers.openai.com/codex/cli"));
        assert!(!plan.recommended.command.trim().is_empty());
        assert!(plan
            .message
            .contains("will not install Codex automatically"));
    }

    #[test]
    fn codex_runtime_is_required_only_for_codex_providers() {
        assert!(codex_runtime_required_for_provider("codex"));
        assert!(codex_runtime_required_for_provider("codex-login"));
        assert!(!codex_runtime_required_for_provider("openai"));
        assert!(!codex_runtime_required_for_provider("openclaw"));
        assert!(!codex_runtime_required_for_provider("custom-openai"));
    }

    #[test]
    fn existing_user_session_requires_explicit_opt_in_flag() {
        let temp = tempfile::tempdir().unwrap();
        let normalized = normalize_codex_config(CodexProviderConfig {
            codex_home_mode: CodexHomeMode::ExistingUserCodexSession,
            use_existing_user_codex_session: false,
            codex_home_path: Some(temp.path().join("isolated").to_string_lossy().to_string()),
            ..CodexProviderConfig::default()
        });

        assert_eq!(
            normalized.effective_home_mode(),
            CodexHomeMode::ClawscribeIsolated
        );
        assert!(normalized.effective_codex_home().is_some());
    }

    #[tokio::test]
    async fn fake_codex_version_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake(&temp);
        let status = provider.check_installation().await.unwrap();
        assert!(status.found);
        assert_eq!(status.version.as_deref(), Some("codex-cli 999.0.0"));
    }

    #[tokio::test]
    async fn fake_codex_version_failure_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake_scenario(&temp, "version-fail");
        let status = provider.check_installation().await.unwrap();
        assert!(!status.found);
        assert!(status.message.contains("missing"));
    }

    #[tokio::test]
    async fn fake_codex_login_browser_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake(&temp);
        let status = provider.login_browser().await.unwrap();
        assert!(status.success);
        assert!(status.stdout.contains("Opening browser"));
    }

    #[tokio::test]
    async fn fake_codex_login_device_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake(&temp);
        let status = provider.login_device().await.unwrap();
        assert!(status.success);
        assert!(status.stdout.contains("ABCD-EFGH"));
    }

    #[tokio::test]
    async fn fake_codex_test_processing_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake(&temp);
        let status = provider.test_processing(Some(temp.path())).await.unwrap();
        assert!(status.success);
        assert!(status.message.contains("succeeded"));
    }

    #[tokio::test]
    async fn fake_codex_process_meeting_writes_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake(&temp);
        let output_dir = temp.path().join("meeting");
        let result = provider
            .process_meeting(CodexMeetingProcessRequest {
                meeting_id: "meeting-1".to_string(),
                meeting_title: Some("Codex Standup".to_string()),
                transcript: "[00:01] We will use Codex.".to_string(),
                output_dir: Some(output_dir.clone()),
                scratch_root: Some(temp.path().join("runs")),
            })
            .await
            .unwrap();
        assert!(result.markdown.contains("Codex Standup"));
        assert!(output_dir.join("meeting-output.json").exists());
        assert!(output_dir.join("meeting-notes.md").exists());
        assert!(output_dir.join("follow-up-email.md").exists());
    }

    #[tokio::test]
    async fn fake_codex_invalid_json_fails() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake_scenario(&temp, "invalid-json");
        let err = provider
            .process_meeting(CodexMeetingProcessRequest {
                meeting_id: "meeting-2".to_string(),
                meeting_title: None,
                transcript: "hello".to_string(),
                output_dir: Some(temp.path().join("meeting")),
                scratch_root: Some(temp.path().join("runs")),
            })
            .await
            .unwrap_err();
        assert!(err.contains("invalid meeting JSON"));
    }

    #[tokio::test]
    async fn fake_codex_nonzero_redacts_stderr() {
        let temp = tempfile::tempdir().unwrap();
        let provider = provider_with_fake_scenario(&temp, "exec-nonzero");
        let err = provider
            .process_meeting(CodexMeetingProcessRequest {
                meeting_id: "meeting-3".to_string(),
                meeting_title: None,
                transcript: "hello".to_string(),
                output_dir: Some(temp.path().join("meeting")),
                scratch_root: Some(temp.path().join("runs")),
            })
            .await
            .unwrap_err();
        assert!(err.contains("Codex meeting processing failed"));
        let log = fs::read_to_string(
            temp.path()
                .join("runs")
                .join("meeting-3")
                .join("processing-log.json"),
        )
        .unwrap();
        assert!(log.contains("[REDACTED]"));
        assert!(!log.contains("secret-token-value"));
    }

    #[tokio::test]
    async fn isolated_codex_home_is_set_by_default() {
        let temp = tempfile::tempdir().unwrap();
        let env_log = temp.path().join("env.log");
        let provider = provider_with_fake(&temp);
        let _ = provider.check_installation().await.unwrap();
        let logged_home = fs::read_to_string(env_log).unwrap();
        assert!(logged_home.contains("clawscribe-codex-home"));
        assert!(provider
            .codex_home
            .as_ref()
            .unwrap()
            .join("config.toml")
            .exists());
    }

    #[tokio::test]
    async fn existing_user_codex_session_does_not_set_codex_home() {
        let temp = tempfile::tempdir().unwrap();
        let env_log = temp.path().join("env.log");
        let fake = fake_codex(temp.path(), "ok");
        let provider = CodexProcessingProvider::new(
            CodexProviderConfig {
                codex_home_mode: CodexHomeMode::ExistingUserCodexSession,
                use_existing_user_codex_session: true,
                codex_binary_path: Some(fake.to_string_lossy().to_string()),
                ..CodexProviderConfig::default()
            },
            fake,
        )
        .unwrap();
        let _ = provider.check_installation().await.unwrap();
        let logged_home = fs::read_to_string(env_log).unwrap();
        assert!(logged_home.trim().is_empty());
    }

    #[test]
    fn redacts_secret_like_strings() {
        let redacted = redact_secrets("stderr Authorization: Bearer abcdefghijklmnopqrstuvwxyz123 api_key = sk-proj-abcdefghijklmnopqrstuvwxyz123456");
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz123456"));
    }
}
