//! Audio cleanup only touches the finalized, individually confirmed app copy.
use crate::audio::{constants::AUDIO_EXTENSIONS, outcome::RecordingOutcome};
use crate::state::AppState;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    collections::HashSet,
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{AppHandle, Manager};
use tokio::sync::OwnedSemaphorePermit;

#[cfg(windows)]
mod windows_file;
#[cfg(windows)]
use windows_file as platform;
#[cfg(unix)]
mod unix_file;
#[cfg(unix)]
use unix_file as platform;

const RECOVERY_DIRS: [&str; 2] = [".audio-spool", ".checkpoints"];
const RECOVERY_AUDIO: [&str; 2] = ["audio-recovered.mp4", "audio-recovered.wav"];
const MAX_SELECTION: usize = 100;
const MAX_MARKER_BYTES: u64 = 1024 * 1024;
const UNVERIFIED: &str = "This saved audio could not be verified as a ClawScribe copy.";
const UNAVAILABLE: &str = "The recording folder is unavailable or outside the recording locations.";
const RECOVERY_PENDING: &str =
    "Keep the audio while this recording has recovery files or needs review.";
const CHANGED: &str = "The saved audio changed. Review it again before removing it.";
const FOLDER_CHANGED: &str = "The recording folder changed during removal. Try again.";
const FOLDER_CLAIMS_CHANGED: &str =
    "Recording folder ownership changed. Review the saved audio again.";

#[derive(Clone, sqlx::FromRow)]
struct ArchiveFolder {
    id: String,
    folder_path: String,
    has_transcript: bool,
    needs_recovery: bool,
}

const FOLDER_CLAIMS_QUERY: &str = "SELECT id, folder_path FROM meetings
    WHERE folder_path IS NOT NULL AND TRIM(folder_path) <> '' ORDER BY id";

const FOLDERS_QUERY: &str = "SELECT m.id, m.folder_path,
    EXISTS(SELECT 1 FROM transcripts t WHERE t.meeting_id = m.id) AS has_transcript,
    (COALESCE(o.audio_save_failed, 0) OR COALESCE(o.transcription_incomplete, 0) OR
     COALESCE(o.capture_incomplete, 0) OR COALESCE(o.recording_files_incomplete, 0) OR
     COALESCE(o.recovery_files_elsewhere, 0) OR
     EXISTS(SELECT 1 FROM transcript_file_sync f WHERE f.meeting_id = m.id)) AS needs_recovery
    FROM meetings m LEFT JOIN recording_outcomes o ON m.id = o.meeting_id
    WHERE m.folder_path IS NOT NULL AND TRIM(m.folder_path) <> ''";

async fn folders(pool: &SqlitePool) -> Result<Vec<ArchiveFolder>, String> {
    sqlx::query_as(&format!("{FOLDERS_QUERY} ORDER BY m.id"))
        .fetch_all(pool)
        .await
        .map_err(|_| "Could not read recording folders".into())
}

fn path_key(path: &Path) -> String {
    crate::local_path::comparable_text(&path.to_string_lossy())
}

fn linked(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn safe_folder(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, &'static str> {
    let spelling = path.to_string_lossy();
    let matching_roots: Vec<_> = roots
        .iter()
        .filter(|root| crate::local_path::lexically_inside(&spelling, root))
        .cloned()
        .collect();
    if matching_roots.is_empty() {
        return Err(UNAVAILABLE);
    }
    // Check ancestors from the root down, so a junction is never traversed.
    let ancestors = path.ancestors().collect::<Vec<_>>();
    for ancestor in ancestors
        .into_iter()
        .rev()
        .filter(|path| path.parent().is_some())
    {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| UNAVAILABLE)?;
        if !metadata.is_dir() || linked(&metadata) {
            return Err(UNAVAILABLE);
        }
    }
    crate::database::meeting_files::resolve_recording_folder(path, &matching_roots)
        .map_err(|_| UNAVAILABLE)
}

fn audio_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| {
            AUDIO_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
}

