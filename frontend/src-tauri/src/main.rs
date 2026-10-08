#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

// Logging is initialized by tauri-plugin-log (registered in app_lib::run),
// which writes to a file in the app log dir, stdout, and the webview console.
// We intentionally do NOT init env_logger here — only one global logger can be
// set, and double-init panics.
fn main() {
    if let Some(code) = app_lib::knowledge::documents::worker::dispatch() {
        std::process::exit(code);
    }
    app_lib::run();
}
