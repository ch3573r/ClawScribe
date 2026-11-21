//! App-local daily backups. Only successfully published files recorded by this
//! profile are eligible for retention.
use super::{
    archive::storage::{backup_identity, pin_backup_folder, remove_owned_backup},
    backup,
};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

const STORE: &str = "backup-schedule.json";
const CHANGED: &str = "backup-schedule-changed";
const DAILY: i64 = 24 * 60 * 60;
const RETRY: i64 = 15 * 60;
const KEEP: usize = 3;
const MAX_TRACKED: usize = 64;
const MAX_SETTINGS: u64 = 64 * 1024;
const SETTINGS_ERROR: &str = "Could not read or save scheduled backup settings.";
static JOB: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize, Deserialize)]
struct Schedule {
    parent: PathBuf,
    namespace: Uuid,
    enabled: bool,
    last_success: Option<i64>,
    last_failure: Option<i64>,
    last_error: Option<String>,
    incomplete_meetings: usize,
    #[serde(default)]
    skipped_template_items: usize,
    files: Vec<OwnedArchive>,
}

#[derive(Clone, Serialize, Deserialize)]
struct OwnedArchive {
    id: Uuid,
    identity: serde_json::Value,
}

#[derive(Serialize)]
pub(crate) struct ScheduleStatus {
    enabled: bool,
    running: bool,
    destination: Option<PathBuf>,
    last_success: Option<i64>,
    last_error: Option<String>,
    incomplete_meetings: usize,
    skipped_template_items: usize,
}

impl Schedule {
    fn new(parent: PathBuf) -> Self {
        Self {
            parent,
            namespace: Uuid::new_v4(),
            enabled: true,
            last_success: None,
            last_failure: None,
            last_error: None,
            incomplete_meetings: 0,
            skipped_template_items: 0,
            files: Vec::new(),
        }
    }

    fn folder(&self) -> PathBuf {
        self.parent
            .join(format!("ClawScribe backups-{}", self.namespace))
    }

    fn due(&self, now: i64) -> bool {
        self.enabled
            && self
                .last_success
                .is_none_or(|then| elapsed(now, then, DAILY))
            && self
                .last_failure
                .is_none_or(|then| elapsed(now, then, RETRY))
    }

    fn prune(&mut self) -> bool {
        let old = self.files.len().saturating_sub(KEEP);
        let folder = self.folder();
        let mut success = true;
        let mut index = 0;
        self.files.retain(|entry| {
            let expired = index < old;
            index += 1;
            if expired
                && remove_owned_backup(&folder.join(filename(entry.id)), &entry.identity).is_err()
            {
                success = false;
                return true;
            }
            !expired
        });
        success
    }
}

fn elapsed(now: i64, then: i64, interval: i64) -> bool {
    now < then || now.saturating_sub(then) >= interval
}

fn filename(id: Uuid) -> String {
    format!("ClawScribe-backup-{id}.zip")
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|data| data.join(STORE))
        .map_err(|_| SETTINGS_ERROR.into())
}

fn read(path: &Path) -> Result<Option<Schedule>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !crate::local_path::is_link(&metadata) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        _ => return Err(SETTINGS_ERROR.into()),
    }
    let file = File::open(path).map_err(|_| SETTINGS_ERROR)?;
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SETTINGS_ERROR)?;
    if bytes.len() as u64 > MAX_SETTINGS {
        return Err(SETTINGS_ERROR.into());
    }
    let schedule: Schedule = serde_json::from_slice(&bytes).map_err(|_| SETTINGS_ERROR)?;
    let ids: std::collections::HashSet<_> = schedule.files.iter().map(|entry| entry.id).collect();
    if schedule.files.len() > MAX_TRACKED || ids.len() != schedule.files.len() {
        return Err(SETTINGS_ERROR.into());
    }
    validate_path(&schedule.parent)?;
    Ok(Some(schedule))
}

fn save(path: &Path, schedule: &Schedule) -> Result<(), String> {
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        let parent = path.parent().ok_or(SETTINGS_ERROR)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut file, schedule)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    };
    write().map_err(|_| SETTINGS_ERROR.into())
}

