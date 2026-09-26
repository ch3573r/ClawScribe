//! Local meeting organization and document export. No provider connection required.
pub mod backup;

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
        let rows: Vec<(String, Option<String>, String)> = sqlx::query_as("SELECT timestamp, speaker, transcript FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time, id")
            .bind(&meeting_id).fetch_all(state.db_manager.pool()).await.map_err(|_| "Could not read transcript.")?;
        Some(
            rows.into_iter()
                .map(|(time, speaker, text)| {
                    format!(
                        "{}{}{}",
                        if include_timestamps {
                            format!("[{time}] ")
                        } else {
                            String::new()
                        },
                        if include_speakers {
                            speaker.map(|s| format!("{s}: ")).unwrap_or_default()
                        } else {
                            String::new()
                        },
                        text
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
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
            &format!("Date: {created}\n\n{markdown}"),
            transcript.as_deref(),
        )?;
        atomic_write(Path::new(&path), &bytes)
    })
    .await
    .map_err(|_| "Word export was interrupted.")?
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
