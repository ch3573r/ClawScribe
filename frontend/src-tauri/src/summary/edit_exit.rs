use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[derive(Default)]
pub struct SummaryEditsState(Mutex<ExitState>);

impl SummaryEditsState {
    pub(crate) fn ensure_saved_for_update(&self) -> Result<(), String> {
        let state = self.0.lock().unwrap();
        if state.pending || state.attempt.is_some() {
            return Err(
                "Save your meeting edits and close any quit prompt before installing the update."
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Default)]
struct ExitState {
    pending: bool,
    next_id: u64,
    attempt: Option<Attempt>,
}

struct Attempt {
    id: u64,
    code: i32,
    deadline: Instant,
    dialog_open: bool,
    from_close_button: bool,
}

enum Begin {
    Allow,
    Wait(u64),
    AlreadyWaiting,
}

impl ExitState {
    fn begin(&mut self, code: i32, now: Instant, from_close_button: bool) -> Begin {
        if self.attempt.is_some() {
            return Begin::AlreadyWaiting;
        }
        if !self.pending && !from_close_button {
            return Begin::Allow;
        }
        self.next_id += 1;
        self.attempt = Some(Attempt {
            id: self.next_id,
            code,
            deadline: now + Duration::from_secs(5),
            dialog_open: false,
            from_close_button,
        });
        Begin::Wait(self.next_id)
    }

    fn finish(&mut self, id: u64) -> Option<Attempt> {
        let attempt = self.attempt.as_ref()?;
        if attempt.id != id || attempt.dialog_open || self.pending {
            return None;
        }
        self.attempt.take()
    }

    fn prompt(&mut self, id: u64, now: Instant, failed: bool) -> bool {
        let Some(attempt) = self.attempt.as_mut() else {
            return false;
        };
        if attempt.id != id || attempt.dialog_open || (!failed && now < attempt.deadline) {
            return false;
        }
        attempt.dialog_open = true;
        true
    }

    fn choose(&mut self, id: u64, quit: bool) -> Option<Attempt> {
        let attempt = self.attempt.as_ref()?;
        if attempt.id != id || !attempt.dialog_open {
            return None;
        }
        let attempt = self.attempt.take()?;
        quit.then_some(attempt)
    }
}

/// Returns true when this exit request must wait for save or user choice.
pub fn request_exit(app: &AppHandle, code: i32, prevent_exit: impl FnOnce()) -> bool {
    begin_exit(app, code, false, prevent_exit)
}

// A close click can race the webview's first pending-edit acknowledgement.
// Give the webview a chance to drain the draft even in that case.
pub(crate) fn quit_from_window(app: &AppHandle) {
    begin_exit(app, 0, true, || {});
}

pub(crate) fn recording_blocks_window_quit() -> bool {
    use crate::audio::recording_commands::{capture_active, is_starting};
    capture_active() || is_starting()
}

pub(crate) fn show_recording_blocks_window_quit(app: &AppHandle) {
    app.dialog()
        .message("Stop and save the recording before quitting ClawScribe.")
        .title("Recording in progress")
        .kind(MessageDialogKind::Warning)
        .show(|_| {});
}

fn resume_exit(app: &AppHandle, attempt: Attempt, discard: bool) {
    if attempt.from_close_button && recording_blocks_window_quit() {
        show_recording_blocks_window_quit(app);
    } else {
        if discard {
            app.state::<SummaryEditsState>().0.lock().unwrap().pending = false;
        }
        app.exit(attempt.code);
    }
}

fn begin_exit(
    app: &AppHandle,
    code: i32,
    from_close_button: bool,
    prevent_exit: impl FnOnce(),
) -> bool {
    let begin = app.state::<SummaryEditsState>().0.lock().unwrap().begin(
        code,
        Instant::now(),
        from_close_button,
    );
    if !matches!(begin, Begin::Allow) {
        prevent_exit();
    }
    match begin {
        Begin::Allow => false,
        Begin::AlreadyWaiting => true,
        Begin::Wait(id) => {
            let timer_app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                show_discard_prompt(&timer_app, id, false);
            });
            if app.emit("summary-flush-before-exit", id).is_err() {
                show_discard_prompt(app, id, true);
            }
            true
        }
    }
}

