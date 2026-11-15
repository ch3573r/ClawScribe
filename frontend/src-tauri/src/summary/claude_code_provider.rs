//! One-shot text through the user's Claude Code. No credential or login handling.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tauri::{AppHandle, Manager, Runtime};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

const TIMEOUT: Duration = Duration::from_secs(300);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const STDERR_LIMIT: usize = 256 * 1024;
const LOGIN: &str = "Claude Code is not signed in. Open a terminal and run: claude auth login";
const NOT_INSTALLED: &str = "Claude Code was not found. Install it yourself or update Program path";
const GENERIC_ERROR: &str = "Claude Code returned an error";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeCodeConfig {
    pub program_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClaudeCodeStatus {
    NotInstalled,
    NotSignedIn,
    Ready {
        version: String,
        plan: Option<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone)]
pub(crate) struct ClaudeCodeProvider {
    config: ClaudeCodeConfig,
}

pub(crate) fn normalize_model(model: &str) -> Result<&str, String> {
    match model.trim() {
        "" => Ok("sonnet"),
        "sonnet" => Ok("sonnet"),
        "opus" => Ok("opus"),
        "haiku" => Ok("haiku"),
        _ => Err("Choose sonnet, opus or haiku in Claude Code settings".into()),
    }
}

fn config_path(data: &Path) -> PathBuf {
    data.join("claude-code-config.json")
}
pub(crate) fn provider_from_data(data: &Path) -> Result<ClaudeCodeProvider, String> {
    let config = match std::fs::read(config_path(data)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| "Could not read Claude Code settings")?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ClaudeCodeConfig::default(),
        Err(_) => return Err("Could not read Claude Code settings".into()),
    };
    Ok(ClaudeCodeProvider { config })
}
fn app_data<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|_| "App data directory unavailable".into())
}

#[tauri::command]
pub async fn claude_code_get_config<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ClaudeCodeConfig, String> {
    Ok(provider_from_data(&app_data(&app)?)?.config)
}
#[tauri::command]
pub async fn claude_code_save_config<R: Runtime>(
    app: AppHandle<R>,
    config: ClaudeCodeConfig,
) -> Result<ClaudeCodeConfig, String> {
    let config = ClaudeCodeConfig {
        program_path: config
            .program_path
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty()),
    };
    let data = app_data(&app)?;
    std::fs::create_dir_all(&data).map_err(|_| "Could not save Claude Code settings")?;
    let mut file = tempfile::NamedTempFile::new_in(&data)
        .map_err(|_| "Could not save Claude Code settings")?;
    use std::io::Write;
    file.write_all(
        &serde_json::to_vec(&config).map_err(|_| "Could not save Claude Code settings")?,
    )
    .map_err(|_| "Could not save Claude Code settings")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "Could not save Claude Code settings")?;
    file.persist(config_path(&data))
        .map_err(|_| "Could not save Claude Code settings")?;
    Ok(config)
}
#[tauri::command]
pub async fn claude_code_check_status<R: Runtime>(
    app: AppHandle<R>,
    config: Option<ClaudeCodeConfig>,
) -> Result<ClaudeCodeStatus, String> {
    let provider = match config {
        Some(config) => Ok(ClaudeCodeProvider { config }),
        None => app_data(&app).and_then(|data| provider_from_data(&data)),
    };
    Ok(match provider {
        Ok(provider) => provider.status().await,
        Err(message) => ClaudeCodeStatus::Error { message },
    })
}

