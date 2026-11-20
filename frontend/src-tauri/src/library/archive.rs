//! Small archive projections. Transcript and summary contents stay in SQLite.
pub(crate) mod storage;

use crate::state::AppState;
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct ArchiveRow {
    id: String,
    has_folder: bool,
    has_transcript: bool,
    has_summary: bool,
    summary_status: Option<String>,
    audio_save_failed: bool,
    transcription_incomplete: bool,
    capture_incomplete: bool,
    recording_files_incomplete: bool,
    recovery_files_elsewhere: bool,
    pending_file_sync: bool,
}

async fn archive_rows(pool: &SqlitePool) -> Result<Vec<ArchiveRow>, String> {
    sqlx::query_as(
        "SELECT m.id, COALESCE(TRIM(m.folder_path) <> '', 0) AS has_folder,
                EXISTS(SELECT 1 FROM transcripts t WHERE t.meeting_id = m.id) AS has_transcript,
                (s.result IS NOT NULL OR s.result_backup IS NOT NULL) AS has_summary,
                LOWER(s.status) AS summary_status,
                COALESCE(o.audio_save_failed, 0) AS audio_save_failed,
                COALESCE(o.transcription_incomplete, 0) AS transcription_incomplete,
                COALESCE(o.capture_incomplete, 0) AS capture_incomplete,
                COALESCE(o.recording_files_incomplete, 0) AS recording_files_incomplete,
                COALESCE(o.recovery_files_elsewhere, 0) AS recovery_files_elsewhere,
                EXISTS(SELECT 1 FROM transcript_file_sync f WHERE f.meeting_id = m.id) AS pending_file_sync
         FROM meetings m LEFT JOIN summary_processes s ON s.meeting_id = m.id
         LEFT JOIN recording_outcomes o ON o.meeting_id = m.id",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| "Could not load archive status".into())
}

#[tauri::command]
pub(crate) async fn list_archive_rows(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ArchiveRow>, String> {
    archive_rows(state.db_manager.pool()).await
}
