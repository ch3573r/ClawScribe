//! One gate for every route that asks the app to exit while a recording is
//! active (tray Quit, the unsaved-edits quit prompt, a platform quit request).
//!
//! `AppHandle::exit` and a platform quit both arrive as
//! `RunEvent::ExitRequested`. The gate defers that exit, stops the recording
//! through the normal stop path, and then hands over to the frontend, which
//! writes the library row after `recording-stop-complete` (the same event the
//! tray stop emits). The frontend reports the end of that save flow through
//! `recording_exit_save_finished`, in the manner of the summary edit-exit
//! handshake. Only then does the gate ask to exit again.
//!
//! The whole wait is bounded: if the stop fails, or the stop plus the library
//! save do not finish in time, the app exits anyway, because a hung Quit is
//! worse than leaving the recording to startup recovery. The log says "saved"
//! only when the frontend confirmed the library save.
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::oneshot;

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

/// When the recording had already been stopped by someone else (typically the
/// window's own Stop), the frontend's save flow is in flight or finished. It
/// reports through the same command; if nothing reports, do not wait the full
/// budget for a save that does not exist.
const ALREADY_STOPPED_SAVE_GRACE: Duration = Duration::from_secs(15);

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

/// The pending "library save finished" confirmation of one exit. It exists only
/// while an exit waits, so the command that confirms it is a no-op otherwise.
struct SaveWait(Mutex<Option<oneshot::Sender<()>>>);

impl SaveWait {
    const fn new() -> Self {
        Self(Mutex::new(None))
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<oneshot::Sender<()>>> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Starts waiting for a confirmation, replacing any earlier one.
    fn arm(&self) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        *self.slot() = Some(sender);
        receiver
    }

    /// Returns true when an exit was waiting for this confirmation.
    fn confirm(&self) -> bool {
        match self.slot().take() {
            Some(sender) => sender.send(()).is_ok(),
            None => false,
        }
    }

    fn clear(&self) {
        self.slot().take();
    }
}

static SAVE_WAIT: SaveWait = SaveWait::new();

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    Confirmed,
    TimedOut,
    /// The wait was replaced or cleared before a confirmation arrived.
    Abandoned,
}

pub(crate) async fn wait_for_save(receiver: oneshot::Receiver<()>, limit: Duration) -> SaveOutcome {
    match tokio::time::timeout(limit, receiver).await {
        Ok(Ok(())) => SaveOutcome::Confirmed,
        Ok(Err(_)) => SaveOutcome::Abandoned,
        Err(_) => SaveOutcome::TimedOut,
    }
}

/// The frontend calls this every time its post-stop save flow ends, whether it
/// succeeded or not. It does nothing unless an exit is waiting for it.
#[tauri::command]
pub fn recording_exit_save_finished() {
    if SAVE_WAIT.confirm() {
        log::info!("Library save finished; continuing with the exit");
    }
}

/// What happened when the recording was stopped for an exit.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    Saved,
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
        Ok(Ok(true)) => StopOutcome::Saved,
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
    let started = Instant::now();
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some("ClawScribe - saving your recording before quitting"));
    }
    // Armed before the stop: if the window's own Stop is already running, its
    // save flow can finish while we wait for it.
    let early_save = SAVE_WAIT.arm();
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
    let remaining = EXIT_FINALIZE_TIMEOUT.saturating_sub(started.elapsed());
    let save_wait = match &outcome {
        StopOutcome::Saved => {
            // Hand the recording to the frontend exactly as the tray stop does.
            let confirmation = SAVE_WAIT.arm();
            match app.emit("recording-stop-complete", true) {
                Ok(()) => Some((confirmation, remaining)),
                Err(error) => {
                    SAVE_WAIT.clear();
                    log::error!(
                        "Recording stopped, but the library save could not be started ({error}); exiting anyway. Startup recovery will offer the recording."
                    );
                    None
                }
            }
        }
        StopOutcome::NotActive => {
            if crate::audio::recording_commands::capture_active() {
                // Only a different recording or a stop that is still running
                // gets here; say so rather than claim the recording is saved.
                log::warn!(
                    "The stop for this exit did not finish the recording and it is still active; exiting anyway. Startup recovery will offer it."
                );
            } else {
                log::info!(
                    "The recording had already been stopped; checking that its library save finished"
                );
            }
            Some((early_save, remaining.min(ALREADY_STOPPED_SAVE_GRACE)))
        }
        StopOutcome::Failed(error) => {
            log::error!("Could not stop the recording before exit: {error}; exiting anyway");
            None
        }
        StopOutcome::TimedOut => {
            log::error!(
                "Stopping the recording took longer than {} s; exiting anyway. Startup recovery will offer it.",
                EXIT_FINALIZE_TIMEOUT.as_secs()
            );
            None
        }
    };
    if let Some((confirmation, limit)) = save_wait {
        match wait_for_save(confirmation, limit).await {
            SaveOutcome::Confirmed => {
                log::info!("Recording stopped and its library save finished; exiting")
            }
            SaveOutcome::TimedOut => log::error!(
                "Recording stopped, but the library save was not confirmed within {} s; exiting anyway. Startup recovery will offer the recording.",
                limit.as_secs()
            ),
            SaveOutcome::Abandoned => log::warn!(
                "Recording stopped, but the library save confirmation was dropped; exiting anyway"
            ),
        }
    }
    SAVE_WAIT.clear();
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
            StopOutcome::Saved
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

    #[tokio::test]
    async fn a_confirmed_save_ends_the_wait_early() {
        let wait = SaveWait::new();
        let receiver = wait.arm();
        let started = std::time::Instant::now();
        // The frontend confirms while the exit is already waiting.
        assert!(wait.confirm());
        assert_eq!(
            wait_for_save(receiver, Duration::from_secs(30)).await,
            SaveOutcome::Confirmed
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        // A repeated confirmation is harmless.
        assert!(!wait.confirm());
    }

    #[tokio::test]
    async fn a_confirmation_that_arrives_before_the_wait_starts_is_kept() {
        let wait = SaveWait::new();
        let receiver = wait.arm();
        assert!(wait.confirm());
        assert_eq!(
            wait_for_save(receiver, Duration::from_millis(50)).await,
            SaveOutcome::Confirmed
        );
    }

    #[tokio::test]
    async fn an_unconfirmed_save_times_out_within_the_limit() {
        let wait = SaveWait::new();
        let receiver = wait.arm();
        let started = std::time::Instant::now();
        assert_eq!(
            wait_for_save(receiver, Duration::from_millis(50)).await,
            SaveOutcome::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        // The pending wait is still cleared by the caller, never left armed.
        wait.clear();
        assert!(!wait.confirm());
    }

    #[test]
    fn confirming_without_a_pending_exit_is_a_no_op() {
        let wait = SaveWait::new();
        assert!(!wait.confirm());
        assert!(!wait.confirm());
    }

    #[tokio::test]
    async fn re_arming_abandons_the_earlier_wait() {
        let wait = SaveWait::new();
        let first = wait.arm();
        let second = wait.arm();
        assert_eq!(
            wait_for_save(first, Duration::from_secs(5)).await,
            SaveOutcome::Abandoned
        );
        assert!(wait.confirm());
        assert_eq!(
            wait_for_save(second, Duration::from_secs(5)).await,
            SaveOutcome::Confirmed
        );
    }

    #[test]
    fn the_command_does_nothing_when_no_exit_is_pending() {
        // Uses the real static; no test arms it.
        recording_exit_save_finished();
        recording_exit_save_finished();
    }
}