/// Fail closed on unknown auth shapes; account identifiers are deliberately ignored.
fn parse_auth_status(value: &Value, version: String) -> Result<ClaudeCodeStatus, String> {
    if ["loggedIn", "authenticated"].iter().any(|key| {
        value
            .get(*key)
            .map(|value| !value.is_boolean())
            .unwrap_or(false)
    }) {
        return Err(
            "Could not determine Claude Code sign-in status. Update Claude Code and check again"
                .into(),
        );
    }
    let flags: Vec<bool> = ["loggedIn", "authenticated"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_bool))
        .collect();
    if flags.is_empty() {
        return Err(
            "Could not determine Claude Code sign-in status. Update Claude Code and check again"
                .into(),
        );
    }
    if flags.iter().any(|flag| !flag) {
        return Ok(ClaudeCodeStatus::NotSignedIn);
    }
    // Only subscription labels, never arbitrary strings that could contain an account ID.
    let plan = value
        .get("subscriptionType")
        .or_else(|| value.get("plan"))
        .and_then(Value::as_str)
        .and_then(|plan| match plan.to_ascii_lowercase().as_str() {
            "free" => Some("Free"),
            "pro" => Some("Pro"),
            "max" => Some("Max"),
            "team" => Some("Team"),
            "enterprise" => Some("Enterprise"),
            _ => None,
        })
        .map(str::to_string);
    Ok(ClaudeCodeStatus::Ready { version, plan })
}

fn parse_version(bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| GENERIC_ERROR)?;
    let version = text.split_whitespace().next().unwrap_or("");
    if version.len() > 64
        || version.split('.').count() != 3
        || !version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    {
        return Err("Could not read Claude Code version. Check Program path".into());
    }
    Ok(version.into())
}

fn map_error(reason: &str) -> String {
    let reason = reason.to_ascii_lowercase();
    if reason.contains("rate limit")
        || reason.contains("usage limit")
        || reason.contains("rate_limit")
    {
        "Claude Code plan limit reached. Wait for your limit to reset or check your Claude plan"
            .into()
    } else if (reason.contains("authenticat")
        || reason.contains("login")
        || reason.contains("sign-in")
        || reason.contains("oauth"))
        && reason.contains("expired")
    {
        "Claude Code sign-in expired. Open a terminal and run: claude auth login".into()
    } else if reason.contains("authenticat")
        || reason.contains("login")
        || reason.contains("not logged in")
        || reason.contains("not signed in")
    {
        LOGIN.into()
    } else if reason.contains("model")
        && (reason.contains("invalid") || reason.contains("not found"))
    {
        "Claude Code returned an error: The selected model is unavailable".into()
    } else {
        GENERIC_ERROR.into()
    }
}

fn parse_result(bytes: &[u8]) -> Result<String, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| "Claude Code returned an error: Invalid response")?;
    let result = match &value {
        Value::Array(messages) => messages
            .iter()
            .rev()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("result")),
        Value::Object(_) if value.get("type").and_then(Value::as_str) == Some("result") => {
            Some(&value)
        }
        _ => None,
    }
    .ok_or("Claude Code returned an empty result")?;
    if result.get("is_error").and_then(Value::as_bool) == Some(true)
        || result.get("subtype").and_then(Value::as_str) != Some("success")
    {
        let reason = result
            .get("result")
            .and_then(Value::as_str)
            .or_else(|| result.get("error").and_then(Value::as_str))
            .unwrap_or("");
        return Err(map_error(reason));
    }
    result
        .get("result")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Claude Code returned an empty result".into())
}

fn generation_args(model: &str, system: &str, persistence: bool) -> Vec<String> {
    let mut args = vec![
        "-p",
        "--output-format",
        "json",
        "--model",
        model,
        "--tools",
        "",
        "--disable-slash-commands",
    ];
    if persistence {
        args.push("--no-session-persistence");
    }
    args.extend([
        "--strict-mcp-config",
        "--permission-mode",
        "dontAsk",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--system-prompt",
        system,
    ]);
    args.into_iter().map(str::to_string).collect()
}

fn unsupported_persistence(stderr: &str) -> bool {
    let text = stderr.to_ascii_lowercase();
    text.contains("--no-session-persistence")
        && (text.contains("unknown option") || text.contains("unrecognized option"))
}

