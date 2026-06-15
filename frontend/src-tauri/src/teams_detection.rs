use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use std::collections::HashSet;
use sysinfo::System;

const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.65;
const PROCESS_SIGNAL_CONFIDENCE: f32 = 0.30;
const BROWSER_PROCESS_SIGNAL_CONFIDENCE: f32 = 0.10;
const TITLE_SIGNAL_CONFIDENCE: f32 = 0.50;
const BROWSER_TITLE_SIGNAL_CONFIDENCE: f32 = 0.35;
const FOREGROUND_TITLE_SIGNAL_CONFIDENCE: f32 = 0.10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsDetectionConfig {
    pub enabled: bool,
    pub confidence_threshold: f32,
    pub require_meeting_title_signal: bool,
    pub max_window_title_samples: usize,
}

impl Default for TeamsDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: cfg!(target_os = "windows"),
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
            require_meeting_title_signal: true,
            max_window_title_samples: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsDetectionStatus {
    pub supported: bool,
    pub enabled: bool,
    pub platform: String,
    pub status: TeamsDetectionState,
    pub detected: bool,
    pub confidence: f32,
    pub threshold: f32,
    pub require_meeting_title_signal: bool,
    pub reason: String,
    pub signals: Vec<TeamsDetectionSignal>,
    pub candidates: Vec<TeamsDetectionCandidate>,
    pub diagnostics: TeamsDetectionDiagnostics,
    pub recording_safety: TeamsDetectionRecordingSafety,
    pub next_recommended_action: TeamsDetectionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsDetectionDiagnostics {
    pub process_count: usize,
    pub teams_process_count: usize,
    pub browser_process_count: usize,
    pub relevant_window_count: usize,
    pub meeting_title_count: usize,
    pub browser_meeting_title_count: usize,
    pub foreground_meeting_title_count: usize,
    pub window_sample_limit: usize,
    pub title_signal_required: bool,
    pub title_signal_satisfied: bool,
    pub confidence_capped_by_title_requirement: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsDetectionRecordingSafety {
    pub mode: String,
    pub automatic_recording_allowed: bool,
    pub prompt_required: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsDetectionSignal {
    pub detector: String,
    pub matched: bool,
    pub confidence: f32,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamsDetectionCandidate {
    pub source: String,
    pub process_id: Option<u32>,
    pub process_name: Option<String>,
    pub window_title: Option<String>,
    pub is_foreground: bool,
    pub is_minimized: bool,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamsDetectionState {
    Unsupported,
    Disabled,
    NotDetected,
    Possible,
    Detected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamsDetectionAction {
    Idle,
    PromptToRecord,
    Unsupported,
    Disabled,
}

#[derive(Debug, Clone)]
struct ProcessSnapshot {
    pid: u32,
    name: String,
    exe_path: Option<String>,
    command_line: Vec<String>,
}

#[derive(Debug, Clone)]
struct WindowSnapshot {
    pid: Option<u32>,
    title: String,
    is_foreground: bool,
    is_minimized: bool,
}

#[tauri::command]
pub fn get_teams_detection_config() -> TeamsDetectionConfig {
    TeamsDetectionConfig::default()
}

#[tauri::command]
pub fn get_teams_detection_status(config: Option<TeamsDetectionConfig>) -> TeamsDetectionStatus {
    detect_teams_meeting(config.unwrap_or_default())
}

fn detect_teams_meeting(config: TeamsDetectionConfig) -> TeamsDetectionStatus {
    let threshold = normalize_threshold(config.confidence_threshold);

    if !cfg!(target_os = "windows") {
        return TeamsDetectionStatus {
            supported: false,
            enabled: false,
            platform: std::env::consts::OS.to_string(),
            status: TeamsDetectionState::Unsupported,
            detected: false,
            confidence: 0.0,
            threshold,
            require_meeting_title_signal: config.require_meeting_title_signal,
            reason: "Teams meeting detection is currently implemented for Windows only".to_string(),
            signals: vec![TeamsDetectionSignal {
                detector: "platform".to_string(),
                matched: false,
                confidence: 0.0,
                detail: format!("Unsupported platform: {}", std::env::consts::OS),
            }],
            candidates: Vec::new(),
            diagnostics: diagnostics_for_unavailable_status(&config, false),
            recording_safety: prompt_only_recording_safety(
                "Unsupported platforms cannot trigger recording from Teams detection",
            ),
            next_recommended_action: TeamsDetectionAction::Unsupported,
        };
    }

    if !config.enabled {
        return TeamsDetectionStatus {
            supported: true,
            enabled: false,
            platform: std::env::consts::OS.to_string(),
            status: TeamsDetectionState::Disabled,
            detected: false,
            confidence: 0.0,
            threshold,
            require_meeting_title_signal: config.require_meeting_title_signal,
            reason: "Teams meeting detection is disabled by configuration".to_string(),
            signals: vec![TeamsDetectionSignal {
                detector: "config".to_string(),
                matched: false,
                confidence: 0.0,
                detail: "Detection disabled".to_string(),
            }],
            candidates: Vec::new(),
            diagnostics: diagnostics_for_unavailable_status(&config, true),
            recording_safety: prompt_only_recording_safety(
                "Disabled Teams detection cannot trigger recording",
            ),
            next_recommended_action: TeamsDetectionAction::Disabled,
        };
    }

    let processes = list_processes();
    let windows = list_relevant_windows(config.max_window_title_samples, &processes);
    evaluate_snapshots(config, threshold, &processes, &windows)
}

fn evaluate_snapshots(
    config: TeamsDetectionConfig,
    threshold: f32,
    processes: &[ProcessSnapshot],
    windows: &[WindowSnapshot],
) -> TeamsDetectionStatus {
    let teams_processes: Vec<&ProcessSnapshot> = processes
        .iter()
        .filter(|process| is_teams_process(process))
        .collect();
    let browser_processes: Vec<&ProcessSnapshot> = processes
        .iter()
        .filter(|process| is_browser_process(process))
        .collect();

    let teams_titles: Vec<&WindowSnapshot> = windows
        .iter()
        .filter(|window| looks_like_teams_meeting_title(&window.title))
        .collect();
    let browser_teams_titles: Vec<&WindowSnapshot> = teams_titles
        .iter()
        .copied()
        .filter(|window| {
            window
                .pid
                .and_then(|pid| process_for_pid(processes, pid))
                .is_some_and(is_browser_process)
        })
        .collect();
    let foreground_meeting_titles: Vec<&WindowSnapshot> = teams_titles
        .iter()
        .copied()
        .filter(|window| window.is_foreground && !window.is_minimized)
        .collect();

    let mut confidence = 0.0;
    let mut signals = Vec::new();

    let teams_process_matched = !teams_processes.is_empty();
    if teams_process_matched {
        confidence += PROCESS_SIGNAL_CONFIDENCE;
    }
    signals.push(TeamsDetectionSignal {
        detector: "teams-process".to_string(),
        matched: teams_process_matched,
        confidence: if teams_process_matched {
            PROCESS_SIGNAL_CONFIDENCE
        } else {
            0.0
        },
        detail: if teams_process_matched {
            format!(
                "{} Teams desktop process(es) found with verified names or Teams paths",
                teams_processes.len()
            )
        } else {
            "No Teams desktop process found".to_string()
        },
    });

    let browser_process_matched = !browser_processes.is_empty();
    if browser_process_matched {
        confidence += BROWSER_PROCESS_SIGNAL_CONFIDENCE;
    }
    signals.push(TeamsDetectionSignal {
        detector: "browser-process".to_string(),
        matched: browser_process_matched,
        confidence: if browser_process_matched {
            BROWSER_PROCESS_SIGNAL_CONFIDENCE
        } else {
            0.0
        },
        detail: if browser_process_matched {
            format!(
                "{} Edge/Chrome/WebView2 process(es) found",
                browser_processes.len()
            )
        } else {
            "No Edge/Chrome browser process found".to_string()
        },
    });

    let title_matched = !teams_titles.is_empty();
    if title_matched {
        confidence += TITLE_SIGNAL_CONFIDENCE;
    }
    signals.push(TeamsDetectionSignal {
        detector: "meeting-window-title".to_string(),
        matched: title_matched,
        confidence: if title_matched {
            TITLE_SIGNAL_CONFIDENCE
        } else {
            0.0
        },
        detail: if title_matched {
            format!(
                "{} visible window title(s) look like Teams meetings",
                teams_titles.len()
            )
        } else {
            "No visible window titles look like an active Teams meeting".to_string()
        },
    });

    let browser_title_matched = !browser_teams_titles.is_empty();
    if browser_title_matched && !teams_process_matched {
        confidence += BROWSER_TITLE_SIGNAL_CONFIDENCE;
    }
    signals.push(TeamsDetectionSignal {
        detector: "browser-meeting-title".to_string(),
        matched: browser_title_matched,
        confidence: if browser_title_matched && !teams_process_matched {
            BROWSER_TITLE_SIGNAL_CONFIDENCE
        } else {
            0.0
        },
        detail: if browser_title_matched {
            "Teams meeting title found in Edge/Chrome".to_string()
        } else {
            "No Teams meeting title found in Edge/Chrome".to_string()
        },
    });

    let foreground_title_matched = !foreground_meeting_titles.is_empty();
    if foreground_title_matched {
        confidence += FOREGROUND_TITLE_SIGNAL_CONFIDENCE;
    }
    signals.push(TeamsDetectionSignal {
        detector: "foreground-meeting-window".to_string(),
        matched: foreground_title_matched,
        confidence: if foreground_title_matched {
            FOREGROUND_TITLE_SIGNAL_CONFIDENCE
        } else {
            0.0
        },
        detail: if foreground_title_matched {
            "A Teams meeting-like window is currently foreground".to_string()
        } else {
            "No Teams meeting-like window is currently foreground".to_string()
        },
    });

    let title_requirement_met = !config.require_meeting_title_signal || title_matched;
    let confidence_before_title_requirement = confidence;
    if !title_requirement_met {
        confidence = confidence.min(threshold - 0.01).max(0.0);
    }
    let confidence_capped_by_title_requirement =
        !title_requirement_met && confidence_before_title_requirement > 0.0;
    confidence = confidence.min(1.0);
    let detected = confidence >= threshold && title_requirement_met;

    let mut candidates = Vec::new();
    candidates.extend(
        teams_processes
            .iter()
            .map(|process| TeamsDetectionCandidate {
                source: "process".to_string(),
                process_id: Some(process.pid),
                process_name: Some(process.name.clone()),
                window_title: None,
                is_foreground: false,
                is_minimized: false,
                confidence: PROCESS_SIGNAL_CONFIDENCE,
            }),
    );
    candidates.extend(teams_titles.iter().map(|window| {
        let process_name = window
            .pid
            .and_then(|pid| process_for_pid(processes, pid))
            .map(|process| process.name.clone());
        TeamsDetectionCandidate {
            source: "window-title".to_string(),
            process_id: window.pid,
            process_name,
            window_title: Some(window.title.clone()),
            is_foreground: window.is_foreground,
            is_minimized: window.is_minimized,
            confidence: TITLE_SIGNAL_CONFIDENCE,
        }
    }));
    let status = detection_state(detected, confidence, threshold, title_requirement_met);
    let diagnostics = TeamsDetectionDiagnostics {
        process_count: processes.len(),
        teams_process_count: teams_processes.len(),
        browser_process_count: browser_processes.len(),
        relevant_window_count: windows.len(),
        meeting_title_count: teams_titles.len(),
        browser_meeting_title_count: browser_teams_titles.len(),
        foreground_meeting_title_count: foreground_meeting_titles.len(),
        window_sample_limit: config.max_window_title_samples.max(1),
        title_signal_required: config.require_meeting_title_signal,
        title_signal_satisfied: title_requirement_met,
        confidence_capped_by_title_requirement,
    };

    TeamsDetectionStatus {
        supported: cfg!(target_os = "windows"),
        enabled: config.enabled,
        platform: std::env::consts::OS.to_string(),
        status,
        detected,
        confidence,
        threshold,
        require_meeting_title_signal: config.require_meeting_title_signal,
        reason: detection_reason(detected, confidence, threshold, title_requirement_met),
        signals,
        candidates,
        diagnostics,
        recording_safety: prompt_only_recording_safety(
            "Teams detection is read-only and can only recommend a user prompt",
        ),
        next_recommended_action: if detected {
            TeamsDetectionAction::PromptToRecord
        } else {
            TeamsDetectionAction::Idle
        },
    }
}

fn diagnostics_for_unavailable_status(
    config: &TeamsDetectionConfig,
    title_signal_satisfied: bool,
) -> TeamsDetectionDiagnostics {
    TeamsDetectionDiagnostics {
        process_count: 0,
        teams_process_count: 0,
        browser_process_count: 0,
        relevant_window_count: 0,
        meeting_title_count: 0,
        browser_meeting_title_count: 0,
        foreground_meeting_title_count: 0,
        window_sample_limit: config.max_window_title_samples.max(1),
        title_signal_required: config.require_meeting_title_signal,
        title_signal_satisfied,
        confidence_capped_by_title_requirement: false,
    }
}

fn prompt_only_recording_safety(detail: &str) -> TeamsDetectionRecordingSafety {
    TeamsDetectionRecordingSafety {
        mode: "prompt-only".to_string(),
        automatic_recording_allowed: false,
        prompt_required: true,
        detail: detail.to_string(),
    }
}

fn detection_state(
    detected: bool,
    confidence: f32,
    threshold: f32,
    title_requirement_met: bool,
) -> TeamsDetectionState {
    if detected {
        TeamsDetectionState::Detected
    } else if confidence > 0.0 || (threshold == 0.0 && title_requirement_met) {
        TeamsDetectionState::Possible
    } else {
        TeamsDetectionState::NotDetected
    }
}

fn detection_reason(
    detected: bool,
    confidence: f32,
    threshold: f32,
    title_requirement_met: bool,
) -> String {
    if detected {
        return "Teams meeting confidence met the configured threshold; recording remains prompt-only"
            .to_string();
    }

    if !title_requirement_met {
        return "Process hints were not enough because no meeting-like window title was found"
            .to_string();
    }

    format!(
        "Teams meeting confidence {:.2} is below threshold {:.2}",
        confidence, threshold
    )
}

fn normalize_threshold(threshold: f32) -> f32 {
    if threshold.is_finite() {
        threshold.clamp(0.0, 1.0)
    } else {
        DEFAULT_CONFIDENCE_THRESHOLD
    }
}

fn list_processes() -> Vec<ProcessSnapshot> {
    let mut system = System::new_all();
    system.refresh_all();
    system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessSnapshot {
            pid: pid.as_u32(),
            name: process.name().to_string_lossy().to_string(),
            exe_path: process.exe().map(|path| path.to_string_lossy().to_string()),
            command_line: process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy().to_string())
                .collect(),
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn list_relevant_windows(max_samples: usize, processes: &[ProcessSnapshot]) -> Vec<WindowSnapshot> {
    let relevant_process_ids = relevant_window_process_ids(processes);
    windows::list_relevant_windows(max_samples, &relevant_process_ids)
}

#[cfg(not(target_os = "windows"))]
fn list_relevant_windows(
    _max_samples: usize,
    _processes: &[ProcessSnapshot],
) -> Vec<WindowSnapshot> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn relevant_window_process_ids(processes: &[ProcessSnapshot]) -> HashSet<u32> {
    processes
        .iter()
        .filter(|process| is_teams_process(process) || is_browser_process(process))
        .map(|process| process.pid)
        .collect()
}

fn process_for_pid(processes: &[ProcessSnapshot], pid: u32) -> Option<&ProcessSnapshot> {
    processes.iter().find(|process| process.pid == pid)
}

fn is_teams_process(process: &ProcessSnapshot) -> bool {
    let name = normalize_process_name(&process.name);

    if matches!(name.as_str(), "teams.exe" | "ms-teams.exe" | "msteams.exe")
        || name.contains("microsoft teams")
    {
        return true;
    }

    name == "update.exe" && process_text_contains_teams_context(process)
}

fn is_browser_process(process: &ProcessSnapshot) -> bool {
    let name = normalize_process_name(&process.name);
    matches!(
        name.as_str(),
        "msedge.exe" | "chrome.exe" | "msedgewebview2.exe"
    )
}

fn normalize_process_name(process_name: &str) -> String {
    process_name.trim().to_lowercase()
}

fn process_text_contains_teams_context(process: &ProcessSnapshot) -> bool {
    let mut text = String::new();
    if let Some(exe_path) = &process.exe_path {
        text.push_str(exe_path);
        text.push(' ');
    }
    for part in &process.command_line {
        text.push_str(part);
        text.push(' ');
    }

    let text = text.to_lowercase().replace('/', "\\");
    text.contains("\\microsoft\\teams\\")
        || text.contains("\\teams\\current\\")
        || text.contains("com.squirrel.teams.teams")
        || text.contains("processstart teams.exe")
        || text.contains("teams.exe")
        || text.contains("ms-teams.exe")
        || text.contains("msteams.exe")
}

#[cfg(target_os = "windows")]
fn looks_like_teams_window_context(title: &str) -> bool {
    let title = title.trim().to_lowercase();
    title.contains("teams")
        || title.contains("microsoft teams")
        || title.contains("teams.microsoft.com")
}

fn looks_like_teams_meeting_title(title: &str) -> bool {
    let title = title.trim().to_lowercase();

    if title.is_empty() {
        return false;
    }

    let has_teams_context = title.contains("teams")
        || title.contains("microsoft teams")
        || title.contains("teams.microsoft.com");
    let has_meeting_context = title.contains("meeting")
        || title.contains("call")
        || title.contains("joined")
        || title.contains("lobby")
        || title.contains("screen sharing")
        || title.contains("presenting")
        || title.contains("participants")
        || title.contains("mute")
        || title.contains("unmute")
        || title.contains("leave");

    has_teams_context && has_meeting_context
}

#[cfg(target_os = "windows")]
mod windows {
    use super::WindowSnapshot;
    use std::collections::HashSet;
    use std::ffi::c_void;

    type Bool = i32;
    type Dword = u32;
    type Hwnd = *mut c_void;
    type Lparam = isize;

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            enum_func: Option<unsafe extern "system" fn(Hwnd, Lparam) -> Bool>,
            lparam: Lparam,
        ) -> Bool;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, string: *mut u16, max_count: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: Hwnd, process_id: *mut Dword) -> Dword;
        fn GetForegroundWindow() -> Hwnd;
        fn IsIconic(hwnd: Hwnd) -> Bool;
        fn IsWindowVisible(hwnd: Hwnd) -> Bool;
    }

    struct WindowCollector<'a> {
        max_samples: usize,
        foreground_window: Hwnd,
        relevant_process_ids: &'a HashSet<u32>,
        windows: Vec<WindowSnapshot>,
    }

    pub fn list_relevant_windows(
        max_samples: usize,
        relevant_process_ids: &HashSet<u32>,
    ) -> Vec<WindowSnapshot> {
        let mut collector = WindowCollector {
            max_samples: max_samples.max(1),
            foreground_window: unsafe { GetForegroundWindow() },
            relevant_process_ids,
            windows: Vec::new(),
        };

        unsafe {
            EnumWindows(
                Some(enum_window),
                &mut collector as *mut WindowCollector<'_> as Lparam,
            );
        }

        collector.windows
    }

    unsafe extern "system" fn enum_window(hwnd: Hwnd, lparam: Lparam) -> Bool {
        let collector = &mut *(lparam as *mut WindowCollector);

        if collector.windows.len() >= collector.max_samples {
            return 0;
        }

        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }

        let mut process_id = 0;
        GetWindowThreadProcessId(hwnd, &mut process_id);

        let title_length = GetWindowTextLengthW(hwnd);
        if title_length <= 0 {
            return 1;
        }

        let mut buffer = vec![0u16; title_length as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if copied <= 0 {
            return 1;
        }

        let title = String::from_utf16_lossy(&buffer[..copied as usize])
            .trim()
            .to_string();
        if title.is_empty() {
            return 1;
        }

        let has_relevant_pid =
            process_id != 0 && collector.relevant_process_ids.contains(&process_id);
        if !has_relevant_pid && !super::looks_like_teams_window_context(&title) {
            return 1;
        }

        collector.windows.push(WindowSnapshot {
            pid: (process_id != 0).then_some(process_id),
            title,
            is_foreground: hwnd == collector.foreground_window,
            is_minimized: IsIconic(hwnd) != 0,
        });

        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str) -> ProcessSnapshot {
        ProcessSnapshot {
            pid,
            name: name.to_string(),
            exe_path: None,
            command_line: Vec::new(),
        }
    }

    fn process_with_context(
        pid: u32,
        name: &str,
        exe_path: Option<&str>,
        command_line: &[&str],
    ) -> ProcessSnapshot {
        ProcessSnapshot {
            pid,
            name: name.to_string(),
            exe_path: exe_path.map(str::to_string),
            command_line: command_line.iter().map(|part| part.to_string()).collect(),
        }
    }

    fn window(pid: Option<u32>, title: &str) -> WindowSnapshot {
        WindowSnapshot {
            pid,
            title: title.to_string(),
            is_foreground: false,
            is_minimized: false,
        }
    }

    fn foreground_window(pid: Option<u32>, title: &str) -> WindowSnapshot {
        WindowSnapshot {
            pid,
            title: title.to_string(),
            is_foreground: true,
            is_minimized: false,
        }
    }

    #[test]
    fn title_detection_requires_teams_and_meeting_context() {
        assert!(looks_like_teams_meeting_title(
            "Weekly sync | Microsoft Teams Meeting"
        ));
        assert!(looks_like_teams_meeting_title(
            "Budget review - Teams - Call in progress"
        ));
        assert!(!looks_like_teams_meeting_title("Microsoft Teams"));
        assert!(!looks_like_teams_meeting_title("Calendar - meeting notes"));
    }

    #[test]
    fn confidence_detects_desktop_meeting_window() {
        let config = TeamsDetectionConfig::default();
        let processes = vec![process(42, "ms-teams.exe")];
        let windows = vec![foreground_window(
            Some(42),
            "Weekly sync | Microsoft Teams Meeting",
        )];

        let status = evaluate_snapshots(config, DEFAULT_CONFIDENCE_THRESHOLD, &processes, &windows);

        assert!(status.detected);
        assert_eq!(status.status, TeamsDetectionState::Detected);
        assert!(status.confidence >= DEFAULT_CONFIDENCE_THRESHOLD);
        assert_eq!(
            status.next_recommended_action,
            TeamsDetectionAction::PromptToRecord
        );
        assert_eq!(status.recording_safety.mode, "prompt-only");
        assert!(!status.recording_safety.automatic_recording_allowed);
        assert!(status.recording_safety.prompt_required);
        assert_eq!(status.diagnostics.teams_process_count, 1);
        assert_eq!(status.diagnostics.meeting_title_count, 1);
        assert_eq!(status.diagnostics.foreground_meeting_title_count, 1);
    }

    #[test]
    fn process_only_is_not_detected_when_title_signal_is_required() {
        let config = TeamsDetectionConfig::default();
        let processes = vec![process(42, "ms-teams.exe")];

        let status = evaluate_snapshots(config, DEFAULT_CONFIDENCE_THRESHOLD, &processes, &[]);

        assert!(!status.detected);
        assert_eq!(status.status, TeamsDetectionState::Possible);
        assert!(status.confidence < DEFAULT_CONFIDENCE_THRESHOLD);
        assert!(status.diagnostics.title_signal_required);
        assert!(!status.diagnostics.title_signal_satisfied);
        assert!(status.diagnostics.confidence_capped_by_title_requirement);
        assert_eq!(status.next_recommended_action, TeamsDetectionAction::Idle);
    }

    #[test]
    fn browser_meeting_title_can_reach_threshold() {
        let config = TeamsDetectionConfig::default();
        let processes = vec![process(99, "msedge.exe")];
        let windows = vec![window(
            Some(99),
            "teams.microsoft.com - Customer call - Microsoft Teams",
        )];

        let status = evaluate_snapshots(config, DEFAULT_CONFIDENCE_THRESHOLD, &processes, &windows);

        assert!(status.detected);
        assert_eq!(status.status, TeamsDetectionState::Detected);
        assert_eq!(
            status.next_recommended_action,
            TeamsDetectionAction::PromptToRecord
        );
        assert!(!status.recording_safety.automatic_recording_allowed);
        assert_eq!(status.diagnostics.browser_process_count, 1);
        assert_eq!(status.diagnostics.browser_meeting_title_count, 1);
        assert!(status.diagnostics.title_signal_satisfied);
    }

    #[test]
    fn browser_process_without_meeting_title_stays_prompt_idle() {
        let config = TeamsDetectionConfig::default();
        let processes = vec![process(99, "chrome.exe")];
        let windows = vec![window(Some(99), "New tab - Google Chrome")];

        let status = evaluate_snapshots(config, DEFAULT_CONFIDENCE_THRESHOLD, &processes, &windows);

        assert!(!status.detected);
        assert_eq!(status.status, TeamsDetectionState::Possible);
        assert_eq!(status.next_recommended_action, TeamsDetectionAction::Idle);
        assert!(!status.recording_safety.automatic_recording_allowed);
        assert_eq!(status.diagnostics.browser_process_count, 1);
        assert_eq!(status.diagnostics.meeting_title_count, 0);
        assert!(status.diagnostics.confidence_capped_by_title_requirement);
    }

    #[test]
    fn generic_update_process_is_not_a_teams_process() {
        let update_process = process_with_context(
            12,
            "Update.exe",
            Some(r"C:\Program Files\Vendor\Update.exe"),
            &[r"C:\Program Files\Vendor\Update.exe", "--background"],
        );

        assert!(!is_teams_process(&update_process));
    }

    #[test]
    fn teams_update_process_requires_teams_context() {
        let update_process = process_with_context(
            13,
            "Update.exe",
            Some(r"C:\Users\user\AppData\Local\Microsoft\Teams\Update.exe"),
            &["Update.exe", "--processStart", "Teams.exe"],
        );

        assert!(is_teams_process(&update_process));
    }

    #[test]
    fn no_signals_reports_not_detected() {
        let config = TeamsDetectionConfig::default();
        let status = evaluate_snapshots(config, DEFAULT_CONFIDENCE_THRESHOLD, &[], &[]);

        assert!(!status.detected);
        assert_eq!(status.status, TeamsDetectionState::NotDetected);
        assert_eq!(status.next_recommended_action, TeamsDetectionAction::Idle);
        assert_eq!(status.diagnostics.process_count, 0);
        assert!(!status.recording_safety.automatic_recording_allowed);
    }
}