fn show_discard_prompt(app: &AppHandle, id: u64, failed: bool) {
    let show =
        app.state::<SummaryEditsState>()
            .0
            .lock()
            .unwrap()
            .prompt(id, Instant::now(), failed);
    if !show {
        return;
    }
    let callback_app = app.clone();
    app.dialog()
        .message("Some meeting edits couldn't be saved. Quit anyway and discard them?")
        .title("Unsaved meeting edits")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit".into(),
            "Keep ClawScribe open".into(),
        ))
        .show(move |quit| {
            let attempt = callback_app
                .state::<SummaryEditsState>()
                .0
                .lock()
                .unwrap()
                .choose(id, quit);
            if let Some(attempt) = attempt {
                resume_exit(&callback_app, attempt, true);
            }
        });
}

#[tauri::command]
pub fn api_set_summary_edits_pending(state: tauri::State<'_, SummaryEditsState>, pending: bool) {
    state.0.lock().unwrap().pending = pending;
}

#[tauri::command]
pub fn api_finish_summary_edit_exit(
    app: AppHandle,
    state: tauri::State<'_, SummaryEditsState>,
    attempt_id: u64,
) {
    let attempt = state.0.lock().unwrap().finish(attempt_id);
    if let Some(attempt) = attempt {
        resume_exit(&app, attempt, false);
    }
}

#[tauri::command]
pub fn api_summary_edit_exit_failed(app: AppHandle, attempt_id: u64) {
    show_discard_prompt(&app, attempt_id, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(now: Instant) -> (ExitState, u64) {
        let mut state = ExitState {
            pending: true,
            ..ExitState::default()
        };
        let Begin::Wait(id) = state.begin(7, now, false) else {
            panic!("Expected save wait")
        };
        (state, id)
    }

    #[test]
    fn native_update_requires_saved_drafts_and_no_pending_quit() {
        let edits = SummaryEditsState::default();
        assert!(edits.ensure_saved_for_update().is_ok());
        let now = Instant::now();
        let (state, id) = waiting(now);
        *edits.0.lock().unwrap() = state;
        assert!(edits.ensure_saved_for_update().is_err());
        edits.0.lock().unwrap().pending = false;
        assert!(edits.ensure_saved_for_update().is_err());
        assert_eq!(
            edits
                .0
                .lock()
                .unwrap()
                .finish(id)
                .map(|attempt| attempt.code),
            Some(7)
        );
        assert!(edits.ensure_saved_for_update().is_ok());
    }

    #[test]
    fn save_finishes_before_deadline_without_dialog() {
        let now = Instant::now();
        let (mut state, id) = waiting(now);
        assert!(!state.prompt(id, now + Duration::from_secs(4), false));
        state.pending = false;
        assert_eq!(state.finish(id).map(|attempt| attempt.code), Some(7));
        assert!(!state.prompt(id, now + Duration::from_secs(5), false));
    }

    #[test]
    fn timeout_prompts_once_and_returns_the_exit_request_on_confirmation() {
        let now = Instant::now();
        let (mut state, id) = waiting(now);
        assert!(state.prompt(id, now + Duration::from_secs(5), false));
        assert!(matches!(state.begin(8, now, false), Begin::AlreadyWaiting));
        assert!(!state.prompt(id, now + Duration::from_secs(10), true));
        assert_eq!(state.choose(id, true).map(|attempt| attempt.code), Some(7));
        assert!(state.attempt.is_none());
    }

    #[test]
    fn failure_prompts_immediately_and_keep_clears_exit_only() {
        let now = Instant::now();
        let (mut state, id) = waiting(now);
        assert!(state.prompt(id, now, true));
        assert!(state.choose(id, false).is_none());
        assert!(state.pending);
        assert!(state.attempt.is_none());
        let Begin::Wait(next) = state.begin(9, now, false) else {
            panic!("Expected another attempt")
        };
        state.pending = false;
        assert!(
            state.finish(id).is_none(),
            "Old callback cannot finish a new quit"
        );
        assert!(!state.prompt(id, now, true));
        assert_eq!(state.finish(next).map(|attempt| attempt.code), Some(9));
    }

    #[test]
    fn the_close_button_waits_even_before_the_webview_reports_pending_edits() {
        let mut state = ExitState::default();
        let Begin::Wait(id) = state.begin(0, Instant::now(), true) else {
            panic!("Expected a flush request");
        };
        let attempt = state.finish(id).unwrap();
        assert_eq!(attempt.code, 0);
        assert!(attempt.from_close_button);
    }
}