fn secret_environment_key(key: &OsStr) -> bool {
    let name = key.to_string_lossy().to_ascii_uppercase();
    matches!(
        name.as_str(),
        "ANTHROPIC_API_KEY"
            | "ANTHROPIC_AUTH_TOKEN"
            | "ANTHROPIC_BASE_URL"
            | "CLAUDE_CODE_OAUTH_TOKEN"
            | "CLAUDECODE"
    ) || name.starts_with("CLAUDE_CODE_")
}
fn scrub_environment(command: &mut Command) {
    let mut keys: Vec<_> = std::env::vars_os()
        .map(|(key, _)| key)
        .chain(
            command
                .as_std()
                .get_envs()
                .map(|(key, _)| key.to_os_string()),
        )
        .filter(|key| secret_environment_key(key))
        .collect();
    keys.extend(
        [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDECODE",
        ]
        .iter()
        .map(|key| std::ffi::OsString::from(*key)),
    );
    for key in keys {
        command.env_remove(key);
    }
}

fn resolve_program(config: &ClaudeCodeConfig) -> Option<PathBuf> {
    let name = config
        .program_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .unwrap_or("claude");
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() > 1 {
        return program_candidates(path)
            .into_iter()
            .find(|path| path.is_file())
            .and_then(|path| std::fs::canonicalize(path).ok());
    }
    if let Some(found) = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| program_candidates(&dir.join(name)))
        .find(|path| path.is_file())
    {
        return std::fs::canonicalize(found).ok();
    }
    if config
        .program_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .is_none()
    {
        return std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .map(|home| home.join(".local/bin/claude.exe"))
            .filter(|path| path.is_file());
    }
    None
}
fn program_candidates(path: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // Restrict PATHEXT to executable forms we can safely launch; retain its order.
        if path.extension().is_some() {
            return vec![path.to_path_buf()];
        }
        let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        return extensions
            .split(';')
            .filter(|ext| {
                [".exe", ".com", ".cmd", ".bat"].contains(&ext.to_ascii_lowercase().as_str())
            })
            .map(|ext| path.with_extension(ext.trim_start_matches('.')))
            .collect();
    }
    #[cfg(not(windows))]
    vec![path.to_path_buf()]
}

fn program_command(program: &Path, args: &[String]) -> Result<Command, String> {
    #[cfg(windows)]
    if program
        .extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
    {
        let program = program
            .to_str()
            .ok_or("Claude Code Program path is not supported")?;
        // cmd cannot use the verbatim prefix produced by canonicalize().
        let program = program
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!(r"\\{rest}"))
            .unwrap_or_else(|| program.strip_prefix(r"\\?\").unwrap_or(program).to_string());
        if program
            .chars()
            .any(|ch| ['"', '\r', '\n', '%', '!'].contains(&ch))
        {
            return Err(
                "Choose a Claude Code Program path without shell expansion characters".into(),
            );
        }
        let interpreter = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .ok_or("Windows command interpreter unavailable")?
            // cmd misparses its own command line when started through a path
            // that contains a forward slash.
            .join("System32")
            .join("cmd.exe");
        let encoded = args
            .iter()
            .map(|arg| batch_argument(arg))
            .collect::<Result<Vec<_>, _>>()?;
        // cmd has a smaller command-line limit than native executables. Never let it
        // truncate an encoded argument or the enclosing quote pair.
        let units = program.encode_utf16().count()
            + encoded
                .iter()
                .map(|arg| arg.encode_utf16().count() + 1)
                .sum::<usize>()
            + 64;
        if units > 8000 {
            return Err("Claude Code system instructions are too long for a Windows command shim. Choose native claude.exe in Program path or shorten the instructions".into());
        }
        let mut command = Command::new(interpreter);
        command.args(["/d", "/s", "/e:on", "/v:off", "/c"]);
        // Each raw argument is separately encoded for cmd, never interpolated as shell syntax.
        // /s removes exactly the outer pair. The final space is inside that pair.
        command.raw_arg(format!("\"\"{program}\""));
        for arg in encoded {
            command.raw_arg(arg);
        }
        command.raw_arg("\"");
        return Ok(command);
    }
    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

#[cfg(windows)]
fn batch_argument(arg: &str) -> Result<String, String> {
    if arg.contains('\0') {
        return Err("Claude Code received an unsupported argument".into());
    }
    // cmd cannot carry line breaks in a batch argument. Keep system instructions as one line.
    // Percent expansion is neutralized; delayed expansion is off. Quotes are doubled for cmd
    // and its CRT consumer, including backslashes immediately before a quote.
    let arg = arg.replace(|ch| ch == '\r' || ch == '\n', " ");
    let mut encoded = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
        } else {
            if ch == '"' {
                encoded.extend(std::iter::repeat('\\').take(slashes));
                encoded.push('"');
            }
            if ch == '%' {
                encoded.push_str("%%cd:~,");
            }
            slashes = 0;
        }
        encoded.push(ch);
    }
    encoded.extend(std::iter::repeat('\\').take(slashes));
    encoded.push('"');
    Ok(encoded)
}