fn read_small_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, &'static str> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file() && !linked(&metadata) && metadata.len() <= MAX_MARKER_BYTES =>
        {
            ()
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        _ => return Err(UNVERIFIED),
    }
    let mut contents = Vec::new();
    let file = platform::open_marker(path).map_err(|_| UNVERIFIED)?;
    let metadata = file.metadata().map_err(|_| UNVERIFIED)?;
    if !metadata.is_file() || linked(&metadata) || metadata.len() > MAX_MARKER_BYTES {
        return Err(UNVERIFIED);
    }
    file.take(MAX_MARKER_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| UNVERIFIED)?;
    if contents.len() as u64 > MAX_MARKER_BYTES {
        return Err(UNVERIFIED);
    }
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|_| UNVERIFIED)
}

/// The older audio_file field can be rewritten from any playable fallback.
/// Only use it as evidence when it still comes from the original writer.
pub(crate) fn owned_audio_name(marker: &serde_json::Value, meeting_id: &str) -> Option<String> {
    if marker["version"].as_str() != Some("1.0")
        || marker["meeting_id"].as_str() != Some(meeting_id)
        || marker["transcript_file"].as_str() != Some("transcripts.json")
        || marker["status"].as_str() != Some("completed")
    {
        return None;
    }
    let name = if marker.get("retranscribed_at").is_some() {
        let owned = marker["archive_saved_audio"].as_str()?;
        (marker["audio_file"].as_str() == Some(owned)).then_some(owned)?
    } else {
        marker["audio_file"].as_str()?
    };
    let extension = name.strip_prefix("audio.")?;
    let import = marker["source"].as_str() == Some("import");
    let recording = marker["recording_mode"].is_string();
    (AUDIO_EXTENSIONS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(extension))
        && (import || (recording && name == "audio.mp4")))
        .then(|| name.to_string())
}

fn recovery_present(folder: &Path) -> Result<bool, &'static str> {
    for name in RECOVERY_DIRS {
        let path = folder.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !linked(&metadata) => {
                if name == ".audio-spool"
                    || fs::read_dir(path)
                        .map_err(|_| RECOVERY_PENDING)?
                        .next()
                        .is_some()
                {
                    return Ok(true);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            _ => return Err(RECOVERY_PENDING),
        }
    }
    Ok(false)
}

fn canonical_folders(roots: &[PathBuf], rows: &[ArchiveFolder]) -> Vec<Option<PathBuf>> {
    rows.iter()
        .map(|row| {
            let path = Path::new(&row.folder_path);
            crate::trusted_roots::text_allowed(&row.folder_path, roots)
                .then(|| path.canonicalize().ok())
                .flatten()
        })
        .collect()
}

fn overlaps(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|rest| rest.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|rest| rest.starts_with('/'))
}

struct Candidate {
    path: PathBuf,
    bytes: u64,
    revision: String,
    file: File,
    // On Windows these handles deny moving/replacing the candidate's directory path.
    _directories: Vec<File>,
}

fn owned_audio_path(folder: &Path, id: &str) -> Result<PathBuf, &'static str> {
    if recovery_present(folder)?
        || read_small_json::<RecordingOutcome>(&folder.join("recording-outcome.json"))?
            .is_some_and(|outcome| outcome.needs_recovery())
    {
        return Err(RECOVERY_PENDING);
    }
    let marker = read_small_json(&folder.join("metadata.json"))?.ok_or(UNVERIFIED)?;
    let name = owned_audio_name(&marker, id).ok_or(UNVERIFIED)?;
    for entry in fs::read_dir(folder).map_err(|_| UNAVAILABLE)? {
        let entry = entry.map_err(|_| UNAVAILABLE)?;
        if entry.file_name() != name.as_str() && audio_extension(&entry.path()) {
            return Err("This folder contains another audio file. Its saved audio was kept.");
        }
    }
    Ok(folder.join(name))
}

