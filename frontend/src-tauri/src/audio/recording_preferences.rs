use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_store::StoreExt;

use anyhow::Result;
#[cfg(target_os = "macos")]
use log::error;

#[cfg(target_os = "macos")]
use crate::audio::capture::AudioCaptureBackend;

const DEFAULT_RECORDINGS_FOLDER_NAME: &str = "ClawScribe";
const LEGACY_RECORDINGS_FOLDER_NAME: &str = "meetily-recordings";
const DEVICE_PREFERENCES_CHANGED: &str = "recording-device-preferences-changed";
static PREFERENCE_WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn default_auto_open_floating_controls() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecordingPreferences {
    pub save_folder: PathBuf,
    pub auto_save: bool,
    #[serde(default = "default_auto_open_floating_controls")]
    pub auto_open_floating_controls: bool,
    #[serde(default = "default_auto_open_floating_controls")]
    pub remember_floating_position: bool,
    pub file_format: String,
    #[serde(default)]
    pub preferred_mic_device: Option<String>,
    #[serde(default)]
    pub preferred_system_device: Option<String>,
    #[cfg(target_os = "macos")]
    #[serde(default)]
    pub system_audio_backend: Option<String>,
    /// Set when the stored recordings folder is not a trusted root and the
    /// default folder is used instead. Never read from the webview, never stored.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub folder_notice: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct RecordingDevicePreferences {
    preferred_mic_device: Option<String>,
    preferred_system_device: Option<String>,
}

impl Default for RecordingPreferences {
    fn default() -> Self {
        Self {
            save_folder: get_default_recordings_folder(),
            auto_save: true,
            auto_open_floating_controls: default_auto_open_floating_controls(),
            remember_floating_position: true,
            file_format: "mp4".to_string(),
            preferred_mic_device: None,
            preferred_system_device: None,
            #[cfg(target_os = "macos")]
            system_audio_backend: Some("coreaudio".to_string()),
            folder_notice: None,
        }
    }
}

fn platform_recordings_folder(folder_name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // Windows: %USERPROFILE%\Music\<folder_name>
        if let Some(music_dir) = dirs::audio_dir() {
            music_dir.join(folder_name)
        } else {
            // Fallback to Documents if Music folder is not available
            dirs::document_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(folder_name)
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Movies/<folder_name>
        if let Some(movies_dir) = dirs::video_dir() {
            movies_dir.join(folder_name)
        } else {
            // Fallback to Documents if Movies folder is not available
            dirs::document_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(folder_name)
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux/Others: ~/Documents/<folder_name>
        dirs::document_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(folder_name)
    }
}

/// Get the default recordings folder based on platform.
pub fn get_default_recordings_folder() -> PathBuf {
    if let Some(root) = crate::database::recovery::data_dir() {
        return root.join("recordings");
    }
    platform_recordings_folder(DEFAULT_RECORDINGS_FOLDER_NAME)
}

pub(crate) fn get_legacy_default_recordings_folder() -> PathBuf {
    if let Some(root) = crate::database::recovery::data_dir() {
        return root.join("legacy-recordings");
    }
    platform_recordings_folder(LEGACY_RECORDINGS_FOLDER_NAME)
}

/// Key, in the same store file as the preferences, of the recordings folders
/// that were configured before. It is not part of `RecordingPreferences`, so a
/// settings screen that saves the preferences cannot erase it.
const PREVIOUS_SAVE_FOLDERS_KEY: &str = "previous_save_folders";
const MAX_PREVIOUS_SAVE_FOLDERS: usize = 10;

/// A path as text for comparison: on Windows both slash directions count as the
/// same and letter case is ignored; everywhere a trailing separator is ignored
/// (a root such as `/` or `C:\` keeps its own).
fn comparable(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(target_os = "windows")]
    let text = text.replace('\\', "/").to_ascii_lowercase();
    #[cfg(not(target_os = "windows"))]
    let text = text.into_owned();
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.ends_with(':') {
        text
    } else {
        trimmed.to_string()
    }
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    comparable(a) == comparable(b)
}

/// Serialises "read the remembered folders, add one, write them back" so two
/// saves at once cannot lose an entry.
static PREVIOUS_FOLDERS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The remembered folders after the save folder changed from `old` to `new`:
/// the old folder first, most recent first, without duplicates and without the
/// folder now in use, bounded to the last ten.
pub(crate) fn remember_previous_folder(
    remembered: &[PathBuf],
    old: &std::path::Path,
    new: &std::path::Path,
) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = Vec::new();
    let candidates = std::iter::once(old)
        .filter(|old| !old.as_os_str().is_empty())
        .chain(remembered.iter().map(PathBuf::as_path));
    for folder in candidates {
        if same_path(folder, new) || folders.iter().any(|kept| same_path(kept, folder)) {
            continue;
        }
        folders.push(folder.to_path_buf());
    }
    folders.truncate(MAX_PREVIOUS_SAVE_FOLDERS);
    folders
}

fn read_previous_save_folders<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    app.store("recording_preferences.json")
        .ok()
        .and_then(|store| store.get(PREVIOUS_SAVE_FOLDERS_KEY))
        .and_then(|value| serde_json::from_value::<Vec<PathBuf>>(value).ok())
        .unwrap_or_default()
}

/// The roots: the default folders, the restored backups and every folder the
/// user trusted (chosen in the core's folder picker, or the folder an earlier
/// version already used, taken over once). Network folders are roots like any
/// other. Judged without touching the disk.
pub(crate) fn roots_from(trusted: Vec<PathBuf>, app_data_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = vec![
        get_default_recordings_folder(),
        get_legacy_default_recordings_folder(),
    ];
    roots.extend(trusted);
    roots.extend(app_data_dir.map(|data| data.join("restored-recordings")));
    roots
}

