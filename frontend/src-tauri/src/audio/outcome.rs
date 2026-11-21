use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmptyLiveTranscript {
    NoSpeechDetected,
    NoTextRecognized,
    Incomplete,
}

impl EmptyLiveTranscript {
    pub(crate) fn from_live_result(
        live: bool,
        has_text: bool,
        incomplete: bool,
        queued: u64,
    ) -> Option<Self> {
        if !live || has_text {
            None
        } else if incomplete {
            Some(Self::Incomplete)
        } else if queued == 0 {
            Some(Self::NoSpeechDetected)
        } else {
            Some(Self::NoTextRecognized)
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureDetails {
    pub dropped_samples: u64,
    pub system_silence_periods: u32,
    pub backend: Option<String>,
    pub unfinished_segments: usize,
    pub empty_live_transcript: Option<EmptyLiveTranscript>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecordingOutcome {
    pub audio_save_failed: bool,
    pub transcription_incomplete: bool,
    #[serde(default)]
    #[sqlx(default)]
    pub capture_incomplete: bool,
    #[serde(default)]
    #[sqlx(default)]
    pub recording_files_incomplete: bool,
    #[serde(default)]
    #[sqlx(default)]
    pub recovery_files_elsewhere: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sqlx(default, json(nullable))]
    pub capture_details: Option<CaptureDetails>,
}

impl RecordingOutcome {
    pub fn needs_recovery(&self) -> bool {
        self.audio_save_failed
            || self.transcription_incomplete
            || self.capture_incomplete
            || self.recording_files_incomplete
            || self.recovery_files_elsewhere
    }

    pub(crate) fn read(folder: &std::path::Path) -> Result<Option<Self>, String> {
        match std::fs::read(folder.join("recording-outcome.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| {
                "Recording recovery status is damaged; existing audio was preserved".into()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Recording recovery status could not be read".into()),
        }
    }

    pub(crate) fn write(&self, folder: &std::path::Path) -> Result<(), String> {
        use std::io::Write;
        let staged = folder.join(format!(".recording-outcome-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<(), String> {
            let bytes = serde_json::to_vec(self).map_err(|_| "Invalid recording outcome")?;
            let mut file =
                std::fs::File::create(&staged).map_err(|_| "Could not stage recording status")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "Could not save recording status")?;
            drop(file);
            std::fs::rename(&staged, folder.join("recording-outcome.json"))
                .map_err(|_| "Could not publish recording status")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        result
    }
}

#[tauri::command]
pub async fn get_recording_outcome(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
) -> Result<Option<RecordingOutcome>, String> {
    sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = ?")
        .bind(meeting_id)
        .fetch_optional(state.db_manager.pool())
        .await
        .map_err(|_| "Could not read meeting recovery status".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_live_recording_reports_what_the_pipeline_observed() {
        use EmptyLiveTranscript::*;
        let reason = EmptyLiveTranscript::from_live_result;
        assert_eq!(reason(false, false, false, 0), None);
        assert_eq!(reason(true, true, true, 9), None);
        assert_eq!(reason(true, false, true, 0), Some(Incomplete));
        assert_eq!(reason(true, false, false, 0), Some(NoSpeechDetected));
        assert_eq!(reason(true, false, false, 9), Some(NoTextRecognized));
    }

    #[tokio::test]
    async fn capture_details_round_trip_without_requiring_them_for_existing_meetings() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES
            ('previous', 'Previous', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
            ('recorded', 'Recorded', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
            INSERT INTO recording_outcomes (meeting_id) VALUES ('previous');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let details = CaptureDetails {
            dropped_samples: 480,
            system_silence_periods: 2,
            empty_live_transcript: Some(EmptyLiveTranscript::Incomplete),
            ..Default::default()
        };
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, capture_incomplete, capture_details) VALUES ('recorded', 1, ?)")
            .bind(sqlx::types::Json(&details)).execute(&pool).await.unwrap();
        let old: RecordingOutcome =
            sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = 'previous'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(old.capture_details.is_none());
        let saved: RecordingOutcome =
            sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = 'recorded'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(saved.capture_incomplete);
        let saved = saved.capture_details.unwrap();
        assert_eq!(
            (saved.dropped_samples, saved.system_silence_periods),
            (480, 2)
        );
        assert_eq!(
            saved.empty_live_transcript,
            Some(EmptyLiveTranscript::Incomplete)
        );
    }
    #[tokio::test]
    async fn recovery_location_migration_preserves_previous_version_outcomes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in sqlx::migrate!("./migrations")
            .iter()
            .filter(|migration| migration.version < 20260927000003)
        {
            sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('existing', 'Synthetic meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete) VALUES ('existing', 1, 1)")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20260927000003_recovery_files_elsewhere.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let outcome: RecordingOutcome =
            sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = 'existing'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(outcome.audio_save_failed && outcome.transcription_incomplete);
        assert!(!outcome.recovery_files_elsewhere);
        assert_eq!(
            serde_json::to_value(outcome).unwrap()["recovery_files_elsewhere"],
            false
        );
    }

    #[tokio::test]
    async fn capture_gap_migration_preserves_existing_recovery_status() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in sqlx::migrate!("./migrations")
            .iter()
            .filter(|migration| migration.version < 20260926000001)
        {
            sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('existing', 'Synthetic meeting', '2026-01-01', '2026-01-01')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete) VALUES ('existing', 1, 1)").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20260926000001_recording_capture_gaps.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let outcome: RecordingOutcome =
            sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = 'existing'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(outcome.audio_save_failed && outcome.transcription_incomplete);
        assert!(!outcome.capture_incomplete && !outcome.recording_files_incomplete);
        let legacy: RecordingOutcome =
            serde_json::from_str(r#"{"audio_save_failed":false,"transcription_incomplete":false}"#)
                .unwrap();
        assert!(!legacy.needs_recovery());
    }

    #[test]
    fn recovery_status_survives_restart_and_corruption_is_not_success() {
        let folder = tempfile::tempdir().unwrap();
        assert!(RecordingOutcome::read(folder.path()).unwrap().is_none());
        RecordingOutcome {
            audio_save_failed: true,
            transcription_incomplete: true,
            ..Default::default()
        }
        .write(folder.path())
        .unwrap();
        assert!(RecordingOutcome::read(folder.path())
            .unwrap()
            .unwrap()
            .needs_recovery());
        RecordingOutcome::default().write(folder.path()).unwrap();
        assert!(!RecordingOutcome::read(folder.path())
            .unwrap()
            .unwrap()
            .needs_recovery());
        std::fs::write(folder.path().join("recording-outcome.json"), b"damaged").unwrap();
        assert!(RecordingOutcome::read(folder.path()).is_err());
    }

    #[tokio::test]
    async fn outcomes_migrate_clean_and_existing_databases_without_losing_meetings() {
        for upgrading in [false, true] {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            let migrator = sqlx::migrate!("./migrations");
            if upgrading {
                for migration in migrator
                    .iter()
                    .filter(|migration| migration.version < 20260906000001)
                {
                    sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
                }
                sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('existing', 'Synthetic meeting', '2026-01-01', '2026-01-01')")
                    .execute(&pool).await.unwrap();
                sqlx::raw_sql(include_str!(
                    "../../migrations/20260906000001_recording_outcomes.sql"
                ))
                .execute(&pool)
                .await
                .unwrap();
            } else {
                migrator.run(&pool).await.unwrap();
                sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('existing', 'Synthetic meeting', '2026-01-01', '2026-01-01')")
                    .execute(&pool).await.unwrap();
            }
            sqlx::query("INSERT INTO recording_outcomes (meeting_id, audio_save_failed, transcription_incomplete) VALUES ('existing', 0, 1)")
                .execute(&pool).await.unwrap();
            let outcome: RecordingOutcome =
                sqlx::query_as("SELECT * FROM recording_outcomes WHERE meeting_id = 'existing'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(outcome.transcription_incomplete);
            assert!(!outcome.audio_save_failed);
            assert!(outcome.needs_recovery());
        }
    }
}
