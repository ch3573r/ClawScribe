use super::{tracker, PromptSurface};
use tauri::{AppHandle, Emitter, Runtime};

pub(crate) const LABEL: &str = "call-recording-prompt";

pub(super) fn refresh<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(windows)]
    {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if update(&app).is_err() {
                log::warn!("Could not show the call recording prompt window");
            }
        });
    }
    #[cfg(not(windows))]
    if let Some(key) = pending_key() {
        publish(app, &key, PromptSurface::Toast);
    }
}

fn pending_key() -> Option<String> {
    let tracker = tracker();
    let prompt = tracker.prompt()?;
    prompt.surface.is_none().then(|| prompt.key.clone())
}

fn publish<R: Runtime>(app: &AppHandle<R>, key: &str, surface: PromptSurface) -> bool {
    let view = tracker().select_prompt_surface(key, surface);
    if let Some(view) = view {
        let _ = app.emit("call-detection-prompt", view);
        true
    } else {
        false
    }
}

#[cfg(windows)]
fn update<R: Runtime>(app: &AppHandle<R>) -> Result<(), ()> {
    use tauri::Manager;

    // Only workers hold this lock. The close callback can run on the event loop
    // while WebView2 creation waits for that loop.
    static WINDOW_KEY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut window_key = WINDOW_KEY.lock().map_err(|_| ())?;
    if let Some(window) = app.get_webview_window(LABEL) {
        let current = super::pending_prompt();
        if current.as_ref().map(|prompt| &prompt.key) != window_key.as_ref() || current.is_none() {
            window.destroy().map_err(|_| ())?;
        }
        // The Destroyed callback refreshes again after the label is freed.
        return Ok(());
    }
    let Some(key) = pending_key() else {
        return Ok(());
    };
    if let Ok(window) = build_window(app, &key) {
        *window_key = Some(key.clone());
        if pending_key().as_deref() != Some(key.as_str()) {
            return window.destroy().map_err(|_| ());
        }
        // First show of a focused(false) window uses SW_SHOWNOACTIVATE.
        if window.show().is_ok() && window.is_visible().unwrap_or(false) {
            if !publish(app, &key, PromptSurface::Native) {
                window.destroy().map_err(|_| ())?;
            }
            return Ok(());
        }
        let _ = window.destroy();
    }
    publish(app, &key, PromptSurface::Toast);
    Err(())
}

#[cfg(windows)]
fn build_window<R: Runtime>(app: &AppHandle<R>, key: &str) -> Result<tauri::WebviewWindow<R>, ()> {
    use tauri::{webview::NewWindowResponse, Manager, WebviewUrl, WebviewWindowBuilder};

    let origin = app
        .get_webview_window("main")
        .ok_or(())?
        .url()
        .map_err(|_| ())?;
    let profile = app
        .path()
        .app_local_data_dir()
        .map_err(|_| ())?
        .join("call-prompt-webview");
    // The UUID binds the webview and its close callback to one prompt.
    let route = format!("call-recording-prompt?key={key}");
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(route.into()))
        .title("ClawScribe call recording")
        .inner_size(375., 176.)
        .resizable(false)
        .center()
        .visible(false)
        .focused(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .content_protected(true)
        .incognito(true)
        .data_directory(profile)
        .disable_drag_drop_handler()
        .on_navigation(move |url| {
            url.scheme() == origin.scheme()
                && url.host_str() == origin.host_str()
                && url.port() == origin.port()
                && url.path() == "/call-recording-prompt"
        })
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .build()
        .map_err(|_| ())?;
    let app = app.clone();
    let key = key.to_owned();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { .. } => {
            super::dismiss_prompt(&app, &key);
        }
        tauri::WindowEvent::Destroyed => refresh(&app),
        _ => {}
    });
    Ok(window)
}
