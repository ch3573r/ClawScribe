//! One floating window per native recording. Its immutable webview
//! label binds every request to the recording, including requests after reload.
use super::{
    active_capture, capture_snapshot, is_stopping, reconnect_device, set_recording_paused,
    stop_recording_session, CaptureSnapshot, StopTarget, IS_RECORDING, SAVING_RECORDING,
};
use crate::audio::outcome::RecordingOutcome;
use crate::audio::recording_levels::RecordingLevels;
use serde::Serialize;
use std::sync::{atomic::Ordering, Mutex};
use tauri::{
    ipc::Invoke, webview::NewWindowResponse, AppHandle, Emitter, LogicalSize, Manager,
    PhysicalPosition, PhysicalSize, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_store::StoreExt;

const PREFIX: &str = "recording-controls-";
const ROUTE: &str = "/recording-controls";
const WIDTH: u32 = 400;
const BAR_HEIGHT: u32 = 62;
const MAX_HEIGHT: u32 = 640;
const CORNER_MARGIN: u32 = 12;
const VISIBILITY_CHANGED: &str = "floating-controls-visibility-changed";
const OPEN_FAILED: &str = "Could not open floating controls. Try again from the tray.";
// Opening WebView2 can block. Only window workers hold this lock, never capture.
static LAST_OPENED_SESSION: Mutex<Option<String>> = Mutex::new(None);
static LAST_REVIEW: Mutex<Option<String>> = Mutex::new(None);
static APPEARANCE: Mutex<Option<Appearance>> = Mutex::new(None);

#[derive(Clone, Serialize)]
pub(crate) struct Appearance {
    theme: String,
    accent: Option<String>,
}

pub(crate) fn set_appearance(theme: String, accent: Option<String>) {
    let accent = accent.filter(|id| id.len() <= 32 && id.bytes().all(|b| b.is_ascii_lowercase()));
    *APPEARANCE.lock().unwrap() = Some(Appearance { theme, accent });
}

#[tauri::command]
pub(crate) fn recording_window_appearance() -> Option<Appearance> {
    APPEARANCE.lock().ok()?.clone()
}

fn session(label: &str) -> Result<&str, &'static str> {
    label
        .strip_prefix(PREFIX)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .ok_or("These controls are unavailable.")
}

fn allowed(label: &str, command: &str) -> bool {
    label == "main"
        || (label == crate::teams_detection::monitor::prompt_window::LABEL
            && matches!(
                command,
                "get_pending_call_prompt"
                    | "confirm_call_detection_prompt"
                    | "dismiss_call_detection_prompt"
                    | "recording_window_appearance"
            ))
        || (session(label).is_ok()
            && matches!(
                command,
                "floating_status"
                    | "floating_note"
                    | "floating_stop"
                    | "dismiss_floating_controls"
                    | "floating_set_paused"
                    | "floating_reconnect"
                    | "floating_drag"
                    | "floating_resize"
                    | "floating_open_main"
            ))
}

// Tauri checks plugin permissions separately. Local app commands are accessible
// by default when there is no app ACL manifest, so gate the entire dispatcher.
pub(crate) fn restrict_commands<R: Runtime>(
    handler: impl Fn(Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        if allowed(
            invoke.message.webview_ref().label(),
            invoke.message.command(),
        ) {
            handler(invoke)
        } else {
            invoke
                .resolver
                .reject("This command is unavailable in this window.");
            true
        }
    }
}

pub(crate) fn available_session<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    OpenReason::Manual.current_session(app)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenReason {
    Automatic,
    Manual,
}

impl OpenReason {
    fn current_session<R: Runtime>(self, app: &AppHandle<R>) -> Option<String> {
        let saving = SAVING_RECORDING.lock().ok()?;
        if let Some(owner) = saving.as_ref() {
            return if self == Self::Manual {
                owner.session_id.clone()
            } else {
                None
            };
        }
        if is_stopping() || !IS_RECORDING.load(Ordering::Acquire) {
            return None;
        }
        // A reconnect can temporarily take the recording manager. The
        // committed live identity survives it, and Stop clears that identity.
        app.state::<crate::knowledge::KnowledgeState>()
            .live
            .current_session_id()
    }

