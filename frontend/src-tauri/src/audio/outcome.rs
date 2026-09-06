use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecordingOutcome {
    pub audio_save_failed: bool,
    pub transcription_incomplete: bool,
}

impl RecordingOutcome {
    pub fn needs_recovery(&self) -> bool {
        self.audio_save_failed || self.transcription_incomplete
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
    sqlx::query_as("SELECT audio_save_failed, transcription_incomplete FROM recording_outcomes WHERE meeting_id = ?")
        .bind(meeting_id).fetch_optional(state.db_manager.pool()).await
        .map_err(|_| "Could not read meeting recovery status".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_status_survives_restart_and_corruption_is_not_success() {
        let folder = tempfile::tempdir().unwrap();
        assert!(RecordingOutcome::read(folder.path()).unwrap().is_none());
        RecordingOutcome {
            audio_save_failed: true,
            transcription_incomplete: true,
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
