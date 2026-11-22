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
const STAGE_UNCERTAIN: &str =
    "Could not verify the staged cleanup. Review the location before removing files manually.";
// The desktop application is single-instance. Keep, retry and the first
// attempt serialize through this gate even after the invoking view is gone.
static CLEANUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    #[serde(skip)]
    stage_token: Option<String>,
    #[serde(skip)]
    staged: bool,
}

const ROW: &str = "SELECT id,meeting_id,title,kind,target,reason,stage_token,staged,
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

#[derive(Clone)]
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

    fn original(&self, pending: &PendingFileCleanup) -> Result<PathBuf, &'static str> {
        if pending.kind == "recording" {
            Ok(PathBuf::from(&pending.target))
        } else {
            self.target(&pending.kind, &pending.meeting_id, &pending.target)
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
        pending = tokio::task::spawn_blocking(move || {
            for entry in &mut pending {
                if let Ok(original) = locations.original(entry) {
                    let location = Stage::for_entry(&locations, entry)
                        .ok()
                        .filter(|stage| entry.staged || stage.holder_exists(false).unwrap_or(false))
                        .map_or(original, |stage| stage.payload);
                    entry.location = Some(location.to_string_lossy().into_owned());
                }
            }
            pending
        })
        .await
        .map_err(|_| "Could not read pending file cleanup locations.")?;
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
    let _cleanup = CLEANUP_LOCK.lock().await;
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
    let pool = pool.clone();
    let locations = locations.clone();
    // A cancelled IPC request cannot release the cleanup gate or interrupt
    // publication of the staging state while a filesystem operation runs.
    tokio::spawn(async move {
        let _work = work;
        let _cleanup = CLEANUP_LOCK.lock().await;
        attempt_locked(&pool, &locations, id, initial_recording_job).await
    })
    .await
    .map_err(|_| "Could not finish removing the file or folder.")?
}

async fn attempt_locked(
    pool: &SqlitePool,
    locations: &CleanupLocations,
    id: i64,
    initial_recording_job: Option<Arc<OwnedSemaphorePermit>>,
) -> Result<Option<String>, String> {
    let result = match stage_pending(pool, locations, id, initial_recording_job).await? {
        Ok(Some(stage)) => tokio::task::spawn_blocking(move || stage.remove())
            .await
            .map_err(|_| "Could not finish removing the file or folder.")?,
        Ok(None) => Ok(()),
        Err(reason) => Err(reason),
    };
    if let Err(reason) = result {
        sqlx::query("UPDATE pending_meeting_file_cleanup SET reason=? WHERE id=?")
            .bind(reason)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| "Could not update the file cleanup list.")?;
    } else {
        sqlx::query("DELETE FROM pending_meeting_file_cleanup WHERE id=? AND kept=0")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| "Could not update the file cleanup list.")?;
    }
    Ok(result.err().map(str::to_owned))
}

