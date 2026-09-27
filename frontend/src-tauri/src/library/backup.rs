//! Portable, additive meeting archives. Provider settings and credentials are never selected.
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use zip::write::SimpleFileOptions;

const TABLES: &[&str] = &[
    "meetings",
    "transcripts",
    "summary_processes",
    "transcript_chunks",
    "meeting_notes",
    "ai_chat_messages",
    "recording_outcomes",
    "transcript_revisions",
    "transcript_edit_batches",
    "transcript_file_sync",
    "meeting_tags",
    "meeting_bookmarks",
];
const MAX_MANIFEST: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_BYTES: u64 = 100 * 1024 * 1024 * 1024;
static ARCHIVE_JOB: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    tables: BTreeMap<String, Vec<Map<String, Value>>>,
    // Index into the meetings array -> leaf filenames. Never contains source paths.
    files: BTreeMap<usize, Vec<String>>,
    // Optional in V1; omissions remain visible when this archive is restored.
    #[serde(default)]
    incomplete_audio: BTreeMap<usize, IncompleteAudio>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct IncompleteAudio {
    recovery_files_excluded: bool,
    audio_unavailable: bool,
    #[serde(default)]
    recording_folder_missing: bool,
}

#[derive(Serialize)]
pub struct IncompleteMeeting {
    meeting_id: String,
    title: String,
    #[serde(flatten)]
    audio: IncompleteAudio,
}

#[derive(Serialize)]
pub struct BackupReport {
    pub meetings: usize,
    pub skipped: usize,
    pub files: usize,
    pub incomplete_meetings: Vec<IncompleteMeeting>,
}

fn incomplete_meetings(
    manifest: &Manifest,
    selected: Option<&HashSet<String>>,
) -> Vec<IncompleteMeeting> {
    manifest
        .incomplete_audio
        .iter()
        .filter_map(|(index, audio)| {
            let row = &manifest.tables["meetings"][*index];
            let id = row.get("id")?.as_str()?;
            if selected.is_some_and(|selected| !selected.contains(id)) {
                return None;
            }
            Some(IncompleteMeeting {
                meeting_id: id.to_string(),
                title: row
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled meeting")
                    .to_string(),
                audio: audio.clone(),
            })
        })
        .collect()
}

fn failure(_: impl std::fmt::Display) -> String {
    "Archive operation failed. Check the file, available disk space, and folder permissions.".into()
}

fn meeting_failure(row: &Map<String, Value>, reason: &str) -> String {
    let title = row
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled meeting");
    // User-facing context only; never include source paths or underlying I/O errors.
    format!("Cannot back up meeting \"{title}\": {reason}")
}

fn allowed_file(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 240
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(char::is_control)
        || name.starts_with('.')
        || name.ends_with(['.', ' '])
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        return false;
    }
    matches!(
        name,
        "metadata.json" | "transcripts.json" | "exports.json" | "recording-outcome.json"
    ) || Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "mp4" | "m4a" | "wav" | "mp3" | "flac" | "ogg" | "aac" | "webm" | "mkv" | "wma"
            )
        })
}

async fn snapshot(pool: &SqlitePool) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    super::attach_bookmarks(pool).await.map_err(failure)?;
    let mut tx = pool.begin().await.map_err(failure)?;
    let mut tables = BTreeMap::new();
    let mut budget = MAX_MANIFEST;
    for table in TABLES {
        let columns = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(&mut *tx)
            .await
            .map_err(failure)?;
        let args = columns
            .iter()
            .map(|c| {
                let name: String = c.get("name");
                format!("'{name}', \"{name}\"")
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut rows = Vec::new();
        // Page the snapshot so the SQLite result itself is bounded.
        loop {
            let page: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT json_object({args}) FROM {table} LIMIT 500 OFFSET ?"
            ))
            .bind(rows.len() as i64)
            .fetch_all(&mut *tx)
            .await
            .map_err(failure)?;
            let count = page.len();
            for row in page {
                budget = budget
                    .checked_sub(row.len() as u64)
                    .ok_or("Meeting metadata exceeds the 128 MiB archive limit.")?;
                rows.push(serde_json::from_str::<Map<String, Value>>(&row).map_err(failure)?);
            }
            if count < 500 {
                break;
            }
        }
        tables.insert(table.to_string(), rows);
    }
    tx.commit().await.map_err(failure)?;
    let mut folders = Vec::new();
    for row in tables.get_mut("meetings").unwrap() {
        folders.push(
            row.get("folder_path")
                .and_then(Value::as_str)
                .map(PathBuf::from),
        );
        row.insert("folder_path".into(), Value::Null);
    }
    // Orphan live markers belong to interrupted recordings, not the saved library.
    tables
        .get_mut("meeting_bookmarks")
        .unwrap()
        .retain(|row| row.get("meeting_id").is_some_and(Value::is_string));
    for row in tables.get_mut("meeting_bookmarks").unwrap() {
        row.insert("folder_path".into(), Value::Null);
    }
    Ok((
        Manifest {
            format: "ClawScribe meeting archive".into(),
            version: 1,
            tables,
            files: BTreeMap::new(),
            incomplete_audio: BTreeMap::new(),
        },
        folders,
    ))
}

