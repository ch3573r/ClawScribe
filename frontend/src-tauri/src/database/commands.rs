use log::{error, info};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use crate::state::AppState;

#[derive(Serialize)]
pub struct DatabaseCheckResult {
    pub exists: bool,
    pub size: u64,
}

/// Check if this is the first launch (no database exists yet)
#[tauri::command]
pub async fn check_first_launch(app: AppHandle) -> Result<bool, String> {
    DatabaseManager::is_first_launch(&app)
        .await
        .map_err(|e| format!("Failed to check first launch: {}", e))
}

/// The legacy databases the core knows about: the one the earlier backend left
/// in the app's own data folder and, on macOS, the Homebrew one. Chosen by the
/// core; nothing here comes from the webview.
fn legacy_database_candidates(app_data_dir: &std::path::Path) -> Vec<PathBuf> {
    [
        app_data_dir.join("meeting_minutes.db"),
        #[cfg(target_os = "macos")]
        PathBuf::from("/usr/local/var/meetily/meeting_minutes.db"),
    ]
    .into()
}

/// The first of the candidates that is a non-empty regular file.
fn find_legacy_database(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find_map(|path| {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        (metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0)
            .then(|| path.clone())
    })
}

/// First launch only: imports the legacy database the core finds, if there is
/// one, and starts the app's database from it. Takes no path: the webview cannot
/// point it at a file. Refused once a database exists. Returns whether a legacy
/// database was imported; when it returns false the caller starts a fresh one.
/// The legacy file is read through a read-only connection and never modified.
#[tauri::command]
pub async fn import_legacy_database_if_present(app: AppHandle) -> Result<bool, String> {
    if super::setup::startup_failed() || app.try_state::<AppState>().is_some() {
        return Err("The database already exists".into());
    }
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    let legacy = tokio::task::spawn_blocking(move || {
        find_legacy_database(&legacy_database_candidates(&app_data_dir))
    })
    .await
    .map_err(|_| "Legacy database detection was interrupted")?;
    let Some(legacy) = legacy else {
        info!("No legacy database found");
        return Ok(false);
    };
    info!("Importing the legacy database");

    let db_manager = DatabaseManager::import_legacy_database(&app, &legacy.to_string_lossy())
        .await
        .map_err(|e| {
            error!("Failed to import legacy database: {}", e);
            format!("Failed to import database: {}", e)
        })?;

    app.manage(AppState { db_manager });
    crate::audio::recording_preferences::take_over_registered_folders(&app).await;
    crate::knowledge::indexer::start_installed_pool(&app);
    info!("Legacy database imported and initialized successfully");
    app.emit("database-initialized", ())
        .map_err(|e| format!("Failed to emit database-initialized event: {}", e))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_non_empty_regular_file_in_a_known_place_counts_as_a_legacy_database() {
        let dir = tempfile::tempdir().unwrap();
        let known = dir.path().join("meeting_minutes.db");
        assert_eq!(find_legacy_database(&[known.clone()]), None);
        std::fs::write(&known, b"").unwrap();
        assert_eq!(
            find_legacy_database(&[known.clone()]),
            None,
            "an empty file is not one"
        );
        std::fs::write(&known, b"SQLite format 3\0").unwrap();
        assert_eq!(find_legacy_database(&[known.clone()]), Some(known.clone()));
        std::fs::remove_file(&known).unwrap();
        std::fs::create_dir(&known).unwrap();
        assert_eq!(find_legacy_database(&[known]), None, "a folder is not one");
        let app_data = std::path::Path::new("app-data");
        assert!(legacy_database_candidates(app_data)[0].ends_with("meeting_minutes.db"));
    }
}

/// Initialize a fresh database (for users who don't want to import)
#[tauri::command]
pub async fn initialize_fresh_database(app: AppHandle) -> Result<(), String> {
    if super::setup::startup_failed() || app.try_state::<AppState>().is_some() {
        return Err("The database cannot be initialized from this window.".into());
    }
    info!("Initializing fresh database");

    let db_manager = DatabaseManager::new_from_app_handle(&app)
        .await
        .map_err(|e| {
            error!("Failed to initialize fresh database: {}", e);
            format!("Failed to initialize database: {}", e)
        })?;

    // Update app state with the new manager
    app.manage(AppState {
        db_manager: db_manager.clone(),
    });
    crate::knowledge::indexer::start_installed_pool(&app);

    info!("Fresh database initialized successfully");

    // Emit event to notify frontend that database is ready
    app.emit("database-initialized", ())
        .map_err(|e| format!("Failed to emit database-initialized event: {}", e))?;

    Ok(())
}

/// Get the database directory path
#[tauri::command]
pub async fn get_database_directory(app: AppHandle) -> Result<String, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    Ok(app_data_dir.to_string_lossy().to_string())
}

/// Open the database folder in the system file explorer
#[tauri::command]
pub async fn open_database_folder(app: AppHandle) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    // Ensure directory exists before trying to open it
    if !app_data_dir.exists() {
        std::fs::create_dir_all(&app_data_dir)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let folder_path = app_data_dir.to_string_lossy().to_string();

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

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    info!("Opened database folder: {}", folder_path);
    Ok(())
}
