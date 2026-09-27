use crate::api::{TranscriptSearchResult, TranscriptSegment};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqlitePool};
use tracing::{error, info};
use uuid::Uuid;

pub struct TranscriptsRepository;

#[cfg(test)]
mod recording_save_tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_saves_and_later_retries_reuse_one_meeting() {
        let root = tempfile::tempdir().unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.path().join("library.sqlite"))
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let segments: Vec<TranscriptSegment> = vec![serde_json::from_value(serde_json::json!({
            "id": "synthetic", "text": "Synthetic retained sentence", "timestamp": "00:01"
        }))
        .unwrap()];
        let outcome = crate::audio::outcome::RecordingOutcome {
            capture_incomplete: true,
            ..Default::default()
        };
        let save = || {
            TranscriptsRepository::save_transcript_with_outcome(
                &pool,
                "Synthetic meeting",
                &segments,
                Some("synthetic-recording".into()),
                Some(&outcome),
            )
        };
        let (first, second) = tokio::join!(save(), save());
        let id = first.unwrap();
        assert_eq!(id, second.unwrap());
        assert_eq!(
            id,
            TranscriptsRepository::save_transcript(
                &pool,
                "Retry",
                &[],
                Some("synthetic-recording".into())
            )
            .await
            .unwrap()
        );
        let (meetings,): (i64,) = sqlx::query_as("SELECT count(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        let (transcripts,): (i64,) = sqlx::query_as("SELECT count(*) FROM transcripts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((meetings, transcripts), (1, 1));
        let saved: crate::audio::outcome::RecordingOutcome =
            sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(saved.capture_incomplete && !saved.audio_save_failed);
        pool.close().await;
    }
}

impl TranscriptsRepository {
    pub async fn update_transcript_speaker(
        pool: &SqlitePool,
        meeting_id: &str,
        transcript_id: &str,
        speaker: Option<&str>,
    ) -> Result<u64, SqlxError> {
        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;
        crate::database::transcript_edits::assert_editable(&mut transaction, meeting_id)
            .await
            .map_err(SqlxError::Protocol)?;
        let now = Utc::now();

        let result =
            sqlx::query("UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND id = ?")
                .bind(speaker)
                .bind(meeting_id)
                .bind(transcript_id)
                .execute(&mut *transaction)
                .await?;

        if result.rows_affected() > 0 {
            sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;
        }

        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn update_transcript_speakers_matching(
        pool: &SqlitePool,
        meeting_id: &str,
        from_speaker: Option<&str>,
        speaker: Option<&str>,
    ) -> Result<u64, SqlxError> {
        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;
        crate::database::transcript_edits::assert_editable(&mut transaction, meeting_id)
            .await
            .map_err(SqlxError::Protocol)?;
        let now = Utc::now();

        let result = match from_speaker {
            Some(from) => {
                sqlx::query(
                    "UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND speaker = ?",
                )
                .bind(speaker)
                .bind(meeting_id)
                .bind(from)
                .execute(&mut *transaction)
                .await?
            }
            None => {
                sqlx::query(
                    "UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND (speaker IS NULL OR TRIM(speaker) = '')",
                )
                .bind(speaker)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?
            }
        };

        if result.rows_affected() > 0 {
            sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;
        }

        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    /// Saves a new meeting and its associated transcript segments.
    /// This function uses a transaction to ensure that either both the meeting
    /// and all its transcripts are saved, or none of them are.
    pub async fn save_transcript(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
    ) -> Result<String, SqlxError> {
        Self::save_transcript_with_outcome(pool, meeting_title, transcripts, folder_path, None)
            .await
    }

    pub async fn save_transcript_with_outcome(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
        outcome: Option<&crate::audio::outcome::RecordingOutcome>,
    ) -> Result<String, SqlxError> {
        let meeting_id = format!("meeting-{}", Uuid::new_v4());

        let mut conn = pool.acquire().await?;
        // Acquire the write reservation before checking the folder, so concurrent
        // stop/retry saves cannot both create a meeting for this recording.
        let mut transaction = conn.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(folder) = folder_path.as_ref().filter(|folder| !folder.is_empty()) {
            if let Some((existing,)) = sqlx::query_as::<_, (String,)>(
                "SELECT id FROM meetings WHERE folder_path = ? LIMIT 1",
            )
            .bind(folder)
            .fetch_optional(&mut *transaction)
            .await?
            {
                transaction.commit().await?;
                return Ok(existing);
            }
        }

        let now = Utc::now();

        // 1. Create the new meeting
        let result = sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(meeting_title)
        .bind(now)
        .bind(now)
        .bind(&folder_path)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = result {
            error!("Failed to create meeting");
            transaction.rollback().await?;
            return Err(e);
        }

        info!("Successfully created meeting with id: {}", meeting_id);

        // 2. Save each transcript segment with audio timing fields
        for segment in transcripts {
            let transcript_id = format!("transcript-{}", Uuid::new_v4());
            let word_timestamps_json = segment
                .word_timestamps
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| SqlxError::Protocol(format!("Invalid word timestamps: {}", e)))?;
            let result = sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker, word_timestamps_json)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&transcript_id)
            .bind(&meeting_id)
            .bind(&segment.text)
            .bind(&segment.timestamp)
            .bind(segment.audio_start_time)
            .bind(segment.audio_end_time)
            .bind(segment.duration)
            .bind(&segment.speaker)
            .bind(word_timestamps_json)
            .execute(&mut *transaction)
            .await;

            if let Err(e) = result {
                error!("Failed to save transcript segment");
                transaction.rollback().await?;
                return Err(e);
            }
        }

