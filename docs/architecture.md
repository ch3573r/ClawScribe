# ClawScribe Architecture

ClawScribe is a local-first desktop meeting recorder built with Tauri 2, Rust,
Next.js, and local model runtimes. The supported product runtime is the Tauri
desktop app under `frontend/`. The legacy Python/FastAPI backend is no longer
part of the repository.

## Runtime Shape

```text
Next.js / React UI
        |
        | Tauri commands and events
        v
Rust app core
  - recording and import pipeline
  - local transcription engines
  - summary providers
  - Microsoft / Atlassian exports
  - updater, tray, settings, and credential storage
        |
        v
Local files, SQLite/settings, OS credential store, optional external providers
```

The app records from the local user session. It does not join meetings as a bot.
Meeting data is stored locally in a Meetily-compatible folder layout so older
recordings and migration paths continue to work.

## Main Modules

- `frontend/src/`: React UI, settings, meeting detail views, export dialogs,
  update UI, Teams/calendar panels, and client-side state.
- `frontend/src-tauri/src/audio/`: recording, import, audio conversion, device
  handling, and live transcription orchestration.
- `frontend/src-tauri/src/parakeet_engine/`: Parakeet ONNX model catalog,
  downloads, validation, and inference.
- `frontend/src-tauri/src/nemotron_engine/`: Nemotron 3.5 ASR streaming ONNX
  model catalog, feature extraction, DirectML/CPU loading, and RNN-T decoding.
- `frontend/src-tauri/src/summary/`: summary generation providers, including
  built-in/local, API-based, OpenClaw, and Codex app-server paths.
- `frontend/src-tauri/src/exports/`: Microsoft Graph auth, calendar lookup,
  OneNote export, Planner export, idempotency, and testable Graph transport.
- `frontend/src-tauri/src/exports/confluence.rs`: Confluence direct publish and
  credential handling.
- `frontend/src-tauri/src/teams_detection.rs`: local Windows Teams meeting
  detection using process/window evidence.
- `llama-helper/`: local summary sidecar helper used by built-in/local paths.

## Data Boundaries

- Transcription is local unless a user explicitly selects a cloud transcription
  provider.
- Summary generation can be local or external depending on the configured
  provider.
- Microsoft Graph is used only after Microsoft sign-in and only for calendar,
  OneNote, and Planner workflows.
- Confluence direct publish uses the configured server URL and PAT. Browser
  draft export remains available when SSO, proxy, or tenant policy blocks REST.
- OpenClaw handoff is optional and sends completed recording artifacts only to
  the configured operator endpoint.

## Compatibility Boundaries

Some internal names, folders, and environment variables still use Meetily names.
That is intentional compatibility debt for existing recordings and deployments,
not product branding.

Windows is the primary release target. Linux/macOS build paths may exist because
of the upstream Tauri app and model libraries, but release validation currently
focuses on Windows installers and GPU paths.
