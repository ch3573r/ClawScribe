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
fn result_parser_maps_only_explicit_errors_never_model_output() {
    for text in [
        "Please login",
        "OAuth session expired",
        "usage limit reached",
        "invalid model",
    ] {
        for is_error in [None, Some(false)] {
            let mut value = json!({"type":"result", "subtype":"success", "result":text});
            if let Some(flag) = is_error {
                value["is_error"] = json!(flag);
            }
            assert_eq!(
                parse_result(&serde_json::to_vec(&value).unwrap()).unwrap(),
                text
            );
            value["subtype"] = json!("error");
            assert_eq!(
                parse_result(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
                GENERIC_ERROR
            );
        }
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
fn generation_arguments_contain_only_fixed_options_model_and_system_file() {
    assert_eq!(
        generation_args("sonnet", "scratch/system-prompt.txt", true),
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
            "--system-prompt-file",
            "scratch/system-prompt.txt"
        ]
    );
    assert!(
        !generation_args("sonnet", "scratch/system-prompt.txt", true)
            .join(" ")
            .contains("private user prompt")
    );
    assert!(
        !generation_args("sonnet", "scratch/system-prompt.txt", false)
            .iter()
            .any(|arg| arg == "--no-session-persistence")
    );
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
            .prefix("claude fake with spaces Grüße ")
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
}

#[tokio::test]
async fn fake_generation_preserves_system_file_bytes_and_keeps_both_prompts_off_arguments() {
    let fixture = Fixture::new("verify");
    let provider = fixture.provider();
    let system = "first\r\nsecond\n\"quotes\" %PATH% & | < > ^ !literal!\tGrüße 日本語 🦀 \\";
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
    let system_file = Path::new(observed["cwd"].as_str().unwrap()).join("system-prompt.txt");
    assert_eq!(
        observed["args"],
        json!(generation_args(
            "sonnet",
            system_file.to_str().unwrap(),
            true
        ))
    );
    assert_eq!(observed["systemBytes"], json!(system.as_bytes()));
    let args = observed["args"].to_string();
    assert!(!args.contains("synthetic private prompt"));
    assert!(!args.contains("first") && !args.contains("quotes") && !args.contains("日本語"));
    assert_eq!(observed["initialFiles"], json!(["system-prompt.txt"]));
    assert_eq!(observed["secretKeys"], json!([]));
    assert!(
        !Path::new(observed["cwd"].as_str().unwrap()).exists(),
        "scratch directory is removed after reaping"
    );
}

#[tokio::test]
async fn one_hundred_thousand_character_system_prompt_works_through_the_shim() {
    let fixture = Fixture::new("verify");
    let system = "x".repeat(100_000);
    assert_eq!(
        fixture
            .provider()
            .generate("sonnet", &system, "synthetic", &CancellationToken::new())
            .await
            .unwrap(),
        "safe answer"
    );
    let observed = fixture.observed();
    assert_eq!(observed["systemBytes"], json!(system.as_bytes()));
    assert!(observed["args"]
        .as_array()
        .unwrap()
        .iter()
        .all(|arg| !arg.as_str().unwrap().contains(system.as_str())));
}

#[tokio::test]
async fn system_file_path_with_spaces_and_unicode_survives_the_shim() {
    let fixture = Fixture::new("verify");
    let scratch = tempfile::Builder::new()
        .prefix("claude scratch Grüße ")
        .tempdir()
        .unwrap();
    let system_file = scratch.path().join("system-prompt.txt");
    std::fs::write(&system_file, "Grüße\n\t日本語").unwrap();
    let command = program_command(
        &fixture.program,
        &generation_args("sonnet", system_file.to_str().unwrap(), true),
    )
    .unwrap();
    let output = run_process(
        command,
        scratch,
        "synthetic".into(),
        CancellationToken::new(),
        tokio::time::Instant::now() + Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(parse_result(&output.stdout).unwrap(), "safe answer");
    assert_eq!(
        fixture.observed()["systemBytes"],
        json!("Grüße\n\t日本語".as_bytes())
    );
}

#[tokio::test]
async fn relative_missing_directory_and_unsupported_program_paths_are_status_errors() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("not-a-program.txt");
    std::fs::write(&text, "synthetic").unwrap();
    // status() is also the path used for the unsaved config passed by the UI.
    for path in [
        PathBuf::from("relative/program.cmd"),
        dir.path().join("missing.exe"),
        dir.path().to_path_buf(),
        text,
    ] {
        let provider = ClaudeCodeProvider {
            config: ClaudeCodeConfig {
                program_path: Some(path.to_str().unwrap().into()),
            },
        };
        assert!(
            matches!(provider.status().await, ClaudeCodeStatus::Error { message } if message == INVALID_PROGRAM)
        );
        assert_eq!(
            provider
                .generate("sonnet", "system", "synthetic", &CancellationToken::new())
                .await
                .unwrap_err(),
            INVALID_PROGRAM
        );
    }
    let relative_dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let relative_file = relative_dir.path().join(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });
    std::fs::write(&relative_file, "synthetic").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&relative_file, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let relative = relative_file
        .strip_prefix(std::env::current_dir().unwrap())
        .unwrap();
    assert!(resolve_program(&ClaudeCodeConfig {
        program_path: Some(relative.to_str().unwrap().into())
    })
    .is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn unix_nonexecutable_program_override_is_a_status_error() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("claude");
    std::fs::write(&program, "synthetic").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o600)).unwrap();
    let provider = ClaudeCodeProvider {
        config: ClaudeCodeConfig {
            program_path: Some(program.to_str().unwrap().into()),
        },
    };
    assert!(
        matches!(provider.status().await, ClaudeCodeStatus::Error { message } if message == INVALID_PROGRAM)
    );
}

