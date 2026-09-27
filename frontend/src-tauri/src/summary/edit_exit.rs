use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[derive(Default)]
pub struct SummaryEditsState(Mutex<ExitState>);

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
}

enum Begin {
    Allow,
    Wait(u64),
    AlreadyWaiting,
}

impl ExitState {
    fn begin(&mut self, code: i32, now: Instant) -> Begin {
        if self.attempt.is_some() {
            return Begin::AlreadyWaiting;
        }
        if !self.pending {
            return Begin::Allow;
        }
        self.next_id += 1;
        self.attempt = Some(Attempt {
            id: self.next_id,
            code,
            deadline: now + Duration::from_secs(5),
            dialog_open: false,
        });
        Begin::Wait(self.next_id)
    }

    fn finish(&mut self, id: u64) -> Option<i32> {
        let attempt = self.attempt.as_ref()?;
        if attempt.id != id || attempt.dialog_open || self.pending {
            return None;
        }
        self.attempt.take().map(|attempt| attempt.code)
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

    fn choose(&mut self, id: u64, quit: bool) -> Option<i32> {
        let attempt = self.attempt.as_ref()?;
        if attempt.id != id || !attempt.dialog_open {
            return None;
        }
        let code = self.attempt.take()?.code;
        if quit {
            self.pending = false;
            Some(code)
        } else {
            None
        }
    }
}

/// Returns true when this exit request must wait for save or user choice.
pub fn request_exit(app: &AppHandle, code: i32, prevent_exit: impl FnOnce()) -> bool {
    let begin = app
        .state::<SummaryEditsState>()
        .0
        .lock()
        .unwrap()
        .begin(code, Instant::now());
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
            let code = callback_app
                .state::<SummaryEditsState>()
                .0
                .lock()
                .unwrap()
                .choose(id, quit);
            if let Some(code) = code {
                callback_app.exit(code);
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
    let code = state.0.lock().unwrap().finish(attempt_id);
    if let Some(code) = code {
        app.exit(code);
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
        let Begin::Wait(id) = state.begin(7, now) else {
            panic!("Expected save wait")
        };
        (state, id)
    }

    #[test]
    fn save_finishes_before_deadline_without_dialog() {
        let now = Instant::now();
        let (mut state, id) = waiting(now);
        assert!(!state.prompt(id, now + Duration::from_secs(4), false));
        state.pending = false;
        assert_eq!(state.finish(id), Some(7));
        assert!(!state.prompt(id, now + Duration::from_secs(5), false));
    }

    #[test]
    fn timeout_prompts_once_and_quit_discards_pending_edits() {
        let now = Instant::now();
        let (mut state, id) = waiting(now);
        assert!(state.prompt(id, now + Duration::from_secs(5), false));
        assert!(matches!(state.begin(8, now), Begin::AlreadyWaiting));
        assert!(!state.prompt(id, now + Duration::from_secs(10), true));
        assert_eq!(state.choose(id, true), Some(7));
        assert!(!state.pending);
        assert!(matches!(state.begin(0, now), Begin::Allow));
    }

    #[test]
    fn failure_prompts_immediately_and_keep_clears_exit_only() {
        let now = Instant::now();
        let (mut state, id) = waiting(now);
        assert!(state.prompt(id, now, true));
        assert_eq!(state.choose(id, false), None);
        assert!(state.pending);
        assert!(state.attempt.is_none());
        let Begin::Wait(next) = state.begin(9, now) else {
            panic!("Expected another attempt")
        };
        state.pending = false;
        assert_eq!(
            state.finish(id),
            None,
            "Old callback cannot finish a new quit"
        );
        assert!(!state.prompt(id, now, true));
        assert_eq!(state.finish(next), Some(9));
    }
}
