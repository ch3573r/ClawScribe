//! Durable cleanup of files owned by meetings deleted from the library.
use crate::{database::meeting_files, state::AppState};
use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::OwnedSemaphorePermit;

const STILL_OWNED: &str = "A saved meeting still uses this file or folder.";
const UNAVAILABLE: &str = "The file location is unavailable.";
const UNSAFE_PATH: &str = "The file or folder could not be verified for removal.";
const DELETE_FAILED: &str =
    "Could not remove the file or folder. Close any app using it and try again.";
const BUSY: &str = "Wait for recording or transcription to finish, then try again.";

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct PendingFileCleanup {
    id: i64,
    #[serde(skip)]
    meeting_id: String,
    title: String,
    kind: String,
    location: Option<String>,
    #[serde(skip)]
    target: String,
    reason: Option<String>,
}

const ROW: &str = "SELECT id,meeting_id,title,kind,target,reason,
                   CASE WHEN kind='recording' THEN target END AS location
                   FROM pending_meeting_file_cleanup";

pub(crate) async fn enqueue(
    tx: &mut SqliteConnection,
    meeting_id: &str,
    title: &str,
    folder: Option<&str>,
    originals: &[String],
) -> Result<Vec<i64>, sqlx::Error> {
    let mut ids = Vec::new();
    let targets = folder
        .map(|folder| ("recording", folder))
        .into_iter()
        .chain(originals.iter().map(|name| ("reference", name.as_str())))
        .chain([("outputs", ""), ("codex", ""), ("exports", "")]);
    for (kind, target) in targets {
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO pending_meeting_file_cleanup(meeting_id,title,kind,target)
             VALUES (?,?,?,?) RETURNING id",
        )
        .bind(meeting_id)
        .bind(title)
        .bind(kind)
        .bind(target)
        .fetch_one(&mut *tx)
        .await?;
        ids.push(id);
    }
    Ok(ids)
}

pub(crate) struct CleanupLocations {
    app_data: Option<PathBuf>,
    codex_runs: PathBuf,
    recording_roots: Vec<PathBuf>,
}

impl CleanupLocations {
    pub(crate) async fn for_app<R: Runtime>(app: &AppHandle<R>) -> Self {
        Self {
            app_data: app.path().app_data_dir().ok(),
            codex_runs: crate::summary::codex_provider::default_codex_runs_root(),
            recording_roots: crate::audio::recording_preferences::recording_roots(app).await,
        }
    }

    fn target(
        &self,
        kind: &str,
        meeting_id: &str,
        original: &str,
    ) -> Result<PathBuf, &'static str> {
        let app_data = || self.app_data.as_deref().ok_or(UNAVAILABLE);
        match kind {
            "reference" => crate::knowledge::documents::original_path(
                &app_data()?.join("reference-documents"),
                original,
            )
            .map_err(|_| UNSAFE_PATH),
            "outputs" => Ok(crate::summary::codex_provider::meeting_outputs_dir(
                app_data()?,
                meeting_id,
            )),
            "codex" => Ok(crate::summary::codex_provider::meeting_run_dir(
                &self.codex_runs,
                meeting_id,
            )),
            "exports" => Ok(crate::exports::ledger::meeting_dir(app_data()?, meeting_id)),
            _ => Err(UNSAFE_PATH),
        }
    }
}

#[tauri::command]
pub(crate) async fn list_pending_meeting_file_cleanup<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<Vec<PendingFileCleanup>, String> {
    let mut pending: Vec<PendingFileCleanup> =
        sqlx::query_as(&format!("{ROW} WHERE kept=0 ORDER BY id DESC LIMIT 51"))
            .fetch_all(state.db_manager.pool())
            .await
            .map_err(|_| "Could not read pending file cleanup.")?;
    if !pending.is_empty() {
        let locations = CleanupLocations::for_app(&app).await;
        for entry in &mut pending {
            if entry.kind != "recording" {
                entry.location = locations
                    .target(&entry.kind, &entry.meeting_id, &entry.target)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned());
            }
        }
    }
    Ok(pending)
}

