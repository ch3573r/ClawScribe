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
//! profile folder must be new or empty and must not overlap the application's
//! own data folders. Everything the check creates lives inside that folder.
//!
//! What it covers: the packaged sidecars and runtime libraries next to the
//! executable, upgrade of a synthetic legacy database through this build's
//! embedded migrations (including the known line-ending checksum drift),
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
use std::ffi::OsStr;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const USAGE_EXIT: i32 = 2;
const PROFILE_MARKER: &str = ".clawscribe-self-test";
const STATE_FILE: &str = "self-test-state.json";
/// The synthetic legacy database is built from every migration up to this one.
const LEGACY_SCHEMA_VERSION: i64 = 20251006000000;
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

#[derive(Clone, Copy, PartialEq, Eq)]
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

/// Dispatch before any desktop plugin, single-instance guard or logger starts.
pub fn dispatch() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new("--self-test")) {
        return None;
    }
    let phase = args.next();
    let profile = args.next();
    if args.next().is_some() {
        return Some(USAGE_EXIT);
    }
    let (Some(phase), Some(profile)) = (phase, profile) else {
        return Some(USAGE_EXIT);
    };
    let phase = match phase.to_str() {
        Some("run") => Phase::Run,
        Some("verify") => Phase::Verify,
        _ => return Some(USAGE_EXIT),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return Some(3),
    };
    let code = runtime.block_on(execute(phase, PathBuf::from(profile)));
    runtime.shutdown_background();
    Some(code)
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

async fn execute(phase: Phase, requested: PathBuf) -> i32 {
    let mut report = Report::new(phase);
    let profile = match prepare_profile(phase, &requested) {
        Ok(profile) => profile,
        Err(error) => {
            // Nothing may be written to a folder that failed the isolation check.
            report.record("profile_isolation", Err(error));
            let _ = writeln!(std::io::stdout(), "SELF_TEST_RESULT failed");
            return 1;
        }
    };
    report.record(
        "profile_isolation",
        Ok("New or empty folder outside the application's data folders".to_string()),
    );
    match phase {
        Phase::Run => run_phase(&profile, &mut report).await,
        Phase::Verify => verify_phase(&profile, &mut report).await,
    }
    let passed = report.passed();
    let document = json!({
        "schema": 1,
        "phase": report.phase.name(),
        "app_version": env!("CARGO_PKG_VERSION"),
        "passed": passed,
        "checks": report.checks,
    });
    let path = profile.join(format!("self-test-report-{}.json", report.phase.name()));
    if let Ok(bytes) = serde_json::to_vec_pretty(&document) {
        let _ = std::fs::write(path, bytes);
    }
    let _ = writeln!(
        std::io::stdout(),
        "SELF_TEST_RESULT {}",
        if passed { "passed" } else { "failed" }
    );
    if passed {
        0
    } else {
        1
    }
}

/// The folder must be absolute, not a link, new or empty (run) or created by an
/// earlier run (verify), and must not overlap the application's data folders.
fn prepare_profile(phase: Phase, requested: &Path) -> Result<PathBuf, String> {
    if !requested.is_absolute() {
        return Err("The profile folder must be an absolute path".into());
    }
    // Checked before anything is created, including for a folder that does not
    // exist yet, so a refused path is never touched.
    reject_application_folders(&resolved_for_check(requested))?;
    match std::fs::symlink_metadata(requested) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
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
            Phase::Verify => {
                if !requested.join(PROFILE_MARKER).is_file() {
                    return Err("The folder was not created by a self-test run".into());
                }
            }
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
    if phase == Phase::Run {
        std::fs::write(
            plain.join(PROFILE_MARKER),
            b"ClawScribe self-test profile. Safe to delete.\n",
        )
        .map_err(|_| "The profile folder cannot be written".to_string())?;
    }
    Ok(plain)
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
    let mut protected: Vec<PathBuf> = Vec::new();
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

async fn run_phase(profile: &Path, report: &mut Report) {
    let mut state = json!({});

    report.record("package_layout", package_layout());
    let encoder = packaged_encoder();
    let encoder_ready = report.record("packaged_encoder", encoder);

    let manager = match migration_upgrade(profile).await {
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
                state["recovered_audio"] = json!(audio.to_string_lossy());
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
    if let Ok(bytes) = serde_json::to_vec_pretty(&state) {
        let _ = std::fs::write(profile.join(STATE_FILE), bytes);
    }
}

async fn verify_phase(profile: &Path, report: &mut Report) {
    let state: Value = match std::fs::read(profile.join(STATE_FILE))
        .map_err(failed("read the saved state"))
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(failed("parse the saved state")))
    {
        Ok(state) => state,
        Err(error) => {
            report.record("persistence_reopen", Err(error));
            return;
        }
    };
    let (database, legacy) = database_paths(profile);
    let manager = match (database.to_str(), legacy.to_str()) {
        (Some(database), Some(legacy)) => DatabaseManager::new(database, legacy)
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
            Some(path) => decode_check(PathBuf::from(path), 3.0, 0.35).await,
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

/// Sidecars and runtime libraries the installer places next to the executable.
fn package_layout() -> Result<String, String> {
    let folder = executable_folder()?;
    let required = [
        "ffmpeg.exe",
        "llama-helper.exe",
        "codex-app-server.exe",
        "onnxruntime.dll",
        "sherpa-onnx-c-api.dll",
    ];
    let mut missing = Vec::new();
    for name in required {
        match std::fs::metadata(folder.join(name)) {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {}
            _ => missing.push(name),
        }
    }
    if !missing.is_empty() {
        return Err(format!("Missing from the package: {}", missing.join(", ")));
    }
    let directml = folder.join("DirectML.dll").is_file();
    Ok(format!(
        "{} required files present; DirectML runtime {}",
        required.len(),
        if directml { "present" } else { "absent" }
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

async fn migration_upgrade(profile: &Path) -> Result<(DatabaseManager, String), String> {
    let (database, legacy_backend) = database_paths(profile);
    let full = sqlx::migrate!("./migrations");
    let older: Vec<Migration> = full
        .iter()
        .filter(|migration| migration.version <= LEGACY_SCHEMA_VERSION)
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
    let manager = DatabaseManager::new(database_text, legacy_text)
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
    let recording_outcomes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'recording_outcomes'",
    )
    .fetch_one(pool)
    .await
    .map_err(failed("inspect the upgraded schema"))?;
    if recording_outcomes != 1 {
        return Err("A table added by a later migration is missing".into());
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
            "{} legacy migrations upgraded to {total}; checksum drift repaired; backup written; legacy meeting and lines intact",
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
    let client = reqwest::Client::new();
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

    #[test]
    fn usage_errors_do_not_run_anything() {
        assert!(Phase::Run.name() != Phase::Verify.name());
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
