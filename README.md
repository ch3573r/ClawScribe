# ClawScribe

![ClawScribe README hero](docs/brand/clawscribe-readme-hero.png)

ClawScribe is a local-first desktop app for recording meetings, calls,
interviews, and long-form audio. It captures microphone and system audio from
your own session, transcribes speech on device, and turns the result into
meeting notes, summaries, action items, and export-ready artifacts.

Current version: `0.5.16`

ClawScribe is based on Meetily Community Edition `0.4.0`. Upstream attribution
and license details are in [UPSTREAM.md](UPSTREAM.md), [NOTICE.md](NOTICE.md),
and [LICENSE.md](LICENSE.md).

## Feature Set

### Capture And Meeting Workflow

- Bot-free desktop capture from the local Windows session.
- Microphone and system-audio recording through the native desktop app.
- Live transcription while recording, plus import/retranscription for existing
  audio and video files.
- Supported import formats include MP4, M4A, WAV, MP3, FLAC, OGG, AAC, MKV,
  WebM, and WMA.
- Recovery path for interrupted recordings so captured transcript fragments are
  not silently lost.

### Local Transcription

- Local Whisper model management through whisper.cpp/whisper-rs.
- Parakeet ONNX models for the fast path, including stock v3 int8,
  SmoothQuant int8, and v2 int8 variants.
- Nemotron 3.5 ASR multilingual ONNX models for broader language coverage,
  including fp16 and int8 variants.
- Windows DirectML acceleration for supported ONNX engines in GPU builds, with
  per-model validation and CPU fallback where the model supports it.
- Language selection and retranscription when a better model or language target
  is selected after import.

### AI Notes

- Meeting summaries generated from transcript and optional meeting context.
- Template-driven summaries, custom context prompts, and summary regeneration.
- Configurable summary providers: Built-in AI, Ollama, OpenAI API keys,
  OpenAI-compatible endpoints, OpenRouter, Anthropic/Claude, Groq, optional
  OpenClaw managed processing, and the advanced bundled Codex app-server path.
- Meeting chat against the selected meeting using the configured summary model.

### Calendar And Attendance

- Microsoft sign-in for calendar and export features.
- Current/next meeting detection from Microsoft calendar events.
- Teams meeting detection with user-controlled prompt or auto-record behavior.
- Meeting title seeding from the selected calendar event.
- Invited attendees can be prepended to the summary as a checklist so absentees
  can be unchecked or crossed out before export.

### Exports

- OneNote export through Microsoft Graph. The current safe path creates a fresh
  dated section for each export, avoiding Graph's 5,000-item section-listing
  limit on large OneDrive/SharePoint libraries.
- Planner task export through Microsoft Graph with review, edit, bucket
  selection, optional bucket creation, and duplicate protection.
- Microsoft To Do export through Microsoft Graph for reviewed personal action
  items, using the existing task permission scope.
- Confluence export through either browser draft handoff or direct REST publish
  with a personal access token.
- Optional OpenClaw handoff for deployments that ingest completed
  Meetily-format recording folders.

### Desktop App

- Dark and light themes.
- User-selectable accent color.
- Interface font picker.
- Custom title bar and tray behavior.
- Automatic update checks with a setting to disable launch-time checks.
- Local Meetily-compatible storage layout for migration and backward
  compatibility.

## Transcription Engines

| Engine | Current role |
| --- | --- |
| Parakeet | Default fast path. Ships stock v3 int8, SmoothQuant int8, and v2 int8 model options. DirectML builds can use GPU acceleration on supported Windows systems. |
| Nemotron | Beta multilingual path for NVIDIA Nemotron 3.5 ASR. Ships fp16 and int8 variants; fp16 is CPU-capable, while int8 is intended for DirectML-capable GPU builds. |
| Whisper | Broad compatibility path through whisper.cpp/whisper-rs with local model management. |

Model downloads are managed inside the app. The downloader validates expected
large-file sizes so CDN errors, partial downloads, and LFS pointer stubs do not
get treated as usable models.

