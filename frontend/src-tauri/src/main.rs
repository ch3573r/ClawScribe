#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

// Logging is initialized by tauri-plugin-log (registered in app_lib::run),
// which writes to a file in the app log dir, stdout, and the webview console.
// We intentionally do NOT init env_logger here — only one global logger can be
// set, and double-init panics.
fn main() {
    app_lib::crash_report::install_panic_hook();
    if let Some(code) = app_lib::mcp::dispatch() {
        std::process::exit(code);
    }
    // Headless packaged-binary acceptance on a synthetic profile; never opens a
    // window or touches the user's own data (see self_test.rs).
    if let Some(code) = app_lib::self_test::dispatch() {
        std::process::exit(code);
    }
    if let Some(code) = app_lib::knowledge::documents::worker::dispatch() {
        std::process::exit(code);
    }
    if let Some(code) = app_lib::parakeet_engine::worker::dispatch() {
        std::process::exit(code);
    }
    app_lib::run();
}
