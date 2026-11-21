//! One explicitly opened window per native recording. Its immutable webview
//! label binds every request to the recording, including requests after reload.
use super::{
    active_bookmark_session, stop_recording_session, StopTarget, IS_RECORDING, RECORDING_MANAGER,
    SAVING_RECORDING,
};
use crate::audio::recording_levels::RecordingLevels;
use serde::Serialize;
use std::sync::{atomic::Ordering, Mutex, TryLockError};
use tauri::{
    ipc::Invoke, webview::NewWindowResponse, AppHandle, Manager, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

const PREFIX: &str = "recording-controls-";
const ROUTE: &str = "/recording-controls";
const OPEN_FAILED: &str = "Could not open floating controls. Try again from the tray.";
static OPEN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
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
                "floating_status" | "floating_note" | "floating_stop" | "dismiss_floating_controls"
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

pub(crate) fn available_session() -> Option<String> {
    let saving = SAVING_RECORDING
        .lock()
        .ok()?
        .as_ref()
        .and_then(|s| s.session_id.clone());
    saving.or_else(active_bookmark_session)
}

#[tauri::command]
pub(crate) async fn open_floating_controls<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    let _opening = OPEN.lock().await;
    if available_session().as_deref() != Some(&session_id) {
        return Err("This recording is no longer available.".into());
    }
    let label = format!("{PREFIX}{session_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        existing
            .unminimize()
            .and_then(|_| existing.show())
            .and_then(|_| existing.set_focus())
            .map_err(|_| OPEN_FAILED)?;
        return Ok(());
    }
    // Replace a previous session's recovery notice on explicit reopen.
    for (other_label, window) in app.webview_windows() {
        if session(&other_label).is_ok() {
            window.destroy().map_err(|_| OPEN_FAILED)?;
        }
    }
    let origin = app
        .get_webview_window("main")
        .ok_or(OPEN_FAILED)?
        .url()
        .map_err(|_| OPEN_FAILED)?;
    let builder =
        WebviewWindowBuilder::new(&app, label, WebviewUrl::App("recording-controls".into()));
    #[cfg(windows)]
    let builder = builder.data_directory(
        app.path()
            .app_local_data_dir()
            .map_err(|_| OPEN_FAILED)?
            .join("controls-webview"),
    );
    builder
        .incognito(true)
        .title("Recording controls")
        .inner_size(340., 214.)
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
    Reconnecting,
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
}

#[tauri::command]
pub(crate) async fn floating_status<R: Runtime>(
    window: WebviewWindow<R>,
) -> Result<Snapshot, String> {
    Ok(Snapshot {
        status: recording_status(session(window.label())?)?,
        appearance: APPEARANCE.lock().unwrap().clone(),
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
    let manager = match RECORDING_MANAGER.try_lock() {
        Ok(manager) => manager,
        Err(TryLockError::WouldBlock) => return Ok(Status::Reconnecting),
        Err(TryLockError::Poisoned(_)) => return Err("Recording status unavailable.".into()),
    };
    Ok(match manager.as_ref() {
        Some(m) if m.live_session_id() == Some(id) && m.is_recording() => Status::Recording {
            seconds: m.get_active_recording_duration().unwrap_or_default(),
            paused: m.is_paused(),
            levels: m.get_state().latest_levels(),
        },
        None if IS_RECORDING.load(Ordering::Acquire) => Status::Reconnecting,
        _ => Status::Ended,
    })
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
    fn only_a_recorder_bound_window_can_use_the_four_panel_commands() {
        let id = uuid::Uuid::new_v4().to_string();
        let label = format!("{PREFIX}{id}");
        assert_eq!(session(&label), Ok(id.as_str()));
        for command in [
            "floating_status",
            "floating_note",
            "floating_stop",
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
}
