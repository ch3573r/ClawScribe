# Teams Meeting Detection

ClawScribe detects likely Microsoft Teams meetings on Windows using local
process/window evidence. It does not use Teams bots, Graph online-meeting APIs,
presence APIs, or cloud transcript APIs.

There are two layers:

- Rust detection commands are read-only and return a status snapshot.
- The frontend can use that snapshot to prompt or auto-start recording,
  depending on the user's setting.

## Commands

- `get_teams_detection_config` returns the default detector configuration.
- `get_teams_detection_status` returns one detection snapshot. It accepts an
  optional config override and does not itself start or stop recording.

Frontend wrappers live in `frontend/src/services/teamsDetectionService.ts`.
The background auto-record poller lives in `frontend/src/components/TeamsAutoRecord.tsx`.

On non-Windows platforms the detector returns a clean unsupported state:
`supported=false`, `enabled=false`, `status=unsupported`, `detected=false`, and
confidence `0.0`.

## Default Config

```json
{
  "enabled": true,
  "confidenceThreshold": 0.65,
  "requireMeetingTitleSignal": true,
  "maxWindowTitleSamples": 100
}
```

Process evidence alone is not enough when `requireMeetingTitleSignal=true`.
This avoids treating a background Teams process as an active meeting.

## Detection Signals

The Windows detector considers:

- Teams desktop processes such as `teams.exe`, `ms-teams.exe`, and
  `msteams.exe`
- Edge, Chrome, and WebView2 browser processes
- visible window titles with Teams and meeting/call/presentation context
- browser Teams meeting titles
- foreground Teams meeting-like windows

The detector returns confidence, status, matched signals, bounded candidate
process/window samples, diagnostics counters, and `nextRecommendedAction`.

## Auto-Record Modes

The user-facing setting is stored by `frontend/src/lib/autoRecord.ts`.

Modes:

- `off`: no polling-driven action
- `prompt`: detect and prompt the user to start recording
- `auto`: start recording once per detected meeting, then re-arm only after the
  meeting has been absent for several consecutive polls

The Rust `recordingSafety.automaticRecordingAllowed` field remains conservative
and currently reports `false`; the frontend auto mode is a user opt-in policy
layer above the read-only detector.

## Calendar Relationship

Teams detection is local. Calendar integration is separate and uses Microsoft
Graph `Calendars.Read` after Microsoft sign-in.

When the user selects a calendar event, ClawScribe can:

- use the event title for the next recording
- carry invited attendees into the meeting summary as a checklist

Teams detection should not require Microsoft sign-in.

## False-Positive Controls

- A visible meeting-like title is required by default.
- Background Teams/browser processes do not trigger detection alone.
- Legacy `update.exe` is ignored unless its path/command line points to Teams.
- Window samples are bounded.
- The frontend re-arms auto-start only after the meeting disappears for several
  consecutive polls.

## Windows Smoke Checklist

1. Start a dev build:

   ```powershell
   cd frontend
   pnpm run tauri:dev:cpu
   ```

2. With Teams closed, call the dev helper if available:

   ```js
   await window.__clawscribeTeamsDetection.printStatus()
   ```

   Expected: `detected=false`, no Teams processes, action `idle`.

3. Open Teams but do not join a meeting.

   Expected: usually `possible`, not detected; title signal not satisfied.

4. Join a Teams desktop meeting.

   Expected: `detected=true` when a visible meeting-like title is present and
   confidence crosses threshold.

5. Test browser Teams in Edge or Chrome.

   Expected: browser process plus Teams meeting title can detect.

6. Test `prompt` and `auto` modes from settings.

   Expected: prompt mode asks; auto mode starts once and does not repeatedly
   restart while the same meeting remains detected.

7. Leave the meeting.

   Expected: status returns to `possible` or `notDetected`; auto mode re-arms
   only after consecutive not-detected polls.

If recording starts while mode is `off`, treat it as a blocker.
