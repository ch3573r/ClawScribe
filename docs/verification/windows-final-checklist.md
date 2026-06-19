# ClawScribe Windows Final Checklist

Date: 2026-06-15
Branch: `feat/clawscribe-productization-auth-theme-exports`
Scope: Windows manual release verification

This checklist must be run on a real Windows desktop session before claiming
Windows runtime readiness. A Linux build, Linux Tauri check, or code inspection
does not verify Windows launch, WebView2, Teams window detection, WASAPI audio,
Microsoft login, OneNote export, Planner export, installer behavior, or OpenClaw
handoff from the recorder.

Do not run a GitHub Windows installer workflow for this checklist unless the
coordinator or Alex explicitly asks for a release checkpoint build.

## Test Setup

Record these values before testing:

```text
Date:
Tester:
Machine:
Windows version:
ClawScribe commit:
ClawScribe version:
Upstream base version:
Build date:
Installer source:
Artifact link:
SHA256SUMS link:
BUILD-METADATA contents:
Installer format: MSI / NSIS / dev build
Teams desktop version:
Edge version:
Chrome version:
WebView2 runtime version:
OpenClaw ingest host:
Microsoft test tenant/account:
OneNote destination:
Planner plan/bucket:
```

Use test meetings, test notebooks, and test Planner plans only. Redact meeting
titles, participant names, bearer tokens, API keys, OAuth codes, and Microsoft
access tokens from screenshots and shared logs.

## Launch And Install

- Install the candidate MSI or NSIS package, or run the agreed dev build.
- Confirm Windows lists the app as `ClawScribe`.
- Confirm publisher/manufacturer is `OpenClaw` where Windows exposes it.
- Launch `ClawScribe` from the Start menu.
- Confirm only one primary app window opens.
- Quit and relaunch from the Start menu.
- Confirm no stale `Meetily` app name appears in the window title, tray tooltip,
  Start menu entry, installed-apps entry, or notifications, except where a page
  clearly identifies upstream Meetily attribution or previous-install migration.
- Confirm app logs do not print API keys, bearer tokens, OAuth codes, or raw
  Microsoft tokens.

Result:

```text
Pass/Fail:
Notes:
```

## About And Branding

- Open About.
- Confirm the primary product name is `ClawScribe`.
- Confirm the displayed version matches the candidate release.
- Confirm upstream attribution says it is based on Meetily Community Edition
  and does not present this build as an official upstream Meetily release.
- Confirm the candidate metadata preserves upstream base version `0.4.0`.
- Confirm links open externally and do not break the app session.
- Confirm the About content is readable in light and dark mode.

Result:

```text
Pass/Fail:
Displayed version:
Build metadata version:
Build metadata upstream base version:
Unexpected strings:
Notes:
```

## Light And Dark Theme

- Start with Windows app mode set to light, launch ClawScribe, and confirm the
  app is readable and controls have usable contrast.
- Switch ClawScribe to dark mode if an in-app control exists; otherwise switch
  Windows app mode to dark and relaunch.
- Confirm dialogs, settings, About, meeting details, transcript, summary,
  action items, menus, and toasts remain readable.
- Switch back to light mode and confirm no mixed-theme panels remain.
- Confirm focus rings and disabled states remain visible.

Result:

```text
Pass/Fail:
Theme control tested:
Notes:
```

## OpenAI Auth, Codex Login, And API-Key Fallback

ClawScribe must not claim private/raw OpenAI OAuth. The supported interactive
OpenAI login path is Codex-managed login; the direct fallback is OpenAI Platform
API-key auth.

- Open model/provider settings.
- Confirm `Advanced: Codex app-server` is a standalone advanced provider and
  does not ask for global `codex.exe`, PATH discovery, or WindowsApps.
- Confirm OpenAI direct auth asks for an OpenAI Platform API key, not ChatGPT web
  login.
- Confirm any OpenAI OAuth/PKCE text is clearly marked unsupported for direct
  API authentication.
- Save a test API key.
- Reload settings and confirm the UI indicates key presence without exposing the
  full key unless the user explicitly reveals an input field.
