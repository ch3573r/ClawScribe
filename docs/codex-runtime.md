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
| Version | `0.159.2` |
| Target | `x86_64-pc-windows-msvc` |
| Source package | `@openai/codex@0.159.2-win32-x64` |
| Source URL | `https://registry.npmjs.org/@openai/codex/-/codex-0.159.2-win32-x64.tgz` |
| Source SHA256 | `a71d5560d56189969350cf42c0121b57d69ae88405fcdab89fc3fa3d704f6bb6` |
| Runtime SHA256 | `52f75c649bebb8001102a1dd129c1ea6d02b0940321e6d7e82ee0526753bd58a` |
| License | Apache-2.0 |
| Build date | 2026-09-30 |
| Tauri sidecar path | `frontend/src-tauri/binaries/codex-app-server-x86_64-pc-windows-msvc.exe` |

The Windows release workflow stages this runtime with
`frontend/scripts/stage-codex-runtime.ps1`, verifies the NPM tarball SHA256,
verifies the executable SHA256, and writes
`frontend/src-tauri/binaries/codex-app-server-runtime.json`.

The bundled catalog includes GPT-6.1 Sol (`gpt-6.1-sol`). Settings reads the live
`model/list` catalog and follows pagination; saved model choices are preserved.
Account and workspace access still determine which models can run. Choose
**Check bundled runtime** after installing an updated runtime to refresh models.

Both Windows release preflight and installer builds run
`frontend/scripts/test-codex-runtime.mjs` on the designated runner. It checks the
staged executable hash, initialization, signed-out account/logout behavior, and
the picker-visible GPT-6.1 Sol entry using a fresh keyring-only profile. Native
provider regressions cover summary output, chat reconciliation, auth failures,
and fallback behavior. Authenticated sign-in, summaries, and meeting chat with
the new runtime still require an installed-app check before stable promotion.

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

The provider does not reuse the user's normal `~/.codex` profile or standalone
Codex CLI sign-in. Existing-user session mode is disabled; legacy mode flags are
normalized to the isolated profile.

## Protocol

The provider uses Codex app-server over stdio JSONL:

1. Spawn bundled sidecar with `app-server`.
2. Send `initialize`.
3. Send `initialized`.
4. Use `account/read`, `account/login/start`, and `account/logout` for auth.
5. Use `model/list` to populate the Summary model picker.
6. Use `thread/start` and `turn/start` for meeting processing. Meeting turns send
   `outputSchema` to constrain the final message; raw prompts do not.
7. Retry JSON-RPC overload errors with bounded backoff.
8. Convert auth failures to a typed re-auth prompt.

Secrets, auth headers, token-looking values, and transcript content are redacted
or omitted from debug logs by default.

Summary threads and turns explicitly set their scratch working directory,
read-only sandbox and `never` approval policy. Shell and unified execution are
disabled through thread configuration; server approval requests are declined.
The fields follow the pinned [0.159.2 thread protocol](https://github.com/openai/codex/blob/rust-v0.159.2/codex-rs/app-server-protocol/schema/json/v2/ThreadStartParams.json)
and [turn protocol](https://github.com/openai/codex/blob/rust-v0.159.2/codex-rs/app-server-protocol/schema/json/v2/TurnStartParams.json).

Each run owns a temporary scratch folder for prompts and transcript input.
Cleanup runs after success, failure, or cancellation and retries transient removal
failures. Output documents and processing logs are saved in the meeting's
recording folder, or app data `meeting-outputs/<id>` when it has no recording
folder. Meeting deletion removes app-data outputs and legacy scratch runs, with
failures reported. Outputs inside a recording folder follow the recording-file
deletion choice and its ownership guards; keeping that folder keeps those outputs.

The isolated profile requires `cli_auth_credentials_store = "keyring"`, supported
by the [pinned credential-storage implementation](https://github.com/openai/codex/blob/rust-v0.159.2/codex-rs/login/src/auth/storage.rs).
Existing isolated profiles receive this setting while retaining other configuration.
Users with an older file-based sign-in sign in once again; the old `auth.json`
is removed only after a keyring-backed account is available. Keyring failures
do not fall back to plaintext.
When that legacy file is present and the account is signed out, the sign-in
message explains the move to Windows Credential Manager. The check uses only
file existence, without reading the old credentials.