fn audio_revision(file: &File, path: &Path, id: &str) -> Result<(u64, String), &'static str> {
    let metadata = file.metadata().map_err(|_| UNVERIFIED)?;
    if !metadata.is_file() || linked(&metadata) || metadata.len() == 0 {
        return Err(UNVERIFIED);
    }
    let identity = platform::file_identity(file).map_err(|_| UNVERIFIED)?;
    let modified = metadata.modified().map_err(|_| UNVERIFIED)?;
    let revision = format!(
        "{:x}",
        Sha256::digest(format!(
            "{id:?}\n{:?}\n{identity:?}\n{modified:?}\n{}",
            path_key(path),
            metadata.len()
        ))
    );
    Ok((metadata.len(), revision))
}

fn inspect_candidate(
    roots: &[PathBuf],
    rows: &[ArchiveFolder],
    canonical: &[Option<PathBuf>],
    id: &str,
    removing: bool,
) -> Result<Candidate, &'static str> {
    let row = rows
        .iter()
        .find(|row| row.id == id)
        .ok_or("This meeting has no saved recording folder.")?;
    if row.needs_recovery {
        return Err(RECOVERY_PENDING);
    }
    if !row.has_transcript {
        return Err("Save a transcript before removing this recording's audio.");
    }
    let folder = safe_folder(Path::new(&row.folder_path), roots)?;
    let folder_key = path_key(&folder);
    if rows.iter().zip(canonical).any(|(other, resolved)| {
        other.id != id
            && (overlaps(&folder_key, &path_key(Path::new(&other.folder_path)))
                || resolved
                    .as_ref()
                    .is_some_and(|path| overlaps(&folder_key, &path_key(path))))
    }) {
        return Err("Another meeting uses this recording folder.");
    }
    let directories = if removing {
        platform::pin_directories(&folder).map_err(|_| UNAVAILABLE)?
    } else {
        Vec::new()
    };
    let path = owned_audio_path(&folder, id)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            "No saved audio was found."
        } else {
            UNVERIFIED
        }
    })?;
    if !metadata.is_file() || linked(&metadata) || metadata.len() == 0 {
        return Err(UNVERIFIED);
    }
    let file = platform::open_audio(&path, removing).map_err(|_| UNVERIFIED)?;
    let (bytes, revision) = audio_revision(&file, &path, id)?;
    Ok(Candidate {
        path,
        bytes,
        revision,
        file,
        _directories: directories,
    })
}

#[derive(Serialize)]
pub(crate) struct AudioRemovalCheck {
    id: String,
    bytes: Option<u64>,
    revision: Option<String>,
    reason: Option<&'static str>,
}

fn inspect_selection(
    roots: &[PathBuf],
    rows: &[ArchiveFolder],
    ids: &[String],
) -> Vec<AudioRemovalCheck> {
    let canonical = canonical_folders(roots, rows);
    ids.iter()
        .map(
            |id| match inspect_candidate(roots, rows, &canonical, id, false) {
                Ok(candidate) => AudioRemovalCheck {
                    id: id.clone(),
                    bytes: Some(candidate.bytes),
                    revision: Some(candidate.revision),
                    reason: None,
                },
                Err(reason) => AudioRemovalCheck {
                    id: id.clone(),
                    bytes: None,
                    revision: None,
                    reason: Some(reason),
                },
            },
        )
        .collect()
}

#[derive(Clone, Deserialize)]
pub(crate) struct AudioRemovalRequest {
    id: String,
    revision: String,
}

#[derive(Serialize)]
pub(crate) struct AudioRemovalResult {
    id: String,
    removed_bytes: Option<u64>,
    reason: Option<&'static str>,
}

