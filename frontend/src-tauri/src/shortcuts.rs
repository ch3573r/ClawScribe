//! Global keyboard shortcuts for recording control.
//!
//! Registers system-wide shortcuts (so they work while you're in Teams/your
//! browser) that drive the same recording path as the tray. Bindings are
//! user-configurable and persisted; registration failures (a combo another app
//! already owns) are surfaced as conflicts so the settings UI can warn.

use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, JavaScriptChannelId};
use tauri::{AppHandle, Manager, Runtime, Webview};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShortcutAction {
    StartStop,
    PauseResume,
    ToggleWindow,
    BookmarkRecording,
}

fn default_toggle_window() -> String {
    "Ctrl+Shift+F11".to_string()
}

fn default_bookmark_recording() -> String {
    "Ctrl+Shift+F8".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutBindings {
    pub start_stop: String,
    pub pause_resume: String,
    // serde default so a shortcuts.json written before this field existed still
    // loads (with the default binding) instead of resetting every shortcut.
    #[serde(default = "default_toggle_window")]
    pub toggle_window: String,
    #[serde(default = "default_bookmark_recording")]
    pub bookmark_recording: String,
}

impl Default for ShortcutBindings {
    fn default() -> Self {
        // Function-key combos avoid the common app/OS conflicts (browser
        // hard-reload, Edge InPrivate, VS Code palette, AltGr, language switch).
        ShortcutBindings {
            start_stop: "Ctrl+Shift+F9".to_string(),
            pause_resume: "Ctrl+Shift+F10".to_string(),
            toggle_window: default_toggle_window(),
            bookmark_recording: default_bookmark_recording(),
        }
    }
}

/// Per-action result so the UI can warn about the specific binding that failed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutApplyResult {
    pub bindings: ShortcutBindings,
    /// Action keys whose binding could not be
    /// registered (already in use by another app, or invalid).
    pub conflicts: Vec<String>,
}

// Currently-registered (Shortcut -> Action) pairs, consulted by the handler.
static REGISTERED: Mutex<Vec<(Shortcut, ShortcutAction)>> = Mutex::new(Vec::new());

// Windows delivers already registered hotkeys to the native handler before the
// WebView can record them. Route them back to the focused shortcut editor.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);
static CAPTURE: Mutex<Option<(Instant, Channel<ShortcutAction>)>> = Mutex::new(None);

#[cfg(windows)]
fn owns_foreground(main: isize, foreground: isize, root: isize) -> bool {
    main != 0 && foreground != 0 && (foreground == main || root == main)
}

#[cfg(windows)]
fn main_is_foreground<R: Runtime>(app: &AppHandle<R>) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOT};
    let Some(main) = app.get_webview_window("main").and_then(|w| w.hwnd().ok()) else {
        return false;
    };
    // The focused WebView2 child and Tao's top-level window can disagree about
    // keyboard focus. Compare the foreground window tree, even for child HWNDs.
    unsafe {
        let foreground = GetForegroundWindow();
        let root = GetAncestor(foreground, GA_ROOT);
        owns_foreground(main.0 as isize, foreground.0, root.0)
    }
}

#[cfg(not(windows))]
fn main_is_foreground<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window("main")
        .is_some_and(|window| window.is_focused().unwrap_or(false))
}

fn capture_channel<R: Runtime>(app: &AppHandle<R>) -> Option<Channel<ShortcutAction>> {
    let (started, channel) = CAPTURE.lock().unwrap().clone()?;
    if started.elapsed() < CAPTURE_TIMEOUT && main_is_foreground(app) {
        return Some(channel);
    }
    let mut capture = CAPTURE.lock().unwrap();
    if capture.as_ref().is_some_and(|(time, _)| *time == started) {
        *capture = None;
    }
    None
}

/// Resolve global virtual keys on the main window's keyboard layout before a
/// reset. For example Shift+Comma produces "<" on US keyboards.
#[tauri::command]
pub fn shortcut_key_characters<R: Runtime>(
    _window: tauri::WebviewWindow<R>,
    keys: Vec<(u8, bool, bool)>,
) -> Result<Vec<Option<String>>, String> {
    if keys.len() > 4 || _window.label() != "main" {
        return Err("Expected the main window's four global shortcuts.".into());
    }
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::HWND,
            UI::{
                Input::KeyboardAndMouse::{
                    GetKeyboardLayout, MapVirtualKeyExW, ToUnicodeEx, MAPVK_VK_TO_VSC, VK_CONTROL,
                    VK_MENU, VK_SHIFT,
                },
                WindowsAndMessaging::GetWindowThreadProcessId,
            },
        };
        let hwnd = _window.hwnd().map_err(|e| e.to_string())?;
        // These APIs read the layout of the main HWND's input thread. Flag 4
        // keeps ToUnicodeEx from changing the user's pending dead-key state.
        unsafe {
            let thread = GetWindowThreadProcessId(HWND(hwnd.0 as isize), None);
            let layout = GetKeyboardLayout(thread);
            Ok(keys
                .into_iter()
                .map(|(vk, shift, alt_graph)| {
                    let mut state = [0u8; 256];
                    state[VK_SHIFT.0 as usize] = if shift { 0x80 } else { 0 };
                    if alt_graph {
                        state[VK_CONTROL.0 as usize] = 0x80;
                        state[VK_MENU.0 as usize] = 0x80;
                    }
                    let mut text = [0u16; 8];
                    let scan = MapVirtualKeyExW(vk.into(), MAPVK_VK_TO_VSC, layout);
                    let count = ToUnicodeEx(vk.into(), scan, &state, &mut text, 4, layout);
                    (count > 0).then(|| String::from_utf16_lossy(&text[..count as usize]))
                })
                .collect())
        }
    }
    #[cfg(not(windows))]
    Ok(vec![None; keys.len()])
}

