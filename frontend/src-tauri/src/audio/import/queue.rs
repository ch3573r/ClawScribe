//! Serial import jobs outlive the dialog. File grants remain in memory.
use super::*;
use crate::audio::{inference, recording_commands};
use crate::file_grants::{self, Purpose, Taken};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const MAX_ITEMS: usize = 32;
const CHANGED: &str = "import-queue-changed";
const RECOVERED_QUEUE: &str =
    "The unreadable import queue was preserved. You can choose files again.";
static QUEUE: tokio::sync::Mutex<Option<Queue>> = tokio::sync::Mutex::const_new(None);
static PROGRESS: std::sync::Mutex<Option<ImportProgress>> = std::sync::Mutex::new(None);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Queued,
    Running,
    Cancelling,
    NeedsSource,
    Cancelled,
    Failed,
    Saved,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QueueOptions {
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    meeting_at: Option<chrono::DateTime<chrono::Utc>>,
    use_file_date: bool,
    summarize: bool,
    summary_destination_fingerprint: Option<String>,
    summary_use_automatic_preference: bool,
    summary_language: Option<String>,
    cloud_destination: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Item {
    id: String,
    filename: String,
    title: String,
    status: Status,
    message: Option<String>,
    result: Option<ImportResult>,
    elapsed_seconds: Option<f64>,
    #[serde(default)]
    options: QueueOptions,
}

#[derive(Serialize)]
pub struct QueueItem {
    id: String,
    filename: String,
    title: String,
    status: Status,
    message: Option<String>,
    result: Option<ImportResult>,
    elapsed_seconds: Option<f64>,
    provider: Option<String>,
    requires_file: bool,
    progress: Option<ImportProgress>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFile {
    source_path: String,
    title: String,
}

#[derive(Clone)]
struct Queue {
    file: PathBuf,
    items: Vec<Item>,
    sources: HashMap<String, Taken>,
    worker_running: bool,
    recovery_notice: Option<&'static str>,
}

impl Queue {
    fn save(&self) -> Result<(), String> {
        let content = serde_json::to_vec(&self.items).map_err(|_| "Could not encode imports")?;
        crate::library::atomic_write(&self.file, &content)
            .map_err(|_| "Could not save the import queue. Check available disk space.".into())
    }

    fn next(&self) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.status == Status::Queued)
    }

    async fn hidden_items(&self, pool: &sqlx::SqlitePool) -> Result<HashSet<String>, String> {
        if self.items.is_empty() {
            return Ok(HashSet::new());
        }
        let ids: Vec<_> = self.items.iter().map(|item| item.id.as_str()).collect();
        sqlx::query_scalar(
            "SELECT id FROM meetings WHERE deleted_at IS NOT NULL
             AND id IN (SELECT value FROM json_each(?))",
        )
        .bind(sqlx::types::Json(ids))
        .fetch_all(pool)
        .await
        .map(|ids| ids.into_iter().collect())
        .map_err(|_| "Could not review imports in Trash".into())
    }

    fn snapshots(&self, hidden: &HashSet<String>) -> Vec<QueueItem> {
        let waits_for_recording = recording_has_priority();
        let active_progress = PROGRESS.lock().unwrap_or_else(|error| error.into_inner());
        self.items
            .iter()
            .filter(|item| !hidden.contains(&item.id))
            .map(|item| QueueItem {
                id: item.id.clone(),
                filename: item.filename.clone(),
                title: item.title.clone(),
                status: item.status,
                message: if item.status == Status::Queued && waits_for_recording {
                    Some("Waiting for recording to finish.".into())
                } else {
                    item.message.clone()
                },
                result: item.result.clone(),
                elapsed_seconds: item.elapsed_seconds,
                provider: item.options.provider.clone(),
                requires_file: !self.sources.contains_key(&item.id),
                progress: matches!(item.status, Status::Running | Status::Cancelling)
                    .then(|| active_progress.clone())
                    .flatten(),
            })
            .collect()
    }

    fn clear_finished(&mut self, hidden: &HashSet<String>) {
        self.items.retain(|item| {
            hidden.contains(&item.id)
                || !matches!(
                    item.status,
                    Status::Saved | Status::Cancelled | Status::Failed
                )
        });
        self.sources
            .retain(|id, _| self.items.iter().any(|item| item.id == *id));
    }
}

