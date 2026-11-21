use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
// Removed unused import

// Performance optimization: Conditional logging macros for hot paths
#[cfg(debug_assertions)]
macro_rules! perf_debug {
    ($($arg:tt)*) => {
        log::debug!($($arg)*)
    };
}

#[cfg(not(debug_assertions))]
macro_rules! perf_debug {
    ($($arg:tt)*) => {};
}

// Make this macro available to other modules. The re-export is consumed
// from submodules, which Rust's unused import lint does not track here.
#[allow(unused_imports)]
pub(crate) use perf_debug;

// Re-export async logging macros for external use (removed due to macro conflicts)

// Declare audio module
pub mod anthropic;
pub mod api;
pub mod audio;
pub mod behavior_settings;
pub mod cli_control;
pub mod config;
pub(crate) mod credentials;
pub mod database;
pub mod diagnostics;
pub mod exit_guard;
pub mod exports;
mod external_url;
pub(crate) mod file_grants;
pub mod groq;
mod ical_sources;
pub mod knowledge;
pub mod library;
pub(crate) mod local_path;
pub mod mcp;
pub(crate) mod model_download;
pub mod nemotron_engine;
pub mod notifications;
pub mod ollama;
pub mod onboarding;
pub mod openai;
pub mod openclaw;
pub mod openrouter;
pub mod parakeet_engine;
pub mod power;
pub mod self_test;
pub mod shortcuts;
pub mod start_request;
pub mod state;
pub mod summary;
pub mod teams_detection;
pub mod tray;
pub(crate) mod trusted_roots;
pub mod updates;
pub mod utils;
pub mod whisper_engine;
#[cfg(windows)]
mod windows_lifecycle;

use audio::recording_commands::floating;
use audio::{list_audio_devices, trigger_audio_permission, AudioDevice};
use log::{error as log_error, info as log_info};
use notifications::commands::NotificationManagerState;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime, Theme};
use tokio::sync::RwLock;

static RECORDING_FLAG: AtomicBool = AtomicBool::new(false);

// Default to transcription in the original language. The frontend normally
// synchronizes this value at startup, but recording can begin before that sync.
static LANGUAGE_PREFERENCE: std::sync::LazyLock<StdMutex<String>> =
    std::sync::LazyLock::new(|| StdMutex::new("auto".to_string()));

#[derive(Debug, Deserialize)]
struct RecordingArgs {
    save_path: String,
}

#[tauri::command]
async fn start_recording<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    log_info!("Recording start requested");

    if !tray::check_can_record(&app).await {
        return Err("recording_setup_incomplete".into());
    }
    if is_recording().await {
        return Err("Recording already in progress".to_string());
    }

    // Call the actual audio recording system with meeting name
    match audio::recording_commands::start_recording_with_devices_and_meeting(
        app.clone(),
        mic_device_name,
        system_device_name,
        meeting_name.clone(),
    )
    .await
    {
        Ok(_) => {
            RECORDING_FLAG.store(true, Ordering::SeqCst);
            tray::update_tray_menu(&app);

            log_info!("Recording started successfully");

            // Notification work must not hold up the command response; capture
            // is already running at this point.
            let app_for_notification = app.clone();
            tauri::async_runtime::spawn(async move {
                let notification_manager_state =
                    app_for_notification.state::<NotificationManagerState<R>>();
                if let Err(e) = notifications::commands::show_recording_started_notification(
                    &app_for_notification,
                    &notification_manager_state,
                    meeting_name.clone(),
                )
                .await
                {
                    log_error!("Failed to show recording started notification: {}", e);
                } else {
                    log_info!("Successfully showed recording started notification");
                }
            });

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to start audio recording: {}", e);
            Err(format!("Failed to start recording: {}", e))
        }
    }
}

#[tauri::command]
async fn stop_recording<R: Runtime>(app: AppHandle<R>, args: RecordingArgs) -> Result<(), String> {
    log_info!("Attempting to stop recording...");

    // Check the actual audio recording system state instead of the flag
    if !audio::recording_commands::is_recording().await {
        log_info!("Recording is already stopped");
        return Ok(());
    }

    // Call the actual audio recording system to stop
    match audio::recording_commands::stop_recording(
        app.clone(),
        audio::recording_commands::RecordingArgs {
            save_path: args.save_path.clone(),
        },
    )
    .await
    {
        Ok(false) => Ok(()),
        Ok(true) => {
            RECORDING_FLAG.store(false, Ordering::SeqCst);
            tray::update_tray_menu(&app);

            // Create the save directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(&args.save_path).parent() {
                if !parent.exists() {
                    log_info!("Creating directory: {:?}", parent);
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        let err_msg = format!("Failed to create save directory: {}", e);
                        log_error!("{}", err_msg);
                        return Err(err_msg);
                    }
                }
            }

            // Show recording stopped notification through NotificationManager
            // This respects user's notification preferences
            let notification_manager_state = app.state::<NotificationManagerState<R>>();
            if let Err(e) = notifications::commands::show_recording_stopped_notification(
                &app,
                &notification_manager_state,
            )
            .await
            {
                log_error!("Failed to show recording stopped notification: {}", e);
            } else {
                log_info!("Successfully showed recording stopped notification");
            }

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to stop audio recording: {}", e);
            // Still update the flag even if stopping failed
            RECORDING_FLAG.store(false, Ordering::SeqCst);
            tray::update_tray_menu(&app);
            Err(format!("Failed to stop recording: {}", e))
        }
    }
}

#[tauri::command]
async fn is_recording() -> bool {
    audio::recording_commands::is_recording().await
}

