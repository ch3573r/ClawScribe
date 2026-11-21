//! Reviewed transcript corrections. SQLite is authoritative; edits preserve timing
//! and the original text, invalidate word alignment, and can be undone atomically.
use crate::audio::outcome::RecordingOutcome;
use crate::{database::models::Transcript, state::AppState};
use regex::{NoExpand, RegexBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Runtime};

#[derive(Clone, Serialize, Deserialize)]
struct Change {
    id: String,
    before: String,
    after: String,
    original: Option<String>,
    words: Option<String>,
}

#[derive(Serialize)]
pub struct ReplacePreview {
    token: String,
    matches: usize,
    segments: usize,
    examples: Vec<ReplaceExample>,
}

#[derive(Serialize)]
pub struct ReplaceExample {
    before: String,
    after: String,
}

#[derive(Serialize)]
pub struct EditResult {
    changed: usize,
    file_warning: bool,
}

fn error(_: impl std::fmt::Display) -> String {
    "Could not update the transcript database".into()
}

fn replacement_plan(
    rows: Vec<Transcript>,
    query: &str,
    replacement: &str,
    match_case: bool,
) -> Result<(ReplacePreview, Vec<Change>), String> {
    if query.is_empty() || query.len() > 1000 || replacement.len() > 4000 {
        return Err(
            "Enter a search term up to 1,000 bytes and a replacement up to 4,000 bytes".into(),
        );
    }
    let pattern = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(!match_case)
        .build()
        .map_err(|_| "Invalid search term")?;
    let mut changes = Vec::new();
    let mut matches = 0;
    for row in rows {
        let count = pattern.find_iter(&row.transcript).count();
        if count == 0 {
            continue;
        }
        let after = pattern
            .replace_all(&row.transcript, NoExpand(replacement))
            .into_owned();
        if after == row.transcript {
            continue;
        }
        validate_text(&after)?;
        matches += count;
        changes.push(Change {
            id: row.id,
            before: row.transcript,
            after,
            original: row.original_transcript,
            words: row.word_timestamps_json,
        });
    }
    let serialized = serde_json::to_vec(&changes).map_err(error)?;
    let token = format!("{:x}", Sha256::digest(&serialized));
    let examples = changes
        .iter()
        .take(5)
        .map(|change| ReplaceExample {
            before: change.before.clone(),
            after: change.after.clone(),
        })
        .collect();
    Ok((
        ReplacePreview {
            token,
            matches,
            segments: changes.len(),
            examples,
        },
        changes,
    ))
}

fn validate_text(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err(
            "Transcript segments cannot be empty. Keep the spoken words or undo the edit.".into(),
        );
    }
    if text.len() > 64 * 1024 {
        return Err("A transcript segment must be smaller than 64 KiB".into());
    }
    Ok(())
}

async fn rows(pool: &SqlitePool, meeting: &str) -> Result<Vec<Transcript>, String> {
    sqlx::query_as("SELECT * FROM transcripts WHERE meeting_id = ? ORDER BY id")
        .bind(meeting)
        .fetch_all(pool)
        .await
        .map_err(error)
}

pub(crate) async fn assert_editable(
    conn: &mut SqliteConnection,
    meeting: &str,
) -> Result<(), String> {
    if crate::audio::diarization::is_diarizing_meeting(meeting) {
        return Err("Wait for speaker detection to finish before editing its transcript".into());
    }
    let processing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM summary_processes WHERE meeting_id = ? AND LOWER(status) IN ('pending', 'processing', 'summarizing', 'regenerating')")
        .bind(meeting).fetch_one(&mut *conn).await.map_err(error)?;
    if processing > 0 {
        return Err("Wait for summary generation to finish before editing its transcript".into());
    }
    Ok(())
}