async fn hidden_items<R: Runtime>(
    app: &AppHandle<R>,
    queue: &Queue,
) -> Result<HashSet<String>, String> {
    if queue.items.is_empty() {
        return Ok(HashSet::new());
    }
    let state = app
        .try_state::<AppState>()
        .ok_or("Meeting storage is unavailable")?;
    queue.hidden_items(state.db_manager.pool()).await
}

#[derive(Serialize)]
pub struct QueueSnapshot {
    items: Vec<QueueItem>,
    recovery_notice: Option<&'static str>,
}

pub(super) fn set_progress(progress: ImportProgress) {
    *PROGRESS.lock().unwrap_or_else(|error| error.into_inner()) = Some(progress);
}

fn clear_progress() {
    *PROGRESS.lock().unwrap_or_else(|error| error.into_inner()) = None;
}

async fn load<R: Runtime>(app: &AppHandle<R>, slot: &mut Option<Queue>) -> Result<(), String> {
    if slot.is_some() {
        return Ok(());
    }
    let _work = crate::updates::admit_work()?;
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|_| "Could not find import settings")?;
    std::fs::create_dir_all(&dir).map_err(|_| "Could not open import settings")?;
    let file = dir.join("import-queue.json");
    let mut queue = read_queue(file)?;
    if !queue.items.is_empty() {
        let state = app
            .try_state::<AppState>()
            .ok_or("Meeting storage is unavailable")?;
        recover_queue(state.db_manager.pool(), &mut queue).await?;
    }
    *slot = Some(queue);
    Ok(())
}

fn read_queue(file: PathBuf) -> Result<Queue, String> {
    const UNAVAILABLE: &str = "The import queue could not be opened.";
    let content = match std::fs::symlink_metadata(&file) {
        Ok(meta) if meta.is_file() && !crate::local_path::is_link(&meta) => {
            Some(std::fs::read(&file).map_err(|_| UNAVAILABLE)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        _ => return Err(UNAVAILABLE.into()),
    };
    let (items, recovery_notice) = match content.as_deref().map(serde_json::from_slice).transpose()
    {
        Ok(items) => (items.unwrap_or_default(), None),
        Err(_) => {
            let backup = file.with_file_name(format!(
                "import-queue.unreadable-{}.json",
                uuid::Uuid::new_v4()
            ));
            std::fs::rename(&file, backup)
                .map_err(|_| "The unreadable import queue could not be preserved. Check access to the app settings folder.")?;
            (Vec::new(), Some(RECOVERED_QUEUE))
        }
    };
    Ok(Queue {
        file,
        items,
        recovery_notice,
        sources: HashMap::new(),
        worker_running: false,
    })
}

async fn recover_queue(pool: &sqlx::SqlitePool, queue: &mut Queue) -> Result<(), String> {
    let mut changed = false;
    let mut recovered = Vec::with_capacity(queue.items.len());
    for mut item in queue.items.drain(..) {
        if meeting_deleted(pool, &item.id).await? {
            changed = true;
            continue;
        }
        if interrupted(item.status) {
            let saved = find_saved(pool, &item).await?;
            recover_item(&mut item, saved);
            changed = true;
        }
        recovered.push(item);
    }
    queue.items = recovered;
    if changed {
        queue.save()?;
    }
    Ok(())
}

async fn meeting_deleted(pool: &sqlx::SqlitePool, id: &str) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM recording_library_deletions WHERE meeting_id = ?1)
        AND NOT EXISTS(SELECT 1 FROM meetings WHERE id = ?1)",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|_| "Could not review deleted imports".into())
}

pub(crate) async fn forget_deleted_meeting<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let mut slot = QUEUE.lock().await;
    if load(app, &mut slot).await.is_err() {
        warn!("Could not review saved import history after meeting deletion");
        return;
    }
    let queue = slot.as_mut().unwrap();
    let before = queue.items.len();
    queue.items.retain(|item| {
        item.id != id || matches!(item.status, Status::Running | Status::Cancelling)
    });
    if queue.items.len() != before {
        queue.sources.remove(id);
        if queue.save().is_err() {
            warn!("Could not remove deleted meeting from saved import history");
        }
        notify(app);
    }
}

