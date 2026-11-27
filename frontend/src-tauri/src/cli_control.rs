//! Command-line control of a running instance.
//!
//! A second launch such as `ClawScribe.exe --toggle-recording` is forwarded to
//! the running instance by the single-instance plugin. This module parses the
//! flags and drives the same recording path the tray and global shortcut use.
//! `--pause-recording` toggles between paused and recording.

use std::ffi::OsStr;
use tauri::{AppHandle, Runtime};

pub(crate) const START_IN_TRAY_ARG: &str = "--start-in-tray";
#[cfg(windows)]
pub(crate) const AUTOSTART_NAME: &str = "ClawScribe";

pub(crate) fn is_sign_in_launch(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> bool {
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == OsStr::new(START_IN_TRAY_ARG))
}

#[cfg(any(windows, test))]
fn quote_startup_exe(path: &std::path::Path) -> Option<String> {
    let path = path.to_str()?;
    (!path.contains('"')).then(|| format!("\"{path}\""))
}

#[tauri::command]
pub(crate) async fn enable_windows_autostart() -> Result<(), String> {
    #[cfg(windows)]
    return tokio::task::spawn_blocking(|| {
        let exe = std::env::current_exe().map_err(|_| "Could not locate ClawScribe.")?;
        let path =
            quote_startup_exe(&exe).ok_or("Could not use the ClawScribe installation path.")?;
        // The plugin's Windows launcher does not quote the Run command's path.
        // Quote it before writing the per-user entry, including on paths with spaces.
        auto_launch::AutoLaunch::new(AUTOSTART_NAME, &path, &[START_IN_TRAY_ARG])
            .enable()
            .map_err(|_| "Windows could not enable ClawScribe startup.".to_string())
    })
    .await
    .map_err(|_| "Changing Windows startup was interrupted.".to_string())?;
    #[cfg(not(windows))]
    Err("This startup setting is available on Windows.".into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliAction {
    ToggleRecording,
    StartRecording,
    StopRecording,
    PauseRecording,
    Bookmark,
}

/// Returns the first recognised flag in `args`. `args[0]` is the program path
/// and is never treated as a flag; unknown arguments are ignored.
pub fn parse_cli_action<S: AsRef<str>>(args: &[S]) -> Option<CliAction> {
    args.iter().skip(1).find_map(|arg| match arg.as_ref() {
        "--toggle-recording" => Some(CliAction::ToggleRecording),
        "--start-recording" => Some(CliAction::StartRecording),
        "--stop-recording" => Some(CliAction::StopRecording),
        "--pause-recording" => Some(CliAction::PauseRecording),
        "--bookmark" => Some(CliAction::Bookmark),
        _ => None,
    })
}

pub(crate) fn dispatch_forwarded(
    args: &[String],
    run: impl FnOnce(CliAction),
    focus_main: impl FnOnce(),
) {
    match parse_cli_action(args) {
        Some(action) => run(action),
        None if !is_sign_in_launch(args) => focus_main(),
        None => {}
    }
}

/// Performs the action through the tray's recording handler (the same path the
/// global start/stop shortcut uses). Start and stop carry their intent, so
/// repeating a command is harmless and a request never does the opposite. A
/// start is carried out in the background: the running window keeps its page and
/// drafts and is not brought forward.
pub fn run_cli_action<R: Runtime>(app: &AppHandle<R>, action: CliAction) {
    // Without the meeting database nothing may start a recording.
    if crate::database::setup::startup_failed() {
        return;
    }
    let want_recording = match action {
        CliAction::ToggleRecording => None,
        CliAction::StartRecording => Some(true),
        CliAction::StopRecording => Some(false),
        CliAction::PauseRecording => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                use crate::audio::recording_commands::{
                    is_recording, is_recording_paused, is_stopping,
                };
                if is_recording().await && !is_stopping() {
                    dispatch_pause_toggle(
                        is_recording_paused().await,
                        || crate::tray::pause_recording_handler(&app),
                        || crate::tray::resume_recording_handler(&app),
                    );
                }
            });
            return;
        }
        CliAction::Bookmark => return crate::library::bookmark_active_recording(app),
    };
    crate::tray::recording_intent_handler(
        app,
        want_recording,
        crate::start_request::StartSource::Cli,
    );
}

