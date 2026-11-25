//! Opt-in supervised Parakeet inference, independent of Tauri.
mod child;
mod directml_probe;
pub mod protocol;
mod supervisor;

pub use supervisor::{
    directml_session_disabled, transcription_timeout, LoadSpec, TranscriptionWorker, WorkerError,
};

pub const WORKER_FLAG: &str = "--clawscribe-transcription-worker";

#[cfg(any(test, all(windows, feature = "directml")))]
static AUTOMATIC_DIRECTML: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[cfg(any(test, all(windows, feature = "directml")))]
pub(super) fn cached_automatic_directml() -> Option<bool> {
    AUTOMATIC_DIRECTML.get().copied()
}

#[cfg(test)]
#[test]
fn directml_cache_read_leaves_an_unchecked_status_unknown() {
    assert_eq!(cached_automatic_directml(), None);
}

#[cfg(all(windows, feature = "directml"))]
pub(super) fn automatic_directml_available(app_data: &std::path::Path) -> bool {
    directml_probe::resolve(&AUTOMATIC_DIRECTML, app_data, || {
        TranscriptionWorker::new()?.probe_directml(std::time::Duration::from_secs(5))
    })
}

/// Dispatch before initializing logging or the desktop runtime.
pub fn dispatch() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(WORKER_FLAG)) {
        return None;
    }
    if args.next().is_some() {
        return Some(2);
    }
    Some(child::run())
}