fn trust_backing<R: Runtime>(
    app: &AppHandle<R>,
) -> Option<std::sync::Arc<tauri_plugin_store::Store<R>>> {
    app.store(crate::trusted_roots::STORE_FILE).ok()
}

/// The recordings folders of an earlier version, as stored: the configured one
/// and those configured before. Read raw, before any fallback.
fn stored_folders<R: Runtime>(app: &AppHandle<R>) -> (Option<PathBuf>, Vec<PathBuf>) {
    let Ok(store) = app.store("recording_preferences.json") else {
        return (None, Vec::new());
    };
    let configured = store
        .get("preferences")
        .and_then(|value| value.get("save_folder").cloned())
        .and_then(|value| serde_json::from_value::<PathBuf>(value).ok());
    (configured, read_previous_save_folders(app))
}

/// The first start of this version: the folders the user already had become
/// roots. Idempotent, and cheap once done.
pub(crate) fn ensure_takeover<R: Runtime>(app: &AppHandle<R>) {
    let Some(backing) = trust_backing(app) else {
        return;
    };
    if crate::trusted_roots::read(backing.as_ref()).takeover_done {
        return;
    }
    let (configured, previous) = stored_folders(app);
    let protected = protected_folders(tauri::Manager::path(app).app_data_dir().ok());
    match crate::trusted_roots::take_over_once(
        backing.as_ref(),
        configured.as_deref(),
        &previous,
        &protected,
    ) {
        Ok(outcome) => {
            if !outcome.rejected.is_empty() {
                warn!(
                    "{} earlier recordings folder(s) were not taken over; see Diagnostics",
                    outcome.rejected.len()
                );
            }
        }
        Err(_) => warn!("The earlier recordings folders could not be taken over"),
    }
}

/// Part of the one-time takeover that needs the database: the parent folder of
/// every folder saved for a meeting becomes a root, so meetings recorded in an
/// earlier folder keep their playback, open, retranscribe, speaker detection and
/// delete. Runs after the database is open. When it is not available nothing is
/// recorded as done and the next start tries again.
pub(crate) async fn take_over_registered_folders<R: Runtime>(app: &AppHandle<R>) {
    let Some(backing) = trust_backing(app) else {
        return;
    };
    if crate::trusted_roots::read(backing.as_ref()).registered_done {
        return;
    }
    let saved: Option<Vec<String>> = match tauri::Manager::try_state::<crate::state::AppState>(app) {
        Some(state) => sqlx::query_scalar(
            "SELECT DISTINCT folder_path FROM meetings WHERE folder_path IS NOT NULL AND TRIM(folder_path) <> ''",
        )
        .fetch_all(state.db_manager.pool())
        .await
        .ok(),
        None => None,
    };
    if saved.is_none() {
        warn!("The folders of saved meetings could not be taken over yet; trying again at the next start");
    }
    let protected = protected_folders(tauri::Manager::path(app).app_data_dir().ok());
    match crate::trusted_roots::take_over_registered_parents(
        backing.as_ref(),
        saved.as_deref(),
        &protected,
    ) {
        Ok((added, skipped)) if added + skipped > 0 => {
            info!("Took over {added} folder(s) of saved meetings; {skipped} skipped");
        }
        Ok(_) => {}
        Err(_) => warn!("The folders of saved meetings could not be taken over"),
    }
}

/// Every folder a meeting's recording files may live in. Opening, recovering
/// and deleting outside these is refused.
pub(crate) async fn recording_roots<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    ensure_takeover(app);
    let trusted = trust_backing(app)
        .map(|store| crate::trusted_roots::read(store.as_ref()).trusted)
        .unwrap_or_default();
    let roots = roots_from(trusted, tauri::Manager::path(app).app_data_dir().ok());
    crate::trusted_roots::set_snapshot(roots.clone());
    roots
}

const FOLDER_NOTICE: &str = "The recordings folder saved earlier is not trusted (it was not chosen with the folder picker of this version, or it is a drive, system or profile folder). Recordings use the default folder until you choose a folder again.";

/// What to tell the user about stored recordings folders that are not trusted.
pub(crate) async fn recording_folder_problems<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    ensure_takeover(app);
    let trust = trust_backing(app)
        .map(|store| crate::trusted_roots::read(store.as_ref()))
        .unwrap_or_default();
    let defaults = [
        get_default_recordings_folder(),
        get_legacy_default_recordings_folder(),
    ];
    let (configured, previous) = stored_folders(app);
    let mut problems = Vec::new();
    if let Some(folder) = &configured {
        if !crate::trusted_roots::is_trusted_or_default(&trust, folder, &defaults) {
            problems.push(format!(
                "{FOLDER_NOTICE} Its recordings are not opened, recovered or deleted from the app."
            ));
        }
    }
    let untrusted = previous
        .iter()
        .filter(|folder| !crate::trusted_roots::is_trusted_or_default(&trust, folder, &defaults))
        .count();
    if untrusted > 0 {
        problems.push(format!(
            "{untrusted} earlier recordings folder(s) are not trusted and are ignored."
        ));
    }
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !problems.is_empty() && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        warn!("A stored recordings folder is not trusted; see Diagnostics");
    }
    problems
}

fn paths_match(a: &PathBuf, b: &PathBuf) -> bool {
    same_path(a, b)
}
fn migrate_legacy_recordings_folder(preferences: &mut RecordingPreferences) -> bool {
    let legacy_default = get_legacy_default_recordings_folder();
    if paths_match(&preferences.save_folder, &legacy_default) {
        preferences.save_folder = get_default_recordings_folder();
        return true;
    }

    false
}