#[tokio::test]
async fn finished_child_returns_after_drain_grace_when_descendant_holds_both_pipes() {
    let fixture = Fixture::new("hold-pipes");
    let command = program_command(&fixture.program, &["-p".into()]).unwrap();
    let started = tokio::time::Instant::now();
    let output = run_process(
        command,
        scratch().unwrap(),
        "synthetic".into(),
        CancellationToken::new(),
        started + Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(parse_result(&output.stdout).unwrap(), "safe answer");
    assert_eq!(output.stderr.as_slice(), b"synthetic stderr\n");
    assert!(output.status.success());
    // No lower bound: on Windows the job object ends the descendant as soon as
    // the child exits, so the pipes may close before the grace period.
    assert!(started.elapsed() < Duration::from_secs(8));
    let observed = fixture.observed();
    assert!(observed["descendantPid"].as_u64().is_some());
    assert!(REAPED_PIDS
        .lock()
        .unwrap()
        .contains(&(observed["parentPid"].as_u64().unwrap() as u32)));
    assert!(!Path::new(observed["cwd"].as_str().unwrap()).exists());
}

#[tokio::test]
async fn bounded_reader_retains_partial_bytes_when_drain_is_cancelled() {
    let (mut writer, reader) = tokio::io::duplex(32);
    writer.write_all(b"synthetic").await.unwrap();
    let mut bytes = Vec::new();
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        read_bounded(reader, 32, &mut bytes)
    )
    .await
    .is_err());
    assert_eq!(bytes.as_slice(), b"synthetic");
}