    fn claim(
        self,
        id: &str,
        current: Option<&str>,
        last_opened: &mut Option<String>,
    ) -> Result<bool, &'static str> {
        if current != Some(id) {
            return match self {
                Self::Automatic => Ok(false),
                Self::Manual => Err("This recording is no longer available."),
            };
        }
        // A manual open can win the race with startup and be dismissed before
        // the automatic work runs. Closing the window never resets this claim.
        if self == Self::Automatic && last_opened.as_deref() == Some(id) {
            return Ok(false);
        }
        *last_opened = Some(id.to_owned());
        Ok(true)
    }
}

pub(super) fn open_on_start<R: Runtime>(app: AppHandle<R>, session_id: String) {
    tauri::async_runtime::spawn_blocking(move || {
        if open(&app, &session_id, OpenReason::Automatic).is_err() {
            log::warn!("Could not open floating controls automatically");
            if OpenReason::Automatic.current_session(&app).as_deref() == Some(&session_id) {
                let _ = app.emit("recording-start-feedback", OPEN_FAILED);
            }
        }
    });
}

#[tauri::command]
pub(crate) async fn open_floating_controls<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open(&app, &session_id, OpenReason::Manual))
        .await
        .map_err(|_| OPEN_FAILED.to_string())?
}

#[tauri::command]
pub(crate) async fn floating_controls_visible<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> bool {
    app.get_webview_window(&format!("{PREFIX}{session_id}"))
        .is_some_and(|window| window.is_visible().unwrap_or(false))
}

