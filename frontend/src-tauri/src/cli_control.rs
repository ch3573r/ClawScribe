//! Command-line control of a running instance.
//!
//! A second launch such as `ClawScribe.exe --toggle-recording` is forwarded to
//! the running instance by the single-instance plugin. This module parses the
//! flags and drives the same recording path the tray and global shortcut use.

use tauri::{AppHandle, Runtime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliAction {
    ToggleRecording,
    StartRecording,
    StopRecording,
}

/// Returns the first recognised flag in `args`. `args[0]` is the program path
/// and is never treated as a flag; unknown arguments are ignored.
pub fn parse_cli_action<S: AsRef<str>>(args: &[S]) -> Option<CliAction> {
    args.iter().skip(1).find_map(|arg| match arg.as_ref() {
        "--toggle-recording" => Some(CliAction::ToggleRecording),
        "--start-recording" => Some(CliAction::StartRecording),
        "--stop-recording" => Some(CliAction::StopRecording),
        _ => None,
    })
}

/// Performs the action through the tray's toggle handler (the same path the
/// global start/stop shortcut uses). Start and stop only toggle when the
/// current state differs, so repeating a command is harmless.
pub fn run_cli_action<R: Runtime>(app: &AppHandle<R>, action: CliAction) {
    match action {
        CliAction::ToggleRecording => crate::tray::toggle_recording_handler(app),
        CliAction::StartRecording | CliAction::StopRecording => {
            let want_recording = action == CliAction::StartRecording;
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let recording = crate::audio::recording_commands::is_recording().await;
                if recording != want_recording {
                    crate::tray::toggle_recording_handler(&app);
                } else {
                    log::info!("Command-line request ignored: recording state already matches");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_flag() {
        assert_eq!(parse_cli_action(&["app", "--toggle-recording"]), Some(CliAction::ToggleRecording));
        assert_eq!(parse_cli_action(&["app", "--start-recording"]), Some(CliAction::StartRecording));
        assert_eq!(parse_cli_action(&["app", "--stop-recording"]), Some(CliAction::StopRecording));
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
}