#[tauri::command]
async fn get_transcription_status() -> audio::recording_commands::TranscriptionStatus {
    audio::recording_commands::get_transcription_status().await
}

#[tauri::command]
async fn resolve_meeting_audio_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, state::AppState>,
    meeting_folder: String,
) -> Result<Option<String>, String> {
    // The trusted roots (a network recordings folder included) decide what may be looked at.
    let _ = audio::recording_preferences::recording_roots(&app).await;
    let Some(folder) = canonicalize_existing_dir(&meeting_folder)? else {
        return Ok(None);
    };

    let saved_meeting_folders = sqlx::query_scalar::<_, String>(
        "SELECT folder_path FROM meetings WHERE folder_path IS NOT NULL AND TRIM(folder_path) <> ''",
    )
    .fetch_all(state.db_manager.pool())
    .await
    .map_err(|e| format!("Failed to verify meeting folder: {}", e))?;

    if !canonical_folder_is_registered(&folder, saved_meeting_folders.iter().map(String::as_str)) {
        log::warn!("Refusing to resolve audio from an unregistered meeting folder");
        return Ok(None);
    }

    let path = audio::incremental_saver::resolve_audio_file_or_recover(&folder).await?;
    if let Some(path) = &path {
        let resolved = path
            .canonicalize()
            .map_err(|_| "Meeting audio could not be resolved")?;
        if resolved.parent() != Some(folder.as_path()) {
            return Err(
                "Meeting audio links outside the recording folder are not supported".into(),
            );
        }
        app.asset_protocol_scope()
            .allow_file(&resolved)
            .map_err(|_| "Meeting audio access could not be granted")?;
    }
    Ok(path.map(|path| path.to_string_lossy().to_string()))
}

fn canonicalize_existing_dir(path: impl AsRef<Path>) -> Result<Option<PathBuf>, String> {
    let path = path.as_ref();
    // A network or device path is never looked at unless its own spelling is
    // inside a root the user trusted: even asking whether it is a folder would
    // contact the remote machine.
    let roots = trusted_roots::snapshot();
    if !trusted_roots::text_allowed(&path.to_string_lossy(), &roots) {
        return Ok(None);
    }
    if !path.is_dir() {
        return Ok(None);
    }

    let resolved = path
        .canonicalize()
        .map_err(|_| "Failed to resolve the meeting folder".to_string())?;
    // A mapped network drive resolves to a UNC path: fine inside a trusted root.
    Ok((local_path::is_local_canonical(&resolved)
        || trusted_roots::canonical_inside(&resolved, &roots))
    .then_some(resolved))
}

fn canonical_folder_is_registered<'a>(
    requested_folder: &Path,
    meeting_folders: impl IntoIterator<Item = &'a str>,
) -> bool {
    meeting_folders.into_iter().any(|meeting_folder| {
        canonicalize_existing_dir(meeting_folder)
            .ok()
            .flatten()
            .is_some_and(|registered_folder| {
                canonical_paths_equal(&registered_folder, requested_folder)
            })
    })
}

fn canonical_paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }

    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_meeting_folder_requires_existing_canonical_match() {
        let temp_dir = tempfile::tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Meeting A");
        let other_folder = temp_dir.path().join("Other");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(&other_folder).unwrap();

        let requested = meeting_folder.canonicalize().unwrap();

        assert!(canonical_folder_is_registered(
            &requested,
            [meeting_folder.to_string_lossy().as_ref()]
        ));
        assert!(!canonical_folder_is_registered(
            &requested,
            [other_folder.to_string_lossy().as_ref()]
        ));
        assert!(!canonical_folder_is_registered(
            &requested,
            [temp_dir.path().join("Missing").to_string_lossy().as_ref()]
        ));
    }

    #[test]
    fn find_audio_file_prefers_named_audio_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let meeting_folder = temp_dir.path();
        let fallback_audio = meeting_folder.join("clip.wav");
        let preferred_audio = meeting_folder.join("audio.m4a");
        std::fs::write(&fallback_audio, b"fallback").unwrap();
        std::fs::write(&preferred_audio, b"preferred").unwrap();

        let resolved = audio::incremental_saver::find_existing_audio_file(meeting_folder)
            .unwrap()
            .to_string_lossy()
            .to_string();

        assert_eq!(PathBuf::from(resolved), preferred_audio);
    }
}

// Audio level monitoring commands
#[tauri::command]
async fn start_audio_level_monitoring<R: Runtime>(
    app: AppHandle<R>,
    device_names: Vec<String>,
) -> Result<Vec<audio::simple_level_monitor::MonitoredMicrophone>, String> {
    log_info!(
        "Starting audio level monitoring for devices: {:?}",
        device_names
    );

    audio::simple_level_monitor::start_monitoring(app, device_names)
        .await
        .map_err(|e| format!("Failed to start audio level monitoring: {}", e))
}

#[tauri::command]
async fn stop_audio_level_monitoring() -> Result<(), String> {
    log_info!("Stopping audio level monitoring");

    audio::simple_level_monitor::stop_monitoring()
        .await
        .map_err(|e| format!("Failed to stop audio level monitoring: {}", e))
}

// Whisper commands are now handled by whisper_engine::commands module

