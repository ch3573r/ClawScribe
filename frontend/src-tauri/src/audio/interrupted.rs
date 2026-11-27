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
//! A folder saved in the library qualifies for unfinished audio. The exact
//! recording ID can also match audio to a transcript saved while offline. The
//! folder of a recording that is running now, and any folder when a recording
//! is running but its folder cannot be told, is never listed. Nothing is
//! deleted here.
use super::provenance::RecordingProvenance;
use crate::database::meeting_files;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) mod recovery;

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
    #[serde(skip)]
    recording_id: Option<String>,
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
    /// A recording is running without a known folder or cache identity.
    pub recording_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_cache_id: Option<String>,
    /// Requested browser cache IDs that are already saved or were deleted.
    pub resolved_cache_ids: Vec<String>,
    /// Cache entries that can be recovered only without their old folder.
    pub detached_cache_ids: Vec<String>,
    /// Cache entries whose transcript can be read but whose folder cannot be used.
    pub unverified_cache_ids: Vec<String>,
    /// A saved transcript still needs its recording folder to be verified.
    pub waiting_for_audio: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCacheEntry {
    id: String,
    folder: Option<String>,
    started_at: i64,
    last_updated: i64,
}

#[derive(sqlx::FromRow)]
struct RecoveryOwner {
    in_trash: bool,
    id: String,
    folder: Option<String>,
    created_at_ms: Option<i64>,
    deleted_at_ms: Option<i64>,
    is_deleted: bool,
}

const LEGACY_HANDOFF_GRACE_MS: i64 = 15_000;

fn marker_start_ms(marker: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(marker.get("created_at")?.as_str()?)
        .ok()
        .map(|date| date.timestamp_millis())
}

impl RecoveryCacheEntry {
    fn matches_marker(&self, marker: &Value) -> bool {
        if let Some(id) = meeting_files::recording_id(marker) {
            return self.id == id;
        }
        meeting_files::stamped_meeting_id(marker) == Some(self.id.as_str())
            || marker_start_ms(marker).is_some_and(|started| {
                started <= self.started_at
                    && self.started_at.saturating_sub(started) <= LEGACY_HANDOFF_GRACE_MS
            })
    }

    fn matches_owner(&self, owner: &RecoveryOwner, marker: Option<&Value>) -> bool {
        if owner
            .deleted_at_ms
            .is_some_and(|deleted| deleted < self.started_at)
        {
            return false;
        }
        if let Some(marker) = marker {
            let same_recording = self.matches_marker(marker);
            if meeting_files::stamped_meeting_id(marker) == Some(owner.id.as_str()) {
                return same_recording;
            }
            if same_recording && meeting_files::recording_id(marker).is_some() {
                return false;
            }
            if !same_recording
                && marker_start_ms(marker)
                    .zip(owner.created_at_ms)
                    .is_some_and(|(started, saved)| started <= saved)
            {
                return false;
            }
        }
        // Old IDs and dates may be generated at save time. Match a native
        // save in this cache entry's time window.
        // A stamp on a different folder generation is never used as evidence.
        self.started_at > 0
            && self.last_updated >= self.started_at
            && owner.created_at_ms.is_some_and(|created| {
                created >= self.started_at
                    && created <= self.last_updated.saturating_add(LEGACY_HANDOFF_GRACE_MS)
            })
    }
}

/// A path as text for comparison: both slash directions and (on Windows) letter
/// case are the same, a trailing separator is ignored.
pub(crate) fn folder_key(path: &Path) -> String {
    crate::local_path::comparable_text(&path.to_string_lossy())
}

fn read_json(path: &Path, max_bytes: u64) -> Option<Value> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || meeting_files::is_link(path, &metadata) || metadata.len() > max_bytes
    {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn regular_file_size(path: &Path) -> Option<u64> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && !meeting_files::is_link(path, &metadata)).then_some(metadata.len())
}

fn audio_evidence(folder: &Path) -> (bool, (u32, u64), (u32, u64)) {
    let final_audio = FINAL_AUDIO_NAMES
        .iter()
        .any(|name| regular_file_size(&folder.join(name)).is_some_and(|size| size > 0));
    let spool = capture_entries(&folder.join(".audio-spool"), |path| {
        has_extension(path, &["chunk", "tmp"])
    });
    let checkpoints = capture_entries(&folder.join(".checkpoints"), |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("audio_chunk_") && name.ends_with(".mp4"))
    });
    (final_audio, spool, checkpoints)
}

pub(crate) fn is_recording_marker(marker: &Value) -> bool {
    marker["source"].as_str() != Some("import")
        && (matches!(marker["source"].as_str(), Some("recording"))
            || matches!(
                marker["recording_mode"].as_str(),
                Some("live" | "audio_only")
            )
            || crate::database::meeting_files::recording_id(marker).is_some()
            || (marker["devices"].is_object() && marker["sample_rate"].is_u64()))
}