async fn stage_pending(
    pool: &SqlitePool,
    locations: &CleanupLocations,
    id: i64,
    initial_recording_job: Option<Arc<OwnedSemaphorePermit>>,
) -> Result<Result<Option<Stage>, &'static str>, String> {
    // Commit the unique destination before touching the filesystem. A restart
    // can find a successful rename even if the staging transaction rolled back.
    sqlx::query("UPDATE pending_meeting_file_cleanup SET stage_token=? WHERE id=? AND kept=0 AND stage_token IS NULL")
        .bind(uuid::Uuid::new_v4().to_string()).bind(id).execute(pool).await
        .map_err(|_| "Could not reserve a file cleanup location.")?;
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
        return Ok(Ok(None));
    };

    let stage = Stage::for_entry(locations, &pending);
    let (mut tx, stage) = match stage {
        Ok(stage) => {
            let already_staged = pending.staged;
            let (stage, status) = tokio::task::spawn_blocking(move || {
                let status = stage.inspect(already_staged);
                (stage, status)
            })
            .await
            .map_err(|_| "Could not inspect the file cleanup location.")?;
            match status {
                Ok(StageStatus::Ready) => (tx, Ok(Some(stage))),
                Ok(StageStatus::Gone) => (tx, Ok(None)),
                Ok(status @ (StageStatus::Fresh | StageStatus::EmptyReservation)) => {
                    match prepare(&mut tx, locations, &pending, initial_recording_job).await {
                        Ok(prepared) => tokio::task::spawn_blocking(move || {
                            let result = prepared
                                .stage(stage, matches!(status, StageStatus::EmptyReservation));
                            (tx, result)
                        })
                        .await
                        .map_err(|_| "Could not move files to the cleanup location.")?,
                        Err(reason) => (tx, Err(reason)),
                    }
                }
                Err(reason) => (tx, Err(reason)),
            }
        }
        Err(reason) => (tx, Err(reason)),
    };
    if matches!(&stage, Ok(Some(_))) {
        sqlx::query("UPDATE pending_meeting_file_cleanup SET staged=1 WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| "Could not save the staged cleanup.")?;
    }
    tx.commit()
        .await
        .map_err(|_| "Could not save the staged cleanup.")?;

    Ok(stage)
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
    fn stage(self, stage: Stage, reset_empty: bool) -> Result<Option<Stage>, &'static str> {
        // Keep the specialized recording/export guard through the rename.
        #[cfg(windows)]
        let _parents =
            match meeting_files::pin_directory_chain(stage.original.parent().ok_or(UNSAFE_PATH)?) {
                Ok(parents) => parents,
                Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
                Err(_) => return Err(UNSAFE_PATH),
            };
        let exists = match &self {
            Self::Recording {
                folder,
                meeting_id,
                roots,
                other_folders,
                exact_marker,
                _job,
            } => meeting_files::verify_recording_deletion(
                folder,
                meeting_id,
                roots,
                other_folders,
                *exact_marker || reset_empty,
            )?
            .is_some(),
            Self::AppData {
                path,
                directory,
                _export,
            } => checked_entry(path, *directory)?,
        };
        if !exists {
            return if reset_empty {
                Err(STAGE_UNCERTAIN)
            } else {
                Ok(None)
            };
        }
        if reset_empty {
            // Restart only after rechecking the target and reservation.
            // Non-empty holders stay available for manual review.
            if !stage.holder_exists(true)? {
                return Err(STAGE_UNCERTAIN);
            }
            fs::remove_dir(&stage.holder).map_err(|_| STAGE_UNCERTAIN)?;
        }
        stage.move_original()?;
        Ok(Some(stage))
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

fn checked_entry(path: &Path, directory: bool) -> Result<bool, &'static str> {
    let parent = path.parent().ok_or(UNSAFE_PATH)?;
    match meeting_files::directory_chain(parent) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(UNSAFE_PATH),
    }
    match fs::symlink_metadata(path) {
        Ok(meta)
            if !meeting_files::is_link(path, &meta)
                && if directory {
                    meta.is_dir()
                } else {
                    meta.is_file()
                } =>
        {
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        _ => Err(UNSAFE_PATH),
    }
}

struct Stage {
    original: PathBuf,
    holder: PathBuf,
    payload: PathBuf,
    directory: bool,
    recording_roots: Option<Vec<PathBuf>>,
}

enum StageStatus {
    Fresh,
    EmptyReservation,
    Ready,
    Gone,
}

