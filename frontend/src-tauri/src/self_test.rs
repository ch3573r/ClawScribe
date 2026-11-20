//! Headless acceptance entry point of the packaged executable.
//!
//! `ClawScribe.exe --self-test run <profile-folder>` exercises the shipped
//! binary against a synthetic profile and writes a JSON report;
//! `ClawScribe.exe --self-test verify <profile-folder>` then reopens the same
//! profile in a fresh process to prove persistence across restarts.
//!
//! It runs before any desktop plugin, single-instance guard or logger starts, so
//! it never opens a window, never forwards to a running copy and never reads the
//! user's real profile, credential store, settings, templates or providers. The
//! database is opened through the isolated path that skips the credential-store
//! step. The profile folder must be new or empty and must not overlap the
//! application's own data folders. The verify phase accepts only a folder that
//! the run phase marked, whose tree holds no link, and only state paths that stay
//! inside it. Everything the check creates lives inside that folder.
//!
//! Exit codes: 0 passed, 1 a check failed, 2 unusable command line, 3 the runtime
//! or the report could not be set up, 4 the profile folder was refused untouched.
//!
//! What it covers: the packaged sidecars, runtime libraries, bundled templates and
//! Codex runtime next to the executable, upgrade of a synthetic legacy database
//! through this build's embedded migrations (including the known line-ending
//! checksum drift),
//! interrupted-recording discovery and recovery through the packaged encoder,
//! decoding and validating fixture audio, persisting an imported meeting, and
//! a summary produced through the production provider path against a loopback
//! stub. What it does not cover: microphone or system-audio capture, speech
//! models and transcription quality, real providers, the graphical interface,
//! installer interaction, and long-running sessions.

use crate::api::TranscriptSegment;
use crate::audio::decoder;
use crate::audio::import;
use crate::audio::incremental_saver;
use crate::audio::interrupted;
use crate::audio::recording_state::{AudioChunk, DeviceType};
use crate::audio::transcription::queue::recording_audio_queue;
use crate::database::manager::DatabaseManager;
use crate::database::repositories::summary::SummaryProcessesRepository;
use crate::summary::llm_client::LLMProvider;
use crate::summary::processor::generate_meeting_summary;
use crate::summary::templates::{Template, TemplateSection};
use serde_json::{json, Value};
use sqlx::migrate::{Migration, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const EXIT_FAILED: i32 = 1;
const USAGE_EXIT: i32 = 2;
const EXIT_RUNTIME: i32 = 3;
const EXIT_REFUSED: i32 = 4;
const PROFILE_MARKER: &str = ".clawscribe-self-test";
const MARKER_HEADER: &str = "ClawScribe self-test profile. Safe to delete.";
const STATE_FILE: &str = "self-test-state.json";
const STATE_SCHEMA: i64 = 1;
const MAX_STATE_BYTES: u64 = 64 * 1024;
const MAX_PROFILE_ENTRIES: usize = 20_000;
/// The synthetic legacy database is built from every migration up to this one.
const LEGACY_SCHEMA_VERSION: i64 = 20251006000000;
/// A recent schema: everything before the calendar-link migration. Upgrading it
/// applies the newest migrations on top of seeded, non-secret rows.
#[cfg(test)]
const RECENT_SCHEMA_VERSION: i64 = 20261010000000;
/// Templates the installer places in `templates/` beside the executable. A unit
/// test keeps this list equal to the files in the source folder.
const BUNDLED_TEMPLATES: [(&str, &str); 6] = [
    (
        "daily_standup.json",
        include_str!("../templates/daily_standup.json"),
    ),
    (
        "project_sync.json",
        include_str!("../templates/project_sync.json"),
    ),
    (
        "psychatric_session.json",
        include_str!("../templates/psychatric_session.json"),
    ),
    (
        "retrospective.json",
        include_str!("../templates/retrospective.json"),
    ),
    (
        "sales_marketing_client_call.json",
        include_str!("../templates/sales_marketing_client_call.json"),
    ),
    (
        "standard_meeting.json",
        include_str!("../templates/standard_meeting.json"),
    ),
];
const LEGACY_MEETING_ID: &str = "synthetic-legacy-meeting";
const LEGACY_MEETING_TITLE: &str = "Synthetic legacy planning meeting";
const LEGACY_LINES: [(&str, &str); 2] = [
    ("synthetic-legacy-line-1", "The legacy budget was approved."),
    (
        "synthetic-legacy-line-2",
        "The legacy launch moved to spring.",
    ),
];
const SENTINEL: &str = "zebra-lantern-self-test";
const IMPORT_LINES: [&str; 2] = [
    "zebra-lantern-self-test opened the synthetic review.",
    "Everyone agreed to keep the synthetic fixtures small.",
];
const STUB_SUMMARY: &str = "## Summary\nThe loopback stub answered the packaged summary path.\n\n## Action items\n- Nobody: nothing real was decided.";
const STUB_MARKER: &str = "loopback stub answered";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Run,
    Verify,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Run => "run",
            Phase::Verify => "verify",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Not a self-test invocation: the application starts normally.
    Ignore,
    /// A self-test invocation whose command line is unusable.
    Usage,
    Execute(Phase, PathBuf),
}

/// `--self-test <run|verify> <absolute profile folder>`, nothing else.
fn parse_command<I: IntoIterator<Item = OsString>>(args: I) -> Command {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some(OsStr::new("--self-test")) {
        return Command::Ignore;
    }
    let (Some(phase), Some(profile), None) = (args.next(), args.next(), args.next()) else {
        return Command::Usage;
    };
    if profile.is_empty() {
        return Command::Usage;
    }
    match phase.to_str() {
        Some("run") => Command::Execute(Phase::Run, PathBuf::from(profile)),
        Some("verify") => Command::Execute(Phase::Verify, PathBuf::from(profile)),
        _ => Command::Usage,
    }
}

/// Dispatch before any desktop plugin, single-instance guard or logger starts.
pub fn dispatch() -> Option<i32> {
    match parse_command(std::env::args_os().skip(1)) {
        Command::Ignore => None,
        Command::Usage => Some(USAGE_EXIT),
        Command::Execute(phase, profile) => {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return Some(EXIT_RUNTIME),
            };
            let code = runtime.block_on(execute(phase, profile));
            runtime.shutdown_background();
            Some(code)
        }
    }
}

struct Report {
    phase: Phase,
    checks: Vec<Value>,
}

impl Report {
    fn new(phase: Phase) -> Self {
        Self {
            phase,
            checks: Vec::new(),
        }
    }

    fn record(&mut self, name: &str, result: Result<String, String>) -> bool {
        let (status, detail) = match result {
            Ok(detail) => ("passed", detail),
            Err(detail) => ("failed", detail),
        };
        let label = if status == "passed" { "PASS" } else { "FAIL" };
        let _ = writeln!(std::io::stdout(), "{label} {name}: {detail}");
        self.checks
            .push(json!({"name": name, "status": status, "detail": detail}));
        status == "passed"
    }

    fn passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|check| check["status"] == "passed")
    }
}

fn failed<E: std::fmt::Display>(step: &'static str) -> impl FnOnce(E) -> String {
    move |error| format!("{step}: {error}")
}

/// A profile folder that passed the isolation checks.
#[derive(Debug)]
struct Profile {
    path: PathBuf,
    /// Written to the marker by the run phase and repeated in the saved state, so
    /// the verify phase only trusts state that belongs to this folder.
    run_id: String,
}