/// Read duration from the selected recording's saved audio or measured marker.
/// Legacy markers may contain the last transcript timestamp, so their duration
/// is only suitable for the interrupted-recording preview.
pub(crate) fn recovered_provenance(
    folder: &Path,
    marker: &Value,
    recovered_name: Option<&str>,
) -> RecordingProvenance {
    let imported = marker["source"].as_str() == Some("import");
    if !imported && !is_recording_marker(marker) {
        return RecordingProvenance::default();
    }
    let is_saved_audio = |name: &str| {
        let path = folder.join(name);
        std::fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.is_file() && !meeting_files::is_link(&path, &metadata) && metadata.len() > 0
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
    if !metadata.is_dir() || meeting_files::is_link(directory, &metadata) {
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
    pending_audio_ids: &HashSet<String>,
    deleted: &HashSet<String>,
) -> Option<InterruptedRecording> {
    use crate::database::meeting_files;
    let marker = crate::database::meeting_files::read_marker(folder)?;
    if meeting_files::has_invalid_marker_id(&marker) {
        return None;
    }
    let recording_id = meeting_files::recording_id(&marker);
    let stamped = meeting_files::stamped_meeting_id(&marker);
    // A folder name can be reused for a new recording. An existing meeting
    // with a different native identity cannot be repaired from that copy.
    if registered.is_some_and(|owner| stamped.or(recording_id).is_some_and(|id| id != owner)) {
        return None;
    }
    if registered.is_none()
        && recording_id.is_some_and(|id| {
            pending_audio_ids.contains(id) && stamped.is_some_and(|owner| owner != id)
        })
    {
        return None;
    }
    let pending_audio = registered.is_none()
        && is_recording_marker(&marker)
        && recording_id.is_some_and(|id| pending_audio_ids.contains(id) && !deleted.contains(id));
    let registered = if pending_audio {
        recording_id
    } else {
        registered
    };
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

    let (has_final_audio, (spool_chunks, spool_bytes), (checkpoints, checkpoint_bytes)) =
        audio_evidence(folder);
    let has_capture = spool_chunks > 0 || checkpoints > 0;
    if pending_audio && !has_capture && !has_final_audio {
        return None;
    }
    let outcome = super::outcome::RecordingOutcome::read(folder);

    let mut reasons: Vec<&'static str> = Vec::new();
    if pending_audio {
        reasons.push("audio_for_saved_transcript");
    }
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
        "audio_for_saved_transcript" => "Audio is available for the recovered transcript",
        "unfinished_audio" => "The recorded audio was not finished into a file",
        "did_not_finish" => "The recording stopped unexpectedly",
        "library_handoff_pending" => "The recording ended before it could be added to your library",
        _ => "The recording ended with problems and was not saved to your library",
    }
    .to_string();
    Some(InterruptedRecording {
        folder: folder.to_string_lossy().into_owned(),
        recording_id: recording_id.map(str::to_owned),
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
    scan_with_pending_audio(input, &HashSet::new())
}

fn scan_with_pending_audio(
    input: &ScanInput<'_>,
    pending_audio_ids: &HashSet<String>,
) -> InterruptedScan {
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
        if meeting_files::is_cleanup_path(root) || meeting_files::directory_chain(root).is_err() {
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
                if meeting_files::is_cleanup_path(&path)
                    || !metadata.is_dir()
                    || meeting_files::is_link(&path, &metadata)
                {
                    continue;
                }
                let key = folder_key(&path);
                if seen_folders.contains(&key) {
                    continue;
                }
                if std::fs::symlink_metadata(path.join("metadata.json")).is_err() {
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
                    pending_audio_ids,
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

async fn pending_audio_ids(pool: &sqlx::SqlitePool) -> Result<HashSet<String>, String> {
    sqlx::query_scalar::<_, String>(
        "SELECT m.id FROM active_meetings m JOIN recording_outcomes ON meeting_id = m.id
         WHERE folder_path IS NULL AND recording_source = 'recording' AND audio_save_failed = 1",
    )
    .fetch_all(pool)
    .await
    .map(|ids| ids.into_iter().collect())
    .map_err(|_| "Could not read the library's recovery status.".into())
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
        if local
            && started.elapsed() < budget
            && meeting_files::directory_chain(Path::new(&folder)).is_ok()
        {
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

#[derive(Debug, Default, PartialEq)]
struct CacheReconciliation {
    resolved: Vec<String>,
    detached: Vec<String>,
    unverified: Vec<String>,
    unverified_folders: HashSet<String>,
}

impl CacheReconciliation {
    fn defer(&mut self, id: String, key: String, canonical_key: Option<String>) {
        self.unverified.push(id);
        self.unverified_folders.insert(key);
        self.unverified_folders.extend(canonical_key);
    }
}

/// A missing leaf only proves absence if its own parent is reachable and
/// inside a trusted root. An offline root can also make metadata return NotFound.
fn cache_folder_is_absent(folder: &Path, roots: &[PathBuf]) -> bool {
    let Some(parent) = folder.parent() else {
        return false;
    };
    if meeting_files::is_cleanup_path(folder)
        || !crate::trusted_roots::text_allowed(&folder.to_string_lossy(), roots)
        || meeting_files::directory_chain(parent).is_err()
    {
        return false;
    }
    let Ok(canonical_parent) = parent.canonicalize() else {
        return false;
    };
    roots.iter().any(|root| {
        meeting_files::directory_chain(root).is_ok()
            && root
                .canonicalize()
                .is_ok_and(|root| canonical_parent.starts_with(root))
    }) && matches!(std::fs::symlink_metadata(folder), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        && meeting_files::directory_chain(parent).is_ok()
}

fn reconcile_cache_entries(
    entries: Vec<RecoveryCacheEntry>,
    owners: &[RecoveryOwner],
    registered: &HashMap<String, String>,
    roots: &[PathBuf],
    active: &ActiveFolder,
    deadline: Instant,
) -> CacheReconciliation {
    let by_id: HashMap<_, _> = owners
        .iter()
        .map(|owner| (owner.id.as_str(), owner))
        .collect();
    let mut by_folder = HashMap::<_, Vec<&RecoveryOwner>>::new();
    for owner in owners {
        if let Some(folder) = owner.folder.as_deref() {
            by_folder
                .entry(folder_key(Path::new(folder)))
                .or_default()
                .push(owner);
        }
    }
    let active_key = match active.canonical() {
        ActiveFolder::Folder(folder) => Some(folder_key(&folder)),
        _ => None,
    };
    let mut result = CacheReconciliation::default();
    for entry in entries {
        if by_id.contains_key(entry.id.as_str())
            || by_id
                .contains_key(recovery::detached_meeting_id(&entry.id, entry.started_at).as_str())
        {
            result.resolved.push(entry.id);
            continue;
        }
        let Some(folder) = entry.folder.as_deref().map(Path::new) else {
            continue;
        };
        let key = folder_key(folder);
        if active_key.as_ref() == Some(&key) {
            result.resolved.push(entry.id);
            continue;
        }
        if Instant::now() >= deadline {
            result.defer(entry.id, key, None);
            continue;
        }
        let verified = meeting_files::resolve_recording_folder(folder, roots).ok();
        let canonical_key = verified.as_deref().map(folder_key);
        if active_key.is_some() && active_key == canonical_key {
            result.resolved.push(entry.id);
            continue;
        }
        let absent = verified.is_none()
            && Instant::now() < deadline
            && cache_folder_is_absent(folder, roots);
        let marker = verified
            .as_deref()
            .filter(|_| Instant::now() < deadline)
            .and_then(meeting_files::read_marker)
            .filter(|marker| !meeting_files::has_invalid_marker_id(marker));
        if Instant::now() >= deadline || (!absent && marker.is_none()) {
            result.defer(entry.id, key, canonical_key);
            continue;
        }
        let matches_marker = marker
            .as_ref()
            .is_some_and(|marker| entry.matches_marker(marker));
        let mut owners_for_folder = by_folder
            .get(&key)
            .into_iter()
            .flatten()
            .copied()
            .chain(
                canonical_key
                    .as_ref()
                    .filter(|canonical| *canonical != &key)
                    .and_then(|canonical| by_folder.get(canonical))
                    .into_iter()
                    .flatten()
                    .copied(),
            )
            .chain(
                canonical_key
                    .as_ref()
                    .and_then(|canonical| registered.get(canonical))
                    .and_then(|id| by_id.get(id.as_str()))
                    .copied(),
            );
        if owners_for_folder
            .clone()
            .any(|owner| entry.matches_owner(owner, marker.as_ref()))
            || marker.as_ref().is_some_and(|marker| {
                meeting_files::stamped_meeting_id(marker)
                    .and_then(|id| by_id.get(id))
                    .is_some_and(|owner| entry.matches_owner(owner, Some(marker)))
            })
        {
            result.resolved.push(entry.id);
        } else if absent
            || (!matches_marker
                && marker.as_ref().is_some_and(|marker| {
                    meeting_files::recording_id(marker).is_some()
                        || marker_start_ms(marker).is_some_and(|started| {
                            entry.last_updated >= entry.started_at
                                && entry.started_at > 0
                                && started
                                    > entry.last_updated.saturating_add(LEGACY_HANDOFF_GRACE_MS)
                        })
                }))
        {
            result.detached.push(entry.id);
        } else if !matches_marker || owners_for_folder.any(|owner| !owner.is_deleted) {
            result.defer(entry.id, key, canonical_key);
        }
    }
    result
}

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
    cache_entries: Option<Vec<RecoveryCacheEntry>>,
) -> Result<InterruptedScan, String> {
    let roots = super::recording_preferences::recording_roots(&app).await;
    let pool = state.db_manager.pool();
    let owners = recovery_owners(pool).await?;
    let pending_audio = pending_audio_ids(pool).await?;
    let rows = owners
        .iter()
        .filter(|owner| !owner.is_deleted)
        .filter_map(|owner| {
            owner
                .folder
                .clone()
                .map(|folder| (owner.id.clone(), folder))
        })
        .collect();
    let registered_ids: HashSet<String> = owners
        .iter()
        .filter(|owner| !owner.is_deleted)
        .map(|owner| owner.id.clone())
        .collect();
    // Earlier deletion rows carry only the ID. They still block recovery by ID.
    let deleted = owners
        .iter()
        .filter(|owner| owner.is_deleted)
        .map(|owner| owner.id.clone())
        .collect();
    let (active, active_cache_id) = super::recording_commands::active_recording_recovery();
    let active_cache_unknown = active != ActiveFolder::None && active_cache_id.is_none();
    let kept = kept_folders(&app);
    let mut result = tokio::task::spawn_blocking(move || {
        // The whole scan stays inside its time limit: the registered-folder
        // lookup comes out of it.
        let started = Instant::now();
        let registered = registered_map(rows, &roots, REGISTERED_BUDGET);
        let limits = Limits::default();
        let cache = reconcile_cache_entries(
            cache_entries.unwrap_or_default(),
            &owners,
            &registered,
            &roots,
            &active,
            started + limits.time_limit,
        );
        let limits = Limits {
            time_limit: limits.time_limit.saturating_sub(started.elapsed()),
            ..limits
        };
        let mut result = scan_with_pending_audio(
            &ScanInput {
                roots: &roots,
                kept: &kept,
                registered: &registered,
                registered_ids: &registered_ids,
                deleted: &deleted,
                active,
                limits,
            },
            &pending_audio,
        );
        result.recording_active |= active_cache_unknown;
        result.active_cache_id = active_cache_id;
        let trashed: HashSet<_> = owners
            .iter()
            .filter(|owner| owner.in_trash)
            .map(|owner| owner.id.as_str())
            .collect();
        result.recordings.retain(|recording| {
            !recording
                .registered_meeting_id
                .as_deref()
                .is_some_and(|id| trashed.contains(id))
                && !cache
                    .unverified_folders
                    .contains(&folder_key(Path::new(&recording.folder)))
        });
        result.resolved_cache_ids = cache.resolved;
        result.detached_cache_ids = cache.detached;
        result.unverified_cache_ids = cache.unverified;
        result.waiting_for_audio = !pending_audio.is_empty();
        result
    })
    .await
    .map_err(|_| "Could not look for interrupted recordings".to_string())?;
    apply_recovery_titles(pool, &mut result.recordings).await;
    Ok(result)
}

async fn apply_recovery_titles(pool: &sqlx::SqlitePool, recordings: &mut [InterruptedRecording]) {
    let ids: HashSet<_> = recordings
        .iter()
        .filter_map(|recording| recording.recording_id.as_deref())
        .collect();
    if ids.is_empty() {
        return;
    }
    let mut query = sqlx::QueryBuilder::new(
        "SELECT recording_id, title FROM recording_title_drafts WHERE recording_id IN (",
    );
    let mut bindings = query.separated(", ");
    for id in ids {
        bindings.push_bind(id);
    }
    bindings.push_unseparated(")");
    let Ok(drafts) = query
        .build_query_as::<(String, String)>()
        .fetch_all(pool)
        .await
    else {
        return; // Keep the folder titles if this optional lookup fails.
    };
    let drafts: HashMap<_, _> = drafts.into_iter().collect();
    for recording in recordings {
        if let Some(title) = recording
            .recording_id
            .as_ref()
            .and_then(|id| drafts.get(id))
        {
            recording.title = Some(title.clone());
        }
    }
}

async fn recovery_owners(pool: &sqlx::SqlitePool) -> Result<Vec<RecoveryOwner>, String> {
    sqlx::query_as(
        "SELECT id, folder_path AS folder, CAST(ROUND(unixepoch(created_at, 'subsec') * 1000) AS INTEGER) AS created_at_ms,
                NULL AS deleted_at_ms, 0 AS is_deleted, deleted_at IS NOT NULL AS in_trash FROM meetings
         UNION ALL
         SELECT meeting_id, folder_path, meeting_created_at_ms, deleted_at_ms, 1, 0
         FROM recording_library_deletions d WHERE NOT EXISTS (SELECT 1 FROM meetings WHERE id=d.meeting_id)",
    ).fetch_all(pool).await.map_err(|_| "Could not read the library's recovery status.".into())
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
struct RepairedRecording {
    meeting_id: String,
    /// `success`, `partial`, `failed` or `none`, as audio recovery reports it.
    audio_status: String,
    audio_path: Option<String>,
    wake_summary: bool,
}

fn import_owner_waiting_for_metadata(folder: &Path, meeting_id: &str, roots: &[PathBuf]) -> bool {
    meeting_files::resolve_recording_folder(folder, roots).is_ok_and(|canonical| {
        !meeting_files::contains_recording_root(&canonical, roots)
            && meeting_files::has_unpublished_import_owner(folder, meeting_id)
    })
}

fn verify_pending_audio_inputs(folder: &Path) -> std::io::Result<()> {
    // Audio recovery opens these children by name. Reject linked inputs even
    // when the recording root and its identity marker were verified.
    let check = |path: &Path, directory: bool| match std::fs::symlink_metadata(path) {
        Ok(meta)
            if meeting_files::is_link(path, &meta)
                || if directory {
                    !meta.is_dir()
                } else {
                    !meta.is_file()
                } =>
        {
            Err(std::io::Error::other("Unsafe recording file"))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    };
    for name in FINAL_AUDIO_NAMES {
        check(&folder.join(name), false)?;
    }
    for name in [".audio-spool", ".checkpoints"] {
        let subfolder = folder.join(name);
        if check(&subfolder, true)? {
            for child in std::fs::read_dir(subfolder)? {
                check(&child?.path(), false)?;
            }
        }
    }
    Ok(())
}

/// Repairs a saved recording or attaches trusted audio to an offline
/// transcript with the same recording ID. Neither path creates a meeting.
async fn repair_saved_recording<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    pool: &sqlx::SqlitePool,
    folder: String,
) -> Result<RepairedRecording, String> {
    let roots = super::recording_preferences::recording_roots(app).await;
    let rows = registered_rows(pool).await?;
    let pending = pending_audio_ids(pool).await?;
    let active = super::recording_commands::active_recording_folder();
    let repair_roots = roots.clone();
    let (authorised, meeting_id, registered_path, marker) =
        tokio::task::spawn_blocking(move || {
            let roots = repair_roots;
            let requested = Path::new(&folder);
            let authorised =
                meeting_files::authorize_new_meeting_folder(&folder, &roots).or_else(|_| {
                    meeting_files::resolve_recording_folder(requested, &roots).map(|canonical| {
                        meeting_files::plain_path(&canonical)
                            .to_string_lossy()
                            .into_owned()
                    })
                })?;
            if active.protects(Path::new(&authorised)) {
                return Err("A recording is running. Try again when it has stopped.".into());
            }
            let registered = registered_map(rows.clone(), &roots, REGISTERED_BUDGET);
            let marker = meeting_files::read_marker(Path::new(&authorised));
            let registered_id = registered
                .get(&folder_key(Path::new(&authorised)))
                .or_else(|| {
                    std::fs::canonicalize(&authorised)
                        .ok()
                        .and_then(|canonical| {
                            registered.get(&folder_key(
                                &crate::database::meeting_files::plain_path(&canonical),
                            ))
                        })
                });
            let (meeting_id, registered_path) = if let Some(meeting) = registered_id {
                let owns_folder = match marker.as_ref() {
                    Some(marker) => {
                        !meeting_files::has_invalid_marker_id(marker)
                            && meeting_files::stamped_meeting_id(marker)
                                .or_else(|| meeting_files::recording_id(marker))
                                .is_none_or(|id| id == meeting.as_str())
                    }
                    None => {
                        import_owner_waiting_for_metadata(Path::new(&authorised), meeting, &roots)
                    }
                };
                if !owns_folder {
                    return Err("This folder belongs to a different recording.".into());
                }
                let path = rows
                    .into_iter()
                    .find_map(|(id, path)| (&id == meeting).then_some(path))
                    .ok_or("The recording is no longer saved in your library.")?;
                (meeting.clone(), Some(path))
            } else {
                let id = marker
                    .as_ref()
                    .filter(|marker| {
                        !meeting_files::has_invalid_marker_id(marker) && is_recording_marker(marker)
                    })
                    .and_then(|marker| {
                        let id = meeting_files::recording_id(marker)?;
                        meeting_files::stamped_meeting_id(marker)
                            .is_none_or(|owner| owner == id)
                            .then_some(id)
                    })
                    .filter(|id| pending.contains(*id))
                    .ok_or("This recording does not match a saved meeting.")?;
                verify_pending_audio_inputs(Path::new(&authorised)).map_err(|_| {
                    "Could not verify the audio files. Keep the recording folder and retry."
                })?;
                (id.to_owned(), None)
            };
            Ok::<_, String>((authorised, meeting_id, registered_path, marker))
        })
        .await
        .map_err(|_| "Could not repair the recording".to_string())??;

    use crate::database::transcript_edits;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM active_meetings WHERE id=?)")
            .bind(&meeting_id)
            .fetch_one(pool)
            .await
            .map_err(|_| "Could not read the saved meeting.")?;
    if !exists {
        return Err("This meeting is unavailable for recording repair.".into());
    }
    if registered_path.is_some()
        && transcript_edits::import_file_stage(pool, &meeting_id)
            .await?
            .is_some()
    {
        transcript_edits::sync_file_with_profile(pool, &meeting_id, &roots, |_| Ok(None)).await?;
        transcript_edits::notify_file_sync(app, &meeting_id);
        return Ok(RepairedRecording {
            meeting_id,
            audio_status: "none".into(),
            audio_path: None,
            wake_summary: false,
        });
    }
    let Some(marker) = marker else {
        return Err("There is no pending import file repair for this folder.".into());
    };

    let audio_folder = PathBuf::from(&authorised);
    tokio::task::spawn_blocking(move || verify_pending_audio_inputs(&audio_folder))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or("Could not verify the audio files. Keep the recording folder and retry.")?;
    let status = super::incremental_saver::recover_audio_in_folder(authorised.clone())
        .await
        .map_err(|_| {
            "Could not recover the audio. Keep the recording folder and retry.".to_string()
        })?;
    let recovered =
        status.audio_file_path.is_some() && matches!(status.status.as_str(), "success" | "partial");
    if !recovered && registered_path.is_none() {
        return Err(
            "No audio could be recovered yet. The saved transcript and recording folder are kept."
                .into(),
        );
    }
    let mut wake_summary = false;
    if recovered {
        let transcribes = super::recording_mode::marker_transcribes(&marker);
        let provenance = {
            let folder = authorised.clone();
            let recovered_path = status.audio_file_path.clone();
            tokio::task::spawn_blocking(move || {
                let name = recovered_path
                    .as_deref()
                    .and_then(|path| Path::new(path).file_name())
                    .and_then(std::ffi::OsStr::to_str);
                name.map_or_else(RecordingProvenance::default, |name| {
                    recovered_provenance(Path::new(&folder), &marker, Some(name))
                })
            })
            .await
            .map_err(|_| "Could not read recovered recording details".to_string())?
        };
        let outcome_updated = commit_repaired_audio(
            pool,
            &meeting_id,
            registered_path.as_deref(),
            &authorised,
            provenance,
            status.status == "partial",
            status.audio_file_path.as_deref(),
        )
        .await?;
        wake_summary = transcribes && outcome_updated;
    }
    Ok(RepairedRecording {
        meeting_id,
        audio_status: status.status,
        audio_path: status.audio_file_path,
        wake_summary,
    })
}

async fn commit_repaired_audio(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    registered_path: Option<&str>,
    authorised: &str,
    provenance: RecordingProvenance,
    partial: bool,
    audio_path: Option<&str>,
) -> Result<bool, String> {
    // The meeting no longer has a failed audio save; a partial recovery says
    // the capture had gaps.
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "Could not save recovered recording details")?;
    let meeting = sqlx::query(
        "UPDATE meetings SET folder_path = COALESCE(folder_path, ?),
         recording_source = COALESCE(recording_source, ?),
         recording_duration_s = COALESCE(recording_duration_s, ?)
         WHERE id = ? AND deleted_at IS NULL AND folder_path IS ?
         AND (? IS NOT NULL OR (
            NOT EXISTS (SELECT 1 FROM meetings AS owner WHERE owner.folder_path = ? COLLATE NOCASE)
            AND EXISTS (SELECT 1 FROM recording_outcomes WHERE meeting_id = meetings.id AND audio_save_failed = 1)
         ))",
    )
    .bind(authorised)
    .bind(provenance.source)
    .bind(provenance.duration_seconds)
    .bind(meeting_id)
    .bind(registered_path)
    .bind(registered_path)
    .bind(authorised)
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
        remove_legacy_checkpoints(authorised, audio_path).await;
    }
    Ok(updated.rows_affected() == 1)
}

async fn remove_legacy_checkpoints(folder: &str, audio_path: Option<&str>) {
    let Some(audio_path) = audio_path.map(PathBuf::from) else {
        return;
    };
    let checkpoints = Path::new(folder).join(".checkpoints");
    let _ = tokio::task::spawn_blocking(move || {
        let Ok(metadata) = std::fs::symlink_metadata(&checkpoints) else {
            return;
        };
        if metadata.is_dir()
            && !meeting_files::is_link(&checkpoints, &metadata)
            && super::incremental_saver::validate_recoverable_temp_audio_file(&audio_path).is_ok()
            && std::fs::remove_dir_all(&checkpoints).is_err()
        {
            log::warn!("Could not remove recovered legacy checkpoints");
        }
    })
    .await;
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
    let _recovery = recovery::MUTATION.lock().await;
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
    if classify_folder_with_deletions(&path, None, registered_ids, &HashSet::new(), deleted)
        .is_none()
    {
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
    fn unpublished_import_repair_requires_matching_owner_inside_a_recording_root() {
        let base = tempfile::tempdir().unwrap();
        let folder = base.path().join("import");
        let roots = [base.path().to_path_buf()];
        std::fs::create_dir(&folder).unwrap();
        let id = "meeting-4f7b980e-b123-4ca9-82e7-927204456483";
        std::fs::write(folder.join(super::super::import::IMPORT_OWNER_FILE), id).unwrap();
        assert!(import_owner_waiting_for_metadata(&folder, id, &roots));
        assert!(!import_owner_waiting_for_metadata(&folder, "other", &roots));
        assert!(!import_owner_waiting_for_metadata(
            &folder,
            id,
            &[folder.clone()]
        ));
        std::fs::write(folder.join("metadata.json"), b"unverified metadata").unwrap();
        assert!(!import_owner_waiting_for_metadata(&folder, id, &roots));
    }

    #[tokio::test]
    async fn cached_legacy_ids_resolve_by_folder_and_saved_or_deleted_provenance() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let root_path = meeting_files::plain_path(&root.path().canonicalize().unwrap());
        let folder = root_path.join("older");
        let folder_text = folder.to_string_lossy();
        let saved = 1_700_000_000_250;
        sqlx::query("UPDATE meetings SET folder_path=?, created_at=? WHERE id='review-test'")
            .bind(folder_text.as_ref())
            .bind(
                chrono::DateTime::from_timestamp_millis(saved)
                    .unwrap()
                    .to_rfc3339(),
            )
            .execute(&pool)
            .await
            .unwrap();
        let entries = || {
            vec![
                RecoveryCacheEntry {
                    id: "legacy-browser-id".into(),
                    folder: Some(folder_text.to_string()),
                    started_at: saved - 60_000,
                    last_updated: saved - 500,
                },
                RecoveryCacheEntry {
                    id: "new-browser-id".into(),
                    folder: Some(folder_text.to_string()),
                    started_at: saved + 60_000,
                    last_updated: saved + 90_000,
                },
            ]
        };
        let roots = [root_path];
        let check = |owners: &[RecoveryOwner]| {
            reconcile_cache_entries(
                entries(),
                owners,
                &HashMap::new(),
                &roots,
                &ActiveFolder::None,
                Instant::now() + Duration::from_secs(2),
            )
        };
        let owners = recovery_owners(&pool).await.unwrap();
        assert_eq!(
            check(&owners),
            CacheReconciliation {
                resolved: vec!["legacy-browser-id".into()],
                detached: vec!["new-browser-id".into()],
                ..CacheReconciliation::default()
            }
        );
        crate::database::repositories::meeting::MeetingsRepository::delete_from_library(
            &pool,
            "review-test",
            false,
        )
        .await
        .unwrap()
        .unwrap();
        let owners = recovery_owners(&pool).await.unwrap();
        assert!(owners[0].is_deleted);
        assert!(owners[0].deleted_at_ms.is_some());
        assert_eq!(
            check(&owners),
            CacheReconciliation {
                resolved: vec!["legacy-browser-id".into()],
                detached: vec!["new-browser-id".into()],
                ..CacheReconciliation::default()
            }
        );

        // A new native capture reusing the deleted recording's folder keeps
        // its own audio and is not treated as a copy of the deleted meeting.
        let id = "meeting-89c5ce77-ffd1-4cc7-9c53-6b4856871e72";
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("metadata.json"), serde_json::json!({
            "version": "1.0", "transcript_file": "transcripts.json", "recording_id": id,
            "created_at": chrono::DateTime::from_timestamp_millis(saved + 60_000).unwrap().to_rfc3339(),
        }).to_string()).unwrap();
        let mut entries = entries();
        entries[1].id = id.into();
        assert_eq!(
            reconcile_cache_entries(
                entries,
                &owners,
                &HashMap::new(),
                &roots,
                &ActiveFolder::None,
                Instant::now() + Duration::from_secs(2)
            ),
            CacheReconciliation {
                resolved: vec!["legacy-browser-id".into()],
                ..CacheReconciliation::default()
            }
        );
    }

    #[test]
    fn cached_folder_uncertainty_defers_until_a_marker_or_absence_is_verified() {
        let base = tempfile::tempdir().unwrap();
        let root =
            meeting_files::plain_path(&base.path().canonicalize().unwrap()).join("recordings");
        let folder = root.join("meeting");
        let id = "meeting-89c5ce77-ffd1-4cc7-9c53-6b4856871e72";
        let check = |deadline| {
            reconcile_cache_entries(
                vec![RecoveryCacheEntry {
                    id: id.into(),
                    folder: Some(folder.to_string_lossy().into_owned()),
                    started_at: 1,
                    last_updated: 2,
                }],
                &[],
                &HashMap::new(),
                std::slice::from_ref(&root),
                &ActiveFolder::None,
                deadline,
            )
        };
        let check_now = || check(Instant::now() + Duration::from_secs(2));
        let deferred = CacheReconciliation {
            unverified: vec![id.into()],
            unverified_folders: HashSet::from([folder_key(&folder)]),
            ..CacheReconciliation::default()
        };
        assert_eq!(check_now(), deferred, "recording root unavailable");

        std::fs::create_dir(&root).unwrap();
        assert_eq!(check(Instant::now()), deferred, "deadline expired");
        assert_eq!(
            check_now().detached,
            [id],
            "folder absent under reachable parent"
        );

        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("audio.wav"), b"Synthetic audio").unwrap();
        assert_eq!(check_now(), deferred, "marker missing but audio present");
        let metadata = folder.join("metadata.json");
        std::fs::write(&metadata, "{").unwrap();
        assert_eq!(check_now(), deferred, "marker only partially available");
        let write_id = |id: &str| {
            std::fs::write(
                &metadata,
                serde_json::json!({
                    "version": "1.0", "transcript_file": "transcripts.json", "recording_id": id
                })
                .to_string(),
            )
            .unwrap();
        };
        write_id(id);
        assert_eq!(
            check_now(),
            CacheReconciliation::default(),
            "matching marker on retry"
        );
        write_id("meeting-2e96a857-4344-4a44-8025-610a152cb99a");
        assert_eq!(check_now().detached, [id], "different recording verified");
    }

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
            classify_folder_with_deletions(
                folder,
                None,
                &HashSet::new(),
                &HashSet::new(),
                &deleted
            )
            .unwrap()
            .reasons,
            ["library_handoff_pending"]
        );
        assert!(classify_folder_with_deletions(
            folder,
            Some(id),
            &HashSet::new(),
            &HashSet::new(),
            &deleted
        )
        .is_none());
        let elsewhere = HashSet::from([id.to_owned()]);
        assert!(classify_folder_with_deletions(
            folder,
            None,
            &elsewhere,
            &HashSet::new(),
            &deleted
        )
        .is_none());
        deleted.insert(id.to_owned());
        assert!(classify_folder_with_deletions(
            folder,
            None,
            &HashSet::new(),
            &HashSet::new(),
            &deleted
        )
        .is_none());
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
            classify_folder_with_deletions(&folder, None, &empty, &empty, &empty)
                .unwrap()
                .reasons[0],
            "unfinished_audio",
        );
        assert!(
            classify_folder_with_deletions(&folder, None, &empty, &empty, &registered).is_none()
        );
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

    #[tokio::test]
    async fn recovery_list_prefers_the_draft_for_the_recording_id() {
        let root = tempfile::tempdir().unwrap();
        let id = "meeting-95e21332-fe98-42f9-bd33-4d1771139765";
        let folder = make(root.path(), "with-draft");
        let metadata = serde_json::json!({
            "version": "1.0", "transcript_file": "transcripts.json",
            "recording_id": id, "status": "recording", "meeting_name": "Folder title"
        });
        std::fs::write(folder.join("metadata.json"), metadata.to_string()).unwrap();
        marker(&make(root.path(), "without-draft"), "recording", None);
        let mut scan = run(
            &[root.path().to_owned()],
            &HashMap::new(),
            ActiveFolder::None,
            Limits::default(),
        );
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("INSERT INTO recording_title_drafts (recording_id, title) VALUES (?, ?)")
            .bind(id)
            .bind("Typed title")
            .execute(&pool)
            .await
            .unwrap();
        apply_recovery_titles(&pool, &mut scan.recordings).await;
        assert_eq!(scan.recordings.len(), 2);
        assert_eq!(scan.recordings[0].title.as_deref(), Some("Weekly sync"));
        assert_eq!(scan.recordings[1].title.as_deref(), Some("Typed title"));
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
    async fn saved_offline_transcript_reuses_its_meeting_when_verified_audio_returns() {
        use crate::database::repositories::transcript::TranscriptsRepository;
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let volume = tempfile::tempdir().unwrap();
        let roots = [volume.path().join("missing-drive")];
        let folder = roots[0].join("meeting");
        let id = "meeting-5dd2d62d-dc83-40af-87f9-33e4eefbc609";
        let segment = serde_json::from_value(serde_json::json!({
            "id": "sample", "text": "Synthetic cached words", "timestamp": "00:01"
        }))
        .unwrap();
        for _ in 0..2 {
            TranscriptsRepository::save_offline_recording_transcript(
                &pool,
                id,
                "Synthetic meeting",
                std::slice::from_ref(&segment),
                1_795_260_000_000,
            )
            .await
            .unwrap();
        }
        let saved: (i64, Option<String>) =
            sqlx::query_as("SELECT COUNT(*), MIN(folder_path) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(saved, (1, None));
        let registered = HashMap::new();
        let registered_ids = HashSet::from([id.to_owned()]);
        let pending = pending_audio_ids(&pool).await.unwrap();
        let scan_input = ScanInput {
            roots: &roots,
            kept: &[],
            registered: &registered,
            registered_ids: &registered_ids,
            deleted: &HashSet::new(),
            active: ActiveFolder::None,
            limits: Limits::default(),
        };
        assert!(scan_with_pending_audio(&scan_input, &pending)
            .recordings
            .is_empty());

        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("metadata.json"), serde_json::json!({
            "version": "1.0", "transcript_file": "transcripts.json", "recording_id": id, "status": "completed"
        }).to_string()).unwrap();
        assert!(
            scan_with_pending_audio(&scan_input, &pending)
                .recordings
                .is_empty(),
            "a transcript alone should not offer audio repair"
        );
        std::fs::write(folder.join("audio.mp4"), b"Synthetic audio").unwrap();
        use crate::database::repositories::meeting::MeetingsRepository;
        assert!(MeetingsRepository::move_to_trash(&pool, id, true)
            .await
            .unwrap());
        assert!(recovery_owners(&pool).await.unwrap()[0].in_trash);
        let while_trashed = pending_audio_ids(&pool).await.unwrap();
        assert!(scan_with_pending_audio(&scan_input, &while_trashed)
            .recordings
            .is_empty());
        assert!(MeetingsRepository::restore_from_trash(&pool, id)
            .await
            .unwrap());
        let scan = scan_with_pending_audio(&scan_input, &pending);
        assert_eq!(scan.recordings.len(), 1);
        assert_eq!(
            scan.recordings[0].registered_meeting_id.as_deref(),
            Some(id)
        );
        let authorised = &scan.recordings[0].folder;
        assert!(commit_repaired_audio(
            &pool,
            id,
            None,
            authorised,
            RecordingProvenance::default(),
            false,
            None,
        )
        .await
        .unwrap());
        assert!(pending_audio_ids(&pool).await.unwrap().is_empty());
        let saved: (i64, String) =
            sqlx::query_as("SELECT COUNT(*), MIN(folder_path) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(saved, (1, authorised.clone()));
        assert!(
            TranscriptsRepository::save_offline_recording_transcript(
                &pool,
                id,
                "Synthetic meeting",
                std::slice::from_ref(&segment),
                1_795_260_000_000,
            )
            .await
            .is_err(),
            "an already attached meeting cannot acknowledge a later offline draft"
        );
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn a_moved_library_link_preserves_the_capture_for_a_repair_retry() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().to_str().unwrap();
        let checkpoints = folder.path().join(".checkpoints");
        std::fs::create_dir(&checkpoints).unwrap();
        let chunk = checkpoints.join("audio_chunk_000.mp4");
        std::fs::write(&chunk, b"synthetic checkpoint").unwrap();
        let recovered = folder.path().join("audio-recovered.mp4");
        crate::audio::encode::encode_single_audio(
            bytemuck::cast_slice(&vec![0.05f32; 4_800]),
            48_000,
            1,
            &recovered,
        )
        .unwrap();
        let audio_path = recovered.to_str();
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
        assert!(commit_repaired_audio(
            &pool,
            "repair",
            Some(path),
            path,
            provenance,
            false,
            audio_path
        )
        .await
        .unwrap_err()
        .contains("moved"));
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
        assert!(commit_repaired_audio(
            &pool,
            "repair",
            Some(path),
            path,
            provenance,
            false,
            audio_path
        )
        .await
        .unwrap());
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
