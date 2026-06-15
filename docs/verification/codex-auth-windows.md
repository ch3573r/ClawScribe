# Codex App-Server Windows Verification

Use `docs/verification/alex-windows-codex-checklist.md` for the Alex-facing
release gate. This file keeps lower-level app-server checks.

## Preconditions

Codex is an advanced standalone provider:

1. OpenAI / OpenAI-compatible API
2. OpenClaw
3. Advanced: Codex app-server

Do not verify or ship a path that depends on global `codex`, `PATH`,
WindowsApps, copied tokens, or a user-browsed executable.

## Runtime

Expected:

- ClawScribe resolves a bundled/pinned app-server runtime, or reports
  `Bundled Codex runtime is missing or damaged. Repair/reinstall ClawScribe.`
- Missing runtime appears only in the Codex app-server panel.
- OpenAI/OpenAI-compatible processing works without Codex.
- OpenClaw processing works without Codex.
- `CODEX_HOME` is isolated under ClawScribe app data.
- The user's normal `~\.codex` is not used.

## Auth RPC

Verify the app-server receives:

```text
initialize
initialized
account/read
```

Browser login:

```json
{ "type": "chatgpt" }
```

Device-code login:

```json
{ "type": "chatgptDeviceCode" }
```

Expected:

- Device-code login surfaces `verificationUrl` and `userCode`.
- Completion is detected through app-server notifications.
- Account state is read through `account/read`.
- Logout uses `account/logout`.

## Processing RPC

Verify meeting processing uses:

```text
thread/start
turn/start
```

Expected output files:

- `meeting-output.json`
- `meeting-notes.md`
- `follow-up-email.md`
- `processing-log.json`

Expected behavior:

- valid structured JSON only
- prompt-injection guard included
- auth failure surfaces re-auth
- overload/transient failures use bounded retry/backoff
- secrets are redacted

## Rejection Checks

These are invalid runtime sources:

- `C:\Users\<user>\AppData\Local\Microsoft\WindowsApps\codex.exe`
- Store app internals
- `codex` from `PATH`
- manually browsed `codex.exe`
- standalone CLI auth from `~\.codex`

ClawScribe should reject them with a friendly explanation.