#[cfg(target_os = "macos")]
fn normalize_platform_preferences(mut preferences: RecordingPreferences) -> RecordingPreferences {
    let backend = crate::audio::capture::get_current_backend();
    preferences.system_audio_backend = Some(backend.to_string());
    preferences
}

#[cfg(not(target_os = "macos"))]
fn normalize_platform_preferences(preferences: RecordingPreferences) -> RecordingPreferences {
    preferences
}

/// Ensure the recordings directory exists
pub fn ensure_recordings_directory(path: &PathBuf) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
        info!("Created recordings directory: {:?}", path);
    }
    Ok(())
}

/// Generate a unique filename for a recording
pub fn generate_recording_filename(format: &str) -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    format!("recording_{}.{}", timestamp, format)
}

/// Load recording preferences from store
pub async fn load_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<RecordingPreferences> {
    let store = app.store("recording_preferences.json")?;
    let mut stored = store.get("preferences");
    if stored.is_none() {
        // Store initialization can silently ignore a corrupt or unreadable
        // file. Only a missing file/key may use first-run defaults.
        let fresh = store.clone();
        match tokio::task::spawn_blocking(move || fresh.reload()).await? {
            Ok(()) => stored = store.get("preferences"),
            Err(tauri_plugin_store::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut prefs = stored
        .map(serde_json::from_value::<RecordingPreferences>)
        .transpose()?
        .map(normalize_platform_preferences)
        .unwrap_or_default();

    if migrate_legacy_recordings_folder(&mut prefs) {
        info!("Migrated the legacy default recordings folder");

        match serde_json::to_value(&prefs) {
            Ok(prefs_value) => {
                store.set("preferences", prefs_value);
                if store.save().is_err() {
                    warn!("Could not persist migrated recording preferences");
                }
            }
            Err(_) => warn!("Could not serialize migrated recording preferences"),
        }
    }

    // The folder must be a root the user chose (or the earlier folder taken over).
    // Anything else, for example a value written outside the app, is not used.
    ensure_takeover(app);
    let trust = trust_backing(app)
        .map(|store| crate::trusted_roots::read(store.as_ref()))
        .unwrap_or_default();
    let defaults = [
        get_default_recordings_folder(),
        get_legacy_default_recordings_folder(),
    ];
    if !crate::trusted_roots::is_trusted_or_default(&trust, &prefs.save_folder, &defaults) {
        warn!("The stored recordings folder is not trusted; the default folder is used");
        prefs.save_folder = get_default_recordings_folder();
        prefs.folder_notice = Some(FOLDER_NOTICE.to_string());
    }

    Ok(prefs)
}

/// Save recording preferences to store
pub async fn save_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
    preferences: &RecordingPreferences,
) -> Result<()> {
    // Check the folder before changing either the in-memory or on-disk state.
    ensure_recordings_directory(&preferences.save_folder)?;

    // Get or create store
    let store = app
        .store("recording_preferences.json")
        .map_err(|e| anyhow::anyhow!("Failed to access store: {}", e))?;

    // Serialize preferences to JSON value
    let mut stored = preferences.clone();
    stored.folder_notice = None;
    let prefs_value = serde_json::to_value(&stored)
        .map_err(|e| anyhow::anyhow!("Failed to serialize preferences: {}", e))?;

    // A changed save folder must not strand recordings made in the old one:
    // remember it so recovery and cleanup still accept it.
    let _remembering = PREVIOUS_FOLDERS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let previous_preferences = store.get("preferences");
    let previous_folders = store.get(PREVIOUS_SAVE_FOLDERS_KEY);
    let old_folder = previous_preferences
        .as_ref()
        .and_then(|value| value.get("save_folder").cloned())
        .and_then(|value| serde_json::from_value::<PathBuf>(value).ok());
    if let Some(old_folder) = old_folder {
        let remembered = previous_folders
            .clone()
            .and_then(|value| serde_json::from_value::<Vec<PathBuf>>(value).ok())
            .unwrap_or_default();
        let updated = remember_previous_folder(&remembered, &old_folder, &preferences.save_folder);
        if updated != remembered {
            store.set(PREVIOUS_SAVE_FOLDERS_KEY, serde_json::json!(updated));
        }
    }

    // Save to store
    store.set("preferences", prefs_value);

    // Persist to disk
    if let Err(error) = store.save() {
        for (key, value) in [
            ("preferences", previous_preferences),
            (PREVIOUS_SAVE_FOLDERS_KEY, previous_folders),
        ] {
            match value {
                Some(previous) => store.set(key, previous),
                None => {
                    store.delete(key);
                }
            }
        }
        return Err(anyhow::anyhow!("Failed to save store to disk: {}", error));
    }

    // Save backend preference to global config
    #[cfg(target_os = "macos")]
    if let Some(backend_str) = &preferences.system_audio_backend {
        if let Some(backend) = AudioCaptureBackend::from_string(backend_str) {
            info!("Setting audio capture backend to: {:?}", backend);
            crate::audio::capture::set_current_backend(backend);
        }
    }

    Ok(())
}

/// Tauri commands for recording preferences
#[tauri::command]
pub async fn get_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RecordingPreferences, String> {
    load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))
}

const SAVE_FOLDER_REFUSED: &str =
    "Choose a normal folder for recordings, not a drive, system, profile or application-data folder.";

