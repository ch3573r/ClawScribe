//! Finds recordings that were interrupted, from the recording folders on disk.
//!
//! The webview's IndexedDB used to be the only place recovery looked, so a
//! recording was never offered when that storage was gone, was never written
//! (an audio-only recording, a crash before the first transcript line, a Quit
//! that timed out) or the recording was started without the window. The scan
//! below reads the recording roots instead.
//!
//! A folder counts only when it carries this app's own marker (a `metadata.json`
//! in the format the saver writes) and shows one of these signs of an unfinished
//! session:
//!
//! * `unfinished_audio`: capture is still on disk (`.audio-spool` chunks or
//!   legacy `.checkpoints`) and there is no usable final audio file;
//! * `did_not_finish`: the metadata still says `recording` (or `error`) and no
//!   `recording-outcome.json` was written, which the stop path does last;
//! * `marked_incomplete`: the outcome file reports a failed audio save, an
//!   incomplete capture or incomplete recording files, and the folder is not
//!   saved in the library.
//!
//! A folder saved in the library qualifies only for `unfinished_audio`. The
//! folder of a recording that is running now, and any folder when a recording
//! is running but its folder cannot be told, is never listed. Nothing is
//! deleted here.
use super::provenance::RecordingProvenance;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DIR_ENTRIES: usize = 20_000;
const LEGACY_CHECKPOINT_SECONDS: f64 = 10.0;
const FINAL_AUDIO_NAMES: &[&str] = &[
    "audio-recovered.mp4",
    "audio-recovered.wav",
    "audio.mp4",
    "audio.m4a",
    "audio.wav",
    "audio.mp3",
    "audio.flac",
    "audio.ogg",
    "recording.mp4",
    "audio.mkv",
    "audio.webm",
    "audio.wma",
];

/// Bounds of one scan.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Folders looked at in total, across all roots and both levels.
    pub max_examined: usize,
    pub max_candidates: usize,
    pub time_limit: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_examined: 2_000,
            max_candidates: 100,
            time_limit: Duration::from_secs(4),
        }
    }
}

/// What the running recording owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActiveFolder {
    /// No recording is running.
    None,
    /// A recording is running in this folder.
    Folder(PathBuf),
    /// A recording is running and its folder cannot be told right now.
    Unknown,
}

impl ActiveFolder {
    fn canonical(&self) -> Self {
        match self {
            Self::Folder(path) => path
                .canonicalize()
                .map(Self::Folder)
                .unwrap_or(Self::Unknown),
            other => other.clone(),
        }
    }

    /// Both paths are core-owned or have already passed recording-root validation.
    /// Windows can return a different spelling for the same folder (e.g. 8.3 names).
    pub(crate) fn protects(&self, path: &Path) -> bool {
        match self {
            Self::None => false,
            Self::Unknown => true,
            Self::Folder(running) if folder_key(running) == folder_key(path) => true,
            Self::Folder(running) => match (running.canonicalize(), path.canonicalize()) {
                (Ok(a), Ok(b)) => folder_key(&a) == folder_key(&b),
                _ => true,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InterruptedRecording {
    pub folder: String,
    pub title: Option<String>,
    /// When the recording started (the marker's `created_at`).
    pub started_at: Option<String>,
    /// Approximate: the marker's duration, legacy checkpoints at ten seconds
    /// each, or the end of the last transcript line.
    pub approx_duration_seconds: Option<f64>,
    pub size_bytes: u64,
    pub has_transcript: bool,
    pub transcript_lines: u32,
    /// Capture is still on disk and can be turned into audio.
    pub has_capture: bool,
    /// There is audio to recover or keep: capture on disk or a final audio file.
    pub has_audio: bool,
    /// Why it is considered interrupted: codes (`unfinished_audio`,
    /// `did_not_finish`, `marked_incomplete`).
    pub reasons: Vec<&'static str>,
    /// The same, in a sentence for the screen.
    pub reason: String,
    /// The library meeting this folder belongs to, when it is saved.
    pub registered_meeting_id: Option<String>,
    /// The user asked not to be asked about this folder again.
    pub kept: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InterruptedScan {
    pub recordings: Vec<InterruptedRecording>,
    /// The scan stopped at a bound; there may be more.
    pub truncated: bool,
    /// A recording is running and its folder could not be told, so nothing was listed.
    pub recording_active: bool,
    /// Requested browser cache IDs that are already saved or were deleted.
    pub resolved_cache_ids: Vec<String>,
}

/// A path as text for comparison: both slash directions and (on Windows) letter
/// case are the same, a trailing separator is ignored.
pub(crate) fn folder_key(path: &Path) -> String {
    crate::local_path::comparable_text(&path.to_string_lossy())
}

fn is_link(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn read_json(path: &Path, max_bytes: u64) -> Option<Value> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > max_bytes {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn regular_file_size(path: &Path) -> Option<u64> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && !is_link(&metadata)).then_some(metadata.len())
}

/// Read duration from the selected recording's saved audio or measured marker.
/// Legacy markers may contain the last transcript timestamp, so their duration
/// is only suitable for the interrupted-recording preview.
pub(crate) fn recovered_provenance(
    folder: &Path,
    marker: &Value,
    recovered_name: Option<&str>,
) -> RecordingProvenance {
    use crate::database::meeting_files;
    let imported = marker["source"].as_str() == Some("import");
    let recorded = matches!(marker["source"].as_str(), Some("recording"))
        || matches!(
            marker["recording_mode"].as_str(),
            Some("live" | "audio_only")
        )
        || meeting_files::recording_id(marker).is_some()
        || (marker["devices"].is_object() && marker["sample_rate"].is_u64());
    if !imported && !recorded {
        return RecordingProvenance::default();
    }
    let is_saved_audio = |name: &str| {
        std::fs::symlink_metadata(folder.join(name)).is_ok_and(|metadata| {
            metadata.is_file() && !meeting_files::is_link(&metadata) && metadata.len() > 0
        })
    };
    let name = if imported {
        marker["audio_file"].as_str().filter(|name| {
            name.strip_prefix("audio.").is_some_and(|ext| {
                super::constants::AUDIO_EXTENSIONS
                    .iter()
                    .any(|allowed| ext.eq_ignore_ascii_case(allowed))
            }) && is_saved_audio(name)
        })
    } else if let Some(name) = recovered_name {
        FINAL_AUDIO_NAMES
            .contains(&name)
            .then_some(name)
            .filter(|name| is_saved_audio(name))
    } else {
        FINAL_AUDIO_NAMES
            .iter()
            .copied()
            .find(|name| is_saved_audio(name))
    };
    let duration = name
        .and_then(|name| super::import::extract_duration_from_metadata(&folder.join(name)).ok())
        .or_else(|| {
            (marker["duration_measured"].as_bool() == Some(true))
                .then(|| marker["duration_seconds"].as_f64())
                .flatten()
        });
    if imported {
        RecordingProvenance::import(duration)
    } else {
        RecordingProvenance::recording(duration, marker["created_at"].as_str())
    }
}

/// Files of a capture directory: how many chunk or temporary files, and their bytes.
fn capture_entries(directory: &Path, accept: impl Fn(&Path) -> bool) -> (u32, u64) {
    let Ok(metadata) = std::fs::symlink_metadata(directory) else {
        return (0, 0);
    };
    if !metadata.is_dir() || is_link(&metadata) {
        return (0, 0);
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return (0, 0);
    };
    let mut count = 0u32;
    let mut bytes = 0u64;
    for entry in entries.take(MAX_DIR_ENTRIES).flatten() {
        let path = entry.path();
        if !accept(&path) {
            continue;
        }
        if let Some(size) = regular_file_size(&path) {
            count += 1;
            bytes += size;
        }
    }
    (count, bytes)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|wanted| extension.eq_ignore_ascii_case(wanted))
        })
}

/// The transcript lines of the on-disk snapshot: how many and when the last one ends.
fn snapshot_summary(folder: &Path) -> (u32, Option<f64>) {
    let Some(snapshot) = read_json(&folder.join("transcripts.json"), MAX_SNAPSHOT_BYTES) else {
        return (0, None);
    };
    let Some(segments) = snapshot.get("segments").and_then(Value::as_array) else {
        return (0, None);
    };
    let lines = segments
        .iter()
        .filter(|segment| {
            segment
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
        })
        .count() as u32;
    let end = segments
        .iter()
        .filter_map(|segment| segment.get("audio_end_time").and_then(Value::as_f64))
        .fold(None, |best: Option<f64>, value| {
            Some(best.map_or(value, |best| best.max(value)))
        });
    (lines, end)
}

/// Whether and why one folder is an interrupted recording. `registered` is the
/// library meeting the folder is saved as, if any. The folder must already be
/// known not to be a link and not to be the running recording's.
fn classify_folder_with_deletions(
    folder: &Path,
    registered: Option<&str>,
    registered_ids: &HashSet<String>,
    deleted: &HashSet<String>,
) -> Option<InterruptedRecording> {
    use crate::database::meeting_files;
    let marker = crate::database::meeting_files::read_marker(folder)?;
    if meeting_files::has_invalid_marker_id(&marker) {
        return None;
    }
    let recording_id = meeting_files::recording_id(&marker);
    let stamped = meeting_files::stamped_meeting_id(&marker);
    // A different restored copy owns the ID. Never offer to save, repair or
    // discard this copy via the path that is absent from the library.
    if [stamped, recording_id]
        .into_iter()
        .flatten()
        .any(|id| registered != Some(id) && (deleted.contains(id) || registered_ids.contains(id)))
    {
        return None;
    }
    let status = marker.get("status").and_then(Value::as_str).unwrap_or("");

    let has_final_audio = FINAL_AUDIO_NAMES
        .iter()
        .any(|name| regular_file_size(&folder.join(name)).is_some_and(|size| size > 0));
    let (spool_chunks, spool_bytes) = capture_entries(&folder.join(".audio-spool"), |path| {
        has_extension(path, &["chunk", "tmp"])
    });
    let (checkpoints, checkpoint_bytes) = capture_entries(&folder.join(".checkpoints"), |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("audio_chunk_") && name.ends_with(".mp4"))
    });
    let has_capture = spool_chunks > 0 || checkpoints > 0;
    let outcome = super::outcome::RecordingOutcome::read(folder);

