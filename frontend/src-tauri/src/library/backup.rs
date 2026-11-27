//! Portable, additive meeting archives. Provider settings and credentials are never selected.
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use tauri::{AppHandle, Emitter, Manager};
use zeroize::Zeroizing;
use zip::write::SimpleFileOptions;
#[path = "backup/documents.rs"]
mod archive_documents;
#[path = "backup/exports.rs"]
mod archive_exports;
#[path = "backup/follow_through.rs"]
mod archive_follow_through;
#[path = "backup/encryption.rs"]
pub(crate) mod encryption;
#[path = "backup/preferences.rs"]
mod preferences;
#[path = "backup/records.rs"]
mod records;
#[path = "backup/repair.rs"]
mod repair;

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
    "meeting_summary_bookmarks",
    "knowledge_owners",
    "knowledge_requests",
    "knowledge_messages",
    "knowledge_request_evidence",
    "knowledge_request_sources",
];
const MAX_MANIFEST: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_BYTES: u64 = 100 * 1024 * 1024 * 1024;
static ARCHIVE_JOB: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub(crate) static PORTABLE_SETTINGS_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

pub(crate) fn clean_backup_scratch_on_startup(app: &AppHandle) {
    if let (Ok(local_data), Ok(archive)) = (app.path().app_local_data_dir(), ARCHIVE_JOB.try_lock())
    {
        tauri::async_runtime::spawn_blocking(move || {
            let _archive = archive;
            if let Err(error) = encryption::prepare_scratch(&local_data) {
                log::warn!("Could not clean the encrypted backup scratch directory. {error}");
            }
        });
    }
}

pub(super) const BACKUP_DEFERRED: &str = "Scheduled backup deferred for a recording.";
const NO_SPACE: &str = "Not enough free disk space for the scheduled backup.";
const COPYING: u8 = 0;
const DEFERRED: u8 = 1;
const SEALED: u8 = 2;

#[derive(Default)]
pub(super) struct ScheduledBackup {
    state: AtomicU8,
    records_budget: Option<u64>,
    archive_budget: Option<u64>,
    shared_volume: bool,
    #[cfg(test)]
    pause_copy: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
}

impl ScheduledBackup {
    pub(super) fn new(
        records_volume: Option<(PathBuf, u64)>,
        archive_volume: Option<(PathBuf, u64)>,
    ) -> Self {
        use crate::audio::disk_space::START_FLOOR_BYTES;
        let budget = |volume: &(_, u64)| volume.1.saturating_sub(START_FLOOR_BYTES);
        Self {
            records_budget: records_volume.as_ref().map(budget),
            archive_budget: archive_volume.as_ref().map(budget),
            shared_volume: records_volume
                .zip(archive_volume)
                .is_some_and(|((source, _), (target, _))| source == target),
            ..Self::default()
        }
    }

    fn defer(&self) -> bool {
        !matches!(
            self.state
                .compare_exchange(COPYING, DEFERRED, Ordering::AcqRel, Ordering::Acquire),
            Err(SEALED)
        )
    }

    fn seal(&self) -> Result<(), String> {
        self.state
            .compare_exchange(COPYING, SEALED, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| BACKUP_DEFERRED.into())
    }

    fn checkpoint(&self) -> Result<(), String> {
        match self.state.load(Ordering::Acquire) {
            DEFERRED => Err(BACKUP_DEFERRED.into()),
            COPYING
                if crate::summary::SummaryService::has_active_local_summary() && self.defer() =>
            {
                Err(BACKUP_DEFERRED.into())
            }
            _ => Ok(()),
        }
    }

    fn archive_budget(&self, staged_bytes: u64) -> Option<u64> {
        self.archive_budget.map(|budget| {
            if self.shared_volume {
                budget.saturating_sub(staged_bytes)
            } else {
                budget
            }
        })
    }
}

fn backup_checkpoint(scheduled: Option<&ScheduledBackup>) -> Result<(), String> {
    scheduled.map_or(Ok(()), ScheduledBackup::checkpoint)
}

// Job admission stays responsive even inside a single large ZIP member. This
// also bounds the space consumed by the stored ZIP, including its headers.
struct ArchiveOutput<'a> {
    file: &'a mut File,
    scheduled: Option<&'a ScheduledBackup>,
    limit: Option<u64>,
    no_space: &'static str,
    position: u64,
}

impl Write for ArchiveOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(test)]
        if self.position >= 64 * 1024 {
            if let Some((entered, blocked)) = self
                .scheduled
                .and_then(|scheduled| scheduled.pause_copy.lock().unwrap().take())
            {
                let _ = entered.send(());
                let _ = blocked.recv_timeout(std::time::Duration::from_secs(5));
            }
        }
        backup_checkpoint(self.scheduled).map_err(io::Error::other)?;
        let bytes = &bytes[..bytes.len().min(64 * 1024)];
        if self
            .limit
            .is_some_and(|limit| self.position.saturating_add(bytes.len() as u64) > limit)
        {
            return Err(io::Error::other(self.no_space));
        }
        let written = self.file.write(bytes)?;
        self.position += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        backup_checkpoint(self.scheduled).map_err(io::Error::other)?;
        self.file.flush()
    }
}

impl Seek for ArchiveOutput<'_> {
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        backup_checkpoint(self.scheduled).map_err(io::Error::other)?;
        self.position = self.file.seek(seek)?;
        Ok(self.position)
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    tables: BTreeMap<String, Vec<Map<String, Value>>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    table_records: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "preferences::Portable::is_empty")]
    portable: preferences::Portable,
    #[serde(
        default,
        skip_serializing_if = "archive_follow_through::Portable::is_empty"
    )]
    follow_through: archive_follow_through::Portable,
    // Index into the meetings array -> leaf filenames. Never contains source paths.
    files: BTreeMap<usize, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    removed_audio: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    audio_excluded: bool,
    // Optional in V1; omissions remain visible when this archive is restored.
    #[serde(default)]
    incomplete_audio: BTreeMap<usize, IncompleteAudio>,
    #[serde(default)]
    document_files: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    export_ledgers: BTreeMap<usize, crate::exports::ExportLedger>,
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
    pub repaired_audio: usize,
    pub unavailable_audio_locations: usize,
    pub files: usize,
    pub audio_excluded: bool,
    /// Meetings restored without a calendar link that could not be read.
    pub unreadable_calendar_links: usize,
    pub skipped_template_items: usize,
    pub incomplete_meetings: Vec<IncompleteMeeting>,
    pub omitted_saved_live_answers: u64,
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

fn copy_entry(input: &mut impl Read, output: &mut impl Write, size: u64) -> Result<(), String> {
    let copied = std::io::copy(&mut input.take(size + 1), output).map_err(failure)?;
    if copied != size {
        return Err("Archive entry size does not match.".into());
    }
    Ok(())
}

// zip 2.x indexes the central directory by name and collapses duplicate names.
// Check that the indexed headers account for every physical directory entry.
fn verify_central_directory(
    file: &mut File,
    mut position: u64,
    mut headers: Vec<u64>,
) -> Result<(), String> {
    headers.sort_unstable();
    for offset in headers {
        if offset != position {
            return Err("Duplicate or unexpected archive entry.".into());
        }
        file.seek(SeekFrom::Start(position)).map_err(failure)?;
        let mut header = [0u8; 46];
        file.read_exact(&mut header).map_err(failure)?;
        if header[..4] != *b"PK\x01\x02" {
            return Err("Invalid archive directory entry.".into());
        }
        // Central headers end with a variable-length name, extra data and comment.
        let variable: u64 = header[28..34]
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) as u64)
            .sum();
        position = position
            .checked_add(46 + variable)
            .ok_or("Archive is too large.")?;
    }
    file.seek(SeekFrom::Start(position)).map_err(failure)?;
    let mut signature = [0; 4];
    file.read_exact(&mut signature).map_err(failure)?;
    if signature == *b"PK\x01\x02" {
        return Err("Duplicate or unexpected archive entry.".into());
    }
    Ok(())
}

fn meeting_failure(row: &Map<String, Value>, reason: &str) -> String {
    let title = row
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled meeting");
    // User-facing context only; never include source paths or underlying I/O errors.
    format!("Cannot back up meeting \"{title}\": {reason}")
}

pub(super) fn allowed_file(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 240
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(char::is_control)
        || (name.starts_with('.') && name != crate::audio::import::IMPORT_OWNER_FILE)
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
        "metadata.json"
            | "transcripts.json"
            | "exports.json"
            | "recording-outcome.json"
            | crate::audio::import::IMPORT_OWNER_FILE
    ) || archive_audio_file(name)
}

pub(super) fn archive_audio_file(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "mp4" | "m4a" | "wav" | "mp3" | "flac" | "ogg" | "aac" | "webm" | "mkv" | "wma"
            )
        })
}

#[cfg(test)]
async fn snapshot(pool: &SqlitePool) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    snapshot_impl(pool, false).await
}

#[cfg(test)]
async fn snapshot_impl(
    pool: &SqlitePool,
    with_documents: bool,
) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    super::attach_bookmarks(pool).await.map_err(failure)?;
    snapshot_tables(pool, TABLES, with_documents).await
}

#[cfg(test)]
async fn snapshot_tables(
    pool: &SqlitePool,
    meeting_tables: &[&str],
    with_documents: bool,
) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    snapshot_tables_impl(pool, meeting_tables, with_documents, None, None).await
}