/// Folders that must never become the recordings folder: `exact` ones as
/// themselves (a drive root, the profile, the application-data roots) and
/// `trees` with everything below them (Windows, Program Files, system folders).
#[derive(Debug, Default)]
pub(crate) struct ProtectedFolders {
    pub exact: Vec<PathBuf>,
    pub trees: Vec<PathBuf>,
}

/// The protected folders of this machine, from the platform's known folders
/// and environment (never a hard-coded user name).
pub(crate) fn protected_folders(app_data_dir: Option<PathBuf>) -> ProtectedFolders {
    let mut protected = ProtectedFolders::default();
    protected.exact.extend(dirs::home_dir());
    protected.exact.extend(dirs::data_dir());
    protected.exact.extend(dirs::data_local_dir());
    protected.exact.extend(dirs::config_dir());
    protected.exact.extend(app_data_dir);
    // The folders people keep things in are not a recordings folder themselves
    // (a subfolder of them is fine), nor is the folder that holds every profile
    // or the profile's application-data folder.
    protected.exact.extend(dirs::document_dir());
    protected.exact.extend(dirs::desktop_dir());
    protected.exact.extend(dirs::download_dir());
    if let Some(home) = dirs::home_dir() {
        protected.exact.push(home.join("AppData"));
        protected
            .exact
            .extend(home.parent().map(std::path::Path::to_path_buf));
    }
    for variable in [
        "SystemRoot",
        "windir",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
    ] {
        protected
            .trees
            .extend(std::env::var_os(variable).map(PathBuf::from));
    }
    #[cfg(unix)]
    protected.trees.extend(
        [
            "/etc", "/usr", "/bin", "/sbin", "/lib", "/var", "/boot", "/dev", "/proc", "/sys",
            "/System", "/Library",
        ]
        .map(PathBuf::from),
    );
    protected
}

/// Top-level folders of any drive that belong to the system: the folder and
/// everything below it for most, and `Users` itself (a profile below it is fine).
/// `X:\Windows`, `X:\Program Files`, `X:\ProgramData`, `X:\Recovery` and so on,
/// whichever drive letter they are on.
pub(crate) fn is_system_top_level(path: &std::path::Path) -> bool {
    const TREES: &[&str] = &[
        "windows",
        "program files",
        "program files (x86)",
        "programdata",
        "recovery",
        "boot",
        "system volume information",
        "$recycle.bin",
        "windows.old",
        "perflogs",
    ];
    let mut parts = path.components().filter_map(|component| match component {
        std::path::Component::Normal(name) => name.to_str().map(str::to_ascii_lowercase),
        _ => None,
    });
    #[cfg(windows)]
    {
        let Some(top) = parts.next() else {
            return false;
        };
        if TREES.contains(&top.as_str()) {
            return true;
        }
        top == "users" && parts.next().is_none()
    }
    #[cfg(not(windows))]
    {
        let _ = (&mut parts, TREES);
        false
    }
}

/// Validate a folder obtained from the core picker, including a network share.
/// Persist its resolved path so recording and recovery use the same directory
/// without following the original alias again.
pub(crate) fn validate_save_folder(
    requested: &std::path::Path,
    protected: &ProtectedFolders,
) -> Result<PathBuf, String> {
    // The folder comes from the core's own folder picker, so a network location
    // is fine; what is refused is a drive or share root, a system, profile or
    // application-data folder.
    if requested.as_os_str().is_empty()
        || !requested.is_absolute() && !requested.to_string_lossy().starts_with("\\\\")
    {
        return Err(SAVE_FOLDER_REFUSED.into());
    }
    let canonical = requested
        .canonicalize()
        .map_err(|_| SAVE_FOLDER_REFUSED.to_string())?;
    let resolved = crate::database::meeting_files::plain_path(&canonical);
    if crate::database::meeting_files::directory_chain(&resolved).is_err()
        || crate::trusted_roots::root_problem(&resolved, protected, true).is_some()
    {
        return Err(SAVE_FOLDER_REFUSED.into());
    }
    Ok(resolved)
}

/// What a request for a recordings folder amounts to.
pub(crate) enum FolderChange {
    /// The folder is the one in use: nothing changes.
    Keep(PathBuf),
    /// The default folder, which is always allowed.
    Default,
    /// A different folder, accepted only because the core's own folder picker
    /// returned exactly this path in this session.
    Granted(crate::file_grants::Taken),
}

/// A folder in the text the webview sent is not trusted. It stays as it is, is
/// the default, or must equal a folder granted by the core's folder picker;
/// anything else is refused.
pub(crate) fn classify_folder_request(
    current: Option<&std::path::Path>,
    requested: &std::path::Path,
    default: &std::path::Path,
    take: impl FnOnce(&str) -> Result<crate::file_grants::Taken, String>,
) -> Result<FolderChange, String> {
    if let Some(current) = current.filter(|current| same_path(current, requested)) {
        return Ok(FolderChange::Keep(current.to_path_buf()));
    }
    if same_path(requested, default) {
        return Ok(FolderChange::Default);
    }
    take(&requested.to_string_lossy()).map(FolderChange::Granted)
}

