use super::*;
use serde_json::json;

#[test]
fn auth_requires_an_explicit_boolean_and_never_returns_account_data() {
    for value in [
        json!({"loggedIn": true, "subscriptionType": "max", "email": "private@example.invalid"}),
        json!({"authenticated": true, "plan": "max"}),
    ] {
        assert!(
            matches!(parse_auth_status(&value, "2.1.0".into()).unwrap(), ClaudeCodeStatus::Ready { plan: Some(plan), .. } if plan == "Max")
        );
    }
    for value in [
        json!({}),
        json!({"loggedIn": "true"}),
        json!({"loggedIn": "false", "authenticated": true}),
        json!({"loggedIn": false}),
        json!({"loggedIn": false, "authenticated": true}),
        json!({"email": "private@example.invalid"}),
    ] {
        assert!(!matches!(
            parse_auth_status(&value, "2.1.0".into()),
            Ok(ClaudeCodeStatus::Ready { .. })
        ));
    }
    let status = parse_auth_status(
        &json!({"loggedIn":true,"plan":"private@example.invalid"}),
        "2.1.0".into(),
    )
    .unwrap();
    assert!(!serde_json::to_string(&status).unwrap().contains("private"));
}

#[test]
fn result_parser_uses_the_last_result_and_rejects_failed_or_empty_results() {
    assert_eq!(
        parse_result(br#"{"type":"result","subtype":"success","result":"hello"}"#).unwrap(),
        "hello"
    );
    assert_eq!(parse_result(br#"[{"type":"result","subtype":"success","result":"first"},{"type":"assistant","result":"ignore"},{"type":"result","subtype":"success","result":"last"}]"#).unwrap(), "last");
    for raw in [
        r#"{"type":"result","subtype":"error","result":"private text"}"#,
        r#"{"type":"result","subtype":"success","is_error":true,"result":"private text"}"#,
        r#"{"type":"result","subtype":"success","result":" "}"#,
        "not json",
    ] {
        let error = parse_result(raw.as_bytes()).unwrap_err();
        assert!(!error.contains("private text"));
    }
}

#[test]
fn errors_are_fixed_actionable_messages_never_raw_diagnostics() {
    assert!(map_error("usage limit reached").contains("plan limit"));
    assert!(map_error("rate limit").contains("plan limit"));
    assert!(map_error("authentication expired").contains("expired"));
    assert!(map_error("Please login").contains("claude auth login"));
    assert_eq!(
        map_error("secret transcript or token"),
        "Claude Code returned an error"
    );
    assert!(unsupported_persistence(
        "error: unknown option '--no-session-persistence'"
    ));
    assert!(!unsupported_persistence("error: unknown option '--tools'"));
}

#[test]
fn generation_arguments_disable_tools_hooks_and_persistence_with_stdin_only_prompt() {
    assert_eq!(
        generation_args("sonnet", "system", true),
        [
            "-p",
            "--output-format",
            "json",
            "--model",
            "sonnet",
            "--tools",
            "",
            "--disable-slash-commands",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--permission-mode",
            "dontAsk",
            "--settings",
            r#"{"disableAllHooks":true}"#,
            "--system-prompt",
            "system"
        ]
    );
    assert!(!generation_args("sonnet", "system", true)
        .join(" ")
        .contains("private user prompt"));
    assert!(!generation_args("sonnet", "system", false)
        .iter()
        .any(|arg| arg == "--no-session-persistence"));
}

#[test]
fn environment_scrubbing_covers_inherited_and_explicit_mixed_case_variables() {
    let mut command = Command::new("unused");
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "claude_code_custom",
    ] {
        command.env(key, "synthetic-secret");
    }
    command
        .env("PATH", "keep-path")
        .env("USERPROFILE", "keep-profile")
        .env("APPDATA", "keep-appdata");
    scrub_environment(&mut command);
    for (key, value) in command.as_std().get_envs() {
        if secret_environment_key(key) {
            assert!(value.is_none());
        }
    }
    for (name, expected) in [
        ("PATH", "keep-path"),
        ("USERPROFILE", "keep-profile"),
        ("APPDATA", "keep-appdata"),
    ] {
        assert!(command
            .as_std()
            .get_envs()
            .any(|(key, value)| key == name && value == Some(std::ffi::OsStr::new(expected))));
    }
}

// These fixtures only execute Node from the installed frontend toolchain, never a real CLI.
struct Fixture {
    dir: tempfile::TempDir,
    program: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("claude fake with spaces ")
            .tempdir()
            .unwrap();
        let node = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join(if cfg!(windows) { "node.exe" } else { "node" }))
            .find(|path| path.is_file())
            .expect("Node is required for frontend tests");
        let script = dir.path().join("fake.cjs");
        let source = include_str!("claude_code_fake.cjs").replace("FIXTURE_MODE", mode);
        std::fs::write(&script, source).unwrap();
        #[cfg(windows)]
        let program = {
            let program = dir.path().join("claude.cmd");
            std::fs::write(
                &program,
                format!("@\"{}\" \"%~dp0fake.cjs\" %*\r\n", node.display()),
            )
            .unwrap();
            program
        };
        #[cfg(not(windows))]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let program = dir.path().join("claude");
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
                    node.display(),
                    script.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
            program
        };
        Self { dir, program }
    }
    fn provider(&self) -> ClaudeCodeProvider {
        ClaudeCodeProvider {
            config: ClaudeCodeConfig {
                program_path: Some(self.program.to_string_lossy().into()),
            },
        }
    }
    fn observed(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.dir.path().join("observed.json")).unwrap())
            .unwrap()
    }
}

