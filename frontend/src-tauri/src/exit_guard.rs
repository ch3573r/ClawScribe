//! One gate for exit requests during recording. The native Stop path owns
//! capture, file finalization and the library commit. If the bounded wait
//! expires, the recording folder remains available to startup recovery.
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Runtime};

/// Longest a quit waits for a recording to stop and its library save to finish.
///
/// A normal stop drains the transcription queue (capped at 120 s plus a 5 s
/// cancel grace in `stop_recording_session`), unloads the model (3 s) and then
/// encodes and publishes the audio (cap 240 s to 3600 s plus 120 s, usually
/// seconds). Waiting for those worst cases would look like a hang, so the quit
/// waits as long as the transcription drain cap, and the library save shares
/// that budget. Anything unfinished stays recoverable: the spool, checkpoints
/// and transcript files are only released after they have been published.
pub(crate) const EXIT_FINALIZE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateState {
    Idle,
    /// A recording is being stopped on behalf of an exit request.
    Finalizing,
    /// Finalizing finished; the next exit request goes through once.
    Released,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExitDecision {
    /// Let this exit request proceed.
    Allow,
    /// An earlier request is already stopping the recording; wait for it.
    Defer,
    /// Defer this request and start stopping the recording.
    StopRecordingFirst,
}

struct ExitGate {
    state: GateState,
}

impl ExitGate {
    const fn new() -> Self {
        Self {
            state: GateState::Idle,
        }
    }

    /// `code` is the `ExitRequested` code: `None` for a platform quit,
    /// `Some` for `AppHandle::exit`/`restart`.
    fn request(&mut self, code: Option<i32>, recording_active: bool) -> ExitDecision {
        // Tauri ignores `prevent_exit` for a restart, so deferring would only
        // leave the recording to be cut off. The update flow already refuses
        // to install while a recording is active.
        if code == Some(tauri::RESTART_EXIT_CODE) {
            return ExitDecision::Allow;
        }
        match self.state {
            GateState::Released => {
                self.state = GateState::Idle;
                ExitDecision::Allow
            }
            GateState::Finalizing => ExitDecision::Defer,
            GateState::Idle if recording_active => {
                self.state = GateState::Finalizing;
                ExitDecision::StopRecordingFirst
            }
            GateState::Idle => ExitDecision::Allow,
        }
    }

    fn finish(&mut self) {
        if self.state == GateState::Finalizing {
            self.state = GateState::Released;
        }
    }

    /// The exit the gate asked for was held back by something else (the
    /// unsaved-edits prompt) and did not happen, so the release is spent.
    fn exit_not_taken(&mut self) {
        if self.state == GateState::Released {
            self.state = GateState::Idle;
        }
    }
}

static GATE: Mutex<ExitGate> = Mutex::new(ExitGate::new());

fn gate() -> std::sync::MutexGuard<'static, ExitGate> {
    GATE.lock().unwrap_or_else(|error| error.into_inner())
}

/// What happened when the recording was stopped for an exit.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    Stopped,
    /// Nothing was active any more, or another stop had already finished.
    NotActive,
    Failed(String),
    TimedOut,
}

/// Runs the stop for at most `limit`. Dropping the stop future at the limit is
/// the same as the process ending mid-finalization: unpublished recording data
/// is still on disk for startup recovery.
pub(crate) async fn bounded_stop(
    limit: Duration,
    stop: impl Future<Output = Result<bool, String>>,
) -> StopOutcome {
    match tokio::time::timeout(limit, stop).await {
        Ok(Ok(true)) => StopOutcome::Stopped,
        Ok(Ok(false)) => StopOutcome::NotActive,
        Ok(Err(error)) => StopOutcome::Failed(error),
        Err(_) => StopOutcome::TimedOut,
    }
}

/// Call when another handler deferred an exit request before the gate saw it.
/// If that request was the gate's own follow-up exit (the user chose to keep
/// the app open), the release is withdrawn so the next exit stops a recording
/// again.
pub fn exit_deferred_elsewhere() {
    gate().exit_not_taken();
}

/// Call from `RunEvent::ExitRequested`. Returns true when the exit was
/// deferred (the caller must not run its own exit handling for this request).
pub fn handle_exit_request<R: Runtime>(
    app: &AppHandle<R>,
    code: Option<i32>,
    prevent_exit: impl FnOnce(),
) -> bool {
    let decision = gate().request(code, crate::audio::recording_commands::capture_active());
    match decision {
        ExitDecision::Allow => false,
        ExitDecision::Defer => {
            prevent_exit();
            true
        }
        ExitDecision::StopRecordingFirst => {
            prevent_exit();
            log::info!("Exit requested during a recording; stopping and saving it first");
            let app = app.clone();
            let code = code.unwrap_or(0);
            tauri::async_runtime::spawn(async move {
                stop_recording_then_exit(app, code).await;
            });
            true
        }
    }
}

