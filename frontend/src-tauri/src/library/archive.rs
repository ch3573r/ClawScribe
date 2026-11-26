//! Small archive projections. Transcript and summary contents stay in SQLite.
pub(crate) mod storage;

use crate::state::AppState;
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Serialize)]
struct ArchiveSeries {
    id: String,
    label: String,
    date: String,
}

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
    // Source of the saved audio when known.
    recording_source: Option<String>,
    // Measured active recording time or duration of imported or recovered media.
    duration_seconds: Option<f64>,
    #[serde(skip)]
    calendar_link: Option<String>,
    #[serde(skip)]
    title: String,
    #[serde(skip)]
    created_at: String,
    #[sqlx(skip)]
    series: Option<ArchiveSeries>,
}

async fn archive_rows(pool: &SqlitePool) -> Result<Vec<ArchiveRow>, String> {
    let mut rows: Vec<ArchiveRow> = sqlx::query_as(
        "SELECT m.id, COALESCE(TRIM(m.folder_path) <> '', 0) AS has_folder,
                EXISTS(SELECT 1 FROM transcripts t WHERE t.meeting_id = m.id) AS has_transcript,
                (s.result IS NOT NULL OR s.result_backup IS NOT NULL) AS has_summary,
                LOWER(s.status) AS summary_status,
                COALESCE(o.audio_save_failed, 0) AS audio_save_failed,
                COALESCE(o.transcription_incomplete, 0) AS transcription_incomplete,
                COALESCE(o.capture_incomplete, 0) AS capture_incomplete,
                COALESCE(o.recording_files_incomplete, 0) AS recording_files_incomplete,
                COALESCE(o.recovery_files_elsewhere, 0) AS recovery_files_elsewhere,
                EXISTS(SELECT 1 FROM transcript_file_sync f WHERE f.meeting_id = m.id) AS pending_file_sync,
                m.recording_source,
                m.recording_duration_s AS duration_seconds,
                CASE WHEN length(m.calendar_link) <= 262144 THEN m.calendar_link END calendar_link,
                m.title, m.created_at
         FROM meetings m LEFT JOIN summary_processes s ON s.meeting_id = m.id
         LEFT JOIN recording_outcomes o ON o.meeting_id = m.id
         ORDER BY julianday(m.created_at) DESC, m.id",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| "Could not load archive status")?;
    for row in &mut rows {
        let link = row
            .calendar_link
            .take()
            .and_then(|raw| super::calendar::decode(&raw).ok().flatten());
        if let Some(link) = link {
            if let Some(key) = link.series_key() {
                row.series = Some(ArchiveSeries {
                    id: serde_json::json!(key).to_string(),
                    label: link.subject().unwrap_or(&row.title).to_owned(),
                    date: std::mem::take(&mut row.created_at),
                });
            }
        }
    }
    Ok(rows)
}

#[tauri::command]
pub(crate) async fn list_archive_rows(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ArchiveRow>, String> {
    archive_rows(state.db_manager.pool()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn archive_series_uses_stable_calendar_identity_without_exposing_the_saved_link() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        for (id, series) in [("first", "a"), ("repeat", "a"), ("other", "b")] {
            let link = json!({
                "eventId": id, "source": "microsoft", "subject": "Planning",
                "seriesId": format!("graph:v1:{}", series.repeat(64)),
                "attendees": [{"email":"invitee@example.com"}]
            });
            sqlx::query(
                "INSERT INTO meetings (id,title,created_at,updated_at,calendar_link)
                        VALUES (?,'Saved meeting','2024-01-01','2024-01-01',?)",
            )
            .bind(id)
            .bind(link.to_string())
            .execute(&pool)
            .await
            .unwrap();
        }
        let value = serde_json::to_value(archive_rows(&pool).await.unwrap()).unwrap();
        let series_id = |id| {
            value
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .unwrap()["series"]["id"]
                .as_str()
                .unwrap()
        };
        assert_eq!(series_id("first"), series_id("repeat"));
        assert_ne!(series_id("first"), series_id("other"));
        assert!(!value.to_string().contains("invitee@example.com"));
    }
}