#[tokio::test]
async fn fake_status_covers_installed_signed_out_unknown_and_error() {
    for (mode, expected) in [
        ("ready", "ready"),
        ("signed-out", "not_signed_in"),
        ("unknown-auth", "error"),
        ("bad-version", "error"),
    ] {
        let fixture = Fixture::new(mode);
        let status = fixture.provider().status().await;
        assert_eq!(serde_json::to_value(status).unwrap()["state"], expected);
    }
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCodeProvider {
        config: ClaudeCodeConfig {
            program_path: Some(dir.path().join("absent").to_string_lossy().into()),
        },
    };
    assert!(matches!(
        provider.status().await,
        ClaudeCodeStatus::NotInstalled
    ));
}

#[tokio::test]
async fn fake_generation_round_trips_shim_arguments_stdin_scrubbed_env_and_empty_cwd() {
    let fixture = Fixture::new("verify");
    let provider = fixture.provider();
    let system = r#"system with "quotes" & | < > ^ %PATH% !literal! \"#;
    assert_eq!(
        provider
            .generate(
                "sonnet",
                system,
                "synthetic private prompt",
                &CancellationToken::new()
            )
            .await
            .unwrap(),
        "safe answer"
    );
    let observed = fixture.observed();
    assert_eq!(observed["input"], "synthetic private prompt");
    assert_eq!(observed["args"][6], "");
    assert_eq!(
        observed["args"],
        json!(generation_args("sonnet", system, true))
    );
    assert_eq!(observed["initialFiles"], json!([]));
    assert_eq!(observed["secretKeys"], json!([]));
    assert!(
        !Path::new(observed["cwd"].as_str().unwrap()).exists(),
        "scratch directory is removed after reaping"
    );
}

#[tokio::test]
async fn unknown_option_retries_only_once_and_array_and_error_results_are_supported() {
    for mode in ["array", "old-cli"] {
        let fixture = Fixture::new(mode);
        assert_eq!(
            fixture
                .provider()
                .generate("haiku", "system", "synthetic", &CancellationToken::new())
                .await
                .unwrap(),
            "safe answer"
        );
        if mode == "old-cli" {
            assert_eq!(fixture.observed()["attempts"], 2);
        }
    }
    let fixture = Fixture::new("always-unknown");
    assert!(fixture
        .provider()
        .generate("sonnet", "system", "synthetic", &CancellationToken::new())
        .await
        .is_err());
    assert_eq!(fixture.observed()["attempts"], 2);
    let fixture = Fixture::new("error");
    assert!(fixture
        .provider()
        .generate("opus", "system", "synthetic", &CancellationToken::new())
        .await
        .unwrap_err()
        .contains("plan limit"));
}