async fn snapshot_tables_impl(
    pool: &SqlitePool,
    meeting_tables: &[&str],
    with_documents: bool,
    record_files: Option<&records::SnapshotFiles>,
    scheduled: Option<&ScheduledBackup>,
) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    let mut tx = pool.begin().await.map_err(failure)?;
    let mut tables = BTreeMap::new();
    let mut table_records = BTreeMap::new();
    let mut records_bytes = 0;
    let mut budget = MAX_MANIFEST;
    for table in meeting_tables
        .iter()
        .chain(archive_documents::TABLES.iter().filter(|_| with_documents))
    {
        backup_checkpoint(scheduled)?;
        let columns = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(&mut *tx)
            .await
            .map_err(failure)?;
        let args = columns
            .iter()
            .filter_map(|c| {
                let name: String = c.get("name");
                // Resume authorization belongs to the source profile.
                (*table != "summary_processes" || name != "automatic_request_fingerprint")
                    .then(|| format!("'{name}', \"{name}\""))
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut rows = Vec::new();
        // Only references owned by meetings in this snapshot may leave the PC.
        let ownership = "SELECT a.document_id FROM knowledge_document_attachments a
                         JOIN meetings m ON m.id=a.meeting_id";
        let filter = match *table {
            "knowledge_documents" => format!(" WHERE id IN ({ownership})"),
            "knowledge_document_blocks" => format!(" WHERE document_id IN ({ownership})"),
            "knowledge_document_attachments" => {
                " WHERE meeting_id IN (SELECT id FROM meetings)".into()
            }
            _ => String::new(),
        };
        let select = format!(
            "SELECT rowid AS record_id, json_object({args}) AS record FROM {table}{filter}"
        );
        if let Some(files) = record_files.filter(|_| records::TABLES.contains(table)) {
            let count = records::snapshot(
                &mut tx,
                files,
                table,
                &select,
                &mut records_bytes,
                scheduled,
            )
            .await?;
            table_records.insert(table.to_string(), count);
            continue;
        }
        // Page the snapshot so the SQLite result itself is bounded.
        loop {
            backup_checkpoint(scheduled)?;
            let page: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT record FROM ({select}) ORDER BY record_id LIMIT 500 OFFSET ?"
            ))
            .bind(rows.len() as i64)
            .fetch_all(&mut *tx)
            .await
            .map_err(failure)?;
            let count = page.len();
            for row in page {
                backup_checkpoint(scheduled)?;
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
    let mut portable = preferences::Portable::default();
    backup_checkpoint(scheduled)?;
    let follow_through = if record_files.is_some() {
        portable.series = preferences::capture_series(&mut tx, &tables["meetings"]).await?;
        archive_follow_through::Portable::capture(
            &mut tx,
            &tables["meetings"],
            &mut budget,
            scheduled,
        )
        .await?
    } else {
        archive_follow_through::Portable::default()
    };
    tx.commit().await.map_err(failure)?;
    let mut folders = Vec::new();
    let mut removed_audio = BTreeSet::new();
    for row in tables.get_mut("meetings").unwrap() {
        folders.push(
            row.get("folder_path")
                .and_then(Value::as_str)
                .map(PathBuf::from),
        );
        row.insert("folder_path".into(), Value::Null);
        if row
            .remove("saved_audio_removed_by_user")
            .and_then(|value| value.as_i64())
            == Some(1)
        {
            removed_audio.insert(text_field(row, "id")?.to_owned());
        }
    }
    // Orphan live markers belong to interrupted recordings, not the saved library.
    tables
        .get_mut("meeting_bookmarks")
        .unwrap()
        .retain(|row| row.get("meeting_id").is_some_and(Value::is_string));
    for row in tables.get_mut("meeting_bookmarks").unwrap() {
        row.insert("folder_path".into(), Value::Null);
        row.remove("recording_id");
    }
    Ok((
        Manifest {
            format: "ClawScribe meeting archive".into(),
            version: if record_files.is_some() {
                3
            } else if with_documents {
                2
            } else {
                1
            },
            tables,
            table_records,
            portable,
            follow_through,
            files: BTreeMap::new(),
            removed_audio,
            audio_excluded: false,
            incomplete_audio: BTreeMap::new(),
            document_files: BTreeMap::new(),
            export_ledgers: BTreeMap::new(),
        },
        folders,
    ))
}

#[cfg(test)]
fn write_archive(
    path: &Path,
    manifest: Manifest,
    folders: Vec<Option<PathBuf>>,
) -> Result<BackupReport, String> {
    write_archive_impl(path, manifest, folders, None, None, false)
}

#[cfg(test)]
fn write_archive_impl(
    path: &Path,
    manifest: Manifest,
    folders: Vec<Option<PathBuf>>,
    reference_root: Option<&Path>,
    records_root: Option<&Path>,
    exclusive: bool,
) -> Result<BackupReport, String> {
    let records = records_root.map(|path| records::SnapshotFiles::Named(path.to_owned()));
    prepare_archive(
        archive_tempfile(path)?,
        manifest,
        folders,
        reference_root,
        records.as_ref(),
        None,
        false,
    )?
    .publish(path, exclusive)
}

fn archive_tempfile(path: &Path) -> Result<tempfile::NamedTempFile, String> {
    tempfile::NamedTempFile::new_in(path.parent().ok_or("Choose a destination folder.")?)
        .map_err(failure)
}

struct PreparedBackup {
    output: tempfile::NamedTempFile,
    report: BackupReport,
}

impl PreparedBackup {
    fn publish(self, path: &Path, exclusive: bool) -> Result<BackupReport, String> {
        self.output.as_file().sync_all().map_err(failure)?;
        if exclusive {
            // A renamed or replaced temporary file must never be published.
            self.output.reopen().map_err(failure)?;
            self.output.persist_noclobber(path).map_err(failure)?;
        } else {
            self.output.persist(path).map_err(failure)?;
        }
        Ok(self.report)
    }
}

fn prepare_archive(
    mut output: tempfile::NamedTempFile,
    manifest: Manifest,
    folders: Vec<Option<PathBuf>>,
    reference_root: Option<&Path>,
    record_files: Option<&records::SnapshotFiles>,
    scheduled: Option<&ScheduledBackup>,
    exclude_audio: bool,
) -> Result<PreparedBackup, String> {
    let report = build_archive(
        output.as_file_mut(),
        manifest,
        folders,
        reference_root,
        record_files,
        scheduled,
        exclude_audio,
        None,
    )?;
    Ok(PreparedBackup { output, report })
}

fn build_archive(
    output: &mut File,
    mut manifest: Manifest,
    folders: Vec<Option<PathBuf>>,
    reference_root: Option<&Path>,
    record_files: Option<&records::SnapshotFiles>,
    scheduled: Option<&ScheduledBackup>,
    exclude_audio: bool,
    scratch_limit: Option<u64>,
) -> Result<BackupReport, String> {
    let result = write_archive_contents(
        output,
        &mut manifest,
        folders,
        reference_root,
        record_files,
        scheduled,
        exclude_audio,
        scratch_limit,
    );
    backup_checkpoint(scheduled)?;
    let files = result?;
    Ok(BackupReport {
        meetings: manifest.tables["meetings"].len(),
        skipped: 0,
        repaired_audio: 0,
        unavailable_audio_locations: 0,
        files,
        audio_excluded: manifest.audio_excluded,
        unreadable_calendar_links: 0,
        skipped_template_items: 0,
        incomplete_meetings: incomplete_meetings(&manifest, None),
        omitted_saved_live_answers: manifest.follow_through.omitted_answers(None),
    })
}

fn estimate_zip_size(
    manifest: &Manifest,
    folders: &[Option<PathBuf>],
    records: &records::SnapshotFiles,
) -> Result<u64, String> {
    #[derive(Default)]
    struct Size(u64);
    impl Write for Size {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut metadata = Size::default();
    serde_json::to_writer(&mut metadata, manifest).map_err(failure)?;
    // Stored ZIP entries add local and central headers, plus manifest filenames.
    const ENTRY_OVERHEAD: u64 = 1024;
    let mut bytes = metadata
        .0
        .saturating_add(records.bytes()?)
        .saturating_add((records::TABLES.len() as u64 + 1) * ENTRY_OVERHEAD);
    for row in manifest
        .tables
        .get("knowledge_documents")
        .into_iter()
        .flatten()
    {
        bytes = bytes
            .saturating_add(row.get("file_size").and_then(Value::as_u64).unwrap_or(0))
            .saturating_add(ENTRY_OVERHEAD);
    }
    for folder in folders.iter().flatten() {
        if !std::fs::symlink_metadata(folder)
            .is_ok_and(|metadata| metadata.is_dir() && !crate::local_path::is_link(&metadata))
        {
            // The writer will report any invalid source by meeting title.
            continue;
        }
        let entries = match std::fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(failure(error)),
        };
        bytes = bytes.saturating_add(ENTRY_OVERHEAD);
        for entry in entries {
            let entry = entry.map_err(failure)?;
            if allowed_file(&entry.file_name().to_string_lossy()) {
                let size = entry.metadata().map_err(failure)?.len();
                bytes = bytes.saturating_add(size).saturating_add(ENTRY_OVERHEAD);
            }
        }
    }
    Ok(bytes)
}

fn write_archive_contents(
    output: &mut File,
    manifest: &mut Manifest,
    folders: Vec<Option<PathBuf>>,
    reference_root: Option<&Path>,
    record_files: Option<&records::SnapshotFiles>,
    scheduled: Option<&ScheduledBackup>,
    exclude_audio: bool,
    scratch_limit: Option<u64>,
) -> Result<usize, String> {
    backup_checkpoint(scheduled)?;
    manifest.audio_excluded = exclude_audio;
    archive_documents::set_file_references(manifest)?;
    let documents = archive_documents::validate(manifest)?;
    manifest.portable.validate(&manifest.tables["meetings"])?;
    manifest.follow_through.validate(manifest, scheduled)?;
    let staged_bytes = record_files.map_or(Ok(0), records::SnapshotFiles::bytes)?;
    let limit =
        scratch_limit.or_else(|| scheduled.and_then(|backup| backup.archive_budget(staged_bytes)));
    let no_space = if scratch_limit.is_some() {
        encryption::SCRATCH_FULL
    } else {
        NO_SPACE
    };
    if limit.is_some_and(|limit| staged_bytes > limit) {
        return Err(no_space.into());
    }
    let mut zip = zip::ZipWriter::new(ArchiveOutput {
        file: output,
        scheduled,
        limit,
        no_space,
        position: 0,
    });
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let mut files = 0;
    let mut total = match record_files {
        Some(files) => records::write(&mut zip, files, options)?,
        None => 0,
    };
    let mut history_budget = archive_exports::remaining_budget(&manifest.export_ledgers)?;
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
        backup_checkpoint(scheduled)?;
        let meeting = &manifest.tables["meetings"][index];
        let recovery_pending = meeting
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| pending_audio.contains(id));
        let Some(folder) = folder else {
            if !exclude_audio && recovery_pending {
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
        if crate::local_path::is_link(&metadata) {
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
                Ok(metadata) if metadata.is_dir() && !crate::local_path::is_link(&metadata) => {
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
        let mut unique = HashSet::new();
        for entry in entries {
            backup_checkpoint(scheduled)?;
            let entry = entry.map_err(failure)?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !allowed_file(&name) || (exclude_audio && archive_audio_file(&name)) {
                continue;
            }
            let metadata = entry.metadata().map_err(failure)?;
            if !metadata.is_file() || crate::local_path::is_link(&metadata) {
                return Err("Recording file links are not supported in backups.".into());
            }
            if !unique.insert(name.to_lowercase()) {
                return Err("Recording files have conflicting names.".into());
            }
            if name == crate::exports::ledger::LEDGER_FILENAME {
                archive_exports::capture_legacy_folder(
                    &mut manifest.export_ledgers,
                    index,
                    text_field(meeting, "id")?,
                    folder,
                    &mut history_budget,
                )
                .map_err(|_| meeting_failure(meeting, archive_exports::UNREADABLE_HISTORY))?;
                continue;
            }
            let mut input = File::open(entry.path()).map_err(failure)?;
            let size = input.metadata().map_err(failure)?.len();
            has_audio |= size > 0 && archive_audio_file(&name);
            total = total.checked_add(size).ok_or("Archive is too large.")?;
            files += 1;
            if files > MAX_FILES || total > MAX_BYTES {
                return Err("Archive exceeds 100 GiB or 100,000 files.".into());
            }
            if limit.is_some_and(|limit| total > limit) {
                return Err(no_space.into());
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
        incomplete.audio_unavailable = !exclude_audio
            && !has_audio
            && (recovery_pending || incomplete.recovery_files_excluded);
        if incomplete.audio_unavailable || incomplete.recovery_files_excluded {
            manifest.incomplete_audio.insert(index, incomplete);
        }
        if !exclude_audio || !names.is_empty() {
            manifest.files.insert(index, names);
        }
    }
    archive_documents::write_originals(
        &mut zip,
        &documents,
        reference_root,
        options,
        &mut files,
        &mut total,
    )?;
    backup_checkpoint(scheduled)?;
    archive_exports::validate(manifest)?;
    let bytes = serde_json::to_vec(manifest).map_err(failure)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("Meeting metadata exceeds the archive limit.".into());
    }
    zip.start_file("manifest.json", options).map_err(failure)?;
    zip.write_all(&bytes).map_err(failure)?;
    zip.finish().map_err(failure)?;
    Ok(files)
}

#[cfg(test)]
fn unpack(
    path: &Path,
    root: &Path,
    existing_ids: &HashSet<String>,
) -> Result<(Manifest, tempfile::TempDir), String> {
    let file = File::open(path).map_err(failure)?;
    unpack_archive(file, root, None, |id, _| {
        if existing_ids.contains(id) {
            Extract::Skip
        } else {
            Extract::All
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Extract {
    Skip,
    All,
    Audio,
    AudioWithMarkers,
}

fn unpack_archive(
    mut file: File,
    root: &Path,
    scratch_volume: Option<&Path>,
    mut extract: impl FnMut(&str, bool) -> Extract,
) -> Result<(Manifest, tempfile::TempDir), String> {
    let mut zip = zip::ZipArchive::new(file.try_clone().map_err(failure)?).map_err(failure)?;
    if zip.len() > MAX_FILES + 1 + records::TABLES.len() {
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
    let legacy_history_budget = MAX_MANIFEST - bytes.len() as u64;
    let mut manifest: Manifest = serde_json::from_slice(&bytes).map_err(failure)?;
    drop(bytes);
    // Older manifests can contain profile-local delivery rows. Discard them
    // before validating the portable table list.
    manifest.tables.remove("openclaw_handoffs");
    if !matches!(manifest.version, 1..=3)
        || manifest.format != "ClawScribe meeting archive"
        || !manifest.tables.contains_key("meetings")
        || !records::validate_layout(&manifest)
        || (manifest.version < 3 && !manifest.portable.is_empty())
        || manifest.tables.keys().any(|table| {
            !TABLES.contains(&table.as_str())
                && !(manifest.version >= 2 && archive_documents::TABLES.contains(&table.as_str()))
        })
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
        validate_reviewed_actions(row)?;
    }
    let documents = archive_documents::validate(&manifest)?;
    if manifest
        .removed_audio
        .iter()
        .any(|id| !known.contains(id.as_str()))
    {
        return Err("Invalid archived audio removal reference.".into());
    }
    manifest.portable.validate(&manifest.tables["meetings"])?;
    manifest.follow_through.validate(&manifest, None)?;
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
    for document in documents.values() {
        if !expected.insert(document.relative()) {
            return Err("Unsafe or duplicate document filename.".into());
        }
    }
    if expected.len() > MAX_FILES + 1 {
        return Err("Too many archive files.".into());
    }
    expected.extend(
        manifest
            .table_records
            .keys()
            .map(|table| records::entry_name(table)),
    );
    if expected.len() != zip.len() {
        return Err("Unexpected or missing files in archive.".into());
    }
    let mut total = 0u64;
    let mut central_headers = Vec::with_capacity(zip.len());
    let mut local_headers = HashSet::with_capacity(zip.len());
    let mut sizes = HashMap::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(failure)?;
        if !expected.remove(entry.name())
            || entry.is_dir()
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            || !local_headers.insert(entry.header_start())
        {
            return Err("Unsafe archive entry.".into());
        }
        central_headers.push(entry.central_header_start());
        if entry.name() == "manifest.json" {
            continue;
        }
        if entry.name().starts_with("recordings/") {
            sizes.insert(entry.name().to_owned(), entry.size());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("Archive is too large.")?;
        if total > MAX_BYTES {
            return Err("Archive exceeds 100 GiB.".into());
        }
    }
    verify_central_directory(&mut file, zip.central_directory_start(), central_headers)?;
    archive_exports::read_legacy(&mut manifest, &mut zip, legacy_history_budget)?;
    archive_exports::validate(&manifest)?;
    // Verify every original before creating any stage or publishing any rows.
    archive_documents::verify_zip(&mut zip, &documents)?;
    let mut extraction_bytes = total;
    let mut recordings = Vec::new();
    for (index, names) in &manifest.files {
        let id = manifest.tables["meetings"][*index]["id"].as_str().unwrap();
        let has_audio = names.iter().any(|name| {
            archive_audio_file(name) && sizes[&format!("recordings/{index}/{name}")] != 0
        });
        let selection = extract(id, has_audio);
        let mut files = Vec::new();
        for name in names {
            let entry = format!("recordings/{index}/{name}");
            let size = sizes[&entry];
            let include = match selection {
                Extract::All => true,
                Extract::Audio | Extract::AudioWithMarkers => {
                    (archive_audio_file(name) && size != 0)
                        || (selection == Extract::AudioWithMarkers && repair::marker_file(name))
                }
                Extract::Skip => false,
            };
            if include {
                files.push((name, entry));
            } else {
                extraction_bytes -= size;
            }
        }
        if selection != Extract::Skip {
            recordings.push((index, files));
        }
    }
    if let Some(volume) = scratch_volume {
        encryption::check_extraction_space(
            volume,
            crate::audio::disk_space::free_space_volume(root),
            extraction_bytes,
        )?;
    }
    std::fs::create_dir_all(root).map_err(failure)?;
    let stage = tempfile::Builder::new()
        .prefix("restored-")
        .tempdir_in(root)
        .map_err(failure)?;
    records::extract(&mut zip, stage.path(), &manifest.table_records)?;
    for (index, files) in recordings {
        let folder = stage.path().join(index.to_string());
        std::fs::create_dir(&folder).map_err(failure)?;
        for (name, entry) in files {
            let mut source = zip.by_name(&entry).map_err(failure)?;
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
    archive_documents::extract_originals(&mut zip, &documents, stage.path())?;
    Ok((manifest, stage))
}

fn validate_reviewed_actions(row: &Map<String, Value>) -> Result<(), String> {
    if let Some(actions) = row.get("reviewed_actions").filter(|value| !value.is_null()) {
        crate::exports::actions::decode(actions.as_str().ok_or("Invalid saved meeting actions.")?)?;
    }
    Ok(())
}

fn text_field<'a>(row: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    row.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| "Invalid conversation archive field.".into())
}
fn uuid_field(row: &Map<String, Value>, key: &str) -> Result<String, String> {
    let value = text_field(row, key)?;
    if uuid::Uuid::parse_str(value)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        != Some(value)
    {
        return Err("Invalid conversation UUID.".into());
    }
    Ok(value.into())
}
async fn insert_archive_row(
    connection: &mut sqlx::SqliteConnection,
    table: &str,
    row: &Map<String, Value>,
) -> Result<(), String> {
    let names = row
        .keys()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut query =
        sqlx::QueryBuilder::<sqlx::Sqlite>::new(format!("INSERT INTO {table} ({names}) VALUES ("));
    let mut values = query.separated(",");
    for value in row.values() {
        match value {
            Value::Null => {
                values.push_bind(Option::<String>::None);
            }
            Value::String(value) => {
                values.push_bind(value);
            }
            Value::Number(value) if value.is_i64() => {
                values.push_bind(value.as_i64().unwrap());
            }
            Value::Number(value) => {
                values.push_bind(value.as_f64().ok_or("Invalid archive number")?);
            }
            _ => return Err("Invalid archive field type.".into()),
        }
    }
    query
        .push(")")
        .build()
        .execute(connection)
        .await
        .map_err(failure)?;
    Ok(())
}

/// Compare the archived source against the destination inside the import transaction.
/// The temporary index holds only identities and comparison results, even for v3.
async fn same_archived_source(
    connection: &mut sqlx::SqliteConnection,
    meeting: &Map<String, Value>,
    id: &str,
) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM meetings WHERE id=?1 AND title IS ?2 AND created_at IS ?3)
         AND (SELECT count(*) FROM transcripts WHERE meeting_id=?1) =
             (SELECT count(*) FROM temp.archive_transcript_comparisons WHERE meeting_id=?1)
         AND NOT EXISTS(SELECT 1 FROM temp.archive_transcript_comparisons
                        WHERE meeting_id=?1 AND matches=0)",
    )
    .bind(id)
    .bind(meeting.get("title").and_then(Value::as_str))
    .bind(meeting.get("created_at").and_then(Value::as_str))
    .fetch_one(connection)
    .await
    .map_err(failure)
}

async fn compare_archived_transcript(
    connection: &mut sqlx::SqliteConnection,
    meeting: &str,
    row: &Map<String, Value>,
) -> Result<(), String> {
    let text = |key| match row.get(key) {
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        None | Some(Value::Null) => Ok(None),
        _ => Err("Invalid transcript text field."),
    };
    let time = |key| match row.get(key) {
        Some(Value::Number(value)) => value.as_f64().map(Some).ok_or("Invalid transcript time."),
        None | Some(Value::Null) => Ok(None),
        _ => Err("Invalid transcript time."),
    };
    sqlx::query(
        "INSERT INTO temp.archive_transcript_comparisons (id, meeting_id, matches)
         VALUES (?1, ?2, EXISTS(SELECT 1 FROM transcripts WHERE id=?1 AND meeting_id=?2
             AND transcript IS ?3 AND speaker IS ?4 AND timestamp IS ?5
             AND audio_start_time IS ?6 AND audio_end_time IS ?7 AND duration IS ?8
             AND word_timestamps_json IS ?9))",
    )
    .bind(text_field(row, "id")?)
    .bind(meeting)
    .bind(text_field(row, "transcript")?)
    .bind(text("speaker")?)
    .bind(text_field(row, "timestamp")?)
    .bind(time("audio_start_time")?)
    .bind(time("audio_end_time")?)
    .bind(time("duration")?)
    .bind(text("word_timestamps_json")?)
    .execute(connection)
    .await
    .map_err(failure)?;
    Ok(())
}

async fn import_conversations(
    connection: &mut sqlx::SqliteConnection,
    manifest: &Manifest,
    known: &HashSet<String>,
    selected: &HashSet<String>,
    documents: &BTreeMap<String, String>,
) -> Result<(), String> {
    use crate::knowledge::{conversations, types::*};
    let rows = |table: &str| manifest.tables.get(table).map(Vec::as_slice).unwrap_or(&[]);
    let canonical_meetings: BTreeMap<_, _> = rows("meetings")
        .iter()
        .filter_map(|row| row.get("id").and_then(Value::as_str).map(|id| (id, row)))
        .collect();
    // Validate even skipped rows. Table-specific identities replace the generic
    // meeting_id assumption used by the older meeting-only child tables.
    for table in TABLES
        .iter()
        .filter(|table| table.starts_with("knowledge_"))
    {
        let columns: HashSet<String> = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(&mut *connection)
            .await
            .map_err(failure)?
            .iter()
            .map(|row| row.get("name"))
            .collect();
        for row in rows(table) {
            if row.keys().any(|key| !columns.contains(key))
                || row.values().any(|value| {
                    !matches!(value, Value::Null | Value::String(_) | Value::Number(_))
                })
            {
                return Err("Unsupported conversation archive columns or types.".into());
            }
        }
    }
    let mut owners = BTreeMap::new();
    for row in rows("knowledge_owners") {
        let id = text_field(row, "id")?.to_string();
        let owner = match text_field(row, "kind")? {
            "meeting" => {
                let meeting = text_field(row, "meeting_id")?;
                if !known.contains(meeting) || id != format!("meeting:{meeting}") {
                    return Err("Invalid conversation meeting owner.".into());
                }
                ConversationOwner::Meeting(meeting.into())
            }
            "library" => {
                let value = id
                    .strip_prefix("library:")
                    .ok_or("Invalid library conversation owner")?;
                if uuid::Uuid::parse_str(value)
                    .map(|id| id.to_string())
                    .ok()
                    .as_deref()
                    != Some(value)
                    || row.get("meeting_id").is_some_and(|v| !v.is_null())
                {
                    return Err("Invalid library conversation owner.".into());
                }
                ConversationOwner::Library(value.into())
            }
            _ => return Err("Unsupported conversation owner.".into()),
        };
        let existing: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_owners WHERE id=?)")
                .bind(&id)
                .fetch_one(&mut *connection)
                .await
                .map_err(failure)?;
        if owners.insert(id, (owner, existing)).is_some() {
            return Err("Duplicate conversation owner.".into());
        }
        if !existing {
            insert_archive_row(connection, "knowledge_owners", row).await?;
        }
    }
    // request -> (skip due to collision, invalidated, original status, question)
    let mut requests = BTreeMap::<String, (bool, bool, String, String)>::new();
    let mut frozen_by_request = BTreeMap::<String, Vec<String>>::new();
    let mut documents_by_request = BTreeMap::<String, Vec<String>>::new();
    for row in rows("knowledge_requests") {
        let id = uuid_field(row, "id")?;
        let owner_id = text_field(row, "owner_id")?;
        let (owner, owner_collision) = owners.get(owner_id).ok_or("Unknown conversation owner")?;
        let status = text_field(row, "status")?;
        if !matches!(
            status,
            "preparing"
                | "running"
                | "completed"
                | "failed"
                | "interrupted"
                | "cancelled"
                | "invalidated"
        ) {
            return Err("Invalid saved answer state.".into());
        }
        let question = text_field(row, "question")?;
        let provider = text_field(row, "provider")?;
        let model = text_field(row, "model")?;
        use crate::summary::llm_client::{canonical_provider, LLMProvider};
        // The historical provider is part of the saved request fingerprint.
        // Only the archive reader accepts it.
        let historical_provider = provider == "openclaw";
        let canonical = historical_provider
            || LLMProvider::from_str(provider)
                .is_ok_and(|parsed| canonical_provider(&parsed) == provider);
        if !canonical || model.trim().is_empty() {
            return Err("Invalid saved provider metadata.".into());
        }
        let fingerprint = text_field(row, "input_fingerprint")?;
        if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid saved request fingerprint.".into());
        }
        let frozen: Vec<String> =
            serde_json::from_str(text_field(row, "frozen_ids_json")?).map_err(failure)?;
        if frozen.iter().collect::<HashSet<_>>().len() != frozen.len() {
            return Err("Duplicate frozen source identity.".into());
        }
        if status != "invalidated" {
            let input: AskRequest =
                serde_json::from_str(text_field(row, "input_json")?).map_err(failure)?;
            conversations::validate_request(&input)?;
            let document_ids = &input.search.document_ids;
            if document_ids.iter().collect::<HashSet<_>>().len() != document_ids.len()
                || document_ids.iter().any(|id| !documents.contains_key(id))
            {
                return Err("Unknown or duplicate saved document selection.".into());
            }
            documents_by_request.insert(id.clone(), document_ids.clone());
            let scope: KnowledgeScope =
                serde_json::from_str(text_field(row, "scope_json")?).map_err(failure)?;
            if input.request_id != id
                || &input.owner != owner
                || input.search.query != question
                || input.search.scope != scope
                || conversations::input_fingerprint(&input, provider, model)? != fingerprint
            {
                return Err("Saved request inputs do not match their immutable identity.".into());
            }
        }
        let collision: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_requests WHERE id=?)")
                .bind(&id)
                .fetch_one(&mut *connection)
                .await
                .map_err(failure)?;
        if requests
            .insert(
                id.clone(),
                (
                    *owner_collision || collision,
                    status == "invalidated",
                    status.into(),
                    question.into(),
                ),
            )
            .is_some()
        {
            return Err("Duplicate saved request UUID.".into());
        }
        frozen_by_request.insert(id, frozen);
    }
    let mut dependencies = BTreeMap::<String, BTreeMap<String, i64>>::new();
    let mut comparable = BTreeMap::new();
    for row in rows("knowledge_request_sources") {
        let id = text_field(row, "request_id")?;
        let request = requests.get_mut(id).ok_or("Unknown source request")?;
        let source = text_field(row, "source_id")?;
        let revision = row
            .get("revision")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0)
            .ok_or("Invalid source revision")?;
        if dependencies
            .entry(id.into())
            .or_default()
            .insert(source.into(), revision)
            .is_some()
        {
            return Err("Invalid or duplicate conversation dependency.".into());
        }
        if let Some(document) = source.strip_prefix("document:") {
            if !documents.contains_key(document)
                || documents_by_request
                    .get(id)
                    .is_none_or(|ids| !ids.iter().any(|id| id == document))
            {
                return Err("Invalid conversation document dependency.".into());
            }
            continue;
        }
        let meeting = source
            .strip_prefix("meeting:")
            .filter(|id| !id.is_empty())
            .ok_or("Invalid conversation source")?;
        if !frozen_by_request[id].iter().any(|id| id == meeting) {
            return Err("Invalid conversation meeting dependency.".into());
        }
        let same = if let Some(same) = comparable.get(meeting) {
            *same
        } else {
            let same = selected.contains(meeting)
                || (known.contains(meeting)
                    && same_archived_source(connection, canonical_meetings[meeting], meeting)
                        .await?);
            comparable.insert(meeting.to_string(), same);
            same
        };
        request.1 |= !same;
    }
    for (id, request) in &mut requests {
        // The selected scope is an absence/deletion dependency even before
        // retrieval finishes; a stripped dependency cannot resurrect a turn.
        for meeting in &frozen_by_request[id] {
            if dependencies
                .get(id)
                .is_none_or(|sources| !sources.contains_key(&format!("meeting:{meeting}")))
            {
                request.1 = true;
            }
        }
        for document in documents_by_request.get(id).into_iter().flatten() {
            if dependencies
                .get(id)
                .is_none_or(|sources| !sources.contains_key(&format!("document:{document}")))
            {
                request.1 = true;
            }
        }
    }
    let mut ordinals = BTreeMap::<String, Vec<i64>>::new();
    let mut context_maps = BTreeMap::<String, BTreeMap<i64, (EvidenceRef, EvidenceDisplay)>>::new();
    for row in rows("knowledge_request_evidence") {
        let id = text_field(row, "request_id")?;
        requests.get(id).ok_or("Unknown evidence request")?;
        let ordinal = row
            .get("ordinal")
            .and_then(Value::as_i64)
            .filter(|n| (1..=crate::knowledge::evidence::MAX_EVIDENCE_ENTRIES as i64).contains(n))
            .ok_or("Invalid citation ordinal")?;
        ordinals.entry(id.into()).or_default().push(ordinal);
        let reference: EvidenceRef =
            serde_json::from_str(text_field(row, "reference_json")?).map_err(failure)?;
        let display: EvidenceDisplay =
            serde_json::from_str(text_field(row, "display_json")?).map_err(failure)?;
        if display.title.len() > 1024
            || display.date.len() > 1024
            || display.speaker.as_ref().is_some_and(|v| v.len() > 1024)
            || dependencies
                .get(id)
                .and_then(|sources| sources.get(&reference.source_id))
                != Some(&reference.source_revision)
        {
            return Err("Invalid saved evidence snapshot.".into());
        }
        context_maps
            .entry(id.into())
            .or_default()
            .insert(ordinal, (reference.clone(), display));
        match &reference.locator {
            EvidenceLocator::Transcript {
                meeting_id,
                transcript_ids,
                spans,
                start_seconds,
            } => {
                if reference.source_id != format!("meeting:{meeting_id}")
                    || transcript_ids.len() != 1
                    || spans.len() != 1
                    || transcript_ids[0] != spans[0].transcript_id
                    || spans[0].end_byte <= spans[0].start_byte
                    || spans[0].end_byte - spans[0].start_byte > crate::knowledge::store::READ_BYTES
                    || start_seconds.is_some_and(|n| !n.is_finite() || n < 0.)
                {
                    return Err("Invalid archived canonical locator.".into());
                }
            }
            EvidenceLocator::Document {
                document_id,
                page,
                paragraph,
                spans,
            } => {
                if reference.source_id != format!("document:{document_id}")
                    || !documents.contains_key(document_id)
                    || *paragraph == 0
                    || page.is_some_and(|n| {
                        n == 0 || n as usize > crate::knowledge::documents::PAGE_LIMIT
                    })
                    || spans.len() > 1
                    || spans.iter().any(|span| {
                        let ordinal = span
                            .transcript_id
                            .strip_prefix(&format!("{document_id}:"))
                            .and_then(|value| value.parse::<u64>().ok())
                            .filter(|n| *n > 0);
                        ordinal.is_none()
                            || span.end_byte <= span.start_byte
                            || span.end_byte - span.start_byte > crate::knowledge::store::READ_BYTES
                    })
                {
                    return Err("Invalid archived document locator.".into());
                }
            }
            _ => return Err("Unsupported archived evidence locator.".into()),
        }
    }
    for values in ordinals.values_mut() {
        values.sort_unstable();
        if values
            .iter()
            .enumerate()
            .any(|(index, value)| *value != index as i64 + 1)
        {
            return Err("Non-contiguous citation map.".into());
        }
    }
    for map in context_maps.values() {
        let references = map
            .values()
            .map(|(reference, _)| reference.clone())
            .collect::<Vec<_>>();
        let metadata = map
            .values()
            .map(|(_, display)| display.clone())
            .collect::<Vec<_>>();
        crate::knowledge::evidence::validate_context_metadata(&references, &metadata)?;
    }
    let mut roles = BTreeMap::<String, HashSet<String>>::new();
    let mut message_ids = HashSet::new();
    for row in rows("knowledge_messages") {
        let message = uuid_field(row, "id")?;
        let id = text_field(row, "request_id")?;
        let request = requests.get(id).ok_or("Unknown message request")?;
        let role = text_field(row, "role")?;
        let content = text_field(row, "content")?;
        if !matches!(role, "user" | "assistant")
            || content.len() > 256 * 1024
            || (role == "user" && !request.1 && content != request.3)
            || !message_ids.insert(message.clone())
            || !roles.entry(id.into()).or_default().insert(role.into())
        {
            return Err("Invalid or duplicate conversation message.".into());
        }
        if !request.0 {
            let collision: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_messages WHERE id=?)")
                    .bind(&message)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(failure)?;
            if collision {
                return Err("Conversation message UUID collision.".into());
            }
        }
    }
    for (id, request) in &requests {
        let role = roles.get(id).ok_or("Saved request has no user turn")?;
        if !role.contains("user")
            || (request.2 == "completed" && !role.contains("assistant"))
            || (!matches!(request.2.as_str(), "completed" | "invalidated")
                && role.contains("assistant"))
        {
            return Err("Saved answer message state is inconsistent.".into());
        }
    }
    // Insert only after all associations were checked. Invalidated replies use
    // a temporary completed state inside this transaction to import blank
    // tombstones without weakening the normal assistant insertion trigger.
    for source in rows("knowledge_requests") {
        let id = text_field(source, "id")?;
        let request = &requests[id];
        if request.0 {
            continue;
        }
        let mut row = source.clone();
        row.insert("restored".into(), Value::from(1));
        if !request.1 {
            let mut input: AskRequest =
                serde_json::from_str(text_field(source, "input_json")?).map_err(failure)?;
            let mut remapped = false;
            for id in &mut input.search.document_ids {
                let restored = &documents[id.as_str()];
                remapped |= id.as_str() != restored.as_str();
                *id = restored.clone();
            }
            if remapped {
                row.insert(
                    "input_json".into(),
                    serde_json::to_string(&input).map_err(failure)?.into(),
                );
                row.insert(
                    "input_fingerprint".into(),
                    conversations::input_fingerprint(
                        &input,
                        text_field(source, "provider")?,
                        text_field(source, "model")?,
                    )?
                    .into(),
                );
            }
        }
        if request.1 {
            row.insert("status".into(), Value::from("completed"));
            row.insert("question".into(), Value::from(""));
            row.insert("scope_json".into(), Value::from("{}"));
            row.insert("input_json".into(), Value::from("{}"));
            row.insert("frozen_ids_json".into(), Value::from("[]"));
            row.insert("failure".into(), Value::from("restored_source_unavailable"));
        } else if matches!(request.2.as_str(), "preparing" | "running") {
            row.insert("status".into(), Value::from("interrupted"));
            row.insert("failure".into(), Value::from("restored_interrupted"));
        }
        insert_archive_row(connection, "knowledge_requests", &row).await?;
    }
    for table in [
        "knowledge_messages",
        "knowledge_request_evidence",
        "knowledge_request_sources",
    ] {
        for source in rows(table) {
            let request = &requests[text_field(source, "request_id")?];
            if request.0 || (request.1 && table != "knowledge_messages") {
                continue;
            }
            let mut row = source.clone();
            if request.1 {
                row.insert("content".into(), Value::from(""));
            }
            if table == "knowledge_request_evidence" {
                let mut reference: EvidenceRef =
                    serde_json::from_str(text_field(&row, "reference_json")?).map_err(failure)?;
                reference.historical = true;
                if let EvidenceLocator::Document {
                    document_id, spans, ..
                } = &mut reference.locator
                {
                    let restored = &documents[document_id.as_str()];
                    for span in spans {
                        let ordinal = span
                            .transcript_id
                            .strip_prefix(&format!("{document_id}:"))
                            .ok_or("Invalid archived document span.")?;
                        span.transcript_id = format!("{restored}:{ordinal}");
                    }
                    *document_id = restored.clone();
                    reference.source_id = format!("document:{restored}");
                }
                row.insert(
                    "reference_json".into(),
                    Value::from(serde_json::to_string(&reference).map_err(failure)?),
                );
            }
            if table == "knowledge_request_sources" {
                if let Some(document) = text_field(&row, "source_id")?.strip_prefix("document:") {
                    let restored = documents
                        .get(document)
                        .ok_or("Unknown restored document source.")?;
                    row.insert("source_id".into(), format!("document:{restored}").into());
                }
            }
            insert_archive_row(connection, table, &row).await?;
        }
    }
    for (id, request) in requests {
        if !request.0 && request.1 {
            sqlx::query("UPDATE knowledge_requests SET status='invalidated' WHERE id=?")
                .bind(id)
                .execute(&mut *connection)
                .await
                .map_err(failure)?;
        }
    }
    Ok(())
}

#[cfg(test)]
async fn import_manifest(
    pool: &SqlitePool,
    manifest: Manifest,
    stage: tempfile::TempDir,
) -> Result<BackupReport, String> {
    import_manifest_impl(pool, manifest, stage, None, None).await
}

#[cfg(test)]
async fn import_manifest_impl(
    pool: &SqlitePool,
    manifest: Manifest,
    stage: tempfile::TempDir,
    reference_root: Option<&Path>,
    app_data: Option<&Path>,
) -> Result<BackupReport, String> {
    import_manifest_and_repair(pool, manifest, stage, reference_root, app_data, None).await
}

async fn import_manifest_and_repair(
    pool: &SqlitePool,
    mut manifest: Manifest,
    stage: tempfile::TempDir,
    reference_root: Option<&Path>,
    app_data: Option<&Path>,
    repair: Option<repair::Plan>,
) -> Result<BackupReport, String> {
    let documents = archive_documents::validate(&manifest)?;
    archive_exports::validate(&manifest)?;
    manifest.portable.validate(&manifest.tables["meetings"])?;
    manifest.follow_through.validate(&manifest, None)?;
    // An in-place repair may publish files without updating a database row.
    // Reserve the write transaction before checking the saved folder again.
    let mut tx = if repair.as_ref().is_some_and(repair::Plan::has_targets) {
        pool.begin_with("BEGIN IMMEDIATE").await
    } else {
        pool.begin().await
    }
    .map_err(failure)?;
    let mut selected = HashSet::new();
    let mut known = HashSet::new();
    let mut skipped = 0;
    let mut unreadable_calendar_links = 0;
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
    let comparisons: HashSet<&str> = manifest
        .tables
        .get("knowledge_request_sources")
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("source_id")?.as_str()?.strip_prefix("meeting:"))
        .filter(|id| known.contains(*id) && !selected.contains(*id))
        .collect();
    if !comparisons.is_empty() {
        // Desktop SQLite's temp database is file-backed. A unique transcript ID
        // keeps a repeated archive row from passing the count check for a missing one.
        sqlx::query(
            "CREATE TEMP TABLE archive_transcript_comparisons (
                 id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, matches INTEGER NOT NULL
             ) WITHOUT ROWID",
        )
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
        sqlx::query(
            "CREATE INDEX temp.archive_transcript_comparison_meeting
             ON archive_transcript_comparisons (meeting_id, matches)",
        )
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    }
    // Meetings restored without a recording folder (the archive carried no
    // files for them). No transcript or metadata file can ever be written for
    // these, so pending file work from the source computer is inapplicable.
    let folderless: HashMap<String, usize> = manifest.tables["meetings"]
        .iter()
        .enumerate()
        .filter(|(index, _)| !manifest.files.contains_key(index))
        .filter_map(|(index, row)| {
            let id = row.get("id")?.as_str()?;
            selected.contains(id).then(|| (id.to_string(), index))
        })
        .collect();
    let mut dropped_file_work = HashSet::new();
    let mut summary_content_revisions = Vec::new();
    for table in TABLES {
        if table.starts_with("knowledge_") {
            continue;
        }
        let schema = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(&mut *tx)
            .await
            .map_err(failure)?;
        let columns: HashSet<String> = schema.iter().map(|row| row.get("name")).collect();
        // Older archives may predate optional tables. Current transcript tables
        // are read one row at a time from verified ZIP entries in the stage.
        let mut inline = manifest.tables.get(*table).into_iter().flatten();
        let mut stream = match manifest.table_records.get(*table) {
            Some(count) => Some(records::Reader::open(stage.path(), table, *count).await?),
            None => None,
        };
        let mut indices = 0usize..;
        while let Some(source) = match &mut stream {
            Some(stream) => stream.next().await?,
            None => inline.next().cloned(),
        } {
            let index = indices.next().unwrap();
            if source.keys().any(|key| !columns.contains(key)) {
                return Err("Archive contains unsupported data columns.".into());
            }
            if stream.is_some()
                && source.values().any(|value| {
                    !matches!(value, Value::Null | Value::String(_) | Value::Number(_))
                })
            {
                return Err("Invalid archive field type.".into());
            }
            if *table == "meetings" {
                validate_reviewed_actions(&source)?;
            }
            let parent = source
                .get(if *table == "meetings" {
                    "id"
                } else {
                    "meeting_id"
                })
                .and_then(Value::as_str)
                .ok_or("Invalid meeting reference.")?
                .to_owned();
            if !known.contains(&parent) {
                return Err("Archive contains an unknown meeting reference.".into());
            }
            if !selected.contains(&parent) {
                if *table == "transcripts" && comparisons.contains(parent.as_str()) {
                    compare_archived_transcript(&mut tx, &parent, &source).await?;
                }
                continue;
            }
            let folderless_meeting = folderless.get(&parent).copied();
            if let Some(meeting_index) = folderless_meeting {
                if *table == "transcript_file_sync" {
                    // A retry marker here could never finish and would fail forever.
                    dropped_file_work.insert(meeting_index);
                    continue;
                }
            }
            let mut row = source;
            if let Some(meeting_index) = folderless_meeting {
                if *table == "recording_outcomes"
                    && row.get("recording_files_incomplete").is_some_and(|value| {
                        value.as_i64() == Some(1) || value.as_bool() == Some(true)
                    })
                {
                    // Only the file-retry flag is inapplicable. Capture gaps and the
                    // other recorded outcomes are restored exactly as archived.
                    row.insert("recording_files_incomplete".into(), Value::from(0));
                    dropped_file_work.insert(meeting_index);
                }
            }
            if *table == "meetings" {
                if manifest.removed_audio.contains(&parent)
                    && columns.contains("saved_audio_removed_by_user")
                {
                    row.insert("saved_audio_removed_by_user".into(), Value::from(1));
                }
                if let Some(revision) = row.remove("summary_content_revision") {
                    let revision = revision
                        .as_i64()
                        .ok_or("Invalid summary content revision.")?;
                    summary_content_revisions.push((parent, revision));
                }
                // One unreadable link must not block the restore. The meeting is
                // kept and only its link is cleared; the title then stays local.
                let readable = match row.get("calendar_link") {
                    Some(Value::String(link)) => super::calendar::decode(link).is_ok(),
                    None | Some(Value::Null) => true,
                    _ => false,
                };
                if !readable {
                    row.insert("calendar_link".into(), Value::String("null".into()));
                    unreadable_calendar_links += 1;
                }
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
                row.remove("recording_id");
            }
            if *table == "summary_processes" {
                // A matching destination on this profile does not authorize a restored job.
                row.insert("automatic_request_fingerprint".into(), Value::Null);
                if row
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| {
                        !matches!(
                            status.to_ascii_lowercase().as_str(),
                            "completed" | "failed" | "cancelled" | "skipped"
                        )
                    })
                {
                    row.insert("status".into(), Value::String("cancelled".into()));
                }
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
            query.push(")");
            if *table == "meeting_summary_bookmarks" {
                // The meeting insert can already have transferred local consent
                // from the same interrupted recording.
                query.push(" ON CONFLICT (meeting_id) DO NOTHING");
            }
            query.build().execute(&mut *tx).await.map_err(failure)?;
        }
    }
    // Replaying transcripts fires the revision triggers. Reinstate the saved
    // counter so both current and stale partials retain their original state.
    // Archives without this column keep the revision built by the triggers.
    for (meeting_id, revision) in summary_content_revisions {
        sqlx::query("UPDATE meetings SET summary_content_revision = ? WHERE id = ?")
            .bind(revision)
            .bind(meeting_id)
            .execute(&mut *tx)
            .await
            .map_err(failure)?;
    }
    manifest
        .portable
        .restore_series(&mut tx, &manifest.tables["meetings"], &selected)
        .await?;
    let omitted_saved_live_answers = manifest
        .follow_through
        .restore(&mut tx, &manifest.tables["meetings"], &selected)
        .await?;
    let document_restore = archive_documents::restore_rows(
        &mut tx,
        &manifest,
        &documents,
        &selected,
        stage.path(),
        reference_root,
    )
    .await?;
    import_conversations(
        &mut tx,
        &manifest,
        &known,
        &selected,
        &document_restore.identities,
    )
    .await?;
    if !comparisons.is_empty() {
        sqlx::query("DROP TABLE temp.archive_transcript_comparisons")
            .execute(&mut *tx)
            .await
            .map_err(failure)?;
    }
    for (index, incomplete) in &manifest.incomplete_audio {
        if !incomplete.recovery_files_excluded {
            continue;
        }
        let meeting_id = manifest.tables["meetings"]
            .get(*index)
            .and_then(|row| row.get("id"))
            .and_then(Value::as_str)
            .ok_or("Invalid incomplete meeting reference.")?;
        if selected.contains(meeting_id) {
            // Preserve archived outcome flags, including when this column was absent.
            sqlx::query("INSERT INTO recording_outcomes (meeting_id, recovery_files_elsewhere) VALUES (?, 1)
                         ON CONFLICT(meeting_id) DO UPDATE SET recovery_files_elsewhere = 1")
                .bind(meeting_id).execute(&mut *tx).await.map_err(failure)?;
        }
    }
    // Missing audio is reported from what the archive lacked, independently of
    // the retry flag that was dropped above.
    for index in dropped_file_work {
        manifest
            .incomplete_audio
            .entry(index)
            .or_default()
            .audio_unavailable = true;
    }
    let unavailable_audio_locations = repair.as_ref().map_or(0, |plan| plan.unavailable);
    let repaired = match repair {
        Some(plan) => plan.apply(&mut tx, stage.path(), &manifest).await?,
        None => repair::RepairedAudio::default(),
    };
    // Delete skipped meetings' extracted files before committing any rows or
    // keeping the stage. Cleanup failure rolls back the complete import.
    let unused: Vec<usize> = manifest
        .files
        .keys()
        .copied()
        .filter(|index| {
            !repaired.staged.contains(index)
                && !manifest.tables["meetings"][*index]
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
    let ledgers = archive_exports::selected(&manifest, &selected);
    let portable = std::mem::take(&mut manifest.portable);
    let has_records = !manifest.table_records.is_empty();
    let app_data = app_data.map(Path::to_path_buf);
    let settings = PORTABLE_SETTINGS_LOCK.lock().await;
    let (originals, history, portable, stage, settings) = tokio::task::spawn_blocking(move || {
        let history = archive_exports::publish(ledgers, app_data.as_deref())?;
        let originals = document_restore.publish()?;
        let portable = portable.publish(app_data.as_deref())?;
        if has_records {
            std::fs::remove_dir_all(stage.path().join(records::STAGING)).map_err(failure)?;
        }
        // Only recording folders survive a successful restore. The originals
        // have their own app-owned publication guard and need no staging copy.
        match std::fs::remove_dir_all(stage.path().join("reference-documents")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(failure(error)),
        }
        Ok((originals, history, portable, stage, settings))
    })
    .await
    .map_err(failure)??;
    let document_files = originals.count();
    let skipped_template_items = portable.skipped_templates;
    let repaired_audio = repaired.meetings;
    let repaired_files = repaired.files;
    // Once commit starts, a detached task owns both the files and transaction.
    // Dropping an invoking future cannot remove originals after a successful commit.
    tokio::spawn(async move {
        let _settings = settings;
        tx.commit().await.map_err(failure)?;
        originals.keep();
        history.keep();
        portable.keep();
        if folders > 0 || !repaired.staged.is_empty() {
            let _ = stage.keep();
        }
        repaired.keep();
        Ok::<_, String>(())
    })
    .await
    .map_err(failure)??;
    Ok(BackupReport {
        meetings: selected.len(),
        skipped: skipped - repaired_audio,
        repaired_audio,
        unavailable_audio_locations,
        files: files + document_files + repaired_files,
        audio_excluded: manifest.audio_excluded,
        unreadable_calendar_links,
        skipped_template_items,
        incomplete_meetings: incomplete_meetings(&manifest, Some(&selected)),
        omitted_saved_live_answers,
    })
}

#[cfg(test)]
async fn snapshot_with_documents(
    pool: &SqlitePool,
) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    snapshot_impl(pool, true).await
}

#[cfg(test)]
async fn snapshot_current(
    pool: &SqlitePool,
    records_root: &Path,
) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    let files = records::SnapshotFiles::Named(records_root.to_owned());
    snapshot_for_backup(pool, &files, None).await
}

async fn snapshot_for_backup(
    pool: &SqlitePool,
    files: &records::SnapshotFiles,
    scheduled: Option<&ScheduledBackup>,
) -> Result<(Manifest, Vec<Option<PathBuf>>), String> {
    backup_checkpoint(scheduled)?;
    super::attach_bookmarks(pool).await.map_err(failure)?;
    snapshot_tables_impl(pool, TABLES, true, Some(files), scheduled).await
}

#[cfg(test)]
fn write_archive_with_documents(
    path: &Path,
    manifest: Manifest,
    folders: Vec<Option<PathBuf>>,
    reference_root: &Path,
) -> Result<BackupReport, String> {
    write_archive_impl(path, manifest, folders, Some(reference_root), None, false)
}

#[cfg(test)]
async fn import_manifest_with_documents(
    pool: &SqlitePool,
    manifest: Manifest,
    stage: tempfile::TempDir,
    reference_root: &Path,
) -> Result<BackupReport, String> {
    import_manifest_impl(pool, manifest, stage, Some(reference_root), None).await
}

#[cfg(test)]
#[path = "backup/document_tests.rs"]
mod document_tests;

#[cfg(test)]
#[path = "backup/follow_through_tests.rs"]
mod follow_through_tests;

#[tauri::command]
pub async fn backup_library(
    app: AppHandle,
    path: String,
    passphrase: Option<String>,
) -> Result<BackupReport, String> {
    let passphrase = passphrase.map(Zeroizing::new);
    if let Some(secret) = &passphrase {
        encryption::validate_passphrase(secret)?;
    }
    // Only a path the user chose in the save dialog the core opened
    // (`pick_backup_save_path`), once.
    let granted = crate::file_grants::take(crate::file_grants::Purpose::BackupSave, &path)?;
    let result = match claim_archive() {
        Ok(permits) => {
            backup_with_permits(app, granted.path.clone(), permits, None, false, passphrase).await
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        crate::file_grants::give_back(crate::file_grants::Purpose::BackupSave, &granted);
    }
    result
}

enum ArchiveAudio {
    Foreground {
        _permit: tokio::sync::OwnedSemaphorePermit,
    },
    Scheduled {
        _permit: crate::audio::inference::BatchPermit,
    },
}

struct ArchivePermits {
    audio: Option<ArchiveAudio>,
    _archive: tokio::sync::MutexGuard<'static, ()>,
    _export: tokio::sync::MutexGuard<'static, ()>,
}

impl ArchivePermits {
    fn finish_reading_for_schedule(&mut self, control: &ScheduledBackup) -> Result<(), String> {
        // The completed ZIP contains everything it needs. Only capture can
        // resume here: restore and exports are locked through publication.
        control.seal()?;
        drop(self.audio.take());
        Ok(())
    }
}

fn claim_archive() -> Result<ArchivePermits, String> {
    claim_archive_with(|| {
        let permit = crate::audio::inference::claim_job()?;
        Ok(ArchiveAudio::Foreground { _permit: permit })
    })
}

pub(super) fn scheduled_backup_should_yield() -> bool {
    // A local summary releases its inference permit between model requests.
    crate::summary::SummaryService::has_active_local_summary()
        || crate::knowledge::scheduler::foreground_waiting()
}

fn claim_scheduled_archive(control: &Arc<ScheduledBackup>) -> Result<ArchivePermits, String> {
    let control = control.clone();
    claim_archive_with(|| {
        use crate::audio::inference::{claim_batch_job_without_preemption, BatchJob};
        if scheduled_backup_should_yield() {
            return Err(BACKUP_DEFERRED.into());
        }
        let permit = claim_batch_job_without_preemption(
            BatchJob::ScheduledBackup,
            Arc::new(move || control.defer()),
        )?;
        Ok(ArchiveAudio::Scheduled { _permit: permit })
    })
}

fn claim_archive_with(
    claim_audio: impl FnOnce() -> Result<ArchiveAudio, String>,
) -> Result<ArchivePermits, String> {
    let archive = ARCHIVE_JOB
        .try_lock()
        .map_err(|_| "Another archive operation is running.")?;
    let export = crate::exports::commands::EXPORT_LOCK
        .try_lock()
        .map_err(|_| "Finish the Microsoft export before working with an archive.")?;
    Ok(ArchivePermits {
        audio: Some(claim_audio()?),
        _archive: archive,
        _export: export,
    })
}

pub(super) async fn try_scheduled_backup(
    app: AppHandle,
    path: PathBuf,
    output: tempfile::NamedTempFile,
    control: Arc<ScheduledBackup>,
    exclude_audio: bool,
) -> Result<Option<BackupReport>, String> {
    let Ok(permits) = claim_scheduled_archive(&control) else {
        return Ok(None);
    };
    let result = backup_with_permits(
        app,
        path,
        permits,
        Some((output, control.clone())),
        exclude_audio,
        None,
    )
    .await;
    match result {
        Err(_) if control.checkpoint().is_err() => Ok(None),
        result => result.map(Some),
    }
}

async fn backup_with_permits(
    app: AppHandle,
    path: PathBuf,
    permits: ArchivePermits,
    scheduled_output: Option<(tempfile::NamedTempFile, Arc<ScheduledBackup>)>,
    exclude_audio: bool,
    passphrase: Option<Zeroizing<String>>,
) -> Result<BackupReport, String> {
    let extension = if passphrase.is_some() {
        encryption::EXTENSION
    } else {
        "zip"
    };
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
    {
        return Err(format!("Choose a .{extension} backup file."));
    }
    if scheduled_output.is_none() {
        crate::file_grants::save_target_parent_exists(&path)?;
    }
    let app_data = app.path().app_data_dir().map_err(failure)?;
    let scratch = passphrase
        .as_ref()
        .map(|_| app.path().app_local_data_dir())
        .transpose()
        .map_err(failure)?;
    let reference_root = app_data.join("reference-documents");
    let stage_root = app_data.clone();
    let records =
        tokio::task::spawn_blocking(move || records::SnapshotFiles::temporary(&stage_root))
            .await
            .map_err(failure)??;
    let (output, control) = match scheduled_output {
        Some((output, control)) => (Some(output), Some(control)),
        None => (None, None),
    };
    let state = app
        .try_state::<AppState>()
        .ok_or("Open the meeting library first.")?;
    let (mut manifest, folders) =
        snapshot_for_backup(state.db_manager.pool(), &records, control.as_deref()).await?;
    tokio::task::spawn_blocking(move || {
        let mut permits = permits;
        backup_checkpoint(control.as_deref())?;
        archive_exports::capture_for_backup(&mut manifest, &app_data, control.as_deref())?;
        backup_checkpoint(control.as_deref())?;
        let skipped_template_items = manifest.portable.capture_files(&app_data)?;
        let mut output = match output {
            Some(output) => output,
            None => archive_tempfile(&path)?,
        };
        let mut prepared = if let (Some(local_data), Some(passphrase)) = (scratch, passphrase) {
            let scratch = encryption::prepare_scratch(&local_data)?;
            let estimate = estimate_zip_size(&manifest, &folders, &records)?;
            let limit = encryption::backup_scratch_limit(&scratch, &path, estimate)?;
            let mut plain_zip = encryption::scratch_file(&scratch)?;
            let report = build_archive(
                &mut plain_zip,
                manifest,
                folders,
                Some(&reference_root),
                Some(&records),
                None,
                false,
                Some(limit),
            )?;
            encryption::seal(plain_zip, output.as_file_mut(), passphrase)?;
            PreparedBackup { output, report }
        } else {
            prepare_archive(
                output,
                manifest,
                folders,
                Some(&reference_root),
                Some(&records),
                control.as_deref(),
                exclude_audio,
            )?
        };
        prepared.report.skipped_template_items = skipped_template_items;
        drop(records);
        if let Some(control) = &control {
            permits.finish_reading_for_schedule(control)?;
        }
        prepared.publish(&path, control.is_some())
    })
    .await
    .map_err(failure)?
}

#[tauri::command]
pub async fn restore_library(
    app: AppHandle,
    path: String,
    passphrase: Option<String>,
    repair_missing_audio: Option<bool>,
) -> Result<BackupReport, String> {
    let passphrase = passphrase.map(Zeroizing::new);
    // Only an archive the user chose in the file dialog the core opened
    // (`pick_backup_restore_path`), once.
    let granted = crate::file_grants::take(crate::file_grants::Purpose::BackupOpen, &path)?;
    let result = restore_library_from(
        app,
        granted.path.clone(),
        passphrase,
        repair_missing_audio.unwrap_or(false),
    )
    .await;
    if result.is_err() {
        crate::file_grants::give_back(crate::file_grants::Purpose::BackupOpen, &granted);
    }
    result
}

async fn restore_library_from(
    app: AppHandle,
    path: PathBuf,
    passphrase: Option<Zeroizing<String>>,
    repair_missing_audio: bool,
) -> Result<BackupReport, String> {
    let permits = claim_archive()?;
    // Keep archive, audio and export ownership until any detached restore has
    // committed or rolled back, even if the invoking window stops waiting.
    tokio::spawn(async move {
        let _permits = permits;
        restore_library_locked(app, path, passphrase, repair_missing_audio).await
    })
    .await
    .map_err(failure)?
}

async fn restore_library_locked(
    app: AppHandle,
    path: PathBuf,
    passphrase: Option<Zeroizing<String>>,
    repair_missing_audio: bool,
) -> Result<BackupReport, String> {
    let app_data = app.path().app_data_dir().map_err(failure)?;
    let scratch = app.path().app_local_data_dir().map_err(failure)?;
    let root = app_data.join("restored-recordings");
    let roots = if repair_missing_audio {
        Some(crate::audio::recording_preferences::recording_roots(&app).await)
    } else {
        None
    };
    let state = app
        .try_state::<AppState>()
        .ok_or("Open the meeting library first.")?;
    let report = restore_archive(
        state.db_manager.pool(),
        path,
        root,
        roots,
        passphrase,
        Some((&app_data, &scratch)),
    )
    .await?;
    let _ = app.emit("library-changed", ());
    let _ = app.emit("summary-templates-changed", ());
    Ok(report)
}

async fn restore_archive(
    pool: &SqlitePool,
    path: PathBuf,
    root: PathBuf,
    roots: Option<Vec<PathBuf>>,
    passphrase: Option<Zeroizing<String>>,
    profile: Option<(&Path, &Path)>,
) -> Result<BackupReport, String> {
    let scratch = profile.map_or_else(|| root.clone(), |(_, scratch)| scratch.to_path_buf());
    let (file, scratch_volume) =
        tokio::task::spawn_blocking(move || encryption::open_zip(&path, &scratch, passphrase))
            .await
            .map_err(failure)??;
    let mut connection = pool.acquire().await.map_err(failure)?;
    let folders = repair::saved_folders(&mut connection).await?;
    drop(connection);
    let (manifest, stage, plan) = tokio::task::spawn_blocking(move || {
        let mut plan = repair::Plan::new(folders, roots);
        let (manifest, stage) =
            unpack_archive(file, &root, scratch_volume.as_deref(), |id, has_audio| {
                if !plan.has_meeting(id) {
                    Extract::All
                } else if has_audio {
                    plan.select_audio(id)
                } else {
                    Extract::Skip
                }
            })?;
        Ok::<_, String>((manifest, stage, plan))
    })
    .await
    .map_err(failure)??;
    let app_data = profile.map(|(data, _)| data);
    let reference_root = app_data.map(|data| data.join("reference-documents"));
    import_manifest_and_repair(
        pool,
        manifest,
        stage,
        reference_root.as_deref(),
        app_data,
        Some(plan),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scheduled_archive_defers_between_local_summary_requests() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let summary =
            crate::summary::SummaryService::register_job("backup-summary", "builtin-ai").unwrap();
        let control = Arc::new(ScheduledBackup::default());
        assert!(
            matches!(claim_scheduled_archive(&control), Err(message) if message == BACKUP_DEFERRED)
        );
        drop(summary);
        let permits = claim_scheduled_archive(&control).unwrap();
        control.checkpoint().unwrap();
        let late_summary =
            crate::summary::SummaryService::register_job("backup-late-summary", "builtin-ai")
                .unwrap();
        assert_eq!(control.checkpoint().unwrap_err(), BACKUP_DEFERRED);
        drop(late_summary);
        assert_eq!(control.seal().unwrap_err(), BACKUP_DEFERRED);
        drop(permits);
        assert!(claim_scheduled_archive(&Default::default()).is_ok());
    }

    #[tokio::test]
    async fn scheduled_archive_yields_to_recording_during_audio_copy() {
        use crate::audio::inference::{claim_recording_for_test, GLOBAL_JOB_TEST_LOCK};
        use std::time::{Duration, Instant};

        let _serial = GLOBAL_JOB_TEST_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let saved = temp.path().join("recording");
        std::fs::create_dir(&saved).unwrap();
        let audio = File::create(saved.join("audio.wav")).unwrap();
        audio.set_len(64 * 1024 * 1024).unwrap();
        drop(audio);
        let source = crate::database::transcript_edits::tests::fixture().await;
        let records = tempfile::tempdir().unwrap();
        let (manifest, _) = snapshot_current(&source, records.path()).await.unwrap();
        assert_eq!(manifest.tables["meetings"].len(), 1);
        let previous = temp.path().join("previous.zip");
        std::fs::write(&previous, b"previous good backup").unwrap();
        let destination = temp.path().join("next.zip");
        let staging = tempfile::NamedTempFile::new_in(temp.path()).unwrap();
        let stage_path = staging.path().to_owned();
        let (entered, copying) = tokio::sync::oneshot::channel();
        let (unblock, blocked) = std::sync::mpsc::channel();
        let mut control = ScheduledBackup::default();
        *control.pause_copy.get_mut().unwrap() = Some((entered, blocked));
        let control = std::sync::Arc::new(control);
        let permits = claim_scheduled_archive(&control).unwrap();
        let worker_destination = destination.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let mut permits = permits;
            let prepared = prepare_archive(
                staging,
                manifest,
                vec![Some(saved)],
                None,
                None,
                Some(&control),
                false,
            )?;
            permits.finish_reading_for_schedule(&control)?;
            prepared.publish(&worker_destination, true)
        });
        tokio::time::timeout(Duration::from_secs(5), copying)
            .await
            .expect("audio copy started")
            .unwrap();
        assert!(stage_path.metadata().unwrap().len() > 64 * 1024);
        assert!(!destination.exists());
        assert!(claim_archive().is_err());

        let start = Instant::now();
        let recording = claim_recording_for_test(|| {
            let _ = unblock.send(());
        })
        .await
        .unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        let result = worker.await.unwrap();
        assert_eq!(result.err().as_deref(), Some(BACKUP_DEFERRED));
        assert!(!stage_path.exists());
        assert!(!destination.exists());
        assert_eq!(std::fs::read(previous).unwrap(), b"previous good backup");
        assert!(claim_archive().is_err());
        drop(recording);

        let restore = claim_archive().unwrap();
        assert!(claim_recording_for_test(|| {}).await.is_err());
        drop(restore);
        assert!(claim_scheduled_archive(&Default::default()).is_ok());
    }

    #[tokio::test]
    async fn scheduled_archive_releases_recording_slot_before_final_disk_flush() {
        use crate::audio::inference::{claim_recording_for_test, GLOBAL_JOB_TEST_LOCK};
        let _serial = GLOBAL_JOB_TEST_LOCK.lock().await;
        let control = Arc::new(ScheduledBackup::default());
        let mut flushing = claim_scheduled_archive(&control).unwrap();
        flushing.finish_reading_for_schedule(&control).unwrap();
        assert!(!control.defer());
        let recording = claim_recording_for_test(|| panic!("already finished reading"))
            .await
            .unwrap();
        assert!(claim_archive().is_err());
        drop(recording);
        assert!(claim_archive().is_err());
        drop(flushing);
        assert!(claim_archive().is_ok());
    }

    #[tokio::test]
    async fn scheduled_backup_counts_staging_against_free_space_on_shared_volume() {
        use crate::audio::disk_space::START_FLOOR_BYTES;
        let temp = tempfile::tempdir().unwrap();
        let source = crate::database::transcript_edits::tests::fixture().await;
        let files = records::SnapshotFiles::temporary(temp.path()).unwrap();
        let volume = PathBuf::from("test-volume");
        let control = ScheduledBackup::new(
            Some((volume.clone(), START_FLOOR_BYTES + 1)),
            Some((volume, START_FLOOR_BYTES + 1)),
        );
        let result = snapshot_for_backup(&source, &files, Some(&control)).await;
        assert_eq!(result.err().as_deref(), Some(NO_SPACE));
        assert_eq!(control.archive_budget(1), Some(0));
        drop(files);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn upgrade_drops_legacy_delivery_history_and_preserves_the_meeting() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let current = sqlx::migrate!("./migrations");
        let previous = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                current
                    .iter()
                    .filter(|migration| migration.version <= 20261122004500)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        previous.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('legacy', 'Archived meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO openclaw_handoffs (meeting_id, updated_at, status_json) VALUES ('legacy', CURRENT_TIMESTAMP, '{}')")
            .execute(&pool).await.unwrap();

        current.run(&pool).await.unwrap();

        let leftover: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_schema WHERE tbl_name = 'openclaw_handoffs'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(leftover, 0);
        let title: String = sqlx::query_scalar("SELECT title FROM meetings WHERE id = 'legacy'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(title, "Archived meeting");
    }

    #[tokio::test]
    async fn restores_legacy_delivery_rows_and_conversation_provenance() {
        let (source, request) = conversation_fixture_schema(false, "openclaw").await;
        let (mut manifest, folders) = snapshot(&source).await.unwrap();
        manifest.tables.insert(
            "openclaw_handoffs".into(),
            vec![serde_json::from_value(serde_json::json!({
                "meeting_id": "archive-source",
                "destination": "https://example.com/unused",
                "updated_at": "2026-11-21",
                "status_json": "{}"
            }))
            .unwrap()],
        );
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("backup.zip");
        write_archive(&archive, manifest, folders).unwrap();

        let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
        assert!(!manifest.tables.contains_key("openclaw_handoffs"));
        let destination = empty_conversation_destination().await;
        let report = import_manifest(&destination, manifest, stage)
            .await
            .unwrap();
        assert_eq!(report.meetings, 1);
        let history = crate::knowledge::conversations::history(&destination, &request.owner)
            .await
            .unwrap();
        let reply = history
            .iter()
            .find_map(|message| message.reply.as_ref())
            .unwrap();
        assert_eq!(reply.content, "21 September [K1].");
        assert_eq!(reply.provider, "openclaw");
    }

    #[tokio::test]
    async fn archive_policy_accounts_for_every_database_table() {
        // Every new table must be explicitly portable or profile-local.
        let excluded = [
            "knowledge_chunks",
            "knowledge_document_fts",
            "knowledge_document_permissions",
            "knowledge_fts",
            "knowledge_index_jobs",
            "knowledge_settings",
            "knowledge_sources",
            "knowledge_title_literal",
            "knowledge_transcript_literal",
            "knowledge_vectors",
            "knowledge_vector_aliases",
            "licensing",
            "pending_meeting_file_cleanup",
            "recording_calendar_drafts",
            "recording_library_deletions",
            "recording_summary_bookmarks",
            "recording_title_drafts",
            "retranscription_notes_retry",
            "saved_live_answer_attachment_instances",
            "saved_live_answer_dependencies",
            "settings",
            "transcript_settings",
            "webhook_deliveries", // delivery history belongs to this profile's destination
        ];
        let source = empty_conversation_destination().await;
        let records = tempfile::tempdir().unwrap();
        let (manifest, _) = snapshot_current(&source, records.path()).await.unwrap();
        let covered = manifest
            .tables
            .keys()
            .map(String::as_str)
            .chain(manifest.table_records.keys().map(String::as_str))
            .chain(["calendar_series_templates"]) // only series linked by an archived meeting
            .chain(archive_follow_through::TABLES.iter().copied())
            .chain(excluded)
            .collect::<std::collections::BTreeSet<_>>();
        let actual: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_list WHERE schema='main' AND type IN ('table','virtual')
             AND substr(name,1,7)<>'sqlite_' AND name<>'_sqlx_migrations' ORDER BY name",
        )
        .fetch_all(&source)
        .await
        .unwrap();
        assert_eq!(
            covered.into_iter().collect::<Vec<_>>(),
            actual.iter().map(String::as_str).collect::<Vec<_>>()
        );
        for table in excluded {
            assert!(!manifest.tables.contains_key(table));
            assert!(!manifest.table_records.contains_key(table));
        }
    }

    #[tokio::test]
    async fn version_three_restores_transcript_history_larger_than_the_legacy_manifest() {
        let source = crate::database::transcript_edits::tests::fixture().await;
        // 129 MiB of revision text in bounded rows, plus the current transcript.
        sqlx::query(
            "WITH RECURSIVE revisions(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM revisions WHERE n<129)
             INSERT INTO transcript_revisions (id, meeting_id, created_at, transcripts_json)
             SELECT 'revision-' || n, 'review-test', '2026-11-20',
                    json_array(json_object('id', 'previous', 'meeting_id', 'review-test',
                               'transcript', printf('%.*c', 1048576, 'x'), 'timestamp', '00:00'))
             FROM revisions",
        )
        .execute(&source)
        .await
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("library.zip");
        let records = tempfile::tempdir_in(temp.path()).unwrap();
        let (manifest, folders) = snapshot_current(&source, records.path()).await.unwrap();
        write_archive_impl(
            &archive,
            manifest,
            folders,
            Some(temp.path()),
            Some(records.path()),
            false,
        )
        .unwrap();
        let (manifest, stage) = unpack(&archive, temp.path(), &HashSet::new()).unwrap();
        assert_eq!(manifest.version, 3);
        assert!(!manifest.tables.contains_key("transcript_revisions"));
        let target = empty_conversation_destination().await;
        import_manifest_with_documents(&target, manifest, stage, temp.path())
            .await
            .unwrap();
        let (count, bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), SUM(length(json_extract(transcripts_json, '$[0].transcript')))
                            FROM transcript_revisions WHERE meeting_id='review-test'",
        )
        .fetch_one(&target)
        .await
        .unwrap();
        assert_eq!((count, bytes), (129, 129 * 1024 * 1024));
    }

    #[tokio::test]
    async fn version_three_rejects_stream_rows_with_foreign_meeting_ownership() {
        let source = crate::database::transcript_edits::tests::fixture().await;
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("library.zip");
        let records = tempfile::tempdir_in(temp.path()).unwrap();
        let (mut manifest, folders) = snapshot_current(&source, records.path()).await.unwrap();
        manifest
            .table_records
            .insert("transcript_revisions".into(), 1);
        std::fs::write(
            records.path().join("transcript_revisions"),
            b"{\"id\":\"revision\",\"meeting_id\":\"unowned\"}\n",
        )
        .unwrap();
        write_archive_impl(
            &archive,
            manifest,
            folders,
            Some(temp.path()),
            Some(records.path()),
            false,
        )
        .unwrap();
        let (manifest, stage) = unpack(&archive, temp.path(), &HashSet::new()).unwrap();
        let target = empty_conversation_destination().await;
        let error = import_manifest_with_documents(&target, manifest, stage, temp.path())
            .await
            .err()
            .unwrap();
        assert!(error.contains("unknown meeting reference"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM meetings")
                .fetch_one(&target)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn version_three_rejects_a_stream_with_missing_records() {
        let source = crate::database::transcript_edits::tests::fixture().await;
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("library.zip");
        let records = tempfile::tempdir_in(temp.path()).unwrap();
        let (mut manifest, folders) = snapshot_current(&source, records.path()).await.unwrap();
        *manifest.table_records.get_mut("transcripts").unwrap() += 1;
        write_archive_impl(
            &archive,
            manifest,
            folders,
            Some(temp.path()),
            Some(records.path()),
            false,
        )
        .unwrap();
        let (manifest, stage) = unpack(&archive, temp.path(), &HashSet::new()).unwrap();
        let target = empty_conversation_destination().await;
        let error = import_manifest_with_documents(&target, manifest, stage, temp.path())
            .await
            .err()
            .unwrap();
        assert!(error.contains("record count does not match"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM transcripts")
                .fetch_one(&target)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn version_three_checks_the_zip_checksum_of_a_stream() {
        use std::fs::OpenOptions;
        let source = crate::database::transcript_edits::tests::fixture().await;
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("library.zip");
        let records = tempfile::tempdir_in(temp.path()).unwrap();
        let (manifest, folders) = snapshot_current(&source, records.path()).await.unwrap();
        write_archive_impl(
            &archive,
            manifest,
            folders,
            Some(temp.path()),
            Some(records.path()),
            false,
        )
        .unwrap();
        let data_start = {
            let mut zip = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
            let entry = zip.by_name("tables/transcripts.jsonl").unwrap();
            assert!(entry.size() > 0);
            entry.data_start()
        };
        let mut file = OpenOptions::new().write(true).open(&archive).unwrap();
        file.seek(SeekFrom::Start(data_start)).unwrap();
        file.write_all(b"x").unwrap();
        drop(file);
        assert!(unpack(&archive, temp.path(), &HashSet::new()).is_err());
    }

    #[tokio::test]
    async fn pending_retranscription_notes_are_private_to_the_profile() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("INSERT INTO retranscription_notes_retry VALUES ('review-test','synthetic-profile','publication')")
            .execute(&pool).await.unwrap();
        assert!(crate::database::repositories::summary::SummaryProcessesRepository::create_automatic_process(
            &pool, "review-test", "recording", "approved"
        ).await.unwrap());
        let (mut manifest, folders) = snapshot(&pool).await.unwrap();
        assert!(!manifest.tables.contains_key("retranscription_notes_retry"));
        assert!(
            !manifest.tables["summary_processes"][0].contains_key("automatic_request_fingerprint")
        );
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("synthetic.zip");
        manifest
            .tables
            .insert("retranscription_notes_retry".into(), vec![]);
        write_archive(&archive, manifest, folders).unwrap();
        assert!(
            matches!(unpack(&archive, temp.path(), &HashSet::new()), Err(error) if error.contains("Unsupported meeting archive"))
        );
    }

    #[tokio::test]
    async fn restores_app_data_export_history_and_prevents_duplicate_remote_creates() {
        use crate::exports::{
            client::{GraphClient, RetryPolicy, TokioSleeper},
            exporter::{export_planner, ExportContext},
            ledger::{meeting_dir, task_key, ExportLedger},
            model::ExportStatus,
            planner::{action_hash, dedupe_key, task_scope, PlannerDestination},
            transport::{GraphResponse, MockGraphTransport},
        };

        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let source = empty_conversation_destination().await;
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('portable','Portable meeting','2026-11-20','2026-11-20')")
            .execute(&source).await.unwrap();
        let profile = tempfile::tempdir().unwrap();
        let profile_dir = meeting_dir(profile.path(), "portable");
        let mut ledger = ExportLedger::load_or_new(&profile_dir, "portable").unwrap();
        let meeting = crate::exports::markdown_notes::meeting_export_for_planner(
            "portable",
            "Portable meeting",
            None,
            "## Action items\n- Send draft\n- Call venue\n- Share report",
        );
        let destination = PlannerDestination {
            plan_id: "plan".into(),
            bucket_id: "bucket".into(),
        };
        let key = |index: usize| {
            dedupe_key(
                "tenant",
                "user",
                &destination,
                &meeting,
                &meeting.action_items[index],
            )
        };
        ledger.record_success(&key(0), Some("old-task".into()), None, None);
        ledger.begin_attempt(&key(1), None);
        let mut shifted = meeting.clone();
        shifted.action_items.truncate(1);
        let action = &mut shifted.action_items[0];
        action.local_action_id = "action-5".into();
        action.task = "File evidence".into();
        let hash = action_hash(action);
        let scope = task_scope("tenant", "user", &destination);
        ledger.record_success(
            &format!("{scope}:1234567890abcdef:action-4:{hash}"),
            Some("previous-task".into()),
            None,
            None,
        );
        let pending_key = task_key(&scope, "portable", "action-6", &hash);
        ledger.begin_attempt(&pending_key, None);
        let saved_id = "00000000-0000-4000-8000-000000000001";
        ledger.record_failure(
            &task_key(&scope, "portable", saved_id, &hash),
            ExportStatus::Failed,
            None,
            None,
        );
        ledger.checkpoint().await.unwrap();

        let calendar = super::super::calendar::tests::sample();
        super::super::calendar::write(&source, "portable", Some(calendar.clone()), false)
            .await
            .unwrap();
        let actions = crate::exports::actions::decode(
            r#"[{"id":"bf6d907c-34d7-4e0a-8482-3eaef7e73a7e","title":"Bericht senden","done":false}]"#,
        )
        .unwrap();
        assert_eq!(actions.len(), 1);
        let actions_json = serde_json::to_string(&actions).unwrap();
        sqlx::query("UPDATE meetings SET reviewed_actions = ? WHERE id = 'portable'")
            .bind(&actions_json)
            .execute(&source)
            .await
            .unwrap();

        let (mut manifest, folders) = snapshot(&source).await.unwrap();
        assert_eq!(folders, vec![None]);
        archive_exports::capture(&mut manifest, profile.path()).unwrap();
        let history = manifest.export_ledgers.get_mut(&0).unwrap();
        history.meeting_id = "different".into();
        assert!(archive_exports::validate(&manifest).is_err());
        manifest.export_ledgers.get_mut(&0).unwrap().meeting_id = "portable".into();
        let archive = profile.path().join("portable.zip");
        write_archive(&archive, manifest, folders).unwrap();

        let rejected = tempfile::tempdir().unwrap();
        let rejected_pool = empty_conversation_destination().await;
        sqlx::query("CREATE TABLE public_fixture_export_commit_failure(parent_id TEXT REFERENCES meetings(id) DEFERRABLE INITIALLY DEFERRED)")
            .execute(&rejected_pool).await.unwrap();
        sqlx::query("CREATE TRIGGER public_fixture_fail_export_commit AFTER INSERT ON meetings BEGIN INSERT INTO public_fixture_export_commit_failure VALUES ('missing-public-fixture-parent'); END")
            .execute(&rejected_pool).await.unwrap();
        let (manifest, stage) = unpack(
            &archive,
            &rejected.path().join("recordings"),
            &HashSet::new(),
        )
        .unwrap();
        let stage_path = stage.path().to_path_buf();
        assert!(
            import_manifest_impl(&rejected_pool, manifest, stage, None, Some(rejected.path()))
                .await
                .is_err()
        );
        assert!(!stage_path.exists());
        assert!(
            ExportLedger::read(&meeting_dir(rejected.path(), "portable"))
                .unwrap()
                .is_none()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM meetings")
            .fetch_one(&rejected_pool)
            .await
            .unwrap();
        assert_eq!(count, 0);

        let fresh = tempfile::tempdir().unwrap();
        let restored = empty_conversation_destination().await;
        let (manifest, stage) =
            unpack(&archive, &fresh.path().join("recordings"), &HashSet::new()).unwrap();
        import_manifest_impl(&restored, manifest, stage, None, Some(fresh.path()))
            .await
            .unwrap();
        assert_eq!(
            super::super::calendar::read(&restored, "portable")
                .await
                .unwrap(),
            Some(calendar)
        );
        let restored_actions: String =
            sqlx::query_scalar("SELECT reviewed_actions FROM meetings WHERE id = 'portable'")
                .fetch_one(&restored)
                .await
                .unwrap();
        assert_eq!(
            crate::exports::actions::decode(&restored_actions).unwrap(),
            actions
        );
        let mut ledger =
            ExportLedger::load_or_new(&meeting_dir(fresh.path(), "portable"), "portable").unwrap();
        let transport = MockGraphTransport::new();
        let url = "https://graph.microsoft.com/v1.0/planner/tasks";
        transport.queue_for_url(url, [GraphResponse::success(201, r#"{"id":"new-task"}"#)]);
        let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        let report = export_planner(&client, &mut ledger, &meeting, &destination, &account, None)
            .await
            .unwrap();
        assert_eq!(report.items[0].resource_id.as_deref(), Some("old-task"));
        assert!(!report.items[0].graph_called);
        assert_eq!(report.items[1].status, ExportStatus::UnknownAfterSubmit);
        assert!(!report.items[1].graph_called);
        assert!(report.items[2].graph_called);
        assert_eq!(
            ledger.entry(&pending_key).unwrap().status,
            ExportStatus::UnknownAfterSubmit
        );
        let report = export_planner(&client, &mut ledger, &shifted, &destination, &account, None)
            .await
            .unwrap();
        assert_eq!(report.items[0].local_id, "action-5");
        assert_eq!(
            report.items[0].code.as_deref(),
            Some("task_identity_review_required")
        );
        assert!(!report.items[0].graph_called);
        let action = &mut shifted.action_items[0];
        action.legacy_action = Some(crate::exports::actions::LegacyAction {
            id: action.local_action_id.clone(),
            title_hash: hash,
        });
        action.local_action_id = saved_id.into();
        let report = export_planner(&client, &mut ledger, &shifted, &destination, &account, None)
            .await
            .unwrap();
        assert_eq!(
            report.items[0].code.as_deref(),
            Some("task_identity_review_required")
        );
        assert!(!report.items[0].graph_called);
        assert_eq!(client.transport().calls_for(url), 1);
    }

    #[tokio::test]
    async fn legacy_folder_history_restores_without_creating_a_recording_folder() {
        use crate::exports::ledger::{meeting_dir, ExportLedger, LEDGER_FILENAME};

        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let source = crate::database::transcript_edits::tests::fixture().await;
        let (mut manifest, _) = snapshot(&source).await.unwrap();
        manifest.files.insert(0, vec![LEDGER_FILENAME.into()]);
        let ledger = ExportLedger::new("review-test");
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("legacy-export.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        for (name, bytes) in [
            (
                "manifest.json".to_string(),
                serde_json::to_vec(&manifest).unwrap(),
            ),
            (
                format!("recordings/0/{LEDGER_FILENAME}"),
                serde_json::to_vec(&ledger).unwrap(),
            ),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();

        let restored = empty_conversation_destination().await;
        let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
        let report = import_manifest_impl(&restored, manifest, stage, None, Some(root.path()))
            .await
            .unwrap();
        assert_eq!(report.files, 0);
        let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings")
            .fetch_one(&restored)
            .await
            .unwrap();
        assert!(folder.is_none());
        assert_eq!(
            ExportLedger::read(&meeting_dir(root.path(), "review-test")).unwrap(),
            Some(ledger)
        );
    }

    pub(super) async fn conversation_fixture() -> (SqlitePool, crate::knowledge::types::AskRequest)
    {
        conversation_fixture_schema(false, "builtin-ai").await
    }

    // Write the evidence columns available in the shipped schema. Current
    // dispatch validates metadata introduced by later migrations.
    async fn seed_legacy_evidence(
        pool: &SqlitePool,
        request: &str,
        passage: &crate::knowledge::types::Passage,
    ) {
        let display = serde_json::json!({
            "title": passage.title,
            "date": passage.date,
            "speaker": passage.speaker,
            "metadata_truncated": passage.metadata_truncated,
        });
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "UPDATE knowledge_requests SET status='running' WHERE id=? AND status='preparing'",
        )
        .bind(request)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("INSERT INTO knowledge_request_evidence(request_id,ordinal,reference_json,display_json) VALUES (?,1,?,?)")
            .bind(request).bind(serde_json::to_string(&passage.evidence).unwrap())
            .bind(display.to_string()).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn conversation_fixture_schema(
        legacy: bool,
        provider: &str,
    ) -> (SqlitePool, crate::knowledge::types::AskRequest) {
        use crate::knowledge::{conversations, retrieval, store, types::*};
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        if legacy {
            let shipped = sqlx::migrate::Migrator {
                migrations: std::borrow::Cow::Owned(
                    sqlx::migrate!("./migrations")
                        .iter()
                        .filter(|migration| migration.version < 20261008000000)
                        .cloned()
                        .collect(),
                ),
                ..sqlx::migrate::Migrator::DEFAULT
            };
            shipped.run(&pool).await.unwrap();
        } else {
            sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('archive-source','Public archive fixture','2026-09-01','2026-09-01')").execute(&pool).await.unwrap();
        for (id, text) in [
            ("archive-first", "Approved: 21 September."),
            ("archive-second", "Budget unchanged."),
        ] {
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES (?,'archive-source',?,'2026-09-01')").bind(id).bind(text).execute(&pool).await.unwrap();
        }
        let request = AskRequest {
            live_reference_scope: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            owner: conversations::create_library(&pool).await.unwrap(),
            search: SearchRequest {
                scope: KnowledgeScope::Library {
                    filter: MeetingFilter {
                        all_meetings: true,
                        ..Default::default()
                    },
                },
                query: "What was approved?".into(),
                document_ids: vec![],
                mode: SearchMode::Keyword,
            },
        };
        conversations::reserve(&pool, &request, provider, "qwen3.5:4b")
            .await
            .unwrap();
        let frozen = retrieval::freeze_scope(&pool, &request.search.scope)
            .await
            .unwrap();
        let job:store::SourceJob=sqlx::query_as("SELECT id AS source_id,meeting_id,revision,generation FROM knowledge_sources WHERE meeting_id='archive-source'").fetch_one(&pool).await.unwrap();
        let passage = store::materialize(
            &pool,
            &store::SelectedRow::for_job(&job, "archive-first".into()),
            TextSpan {
                transcript_id: "archive-first".into(),
                start_byte: 0,
                end_byte: "Approved: 21 September.".len(),
            },
            false,
        )
        .await
        .unwrap();
        if legacy {
            seed_legacy_evidence(&pool, &request.request_id, &passage).await;
        } else {
            conversations::prepare(
                &pool,
                &request.request_id,
                &frozen,
                &[passage],
                &BTreeMap::new(),
                SearchMode::Keyword,
            )
            .await
            .unwrap();
        }
        conversations::finish(
            &pool,
            &request.request_id,
            &frozen,
            "21 September [K1].",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        (pool, request)
    }

    pub(super) async fn empty_conversation_destination() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn summary_archive_preserves_partial_validity_and_excludes_imported_startup_retries() {
        use crate::database::repositories::summary::SummaryProcessesRepository as Summaries;

        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let source = empty_conversation_destination().await;
        for meeting in ["matching", "stale", "existing"] {
            sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, 'Archived meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
                .bind(meeting).execute(&source).await.unwrap();
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES (?1, ?1, 'Original passage', '00:00')")
                .bind(meeting).execute(&source).await.unwrap();
        }
        sqlx::query(
            "UPDATE transcripts SET transcript = 'Corrected passage' WHERE id = 'matching'",
        )
        .execute(&source)
        .await
        .unwrap();
        let fingerprint = "synthetic-approved-destination";
        assert!(
            Summaries::create_automatic_process(&source, "matching", "import", fingerprint)
                .await
                .unwrap()
        );
        // Make the synthetic job eligible through the same interruption path as startup.
        Summaries::fail_interrupted_processes(&source)
            .await
            .unwrap();
        let (interrupted, _) = Summaries::get_summary_status_data(&source, "matching")
            .await
            .unwrap()
            .unwrap();
        let reason = interrupted.error.unwrap();
        assert!(
            Summaries::fail_with_partial(&source, "matching", &reason, 2, "Matching partial")
                .await
                .unwrap()
        );
        let saved = serde_json::json!({"markdown": "Saved notes"}).to_string();
        sqlx::query("UPDATE summary_processes SET result = ? WHERE meeting_id = 'matching'")
            .bind(&saved)
            .execute(&source)
            .await
            .unwrap();
        Summaries::create_or_reset_process(&source, "stale")
            .await
            .unwrap();
        assert!(Summaries::fail_with_partial(
            &source,
            "stale",
            "Generation failed",
            1,
            "Stale partial"
        )
        .await
        .unwrap());
        sqlx::query(
            "UPDATE transcripts SET transcript = 'Edited after generation' WHERE id = 'stale'",
        )
        .execute(&source)
        .await
        .unwrap();
        Summaries::record_automatic_outcome(
            &source,
            "existing",
            "skipped",
            "Disabled",
            "recording",
        )
        .await
        .unwrap();
        assert_eq!(
            Summaries::automatic_resume_candidates(&source, fingerprint, None)
                .await
                .unwrap(),
            vec![("matching".to_owned(), fingerprint.to_owned())]
        );

        let destination = empty_conversation_destination().await;
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('existing', 'Local meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&destination).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('existing', 'existing', 'Local passage', '00:00')")
            .execute(&destination).await.unwrap();
        sqlx::query(
            "UPDATE transcripts SET transcript = 'Edited local passage' WHERE id = 'existing'",
        )
        .execute(&destination)
        .await
        .unwrap();
        assert!(Summaries::create_automatic_process(
            &destination,
            "existing",
            "recording",
            fingerprint
        )
        .await
        .unwrap());
        assert!(Summaries::fail_with_partial(
            &destination,
            "existing",
            &reason,
            2,
            "Local partial"
        )
        .await
        .unwrap());

        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("backup.zip");
        let (manifest, folders) = snapshot(&source).await.unwrap();
        write_archive(&archive, manifest, folders).unwrap();
        let existing = HashSet::from(["existing".to_string()]);
        let (manifest, stage) = unpack(&archive, root.path(), &existing).unwrap();
        let report = import_manifest(&destination, manifest, stage)
            .await
            .unwrap();
        assert_eq!((report.meetings, report.skipped), (2, 1));

        let (matching, partial) = Summaries::get_summary_status_data(&destination, "matching")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(matching.status, "failed");
        assert_eq!(matching.error.as_deref(), Some(reason.as_str()));
        assert_eq!(matching.result.as_deref(), Some(saved.as_str()));
        assert_eq!(partial.as_deref(), Some("Matching partial"));

        let (stale, partial) = Summaries::get_summary_status_data(&destination, "stale")
            .await
            .unwrap()
            .unwrap();
        assert!(stale.metadata.unwrap().contains("Stale partial"));
        assert!(partial.is_none());

        let existing_transcript: String =
            sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id = 'existing'")
                .fetch_one(&destination)
                .await
                .unwrap();
        assert_eq!(existing_transcript, "Edited local passage");
        let (_, partial) = Summaries::get_summary_status_data(&destination, "existing")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(partial.as_deref(), Some("Local partial"));
        assert_eq!(
            Summaries::automatic_resume_candidates(&destination, fingerprint, None)
                .await
                .unwrap(),
            vec![("existing".to_owned(), fingerprint.to_owned())]
        );
        assert!(
            !Summaries::claim_automatic_resume(&destination, "matching", fingerprint, false)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn legacy_summary_archive_retains_a_partial_with_its_original_revision_check() {
        use crate::database::repositories::summary::SummaryProcessesRepository as Summaries;

        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let source = crate::database::transcript_edits::tests::fixture().await;
        let (mut manifest, folders) = snapshot(&source).await.unwrap();
        manifest.tables.get_mut("meetings").unwrap()[0].remove("summary_content_revision");
        manifest.tables.get_mut("summary_processes").unwrap().push(
            serde_json::from_value(serde_json::json!({
                "meeting_id": "review-test",
                "status": "FAILED",
                "created_at": "2026-10-01 00:00:00",
                "updated_at": "2026-10-01 00:00:00",
                "metadata": r#"{"partial_notes":"Legacy partial","transcript_revision":3}"#
            }))
            .unwrap(),
        );
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("legacy.zip");
        write_archive(&archive, manifest, folders).unwrap();
        let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
        let destination = empty_conversation_destination().await;
        import_manifest(&destination, manifest, stage)
            .await
            .unwrap();
        let (summary, partial) = Summaries::get_summary_status_data(&destination, "review-test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, "FAILED");
        assert_eq!(partial.as_deref(), Some("Legacy partial"));
        let content_revision: i64 = sqlx::query_scalar(
            "SELECT summary_content_revision FROM meetings WHERE id = 'review-test'",
        )
        .fetch_one(&destination)
        .await
        .unwrap();
        assert_eq!(content_revision, 2);
    }

    #[tokio::test]
    async fn conversation_question_context_round_trips_and_rejects_invalid_links() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::{conversations, types::SearchMode};
        let (source, request, frozen, passages) =
            conversations::tests::question_pair_fixture("Nein.").await;
        conversations::prepare(
            &source,
            &request.request_id,
            &frozen,
            &passages,
            &BTreeMap::new(),
            SearchMode::Keyword,
        )
        .await
        .unwrap();
        conversations::finish(
            &source,
            &request.request_id,
            &frozen,
            "Abgelehnt [K2].",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        let (manifest, _) = snapshot(&source).await.unwrap();
        let destination = empty_conversation_destination().await;
        import_manifest(&destination, manifest, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        let history = conversations::history(&destination, &request.owner)
            .await
            .unwrap();
        let reply = history[1].reply.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(reply).unwrap()["context_links"],
            serde_json::json!([{"kind":"preceding_question","cited_tag":2,"context_tag":1}])
        );
        assert_eq!(reply.content, "Abgelehnt [K2].");
        assert!(reply.evidence.iter().all(|r| r.historical));
        let (mut older, _) = snapshot(&source).await.unwrap();
        for row in older.tables.get_mut("knowledge_request_evidence").unwrap() {
            let mut display: Value =
                serde_json::from_str(row["display_json"].as_str().unwrap()).unwrap();
            display
                .as_object_mut()
                .unwrap()
                .remove("preceding_question_tag");
            row.insert("display_json".into(), Value::String(display.to_string()));
        }
        let old_destination = empty_conversation_destination().await;
        import_manifest(&old_destination, older, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        let old_history = conversations::history(&old_destination, &request.owner)
            .await
            .unwrap();
        assert!(old_history[1]
            .reply
            .as_ref()
            .unwrap()
            .context_links
            .is_empty());
        for tag in [0, 2, 999] {
            let (mut malformed, _) = snapshot(&source).await.unwrap();
            for row in malformed
                .tables
                .get_mut("knowledge_request_evidence")
                .unwrap()
            {
                if row["ordinal"] == 2 {
                    let mut display: Value =
                        serde_json::from_str(row["display_json"].as_str().unwrap()).unwrap();
                    display["preceding_question_tag"] = Value::from(tag);
                    row.insert("display_json".into(), Value::String(display.to_string()));
                }
            }
            assert!(import_manifest(
                &empty_conversation_destination().await,
                malformed,
                tempfile::tempdir().unwrap()
            )
            .await
            .is_err());
        }
    }

    #[tokio::test]
    async fn conversation_evidence_upgrade_preserves_populated_schema_and_old_backup() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::{conversations, retrieval, store, types::*};
        async fn counts(pool: &SqlitePool) -> Vec<(String, i64)> {
            let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'knowledge_%' ORDER BY name").fetch_all(pool).await.unwrap();
            let mut result = Vec::new();
            for table in tables {
                let count: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM \"{}\"",
                    table.replace('"', "\"\"")
                ))
                .fetch_one(pool)
                .await
                .unwrap();
                result.push((table, count));
            }
            result
        }
        async fn schema(pool: &SqlitePool) -> Vec<(String, String, String, Option<String>)> {
            sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE type IN('index','trigger') AND (name LIKE 'knowledge_%' OR tbl_name LIKE 'knowledge_%') ORDER BY type,name").fetch_all(pool).await.unwrap()
        }
        let (source, first) = conversation_fixture_schema(true, "builtin-ai").await;
        let mut owners = vec![first.owner.clone()];
        for index in 0..3 {
            let meeting = format!("upgrade-meeting-{index}");
            let transcript = format!("upgrade-row-{index}");
            let text = "Launch plan approved.";
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?,'Public upgrade fixture','2026-09-02','2026-09-02')").bind(&meeting).execute(&source).await.unwrap();
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES (?,?,?,'2026-09-02')").bind(&transcript).bind(&meeting).bind(text).execute(&source).await.unwrap();
            let request = AskRequest {
                live_reference_scope: None,
                request_id: uuid::Uuid::new_v4().to_string(),
                owner: conversations::create_library(&source).await.unwrap(),
                search: SearchRequest {
                    scope: KnowledgeScope::Library {
                        filter: MeetingFilter {
                            meeting_ids: vec![meeting.clone()],
                            ..Default::default()
                        },
                    },
                    query: "What was approved?".into(),
                    document_ids: vec![],
                    mode: SearchMode::Keyword,
                },
            };
            owners.push(request.owner.clone());
            conversations::reserve(&source, &request, "builtin-ai", "qwen3.5:4b")
                .await
                .unwrap();
            let frozen = retrieval::freeze_scope(&source, &request.search.scope)
                .await
                .unwrap();
            let job:store::SourceJob=sqlx::query_as("SELECT id AS source_id,meeting_id,revision,generation FROM knowledge_sources WHERE meeting_id=?").bind(&meeting).fetch_one(&source).await.unwrap();
            let passage = store::materialize(
                &source,
                &store::SelectedRow::for_job(&job, transcript.clone()),
                TextSpan {
                    transcript_id: transcript,
                    start_byte: 0,
                    end_byte: text.len(),
                },
                false,
            )
            .await
            .unwrap();
            seed_legacy_evidence(&source, &request.request_id, &passage).await;
            conversations::finish(
                &source,
                &request.request_id,
                &frozen,
                "Launch approved [K1].",
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO knowledge_chunks(id,source_id,revision,generation,ordinal,transcript_id,start_byte,end_byte,fingerprint,text) SELECT 'upgrade-chunk',id,revision,generation,1,'archive-first',0,1,'public-fixture','A' FROM knowledge_sources WHERE meeting_id='archive-source'").execute(&source).await.unwrap();
        sqlx::query("INSERT INTO knowledge_vectors(chunk_id,space,dimensions,vector) VALUES ('upgrade-chunk','public-fixture',384,zeroblob(1536))").execute(&source).await.unwrap();
        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table'")
                .fetch_all(&source)
                .await
                .unwrap();
        for table in tables {
            let children = sqlx::query(&format!(
                "PRAGMA foreign_key_list(\"{}\")",
                table.replace('"', "\"\"")
            ))
            .fetch_all(&source)
            .await
            .unwrap();
            assert!(
                children
                    .iter()
                    .all(|row| row.get::<String, _>("table") != "knowledge_request_evidence"),
                "Evidence rebuild has a child table: {table}"
            );
        }
        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&source)
            .await
            .unwrap();
        assert_eq!(enabled, 1, "Migration must run with foreign keys enabled");
        let before_counts = counts(&source).await;
        let before_schema = schema(&source).await;
        assert!(before_schema.len() > 10);
        // Export exactly the archive tables available before the evidence upgrade.
        // The current writer also attaches recording-ID bookmarks, introduced later.
        const HISTORICAL_TABLES: &[&str] = &[
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
            "knowledge_owners",
            "knowledge_requests",
            "knowledge_messages",
            "knowledge_request_evidence",
            "knowledge_request_sources",
        ];
        let (old_backup, _) = snapshot_tables(&source, HISTORICAL_TABLES, false)
            .await
            .unwrap();
        assert!(!old_backup.tables.contains_key("meeting_summary_bookmarks"));
        let mut histories = Vec::new();
        for owner in &owners {
            histories.push(conversations::history(&source, owner).await.unwrap());
        }
        // This regression targets the shipped evidence-table rebuild. Later additive
        // migrations have their own upgrade contracts and may create new tables.
        let full = sqlx::migrate!("./migrations");
        let evidence_upgrade = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                full.iter()
                    .filter(|m| m.version <= 20261008000000)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        evidence_upgrade.run(&source).await.unwrap();
        assert_eq!(
            counts(&source).await,
            before_counts,
            "Every knowledge table, including derived tables, must retain its rows"
        );
        assert_eq!(
            schema(&source).await,
            before_schema,
            "Indexes and triggers must retain their exact names, owners and SQL"
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&source)
            .await
            .unwrap()
            .is_empty());
        for (owner, history) in owners.iter().zip(&histories) {
            assert_eq!(
                serde_json::to_value(conversations::history(&source, owner).await.unwrap())
                    .unwrap(),
                serde_json::to_value(history).unwrap()
            );
        }
        sqlx::query("INSERT INTO knowledge_request_evidence SELECT request_id,700,reference_json,display_json FROM knowledge_request_evidence WHERE request_id=? AND ordinal=1").bind(&first.request_id).execute(&source).await.unwrap();
        assert!(sqlx::query("INSERT INTO knowledge_request_evidence SELECT request_id,1025,reference_json,display_json FROM knowledge_request_evidence WHERE request_id=? AND ordinal=1").bind(&first.request_id).execute(&source).await.is_err());
        let destination = empty_conversation_destination().await;
        import_manifest(&destination, old_backup, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        for (owner, history) in owners.iter().zip(&histories) {
            let restored = conversations::history(&destination, owner).await.unwrap();
            assert_eq!(restored.len(), history.len());
            for (after, before) in restored.iter().zip(history) {
                assert_eq!(after.content, before.content);
                if let Some(before) = &before.reply {
                    let after = after.reply.as_ref().unwrap();
                    assert_eq!(after.cited_tags, before.cited_tags);
                    assert_eq!(after.evidence.len(), before.evidence.len());
                    for (a, b) in after.evidence.iter().zip(&before.evidence) {
                        assert_eq!(a.fingerprint, b.fingerprint);
                        assert_eq!(a.locator, b.locator);
                    }
                }
            }
        }
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&destination)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn conversation_round_trip_keeps_seven_hundred_evidence_entries() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::{conversations, retrieval, store, types::*};
        let (source, mut request) = conversation_fixture().await;
        let text = "Launch plan remains approved.";
        for index in 0..700 {
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES (?,'archive-source',?,'2026-09-01')")
                .bind(format!("large-{index:04}")).bind(text).execute(&source).await.unwrap();
        }
        request.request_id = uuid::Uuid::new_v4().to_string();
        conversations::reserve(&source, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap();
        let frozen = retrieval::freeze_scope(&source, &request.search.scope)
            .await
            .unwrap();
        let job: store::SourceJob = sqlx::query_as("SELECT id AS source_id,meeting_id,revision,generation FROM knowledge_sources WHERE meeting_id='archive-source'").fetch_one(&source).await.unwrap();
        let mut passages = Vec::new();
        for index in 0..700 {
            let id = format!("large-{index:04}");
            passages.push(
                store::materialize(
                    &source,
                    &store::SelectedRow::for_job(&job, id.clone()),
                    TextSpan {
                        transcript_id: id,
                        start_byte: 0,
                        end_byte: text.len(),
                    },
                    false,
                )
                .await
                .unwrap(),
            );
        }
        conversations::prepare(
            &source,
            &request.request_id,
            &frozen,
            &passages,
            &BTreeMap::new(),
            SearchMode::Keyword,
        )
        .await
        .unwrap();
        conversations::finish(
            &source,
            &request.request_id,
            &frozen,
            "Launch approved [K1][K700].",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        let (manifest, _) = snapshot(&source).await.unwrap();
        let destination = empty_conversation_destination().await;
        import_manifest(&destination, manifest, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        let reply = conversations::history(&destination, &request.owner)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .reply
            .unwrap();
        assert_eq!(reply.evidence.len(), 700);
        assert_eq!(reply.evidence_metadata.len(), 700);
        assert_eq!(reply.cited_tags, vec![1, 700]);
    }

    #[tokio::test]
    async fn conversation_round_trip_preserves_stale_refs_when_revision_counters_match() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::{conversations, evidence};
        let (source, request) = conversation_fixture().await;
        let original = conversations::history(&source, &request.owner)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .reply
            .unwrap()
            .evidence[0]
            .clone();
        sqlx::query("UPDATE transcripts SET transcript='Budget changed after the answer.' WHERE id='archive-second'").execute(&source).await.unwrap();
        let (manifest, _) = snapshot(&source).await.unwrap();
        assert!(manifest.tables.contains_key("knowledge_owners"));
        assert!(!manifest.tables.contains_key("knowledge_chunks"));
        let destination = empty_conversation_destination().await;
        import_manifest(&destination, manifest, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM knowledge_sources WHERE meeting_id='archive-source'",
        )
        .fetch_one(&destination)
        .await
        .unwrap();
        assert_eq!(
            revision, original.source_revision,
            "Fixture must prove coincidentally matching restored counters"
        );
        let history = conversations::history(&destination, &request.owner)
            .await
            .unwrap();
        assert_eq!(history.len(), 2);
        let restored = &history[1].reply.as_ref().unwrap().evidence[0];
        assert!(restored.historical);
        assert_eq!(restored.fingerprint, original.fingerprint);
        assert_eq!(restored.chunk_id, original.chunk_id);
        assert_eq!(restored.locator, original.locator);
        assert_eq!(
            evidence::resolve(&destination, restored)
                .await
                .unwrap()
                .status,
            evidence::EvidenceStatus::Stale
        );
        let frozen = crate::knowledge::retrieval::freeze_scope(&destination, &request.search.scope)
            .await
            .unwrap();
        assert!(
            conversations::eligible_history(&destination, &request.owner, &frozen, 8192)
                .await
                .unwrap()
                .0
                .is_empty()
        );
    }

    #[tokio::test]
    async fn conversation_restore_keeps_library_owner_and_interrupts_pending_work() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::conversations;
        let (source, mut request) = conversation_fixture().await;
        let empty_owner = conversations::create_library(&source).await.unwrap();
        request.request_id = uuid::Uuid::new_v4().to_string();
        conversations::reserve(&source, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap();
        let (manifest, _) = snapshot(&source).await.unwrap();
        let destination = empty_conversation_destination().await;
        import_manifest(&destination, manifest, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        assert!(conversations::list_libraries(&destination)
            .await
            .unwrap()
            .contains(&empty_owner));
        let status: String = sqlx::query_scalar("SELECT status FROM knowledge_requests WHERE id=?")
            .bind(&request.request_id)
            .fetch_one(&destination)
            .await
            .unwrap();
        assert_eq!(status, "interrupted");
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM knowledge_requests WHERE status IN ('preparing','running')",
        )
        .fetch_one(&destination)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn conversation_restore_skips_colliding_owner_without_overwriting_history() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::conversations;
        let (pool, request) = conversation_fixture().await;
        let (manifest, _) = snapshot(&pool).await.unwrap();
        import_manifest(&pool, manifest, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        let history = conversations::history(&pool, &request.owner).await.unwrap();
        assert_eq!(history.len(), 2);
        assert!(!history[1].reply.as_ref().unwrap().evidence[0].historical);
    }

    #[tokio::test]
    async fn conversation_restore_redacts_changed_skipped_source_but_keeps_identical_source() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::{conversations, retrieval, types::SearchMode};
        for changed in [false, true] {
            let (source, request) = conversation_fixture().await;
            let destination = empty_conversation_destination().await;
            let (mut canonical, _) = snapshot(&source).await.unwrap();
            canonical
                .tables
                .retain(|table, _| !table.starts_with("knowledge_"));
            import_manifest(&destination, canonical, tempfile::tempdir().unwrap())
                .await
                .unwrap();
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('archive-other','New archive source','2026-09-02','2026-09-02')").execute(&source).await.unwrap();
            let mut mixed = request.clone();
            mixed.request_id = uuid::Uuid::new_v4().to_string();
            conversations::reserve(&source, &mixed, "builtin-ai", "qwen3.5:4b")
                .await
                .unwrap();
            let frozen = retrieval::freeze_scope(&source, &mixed.search.scope)
                .await
                .unwrap();
            conversations::prepare(
                &source,
                &mixed.request_id,
                &frozen,
                &[],
                &BTreeMap::new(),
                SearchMode::Keyword,
            )
            .await
            .unwrap();
            conversations::finish(
                &source,
                &mixed.request_id,
                &frozen,
                "The selected evidence does not establish another action.",
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
            let (manifest, _) = snapshot(&source).await.unwrap();
            if changed {
                sqlx::query("UPDATE transcripts SET transcript='Different destination evidence.' WHERE id='archive-second'").execute(&destination).await.unwrap();
            }
            import_manifest(&destination, manifest, tempfile::tempdir().unwrap())
                .await
                .unwrap();
            let history = conversations::history(&destination, &request.owner)
                .await
                .unwrap();
            assert_eq!(history.len(), 4);
            let new_source: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM meetings WHERE id='archive-other')",
            )
            .fetch_one(&destination)
            .await
            .unwrap();
            assert!(new_source);
            if changed {
                assert!(history
                    .iter()
                    .all(|message| message.status == "invalidated" && message.content.is_empty()));
            } else {
                assert!(history
                    .iter()
                    .any(|message| message.content == "21 September [K1]."));
            }
        }
    }

    #[tokio::test]
    async fn version_three_compares_skipped_conversation_transcripts_from_the_zip() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::conversations;

        let (source, request) = conversation_fixture().await;
        let temp = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir_in(temp.path()).unwrap();
        for (case, source_empty, destination_change, matches) in [
            ("same", false, "", true),
            (
                "changed",
                false,
                "UPDATE transcripts SET transcript='Different' WHERE id='archive-second'",
                false,
            ),
            ("destination-empty", false, "DELETE FROM transcripts", false),
            ("archive-empty", true, "", false),
            ("both-empty", true, "DELETE FROM transcripts", true),
        ] {
            let target = empty_conversation_destination().await;
            // Use independent transcripts so a restore has to compare rather than
            // relying on the origin of the destination's data or revision number.
            sqlx::raw_sql(
                "INSERT INTO meetings(id,title,created_at,updated_at)
                 VALUES ('archive-source','Public archive fixture','2026-09-01','2026-11-21');
                 INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES
                 ('archive-second','archive-source','Budget unchanged.','2026-09-01'),
                 ('archive-first','archive-source','Approved: 21 September.','2026-09-01');",
            )
            .execute(&target)
            .await
            .unwrap();
            if !destination_change.is_empty() {
                sqlx::query(destination_change)
                    .execute(&target)
                    .await
                    .unwrap();
            }
            let transcript = || async {
                sqlx::query_as::<_, (String, String)>(
                    "SELECT id,transcript FROM transcripts ORDER BY id",
                )
                .fetch_all(&target)
                .await
                .unwrap()
            };
            let before = transcript().await;
            if source_empty {
                sqlx::query("DELETE FROM transcripts")
                    .execute(&source)
                    .await
                    .unwrap();
            }
            let archive = temp.path().join(format!("{case}.zip"));
            let (manifest, folders) = snapshot_current(&source, records.path()).await.unwrap();
            assert!(!manifest.tables.contains_key("transcripts"));
            assert_eq!(
                manifest.table_records["transcripts"],
                if source_empty { 0 } else { 2 }
            );
            write_archive_impl(
                &archive,
                manifest,
                folders,
                Some(temp.path()),
                Some(records.path()),
                false,
            )
            .unwrap();
            let (manifest, stage) = unpack(
                &archive,
                temp.path(),
                &HashSet::from(["archive-source".into()]),
            )
            .unwrap();
            let report = import_manifest_with_documents(&target, manifest, stage, temp.path())
                .await
                .unwrap();
            assert_eq!((report.meetings, report.skipped), (0, 1), "{case}");
            assert_eq!(transcript().await, before, "{case}");

            let history = conversations::history(&target, &request.owner)
                .await
                .unwrap();
            assert_eq!(history.len(), 2, "{case}");
            if matches {
                assert_eq!(history[0].content, "What was approved?", "{case}");
                assert_eq!(history[1].content, "21 September [K1].", "{case}");
                let reply = history[1].reply.as_ref().unwrap();
                assert_eq!(reply.cited_tags, vec![1], "{case}");
                assert!(reply.evidence[0].historical, "{case}");
            } else {
                assert!(
                    history.iter().all(|message| message.status == "invalidated"
                        && message.content.is_empty()
                        && message.reply.is_none()),
                    "{case}"
                );
            }
        }
    }

    #[tokio::test]
    async fn conversation_library_only_answer_round_trips_without_meetings() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::knowledge::{conversations, retrieval, types::*};
        let source = empty_conversation_destination().await;
        let request = AskRequest {
            live_reference_scope: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            owner: conversations::create_library(&source).await.unwrap(),
            search: SearchRequest {
                scope: KnowledgeScope::Library {
                    filter: MeetingFilter {
                        all_meetings: true,
                        ..Default::default()
                    },
                },
                query: "What evidence is available?".into(),
                document_ids: vec![],
                mode: SearchMode::Keyword,
            },
        };
        conversations::reserve(&source, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap();
        let frozen = retrieval::freeze_scope(&source, &request.search.scope)
            .await
            .unwrap();
        conversations::prepare(
            &source,
            &request.request_id,
            &frozen,
            &[],
            &BTreeMap::new(),
            SearchMode::Keyword,
        )
        .await
        .unwrap();
        conversations::finish(
            &source,
            &request.request_id,
            &frozen,
            "No evidence is available in the selected library.",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        let (manifest, _) = snapshot(&source).await.unwrap();
        assert!(manifest.tables["meetings"].is_empty());
        let destination = empty_conversation_destination().await;
        import_manifest(&destination, manifest, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        assert_eq!(
            conversations::history(&destination, &request.owner)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn conversation_restore_rejects_changed_input_atomically_and_accepts_prefeature_archive()
    {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let (source, _) = conversation_fixture().await;
        let (mut malformed, _) = snapshot(&source).await.unwrap();
        malformed.tables.get_mut("knowledge_requests").unwrap()[0]
            .insert("question".into(), Value::from("Changed immutable question"));
        let destination = empty_conversation_destination().await;
        assert!(
            import_manifest(&destination, malformed, tempfile::tempdir().unwrap())
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM meetings")
            .fetch_one(&destination)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let (mut old, _) = snapshot(&source).await.unwrap();
        old.tables
            .retain(|table, _| !table.starts_with("knowledge_"));
        import_manifest(&destination, old, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        assert!(
            crate::knowledge::conversations::list_libraries(&destination)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn backup_without_audio_restores_meeting_and_recording_metadata() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("audio.WAV"), b"synthetic audio").unwrap();
        std::fs::write(source.path().join("metadata.json"), b"{}").unwrap();
        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'review-test'")
            .bind(source.path().to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        let audio_only = tempfile::tempdir().unwrap();
        std::fs::write(audio_only.path().join("audio.wav"), b"synthetic audio").unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES ('audio-only', 'Audio only', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
            .bind(audio_only.path().to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO recording_outcomes (meeting_id) VALUES ('audio-only')")
            .execute(&pool)
            .await
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let records = records::SnapshotFiles::temporary(target.path()).unwrap();
        let (manifest, folders) = snapshot_for_backup(&pool, &records, None).await.unwrap();
        let archive = target.path().join("without-audio.zip");
        let report = prepare_archive(
            archive_tempfile(&archive).unwrap(),
            manifest,
            folders,
            None,
            Some(&records),
            None,
            true,
        )
        .unwrap()
        .publish(&archive, false)
        .unwrap();
        assert_eq!(report.files, 1);
        assert!(report.audio_excluded);
        assert!(report.incomplete_meetings.is_empty());

        let (manifest, stage) = unpack(&archive, target.path(), &HashSet::new()).unwrap();
        assert!(manifest.audio_excluded);
        let restored_pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("DELETE FROM meetings")
            .execute(&restored_pool)
            .await
            .unwrap();
        let report = import_manifest(&restored_pool, manifest, stage)
            .await
            .unwrap();
        assert_eq!(report.meetings, 2);
        assert!(report.audio_excluded);
        let transcript: String =
            sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id = 'b'")
                .fetch_one(&restored_pool)
                .await
                .unwrap();
        assert_eq!(transcript, "Project confirmed");
        let folder: Option<String> =
            sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'audio-only'")
                .fetch_one(&restored_pool)
                .await
                .unwrap();
        assert_eq!(folder, None);
        let outcome: (bool, bool) = sqlx::query_as(
            "SELECT audio_save_failed, recovery_files_elsewhere FROM recording_outcomes WHERE meeting_id = 'audio-only'",
        )
        .fetch_one(&restored_pool)
        .await
        .unwrap();
        assert_eq!(outcome, (false, false));
        let restored: String =
            sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'review-test'")
                .fetch_one(&restored_pool)
                .await
                .unwrap();
        assert!(Path::new(&restored).join("metadata.json").is_file());
        assert!(!Path::new(&restored).join("audio.WAV").exists());
        assert!(source.path().join("audio.WAV").is_file());
    }

    #[tokio::test]
    async fn mixed_restore_keeps_only_new_meetings_audio() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
    async fn restore_repairs_missing_audio_without_replacing_local_data() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("audio.wav"), b"archived audio").unwrap();
        std::fs::write(source.path().join("metadata.json"), b"archived metadata").unwrap();
        let (manifest, _) = snapshot(&pool).await.unwrap();
        let archive = source.path().join("backup.zip");
        write_archive(&archive, manifest, vec![Some(source.path().to_path_buf())]).unwrap();

        let target = tempfile::tempdir().unwrap();
        let local = target.path().join("meeting");
        std::fs::create_dir(&local).unwrap();
        let local_metadata =
            br#"{"version":"1.0","transcript_file":"transcripts.json","meeting_id":"review-test"}"#;
        std::fs::write(local.join("metadata.json"), local_metadata).unwrap();
        sqlx::query("UPDATE meetings SET title = 'Local title', folder_path = ?")
            .bind(local.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();

        let restored = target.path().join("restored-recordings");
        let roots = vec![target.path().to_path_buf()];
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path)
             VALUES ('shared', 'Shared folder', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)",
        )
        .bind(local.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();
        let report = restore_archive(
            &pool,
            archive.clone(),
            restored.clone(),
            Some(roots.clone()),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!((report.skipped, report.repaired_audio), (1, 0));
        assert!(!local.join("audio.wav").exists());
        sqlx::query("DELETE FROM meetings WHERE id = 'shared'")
            .execute(&pool)
            .await
            .unwrap();

        let report = restore_archive(&pool, archive.clone(), restored.clone(), None, None, None)
            .await
            .unwrap();
        assert_eq!((report.skipped, report.repaired_audio), (1, 0));
        assert!(!local.join("audio.wav").exists());

        let report = restore_archive(
            &pool,
            archive.clone(),
            restored.clone(),
            Some(roots.clone()),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            (report.meetings, report.skipped, report.repaired_audio),
            (0, 0, 1)
        );
        assert_eq!(
            std::fs::read(local.join("audio.wav")).unwrap(),
            b"archived audio"
        );
        assert_eq!(
            std::fs::read(local.join("metadata.json")).unwrap(),
            local_metadata
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT title FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "Local title"
        );

        std::fs::write(local.join("audio.wav"), b"local audio").unwrap();
        let report = restore_archive(
            &pool,
            archive.clone(),
            restored.clone(),
            Some(roots.clone()),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!((report.skipped, report.repaired_audio), (1, 0));
        assert_eq!(
            std::fs::read(local.join("audio.wav")).unwrap(),
            b"local audio"
        );

        let offline = target.path().join("offline");
        let offline_folder = offline.join("meeting");
        sqlx::query("UPDATE meetings SET folder_path = ?")
            .bind(offline_folder.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        let report = restore_archive(
            &pool,
            archive.clone(),
            restored.clone(),
            Some(vec![offline]),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            (
                report.skipped,
                report.repaired_audio,
                report.unavailable_audio_locations
            ),
            (1, 0, 1)
        );
        let kept: String = sqlx::query_scalar("SELECT folder_path FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(Path::new(&kept), offline_folder);

        sqlx::query("UPDATE meetings SET folder_path = ?")
            .bind(target.path().join("missing").to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        let report = restore_archive(
            &pool,
            archive,
            restored.clone(),
            Some(roots.clone()),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(report.repaired_audio, 1);
        let folder: String = sqlx::query_scalar("SELECT folder_path FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(Path::new(&folder).starts_with(&restored));
        assert_eq!(
            std::fs::read(Path::new(&folder).join("audio.wav")).unwrap(),
            b"archived audio"
        );
        let folder = Path::new(&folder);
        assert!(folder.join("metadata.json").is_file());
        crate::library::archive::storage::remove_audio_for_test(&pool, &roots, "review-test")
            .await
            .unwrap();
        assert!(!folder.join("audio.wav").exists());
        let deletion =
            crate::database::repositories::meeting::MeetingsRepository::delete_from_library(
                &pool,
                "review-test",
                true,
            )
            .await
            .unwrap()
            .unwrap();
        let locations =
            super::super::file_cleanup::CleanupLocations::test_recordings(target.path(), roots);
        let cleanup = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM pending_meeting_file_cleanup WHERE kind = 'recording'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(deletion.cleanup_ids.contains(&cleanup));
        assert_eq!(
            super::super::file_cleanup::attempt(&pool, &locations, cleanup, None)
                .await
                .unwrap(),
            None
        );
        assert!(!folder.exists());
    }

    #[tokio::test]
    async fn restore_keeps_confirmed_audio_removal_with_or_without_repair() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("meeting");
        std::fs::create_dir(&folder).unwrap();
        let audio = folder.join("audio.wav");
        std::fs::write(&audio, b"synthetic audio").unwrap();
        std::fs::write(folder.join("metadata.json"), br#"{"version":"1.0","meeting_id":"review-test","audio_file":"audio.wav","transcript_file":"transcripts.json","status":"completed","source":"import"}"#).unwrap();
        sqlx::query("UPDATE meetings SET folder_path = ?")
            .bind(folder.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        let (manifest, folders) = snapshot(&pool).await.unwrap();
        let path = root.path().join("earlier.zip");
        write_archive(&path, manifest, folders).unwrap();
        let roots = vec![root.path().to_path_buf()];
        crate::library::archive::storage::remove_audio_for_test(&pool, &roots, "review-test")
            .await
            .unwrap();
        assert!(!audio.exists());
        let (manifest, _) = snapshot(&pool).await.unwrap();
        assert!(manifest.removed_audio.contains("review-test"));
        for repair in [None, Some(roots)] {
            let report = restore_archive(
                &pool,
                path.clone(),
                root.path().join("restored"),
                repair,
                None,
                None,
            )
            .await
            .unwrap();
            assert_eq!((report.skipped, report.repaired_audio), (1, 0));
            assert!(!audio.exists());
        }
    }

    #[tokio::test]
    async fn encrypted_library_restore_requires_authentication_before_staging() {
        let source_pool = crate::database::transcript_edits::tests::fixture().await;
        let source = tempfile::tempdir().unwrap();
        let audio = vec![0xa5; 2 * 1024 * 1024];
        std::fs::write(source.path().join("audio.wav"), &audio).unwrap();
        let (manifest, _) = snapshot(&source_pool).await.unwrap();
        let archive = source.path().join("backup.clawbackup");
        let scratch = encryption::prepare_scratch(source.path()).unwrap();
        let records = records::SnapshotFiles::temporary(source.path()).unwrap();
        let folders = vec![Some(source.path().to_path_buf())];
        let estimate = estimate_zip_size(&manifest, &folders, &records).unwrap();
        let limit = encryption::backup_scratch_limit(&scratch, &archive, estimate).unwrap();
        let mut zip = encryption::scratch_file(&scratch).unwrap();
        let report = build_archive(
            &mut zip,
            manifest,
            folders,
            None,
            None,
            None,
            false,
            Some(limit),
        )
        .unwrap();
        assert!(zip.metadata().unwrap().len() <= estimate);
        let phrase = || Zeroizing::new(["synthetic", "archive", "example"].join(" "));
        let mut output = archive_tempfile(&archive).unwrap();
        encryption::seal(zip, output.as_file_mut(), phrase()).unwrap();
        PreparedBackup { output, report }
            .publish(&archive, false)
            .unwrap();
        let original = std::fs::read(&archive).unwrap();
        assert!(!original
            .windows(b"manifest.json".len())
            .any(|bytes| bytes == b"manifest.json"));

        let dest_pool = empty_conversation_destination().await;
        let dest = tempfile::tempdir().unwrap();
        let restored = dest.path().join("restored");
        let profile = Some((dest.path(), dest.path()));
        for (data, secret, expected) in [
            (&original[..], None, encryption::PASSPHRASE_REQUIRED),
            (
                &original[..],
                Some(Zeroizing::new("wrong".into())),
                "incorrect or",
            ),
            (&original[..original.len() - 1], Some(phrase()), "damaged"),
        ] {
            std::fs::write(&archive, data).unwrap();
            let error = restore_archive(
                &dest_pool,
                archive.clone(),
                restored.clone(),
                None,
                secret,
                profile,
            )
            .await
            .err()
            .unwrap();
            assert!(error.contains(expected));
            assert!(!restored.exists());
        }
        let mut tampered = original.clone();
        tampered[1024 * 1024 + 512] ^= 1;
        std::fs::write(&archive, tampered).unwrap();
        assert!(restore_archive(
            &dest_pool,
            archive.clone(),
            restored.clone(),
            None,
            Some(phrase()),
            profile
        )
        .await
        .is_err());
        assert!(!restored.exists());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings")
                .fetch_one(&dest_pool)
                .await
                .unwrap(),
            0
        );

        std::fs::write(&archive, original).unwrap();
        let report = restore_archive(&dest_pool, archive, restored, None, Some(phrase()), profile)
            .await
            .unwrap();
        assert_eq!((report.meetings, report.files), (1, 1));
        let folder: String = sqlx::query_scalar("SELECT folder_path FROM meetings")
            .fetch_one(&dest_pool)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(Path::new(&folder).join("audio.wav")).unwrap(),
            audio
        );
    }

    #[tokio::test]
    async fn missing_folder_does_not_block_other_audio_and_restores_without_a_folder() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
    async fn restore_without_files_drops_the_inapplicable_retry_but_reports_missing_audio() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let available = root.path().join("available");
        std::fs::create_dir(&available).unwrap();
        std::fs::write(available.join("audio.wav"), b"synthetic audio").unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES ('available', 'Available audio', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
            .bind(available.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let payload = serde_json::json!({
            "retranscribed_at": "2026-01-01T00:00:00Z", "duration_seconds": 2.0,
            "audio_filename": "audio.wav", "provider": "localWhisper", "model": "tiny",
            "source_language": "de",
        })
        .to_string();
        // Both meetings have a committed retranscription whose files are unpublished.
        for meeting in ["review-test", "available"] {
            sqlx::query(
                "INSERT INTO transcript_file_sync (meeting_id, retranscription_json) VALUES (?, ?)",
            )
            .bind(meeting)
            .bind(&payload)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO recording_outcomes (meeting_id, capture_incomplete, recording_files_incomplete) VALUES (?, 1, 1)")
                .bind(meeting).execute(&pool).await.unwrap();
        }
        // 'review-test' has no recording folder, so the archive carries no files for it.
        let (manifest, folders) = snapshot(&pool).await.unwrap();
        let archive = root.path().join("backup.zip");
        write_archive(&archive, manifest, folders).unwrap();
        let target = tempfile::tempdir().unwrap();
        let (manifest, stage) = unpack(&archive, target.path(), &HashSet::new()).unwrap();
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        let restored = import_manifest(&pool, manifest, stage).await.unwrap();
        assert_eq!(restored.meetings, 2);

        let state = |meeting: &'static str| {
            let pool = pool.clone();
            async move {
                let rows: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM transcript_file_sync WHERE meeting_id = ?",
                )
                .bind(meeting)
                .fetch_one(&pool)
                .await
                .unwrap();
                let flags: (bool, bool) = sqlx::query_as("SELECT recording_files_incomplete, capture_incomplete FROM recording_outcomes WHERE meeting_id = ?")
                    .bind(meeting).fetch_one(&pool).await.unwrap();
                (rows, flags)
            }
        };
        // No folder: nothing to retry. The capture gap is preserved as archived.
        assert_eq!(state("review-test").await, (0, (false, true)));
        crate::database::transcript_edits::sync_file(&pool, "review-test", &[])
            .await
            .unwrap();
        // With files the retry stays pending until it succeeds.
        assert_eq!(state("available").await, (1, (true, true)));
        // Missing audio is reported from what the archive lacked, not from the flag.
        assert_eq!(restored.incomplete_meetings.len(), 1);
        let warning = &restored.incomplete_meetings[0];
        assert_eq!(warning.meeting_id, "review-test");
        assert!(warning.audio.audio_unavailable && !warning.audio.recovery_files_excluded);
        let folder: Option<String> =
            sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'review-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(folder, None);
    }

    #[tokio::test]
    async fn unreadable_folder_names_meeting_and_preserves_previous_backup() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        use crate::audio::incremental_saver::{
            recover_audio_in_folder, release_registered_capture,
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
            let recovered = recover_audio_in_folder(folder.clone()).await.unwrap();
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
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("backup.zip");
        let (mut manifest, folders) = snapshot(&pool).await.unwrap();
        manifest.tables.remove("meeting_tags");
        manifest.tables.remove("meeting_bookmarks");
        manifest.tables.remove("meeting_summary_bookmarks");
        write_archive(&archive, manifest, folders).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let legacy_json: Value =
            serde_json::from_reader(zip.by_name("manifest.json").unwrap()).unwrap();
        assert!(legacy_json.get("audio_excluded").is_none());
        drop(zip);
        let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
        assert!(!manifest.audio_excluded);
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        let report = import_manifest(&pool, manifest, stage).await.unwrap();
        assert_eq!(report.meetings, 1);
        assert!(!report.audio_excluded);
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
    async fn restore_preserves_meeting_summary_bookmark_consent() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("INSERT INTO meeting_summary_bookmarks (meeting_id) SELECT id FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("backup.zip");
        let (manifest, folders) = snapshot(&pool).await.unwrap();
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
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meeting_summary_bookmarks")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn skipped_folder_cleanup_failure_rolls_back_import() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
        assert!(allowed_file(crate::audio::import::IMPORT_OWNER_FILE));
    }
    #[tokio::test]
    async fn calendar_and_recording_facts_round_trip_and_older_archives_still_restore() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let link = super::super::calendar::tests::sample();
        super::super::calendar::write(&pool, "review-test", Some(link.clone()), false)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE meetings SET recording_source = 'import', recording_duration_s = 93.5
             WHERE id = 'review-test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        for legacy in [false, true] {
            let (mut manifest, folders) = snapshot(&pool).await.unwrap();
            if legacy {
                let row = &mut manifest.tables.get_mut("meetings").unwrap()[0];
                for column in ["calendar_link", "recording_source", "recording_duration_s"] {
                    row.remove(column);
                }
            }
            let root = tempfile::tempdir().unwrap();
            let archive = root.path().join("calendar.zip");
            write_archive(&archive, manifest, folders).unwrap();
            sqlx::query("DELETE FROM meetings")
                .execute(&pool)
                .await
                .unwrap();
            let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
            import_manifest(&pool, manifest, stage).await.unwrap();
            assert_eq!(
                super::super::calendar::read(&pool, "review-test")
                    .await
                    .unwrap(),
                (!legacy).then(|| link.clone())
            );
            let facts: (Option<String>, Option<f64>) = sqlx::query_as(
                "SELECT recording_source, recording_duration_s FROM meetings WHERE id = 'review-test'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                facts,
                (
                    (!legacy).then(|| "import".into()),
                    (!legacy).then_some(93.5)
                )
            );
        }
    }

    #[tokio::test]
    async fn an_unreadable_calendar_link_does_not_block_a_restore() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        for unreadable in ["{not json", "{\"eventId\": \"two words\"}"] {
            let (mut manifest, folders) = snapshot(&pool).await.unwrap();
            manifest.tables.get_mut("meetings").unwrap()[0]
                .insert("calendar_link".into(), Value::String(unreadable.into()));
            let root = tempfile::tempdir().unwrap();
            let archive = root.path().join("calendar.zip");
            write_archive(&archive, manifest, folders).unwrap();
            sqlx::query("DELETE FROM meetings")
                .execute(&pool)
                .await
                .unwrap();
            let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
            let report = import_manifest(&pool, manifest, stage).await.unwrap();
            assert_eq!(report.meetings, 1);
            assert_eq!(report.unreadable_calendar_links, 1);
            assert!(super::super::calendar::read(&pool, "review-test")
                .await
                .unwrap()
                .is_none());
            let transcripts: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM transcripts WHERE meeting_id = 'review-test'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(transcripts > 0);
        }
    }

    #[tokio::test]
    async fn context_round_trip_accepts_older_archives_and_is_deleted_with_the_meeting() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
    async fn restored_recovery_location_preserves_flags_and_accepts_legacy_archives() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        // Metadata can be absent in old archives, explicitly false, or true.
        for excluded in [None, Some(false), Some(true)] {
            for archived_outcome in [false, true] {
                let pool = crate::database::transcript_edits::tests::fixture().await;
                if archived_outcome {
                    sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete) VALUES ('review-test', 1, 1)")
                        .execute(&pool).await.unwrap();
                }
                let (mut manifest, _) = snapshot(&pool).await.unwrap();
                // Exercise archives from before the new outcome column existed.
                for outcome in manifest.tables.get_mut("recording_outcomes").unwrap() {
                    outcome.remove("recovery_files_elsewhere");
                }
                if let Some(excluded) = excluded {
                    manifest.incomplete_audio.insert(
                        0,
                        IncompleteAudio {
                            recovery_files_excluded: excluded,
                            audio_unavailable: true,
                            ..Default::default()
                        },
                    );
                }
                let root = tempfile::tempdir().unwrap();
                let archive = root.path().join("recovery.zip");
                // Construct the archive directly so backup detection cannot replace the metadata.
                let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
                zip.start_file("manifest.json", SimpleFileOptions::default())
                    .unwrap();
                let mut json = serde_json::to_value(&manifest).unwrap();
                if excluded.is_none() {
                    json.as_object_mut().unwrap().remove("incomplete_audio");
                }
                zip.write_all(&serde_json::to_vec(&json).unwrap()).unwrap();
                zip.finish().unwrap();
                sqlx::query("DELETE FROM meetings")
                    .execute(&pool)
                    .await
                    .unwrap();
                let (manifest, stage) = unpack(&archive, root.path(), &HashSet::new()).unwrap();
                import_manifest(&pool, manifest, stage).await.unwrap();
                let outcome: Option<crate::audio::outcome::RecordingOutcome> = sqlx::query_as(
                    "SELECT * FROM recording_outcomes WHERE meeting_id = 'review-test'",
                )
                .fetch_optional(&pool)
                .await
                .unwrap();
                if archived_outcome || excluded == Some(true) {
                    let outcome = outcome.unwrap();
                    assert_eq!(outcome.recovery_files_elsewhere, excluded == Some(true));
                    assert_eq!(outcome.audio_save_failed, archived_outcome);
                    assert_eq!(outcome.transcription_incomplete, archived_outcome);
                } else {
                    assert!(outcome.is_none());
                }
            }
        }
    }

    #[tokio::test]
    async fn round_trip_is_additive_and_excludes_credentials() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
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

    // ── export-history merge + rollback ──────────────────────────────────────

    #[tokio::test]
    async fn fresh_db_with_leftover_ledger_merges_instead_of_aborting() {
        use crate::exports::ledger::{meeting_dir, task_key, ExportLedger};
        use crate::exports::model::ExportStatus;

        const SCOPE: &str = "planner:tenant-t:user-u:plan-p:bucket-b";
        let key1 = task_key(SCOPE, "m-merge", "11111111-1111-1111-1111-111111111111", "");
        let key2 = task_key(SCOPE, "m-merge", "22222222-2222-2222-2222-222222222222", "");

        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let source = empty_conversation_destination().await;
        sqlx::query(
            "INSERT INTO meetings(id,title,created_at,updated_at) VALUES \
             ('m-merge','Merge test','2026-11-20','2026-11-20')",
        )
        .execute(&source)
        .await
        .unwrap();

        // Build an archive with one succeeded entry for the meeting.
        let profile = tempfile::tempdir().unwrap();
        let profile_dir = meeting_dir(profile.path(), "m-merge");
        let mut archive_ledger = ExportLedger::load_or_new(&profile_dir, "m-merge").unwrap();
        archive_ledger.record_success(&key1, Some("task-archive".into()), None, None);
        archive_ledger.checkpoint().await.unwrap();

        let (mut manifest, folders) = snapshot(&source).await.unwrap();
        archive_exports::capture(&mut manifest, profile.path()).unwrap();
        let archive = profile.path().join("m-merge.zip");
        write_archive(&archive, manifest, folders).unwrap();

        // Simulate a fresh DB that has a leftover ledger from a previous partial
        // restore (a different entry for the same meeting).
        let fresh = tempfile::tempdir().unwrap();
        let leftover_dir = meeting_dir(fresh.path(), "m-merge");
        let mut leftover = ExportLedger::load_or_new(&leftover_dir, "m-merge").unwrap();
        leftover.record_failure(&key2, ExportStatus::Failed, Some("timeout".into()), None);
        leftover.checkpoint().await.unwrap();

        let restored = empty_conversation_destination().await;
        let (manifest, stage) =
            unpack(&archive, &fresh.path().join("recordings"), &HashSet::new()).unwrap();
        // Should succeed even though a leftover ledger file already exists.
        import_manifest_impl(&restored, manifest, stage, None, Some(fresh.path()))
            .await
            .unwrap();

        // Merged ledger must contain both entries.
        let merged =
            ExportLedger::load_or_new(&meeting_dir(fresh.path(), "m-merge"), "m-merge").unwrap();
        assert_eq!(merged.entry(&key1).unwrap().status, ExportStatus::Succeeded);
        assert_eq!(merged.entry(&key2).unwrap().status, ExportStatus::Failed);
    }

    #[tokio::test]
    async fn ledger_rollback_on_db_commit_failure_restores_previous_content() {
        use crate::exports::ledger::{meeting_dir, task_key, ExportLedger};
        use crate::exports::model::ExportStatus;

        const SCOPE: &str = "planner:tenant-t:user-u:plan-p:bucket-b";
        let key1 = task_key(
            SCOPE,
            "m-rollback",
            "11111111-1111-1111-1111-111111111111",
            "",
        );
        let key2 = task_key(
            SCOPE,
            "m-rollback",
            "22222222-2222-2222-2222-222222222222",
            "",
        );

        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let source = empty_conversation_destination().await;
        sqlx::query(
            "INSERT INTO meetings(id,title,created_at,updated_at) VALUES \
             ('m-rollback','Rollback test','2026-11-20','2026-11-20')",
        )
        .execute(&source)
        .await
        .unwrap();

        // Archive has entry key1 (succeeded).
        let profile = tempfile::tempdir().unwrap();
        let profile_dir = meeting_dir(profile.path(), "m-rollback");
        let mut ledger = ExportLedger::load_or_new(&profile_dir, "m-rollback").unwrap();
        ledger.record_success(&key1, Some("task-orig".into()), None, None);
        ledger.checkpoint().await.unwrap();

        let (mut manifest, folders) = snapshot(&source).await.unwrap();
        archive_exports::capture(&mut manifest, profile.path()).unwrap();
        let archive = profile.path().join("m-rollback.zip");
        write_archive(&archive, manifest, folders).unwrap();

        // Destination has leftover ledger with a different entry.
        let dest_dir = tempfile::tempdir().unwrap();
        let leftover_dir = meeting_dir(dest_dir.path(), "m-rollback");
        let mut leftover = ExportLedger::load_or_new(&leftover_dir, "m-rollback").unwrap();
        leftover.record_failure(&key2, ExportStatus::Failed, None, None);
        leftover.checkpoint().await.unwrap();
        let prev_bytes =
            std::fs::read(leftover_dir.join(crate::exports::ledger::LEDGER_FILENAME)).unwrap();

        // Use a DB that will reject the commit (FK violation trigger).
        let rejected_pool = empty_conversation_destination().await;
        sqlx::query("CREATE TABLE rollback_fixture_fail(x TEXT REFERENCES meetings(id) DEFERRABLE INITIALLY DEFERRED)")
            .execute(&rejected_pool).await.unwrap();
        sqlx::query("CREATE TRIGGER rollback_fixture_trigger AFTER INSERT ON meetings BEGIN INSERT INTO rollback_fixture_fail VALUES ('no-parent'); END")
            .execute(&rejected_pool).await.unwrap();

        let (manifest, stage) = unpack(
            &archive,
            &dest_dir.path().join("recordings"),
            &HashSet::new(),
        )
        .unwrap();
        assert!(
            import_manifest_impl(&rejected_pool, manifest, stage, None, Some(dest_dir.path()))
                .await
                .is_err()
        );

        // After rollback the ledger file must be restored to its previous content.
        let after_rollback =
            std::fs::read(leftover_dir.join(crate::exports::ledger::LEDGER_FILENAME)).unwrap();
        assert_eq!(after_rollback, prev_bytes);
    }

    #[tokio::test]
    async fn same_profile_restore_is_idempotent() {
        use crate::exports::ledger::{meeting_dir, task_key, ExportLedger};

        const SCOPE: &str = "planner:tenant-t:user-u:plan-p:bucket-b";
        let key1 = task_key(SCOPE, "m-idem", "11111111-1111-1111-1111-111111111111", "");

        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let source = empty_conversation_destination().await;
        sqlx::query(
            "INSERT INTO meetings(id,title,created_at,updated_at) VALUES \
             ('m-idem','Idempotent test','2026-11-20','2026-11-20')",
        )
        .execute(&source)
        .await
        .unwrap();

        let profile = tempfile::tempdir().unwrap();
        let profile_dir = meeting_dir(profile.path(), "m-idem");
        let mut ledger = ExportLedger::load_or_new(&profile_dir, "m-idem").unwrap();
        ledger.record_success(&key1, Some("task".into()), None, None);
        ledger.checkpoint().await.unwrap();

        let (mut manifest, folders) = snapshot(&source).await.unwrap();
        archive_exports::capture(&mut manifest, profile.path()).unwrap();
        let archive = profile.path().join("m-idem.zip");
        write_archive(&archive, manifest, folders).unwrap();

        // First restore succeeds.
        let fresh = tempfile::tempdir().unwrap();
        let dest1 = empty_conversation_destination().await;
        let (manifest, stage) =
            unpack(&archive, &fresh.path().join("recordings"), &HashSet::new()).unwrap();
        import_manifest_impl(&dest1, manifest, stage, None, Some(fresh.path()))
            .await
            .unwrap();

        // Second restore into the same DB (meeting already exists) succeeds
        // without touching the ledger file or reporting an error.
        let (manifest2, stage2) =
            unpack(&archive, &fresh.path().join("recordings2"), &HashSet::new()).unwrap();
        let report = import_manifest_impl(&dest1, manifest2, stage2, None, Some(fresh.path()))
            .await
            .unwrap();
        assert_eq!(report.meetings, 0);
        assert_eq!(report.skipped, 1);
    }
}