impl ClaudeCodeProvider {
    pub async fn status(&self) -> ClaudeCodeStatus {
        let Some(program) = resolve_program(&self.config) else {
            return ClaudeCodeStatus::NotInstalled;
        };
        let result = async {
            let version = self
                .probe(&program, &["--version"], Duration::from_secs(10))
                .await?;
            if !version.status.success() {
                return Err(map_error(&String::from_utf8_lossy(&version.stderr)));
            }
            let version = parse_version(&version.stdout)?;
            let auth = self
                .probe(
                    &program,
                    &["auth", "status", "--json"],
                    Duration::from_secs(15),
                )
                .await?;
            let value = serde_json::from_slice(&auth.stdout)
                .map_err(|_| map_error(&String::from_utf8_lossy(&auth.stderr)))?;
            let status = parse_auth_status(&value, version)?;
            if matches!(status, ClaudeCodeStatus::Ready { .. }) && !auth.status.success() {
                return Err(map_error(&String::from_utf8_lossy(&auth.stderr)));
            }
            Ok(status)
        }
        .await;
        result.unwrap_or_else(|message| ClaudeCodeStatus::Error { message })
    }
    async fn probe(
        &self,
        program: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<ProcessOutput, String> {
        let command = program_command(
            program,
            &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
        )?;
        let scratch = scratch()?;
        run_process(
            command,
            scratch,
            String::new(),
            CancellationToken::new(),
            tokio::time::Instant::now() + timeout,
        )
        .await
    }
    pub(crate) async fn generate(
        &self,
        model: &str,
        system: &str,
        prompt: &str,
        token: &CancellationToken,
    ) -> Result<String, String> {
        let model = normalize_model(model)?;
        if token.is_cancelled() {
            return Err("Claude Code request cancelled".into());
        }
        let program = resolve_program(&self.config).ok_or(NOT_INSTALLED)?;
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        for persistence in [true, false] {
            let command = program_command(&program, &generation_args(model, system, persistence))?;
            let output =
                run_process(command, scratch()?, prompt.into(), token.clone(), deadline).await?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            if persistence && unsupported_persistence(&stderr) {
                continue;
            }
            if !output.status.success() {
                // Some versions return a structured auth/limit error even on nonzero exit.
                if let Err(error) = parse_result(&output.stdout) {
                    if error != GENERIC_ERROR
                        && !error.contains("Invalid response")
                        && !error.contains("empty result")
                    {
                        return Err(error);
                    }
                }
                return Err(map_error(&stderr));
            }
            return parse_result(&output.stdout);
        }
        Err(GENERIC_ERROR.into())
    }
}

fn scratch() -> Result<tempfile::TempDir, String> {
    tempfile::Builder::new()
        .prefix("clawscribe-claude-")
        .tempdir()
        .map_err(|_| "Could not prepare Claude Code working directory".into())
}
#[cfg_attr(test, derive(Debug))]
struct ProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// The detached supervisor retains the temp directory, job and child until wait() completes,
/// even if its caller disappears. No reader task or unbounded output buffer survives it.
async fn run_process(
    mut command: Command,
    scratch: tempfile::TempDir,
    input: String,
    token: CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<ProcessOutput, String> {
    let owned = token.child_token();
    let guard = owned.clone().drop_guard();
    let task = tokio::spawn(async move {
        if owned.is_cancelled() {
            return Err("Claude Code request cancelled".into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Claude Code timed out. Try again".into());
        }
        scrub_environment(&mut command);
        command
            .current_dir(scratch.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let job = job::Job::new()?;
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                NOT_INSTALLED.to_string()
            } else {
                "Could not start Claude Code. Check Program path".into()
            }
        })?;
        let pid = match child.id() {
            Some(pid) => pid,
            None => {
                let _ = child.start_kill();
                drop(job);
                let _ = child.wait().await;
                return Err(GENERIC_ERROR.into());
            }
        };
        let assigned = job.assign(pid);
        let result = async {
            assigned?;
            let mut stdin = child.stdin.take().ok_or(GENERIC_ERROR)?;
            let stdout = child.stdout.take().ok_or(GENERIC_ERROR)?;
            let stderr = child.stderr.take().ok_or(GENERIC_ERROR)?;
            let exchange = async {
                let write = async {
                    if let Err(error) = stdin.write_all(input.as_bytes()).await {
                        // An auth/option failure may close stdin early. Preserve its sanitized
                        // status instead of replacing it with a generic broken-pipe error.
                        if error.kind() != std::io::ErrorKind::BrokenPipe { return Err(GENERIC_ERROR.to_string()); }
                    }
                    drop(stdin);
                    Ok::<_, String>(())
                };
                let (_, stdout, stderr, status) = tokio::try_join!(write, read_bounded(stdout, OUTPUT_LIMIT), read_bounded(stderr, STDERR_LIMIT), async { child.wait().await.map_err(|_| GENERIC_ERROR.to_string()) })?;
                Ok(ProcessOutput { status, stdout, stderr })
            };
            tokio::select! { biased;
                _ = owned.cancelled() => Err("Claude Code request cancelled".into()),
                _ = tokio::time::sleep_until(deadline) => Err("Claude Code timed out. Try again".into()),
                result = exchange => result,
            }
        }.await;
        if result.is_err() {
            let _ = child.start_kill();
        }
        drop(job); // Kills descendants before temp directory removal, on success too.
        let _ = child.wait().await;
        #[cfg(test)]
        REAPED_PIDS.lock().unwrap().push(pid);
        drop(scratch);
        result
    });
    let result = task.await.map_err(|_| GENERIC_ERROR.to_string())?;
    guard.disarm();
    result
}
async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| GENERIC_ERROR)?;
    if bytes.len() > limit {
        return Err("Claude Code returned an error: Response exceeded the size limit".into());
    }
    Ok(bytes)
}

#[cfg(windows)]
mod job {
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                    SetInformationJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                },
                Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
            },
        },
    };
    pub struct Job(HANDLE);
    // Exclusively owned job handle; Windows handles may be used by the supervisor thread.
    unsafe impl Send for Job {}
    impl Job {
        pub fn new() -> Result<Self, String> {
            let job = Self(
                unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                    .map_err(|_| "Could not supervise Claude Code")?,
            );
            let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                ..Default::default()
            };
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32,
                )
            }
            .map_err(|_| "Could not supervise Claude Code")?;
            Ok(job)
        }
        pub fn assign(&self, pid: u32) -> Result<(), String> {
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
                .map_err(|_| "Could not supervise Claude Code")?;
            let result = unsafe { AssignProcessToJobObject(self.0, process) };
            let _ = unsafe { CloseHandle(process) };
            result.map_err(|_| "Could not supervise Claude Code".into())
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}
#[cfg(not(windows))]
mod job {
    pub struct Job;
    impl Job {
        pub fn new() -> Result<Self, String> {
            Ok(Self)
        }
        pub fn assign(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
    }
}

#[cfg(test)]
static REAPED_PIDS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
#[cfg(test)]
#[path = "claude_code_provider_tests.rs"]
mod tests;