// Reserve SQLite for one file. Compare the folder ownership snapshot first, so
// aliases added or reassigned by a concurrent writer invalidate the candidate
// without resolving every registered directory under the write reservation.
async fn remove_prepared(
    pool: &SqlitePool,
    roots: &[PathBuf],
    request: &AudioRemovalRequest,
    rows: &[ArchiveFolder],
    candidate: Candidate,
    job: Arc<OwnedSemaphorePermit>,
) -> Result<u64, &'static str> {
    if candidate.revision != request.revision {
        return Err(CHANGED);
    }

    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "Could not lock the library. Try again.")?;
    let claims: Vec<(String, String)> = sqlx::query_as(FOLDER_CLAIMS_QUERY)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| "Could not re-read recording folders.")?;
    if !claims
        .iter()
        .map(|(id, path)| (id, path))
        .eq(rows.iter().map(|row| (&row.id, &row.folder_path)))
    {
        return Err(FOLDER_CLAIMS_CHANGED);
    }
    let current: ArchiveFolder = sqlx::query_as(&format!("{FOLDERS_QUERY} AND m.id = ?"))
        .bind(&request.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| FOLDER_CHANGED)?;
    if current.needs_recovery {
        return Err(RECOVERY_PENDING);
    }
    if !current.has_transcript {
        return Err("Save a transcript before removing this recording's audio.");
    }

    let roots = roots.to_vec();
    let result = tokio::task::spawn_blocking(move || {
        // Keep the reservation and job permit until after the delete handle
        // closes, including when the invoking task is cancelled.
        let _job = job;
        let result = (|| {
            let folder = safe_folder(Path::new(&current.folder_path), &roots)?;
            let path = owned_audio_path(&folder, &current.id)?;
            if path != candidate.path
                || audio_revision(&candidate.file, &path, &current.id)?.1 != candidate.revision
            {
                return Err(CHANGED);
            }
            platform::remove_audio(&candidate.path, &candidate.file).map_err(|_| {
                "Could not remove the saved audio. Close any player using it and try again."
            })?;
            Ok(candidate.bytes)
        })();
        drop(candidate);
        (tx, result)
    })
    .await
    .map_err(|_| "Could not finish removing saved audio.")?;
    let (tx, result) = result;
    // The transaction reserves the writer and makes no database changes.
    let _ = tx.rollback().await;
    result
}

async fn remove_selected(
    pool: SqlitePool,
    roots: Vec<PathBuf>,
    requests: Vec<AudioRemovalRequest>,
) -> Result<Vec<AudioRemovalResult>, String> {
    let job = Arc::new(crate::audio::inference::claim_job()?);
    let rows = Arc::new(folders(&pool).await?);
    let canonical = {
        let roots = roots.clone();
        let rows = rows.clone();
        tokio::task::spawn_blocking(move || Arc::new(canonical_folders(&roots, &rows)))
            .await
            .map_err(|_| "Could not locate recording folders")?
    };

    let mut results = Vec::with_capacity(requests.len());
    for request in requests {
        let candidate = {
            let roots = roots.clone();
            let rows = rows.clone();
            let canonical = canonical.clone();
            let id = request.id.clone();
            tokio::task::spawn_blocking(move || {
                inspect_candidate(&roots, &rows, &canonical, &id, true)
            })
            .await
        };
        let result = match candidate {
            Ok(Ok(candidate)) => {
                remove_prepared(&pool, &roots, &request, &rows, candidate, job.clone()).await
            }
            Ok(Err(reason)) => Err(reason),
            Err(_) => Err(UNAVAILABLE),
        };
        results.push(AudioRemovalResult {
            id: request.id,
            removed_bytes: result.ok(),
            reason: result.err(),
        });
        tokio::task::yield_now().await;
    }
    Ok(results)
}