async fn execute(phase: Phase, requested: PathBuf) -> i32 {
    let mut report = Report::new(phase);
    let profile = match prepare_profile(phase, &requested) {
        Ok(profile) => profile,
        Err(error) => {
            // Nothing may be written to a folder that failed the isolation check.
            report.record("profile_isolation", Err(error));
            let _ = writeln!(std::io::stdout(), "SELF_TEST_RESULT refused");
            return EXIT_REFUSED;
        }
    };
    report.record(
        "profile_isolation",
        Ok("Marked folder outside the application's data folders, without links".to_string()),
    );
    match phase {
        Phase::Run => run_phase(&profile, &mut report).await,
        Phase::Verify => verify_phase(&profile, &mut report).await,
    }
    let profile = profile.path;
    let passed = report.passed();
    let document = json!({
        "schema": 1,
        "phase": report.phase.name(),
        "app_version": env!("CARGO_PKG_VERSION"),
        "passed": passed,
        "checks": report.checks,
    });
    let path = profile.join(format!("self-test-report-{}.json", report.phase.name()));
    // A report that already exists (or is a link) is never written through.
    let written = serde_json::to_vec_pretty(&document)
        .ok()
        .map(|bytes| write_new(&path, &bytes).is_ok())
        .unwrap_or(false);
    let _ = writeln!(
        std::io::stdout(),
        "SELF_TEST_RESULT {}",
        if passed { "passed" } else { "failed" }
    );
    if !passed {
        EXIT_FAILED
    } else if !written {
        EXIT_RUNTIME
    } else {
        0
    }
}

/// Creates a new file and refuses to touch one that exists, so nothing planted in
/// the profile can redirect a write.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Rejects any link (symbolic link, junction or other reparse point) anywhere in
/// the folder, so no path inside the profile can lead outside it.
fn reject_links(root: &Path) -> Result<(), String> {
    let mut pending = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(folder) = pending.pop() {
        let entries = std::fs::read_dir(&folder)
            .map_err(|_| "The profile folder cannot be read".to_string())?;
        for entry in entries {
            let entry = entry.map_err(|_| "The profile folder cannot be read".to_string())?;
            seen += 1;
            if seen > MAX_PROFILE_ENTRIES {
                return Err("The profile folder holds too many entries".into());
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|_| "The profile folder cannot be read".to_string())?;
            if is_link(&metadata) {
                return Err("The profile folder must not contain links".into());
            }
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(())
}

fn marker_text(run_id: &str) -> String {
    format!("{MARKER_HEADER}\nrun-id: {run_id}\n")
}

/// The run id of a marker this program wrote; anything else is refused.
fn read_marker(profile: &Path) -> Result<String, String> {
    const NOT_MARKED: &str = "The folder was not created by a self-test run";
    let path = profile.join(PROFILE_MARKER);
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| NOT_MARKED.to_string())?;
    if is_link(&metadata) || !metadata.is_file() || metadata.len() > 256 {
        return Err(NOT_MARKED.into());
    }
    let text = std::fs::read_to_string(&path).map_err(|_| NOT_MARKED.to_string())?;
    let mut lines = text.lines();
    let run_id = match (lines.next(), lines.next(), lines.next()) {
        (Some(MARKER_HEADER), Some(second), None) => second
            .strip_prefix("run-id: ")
            .filter(|id| uuid::Uuid::parse_str(id).is_ok()),
        _ => None,
    };
    run_id.map(str::to_string).ok_or_else(|| NOT_MARKED.into())
}

/// A relative path from the saved state, resolved only if it names an existing
/// regular file inside the profile.
fn profile_file(profile: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err("The saved state names a path outside the profile".into());
    }
    let path = profile.join(relative);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !is_link(&metadata) => Ok(path),
        _ => Err("The saved state names a file that is not in the profile".into()),
    }
}