fn validate_path(path: &Path) -> Result<(), String> {
    if !crate::local_path::is_local_path_text(&path.to_string_lossy())
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("Choose a local folder for scheduled backups.".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn scheduled_backup_status(app: AppHandle) -> Result<ScheduleStatus, String> {
    let path = settings_path(&app)?;
    tokio::task::spawn_blocking(move || {
        let schedule = read(&path)?;
        Ok(ScheduleStatus {
            enabled: schedule.as_ref().is_some_and(|schedule| schedule.enabled),
            running: RUNNING.load(Ordering::Acquire),
            destination: schedule.as_ref().map(Schedule::folder),
            last_success: schedule.as_ref().and_then(|schedule| schedule.last_success),
            last_error: schedule
                .as_ref()
                .and_then(|schedule| schedule.last_error.clone()),
            incomplete_meetings: schedule
                .as_ref()
                .map_or(0, |schedule| schedule.incomplete_meetings),
            skipped_template_items: schedule
                .as_ref()
                .map_or(0, |schedule| schedule.skipped_template_items),
        })
    })
    .await
    .map_err(|_| SETTINGS_ERROR)?
}

#[tauri::command]
pub(crate) async fn choose_scheduled_backup_folder(app: AppHandle) -> Result<(), String> {
    if app.try_state::<AppState>().is_none() {
        return Err("Open the meeting library first.".into());
    }
    let picker = app.clone();
    let parent = tokio::task::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("Choose a folder for daily backups")
            .blocking_pick_folder()
            .and_then(|path| path.into_path().ok())
    })
    .await
    .map_err(|_| "The folder picker could not open.")?;
    let Some(parent) = parent else { return Ok(()) };
    let job = JOB
        .try_lock()
        .map_err(|_| "Wait for the scheduled backup to finish.")?;
    let settings = settings_path(&app)?;
    tokio::task::spawn_blocking(move || {
        let _job = job;
        validate_path(&parent)?;
        let _pins = pin_backup_folder(&parent)
            .map_err(|_| "The backup folder is unavailable or linked.")?;
        let mut schedule = Schedule::new(parent);
        if let Ok(Some(previous)) = read(&settings) {
            schedule.enabled = previous.enabled;
        }
        let folder = schedule.folder();
        fs::create_dir(&folder).map_err(|_| "Could not create the scheduled backups folder.")?;
        if let Err(error) = save(&settings, &schedule) {
            let _ = fs::remove_dir(folder);
            return Err(error);
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| SETTINGS_ERROR)??;
    let _ = app.emit(CHANGED, ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn set_scheduled_backups(app: AppHandle, enabled: bool) -> Result<(), String> {
    if app.try_state::<AppState>().is_none() {
        return Err("Open the meeting library first.".into());
    }
    let job = JOB
        .try_lock()
        .map_err(|_| "Wait for the scheduled backup to finish.")?;
    let settings = settings_path(&app)?;
    tokio::task::spawn_blocking(move || {
        let _job = job;
        let mut schedule =
            read(&settings)?.ok_or("Choose a folder for scheduled backups first.")?;
        schedule.enabled = enabled;
        schedule.last_failure = None;
        schedule.last_error = None;
        save(&settings, &schedule)
    })
    .await
    .map_err(|_| SETTINGS_ERROR)??;
    let _ = app.emit(CHANGED, ());
    Ok(())
}

pub(crate) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if app.try_state::<AppState>().is_some() && run_if_due(&app).await.is_err() {
                log::warn!("Could not update scheduled backup status");
            }
        }
    });
}

async fn run_if_due(app: &AppHandle) -> Result<(), String> {
    let Ok(job) = JOB.try_lock() else {
        return Ok(());
    };
    let settings = settings_path(app)?;
    let source = settings.clone();
    let Some(mut schedule) = tokio::task::spawn_blocking(move || read(&source))
        .await
        .map_err(|_| SETTINGS_ERROR)??
    else {
        return Ok(());
    };
    let now = chrono::Utc::now().timestamp();
    if !schedule.due(now) {
        return Ok(());
    }
    let Ok(permits) = backup::claim_archive() else {
        return Ok(());
    };
    // Reserve the attempt before writing an archive. If the destination is
    // writable but the local index cannot be saved, avoid producing a new
    // untracked archive every minute.
    schedule.last_failure = Some(now);
    let reservation = schedule.clone();
    let index_path = settings.clone();
    tokio::task::spawn_blocking(move || save(&index_path, &reservation))
        .await
        .map_err(|_| SETTINGS_ERROR)??;
    RUNNING.store(true, Ordering::Release);
    let _ = app.emit(CHANGED, ());
    let result = run_backup(app, &settings, &mut schedule, permits).await;
    if let Err(error) = result {
        schedule.last_failure = Some(chrono::Utc::now().timestamp());
        schedule.last_error = Some(error);
    }
    let saved = tokio::task::spawn_blocking(move || save(&settings, &schedule))
        .await
        .map_err(|_| SETTINGS_ERROR);
    RUNNING.store(false, Ordering::Release);
    drop(job);
    let _ = app.emit(CHANGED, ());
    saved?
}

