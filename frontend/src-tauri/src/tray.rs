use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Runtime,
};

use crate::start_request::{StartOutcome, StartSource};
use crate::teams_detection::monitor::PromptView;

const TRAY_ID: &str = "main-tray";
const APP_NAME: &str = "ClawScribe";
/// Windows keeps 127 characters of a tray tooltip.
const TOOLTIP_LIMIT: usize = 120;
const TITLE_LIMIT: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Stopped,
    Starting,
    Recording,
    Pausing,
    Paused,
    Resuming,
    Stopping,
}

pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // Start with default menu, will update with actual state after initialization
    // Pass can_record=true initially, will be updated by update_tray_menu immediately
    let menu = build_menu(app, RecordingState::Stopped, true)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(APP_NAME)
        .icon(app.default_window_icon().unwrap().clone())
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
        .build(app)?;

    // Update tray menu with actual recording state after creation
    update_tray_menu(app);

    Ok(())
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, item_id: &str) {
    match item_id {
        "toggle_recording" => recording_intent_handler(app, Some(true), StartSource::Tray),
        "start_after_save" => {
            handle_saving_intent(app, Some(true));
        }
        "pause_recording" => pause_recording_handler(app),
        "resume_recording" => resume_recording_handler(app),
        "stop_recording" => stop_recording_handler(app),
        "floating_controls" => {
            use crate::audio::recording_commands::floating;
            if let Some(id) = floating::available_session(app) {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(message) = floating::open_floating_controls(app.clone(), id).await {
                        start_feedback(&app, &message);
                    }
                });
            }
        }
        "bookmark_recording" => crate::library::bookmark_active_recording(app),
        item if item.starts_with("confirm_call:") => {
            // Answers the call prompt without opening the window.
            let app = app.clone();
            let key = item["confirm_call:".len()..].to_owned();
            tauri::async_runtime::spawn(async move {
                crate::teams_detection::monitor::confirm_prompt(&app, &key).await;
            });
        }
        "open_window" => focus_main_window(app),
        "settings" => {
            focus_main_window(app);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval("window.location.assign('/settings')");
            }
        }
        "check_updates" => check_updates_handler(app),
        // `exit` raises `RunEvent::ExitRequested`; `exit_guard` stops and saves
        // an active recording before this exit goes through.
        "quit" => app.exit(if crate::database::setup::startup_failed() {
            1
        } else {
            0
        }),
        _ => {}
    }
}

pub(crate) fn toggle_recording_handler<R: Runtime>(app: &AppHandle<R>, source: StartSource) {
    recording_intent_handler(app, None, source);
}

/// Starts or stops recording. `want_recording` of `None` toggles; `Some`
/// does nothing when the recording state already matches. A start that is
/// already under way counts as recording, so a repeated request never starts
/// twice. The state is read once, in the same task that acts on it, so a start
/// request can never stop a recording that began in the meantime, nor the
/// reverse.
///
/// A start is asked of the window in the background: the page the user is on
/// and its drafts stay as they are, and the window is not brought forward.
pub(crate) fn recording_intent_handler<R: Runtime>(
    app: &AppHandle<R>,
    want_recording: Option<bool>,
    source: StartSource,
) {
    if handle_saving_intent(app, want_recording) {
        return;
    }
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if handle_saving_intent(&app_clone, want_recording) {
            return;
        }
        let recording = crate::is_recording().await;
        let starting = crate::start_request::is_pending();
        if want_recording == Some(recording || starting) {
            log::info!("Recording request ignored: state already matches");
            return;
        }
        if recording {
            stop_active_recording(&app_clone, "Tray toggle").await;
        } else if starting {
            log::info!("Recording request ignored: a start is already under way");
        } else {
            request_start(&app_clone, source).await;
        }
    });
}

fn handle_saving_intent<R: Runtime>(app: &AppHandle<R>, intent: Option<bool>) -> bool {
    if !crate::audio::recording_commands::set_start_after_save(intent, None) {
        return false;
    }
    let message = match intent {
        Some(true) => {
            "New recording queued for after this save. The request expires in one minute."
        }
        Some(false) => "Saving this recording. No new recording is queued.",
        None => "Saving this recording. Use Start after saving in the tray to record again.",
    };
    start_feedback(app, message);
    true
}