/// The folder must be absolute, not a link, new or empty (run) or marked by an
/// earlier run and free of links (verify), and must not overlap the application's
/// data folders.
fn prepare_profile(phase: Phase, requested: &Path) -> Result<Profile, String> {
    if !requested.is_absolute() {
        return Err("The profile folder must be an absolute path".into());
    }
    // Checked before anything is created, including for a folder that does not
    // exist yet, so a refused path is never touched.
    reject_application_folders(&resolved_for_check(requested))?;
    match std::fs::symlink_metadata(requested) {
        Ok(metadata) if is_link(&metadata) => {
            return Err("The profile folder must not be a link".into());
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err("The profile path is not a folder".into());
        }
        Ok(_) => match phase {
            Phase::Run => {
                let mut entries = std::fs::read_dir(requested)
                    .map_err(|_| "The profile folder cannot be read".to_string())?;
                if entries.next().is_some() {
                    return Err(
                        "The profile folder must be empty so that no earlier data is touched"
                            .into(),
                    );
                }
            }
            // The marker and the absence of links are checked once the folder is
            // resolved, below.
            Phase::Verify => {}
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && phase == Phase::Run => {
            std::fs::create_dir_all(requested)
                .map_err(|_| "The profile folder cannot be created".to_string())?;
        }
        Err(_) => return Err("The profile folder is not available".into()),
    }
    let canonical = std::fs::canonicalize(requested)
        .map_err(|_| "The profile folder cannot be resolved".to_string())?;
    let plain = crate::database::meeting_files::plain_path(&canonical);
    reject_application_folders(&plain)?;
    let run_id = match phase {
        Phase::Run => {
            let run_id = uuid::Uuid::new_v4().to_string();
            write_new(&plain.join(PROFILE_MARKER), marker_text(&run_id).as_bytes())
                .map_err(|_| "The profile folder cannot be written".to_string())?;
            run_id
        }
        Phase::Verify => {
            let run_id = read_marker(&plain)?;
            reject_links(&plain)?;
            run_id
        }
    };
    Ok(Profile {
        path: plain,
        run_id,
    })
}

/// The path with its nearest existing ancestor resolved (links followed), so a
/// folder that does not exist yet can still be compared before it is created.
fn resolved_for_check(path: &Path) -> PathBuf {
    let mut missing: Vec<&OsStr> = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(current) {
            let mut resolved = crate::database::meeting_files::plain_path(&canonical);
            for part in missing.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Rejects a profile that is, contains or lies inside any folder the real
/// application (or its predecessor) keeps data in.
fn reject_application_folders(profile: &Path) -> Result<(), String> {
    let config: Value = serde_json::from_str(include_str!("../tauri.conf.json"))
        .map_err(|_| "The application identifier is unavailable".to_string())?;
    let identifier = config["identifier"]
        .as_str()
        .ok_or_else(|| "The application identifier is unavailable".to_string())?;
    let mut protected = vec![
        crate::audio::get_default_recordings_folder(),
        crate::audio::recording_preferences::get_legacy_default_recordings_folder(),
    ];
    for base in [dirs::data_dir(), dirs::data_local_dir(), dirs::config_dir()]
        .into_iter()
        .flatten()
    {
        protected.push(base.join(identifier));
        protected.push(base.join("ClawScribe"));
        protected.push(base.join("Meetily"));
    }
    overlaps_any(profile, &protected)
}

fn overlaps_any(profile: &Path, protected: &[PathBuf]) -> Result<(), String> {
    let candidate = crate::local_path::comparable_text(&profile.to_string_lossy());
    for path in protected {
        let other = crate::local_path::comparable_text(&path.to_string_lossy());
        if candidate == other
            || candidate.starts_with(&format!("{other}/"))
            || other.starts_with(&format!("{candidate}/"))
        {
            return Err(
                "The profile folder must not overlap the application's own data folders".into(),
            );
        }
    }
    Ok(())
}

async fn run_phase(profile: &Profile, report: &mut Report) {
    let run_id = profile.run_id.as_str();
    let profile = profile.path.as_path();
    let mut state = json!({"schema": STATE_SCHEMA, "run_id": run_id});

    report.record("package_layout", package_layout());
    let encoder = packaged_encoder();
    let encoder_ready = report.record("packaged_encoder", encoder);

    let manager = match migration_upgrade(profile, LEGACY_SCHEMA_VERSION).await {
        Ok((manager, detail)) => {
            report.record("migration_upgrade", Ok(detail));
            state["legacy_meeting_id"] = json!(LEGACY_MEETING_ID);
            Some(manager)
        }
        Err(error) => {
            report.record("migration_upgrade", Err(error));
            None
        }
    };

    // Recovery needs the packaged encoder. Without it the check must not fall
    // back to a downloaded or PATH encoder, so it is reported as failed.
    let recovered = if encoder_ready {
        match recording_recovery(profile).await {
            Ok((detail, audio)) => {
                report.record("recording_recovery", Ok(detail));
                // Saved relative to the profile; verify refuses anything else.
                if let Ok(relative) = audio.strip_prefix(profile) {
                    state["recovered_audio"] = json!(relative.to_string_lossy());
                }
                Some(audio)
            }
            Err(error) => {
                report.record("recording_recovery", Err(error));
                None
            }
        }
    } else {
        report.record(
            "recording_recovery",
            Err("Not run: the packaged encoder is missing".into()),
        );
        None
    };

    let mut import_meeting: Option<String> = None;
    if let Some(manager) = manager.as_ref() {
        match import_fixture(profile, manager.pool(), recovered.as_deref()).await {
            Ok((detail, meeting_id)) => {
                report.record("import_fixture", Ok(detail));
                state["import_meeting_id"] = json!(meeting_id);
                import_meeting = Some(meeting_id);
            }
            Err(error) => {
                report.record("import_fixture", Err(error));
            }
        }
    } else {
        report.record(
            "import_fixture",
            Err("Not run: the database did not open".into()),
        );
    }

    match (manager.as_ref(), import_meeting.as_deref()) {
        (Some(manager), Some(meeting_id)) => {
            report.record(
                "summary_stub",
                summary_stub(manager.pool(), meeting_id).await,
            );
        }
        _ => {
            report.record(
                "summary_stub",
                Err("Not run: there is no imported meeting".into()),
            );
        }
    }

    if let Some(manager) = manager {
        let _ = manager.cleanup().await;
    }
    let saved = serde_json::to_vec_pretty(&state)
        .map_err(failed("encode the saved state"))
        .and_then(|bytes| {
            write_new(&profile.join(STATE_FILE), &bytes).map_err(failed("write the saved state"))
        });
    report.record(
        "state_saved",
        saved.map(|_| "Saved the state that the verify phase reopens".to_string()),
    );
}

/// The state the run phase saved, accepted only if it is a regular file of a
/// sensible size that carries this folder's schema and run id.
fn read_state(profile: &Path, run_id: &str) -> Result<Value, String> {
    let path = profile.join(STATE_FILE);
    let metadata = std::fs::symlink_metadata(&path).map_err(failed("read the saved state"))?;
    if is_link(&metadata) || !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
        return Err("The saved state is not a regular file of a sensible size".into());
    }
    let bytes = std::fs::read(&path).map_err(failed("read the saved state"))?;
    let state: Value = serde_json::from_slice(&bytes).map_err(failed("parse the saved state"))?;
    if state["schema"] != STATE_SCHEMA || state["run_id"].as_str() != Some(run_id) {
        return Err("The saved state does not belong to this profile".into());
    }
    Ok(state)
}

async fn verify_phase(profile: &Profile, report: &mut Report) {
    let run_id = profile.run_id.as_str();
    let profile = profile.path.as_path();
    let state = match read_state(profile, run_id) {
        Ok(state) => state,
        Err(error) => {
            report.record("persistence_reopen", Err(error));
            return;
        }
    };
    let (database, legacy) = database_paths(profile);
    if !database.is_file() {
        // Opening would create an empty database, so a missing one is a failure.
        report.record(
            "persistence_reopen",
            Err("The profile holds no database from a run phase".into()),
        );
        return;
    }
    let manager = match (database.to_str(), legacy.to_str()) {
        (Some(database), Some(legacy)) => DatabaseManager::new_isolated(database, legacy)
            .await
            .map_err(failed("reopen the database in a new process")),
        _ => Err("The profile path is not valid text".to_string()),
    };
    let manager = match manager {
        Ok(manager) => manager,
        Err(error) => {
            report.record("persistence_reopen", Err(error));
            return;
        }
    };
    report.record(
        "persistence_reopen",
        persisted_data(manager.pool(), &state).await,
    );
    report.record(
        "database_integrity",
        database_integrity(manager.pool()).await,
    );
    report.record(
        "recovered_audio_persisted",
        match state["recovered_audio"].as_str() {
            Some(relative) => match profile_file(profile, relative) {
                Ok(path) => decode_check(path, 3.0, 0.35).await,
                Err(error) => Err(error),
            },
            None => Err("The first run saved no recovered audio".to_string()),
        },
    );
    let _ = manager.cleanup().await;
}

fn database_paths(profile: &Path) -> (PathBuf, PathBuf) {
    (
        profile.join("meeting_minutes.sqlite"),
        profile.join("no-legacy-backend.db"),
    )
}

// ---- package -------------------------------------------------------------------

fn executable_folder() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(failed("locate the executable"))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "The executable has no folder".to_string())
}

const PACKAGED_FILES: [&str; 5] = [
    "ffmpeg.exe",
    "llama-helper.exe",
    "codex-app-server.exe",
    "onnxruntime.dll",
    "sherpa-onnx-c-api.dll",
];

fn non_empty_file(path: &Path) -> bool {
    matches!(std::fs::metadata(path), Ok(metadata) if metadata.is_file() && metadata.len() > 0)
}

/// A bundled template must exist, match the copy compiled into this build (line
/// endings aside) and parse with at least one section.
fn template_problem(folder: &Path, name: &str, embedded: &str) -> Option<String> {
    let Ok(text) = std::fs::read_to_string(folder.join("templates").join(name)) else {
        return Some(format!("templates/{name}"));
    };
    let same = text.replace("\r\n", "\n") == embedded.replace("\r\n", "\n");
    let usable = serde_json::from_str::<Template>(&text)
        .map(|template| !template.sections.is_empty())
        .unwrap_or(false);
    if same && usable {
        None
    } else {
        Some(format!("templates/{name} (not this build's copy)"))
    }
}