## Microsoft 365 Integration

Microsoft integration uses an interactive Microsoft sign-in and stores export
tokens in the platform credential store.

- Calendar: read upcoming events, choose the meeting context, refresh event
  data, and carry invited attendees into the summary attendance checklist.
- Teams: detect active Teams meeting signals and either prompt or auto-start
  recording depending on the user's setting.
- OneNote: choose or create a notebook. Each export creates a fresh dated
  section and writes the summary plus transcript pages. This avoids the Graph
  section-listing limit that affects large OneDrive/SharePoint libraries.
- Planner: review parsed action items, edit titles/details, choose buckets, and
  export selected tasks. Re-exporting uses a local ledger to avoid duplicates.
- Microsoft To Do: review parsed action items, edit titles/notes, choose a To
  Do list, and export selected personal tasks. This uses the same
  `Tasks.ReadWrite` consent as Planner.

## Confluence Export

ClawScribe supports two Confluence paths:

- Browser draft: copy rich text and open Confluence in the user's existing
  browser session. Use this when SSO, App Proxy, or tenant policy blocks direct
  API access.
- Direct publish: create pages through a reachable self-hosted Confluence Server
  or Data Center REST endpoint with a personal access token.

## Product Status

ClawScribe is in active pre-RC development. The Tauri desktop app is the
supported runtime. The legacy Python/FastAPI backend has been removed from this
repository; current runtime work lives in the Tauri desktop app.

Current boundaries:

- Windows is the primary release target.
- Some compatibility names and environment variables still use Meetily naming
  to preserve existing data and integrations.
- Nemotron remains labeled beta while the DirectML and model-variant behavior is
  still being validated across hardware.
- Optional cloud AI and export providers must be configured by the user or
  operator. No private endpoint or credential is baked into the app.

## Repository Layout

```text
frontend/                  Next.js UI and Tauri desktop app
frontend/src-tauri/src/    Rust app core, audio, transcription, exports, summary
frontend/src/              React components, hooks, services, and app routes
docs/                      Product, build, verification, and integration notes
llama-helper/              Local summary sidecar helper
scripts/                   Repository utility scripts
```

## Development

Install frontend dependencies and run the Tauri app:

```bash
cd frontend
pnpm install
pnpm run tauri:dev
```

Run the web UI only:

```bash
cd frontend
pnpm run dev
```

Build the desktop app:

```bash
cd frontend
pnpm run tauri:build
```

Windows release validation and packaging:

```powershell
cd frontend
.\scripts\build-windows-release.ps1 -CheckOnly
.\scripts\build-windows-release.ps1
```

GPU-specific developer scripts live in `frontend/`:

```text
dev-gpu.*
build-gpu.*
```

Use those scripts for acceleration-path validation.

## Privacy And Credentials

ClawScribe records from the local user session and keeps transcription local
unless you configure an external summary or export provider.

Contributor rules:

- Do not commit `.env` files, tokens, API keys, bearer strings, certificates,
  local auth stores, logs, databases, generated installers, or local build tool
  installers.
- Commit placeholders only in `.env.example` files.
- Use example hosts such as `openclaw.local` or `example.com` in docs instead
  of private LAN addresses.
- Keep logs, tests, analytics, and error messages redacted.

## Support

ClawScribe remains MIT licensed and free to use, modify, and redistribute under
the terms in [LICENSE.md](LICENSE.md). To support development:

[Buy me a coffee](https://buymeacoffee.com/ch3573r)

## Build Documentation

- [docs/README.md](docs/README.md)
- [docs/BUILDING.md](docs/BUILDING.md)
- [docs/windows-release.md](docs/windows-release.md)
- [docs/GPU_ACCELERATION.md](docs/GPU_ACCELERATION.md)
- [frontend/README.md](frontend/README.md)

## License

ClawScribe remains under the MIT License. See [LICENSE.md](LICENSE.md).

Upstream Meetily code is copyright Zackriya Solutions and contributors.
ClawScribe changes are copyright OpenClaw contributors unless otherwise noted.
