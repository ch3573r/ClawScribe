//! Reviewed transcript corrections. SQLite is authoritative; edits preserve timing
//! and the original text, invalidate word alignment, and can be undone atomically.
use crate::{database::models::Transcript, state::AppState};
use regex::{NoExpand, RegexBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};
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

async fn assert_editable(conn: &mut SqliteConnection, meeting: &str) -> Result<(), String> {
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

// A failed file mirror never rolls back a committed correction or reports it as
// unsaved. The UI offers a retry; summaries and Graph exports read SQLite.
async fn sync_file(pool: &SqlitePool, meeting: &str) -> Result<(), String> {
    let folder: Option<String> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting)
            .fetch_one(pool)
            .await
            .map_err(error)?;
    let Some(folder) = folder.filter(|value| !value.trim().is_empty()) else {
        sqlx::query("DELETE FROM transcript_file_sync WHERE meeting_id = ?")
            .bind(meeting)
            .execute(pool)
            .await
            .map_err(error)?;
        return Ok(());
    };
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
        "display_time": t.timestamp, "sequence_id": index, "confidence": null,
        "word_timestamps": t.word_timestamps_json.as_deref().and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
    })).collect();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use std::io::Write;
        let folder = std::path::Path::new(&folder);
        let mut file = tempfile::NamedTempFile::new_in(folder).map_err(error)?;
        serde_json::to_writer_pretty(
            &mut file,
            &serde_json::json!({
                "version": "1.0", "total_segments": segments.len(), "segments": segments,
                "last_updated": chrono::Utc::now().to_rfc3339()
            }),
        )
        .map_err(error)?;
        file.flush().map_err(error)?;
        file.as_file().sync_all().map_err(error)?;
        file.persist(folder.join("transcripts.json"))
            .map_err(error)?;
        Ok(())
    })
    .await
    .map_err(error)??;
    sqlx::query("DELETE FROM transcript_file_sync WHERE meeting_id = ?")
        .bind(meeting)
        .execute(pool)
        .await
        .map_err(error)?;
    Ok(())
}

#[derive(Serialize)]
pub struct EditState {
    can_undo: bool,
    pending_file_sync: bool,
    has_edits: bool,
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
    Ok(EditState {
        can_undo: can_undo > 0,
        pending_file_sync: pending > 0,
        has_edits: edits > 0,
    })
}

async fn complete<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting: &str,
    changed: usize,
) -> EditResult {
    let file_warning = sync_file(pool, meeting).await.is_err();
    let _ = app.emit(
        "transcript-text-edited",
        serde_json::json!({ "meeting_id": meeting }),
    );
    EditResult {
        changed,
        file_warning,
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
    let _job = crate::audio::inference::claim_job()?;
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
    Ok(complete(&app, pool, &meeting_id, changed).await)
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
    let _job = crate::audio::inference::claim_job()?;
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
    Ok(complete(&app, pool, &meeting_id, changes.len()).await)
}

#[tauri::command]
pub async fn api_undo_transcript_edit<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<EditResult, String> {
    let _job = crate::audio::inference::claim_job()?;
    let pool = state.db_manager.pool();
    let count = undo(pool, &meeting_id).await?;
    Ok(complete(&app, pool, &meeting_id, count).await)
}

#[tauri::command]
pub async fn api_sync_transcript_file(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<(), String> {
    let _job = crate::audio::inference::claim_job()?;
    sync_file(state.db_manager.pool(), &meeting_id).await
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
        assert!(sync_file(&pool, "review-test").await.is_err());
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcript_file_sync")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pending, 1);
        std::fs::create_dir(&folder).unwrap();
        sync_file(&pool, "review-test").await.unwrap();
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