/// The pinned Codex runtime: its manifest and a populated runtime folder, in the
/// places the provider looks for them.
fn codex_runtime_problem(folder: &Path) -> Option<String> {
    let manifest = folder
        .join("binaries")
        .join("codex-app-server-runtime.json");
    let pinned = std::fs::read(&manifest)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value["runtime_sha256"].as_str().map(|sum| sum.len() == 64))
        .unwrap_or(false);
    if !pinned {
        return Some("binaries/codex-app-server-runtime.json".to_string());
    }
    let populated = [
        folder.join("codex-app-server-runtime"),
        folder.join("binaries").join("codex-app-server-runtime"),
    ]
    .iter()
    .any(|runtime| {
        std::fs::read_dir(runtime)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    });
    if populated {
        None
    } else {
        Some("codex-app-server-runtime".to_string())
    }
}

/// Everything the installer should have placed beside the executable that is
/// absent or unusable.
fn layout_problems(folder: &Path, directml_expected: bool) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    for name in PACKAGED_FILES {
        if !non_empty_file(&folder.join(name)) {
            problems.push(name.to_string());
        }
    }
    if directml_expected && !non_empty_file(&folder.join("DirectML.dll")) {
        problems.push("DirectML.dll".to_string());
    }
    for (name, embedded) in BUNDLED_TEMPLATES {
        problems.extend(template_problem(folder, name, embedded));
    }
    problems.extend(codex_runtime_problem(folder));
    problems
}

/// Sidecars, runtime libraries, bundled templates and the Codex runtime the
/// installer places next to the executable.
fn package_layout() -> Result<String, String> {
    let folder = executable_folder()?;
    // The DirectML runtime ships with the DirectML and windows-gpu builds only.
    let directml = cfg!(feature = "directml");
    let problems = layout_problems(&folder, directml);
    if !problems.is_empty() {
        return Err(format!(
            "Missing or unusable in the package: {}",
            problems.join(", ")
        ));
    }
    Ok(format!(
        "{} required files, {} templates, the runtime manifest and the Codex runtime present; DirectML runtime {}",
        PACKAGED_FILES.len(),
        BUNDLED_TEMPLATES.len(),
        if directml { "required and present" } else { "not required" }
    ))
}