fn validate_selection<'a>(ids: impl ExactSizeIterator<Item = &'a str>) -> Result<(), String> {
    if ids.len() == 0 || ids.len() > MAX_SELECTION {
        return Err(format!("Select between 1 and {MAX_SELECTION} meetings."));
    }
    let mut seen = HashSet::new();
    if !ids
        .into_iter()
        .all(|id| !id.is_empty() && id.len() <= 256 && seen.insert(id))
    {
        return Err("The meeting selection is invalid.".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn inspect_archive_audio(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<AudioRemovalCheck>, String> {
    validate_selection(ids.iter().map(String::as_str))?;
    let roots = crate::audio::recording_preferences::recording_roots(&app).await;
    let rows = folders(state.db_manager.pool()).await?;
    tokio::task::spawn_blocking(move || inspect_selection(&roots, &rows, &ids))
        .await
        .map_err(|_| "Could not inspect the saved audio".into())
}

#[tauri::command]
pub(crate) async fn remove_archive_audio(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    requests: Vec<AudioRemovalRequest>,
) -> Result<Vec<AudioRemovalResult>, String> {
    validate_selection(requests.iter().map(|request| request.id.as_str()))?;
    if requests.iter().any(|request| request.revision.len() != 64) {
        return Err(CHANGED.into());
    }
    let roots = crate::audio::recording_preferences::recording_roots(&app).await;
    remove_selected(state.db_manager.pool().clone(), roots, requests).await
}

#[derive(Default, Serialize)]
pub(crate) struct ArchiveStorage {
    saved_audio_bytes: u64,
    recovery_bytes: u64,
    database_bytes: u64,
    reference_bytes: u64,
    database_backup_bytes: u64,
    unavailable_folders: usize,
    partial: bool,
}

fn measured_file(path: &Path, partial: &mut bool) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !linked(&metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        _ => {
            *partial = true;
            0
        }
    }
}

fn tree_bytes(path: &Path, depth: u8, partial: &mut bool) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !linked(&metadata) => metadata.len(),
        Ok(metadata) if metadata.is_dir() && !linked(&metadata) && depth < 16 => {
            match fs::read_dir(path) {
                Ok(entries) => entries.fold(0u64, |bytes, entry| {
                    bytes.saturating_add(match entry {
                        Ok(entry) => tree_bytes(&entry.path(), depth + 1, partial),
                        Err(_) => {
                            *partial = true;
                            0
                        }
                    })
                }),
                Err(_) => {
                    *partial = true;
                    0
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        _ => {
            *partial = true;
            0
        }
    }
}

fn measure_storage(roots: &[PathBuf], folders: &[ArchiveFolder], data: &Path) -> ArchiveStorage {
    let mut totals = ArchiveStorage::default();
    let mut seen = HashSet::new();
    for row in folders {
        let Ok(folder) = safe_folder(Path::new(&row.folder_path), roots) else {
            totals.unavailable_folders += 1;
            continue;
        };
        if !seen.insert(path_key(&folder)) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&folder) else {
            totals.unavailable_folders += 1;
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                totals.partial = true;
                continue;
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                totals.partial = true;
                continue;
            };
            if RECOVERY_DIRS.contains(&name) {
                totals.recovery_bytes = totals.recovery_bytes.saturating_add(tree_bytes(
                    &entry.path(),
                    0,
                    &mut totals.partial,
                ));
            } else if RECOVERY_AUDIO.contains(&name) {
                totals.recovery_bytes = totals
                    .recovery_bytes
                    .saturating_add(measured_file(&entry.path(), &mut totals.partial));
            } else if !name.starts_with('.') && audio_extension(&entry.path()) {
                totals.saved_audio_bytes = totals
                    .saved_audio_bytes
                    .saturating_add(measured_file(&entry.path(), &mut totals.partial));
            }
        }
    }
    for name in [
        "meeting_minutes.sqlite",
        "meeting_minutes.sqlite-wal",
        "meeting_minutes.sqlite-shm",
    ] {
        totals.database_bytes = totals
            .database_bytes
            .saturating_add(measured_file(&data.join(name), &mut totals.partial));
    }
    totals.reference_bytes = tree_bytes(&data.join("reference-documents"), 0, &mut totals.partial);
    totals.database_backup_bytes = tree_bytes(&data.join("backups"), 0, &mut totals.partial);
    totals
}

#[tauri::command]
pub(crate) async fn measure_archive_storage(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ArchiveStorage, String> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|_| "Could not locate library storage")?;
    let roots = crate::audio::recording_preferences::recording_roots(&app).await;
    let folders = folders(state.db_manager.pool()).await?;
    tokio::task::spawn_blocking(move || measure_storage(&roots, &folders, &data))
        .await
        .map_err(|_| "Could not measure library storage".into())
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