fn interrupted(status: Status) -> bool {
    matches!(
        status,
        Status::Queued | Status::Running | Status::Cancelling
    )
}

fn recover_item(item: &mut Item, saved: Option<ImportResult>) {
    (item.status, item.message) = match (&saved, item.status) {
        (Some(_), _) => (Status::Saved, None),
        (None, Status::Cancelling) => (Status::Cancelled, Some("Import cancelled.".into())),
        (None, _) => (
            Status::NeedsSource,
            Some("Choose the original file to resume after restart.".into()),
        ),
    };
    item.result = saved;
}

async fn find_saved(pool: &sqlx::SqlitePool, item: &Item) -> Result<Option<ImportResult>, String> {
    let row: Option<(String, Option<f64>, i64, bool, bool)> = sqlx::query_as(
        "SELECT m.title, m.recording_duration_s,
                (SELECT COUNT(*) FROM transcripts WHERE meeting_id = m.id),
                EXISTS(SELECT 1 FROM transcript_file_sync WHERE meeting_id = m.id),
                EXISTS(SELECT 1 FROM recording_outcomes WHERE meeting_id = m.id AND transcription_incomplete = 1)
         FROM meetings m WHERE m.id = ? AND m.recording_source = 'import'",
    )
    .bind(&item.id)
    .fetch_optional(pool)
    .await
    .map_err(|_| "Could not review saved imports")?;
    Ok(row.map(
        |(title, duration, segments, files_pending, transcription_incomplete)| ImportResult {
            meeting_id: item.id.clone(),
            title,
            duration_seconds: duration.unwrap_or_default(),
            segments_count: segments as usize,
            files_pending,
            transcription_provider: None,
            transcription_incomplete,
        },
    ))
}

pub(crate) fn notify<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit(CHANGED, ());
}

fn kick<R: Runtime>(
    app: &AppHandle<R>,
    queue: &mut Queue,
    work: tokio::sync::RwLockReadGuard<'static, ()>,
) {
    if !queue.worker_running && queue.next().is_some() {
        queue.worker_running = true;
        let app = app.clone();
        tauri::async_runtime::spawn(async move { worker(app, work).await });
    }
    notify(app);
}

fn recording_has_priority() -> bool {
    recording_commands::capture_active()
        || recording_commands::is_starting()
        || crate::start_request::is_pending()
        || crate::exit_guard::exit_in_progress()
}

