//! Global keyboard shortcuts for recording control.
//!
//! Registers system-wide shortcuts (so they work while you're in Teams/your
//! browser) that drive the same recording path as the tray. Bindings are
//! user-configurable and persisted; registration failures (a combo another app
//! already owns) are surfaced as conflicts so the settings UI can warn.

use std::str::FromStr;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    StartStop,
    PauseResume,
    ToggleWindow,
}

fn default_toggle_window() -> String {
    "Ctrl+Shift+F11".to_string()
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
}

impl Default for ShortcutBindings {
    fn default() -> Self {
        // Function-key combos avoid the common app/OS conflicts (browser
        // hard-reload, Edge InPrivate, VS Code palette, AltGr, language switch).
        ShortcutBindings {
            start_stop: "Ctrl+Shift+F9".to_string(),
            pause_resume: "Ctrl+Shift+F10".to_string(),
            toggle_window: default_toggle_window(),
        }
    }
}

/// Per-action result so the UI can warn about the specific binding that failed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutApplyResult {
    pub bindings: ShortcutBindings,
    /// Action keys ("startStop"/"pauseResume") whose binding could not be
    /// registered (already in use by another app, or invalid).
    pub conflicts: Vec<String>,
}

// Currently-registered (Shortcut -> Action) pairs, consulted by the handler.
static REGISTERED: Mutex<Vec<(Shortcut, ShortcutAction)>> = Mutex::new(Vec::new());

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
    if state != ShortcutState::Pressed {
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

    match action {
        ShortcutAction::StartStop => crate::tray::toggle_recording_handler(app),
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
                if window.is_visible().unwrap_or(false) {
                    let _ = window.hide();
                } else {
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
        ("startStop", bindings.start_stop.as_str(), ShortcutAction::StartStop),
        ("pauseResume", bindings.pause_resume.as_str(), ShortcutAction::PauseResume),
        ("toggleWindow", bindings.toggle_window.as_str(), ShortcutAction::ToggleWindow),
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