fn write_archive(
    path: &Path,
    mut manifest: Manifest,
    folders: Vec<Option<PathBuf>>,
) -> Result<BackupReport, String> {
    let mut output =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("Choose a destination folder.")?)
            .map_err(failure)?;
    let mut zip = zip::ZipWriter::new(output.as_file_mut());
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let mut files = 0;
    let mut total = 0u64;
    let pending_audio: HashSet<String> = manifest.tables["recording_outcomes"]
        .iter()
        .filter(|row| {
            row.get("audio_save_failed")
                .is_some_and(|value| value.as_i64() == Some(1) || value.as_bool() == Some(true))
        })
        .filter_map(|row| {
            row.get("meeting_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    for (index, folder) in folders.iter().enumerate() {
        let meeting = &manifest.tables["meetings"][index];
        let recovery_pending = meeting
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| pending_audio.contains(id));
        let Some(folder) = folder else {
            if recovery_pending {
                manifest
                    .incomplete_audio
                    .entry(index)
                    .or_default()
                    .audio_unavailable = true;
            }
            continue;
        };
        let metadata = match std::fs::symlink_metadata(folder) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                manifest.incomplete_audio.insert(
                    index,
                    IncompleteAudio {
                        recording_folder_missing: true,
                        audio_unavailable: true,
                        ..Default::default()
                    },
                );
                continue;
            }
            Err(_) => {
                return Err(meeting_failure(
                    meeting,
                    "the recording folder is unreadable. Restore folder access, then retry.",
                ))
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(meeting_failure(
                meeting,
                "recording folder links are not supported in backups.",
            ));
        }
        let entries = match std::fs::read_dir(folder) {
            Ok(entries) => entries,
            // The folder may disappear after the metadata check. No files have
            // been added yet, so this meeting can still be saved without audio.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                manifest.incomplete_audio.insert(
                    index,
                    IncompleteAudio {
                        recording_folder_missing: true,
                        audio_unavailable: true,
                        ..Default::default()
                    },
                );
                continue;
            }
            Err(_) => {
                return Err(meeting_failure(
                    meeting,
                    "the recording folder cannot be read. Restore folder access, then retry.",
                ))
            }
        };
        // Recovery retains its originals. Exclude them from V1 archives, but
        // report the omission without preventing backup of the saved library.
        let mut incomplete = IncompleteAudio::default();
        for recovery_dir in [".audio-spool", ".checkpoints"] {
            match std::fs::symlink_metadata(folder.join(recovery_dir)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    if std::fs::read_dir(folder.join(recovery_dir))
                        .is_ok_and(|mut entries| entries.next().is_none())
                    {
                        continue;
                    }
                }
                _ => {}
            }
            incomplete.recovery_files_excluded = true;
        }
        let mut names = Vec::new();
        let mut has_audio = false;
        for entry in entries {
            let entry = entry.map_err(failure)?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !allowed_file(&name) {
                continue;
            }
            if !entry.file_type().map_err(failure)?.is_file() {
                return Err("Recording file links are not supported in backups.".into());
            }
            let mut input = File::open(entry.path()).map_err(failure)?;
            let size = input.metadata().map_err(failure)?.len();
            has_audio |= size > 0
                && Path::new(&name)
                    .extension()
                    .is_some_and(|ext| ext != "json");
            total = total.checked_add(size).ok_or("Archive is too large.")?;
            files += 1;
            if files > MAX_FILES || total > MAX_BYTES {
                return Err("Archive exceeds 100 GiB or 100,000 files.".into());
            }
            zip.start_file(format!("recordings/{index}/{name}"), options)
                .map_err(failure)?;
            if std::io::copy(&mut Read::take(&mut input, size + 1), &mut zip).map_err(failure)?
                != size
            {
                return Err(
                    "A recording changed during backup. Retry when processing finishes.".into(),
                );
            }
            names.push(name);
        }
        incomplete.audio_unavailable =
            !has_audio && (recovery_pending || incomplete.recovery_files_excluded);
        if incomplete.audio_unavailable || incomplete.recovery_files_excluded {
            manifest.incomplete_audio.insert(index, incomplete);
        }
        manifest.files.insert(index, names);
    }
    let bytes = serde_json::to_vec(&manifest).map_err(failure)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("Meeting metadata exceeds the archive limit.".into());
    }
    zip.start_file("manifest.json", options).map_err(failure)?;
    zip.write_all(&bytes).map_err(failure)?;
    zip.finish().map_err(failure)?;
    output.as_file().sync_all().map_err(failure)?;
    output.persist(path).map_err(failure)?;
    Ok(BackupReport {
        meetings: manifest.tables["meetings"].len(),
        skipped: 0,
        files,
        incomplete_meetings: incomplete_meetings(&manifest, None),
    })
}