- Generate a summary from a short test recording using the OpenAI provider.
- Remove or replace the key and confirm the app returns to a usable fallback
  state with a clear message.
- Check logs for provider/model/status only; no full API key may appear.

Result:

```text
Pass/Fail:
Provider path:
Summary generated:
Secrets observed in logs:
Notes:
```

## OpenClaw Managed Auth Provider

- Configure `openclaw.json` or environment variables according to
  `docs/openclaw-handoff.md`.
- Set the bearer token through a user environment variable or local secure test
  config. Do not paste the real token into shared notes.
- From the Windows recorder, run:

  ```powershell
  Invoke-RestMethod http://openclaw.local:8765/readyz
  ```

- Launch ClawScribe, select `OpenClaw managed auth`, and refresh handoff status.
- Confirm the status says the bearer token is present without displaying it.
- Generate a summary through the OpenClaw managed provider.
- Confirm no ChatGPT/Codex token is stored in ClawScribe.
- Confirm logs do not expose the handoff bearer token.

Result:

```text
Pass/Fail:
Ready endpoint result:
Summary generated:
Secrets observed in logs:
Notes:
```

## Microsoft Login

Microsoft auth/export verification is pending until implementation lands. Do not
mark this section pass because design docs exist.

When a Microsoft login implementation is available:

- Sign in with a test Microsoft account.
- Confirm the app requests only the scopes needed for the selected export path.
- Cancel the Microsoft consent prompt and confirm recording, summary, OpenAI,
  and OpenClaw flows still work.
- Complete sign-in and confirm connection status is visible.
- Expire or revoke the session and confirm the app asks the user to reconnect
  without retrying indefinitely.
- Confirm no Microsoft access token, refresh token, auth code, or full
  authorization URL appears in logs, app state, screenshots, or exported files.

Result:

```text
Pass/Fail/Not implemented:
Scopes observed:
Secrets observed in logs:
Notes:
```

## Teams Dry Run

Use `docs/verification/teams-detection.md` for the detailed matrix. This dry run
must not start recording automatically.

- Close Teams desktop and browser Teams, then confirm idle detection.
- Open Teams desktop outside a meeting and confirm it does not detect an active
  meeting.
- Join a Teams desktop test meeting and confirm detection recommends only
  `promptToRecord`.
- Join Teams from Edge and Chrome and confirm browser detection.
- Disable the detector config and confirm status is disabled.
- Confirm `recordingSafety.automaticRecordingAllowed=false` in every positive
  detection case.

Result:

```text
Pass/Fail:
Matrix rows completed:
Unexpected recording side effects:
Notes:
```

## Recording

- Select microphone and system-audio devices.
- Start a short test recording from the normal UI control.
- Speak through the microphone and play local system audio.
- Stop the recording.
- Confirm the UI does not freeze during start, stop, or artifact finalization.
- Confirm a new recording folder appears under the configured recording root.
- Confirm transcript content is non-empty and plausibly aligned with the test
  audio.
- Confirm a summary and action items can be generated from the recording.

Result:

```text
Pass/Fail:
Recording folder:
Transcript non-empty:
Summary non-empty:
Notes:
```

## Artifacts

For installer artifact bundles, verify checksums before installing:

```powershell
cd <downloaded-artifact-root>
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $parts = $_ -split '\s+', 2
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $parts[1]).Hash.ToLowerInvariant() -ne $parts[0]) {
        throw "Checksum mismatch: $($parts[1])"
    }
}
Get-Content .\BUILD-METADATA.txt
```

Confirm `BUILD-METADATA.txt` includes product `ClawScribe`, the candidate
version, upstream base version `0.4.0`, build commit, and UTC build date.

Inspect the completed recording folder.

Required core artifacts:

- `metadata.json`
- `transcripts.json`
- audio artifact produced by the recorder

Expected after summary/export/handoff as applicable:

- summary content visible in the app
- action items visible in the app
- `.openclaw-pending.json`, `.openclaw-submitted.json`, or
  `.openclaw-failed.json`
