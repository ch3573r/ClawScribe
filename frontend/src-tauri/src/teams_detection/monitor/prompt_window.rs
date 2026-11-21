use tauri::{AppHandle, Runtime};

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
    let _ = app;
}

#[cfg(windows)]
fn update<R: Runtime>(app: &AppHandle<R>) -> Result<(), ()> {
    use tauri::{webview::NewWindowResponse, Manager, WebviewUrl, WebviewWindowBuilder};

    // WebView2 creation can block. Recheck the current prompt before showing.
    static WINDOW: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _window = WINDOW.lock().map_err(|_| ())?;
    if super::pending_prompt().is_none() {
        if let Some(window) = app.get_webview_window(LABEL) {
            window.destroy().map_err(|_| ())?;
        }
        return Ok(());
    }
    if app.get_webview_window(LABEL).is_some() {
        return Ok(());
    }
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
    let window =
        WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("call-recording-prompt".into()))
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
    if super::pending_prompt().is_none() {
        window.destroy().map_err(|_| ())
    } else {
        // First show of a focused(false) window uses SW_SHOWNOACTIVATE.
        window.show().map_err(|_| ())
    }
}