#[tauri::command]
pub(crate) async fn retry_meeting_file_cleanup<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    id: i64,
) -> Result<Option<String>, String> {
    let locations = CleanupLocations::for_app(&app).await;
    attempt(state.db_manager.pool(), &locations, id, None).await
}

#[tauri::command]
pub(crate) async fn keep_pending_meeting_files(
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let _work = crate::updates::admit_work()?;
    sqlx::query("UPDATE pending_meeting_file_cleanup SET kept=1 WHERE id=?")
        .bind(id)
        .execute(state.db_manager.pool())
        .await
        .map_err(|_| "Could not update the file cleanup list.".to_string())?;
    Ok(())
}

pub(crate) async fn attempt(
    pool: &SqlitePool,
    locations: &CleanupLocations,
    id: i64,
    initial_recording_job: Option<Arc<OwnedSemaphorePermit>>,
) -> Result<Option<String>, String> {
    let work = crate::updates::admit_work()?;
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "Could not lock the library. Try again.")?;
    let pending: Option<PendingFileCleanup> =
        sqlx::query_as(&format!("{ROW} WHERE id=? AND kept=0"))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| "Could not read this cleanup entry.")?;
    let Some(pending) = pending else {
        return Ok(None);
    };

    let prepared = prepare(&mut tx, locations, &pending, initial_recording_job).await;
    let (mut tx, result) = match prepared {
        Ok(prepared) => tokio::task::spawn_blocking(move || {
            // A cancelled webview request must not release file ownership or the
            // recording/export guard while its filesystem call is still running.
            let _work = work;
            (tx, prepared.remove())
        })
        .await
        .map_err(|_| "Could not finish removing the file or folder.")?,
        Err(reason) => (tx, Err(reason)),
    };
    if let Err(reason) = result {
        sqlx::query("UPDATE pending_meeting_file_cleanup SET reason=? WHERE id=?")
            .bind(reason)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Could not update the file cleanup list.")?;
    } else {
        sqlx::query("DELETE FROM pending_meeting_file_cleanup WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Could not update the file cleanup list.")?;
    }
    tx.commit()
        .await
        .map_err(|_| "Could not update the file cleanup list.")?;
    Ok(result.err().map(str::to_owned))
}

const OWNERSHIP_CHECK: &str = "Could not check whether the file or folder is still in use.";

enum CleanupTarget {
    Recording {
        folder: PathBuf,
        meeting_id: String,
        roots: Vec<PathBuf>,
        other_folders: Vec<PathBuf>,
        exact_marker: bool,
        _job: Arc<OwnedSemaphorePermit>,
    },
    AppData {
        path: PathBuf,
        directory: bool,
        _export: Option<tokio::sync::MutexGuard<'static, ()>>,
    },
}

impl CleanupTarget {
    fn remove(self) -> Result<(), &'static str> {
        match self {
            Self::Recording {
                folder,
                meeting_id,
                roots,
                other_folders,
                exact_marker,
                _job,
            } => meeting_files::remove_library_recording_folder(
                &folder,
                &meeting_id,
                &roots,
                &other_folders,
                exact_marker,
            ),
            Self::AppData {
                path,
                directory,
                _export,
            } => remove_app_file(&path, directory),
        }
    }
}

