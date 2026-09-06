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
  OneNote export, Planner/To Do task export, idempotency, and testable Graph
  transport.
- `frontend/src-tauri/src/exports/confluence.rs`: Confluence direct publish and
  credential handling.
- `frontend/src-tauri/src/teams_detection.rs`: local Windows Teams meeting
  detection using process/window evidence.
- `llama-helper/`: local summary sidecar helper used by built-in/local paths.

## Data Boundaries

- Transcription is local unless a user explicitly selects a cloud transcription
  provider.
- Hosted Whisper uses OpenAI-compatible file transcription. The official
  OpenAI endpoint currently limits uploads to 25 MB; larger uploads are reported
  as a size-limit fallback and transcribed locally.
- MAI-Transcribe uses Azure Speech Fast Transcription with its own Cognitive
  Services key or Azure Speech credential. Microsoft Graph sign-in/export
  scopes are not reused for Azure Speech.
- MAI sentence-level timing is never promoted to word timestamps. If Azure
  returns a combined-only or single-phrase transcript, ClawScribe can remap the
  cloud text onto the local VAD speech grid for readable rows, but those row
  timings are approximate and speaker diarization stays on the conservative
  path.
- Cloud transcription calls upload the whole recording file once. They are not
  split into per-VAD-segment provider calls.
- Summary generation can be local or external depending on the configured
  provider.
- Microsoft Graph is used only after Microsoft sign-in and only for calendar,
  OneNote, Planner, and To Do workflows.
- Confluence direct publish uses the configured server URL and PAT. Browser
  draft export remains available when SSO, proxy, or tenant policy blocks REST.
- OpenClaw handoff is optional and sends completed recording artifacts only to
  the configured operator endpoint.

## Recording And Inference Lifetimes

`audio/inference.rs` serializes recording, batch, diarization and model-changing jobs and holds a
separate native-call permit inside the blocking task. Cancelling the async caller
cannot free a model that native code still uses. `audio/batch_audio.rs` normalizes
imports/retranscription to temporary PCM on disk, uses one continuous VAD state,
and reads one bounded speech segment for inference. Preparation needs temporary
disk capacity; it does not retain a full decoded meeting in RAM.

`audio/transcription/queue.rs` serves both live recognition and the retained
recording spool. `audio/audio_spool.rs` recovers mixed float PCM into a playable
WAV. `audio/outcome.rs` persists recording failures before the completion event;
meeting/transcript/outcome database writes share a transaction. New schema is
added through a migration; shipped migrations remain unchanged.

`credentials.rs` supplies protected, provider-scoped references for legacy
settings columns. Optional providers can fail independently without preventing
local recording. `summary/context_budget.rs` bounds hierarchical reduction, and
`summary/chat_context.rs` selects question-relevant evidence from across the
meeting instead of silently discarding its middle.

## Compatibility Boundaries

Some internal names, folders, and environment variables still use Meetily names.
That is intentional compatibility debt for existing recordings and deployments,
not product branding.

Windows is the primary release target. Linux/macOS build paths may exist because
of the upstream Tauri app and model libraries, but release validation currently
focuses on Windows installers and GPU paths.
