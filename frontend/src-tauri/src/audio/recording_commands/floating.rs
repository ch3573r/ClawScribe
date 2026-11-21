//! One floating window per native recording. Its immutable webview
//! label binds every request to the recording, including requests after reload.
use super::{
    active_capture, capture_snapshot, is_stopping, reconnect_device, set_recording_paused,
    stop_recording_session, CaptureSnapshot, StopTarget, IS_RECORDING, SAVING_RECORDING,
};
use crate::audio::recording_levels::RecordingLevels;
use serde::Serialize;
use std::sync::{atomic::Ordering, Mutex};
use tauri::{
    ipc::Invoke, webview::NewWindowResponse, AppHandle, Emitter, Manager, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_store::StoreExt;

const PREFIX: &str = "recording-controls-";
const ROUTE: &str = "/recording-controls";
const OPEN_FAILED: &str = "Could not open floating controls. Try again from the tray.";
// Opening WebView2 can block. Only window workers hold this lock, never capture.
static LAST_OPENED_SESSION: Mutex<Option<String>> = Mutex::new(None);
static LAST_REVIEW: Mutex<Option<String>> = Mutex::new(None);
static APPEARANCE: Mutex<Option<Appearance>> = Mutex::new(None);

#[derive(Clone, Serialize)]
struct Appearance {
    theme: String,
    accent: Option<String>,
}

pub(crate) fn set_appearance(theme: String, accent: Option<String>) {
    let accent = accent.filter(|id| id.len() <= 32 && id.bytes().all(|b| b.is_ascii_lowercase()));
    *APPEARANCE.lock().unwrap() = Some(Appearance { theme, accent });
}

fn session(label: &str) -> Result<&str, &'static str> {
    label
        .strip_prefix(PREFIX)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .ok_or("These controls are unavailable.")
}

fn allowed(label: &str, command: &str) -> bool {
    label == "main"
        || (session(label).is_ok()
            && matches!(
                command,
                "floating_status"
                    | "floating_note"
                    | "floating_stop"
                    | "dismiss_floating_controls"
                    | "floating_set_paused"
                    | "floating_reconnect"
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
        }
        return Ok(());
    }
    let origin = app
        .get_webview_window("main")
        .ok_or(OPEN_FAILED)?
        .url()
        .map_err(|_| OPEN_FAILED)?;
    let builder =
        WebviewWindowBuilder::new(app, &label, WebviewUrl::App("recording-controls".into()));
    #[cfg(windows)]
    let builder = builder.visible(manual).data_directory(
        app.path()
            .app_local_data_dir()
            .map_err(|_| OPEN_FAILED)?
            .join("controls-webview"),
    );
    let window = builder
        .focused(manual)
        .incognito(true)
        .title("Recording controls")
        .inner_size(380., 300.)
        .min_inner_size(294., 190.)
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
    remember_position(&window);
    // The webview may take longer to initialize than a short recording. Keep
    // a late window from replacing any other session's controls.
    if reason.current_session(app).as_deref() != Some(session_id) {
        window.destroy().map_err(|_| OPEN_FAILED)?;
        return Ok(());
    }
    // On Windows the first show of a focused(false) window uses
    // SW_SHOWNOACTIVATE. The webview also skips its initial focus request.
    #[cfg(windows)]
    if !manual {
        window.show().map_err(|_| OPEN_FAILED)?;
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
    },
    Saving {
        seconds: f64,
        progress: u8,
        message: &'static str,
    },
    Review,
    Ended,
}

pub(super) fn remember_outcome(
    identity: Option<&crate::knowledge::live::RecordingIdentity>,
    needs_review: bool,
) {
    *LAST_REVIEW.lock().unwrap() = identity
        .filter(|_| needs_review)
        .map(|id| id.session_id.clone());
}

#[derive(Serialize)]
pub(crate) struct Snapshot {
    #[serde(flatten)]
    status: Status,
    appearance: Option<Appearance>,
    health: Option<CaptureSnapshot>,
}

#[tauri::command]
pub(crate) async fn floating_status<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<Snapshot, String> {
    let id = session(window.label())?;
    Ok(Snapshot {
        status: recording_status(id)?,
        appearance: APPEARANCE.lock().unwrap().clone(),
        health: capture_snapshot(id),
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
        });
    }
    if LAST_REVIEW.lock().unwrap().as_deref() == Some(id) {
        return Ok(Status::Review);
    }
    Ok(match active_capture(id) {
        Some(state) => Status::Recording {
            seconds: state.get_active_recording_duration().unwrap_or_default(),
            paused: state.is_paused(),
            levels: state.latest_levels(),
        },
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

fn remember_position<R: Runtime>(window: &WebviewWindow<R>) {
    let app = window.app_handle();
    if app
        .store("recording_preferences.json")
        .ok()
        .and_then(|store| store.get("preferences"))
        .and_then(|prefs| prefs.get("remember_floating_position")?.as_bool())
        == Some(false)
    {
        return;
    }
    let Ok(store) = app.store("floating_controls.json") else {
        return;
    };
    if let Some((x, y)) = store
        .get("position")
        .and_then(|value| serde_json::from_value::<(i32, i32)>(value).ok())
    {
        let visible = window.available_monitors().is_ok_and(|monitors| {
            monitors.iter().any(|monitor| {
                let work = monitor.work_area();
                position_visible(
                    (x, y),
                    (work.position.x, work.position.y),
                    (work.size.width, work.size.height),
                )
            })
        });
        if visible {
            let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
        }
    }
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Moved(position) = event {
            store.set("position", serde_json::json!([position.x, position.y]));
        }
    });
}

fn position_visible(point: (i32, i32), origin: (i32, i32), size: (u32, u32)) -> bool {
    let x = i64::from(point.0) - i64::from(origin.0);
    let y = i64::from(point.1) - i64::from(origin.1);
    x >= 0 && y >= 0 && x + 96 <= i64::from(size.0) && y + 48 <= i64::from(size.1)
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
        .map_err(|_| "Close this window using its title bar.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn remembered_position_accepts_a_secondary_monitor_and_rejects_off_screen_title_bar() {
        assert!(position_visible((-1200, 32), (-1920, 0), (1920, 1080)));
        assert!(!position_visible((-1200, 32), (0, 0), (1920, 1080)));
        assert!(!position_visible((1900, 100), (0, 0), (1920, 1080)));
    }
}