#[tokio::test]
async fn timeout_and_cancellation_reap_the_owned_child() {
    for cancel in [false, true] {
        let fixture = Fixture::new("hang");
        let token = CancellationToken::new();
        let trigger = token.clone();
        let command = program_command(&fixture.program, &["-p".into()]).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let operation = run_process(
            command,
            scratch,
            "synthetic".into(),
            token,
            tokio::time::Instant::now() + Duration::from_secs(if cancel { 10 } else { 3 }),
        );
        let signal = async {
            if cancel {
                wait_for_observed(&fixture).await;
                trigger.cancel();
            }
        };
        let (result, ()) = tokio::join!(operation, signal);
        assert!(result
            .unwrap_err()
            .contains(if cancel { "cancelled" } else { "timed out" }));
        // The supervisor records only owned PIDs after wait(), never process names.
        let observed = fixture.observed();
        assert!(REAPED_PIDS
            .lock()
            .unwrap()
            .contains(&(observed["parentPid"].as_u64().unwrap() as u32)));
        #[cfg(windows)]
        assert_process_exited(observed["pid"].as_u64().unwrap() as u32);
    }
}

async fn wait_for_observed(fixture: &Fixture) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.dir.path().join("observed.json").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the fake process must start before cancellation");
}

#[cfg(windows)]
fn assert_process_exited(pid: u32) {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    if let Ok(process) = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        let mut code = 259; // STILL_ACTIVE
        let result = unsafe { GetExitCodeProcess(process, &mut code) };
        let _ = unsafe { CloseHandle(process) };
        assert!(result.is_ok());
        assert_ne!(code, 259, "the owned descendant must have exited");
    }
}

#[tokio::test]
async fn caller_drop_cancels_and_reaps_before_removing_scratch() {
    let fixture = Fixture::new("hang");
    let command = program_command(&fixture.program, &["-p".into()]).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let scratch_path = scratch.path().to_path_buf();
    let task = tokio::spawn(run_process(
        command,
        scratch,
        "synthetic".into(),
        CancellationToken::new(),
        tokio::time::Instant::now() + Duration::from_secs(10),
    ));
    wait_for_observed(&fixture).await;
    let observed = fixture.observed();
    task.abort(); // Drops only our waiter, never the supervisor/resource owner.
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while scratch_path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(REAPED_PIDS
        .lock()
        .unwrap()
        .contains(&(observed["parentPid"].as_u64().unwrap() as u32)));
    #[cfg(windows)]
    assert_process_exited(observed["pid"].as_u64().unwrap() as u32);
}

#[test]
fn configured_model_and_program_path_are_independent_of_api_credentials() {
    assert_eq!(normalize_model("").unwrap(), "sonnet");
    for model in ["sonnet", "opus", "haiku"] {
        assert_eq!(normalize_model(model).unwrap(), model);
    }
    assert!(normalize_model("--other-option").is_err());
    assert_eq!(
        super::super::llm_client::LLMProvider::from_str("claude-code").unwrap(),
        super::super::llm_client::LLMProvider::ClaudeCode
    );
    let value = serde_json::to_value(ClaudeCodeConfig::default()).unwrap();
    assert_eq!(value, json!({"programPath":null}));
    assert!(
        serde_json::from_value::<ClaudeCodeConfig>(json!({"apiKey":"synthetic-secret"})).is_err()
    );
}