async fn prepare(
    tx: &mut SqliteConnection,
    locations: &CleanupLocations,
    pending: &PendingFileCleanup,
    initial_recording_job: Option<Arc<OwnedSemaphorePermit>>,
) -> Result<CleanupTarget, &'static str> {
    let kind = pending.kind.as_str();
    let meeting_id = &pending.meeting_id;
    let folder = &pending.target;
    let reclaimed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id=?)")
        .bind(meeting_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| OWNERSHIP_CHECK)?;
    if reclaimed {
        return Err(STILL_OWNED);
    }
    if kind == "recording" {
        let exact_marker = initial_recording_job.is_none();
        let job = match initial_recording_job {
            Some(job) => job,
            None => Arc::new(crate::audio::inference::claim_job().map_err(|_| BUSY)?),
        };
        let other_folders = sqlx::query_scalar::<_, String>(
            "SELECT folder_path FROM meetings WHERE folder_path IS NOT NULL",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| OWNERSHIP_CHECK)?;
        return Ok(CleanupTarget::Recording {
            folder: PathBuf::from(folder),
            meeting_id: meeting_id.clone(),
            roots: locations.recording_roots.clone(),
            other_folders: other_folders.into_iter().map(PathBuf::from).collect(),
            exact_marker,
            _job: job,
        });
    }

    let export = if kind == "exports" {
        Some(
            crate::exports::commands::EXPORT_LOCK
                .try_lock()
                .map_err(|_| "Wait for the active export to finish, then try again.")?,
        )
    } else {
        None
    };
    let path = locations.target(kind, meeting_id, folder)?;
    if kind == "reference" {
        let attached: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM knowledge_documents WHERE storage_name=?)",
        )
        .bind(folder)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| OWNERSHIP_CHECK)?;
        if attached {
            return Err(STILL_OWNED);
        }
    } else {
        if matches!(kind, "outputs" | "codex")
            && crate::summary::service::SummaryService::is_running_for(meeting_id)
        {
            return Err("Wait for summary generation to finish, then try again.");
        }
        let key = directory_key(&path);
        let ids = sqlx::query_scalar::<_, String>("SELECT id FROM meetings")
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| OWNERSHIP_CHECK)?;
        if ids.iter().any(|id| {
            locations
                .target(kind, id, "")
                .is_ok_and(|other| directory_key(&other) == key)
        }) {
            return Err(STILL_OWNED);
        }
    }
    Ok(CleanupTarget::AppData {
        path,
        directory: kind != "reference",
        _export: export,
    })
}

pub(crate) fn directory_key(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if cfg!(windows) {
        name.trim_end_matches(['.', ' ']).to_ascii_lowercase()
    } else {
        name.into_owned()
    }
}

