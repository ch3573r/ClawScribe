# Teams Meeting Detection

This fork includes a read-only Teams meeting detector for Windows. It makes
meeting state observable through local process and window-title evidence, then
returns a conservative status snapshot that callers can use for prompts or
future debounced automation.

The detector does not start, stop, or schedule recordings.

## Tauri Commands

- `get_teams_detection_config` returns the default detector configuration.
- `get_teams_detection_status` returns a single detection snapshot. It accepts
  an optional config override and does not start or stop recording.

The frontend call path is `frontend/src/services/teamsDetectionService.ts`.
It exposes typed wrappers around both commands and intentionally keeps the
detector pull-based.

On non-Windows platforms the status response is cleanly unsupported:
`supported=false`, `enabled=false`, `status=unsupported`, `detected=false`,
and confidence `0.0`.

## Config

Default config:

```json
{
  "enabled": true,
  "confidenceThreshold": 0.65,
  "requireMeetingTitleSignal": true,
  "maxWindowTitleSamples": 100
}
```

`enabled` defaults to `true` only on Windows. `confidenceThreshold` is clamped
to `0.0..1.0`. `requireMeetingTitleSignal` prevents a background Teams process
from being enough by itself. `maxWindowTitleSamples` limits how many visible
relevant window titles the detector returns per status call.

## Detector Signals

The Windows detector uses lightweight local heuristics:

- Teams desktop process: `teams.exe`, `ms-teams.exe`, `msteams.exe`, process
  names containing `Microsoft Teams`, or legacy `update.exe` only when its
  executable path or command line contains Teams context.
- Browser process: `msedge.exe`, `chrome.exe`, or `msedgewebview2.exe`.
- Visible window titles: titles containing Teams context plus meeting context
  such as meeting, call, joined, lobby, presenting, participants, mute/unmute,
  or leave.
- Browser meeting title: a Teams-like meeting title tied to an Edge/Chrome
  process.
- Foreground window: a small confidence boost when the Teams meeting-like
  window is the active foreground window.

The detector intentionally avoids Graph APIs, tenant permissions, Teams bots,
or cloud transcript APIs.

## Confidence Thresholds

Current weights:

- Teams desktop process: `0.30`
- Edge/Chrome/WebView2 process: `0.10`
- Teams meeting-like visible title: `0.50`
- Teams meeting-like browser title, when no desktop Teams process exists:
  `0.35`
- Foreground Teams meeting-like title: `0.10`

The default threshold is `0.65`. With `requireMeetingTitleSignal=true`, process
signals alone cannot cross the threshold. Practical examples:

- Teams desktop process plus meeting title: `0.80`, detected.
- Foreground Teams desktop meeting window: `0.90`, detected.
- Edge/Chrome plus Teams meeting title: `0.95`, detected.
- Teams desktop process only: capped below threshold, `status=possible`.
- Browser process only: `0.10`, `status=possible`.
- No signals: `status=notDetected`.

The status response includes `status`, final `confidence`, `threshold`, each
signal, candidate processes/window titles, foreground/minimized window flags, a
human-readable reason, diagnostics counters, recording safety metadata, and
`nextRecommendedAction`. The current positive action is `PromptToRecord`, not
automatic recording.

## Status Diagnostics

`get_teams_detection_status` returns a `diagnostics` object to make live
Windows validation explainable without enabling automation:

- `processCount`, `teamsProcessCount`, and `browserProcessCount`
- `relevantWindowCount`, `meetingTitleCount`, `browserMeetingTitleCount`, and
  `foregroundMeetingTitleCount`
- `windowSampleLimit`
- `titleSignalRequired` and `titleSignalSatisfied`
- `confidenceCappedByTitleRequirement`

Every status response also includes:

```json
{
  "recordingSafety": {
    "mode": "prompt-only",
    "automaticRecordingAllowed": false,
    "promptRequired": true
  }
}
```

This is intentional. Teams detection is a read-only signal path. It can report
`nextRecommendedAction=promptToRecord`, but it must not start, stop, schedule,
or silently trigger recording.

## False-Positive Controls

The detector defaults are conservative:

- A visible meeting-like title is required.
- Background Teams, Edge, Chrome, and WebView2 processes do not trigger
  detection alone.