    let mut reasons: Vec<&'static str> = Vec::new();
    if has_capture && !has_final_audio {
        reasons.push("unfinished_audio");
    }
    if registered.is_none() {
        if status == "completed" && recording_id.is_some() {
            reasons.push("library_handoff_pending");
        }
        if status != "completed" && matches!(outcome, Ok(None)) {
            reasons.push("did_not_finish");
        }
        let flagged = match &outcome {
            Ok(Some(outcome)) => {
                outcome.audio_save_failed
                    || outcome.capture_incomplete
                    || outcome.recording_files_incomplete
            }
            // A damaged status file is a sign that something went wrong.
            Err(_) => true,
            Ok(None) => false,
        };
        if flagged {
            reasons.push("marked_incomplete");
        }
    }
    if reasons.is_empty() {
        return None;
    }

    let (transcript_lines, last_line_end) = snapshot_summary(folder);
    let snapshot_bytes = regular_file_size(&folder.join("transcripts.json")).unwrap_or(0);
    let audio_bytes: u64 = FINAL_AUDIO_NAMES
        .iter()
        .filter_map(|name| regular_file_size(&folder.join(name)))
        .sum();
    let duration = marker
        .get("duration_seconds")
        .and_then(Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .or_else(|| (checkpoints > 0).then(|| f64::from(checkpoints) * LEGACY_CHECKPOINT_SECONDS))
        .or(last_line_end.filter(|seconds| seconds.is_finite() && *seconds > 0.0));
    let reason = match reasons[0] {
        "unfinished_audio" => "The recorded audio was not finished into a file",
        "did_not_finish" => "The recording stopped unexpectedly",
        "library_handoff_pending" => "The recording ended before it could be added to your library",
        _ => "The recording ended with problems and was not saved to your library",
    }
    .to_string();
    Some(InterruptedRecording {
        folder: folder.to_string_lossy().into_owned(),
        title: marker
            .get("meeting_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string),
        started_at: marker
            .get("created_at")
            .and_then(Value::as_str)
            .map(str::to_string),
        approx_duration_seconds: duration,
        size_bytes: spool_bytes + checkpoint_bytes + snapshot_bytes + audio_bytes,
        has_transcript: transcript_lines > 0,
        transcript_lines,
        has_capture,
        has_audio: has_capture || has_final_audio,
        reasons,
        reason,
        registered_meeting_id: registered.map(str::to_string),
        kept: false,
    })
}

pub(crate) struct ScanInput<'a> {
    pub roots: &'a [PathBuf],
    /// Folders the user asked not to be asked about again.
    pub kept: &'a [String],
    /// Folder key -> library meeting id.
    pub registered: &'a HashMap<String, String>,
    pub registered_ids: &'a HashSet<String>,
    pub deleted: &'a HashSet<String>,
    pub active: ActiveFolder,
    pub limits: Limits,
}

/// Looks through the recording roots: each root's folders, and one level below a
/// folder that is not itself a recording (a container). Links are never followed.
pub(crate) fn scan(input: &ScanInput<'_>) -> InterruptedScan {
    let mut result = InterruptedScan::default();
    let active = input.active.canonical();
    if active == ActiveFolder::Unknown {
        result.recording_active = true;
        return result;
    }
    let started = Instant::now();
    let mut examined = 0usize;
    let mut seen_roots: Vec<String> = Vec::new();
    let mut seen_folders: Vec<String> = Vec::new();

    let active_key = match &active {
        ActiveFolder::Folder(path) => Some(folder_key(path)),
        _ => None,
    };

    'roots: for root in input.roots {
        let root_key = folder_key(root);
        if seen_roots.contains(&root_key) {
            continue;
        }
        seen_roots.push(root_key);
        let Ok(root_metadata) = std::fs::symlink_metadata(root) else {
            continue;
        };
        if !root_metadata.is_dir() || is_link(&root_metadata) {
            continue;
        }
        let Ok(top) = std::fs::read_dir(root) else {
            continue;
        };
        for first in top.take(MAX_DIR_ENTRIES).flatten() {
            let mut level: Vec<PathBuf> = vec![first.path()];
            // Second level: only below a folder that is not a recording itself.
            let mut index = 0;
            while index < level.len() {
                let path = level[index].clone();
                index += 1;
                if started.elapsed() >= input.limits.time_limit
                    || examined >= input.limits.max_examined
                {
                    result.truncated = true;
                    break 'roots;
                }
                examined += 1;
                let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if !metadata.is_dir() || is_link(&metadata) {
                    continue;
                }
                let key = folder_key(&path);
                if seen_folders.contains(&key) {
                    continue;
                }
                if !path.join("metadata.json").exists() {
                    // A container: look one level below, once.
                    if level.len() == 1 && index == 1 {
                        if let Ok(children) = std::fs::read_dir(&path) {
                            level.extend(
                                children
                                    .take(MAX_DIR_ENTRIES)
                                    .flatten()
                                    .map(|child| child.path()),
                            );
                        }
                    }
                    continue;
                }
                seen_folders.push(key.clone());
                if active_key.as_deref() == Some(key.as_str()) {
                    continue;
                }
                let registered = input.registered.get(&key).map(String::as_str);
                let Some(mut found) = classify_folder_with_deletions(
                    &path,
                    registered,
                    input.registered_ids,
                    input.deleted,
                ) else {
                    continue;
                };
                found.kept = input
                    .kept
                    .iter()
                    .any(|known| crate::local_path::comparable_text(known) == key);
                // The folder as the shell and the app's other commands will see it, inside a root.
                match crate::database::meeting_files::resolve_recording_folder(&path, input.roots) {
                    Ok(canonical) => {
                        if active_key.as_deref() == Some(folder_key(&canonical).as_str()) {
                            continue;
                        }
                        found.folder = crate::database::meeting_files::plain_path(&canonical)
                            .to_string_lossy()
                            .into_owned();
                    }
                    Err(_) => continue,
                }
                result.recordings.push(found);
                if result.recordings.len() >= input.limits.max_candidates {
                    result.truncated = true;
                    break 'roots;
                }
            }
        }
    }
    result.recordings.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.folder.cmp(&b.folder))
    });
    result
}