/// The packaged encoder must sit next to the executable and run. The shared
/// lookup would otherwise fall back to PATH or a download, which the check must
/// never do.
fn packaged_encoder() -> Result<String, String> {
    let ffmpeg = executable_folder()?.join("ffmpeg.exe");
    if !ffmpeg.is_file() {
        return Err("The packaged encoder is not next to the executable".into());
    }
    let mut command = std::process::Command::new(&ffmpeg);
    command
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command
        .output()
        .map_err(failed("start the packaged encoder"))?;
    if !output.status.success() {
        return Err("The packaged encoder did not run".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().next().unwrap_or("encoder ran").to_string())
}

// ---- database ------------------------------------------------------------------

fn embedded_migration_count() -> i64 {
    sqlx::migrate!("./migrations").iter().count() as i64
}

async fn table_exists(pool: &SqlitePool, table: &str) -> Result<bool, String> {
    let found: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_one(pool)
            .await
            .map_err(failed("inspect the schema"))?;
    Ok(found == 1)
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> Result<bool, String> {
    let found: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?")
        .bind(table)
        .bind(column)
        .fetch_one(pool)
        .await
        .map_err(failed("inspect the schema"))?;
    Ok(found == 1)
}

/// Builds a database as an older build left it (only the migrations up to
/// `cutoff` applied, seeded with non-secret rows, one checksum stamped with the
/// other line-ending variant), then opens it through the production path that
/// backs it up, repairs the checksum and applies the remaining migrations. The
/// isolated open path is used so no credential store is read or written.
async fn migration_upgrade(
    profile: &Path,
    cutoff: i64,
) -> Result<(DatabaseManager, String), String> {
    let (database, legacy_backend) = database_paths(profile);
    let full = sqlx::migrate!("./migrations");
    let older: Vec<Migration> = full
        .iter()
        .filter(|migration| migration.version <= cutoff)
        .cloned()
        .collect();
    let total = full.iter().count();
    if older.len() < 3 || older.len() >= total {
        return Err("The legacy schema fixture does not leave migrations to apply".into());
    }
    let drift = older[0].clone();

    // A database as an older build left it: only the older migrations applied.
    let options = SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(failed("create the legacy database"))?;
    let legacy = Migrator {
        migrations: Cow::Owned(older.clone()),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    legacy
        .run(&pool)
        .await
        .map_err(failed("apply the legacy schema"))?;
    sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
        .bind(LEGACY_MEETING_ID)
        .bind(LEGACY_MEETING_TITLE)
        .bind("2025-10-01T09:00:00Z")
        .bind("2025-10-01T09:00:00Z")
        .execute(&pool)
        .await
        .map_err(failed("insert the legacy meeting"))?;
    for (id, text) in LEGACY_LINES {
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(LEGACY_MEETING_ID)
        .bind(text)
        .bind("2025-10-01T09:00:00Z")
        .execute(&pool)
        .await
        .map_err(failed("insert a legacy transcript line"))?;
    }
    // A recent schema also has the file-sync table, which a later migration extends.
    let seeded_sync = table_exists(&pool, "transcript_file_sync").await?;
    if seeded_sync {
        sqlx::query("INSERT INTO transcript_file_sync (meeting_id) VALUES (?)")
            .bind(LEGACY_MEETING_ID)
            .execute(&pool)
            .await
            .map_err(failed("seed the file-sync row"))?;
    }

    // Line-ending checksum drift: stamp one applied migration with the checksum
    // of the other line-ending variant, as an older CRLF checkout did.
    {
        use sha2::{Digest, Sha384};
        let lf = drift.sql.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let lf_sum = Sha384::digest(lf.as_bytes()).to_vec();
        let crlf_sum = Sha384::digest(crlf.as_bytes()).to_vec();
        let drifted = if drift.checksum.as_ref() == lf_sum.as_slice() {
            crlf_sum
        } else {
            lf_sum
        };
        if drifted.as_slice() == drift.checksum.as_ref() {
            return Err("The line-ending drift fixture matches the embedded checksum".into());
        }
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
            .bind(drifted)
            .bind(drift.version)
            .execute(&pool)
            .await
            .map_err(failed("stamp the drifted checksum"))?;
    }
    pool.close().await;

    // The production open path: backup, self-heal, migrate.
    let (Some(database_text), Some(legacy_text)) = (database.to_str(), legacy_backend.to_str())
    else {
        return Err("The profile path is not valid text".into());
    };
    let manager = DatabaseManager::new_isolated(database_text, legacy_text)
        .await
        .map_err(failed("open the upgraded database"))?;
    let pool = manager.pool();

    let applied: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(pool)
            .await
            .map_err(failed("count applied migrations"))?;
    if applied != total as i64 {
        return Err(format!("{applied} of {total} migrations are applied"));
    }
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
            .bind(drift.version)
            .fetch_one(pool)
            .await
            .map_err(failed("read the repaired checksum"))?;
    if stored.as_slice() != drift.checksum.as_ref() {
        return Err("The line-ending checksum drift was not repaired".into());
    }
    // Additions of the newest migrations must exist after the upgrade.
    let latest = [
        table_exists(pool, "recording_outcomes").await?,
        table_exists(pool, "calendar_series_templates").await?,
        column_exists(pool, "meetings", "calendar_link").await?,
        column_exists(pool, "transcript_file_sync", "retranscription_json").await?,
    ];
    if latest.iter().any(|present| !present) {
        return Err("A table or column added by a later migration is missing".into());
    }
    if seeded_sync {
        let kept: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync WHERE meeting_id = ?")
                .bind(LEGACY_MEETING_ID)
                .fetch_one(pool)
                .await
                .map_err(failed("read the seeded file-sync row"))?;
        if kept != 1 {
            return Err("The seeded file-sync row did not survive the upgrade".into());
        }
    }
    legacy_data_intact(pool).await?;
    database_integrity(pool).await?;
    let backup = std::fs::read_dir(profile.join("backups"))
        .map_err(|_| "No pre-migration backup folder was created".to_string())?
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("meeting_minutes-")
                && name.ends_with(".sqlite")
                && entry
                    .metadata()
                    .map(|metadata| metadata.len() > 0)
                    .unwrap_or(false)
        });
    if !backup {
        return Err("No pre-migration backup was written".into());
    }
    Ok((
        manager,
        format!(
            "{} older migrations upgraded to {total}; checksum drift repaired; backup written; seeded meeting and lines intact",
            older.len()
        ),
    ))
}

async fn legacy_data_intact(pool: &SqlitePool) -> Result<(), String> {
    let titles: Vec<String> = sqlx::query_scalar("SELECT title FROM meetings WHERE id = ?")
        .bind(LEGACY_MEETING_ID)
        .fetch_all(pool)
        .await
        .map_err(failed("read the legacy meeting"))?;
    if titles.len() != 1 || titles[0] != LEGACY_MEETING_TITLE {
        return Err("The legacy meeting did not survive the upgrade".into());
    }
    let lines: Vec<String> =
        sqlx::query_scalar("SELECT transcript FROM transcripts WHERE meeting_id = ? ORDER BY id")
            .bind(LEGACY_MEETING_ID)
            .fetch_all(pool)
            .await
            .map_err(failed("read the legacy transcript"))?;
    let expected: Vec<&str> = LEGACY_LINES.iter().map(|(_, text)| *text).collect();
    if lines.iter().map(String::as_str).collect::<Vec<_>>() != expected {
        return Err("The legacy transcript did not survive the upgrade".into());
    }
    Ok(())
}

async fn database_integrity(pool: &SqlitePool) -> Result<String, String> {
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .map_err(failed("run the integrity check"))?;
    if integrity != "ok" {
        return Err("The database integrity check reported problems".into());
    }
    let total = embedded_migration_count();
    let applied: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(pool)
            .await
            .map_err(failed("count applied migrations"))?;
    if applied != total {
        return Err(format!("{applied} of {total} migrations are applied"));
    }
    Ok(format!(
        "Integrity check passed; {total} migrations applied"
    ))
}

async fn persisted_data(pool: &SqlitePool, state: &Value) -> Result<String, String> {
    legacy_data_intact(pool).await?;
    let meeting_id = state["import_meeting_id"]
        .as_str()
        .ok_or_else(|| "The first run saved no imported meeting".to_string())?;
    let lines: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .fetch_one(pool)
        .await
        .map_err(failed("read the imported transcript"))?;
    if lines != IMPORT_LINES.len() as i64 {
        return Err(format!("{lines} imported transcript lines were found"));
    }
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status, result FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(failed("read the saved summary"))?;
    match row {
        Some((status, Some(result)))
            if status.eq_ignore_ascii_case("completed") && result.contains(STUB_MARKER) => {}
        _ => return Err("The saved summary did not survive the restart".into()),
    }
    Ok("Legacy meeting, imported transcript and summary survived a new process".into())
}

// ---- recording recovery -----------------------------------------------------------

fn sine(sample_rate: u32, start: usize, count: usize, frequency: f32) -> Vec<f32> {
    (start..start + count)
        .map(|index| {
            0.25 * (2.0 * std::f32::consts::PI * frequency * index as f32 / sample_rate as f32)
                .sin()
        })
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn wav_bytes(sample_rate: u32, samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    bytes
}

/// Decodes an audio file and checks its length and that it is not silent.
async fn decode_check(path: PathBuf, seconds: f64, tolerance: f64) -> Result<String, String> {
    let decoded = tokio::task::spawn_blocking(move || decoder::decode_audio_file(&path))
        .await
        .map_err(failed("decode the audio"))?
        .map_err(failed("decode the audio"))?;
    if (decoded.duration_seconds - seconds).abs() > tolerance {
        return Err(format!(
            "The decoded audio is {:.2} s long, expected about {seconds:.1} s",
            decoded.duration_seconds
        ));
    }
    let level = rms(&decoded.samples);
    if level < 0.02 {
        return Err("The decoded audio is silent".into());
    }
    Ok(format!(
        "Decoded {:.2} s at {} Hz, level {:.2}",
        decoded.duration_seconds, decoded.sample_rate, level
    ))
}

/// A recording that stopped without being finished: durable capture chunks and
/// the app's marker, no final audio file.
async fn recording_recovery(profile: &Path) -> Result<(String, PathBuf), String> {
    let root = profile.join("recordings");
    let folder = root.join("Synthetic interrupted meeting");
    std::fs::create_dir_all(&folder).map_err(failed("create the recording folder"))?;
    let marker = json!({
        "version": "1.0",
        "meeting_name": "Synthetic interrupted meeting",
        "transcript_file": "transcripts.json",
        "created_at": "2026-11-01T10:00:00Z",
        "status": "recording",
    });
    std::fs::write(folder.join("metadata.json"), marker.to_string())
        .map_err(failed("write the recording marker"))?;
    let snapshot = json!({
        "version": "1.0",
        "segments": [{"id": "snapshot-1", "text": "Synthetic captured line", "audio_start_time": 0.0, "audio_end_time": 2.0}],
    });
    std::fs::write(folder.join("transcripts.json"), snapshot.to_string())
        .map_err(failed("write the transcript snapshot"))?;

    let (sender, receiver, _metrics) =
        recording_audio_queue(&folder).map_err(failed("create the capture spool"))?;
    for id in 0..6u64 {
        sender
            .send(AudioChunk {
                data: sine(48_000, id as usize * 24_000, 24_000, 440.0),
                sample_rate: 48_000,
                timestamp: id as f64 * 0.5,
                chunk_id: id,
                device_type: DeviceType::System,
            })
            .await
            .map_err(failed("write a capture chunk"))?;
    }
    drop(sender);
    drop(receiver);

    let roots = vec![root];
    let registered: HashMap<String, String> = HashMap::new();
    let kept: Vec<String> = Vec::new();
    let scan = interrupted::scan(&interrupted::ScanInput {
        roots: &roots,
        kept: &kept,
        registered: &registered,
        active: interrupted::ActiveFolder::None,
        limits: interrupted::Limits::default(),
    });
    if scan.recordings.len() != 1 {
        return Err(format!(
            "The scan found {} interrupted recordings, expected 1",
            scan.recordings.len()
        ));
    }
    let found = &scan.recordings[0];
    if !found.has_capture || !found.has_transcript || !found.reasons.contains(&"unfinished_audio") {
        return Err("The interrupted recording was found but not recognised as recoverable".into());
    }

    let status = incremental_saver::recover_audio_in_folder(folder.to_string_lossy().into_owned())
        .await
        .map_err(failed("recover the audio"))?;
    if status.status != "success" {
        return Err(format!("Recovery ended with status {}", status.status));
    }
    let audio = PathBuf::from(
        status
            .audio_file_path
            .ok_or_else(|| "Recovery produced no audio file".to_string())?,
    );
    if !audio.starts_with(profile) {
        return Err("Recovery wrote audio outside the profile folder".into());
    }
    // The compressed result proves the packaged encoder ran; the encoder-free
    // fallback writes WAV.
    if audio
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("mp4"))
        != Some(true)
    {
        return Err(
            "Recovery fell back to uncompressed audio instead of the packaged encoder".into(),
        );
    }
    let decoded = decode_check(audio.clone(), 3.0, 0.35).await?;
    Ok((
        format!(
            "Found 1 interrupted recording, recovered {} chunks to compressed audio. {decoded}",
            status.chunk_count
        ),
        audio,
    ))
}

// ---- import -------------------------------------------------------------------------

async fn import_fixture(
    profile: &Path,
    pool: &SqlitePool,
    recovered: Option<&Path>,
) -> Result<(String, String), String> {
    let folder = profile
        .join("recordings")
        .join("Synthetic imported meeting");
    std::fs::create_dir_all(&folder).map_err(failed("create the import folder"))?;
    let fixture = folder.join("synthetic-import.wav");
    let mut samples = sine(16_000, 0, 40_000, 330.0);
    samples.extend(sine(16_000, 40_000, 40_000, 495.0));
    std::fs::write(&fixture, wav_bytes(16_000, &samples)).map_err(failed("write the fixture"))?;

    let info = import::validate_audio_file(&fixture).map_err(failed("validate the fixture"))?;
    if info.size_bytes == 0 {
        return Err("The fixture was validated as empty".into());
    }
    let decoded = decode_check(fixture.clone(), 5.0, 0.25).await?;
    let mut detail = format!("Validated and decoded the WAV fixture. {decoded}");
    if let Some(recovered) = recovered {
        import::validate_audio_file(recovered)
            .map_err(failed("validate the recovered compressed audio"))?;
        detail.push_str(" The recovered compressed audio also validated.");
    }

    let segments: Vec<TranscriptSegment> = IMPORT_LINES
        .iter()
        .enumerate()
        .map(|(index, text)| TranscriptSegment {
            id: format!("synthetic-import-line-{}", index + 1),
            text: (*text).to_string(),
            timestamp: "2026-11-01T10:00:00Z".to_string(),
            audio_start_time: Some(index as f64 * 2.5),
            audio_end_time: Some(index as f64 * 2.5 + 2.5),
            duration: Some(2.5),
            speaker: Some("Participants".to_string()),
            word_timestamps: None,
        })
        .collect();
    let meeting_id = import::create_meeting_with_transcripts(
        pool,
        "Synthetic imported meeting",
        &segments,
        folder.to_string_lossy().into_owned(),
        false,
        import::ImportDate::default(),
    )
    .await
    .map_err(failed("save the imported meeting"))?;
    let saved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?")
        .bind(&meeting_id)
        .fetch_one(pool)
        .await
        .map_err(failed("read the imported meeting"))?;
    if saved != segments.len() as i64 {
        return Err(format!(
            "{saved} of {} imported lines were saved",
            segments.len()
        ));
    }
    Ok((
        format!("{detail} Saved {saved} transcript lines."),
        meeting_id,
    ))
}

// ---- summary ---------------------------------------------------------------------

type SeenRequests = Arc<Mutex<Vec<(String, String)>>>;

struct Stub {
    address: SocketAddr,
    requests: SeenRequests,
    task: tokio::task::JoinHandle<()>,
}

fn header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

async fn serve_one(mut stream: tokio::net::TcpStream, seen: SeenRequests) -> std::io::Result<()> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_len = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = header_end(&buffer) {
            break end;
        }
        if buffer.len() > 1 << 20 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_len]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while buffer.len() < head_len + length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[head_len..]).into_owned();
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    if let Ok(mut requests) = seen.lock() {
        requests.push((path, body));
    }
    let reply = json!({
        "id": "stub-completion",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": STUB_SUMMARY},
            "finish_reason": "stop",
        }],
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    stream.shutdown().await
}