async fn worker<R: Runtime>(app: AppHandle<R>, _work: tokio::sync::RwLockReadGuard<'static, ()>) {
    // Keep restart admission through the final queue save, including gaps in
    // which the native slot is free for recording or another foreground job.
    loop {
        let selection = {
            let mut slot = QUEUE.lock().await;
            let queue = slot.as_mut().expect("worker needs a loaded queue");
            let Some(index) = queue.next() else {
                queue.worker_running = false;
                return;
            };
            try_start(queue, index)
        };
        let Some((item, source, guard, job, phase)) = selection else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        };
        notify(&app);
        let started = Instant::now();
        let outcome = match prepare_options(&app, &item, &source.path).await {
            Ok(options) => {
                start_import(
                    app.clone(),
                    source.path.to_string_lossy().into_owned(),
                    item.title.clone(),
                    item.options.language,
                    item.options.model,
                    item.options.provider,
                    options,
                    guard,
                    job,
                    phase,
                )
                .await
            }
            Err(error) => {
                drop(phase);
                drop(job);
                drop(guard);
                Err(anyhow!(error))
            }
        };
        let mut slot = QUEUE.lock().await;
        let queue = slot.as_mut().expect("worker owns the active item");
        let index = queue
            .items
            .iter()
            .position(|entry| entry.id == item.id)
            .expect("running items cannot be removed");
        let entry = &mut queue.items[index];
        entry.elapsed_seconds = Some(started.elapsed().as_secs_f64());
        match outcome {
            Ok(result) => {
                entry.status = Status::Saved;
                entry.result = Some(result);
                queue.sources.remove(&item.id);
                let _ = app.emit("library-changed", ());
            }
            Err(error) if IMPORT_CANCELLED.load(Ordering::Acquire) => {
                entry.status = Status::Cancelled;
                entry.message = Some(if entry.message.is_some() {
                    "Import cancelled.".into()
                } else {
                    "Import stopped so recording could start. Retry when ready.".into()
                });
                drop(error);
            }
            Err(error) => {
                entry.status = Status::Failed;
                entry.message = Some(error.to_string());
            }
        }
        // Permanent deletion can commit after the import releases its permit
        // and before the worker publishes the outcome.
        if let Some(state) = app.try_state::<AppState>() {
            if meeting_deleted(state.db_manager.pool(), &item.id).await == Ok(true) {
                queue.items.remove(index);
                queue.sources.remove(&item.id);
            }
        }
        clear_progress();
        if queue.save().is_err() {
            warn!("Could not persist an import queue outcome");
        }
        notify(&app);
        if queue.next().is_none() {
            queue.worker_running = false;
            return;
        }
        drop(slot);
        // Let a waiting recording acquire the exclusive slot between files.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn try_start(
    queue: &mut Queue,
    index: usize,
) -> Option<(Item, Taken, ImportGuard, inference::BatchPermit, PhaseGuard)> {
    if recording_has_priority() {
        return None;
    }
    let guard = ImportGuard::acquire().ok()?;
    IMPORT_CANCELLED.store(false, Ordering::Release);
    let phase = PhaseGuard::open();
    let job = inference::claim_batch_job_without_preemption(
        inference::BatchJob::Import,
        Arc::new(cancel_import_if_possible),
    )
    .ok()?;
    if recording_has_priority() {
        drop(phase);
        return None;
    }
    let item = &mut queue.items[index];
    let Some(source) = queue.sources.get(&item.id).cloned() else {
        recover_item(item, None);
        let _ = queue.save();
        drop(phase);
        return None;
    };
    item.status = Status::Running;
    item.message = None;
    let item = item.clone();
    if queue.save().is_err() {
        queue.items[index].status = Status::Failed;
        queue.items[index].message =
            Some("Could not save the import queue. Free disk space and retry.".into());
        drop(phase);
        return None;
    }
    clear_progress();
    Some((item, source, guard, job, phase))
}

async fn prepare_options<R: Runtime>(
    app: &AppHandle<R>,
    item: &Item,
    source: &Path,
) -> Result<ImportOptions, String> {
    let settings = &item.options;
    let meeting_at = if settings.use_file_date {
        Some(
            tokio::fs::metadata(source)
                .await
                .and_then(|meta| meta.modified())
                .map(chrono::DateTime::<chrono::Utc>::from)
                .map_err(|_| "Could not read the file's modified time")?,
        )
    } else {
        settings.meeting_at
    };
    let summary_request = if settings.summarize {
        let state = app
            .try_state::<AppState>()
            .ok_or("Meeting storage is unavailable")?;
        Some(
            crate::summary::automatic::prepare_import(
                app,
                state.db_manager.pool(),
                settings.summary_destination_fingerprint.as_deref(),
                settings.summary_use_automatic_preference,
            )
            .await,
        )
    } else {
        None
    };
    Ok(ImportOptions {
        meeting_id: item.id.clone(),
        date: ImportDate {
            meeting_at,
            from_file: settings.use_file_date,
            ..ImportDate::default()
        },
        summary_language: settings.summary_language.clone(),
        summary_request,
        cloud_destination: settings.cloud_destination.clone(),
    })
}

#[tauri::command]
pub async fn import_queue_snapshot<R: Runtime>(app: AppHandle<R>) -> Result<QueueSnapshot, String> {
    let mut slot = QUEUE.lock().await;
    load(&app, &mut slot).await?;
    let queue = slot.as_ref().unwrap();
    let mut items = queue.snapshots(&hidden_items(&app, queue).await?);
    let recovery_notice = queue.recovery_notice;
    drop(slot);
    if let Some(state) = app.try_state::<AppState>() {
        let pool = state.db_manager.pool();
        for item in &mut items {
            if let Some(result) = item.result.as_mut().filter(|result| result.files_pending) {
                result.files_pending = !matches!(
                    crate::database::transcript_edits::import_file_stage(pool, &item.id).await,
                    Ok(None)
                );
            }
        }
    }
    Ok(QueueSnapshot {
        items,
        recovery_notice,
    })
}