- `exports.json` only after Microsoft export implementation lands

Checks:

- JSON files parse successfully.
- Transcript is not effectively empty.
- Metadata identifies ClawScribe source where appropriate.
- Compatibility names such as `meetily-json-v1`, `meetily-recordings`, and
  `openclaw.meetily-submission*.v1` remain acceptable only where documented for
  backward compatibility.
- No secrets are written into artifact files.

Result:

```text
Pass/Fail:
Installer checksum verified:
Build metadata verified:
Artifacts present:
Secrets observed:
Notes:
```

## OpenClaw Handoff

Before testing from Windows, verify the OpenClaw host separately:

```bash
cd /path/to/openclaw-ingest
curl -sS http://127.0.0.1:8765/readyz
sudo scripts/smoke-openclaw-bridge.py
```

From Windows:

- Confirm `Invoke-RestMethod http://openclaw.local:8765/readyz` succeeds.
- Enable OpenClaw handoff in ClawScribe.
- Record a short meeting.
- Stop recording and wait for finalization.
- Confirm `.openclaw-pending.json` is replaced by `.openclaw-submitted.json`.
- Confirm a duplicate manual retry does not double-submit an already submitted
  folder.
- Confirm OpenClaw ingest output contains non-empty transcript content and the
  expected meeting metadata.
- Confirm failure mode writes `.openclaw-failed.json` with a sanitized error
  when the endpoint or token is intentionally wrong.

Result:

```text
Pass/Fail:
Submission marker:
Ingest output path:
Transcript non-empty in ingest:
Failure marker tested:
Notes:
```

## OneNote Export

OneNote export cannot pass until Microsoft Graph export implementation exists.
Use `docs/integrations/onenote-export.md` and
`docs/productization/microsoft-export-test-plan.md` when it lands.

When available:

- Connect Microsoft account.
- Export a recording summary to the default `sectionName=ClawScribe` route.
- Export to an explicit test section ID.
- Confirm the page has escaped title, summary, transcript excerpt, action items,
  and source metadata.
- Retry the same export and confirm no duplicate page is created unless the user
  explicitly requests a duplicate.
- Test 401, 403, 404, and 429 behavior with mock or test responses before using
  live tenant data.
- Confirm no Microsoft tokens are stored in `exports.json` or logs.

Result:

```text
Pass/Fail/Not implemented:
Page URL:
Duplicate behavior:
Secrets observed:
Notes:
```

## Planner Export

Planner export cannot pass until Microsoft Graph export implementation exists.
Use `docs/integrations/planner-export.md` and
`docs/productization/microsoft-export-test-plan.md` when it lands.

When available:

- Connect Microsoft account with the minimal Planner export scope.
- Select a test plan and bucket.
- Review action items before export.
- Create Planner tasks for selected reviewed actions.
- Confirm titles, notes, due dates, priority, and source links are mapped as
  expected.
- Retry the same export and confirm no duplicate tasks are created for succeeded
  items.
- Test partial failure and confirm succeeded task IDs are preserved while failed
  items can be retried individually.
- Confirm no Microsoft tokens are stored in `exports.json` or logs.

Result:

```text
Pass/Fail/Not implemented:
Plan:
Bucket:
Created task count:
Duplicate behavior:
Secrets observed:
Notes:
```

## Release Decision

Use this language in the release report:

- "Windows runtime verification passed on <date> using <commit>" only after this
  checklist is completed on Windows.
- "Linux-side frontend/Rust checks passed" only for commands actually run on
  Linux.
- "Microsoft Graph export not implemented" or "pending runtime verification"
  until live or mock-backed export tests pass.
- "OpenAI API-key auth verified" only after a real summary is generated without
  logging the key.
- "Codex login verified" only after `docs/verification/codex-auth-windows.md`
  passes on Windows with a real Codex login.
- "OpenClaw handoff verified" only after the Windows recorder produces a
  submitted marker and OpenClaw ingest receives non-empty content.

Overall result:

```text
Pass/Fail/Blocked:
Release blockers:
Required reruns:
Approver:
```
