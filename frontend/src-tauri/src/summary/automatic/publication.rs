//! A successful retranscription can outlive the process that was about to dispatch its notes.
use super::{launch_recording, with_preference, Launch};
use crate::{behavior_settings, state::AppState};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::path::Path;
use tauri::{AppHandle, Manager, Runtime};

pub(crate) fn profile_for_folder<R: Runtime>(
    app: &AppHandle<R>,
    folder: &Path,
) -> Result<Option<String>, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "Could not identify the notes profile")?;
    if crate::local_path::lexically_inside(
        &folder.to_string_lossy(),
        &directory.join("restored-recordings"),
    ) {
        return Ok(None);
    }
    with_preference(app, |store| profile_key(store, &directory)).map(Some)
}

pub(super) fn stored_profile<R: Runtime>(app: &AppHandle<R>) -> Result<Option<String>, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "Could not identify the notes profile")?;
    with_preference(app, |store| {
        Ok(saved_nonce(store).map(|nonce| key(nonce, &directory)))
    })
}

const PROFILE_KEY: &str = "retranscription_notes_profile";

fn saved_nonce(store: &dyn behavior_settings::Backing) -> Option<uuid::Uuid> {
    store.read(PROFILE_KEY)?.as_str()?.parse().ok()
}

fn profile_key(store: &dyn behavior_settings::Backing, directory: &Path) -> Result<String, String> {
    let nonce = match saved_nonce(store) {
        Some(saved) => saved,
        None => {
            let nonce = uuid::Uuid::new_v4();
            behavior_settings::persist(store, PROFILE_KEY, serde_json::json!(nonce.to_string()))?;
            nonce
        }
    };
    Ok(key(nonce, directory))
}

fn key(nonce: uuid::Uuid, directory: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(nonce.as_bytes());
    hash.update(crate::local_path::comparable_text(&directory.to_string_lossy()).as_bytes());
    format!("{:x}", hash.finalize())
}

pub(crate) async fn remember(
    tx: &mut Transaction<'_, Sqlite>,
    meeting: &str,
    profile: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO retranscription_notes_retry (meeting_id, profile, publication)
        SELECT id, ?, ? FROM meetings WHERE id = ? AND recording_source IS NOT 'import'
        ON CONFLICT(meeting_id) DO UPDATE SET
        profile = excluded.profile, publication = excluded.publication",
    )
    .bind(profile)
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(meeting)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) fn wake<R: Runtime>(app: &AppHandle<R>, meeting: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let pool = app.state::<AppState>().db_manager.pool().clone();
        if dispatch(&app, &pool, &meeting).await.is_err() {
            log::warn!("Could not dispatch notes after retranscription");
        }
    });
}

async fn dispatch<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting: &str,
) -> Result<Launch, String> {
    consume(
        pool,
        meeting,
        || stored_profile(app),
        launch_recording(app.clone(), pool.clone(), meeting.to_owned()),
    )
    .await
}

async fn consume(
    pool: &SqlitePool,
    meeting: &str,
    current_profile: impl FnOnce() -> Result<Option<String>, String>,
    launch: impl std::future::Future<Output = Result<Launch, String>>,
) -> Result<Launch, String> {
    let marker: Option<(String, String)> = sqlx::query_as(
        "SELECT profile, publication FROM retranscription_notes_retry WHERE meeting_id = ?",
    )
    .bind(meeting)
    .fetch_optional(pool)
    .await
    .map_err(|_| "Could not read pending notes")?;
    let Some((owner, publication)) = marker else {
        return Ok(Launch::Handled);
    };
    let outcome = if current_profile()?.as_ref() == Some(&owner) {
        launch.await?
    } else {
        Launch::Handled
    };
    // An in-memory registration alone isn't a durable job or skip.
    if matches!(outcome, Launch::Busy) {
        return Ok(outcome);
    }
    // A crash before acknowledgement is safe: the normal job claim or skip
    // is already durable. A newer publication keeps its own marker.
    sqlx::query("DELETE FROM retranscription_notes_retry WHERE meeting_id = ? AND publication = ?")
        .bind(meeting)
        .bind(publication)
        .execute(pool)
        .await
        .map_err(|_| "Could not acknowledge pending notes")?;
    Ok(outcome)
}