#[tauri::command]
pub async fn set_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
    mut preferences: RecordingPreferences,
    preserve_device_preferences: Option<bool>,
) -> Result<(), String> {
    let _write = PREFERENCE_WRITE.lock().await;
    // The folder is not taken from the text the webview sends. Unchanged, it
    // stays; the default folder is always allowed; anything else must be a
    // folder the user chose in the core's folder picker (`select_recording_folder`)
    // in this session, and becomes a trusted root.
    let current = load_recording_preferences(&app).await.ok();
    let preserve_devices = preserve_device_preferences.unwrap_or(false);
    if preserve_devices {
        let current = current
            .as_ref()
            .ok_or("Could not load audio device preferences")?;
        preferences
            .preferred_mic_device
            .clone_from(&current.preferred_mic_device);
        preferences
            .preferred_system_device
            .clone_from(&current.preferred_system_device);
    }
    let change = classify_folder_request(
        current
            .as_ref()
            .map(|current| current.save_folder.as_path()),
        &preferences.save_folder,
        &get_default_recordings_folder(),
        |text| crate::file_grants::take(crate::file_grants::Purpose::RecordingsFolder, text),
    )?;
    if let FolderChange::Keep(folder) = &change {
        preferences.save_folder = folder.clone();
    } else if let FolderChange::Default = &change {
        preferences.save_folder = get_default_recordings_folder();
        ensure_recordings_directory(&preferences.save_folder)
            .map_err(|_| "The recordings folder could not be created".to_string())?;
    } else if let FolderChange::Granted(taken) = change {
        let granted = taken.path.clone();
        let protected = protected_folders(tauri::Manager::path(&app).app_data_dir().ok());
        let checked = granted.clone();
        let trusted_app = app.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
            let resolved = validate_save_folder(&checked, &protected)?;
            if !same_path(&checked, &resolved) {
                return Err(
                    "The selected folder changed. Choose it again in Settings → Recording.".into(),
                );
            }
            let backing = trust_backing(&trusted_app)
                .ok_or_else(|| "Could not save the folder".to_string())?;
            crate::trusted_roots::trust_folder(backing.as_ref(), &resolved, &protected)?;
            Ok(resolved)
        })
        .await
        .map_err(|_| "Could not check the recordings folder".to_string())
        .and_then(|result| result);
        preferences.save_folder = match result {
            Ok(resolved) => resolved,
            Err(message) => {
                crate::file_grants::give_back(
                    crate::file_grants::Purpose::RecordingsFolder,
                    &taken,
                );
                return Err(message);
            }
        };
    }
    preferences.folder_notice = None;
    save_recording_preferences(&app, &preferences)
        .await
        .map_err(|_| {
            warn!("Could not persist recording preferences");
            "Could not save recording preferences".to_string()
        })?;
    // The roots changed with it.
    let _ = recording_roots(&app).await;
    if !preserve_devices {
        let _ = app.emit(
            DEVICE_PREFERENCES_CHANGED,
            RecordingDevicePreferences {
                preferred_mic_device: preferences.preferred_mic_device,
                preferred_system_device: preferences.preferred_system_device,
            },
        );
    }
    Ok(())
}

/// Patch just the device fields so Home and Settings never write a stale
/// recordings folder or other unrelated preferences when choosing a device.
#[tauri::command]
pub async fn set_recording_device_preferences<R: Runtime>(
    app: AppHandle<R>,
    preferences: RecordingDevicePreferences,
) -> Result<RecordingDevicePreferences, String> {
    let _write = PREFERENCE_WRITE.lock().await;
    let current = load_recording_preferences(&app)
        .await
        .map_err(|_| "Could not load recording preferences")?;
    let store = app
        .store("recording_preferences.json")
        .map_err(|_| "Could not access recording preferences")?;
    let previous = store.get("preferences");
    let mut stored = match &previous {
        Some(value) => value.clone(),
        None => {
            serde_json::to_value(current).map_err(|_| "Could not prepare recording preferences")?
        }
    };
    stored["preferred_mic_device"] = serde_json::json!(preferences.preferred_mic_device);
    stored["preferred_system_device"] = serde_json::json!(preferences.preferred_system_device);
    store.set("preferences", stored);
    if store.save().is_err() {
        match previous {
            Some(value) => store.set("preferences", value),
            None => {
                store.delete("preferences");
            }
        }
        warn!("Could not persist audio device preferences");
        return Err("Could not save audio device preferences".into());
    }
    let _ = app.emit(DEVICE_PREFERENCES_CHANGED, &preferences);
    Ok(preferences)
}

#[tauri::command]
pub async fn get_default_recordings_folder_path() -> Result<String, String> {
    let path = get_default_recordings_folder();
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_recordings_folder<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let preferences = load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load preferences: {}", e))?;

    // Ensure directory exists before trying to open it
    ensure_recordings_directory(&preferences.save_folder)
        .map_err(|e| format!("Failed to create directory: {}", e))?;

    let folder_path = preferences.save_folder.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        crate::local_path::open_in_explorer(std::path::Path::new(&folder_path))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    info!("Opened recordings folder: {}", folder_path);
    Ok(())
}

#[tauri::command]
pub async fn select_recording_folder<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<String>, String> {
    let app_clone = app.clone();
    let folder_path =
        tokio::task::spawn_blocking(move || app_clone.dialog().file().blocking_pick_folder())
            .await
            .map_err(|e| format!("Folder dialog task failed: {}", e))?;

    if let Some(path) = folder_path {
        let folder = PathBuf::from(path.to_string());
        let protected = protected_folders(tauri::Manager::path(&app).app_data_dir().ok());
        let resolved =
            tokio::task::spawn_blocking(move || validate_save_folder(&folder, &protected))
                .await
                .map_err(|_| "Could not check the recordings folder".to_string())??;
        // The choice is remembered by the core; `set_recording_preferences`
        // accepts a changed folder only if it is this one.
        crate::file_grants::grant(crate::file_grants::Purpose::RecordingsFolder, &resolved)?;

        info!("User selected a recordings folder");
        Ok(Some(resolved.to_string_lossy().into_owned()))
    } else {
        info!("User cancelled recordings folder selection");
        Ok(None)
    }
}

