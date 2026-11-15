//! Opt-in supervised Parakeet inference, independent of Tauri.
mod child;
pub mod protocol;
mod supervisor;

pub use supervisor::{
    directml_session_disabled, transcription_timeout, LoadSpec, TranscriptionWorker, WorkerError,
};

pub const WORKER_FLAG: &str = "--clawscribe-transcription-worker";

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