async fn run_backup(
    app: &AppHandle,
    settings: &Path,
    schedule: &mut Schedule,
    permits: backup::ArchivePermits,
) -> Result<(), String> {
    if schedule.files.len() >= MAX_TRACKED {
        return Err(
            "Retention could not remove older backups. Check the folder or choose a new one."
                .into(),
        );
    }
    let folder = schedule.folder();
    let id = Uuid::new_v4();
    let destination = folder.join(filename(id));
    let pins = tokio::task::spawn_blocking(move || pin_backup_folder(&folder))
        .await
        .map_err(|_| "Could not check the backup folder.")?
        .map_err(|_| "The backup folder is unavailable or linked.")?;
    let report =
        backup::backup_with_permits(app.clone(), destination.clone(), true, permits).await?;
    let identity = tokio::task::spawn_blocking(move || {
        let _pins = pins;
        backup_identity(&destination)
    })
    .await
    .map_err(|_| "Could not verify the saved backup.")?
    .map_err(|_| "Could not verify the saved backup.")?;

    schedule.files.push(OwnedArchive { id, identity });
    schedule.last_success = Some(chrono::Utc::now().timestamp());
    schedule.last_failure = None;
    schedule.last_error = None;
    schedule.incomplete_meetings = report.incomplete_meetings.len();
    schedule.skipped_template_items = report.skipped_template_items;
    // Make the new archive durable in the local index before touching old files.
    let settings = settings.to_owned();
    let mut index = schedule.clone();
    let (saved, complete) = tokio::task::spawn_blocking(move || {
        save(&settings, &index)?;
        let complete = index.prune();
        Ok::<_, String>((index.files, complete))
    })
    .await
    .map_err(|_| SETTINGS_ERROR)??;
    schedule.files = saved;
    if !complete {
        return Err("Backup saved. Some older backups could not be verified or removed.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_only_removes_identified_old_files() {
        let temp = tempfile::tempdir().unwrap();
        let mut schedule = Schedule::new(temp.path().to_owned());
        fs::create_dir(schedule.folder()).unwrap();
        for _ in 0..5 {
            let id = Uuid::new_v4();
            let path = schedule.folder().join(filename(id));
            fs::write(&path, "archive data").unwrap();
            let identity = backup_identity(&path).unwrap();
            schedule.files.push(OwnedArchive { id, identity });
        }
        let oldest = schedule.folder().join(filename(schedule.files[0].id));
        let replaced = schedule.folder().join(filename(schedule.files[1].id));
        fs::remove_file(&replaced).unwrap();
        fs::write(&replaced, "replacement data").unwrap();
        let unrelated = schedule.folder().join(filename(Uuid::new_v4()));
        fs::write(&unrelated, "manual backup").unwrap();

        assert!(!schedule.prune());
        assert!(!oldest.exists());
        assert_eq!(fs::read_to_string(replaced).unwrap(), "replacement data");
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "manual backup");
        assert_eq!(schedule.files.len(), 4);
    }

    #[test]
    fn schedule_waits_a_day_after_success_and_backs_off_after_failure() {
        let mut schedule = Schedule::new(PathBuf::new());
        assert!(schedule.due(100));
        schedule.last_failure = Some(100);
        assert!(!schedule.due(999));
        assert!(schedule.due(1000));
        schedule.last_success = Some(1000);
        assert!(!schedule.due(87399));
        assert!(schedule.due(87400));
        schedule.enabled = false;
        assert!(!schedule.due(87400));
    }
}