async fn stop_recording_then_exit<R: Runtime>(app: AppHandle<R>, code: i32) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some("ClawScribe - saving your recording before quitting"));
    }
    let outcome = bounded_stop(
        EXIT_FINALIZE_TIMEOUT,
        crate::audio::recording_commands::stop_recording(
            app.clone(),
            // The stop path ignores the save path; the recording folder is
            // chosen when the recording starts.
            crate::audio::recording_commands::RecordingArgs {
                save_path: String::new(),
            },
        ),
    )
    .await;
    match outcome {
        StopOutcome::Stopped => log::info!("Recording stop completed before exit"),
        StopOutcome::NotActive => log::info!("No recording remained for the exit Stop"),
        StopOutcome::Failed(error) => {
            log::error!("Could not stop the recording before exit: {error}; exiting anyway");
        }
        StopOutcome::TimedOut => {
            log::error!(
                "Stopping the recording took longer than {} s; exiting anyway. Startup recovery will offer it.",
                EXIT_FINALIZE_TIMEOUT.as_secs()
            );
        }
    }
    gate().finish();
    app.exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_recording_exit_is_unchanged() {
        let mut gate = ExitGate::new();
        assert_eq!(gate.request(Some(0), false), ExitDecision::Allow);
        assert_eq!(gate.request(None, false), ExitDecision::Allow);
        assert_eq!(gate.state, GateState::Idle);
    }

    #[test]
    fn a_recording_is_stopped_before_the_exit_goes_through() {
        let mut gate = ExitGate::new();
        assert_eq!(gate.request(Some(0), true), ExitDecision::StopRecordingFirst);
        // A second Quit while stopping neither exits nor starts a second stop.
        assert_eq!(gate.request(Some(0), true), ExitDecision::Defer);
        assert_eq!(gate.request(None, true), ExitDecision::Defer);
        gate.finish();
        // The exit the gate asks for itself passes, even if the recording is
        // still reported active (stop failed or timed out).
        assert_eq!(gate.request(Some(0), true), ExitDecision::Allow);
        assert_eq!(gate.state, GateState::Idle);
    }

    #[test]
    fn the_release_is_consumed_once() {
        let mut gate = ExitGate::new();
        gate.request(Some(0), true);
        gate.finish();
        assert_eq!(gate.request(Some(0), true), ExitDecision::Allow);
        // Back to normal: a later recording is protected again.
        assert_eq!(gate.request(Some(0), true), ExitDecision::StopRecordingFirst);
    }

    #[test]
    fn a_restart_cannot_be_deferred_and_does_not_disturb_the_gate() {
        let mut gate = ExitGate::new();
        assert_eq!(
            gate.request(Some(tauri::RESTART_EXIT_CODE), true),
            ExitDecision::Allow
        );
        assert_eq!(gate.state, GateState::Idle);
        gate.request(Some(0), true);
        assert_eq!(
            gate.request(Some(tauri::RESTART_EXIT_CODE), true),
            ExitDecision::Allow
        );
        assert_eq!(gate.state, GateState::Finalizing);
    }

    #[test]
    fn a_follow_up_exit_that_does_not_happen_withdraws_the_release() {
        let mut gate = ExitGate::new();
        assert_eq!(gate.request(Some(0), true), ExitDecision::StopRecordingFirst);
        gate.finish();
        // The unsaved-edits prompt held the exit back and the user kept the
        // app open; a new recording must be stopped again on the next Quit.
        gate.exit_not_taken();
        assert_eq!(gate.state, GateState::Idle);
        assert_eq!(gate.request(Some(0), true), ExitDecision::StopRecordingFirst);
    }

    #[test]
    fn withdrawing_never_disturbs_a_stop_in_progress() {
        let mut gate = ExitGate::new();
        gate.request(Some(0), true);
        gate.exit_not_taken();
        assert_eq!(gate.state, GateState::Finalizing);
        assert_eq!(gate.request(Some(0), true), ExitDecision::Defer);
    }

    #[test]
    fn finish_without_a_stop_in_progress_does_nothing() {
        let mut gate = ExitGate::new();
        gate.finish();
        assert_eq!(gate.state, GateState::Idle);
        assert_eq!(gate.request(Some(0), false), ExitDecision::Allow);
    }

    #[tokio::test]
    async fn a_finished_stop_is_reported() {
        let limit = Duration::from_secs(5);
        assert_eq!(
            bounded_stop(limit, async { Ok(true) }).await,
            StopOutcome::Stopped
        );
        assert_eq!(
            bounded_stop(limit, async { Ok(false) }).await,
            StopOutcome::NotActive
        );
    }

    #[tokio::test]
    async fn a_failed_stop_does_not_block_the_exit() {
        let outcome = bounded_stop(Duration::from_secs(5), async {
            Err("disk full".to_string())
        })
        .await;
        assert_eq!(outcome, StopOutcome::Failed("disk full".into()));
    }

    #[tokio::test]
    async fn a_stop_that_never_finishes_times_out() {
        let started = std::time::Instant::now();
        let outcome = bounded_stop(
            Duration::from_millis(50),
            std::future::pending::<Result<bool, String>>(),
        )
        .await;
        assert_eq!(outcome, StopOutcome::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
