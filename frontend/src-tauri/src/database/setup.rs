use log::info;
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use crate::state::AppState;

/// Initialize database on app startup
/// Handles first launch detection and conditional initialization
pub async fn initialize_database_on_startup(app: &AppHandle) -> Result<(), String> {
    super::recovery::startup_status()?;
    // Check if this is the first launch (no database exists yet)
    let is_first_launch = DatabaseManager::is_first_launch(app)
        .await
        .map_err(|e| format!("Failed to check first launch status: {}", e))?;

    if is_first_launch && super::recovery::data_dir().is_some() {
        return Err("The selected recovery database is missing.".into());
    }
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

static STARTUP_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True once the meeting database could not be opened at startup. The app is
/// then waiting for the recovery choice. Do not show the main window, start
/// a recording or run a command-line request in this state.
pub fn startup_failed() -> bool {
    STARTUP_FAILED.load(std::sync::atomic::Ordering::SeqCst)
}

/// How long the app stays alive for a startup-failure dialog nobody closes (or
/// that never appeared). Ten minutes is long enough to read the message and
/// find the log; after that the process ends with code 1 rather than staying
/// alive invisibly without a window or a tray.
pub(crate) const STARTUP_FAILURE_EXIT_AFTER: std::time::Duration =
    std::time::Duration::from_secs(10 * 60);

#[cfg(test)]
pub(crate) fn set_startup_failed_for_tests(failed: bool) {
    STARTUP_FAILED.store(failed, std::sync::atomic::Ordering::SeqCst);
}

/// Text of the dialog shown when the meeting database cannot be opened.
/// The underlying error goes to the log only: it can contain paths and
/// internals that mean nothing to the user.
pub(crate) fn startup_failure_message(
    data_dir: Option<&std::path::Path>,
    log_dir: Option<&std::path::Path>,
) -> String {
    let show = |dir: Option<&std::path::Path>| {
        dir.map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "not available".to_string())
    };
    format!(
        "ClawScribe could not open its meeting database.\n\n\
         App data folder: {}\n\
         Log folder: {}\n\n\
         Choose Open recovery library to restart ClawScribe in a separate \
         workspace. A previous recovery workspace is reused when available. \
         You can restore a ZIP from Meetings. \
         Your current database, recovery files and recordings stay in place.\n\n\
         Choose Exit to close ClawScribe and check the log.",
        show(data_dir),
        show(log_dir)
    )
}

/// Reports a database that could not be opened at startup: logs the error,
/// shows a native error dialog, and exits with a non-zero code once the user
/// closes it. The dialog is not blocking, because setup runs on the main
/// thread before the event loop starts.
pub fn report_startup_failure(app: &AppHandle, error: &str) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    STARTUP_FAILED.store(true, std::sync::atomic::Ordering::SeqCst);
    log::error!("The meeting database could not be opened: {error}");
    let message = startup_failure_message(
        app.path().app_data_dir().ok().as_deref(),
        app.path().app_log_dir().ok().as_deref(),
    );
    // Nothing useful can be shown without the database.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    let recovery_app = app.clone();
    app.dialog()
        .message(message)
        .title("Recover the meeting library")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Open recovery library".into(),
            "Exit".into(),
        ))
        .show(move |recover| {
            if !recover {
                recovery_app.exit(1);
                return;
            }
            tauri::async_runtime::spawn(async move {
                match super::recovery::prepare().await {
                    Ok(()) => recovery_app.request_restart(),
                    Err(message) => {
                        let exit_app = recovery_app.clone();
                        recovery_app
                            .dialog()
                            .message(message)
                            .title("Could not open a recovery library")
                            .kind(MessageDialogKind::Error)
                            .show(move |_| exit_app.exit(1));
                    }
                }
            });
        });
    // Fallback: the dialog normally ends the process (above). If it never
    // appears or is never closed, do not stay alive without a window.
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(STARTUP_FAILURE_EXIT_AFTER).await;
        log::error!("The startup error dialog was not closed in time; exiting");
        std::process::exit(1);
    });
}

#[cfg(test)]
mod startup_failure_tests {
    use super::startup_failure_message;
    use std::path::Path;

    #[test]
    fn the_message_names_the_data_and_log_folders_in_plain_words() {
        let message = startup_failure_message(
            Some(Path::new("D:/data/app")),
            Some(Path::new("D:/data/app/logs")),
        );
        assert!(message.contains("could not open its meeting database"));
        assert!(message.contains("App data folder: D:/data/app"));
        assert!(message.contains("Log folder: D:/data/app/logs"));
        assert!(!message.contains("panicked"));
    }

    #[test]
    fn the_fallback_waits_long_enough_to_read_the_dialog_but_is_bounded() {
        use super::STARTUP_FAILURE_EXIT_AFTER as limit;
        assert!(limit >= std::time::Duration::from_secs(5 * 60));
        assert!(limit <= std::time::Duration::from_secs(60 * 60));
    }

    #[test]
    fn missing_folders_are_reported_not_hidden() {
        let message = startup_failure_message(None, None);
        assert!(message.contains("App data folder: not available"));
        assert!(message.contains("Log folder: not available"));
    }
}
