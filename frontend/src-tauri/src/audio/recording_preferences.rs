use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_store::StoreExt;

use anyhow::Result;
#[cfg(target_os = "macos")]
use log::error;

#[cfg(target_os = "macos")]
use crate::audio::capture::AudioCaptureBackend;

const DEFAULT_RECORDINGS_FOLDER_NAME: &str = "ClawScribe";
const LEGACY_RECORDINGS_FOLDER_NAME: &str = "meetily-recordings";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecordingPreferences {
    pub save_folder: PathBuf,
    pub auto_save: bool,
    pub file_format: String,
    #[serde(default)]
    pub preferred_mic_device: Option<String>,
    #[serde(default)]
    pub preferred_system_device: Option<String>,
    #[cfg(target_os = "macos")]
    #[serde(default)]
    pub system_audio_backend: Option<String>,
}

impl Default for RecordingPreferences {
    fn default() -> Self {
        Self {
            save_folder: get_default_recordings_folder(),
            auto_save: true,
            file_format: "mp4".to_string(),
            preferred_mic_device: None,
            preferred_system_device: None,
            #[cfg(target_os = "macos")]
            system_audio_backend: Some("coreaudio".to_string()),
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
    platform_recordings_folder(DEFAULT_RECORDINGS_FOLDER_NAME)
}

pub(crate) fn get_legacy_default_recordings_folder() -> PathBuf {
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

/// The roots from their parts: the current and previous defaults, the
/// configured folder, folders configured before, and restored backups.
pub(crate) fn roots_from(
    configured: Option<PathBuf>,
    previous: Vec<PathBuf>,
    app_data_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut roots = vec![
        get_default_recordings_folder(),
        get_legacy_default_recordings_folder(),
    ];
    roots.extend(configured);
    roots.extend(previous);
    roots.extend(app_data_dir.map(|data| data.join("restored-recordings")));
    roots
}

/// Every folder a meeting's recording files may live in. Deleting or
/// recovering a folder outside these is refused.
pub(crate) async fn recording_roots<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    roots_from(
        load_recording_preferences(app)
            .await
            .ok()
            .map(|preferences| preferences.save_folder),
        read_previous_save_folders(app),
        tauri::Manager::path(app).app_data_dir().ok(),
    )
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
    // Try to load from Tauri store
    let store = match app.store("recording_preferences.json") {
        Ok(store) => store,
        Err(e) => {
            warn!("Failed to access store: {}, using defaults", e);
            return Ok(RecordingPreferences::default());
        }
    };

    // Try to get the preferences from store
    let mut prefs = if let Some(value) = store.get("preferences") {
        match serde_json::from_value::<RecordingPreferences>(value.clone()) {
            Ok(p) => {
                info!("Loaded recording preferences from store");
                normalize_platform_preferences(p)
            }
            Err(e) => {
                warn!("Failed to deserialize preferences: {}, using defaults", e);
                RecordingPreferences::default()
            }
        }
    } else {
        info!("No stored preferences found, using defaults");
        RecordingPreferences::default()
    };

    if migrate_legacy_recordings_folder(&mut prefs) {
        info!(
            "Migrated legacy default recordings folder to {:?}",
            prefs.save_folder
        );

        match serde_json::to_value(&prefs) {
            Ok(prefs_value) => {
                store.set("preferences", prefs_value);
                if let Err(e) = store.save() {
                    warn!("Failed to persist migrated recording preferences: {}", e);
                }
            }
            Err(e) => warn!("Failed to serialize migrated preferences: {}", e),
        }
    }

    info!("Loaded recording preferences: save_folder={:?}, auto_save={}, format={}, mic={:?}, system={:?}",
          prefs.save_folder, prefs.auto_save, prefs.file_format,
          prefs.preferred_mic_device, prefs.preferred_system_device);
    Ok(prefs)
}

/// Save recording preferences to store
pub async fn save_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
    preferences: &RecordingPreferences,
) -> Result<()> {
    info!("Saving recording preferences: save_folder={:?}, auto_save={}, format={}, mic={:?}, system={:?}",
          preferences.save_folder, preferences.auto_save, preferences.file_format,
          preferences.preferred_mic_device, preferences.preferred_system_device);

    // Get or create store
    let store = app
        .store("recording_preferences.json")
        .map_err(|e| anyhow::anyhow!("Failed to access store: {}", e))?;

    // Serialize preferences to JSON value
    let prefs_value = serde_json::to_value(preferences)
        .map_err(|e| anyhow::anyhow!("Failed to serialize preferences: {}", e))?;

    // A changed save folder must not strand recordings made in the old one:
    // remember it so recovery and cleanup still accept it.
    let _remembering = PREVIOUS_FOLDERS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let old_folder = store
        .get("preferences")
        .and_then(|value| value.get("save_folder").cloned())
        .and_then(|value| serde_json::from_value::<PathBuf>(value).ok());
    if let Some(old_folder) = old_folder {
        let remembered = store
            .get(PREVIOUS_SAVE_FOLDERS_KEY)
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
    store
        .save()
        .map_err(|e| anyhow::anyhow!("Failed to save store to disk: {}", e))?;

    info!("Successfully persisted recording preferences to disk");

    // Save backend preference to global config
    #[cfg(target_os = "macos")]
    if let Some(backend_str) = &preferences.system_audio_backend {
        if let Some(backend) = AudioCaptureBackend::from_string(backend_str) {
            info!("Setting audio capture backend to: {:?}", backend);
            crate::audio::capture::set_current_backend(backend);
        }
    }

    // Ensure the directory exists
    ensure_recordings_directory(&preferences.save_folder)?;

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
    for variable in ["SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"] {
        protected.trees.extend(std::env::var_os(variable).map(PathBuf::from));
    }
    #[cfg(unix)]
    protected.trees.extend(
        ["/etc", "/usr", "/bin", "/sbin", "/lib", "/var", "/boot", "/dev", "/proc", "/sys", "/System", "/Library"]
            .map(PathBuf::from),
    );
    protected
}

/// The recordings folder a caller asked for, if it may be one: an existing
/// local directory that is not a drive root, UNC or device path, the profile or
/// application-data root, or part of the system. UNC, device and
/// extended-length text is refused before any filesystem call. Returns the
/// resolved folder; the error never contains the path.
pub(crate) fn validate_save_folder(
    requested: &std::path::Path,
    protected: &ProtectedFolders,
) -> Result<PathBuf, String> {
    let text = requested.to_string_lossy();
    if text.trim().is_empty()
        || crate::database::meeting_files::is_network_or_device_path(&text)
        || !requested.is_absolute()
    {
        return Err(SAVE_FOLDER_REFUSED.into());
    }
    let canonical = requested
        .canonicalize()
        .map_err(|_| SAVE_FOLDER_REFUSED.to_string())?;
    if !canonical.is_dir() {
        return Err(SAVE_FOLDER_REFUSED.into());
    }
    let resolved = crate::database::meeting_files::plain_path(&canonical);
    let resolved_text = resolved.to_string_lossy();
    if crate::database::meeting_files::is_network_or_device_path(&resolved_text)
        || resolved.parent().is_none()
    {
        return Err(SAVE_FOLDER_REFUSED.into());
    }
    let known = |folder: &PathBuf| {
        folder
            .canonicalize()
            .ok()
            .map(|folder| crate::database::meeting_files::plain_path(&folder))
    };
    if protected
        .exact
        .iter()
        .filter_map(known)
        .any(|folder| same_path(&folder, &resolved))
        || protected
            .trees
            .iter()
            .filter_map(known)
            .any(|folder| resolved.starts_with(&folder) || same_path(&folder, &resolved))
    {
        return Err(SAVE_FOLDER_REFUSED.into());
    }
    Ok(resolved)
}

#[tauri::command]
pub async fn set_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
    mut preferences: RecordingPreferences,
) -> Result<(), String> {
    // A save folder that stays as it is needs no new check (other settings can
    // still be saved); a changed one must be a proper recordings folder.
    let current = load_recording_preferences(&app).await.ok();
    let unchanged = current
        .as_ref()
        .is_some_and(|current| same_path(&current.save_folder, &preferences.save_folder));
    if !unchanged {
        let requested = preferences.save_folder.clone();
        // The default folder may not exist yet; it is the app's own to create.
        if same_path(&requested, &get_default_recordings_folder()) {
            ensure_recordings_directory(&requested)
                .map_err(|_| "The recordings folder could not be created".to_string())?;
        }
        let protected = protected_folders(tauri::Manager::path(&app).app_data_dir().ok());
        preferences.save_folder = tokio::task::spawn_blocking(move || {
            validate_save_folder(&requested, &protected)
        })
        .await
        .map_err(|_| "Could not check the recordings folder".to_string())??;
    }
    save_recording_preferences(&app, &preferences)
        .await
        .map_err(|e| format!("Failed to save recording preferences: {}", e))
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
        std::process::Command::new("explorer")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
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
        let folder_path = path.to_string();
        let folder = PathBuf::from(&folder_path);
        ensure_recordings_directory(&folder)
            .map_err(|e| format!("Failed to create selected recordings folder: {}", e))?;
        let protected = protected_folders(tauri::Manager::path(&app).app_data_dir().ok());
        let checked = folder.clone();
        tokio::task::spawn_blocking(move || validate_save_folder(&checked, &protected))
            .await
            .map_err(|_| "Could not check the recordings folder".to_string())??;

        info!("User selected recordings folder: {}", folder_path);
        Ok(Some(folder_path))
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
        assert_eq!(again.iter().filter(|folder| **folder == p("/old14")).count(), 1);
    }

