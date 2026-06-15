# Codex App-Server Auth

Codex is an advanced standalone ClawScribe provider, not the default processing
path and not an OpenClaw dependency.

Provider order:

1. OpenAI / OpenAI-compatible API key
2. OpenClaw
3. Advanced: Codex app-server

Normal ClawScribe meeting processing must work without any Codex runtime.

## Boundary

ClawScribe must not implement DIY OpenAI/Codex OAuth, copy Codex tokens, shape
private Codex backend traffic, or depend on a global `codex.exe`.

The supported boundary is the official Codex app-server runtime:

- bundled/pinned platform-specific runtime, or controlled first-run installer
- JSON-RPC over stdio / JSONL
- isolated ClawScribe `CODEX_HOME`
- app-server account/thread/turn RPC methods

ClawScribe must not launch or suggest:

- `%LOCALAPPDATA%\Microsoft\WindowsApps\codex.exe`
- Microsoft Store app internals
- user-browsed Codex executables
- `codex` from `PATH`
- the user's normal `~/.codex` profile

## Provider Shape

```json
{
  "processing": {
    "provider": "codex",
    "codex": {
      "codexHomeMode": "clawscribe-isolated",
      "codexHomePath": "%APPDATA%\\ClawScribe\\codex",
      "useExistingUserCodexSession": false,
      "model": "gpt-5.1-codex",
      "timeoutSeconds": 600
    }
  }
}
```

`useExistingUserCodexSession` is retained only for backward config
compatibility. Runtime normalization forces an isolated ClawScribe-owned
`CODEX_HOME`.

## App-Server Flow

Startup:

```text
resolve bundled/pinned runtime
start Codex app-server with stdio transport
send initialize
send notifications/initialized
send account/read
```

Login:

```text
account/login/start { "type": "chatgpt" }
```

Device-code login:

```text
account/login/start { "type": "chatgptDeviceCode" }
```

The UI must surface `verificationUrl` and `userCode` when the device-code flow
returns them, then listen for:

- `account/login/completed`
- `account/updated`
- progress notifications
- auth failures
- rate-limit or overload events

Logout:

```text
account/logout
```

Meeting processing:

```text
thread/start
turn/run
```

Each meeting must use a fresh thread/turn. The only meeting content sent to
Codex is the normalized transcript, metadata, and strict output instructions.

## Output Contract

Codex app-server output must match the other providers:

- `meeting-output.json`
- `meeting-notes.md`
- `follow-up-email.md`
- `processing-log.json`

Malformed structured output must fail loudly. Never silently accept malformed
JSON.

## Security

- Use isolated `%APPDATA%\ClawScribe\codex` by default.
- Do not read or write the user's global `~/.codex`.
- Do not share auth state with standalone Codex CLI.
- Prefer OS credential storage if the app-server runtime supports it.
- Redact access tokens, refresh tokens, bearer strings, auth files, API keys,
  and full command environments from logs and UI output.

## Packaging

The Codex provider is valid only when ClawScribe can resolve its own pinned
app-server runtime. If no runtime is bundled yet, Settings must show:

```text
runtime not installed
```

with a controlled repair/install action. OpenAI API key and OpenClaw must remain
fully usable when the Codex runtime is missing.

## Current Implementation Status

The product-facing direction has been switched to `Advanced: Codex app-server`.
CLI discovery, WindowsApps suggestions, user-browsed `codex.exe`, global
`PATH`, existing-user Codex sessions, and `codex exec` fallback are disabled for
the provider surface.

Until the pinned app-server runtime is bundled or a controlled first-run
installer is implemented, the Codex provider reports runtime-not-installed and
normal processing should use OpenAI / OpenAI-compatible API or OpenClaw.