fn open<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    reason: OpenReason,
) -> Result<(), String> {
    let mut last_opened = LAST_OPENED_SESSION.lock().map_err(|_| OPEN_FAILED)?;
    let manual = reason == OpenReason::Manual;
    let current = reason.current_session(app);
    if !reason.claim(session_id, current.as_deref(), &mut last_opened)? {
        return Ok(());
    }
    let label = format!("{PREFIX}{session_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        if manual {
            existing
                .unminimize()
                .and_then(|_| existing.show())
                .and_then(|_| existing.set_focus())
                .map_err(|_| OPEN_FAILED)?;
            let _ = app.emit_to("main", VISIBILITY_CHANGED, (session_id, true));
        }
        return Ok(());
    }
    let main = app.get_webview_window("main").ok_or(OPEN_FAILED)?;
    let origin = main.url().map_err(|_| OPEN_FAILED)?;
    let monitor = if main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false) {
        main.current_monitor().ok().flatten()
    } else {
        app.cursor_position()
            .ok()
            .and_then(|point| main.monitor_from_point(point.x, point.y).ok().flatten())
    }
    .or_else(|| main.current_monitor().ok().flatten())
    .or_else(|| main.primary_monitor().ok().flatten());
    let builder =
        WebviewWindowBuilder::new(app, &label, WebviewUrl::App("recording-controls".into()));
    #[cfg(windows)]
    let builder = builder.data_directory(
        app.path()
            .app_local_data_dir()
            .map_err(|_| OPEN_FAILED)?
            .join("controls-webview"),
    );
    let window = builder
        .visible(false)
        .focused(manual)
        .incognito(true)
        .title("Recording controls")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .inner_size(WIDTH.into(), BAR_HEIGHT.into())
        .min_inner_size(WIDTH.into(), BAR_HEIGHT.into())
        .maximizable(false)
        .always_on_top(true)
        .content_protected(true)
        .disable_drag_drop_handler()
        .on_navigation(move |url| {
            url.scheme() == origin.scheme()
                && url.host_str() == origin.host_str()
                && url.port() == origin.port()
                && url.path() == ROUTE
        })
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .build()
        .map_err(|_| OPEN_FAILED)?;
    place_window(&window, monitor.as_ref());
    // The webview may take longer to initialize than a short recording. Keep
    // a late window from replacing any other session's controls.
    if reason.current_session(app).as_deref() != Some(session_id) {
        window.destroy().map_err(|_| OPEN_FAILED)?;
        return Ok(());
    }
    // On Windows the first show of a focused(false) window uses
    // SW_SHOWNOACTIVATE. The webview also skips its initial focus request.
    let app_for_close = app.clone();
    let id_for_close = session_id.to_owned();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = app_for_close.emit_to("main", VISIBILITY_CHANGED, (&id_for_close, false));
        }
    });
    window.show().map_err(|_| OPEN_FAILED)?;
    let _ = app.emit_to("main", VISIBILITY_CHANGED, (session_id, true));
    if manual {
        window.set_focus().map_err(|_| OPEN_FAILED)?;
    }
    for (other_label, previous) in app.webview_windows() {
        if other_label != label && session(&other_label).is_ok() && previous.destroy().is_err() {
            log::warn!("Could not close previous floating controls");
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Status {
    Recording {
        seconds: f64,
        paused: bool,
        levels: Option<RecordingLevels>,
        mic_no_signal: bool,
        mode: super::super::recording_mode::RecordingMode,
    },
    Saving {
        seconds: f64,
        progress: u8,
        message: &'static str,
        estimated_seconds_remaining: Option<f64>,
    },
    Review,
    Ended,
}

impl Status {
    pub(super) fn from_outcome(outcome: &RecordingOutcome, library_save_failed: bool) -> Self {
        if library_save_failed || outcome.needs_recovery() {
            Self::Review
        } else {
            Self::Ended
        }
    }

    pub(super) fn remember_outcome(&self, session_id: Option<&str>) {
        *LAST_REVIEW.lock().unwrap() = session_id
            .filter(|_| matches!(self, Self::Review))
            .map(str::to_owned);
    }

    fn close_finished_window(&self, session_id: Option<&str>, close: impl FnOnce(&str)) {
        if let (Self::Ended, Some(id)) = (self, session_id) {
            close(&format!("{PREFIX}{id}"));
        }
    }

    pub(super) fn finish<R: Runtime>(&self, app: &AppHandle<R>, session_id: Option<&str>) {
        // Save admission must be closed before this lookup. An open still
        // building at this point sees a finished session in its late guard.
        self.close_finished_window(session_id, |label| {
            if let Some(window) = app.get_webview_window(label) {
                if window.destroy().is_err() {
                    log::warn!("Could not close finished floating controls");
                }
            }
        });
    }
}

#[derive(Serialize)]
pub(crate) struct Snapshot {
    #[serde(flatten)]
    status: Status,
    appearance: Option<Appearance>,
    health: Option<CaptureSnapshot>,
    shortcuts: Vec<(crate::shortcuts::ShortcutAction, String)>,
}

#[tauri::command]
pub(crate) async fn floating_status<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<Snapshot, String> {
    let id = session(window.label())?;
    Ok(Snapshot {
        status: recording_status(id)?,
        appearance: recording_window_appearance(),
        health: capture_snapshot(id),
        shortcuts: crate::shortcuts::get_active_shortcuts(),
    })
}

fn recording_status(id: &str) -> Result<Status, String> {
    // Hold admission through the manager read. Stop temporarily takes the
    // manager away, and the panel must not mistake that handoff for completion.
    let saving = SAVING_RECORDING
        .lock()
        .map_err(|_| "Recording status unavailable.")?;
    if let Some(owner) = saving
        .as_ref()
        .filter(|s| s.session_id.as_deref() == Some(id))
    {
        return Ok(Status::Saving {
            seconds: owner.active_seconds,
            progress: owner.progress,
            message: owner.message,
            estimated_seconds_remaining: owner.estimated_seconds_remaining,
        });
    }
    if LAST_REVIEW.lock().unwrap().as_deref() == Some(id) {
        return Ok(Status::Review);
    }
    Ok(match active_capture(id) {
        Some(state) => {
            let (levels, mic_no_signal) = state.live_levels();
            Status::Recording {
                seconds: state.get_active_recording_duration().unwrap_or_default(),
                paused: state.is_paused(),
                levels,
                mic_no_signal,
                mode: if super::LIVE_TRANSCRIPTION.load(Ordering::Acquire) {
                    super::super::recording_mode::RecordingMode::Live
                } else {
                    super::super::recording_mode::RecordingMode::AudioOnly
                },
            }
        }
        _ => Status::Ended,
    })
}

#[tauri::command]
pub(crate) async fn floating_set_paused<R: Runtime>(
    window: WebviewWindow<R>,
    paused: bool,
) -> Result<(), String> {
    set_recording_paused(window.app_handle(), Some(session(window.label())?), paused).await
}

#[tauri::command]
pub(crate) async fn floating_reconnect<R: Runtime>(
    window: WebviewWindow<R>,
    device_name: String,
    device_type: String,
) -> Result<super::super::recording_manager::ReconnectResult, String> {
    use super::super::device_monitor::DeviceMonitorType;
    let kind = match device_type.as_str() {
        "Microphone" => DeviceMonitorType::Microphone,
        "SystemAudio" => DeviceMonitorType::SystemAudio,
        _ => return Err("Invalid audio source.".into()),
    };
    reconnect_device(device_name, kind, session(window.label())?.into()).await
}

fn place_window<R: Runtime>(window: &WebviewWindow<R>, default_monitor: Option<&tauri::Monitor>) {
    let app = window.app_handle();
    let remember = app
        .store("recording_preferences.json")
        .ok()
        .and_then(|store| store.get("preferences"))
        .and_then(|prefs| prefs.get("remember_floating_position")?.as_bool())
        != Some(false);
    let store = remember
        .then(|| app.store("floating_controls.json").ok())
        .flatten();
    let saved = store
        .as_ref()
        .and_then(|store| store.get("position"))
        .and_then(|value| serde_json::from_value::<(i32, i32)>(value).ok())
        .and_then(|(x, y)| {
            let monitor = window
                .available_monitors()
                .ok()?
                .into_iter()
                .find(|monitor| {
                    let work = monitor.work_area();
                    position_visible(
                        (x, y),
                        (work.position.x, work.position.y),
                        (work.size.width, work.size.height),
                    )
                })?;
            Some((PhysicalPosition::new(x, y), monitor))
        });
    if let Some((point, monitor)) = saved {
        fit_position(window, &monitor, point, BAR_HEIGHT);
    } else if let Some(monitor) = default_monitor {
        let work = monitor.work_area();
        let scale = monitor.scale_factor();
        let size = LogicalSize::new(WIDTH, BAR_HEIGHT).to_physical::<u32>(scale);
        let margin = (f64::from(CORNER_MARGIN) * scale).round() as u32;
        let _ = window.set_position(bottom_right(work.position, work.size, size, margin));
    }
    if let Some(store) = store {
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::Moved(position) = event {
                store.set("position", serde_json::json!([position.x, position.y]));
            }
        });
    }
}