// Backend selection commands

/// Get current audio capture backend
#[tauri::command]
pub async fn get_current_audio_backend() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let backend = crate::audio::capture::get_current_backend();
        Ok(backend.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok("screencapturekit".to_string())
    }
}

/// Set audio capture backend
#[tauri::command]
pub async fn set_audio_backend(backend: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;
        use crate::audio::permissions::{
            check_screen_recording_permission, request_screen_recording_permission,
        };

        let backend_enum = AudioCaptureBackend::from_string(&backend)
            .ok_or_else(|| format!("Invalid backend: {}", backend))?;

        // If switching to Core Audio, log information about Audio Capture permission
        if backend_enum == AudioCaptureBackend::CoreAudio {
            info!("🔐 Core Audio backend requires Audio Capture permission (macOS 14.4+)");
            info!("📍 Permission dialog will appear automatically when recording starts");

            // Check if permission is already granted (this is informational only)
            if !check_screen_recording_permission() {
                warn!("⚠️  Audio Capture permission may not be granted");

                // Attempt to open System Settings (opens System Settings)
                if let Err(e) = request_screen_recording_permission() {
                    error!("Failed to open System Settings: {}", e);
                }

                return Err(
                    "Core Audio requires Audio Capture permission. \
                    The permission dialog will appear when you start recording. \
                    If already denied, enable it in System Settings → Privacy & Security → Audio Capture, \
                    then restart the app.".to_string()
                );
            }

            info!(
                "✅ Core Audio backend selected - permission check will occur at recording start"
            );
        }

        info!("Setting audio backend to: {:?}", backend_enum);
        crate::audio::capture::set_current_backend(backend_enum);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        if backend != "screencapturekit" {
            return Err(format!(
                "Backend {} not available on this platform",
                backend
            ));
        }
        Ok(())
    }
}

/// Get backend information (name and description)
#[derive(Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[tauri::command]
pub async fn get_audio_backend_info() -> Result<Vec<BackendInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;

        let backends = vec![
            BackendInfo {
                id: AudioCaptureBackend::ScreenCaptureKit.to_string(),
                name: AudioCaptureBackend::ScreenCaptureKit.name().to_string(),
                description: AudioCaptureBackend::ScreenCaptureKit
                    .description()
                    .to_string(),
            },
            BackendInfo {
                id: AudioCaptureBackend::CoreAudio.to_string(),
                name: AudioCaptureBackend::CoreAudio.name().to_string(),
                description: AudioCaptureBackend::CoreAudio.description().to_string(),
            },
        ];
        Ok(backends)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(vec![BackendInfo {
            id: "screencapturekit".to_string(),
            name: "ScreenCaptureKit".to_string(),
            description: "Default system audio capture".to_string(),
        }])
    }
}

#[cfg(test)]
mod floating_controls_tests {
    use super::*;

    #[test]
    fn new_and_legacy_preferences_open_controls_and_an_opt_out_survives_reload() {
        assert!(RecordingPreferences::default().auto_open_floating_controls);
        let legacy =
            serde_json::json!({"save_folder":"Recordings","auto_save":true,"file_format":"mp4"});
        let mut prefs: RecordingPreferences = serde_json::from_value(legacy).unwrap();
        assert!(prefs.auto_open_floating_controls);
        prefs.auto_open_floating_controls = false;
        let stored = serde_json::to_value(prefs).unwrap();
        let reloaded: RecordingPreferences = serde_json::from_value(stored).unwrap();
        assert!(!reloaded.auto_open_floating_controls);
    }
}

#[cfg(test)]
mod previous_folder_tests {
    use super::*;
    use std::path::Path;

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn a_changed_save_folder_is_remembered_most_recent_first() {
        let first = remember_previous_folder(&[], Path::new("/a"), Path::new("/b"));
        assert_eq!(first, vec![p("/a")]);
        let second = remember_previous_folder(&first, Path::new("/b"), Path::new("/c"));
        assert_eq!(second, vec![p("/b"), p("/a")]);
    }

    #[test]
    fn an_unchanged_or_empty_folder_adds_nothing_and_the_current_one_is_dropped() {
        let remembered = vec![p("/a"), p("/b")];
        assert_eq!(
            remember_previous_folder(&remembered, Path::new("/c"), Path::new("/c")),
            remembered
        );
        assert_eq!(
            remember_previous_folder(&remembered, Path::new(""), Path::new("/c")),
            remembered
        );
        // Going back to an earlier folder removes it from the list.
        assert_eq!(
            remember_previous_folder(&remembered, Path::new("/c"), Path::new("/a")),
            vec![p("/c"), p("/b")]
        );
    }

    #[test]
    fn duplicates_collapse_and_the_list_is_bounded() {
        let mut remembered = Vec::new();
        for index in 0..15 {
            remembered = remember_previous_folder(
                &remembered,
                Path::new(&format!("/old{index}")),
                Path::new(&format!("/new{index}")),
            );
        }
        assert_eq!(remembered.len(), MAX_PREVIOUS_SAVE_FOLDERS);
        assert_eq!(remembered[0], p("/old14"));
        assert!(!remembered.contains(&p("/old0")), "the oldest are dropped");
        let again = remember_previous_folder(&remembered, Path::new("/old14"), Path::new("/x"));
        assert_eq!(
            again
                .iter()
                .filter(|folder| **folder == p("/old14"))
                .count(),
            1
        );
    }