pub(super) async fn resume<R: Runtime>(app: &AppHandle<R>, pool: &SqlitePool) {
    let mut after = String::new();
    loop {
        let next = sqlx::query_scalar::<_, String>(
            "SELECT meeting_id FROM retranscription_notes_retry WHERE meeting_id > ? ORDER BY meeting_id LIMIT 1",
        ).bind(&after).fetch_optional(pool).await;
        match next {
            Ok(Some(meeting)) => after = meeting,
            Ok(None) => return,
            Err(_) => {
                log::warn!("Could not read pending retranscription notes");
                return;
            }
        }
        match dispatch(app, pool, &after).await {
            Ok(Launch::Started(worker)) => {
                let _ = worker.await;
            }
            Ok(_) => {}
            Err(_) => log::warn!("Could not retry notes after retranscription"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::summary::SummaryProcessesRepository as Jobs;

    async fn publish(pool: &SqlitePool) {
        let mut tx = pool.begin().await.unwrap();
        remember(&mut tx, "review-test", "local").await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn pending(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM retranscription_notes_retry")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn retry_profile_requires_the_original_store_and_location() {
        use crate::behavior_settings::test_memory::Memory;
        let store = Memory::default();
        let first = Path::new("first-profile");
        let key = profile_key(&store, first).unwrap();
        assert_eq!(profile_key(&store, first).unwrap(), key);
        assert_ne!(profile_key(&Memory::default(), first).unwrap(), key);
        assert_ne!(
            profile_key(&store, Path::new("copied-profile")).unwrap(),
            key
        );
    }

    #[tokio::test]
    async fn immediate_and_startup_retries_share_one_durable_claim() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        publish(&pool).await;
        let claims = std::sync::atomic::AtomicUsize::new(0);
        let launch = || async {
            if Jobs::create_automatic_process(&pool, meeting, "recording", "approved")
                .await
                .unwrap()
            {
                claims.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(Launch::Handled)
        };
        let attempt = || consume(&pool, meeting, || Ok(Some("local".into())), launch());
        let (first, second) = tokio::join!(attempt(), attempt());
        first.unwrap();
        second.unwrap();
        assert_eq!(claims.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(pending(&pool).await, 0);
        // Even after acknowledgement, a copied library cannot resume the job
        // if the original process exits before the worker finishes.
        Jobs::fail_interrupted_processes(&pool).await.unwrap();
        for foreign in [None, Some("copied-profile")] {
            assert!(
                Jobs::automatic_resume_candidates(&pool, "approved", foreign)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        let candidates = Jobs::automatic_resume_candidates(&pool, "approved", Some("local"))
            .await
            .unwrap();
        assert_eq!(
            candidates,
            vec![(meeting.to_owned(), "local:approved".to_owned())]
        );
        assert!(
            Jobs::claim_automatic_resume(&pool, meeting, &candidates[0].1)
                .await
                .unwrap()
        );
        assert!(
            !Jobs::claim_automatic_resume(&pool, meeting, &candidates[0].1)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn retry_keeps_transient_errors_and_only_acknowledges_its_own_publication() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let owner = || Ok(Some("local".into()));
        let meeting = "review-test";
        publish(&pool).await;
        assert!(
            consume(&pool, meeting, owner, async { Err("temporary".into()) })
                .await
                .is_err()
        );
        consume(&pool, meeting, owner, async { Ok(Launch::Busy) })
            .await
            .unwrap();
        assert_eq!(pending(&pool).await, 1);
        consume(&pool, meeting, owner, async {
            publish(&pool).await;
            Ok(Launch::Handled)
        })
        .await
        .unwrap();
        assert_eq!(pending(&pool).await, 1);
        consume(
            &pool,
            meeting,
            || Ok(Some("another-profile".into())),
            async { panic!("a copied database must not dispatch another profile's retry") },
        )
        .await
        .unwrap();
        assert_eq!(pending(&pool).await, 0);
    }

    #[tokio::test]
    async fn retry_acknowledges_durable_skips_and_preserves_completed_notes() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let owner = || Ok(Some("local".into()));
        let meeting = "review-test";
        for reason in [super::super::DISABLED, super::super::REVIEW_REQUIRED] {
            publish(&pool).await;
            consume(&pool, meeting, owner, async {
                Jobs::record_automatic_outcome(&pool, meeting, "skipped", reason, "recording")
                    .await
                    .unwrap();
                Ok(Launch::Handled)
            })
            .await
            .unwrap();
            assert_eq!(pending(&pool).await, 0);
            assert!(
                !Jobs::create_automatic_process(&pool, meeting, "recording", "approved")
                    .await
                    .unwrap()
            );
            sqlx::query("DELETE FROM summary_processes")
                .execute(&pool)
                .await
                .unwrap();
        }
        Jobs::create_or_reset_process(&pool, meeting).await.unwrap();
        Jobs::update_process_completed(
            &pool,
            meeting,
            serde_json::json!({"markdown": "Manual notes"}),
            1,
            0.0,
        )
        .await
        .unwrap();
        publish(&pool).await;
        consume(&pool, meeting, owner, async {
            assert!(
                !Jobs::can_start_automatic_process(&pool, meeting, "recording")
                    .await
                    .unwrap()
            );
            Ok(Launch::Handled)
        })
        .await
        .unwrap();
        consume(&pool, meeting, owner, async { panic!("duplicate wake") })
            .await
            .unwrap();
        let result: String = sqlx::query_scalar("SELECT result FROM summary_processes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(result.contains("Manual notes"));
    }
}
