//! Local meeting organization and document export. No provider connection required.
pub mod backup;
pub mod context;

use crate::state::AppState;
use serde::Serialize;
use sqlx::SqlitePool;
use std::io::Write;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, sqlx::FromRow)]
pub struct MeetingTag {
    meeting_id: String,
    tag: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Bookmark {
    id: String,
    seconds: f64,
    label: String,
}

fn clean_label(value: &str, max: usize) -> Result<String, String> {
    let clean = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() || clean.chars().count() > max {
        return Err(format!("Enter between 1 and {max} characters."));
    }
    Ok(clean)
}

pub(crate) async fn attach_bookmarks(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE meeting_bookmarks SET meeting_id = (SELECT id FROM meetings WHERE meetings.folder_path = meeting_bookmarks.folder_path LIMIT 1) WHERE meeting_id IS NULL AND EXISTS (SELECT 1 FROM meetings WHERE meetings.folder_path = meeting_bookmarks.folder_path)")
        .execute(pool).await?;
    Ok(())
}

pub(crate) async fn cleanup_orphaned_bookmarks(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    if crate::audio::recording_commands::is_recording().await {
        return Ok(());
    }
    let candidates: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, folder_path FROM meeting_bookmarks WHERE meeting_id IS NULL AND folder_path IS NOT NULL
         AND NOT EXISTS (SELECT 1 FROM meetings WHERE meetings.folder_path = meeting_bookmarks.folder_path)",
    ).fetch_all(pool).await?;
    for (id, folder) in candidates {
        // An inaccessible folder is not evidence that recovery data is gone.
        if !matches!(tokio::fs::try_exists(&folder).await, Ok(false)) {
            continue;
        }
        if crate::audio::recording_commands::is_recording().await {
            return Ok(());
        }
        sqlx::query("DELETE FROM meeting_bookmarks WHERE id = ? AND folder_path = ? AND meeting_id IS NULL
                    AND NOT EXISTS (SELECT 1 FROM meetings WHERE meetings.folder_path = meeting_bookmarks.folder_path)")
            .bind(id).bind(folder).execute(pool).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn list_meeting_tags(app: AppHandle) -> Result<Vec<MeetingTag>, String> {
    sqlx::query_as("SELECT meeting_id, tag FROM meeting_tags ORDER BY tag COLLATE NOCASE")
        .fetch_all(app.state::<AppState>().db_manager.pool())
        .await
        .map_err(|_| "Could not load project tags.".into())
}

#[tauri::command]
pub async fn set_meeting_tags(
    app: AppHandle,
    meeting_id: String,
    tags: Vec<String>,
) -> Result<(), String> {
    if tags.len() > 20 {
        return Err("Use at most 20 tags per meeting.".into());
    }
    let tags = tags
        .iter()
        .map(|tag| clean_label(tag, 60))
        .collect::<Result<Vec<_>, _>>()?;
    let state = app.state::<AppState>();
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|_| "Could not save tags.")?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?)")
        .bind(&meeting_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| "Could not read meeting.")?;
    if !exists {
        return Err("Meeting no longer exists.".into());
    }
    sqlx::query("DELETE FROM meeting_tags WHERE meeting_id = ?")
        .bind(&meeting_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| "Could not save tags.")?;
    for tag in tags {
        sqlx::query("INSERT OR IGNORE INTO meeting_tags (meeting_id, tag) VALUES (?, ?)")
            .bind(&meeting_id)
            .bind(tag)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Could not save tags.")?;
    }
    tx.commit().await.map_err(|_| "Could not save tags.")?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn list_meeting_bookmarks(
    app: AppHandle,
    meeting_id: String,
) -> Result<Vec<Bookmark>, String> {
    let state = app.state::<AppState>();
    attach_bookmarks(state.db_manager.pool())
        .await
        .map_err(|_| "Could not load bookmarks.")?;
    sqlx::query_as("SELECT id, seconds, label FROM meeting_bookmarks WHERE meeting_id = ? ORDER BY seconds, id")
        .bind(meeting_id).fetch_all(state.db_manager.pool()).await.map_err(|_| "Could not load bookmarks.".into())
}

#[tauri::command]
pub async fn add_meeting_bookmark(
    app: AppHandle,
    meeting_id: Option<String>,
    seconds: Option<f64>,
    label: String,
) -> Result<(), String> {
    let label = clean_label(&label, 160)?;
    let (folder, seconds) = if meeting_id.is_some() {
        (None, seconds.ok_or("Choose a bookmark time.")?)
    } else {
        let (folder, seconds) = crate::audio::recording_commands::bookmark_position()?;
        (Some(folder), seconds)
    };
    if !seconds.is_finite() || !(0.0..=604800.0).contains(&seconds) {
        return Err("Invalid bookmark time.".into());
    }
    sqlx::query("INSERT INTO meeting_bookmarks (id, meeting_id, folder_path, seconds, label) VALUES (?, ?, ?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(meeting_id).bind(folder).bind(seconds).bind(label)
        .execute(app.state::<AppState>().db_manager.pool()).await.map_err(|_| "Could not save bookmark.")?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn rename_meeting_bookmark(
    app: AppHandle,
    id: String,
    label: String,
) -> Result<(), String> {
    let label = clean_label(&label, 160)?;
    let result = sqlx::query("UPDATE meeting_bookmarks SET label = ? WHERE id = ?")
        .bind(label)
        .bind(id)
        .execute(app.state::<AppState>().db_manager.pool())
        .await
        .map_err(|_| "Could not rename bookmark.")?;
    if result.rows_affected() == 0 {
        return Err("Bookmark no longer exists.".into());
    }
    let _ = app.emit("library-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_meeting_bookmark(app: AppHandle, id: String) -> Result<(), String> {
    sqlx::query("DELETE FROM meeting_bookmarks WHERE id = ?")
        .bind(id)
        .execute(app.state::<AppState>().db_manager.pool())
        .await
        .map_err(|_| "Could not remove bookmark.")?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Choose a destination folder.")?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "Could not create output file.")?;
    file.write_all(bytes)
        .map_err(|_| "Could not write output file. Check free disk space.")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "Could not finish writing output file.")?;
    file.persist(path)
        .map_err(|_| "Could not save output file. Close it in other applications and retry.")?;
    Ok(())
}

#[tauri::command]
pub async fn export_local_word(
    app: AppHandle,
    meeting_id: String,
    title: String,
    markdown: String,
    include_transcript: bool,
    include_speakers: bool,
    include_timestamps: bool,
    path: String,
) -> Result<(), String> {
    if !Path::new(&path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("docx"))
    {
        return Err("Choose a .docx file.".into());
    }
    let state = app.state::<AppState>();
    let created: String = sqlx::query_scalar("SELECT created_at FROM meetings WHERE id = ?")
        .bind(&meeting_id)
        .fetch_one(state.db_manager.pool())
        .await
        .map_err(|_| "Meeting no longer exists.")?;
    // Fetch the full saved transcript, independently of the UI's current page.
    let transcript = if include_transcript {
        Some(
            read_word_transcript(
                state.db_manager.pool(),
                &meeting_id,
                include_speakers,
                include_timestamps,
            )
            .await?,
        )
    } else {
        None
    };
    if markdown.trim().is_empty() && transcript.as_deref().unwrap_or_default().trim().is_empty() {
        return Err("There are no notes or transcripts to export.".into());
    }
    tokio::task::spawn_blocking(move || {
        let bytes = crate::exports::document::build_meeting_docx(
            &title,
            &format!("Date: {}\n\n{markdown}", export_meeting_date(&created)),
            transcript.as_deref(),
        )?;
        atomic_write(Path::new(&path), &bytes)
    })
    .await
    .map_err(|_| "Word export was interrupted.")?
}

fn export_timestamp(timestamp: &str, audio_start_time: Option<f64>) -> String {
    match audio_start_time.filter(|time| time.is_finite() && *time >= 0.0) {
        Some(time) => {
            let seconds = time.floor() as u64;
            format!(
                "{:02}:{:02}:{:02}",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            )
        }
        None => timestamp.trim().to_string(),
    }
}

fn export_meeting_date(created: &str) -> String {
    let date = chrono::DateTime::parse_from_rfc3339(created)
        .map(|date| date.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(created, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|date| date.and_utc())
        });
    date.map(|date| {
        date.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
    .unwrap_or_else(|| created.to_string())
}

async fn read_word_transcript(
    pool: &SqlitePool,
    meeting_id: &str,
    speakers: bool,
    timestamps: bool,
) -> Result<String, String> {
    let rows: Vec<(String, Option<f64>, Option<String>, String)> = sqlx::query_as(
        "SELECT timestamp, audio_start_time, speaker, transcript FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time, id"
    ).bind(meeting_id).fetch_all(pool).await.map_err(|_| "Could not read transcript.")?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, _, text)| !text.trim().is_empty())
        .map(|(time, start, speaker, text)| {
            let time = if timestamps {
                export_timestamp(&time, start)
            } else {
                String::new()
            };
            let speaker = if speakers {
                speaker.unwrap_or_default().trim().to_string()
            } else {
                String::new()
            };
            format!(
                "{}{}{}",
                if time.is_empty() {
                    String::new()
                } else {
                    format!("[{time}] ")
                },
                if speaker.is_empty() {
                    String::new()
                } else {
                    format!("{speaker}: ")
                },
                text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_times_prefer_recording_offsets_and_keep_legacy_fallback() {
        for (start, expected) in [
            (0.0, "00:00:00"),
            (65.9, "00:01:05"),
            (3661.1, "01:01:01"),
            (360000.0, "100:00:00"),
        ] {
            assert_eq!(
                export_timestamp("2026-09-26T08:15:30+00:00", Some(start)),
                expected
            );
        }
        for start in [None, Some(f64::NAN), Some(f64::INFINITY), Some(-1.0)] {
            assert_eq!(export_timestamp(" 09:10:11 ", start), "09:10:11");
        }
        assert_eq!(
            export_meeting_date("2026-09-26 08:15:30"),
            export_meeting_date("2026-09-26T08:15:30+00:00")
        );
        assert!(!export_meeting_date("2026-09-26T08:15:30+00:00").contains('T'));
    }

    #[tokio::test]
    async fn word_export_reads_audio_offsets_and_honors_formatting_options() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query(
            "UPDATE transcripts SET timestamp = '2026-09-26T08:15:30+00:00', speaker = 'Speaker A'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            read_word_transcript(&pool, "review-test", true, true)
                .await
                .unwrap(),
            "[00:00:00] Speaker A: Äpfel project\n\n[00:15:00] Speaker A: Project confirmed"
        );
        assert_eq!(
            read_word_transcript(&pool, "review-test", false, false)
                .await
                .unwrap(),
            "Äpfel project\n\nProject confirmed"
        );
        assert_eq!(
            read_word_transcript(&pool, "review-test", false, true)
                .await
                .unwrap(),
            "[00:00:00] Äpfel project\n\n[00:15:00] Project confirmed"
        );
        assert_eq!(
            read_word_transcript(&pool, "review-test", true, false)
                .await
                .unwrap(),
            "Speaker A: Äpfel project\n\nSpeaker A: Project confirmed"
        );
        sqlx::query("UPDATE transcripts SET audio_start_time = NULL WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(read_word_transcript(&pool, "review-test", false, true)
            .await
            .unwrap()
            .starts_with("[2026-09-26T08:15:30+00:00]"));
    }
    #[test]
    fn labels_are_bounded_and_normalized() {
        assert_eq!(clean_label("  Project   Ä  ", 60).unwrap(), "Project Ä");
        assert!(clean_label("\n", 60).is_err());
        assert!(clean_label(&"x".repeat(61), 60).is_err());
    }
    #[tokio::test]
    async fn live_bookmarks_attach_and_tags_cascade() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query(
            "UPDATE meetings SET folder_path = 'synthetic-folder' WHERE id = 'review-test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO meeting_bookmarks VALUES ('mark', NULL, 'synthetic-folder', 12.5, 'Decision')").execute(&pool).await.unwrap();
        attach_bookmarks(&pool).await.unwrap();
        let owner: String = sqlx::query_scalar("SELECT meeting_id FROM meeting_bookmarks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(owner, "review-test");
        sqlx::query("INSERT INTO meeting_tags VALUES ('review-test', 'Project')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            sqlx::query("INSERT INTO meeting_tags VALUES ('review-test', 'project')")
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        for table in ["meeting_bookmarks", "meeting_tags"] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[tokio::test]
    async fn orphaned_bookmarks_preserve_recoverable_and_saved_recordings() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let recoverable = root.path().join("recoverable");
        std::fs::create_dir(&recoverable).unwrap();
        let saved = root.path().join("saved-but-unavailable");
        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'review-test'")
            .bind(saved.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        for (id, folder, owner) in [
            ("gone", root.path().join("gone"), None),
            ("recoverable", recoverable, None),
            ("saved", saved, None),
            (
                "attached",
                root.path().join("attached-but-unavailable"),
                Some("review-test"),
            ),
        ] {
            sqlx::query("INSERT INTO meeting_bookmarks (id, meeting_id, folder_path, seconds, label) VALUES (?, ?, ?, 1, 'Synthetic bookmark')")
                .bind(id).bind(owner).bind(folder.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        }
        cleanup_orphaned_bookmarks(&pool).await.unwrap();
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM meeting_bookmarks ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, ["attached", "recoverable", "saved"]);
        attach_bookmarks(&pool).await.unwrap();
        let owner: String =
            sqlx::query_scalar("SELECT meeting_id FROM meeting_bookmarks WHERE id = 'saved'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner, "review-test");
    }

    #[tokio::test]
    async fn library_migration_preserves_existing_meetings_and_transcripts() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::raw_sql("DROP TABLE meeting_tags; DROP TABLE meeting_bookmarks;")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/20260926000000_library_tools.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM transcripts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn deleting_unopened_meeting_removes_live_bookmarks() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("UPDATE meetings SET folder_path = 'synthetic-folder'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meeting_bookmarks VALUES ('mark', NULL, 'synthetic-folder', 12.5, 'Review')").execute(&pool).await.unwrap();
        crate::database::repositories::meeting::MeetingsRepository::delete_meeting(
            &pool,
            "review-test",
        )
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meeting_bookmarks")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }
}