    #[test]
    fn trailing_separators_do_not_make_a_folder_a_different_one() {
        let remembered = remember_previous_folder(
            &[p("/data/old")],
            Path::new("/data/old/"),
            Path::new("/data/new"),
        );
        assert_eq!(
            remembered,
            vec![p("/data/old")],
            "the same folder with a trailing slash is not added twice"
        );
        let again = remember_previous_folder(
            &remembered,
            Path::new("/data/older//"),
            Path::new("/data/old/"),
        );
        // The folder now in use leaves the list whatever its trailing slash.
        assert_eq!(again, vec![p("/data/older//")]);
        assert!(same_path(Path::new("/"), Path::new("/")));
        assert!(!same_path(Path::new("/"), Path::new("/data")));
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_slash_direction_and_case_do_not_matter() {
        assert!(same_path(
            Path::new(r"D:\Recordings\"),
            Path::new("d:/recordings")
        ));
        assert!(same_path(Path::new(r"C:\"), Path::new("c:/")));
        assert!(!same_path(Path::new(r"C:\"), Path::new(r"D:\")));
        let list =
            remember_previous_folder(&[p(r"D:\Old")], Path::new("d:/old/"), Path::new(r"E:\New"));
        assert_eq!(list.len(), 1);
    }
}

#[cfg(test)]
mod save_folder_tests {
    use super::*;

    fn protected(base: &std::path::Path) -> ProtectedFolders {
        ProtectedFolders {
            exact: vec![base.join("profile"), base.join("appdata")],
            trees: vec![base.join("system")],
        }
    }

    fn layout() -> (tempfile::TempDir, ProtectedFolders) {
        let base = tempfile::tempdir().unwrap();
        for dir in [
            "profile/Documents/Recordings",
            "appdata",
            "system/sub",
            "ordinary/recordings",
        ] {
            std::fs::create_dir_all(base.path().join(dir)).unwrap();
        }
        let protected = protected(&base.path().canonicalize().unwrap());
        (base, protected)
    }

    #[test]
    fn an_ordinary_existing_folder_is_accepted_and_resolved() {
        let (base, protected) = layout();
        let folder = base.path().join("ordinary").join("recordings");
        let resolved = validate_save_folder(&folder, &protected).unwrap();
        assert!(same_path(
            &resolved,
            &crate::database::meeting_files::plain_path(&folder.canonicalize().unwrap())
        ));
        // A folder below the profile is fine; the profile itself is not.
        assert!(validate_save_folder(
            &base.path().join("profile/Documents/Recordings"),
            &protected
        )
        .is_ok());
        // `..` segments are resolved before judging.
        assert!(validate_save_folder(&folder.join("..").join("recordings"), &protected).is_ok());
    }

    #[test]
    fn profile_app_data_system_and_drive_roots_are_refused() {
        let (base, protected) = layout();
        for refused in [
            base.path().join("profile"),
            base.path().join("appdata"),
            base.path().join("system"),
            base.path().join("system/sub"),
            base.path().join("ordinary/recordings/../../profile"),
        ] {
            assert_eq!(
                validate_save_folder(&refused, &protected).unwrap_err(),
                SAVE_FOLDER_REFUSED
            );
        }
        // A filesystem root (a drive root on Windows) is never acceptable.
        let root = base.path().ancestors().last().unwrap().to_path_buf();
        assert_eq!(
            validate_save_folder(&root, &ProtectedFolders::default()).unwrap_err(),
            SAVE_FOLDER_REFUSED
        );
    }

    #[test]
    fn missing_relative_empty_file_and_network_text_are_refused() {
        let (base, protected) = layout();
        std::fs::write(base.path().join("ordinary/file.txt"), b"x").unwrap();
        for refused in [
            base.path().join("ordinary/missing"),
            base.path().join("ordinary/file.txt"),
            PathBuf::from("relative/folder"),
            PathBuf::new(),
            PathBuf::from(r"\\server\share\recordings"),
            PathBuf::from(r"\\?\C:\recordings"),
            PathBuf::from(r"\\?\UNC\server\share\recordings"),
            PathBuf::from("//server/share/recordings"),
        ] {
            let error = validate_save_folder(&refused, &protected).unwrap_err();
            assert_eq!(error, SAVE_FOLDER_REFUSED);
            assert!(
                !error.contains("server"),
                "the message never names the path"
            );
        }
    }

    #[test]
    fn the_known_folders_of_this_machine_are_derived_not_hard_coded() {
        let protected = protected_folders(Some(PathBuf::from("app-data-root")));
        assert!(protected
            .exact
            .iter()
            .any(|folder| folder == &PathBuf::from("app-data-root")));
        if let Some(home) = dirs::home_dir() {
            assert!(protected.exact.contains(&home));
            assert_eq!(
                validate_save_folder(&home, &protected).unwrap_err(),
                SAVE_FOLDER_REFUSED
            );
        }
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            assert_eq!(
                validate_save_folder(std::path::Path::new(&root), &protected).unwrap_err(),
                SAVE_FOLDER_REFUSED
            );
        }
    }
}

#[cfg(test)]
mod root_tests {
    use super::*;
    use crate::behavior_settings::test_memory::Memory;

    #[test]
    fn the_roots_are_the_defaults_the_restored_backups_and_what_the_user_trusted() {
        let base = tempfile::tempdir().unwrap();
        let (share, local, data) = (
            base.path().join("share"),
            base.path().join("local"),
            base.path().join("data"),
        );
        let roots = roots_from(vec![share.clone(), local.clone()], Some(data.clone()));
        for expected in [
            get_default_recordings_folder(),
            get_legacy_default_recordings_folder(),
            share,
            local,
            data.join("restored-recordings"),
        ] {
            assert!(roots.contains(&expected), "{expected:?} missing");
        }
        assert_eq!(roots_from(vec![], None).len(), 2);
    }

    #[test]
    fn a_changed_folder_as_plain_text_is_refused_and_only_the_pickers_choice_is_accepted() {
        let base = tempfile::tempdir().unwrap();
        let (current, picked, typed) = (
            base.path().join("current"),
            base.path().join("picked"),
            base.path().join("typed"),
        );
        let default = get_default_recordings_folder();
        let take = |text: &str| {
            crate::file_grants::take(crate::file_grants::Purpose::RecordingsFolder, text)
        };
        // Unchanged and the default need no grant.
        assert!(matches!(
            classify_folder_request(Some(&current), &current, &default, take),
            Ok(FolderChange::Keep(_))
        ));
        assert!(matches!(
            classify_folder_request(Some(&current), &default, &default, take),
            Ok(FolderChange::Default)
        ));
        // Text the webview made up, a network folder included, is not trusted.
        crate::local_path::mark_remote_for_tests(&base.path().join("share"));
        for text in [
            typed.clone(),
            base.path().join("share").join("recordings"),
            PathBuf::from(r"\\host\share\recordings"),
        ] {
            assert!(
                classify_folder_request(Some(&current), &text, &default, take).is_err(),
                "{}",
                text.display()
            );
        }
        // The folder the picker returned is accepted once, network or not.
        crate::file_grants::grant(crate::file_grants::Purpose::RecordingsFolder, &picked).unwrap();
        assert!(matches!(
            classify_folder_request(Some(&current), &picked, &default, take),
            Ok(FolderChange::Granted(_))
        ));
        assert!(
            classify_folder_request(Some(&current), &picked, &default, take).is_err(),
            "used up"
        );
        let share_pick = base.path().join("share").join("picked");
        crate::file_grants::grant(crate::file_grants::Purpose::RecordingsFolder, &share_pick)
            .unwrap();
        assert!(matches!(
            classify_folder_request(Some(&current), &share_pick, &default, take),
            Ok(FolderChange::Granted(_))
        ));
    }

    #[test]
    fn a_folder_the_picker_returned_is_validated_and_a_share_root_is_refused() {
        let protected = protected_folders(None);
        let base = tempfile::tempdir().unwrap();
        let fine = base.path().join("recordings");
        std::fs::create_dir_all(&fine).unwrap();
        assert!(validate_save_folder(&fine, &protected).is_ok());
        if let Some(home) = dirs::home_dir() {
            assert!(validate_save_folder(&home, &protected).is_err());
            assert!(validate_save_folder(&home.join("AppData"), &protected).is_err());
        }
        // A folder marked as network is accepted like any other; a drive root is not.
        let share = base.path().join("share");
        std::fs::create_dir_all(share.join("recordings")).unwrap();
        crate::local_path::mark_remote_for_tests(&share);
        assert!(validate_save_folder(&share.join("recordings"), &protected).is_ok());
        let root = base.path().ancestors().last().unwrap().to_path_buf();
        assert!(
            validate_save_folder(&root, &protected).is_err(),
            "a drive root"
        );
    }

    #[test]
    fn the_known_user_folders_themselves_are_refused_and_subfolders_are_not() {
        let protected = protected_folders(None);
        for known in [
            dirs::document_dir(),
            dirs::desktop_dir(),
            dirs::download_dir(),
        ]
        .into_iter()
        .flatten()
        {
            if known.is_dir() {
                assert_eq!(
                    validate_save_folder(&known, &protected).unwrap_err(),
                    SAVE_FOLDER_REFUSED,
                    "{}",
                    known.display()
                );
            }
        }
        if let Some(home) = dirs::home_dir() {
            if let Some(users) = home.parent() {
                assert!(
                    validate_save_folder(users, &protected).is_err(),
                    "the folder holding every profile"
                );
            }
        }
    }

    #[test]
    fn only_trusted_or_default_folders_are_accepted_as_the_recordings_folder() {
        let trust = crate::trusted_roots::Trust::default();
        let defaults = [
            get_default_recordings_folder(),
            get_legacy_default_recordings_folder(),
        ];
        let base = tempfile::tempdir().unwrap();
        let elsewhere = base.path().join("elsewhere");
        assert!(!crate::trusted_roots::is_trusted_or_default(
            &trust, &elsewhere, &defaults
        ));
        assert!(crate::trusted_roots::is_trusted_or_default(
            &trust,
            &get_default_recordings_folder(),
            &defaults
        ));
        let memory = Memory::default();
        std::fs::create_dir_all(&elsewhere).unwrap();
        crate::trusted_roots::trust_folder(&memory, &elsewhere, &ProtectedFolders::default())
            .unwrap();
        assert!(crate::trusted_roots::is_trusted_or_default(
            &crate::trusted_roots::read(&memory),
            &elsewhere,
            &defaults
        ));
        assert!(!FOLDER_NOTICE.contains(":\\"), "the notice names no path");
    }

    #[cfg(windows)]
    #[test]
    fn system_top_level_folders_are_recognised_on_any_drive() {
        for refused in [
            r"C:\Windows",
            r"D:\windows\System32",
            r"E:\Program Files",
            r"G:\ProgramData",
            r"C:\Users",
            r"C:\$Recycle.Bin\S-1",
        ] {
            assert!(
                is_system_top_level(std::path::Path::new(refused)),
                "{refused}"
            );
        }
        for fine in [r"D:\Recordings", r"E:\Data\Windows", r"C:\"] {
            assert!(!is_system_top_level(std::path::Path::new(fine)), "{fine}");
        }
    }
}
