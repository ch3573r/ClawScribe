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
| Version | `0.139.0` |
| Target | `x86_64-pc-windows-msvc` |
| Source package | `@openai/codex@0.139.0-win32-x64` |
| Source URL | `https://registry.npmjs.org/@openai/codex/-/codex-0.139.0-win32-x64.tgz` |
| Source SHA256 | `99698e69d6acf91c75703669fdfd00d54f4b249beabc7d32a03404e8c2c3b2c7` |
| Runtime SHA256 | `77a84f8078400467ade4301d827b8bcea2d29b6838c9cd162bf3573b7ef97e10` |
| License | Apache-2.0 |
| Build date | 2026-06-15 |
| Tauri sidecar path | `frontend/src-tauri/binaries/codex-app-server-x86_64-pc-windows-msvc.exe` |

The Windows release workflow stages this runtime with
`frontend/scripts/stage-codex-runtime.ps1`, verifies the NPM tarball SHA256,
verifies the executable SHA256, and writes
`frontend/src-tauri/binaries/codex-app-server-runtime.json`.

## Runtime Rules

- Use Tauri `bundle.externalBin` for the app-server sidecar.
- Launch only the ClawScribe-bundled sidecar from the app install/resource path.
- Do not use a global `codex.exe`, `PATH` discovery, Microsoft Store Codex,
  WindowsApps package internals, or user-browsed executables.
- If the bundled runtime is missing or its SHA256 does not match, show:
  `Bundled Codex runtime is missing or damaged. Repair/reinstall ClawScribe.`

## Auth And State

ClawScribe always sets an isolated `CODEX_HOME` for the sidecar:

```text
%APPDATA%\ClawScribe\codex
```

ClawScribe does not read or write the user's normal `~/.codex` profile and does
not reuse the user's standalone Codex CLI auth state.

## Protocol

The provider uses Codex app-server over stdio JSONL:

1. Spawn bundled sidecar with `app-server`.
2. Send `initialize`.
3. Send `initialized`.
4. Use `account/read`, `account/login/start`, and `account/logout` for auth.
5. Use `thread/start` and `turn/start` for meeting processing.
6. Retry JSON-RPC overload errors with bounded backoff.
7. Convert auth failures to a typed re-auth prompt.

Secrets, auth headers, token-looking values, and transcript content are redacted
or omitted from debug logs by default.