/// One line of an interrupted recording's transcript snapshot, for the preview.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewLine {
    pub text: String,
    pub timestamp: String,
    pub audio_start_time: f64,
}

pub(crate) fn preview_lines(folder: &Path, limit: usize) -> Vec<PreviewLine> {
    let Some(snapshot) = read_json(&folder.join("transcripts.json"), MAX_SNAPSHOT_BYTES) else {
        return Vec::new();
    };
    snapshot
        .get("segments")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| {
                    let text = segment.get("text").and_then(Value::as_str)?.trim();
                    (!text.is_empty()).then(|| PreviewLine {
                        text: text.to_string(),
                        timestamp: segment
                            .get("display_time")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        audio_start_time: segment
                            .get("audio_start_time")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0),
                    })
                })
                .take(limit)
                .collect()
        })
        .unwrap_or_default()
}

// ---- commands -----------------------------------------------------------------

async fn registered_rows(pool: &sqlx::SqlitePool) -> Result<Vec<(String, String)>, String> {
    sqlx::query_as("SELECT id, COALESCE(folder_path, '') FROM meetings")
        .fetch_all(pool)
        .await
        .map_err(|_| "Could not read the library".to_string())
}

async fn deleted_recording_ids(pool: &sqlx::SqlitePool) -> Result<HashSet<String>, String> {
    sqlx::query_scalar("SELECT meeting_id FROM recording_library_deletions")
        .fetch_all(pool)
        .await
        .map(|ids| ids.into_iter().collect())
        .map_err(|_| "Could not read deleted recording IDs".to_string())
}

/// Folder key -> meeting id for each saved folder, by its own text, and by its
/// resolved path where that is cheap and safe: only a folder on a local drive is
/// resolved (a network folder is never contacted for this), and only until the
/// budget is spent. A saved folder that is neither local nor spelled inside a
/// trusted root is not looked at at all. Call it off the async threads.
fn registered_map(
    rows: Vec<(String, String)>,
    roots: &[PathBuf],
    budget: Duration,
) -> HashMap<String, String> {
    let started = Instant::now();
    let mut map = HashMap::new();
    for (id, folder) in rows {
        if folder.trim().is_empty() {
            continue;
        }
        let local = crate::local_path::is_local_path_text(&folder);
        if !local && !crate::trusted_roots::text_allowed(&folder, roots) {
            continue;
        }
        map.insert(folder_key(Path::new(&folder)), id.clone());
        if local && started.elapsed() < budget {
            if let Ok(canonical) = std::fs::canonicalize(&folder) {
                map.insert(
                    folder_key(&crate::database::meeting_files::plain_path(&canonical)),
                    id,
                );
            }
        }
    }
    map
}

/// How much of a scan's time the registered-folder lookup may use.
const REGISTERED_BUDGET: Duration = Duration::from_millis(1_000);

fn kept_folders<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<String> {
    use tauri_plugin_store::StoreExt;
    app.store(crate::trusted_roots::STORE_FILE)
        .map(|store| crate::trusted_roots::read(store.as_ref()).kept)
        .unwrap_or_default()
}