async fn start_stub() -> Result<Stub, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(failed("start the loopback stub"))?;
    let address = listener
        .local_addr()
        .map_err(failed("read the loopback stub address"))?;
    let requests: SeenRequests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let _ = serve_one(stream, seen).await;
            });
        }
    });
    Ok(Stub {
        address,
        requests,
        task,
    })
}

/// The production summary path (provider call, prompt assembly, output checks)
/// against an offline loopback stub, then saved like a real summary.
async fn summary_stub(pool: &SqlitePool, meeting_id: &str) -> Result<String, String> {
    let stub = start_stub().await?;
    let result = summary_against(&stub, pool, meeting_id).await;
    stub.task.abort();
    result
}

async fn summary_against(
    stub: &Stub,
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<String, String> {
    let template = Template {
        name: "Self-test".to_string(),
        description: "Synthetic template for packaged-binary acceptance".to_string(),
        sections: vec![TemplateSection {
            title: "Summary".to_string(),
            instruction: "Summarize the meeting in two sentences.".to_string(),
            format: "paragraph".to_string(),
            item_format: None,
            example_item_format: None,
        }],
    };
    let endpoint = format!("http://{}/v1", stub.address);
    // No proxy: the request must reach the loopback stub and nothing else.
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(failed("create the loopback client"))?;
    let transcript = IMPORT_LINES.join("\n");
    let (markdown, _english, chunks) = generate_meeting_summary(
        &client,
        &LLMProvider::CustomOpenAI,
        "stub-model",
        "synthetic-self-test-key",
        &transcript,
        "",
        "self_test",
        &template,
        8_000,
        None,
        Some(endpoint.as_str()),
        false,
        Some(1_024),
        None,
        None,
        None,
        None,
        Some("en"),
        Some("en"),
        None,
        &crate::summary::context_budget::Reduction::default(),
    )
    .await
    .map_err(failed("generate the summary"))?;
    if !markdown.contains(STUB_MARKER) {
        return Err("The summary does not contain the stub's answer".into());
    }
    let seen = stub
        .requests
        .lock()
        .map_err(|_| "The stub request log is unavailable".to_string())?
        .clone();
    if !seen
        .iter()
        .any(|(path, body)| path.ends_with("/chat/completions") && body.contains(SENTINEL))
    {
        return Err("The stub did not receive the transcript through /chat/completions".into());
    }
    SummaryProcessesRepository::create_or_reset_process(pool, meeting_id)
        .await
        .map_err(failed("start the summary record"))?;
    SummaryProcessesRepository::update_process_completed(
        pool,
        meeting_id,
        json!({ "markdown": markdown }),
        chunks,
        0.1,
    )
    .await
    .map_err(failed("save the summary"))?;
    Ok(format!(
        "Summary generated through the provider path from {} stub request(s) and saved",
        seen.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_profile_inside_or_around_application_folders() {
        let protected = vec![PathBuf::from("D:/data/app")];
        for path in [
            "D:/data/app",
            "D:/data/app/run",
            "D:/data",
            "d:\\DATA\\app\\",
        ] {
            assert!(overlaps_any(Path::new(path), &protected).is_err(), "{path}");
        }
        for path in ["D:/data/app-other", "D:/scratch/run", "E:/data/app"] {
            assert!(overlaps_any(Path::new(path), &protected).is_ok(), "{path}");
        }
    }

    #[test]
    fn relative_profile_paths_are_refused() {
        assert!(prepare_profile(Phase::Run, Path::new("relative/profile")).is_err());
    }

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_self_test_invocations_are_handled() {
        assert_eq!(parse_command(args(&[])), Command::Ignore);
        assert_eq!(parse_command(args(&["run", "D:/x"])), Command::Ignore);
        assert_eq!(
            parse_command(args(&["--other", "run", "D:/x"])),
            Command::Ignore
        );
    }

    #[test]
    fn malformed_self_test_command_lines_are_usage_errors() {
        let cases: [&[&str]; 8] = [
            &["--self-test"],
            &["--self-test", "run"],
            &["--self-test", "verify"],
            &["--self-test", "run", "D:/x", "extra"],
            &["--self-test", "unknown", "D:/x"],
            &["--self-test", "RUN", "D:/x"],
            &["--self-test", "run", ""],
            &["--self-test", "", "D:/x"],
        ];
        for case in cases {
            assert_eq!(parse_command(args(case)), Command::Usage, "{case:?}");
        }
    }

    #[test]
    fn well_formed_command_lines_select_the_phase_and_folder() {
        assert_eq!(
            parse_command(args(&["--self-test", "run", "D:/x"])),
            Command::Execute(Phase::Run, PathBuf::from("D:/x"))
        );
        assert_eq!(
            parse_command(args(&["--self-test", "verify", "D:/x"])),
            Command::Execute(Phase::Verify, PathBuf::from("D:/x"))
        );
    }

    fn folder_names(path: &Path) -> Vec<OsString> {
        let mut names: Vec<OsString> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_non_empty_profile_folder_is_refused_untouched() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "untouched").unwrap();
        assert!(prepare_profile(Phase::Run, dir.path()).is_err());
        assert_eq!(folder_names(dir.path()), vec![OsString::from("keep.txt")]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(),
            "untouched"
        );
    }

    #[test]
    fn a_run_marks_the_folder_and_verify_reads_the_same_run_id_back() {
        let dir = tempfile::tempdir().unwrap();
        let run = prepare_profile(Phase::Run, dir.path()).unwrap();
        let verify = prepare_profile(Phase::Verify, dir.path()).unwrap();
        assert_eq!(run.run_id, verify.run_id);
        assert!(uuid::Uuid::parse_str(&run.run_id).is_ok());
        // The folder is no longer empty, so a second run is refused.
        assert!(prepare_profile(Phase::Run, dir.path()).is_err());
    }

    #[test]
    fn verify_refuses_unmarked_and_forged_markers() {
        let id = uuid::Uuid::new_v4().to_string();
        let forged = [
            "something else\n".to_string(),
            format!("{MARKER_HEADER}\nrun-id: not-an-id\n"),
            format!("{MARKER_HEADER}\nrun-id: {id}\nextra\n"),
            format!("{MARKER_HEADER}\n"),
            String::new(),
        ];
        let unmarked = tempfile::tempdir().unwrap();
        assert!(prepare_profile(Phase::Verify, unmarked.path()).is_err());
        for text in forged {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(PROFILE_MARKER), &text).unwrap();
            assert!(
                prepare_profile(Phase::Verify, dir.path()).is_err(),
                "{text:?}"
            );
        }
        let folder_marker = tempfile::tempdir().unwrap();
        std::fs::create_dir(folder_marker.path().join(PROFILE_MARKER)).unwrap();
        assert!(prepare_profile(Phase::Verify, folder_marker.path()).is_err());
        // A well-formed marker is accepted.
        let valid = tempfile::tempdir().unwrap();
        std::fs::write(valid.path().join(PROFILE_MARKER), marker_text(&id)).unwrap();
        assert_eq!(
            prepare_profile(Phase::Verify, valid.path()).unwrap().run_id,
            id
        );
    }

    #[cfg(windows)]
    fn make_dir_link(link: &Path, target: &Path) {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return;
        }
        // A junction needs no special privilege.
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the test link could not be created");
    }

    #[cfg(unix)]
    fn make_dir_link(link: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn verify_refuses_a_profile_that_contains_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        prepare_profile(Phase::Run, dir.path()).unwrap();
        assert!(prepare_profile(Phase::Verify, dir.path()).is_ok());
        let link = dir.path().join("recordings");
        make_dir_link(&link, outside.path());
        let error = prepare_profile(Phase::Verify, dir.path()).unwrap_err();
        assert!(error.contains("links"), "{error}");
        std::fs::remove_dir(&link).unwrap();
        assert!(prepare_profile(Phase::Verify, dir.path()).is_ok());
    }

    #[test]
    fn a_profile_folder_that_is_a_link_is_refused_untouched() {
        let parent = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let link = parent.path().join("profile");
        make_dir_link(&link, target.path());
        for phase in [Phase::Run, Phase::Verify] {
            assert!(prepare_profile(phase, &link).is_err());
        }
        assert!(folder_names(target.path()).is_empty());
        std::fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn writes_never_replace_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        write_new(&path, b"first").unwrap();
        assert!(write_new(&path, b"second").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
    }

    #[test]
    fn saved_state_paths_stay_inside_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("recordings")).unwrap();
        std::fs::write(dir.path().join("recordings").join("audio.mp4"), b"x").unwrap();
        assert!(profile_file(dir.path(), "recordings/audio.mp4").is_ok());
        for bad in [
            "",
            "../audio.mp4",
            "recordings/../../x",
            "C:/Windows/notepad.exe",
            "/etc/passwd",
            "\\\\server\\share\\x",
            "recordings",
            "missing.mp4",
        ] {
            assert!(profile_file(dir.path(), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn saved_state_must_belong_to_this_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let write = |text: &str| std::fs::write(&path, text).unwrap();
        write(r#"{"schema": 1, "run_id": "a"}"#);
        assert!(read_state(dir.path(), "a").is_ok());
        assert!(read_state(dir.path(), "b").is_err());
        write(r#"{"schema": 2, "run_id": "a"}"#);
        assert!(read_state(dir.path(), "a").is_err());
        write("not json");
        assert!(read_state(dir.path(), "a").is_err());
        write(&" ".repeat(MAX_STATE_BYTES as usize + 1));
        assert!(read_state(dir.path(), "a").is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(read_state(dir.path(), "a").is_err());
    }

    #[tokio::test]
    async fn a_refused_run_reports_the_refusal_exit_code_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "untouched").unwrap();
        let code = execute(Phase::Run, dir.path().to_path_buf()).await;
        assert_eq!(code, EXIT_REFUSED);
        assert_eq!(folder_names(dir.path()), vec![OsString::from("keep.txt")]);
        let relative = execute(Phase::Run, PathBuf::from("relative/profile")).await;
        assert_eq!(relative, EXIT_REFUSED);
    }

    #[tokio::test]
    async fn a_refused_verify_reports_the_refusal_exit_code_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let code = execute(Phase::Verify, dir.path().to_path_buf()).await;
        assert_eq!(code, EXIT_REFUSED);
        assert!(folder_names(dir.path()).is_empty());
    }

    #[test]
    fn exit_codes_are_distinct() {
        let codes = [0, EXIT_FAILED, USAGE_EXIT, EXIT_RUNTIME, EXIT_REFUSED];
        for (index, code) in codes.iter().enumerate() {
            assert!(!codes[index + 1..].contains(code), "{code}");
        }
    }

    #[test]
    fn bundled_template_list_matches_the_source_folder_and_the_bundle_config() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let mut on_disk: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = BUNDLED_TEMPLATES
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        listed.sort();
        assert_eq!(on_disk, listed);
        for (name, text) in BUNDLED_TEMPLATES {
            let template: Template = serde_json::from_str(text).unwrap();
            assert!(!template.sections.is_empty(), "{name}");
        }
        // The layout check looks where the bundle configuration puts resources.
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let resources = &config["bundle"]["resources"];
        assert_eq!(resources["templates/"], "templates/");
        assert_eq!(
            resources["binaries/codex-app-server-runtime.json"],
            "binaries/codex-app-server-runtime.json"
        );
        assert!(resources["binaries/codex-app-server-runtime/"].is_string());
        let sidecars: Vec<&str> = config["bundle"]["externalBin"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for name in ["llama-helper", "codex-app-server", "ffmpeg"] {
            assert!(
                sidecars.contains(&format!("binaries/{name}").as_str()),
                "{name}"
            );
        }
    }

    fn complete_package(folder: &Path, directml: bool) {
        for name in PACKAGED_FILES {
            std::fs::write(folder.join(name), b"binary").unwrap();
        }
        if directml {
            std::fs::write(folder.join("DirectML.dll"), b"binary").unwrap();
        }
        std::fs::create_dir(folder.join("templates")).unwrap();
        for (name, text) in BUNDLED_TEMPLATES {
            // Installers may have changed the line endings; that is accepted.
            std::fs::write(
                folder.join("templates").join(name),
                text.replace("\r\n", "\n").replace('\n', "\r\n"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(folder.join("binaries").join("codex-app-server-runtime")).unwrap();
        std::fs::write(
            folder
                .join("binaries")
                .join("codex-app-server-runtime")
                .join("package.json"),
            b"{}",
        )
        .unwrap();
        std::fs::write(
            folder
                .join("binaries")
                .join("codex-app-server-runtime.json"),
            format!(r#"{{"runtime_sha256": "{}"}}"#, "a".repeat(64)),
        )
        .unwrap();
    }

    #[test]
    fn a_complete_package_has_no_layout_problems() {
        let dir = tempfile::tempdir().unwrap();
        complete_package(dir.path(), true);
        assert!(layout_problems(dir.path(), true).is_empty());
        assert!(layout_problems(dir.path(), false).is_empty());
    }

    #[test]
    fn each_missing_or_wrong_package_element_is_named() {
        let expectations: [(&str, &str); 6] = [
            ("ffmpeg.exe", "ffmpeg.exe"),
            ("onnxruntime.dll", "onnxruntime.dll"),
            ("DirectML.dll", "DirectML.dll"),
            (
                "templates/standard_meeting.json",
                "templates/standard_meeting.json",
            ),
            (
                "binaries/codex-app-server-runtime.json",
                "binaries/codex-app-server-runtime.json",
            ),
            (
                "binaries/codex-app-server-runtime/package.json",
                "codex-app-server-runtime",
            ),
        ];
        for (removed, reported) in expectations {
            let dir = tempfile::tempdir().unwrap();
            complete_package(dir.path(), true);
            std::fs::remove_file(dir.path().join(removed)).unwrap();
            let problems = layout_problems(dir.path(), true);
            assert!(
                problems.iter().any(|problem| problem.starts_with(reported)),
                "{removed}: {problems:?}"
            );
        }
        // An empty sidecar, a changed template and an unparsable one are all unusable.
        let dir = tempfile::tempdir().unwrap();
        complete_package(dir.path(), false);
        std::fs::write(dir.path().join("llama-helper.exe"), b"").unwrap();
        std::fs::write(
            dir.path().join("templates").join("retrospective.json"),
            "{}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("templates").join("project_sync.json"),
            "nope",
        )
        .unwrap();
        let problems = layout_problems(dir.path(), false);
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    async fn upgrade_import_summarize_and_reopen(cutoff: i64) {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path();
        let (manager, upgrade) = migration_upgrade(profile, cutoff).await.unwrap();
        assert!(upgrade.contains("upgraded"), "{upgrade}");
        let (_, meeting_id) = import_fixture(profile, manager.pool(), None).await.unwrap();
        summary_stub(manager.pool(), &meeting_id).await.unwrap();
        manager.cleanup().await.unwrap();

        // A fresh open of the same files, as the verify phase does in a new process.
        let (database, legacy) = database_paths(profile);
        let reopened =
            DatabaseManager::new_isolated(database.to_str().unwrap(), legacy.to_str().unwrap())
                .await
                .unwrap();
        let state = json!({ "import_meeting_id": meeting_id });
        persisted_data(reopened.pool(), &state).await.unwrap();
        database_integrity(reopened.pool()).await.unwrap();
        reopened.cleanup().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn legacy_database_upgrades_imports_summarizes_and_reopens() {
        upgrade_import_summarize_and_reopen(LEGACY_SCHEMA_VERSION).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recent_schema_with_seeded_rows_upgrades_imports_summarizes_and_reopens() {
        upgrade_import_summarize_and_reopen(RECENT_SCHEMA_VERSION).await;
    }

    #[test]
    fn recent_cutoff_leaves_the_newest_migrations_to_apply() {
        let migrator = sqlx::migrate!("./migrations");
        let newer: Vec<i64> = migrator
            .iter()
            .map(|migration| migration.version)
            .filter(|version| *version > RECENT_SCHEMA_VERSION)
            .collect();
        assert!(!newer.is_empty());
        assert!(migrator
            .iter()
            .any(|migration| migration.version == RECENT_SCHEMA_VERSION));
    }

    #[test]
    fn wav_fixture_has_a_valid_header_and_length() {
        let samples = sine(16_000, 0, 160, 330.0);
        let bytes = wav_bytes(16_000, &samples);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(bytes.len(), 44 + 160 * 2);
        assert!(rms(&samples) > 0.1);
    }

    #[test]
    fn profile_guard_rejects_children_of_both_recording_roots_without_creating_them() {
        for root in [
            crate::audio::get_default_recordings_folder(),
            crate::audio::recording_preferences::get_legacy_default_recordings_folder(),
        ] {
            assert!(reject_application_folders(&root.join("synthetic-acceptance")).is_err());
        }
    }

    #[test]
    fn header_end_finds_the_blank_line() {
        assert_eq!(header_end(b"POST / HTTP/1.1\r\nA: b\r\n\r\nbody"), Some(25));
        assert_eq!(header_end(b"POST / HTTP/1.1\r\nA: b\r\n"), None);
    }

    #[test]
    fn legacy_cutoff_leaves_migrations_to_apply() {
        let migrator = sqlx::migrate!("./migrations");
        let older = migrator
            .iter()
            .filter(|migration| migration.version <= LEGACY_SCHEMA_VERSION)
            .count();
        assert!(older >= 3 && older < migrator.iter().count());
        assert!(migrator
            .iter()
            .any(|migration| migration.version == LEGACY_SCHEMA_VERSION));
    }

    #[test]
    fn reports_pass_only_when_every_check_passed() {
        let mut report = Report::new(Phase::Run);
        assert!(!report.passed());
        report.record("one", Ok("fine".into()));
        assert!(report.passed());
        report.record("two", Err("broken".into()));
        assert!(!report.passed());
    }
}