#[tauri::command]
pub async fn import_cloud_destination<R: Runtime>(
    app: AppHandle<R>,
    provider: String,
) -> Result<super::super::transcription::cloud::ImportCloudDestination, String> {
    if !recording_commands::cloud_transcription_enabled() {
        return Err("Enable cloud transcription in Settings first".into());
    }
    let state = app
        .try_state::<AppState>()
        .ok_or("Meeting storage is unavailable")?;
    super::super::transcription::cloud::import_destination(state.db_manager.pool(), &provider).await
}

#[tauri::command]
pub async fn enqueue_imports<R: Runtime>(
    app: AppHandle<R>,
    files: Vec<ImportFile>,
    mut options: QueueOptions,
) -> Result<(), String> {
    let work = crate::updates::admit_work()?;
    if files.is_empty() || files.len() > MAX_ITEMS {
        return Err("Choose between 1 and 32 files".into());
    }
    options.summary_language = options
        .summary_language
        .as_deref()
        .map(crate::summary::metadata::normalise_supported_summary_language)
        .transpose()
        .map_err(|_| "Unsupported summary language")?;
    if let Some(provider) = options
        .provider
        .as_deref()
        .filter(|provider| super::super::transcription::cloud::is_cloud_provider(Some(provider)))
    {
        if !recording_commands::cloud_transcription_enabled() {
            return Err("Enable cloud transcription in Settings first".into());
        }
        let state = app
            .try_state::<AppState>()
            .ok_or("Meeting storage is unavailable")?;
        let destination = super::super::transcription::cloud::import_destination(
            state.db_manager.pool(),
            provider,
        )
        .await?;
        if options.cloud_destination.as_deref() != Some(destination.fingerprint.as_str()) {
            return Err("Review and approve the current cloud upload destination".into());
        }
    }
    let mut slot = QUEUE.lock().await;
    load(&app, &mut slot).await?;
    let queue = slot.as_mut().unwrap();
    if queue.items.len() + files.len() > MAX_ITEMS {
        return Err("The import queue holds 32 items, including imports in Trash. Clear finished items to free space.".into());
    }
    let mut seen = HashSet::new();
    let mut next = queue.clone();
    let mut added = Vec::new();
    let result = (|| {
        for file in files {
            if !seen.insert(crate::local_path::comparable_text(&file.source_path)) {
                return Err("Choose each source file once".into());
            }
            let taken = file_grants::take(Purpose::AudioImport, &file.source_path)?;
            added.push(taken.clone());
            let filename = source_filename(&taken)?;
            let id = new_meeting_id();
            let title = if file.title.trim().is_empty() {
                filename.clone()
            } else {
                file.title.trim().chars().take(150).collect()
            };
            next.items.push(Item {
                id: id.clone(),
                filename,
                title,
                options: options.clone(),
                status: Status::Queued,
                message: None,
                result: None,
                elapsed_seconds: None,
            });
            next.sources.insert(id, taken);
        }
        next.save()
    })();
    if result.is_err() {
        for taken in &added {
            file_grants::give_back(Purpose::AudioImport, taken);
        }
    }
    result?;
    *queue = next;
    kick(&app, queue, work);
    Ok(())
}

fn source_filename(source: &Taken) -> Result<String, String> {
    let supported = source
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            AUDIO_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        });
    source
        .path
        .file_name()
        .filter(|_| supported)
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "Choose a supported audio or video file".into())
}

#[tauri::command]
pub async fn cancel_import_item<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    let _work = crate::updates::admit_work()?;
    let mut slot = QUEUE.lock().await;
    load(&app, &mut slot).await?;
    let queue = slot.as_mut().unwrap();
    if hidden_items(&app, queue).await?.contains(&id) {
        return Err("Restore the meeting from Trash to manage this import.".into());
    }
    let mut next = queue.clone();
    let entry = next
        .items
        .iter_mut()
        .find(|item| item.id == id)
        .ok_or("Import item is unavailable")?;
    match entry.status {
        Status::Queued | Status::NeedsSource => entry.status = Status::Cancelled,
        Status::Running => {
            if !cancel_import_if_possible() {
                return Err("The meeting is being saved. Wait for the import to finish.".into());
            }
            entry.status = Status::Cancelling;
        }
        _ => return Ok(()),
    }
    entry.message = Some("Import cancelled.".into());
    let cancelling = entry.status == Status::Cancelling;
    let saved = next.save();
    // Keep an accepted running cancellation reflected even if storage failed.
    if cancelling || saved.is_ok() {
        *queue = next;
        notify(&app);
    }
    saved
}