fn dispatch_pause_toggle(paused: bool, pause: impl FnOnce(), resume: impl FnOnce()) {
    if paused {
        resume();
    } else {
        pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_flag_dispatches_both_directions() {
        use std::cell::Cell;
        let action = Cell::new(None);
        for (paused, expected) in [(false, "pause"), (true, "resume")] {
            dispatch_pause_toggle(
                paused,
                || action.set(Some("pause")),
                || action.set(Some("resume")),
            );
            assert_eq!(action.take(), Some(expected));
        }
    }

    #[test]
    fn parses_each_flag() {
        assert_eq!(
            parse_cli_action(&["app", "--toggle-recording"]),
            Some(CliAction::ToggleRecording)
        );
        assert_eq!(
            parse_cli_action(&["app", "--start-recording"]),
            Some(CliAction::StartRecording)
        );
        assert_eq!(
            parse_cli_action(&["app", "--stop-recording"]),
            Some(CliAction::StopRecording)
        );
    }

    #[test]
    fn no_flag_or_unknown_flags_give_none() {
        assert_eq!(parse_cli_action::<&str>(&[]), None);
        assert_eq!(parse_cli_action(&["app"]), None);
        assert_eq!(parse_cli_action(&["app", "--verbose", "file.wav"]), None);
    }

    #[test]
    fn first_recognised_flag_wins() {
        assert_eq!(
            parse_cli_action(&["app", "--bogus", "--stop-recording", "--start-recording"]),
            Some(CliAction::StopRecording)
        );
    }

    #[test]
    fn program_path_is_not_a_flag() {
        assert_eq!(parse_cli_action(&["--start-recording"]), None);
        let owned = vec!["app".to_string(), "--start-recording".to_string()];
        assert_eq!(parse_cli_action(&owned), Some(CliAction::StartRecording));
    }

    #[test]
    fn forwarded_pause_and_bookmark_dispatch_without_focusing_the_window() {
        for (flag, action) in [
            ("--pause-recording", CliAction::PauseRecording),
            ("--bookmark", CliAction::Bookmark),
        ] {
            let args = vec!["ClawScribe.exe".to_owned(), flag.to_owned()];
            let mut actions = Vec::new();
            dispatch_forwarded(
                &args,
                |action| actions.push(action),
                || panic!("handled recording flags must not focus the window"),
            );
            assert_eq!(actions, [action]);
        }
    }

    #[test]
    fn sign_in_launch_stays_hidden_but_an_explicit_launch_focuses_the_window() {
        assert!(is_sign_in_launch(["app.exe", "--start-in-tray"]));
        assert!(!is_sign_in_launch(["app.exe"]));
        assert!(!is_sign_in_launch(["--start-in-tray"]));

        let focused = std::cell::Cell::new(false);
        for (args, should_focus) in [
            (vec!["app.exe", "--start-in-tray"], false),
            (vec!["app.exe"], true),
        ] {
            let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
            dispatch_forwarded(
                &args,
                |_| panic!("sign-in launch must not issue a recording command"),
                || focused.set(true),
            );
            assert_eq!(focused.replace(false), should_focus);
        }
    }

    #[test]
    fn startup_executable_with_spaces_is_quoted() {
        let exe = std::path::Path::new(r"C:\Program Files\ClawScribe\ClawScribe.exe");
        assert_eq!(
            quote_startup_exe(exe).as_deref(),
            Some(r#""C:\Program Files\ClawScribe\ClawScribe.exe""#)
        );
    }
}
