# Codex Runtime

ClawScribe bundles Codex only for the `Advanced: Codex app-server` provider.
Normal meeting processing must continue to work through OpenAI/OpenAI-compatible
API keys and OpenClaw without any Codex runtime.

## Provider Order

1. OpenAI / OpenAI-compatible API key
2. OpenClaw
3. Advanced: Codex app-server

## Bundled Runtime

| Field | Value |
| --- | --- |
| Runtime | Codex app-server |
| Version | `0.157.0` |
| Target | `x86_64-pc-windows-msvc` |
| Source package | `@openai/codex@0.157.0-win32-x64` |
| Source URL | `https://registry.npmjs.org/@openai/codex/-/codex-0.157.0-win32-x64.tgz` |
| Source SHA256 | `fe71b497453d7a5372842f50a8b737668dc0755624713ba8a9093f09419b79b8` |
| Runtime SHA256 | `ed1c7b36e44536809c868864c833af8a857f56599a7a7fe23b908a1ba1093b1f` |
| License | Apache-2.0 |
| Build date | 2026-09-25 |
| Tauri sidecar path | `frontend/src-tauri/binaries/codex-app-server-x86_64-pc-windows-msvc.exe` |

The Windows release workflow stages this runtime with
`frontend/scripts/stage-codex-runtime.ps1`, verifies the NPM tarball SHA256,
verifies the executable SHA256, and writes
`frontend/src-tauri/binaries/codex-app-server-runtime.json`.

The optional `codex-resources/voice` host from the NPM package is not staged.
ClawScribe only runs text turns, and the voice host bundles separately licensed
GStreamer/GLib and Microsoft Visual C++ runtime libraries.

## Runtime Rules

- Use Tauri `bundle.externalBin` for the app-server sidecar.
- Launch only the ClawScribe-bundled sidecar from the app install/resource path.
- Do not use a global `codex.exe`, `PATH` discovery, Microsoft Store Codex,
  WindowsApps package internals, or user-browsed executables.
- If the bundled runtime is missing or its SHA256 does not match, show:
  `Bundled Codex runtime is missing or damaged. Repair/reinstall ClawScribe.`

## Auth And State

ClawScribe defaults to an isolated `CODEX_HOME` for the sidecar:

```text
%APPDATA%\ClawScribe\codex
```

The default mode does not read or write the user's normal `~/.codex` profile or
reuse standalone Codex CLI auth state. Advanced existing-session mode requires
explicit opt-in and the MCP check described below.

## Protocol

The provider uses Codex app-server over stdio JSONL:

1. Spawn bundled sidecar with `app-server`.
2. Send `initialize`.
3. Send `initialized`.
4. Use `account/read`, `account/login/start`, and `account/logout` for auth.
5. Use `model/list` to populate the Summary model picker.
6. Use `thread/start` and `turn/start` for meeting processing.
7. Retry JSON-RPC overload errors with bounded backoff.
8. Convert auth failures to a typed re-auth prompt.

Secrets, auth headers, token-looking values, and transcript content are redacted
or omitted from debug logs by default.

Summary threads and turns explicitly set their scratch working directory,
read-only sandbox and `never` approval policy. Shell and unified execution are
disabled through thread configuration; server approval requests are declined.
The fields follow the pinned [0.157 thread protocol](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/app-server-protocol/schema/json/v2/ThreadStartParams.json)
and [turn protocol](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/app-server-protocol/schema/json/v2/TurnStartParams.json).

In the advanced existing-session mode, ClawScribe checks the effective config
for the summary's working directory before starting a thread. If it defines MCP
servers, summaries require switching to the isolated profile. A config-read
failure also prevents submission. An empty `mcp_servers` override cannot disable
inherited servers because the pinned [0.157 config merger](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/config/src/merge.rs)
merges tables recursively; the check uses the pinned [config/read implementation](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/app-server/src/config_manager_service.rs).

The isolated profile requires `cli_auth_credentials_store = "keyring"`, supported
by the [pinned credential-storage implementation](https://github.com/openai/codex/blob/rust-v0.157.0/codex-rs/login/src/auth/storage.rs).
Existing profiles receive this setting while retaining other configuration.
Users with an older file-based sign-in sign in once again; the old `auth.json`
is removed only after a keyring-backed account is available. Keyring failures
do not fall back to plaintext.