#[tauri::command]
pub fn set_shortcut_capture<R: Runtime>(
    webview: Webview<R>,
    channel: Option<JavaScriptChannelId>,
) -> Result<u32, String> {
    if webview.label() != "main" {
        return Err("Open shortcut settings in the main window.".into());
    }
    *CAPTURE.lock().unwrap() = channel.map(|id| (Instant::now(), id.channel_on(webview)));
    Ok(CAPTURE_TIMEOUT.as_millis() as u32)
}

fn config_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("ClawScribe").join("shortcuts.json"))
}

pub fn load_bindings() -> ShortcutBindings {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<ShortcutBindings>(&s).ok())
        .unwrap_or_default()
}

fn save_bindings(bindings: &ShortcutBindings) {
    if let Some(path) = config_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(bindings) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// The handler invoked by the plugin when any registered shortcut fires.
pub fn on_shortcut<R: Runtime>(app: &AppHandle<R>, shortcut: &Shortcut, state: ShortcutState) {
    if state != ShortcutState::Pressed || crate::database::setup::startup_failed() {
        return;
    }
    let action = {
        let registered = REGISTERED.lock().unwrap();
        registered
            .iter()
            .find(|(sc, _)| sc == shortcut)
            .map(|(_, a)| *a)
    };
    let Some(action) = action else { return };
    if let Some(channel) = capture_channel(app) {
        let _ = channel.send(action);
        return;
    }

    match action {
        ShortcutAction::BookmarkRecording => crate::library::bookmark_active_recording(app),
        ShortcutAction::StartStop => {
            crate::tray::toggle_recording_handler(app, crate::start_request::StartSource::Shortcut)
        }
        ShortcutAction::PauseResume => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if !crate::audio::recording_commands::is_recording().await {
                    return;
                }
                if crate::audio::recording_commands::is_recording_paused().await {
                    crate::tray::resume_recording_handler(&app);
                } else {
                    crate::tray::pause_recording_handler(&app);
                }
            });
        }
        ShortcutAction::ToggleWindow => {
            if let Some(window) = app.get_webview_window("main") {
                // Hide only when genuinely visible AND not minimized. Windows
                // still reports a minimized window as "visible", so without the
                // is_minimized check the hotkey would hide a minimized window
                // instead of restoring it to the front.
                if window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false) {
                    let _ = window.hide();
                } else {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        }
    }
}

/// Register the given bindings, replacing any previously registered shortcuts.
/// Returns the bindings actually applied plus any per-action conflicts.
pub fn apply_bindings<R: Runtime>(
    app: &AppHandle<R>,
    bindings: &ShortcutBindings,
) -> ShortcutApplyResult {
    let gs = app.global_shortcut();
    // Clear previous registrations.
    {
        let mut registered = REGISTERED.lock().unwrap();
        let _ = gs.unregister_all();
        registered.clear();
    }

    let mut conflicts = Vec::new();
    for (key, accel, action) in [
        (
            "startStop",
            bindings.start_stop.as_str(),
            ShortcutAction::StartStop,
        ),
        (
            "pauseResume",
            bindings.pause_resume.as_str(),
            ShortcutAction::PauseResume,
        ),
        (
            "toggleWindow",
            bindings.toggle_window.as_str(),
            ShortcutAction::ToggleWindow,
        ),
        (
            "bookmarkRecording",
            bindings.bookmark_recording.as_str(),
            ShortcutAction::BookmarkRecording,
        ),
    ] {
        match Shortcut::from_str(accel) {
            Ok(shortcut) => match gs.register(shortcut) {
                Ok(()) => {
                    REGISTERED.lock().unwrap().push((shortcut, action));
                }
                Err(e) => {
                    log::warn!("Global shortcut '{accel}' could not be registered: {e}");
                    conflicts.push(key.to_string());
                }
            },
            Err(e) => {
                log::warn!("Invalid shortcut accelerator '{accel}': {e}");
                conflicts.push(key.to_string());
            }
        }
    }

    ShortcutApplyResult {
        bindings: bindings.clone(),
        conflicts,
    }
}

/// Register the persisted (or default) bindings at startup.
pub fn init_from_storage<R: Runtime>(app: &AppHandle<R>) {
    let _ = apply_bindings(app, &load_bindings());
}

// ── Tauri commands ────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_shortcuts() -> ShortcutBindings {
    load_bindings()
}

#[tauri::command]
pub fn set_shortcuts<R: Runtime>(
    app: AppHandle<R>,
    bindings: ShortcutBindings,
) -> ShortcutApplyResult {
    let result = apply_bindings(&app, &bindings);
    // Persist what the user chose even if a binding conflicts, so the UI keeps
    // showing their intent; conflicting ones simply aren't active.
    save_bindings(&bindings);
    result
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn capture_accepts_the_main_foreground_root_only() {
        assert!(owns_foreground(100, 100, 100));
        assert!(owns_foreground(100, 101, 100));
        assert!(!owns_foreground(100, 200, 200));
        assert!(!owns_foreground(100, 0, 0));
        assert!(!owns_foreground(0, 0, 0));
    }
}
