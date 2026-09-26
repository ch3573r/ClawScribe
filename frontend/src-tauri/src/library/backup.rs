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
}

#[derive(Serialize)]
pub struct BackupReport {
    pub meetings: usize,
    pub skipped: usize,
    pub files: usize,
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
                return Err(meeting_failure(meeting, "audio recovery is pending but its recording folder is unavailable. Restore access to the recording folder before backing up."));
            }
            continue;
        };
        // Missing recording folders must not silently produce an incomplete backup.
        if std::fs::symlink_metadata(folder)
            .map_err(|_| meeting_failure(meeting, "the recording folder is missing or unreadable. Reconnect its drive or restore the folder, then retry."))?
            .file_type()
            .is_symlink()
        {
            return Err("Recording folder links are not supported in backups.".into());
        }
        // Recovery originals can be the only surviving capture, even if a partial
        // final file exists. V1 archives cannot represent these directories.
        for recovery_dir in [".audio-spool", ".checkpoints"] {
            match std::fs::symlink_metadata(folder.join(recovery_dir)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    let mut entries = std::fs::read_dir(folder.join(recovery_dir))
                        .map_err(|_| meeting_failure(meeting, "recovery files could not be checked. Restore folder access, then retry."))?;
                    if entries.next().is_none() {
                        continue;
                    }
                }
                _ => {}
            }
            return Err(meeting_failure(meeting, "recording recovery files are still present and cannot be included in this library archive. Preserve a separate copy of the recording folder, including hidden recovery files. Do not delete recovery originals to make backup succeed."));
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(folder).map_err(|_| {
            meeting_failure(
                meeting,
                "the recording folder cannot be read. Restore folder access, then retry.",
            )
        })? {
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
        if recovery_pending
            && !names
                .iter()
                .any(|name| Path::new(name).extension().is_some_and(|ext| ext != "json"))
        {
            return Err(meeting_failure(meeting, "audio recovery is pending and no saved audio is available. Recover the recording before creating a library backup."));
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
    })
}

fn unpack(path: &Path, root: &Path) -> Result<(Manifest, tempfile::TempDir), String> {
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
            std::fs::remove_dir_all(stage.path().join(index.to_string())).map_err(failure)?;
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
    let (manifest, stage) = tokio::task::spawn_blocking(move || unpack(Path::new(&path), &root))
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
        let (manifest, stage) = unpack(&archive, target.path()).unwrap();
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
        let (manifest, stage) = unpack(&archive, target.path()).unwrap();
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
    async fn missing_folder_names_meeting_and_preserves_previous_backup() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let (manifest, _) = snapshot(&pool).await.unwrap();
        let archive = root.path().join("backup.zip");
        std::fs::write(&archive, b"previous backup").unwrap();
        let error = write_archive(&archive, manifest, vec![Some(root.path().join("missing"))])
            .err()
            .unwrap();
        assert!(error.contains("Synthetic review") && error.contains("missing or unreadable"));
        assert!(!error.contains(root.path().to_string_lossy().as_ref()));
        assert_eq!(std::fs::read(archive).unwrap(), b"previous backup");
    }

    #[tokio::test]
    async fn recovery_originals_cannot_be_silently_omitted() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete) VALUES ('review-test', 1, 0)").execute(&pool).await.unwrap();
        for directory in [".checkpoints", ".audio-spool"] {
            let root = tempfile::tempdir().unwrap();
            let chunks = root.path().join(directory);
            std::fs::create_dir(&chunks).unwrap();
            std::fs::write(chunks.join("synthetic-chunk"), b"only surviving audio").unwrap();
            // Even a partial/recovered output is not a reason to discard originals.
            for saved_audio in [false, true] {
                if saved_audio {
                    std::fs::write(root.path().join("audio.wav"), b"partial audio").unwrap();
                }
                let (manifest, _) = snapshot(&pool).await.unwrap();
                let archive = root.path().join("backup.zip");
                let error =
                    write_archive(&archive, manifest, vec![Some(root.path().to_path_buf())])
                        .err()
                        .unwrap();
                assert!(error.contains("Synthetic review") && error.contains("recovery files"));
                assert!(!archive.exists());
                assert_eq!(
                    std::fs::read(chunks.join("synthetic-chunk")).unwrap(),
                    b"only surviving audio"
                );
            }
        }
        let root = tempfile::tempdir().unwrap();
        for folder in [None, Some(root.path().to_path_buf())] {
            let (manifest, _) = snapshot(&pool).await.unwrap();
            assert!(
                write_archive(&root.path().join("backup.zip"), manifest, vec![folder])
                    .err()
                    .unwrap()
                    .contains("audio recovery is pending")
            );
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
        let (manifest, stage) = unpack(&archive, root.path()).unwrap();
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
            assert!(unpack(&archive, root.path()).is_err());
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
        // The missing stage directory simulates a filesystem cleanup failure.
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
        let (manifest, stage) = unpack(&archive, target.path()).unwrap();
        let report = import_manifest(&pool, manifest, stage).await.unwrap();
        assert_eq!(report.skipped, 1);
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        let (manifest, stage) = unpack(&archive, target.path()).unwrap();
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
        assert!(unpack(&path, &destination).is_err());
        assert!(!destination.exists());
    }
}