- Generic `update.exe` processes are ignored unless path/command-line evidence
  ties them to Teams.
- Window enumeration stores only relevant Teams/browser windows or titles that
  contain Teams context, and returned candidates are bounded.
- The command is pull-based; callers choose polling cadence and can debounce in
  the UI or a future background worker.

Recommended UI behavior is to show an observable "possible Teams meeting"
state or prompt rather than starting capture immediately.

## Windows Smoke Checklist

Use this checklist on the Windows test laptop to validate Teams desktop and
browser detection. Do not click **Start Recording** during this smoke test.

1. Start a dev build:

   ```powershell
   cd frontend
   pnpm tauri:dev:cpu
   ```

2. Open the ClawScribe devtools console. In dev builds, the app installs a
   read-only helper:

   ```js
   await window.__clawscribeTeamsDetection.printStatus()
   ```

   If the helper is missing, confirm this is a dev build. The helper is not
   installed in production builds.

3. Baseline with Teams closed:

   - Expected: `supported=true`, `detected=false`,
     `nextRecommendedAction="idle"`.
   - Expected diagnostics: `teamsProcessCount=0`, `meetingTitleCount=0`.
   - Required safety check: `recordingSafety.automaticRecordingAllowed=false`.

4. Open Teams desktop, but do not join a meeting:

   - Expected: `detected=false`, usually `status="possible"` if a Teams process
     is visible.
   - Expected diagnostics: `teamsProcessCount>0`, `meetingTitleCount=0`,
     `titleSignalSatisfied=false`.
   - Required safety check: `nextRecommendedAction="idle"` and
     `recordingSafety.automaticRecordingAllowed=false`.

5. Join a Teams desktop test meeting:

   - Expected: `detected=true` only when a visible meeting-like Teams title is
     found and confidence meets threshold.
   - Expected diagnostics: `teamsProcessCount>0`, `meetingTitleCount>0`; if the
     meeting window is active, `foregroundMeetingTitleCount>0`.
   - Required safety check: `nextRecommendedAction="promptToRecord"` and
     `recordingSafety.automaticRecordingAllowed=false`.
   - No recording should start unless the user explicitly starts one.

6. Leave the meeting but keep Teams open:

   - Expected: status returns to `possible` or `notDetected`, with
     `nextRecommendedAction="idle"`.
   - Required safety check: no recording starts or stops as a side effect.

7. Validate browser Teams with Edge or Chrome:

   - Join a Teams meeting from `teams.microsoft.com`.
   - Expected diagnostics: `browserProcessCount>0`,
     `browserMeetingTitleCount>0`, and `meetingTitleCount>0`.
   - Expected positive status: `detected=true` and
     `nextRecommendedAction="promptToRecord"` when threshold is met.
   - Required safety check: `recordingSafety.automaticRecordingAllowed=false`.

8. Validate disabled config:

   ```js
   const config = await window.__clawscribeTeamsDetection.getConfig()
   await window.__clawscribeTeamsDetection.printStatus({
     ...config,
     enabled: false
   })
   ```

   Expected: `status="disabled"`, `detected=false`,
   `nextRecommendedAction="disabled"`, and no recording side effects.

If a recording starts without an explicit user action, stop testing and treat it
as a blocker.

## WASAPI Future Detector

WASAPI/system-audio correlation is a good next detector, but it is intentionally
not part of this scaffold. A future implementation can raise confidence when:

- The default communications device or loopback stream is active.
- Teams/Edge/Chrome owns an active audio session.
- Microphone activity and system audio overlap for a sustained period.

This should be added through a dedicated audio-session detector so it can be
debounced and tested independently from process/window heuristics.

## Next Steps for Auto-Recording

1. Add a frontend or background polling loop for `get_teams_detection_status`.
2. Debounce positive detection for a sustained interval, for example 10-20
   seconds above threshold.
3. Debounce stop detection separately, for example 60-120 seconds below
   threshold, to avoid stopping during reconnects or Teams window changes.
4. Require user opt-in and expose the threshold in settings.
5. Wire the positive state to the existing recording commands only after the
   recorder can safely reject duplicate starts and preserve manual override.
6. Add WASAPI audio-session evidence before enabling unattended start/stop by
   default.