fn start_feedback<R: Runtime>(app: &AppHandle<R>, message: &str) {
    let _ = app.emit("recording-start-feedback", message);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(message));
    }
}

pub(crate) fn queued_start_failed<R: Runtime>(app: &AppHandle<R>) {
    start_feedback(app, "The queued recording did not start. Check the recording status and start again when ready.");
    let _ = app.emit("recording-queued-start-failed", ());
}

pub(crate) async fn start_after_save<R: Runtime>(
    app: &AppHandle<R>,
    epoch: u64,
    requested: std::time::Instant,
) {
    request_start(app, StartSource::AfterSave { epoch, requested }).await;
}

/// Asks the window to start a recording and shows the outcome in the tray.
async fn request_start<R: Runtime>(app: &AppHandle<R>, source: StartSource) {
    match crate::start_request::request_and_wait(app, source, None).await {
        None => {
            if matches!(source, StartSource::AfterSave { .. }) {
                queued_start_failed(app);
            } else {
                handle_saving_intent(app, None);
            }
            log::info!(
                "Recording request ignored: another start is under way or capture is running"
            );
            update_tray_menu_async(app).await;
        }
        Some(StartOutcome::Started { .. }) => {}
        Some(StartOutcome::Busy) => {
            if matches!(source, StartSource::AfterSave { .. }) {
                queued_start_failed(app);
            } else {
                start_feedback(
                    app,
                    "Recording is busy. Check its status before starting again.",
                );
            }
            log::info!("Recording request ignored: the window is busy with another recording step");
            update_tray_menu_async(app).await;
        }
        Some(StartOutcome::Failed { .. }) | Some(StartOutcome::TimedOut) => {
            if matches!(source, StartSource::AfterSave { .. }) {
                queued_start_failed(app);
            }
            log::warn!("A recording start requested from the tray did not complete");
            update_tray_menu_async(app).await;
            // The window explains why; bring it forward so the reason is seen.
            focus_main_window(app);
        }
    }
}

/// Stops and saves through the same native command as the Stop button.
async fn stop_active_recording<R: Runtime>(app: &AppHandle<R>, origin: &str) {
    // Immediately show the stopping state.
    set_tray_state(app, RecordingState::Stopping);
    log::info!("{origin}: stopping recording...");

    // Generate save path (same as RecordingControls.tsx)
    let data_dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(e) => {
            log::error!("Failed to get app data dir: {}", e);
            update_tray_menu_async(app).await;
            return;
        }
    };
    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let save_path = data_dir.join(format!("recording-{}.wav", timestamp));

    // Call Rust stop_recording command (like pause/resume pattern)
    let stop_result = crate::audio::recording_commands::stop_recording(
        app.clone(),
        crate::audio::recording_commands::RecordingArgs {
            save_path: save_path.to_string_lossy().to_string(),
        },
    )
    .await;

    match stop_result {
        Ok(false) => {}
        Ok(true) => {
            log::info!("{origin}: recording stopped successfully");
        }
        Err(e) => {
            log::error!("{origin}: failed to stop recording: {e}");
            report_stop_failure(app, &e);
            // Revert tray state on error
            update_tray_menu_async(app).await;
            focus_main_window(app);
        }
    }
}

/// Tells the window that a Stop from the tray (or a shortcut) failed and the
/// recording is still running, so it can say why instead of staying silent.
fn report_stop_failure<R: Runtime>(app: &AppHandle<R>, message: &str) {
    let _ = app.emit(
        "recording-stop-failed",
        serde_json::json!({ "message": message }),
    );
}

pub(crate) fn pause_recording_handler<R: Runtime>(app: &AppHandle<R>) {
    // Immediately show pausing state
    set_tray_state(app, RecordingState::Pausing);

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::audio::recording_commands::pause_recording(app_clone.clone()).await {
            log::error!("Failed to pause recording from tray: {}", e);
            // Revert to current state on error
            update_tray_menu_async(&app_clone).await;
        } else {
            log::info!("Recording paused from tray");
            // The pause_recording function will call update_tray_menu, so no need to call it here
        }
    });
}