#[tauri::command]
async fn get_audio_devices() -> Result<Vec<AudioDevice>, String> {
    list_audio_devices()
        .await
        .map_err(|e| format!("Failed to list audio devices: {}", e))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordingDeviceInventory {
    devices: Vec<AudioDevice>,
    default_microphone: Option<String>,
    default_output: Option<String>,
}

#[tauri::command]
async fn get_recording_device_inventory() -> Result<RecordingDeviceInventory, String> {
    let devices = get_audio_devices().await?;
    Ok(RecordingDeviceInventory {
        devices,
        default_microphone: audio::default_input_device().ok().map(|device| device.name),
        default_output: audio::default_output_device()
            .ok()
            .map(|device| device.name),
    })
}

#[tauri::command]
async fn trigger_microphone_permission() -> Result<bool, String> {
    trigger_audio_permission()
        .map_err(|e| format!("Failed to trigger microphone permission: {}", e))
}

#[tauri::command]
async fn start_recording_with_devices_and_meeting<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
    use_default_microphone: Option<bool>,
    calendar_link: Option<library::calendar::CalendarLink>,
    summary_language: Option<String>,
    request_id: Option<String>,
) -> Result<String, String> {
    let request = request_id
        .as_deref()
        .map(start_request::claim)
        .transpose()?;
    let result = if !tray::check_can_record(&app).await {
        Err("recording_setup_incomplete".into())
    } else if let Err(error) = request.as_ref().map(|claim| claim.check_call()).transpose() {
        Err(error)
    } else {
        start_named_session(
            app,
            mic_device_name,
            system_device_name,
            meeting_name,
            audio::recording_commands::SelectedStart {
                use_default_microphone: use_default_microphone.unwrap_or(false),
                calendar_link: request
                    .as_ref()
                    .and_then(|claim| claim.calendar())
                    .cloned()
                    .or(calendar_link),
                summary_language,
                request: request.as_ref(),
            },
        )
        .await
    };
    if let Some(request) = request {
        request.complete(&result);
    }
    result
}

async fn start_named_session<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
    options: audio::recording_commands::SelectedStart<'_>,
) -> Result<String, String> {
    log_info!("Recording start requested");

    // Clone meeting_name for notification use later
    let meeting_name_for_notification = meeting_name.clone();

    // Call the recording module functions that support meeting names
    let recording_result = audio::recording_commands::start_selected_session(
        app.clone(),
        mic_device_name,
        system_device_name,
        meeting_name,
        options,
    )
    .await;

    match recording_result {
        Ok(session_id) => {
            log_info!("Recording started successfully via tauri command");

            // Notification work must not hold up the command response; capture
            // is already running at this point.
            let app_for_notification = app.clone();
            tauri::async_runtime::spawn(async move {
                let notification_manager_state =
                    app_for_notification.state::<NotificationManagerState<R>>();
                if let Err(e) = notifications::commands::show_recording_started_notification(
                    &app_for_notification,
                    &notification_manager_state,
                    meeting_name_for_notification.clone(),
                )
                .await
                {
                    log_error!("Failed to show recording started notification: {}", e);
                }
            });

            Ok(session_id)
        }
        Err(e) => {
            log_error!("Failed to start recording via tauri command: {}", e);
            Err(e)
        }
    }
}

/// Sets the language the running core uses, without persisting it. Used when
/// the stored settings are applied at start-up, before any window exists.
pub(crate) fn set_language_runtime(language: &str) {
    if let Ok(mut lang_pref) = LANGUAGE_PREFERENCE.lock() {
        *lang_pref = language.to_string();
    }
}

/// The language is owned by the core: a change is stored first, then applied.
#[tauri::command]
async fn set_language_preference(app: AppHandle, language: String) -> Result<(), String> {
    let _change = behavior_settings::CHANGE_LOCK.lock().await;
    apply_language(&app, &language).await
}

/// The change itself, shared by the command and the hand-over of the webview's
/// old value: stored first, then applied, and live transcription starts over
/// for the new language. The caller holds `behavior_settings::CHANGE_LOCK`.
pub(crate) async fn apply_language(app: &AppHandle, language: &str) -> Result<(), String> {
    let language = language.to_string();
    {
        let lang_pref = LANGUAGE_PREFERENCE
            .lock()
            .map_err(|e| format!("Failed to set language preference: {}", e))?;
        if *lang_pref == language {
            return Ok(());
        }
    }
    let stored = app.clone();
    let stored_language = language.clone();
    tauri::async_runtime::spawn_blocking(move || {
        behavior_settings::save_language(&stored, &stored_language)
    })
    .await
    .map_err(|_| "Could not save the language".to_string())??;
    log_info!("Setting language preference to: {}", language);
    set_language_runtime(&language);
    audio::transcription::worker::reset_live_source_languages(&app);
    Ok(())
}

// Internal helper function to get language preference (for use within Rust code)
pub fn get_language_preference_internal() -> Option<String> {
    LANGUAGE_PREFERENCE.lock().ok().map(|lang| lang.clone())
}

#[tauri::command]
fn set_native_theme(app: AppHandle, theme: String, accent: Option<String>) -> Result<(), String> {
    let native_theme = match theme.as_str() {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        "system" => None,
        other => return Err(format!("Unknown native theme preference: {}", other)),
    };

    floating::set_appearance(theme, accent);
    app.set_theme(native_theme);

    if let Some(window) = app.get_webview_window("main") {
        window
            .set_theme(native_theme)
            .map_err(|error| format!("Failed to update native window theme: {}", error))?;
    }

    Ok(())
}