#[tokio::test]
async fn bounded_reader_rejects_excess_output_without_growing_past_the_limit() {
    for (data, succeeds) in [(b"abcd".as_slice(), true), (b"abcde".as_slice(), false)] {
        let mut bytes = Vec::new();
        assert_eq!(read_bounded(data, 4, &mut bytes).await.is_ok(), succeeds);
        assert!(bytes.len() <= 4);
    }
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
    let fixture = Fixture::new("expired");
    assert!(fixture
        .provider()
        .generate("sonnet", "system", "synthetic", &CancellationToken::new())
        .await
        .unwrap_err()
        .contains("sign-in expired"));
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
fn command_shims_reject_unsupported_path_characters_instead_of_rewriting_them() {
    for arg in ["first\r\nsecond", "path\0name"] {
        assert!(batch_argument(arg).is_err());
    }
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
    let api_budget = super::super::context_budget::resolve(
        &super::super::llm_client::LLMProvider::Claude,
        "claude-sonnet",
        None,
        None,
    );
    assert_eq!(resolved.budget.context_tokens, api_budget.context_tokens);
    assert_eq!(resolved.budget.output_tokens, api_budget.output_tokens);
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

static ENVIRONMENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Default)]
struct TestEnvironment(Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>);
impl TestEnvironment {
    fn set(&mut self, key: &str, value: &OsStr) {
        self.0.push((key.into(), std::env::var_os(key)));
        std::env::set_var(key, value);
    }
}
impl Drop for TestEnvironment {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[tokio::test]
async fn child_environment_excludes_secrets_inherited_from_the_test_process() {
    let _lock = ENVIRONMENT_LOCK.lock().await;
    let fixture = Fixture::new("verify");
    let mut environment = TestEnvironment::default();
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "claude_code_custom",
        "aNtHrOpIc_ApI_kEy",
    ] {
        environment.set(key, OsStr::new("synthetic-secret"));
    }
    environment.set("CLAWSCRIBE_TEST_PRESERVED", OsStr::new("preserved"));
    assert_eq!(
        fixture
            .provider()
            .generate("sonnet", "system", "synthetic", &CancellationToken::new())
            .await
            .unwrap(),
        "safe answer"
    );
    let observed = fixture.observed();
    assert_eq!(observed["secretKeys"], json!([]));
    assert_eq!(observed["preservedTestVariable"], "preserved");
}

#[test]
fn network_and_device_program_paths_are_refused_in_every_slash_mix_without_touching_them() {
    for path in [
        r"\\server\share\claude.exe",
        r"\/server/share/claude.exe",
        r"/\server\share\claude.exe",
        r"\??\UNC\server\share\claude.exe",
        r"\\?\UNC\server\share\claude.exe",
        r"\\?\C:\claude.exe",
        r"\\.\pipe\claude",
        "//host/share/claude",
        r"C:claude.exe",
        r"claude.exe",
    ] {
        let error = resolve_program(&ClaudeCodeConfig {
            program_path: Some(path.to_string()),
        })
        .unwrap_err();
        assert_eq!(error, INVALID_PROGRAM, "{path}");
    }
}

#[test]
fn only_a_file_named_claude_is_accepted_as_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let name = if cfg!(windows) { "claude.exe" } else { "claude" };
    let good = dir.path().join(name);
    std::fs::write(&good, b"synthetic").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let accepted = resolve_program(&ClaudeCodeConfig { program_path: Some(good.to_string_lossy().into_owned()) });
    assert!(accepted.unwrap().is_some());
    for other in ["cmd.exe", "powershell.exe", "claude-helper.exe", "notclaude.exe", "claude.exe.txt", "sh"] {
        let path = dir.path().join(other);
        std::fs::write(&path, b"synthetic").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            resolve_program(&ClaudeCodeConfig { program_path: Some(path.to_string_lossy().into_owned()) }).unwrap_err(),
            INVALID_PROGRAM,
            "{other}"
        );
    }
    // A directory named claude, and a missing file, are not programs.
    let directory = dir.path().join("sub").join(name);
    std::fs::create_dir_all(&directory).unwrap();
    assert!(resolve_program(&ClaudeCodeConfig { program_path: Some(directory.to_string_lossy().into_owned()) }).is_err());
    assert!(resolve_program(&ClaudeCodeConfig { program_path: Some(dir.path().join("missing").join(name).to_string_lossy().into_owned()) }).is_err());
    #[cfg(windows)]
    {
        let shouted = dir.path().join("shout").join("CLAUDE.CMD");
        std::fs::create_dir_all(shouted.parent().unwrap()).unwrap();
        std::fs::write(&shouted, b"synthetic").unwrap();
        let accepted = resolve_program(&ClaudeCodeConfig { program_path: Some(shouted.to_string_lossy().into_owned()) });
        assert!(accepted.is_ok(), "the file name's case does not matter on Windows: {accepted:?}");
    }
}