#[tauri::command]
pub async fn retry_import_item<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    source_path: Option<String>,
    cloud_destination: Option<String>,
) -> Result<(), String> {
    let work = crate::updates::admit_work()?;
    let mut slot = QUEUE.lock().await;
    load(&app, &mut slot).await?;
    let queue = slot.as_mut().unwrap();
    if hidden_items(&app, queue).await?.contains(&id) {
        return Err("Restore the meeting from Trash to retry this import.".into());
    }
    let mut next = queue.clone();
    let item = next
        .items
        .iter_mut()
        .find(|item| item.id == id)
        .ok_or("Import item is unavailable")?;
    if !matches!(
        item.status,
        Status::Failed | Status::Cancelled | Status::NeedsSource
    ) {
        return Err("This import cannot be retried".into());
    }
    if let Some(provider) =
        item.options.provider.as_deref().filter(|provider| {
            super::super::transcription::cloud::is_cloud_provider(Some(provider))
        })
    {
        let reviewed = import_cloud_destination(app.clone(), provider.to_owned()).await?;
        if cloud_destination.as_deref() != Some(reviewed.fingerprint.as_str()) {
            return Err(
                "Review and approve the current cloud upload destination before retrying".into(),
            );
        }
        item.options.cloud_destination = cloud_destination;
    }
    let replacement = if let Some(path) = source_path {
        let taken = file_grants::take(Purpose::AudioImport, &path)?;
        if source_filename(&taken).as_deref() != Ok(item.filename.as_str()) {
            file_grants::give_back(Purpose::AudioImport, &taken);
            return Err("Choose the original file with the same name".into());
        }
        next.sources.insert(id.clone(), taken.clone());
        Some(taken)
    } else {
        if !next.sources.contains_key(&id) {
            return Err("Choose the original file before retrying".into());
        }
        None
    };
    item.status = Status::Queued;
    item.message = None;
    if let Err(error) = next.save() {
        if let Some(source) = &replacement {
            file_grants::give_back(Purpose::AudioImport, source);
        }
        return Err(error);
    }
    *queue = next;
    kick(&app, queue, work);
    Ok(())
}