/// Interrupted recordings found in the recording folders. Bounded in folders
/// examined, candidates and time; it never deletes anything.
#[tauri::command]
pub async fn find_interrupted_recordings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, crate::state::AppState>,
    cache_ids: Option<Vec<String>>,
) -> Result<InterruptedScan, String> {
    let roots = super::recording_preferences::recording_roots(&app).await;
    let rows = registered_rows(state.db_manager.pool()).await?;
    let deleted = deleted_recording_ids(state.db_manager.pool()).await?;
    let registered_ids: HashSet<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let resolved_cache_ids = {
        cache_ids
            .unwrap_or_default()
            .into_iter()
            .take(Limits::default().max_examined)
            .filter(|id| registered_ids.contains(id) || deleted.contains(id))
            .collect()
    };
    let active = super::recording_commands::active_recording_folder();
    let kept = kept_folders(&app);
    tokio::task::spawn_blocking(move || {
        // The whole scan stays inside its time limit: the registered-folder
        // lookup comes out of it.
        let started = Instant::now();
        let registered = registered_map(rows, &roots, REGISTERED_BUDGET);
        let limits = Limits::default();
        let limits = Limits {
            time_limit: limits.time_limit.saturating_sub(started.elapsed()),
            ..limits
        };
        let mut result = scan(&ScanInput {
            roots: &roots,
            kept: &kept,
            registered: &registered,
            registered_ids: &registered_ids,
            deleted: &deleted,
            active,
            limits,
        });
        result.resolved_cache_ids = resolved_cache_ids;
        result
    })
    .await
    .map_err(|_| "Could not look for interrupted recordings".to_string())
}

/// The first lines of an interrupted recording's transcript snapshot.
#[tauri::command]
pub async fn preview_interrupted_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    folder: String,
    limit: Option<usize>,
) -> Result<Vec<PreviewLine>, String> {
    let roots = super::recording_preferences::recording_roots(&app).await;
    tokio::task::spawn_blocking(move || -> Result<Vec<PreviewLine>, String> {
        let folder = crate::database::meeting_files::authorize_new_meeting_folder(&folder, &roots)?;
        Ok(preview_lines(
            Path::new(&folder),
            limit.unwrap_or(10).clamp(1, 50),
        ))
    })
    .await
    .map_err(|_| "Could not read the transcript".to_string())?
}

/// "Keep, do not ask again": remembers (or forgets) that this recording folder
/// is not to be asked about. Nothing is deleted or changed in the folder.
#[tauri::command]
pub async fn keep_interrupted_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    folder: String,
    keep: bool,
) -> Result<(), String> {
    use tauri_plugin_store::StoreExt;
    let roots = super::recording_preferences::recording_roots(&app).await;
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let authorised =
            crate::database::meeting_files::authorize_new_meeting_folder(&folder, &roots)?;
        let store = app
            .store(crate::trusted_roots::STORE_FILE)
            .map_err(|_| "Could not save the choice".to_string())?;
        crate::trusted_roots::set_kept(store.as_ref(), Path::new(&authorised), keep)
    })
    .await
    .map_err(|_| "Could not save the choice".to_string())?
}

/// What repairing a saved recording achieved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairedRecording {
    pub meeting_id: String,
    /// `success`, `partial`, `failed` or `none`, as audio recovery reports it.
    pub audio_status: String,
    pub audio_path: Option<String>,
}

/// Repairs the library meeting a folder already belongs to: the unfinished
/// capture in its folder becomes its audio file, the capture is released, and
/// the meeting's recovery status is updated. It never creates another meeting.
#[tauri::command]
pub async fn repair_saved_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, crate::state::AppState>,
    folder: String,
) -> Result<RepairedRecording, String> {
    let job = super::inference::claim_job().map_err(|_| {
        "Another operation is using the recorder. Try repairing again when it finishes.".to_string()
    })?;
    let roots = super::recording_preferences::recording_roots(&app).await;
    let rows = registered_rows(state.db_manager.pool()).await?;
    let active = super::recording_commands::active_recording_folder();
    let (authorised, meeting_id, registered_path, marker) =
        tokio::task::spawn_blocking(move || {
            let authorised =
                crate::database::meeting_files::authorize_new_meeting_folder(&folder, &roots)?;
            if active.protects(Path::new(&authorised)) {
                return Err("A recording is running. Try again when it has stopped.".into());
            }
            let registered = registered_map(rows.clone(), &roots, REGISTERED_BUDGET);
            let meeting = registered
                .get(&folder_key(Path::new(&authorised)))
                .or_else(|| {
                    std::fs::canonicalize(&authorised)
                        .ok()
                        .and_then(|canonical| {
                            registered.get(&folder_key(
                                &crate::database::meeting_files::plain_path(&canonical),
                            ))
                        })
                })
                .cloned()
                .ok_or_else(|| "This recording is not saved in your library".to_string())?;
            let registered_path = rows
                .into_iter()
                .find_map(|(id, path)| (id == meeting).then_some(path))
                .ok_or("The recording is no longer saved in your library.")?;
            let marker = crate::database::meeting_files::read_marker(Path::new(&authorised));
            Ok::<_, String>((authorised, meeting, registered_path, marker))
        })
        .await
        .map_err(|_| "Could not repair the recording".to_string())??;

    let status = super::incremental_saver::recover_audio_in_folder(authorised.clone()).await?;
    let recovered =
        status.audio_file_path.is_some() && matches!(status.status.as_str(), "success" | "partial");
    if recovered {
        let transcribes = marker
            .as_ref()
            .is_some_and(super::recording_mode::marker_transcribes);
        let provenance = {
            let folder = authorised.clone();
            let recovered_path = status.audio_file_path.clone();
            tokio::task::spawn_blocking(move || {
                let name = recovered_path
                    .as_deref()
                    .and_then(|path| Path::new(path).file_name())
                    .and_then(std::ffi::OsStr::to_str);
                marker
                    .as_ref()
                    .zip(name)
                    .map_or_else(RecordingProvenance::default, |(marker, name)| {
                        recovered_provenance(Path::new(&folder), marker, Some(name))
                    })
            })
            .await
            .map_err(|_| "Could not read recovered recording details".to_string())?
        };
        let outcome_updated = commit_repaired_audio(
            state.db_manager.pool(),
            &meeting_id,
            &registered_path,
            &authorised,
            provenance,
            status.status == "partial",
        )
        .await?;
        drop(job);
        if transcribes && outcome_updated {
            super::recording_commands::wake_saved_recording_summary(&app, meeting_id.clone());
        }
    }
    Ok(RepairedRecording {
        meeting_id,
        audio_status: status.status,
        audio_path: status.audio_file_path,
    })
}

