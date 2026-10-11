use log::info;
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use crate::state::AppState;

/// Initialize database on app startup
/// Handles first launch detection and conditional initialization
pub async fn initialize_database_on_startup(app: &AppHandle) -> Result<(), String> {
    // Check if this is the first launch (no database exists yet)
    let is_first_launch = DatabaseManager::is_first_launch(app)
        .await
        .map_err(|e| format!("Failed to check first launch status: {}", e))?;

    if is_first_launch {
        info!("First launch detected - will notify window when ready");

        // Delay event emission to ensure window is ready and React listeners are registered
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            app_handle
                .emit("first-launch-detected", ())
                .expect("Failed to emit first-launch-detected event");
            info!("Emitted first-launch-detected after delay");
        });
    } else {
        // Normal flow - initialize database immediately
        let db_manager = DatabaseManager::new_from_app_handle(app)
            .await
            .map_err(|e| format!("Failed to initialize database manager: {}", e))?;

        let pool = db_manager.pool().clone();
        // Finish bookmark cleanup before publishing the database to recording commands.
        if crate::library::cleanup_orphaned_bookmarks(&pool)
            .await
            .is_err()
        {
            log::warn!("Could not clean abandoned recording bookmarks");
        }
        // Run once per startup in the background; busy foreground work skips it.
        let cleanup_pool = pool.clone();
        let originals = app
            .path()
            .app_data_dir()
            .map(|data| data.join("reference-documents"));
        let imports = app
            .state::<crate::knowledge::KnowledgeState>()
            .documents
            .clone();
        tauri::async_runtime::spawn(async move {
            match originals {
                Ok(root) => {
                    match crate::knowledge::document_cleanup::sweep(&cleanup_pool, &root, &imports)
                        .await
                    {
                        Ok(warnings) => {
                            for warning in warnings {
                                log::warn!("{}", warning);
                            }
                        }
                        Err(warning) => log::warn!("{}", warning),
                    }
                }
                Err(_) => log::warn!("Reference document storage is unavailable for cleanup"),
            }
        });
        tauri::async_runtime::spawn(async move {
            crate::summary::codex_provider::cleanup_orphaned_runs(&pool).await;
        });
        app.manage(AppState { db_manager });
        crate::knowledge::indexer::start_installed_pool(app);
        info!("Database initialized successfully");
    }

    Ok(())
}