#[tauri::command]
pub async fn clear_finished_imports<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let _work = crate::updates::admit_work()?;
    let mut slot = QUEUE.lock().await;
    load(&app, &mut slot).await?;
    let queue = slot.as_mut().unwrap();
    let mut next = queue.clone();
    next.clear_finished(&hidden_items(&app, queue).await?);
    next.save()?;
    *queue = next;
    notify(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(id: &str) -> Item {
        serde_json::from_value(serde_json::json!({
            "id": id, "filename": "synthetic.wav", "title": "Synthetic meeting", "status": "queued",
            "options": { "useFileDate": false, "summarize": false, "summaryUseAutomaticPreference": false }
        })).unwrap()
    }

    #[tokio::test]
    async fn restart_recovers_a_committed_import_and_requires_a_new_grant_for_the_rest() {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let mut created = queued("meeting-imported");
        let mut interrupted_item = queued("meeting-interrupted");
        created.status = Status::Running;
        interrupted_item.status = Status::Cancelling;
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, recording_source)
                     VALUES (?, 'Saved meeting', '2026-01-01', '2026-01-01', 'import')",
        )
        .bind(&created.id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(interrupted(created.status) && interrupted(interrupted_item.status));
        let committed = find_saved(&pool, &created).await.unwrap();
        let uncommitted = find_saved(&pool, &interrupted_item).await.unwrap();
        recover_item(&mut created, committed);
        recover_item(&mut interrupted_item, uncommitted);
        assert_eq!(created.status, Status::Saved);
        assert_eq!(created.result.unwrap().meeting_id, "meeting-imported");
        assert_eq!(interrupted_item.status, Status::Cancelled);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("queue.json");
        let source = dir.path().join("chosen.wav");
        file_grants::grant(Purpose::AudioImport, &source).unwrap();
        let grant = file_grants::take(Purpose::AudioImport, &source.to_string_lossy()).unwrap();
        let queue = Queue {
            file: file.clone(),
            items: vec![interrupted_item],
            sources: HashMap::from([("meeting-interrupted".into(), grant)]),
            worker_running: false,
            recovery_notice: None,
        };
        assert!(queue.next().is_none());
        queue.save().unwrap();
        assert!(!std::fs::read_to_string(file)
            .unwrap()
            .contains("chosen.wav"));
    }

    #[test]
    fn an_unreadable_queue_is_preserved_before_accepting_new_items() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("import-queue.json");
        let damaged = b"[{\"id\":\"meeting-imported\"";
        std::fs::write(&file, damaged).unwrap();
        let mut recovered = read_queue(file.clone()).unwrap();
        assert!(recovered.items.is_empty());
        assert!(recovered.recovery_notice.is_some());
        recovered.items.push(queued("meeting-new"));
        recovered.save().unwrap();
        let backup = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("import-queue.unreadable-")
            })
            .unwrap();
        assert_eq!(std::fs::read(backup.path()).unwrap().as_slice(), damaged);
        assert_eq!(read_queue(file).unwrap().items[0].id, "meeting-new");
    }

    #[test]
    fn older_queue_options_do_not_enable_uploads_or_automatic_notes() {
        for options in [None, Some(serde_json::json!({}))] {
            let mut item = serde_json::json!({
                "id": "meeting-old", "filename": "old.wav", "title": "Old meeting", "status": "queued"
            });
            if let Some(options) = options {
                item["options"] = options;
            }
            let item: Item = serde_json::from_value(item).unwrap();
            assert!(!item.options.use_file_date);
            assert!(!item.options.summarize);
            assert!(!item.options.summary_use_automatic_preference);
            assert!(item.options.cloud_destination.is_none());
        }
    }

    #[tokio::test]
    async fn trash_then_restore_keeps_a_failed_import_and_its_retry_source() {
        use crate::database::repositories::meeting::MeetingsRepository;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("synthetic.wav");
        file_grants::grant(Purpose::AudioImport, &source).unwrap();
        let grant = file_grants::take(Purpose::AudioImport, &source.to_string_lossy()).unwrap();
        let id = "review-test";
        let mut failed = queued(id);
        failed.status = Status::Failed;
        let mut queue = read_queue(dir.path().join("queue.json")).unwrap();
        queue.items.push(failed);
        queue.items.push(queued("unsaved-import"));
        queue.sources.insert(id.into(), grant);

        assert!(MeetingsRepository::move_to_trash(&pool, id, true)
            .await
            .unwrap());
        let hidden = queue.hidden_items(&pool).await.unwrap();
        queue.clear_finished(&hidden);
        let shown = queue.snapshots(&hidden);
        assert_eq!(
            shown
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["unsaved-import"]
        );

        assert!(MeetingsRepository::restore_from_trash(&pool, id)
            .await
            .unwrap());
        let restored = queue.snapshots(&queue.hidden_items(&pool).await.unwrap());
        assert_eq!(restored[0].id, id);
        assert_eq!(restored[0].status, Status::Failed);
        assert!(!restored[0].requires_file);
    }

    #[tokio::test]
    async fn a_deleted_meeting_does_not_reappear_in_queue_history_after_restart() {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO recording_library_deletions (meeting_id) VALUES ('meeting-deleted')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("import-queue.json");
        let mut queue = read_queue(file.clone()).unwrap();
        let mut saved = queued("meeting-deleted");
        saved.status = Status::Saved;
        queue.items.push(saved);
        queue.items.push(queued("meeting-interrupted"));
        queue.save().unwrap();
        recover_queue(&pool, &mut queue).await.unwrap();
        let items = read_queue(file).unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "meeting-interrupted");
        assert_eq!(items[0].status, Status::NeedsSource);
    }
}