fn bottom_right(
    origin: PhysicalPosition<i32>,
    work: PhysicalSize<u32>,
    window: PhysicalSize<u32>,
    margin: u32,
) -> PhysicalPosition<i32> {
    PhysicalPosition::new(
        origin.x
            + work
                .width
                .saturating_sub(window.width.saturating_add(margin)) as i32,
        origin.y
            + work
                .height
                .saturating_sub(window.height.saturating_add(margin)) as i32,
    )
}

fn position_visible(point: (i32, i32), origin: (i32, i32), size: (u32, u32)) -> bool {
    let x = i64::from(point.0) - i64::from(origin.0);
    let y = i64::from(point.1) - i64::from(origin.1);
    x >= 0 && y >= 0 && x + 96 <= i64::from(size.0) && y + 48 <= i64::from(size.1)
}

fn fit_position<R: Runtime>(
    window: &WebviewWindow<R>,
    monitor: &tauri::Monitor,
    position: PhysicalPosition<i32>,
    height: u32,
) {
    let work = monitor.work_area();
    let size = LogicalSize::new(WIDTH, height).to_physical::<u32>(monitor.scale_factor());
    let fitted = PhysicalPosition::new(
        position.x.clamp(
            work.position.x,
            work.position.x + work.size.width.saturating_sub(size.width) as i32,
        ),
        position.y.clamp(
            work.position.y,
            work.position.y + work.size.height.saturating_sub(size.height) as i32,
        ),
    );
    if fitted != position
        || window
            .outer_position()
            .is_ok_and(|current| current != fitted)
    {
        let _ = window.set_position(fitted);
    }
}