async fn commit_repaired_audio(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    registered_path: &str,
    authorised: &str,
    provenance: RecordingProvenance,
    partial: bool,
) -> Result<bool, String> {
    // The meeting no longer has a failed audio save; a partial recovery says
    // the capture had gaps.
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| "Could not save recovered recording details")?;
    let meeting = sqlx::query(
        "UPDATE meetings SET recording_source = COALESCE(recording_source, ?),
         recording_duration_s = COALESCE(recording_duration_s, ?)
         WHERE id = ? AND folder_path = ?",
    )
    .bind(provenance.source)
    .bind(provenance.duration_seconds)
    .bind(meeting_id)
    .bind(registered_path)
    .execute(&mut *tx)
    .await
    .map_err(|_| "Could not save recovered recording details")?;
    if meeting.rows_affected() != 1 {
        // Keep the recovered file and its originals available to their owner.
        return Err("The recording was moved in your library during repair. \
             Reload the library and try again."
            .into());
    }
    let updated = sqlx::query(
        "UPDATE recording_outcomes SET audio_save_failed = 0, capture_incomplete = capture_incomplete OR ? WHERE meeting_id = ?",
    )
    .bind(i64::from(partial))
    .bind(meeting_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| "Could not save recovered recording status")?;
    tx.commit()
        .await
        .map_err(|_| "Could not save recovered recording details")?;
    // Release capture only once the repaired meeting is committed.
    let _ = super::incremental_saver::release_registered_capture(pool, authorised).await;
    if !partial {
        let checkpoints = Path::new(authorised).join(".checkpoints");
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(checkpoints)).await;
    }
    Ok(updated.rows_affected() == 1)
}

/// Deletes an interrupted recording's folder, only on an explicit request and
/// only for a folder that is still a candidate, is not saved in the library and
/// is not the running recording's. The deletion is the validated cleanup the
/// library uses for a deleted meeting.
#[tauri::command]
pub async fn discard_interrupted_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, crate::state::AppState>,
    folder: String,
) -> Result<(), String> {
    let roots = super::recording_preferences::recording_roots(&app).await;
    let rows = registered_rows(state.db_manager.pool()).await?;
    let deleted = deleted_recording_ids(state.db_manager.pool()).await?;
    let active = super::recording_commands::active_recording_folder();
    let recording_id = tokio::task::spawn_blocking(move || {
        let registered_ids = rows.iter().map(|(id, _)| id.clone()).collect();
        let registered = registered_map(rows, &roots, REGISTERED_BUDGET);
        discard_folder(
            &folder,
            &roots,
            &registered,
            &registered_ids,
            &active,
            &deleted,
        )
    })
    .await
    .map_err(|_| "Could not delete the recording".to_string())??;
    if let Some(recording_id) = recording_id {
        crate::library::discard_recording_drafts(state.db_manager.pool(), &recording_id)
            .await
            .map_err(|_| {
                "The recording was deleted, but its pending notes or calendar link could not be removed."
            })?;
    }
    Ok(())
}