fn unpack(
    path: &Path,
    root: &Path,
    existing_ids: &HashSet<String>,
) -> Result<(Manifest, tempfile::TempDir), String> {
    let mut zip = zip::ZipArchive::new(File::open(path).map_err(failure)?).map_err(failure)?;
    if zip.len() > MAX_FILES + 1 {
        return Err("Too many archive files.".into());
    }
    let mut bytes = Vec::new();
    zip.by_name("manifest.json")
        .map_err(failure)?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("Archive metadata is too large.".into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(failure)?;
    if manifest.version != 1
        || manifest.format != "ClawScribe meeting archive"
        || !manifest.tables.contains_key("meetings")
        || manifest
            .tables
            .keys()
            .any(|table| !TABLES.contains(&table.as_str()))
    {
        return Err("Unsupported meeting archive format.".into());
    }
    let count = manifest.tables["meetings"].len();
    let mut known = HashSet::new();
    for row in &manifest.tables["meetings"] {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Invalid meeting identity.")?;
        if id.is_empty() || !known.insert(id) {
            return Err("Duplicate meeting identity in archive.".into());
        }
    }
    if manifest
        .incomplete_audio
        .keys()
        .any(|index| *index >= count)
    {
        return Err("Invalid archive omission reference.".into());
    }
    let mut expected = HashSet::from(["manifest.json".to_string()]);
    for (index, names) in &manifest.files {
        if *index >= count {
            return Err("Invalid archive meeting reference.".into());
        }
        let mut unique = HashSet::new();
        for name in names {
            if !allowed_file(name)
                || !unique.insert(name.to_lowercase())
                || !expected.insert(format!("recordings/{index}/{name}"))
            {
                return Err("Unsafe or duplicate archive filename.".into());
            }
        }
    }
    if expected.len() != zip.len() {
        return Err("Unexpected or missing files in archive.".into());
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(failure)?;
        if !expected.remove(entry.name())
            || entry.is_dir()
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err("Unsafe archive entry.".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("Archive is too large.")?;
        if total > MAX_BYTES + MAX_MANIFEST {
            return Err("Archive exceeds 100 GiB.".into());
        }
    }
    std::fs::create_dir_all(root).map_err(failure)?;
    let stage = tempfile::Builder::new()
        .prefix("restored-")
        .tempdir_in(root)
        .map_err(failure)?;
    for (index, names) in &manifest.files {
        let id = manifest.tables["meetings"][*index]["id"].as_str().unwrap();
        if existing_ids.contains(id) {
            continue;
        }
        let folder = stage.path().join(index.to_string());
        std::fs::create_dir(&folder).map_err(failure)?;
        for name in names {
            let mut source = zip
                .by_name(&format!("recordings/{index}/{name}"))
                .map_err(failure)?;
            let size = source.size();
            let mut destination = File::create(folder.join(name)).map_err(failure)?;
            let copied = std::io::copy(&mut Read::take(&mut source, size + 1), &mut destination)
                .map_err(failure)?;
            if copied != size {
                return Err("Archive file is truncated.".into());
            }
            destination.sync_all().map_err(failure)?;
        }
    }
    Ok((manifest, stage))
}

async fn import_manifest(
    pool: &SqlitePool,
    manifest: Manifest,
    stage: tempfile::TempDir,
) -> Result<BackupReport, String> {
    let mut tx = pool.begin().await.map_err(failure)?;
    let mut selected = HashSet::new();
    let mut known = HashSet::new();
    let mut skipped = 0;
    for row in &manifest.tables["meetings"] {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Invalid meeting identity.")?;
        if id.is_empty() || !known.insert(id.to_string()) {
            return Err("Duplicate meeting identity in archive.".into());
        }
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(failure)?;
        if exists {
            skipped += 1;
        } else {
            selected.insert(id.to_string());
        }
    }
    for table in TABLES {
        let schema = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(&mut *tx)
            .await
            .map_err(failure)?;
        let columns: HashSet<String> = schema.iter().map(|row| row.get("name")).collect();
        // Older V1 archives may predate optional meeting-related tables.
        for (index, source) in manifest
            .tables
            .get(*table)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if source.keys().any(|key| !columns.contains(key)) {
                return Err("Archive contains unsupported data columns.".into());
            }
            let parent = source
                .get(if *table == "meetings" {
                    "id"
                } else {
                    "meeting_id"
                })
                .and_then(Value::as_str)
                .ok_or("Invalid meeting reference.")?;
            if !known.contains(parent) {
                return Err("Archive contains an unknown meeting reference.".into());
            }
            if !selected.contains(parent) {
                continue;
            }
            let mut row = source.clone();
            if *table == "meetings" {
                if manifest.files.contains_key(&index)
                    && !stage.path().join(index.to_string()).is_dir()
                {
                    // A meeting may have been deleted since the pre-extraction
                    // snapshot. Never import its metadata with missing audio.
                    return Err(
                        "The meeting library changed during restore. Retry the restore.".into(),
                    );
                }
                row.insert(
                    "folder_path".into(),
                    if manifest.files.contains_key(&index) {
                        Value::String(
                            stage
                                .path()
                                .join(index.to_string())
                                .to_string_lossy()
                                .into(),
                        )
                    } else {
                        Value::Null
                    },
                );
            }
            if *table == "meeting_bookmarks" {
                row.insert("folder_path".into(), Value::Null);
            }
            // Interrupted remote summary jobs cannot resume on the destination machine.
            if *table == "summary_processes"
                && row
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !matches!(s, "completed" | "failed" | "cancelled"))
            {
                row.insert("status".into(), Value::String("cancelled".into()));
            }
            let names = row
                .keys()
                .map(|k| format!("\"{k}\""))
                .collect::<Vec<_>>()
                .join(",");
            let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(format!(
                "INSERT INTO {table} ({names}) VALUES ("
            ));
            let mut values = query.separated(",");
            for value in row.values() {
                match value {
                    Value::Null => {
                        values.push_bind(Option::<String>::None);
                    }
                    Value::String(s) => {
                        values.push_bind(s);
                    }
                    Value::Number(n) if n.is_i64() => {
                        values.push_bind(n.as_i64().unwrap());
                    }
                    Value::Number(n) => {
                        values.push_bind(n.as_f64().ok_or("Invalid numeric value.")?);
                    }
                    _ => return Err("Invalid archive field type.".into()),
                }
            }
            query
                .push(")")
                .build()
                .execute(&mut *tx)
                .await
                .map_err(failure)?;
        }
    }
    // Delete skipped meetings' extracted files before committing any rows or
    // keeping the stage. Cleanup failure rolls back the complete import.
    let unused: Vec<usize> = manifest
        .files
        .keys()
        .copied()
        .filter(|index| {
            !manifest.tables["meetings"][*index]
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| selected.contains(id))
        })
        .collect();
    let stage = tokio::task::spawn_blocking(move || -> Result<_, String> {
        for index in unused {
            match std::fs::remove_dir_all(stage.path().join(index.to_string())) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(failure(error)),
            }
        }
        Ok(stage)
    })
    .await
    .map_err(failure)??;
    tx.commit().await.map_err(failure)?;
    let (files, folders) = manifest
        .files
        .iter()
        .filter(|(i, _)| {
            manifest.tables["meetings"][**i]
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| selected.contains(id))
        })
        .fold((0, 0), |(files, folders), (_, names)| {
            (files + names.len(), folders + 1)
        });
    if folders > 0 {
        stage.keep();
    }
    Ok(BackupReport {
        meetings: selected.len(),
        skipped,
        files,
        incomplete_meetings: incomplete_meetings(&manifest, Some(&selected)),
    })
}

