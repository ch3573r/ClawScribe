use futures_util::TryStreamExt;
use sqlx::{Row, SqliteConnection, SqlitePool};

use super::{
    delivery::Failure,
    payload::{Content, Meeting, Passage, Transcript, MAX_BODY_BYTES},
    settings::Switches,
};
use crate::{database::models::meeting_timestamp, library::calendar};

pub(super) async fn load(
    pool: &SqlitePool,
    meeting_id: String,
    generated_markdown: Option<String>,
    switches: Switches,
) -> Result<Content, Failure> {
    let unavailable = |_| Failure::MeetingUnavailable;
    let mut tx = pool.begin().await.map_err(unavailable)?;
    let row = sqlx::query(
        "SELECT title, created_at, recording_duration_s, calendar_link
         FROM meetings WHERE id = ?",
    )
    .bind(&meeting_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(unavailable)?;
    let project_tags = sqlx::query_scalar(
        "SELECT tag FROM meeting_tags WHERE meeting_id = ? ORDER BY tag COLLATE NOCASE, tag",
    )
    .bind(&meeting_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(unavailable)?;
    let attendees = row
        .try_get::<Option<&str>, _>("calendar_link")
        .map_err(unavailable)?
        .and_then(|raw| calendar::decode(raw).ok().flatten())
        .filter(calendar::CalendarLink::is_reviewed)
        .map(|link| {
            link.confirmed_names()
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let summary = if switches.include_summary {
        match generated_markdown {
            Some(markdown) => Some(markdown),
            None => saved_markdown(&mut tx, &meeting_id)
                .await
                .map_err(unavailable)?,
        }
    } else {
        None
    };
    let transcript = if switches.include_transcript {
        saved_transcript(&mut tx, &meeting_id)
            .await
            .map_err(unavailable)?
    } else {
        Transcript::Absent
    };
    tx.commit().await.map_err(unavailable)?;
    Ok(Content {
        meeting: Meeting {
            id: meeting_id,
            title: row.try_get("title").map_err(unavailable)?,
            started_at: meeting_timestamp(&row, "created_at").map_err(unavailable)?,
            duration_seconds: row.try_get("recording_duration_s").map_err(unavailable)?,
            project_tags,
            attendees,
        },
        summary,
        transcript,
    })
}

async fn saved_markdown(
    connection: &mut SqliteConnection,
    meeting: &str,
) -> Result<Option<String>, sqlx::Error> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT CASE WHEN LOWER(status) = 'completed' THEN result ELSE result_backup END
         FROM summary_processes WHERE meeting_id = ?",
    )
    .bind(meeting)
    .fetch_optional(connection)
    .await?
    .flatten();
    let Some(raw) = raw else { return Ok(None) };
    // Current summaries store Markdown. The visible legacy text is safe to
    // fall back to when older summaries contain only editor blocks.
    let markdown = serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| {
            crate::summary::processor::validate_saved_summary(&value).ok()?;
            value.get("markdown")?.as_str().map(str::to_owned)
        })
        .filter(|value| !value.trim().is_empty())
        .or_else(|| crate::library::saved_text::summary_text(&raw));
    Ok(markdown)
}

async fn saved_transcript(
    connection: &mut SqliteConnection,
    meeting: &str,
) -> Result<Transcript, sqlx::Error> {
    // Edits and renamed speakers update these same columns. Stream a bounded
    // projection without fetching originals, IDs, timestamps or word alignment.
    let mut rows = sqlx::query_as::<_, Passage>(
        "SELECT audio_start_time AS start_seconds, audio_end_time AS end_seconds,
                NULLIF(TRIM(speaker), '') AS speaker,
                CASE WHEN length(CAST(transcript AS BLOB)) + COALESCE(length(CAST(speaker AS BLOB)), 0) <= ?
                     THEN transcript ELSE '' END AS text
         FROM transcripts WHERE meeting_id = ? AND TRIM(transcript) <> ''
         ORDER BY audio_start_time, id",
    )
    .bind(MAX_BODY_BYTES as i64)
    .bind(meeting)
    .fetch(connection);
    let mut passages = Vec::new();
    let mut text_bytes = 0;
    while let Some(row) = rows.try_next().await? {
        text_bytes += row.text.len() + row.speaker.as_ref().map_or(0, String::len) + 24;
        if row.text.is_empty() || text_bytes > MAX_BODY_BYTES {
            return Ok(Transcript::TooLarge);
        }
        passages.push(row);
    }
    Ok(Transcript::Passages(passages))
}
