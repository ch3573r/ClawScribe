use crate::api::{TranscriptSearchResult, TranscriptSegment};
use crate::audio::provenance::RecordingProvenance;
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqlitePool};
use tracing::{error, info};
use uuid::Uuid;

pub struct TranscriptsRepository;

#[cfg(test)]
mod recording_save_tests {
    use super::*;

    #[tokio::test]
    async fn a_manual_title_survives_handoff_and_keeps_the_calendar_link() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let current = sqlx::migrate!("./migrations");
        let before_titles = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                current
                    .iter()
                    .filter(|migration| migration.version < 20261122004800)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        before_titles.run(&pool).await.unwrap();
        let id = "meeting-26f07e03-9b02-41d1-996f-a4216de6b2df";
        let link = r#"{"source":"ical","sourceId":"calendar","eventId":"event","subject":"Invitation","attendees":[]}"#;
        sqlx::query("INSERT INTO recording_calendar_drafts VALUES (?, ?, 1)")
            .bind(id)
            .bind(link)
            .execute(&pool)
            .await
            .unwrap();
        current.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO recording_title_drafts VALUES (?, ?)")
            .bind(id)
            .bind("My meeting title")
            .execute(&pool)
            .await
            .unwrap();
        for title in ["Default title", "Duplicate attempt"] {
            TranscriptsRepository::save_recording_handoff(
                &pool,
                id,
                title,
                &[],
                "synthetic-folder".into(),
                &Default::default(),
                RecordingProvenance::recording(None, None),
            )
            .await
            .unwrap();
        }
        let saved: (String, String) =
            sqlx::query_as("SELECT title, calendar_link FROM active_meetings WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(saved, ("My meeting title".into(), link.into()));
        let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recording_title_drafts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(drafts, 0);
    }