fn remove_app_file(path: &Path, directory: bool) -> Result<(), &'static str> {
    // App-owned roots are derived in the core. Refuse replacement links or
    // junctions anywhere on the path before touching either kind of target.
    let parents = path
        .ancestors()
        .skip(1)
        .filter(|path| path.parent().is_some())
        .collect::<Vec<_>>();
    for parent in parents.into_iter().rev() {
        match fs::symlink_metadata(parent) {
            Ok(meta) if meta.is_dir() && !meeting_files::is_link(&meta) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            _ => return Err(UNSAFE_PATH),
        }
    }
    match fs::symlink_metadata(path) {
        Ok(meta)
            if !meeting_files::is_link(&meta)
                && if directory {
                    meta.is_dir()
                } else {
                    meta.is_file()
                } => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        _ => return Err(UNSAFE_PATH),
    }
    let removed = if directory {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    removed.map_err(|_| DELETE_FAILED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::meeting::MeetingsRepository;

    fn locations(root: &Path) -> CleanupLocations {
        let root = root.canonicalize().unwrap();
        let root = meeting_files::plain_path(&root);
        CleanupLocations {
            app_data: Some(root.clone()),
            codex_runs: root.join("runs"),
            recording_roots: vec![root.join("recordings")],
        }
    }

    async fn entry(pool: &SqlitePool, kind: &str) -> PendingFileCleanup {
        sqlx::query_as(&format!("{ROW} WHERE kind=?"))
            .bind(kind)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn cleanup_and_library_deletion_commit_together_and_keep_choice_omits_recording() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("CREATE TRIGGER fixture_fail_cleanup BEFORE INSERT ON pending_meeting_file_cleanup BEGIN SELECT RAISE(ABORT,'synthetic failure'); END")
            .execute(&pool).await.unwrap();
        assert!(
            MeetingsRepository::delete_from_library(&pool, "review-test", true)
                .await
                .is_err()
        );
        assert!(MeetingsRepository::get_meeting(&pool, "review-test")
            .await
            .unwrap()
            .is_some());
        sqlx::query("DROP TRIGGER fixture_fail_cleanup")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE meetings SET folder_path='synthetic recording' WHERE id='review-test'")
            .execute(&pool)
            .await
            .unwrap();
        let report = MeetingsRepository::delete_from_library(&pool, "review-test", false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.folder.as_deref(), Some("synthetic recording"));
        let kinds: Vec<String> =
            sqlx::query_scalar("SELECT kind FROM pending_meeting_file_cleanup ORDER BY kind")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(kinds, ["codex", "exports", "outputs"]);
        assert_eq!(report.cleanup_ids.len(), 3);
        assert!(
            MeetingsRepository::delete_from_library(&pool, "review-test", true)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn deleting_a_meeting_without_a_folder_resolves_its_browser_recovery_id() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        MeetingsRepository::delete_from_library(&pool, "review-test", false)
            .await
            .unwrap()
            .unwrap();
        let deleted: String =
            sqlx::query_scalar("SELECT meeting_id FROM recording_library_deletions")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(deleted, "review-test");
    }

    #[tokio::test]
    async fn app_directory_retry_remembers_failure_and_protects_a_restored_owner() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let outputs = locations.target("outputs", "review-test", "").unwrap();
        fs::create_dir_all(outputs.parent().unwrap()).unwrap();
        fs::write(&outputs, b"Synthetic unexpected file").unwrap();

        MeetingsRepository::delete_from_library(&pool, "review-test", false)
            .await
            .unwrap()
            .unwrap();
        let id = entry(&pool, "outputs").await.id;
        assert_eq!(
            attempt(&pool, &locations, id, None)
                .await
                .unwrap()
                .as_deref(),
            Some(UNSAFE_PATH)
        );
        assert_eq!(
            entry(&pool, "outputs").await.reason.as_deref(),
            Some(UNSAFE_PATH)
        );
        assert!(outputs.is_file());

        fs::remove_file(&outputs).unwrap();
        fs::create_dir(&outputs).unwrap();
        fs::write(outputs.join("meeting-notes.md"), b"Synthetic summary").unwrap();
        let restore = "INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('review-test','Restored meeting',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)";
        sqlx::query(restore).execute(&pool).await.unwrap();
        assert_eq!(
            attempt(&pool, &locations, id, None)
                .await
                .unwrap()
                .as_deref(),
            Some(STILL_OWNED)
        );
        assert!(outputs.join("meeting-notes.md").is_file());

        sqlx::query("DELETE FROM meetings WHERE id='review-test'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
        assert!(!outputs.exists());
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn recording_retry_checks_active_jobs_shared_folders_and_the_marker_again() {
        let _test = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let folder = locations.recording_roots[0].join("meeting");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("audio.wav"), b"Synthetic audio").unwrap();
        let marker = |id: &str| {
            fs::write(
                folder.join("metadata.json"),
                serde_json::json!({
                    "version": "1.0", "meeting_id": id, "transcript_file": "transcripts.json",
                })
                .to_string(),
            )
            .unwrap()
        };
        let folder_text = folder.to_str().unwrap();
        sqlx::query("UPDATE meetings SET folder_path=? WHERE id='review-test'")
            .bind(folder_text)
            .execute(&pool)
            .await
            .unwrap();
        MeetingsRepository::delete_from_library(&pool, "review-test", true)
            .await
            .unwrap()
            .unwrap();
        let id = entry(&pool, "recording").await.id;
        assert_eq!(
            entry(&pool, "recording").await.location.as_deref(),
            Some(folder_text)
        );

        marker("other-meeting");
        assert!(attempt(&pool, &locations, id, None)
            .await
            .unwrap()
            .unwrap()
            .contains("marker"));
        marker("review-test");
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at,folder_path) VALUES ('sharing','Shared',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,?)")
            .bind(folder_text).execute(&pool).await.unwrap();
        assert!(attempt(&pool, &locations, id, None)
            .await
            .unwrap()
            .unwrap()
            .contains("another meeting"));
        sqlx::query("DELETE FROM meetings WHERE id='sharing'")
            .execute(&pool)
            .await
            .unwrap();
        let busy = crate::audio::inference::claim_job().unwrap();
        assert_eq!(
            attempt(&pool, &locations, id, None)
                .await
                .unwrap()
                .as_deref(),
            Some(BUSY)
        );
        assert!(folder.join("audio.wav").is_file());
        drop(busy);
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
        assert!(!folder.exists());
    }
}