impl Stage {
    fn for_entry(
        locations: &CleanupLocations,
        pending: &PendingFileCleanup,
    ) -> Result<Self, &'static str> {
        let original = locations.original(pending)?;
        let parent = original.parent().ok_or(UNSAFE_PATH)?;
        if meeting_files::is_cleanup_path(&original) {
            return Err(UNSAFE_PATH);
        }
        let token = pending
            .stage_token
            .as_deref()
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .ok_or(STAGE_UNCERTAIN)?;
        let holder = parent.join(format!("{}{token}", meeting_files::CLEANUP_PREFIX));
        let payload = holder.join("files");
        Ok(Self {
            original,
            holder,
            payload,
            directory: pending.kind != "reference",
            recording_roots: (pending.kind == "recording")
                .then(|| locations.recording_roots.clone()),
        })
    }

    fn holder_exists(&self, staged: bool) -> Result<bool, &'static str> {
        let parent = self.holder.parent().ok_or(UNSAFE_PATH)?;
        if let Some(roots) = &self.recording_roots {
            // The original may have been reused or replaced since the move.
            // Inspect only the reserved sibling, under a still-trusted root.
            if !crate::trusted_roots::text_allowed(&self.holder.to_string_lossy(), roots) {
                return Err(UNSAFE_PATH);
            }
        }
        match meeting_files::directory_chain(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::InvalidInput => return Err(UNSAFE_PATH),
            Err(error)
                if error.kind() == ErrorKind::NotFound
                    && !staged
                    && self.recording_roots.is_none() =>
            {
                // App-managed subdirectories may never have been created.
                // Verify the volume/share is reachable before treating them as absent.
                let volume = parent
                    .ancestors()
                    .filter(|part| part.has_root())
                    .last()
                    .ok_or(UNSAFE_PATH)?;
                meeting_files::directory_chain(volume).map_err(|_| UNAVAILABLE)?;
                return Ok(false);
            }
            Err(_) => return Err(UNAVAILABLE),
        }

        let canonical_parent = if let Some(roots) = &self.recording_roots {
            let canonical = parent.canonicalize().map_err(|_| UNAVAILABLE)?;
            if !roots
                .iter()
                .filter(|root| meeting_files::directory_chain(root).is_ok())
                .filter_map(|root| root.canonicalize().ok())
                .any(|root| canonical.starts_with(root))
            {
                return Err(UNSAFE_PATH);
            }
            Some(canonical)
        } else {
            None
        };
        match fs::symlink_metadata(&self.holder) {
            Ok(meta) if meta.is_dir() && !meeting_files::is_link(&self.holder, &meta) => {
                if let Some(parent) = canonical_parent {
                    let holder = self.holder.canonicalize().map_err(|_| UNAVAILABLE)?;
                    if holder.parent() != Some(parent.as_path()) {
                        return Err(UNSAFE_PATH);
                    }
                }
                Ok(true)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                // A drive can disconnect between checking its parent and the leaf.
                meeting_files::directory_chain(parent).map_err(|_| UNAVAILABLE)?;
                Ok(false)
            }
            Err(error) if error.kind() != ErrorKind::InvalidInput => Err(UNAVAILABLE),
            _ => Err(UNSAFE_PATH),
        }
    }

    fn payload_exists(&self) -> Result<bool, &'static str> {
        checked_entry(&self.payload, self.directory)
    }

    fn inspect(&self, staged: bool) -> Result<StageStatus, &'static str> {
        if !self.holder_exists(staged)? {
            return Ok(if staged {
                StageStatus::Gone
            } else {
                StageStatus::Fresh
            });
        }
        if self.payload_exists()? || staged {
            return Ok(StageStatus::Ready);
        }
        // A crash before publishing the rename can leave only the reservation.
        // If the original is absent, a non-recursive removal can clear an empty
        // holder. Otherwise recheck ownership before retrying the move.
        let parent = self.holder.parent().ok_or(UNSAFE_PATH)?;
        #[cfg(windows)]
        let _parents = meeting_files::pin_directory_chain(parent).map_err(|_| UNAVAILABLE)?;
        match fs::symlink_metadata(&self.original) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Ok(_) => return Ok(StageStatus::EmptyReservation),
            _ => return Err(STAGE_UNCERTAIN),
        }
        meeting_files::directory_chain(parent).map_err(|_| UNAVAILABLE)?;
        fs::remove_dir(&self.holder).map_err(|_| STAGE_UNCERTAIN)?;
        Ok(StageStatus::Gone)
    }

    fn move_original(&self) -> Result<(), &'static str> {
        // A newly made holder guarantees the rename cannot overwrite another
        // staged item. The sibling location also keeps the move on one volume.
        fs::create_dir(&self.holder).map_err(|_| DELETE_FAILED)?;
        if fs::rename(&self.original, &self.payload).is_err() {
            let _ = fs::remove_dir(&self.holder);
            return Err(DELETE_FAILED);
        }
        Ok(())
    }

    fn remove(self) -> Result<(), &'static str> {
        if !self.holder_exists(true)? {
            return Ok(());
        }
        #[cfg(windows)]
        let _parents = {
            let parent = self.holder.parent().ok_or(UNSAFE_PATH)?;
            let parents = meeting_files::pin_directory_chain(parent).map_err(|error| {
                if error.kind() == ErrorKind::InvalidInput {
                    UNSAFE_PATH
                } else {
                    UNAVAILABLE
                }
            })?;
            if !self.holder_exists(true)? {
                return Ok(());
            }
            parents
        };
        #[cfg(windows)]
        let parents = meeting_files::pin_directory_chain(&self.holder).map_err(|_| UNSAFE_PATH)?;
        if self.payload_exists()? {
            if self.directory {
                fs::remove_dir_all(&self.payload)
            } else {
                fs::remove_file(&self.payload)
            }
            .map_err(|_| DELETE_FAILED)?;
        }
        #[cfg(windows)]
        drop(parents);
        fs::remove_dir(&self.holder).map_err(|_| DELETE_FAILED)
    }
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
    async fn retry_keeps_a_staged_row_when_the_parent_disappears() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let original = locations.target("outputs", "review-test", "").unwrap();
        fs::create_dir_all(&original).unwrap();
        fs::write(original.join("old.txt"), b"Old output").unwrap();
        MeetingsRepository::delete_from_library(&pool, "review-test", false)
            .await
            .unwrap()
            .unwrap();
        let id = entry(&pool, "outputs").await.id;
        let stage = stage_pending(&pool, &locations, id, None)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let parent = stage.holder.parent().unwrap().to_path_buf();
        let offline = parent.with_file_name("offline-storage");
        fs::rename(&parent, &offline).unwrap();

        assert_eq!(
            attempt(&pool, &locations, id, None)
                .await
                .unwrap()
                .as_deref(),
            Some(UNAVAILABLE)
        );
        assert!(entry(&pool, "outputs").await.staged);
        assert_eq!(stage.remove(), Err(UNAVAILABLE));

        fs::rename(&offline, &parent).unwrap();
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
        assert!(sqlx::query_scalar::<_, bool>(
            "SELECT NOT EXISTS(SELECT 1 FROM pending_meeting_file_cleanup WHERE id=?)"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap());
    }

    #[test]
    fn empty_uncommitted_stage_can_only_finish_with_a_verified_parent() {
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let parent = &locations.recording_roots[0];
        let stage = Stage {
            original: parent.join("meeting"),
            holder: parent.join(".clawscribe-cleanup~empty"),
            payload: parent.join(".clawscribe-cleanup~empty/files"),
            directory: true,
            recording_roots: Some(locations.recording_roots.clone()),
        };
        assert!(matches!(stage.inspect(true), Err(UNAVAILABLE)));
        fs::create_dir_all(&stage.holder).unwrap();
        fs::create_dir(&stage.original).unwrap();
        assert!(matches!(
            stage.inspect(false),
            Ok(StageStatus::EmptyReservation)
        ));
        assert!(stage.holder.is_dir());
        fs::remove_dir(&stage.original).unwrap();
        assert!(matches!(stage.inspect(false), Ok(StageStatus::Gone)));
        assert!(!stage.holder.exists());
        assert!(matches!(stage.inspect(true), Ok(StageStatus::Gone)));
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
    async fn slow_removal_runs_after_commit_and_cannot_touch_a_replacement_owner() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let original = locations.target("outputs", "review-test", "").unwrap();
        fs::create_dir_all(&original).unwrap();
        fs::write(original.join("old.txt"), b"Old output").unwrap();
        MeetingsRepository::delete_from_library(&pool, "review-test", false)
            .await
            .unwrap()
            .unwrap();

        let id = entry(&pool, "outputs").await.id;
        let stage = stage_pending(&pool, &locations, id, None)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(entry(&pool, "outputs").await.staged);
        assert!(stage.payload.join("old.txt").is_file());
        assert!(!original.exists());

        // A new writer can claim and populate the original path before any
        // recursive deletion starts, including the same restored meeting ID.
        let mut writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('review-test','Restored',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")
            .execute(&mut *writer).await.unwrap();
        writer.commit().await.unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("new.txt"), b"Replacement output").unwrap();
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
        assert!(!stage.holder.exists());
        assert!(original.join("new.txt").is_file());
    }

    #[tokio::test]
    async fn retry_resumes_a_rename_that_happened_before_the_staging_commit() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        MeetingsRepository::delete_from_library(&pool, "review-test", false)
            .await
            .unwrap()
            .unwrap();
        let id = entry(&pool, "outputs").await.id;
        sqlx::query("UPDATE pending_meeting_file_cleanup SET stage_token=? WHERE id=?")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let stage = Stage::for_entry(&locations, &entry(&pool, "outputs").await).unwrap();
        fs::create_dir_all(&stage.original).unwrap();
        stage.move_original().unwrap();
        fs::create_dir_all(&stage.original).unwrap();
        fs::write(stage.original.join("new.txt"), b"Replacement output").unwrap();
        assert!(!entry(&pool, "outputs").await.staged);
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
        assert!(!stage.holder.exists());
        assert!(stage.original.join("new.txt").is_file());
    }

    #[tokio::test]
    async fn retry_recovers_an_empty_app_data_reservation_without_removing_unknown_files() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        MeetingsRepository::delete_from_library(&pool, "review-test", false)
            .await
            .unwrap()
            .unwrap();
        let id = entry(&pool, "outputs").await.id;
        sqlx::query("UPDATE pending_meeting_file_cleanup SET stage_token=? WHERE id=?")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let stage = Stage::for_entry(&locations, &entry(&pool, "outputs").await).unwrap();
        fs::create_dir_all(&stage.holder).unwrap();
        fs::create_dir(&stage.original).unwrap();
        let unexpected = stage.holder.join("unexpected.txt");
        fs::write(&unexpected, b"Preserve this file").unwrap();
        assert_eq!(
            attempt(&pool, &locations, id, None)
                .await
                .unwrap()
                .as_deref(),
            Some(STAGE_UNCERTAIN)
        );
        assert!(stage.original.is_dir() && unexpected.is_file());
        fs::remove_file(unexpected).unwrap();
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
        assert!(!stage.original.exists() && !stage.holder.exists());
    }

    #[tokio::test]
    async fn retry_keeps_a_legacy_folder_without_an_identity_marker() {
        let _test = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let root = tempfile::tempdir().unwrap();
        let locations = locations(root.path());
        let folder = locations.recording_roots[0].join("legacy");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("metadata.json"),
            r#"{"version":"1.0","transcript_file":"transcripts.json"}"#,
        )
        .unwrap();
        sqlx::query("UPDATE meetings SET folder_path=? WHERE id='review-test'")
            .bind(folder.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        MeetingsRepository::delete_from_library(&pool, "review-test", true)
            .await
            .unwrap()
            .unwrap();
        let id = entry(&pool, "recording").await.id;
        let reason = attempt(&pool, &locations, id, None).await.unwrap().unwrap();
        assert!(reason.contains("marker"));
        assert!(folder.join("metadata.json").is_file());
        assert!(!entry(&pool, "recording").await.staged);
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
        sqlx::query("UPDATE pending_meeting_file_cleanup SET stage_token=? WHERE id=?")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let stage = Stage::for_entry(&locations, &entry(&pool, "recording").await).unwrap();
        fs::create_dir(&stage.holder).unwrap();

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
        let unexpected = stage.holder.join("unexpected.txt");
        fs::write(&unexpected, b"Do not remove").unwrap();
        assert_eq!(
            attempt(&pool, &locations, id, None)
                .await
                .unwrap()
                .as_deref(),
            Some(STAGE_UNCERTAIN)
        );
        assert!(folder.join("audio.wav").is_file() && unexpected.is_file());
        fs::remove_file(unexpected).unwrap();
        assert_eq!(attempt(&pool, &locations, id, None).await.unwrap(), None);
        assert!(!folder.exists());
        assert!(!stage.holder.exists());
    }
}