    #[test]
    fn trailing_separators_do_not_make_a_folder_a_different_one() {
        let remembered = remember_previous_folder(&[p("/data/old")], Path::new("/data/old/"), Path::new("/data/new"));
        assert_eq!(remembered, vec![p("/data/old")], "the same folder with a trailing slash is not added twice");
        let again = remember_previous_folder(&remembered, Path::new("/data/older//"), Path::new("/data/old/"));
        // The folder now in use leaves the list whatever its trailing slash.
        assert_eq!(again, vec![p("/data/older//")]);
        assert!(same_path(Path::new("/"), Path::new("/")));
        assert!(!same_path(Path::new("/"), Path::new("/data")));
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_slash_direction_and_case_do_not_matter() {
        assert!(same_path(Path::new(r"D:\Recordings\"), Path::new("d:/recordings")));
        assert!(same_path(Path::new(r"C:\"), Path::new("c:/")));
        assert!(!same_path(Path::new(r"C:\"), Path::new(r"D:\")));
        let list = remember_previous_folder(&[p(r"D:\Old")], Path::new("d:/old/"), Path::new(r"E:\New"));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn the_roots_include_previous_folders_defaults_and_restored_backups() {
        let roots = roots_from(
            Some(p("/current")),
            vec![p("/before"), p("/long-ago")],
            Some(p("/data")),
        );
        for expected in [
            get_default_recordings_folder(),
            get_legacy_default_recordings_folder(),
            p("/current"),
            p("/before"),
            p("/long-ago"),
            p("/data").join("restored-recordings"),
        ] {
            assert!(roots.contains(&expected), "{expected:?} missing");
        }
        assert_eq!(roots_from(None, vec![], None).len(), 2);
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
        for dir in ["profile/Documents/Recordings", "appdata", "system/sub", "ordinary/recordings"] {
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
        assert!(same_path(&resolved, &crate::database::meeting_files::plain_path(&folder.canonicalize().unwrap())));
        // A folder below the profile is fine; the profile itself is not.
        assert!(validate_save_folder(&base.path().join("profile/Documents/Recordings"), &protected).is_ok());
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
            assert_eq!(validate_save_folder(&refused, &protected).unwrap_err(), SAVE_FOLDER_REFUSED);
        }
        // A filesystem root (a drive root on Windows) is never acceptable.
        let root = base.path().ancestors().last().unwrap().to_path_buf();
        assert_eq!(validate_save_folder(&root, &ProtectedFolders::default()).unwrap_err(), SAVE_FOLDER_REFUSED);
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
            assert!(!error.contains("server"), "the message never names the path");
        }
    }

    #[test]
    fn the_known_folders_of_this_machine_are_derived_not_hard_coded() {
        let protected = protected_folders(Some(PathBuf::from("app-data-root")));
        assert!(protected.exact.iter().any(|folder| folder == &PathBuf::from("app-data-root")));
        if let Some(home) = dirs::home_dir() {
            assert!(protected.exact.contains(&home));
            assert_eq!(validate_save_folder(&home, &protected).unwrap_err(), SAVE_FOLDER_REFUSED);
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