    #[tokio::test]
    async fn a_recovered_recording_uses_its_start_time_even_when_saved_another_day() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        let marker = serde_json::json!({
            "recording_mode": "audio_only",
            "created_at": "2025-07-01T00:01:02+02:00"
        });
        let provenance =
            crate::audio::interrupted::recovered_provenance(folder.path(), &marker, None);
        let outcome = crate::audio::outcome::RecordingOutcome::default();
        let id = "meeting-5dd2d62d-dc83-40af-87f9-33e4eefbc609";
        let path = folder.path().to_string_lossy().into_owned();
        for facts in [provenance, RecordingProvenance::default()] {
            TranscriptsRepository::save_recording_handoff(
                &pool,
                id,
                "Overnight recording",
                &[],
                path.clone(),
                &outcome,
                facts,
            )
            .await
            .unwrap();
        }
        let (created, updated): (chrono::DateTime<Utc>, chrono::DateTime<Utc>) =
            sqlx::query_as("SELECT created_at, updated_at FROM active_meetings WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            created,
            "2025-06-30T22:01:02Z"
                .parse::<chrono::DateTime<Utc>>()
                .unwrap()
        );
        assert!(updated > created);
        for bad in [serde_json::Value::Null, serde_json::json!("yesterday")] {
            let marker = serde_json::json!({"source": "recording", "created_at": bad});
            assert!(
                crate::audio::interrupted::recovered_provenance(folder.path(), &marker, None)
                    .started_at
                    .is_none()
            );
        }
        let imported =
            serde_json::json!({"source": "import", "created_at": "2025-06-30T22:01:02Z"});
        assert!(
            crate::audio::interrupted::recovered_provenance(folder.path(), &imported, None)
                .started_at
                .is_none()
        );
    }

    #[tokio::test]
    async fn handoff_retries_after_rollback_but_never_after_deletion() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let id = "meeting-4f7b980e-b123-4ca9-82e7-927204456483";
        let link = serde_json::json!({
            "source": "ical", "sourceId": "calendar", "eventId": "event", "subject": "Synthetic invitation",
            "attendees": [{ "name": "Unreviewed invitee", "email": null, "attendance": "unreviewed" }]
        }).to_string();
        sqlx::query("INSERT INTO recording_calendar_drafts (recording_id, calendar_link, revision) VALUES (?, ?, 2)")
            .bind(id).bind(&link).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meeting_bookmarks (id, recording_id, folder_path, seconds, label) VALUES ('note', ?, 'old-folder', 42.0, 'Check the deployment')")
            .bind(id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO recording_summary_bookmarks (recording_id) VALUES (?)")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let outcome = crate::audio::outcome::RecordingOutcome::default();
        let save = || {
            TranscriptsRepository::save_recording_handoff(
                &pool,
                id,
                "Synthetic meeting",
                &[],
                "synthetic-recording".into(),
                &outcome,
                RecordingProvenance::recording(Some(93.5), None),
            )
        };
        sqlx::raw_sql("CREATE TRIGGER fail_outcome BEFORE INSERT ON recording_outcomes BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;")
            .execute(&pool).await.unwrap();
        assert!(save().await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT calendar_link FROM recording_calendar_drafts WHERE recording_id = ?"
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            link
        );
        let pending: Option<String> =
            sqlx::query_scalar("SELECT recording_id FROM meeting_bookmarks WHERE id = 'note'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending.as_deref(), Some(id));
        sqlx::query("DROP TRIGGER fail_outcome")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(save().await.unwrap(), id);
        assert_eq!(
            TranscriptsRepository::save_recording_handoff(
                &pool,
                id,
                "Duplicate",
                &[],
                "original-before-restore".into(),
                &outcome,
                RecordingProvenance::recording(Some(500.0), None),
            )
            .await
            .unwrap(),
            id
        );
        let (title, folder, saved_link, source, duration): (String, String, String, String, f64) =
            sqlx::query_as("SELECT title, folder_path, calendar_link, recording_source, recording_duration_s FROM active_meetings WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title, "Synthetic invitation");
        assert_eq!(folder, "synthetic-recording");
        assert_eq!(saved_link, link);
        assert_eq!(source, "recording");
        assert_eq!(duration, 93.5);
        let context = crate::library::summary_bookmarks::append_context(&pool, id, String::new())
            .await
            .unwrap();
        assert!(context.contains("[00:00:42] Check the deployment"));
        assert!(crate::library::calendar::read_snapshot(&pool, id)
            .await
            .unwrap()
            .1
            .for_provider(title)
            .is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recording_calendar_drafts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DELETE FROM meetings WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(save().await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn recovered_notes_attach_by_id_and_stay_local_without_consent() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let id = "meeting-4f7b980e-b123-4ca9-82e7-927204456483";
        sqlx::query("INSERT INTO meeting_bookmarks (id, recording_id, folder_path, seconds, label) VALUES ('private', ?, 'original-folder', 5.0, 'Local only')")
            .bind(id).execute(&pool).await.unwrap();
        TranscriptsRepository::save_recording_handoff(
            &pool,
            id,
            "Recovered meeting",
            &[],
            "relocated-folder".into(),
            &Default::default(),
            RecordingProvenance::recording(None, None),
        )
        .await
        .unwrap();
        let saved_id: String =
            sqlx::query_scalar("SELECT meeting_id FROM meeting_bookmarks WHERE id = 'private'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(saved_id, id);
        assert_eq!(
            crate::library::summary_bookmarks::append_context(&pool, id, "Saved context".into())
                .await
                .unwrap(),
            "Saved context"
        );
    }

    #[tokio::test]
    async fn sign_out_clears_the_unreviewed_draft_and_keeps_reviewed_attendance() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let ids = ["meeting-unreviewed", "meeting-reviewed"];
        for (id, attendance) in ids.into_iter().zip(["unreviewed", "present"]) {
            let link = serde_json::json!({
                "source": "microsoft", "eventId": id, "subject": "Synthetic invitation",
                "attendees": [{ "name": "Synthetic invitee", "attendance": attendance }],
            })
            .to_string();
            sqlx::query("INSERT INTO recording_calendar_drafts VALUES (?, ?, 1)")
                .bind(id)
                .bind(link)
                .execute(&pool)
                .await
                .unwrap();
        }
        crate::library::calendar::clear_microsoft(&pool)
            .await
            .unwrap();
        let cleared: (String, i64) = sqlx::query_as(
            "SELECT calendar_link, revision FROM recording_calendar_drafts WHERE recording_id = ?",
        )
        .bind(ids[0])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cleared, ("null".into(), 2));
        for id in ids {
            let saved = TranscriptsRepository::save_recording_handoff(
                &pool,
                id,
                "Meeting 20_11_26_12_00_00",
                &[],
                id.into(),
                &Default::default(),
                RecordingProvenance::recording(None, None),
            )
            .await
            .unwrap();
            assert_eq!(saved, id);
        }
        let title: String = sqlx::query_scalar("SELECT title FROM meetings WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(title, "Meeting 20_11_26_12_00_00");
        let privacy = crate::library::calendar::read_snapshot(&pool, ids[0])
            .await
            .unwrap()
            .1;
        assert!(privacy
            .for_provider("Synthetic invitation".into())
            .is_none());
        assert!(crate::library::calendar::read(&pool, ids[1])
            .await
            .unwrap()
            .unwrap()
            .is_reviewed());
    }

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
        crate::summary::sources::preserve_legacy_speaker_links(&mut transaction, meeting_id)
            .await?;
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
        crate::summary::sources::preserve_legacy_speaker_links(&mut transaction, meeting_id)
            .await?;
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
        Self::save_recording(
            pool,
            meeting_title,
            transcripts,
            folder_path,
            outcome,
            None,
            RecordingProvenance::default(),
        )
        .await
    }

    pub(crate) async fn save_recording(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
        outcome: Option<&crate::audio::outcome::RecordingOutcome>,
        calendar_link: Option<String>,
        provenance: RecordingProvenance,
    ) -> Result<String, SqlxError> {
        let meeting_id = format!("meeting-{}", Uuid::new_v4());
        Self::save_with_id(
            pool,
            meeting_id,
            meeting_title,
            transcripts,
            folder_path,
            outcome,
            calendar_link,
            false,
            provenance,
        )
        .await
    }

    pub(crate) async fn save_recording_handoff(
        pool: &SqlitePool,
        recording_id: &str,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: String,
        outcome: &crate::audio::outcome::RecordingOutcome,
        provenance: RecordingProvenance,
    ) -> Result<String, SqlxError> {
        Self::save_with_id(
            pool,
            recording_id.to_owned(),
            meeting_title,
            transcripts,
            Some(folder_path),
            Some(outcome),
            None,
            true,
            provenance,
        )
        .await
    }

    /// The native recording ID is sufficient to save the text without
    /// attaching an unverified path. Audio repair can later bind a trusted
    /// folder whose recording marker carries this same ID.
    pub(crate) async fn save_offline_recording_transcript(
        pool: &SqlitePool,
        recording_id: &str,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        started_at_ms: i64,
    ) -> Result<String, SqlxError> {
        Self::save_with_id(
            pool,
            recording_id.to_owned(),
            meeting_title,
            transcripts,
            None,
            Some(&crate::audio::outcome::RecordingOutcome {
                audio_save_failed: true,
                transcription_incomplete: true,
                ..Default::default()
            }),
            None,
            true,
            RecordingProvenance {
                started_at: chrono::DateTime::from_timestamp_millis(started_at_ms),
                ..RecordingProvenance::recording(None, None)
            },
        )
        .await
    }

    async fn save_with_id(
        pool: &SqlitePool,
        meeting_id: String,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
        outcome: Option<&crate::audio::outcome::RecordingOutcome>,
        calendar_link: Option<String>,
        has_recording_id: bool,
        provenance: RecordingProvenance,
    ) -> Result<String, SqlxError> {
        let mut conn = pool.acquire().await?;
        // Acquire the write reservation before checking the folder, so concurrent
        // stop/retry saves cannot both create a meeting for this recording.
        let mut transaction = conn.begin_with("BEGIN IMMEDIATE").await?;
        let existing: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?)")
                .bind(&meeting_id)
                .fetch_one(&mut *transaction)
                .await?;
        if existing {
            if has_recording_id && folder_path.is_none() {
                use futures_util::TryStreamExt;
                let mut saved = sqlx::query_as::<_, (String, String, Option<f64>, Option<f64>, Option<f64>, Option<String>, Option<String>)>(
                    "SELECT t.transcript,t.timestamp,t.audio_start_time,t.audio_end_time,t.duration,t.speaker,t.word_timestamps_json
                     FROM transcripts t JOIN meetings m ON m.id=t.meeting_id
                     JOIN recording_outcomes o ON o.meeting_id=m.id
                     WHERE m.id=? AND m.folder_path IS NULL AND m.recording_source='recording'
                       AND o.audio_save_failed=1 ORDER BY t.rowid",
                ).bind(&meeting_id).fetch(&mut *transaction);
                let conflict =
                    || SqlxError::Protocol("The saved meeting differs from this draft".into());
                for segment in transcripts {
                    let (text, time, start, end, duration, speaker, words) =
                        saved.try_next().await?.ok_or_else(conflict)?;
                    let word_json = segment
                        .word_timestamps
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|_| conflict())?;
                    if text != segment.text
                        || time != segment.timestamp
                        || start != segment.audio_start_time
                        || end != segment.audio_end_time
                        || duration != segment.duration
                        || speaker != segment.speaker
                        || words != word_json
                    {
                        return Err(conflict());
                    }
                }
                if saved.try_next().await?.is_some() {
                    return Err(conflict());
                }
            }
            transaction.commit().await?;
            return Ok(meeting_id);
        }
        let deleted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM recording_library_deletions WHERE meeting_id = ?)",
        )
        .bind(&meeting_id)
        .fetch_one(&mut *transaction)
        .await?;
        if deleted {
            return Err(SqlxError::Protocol(
                "This meeting was deleted from the library".into(),
            ));
        }
        if let Some(folder) = folder_path.as_ref().filter(|folder| !folder.is_empty()) {
            if let Some((existing,)) = sqlx::query_as::<_, (String,)>(
                "SELECT id FROM meetings WHERE folder_path = ? LIMIT 1",
            )
            .bind(folder)
            .fetch_optional(&mut *transaction)
            .await?
            {
                if has_recording_id && existing != meeting_id {
                    return Err(SqlxError::Protocol(
                        "The recording folder belongs to another meeting".into(),
                    ));
                }
                transaction.commit().await?;
                return Ok(existing);
            }
        }

        let draft = sqlx::query_scalar::<_, String>(
            "DELETE FROM recording_calendar_drafts WHERE recording_id = ? RETURNING calendar_link",
        )
        .bind(&meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let calendar_title = draft
            .as_deref()
            .and_then(|stored| crate::library::calendar::decode(stored).ok().flatten());
        let custom_title = if has_recording_id {
            sqlx::query_scalar::<_, String>(
                "DELETE FROM recording_title_drafts WHERE recording_id = ? RETURNING title",
            )
            .bind(&meeting_id)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            None
        };
        let meeting_title = custom_title
            .as_deref()
            .or_else(|| calendar_title.as_ref().and_then(|link| link.subject()))
            .unwrap_or(meeting_title);
        let now = Utc::now();

        // 1. Create the new meeting
        let result = sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path, calendar_link,
                                   recording_source, recording_duration_s)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(meeting_title)
        .bind(provenance.started_at.unwrap_or(now))
        .bind(now)
        .bind(&folder_path)
        .bind(draft.or(calendar_link))
        .bind(provenance.source)
        .bind(provenance.duration_seconds)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = result {
            error!("Failed to create meeting");
            transaction.rollback().await?;
            return Err(e);
        }
        if has_recording_id {
            sqlx::query(
                "UPDATE saved_live_answers SET meeting_id = ?1
                WHERE recording_id = ?1 AND meeting_id IS NULL",
            )
            .bind(&meeting_id)
            .execute(&mut *transaction)
            .await?;
        }

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

        info!("Saved {} transcript segments", transcripts.len());

        if let Some(outcome) = outcome {
            sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete, capture_incomplete, recording_files_incomplete, capture_details) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(&meeting_id).bind(outcome.audio_save_failed).bind(outcome.transcription_incomplete)
                .bind(outcome.capture_incomplete).bind(outcome.recording_files_incomplete)
                .bind(outcome.capture_details.as_ref().map(sqlx::types::Json))
                .execute(&mut *transaction).await?;
        }
        // Commit the meeting, transcript and recovery status together.
        transaction.commit().await?;

        Ok(meeting_id)
    }

    /// Legacy IPC shape for bounded literal searches of current transcripts.
    pub async fn search_transcripts(
        pool: &SqlitePool,
        query: &str,
    ) -> Result<Vec<TranscriptSearchResult>, crate::knowledge::types::KnowledgeError> {
        let hits =
            crate::knowledge::store::literal::desktop(pool, query, None, false, false).await?;
        Ok(hits
            .passages
            .into_iter()
            .map(|hit| TranscriptSearchResult {
                id: hit.meeting_id,
                title: hit.title,
                match_context: hit.text,
                timestamp: hit.timestamp,
            })
            .collect())
    }
}
