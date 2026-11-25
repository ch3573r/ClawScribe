//! Local meeting organization and document export. No provider connection required.
pub(crate) mod archive;
pub mod backup;
pub(crate) mod backup_schedule;
pub mod calendar;
pub mod context;
pub(crate) mod file_cleanup;
pub mod follow_through;
mod markdown;
pub mod pending_recording_notes;
pub(crate) mod recording_calendar;
pub(crate) mod saved_text;
pub mod summary_bookmarks;

use crate::state::AppState;
use serde::Serialize;
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::io::Write;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, Runtime};

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
    sqlx::query("UPDATE meeting_bookmarks SET meeting_id = (SELECT id FROM meetings WHERE meetings.folder_path = meeting_bookmarks.folder_path LIMIT 1) WHERE meeting_id IS NULL AND recording_id IS NULL AND EXISTS (SELECT 1 FROM meetings WHERE meetings.folder_path = meeting_bookmarks.folder_path)")
        .execute(pool).await?;
    Ok(())
}

pub(crate) async fn cleanup_orphaned_bookmarks(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    if crate::audio::recording_commands::is_recording().await {
        return Ok(());
    }
    let candidates: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, folder_path FROM meeting_bookmarks WHERE meeting_id IS NULL AND recording_id IS NULL AND folder_path IS NOT NULL
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
        sqlx::query("DELETE FROM meeting_bookmarks WHERE id = ? AND folder_path = ? AND meeting_id IS NULL AND recording_id IS NULL
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
pub(crate) async fn add_archive_project_tags(
    app: AppHandle,
    ids: Vec<String>,
    tags: Vec<String>,
) -> Result<(), String> {
    add_tags_to_meetings(app.state::<AppState>().db_manager.pool(), ids, tags).await?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

async fn add_tags_to_meetings(
    pool: &SqlitePool,
    ids: Vec<String>,
    tags: Vec<String>,
) -> Result<(), String> {
    use std::collections::HashSet;
    const SAVE: &str = "Could not save project tags.";
    archive::storage::validate_selection(ids.iter().map(String::as_str))?;
    if tags.is_empty() || tags.len() > 20 {
        return Err("Enter between 1 and 20 project tags.".into());
    }
    let tags = tags
        .iter()
        .map(|tag| clean_label(tag, 60))
        .collect::<Result<Vec<_>, _>>()?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(|_| SAVE)?;
    for id in ids {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?)")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| SAVE)?;
        if !exists {
            return Err(
                "A selected meeting is no longer in the library. Select the meetings again.".into(),
            );
        }
        let existing: Vec<String> =
            sqlx::query_scalar("SELECT tag FROM meeting_tags WHERE meeting_id = ?")
                .bind(&id)
                .fetch_all(&mut *tx)
                .await
                .map_err(|_| SAVE)?;
        let mut seen: HashSet<_> = existing.iter().map(|tag| tag.to_lowercase()).collect();
        let added: Vec<_> = tags
            .iter()
            .filter(|tag| seen.insert(tag.to_lowercase()))
            .collect();
        if existing.len() + added.len() > 20 {
            return Err(
                "Adding these tags would exceed the 20-tag limit for a selected meeting.".into(),
            );
        }
        for tag in added {
            sqlx::query("INSERT INTO meeting_tags (meeting_id, tag) VALUES (?, ?)")
                .bind(&id)
                .bind(tag)
                .execute(&mut *tx)
                .await
                .map_err(|_| SAVE)?;
        }
    }
    tx.commit().await.map_err(|_| SAVE.into())
}

#[tauri::command]
pub async fn list_meeting_bookmarks(
    app: AppHandle,
    meeting_id: String,
) -> Result<Vec<Bookmark>, String> {
    read_bookmarks(app.state::<AppState>().db_manager.pool(), &meeting_id).await
}

async fn read_bookmarks(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<Bookmark>, String> {
    attach_bookmarks(pool)
        .await
        .map_err(|_| "Could not load bookmarks.")?;
    sqlx::query_as("SELECT id, seconds, label FROM meeting_bookmarks WHERE meeting_id = ? ORDER BY seconds, id")
        .bind(meeting_id).fetch_all(pool).await.map_err(|_| "Could not load bookmarks.".into())
}

#[tauri::command]
pub async fn add_meeting_bookmark<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: Option<String>,
    seconds: Option<f64>,
    label: String,
    session_id: Option<String>,
) -> Result<(), String> {
    let label = clean_label(&label, 160)?;
    let Some(meeting_id) = meeting_id else {
        let session_id = session_id.ok_or("The recording identity is missing.")?;
        return save_live_bookmark(&app, &session_id, &label).await;
    };
    let seconds = seconds.ok_or("Choose a bookmark time.")?;
    validate_bookmark_time(seconds)?;
    sqlx::query(
        "INSERT INTO meeting_bookmarks (id, meeting_id, seconds, label) VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(meeting_id)
    .bind(seconds)
    .bind(label)
    .execute(app.state::<AppState>().db_manager.pool())
    .await
    .map_err(|_| "Could not save bookmark.")?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

fn validate_bookmark_time(seconds: f64) -> Result<(), String> {
    if !seconds.is_finite() || !(0.0..=604800.0).contains(&seconds) {
        return Err("Invalid bookmark time.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn list_live_recording_bookmarks(
    app: AppHandle,
    session_id: String,
) -> Result<LiveRecordingBookmarks, String> {
    let (recording_id, _, _) = crate::audio::recording_commands::bookmark_position(&session_id)?;
    let state = app
        .try_state::<AppState>()
        .ok_or("The meeting library is unavailable.")?;
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|_| "Could not load recording notes.")?;
    let bookmarks = sqlx::query_as(
        "SELECT id, seconds, label FROM meeting_bookmarks
         WHERE recording_id = ? AND meeting_id IS NULL
         ORDER BY seconds DESC, id LIMIT 100",
    )
    .bind(&recording_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| "Could not load recording notes.")?;
    let include_in_summary = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM recording_summary_bookmarks WHERE recording_id = ?)",
    )
    .bind(&recording_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| "Could not load the summary preference.")?;
    tx.commit()
        .await
        .map_err(|_| "Could not load recording notes.")?;
    Ok(LiveRecordingBookmarks {
        bookmarks,
        include_in_summary,
    })
}

#[derive(Serialize)]
pub struct LiveRecordingBookmarks {
    bookmarks: Vec<Bookmark>,
    include_in_summary: bool,
}

#[tauri::command]
pub async fn set_live_summary_bookmarks(
    app: AppHandle,
    session_id: String,
    enabled: bool,
) -> Result<(), String> {
    let mut tx = begin_recording_bookmark_write(&app).await?;
    let (recording_id, _, _) = crate::audio::recording_commands::bookmark_position(&session_id)?;
    let query = if enabled {
        "INSERT INTO recording_summary_bookmarks (recording_id) VALUES (?) ON CONFLICT (recording_id) DO NOTHING"
    } else {
        "DELETE FROM recording_summary_bookmarks WHERE recording_id = ?"
    };
    sqlx::query(query)
        .bind(recording_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| "Could not save the summary preference.")?;
    tx.commit()
        .await
        .map_err(|_| "Could not save the summary preference.")?;
    Ok(())
}

async fn begin_recording_bookmark_write<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Transaction<'static, Sqlite>, String> {
    // Take the reservation before reading capture state. A matching Stop
    // saves through the same database and will attach this row at commit.
    let state = app
        .try_state::<AppState>()
        .ok_or("The meeting library is unavailable.")?;
    state
        .db_manager
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "Could not save changes to the meeting library.".into())
}

async fn save_live_bookmark<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    label: &str,
) -> Result<(), String> {
    let mut tx = begin_recording_bookmark_write(app).await?;
    let (recording_id, folder, seconds) =
        crate::audio::recording_commands::bookmark_position(session_id)?;
    validate_bookmark_time(seconds)?;
    sqlx::query("INSERT INTO meeting_bookmarks (id, folder_path, seconds, label, recording_id) VALUES (?, ?, ?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(folder).bind(seconds).bind(label).bind(recording_id)
        .execute(&mut *tx).await.map_err(|_| "Could not save bookmark.")?;
    tx.commit().await.map_err(|_| "Could not save bookmark.")?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

pub(crate) async fn discard_recording_drafts(
    pool: &SqlitePool,
    recording_id: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    remove_pending_bookmarks(&mut tx, recording_id).await?;
    sqlx::query("DELETE FROM recording_calendar_drafts WHERE recording_id = ?")
        .bind(recording_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM recording_title_drafts WHERE recording_id = ?")
        .bind(recording_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM saved_live_answers WHERE recording_id = ? AND meeting_id IS NULL")
        .bind(recording_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

async fn remove_pending_bookmarks(
    connection: &mut sqlx::SqliteConnection,
    recording_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM meeting_bookmarks WHERE recording_id = ? AND meeting_id IS NULL")
        .bind(recording_id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("DELETE FROM recording_summary_bookmarks WHERE recording_id = ?")
        .bind(recording_id)
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) fn bookmark_active_recording<R: Runtime>(app: &AppHandle<R>) {
    let Some(session_id) = crate::audio::recording_commands::active_bookmark_session() else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let saved = save_live_bookmark(&app, &session_id, "Review this")
            .await
            .is_ok();
        if !saved {
            log::warn!("Could not save a recording bookmark");
        }
        let _ = app.emit("recording-bookmark-result", saved);
    });
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
    // Only a path the user chose in the save dialog the core opened
    // (`pick_word_save_path`), once.
    let granted = crate::file_grants::take(crate::file_grants::Purpose::WordSave, &path)?;
    let result = export_local_word_to(
        app,
        meeting_id,
        title,
        markdown,
        include_transcript,
        include_speakers,
        include_timestamps,
        granted.path.to_string_lossy().into_owned(),
    )
    .await;
    if result.is_err() {
        crate::file_grants::give_back(crate::file_grants::Purpose::WordSave, &granted);
    }
    result
}

async fn export_local_word_to(
    app: AppHandle,
    meeting_id: String,
    title: String,
    markdown: String,
    include_transcript: bool,
    include_speakers: bool,
    include_timestamps: bool,
    path: String,
) -> Result<(), String> {
    crate::file_grants::save_target_parent_exists(Path::new(&path))?;
    if !Path::new(&path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("docx"))
    {
        return Err("Choose a .docx file.".into());
    }
    let state = app.state::<AppState>();
    let created = read_export_date(state.db_manager.pool(), &meeting_id).await?;
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

#[tauri::command]
pub async fn export_local_markdown(
    app: AppHandle,
    meeting_id: String,
    title: String,
    markdown: String,
    include_transcript: bool,
    include_speakers: bool,
    include_timestamps: bool,
    path: String,
) -> Result<(), String> {
    let purpose = crate::file_grants::Purpose::MarkdownSave;
    let granted = crate::file_grants::take(purpose, &path)?;
    let state = app.state::<AppState>();
    let result = export_local_markdown_to(
        state.db_manager.pool(),
        &meeting_id,
        title,
        markdown,
        MarkdownOptions {
            include_transcript,
            include_speakers,
            include_timestamps,
        },
        granted.path.clone(),
    )
    .await;
    if result.is_err() {
        crate::file_grants::give_back(purpose, &granted);
    }
    result
}

struct MarkdownOptions {
    include_transcript: bool,
    include_speakers: bool,
    include_timestamps: bool,
}

async fn export_local_markdown_to(
    pool: &SqlitePool,
    meeting_id: &str,
    title: String,
    summary: String,
    options: MarkdownOptions,
    path: std::path::PathBuf,
) -> Result<(), String> {
    crate::file_grants::save_target_parent_exists(&path)?;
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    {
        return Err("Choose a .md file.".into());
    }

    let created = read_export_date(pool, meeting_id).await?;
    let (duration, calendar_link): (Option<f64>, Option<String>) =
        sqlx::query_as("SELECT recording_duration_s, calendar_link FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_one(pool)
            .await
            .map_err(|_| "Could not read meeting details.")?;
    let attendees = calendar_link
        .as_deref()
        .and_then(|raw| calendar::decode(raw).ok().flatten())
        .map(|link| {
            link.confirmed_names()
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let tags = sqlx::query_scalar("SELECT tag FROM meeting_tags WHERE meeting_id = ? ORDER BY tag")
        .bind(meeting_id)
        .fetch_all(pool)
        .await
        .map_err(|_| "Could not read project tags.")?;
    let bookmarks = read_bookmarks(pool, meeting_id)
        .await?
        .into_iter()
        .map(|bookmark| (bookmark.seconds, bookmark.label))
        .collect();
    let metadata = markdown::Metadata {
        duration,
        attendees,
        tags,
        bookmarks,
    };
    let passages = if options.include_transcript {
        read_export_transcript(pool, meeting_id).await?
    } else {
        Vec::new()
    };
    if summary.trim().is_empty()
        && metadata.bookmarks.is_empty()
        && passages
            .iter()
            .all(|passage| passage.text.trim().is_empty())
    {
        return Err("There are no notes or transcripts to export.".into());
    }
    tokio::task::spawn_blocking(move || {
        let rendered = markdown::render(
            &title,
            &export_meeting_date(&created),
            &metadata,
            &summary,
            &passages,
            options.include_speakers,
            options.include_timestamps,
        );
        atomic_write(&path, rendered.as_bytes())
    })
    .await
    .map_err(|_| "Markdown export was interrupted.")?
}

async fn read_export_date(pool: &SqlitePool, meeting_id: &str) -> Result<String, String> {
    sqlx::query_scalar("SELECT created_at FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_one(pool)
        .await
        .map_err(|_| "Meeting no longer exists.".into())
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

#[derive(sqlx::FromRow)]
struct TranscriptPassage {
    timestamp: String,
    audio_start_time: Option<f64>,
    speaker: Option<String>,
    text: String,
}

async fn read_export_transcript(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Vec<TranscriptPassage>, String> {
    // Use the current saved text and speaker, independently of the UI's current page.
    sqlx::query_as(
        "SELECT timestamp, audio_start_time, speaker, transcript AS text FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time, id",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|_| "Could not read transcript.".into())
}

async fn read_word_transcript(
    pool: &SqlitePool,
    meeting_id: &str,
    speakers: bool,
    timestamps: bool,
) -> Result<String, String> {
    Ok(read_export_transcript(pool, meeting_id)
        .await?
        .into_iter()
        .filter(|passage| !passage.text.trim().is_empty())
        .map(
            |TranscriptPassage {
                 timestamp,
                 audio_start_time,
                 speaker,
                 text,
             }| {
                let time = if timestamps {
                    export_timestamp(&timestamp, audio_start_time)
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
            },
        )
        .collect::<Vec<_>>()
        .join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn archive_tags_are_additive_and_a_full_meeting_rolls_back_the_batch() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        for id in ["bulk-a", "bulk-b"] {
            sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
                .bind(id).bind(id).execute(&pool).await.unwrap();
        }
        let ids = || vec!["bulk-a".to_owned(), "bulk-b".to_owned()];
        add_tags_to_meetings(&pool, ids(), vec!["Müller".into(), "Roadmap".into()])
            .await
            .unwrap();
        add_tags_to_meetings(&pool, ids(), vec!["MÜLLER".into()])
            .await
            .unwrap();
        add_tags_to_meetings(
            &pool,
            vec!["bulk-b".into()],
            (1..=18).map(|index| format!("Tag {index}")).collect(),
        )
        .await
        .unwrap();
        assert!(add_tags_to_meetings(&pool, ids(), vec!["New tag".into()])
            .await
            .is_err());
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT tag FROM meeting_tags WHERE meeting_id = 'bulk-a' ORDER BY tag",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(tags, ["Müller", "Roadmap"]);
    }

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

    #[tokio::test]
    async fn markdown_export_writes_current_saved_content() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query(
            "UPDATE transcripts SET transcript = 'Korrigiert.', original_transcript = 'Old draft',
             speaker = 'Änne' WHERE id = 'a'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO meeting_tags VALUES ('review-test', 'Planning')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO meeting_bookmarks (id, meeting_id, seconds, label)
             VALUES ('bookmark-fixture', 'review-test', 12, 'Decision')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("meeting.md");
        export_local_markdown_to(
            &pool,
            "review-test",
            "Synthetic review".into(),
            "- **Reviewed**".into(),
            MarkdownOptions {
                include_transcript: true,
                include_speakers: true,
                include_timestamps: true,
            },
            path.clone(),
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(path).unwrap();
        assert!(written.contains("- Project tags: Planning"));
        assert!(written.contains("## Summary\n\n- **Reviewed**"));
        assert!(written.contains("## Notes and bookmarks\n\n- **[00:00:12]** Decision"));
        assert!(written.contains("**[00:00:00] Änne:** Korrigiert."));
        assert!(!written.contains("Old draft"));
        assert!(!written.contains("review-test"));
        assert!(!written.contains('\r'));
        assert!(written.ends_with('\n'));
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
        sqlx::query("INSERT INTO meeting_bookmarks (id, meeting_id, folder_path, seconds, label) VALUES ('mark', NULL, 'synthetic-folder', 12.5, 'Decision')").execute(&pool).await.unwrap();
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
    async fn live_notes_follow_the_recording_id_through_rollback_and_recovery() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::raw_sql(
            "UPDATE meetings SET folder_path = 'old-folder' WHERE id = 'review-test';
             INSERT INTO meeting_bookmarks (id, folder_path, seconds, label, recording_id) VALUES
             ('a', 'old-folder', 11.5, 'A note', 'recording-a'),
             ('b', 'old-folder', 23.0, 'B note', 'recording-b');
             INSERT INTO recording_summary_bookmarks (recording_id) VALUES ('recording-a');",
        )
        .execute(&pool)
        .await
        .unwrap();
        attach_bookmarks(&pool).await.unwrap();
        let owners = || {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT id, meeting_id FROM meeting_bookmarks ORDER BY id",
            )
            .fetch_all(&pool)
        };
        let unattached = [
            (String::from("a"), None::<String>),
            (String::from("b"), None),
        ];
        assert_eq!(owners().await.unwrap(), unattached);

        let insert = "INSERT INTO meetings (id, title, folder_path, created_at, updated_at)
                      VALUES ('recording-a', 'Recovered meeting', 'moved-folder', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)";
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(insert).execute(&mut *tx).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(owners().await.unwrap(), unattached);

        sqlx::query(insert).execute(&pool).await.unwrap();
        let recovered = (String::from("a"), Some(String::from("recording-a")));
        assert_eq!(
            owners().await.unwrap(),
            [recovered.clone(), (String::from("b"), None)]
        );
        let saved: (f64, String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT seconds, label, folder_path, recording_id FROM meeting_bookmarks WHERE id = 'a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(saved, (11.5, String::from("A note"), None, None));
        let context = super::summary_bookmarks::append_context(&pool, "recording-a", String::new())
            .await
            .unwrap();
        assert!(context.ends_with("- [00:00:11] A note"));
        assert!(!context.contains("B note"));
        discard_recording_drafts(&pool, "recording-b")
            .await
            .unwrap();
        assert_eq!(owners().await.unwrap(), [recovered]);
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
        sqlx::query("INSERT INTO meeting_bookmarks (id, meeting_id, folder_path, seconds, label) VALUES ('mark', NULL, 'synthetic-folder', 12.5, 'Review')").execute(&pool).await.unwrap();
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