pub(crate) fn resume_recording_handler<R: Runtime>(app: &AppHandle<R>) {
    // Immediately show resuming state
    set_tray_state(app, RecordingState::Resuming);

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::audio::recording_commands::resume_recording(app_clone.clone()).await
        {
            log::error!("Failed to resume recording from tray: {}", e);
            // Revert to current state on error
            update_tray_menu_async(&app_clone).await;
        } else {
            log::info!("Recording resumed from tray");
            // The resume_recording function will call update_tray_menu, so no need to call it here
        }
    });
}

fn stop_recording_handler<R: Runtime>(app: &AppHandle<R>) {
    if crate::audio::recording_commands::is_stopping() {
        return;
    }
    // Immediately show stopping state
    set_tray_state(app, RecordingState::Stopping);

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        stop_active_recording(&app_clone, "Tray").await;
    });
}

fn check_updates_handler<R: Runtime>(app: &AppHandle<R>) {
    focus_main_window(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("window.dispatchEvent(new CustomEvent('check-updates-from-tray'))");
    }
}

pub fn update_tray_menu<R: Runtime>(app: &AppHandle<R>) {
    // For sync update, spawn async task to get current state
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        // Small delay to ensure recording state has been updated
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        update_tray_menu_async(&app_clone).await;
    });
}

pub fn set_tray_state<R: Runtime>(app: &AppHandle<R>, state: RecordingState) {
    log::info!("Tray: Setting intermediate state: {:?}", state);
    // During recording state transitions, we assume recording is allowed (we're already recording)
    if let Ok(menu) = build_menu(app, state, true) {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let result = tray.set_menu(Some(menu));
            log::info!("Tray: Intermediate state menu update result: {:?}", result);
        } else {
            log::warn!("Tray: Could not find tray with id 'main-tray'");
        }
    } else {
        log::error!("Tray: Failed to build menu for intermediate state");
    }
}

/// The recorder's own state, read without logging: the lifecycle loop calls it
/// every second. A start the core has asked the window for counts as starting
/// until the recording is running or the request has ended.
async fn get_current_recording_state() -> RecordingState {
    if crate::audio::recording_commands::is_stopping() {
        return RecordingState::Stopping;
    }
    if !crate::audio::recording_commands::is_recording().await {
        return if crate::start_request::is_pending()
            || crate::audio::recording_commands::is_starting()
        {
            RecordingState::Starting
        } else {
            RecordingState::Stopped
        };
    }
    if crate::audio::recording_commands::is_recording_paused().await {
        RecordingState::Paused
    } else {
        RecordingState::Recording
    }
}

pub(crate) async fn check_can_record<R: Runtime>(app: &AppHandle<R>) -> bool {
    crate::onboarding::is_complete(app)
}

pub async fn update_tray_menu_async<R: Runtime>(app: &AppHandle<R>) {
    // Get the current recording state
    let recording_state = get_current_recording_state().await;
    log::info!("Tray: Current recording state: {:?}", recording_state);

    let can_record = check_can_record(app).await;

    if let Ok(menu) = build_menu(app, recording_state, can_record) {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let result = tray.set_menu(Some(menu));
            log::info!("Tray: Menu update result: {:?}", result);
        } else {
            log::warn!("Tray: Could not find tray with id 'main-tray'");
        }
    } else {
        log::error!("Tray: Failed to build menu");
    }
}

fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    state: RecordingState,
    can_record: bool,
) -> tauri::Result<tauri::menu::Menu<R>> {
    let mut builder = MenuBuilder::new(app);

    if !can_record && matches!(state, RecordingState::Stopped) {
        builder = builder.item(
            &MenuItemBuilder::new("Complete setup to record")
                .enabled(false)
                .build(app)?,
        );
    } else {
        match state {
            RecordingState::Stopped => {
                builder = builder.item(
                    &MenuItemBuilder::with_id("toggle_recording", "Start Recording").build(app)?,
                );
                // A detected call waits for an answer: it can be given here
                // while the window is hidden.
                if let Some(prompt) = crate::teams_detection::monitor::pending_prompt() {
                    builder = builder.item(
                        &MenuItemBuilder::with_id(
                            format!("confirm_call:{}", prompt.key),
                            prompt.event_title.as_ref().map_or_else(
                                || format!("Record this {} call", prompt.provider.label()),
                                |title| {
                                    format!(
                                        "Record this Teams call as {}",
                                        clean_title(title).unwrap_or_default().replace('&', "&&")
                                    )
                                },
                            ),
                        )
                        .build(app)?,
                    );
                }
            }
            RecordingState::Starting => {
                builder = builder.item(
                    &MenuItemBuilder::new("🔄 Starting Recording...")
                        .enabled(false)
                        .build(app)?,
                );
            }
            RecordingState::Recording => {
                builder = builder
                    .item(
                        &MenuItemBuilder::with_id("bookmark_recording", "Bookmark this moment")
                            .build(app)?,
                    )
                    .item(
                        &MenuItemBuilder::with_id("pause_recording", "⏸ Pause Recording")
                            .build(app)?,
                    )
                    .item(
                        &MenuItemBuilder::with_id("stop_recording", "⏹ Stop Recording")
                            .build(app)?,
                    );
            }
            RecordingState::Pausing => {
                builder = builder
                    .item(
                        &MenuItemBuilder::new("⏸ Pausing...")
                            .enabled(false)
                            .build(app)?,
                    )
                    .item(
                        &MenuItemBuilder::with_id("stop_recording", "⏹ Stop Recording")
                            .build(app)?,
                    );
            }
            RecordingState::Paused => {
                builder = builder
                    .item(
                        &MenuItemBuilder::with_id("bookmark_recording", "Bookmark this moment")
                            .build(app)?,
                    )
                    .item(
                        &MenuItemBuilder::with_id("resume_recording", "▶ Resume Recording")
                            .build(app)?,
                    )
                    .item(
                        &MenuItemBuilder::with_id("stop_recording", "⏹ Stop Recording")
                            .build(app)?,
                    );
            }
            RecordingState::Resuming => {
                builder = builder
                    .item(
                        &MenuItemBuilder::new("▶ Resuming...")
                            .enabled(false)
                            .build(app)?,
                    )
                    .item(
                        &MenuItemBuilder::with_id("stop_recording", "⏹ Stop Recording")
                            .build(app)?,
                    );
            }
            RecordingState::Stopping => {
                builder = builder.item(
                    &MenuItemBuilder::with_id("start_after_save", "Start after saving")
                        .build(app)?,
                );
            }
        }
    }

    if matches!(
        state,
        RecordingState::Recording | RecordingState::Paused | RecordingState::Stopping
    ) {
        builder = builder.item(
            &MenuItemBuilder::with_id("floating_controls", "Open floating controls").build(app)?,
        );
    }

    builder
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItemBuilder::with_id("open_window", "Open Main Window").build(app)?)
        .item(&MenuItemBuilder::with_id("settings", "Settings").build(app)?)
        .item(&MenuItemBuilder::with_id("check_updates", "Check for Updates").build(app)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItemBuilder::with_id("quit", "Quit").build(app)?)
        .build()
}

pub(crate) fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    // After a database failure the window stays hidden behind the error dialog.
    if crate::database::setup::startup_failed() {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.unminimize() {
            log::error!("Failed to unminimize main window: {}", e);
        }

        if let Err(e) = window.show() {
            log::error!("Failed to show main window: {}", e);
        }

        if let Err(e) = window.set_focus() {
            log::error!("Failed to focus main window: {}", e);
        }

        if let Err(e) = window.eval("window.focus()") {
            log::error!("Failed to focus main webview: {}", e);
        }
    } else {
        log::warn!("Could not find main window");
    }
}

// ── Recording lifecycle: tray icon, tooltip and the power request ─────────

static LIFECYCLE_STARTED: AtomicBool = AtomicBool::new(false);

/// What the tray shows, read from the recorder.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrayView {
    state: RecordingState,
    /// Recording time without pauses, in whole minutes.
    minutes: Option<u64>,
    title: Option<String>,
    prompt: Option<PromptView>,
}

/// Starts the loop that keeps the tray icon, its tooltip and the power request
/// in line with the recorder, whatever changed its state. It only watches state
/// changes, so it never fights the transitional menus the handlers show.
pub fn start_lifecycle<R: Runtime>(app: &AppHandle<R>) {
    if LIFECYCLE_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::notifications::commands::listen_for_background_alerts(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        lifecycle_loop(app).await;
    });
}