#[tauri::command]
pub(crate) fn floating_drag<R: Runtime>(window: WebviewWindow<R>) -> Result<(), String> {
    session(window.label())?;
    window
        .start_dragging()
        .map_err(|_| "Could not move controls.".into())
}

#[tauri::command]
pub(crate) fn floating_resize<R: Runtime>(
    window: WebviewWindow<R>,
    height: u32,
) -> Result<(), String> {
    session(window.label())?;
    let height = height.clamp(BAR_HEIGHT, MAX_HEIGHT);
    let height = if let (Ok(Some(monitor)), Ok(point)) =
        (window.current_monitor(), window.outer_position())
    {
        let work_height =
            (f64::from(monitor.work_area().size.height) / monitor.scale_factor()).floor() as u32;
        let height = height.min(work_height.max(BAR_HEIGHT));
        fit_position(&window, &monitor, point, height);
        height
    } else {
        height
    };
    window
        .set_size(LogicalSize::new(WIDTH, height))
        .map_err(|_| "Could not resize controls.".into())
}

#[tauri::command]
pub(crate) async fn floating_open_main<R: Runtime>(window: WebviewWindow<R>) -> Result<(), String> {
    let id = session(window.label())?;
    let finished = matches!(recording_status(id)?, Status::Review | Status::Ended);
    let main = window
        .app_handle()
        .get_webview_window("main")
        .ok_or("Could not open ClawScribe.")?;
    main.unminimize()
        .and_then(|_| main.show())
        .and_then(|_| main.set_focus())
        .map_err(|_| "Could not open ClawScribe.")?;
    if finished {
        dismiss_floating_controls(window)?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn floating_note<R: Runtime>(
    window: WebviewWindow<R>,
    label: String,
) -> Result<(), String> {
    let id = session(window.label())?.to_owned();
    crate::library::add_meeting_bookmark(window.app_handle().clone(), None, None, label, Some(id))
        .await
}

#[tauri::command]
pub(crate) async fn floating_stop<R: Runtime>(window: WebviewWindow<R>) -> Result<bool, String> {
    let id = session(window.label())?;
    stop_recording_session(
        window.app_handle().clone(),
        Some(StopTarget::Session(id)),
        None,
    )
    .await
}

#[tauri::command]
pub(crate) fn dismiss_floating_controls<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<(), String> {
    session(window.label())?;
    window
        .destroy()
        .map_err(|_| "Could not close controls. Press Alt+F4 to close the window.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_placement_uses_the_work_area_on_a_scaled_secondary_display() {
        let corner = bottom_right(
            PhysicalPosition::new(-1920, 40),
            tauri::PhysicalSize::new(1920, 1000),
            tauri::PhysicalSize::new(549, 90),
            18,
        );
        assert_eq!(corner, PhysicalPosition::new(-567, 932));
        assert_eq!(
            bottom_right(
                PhysicalPosition::new(40, 50),
                tauri::PhysicalSize::new(80, 60),
                tauri::PhysicalSize::new(100, 100),
                18,
            ),
            PhysicalPosition::new(40, 50)
        );
    }

    #[test]
    fn a_dismissed_panel_only_reopens_on_request_until_the_next_recording() {
        use OpenReason::{Automatic, Manual};
        let mut opened = None;
        assert_eq!(Automatic.claim("a", Some("a"), &mut opened), Ok(true));
        assert_eq!(Automatic.claim("a", Some("a"), &mut opened), Ok(false));
        assert_eq!(Manual.claim("a", Some("a"), &mut opened), Ok(true));
        assert_eq!(Automatic.claim("b", Some("b"), &mut opened), Ok(true));
        // Delayed work for A cannot replace B or reset its dismissal.
        assert_eq!(Automatic.claim("a", Some("b"), &mut opened), Ok(false));
        assert!(Manual.claim("a", Some("b"), &mut opened).is_err());
        assert_eq!(Automatic.claim("b", Some("b"), &mut opened), Ok(false));
    }

    #[test]
    fn manual_open_before_delayed_automatic_open_keeps_ownership_of_focus_and_dismissal() {
        let mut opened = None;
        assert_eq!(
            OpenReason::Manual.claim("a", Some("a"), &mut opened),
            Ok(true)
        );
        assert_eq!(
            OpenReason::Automatic.claim("a", Some("a"), &mut opened),
            Ok(false)
        );
        assert_eq!(
            OpenReason::Automatic.claim("b", None, &mut opened),
            Ok(false)
        );
    }

    #[test]
    fn only_a_recorder_bound_window_can_use_the_panel_commands() {
        let id = uuid::Uuid::new_v4().to_string();
        let label = format!("{PREFIX}{id}");
        assert_eq!(session(&label), Ok(id.as_str()));
        for command in [
            "floating_status",
            "floating_note",
            "floating_stop",
            "floating_set_paused",
            "floating_reconnect",
            "floating_drag",
            "floating_resize",
            "floating_open_main",
            "dismiss_floating_controls",
        ] {
            assert!(allowed(&label, command));
            assert!(!allowed("recording-controls-", command));
            assert!(!allowed("another-window", command));
        }
        for command in [
            "add_meeting_bookmark",
            "stop_recording",
            "open_floating_controls",
            "get_meeting_folder_path",
            "future_command",
        ] {
            assert!(!allowed(&label, command));
            assert!(allowed("main", command));
        }
    }

    #[test]
    fn only_outcomes_needing_recovery_keep_the_window() {
        use crate::audio::outcome::{CaptureDetails, EmptyLiveTranscript};
        let mut outcome = RecordingOutcome::default();
        assert!(matches!(
            Status::from_outcome(&outcome, false),
            Status::Ended
        ));
        for empty in [
            EmptyLiveTranscript::NoSpeechDetected,
            EmptyLiveTranscript::NoTextRecognized,
        ] {
            outcome.capture_details = Some(CaptureDetails {
                empty_live_transcript: Some(empty),
                ..Default::default()
            });
            assert!(matches!(
                Status::from_outcome(&outcome, false),
                Status::Ended
            ));
        }
        assert!(matches!(
            Status::from_outcome(&outcome, true),
            Status::Review
        ));
        outcome.capture_incomplete = true;
        assert!(matches!(
            Status::from_outcome(&outcome, false),
            Status::Review
        ));
    }

    #[test]
    fn core_finish_closes_only_the_matching_session_for_an_ended_outcome() {
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let first = format!("{PREFIX}{a}");
        let second = format!("{PREFIX}{b}");
        let mut windows = std::collections::HashSet::from([first.clone(), second.clone()]);
        Status::Review.close_finished_window(Some(&b), |_| panic!("review must stay open"));
        Status::Ended.close_finished_window(None, |_| panic!("finish requires a session"));
        Status::Ended.close_finished_window(Some(&a), |label| {
            assert!(windows.remove(label));
        });
        assert!(!windows.contains(&first));
        assert!(windows.contains(&second));
    }

    #[test]
    fn remembered_position_accepts_a_secondary_monitor_and_rejects_off_screen_title_bar() {
        assert!(position_visible((-1200, 32), (-1920, 0), (1920, 1080)));
        assert!(!position_visible((-1200, 32), (0, 0), (1920, 1080)));
        assert!(!position_visible((1900, 100), (0, 0), (1920, 1080)));
    }

    #[test]
    fn call_prompt_can_only_read_appearance_or_read_and_confirm_the_pending_call() {
        let label = crate::teams_detection::monitor::prompt_window::LABEL;
        assert!(allowed(label, "get_pending_call_prompt"));
        assert!(allowed(label, "confirm_call_detection_prompt"));
        assert!(allowed(label, "dismiss_call_detection_prompt"));
        assert!(allowed(label, "recording_window_appearance"));
        assert!(!allowed(label, "start_recording_with_devices_and_meeting"));
        assert!(!allowed(label, "floating_note"));
    }
}
