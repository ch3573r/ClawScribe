# Alex Windows Codex App-Server Checklist

Status before this checklist:

> Codex is an advanced standalone app-server provider. It is not the default
> processing path and must not be required for OpenAI API key or OpenClaw
> processing.

Do not ship another Windows build where Codex means browsing to `codex.exe`,
using `codex` from `PATH`, using WindowsApps, or running `codex exec`.

## 1. Provider Order

Open **Settings -> AI Provider**.

Expected order:

1. `OpenAI / OpenAI-compatible API`
2. `OpenClaw`
3. `Advanced: Codex app-server`

Expected:

- OpenAI/OpenAI-compatible remains the normal default path.
- OpenClaw remains available.
- Missing Codex runtime does not block app startup.
- Missing Codex runtime does not block OpenAI/OpenClaw processing.

## 2. Codex App-Server Panel

Select:

```text
Advanced: Codex app-server
```

Expected text:

```text
Codex app-server mode uses a bundled/pinned Codex runtime and ChatGPT/Codex sign-in. It does not use the Microsoft Store app executable and does not require Codex to be installed globally. For normal use without Codex, choose OpenAI API key or OpenClaw.
```

Expected controls:

- Status: bundled runtime found / runtime not installed
- Runtime version/path when available
- Isolated `CODEX_HOME` location
- Sign in with ChatGPT
- Sign in with device code
- Logout
- Test Codex app-server
- Test meeting processing
- Rate-limit/account state if available

Expected not present:

- Browse for `codex.exe`
- Find `codex` on `PATH`
- Use existing Codex session
- WindowsApps path suggestions
- CLI install instructions as the normal path

## 3. Missing Runtime Behavior

If the pinned app-server runtime is not bundled yet, expected status:

```text
runtime not installed
```

Expected:

- The Codex panel shows a repair/install action.
- The repair/install action does not silently install anything.
- OpenAI/OpenAI-compatible processing still works.
- OpenClaw processing still works.

## 4. WindowsApps Rejection

If any old config points to a path like:

```text
C:\Users\<user>\AppData\Local\Microsoft\WindowsApps\codex.exe
```

Expected:

- ClawScribe rejects it with a friendly explanation.
- It does not launch it.
- It does not treat it as a valid runtime source.

## 5. App-Server Auth Flow

When the bundled runtime exists, verify:

```text
initialize
notifications/initialized
account/read
```

Browser login should use:

```json
{ "type": "chatgpt" }
```

Device-code login should use:

```json
{ "type": "chatgptDeviceCode" }
```

Expected:

- Device-code flow shows `verificationUrl` and `userCode`.
- Completion is driven by app-server notifications such as
  `account/login/completed` and `account/updated`.
- `account/read` shows signed-in state, email if returned, and plan type if
  returned.
- `account/logout` signs out.

## 6. Meeting Processing

When the bundled runtime exists, process a tiny synthetic transcript.

Expected app-server methods:

```text
thread/start
turn/run
```

Expected files:

- `meeting-output.json`
- `meeting-notes.md`
- `follow-up-email.md`
- `processing-log.json`

Expected:

- Valid structured JSON only.
- No invented owners or due dates.
- Prompt-injection guard remains active.
- Auth failures show a re-auth prompt.
- Overload/transient failures use bounded retry/backoff.
- No infinite retry loops.

## 7. Secret Hygiene

Search app output/log folders for:

```text
sk-
sk-proj-
access_token
refresh_token
Authorization: Bearer
auth.json
```

Expected:

- No raw tokens.
- No auth file contents.
- No full bearer strings.
- Redacted placeholders are acceptable.

## 8. Provider Switching

Verify:

1. OpenAI API provider works without Codex runtime.
2. OpenClaw provider works without Codex runtime.
3. Codex missing-runtime warning appears only when `Advanced: Codex app-server`
   is selected.
4. Switching back from Codex does not leave a global Codex error on other
   provider screens.