#[cfg(windows)]
#[test]
fn command_shims_reject_truncation_and_normalize_system_line_breaks() {
    assert_eq!(
        batch_argument("first\r\nsecond").unwrap(),
        "\"first  second\""
    );
    assert!(program_command(
        Path::new("claude.cmd"),
        &generation_args("sonnet", &"x".repeat(9000), true)
    )
    .err()
    .unwrap()
    .contains("too long"));
    assert!(program_command(Path::new("percent%path/claude.cmd"), &["--version".into()]).is_err());
}

#[tokio::test]
async fn generic_summary_dispatch_never_forwards_the_supplied_api_key() {
    let fixture = Fixture::new("verify");
    let data = fixture.dir.path().to_path_buf();
    std::fs::write(
        config_path(&data),
        serde_json::to_vec(&fixture.provider().config).unwrap(),
    )
    .unwrap();
    let text = super::super::llm_client::generate_summary(
        &reqwest::Client::new(),
        &super::super::llm_client::LLMProvider::ClaudeCode,
        "sonnet",
        "synthetic-stored-key",
        "system",
        "synthetic prompt",
        None,
        None,
        false,
        None,
        None,
        None,
        Some(&data),
        Some(&CancellationToken::new()),
    )
    .await
    .unwrap();
    assert_eq!(text, "safe answer");
    let observed = fixture.observed();
    assert_eq!(observed["secretKeys"], json!([]));
    assert_eq!(observed["leakedStoredKey"], false);
    assert!(!observed.to_string().contains("synthetic-stored-key"));
}

#[tokio::test]
async fn subprocess_environment_removes_explicit_secrets_but_preserves_other_variables() {
    let fixture = Fixture::new("verify");
    let mut command = program_command(&fixture.program, &["-p".into()]).unwrap();
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "claude_code_custom",
    ] {
        command.env(name, "synthetic-secret");
    }
    command.env("CLAWSCRIBE_TEST_PRESERVED", "preserved");
    run_process(
        command,
        tempfile::tempdir().unwrap(),
        "synthetic".into(),
        CancellationToken::new(),
        tokio::time::Instant::now() + Duration::from_secs(10),
    )
    .await
    .unwrap();
    let observed = fixture.observed();
    assert_eq!(observed["secretKeys"], json!([]));
    assert_eq!(observed["preservedTestVariable"], "preserved");
}

#[tokio::test]
async fn configured_chat_dispatch_needs_no_credential_table_and_reports_actual_provider() {
    let fixture = Fixture::new("verify");
    let data = fixture.dir.path().to_path_buf();
    std::fs::write(
        config_path(&data),
        serde_json::to_vec(&fixture.provider().config).unwrap(),
    )
    .unwrap();
    // No settings/credential tables: any accidental key lookup makes resolution fail.
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    let token = CancellationToken::new();
    let resolved = super::super::llm_client::resolve_configured_text(
        &pool,
        super::super::llm_client::TextEnvironment::local(data),
        "claude-code",
        "",
        tokio::time::Instant::now(),
        &token,
    )
    .await
    .unwrap();
    assert_eq!(resolved.model, "sonnet");
    assert_eq!(resolved.budget.context_tokens, 32_768);
    let reply = super::super::llm_client::dispatch_resolved_text(
        resolved,
        "system".into(),
        "synthetic prompt".into(),
        &token,
    )
    .await
    .unwrap();
    assert_eq!(reply.text, "safe answer");
    assert_eq!(reply.provider, "claude-code");
    assert_eq!(reply.model, "sonnet");
}

// Shapes observed from Claude Code 2.1.312: a signed-out one-shot run exits 1
// and still prints a result object whose subtype is "success".
#[test]
fn observed_signed_out_result_and_status_shapes_are_understood() {
    let expired = br#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate: OAuth session expired"}"#;
    let error = super::parse_result(expired).unwrap_err();
    assert!(error.contains("sign-in expired"), "{error}");
    let failed = br#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate"}"#;
    let error = super::parse_result(failed).unwrap_err();
    assert!(error.contains("claude auth login"), "{error}");
}