#[tauri::command]
pub async fn backup_library(app: AppHandle, path: String) -> Result<BackupReport, String> {
    if !Path::new(&path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        return Err("Choose a .zip backup file.".into());
    }
    let _guard = ARCHIVE_JOB
        .try_lock()
        .map_err(|_| "Another archive operation is running.")?;
    let _audio = crate::audio::inference::claim_job()?;
    let (manifest, folders) = snapshot(app.state::<AppState>().db_manager.pool()).await?;
    tokio::task::spawn_blocking(move || write_archive(Path::new(&path), manifest, folders))
        .await
        .map_err(failure)?
}

#[tauri::command]
pub async fn restore_library(app: AppHandle, path: String) -> Result<BackupReport, String> {
    let _guard = ARCHIVE_JOB
        .try_lock()
        .map_err(|_| "Another archive operation is running.")?;
    let _audio = crate::audio::inference::claim_job()?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(failure)?
        .join("restored-recordings");
    let existing_ids: HashSet<String> = sqlx::query_scalar::<_, String>("SELECT id FROM meetings")
        .fetch_all(app.state::<AppState>().db_manager.pool())
        .await
        .map_err(failure)?
        .into_iter()
        .collect();
    let (manifest, stage) =
        tokio::task::spawn_blocking(move || unpack(Path::new(&path), &root, &existing_ids))
            .await
            .map_err(failure)??;
    let report =
        import_manifest(app.state::<AppState>().db_manager.pool(), manifest, stage).await?;
    let _ = app.emit("library-changed", ());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mixed_restore_keeps_only_new_meetings_audio() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('new', 'New meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool).await.unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("audio.wav"), b"synthetic audio").unwrap();
        sqlx::query("UPDATE meetings SET folder_path = ?")
            .bind(source.path().to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        let (manifest, folders) = snapshot(&pool).await.unwrap();
        let archive = source.path().join("backup.zip");
        write_archive(&archive, manifest, folders).unwrap();
        sqlx::query("DELETE FROM meetings WHERE id = 'new'")
            .execute(&pool)
            .await
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let existing_ids = HashSet::from(["review-test".to_string()]);
        let (manifest, stage) = unpack(&archive, target.path(), &existing_ids).unwrap();
        // Assert before import/cleanup: existing audio was never extracted.
        assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 1);
        let existing_index = manifest.tables["meetings"]
            .iter()
            .position(|row| row["id"] == "review-test")
            .unwrap();
        assert!(!stage.path().join(existing_index.to_string()).exists());
        let stage_path = stage.path().to_path_buf();
        let report = import_manifest(&pool, manifest, stage).await.unwrap();
        assert_eq!((report.meetings, report.skipped, report.files), (1, 1, 1));
        let restored: String =
            sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'new'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let children = std::fs::read_dir(stage_path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(children, vec![PathBuf::from(&restored)]);
        assert_eq!(
            std::fs::read(Path::new(&restored).join("audio.wav")).unwrap(),
            b"synthetic audio"
        );
        let existing: String =
            sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'review-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(existing, source.path().to_string_lossy());
        assert!(source.path().join("audio.wav").exists());
        let existing_ids = HashSet::from(["review-test".to_string(), "new".to_string()]);
        let (manifest, stage) = unpack(&archive, target.path(), &existing_ids).unwrap();
        assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
        let discarded = stage.path().to_path_buf();
        assert_eq!(
            import_manifest(&pool, manifest, stage)
                .await
                .unwrap()
                .skipped,
            2
        );
        assert!(!discarded.exists());
    }

    #[tokio::test]
    async fn missing_folder_does_not_block_other_audio_and_restores_without_a_folder() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing");
        let available = root.path().join("available");
        std::fs::create_dir(&available).unwrap();
        std::fs::write(available.join("audio.wav"), b"synthetic audio").unwrap();
        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'review-test'")
            .bind(missing.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES ('available', 'Available audio', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
            .bind(available.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let (manifest, folders) = snapshot(&pool).await.unwrap();
        let missing_index = manifest.tables["meetings"]
            .iter()
            .position(|row| row["id"] == "review-test")
            .unwrap();
        let archive = root.path().join("backup.zip");
        let report = write_archive(&archive, manifest, folders).unwrap();
        assert_eq!((report.meetings, report.files), (2, 1));
        assert_eq!(report.incomplete_meetings.len(), 1);
        let warning = &report.incomplete_meetings[0];
        assert_eq!(warning.meeting_id, "review-test");
        assert!(warning.audio.recording_folder_missing && warning.audio.audio_unavailable);
        assert!(!warning.audio.recovery_files_excluded);
        let target = tempfile::tempdir().unwrap();
        let (manifest, stage) = unpack(&archive, target.path(), &HashSet::new()).unwrap();
        assert!(!manifest.files.contains_key(&missing_index));
        assert!(!stage.path().join(missing_index.to_string()).exists());
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        let restored = import_manifest(&pool, manifest, stage).await.unwrap();
        assert_eq!((restored.meetings, restored.files), (2, 1));
        assert_eq!(restored.incomplete_meetings.len(), 1);
        let warning = &restored.incomplete_meetings[0];
        assert_eq!(warning.meeting_id, "review-test");
        assert!(warning.audio.recording_folder_missing && warning.audio.audio_unavailable);
        let missing_folder: Option<String> =
            sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'review-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(missing_folder, None);
        let audio_folder: String =
            sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'available'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            std::fs::read(Path::new(&audio_folder).join("audio.wav")).unwrap(),
            b"synthetic audio"
        );
    }

    #[tokio::test]
    async fn unreadable_folder_names_meeting_and_preserves_previous_backup() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let not_a_folder = root.path().join("not-a-folder");
        std::fs::write(&not_a_folder, b"not a directory").unwrap();
        let (manifest, _) = snapshot(&pool).await.unwrap();
        let archive = root.path().join("backup.zip");
        std::fs::write(&archive, b"previous backup").unwrap();
        let error = write_archive(&archive, manifest, vec![Some(not_a_folder)])
            .err()
            .unwrap();
        assert!(error.contains("Synthetic review") && error.contains("cannot be read"));
        assert!(!error.contains(root.path().to_string_lossy().as_ref()));
        assert_eq!(std::fs::read(archive).unwrap(), b"previous backup");
    }

    #[tokio::test]
    async fn deleting_a_meeting_during_restore_requires_retry_instead_of_importing_without_audio() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("audio.wav"), b"synthetic audio").unwrap();
        let (manifest, _) = snapshot(&pool).await.unwrap();
        let archive = source.path().join("backup.zip");
        write_archive(&archive, manifest, vec![Some(source.path().to_path_buf())]).unwrap();
        let target = tempfile::tempdir().unwrap();
        let (manifest, stage) = unpack(
            &archive,
            target.path(),
            &HashSet::from(["review-test".to_string()]),
        )
        .unwrap();
        let stage_path = stage.path().to_path_buf();
        assert_eq!(std::fs::read_dir(&stage_path).unwrap().count(), 0);
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        let error = import_manifest(&pool, manifest, stage).await.err().unwrap();
        assert!(error.contains("changed during restore"));
        assert!(!stage_path.exists());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn v1_omission_metadata_is_optional_and_references_are_validated() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let (manifest, _) = snapshot(&pool).await.unwrap();
        let mut json = serde_json::to_value(&manifest).unwrap();
        json.as_object_mut().unwrap().remove("incomplete_audio");
        let legacy: Manifest = serde_json::from_value(json).unwrap();
        assert!(legacy.incomplete_audio.is_empty());
        let legacy_warning: IncompleteAudio = serde_json::from_value(serde_json::json!({
            "recovery_files_excluded": true,
            "audio_unavailable": false
        }))
        .unwrap();
        assert!(!legacy_warning.recording_folder_missing);
        assert!(legacy_warning.recovery_files_excluded);
        let mut manifest = legacy;
        manifest.incomplete_audio.insert(
            99,
            IncompleteAudio {
                recovery_files_excluded: true,
                audio_unavailable: true,
                ..Default::default()
            },
        );
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("invalid.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        zip.start_file("manifest.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.finish().unwrap();
        let target = root.path().join("restored");
        assert!(unpack(&archive, &target, &HashSet::new()).is_err());
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn saved_recovery_releases_registered_spool_and_clears_backup_exclusion() {
        use crate::audio::incremental_saver::{
            recover_audio_from_checkpoints, release_registered_capture,
        };
        use crate::audio::recording_state::{AudioChunk, DeviceType};
        for gap_marker in [false, true] {
            let pool = crate::database::transcript_edits::tests::fixture().await;
            let root = tempfile::tempdir().unwrap();
            let (sender, _, _) =
                crate::audio::transcription::queue::recording_audio_queue(root.path()).unwrap();
            sender
                .send(AudioChunk {
                    data: vec![0.05; 4800],
                    sample_rate: 48000,
                    timestamp: 0.0,
                    chunk_id: 0,
                    device_type: DeviceType::System,
                })
                .await
                .unwrap();
            drop(sender);
            let spool = root.path().join(".audio-spool");
            if gap_marker {
                std::fs::write(spool.join(".incomplete"), b"capture gap").unwrap();
            }
            let folder = root.path().to_string_lossy().into_owned();
            let recovered = recover_audio_from_checkpoints(folder.clone())
                .await
                .unwrap();
            assert!(recovered
                .audio_file_path
                .unwrap()
                .ends_with("audio-recovered.mp4"));
            assert!(release_registered_capture(&pool, &folder)
                .await
                .unwrap_err()
                .contains("not registered"));
            assert!(spool.exists());
            sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'review-test'")
                .bind(&folder)
                .execute(&pool)
                .await
                .unwrap();
            // A failed outcome write must keep the originals.
            std::fs::create_dir(root.path().join("recording-outcome.json")).unwrap();
            assert!(release_registered_capture(&pool, &folder).await.is_err());
            assert!(spool.exists());
            std::fs::remove_dir(root.path().join("recording-outcome.json")).unwrap();
            release_registered_capture(&pool, &folder).await.unwrap();
            assert!(!spool.exists());
            assert_eq!(
                crate::audio::outcome::RecordingOutcome::read(root.path())
                    .unwrap()
                    .unwrap()
                    .capture_incomplete,
                gap_marker
            );
            let (manifest, folders) = snapshot(&pool).await.unwrap();
            let archive = tempfile::tempdir().unwrap();
            let report =
                write_archive(&archive.path().join("backup.zip"), manifest, folders).unwrap();
            assert!(report.incomplete_meetings.is_empty());
        }
    }

    #[tokio::test]
    async fn recovery_originals_are_reported_without_blocking_backup_or_being_deleted() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete) VALUES ('review-test', 1, 0)").execute(&pool).await.unwrap();
        for directory in [".checkpoints", ".audio-spool"] {
            let root = tempfile::tempdir().unwrap();
            let chunks = root.path().join(directory);
            std::fs::create_dir(&chunks).unwrap();
            std::fs::write(chunks.join("synthetic-chunk"), b"only surviving audio").unwrap();
            // Backup works before and after recovery, which retains originals.
            for saved_audio in [false, true] {
                if saved_audio {
                    std::fs::write(root.path().join("audio-recovered.wav"), b"recovered audio")
                        .unwrap();
                }
                let (manifest, _) = snapshot(&pool).await.unwrap();
                let archive = root.path().join("backup.zip");
                let report =
                    write_archive(&archive, manifest, vec![Some(root.path().to_path_buf())])
                        .unwrap();
                assert_eq!(report.meetings, 1);
                assert_eq!(report.files, if saved_audio { 1 } else { 0 });
                assert_eq!(report.incomplete_meetings.len(), 1);
                let omitted = &report.incomplete_meetings[0];
                assert_eq!(omitted.title, "Synthetic review");
                assert!(omitted.audio.recovery_files_excluded);
                assert_eq!(omitted.audio.audio_unavailable, !saved_audio);
                let target = tempfile::tempdir().unwrap();
                let (manifest, stage) = unpack(&archive, target.path(), &HashSet::new()).unwrap();
                assert!(!stage.path().join("0").join(directory).exists());
                if saved_audio {
                    assert_eq!(
                        std::fs::read(stage.path().join("0/audio-recovered.wav")).unwrap(),
                        b"recovered audio"
                    );
                }
                let restored_pool = crate::database::transcript_edits::tests::fixture().await;
                sqlx::query("DELETE FROM meetings")
                    .execute(&restored_pool)
                    .await
                    .unwrap();
                let restored = import_manifest(&restored_pool, manifest, stage)
                    .await
                    .unwrap();
                assert_eq!(restored.incomplete_meetings.len(), 1);
                assert!(
                    restored.incomplete_meetings[0]
                        .audio
                        .recovery_files_excluded
                );
                assert_eq!(
                    restored.incomplete_meetings[0].audio.audio_unavailable,
                    !saved_audio
                );
                // Skipped meetings do not produce warnings about imported audio.
                let (manifest, stage) = unpack(
                    &archive,
                    target.path(),
                    &HashSet::from(["review-test".to_string()]),
                )
                .unwrap();
                assert!(import_manifest(&pool, manifest, stage)
                    .await
                    .unwrap()
                    .incomplete_meetings
                    .is_empty());
                assert_eq!(
                    std::fs::read(chunks.join("synthetic-chunk")).unwrap(),
                    b"only surviving audio"
                );
            }
        }
        let root = tempfile::tempdir().unwrap();
        for folder in [None, Some(root.path().to_path_buf())] {
            let (manifest, _) = snapshot(&pool).await.unwrap();
            let report =
                write_archive(&root.path().join("backup.zip"), manifest, vec![folder]).unwrap();
            assert_eq!(report.incomplete_meetings.len(), 1);
            assert!(report.incomplete_meetings[0].audio.audio_unavailable);
            assert!(!report.incomplete_meetings[0].audio.recovery_files_excluded);
        }
    }

    #[tokio::test]
    async fn older_v1_tables_are_optional_but_unknown_tables_and_missing_meetings_are_rejected() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("backup.zip");
        let (mut manifest, folders) = snapshot(&pool).await.unwrap();
        manifest.tables.remove("meeting_tags");
        manifest.tables.remove("meeting_bookmarks");
        write_archive(&archive, manifest, folders).unwrap();
        let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            import_manifest(&pool, manifest, stage)
                .await
                .unwrap()
                .meetings,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM transcripts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meeting_bookmarks")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        for missing_meetings in [false, true] {
            let (mut manifest, _) = snapshot(&pool).await.unwrap();
            if missing_meetings {
                manifest.tables.remove("meetings");
            } else {
                manifest.tables.insert("settings".into(), Vec::new());
            }
            let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
            zip.start_file("manifest.json", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&serde_json::to_vec(&manifest).unwrap())
                .unwrap();
            zip.finish().unwrap();
            assert!(unpack(&archive, root.path(), &HashSet::new()).is_err());
        }
    }

    #[tokio::test]
    async fn skipped_folder_cleanup_failure_rolls_back_import() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let (mut manifest, _) = snapshot(&pool).await.unwrap();
        let mut new_meeting = manifest.tables["meetings"][0].clone();
        new_meeting.insert("id".into(), Value::String("new".into()));
        manifest
            .tables
            .get_mut("meetings")
            .unwrap()
            .push(new_meeting);
        manifest.files.insert(0, vec!["audio.wav".into()]);
        let stage = tempfile::tempdir().unwrap();
        let stage_path = stage.path().to_path_buf();
        // A file in place of the stage directory simulates a cleanup failure.
        std::fs::write(stage.path().join("0"), b"not a directory").unwrap();
        assert!(import_manifest(&pool, manifest, stage).await.is_err());
        assert!(!stage_path.exists());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
    #[test]
    fn rejects_unsafe_and_private_files() {
        for name in [
            "../audio.wav",
            "a\\audio.wav",
            "C:audio.wav",
            "CON.wav",
            "settings.json",
            "tokens.json",
            ".env",
            "audio.wav.",
        ] {
            assert!(!allowed_file(name), "{name}");
        }
        assert!(allowed_file("audio.mp4"));
        assert!(allowed_file("exports.json"));
    }
    #[tokio::test]
    async fn context_round_trip_accepts_older_archives_and_is_deleted_with_the_meeting() {
        for legacy in [false, true] {
            let pool = crate::database::transcript_edits::tests::fixture().await;
            super::super::context::write(&pool, "review-test", "Synthetic summary context", false)
                .await
                .unwrap();
            let (mut manifest, folders) = snapshot(&pool).await.unwrap();
            if legacy {
                for meeting in manifest.tables.get_mut("meetings").unwrap() {
                    meeting.remove("summary_context");
                }
            }
            let root = tempfile::tempdir().unwrap();
            let archive = root.path().join("context.zip");
            write_archive(&archive, manifest, folders).unwrap();
            sqlx::query("DELETE FROM meetings")
                .execute(&pool)
                .await
                .unwrap();
            assert!(super::super::context::read(&pool, "review-test")
                .await
                .is_err());
            let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
            import_manifest(&pool, manifest, stage).await.unwrap();
            assert_eq!(
                super::super::context::read(&pool, "review-test")
                    .await
                    .unwrap(),
                if legacy {
                    ""
                } else {
                    "Synthetic summary context"
                }
            );
            sqlx::query("DELETE FROM meetings")
                .execute(&pool)
                .await
                .unwrap();
            assert!(super::super::context::read(&pool, "review-test")
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn round_trip_is_additive_and_excludes_credentials() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("audio.wav"), b"synthetic audio").unwrap();
        std::fs::write(folder.path().join("tokens.json"), b"excluded").unwrap();
        sqlx::query("UPDATE meetings SET folder_path = ?")
            .bind(folder.path().to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meeting_tags VALUES ('review-test', 'Example')")
            .execute(&pool)
            .await
            .unwrap();
        let (manifest, folders) = snapshot(&pool).await.unwrap();
        assert!(!manifest.tables.contains_key("settings"));
        let archive = folder.path().join("backup.zip");
        assert_eq!(write_archive(&archive, manifest, folders).unwrap().files, 1);
        let target = tempfile::tempdir().unwrap();
        let (manifest, stage) = unpack(&archive, target.path(), &HashSet::new()).unwrap();
        let report = import_manifest(&pool, manifest, stage).await.unwrap();
        assert_eq!(report.skipped, 1);
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        let (manifest, stage) = unpack(&archive, target.path(), &HashSet::new()).unwrap();
        let report = import_manifest(&pool, manifest, stage).await.unwrap();
        assert_eq!(report.meetings, 1);
        let restored: String = sqlx::query_scalar("SELECT folder_path FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(Path::new(&restored).join("audio.wav")).unwrap(),
            b"synthetic audio"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM transcripts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meeting_tags")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn invalid_rows_roll_back_the_whole_import_and_remove_staged_files() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let (mut manifest, _) = snapshot(&pool).await.unwrap();
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        manifest.tables.get_mut("transcripts").unwrap()[1]
            .insert("unexpected_column".into(), Value::String("invalid".into()));
        let stage = tempfile::tempdir().unwrap();
        let stage_path = stage.path().to_path_buf();
        assert!(import_manifest(&pool, manifest, stage).await.is_err());
        assert!(!stage_path.exists());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM transcripts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn archive_traversal_is_rejected_before_extraction() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let (mut manifest, _) = snapshot(&pool).await.unwrap();
        manifest.files.insert(0, vec!["../escape.wav".into()]);
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("unsafe.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file("manifest.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.start_file("recordings/0/../escape.wav", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"invalid").unwrap();
        zip.finish().unwrap();
        let destination = root.path().join("restored");
        assert!(unpack(
            &path,
            &destination,
            &HashSet::from(["review-test".to_string()])
        )
        .is_err());
        assert!(!destination.exists());
    }
}