async fn apply(pool: &SqlitePool, meeting: &str, changes: &[Change]) -> Result<(), String> {
    if changes.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await.map_err(error)?;
    assert_editable(&mut tx, meeting).await?;
    for change in changes {
        validate_text(&change.after)?;
        let result = sqlx::query("UPDATE transcripts SET original_transcript = COALESCE(original_transcript, transcript), transcript = ?, word_timestamps_json = NULL WHERE meeting_id = ? AND id = ? AND transcript = ?")
            .bind(&change.after).bind(meeting).bind(&change.id).bind(&change.before).execute(&mut *tx).await.map_err(error)?;
        if result.rows_affected() != 1 {
            return Err(
                "The transcript changed. Reload it and review the correction again.".into(),
            );
        }
    }
    sqlx::query("INSERT INTO transcript_edit_batches (id, meeting_id, created_at, changes_json) VALUES (?, ?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(meeting).bind(chrono::Utc::now().to_rfc3339())
        .bind(serde_json::to_string(changes).map_err(error)?).execute(&mut *tx).await.map_err(error)?;
    sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
        .bind(chrono::Utc::now())
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    sqlx::query("INSERT OR IGNORE INTO transcript_file_sync (meeting_id) VALUES (?)")
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    tx.commit().await.map_err(error)
}

async fn undo(pool: &SqlitePool, meeting: &str) -> Result<usize, String> {
    let mut tx = pool.begin().await.map_err(error)?;
    assert_editable(&mut tx, meeting).await?;
    let batch: Option<(String, String)> = sqlx::query_as("SELECT id, changes_json FROM transcript_edit_batches WHERE meeting_id = ? AND undone = 0 ORDER BY rowid DESC LIMIT 1")
        .bind(meeting).fetch_optional(&mut *tx).await.map_err(error)?;
    let (id, json) = batch.ok_or("No transcript correction is available to undo")?;
    let changes: Vec<Change> = serde_json::from_str(&json).map_err(error)?;
    for change in &changes {
        let result = sqlx::query("UPDATE transcripts SET transcript = ?, original_transcript = ?, word_timestamps_json = ? WHERE meeting_id = ? AND id = ? AND transcript = ?")
            .bind(&change.before).bind(&change.original).bind(&change.words).bind(meeting).bind(&change.id).bind(&change.after).execute(&mut *tx).await.map_err(error)?;
        if result.rows_affected() != 1 {
            return Err("This correction cannot be undone because the transcript was replaced or changed. Your current transcript is intact.".into());
        }
    }
    sqlx::query("UPDATE transcript_edit_batches SET undone = 1 WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    sqlx::query("INSERT OR IGNORE INTO transcript_file_sync (meeting_id) VALUES (?)")
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
        .bind(chrono::Utc::now())
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    tx.commit().await.map_err(error)?;
    Ok(changes.len())
}

pub(crate) async fn restore_previous(
    pool: &SqlitePool,
    meeting: &str,
    roots: &[PathBuf],
) -> Result<usize, String> {
    let files = file_provenance(pool, meeting, roots).await;
    let mut tx = pool.begin().await.map_err(error)?;
    assert_editable(&mut tx, meeting).await?;
    let revision: Option<(String, Option<String>)> = sqlx::query_as("SELECT transcripts_json, provenance_json FROM transcript_revisions WHERE meeting_id = ? ORDER BY rowid DESC LIMIT 1")
        .bind(meeting).fetch_optional(&mut *tx).await.map_err(error)?;
    let (json, restored) = revision.ok_or("No previous transcript is available")?;
    let previous: Vec<Transcript> = serde_json::from_str(&json).map_err(error)?;
    if previous.iter().any(|row| row.meeting_id != meeting) {
        return Err("The archived transcript does not belong to this meeting".into());
    }
    let current: Vec<Transcript> =
        sqlx::query_as("SELECT * FROM transcripts WHERE meeting_id = ? ORDER BY id")
            .bind(meeting)
            .fetch_all(&mut *tx)
            .await
            .map_err(error)?;
    // The replaced text keeps its own provenance so a second restore can undo this one.
    let archived = revision_provenance(&mut tx, meeting, files).await?;
    sqlx::query("INSERT INTO transcript_revisions (id, meeting_id, created_at, transcripts_json, provenance_json) VALUES (?, ?, ?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(meeting).bind(chrono::Utc::now().to_rfc3339())
        .bind(serde_json::to_string(&current).map_err(error)?).bind(archived).execute(&mut *tx).await.map_err(error)?;
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    for row in &previous {
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, summary, action_items, key_points, audio_start_time, audio_end_time, duration, speaker, word_timestamps_json, original_transcript) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&row.id).bind(meeting).bind(&row.transcript).bind(&row.timestamp)
            .bind(&row.summary).bind(&row.action_items).bind(&row.key_points)
            .bind(row.audio_start_time).bind(row.audio_end_time).bind(row.duration)
            .bind(&row.speaker).bind(&row.word_timestamps_json).bind(&row.original_transcript)
            .execute(&mut *tx).await.map_err(error)?;
    }
    sqlx::query("UPDATE transcript_edit_batches SET undone = 1 WHERE meeting_id = ?")
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    // Any unpublished retranscription payload described the text just replaced.
    // Replace it with the restored text's own provenance, or "unknown" when it
    // was never recorded. It is never reassigned from the replaced revision.
    let has_folder =
        sqlx::query_scalar::<_, Option<String>>("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting)
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?
            .is_some_and(|folder| !folder.trim().is_empty());
    let publication = has_folder
        .then(|| {
            let restored_provenance = restored
                .as_deref()
                .and_then(|json| serde_json::from_str::<Provenance>(json).ok())
                .map_or(RestoredProvenance::Unknown, RestoredProvenance::Known);
            serde_json::to_string(&FilePublication::Restored(RestoredFileUpdate {
                restored_provenance,
            }))
        })
        .transpose()
        .map_err(error)?;
    sqlx::query("INSERT INTO transcript_file_sync (meeting_id, retranscription_json) VALUES (?, ?)
                 ON CONFLICT(meeting_id) DO UPDATE SET retranscription_json = excluded.retranscription_json")
        .bind(meeting)
        .bind(publication)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    if has_folder {
        // Files stay flagged until the restored metadata is published. Capture
        // gaps and the other recorded outcomes are left exactly as they were.
        sqlx::query(
            "INSERT INTO recording_outcomes (meeting_id, recording_files_incomplete) VALUES (?, 1)
             ON CONFLICT(meeting_id) DO UPDATE SET recording_files_incomplete = 1",
        )
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    }
    sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
        .bind(chrono::Utc::now())
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    tx.commit().await.map_err(error)?;
    Ok(previous.len())
}

/// Transcription settings that produced stored transcript text. A transcript
/// without a recorded provenance is "unknown", never the current provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Provenance {
    pub provider: String,
    pub model: String,
    pub source_language: Option<String>,
    pub retranscribed_at: Option<String>,
}

impl From<&RetranscriptionFileUpdate> for Provenance {
    fn from(update: &RetranscriptionFileUpdate) -> Self {
        Self {
            provider: update.provider.clone(),
            model: update.model.clone(),
            source_language: update.source_language.clone(),
            retranscribed_at: Some(update.retranscribed_at.clone()),
        }
    }
}

#[derive(Serialize, Deserialize)]
enum RestoredProvenance {
    Known(Provenance),
    Unknown,
}

#[derive(Serialize, Deserialize)]
struct RestoredFileUpdate {
    restored_provenance: RestoredProvenance,
}

/// The pending file publication stored with the retry marker. Payloads written
/// before restores existed are plain retranscription updates and still parse.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum FilePublication {
    Import(ImportFileUpdate),
    Restored(RestoredFileUpdate),
    Retranscription(RetranscriptionFileUpdate),
}

impl FilePublication {
    fn provenance(&self) -> Option<Provenance> {
        match self {
            Self::Retranscription(update) => Some(update.into()),
            Self::Restored(RestoredFileUpdate {
                restored_provenance: RestoredProvenance::Known(provenance),
            }) => Some(provenance.clone()),
            Self::Restored(_) | Self::Import(_) => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ImportFileUpdate {
    import_metadata: serde_json::Value,
    transcript_pending: bool,
}

pub(crate) fn import_file_payload(metadata: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(&FilePublication::Import(ImportFileUpdate {
        import_metadata: metadata.clone(),
        transcript_pending: false,
    }))
    .map_err(error)
}

pub(crate) async fn import_file_stage(
    pool: &SqlitePool,
    meeting: &str,
) -> Result<Option<&'static str>, String> {
    let payload = sqlx::query_scalar::<_, Option<String>>(
        "SELECT retranscription_json FROM transcript_file_sync WHERE meeting_id = ?",
    )
    .bind(meeting)
    .fetch_optional(pool)
    .await
    .map_err(error)?
    .flatten();
    let publication = payload
        .as_deref()
        .map(serde_json::from_str::<FilePublication>)
        .transpose()
        .map_err(error)?;
    Ok(match publication {
        Some(FilePublication::Import(update)) if update.transcript_pending => Some("transcript"),
        Some(FilePublication::Import(_)) => Some("metadata"),
        _ => None,
    })
}

fn guarded_import_folder(
    folder: &str,
    meeting: &str,
    roots: &[PathBuf],
    others: &[String],
) -> Result<PathBuf, String> {
    const REFUSED: &str = "The import folder could not be verified for this meeting.";
    let path = Path::new(folder);
    let canonical = crate::database::meeting_files::resolve_recording_folder(path, roots)?;
    if !crate::audio::import::owns_import_folder(path, meeting)
        || others.iter().any(|other| {
            crate::local_path::comparable_text(other) == crate::local_path::comparable_text(folder)
                || (crate::trusted_roots::text_allowed(other, roots)
                    && Path::new(other).canonicalize().is_ok_and(|other| {
                        other == canonical
                            || other.starts_with(&canonical)
                            || canonical.starts_with(other)
                    }))
        })
    {
        return Err(REFUSED.into());
    }
    let folder = crate::database::meeting_files::plain_path(&canonical);
    match std::fs::symlink_metadata(folder.join("metadata.json")) {
        Ok(_)
            if !crate::database::meeting_files::read_marker(&folder).is_some_and(|marker| {
                crate::database::meeting_files::stamped_meeting_id(&marker) == Some(meeting)
            }) =>
        {
            Err(REFUSED.into())
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(REFUSED.into()),
        _ => Ok(folder),
    }
}

fn publish_import_metadata(
    folder: &Path,
    meeting: &str,
    metadata: &serde_json::Value,
) -> Result<(), String> {
    let audio = crate::library::archive::storage::owned_audio_name(metadata, meeting)
        .ok_or("Missing import audio name")?;
    if !std::fs::symlink_metadata(folder.join(audio)).is_ok_and(|meta| meta.is_file()) {
        return Err("The saved import audio could not be verified".into());
    }
    let destination = folder.join("metadata.json");
    if !destination.is_file() {
        publish_json(&destination, metadata)?;
    }
    Ok(())
}

impl RestoredFileUpdate {
    fn write_metadata(&self, folder: &Path) -> Result<(), String> {
        let path = folder.join("metadata.json");
        let mut metadata: serde_json::Value = match std::fs::symlink_metadata(&path) {
            Ok(stat) if stat.is_file() => {
                serde_json::from_slice(&std::fs::read(&path).map_err(error)?).map_err(error)?
            }
            // Nothing on disk names a provider, so there is nothing to correct.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            _ => return Err("Could not read recording metadata".into()),
        };
        let fields = metadata
            .as_object_mut()
            .ok_or("Invalid recording metadata")?;
        match &self.restored_provenance {
            RestoredProvenance::Known(provenance) => {
                fields.insert(
                    "transcription_provider".into(),
                    serde_json::json!(provenance.provider),
                );
                fields.insert(
                    "transcription_model".into(),
                    serde_json::json!(provenance.model),
                );
                for (key, value) in [
                    (
                        "transcription_source_language",
                        provenance.source_language.as_deref(),
                    ),
                    ("retranscribed_at", provenance.retranscribed_at.as_deref()),
                ] {
                    match value {
                        Some(value) => {
                            fields.insert(key.into(), serde_json::json!(value));
                        }
                        None => {
                            fields.remove(key);
                        }
                    }
                }
                fields.remove("transcription_provenance");
            }
            RestoredProvenance::Unknown => {
                for key in [
                    "transcription_provider",
                    "transcription_model",
                    "transcription_source_language",
                    "retranscribed_at",
                ] {
                    fields.remove(key);
                }
                fields.insert(
                    "transcription_provenance".into(),
                    serde_json::json!("unknown"),
                );
            }
        }
        publish_json(&path, &metadata)
    }
}

/// Provenance recorded in the recording folder's metadata, if any. Absent,
/// unreadable or marked-unknown metadata yields `None`.
pub(crate) async fn file_provenance(
    pool: &SqlitePool,
    meeting: &str,
    roots: &[PathBuf],
) -> Option<Provenance> {
    let folder =
        sqlx::query_scalar::<_, Option<String>>("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten()
            .filter(|value| !value.trim().is_empty())?;
    let roots = roots.to_vec();
    tauri::async_runtime::spawn_blocking(move || {
        let folder = crate::trusted_roots::resolve_allowed(&folder, &roots).ok()?;
        read_file_provenance(&crate::database::meeting_files::plain_path(&folder))
    })
    .await
    .ok()
    .flatten()
}

fn read_file_provenance(folder: &Path) -> Option<Provenance> {
    let path = folder.join("metadata.json");
    if !std::fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    let text = |key: &str| {
        metadata
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Some(Provenance {
        provider: text("transcription_provider")?,
        model: text("transcription_model")?,
        source_language: text("transcription_source_language"),
        retranscribed_at: text("retranscribed_at"),
    })
}

/// The provenance to store with a revision that archives the current text: an
/// unpublished payload describes the current rows; otherwise the recording
/// folder's metadata does. `None` means unknown.
pub(crate) async fn revision_provenance(
    conn: &mut SqliteConnection,
    meeting: &str,
    files: Option<Provenance>,
) -> Result<Option<String>, String> {
    let payload = sqlx::query_scalar::<_, Option<String>>(
        "SELECT retranscription_json FROM transcript_file_sync WHERE meeting_id = ?",
    )
    .bind(meeting)
    .fetch_optional(&mut *conn)
    .await
    .map_err(error)?
    .flatten();
    let provenance = match payload {
        Some(json) => serde_json::from_str::<FilePublication>(&json)
            .map_err(error)?
            .provenance(),
        None => files,
    };
    provenance
        .map(|value| serde_json::to_string(&value))
        .transpose()
        .map_err(error)
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RetranscriptionFileUpdate {
    pub retranscribed_at: String,
    pub duration_seconds: f64,
    pub audio_filename: String,
    pub provider: String,
    pub model: String,
    pub source_language: Option<String>,
}

impl RetranscriptionFileUpdate {
    fn write_metadata(&self, folder: &Path, meeting: &str) -> Result<(), String> {
        let path = folder.join("metadata.json");
        let mut metadata = match std::fs::symlink_metadata(&path) {
            Ok(stat) if stat.is_file() => {
                serde_json::from_slice(&std::fs::read(&path).map_err(error)?).map_err(error)?
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => serde_json::json!({
                "version": "1.0",
                "meeting_id": meeting,
                "created_at": self.retranscribed_at,
                "completed_at": self.retranscribed_at,
                "source": "retranscription",
            }),
            _ => return Err("Could not read recording metadata".into()),
        };
        let original_audio = crate::library::archive::storage::owned_audio_name(&metadata, meeting);
        let fields = metadata
            .as_object_mut()
            .ok_or("Invalid recording metadata")?;
        if let Some(name) = original_audio {
            fields.insert("archive_saved_audio".into(), serde_json::json!(name));
        }
        let changes = serde_json::Map::from_iter([
            (
                "retranscribed_at".into(),
                serde_json::json!(self.retranscribed_at),
            ),
            ("status".into(), serde_json::json!("completed")),
            ("audio_file".into(), serde_json::json!(self.audio_filename)),
            (
                "duration_seconds".into(),
                serde_json::json!(self.duration_seconds),
            ),
            (
                "transcript_file".into(),
                serde_json::json!("transcripts.json"),
            ),
            (
                "transcription_provider".into(),
                serde_json::json!(self.provider),
            ),
            ("transcription_model".into(), serde_json::json!(self.model)),
        ]);
        fields.extend(changes);
        fields.remove("detected_summary_language");
        fields.remove("transcription_provenance");
        if let Some(language) = &self.source_language {
            fields.insert(
                "transcription_source_language".into(),
                serde_json::json!(language),
            );
        } else {
            fields.remove("transcription_source_language");
        }
        publish_json(&path, &metadata)
    }
}

fn publish_json(path: &Path, data: &serde_json::Value) -> Result<(), String> {
    use std::io::Write;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("Missing recording folder")?)
            .map_err(error)?;
    serde_json::to_writer_pretty(&mut file, data).map_err(error)?;
    file.flush().map_err(error)?;
    file.as_file().sync_all().map_err(error)?;
    file.persist(path).map_err(error)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SyncedFiles {
    Other,
    Retranscription,
}

#[cfg(test)]
pub(crate) async fn sync_file(
    pool: &SqlitePool,
    meeting: &str,
    roots: &[PathBuf],
) -> Result<SyncedFiles, String> {
    sync_file_with_profile(pool, meeting, roots, |_| {
        Ok(Some("synthetic-profile".into()))
    })
    .await
}

// The calling job holds the inference permit through publication. Only the
// caller that publishes a pending retranscription can wake automatic notes.
pub(crate) async fn sync_file_with_profile(
    pool: &SqlitePool,
    meeting: &str,
    roots: &[PathBuf],
    notes_profile: impl FnOnce(&Path) -> Result<Option<String>, String>,
) -> Result<SyncedFiles, String> {
    let Some(payload) = sqlx::query_scalar::<_, Option<String>>(
        "SELECT retranscription_json FROM transcript_file_sync WHERE meeting_id = ?",
    )
    .bind(meeting)
    .fetch_optional(pool)
    .await
    .map_err(error)?
    else {
        return Ok(SyncedFiles::Other);
    };
    let mut publication: Option<FilePublication> = payload
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(error)?;
    let synced = match publication {
        Some(FilePublication::Retranscription(_)) => SyncedFiles::Retranscription,
        _ => SyncedFiles::Other,
    };
    let folder: Option<String> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting)
            .fetch_one(pool)
            .await
            .map_err(error)?;
    let Some(folder) = folder.filter(|value| !value.trim().is_empty()) else {
        // Without a recording folder (for example a library restored without
        // its files) no file can ever be written. Retrying would fail forever,
        // so the file work is dropped. The missing audio is reported by the
        // meeting having no recording folder, not by a retry that cannot finish.
        let mut tx = pool.begin().await.map_err(error)?;
        sqlx::query(
            "UPDATE recording_outcomes SET recording_files_incomplete = 0 WHERE meeting_id = ?",
        )
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        sqlx::query("DELETE FROM transcript_file_sync WHERE meeting_id = ?")
            .bind(meeting)
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        return Ok(SyncedFiles::Other);
    };
    let mut outcome: Option<RecordingOutcome> = None;
    if publication.is_some() {
        let mut saved: RecordingOutcome =
            sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = ?")
                .bind(meeting)
                .fetch_one(pool)
                .await
                .map_err(error)?;
        saved.recording_files_incomplete = false;
        outcome = Some(saved);
    }
    let mut import_others = None;
    if let Some(FilePublication::Import(update)) = publication.as_mut() {
        let others: Vec<String> = sqlx::query_scalar(
            "SELECT folder_path FROM meetings WHERE id <> ? AND folder_path IS NOT NULL AND TRIM(folder_path) <> ''",
        ).bind(meeting).fetch_all(pool).await.map_err(error)?;
        if !update.transcript_pending {
            let target = folder.clone();
            let roots = roots.to_vec();
            let meeting = meeting.to_string();
            let other_folders = others.clone();
            let metadata = update.import_metadata.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let folder = guarded_import_folder(&target, &meeting, &roots, &other_folders)?;
                publish_import_metadata(&folder, &meeting, &metadata)
            })
            .await
            .map_err(error)??;
            update.transcript_pending = true;
            sqlx::query(
                "UPDATE transcript_file_sync SET retranscription_json = ? WHERE meeting_id = ?",
            )
            .bind(serde_json::to_string(update).map_err(error)?)
            .bind(meeting)
            .execute(pool)
            .await
            .map_err(error)?;
        }
        import_others = Some(others);
    }
    let mut transcripts = rows(pool, meeting).await?;
    transcripts.sort_by(|a, b| {
        a.audio_start_time
            .unwrap_or(0.0)
            .total_cmp(&b.audio_start_time.unwrap_or(0.0))
            .then(a.id.cmp(&b.id))
    });
    let segments: Vec<serde_json::Value> = transcripts.into_iter().enumerate().map(|(index, t)| serde_json::json!({
        "id": t.id, "text": t.transcript, "original_text": t.original_transcript, "speaker": t.speaker,
        "audio_start_time": t.audio_start_time, "audio_end_time": t.audio_end_time, "duration": t.duration,
        "display_time": t.timestamp, "timestamp": t.timestamp, "sequence_id": index, "confidence": null,
        "word_timestamps": t.word_timestamps_json.as_deref().and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
    })).collect();
    let roots = roots.to_vec();
    let meeting_id = meeting.to_string();
    let (folder, recovered_audio) =
        tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
            let is_import = import_others.is_some();
            let folder = match import_others {
                Some(others) => guarded_import_folder(&folder, &meeting_id, &roots, &others)?,
                None => crate::trusted_roots::resolve_allowed(&folder, &roots)?,
            };
            let folder = crate::database::meeting_files::plain_path(&folder);
            let transcript_path = folder.join("transcripts.json");
            let mut transcript = serde_json::json!({
                "version": "1.0", "total_segments": segments.len(), "segments": segments,
                "last_updated": chrono::Utc::now().to_rfc3339()
            });
            if is_import {
                let Some(FilePublication::Import(update)) = publication.as_ref() else {
                    return Err("The pending import metadata is unavailable".into());
                };
                publish_import_metadata(&folder, &meeting_id, &update.import_metadata)?;
                match std::fs::symlink_metadata(&transcript_path) {
                    Ok(meta) if meta.is_file() => {
                        let existing: serde_json::Value =
                            serde_json::from_reader(std::io::BufReader::new(
                                std::fs::File::open(&transcript_path).map_err(error)?,
                            ))
                            .map_err(error)?;
                        if existing["meeting_id"].as_str() != Some(meeting_id.as_str()) {
                            return Err(
                                "The transcript file could not be verified for this meeting."
                                    .into(),
                            );
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err("The transcript destination is unavailable.".into()),
                }
                transcript["meeting_id"] = serde_json::json!(meeting_id);
            }
            publish_json(&transcript_path, &transcript)?;
            let mut recovered_audio = false;
            if let Some((publication, outcome)) = publication.zip(outcome) {
                match publication {
                    FilePublication::Retranscription(update) => {
                        update.write_metadata(&folder, &meeting_id)?;
                        outcome.write(&folder)?;
                        recovered_audio = matches!(
                            update.audio_filename.as_str(),
                            "audio-recovered.mp4" | "audio-recovered.wav"
                        );
                    }
                    FilePublication::Restored(update) => {
                        update.write_metadata(&folder)?;
                        outcome.write(&folder)?;
                    }
                    FilePublication::Import(_) => {
                        if outcome.needs_recovery() {
                            outcome.write(&folder)?;
                        }
                    }
                }
            }
            Ok((folder, recovered_audio))
        })
        .await
        .map_err(error)??;
    let owner = if synced == SyncedFiles::Retranscription {
        notes_profile(&folder)?
    } else {
        None
    };
    let mut tx = pool.begin().await.map_err(error)?;
    if payload.is_some() {
        sqlx::query(
            "UPDATE recording_outcomes SET recording_files_incomplete = 0 WHERE meeting_id = ?",
        )
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    }
    sqlx::query("DELETE FROM transcript_file_sync WHERE meeting_id = ?")
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    if let Some(owner) = owner {
        crate::summary::automatic::publication::remember(&mut tx, meeting, &owner)
            .await
            .map_err(error)?;
    }
    tx.commit().await.map_err(error)?;
    if recovered_audio
        && crate::audio::incremental_saver::release_registered_capture(
            pool,
            &folder.to_string_lossy(),
        )
        .await
        .is_err()
    {
        log::warn!(
            "Transcript saved; recovery originals retained because cleanup could not finish"
        );
    }
    Ok(synced)
}

#[derive(Serialize)]
pub struct EditState {
    can_undo: bool,
    pending_file_sync: bool,
    import_file_stage: Option<&'static str>,
    has_edits: bool,
    can_restore_previous: bool,
}

#[tauri::command]
pub async fn api_get_transcript_edit_state(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<EditState, String> {
    let pool = state.db_manager.pool();
    let can_undo: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transcript_edit_batches WHERE meeting_id = ? AND undone = 0",
    )
    .bind(&meeting_id)
    .fetch_one(pool)
    .await
    .map_err(error)?;
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync WHERE meeting_id = ?")
            .bind(&meeting_id)
            .fetch_one(pool)
            .await
            .map_err(error)?;
    let edits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ? AND original_transcript IS NOT NULL AND transcript != original_transcript").bind(&meeting_id).fetch_one(pool).await.map_err(error)?;
    let revisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM transcript_revisions WHERE meeting_id = ?")
            .bind(&meeting_id)
            .fetch_one(pool)
            .await
            .map_err(error)?;
    Ok(EditState {
        can_undo: can_undo > 0,
        pending_file_sync: pending > 0,
        import_file_stage: import_file_stage(pool, &meeting_id).await?,
        has_edits: edits > 0,
        can_restore_previous: revisions > 0,
    })
}

async fn complete<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting: &str,
    changed: usize,
    job: tokio::sync::OwnedSemaphorePermit,
) -> EditResult {
    let roots = crate::audio::recording_preferences::recording_roots(app).await;
    let synced = sync_file_with_profile(pool, meeting, &roots, |folder| {
        crate::summary::automatic::publication::profile_for_folder(app, folder)
    })
    .await;
    let _ = app.emit(
        "transcript-text-edited",
        serde_json::json!({ "meeting_id": meeting }),
    );
    drop(job);
    if synced == Ok(SyncedFiles::Retranscription) {
        crate::summary::automatic::publication::wake(app, meeting.to_owned());
    }
    EditResult {
        changed,
        file_warning: synced.is_err(),
    }
}

#[tauri::command]
pub async fn api_update_transcript_text<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    transcript_id: String,
    text: String,
    expected_text: String,
) -> Result<EditResult, String> {
    let job = crate::audio::inference::claim_job()?;
    validate_text(&text)?;
    let pool = state.db_manager.pool();
    let row: Transcript =
        sqlx::query_as("SELECT * FROM transcripts WHERE meeting_id = ? AND id = ?")
            .bind(&meeting_id)
            .bind(transcript_id)
            .fetch_one(pool)
            .await
            .map_err(error)?;
    if row.transcript != expected_text {
        return Err("The transcript changed. Reload it before editing.".into());
    }
    let changed = usize::from(row.transcript != text);
    if changed > 0 {
        apply(
            pool,
            &meeting_id,
            &[Change {
                id: row.id,
                before: row.transcript,
                after: text,
                original: row.original_transcript,
                words: row.word_timestamps_json,
            }],
        )
        .await?;
    }
    Ok(complete(&app, pool, &meeting_id, changed, job).await)
}

#[tauri::command]
pub async fn api_preview_transcript_replace(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    query: String,
    replacement: String,
    match_case: bool,
) -> Result<ReplacePreview, String> {
    let transcripts = rows(state.db_manager.pool(), &meeting_id).await?;
    Ok(tauri::async_runtime::spawn_blocking(move || {
        replacement_plan(transcripts, &query, &replacement, match_case)
    })
    .await
    .map_err(error)??
    .0)
}

#[tauri::command]
pub async fn api_replace_transcript_text<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    query: String,
    replacement: String,
    match_case: bool,
    preview_token: String,
) -> Result<EditResult, String> {
    let job = crate::audio::inference::claim_job()?;
    let pool = state.db_manager.pool();
    let transcripts = rows(pool, &meeting_id).await?;
    let (preview, changes) = tauri::async_runtime::spawn_blocking(move || {
        replacement_plan(transcripts, &query, &replacement, match_case)
    })
    .await
    .map_err(error)??;
    if preview.token != preview_token {
        return Err("The transcript changed. Preview the replacements again.".into());
    }
    apply(pool, &meeting_id, &changes).await?;
    Ok(complete(&app, pool, &meeting_id, changes.len(), job).await)
}

#[tauri::command]
pub async fn api_undo_transcript_edit<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<EditResult, String> {
    let job = crate::audio::inference::claim_job()?;
    let pool = state.db_manager.pool();
    let count = undo(pool, &meeting_id).await?;
    Ok(complete(&app, pool, &meeting_id, count, job).await)
}

#[tauri::command]
pub async fn api_restore_previous_transcript<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<EditResult, String> {
    let job = crate::audio::inference::claim_job()?;
    let pool = state.db_manager.pool();
    let roots = crate::audio::recording_preferences::recording_roots(&app).await;
    let count = restore_previous(pool, &meeting_id, &roots).await?;
    Ok(complete(&app, pool, &meeting_id, count, job).await)
}

#[tauri::command]
pub async fn api_sync_transcript_file<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<(), String> {
    let job = crate::audio::inference::claim_job()?;
    let roots = crate::audio::recording_preferences::recording_roots(&app).await;
    let synced = sync_file_with_profile(state.db_manager.pool(), &meeting_id, &roots, |folder| {
        crate::summary::automatic::publication::profile_for_folder(&app, folder)
    })
    .await?;
    let _ = app.emit(
        "transcript-files-synchronized",
        serde_json::json!({ "meeting_id": meeting_id }),
    );
    drop(job);
    if synced == SyncedFiles::Retranscription {
        crate::summary::automatic::publication::wake(&app, meeting_id);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) async fn fixture() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('review-test', 'Synthetic review', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool).await.unwrap();
        for (id, content, start) in [
            ("a", "Äpfel project", 0.0),
            ("b", "Project confirmed", 900.0),
        ] {
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, word_timestamps_json) VALUES (?, 'review-test', ?, '00:00', ?, ?, '[]')")
                .bind(id).bind(content).bind(start).bind(start + 3.0).execute(&pool).await.unwrap();
        }
        pool
    }

    #[tokio::test]
    async fn active_speaker_detection_blocks_edits_undo_restore_and_renames() {
        let _lock = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = fixture().await;
        let meeting = "diarization-edit-test";
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, 'Synthetic detection', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").bind(meeting).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('detection-row', ?, 'Original', '00:00')").bind(meeting).execute(&pool).await.unwrap();
        let changes = vec![Change {
            id: "detection-row".into(),
            before: "Original".into(),
            after: "Corrected".into(),
            original: None,
            words: None,
        }];
        let guard = crate::audio::diarization::DiarizationRunGuard::acquire(meeting).unwrap();
        assert!(apply(&pool, meeting, &changes)
            .await
            .unwrap_err()
            .contains("speaker detection"));
        assert!(undo(&pool, meeting)
            .await
            .unwrap_err()
            .contains("speaker detection"));
        assert!(restore_previous(&pool, meeting, &[])
            .await
            .unwrap_err()
            .contains("speaker detection"));
        use crate::database::repositories::transcript::TranscriptsRepository;
        assert!(TranscriptsRepository::update_transcript_speaker(
            &pool,
            meeting,
            "detection-row",
            Some("Edited")
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("speaker detection"));
        assert!(TranscriptsRepository::update_transcript_speakers_matching(
            &pool,
            meeting,
            None,
            Some("Edited")
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("speaker detection"));
        assert_editable(&mut pool.acquire().await.unwrap(), "review-test")
            .await
            .unwrap();
        drop(guard);
        apply(&pool, meeting, &changes).await.unwrap();
    }

    #[tokio::test]
    async fn interrupted_summary_restores_saved_result_and_unblocks_corrections() {
        let pool = fixture().await;
        for status in ["pending", "processing", "summarizing", "regenerating"] {
            sqlx::query("INSERT OR REPLACE INTO summary_processes (meeting_id, status, created_at, updated_at, result, result_backup) VALUES ('review-test', ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 'partial', 'saved summary')")
                .bind(status).execute(&pool).await.unwrap();
            crate::database::repositories::summary::SummaryProcessesRepository::fail_interrupted_processes(&pool).await.unwrap();
            let row: (String, String, String, Option<String>) = sqlx::query_as("SELECT status, error, result, result_backup FROM summary_processes WHERE meeting_id = 'review-test'").fetch_one(&pool).await.unwrap();
            assert_eq!(
                row,
                (
                    "failed".into(),
                    "Summary generation was interrupted when ClawScribe closed".into(),
                    "saved summary".into(),
                    None
                )
            );
            assert_editable(&mut pool.acquire().await.unwrap(), "review-test")
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn replacement_is_literal_case_insensitive_and_undo_restores_original_alignment() {
        let pool = fixture().await;
        let (preview, changes) = replacement_plan(
            rows(&pool, "review-test").await.unwrap(),
            "project",
            "$1",
            false,
        )
        .unwrap();
        assert_eq!(preview.matches, 2);
        apply(&pool, "review-test", &changes).await.unwrap();
        let changed = rows(&pool, "review-test").await.unwrap();
        assert_eq!(changed[0].transcript, "Äpfel $1");
        assert_eq!(
            changed[0].original_transcript.as_deref(),
            Some("Äpfel project")
        );
        assert!(changed.iter().all(|row| row.word_timestamps_json.is_none()));
        assert_eq!(changed[1].audio_start_time, Some(900.0));
        let (_, second) = replacement_plan(changed, "$1", "review", true).unwrap();
        apply(&pool, "review-test", &second).await.unwrap();
        undo(&pool, "review-test").await.unwrap();
        assert_eq!(
            rows(&pool, "review-test").await.unwrap()[0].transcript,
            "Äpfel $1"
        );
        undo(&pool, "review-test").await.unwrap();
        let restored = rows(&pool, "review-test").await.unwrap();
        assert_eq!(restored[0].transcript, "Äpfel project");
        assert_eq!(restored[0].word_timestamps_json.as_deref(), Some("[]"));
        assert!(restored[0].original_transcript.is_none());
    }

    #[tokio::test]
    async fn restore_revision_preserves_every_field_and_toggles_without_reusing_edit_batches() {
        let pool = fixture().await;
        let (_, changes) = replacement_plan(
            rows(&pool, "review-test").await.unwrap(),
            "project",
            "correction",
            false,
        )
        .unwrap();
        apply(&pool, "review-test", &changes).await.unwrap();
        let original = serde_json::to_value(rows(&pool, "review-test").await.unwrap()).unwrap();
        sqlx::query("INSERT INTO transcript_revisions (id, meeting_id, created_at, transcripts_json) VALUES ('revision', 'review-test', CURRENT_TIMESTAMP, ?)")
            .bind(original.to_string()).execute(&pool).await.unwrap();
        sqlx::query("UPDATE transcripts SET id = id || '-new', transcript = 'replacement', original_transcript = NULL").execute(&pool).await.unwrap();
        let replacement = serde_json::to_value(rows(&pool, "review-test").await.unwrap()).unwrap();
        assert_eq!(
            restore_previous(&pool, "review-test", &[]).await.unwrap(),
            2
        );
        assert_eq!(
            serde_json::to_value(rows(&pool, "review-test").await.unwrap()).unwrap(),
            original
        );
        assert!(undo(&pool, "review-test").await.is_err());
        assert_eq!(
            restore_previous(&pool, "review-test", &[]).await.unwrap(),
            2
        );
        assert_eq!(
            serde_json::to_value(rows(&pool, "review-test").await.unwrap()).unwrap(),
            replacement
        );
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM transcript_file_sync WHERE meeting_id = 'review-test'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, 1);
        sqlx::query("INSERT INTO summary_processes (meeting_id, status, created_at, updated_at) VALUES ('review-test', 'processing', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").execute(&pool).await.unwrap();
        assert!(restore_previous(&pool, "review-test", &[])
            .await
            .unwrap_err()
            .contains("summary generation"));
        assert_eq!(
            serde_json::to_value(rows(&pool, "review-test").await.unwrap()).unwrap(),
            replacement
        );
    }

    #[tokio::test]
    async fn folderless_meeting_never_keeps_an_unfinishable_file_retry() {
        let pool = fixture().await;
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, capture_incomplete, recording_files_incomplete) VALUES ('review-test', 1, 1)")
            .execute(&pool).await.unwrap();
        let payload = serde_json::json!({
            "retranscribed_at": "2026-01-01T00:00:00Z", "duration_seconds": 2.0,
            "audio_filename": "audio.wav", "provider": "localWhisper", "model": "tiny",
            "source_language": "de",
        })
        .to_string();
        sqlx::query("INSERT INTO transcript_file_sync (meeting_id, retranscription_json) VALUES ('review-test', ?)")
            .bind(payload).execute(&pool).await.unwrap();
        // A library restored without its files must not retry forever.
        sync_file(&pool, "review-test", &[]).await.unwrap();
        async fn state(pool: &SqlitePool) -> (i64, (bool, bool)) {
            let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync")
                .fetch_one(pool)
                .await
                .unwrap();
            let flags: (bool, bool) = sqlx::query_as(
                "SELECT recording_files_incomplete, capture_incomplete FROM recording_outcomes",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            (rows, flags)
        }
        assert_eq!(state(&pool).await, (0, (false, true)));

        // Restoring a revision of a folderless meeting stores no file work either.
        let archived = serde_json::to_value(rows(&pool, "review-test").await.unwrap())
            .unwrap()
            .to_string();
        sqlx::query("INSERT INTO transcript_revisions (id, meeting_id, created_at, transcripts_json) VALUES ('revision', 'review-test', CURRENT_TIMESTAMP, ?)")
            .bind(archived).execute(&pool).await.unwrap();
        restore_previous(&pool, "review-test", &[]).await.unwrap();
        let payload: Option<String> =
            sqlx::query_scalar("SELECT retranscription_json FROM transcript_file_sync")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(payload.is_none());
        assert_eq!(state(&pool).await, (1, (false, true)));
        sync_file(&pool, "review-test", &[]).await.unwrap();
        assert_eq!(state(&pool).await, (0, (false, true)));
    }

    #[tokio::test]
    async fn concurrent_changes_and_pending_summary_reject_edits_without_partial_updates() {
        let pool = fixture().await;
        let (_, changes) = replacement_plan(
            rows(&pool, "review-test").await.unwrap(),
            "project",
            "review",
            false,
        )
        .unwrap();
        sqlx::query("UPDATE transcripts SET transcript = 'A newer correction' WHERE id = 'b'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(apply(&pool, "review-test", &changes).await.is_err());
        assert_eq!(
            rows(&pool, "review-test").await.unwrap()[0].transcript,
            "Äpfel project"
        );
        sqlx::query("INSERT INTO summary_processes (meeting_id, status, created_at, updated_at) VALUES ('review-test', 'PENDING', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").execute(&pool).await.unwrap();
        assert!(apply(&pool, "review-test", &changes[..1]).await.is_err());
        assert!(replacement_plan(
            rows(&pool, "review-test").await.unwrap(),
            "Äpfel project",
            "",
            true
        )
        .is_err());
    }

    #[tokio::test]
    async fn file_mirror_preserves_recording_format_and_retry_marker_until_success() {
        let pool = fixture().await;
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("recording");
        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'review-test'")
            .bind(folder.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        let (_, changes) = replacement_plan(
            rows(&pool, "review-test").await.unwrap(),
            "project",
            "review",
            false,
        )
        .unwrap();
        apply(&pool, "review-test", &changes).await.unwrap();
        assert!(sync_file(&pool, "review-test", &[]).await.is_err());
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pending, 1);
        std::fs::create_dir(&folder).unwrap();
        sync_file(&pool, "review-test", &[]).await.unwrap();
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.join("transcripts.json")).unwrap())
                .unwrap();
        assert_eq!(data["version"], "1.0");
        assert_eq!(data["segments"][0]["text"], "Äpfel review");
        assert_eq!(data["segments"][0]["original_text"], "Äpfel project");
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pending, 0);
    }
}