        info!(
            "Successfully saved {} transcript segments for meeting {}",
            transcripts.len(),
            meeting_id
        );

        if let Some(outcome) = outcome {
            sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete, capture_incomplete, recording_files_incomplete) VALUES (?, ?, ?, ?, ?)")
                .bind(&meeting_id).bind(outcome.audio_save_failed).bind(outcome.transcription_incomplete)
                .bind(outcome.capture_incomplete).bind(outcome.recording_files_incomplete)
                .execute(&mut *transaction).await?;
        }
        // Commit the meeting, transcript and recovery status together.
        transaction.commit().await?;

        Ok(meeting_id)
    }

    /// Searches for a query string within the transcripts.
    /// It returns a list of matching transcripts with context.
    pub async fn search_transcripts(
        pool: &SqlitePool,
        query: &str,
    ) -> Result<Vec<TranscriptSearchResult>, SqlxError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let search_query = format!("%{}%", query.to_lowercase());

        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT m.id, m.title, t.transcript, t.timestamp
             FROM meetings m
             JOIN transcripts t ON m.id = t.meeting_id
             WHERE LOWER(t.transcript) LIKE ?",
        )
        .bind(&search_query)
        .fetch_all(pool)
        .await?;

        let results = rows
            .into_iter()
            .map(|(id, title, transcript, timestamp)| {
                let match_context = Self::get_match_context(&transcript, query);
                TranscriptSearchResult {
                    id,
                    title,
                    match_context,
                    timestamp,
                }
            })
            .collect();

        Ok(results)
    }

    /// Helper function to extract a snippet of text around the first match of a query.
    fn get_match_context(transcript: &str, query: &str) -> String {
        let transcript_lower = transcript.to_lowercase();
        let query_lower = query.to_lowercase();

        match transcript_lower.find(&query_lower) {
            Some(match_index) => {
                let start_index = match_index.saturating_sub(100);
                let end_index = (match_index + query.len() + 100).min(transcript.len());

                let mut context = String::new();
                if start_index > 0 {
                    context.push_str("...");
                }
                context.push_str(&transcript[start_index..end_index]);
                if end_index < transcript.len() {
                    context.push_str("...");
                }
                context
            }
            None => transcript.chars().take(200).collect(), // Fallback to the start of the transcript
        }
    }
}