async fn lifecycle_loop<R: Runtime>(app: AppHandle<R>) {
    let mut shown: Option<TrayView> = None;
    loop {
        // Each reading runs as a task of its own: if it panics (a poisoned
        // recorder lock, say) the loop survives, and the power request is let
        // go because the state is no longer known.
        let view = match tauri::async_runtime::spawn(read_view()).await {
            Ok(view) => view,
            Err(error) => {
                log::error!("Tray: could not read the recording state: {error}");
                crate::power::reconcile(false);
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        crate::power::reconcile(crate::power::needs_wake(
            matches!(
                view.state,
                RecordingState::Recording | RecordingState::Pausing | RecordingState::Resuming
            ),
            view.state == RecordingState::Paused,
            view.state == RecordingState::Stopping,
        ));
        if shown.as_ref() != Some(&view) {
            let state_changed = shown.as_ref().map_or(true, |old| old.state != view.state);
            let prompt_changed = shown
                .as_ref()
                .map_or(false, |old| old.prompt != view.prompt);
            show_view(&app, &view, state_changed);
            if state_changed || prompt_changed {
                let app_for_menu = app.clone();
                let _ = tauri::async_runtime::spawn(async move {
                    update_tray_menu_async(&app_for_menu).await;
                })
                .await;
            }
            shown = Some(view);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn read_view() -> TrayView {
    let state = get_current_recording_state().await;
    let live = matches!(state, RecordingState::Recording | RecordingState::Paused);
    let (minutes, title) = if live {
        let snapshot = crate::audio::recording_commands::get_recording_state().await;
        let seconds = snapshot
            .get("active_duration")
            .and_then(|value| value.as_f64());
        let title = crate::audio::recording_commands::get_recording_meeting_name()
            .await
            .ok()
            .flatten();
        (
            seconds.map(|seconds| (seconds.max(0.0) / 60.0) as u64),
            title,
        )
    } else {
        (None, None)
    };
    TrayView {
        state,
        minutes,
        title,
        prompt: crate::teams_detection::monitor::pending_prompt(),
    }
}

fn show_view<R: Runtime>(app: &AppHandle<R>, view: &TrayView, state_changed: bool) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Err(e) = tray.set_tooltip(Some(tooltip_text(view))) {
        log::warn!("Tray: could not set the tooltip: {e}");
    }
    if state_changed {
        if let Some(icon) = icon_for(app, view.state) {
            if let Err(e) = tray.set_icon(Some(icon)) {
                log::warn!("Tray: could not set the icon: {e}");
            }
        }
    }
}

fn icon_for<R: Runtime>(app: &AppHandle<R>, state: RecordingState) -> Option<Image<'_>> {
    let base = app.default_window_icon()?;
    let badge = match state {
        RecordingState::Recording | RecordingState::Pausing | RecordingState::Resuming => {
            Some(Badge::Recording)
        }
        RecordingState::Paused => Some(Badge::Paused),
        RecordingState::Stopped | RecordingState::Starting | RecordingState::Stopping => None,
    };
    Some(match badge {
        Some(badge) => {
            let mut rgba = base.rgba().to_vec();
            draw_badge(
                &mut rgba,
                base.width() as usize,
                base.height() as usize,
                badge,
            );
            Image::new_owned(rgba, base.width(), base.height())
        }
        None => base.clone(),
    })
}

/// "12 min", "1 h 05 min". The tooltip updates once a minute, so it says minutes.
fn elapsed_text(minutes: u64) -> String {
    if minutes == 0 {
        "under 1 min".to_string()
    } else if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(limit.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

/// A one-line, bounded title: control characters become spaces.
fn clean_title(raw: &str) -> Option<String> {
    let flat: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    if words.is_empty() {
        None
    } else {
        Some(truncate_chars(&words.join(" "), TITLE_LIMIT))
    }
}

/// The tooltip says what the recorder is doing now. A title is shown only for a
/// recording that exists, shortened to one line.
fn tooltip_text(view: &TrayView) -> String {
    let mut text = match view.state {
        RecordingState::Stopped => APP_NAME.to_string(),
        RecordingState::Starting => format!("{APP_NAME}: starting recording"),
        RecordingState::Recording | RecordingState::Pausing | RecordingState::Resuming => {
            match view.minutes {
                Some(minutes) => format!("{APP_NAME}: recording, {}", elapsed_text(minutes)),
                None => format!("{APP_NAME}: recording"),
            }
        }
        RecordingState::Paused => match view.minutes {
            Some(minutes) => format!("{APP_NAME}: paused after {}", elapsed_text(minutes)),
            None => format!("{APP_NAME}: paused"),
        },
        RecordingState::Stopping => format!("{APP_NAME}: saving recording"),
    };
    let live = matches!(
        view.state,
        RecordingState::Recording
            | RecordingState::Pausing
            | RecordingState::Resuming
            | RecordingState::Paused
    );
    if live {
        if let Some(title) = view.title.as_deref().and_then(clean_title) {
            text.push('\n');
            text.push_str(&title);
        }
    }
    if view.state == RecordingState::Stopped {
        if let Some(prompt) = &view.prompt {
            text.push('\n');
            text.push_str(&format!("{} call detected", prompt.provider.label()));
        }
    }
    truncate_chars(&text, TOOLTIP_LIMIT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Badge {
    Recording,
    Paused,
}

const BADGE_RING: [u8; 3] = [255, 255, 255];
const BADGE_RECORDING: [u8; 3] = [220, 38, 38];
const BADGE_PAUSED: [u8; 3] = [245, 158, 11];
const BADGE_PAUSE_BARS: [u8; 3] = [31, 41, 55];

/// The colour of the badge at an offset from its centre, or `None` outside it.
/// Recording is a red dot; paused is an amber dot with two bars, so the two
/// differ in shape as well as colour.
fn badge_color(badge: Badge, dx: f32, dy: f32, radius: f32) -> Option<[u8; 3]> {
    let distance = (dx * dx + dy * dy).sqrt();
    if distance > radius {
        return None;
    }
    if distance > radius * 0.8 {
        return Some(BADGE_RING);
    }
    Some(match badge {
        Badge::Recording => BADGE_RECORDING,
        Badge::Paused => {
            let bar =
                dx.abs() >= radius * 0.12 && dx.abs() <= radius * 0.40 && dy.abs() <= radius * 0.5;
            if bar {
                BADGE_PAUSE_BARS
            } else {
                BADGE_PAUSED
            }
        }
    })
}

/// Draws the badge into the lower right of an RGBA image.
fn draw_badge(rgba: &mut [u8], width: usize, height: usize, badge: Badge) {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return;
    }
    let side = width.min(height) as f32;
    let radius = side * 0.28;
    let margin = side * 0.03;
    let center_x = width as f32 - radius - margin;
    let center_y = height as f32 - radius - margin;
    let first_x = (center_x - radius).floor().max(0.0) as usize;
    let last_x = ((center_x + radius).ceil() as usize).min(width - 1);
    let first_y = (center_y - radius).floor().max(0.0) as usize;
    let last_y = ((center_y + radius).ceil() as usize).min(height - 1);
    for y in first_y..=last_y {
        for x in first_x..=last_x {
            let dx = x as f32 + 0.5 - center_x;
            let dy = y as f32 + 0.5 - center_y;
            if let Some([red, green, blue]) = badge_color(badge, dx, dy, radius) {
                let index = (y * width + x) * 4;
                rgba[index] = red;
                rgba[index + 1] = green;
                rgba[index + 2] = blue;
                rgba[index + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams_detection::calls::Provider;

    fn view(state: RecordingState, minutes: Option<u64>, title: Option<&str>) -> TrayView {
        TrayView {
            state,
            minutes,
            title: title.map(str::to_string),
            prompt: None,
        }
    }

    #[test]
    fn the_tooltip_tells_the_truth_about_each_state() {
        assert_eq!(
            tooltip_text(&view(RecordingState::Stopped, None, None)),
            "ClawScribe"
        );
        assert_eq!(
            tooltip_text(&view(RecordingState::Starting, None, None)),
            "ClawScribe: starting recording"
        );
        assert_eq!(
            tooltip_text(&view(RecordingState::Recording, Some(12), None)),
            "ClawScribe: recording, 12 min"
        );
        assert_eq!(
            tooltip_text(&view(RecordingState::Recording, Some(0), None)),
            "ClawScribe: recording, under 1 min"
        );
        assert_eq!(
            tooltip_text(&view(RecordingState::Recording, Some(65), None)),
            "ClawScribe: recording, 1 h 05 min"
        );
        assert_eq!(
            tooltip_text(&view(RecordingState::Paused, Some(7), None)),
            "ClawScribe: paused after 7 min"
        );
        assert_eq!(
            tooltip_text(&view(RecordingState::Stopping, None, None)),
            "ClawScribe: saving recording"
        );
        // Without a reading of the time it says only what it knows.
        assert_eq!(
            tooltip_text(&view(RecordingState::Recording, None, None)),
            "ClawScribe: recording"
        );
    }

    #[test]
    fn the_title_is_one_bounded_line_and_only_for_a_live_recording() {
        let text = tooltip_text(&view(
            RecordingState::Recording,
            Some(3),
            Some("Weekly\nsync\t with   the team"),
        ));
        assert_eq!(
            text,
            "ClawScribe: recording, 3 min\nWeekly sync with the team"
        );
        let long = "x".repeat(300);
        let capped = tooltip_text(&view(RecordingState::Recording, Some(3), Some(&long)));
        assert!(capped.chars().count() <= TOOLTIP_LIMIT);
        assert!(capped
            .lines()
            .nth(1)
            .is_some_and(|line| line.ends_with('…')));
        // No recording, no title.
        assert_eq!(
            tooltip_text(&view(RecordingState::Stopped, None, Some("Left over"))),
            "ClawScribe"
        );
        assert_eq!(clean_title("  \n\t "), None);
    }

    #[test]
    fn a_pending_call_prompt_shows_only_while_idle() {
        let prompt = PromptView {
            key: "k".to_string(),
            provider: Provider::Teams,
            retry: false,
            event_title: None,
        };
        let mut idle = view(RecordingState::Stopped, None, None);
        idle.prompt = Some(prompt.clone());
        assert_eq!(
            tooltip_text(&idle),
            "ClawScribe\nMicrosoft Teams call detected"
        );
        let mut recording = view(RecordingState::Recording, Some(1), None);
        recording.prompt = Some(prompt);
        assert!(!tooltip_text(&recording).contains("detected"));
    }

    #[test]
    fn the_badge_marks_only_the_lower_right_and_keeps_the_image_size() {
        let (width, height) = (64usize, 64usize);
        for badge in [Badge::Recording, Badge::Paused] {
            let mut rgba = vec![0u8; width * height * 4];
            draw_badge(&mut rgba, width, height, badge);
            assert_eq!(rgba.len(), width * height * 4);
            // Top left is untouched.
            assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
            // The centre of the badge is opaque.
            let radius = 64.0f32 * 0.28;
            let centre = (64.0f32 - radius - 64.0 * 0.03) as usize;
            let index = (centre * width + centre) * 4;
            assert_eq!(rgba[index + 3], 255);
        }
        let mut recording = vec![0u8; width * height * 4];
        draw_badge(&mut recording, width, height, Badge::Recording);
        let mut paused = vec![0u8; width * height * 4];
        draw_badge(&mut paused, width, height, Badge::Paused);
        assert_ne!(recording, paused, "the two states look different");
        let index = (46 * width + 46) * 4;
        assert_eq!(&recording[index..index + 3], &BADGE_RECORDING);
    }

    #[test]
    fn a_badge_never_draws_outside_a_small_or_malformed_image() {
        let mut tiny = vec![0u8; 16 * 16 * 4];
        draw_badge(&mut tiny, 16, 16, Badge::Recording);
        assert_eq!(tiny.len(), 16 * 16 * 4);
        let mut short = vec![0u8; 8];
        draw_badge(&mut short, 64, 64, Badge::Recording);
        assert_eq!(short, vec![0u8; 8]);
        let mut empty: Vec<u8> = Vec::new();
        draw_badge(&mut empty, 0, 0, Badge::Paused);
        assert!(empty.is_empty());
    }
}