pub fn run() {
    log::set_max_level(log::LevelFilter::Info);

    let mut context = tauri::generate_context!();
    database::recovery::configure(&mut context);
    let mut builder = tauri::Builder::default();

    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            log_info!(
                "Second app instance requested with args: {:?}, cwd: {:?}",
                args,
                cwd
            );

            if let Some(action) = cli_control::parse_cli_action(args.as_slice()) {
                cli_control::run_cli_action(app, action);
            } else {
                tray::focus_main_window(app);
            }
        }));
    }

    builder
        // File + stdout + webview logging. The file lives in the app log dir
        // (Windows: %APPDATA%\net.rismondo.openclaw.clawscribe\logs\ClawScribe.log).
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                // Bounded so logs never clutter the disk: rotate at 5 MB and keep
                // only the single most recent rotated file (so ~10 MB max total).
                .max_file_size(5_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                // targets() REPLACES the plugin's defaults. The previous
                // .target() calls appended to the default stdout + log-dir
                // targets, so every record was written twice.
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: None,
                    }),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Webview),
                ])
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    shortcuts::on_shortcut(app, shortcut, event.state);
                })
                .build(),
        )
        .manage(Arc::new(RwLock::new(
            None::<notifications::manager::NotificationManager<tauri::Wry>>,
        )) as NotificationManagerState<tauri::Wry>)
        .manage(exports::ms_auth_state::MicrosoftAuthState::new())
        .manage(diagnostics::DiagnosticsReportState::default())
        .manage(ical_sources::CalendarStore::default())
        .manage(summary::edit_exit::SummaryEditsState::default())
        .manage(knowledge::KnowledgeState::default())
        .manage(summary::summary_engine::ModelManagerState(Arc::new(
            tokio::sync::Mutex::new(None),
        )))
        .setup(|_app| {
            // Settings that change how the core behaves come from the core's own
            // storage, before anything can start a recording or an import.
            behavior_settings::load_and_apply(_app.handle());
            audio::com_anchor::ensure_device_enumerator();
            audio::transcription::queue::cleanup_abandoned_transcription_spools();

            // The database comes first. If it cannot be opened, nothing else is
            // set up: no tray, no global shortcuts and no engine start-up, so
            // nothing can show the window or start a recording behind the
            // recovery dialog.
            if let Err(error) = tauri::async_runtime::block_on(async {
                database::setup::initialize_database_on_startup(&_app.handle()).await
            }) {
                database::setup::report_startup_failure(&_app.handle(), &error);
                return Ok(());
            }
            log::info!("Application setup complete");
            library::backup_schedule::start(_app.handle().clone());
            #[cfg(windows)]
            windows_lifecycle::install(_app.handle());

            // What an earlier version already had becomes trusted, once, before
            // any window exists and after the database is open: the recordings
            // folders, the folders saved meetings live in, and a custom Codex
            // profile folder.
            tauri::async_runtime::block_on(async {
                audio::recording_preferences::take_over_registered_folders(_app.handle()).await;
                let _ = audio::recording_preferences::recording_roots(_app.handle()).await;
            });
            summary::codex_provider::mark_custom_home_once(_app.handle());

            // Initialize system tray
            if let Err(e) = tray::create_tray(_app.handle()) {
                log::error!("Failed to create system tray: {}", e);
            }
            // Tray icon and tooltip follow the recorder, and the PC is kept
            // awake only while audio is captured. Independent of the tray
            // itself, so the power request works even without one.
            tray::start_lifecycle(_app.handle());

            // Register global recording shortcuts (persisted or defaults).
            shortcuts::init_from_storage(_app.handle());

            // Call detection runs here, on the core's own schedule, so a window
            // hidden in the tray cannot slow it down.
            teams_detection::monitor::spawn(_app.handle().clone());

            // Initialize notification system with proper defaults
            log::info!("Initializing notification system...");
            let app_for_notif = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let notif_state = app_for_notif.state::<NotificationManagerState<tauri::Wry>>();
                match notifications::commands::initialize_notification_manager(
                    app_for_notif.clone(),
                )
                .await
                {
                    Ok(manager) => {
                        // Set default consent and permissions on first launch
                        if let Err(e) = manager.set_consent(true).await {
                            log::error!("Failed to set initial consent: {}", e);
                        }
                        if let Err(e) = manager.request_permission().await {
                            log::error!("Failed to request initial permission: {}", e);
                        }

                        // Store the initialized manager
                        let mut state_lock = notif_state.write().await;
                        *state_lock = Some(manager);
                        log::info!("Notification system initialized with default permissions");
                    }
                    Err(e) => {
                        log::error!("Failed to initialize notification manager: {}", e);
                    }
                }
            });

            // Set models directory to use app_data_dir (unified storage location)
            whisper_engine::commands::set_models_directory(&_app.handle());

            // Initialize Whisper engine on startup
            tauri::async_runtime::spawn(async {
                if let Err(e) = whisper_engine::commands::whisper_init().await {
                    log::error!("Failed to initialize Whisper engine on startup: {}", e);
                }
            });

            // Set Parakeet models directory
            parakeet_engine::commands::set_models_directory(&_app.handle());

            // Initialize Parakeet engine on startup
            tauri::async_runtime::spawn(async {
                if let Err(e) = parakeet_engine::commands::parakeet_init().await {
                    log::error!("Failed to initialize Parakeet engine on startup: {}", e);
                }
            });

            // Initialize ModelManager for summary engine (async, non-blocking)
            let app_handle_for_model_manager = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match summary::summary_engine::commands::init_model_manager_at_startup(
                    &app_handle_for_model_manager,
                )
                .await
                {
                    Ok(_) => log::info!("ModelManager initialized successfully at startup"),
                    Err(e) => {
                        log::warn!("Failed to initialize ModelManager at startup: {}", e);
                        log::warn!("ModelManager will be lazy-initialized on first use");
                    }
                }
            });

            // Trigger system audio permission request on startup (similar to microphone permission)
            // #[cfg(target_os = "macos")]
            // {
            //     tauri::async_runtime::spawn(async {
            //         if let Err(e) = audio::permissions::trigger_system_audio_permission() {
            //             log::warn!("Failed to trigger system audio permission: {}", e);
            //         }
            //     });
            // }

            // Initialize bundled templates directory for dynamic template discovery
            if let Ok(resource_path) = _app.handle().path().resource_dir() {
                let templates_dir = resource_path.join("templates");
                summary::templates::set_bundled_templates_dir(templates_dir);
            } else {
                log::warn!("Failed to resolve resource directory for templates");
            }

            if let Some(state) = _app.try_state::<state::AppState>() {
                let pool = state.db_manager.pool().clone();
                let app = _app.handle().clone();
                tauri::async_runtime::spawn(summary::automatic::resume_at_startup(app, pool));
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Files dropped on the window: the core saw the drop, so it is the
            // user's choice. The audio files are granted for import here, then
            // the window is told; the webview never supplies these paths.
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                if window.label() == "main" {
                    let audio = file_grants::grant_dropped_audio(paths);
                    let _ = window.emit(
                        "files-dropped",
                        serde_json::json!({ "audio": audio, "other": paths.len() - audio.len() }),
                    );
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.emit("summary-flush-on-close", ());
                    if let Err(e) = window.hide() {
                        log::error!("Failed to hide main window on close request: {}", e);
                    } else {
                        log::info!("Main window hidden to tray on close request");
                    }
                }
            }
        })
        .invoke_handler(floating::restrict_commands(tauri::generate_handler![
            floating::open_floating_controls,
            floating::floating_status,
            floating::floating_note,
            floating::floating_stop,
            floating::dismiss_floating_controls,
            library::context::get_meeting_context,
            library::context::set_meeting_context,
            library::calendar::get_meeting_calendar,
            library::calendar::set_meeting_calendar,
            library::calendar::migrate_legacy_meeting_calendars,
            library::calendar::get_meeting_series_template,
            library::calendar::set_meeting_series_template,
            library::recording_calendar::get_active_recording_calendar,
            library::recording_calendar::set_active_recording_calendar,
            library::list_meeting_tags,
            library::set_meeting_tags,
            library::list_meeting_bookmarks,
            library::list_live_recording_bookmarks,
            library::set_live_summary_bookmarks,
            library::pending_recording_notes::list_pending_recording_notes,
            library::pending_recording_notes::delete_pending_recording_notes,
            library::add_meeting_bookmark,
            library::delete_meeting_bookmark,
            library::rename_meeting_bookmark,
            library::summary_bookmarks::get_meeting_summary_bookmarks,
            library::summary_bookmarks::set_meeting_summary_bookmarks,
            library::export_local_word,
            library::backup::backup_library,
            library::backup::restore_library,
            library::backup_schedule::scheduled_backup_status,
            library::backup_schedule::choose_scheduled_backup_folder,
            library::backup_schedule::set_scheduled_backups,
            database::recovery::startup_recovery_active,
            database::recovery::return_to_original_library,
            library::archive::list_archive_rows,
            library::archive::storage::measure_archive_storage,
            library::file_cleanup::list_pending_meeting_file_cleanup,
            library::file_cleanup::retry_meeting_file_cleanup,
            library::file_cleanup::keep_pending_meeting_files,
            library::archive::storage::inspect_archive_audio,
            library::archive::storage::remove_archive_audio,
            updates::get_update_channel,
            updates::download_app_update,
            updates::install_app_update,
            updates::discard_app_update,
            updates::set_update_channel,
            updates::check_app_update,
            start_recording,
            stop_recording,
            is_recording,
            set_native_theme,
            get_transcription_status,
            audio::outcome::get_recording_outcome,
            resolve_meeting_audio_file,
            whisper_engine::commands::whisper_init,
            whisper_engine::commands::whisper_get_acceleration_status,
            whisper_engine::commands::whisper_get_available_models,
            whisper_engine::commands::whisper_load_model,
            whisper_engine::commands::whisper_get_current_model,
            whisper_engine::commands::whisper_is_model_loaded,
            whisper_engine::commands::whisper_has_available_models,
            whisper_engine::commands::whisper_validate_model_ready,
            whisper_engine::commands::whisper_transcribe_audio,
            whisper_engine::commands::whisper_get_models_directory,
            whisper_engine::commands::whisper_download_model,
            whisper_engine::commands::whisper_cancel_download,
            whisper_engine::commands::whisper_delete_corrupted_model,
            // Parakeet engine commands
            parakeet_engine::commands::parakeet_init,
            parakeet_engine::commands::parakeet_get_available_models,
            parakeet_engine::commands::parakeet_load_model,
            parakeet_engine::commands::parakeet_get_current_model,
            parakeet_engine::commands::parakeet_is_model_loaded,
            parakeet_engine::commands::parakeet_has_available_models,
            parakeet_engine::commands::parakeet_validate_model_ready,
            parakeet_engine::commands::parakeet_transcribe_audio,
            parakeet_engine::commands::parakeet_get_models_directory,
            parakeet_engine::commands::parakeet_download_model,
            parakeet_engine::commands::parakeet_retry_download,
            parakeet_engine::commands::parakeet_cancel_download,
            parakeet_engine::commands::parakeet_delete_corrupted_model,
            parakeet_engine::commands::open_parakeet_models_folder,
            parakeet_engine::commands::set_parakeet_use_directml,
            parakeet_engine::commands::get_parakeet_use_directml,
            parakeet_engine::commands::get_parakeet_directml_session_disabled,
            parakeet_engine::commands::set_parakeet_use_worker,
            parakeet_engine::commands::get_parakeet_use_worker,
            // Nemotron (streaming RNN-T, Beta) commands
            nemotron_engine::commands::nemotron_init,
            nemotron_engine::commands::nemotron_get_available_models,
            nemotron_engine::commands::nemotron_download_model,
            nemotron_engine::commands::nemotron_cancel_download,
            // Parallel processing commands
            get_audio_devices,
            get_recording_device_inventory,
            audio::transcription::engine::get_live_transcription_readiness,
            trigger_microphone_permission,
            audio::recording_commands::stop_detected_call_recording,
            audio::transcription::worker::get_live_source_languages,
            start_recording_with_devices_and_meeting,
            start_audio_level_monitoring,
            stop_audio_level_monitoring,
            // Global keyboard shortcuts
            shortcuts::get_shortcuts,
            shortcuts::set_shortcuts,
            shortcuts::set_shortcut_capture,
            shortcuts::shortcut_key_characters,
            // Recording pause/resume commands
            audio::recording_commands::pause_recording,
            audio::recording_commands::resume_recording,
            audio::recording_commands::get_recording_state,
            audio::recording_commands::get_meeting_folder_path,
            // Source attribution (Beta, opt-in): Me/Participants labelling
            audio::recording_commands::set_source_attribution_enabled,
            audio::recording_commands::set_cloud_transcription_enabled,
            // Reload sync commands (retrieve transcript history and meeting name)
            audio::recording_commands::get_transcript_history,
            audio::recording_commands::get_recording_meeting_name,
            // Device monitoring commands (AirPods/Bluetooth disconnect/reconnect)
            audio::recording_commands::poll_audio_device_events,
            audio::recording_commands::get_reconnection_status,
            audio::recording_commands::attempt_device_reconnect,
            // Playback device detection (Bluetooth warning)
            // OpenClaw handoff commands
            diagnostics::get_diagnostics_snapshot,
            diagnostics::save_diagnostics_report,
            audio::pc_test::run_transcription_pc_test,
            mcp::settings::get_mcp_access,
            mcp::settings::set_mcp_access,
            openclaw::get_openclaw_config_status,
            openclaw::save_openclaw_config,
            openclaw::clear_openclaw_bearer_token,
            // Teams meeting detection commands
            teams_detection::get_teams_detection_config,
            teams_detection::get_teams_detection_status,
            teams_detection::calls::get_call_detection_status,
            teams_detection::monitor::get_call_detection_settings,
            teams_detection::monitor::set_call_detection_settings,
            teams_detection::monitor::get_pending_call_prompt,
            teams_detection::monitor::confirm_call_detection_prompt,
            teams_detection::monitor::join::join_and_record,
            teams_detection::monitor::join::cancel_join_recording,
            teams_detection::monitor::join::get_join_recording,
            start_request::finish_recording_start,
            start_request::get_pending_start_request,
            audio::recording_commands::queue_recording_after_save,
            // Audio recovery commands (for transcript recovery feature)
            audio::incremental_saver::recover_audio_from_checkpoints,
            audio::incremental_saver::cleanup_checkpoints,
            audio::incremental_saver::release_recovered_capture,
            audio::incremental_saver::has_audio_checkpoints,
            file_grants::pick_transcription_test_audio,
            file_grants::pick_reference_document,
            file_grants::pick_backup_save_path,
            file_grants::pick_backup_restore_path,
            file_grants::pick_word_save_path,
            audio::interrupted::find_interrupted_recordings,
            audio::interrupted::preview_interrupted_recording,
            audio::interrupted::discard_interrupted_recording,
            audio::interrupted::keep_interrupted_recording,
            audio::interrupted::repair_saved_recording,
            ollama::get_ollama_models,
            ollama::pull_ollama_model,
            openai::auth::api_get_openai_auth_status,
            openai::auth::api_save_openai_auth_config,
            openai::auth::api_clear_openai_auth_config,
            openai::openai::get_openai_models,
            summary::codex_provider::codex_get_config,
            summary::claude_code_provider::claude_code_get_config,
            summary::claude_code_provider::claude_code_save_config,
            summary::claude_code_provider::claude_code_check_status,
            summary::codex_provider::codex_save_config,
            summary::codex_provider::pick_codex_home_folder,
            summary::codex_provider::codex_check_installation,
            summary::codex_provider::codex_list_models,
            summary::codex_provider::codex_find_automatically,
            summary::codex_provider::codex_prepare_install_command,
            summary::codex_provider::codex_login_browser,
            summary::codex_provider::codex_login_device,
            summary::codex_provider::codex_logout,
            summary::codex_provider::codex_test_app_server,
            summary::codex_provider::codex_test_processing,
            anthropic::anthropic::get_anthropic_models,
            groq::groq::get_groq_models,
            api::api_get_meetings,
            api::api_search_transcripts,
            api::api_search_library,
            knowledge::store::saved_search::search_saved_meetings,
            knowledge::store::saved_search::resolve_saved_search_hit,
            api::api_get_model_config,
            api::api_save_model_config,
            api::api_get_api_key_status,
            api::api_clear_api_key,
            api::api_get_transcript_config,
            api::api_save_transcript_config,
            api::api_get_transcript_api_key_status,
            api::api_clear_transcript_api_key,
            api::api_test_transcript_provider,
            api::api_delete_meeting,
            api::api_get_meeting,
            api::api_get_meeting_metadata,
            api::api_get_meeting_transcripts,
            api::api_update_transcript_speaker,
            api::api_update_transcript_speakers_matching,
            api::api_save_meeting_title,
            api::api_save_transcript,
            api::open_meeting_folder,
            api::open_external_url,
            // Custom OpenAI commands
            api::api_save_custom_openai_config,
            api::api_get_custom_openai_config,
            api::api_clear_custom_openai_api_key,
            api::api_test_custom_openai_connection,
            api::api_test_custom_openai_processing,
            // Summary commands
            summary::commands::api_process_transcript,
            summary::commands::api_generate_meeting_summary,
            summary::automatic::get_automatic_summary_preference,
            summary::automatic::migrate_automatic_summary_preference,
            summary::automatic::get_automatic_summary_destination,
            summary::automatic::set_automatic_summary_preference,
            summary::automatic::start_recording_summary,
            summary::commands::polish_planner_tasks,
            // Meeting chat (transcript-grounded)
            summary::commands::api_get_summary,
            summary::commands::api_save_meeting_summary,
            summary::commands::api_restore_previous_summary,
            summary::edit_exit::api_set_summary_edits_pending,
            summary::edit_exit::api_finish_summary_edit_exit,
            summary::edit_exit::api_summary_edit_exit_failed,
            summary::commands::api_get_meeting_summary_language,
            summary::commands::api_save_meeting_summary_language,
            summary::commands::api_get_meeting_detected_summary_language,
            summary::commands::api_save_meeting_detected_summary_language,
            summary::commands::api_detect_transcript_summary_language,
            summary::commands::api_cancel_summary,
            // Template commands
            summary::template_commands::api_list_templates,
            summary::template_commands::api_get_default_template,
            summary::template_commands::api_save_default_template,
            summary::template_commands::api_get_template_editor,
            summary::template_commands::api_save_template,
            summary::template_commands::api_remove_template,
            audio::recording_mode::get_recording_mode,
            audio::recording_mode::set_recording_mode,
            database::transcript_edits::api_update_transcript_text,
            database::transcript_edits::api_preview_transcript_replace,
            database::transcript_edits::api_replace_transcript_text,
            database::transcript_edits::api_undo_transcript_edit,
            database::transcript_edits::api_restore_previous_transcript,
            database::transcript_edits::api_sync_transcript_file,
            summary::sources::api_get_summary_sources,
            summary::sources::api_resolve_summary_source,
            database::transcript_edits::api_get_transcript_edit_state,
            // Built-in AI commands
            summary::summary_engine::commands::builtin_ai_list_models,
            summary::summary_engine::commands::builtin_ai_get_model_info,
            summary::summary_engine::commands::builtin_ai_download_model,
            summary::summary_engine::commands::builtin_ai_cancel_download,
            summary::summary_engine::commands::builtin_ai_delete_model,
            summary::summary_engine::commands::builtin_ai_is_model_ready,
            summary::summary_engine::commands::builtin_ai_get_available_summary_model,
            summary::summary_engine::commands::builtin_ai_get_recommended_model,
            openrouter::get_openrouter_models,
            audio::recording_preferences::get_recording_preferences,
            audio::recording_preferences::set_recording_preferences,
            audio::recording_preferences::set_recording_device_preferences,
            audio::vocabulary_settings::get_custom_vocabulary,
            audio::vocabulary_settings::set_custom_vocabulary,
            audio::recording_preferences::get_default_recordings_folder_path,
            audio::recording_preferences::open_recordings_folder,
            audio::recording_preferences::select_recording_folder,
            audio::recording_preferences::get_current_audio_backend,
            audio::recording_preferences::set_audio_backend,
            audio::recording_preferences::get_audio_backend_info,
            // Language preference commands
            set_language_preference,
            behavior_settings::get_behavior_settings,
            behavior_settings::migrate_legacy_behavior_settings,
            behavior_settings::set_recording_notification_preference,
            // Notification system commands
            notifications::commands::get_notification_settings,
            notifications::commands::set_notification_settings,
            // System audio capture commands
            // Screen Recording permission commands
            audio::permissions::check_screen_recording_permission_command,
            audio::permissions::request_screen_recording_permission_command,
            audio::permissions::trigger_system_audio_permission_command,
            // Database import commands
            database::commands::check_first_launch,
            database::commands::import_legacy_database_if_present,
            knowledge::indexer::knowledge_search,
            knowledge::commands::knowledge_ask,
            knowledge::note_review::knowledge_preview_notes,
            knowledge::note_review::knowledge_discard_note_review,
            knowledge::note_review::knowledge_ask_with_notes,
            knowledge::commands::knowledge_live_snapshot,
            knowledge::commands::knowledge_live_sharing,
            knowledge::commands::knowledge_set_live_sharing,
            knowledge::commands::knowledge_import_document,
            knowledge::commands::knowledge_cancel_document_import,
            knowledge::commands::knowledge_list_documents,
            knowledge::commands::knowledge_list_scope_documents,
            knowledge::commands::knowledge_document_sharing,
            knowledge::commands::knowledge_set_document_sharing,
            knowledge::commands::knowledge_retry_document_index,
            knowledge::commands::knowledge_get_document_blocks,
            knowledge::commands::knowledge_remove_attachment,
            knowledge::commands::knowledge_delete_document,
            knowledge::commands::knowledge_cancel_request,
            knowledge::commands::knowledge_history,
            knowledge::commands::knowledge_resolve_evidence,
            knowledge::commands::knowledge_create_library_conversation,
            knowledge::commands::knowledge_list_library_conversations,
            knowledge::commands::knowledge_library_conversation_scope,
            knowledge::commands::knowledge_clear_history,
            knowledge::indexer::knowledge_index_status,
            knowledge::indexer::knowledge_reindex,
            knowledge::indexer::knowledge_cancel_index,
            knowledge::indexer::knowledge_model_enable,
            knowledge::indexer::knowledge_model_status,
            knowledge::indexer::knowledge_model_download,
            knowledge::indexer::knowledge_model_cancel_download,
            database::commands::initialize_fresh_database,
            // Database and Models path commands
            database::commands::get_database_directory,
            database::commands::open_database_folder,
            whisper_engine::commands::open_models_folder,
            // Onboarding commands
            onboarding::get_onboarding_status,
            onboarding::save_onboarding_status_cmd,
            onboarding::complete_onboarding,
            // System settings commands
            #[cfg(target_os = "macos")]
            utils::open_system_settings,
            // Retranscription commands
            audio::retranscription::start_retranscription_command,
            audio::retranscription::cancel_retranscription_command,
            // Speaker diarization commands
            audio::diarization::start_speaker_diarization_command,
            audio::diarization::active_speaker_diarization_command,
            audio::diarization::cancel_speaker_diarization_command,
            // Import audio commands
            audio::import::select_and_validate_audio_command,
            audio::import::select_audio_files_command,
            audio::import::validate_audio_file_command,
            audio::import::start_import_audio_command,
            audio::import::cancel_import_command,
            // Microsoft Graph export commands
            exports::commands::microsoft_sign_in,
            exports::commands::microsoft_cancel_sign_in,
            exports::commands::microsoft_sign_out,
            exports::commands::microsoft_connection_status,
            exports::commands::export_to_onenote,
            exports::commands::export_to_planner,
            exports::commands::export_meeting_markdown_to_onenote,
            exports::commands::export_meeting_to_onenote_section,
            exports::commands::list_onenote_resumes,
            exports::commands::get_meeting_export_history,
            exports::commands::reconcile_meeting_export,
            exports::commands::export_meeting_markdown_to_planner,
            exports::commands::summary_has_action_items,
            exports::commands::list_onenote_notebooks,
            exports::commands::list_planner_plans,
            exports::commands::list_planner_buckets,
            exports::commands::list_todo_lists,
            exports::commands::create_onenote_notebook,
            exports::commands::create_planner_bucket,
            exports::commands::create_todo_list,
            exports::commands::list_onedrive_destinations,
            exports::commands::resolve_onedrive_destination_url,
            exports::commands::create_onedrive_destination_folder,
            exports::commands::export_meeting_to_onedrive_files,
            exports::commands::list_calendar_events,
            ical_sources::list_ical_sources,
            ical_sources::add_ical_file,
            ical_sources::add_ical_subscription,
            ical_sources::read_ical_source,
            ical_sources::remove_ical_source,
            exports::commands::current_or_next_meeting,
            exports::commands::preview_planner_tasks,
            exports::actions::get_reviewed_meeting_actions,
            exports::actions::suggest_meeting_actions,
            exports::actions::resolve_reviewed_action_source,
            exports::actions::save_reviewed_meeting_actions,
            exports::commands::export_selected_planner_tasks,
            exports::commands::preview_todo_tasks,
            exports::commands::export_selected_todo_tasks,
            exports::confluence::confluence_save_pat,
            exports::confluence::confluence_clear_pat,
            exports::confluence::confluence_settings_status,
            exports::confluence::confluence_connection_status,
            exports::confluence::confluence_export_page,
        ]))
        .build(context)
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            match event {
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    tray::focus_main_window(_app_handle);
                }
                tauri::RunEvent::ExitRequested { code, api, .. } => {
                    // Unsaved summary edits come first: the user confirms (or
                    // keeps the app open) before an active recording is
                    // stopped. Once that exit is allowed, a recording is
                    // stopped and saved before the exit proceeds.
                    if summary::edit_exit::request_exit(_app_handle, code.unwrap_or(0), || {
                        api.prevent_exit()
                    }) {
                        exit_guard::exit_deferred_elsewhere();
                        tray::focus_main_window(_app_handle);
                        return;
                    }
                    exit_guard::handle_exit_request(_app_handle, code, || api.prevent_exit());
                }
                tauri::RunEvent::Exit => {
                    #[cfg(windows)]
                    if windows_lifecycle::session_ending() {
                        // Tao can dispatch Exit synchronously from WM_ENDSESSION.
                        // Windows will release the wake lease with the process.
                        return;
                    }
                    log::info!("Application exiting, cleaning up resources...");
                    power::release();
                    tauri::async_runtime::block_on(async {
                        // Clean up database connection and checkpoint WAL
                        if let Some(app_state) = _app_handle.try_state::<state::AppState>() {
                            log::info!("Starting database cleanup...");
                            if let Err(e) = app_state.db_manager.cleanup().await {
                                log::error!("Failed to cleanup database: {}", e);
                            } else {
                                log::info!("Database cleanup completed successfully");
                            }
                        } else {
                            log::warn!(
                                "AppState not available for database cleanup (likely first launch)"
                            );
                        }

                        // Clean up sidecar
                        log::info!("Cleaning up sidecar...");
                        if let Err(e) = summary::summary_engine::force_shutdown_sidecar().await {
                            log::error!("Failed to force shutdown sidecar: {}", e);
                        }
                    });
                    log::info!("Application cleanup complete");
                }
                _ => {}
            }
        });
}