pub(crate) fn discard_folder(
    folder: &str,
    roots: &[PathBuf],
    registered: &HashMap<String, String>,
    registered_ids: &HashSet<String>,
    active: &ActiveFolder,
    deleted: &HashSet<String>,
) -> Result<Option<String>, String> {
    const REFUSED: &str = "This recording cannot be deleted from here.";
    let authorised = crate::database::meeting_files::authorize_new_meeting_folder(folder, roots)?;
    let path = PathBuf::from(&authorised);
    let key = folder_key(&path);
    let canonical_key = std::fs::canonicalize(&path)
        .ok()
        .map(|canonical| folder_key(&crate::database::meeting_files::plain_path(&canonical)));
    if active.protects(&path) {
        return Err("A recording is running. Try again when it has stopped.".into());
    }
    if registered.contains_key(&key)
        || canonical_key
            .as_ref()
            .is_some_and(|key| registered.contains_key(key))
    {
        return Err(REFUSED.into());
    }
    if classify_folder_with_deletions(&path, None, registered_ids, deleted).is_none() {
        return Err(REFUSED.into());
    }
    let others: Vec<PathBuf> = registered.keys().map(PathBuf::from).collect();
    let marker = crate::database::meeting_files::read_marker(&path).ok_or(REFUSED)?;
    let id = crate::database::meeting_files::stamped_meeting_id(&marker)
        .or_else(|| crate::database::meeting_files::recording_id(&marker));
    crate::database::meeting_files::remove_recording_folder(
        &path,
        id.unwrap_or(""),
        roots,
        &others,
    )?;
    Ok(crate::database::meeting_files::recording_id(&marker)
        .map(str::to_owned)
        .or_else(|| id.map(str::to_owned)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_stop_with_pending_handoff_is_recoverable_until_saved_or_deleted() {
        let base = tempfile::tempdir().unwrap();
        let folder = base.path();
        let id = "meeting-4f7b980e-b123-4ca9-82e7-927204456483";
        marker(folder, "completed", None);
        let path = folder.join("metadata.json");
        let mut metadata: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        metadata["recording_id"] = serde_json::json!(id);
        std::fs::write(path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        std::fs::write(folder.join("audio.mp4"), [1u8]).unwrap();
        let mut deleted = std::collections::HashSet::new();
        assert_eq!(
            classify_folder_with_deletions(folder, None, &HashSet::new(), &deleted)
                .unwrap()
                .reasons,
            ["library_handoff_pending"]
        );
        assert!(
            classify_folder_with_deletions(folder, Some(id), &HashSet::new(), &deleted).is_none()
        );
        let elsewhere = HashSet::from([id.to_owned()]);
        assert!(classify_folder_with_deletions(folder, None, &elsewhere, &deleted).is_none());
        deleted.insert(id.to_owned());
        assert!(classify_folder_with_deletions(folder, None, &HashSet::new(), &deleted).is_none());
    }

    #[test]
    fn a_stamped_unfinished_recording_is_recoverable_in_a_fresh_library() {
        let base = tempfile::tempdir().unwrap();
        let roots = [base.path().to_path_buf()];
        let folder = make(base.path(), "unfinished");
        let id = "meeting-4f7b980e-b123-4ca9-82e7-927204456483";
        marker(&folder, "error", Some(id));
        spool(&folder, 2);
        let registered = HashSet::from([id.to_owned()]);
        let empty = HashSet::new();
        assert_eq!(
            classify_folder_with_deletions(&folder, None, &empty, &empty)
                .unwrap()
                .reasons[0],
            "unfinished_audio",
        );
        assert!(classify_folder_with_deletions(&folder, None, &empty, &registered).is_none());
        assert!(discard_folder(
            &folder.to_string_lossy(),
            &roots,
            &HashMap::new(),
            &registered,
            &ActiveFolder::None,
            &empty
        )
        .is_err());
        assert!(folder.is_dir());
    }

    fn marker(folder: &Path, status: &str, id: Option<&str>) {
        let mut value = serde_json::json!({
            "version": "1.0", "meeting_name": "Weekly sync", "transcript_file": "transcripts.json",
            "created_at": "2026-11-01T10:00:00Z", "status": status,
        });
        if let Some(id) = id {
            value["meeting_id"] = serde_json::json!(id);
        }
        std::fs::write(folder.join("metadata.json"), value.to_string()).unwrap();
    }

    fn spool(folder: &Path, chunks: usize) {
        let dir = folder.join(".audio-spool");
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..chunks {
            std::fs::write(dir.join(format!("{index:020}.chunk")), vec![1u8; 100]).unwrap();
        }
    }

    fn snapshot(folder: &Path, lines: usize) {
        let segments: Vec<Value> = (0..lines)
            .map(|index| {
                serde_json::json!({"text": format!("line {index}"), "audio_start_time": index as f64, "audio_end_time": index as f64 + 1.0, "display_time": "[00:00]"})
            })
            .collect();
        std::fs::write(
            folder.join("transcripts.json"),
            serde_json::json!({"version": "1.0", "segments": segments}).to_string(),
        )
        .unwrap();
    }

    fn outcome(folder: &Path, flags: serde_json::Value) {
        std::fs::write(folder.join("recording-outcome.json"), flags.to_string()).unwrap();
    }

    fn make(root: &Path, name: &str) -> PathBuf {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn run(
        roots: &[PathBuf],
        registered: &HashMap<String, String>,
        active: ActiveFolder,
        limits: Limits,
    ) -> InterruptedScan {
        scan(&ScanInput {
            roots,
            kept: &[],
            registered,
            registered_ids: &registered.values().cloned().collect(),
            deleted: &HashSet::new(),
            active,
            limits,
        })
    }

    fn names(scan: &InterruptedScan) -> Vec<String> {
        let mut names: Vec<String> = scan
            .recordings
            .iter()
            .map(|found| {
                Path::new(&found.folder)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn classification_of_finished_running_interrupted_and_foreign_folders() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        std::fs::create_dir_all(&root).unwrap();

        // Finished meeting: completed, audio, outcome, no capture.
        let finished = make(&root, "finished");
        marker(&finished, "completed", None);
        std::fs::write(finished.join("audio.mp4"), b"audio").unwrap();
        outcome(
            &finished,
            serde_json::json!({"audio_save_failed": false, "transcription_incomplete": false}),
        );

        // Running recording: still says recording, spool growing.
        let running = make(&root, "running");
        marker(&running, "recording", None);
        spool(&running, 3);

        // Interrupted, with a transcript snapshot.
        let with_text = make(&root, "interrupted-with-text");
        marker(&with_text, "recording", None);
        spool(&with_text, 5);
        snapshot(&with_text, 4);

        // Interrupted, without a snapshot and without capture (crash before audio).
        let bare = make(&root, "interrupted-bare");
        marker(&bare, "recording", None);

        // Foreign folder with a generic metadata.json, and an outcome-only folder.
        let foreign = make(&root, "foreign");
        std::fs::write(foreign.join("metadata.json"), b"{}").unwrap();
        spool(&foreign, 2);
        let plain = make(&root, "plain");
        std::fs::write(plain.join("notes.txt"), b"x").unwrap();

        let result = run(
            &[root.clone()],
            &HashMap::new(),
            ActiveFolder::Folder(running.clone()),
            Limits::default(),
        );
        assert_eq!(
            names(&result),
            vec!["interrupted-bare", "interrupted-with-text"]
        );
        let found = result
            .recordings
            .iter()
            .find(|found| found.folder.ends_with("interrupted-with-text"))
            .unwrap();
        assert!(found.has_transcript && found.transcript_lines == 4 && found.has_capture);
        assert_eq!(found.reasons, vec!["unfinished_audio", "did_not_finish"]);
        assert_eq!(found.title.as_deref(), Some("Weekly sync"));
        assert_eq!(found.started_at.as_deref(), Some("2026-11-01T10:00:00Z"));
        assert_eq!(found.approx_duration_seconds, Some(4.0));
        assert!(found.size_bytes >= 500);
        let bare = result
            .recordings
            .iter()
            .find(|found| found.folder.ends_with("interrupted-bare"))
            .unwrap();
        assert!(!bare.has_transcript && !bare.has_capture && !bare.has_audio);
        assert!(found.has_audio);
        assert_eq!(bare.reasons, vec!["did_not_finish"]);
        assert!(!result.truncated && !result.recording_active);
    }

    #[test]
    fn the_running_recording_is_not_listed_and_an_unknown_one_lists_nothing() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let running = make(&root, "running");
        marker(&running, "recording", None);
        spool(&running, 1);
        let other = make(&root, "other");
        marker(&other, "recording", None);
        let with_active = run(
            &[root.clone()],
            &HashMap::new(),
            ActiveFolder::Folder(running.clone()),
            Limits::default(),
        );
        assert_eq!(names(&with_active), vec!["other"]);
        // The same folder spelled differently is still the running one.
        let spelled = PathBuf::from(format!("{}/", running.to_string_lossy()));
        assert_eq!(
            names(&run(
                &[root.clone()],
                &HashMap::new(),
                ActiveFolder::Folder(spelled),
                Limits::default()
            )),
            vec!["other"]
        );
        let unknown = run(
            &[root.clone()],
            &HashMap::new(),
            ActiveFolder::Unknown,
            Limits::default(),
        );
        assert!(unknown.recordings.is_empty() && unknown.recording_active);
        assert_eq!(
            names(&run(
                &[root],
                &HashMap::new(),
                ActiveFolder::None,
                Limits::default()
            )),
            vec!["other", "running"]
        );
    }

    #[test]
    fn a_saved_meeting_is_listed_only_for_unfinished_audio() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let saved_ok = make(&root, "saved-ok");
        marker(&saved_ok, "recording", None);
        let saved_finished = make(&root, "saved-with-audio");
        marker(&saved_finished, "completed", None);
        spool(&saved_finished, 2);
        std::fs::write(saved_finished.join("audio.mp4"), b"audio").unwrap();
        let saved_unfinished = make(&root, "saved-no-audio");
        marker(&saved_unfinished, "completed", None);
        spool(&saved_unfinished, 2);
        let mut registered = HashMap::new();
        for (folder, id) in [
            (&saved_ok, "meeting-1"),
            (&saved_finished, "meeting-2"),
            (&saved_unfinished, "meeting-3"),
        ] {
            registered.insert(folder_key(folder), id.to_string());
        }
        let result = run(&[root], &registered, ActiveFolder::None, Limits::default());
        assert_eq!(names(&result), vec!["saved-no-audio"]);
        assert_eq!(
            result.recordings[0].registered_meeting_id.as_deref(),
            Some("meeting-3")
        );
        assert_eq!(result.recordings[0].reasons, vec!["unfinished_audio"]);
    }

    #[test]
    fn an_unsaved_folder_marked_incomplete_is_listed_and_a_clean_one_is_not() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let failed = make(&root, "audio-failed");
        marker(&failed, "completed", None);
        outcome(
            &failed,
            serde_json::json!({"audio_save_failed": true, "transcription_incomplete": false}),
        );
        let gaps = make(&root, "capture-gaps");
        marker(&gaps, "completed", None);
        std::fs::write(gaps.join("audio.mp4"), b"audio").unwrap();
        outcome(
            &gaps,
            serde_json::json!({"audio_save_failed": false, "transcription_incomplete": false, "capture_incomplete": true}),
        );
        let damaged = make(&root, "damaged-status");
        marker(&damaged, "completed", None);
        std::fs::write(damaged.join("recording-outcome.json"), b"not json").unwrap();
        let clean = make(&root, "clean");
        marker(&clean, "completed", None);
        std::fs::write(clean.join("audio.mp4"), b"audio").unwrap();
        outcome(
            &clean,
            serde_json::json!({"audio_save_failed": false, "transcription_incomplete": false}),
        );
        let result = run(
            &[root],
            &HashMap::new(),
            ActiveFolder::None,
            Limits::default(),
        );
        assert_eq!(
            names(&result),
            vec!["audio-failed", "capture-gaps", "damaged-status"]
        );
        assert!(result
            .recordings
            .iter()
            .all(|found| found.reasons == vec!["marked_incomplete"]));
    }

    #[test]
    fn legacy_checkpoints_count_as_capture_and_estimate_the_duration() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let folder = make(&root, "legacy");
        marker(&folder, "completed", None);
        std::fs::create_dir_all(folder.join(".checkpoints")).unwrap();
        for index in 0..3 {
            std::fs::write(
                folder
                    .join(".checkpoints")
                    .join(format!("audio_chunk_{index:05}.mp4")),
                b"chunk",
            )
            .unwrap();
        }
        std::fs::write(
            folder.join(".checkpoints").join("concat_list.txt"),
            b"ignored",
        )
        .unwrap();
        let result = run(
            &[root],
            &HashMap::new(),
            ActiveFolder::None,
            Limits::default(),
        );
        assert_eq!(result.recordings.len(), 1);
        assert_eq!(result.recordings[0].approx_duration_seconds, Some(30.0));
        assert_eq!(result.recordings[0].reasons, vec!["unfinished_audio"]);
    }

    #[test]
    fn links_and_folders_outside_the_roots_are_never_listed() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        std::fs::create_dir_all(&root).unwrap();
        let outside = make(base.path(), "outside");
        marker(&outside, "recording", None);
        spool(&outside, 1);
        let inside = make(&root, "inside");
        marker(&inside, "recording", None);
        // A link inside the root that points at the outside recording.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let status = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(root.join("looks-inside"))
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(status.status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("looks-inside")).unwrap();
        let result = run(
            &[root.clone()],
            &HashMap::new(),
            ActiveFolder::None,
            Limits::default(),
        );
        assert_eq!(names(&result), vec!["inside"]);
        // The folder outside every root is not scanned at all.
        assert!(result
            .recordings
            .iter()
            .all(|found| !found.folder.ends_with("outside")));
        #[cfg(windows)]
        std::fs::remove_dir(root.join("looks-inside")).unwrap();
    }

    #[test]
    fn a_container_folder_is_looked_into_one_level_and_no_deeper() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let container = make(&root, "restored-batch");
        let one = make(&container, "one");
        marker(&one, "recording", None);
        let too_deep = make(&make(&container, "nested"), "two");
        marker(&too_deep, "recording", None);
        let result = run(
            &[root],
            &HashMap::new(),
            ActiveFolder::None,
            Limits::default(),
        );
        assert_eq!(names(&result), vec!["one"]);
    }

    #[test]
    fn the_scan_stops_at_its_bounds_and_says_so() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        for index in 0..30 {
            let folder = make(&root, &format!("meeting-{index:02}"));
            marker(&folder, "recording", None);
        }
        let capped = run(
            &[root.clone()],
            &HashMap::new(),
            ActiveFolder::None,
            Limits {
                max_candidates: 5,
                ..Limits::default()
            },
        );
        assert_eq!(capped.recordings.len(), 5);
        assert!(capped.truncated);
        let examined = run(
            &[root.clone()],
            &HashMap::new(),
            ActiveFolder::None,
            Limits {
                max_examined: 10,
                ..Limits::default()
            },
        );
        assert!(examined.recordings.len() <= 10 && examined.truncated);
        let timed = run(
            &[root.clone()],
            &HashMap::new(),
            ActiveFolder::None,
            Limits {
                time_limit: Duration::ZERO,
                ..Limits::default()
            },
        );
        assert!(timed.recordings.is_empty() && timed.truncated);
        let full = run(
            &[root],
            &HashMap::new(),
            ActiveFolder::None,
            Limits::default(),
        );
        assert_eq!(full.recordings.len(), 30);
        assert!(!full.truncated);
    }

    #[test]
    fn roots_listed_twice_or_missing_are_handled_and_results_are_newest_first() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let older = make(&root, "older");
        marker(&older, "recording", None);
        let newer = make(&root, "newer");
        std::fs::write(
            newer.join("metadata.json"),
            serde_json::json!({"version": "1.0", "transcript_file": "t.json", "created_at": "2026-12-01T10:00:00Z", "status": "recording"}).to_string(),
        )
        .unwrap();
        let result = run(
            &[root.clone(), root.clone(), base.path().join("missing")],
            &HashMap::new(),
            ActiveFolder::None,
            Limits::default(),
        );
        assert_eq!(result.recordings.len(), 2);
        assert!(result.recordings[0].folder.ends_with("newer"));
    }

    #[test]
    fn a_kept_folder_is_flagged_and_still_listed() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let keptf = make(&root, "kept");
        marker(&keptf, "recording", None);
        let other = make(&root, "other");
        marker(&other, "recording", None);
        let kept = vec![format!(
            "{}/",
            keptf.to_string_lossy().to_uppercase().replace('\\', "/")
        )];
        let result = scan(&ScanInput {
            roots: &[root],
            kept: &kept,
            registered: &HashMap::new(),
            registered_ids: &HashSet::new(),
            deleted: &HashSet::new(),
            active: ActiveFolder::None,
            limits: Limits::default(),
        });
        assert_eq!(result.recordings.len(), 2);
        for found in &result.recordings {
            #[cfg(windows)]
            assert_eq!(
                found.kept,
                found.folder.ends_with("kept"),
                "{}",
                found.folder
            );
            #[cfg(not(windows))]
            let _ = found;
        }
    }

    #[test]
    fn the_registered_lookup_never_contacts_a_network_folder_outside_a_trusted_root() {
        let base = tempfile::tempdir().unwrap();
        let share = base.path().join("share");
        let trusted = share.join("recordings");
        let stranger = share.join("stranger");
        for folder in [&trusted, &stranger] {
            std::fs::create_dir_all(folder.join("m")).unwrap();
        }
        crate::local_path::mark_remote_for_tests(&share);
        let rows = vec![
            (
                "in-root".to_string(),
                trusted.join("m").to_string_lossy().into_owned(),
            ),
            (
                "stranger".to_string(),
                stranger.join("m").to_string_lossy().into_owned(),
            ),
            (
                "local".to_string(),
                base.path().to_string_lossy().into_owned(),
            ),
        ];
        let map = registered_map(rows, &[trusted.clone()], Duration::from_secs(5));
        assert_eq!(
            map.get(&folder_key(&trusted.join("m"))).map(String::as_str),
            Some("in-root")
        );
        assert!(
            !map.contains_key(&folder_key(&stranger.join("m"))),
            "not inside a trusted root: skipped"
        );
        assert!(
            map.values().any(|id| id == "local"),
            "a local folder is looked up"
        );
        // With no time left only the text keys are used.
        let rushed = registered_map(
            vec![("local".into(), base.path().to_string_lossy().into_owned())],
            &[],
            Duration::ZERO,
        );
        assert_eq!(rushed.len(), 1);
    }

    #[test]
    fn a_scan_of_a_trusted_network_root_finds_its_interrupted_recordings() {
        let base = tempfile::tempdir().unwrap();
        let share = base.path().join("share");
        let root = share.join("recordings");
        let folder = make(&root, "meeting");
        marker(&folder, "recording", None);
        crate::local_path::mark_remote_for_tests(&share);
        let result = scan(&ScanInput {
            roots: &[root],
            kept: &[],
            registered: &HashMap::new(),
            registered_ids: &HashSet::new(),
            deleted: &HashSet::new(),
            active: ActiveFolder::None,
            limits: Limits::default(),
        });
        assert_eq!(names(&result), vec!["meeting"]);
    }

    #[test]
    fn the_preview_reads_only_the_first_lines_of_the_snapshot() {
        let base = tempfile::tempdir().unwrap();
        snapshot(base.path(), 30);
        let lines = preview_lines(base.path(), 10);
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0].text, "line 0");
        assert!(preview_lines(&base.path().join("missing"), 10).is_empty());
    }

    #[test]
    fn active_folder_uses_directory_identity_and_refuses_an_unresolved_owner() {
        let base = tempfile::tempdir().unwrap();
        let running = make(base.path(), "running");
        let other = make(base.path(), "other");
        let alias = other.join("..").join("running");
        let active = ActiveFolder::Folder(alias);
        assert!(active.protects(&running));
        assert!(!active.protects(&other));
        assert!(ActiveFolder::Folder(base.path().join("unavailable")).protects(&other));
    }

    #[test]
    fn discarding_removes_only_an_unsaved_candidate_that_is_not_running() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let victim = make(&root, "victim");
        marker(&victim, "recording", None);
        spool(&victim, 2);
        let saved = make(&root, "saved");
        marker(&saved, "recording", Some("meeting-9"));
        spool(&saved, 1);
        let running = make(&root, "running");
        marker(&running, "recording", None);
        let finished = make(&root, "finished");
        marker(&finished, "completed", None);
        std::fs::write(finished.join("audio.mp4"), b"audio").unwrap();
        let foreign = make(&root, "foreign");
        std::fs::write(foreign.join("metadata.json"), b"{}").unwrap();
        let outside = make(base.path(), "outside");
        marker(&outside, "recording", None);
        let roots = [root.clone()];
        let mut registered = HashMap::new();
        registered.insert(folder_key(&saved), "meeting-9".to_string());
        let active = ActiveFolder::Folder(running.clone());
        for refused in [
            &saved,
            &running,
            &finished,
            &foreign,
            &outside,
            &root.join("missing"),
        ] {
            let existed = refused.exists();
            assert!(
                discard_folder(
                    &refused.to_string_lossy(),
                    &roots,
                    &registered,
                    &HashSet::new(),
                    &active,
                    &HashSet::new()
                )
                .is_err(),
                "{}",
                refused.display()
            );
            assert_eq!(
                refused.exists(),
                existed,
                "the refused folder must be left alone"
            );
        }
        assert!(discard_folder(
            r"\\server\share\victim",
            &roots,
            &registered,
            &HashSet::new(),
            &active,
            &HashSet::new()
        )
        .is_err());
        assert!(discard_folder(
            &victim.to_string_lossy(),
            &roots,
            &registered,
            &HashSet::new(),
            &ActiveFolder::Unknown,
            &HashSet::new()
        )
        .is_err());
        assert!(
            victim.exists(),
            "nothing is deleted while a recording's folder is unknown"
        );
        discard_folder(
            &victim.to_string_lossy(),
            &roots,
            &registered,
            &HashSet::new(),
            &active,
            &HashSet::new(),
        )
        .unwrap();
        assert!(!victim.exists());
        assert!(
            saved.exists()
                && running.exists()
                && finished.exists()
                && foreign.exists()
                && outside.exists()
        );
    }

    #[tokio::test]
    async fn a_moved_library_link_preserves_the_capture_for_a_repair_retry() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().to_str().unwrap();
        let checkpoints = folder.path().join(".checkpoints");
        std::fs::create_dir(&checkpoints).unwrap();
        let chunk = checkpoints.join("audio_chunk_000.mp4");
        std::fs::write(&chunk, b"synthetic checkpoint").unwrap();
        let recovered = folder.path().join("audio-recovered.wav");
        std::fs::write(&recovered, b"synthetic materialised audio").unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path)
                     VALUES ('repair', 'Test', datetime('now'), datetime('now'), 'changed')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO recording_outcomes (meeting_id, audio_save_failed) VALUES ('repair', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let provenance = RecordingProvenance::recording(Some(5.0), None);
        assert!(
            commit_repaired_audio(&pool, "repair", path, path, provenance, false)
                .await
                .unwrap_err()
                .contains("moved")
        );
        assert!(chunk.is_file() && recovered.is_file());
        let failed: i64 = sqlx::query_scalar(
            "SELECT audio_save_failed FROM recording_outcomes WHERE meeting_id = 'repair'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(failed, 1);

        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'repair'")
            .bind(path)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            commit_repaired_audio(&pool, "repair", path, path, provenance, false)
                .await
                .unwrap()
        );
        assert!(!checkpoints.exists() && recovered.is_file());
        let stored: (String, f64, i64) = sqlx::query_as(
            "SELECT recording_source, recording_duration_s, audio_save_failed FROM meetings
             JOIN recording_outcomes ON meeting_id = id WHERE id = 'repair'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored, ("recording".into(), 5.0, 0));
    }
}
