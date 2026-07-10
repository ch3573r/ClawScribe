# Claude Code Handoff - 2026-07-04

This is the working handoff for returning to ClawScribe in Claude Code.
Treat it as an orientation document, not as the canonical product spec. When
anything here disagrees with live code or the linked docs, verify from the live
checkout first and update the relevant canonical doc.

## Current Snapshot

- Local checkout: `D:\AI\ClawScribe`
- Git remote: `https://github.com/ch3573r/ClawScribe.git`
- Current branch: `main`
- Current commit/tag: `39a0829`, `v0.5.32`
- Worktree status at handoff time: clean
- App/package version: `0.5.32`
- Installed local Windows app verified at:
  - `C:\Users\blyth\AppData\Local\ClawScribe\ClawScribe.exe`
  - `C:\Users\blyth\AppData\Local\ClawScribe\clawscribe.exe`
  - both report `ProductVersion 0.5.32`
- App logs and local DB, if runtime triage is needed:
  - log: `%LOCALAPPDATA%\net.rismondo.openclaw.clawscribe\logs\ClawScribe.log`
  - DB: `%APPDATA%\net.rismondo.openclaw.clawscribe\meeting_minutes.sqlite`

Do not assume the installed app, release assets, GitHub latest release, or
provider schemas are still current. They were verified on 2026-07-04 only where
explicitly stated above.

## Product In One Paragraph

ClawScribe is the Windows-first, local-first Tauri desktop recorder forked from
Meetily CE 0.4.0. The supported runtime is the Tauri app: Next.js/React UI in
`frontend/src`, Rust/Tauri core in `frontend/src-tauri/src`, local capture and
transcription by default, optional Microsoft exports, optional OpenClaw handoff,
optional cloud transcription beta, and optional external summary providers. The
legacy Python/FastAPI backend has been removed and should not be reintroduced
unless that direction is explicitly chosen.

## Start Here

- Agent rules and repo conventions: `CLAUDE.md`
- Product overview and layout: `README.md`
- Current release history: `CHANGELOG.md`
- Docs index: `docs/README.md`
- Windows build/release process: `docs/windows-release.md`
- OpenClaw recorder-to-ingest handoff: `docs/openclaw-handoff.md`
- Hosted transcription live smoke: `docs/hosted-transcription-smoke.md`
- Bundled Codex app-server notes: `docs/codex-runtime.md`
- OpenAI/OpenClaw auth design notes: `docs/openai-oauth.md`
- Microsoft export docs:
  - `docs/integrations/microsoft-graph.md`
  - `docs/integrations/onenote-export.md`
  - `docs/integrations/planner-export.md`
  - `docs/integrations/todo-export.md`

## Repository Map

- `frontend/package.json`: Node scripts, app version, frontend dependencies.
- `frontend/src/app/`: Next.js route surfaces. Current app screens include
  home, meetings, meeting details, notes, and settings.
- `frontend/src/components/`: UI components and workflows. Important surfaces:
  - `ModelSettingsModal.tsx`: summary provider settings, OpenClaw fields, Codex
    provider state.
  - `TranscriptSettings.tsx`: transcription provider settings, cloud beta UI,
    hosted-provider test button.
  - `MeetingDetails/`: summary, transcript, speaker diarization, export, and
    retranscription workflows.
  - `UpdateDialog.tsx`, `UpdateNotification.tsx`, `UpdateCheckProvider.tsx`:
    updater UI.
- `frontend/src/hooks/`: UI orchestration hooks. Important areas:
  - `useCloudTranscription.ts`
  - `useMicrosoftExport.ts`
  - `meeting-details/*`
  - `useUpdateCheck.ts`
- `frontend/src/lib/`: pure TypeScript helpers for summaries, Markdown,
  meeting context, cloud transcription UI state, analytics, etc.
- `frontend/src/services/`: frontend service wrappers around Tauri commands and
  browser/local storage.
- `frontend/src-tauri/src/lib.rs`: command registration, app state, plugin setup.
- `frontend/src-tauri/src/audio/`: capture, recording, import, VAD,
  retranscription, device handling, and local/cloud transcription orchestration.
- `frontend/src-tauri/src/audio/transcription/`: local transcription provider
  abstraction and provider implementations.
- `frontend/src-tauri/src/audio/transcription/cloud/`: Hosted Whisper and
  MAI-Transcribe beta implementation.
- `frontend/src-tauri/src/parakeet_engine/`: Parakeet ONNX transcription.
- `frontend/src-tauri/src/nemotron_engine/`: Nemotron ASR beta path.
- `frontend/src-tauri/src/whisper_engine/`: Whisper local model path.
- `frontend/src-tauri/src/summary/`: summary providers, Codex app-server bridge,
  templates, language handling, chat commands.
- `frontend/src-tauri/src/exports/`: Microsoft Graph, Confluence, OneNote,
  Planner, To Do, file/document export logic.
- `frontend/src-tauri/src/database/`: SQLx setup, models, commands, repositories,
  migrations.
- `frontend/src-tauri/src/notifications/`: app/system notification handling.
- `frontend/src-tauri/templates/`: built-in summary templates.
- `frontend/src-tauri/migrations/`: SQLite migrations. Be careful with checksum
  changes after a release; installed clients track applied SQLx migrations.
- `frontend/scripts/`: Windows build, runtime staging, release helper scripts.
- `.github/workflows/clawscribe-windows-release.yml`: self-hosted Windows release
  workflow and GitHub Release publishing path.
- `llama-helper/`: Rust sidecar used by local summary paths.
- `scripts/`: repo-level helper scripts.

## What Recently Shipped

Current shipped version is `0.5.32`.

Recent release theme:

- `0.5.29`: beta cloud retranscription providers:
  - Hosted Whisper through OpenAI-compatible file transcription.
  - MAI-Transcribe 1.5 through Azure Speech Fast Transcription.
  - Explicit opt-in, separate credentials, local fallback, and word-timestamp
    provenance invariants.
- `0.5.30`: diarization threshold wording and regression coverage for preserving
  brief real speakers in long meetings.
- `0.5.31`: in-app hosted transcription provider smoke test under Settings ->
  Transcription.
- `0.5.32`: Hosted Whisper 25 MB upload preflight, additional
  upload-too-large classification, and corrected fallback toast copy.

The release workflow currently publishes Windows MSI/NSIS installers,
`latest.json`, `SHA256SUMS.txt`, `BUILD-METADATA.txt`, and
`BUILD-METRICS.json`. The Tauri updater reads
`https://github.com/ch3573r/ClawScribe/releases/latest/download/latest.json`.

## Build And Validation Commands

Run from `frontend/` unless stated otherwise.

Install and run development app:

```powershell
pnpm install --frozen-lockfile
pnpm run tauri:dev
```

Frontend-only dev server:

```powershell
pnpm run dev
```

Focused frontend checks:

```powershell
pnpm run typecheck
pnpm run test
```

Rust check from the Tauri crate:

```powershell
cargo check --manifest-path src-tauri/Cargo.toml
```

Windows release validation:

```powershell
.\scripts\build-windows-release.ps1 -CheckOnly
```

Windows release build:

```powershell
.\scripts\build-windows-release.ps1
```

Local-machine warning: on this machine, `directml` has historically been safer
for local builds than the full `windows-gpu` path unless `VULKAN_SDK` is ready.
Use `pwsh`, and if path-length or `whisper-rs-sys`/CMake `FTK1011` failures
appear, set `CARGO_TARGET_DIR` to a short temp path such as `%TEMP%\csr`.

Hosted transcription live smoke:

```powershell
pnpm run test:cloud-live
```

See `docs/hosted-transcription-smoke.md` for required environment variables.
Do not run live provider tests without real test credentials and a short,
non-sensitive audio file.

## Windows Release Path

Canonical doc: `docs/windows-release.md`.

Local build entrypoint:

```powershell
cd frontend
.\scripts\build-windows-release.ps1 -CheckOnly
.\scripts\build-windows-release.ps1
```

GitHub Actions entrypoint:

- Workflow: `.github/workflows/clawscribe-windows-release.yml`
- Runner labels: `self-hosted`, `windows`, `clawscribe`
- Normal feature set: `windows-gpu`
- Local fallback feature set when Vulkan is not ready: `directml`
- Publish only when version, release notes, updater metadata, and artifacts are
  internally consistent.

Release-commit rule: release commits should be metadata-only. Feature code,
fixes, refactors, and dependency churn should land before release prep.

Important release assets:

- `ClawScribe_<version>_x64_en-US.msi`
- `ClawScribe_<version>_x64-setup.exe`
- `latest.json`
- `SHA256SUMS.txt`
- `BUILD-METADATA.txt`
- `BUILD-METRICS.json`

Important gotcha: GitHub `/releases/latest/download/latest.json` ignores
prereleases. While this updater channel is active, alpha builds that should be
seen by installed clients must be published as normal GitHub releases or the
updater path must be redesigned.

Durable expectation: GitHub Releases need descriptive notes, not just binary
uploads. Include user-facing changes, fixes, update-impact notes, known caveats,
and confirm the notes match the version in `latest.json`.

## Cloud Transcription Area

Canonical docs:

- `README.md`, "Cloud Transcription Beta"
- `docs/hosted-transcription-smoke.md`

Main frontend files:

- `frontend/src/components/TranscriptSettings.tsx`
- `frontend/src/hooks/useCloudTranscription.ts`
- `frontend/src/lib/cloudTranscription.ts`

Main Rust files:

- `frontend/src-tauri/src/api/api.rs`
- `frontend/src-tauri/src/audio/retranscription.rs`
- `frontend/src-tauri/src/audio/transcription/cloud/mod.rs`
- `frontend/src-tauri/src/audio/transcription/cloud/openai_whisper.rs`
- `frontend/src-tauri/src/audio/transcription/cloud/mai_transcribe.rs`

Key behavior to preserve:

- Cloud retranscription is beta and opt-in.
- Hosted Whisper uses OpenAI-compatible file transcription and may return real
  word timestamps. Those timestamps can be marked as real ASR anchors.
- OpenAI-hosted Whisper uploads are capped at 25 MB. Oversized files should be
  classified as `upload_too_large` and fall back locally without a doomed upload.
- MAI-Transcribe uses Azure Speech Fast Transcription, separate from Microsoft
  Graph sign-in.
- MAI sentence-level timing must not be promoted to real word timestamps.
- If Azure returns collapsed output, ClawScribe may map rows onto the local VAD
  timing grid, but that timing remains approximate.
- If cloud fails, notify the user that local fallback is starting; do not claim
  success before fallback completes.

Suggested next work:

- Run real audio through both Hosted Whisper and MAI using the in-app test and
  CLI live smoke. Capture whether MAI still collapses output for realistic long
  meetings.
- Add fixture/regression coverage for provider error bodies that indicate size
  limits without HTTP 413.
- Verify current Azure Speech Fast Transcription API shape before changing
  `mai_transcribe.rs`; do not rely on stale assumptions.

## OpenClaw Handoff Area

Canonical docs:

- `docs/openclaw-handoff.md`
- `docs/openai-oauth.md`

Main frontend files:

- `frontend/src/components/ModelSettingsModal.tsx`
- `frontend/src/services/configService.ts`

Main Rust files:

- `frontend/src-tauri/src/openclaw.rs`
- `frontend/src-tauri/src/config.rs`
- `frontend/src-tauri/src/api/api.rs`
- `frontend/src-tauri/src/summary/openai_provider.rs`

Key behavior to preserve:

- ClawScribe posts completed Meetily-format recording folders to an operator-run
  OpenClaw ingest endpoint after recording artifacts are finalized.
- Config can come from the Tauri app config JSON or `MEETILY_OPENCLAW_*`
  environment variables. Those env names are intentionally retained for
  compatibility.
- Bearer tokens must stay out of committed docs and examples.
- Submission marker files in recording folders are part of duplicate protection:
  `.openclaw-pending.json`, `.openclaw-submitted.json`, `.openclaw-failed.json`.
- The app also records `openclaw-last-submission.json` in the Tauri config
  directory for settings/status UI.
- OpenClaw managed auth uses the configured model endpoint as an
  OpenAI-compatible chat-completions bridge. ClawScribe stores only the handoff
  bearer token; it must not store ChatGPT/Codex tokens.

Suggested next work:

- Smoke a completed recording against a real ingest service and confirm both
  `.openclaw-submitted.json` and Settings last-status UI update.
- Add or refresh operator-facing docs only with placeholder hosts and redacted
  tokens.
- If endpoint behavior changes, update `docs/openclaw-handoff.md` before
  release prep.

## Codex App-Server Area

Canonical doc: `docs/codex-runtime.md`.

Main files:

- `frontend/scripts/stage-codex-runtime.ps1`
- `frontend/src-tauri/src/summary/codex_provider.rs`
- `frontend/src-tauri/src/summary/llm_client.rs`
- `frontend/src/components/ModelSettingsModal.tsx`
- `frontend/src-tauri/binaries/codex-app-server-runtime.json`

Pinned runtime at handoff time:

- package: `@openai/codex@0.139.0-win32-x64`
- target: `x86_64-pc-windows-msvc`
- executable SHA256:
  `77a84f8078400467ade4301d827b8bcea2d29b6838c9cd162bf3573b7ef97e10`

Rules:

- Codex is only for the "Advanced: Codex app-server" provider.
- Normal meeting processing must continue through built-in, OpenAI-compatible,
  OpenClaw, and other providers without Codex installed or configured.
- Launch only the bundled sidecar from the app install/resource path.
- Do not read or write the user's normal `~/.codex` profile. ClawScribe uses
  isolated sidecar state under `%APPDATA%\ClawScribe\codex`.
- If the bundled runtime is missing or hash verification fails, show a repair or
  reinstall message rather than discovering arbitrary `codex.exe` from `PATH`.

Suggested next work:

- If updating the Codex runtime, verify the NPM package and executable SHA256s,
  update `stage-codex-runtime.ps1`, `docs/codex-runtime.md`, and release build
  metadata together.
- Exercise auth/login, `thread/start`, `turn/start`, overload retry, and redacted
  logging before publishing a runtime bump.

## Speaker Diarization And Local ASR Area

Main files:

- `frontend/src-tauri/src/audio/diarization.rs`
- `frontend/src-tauri/src/audio/retranscription.rs`
- `frontend/src-tauri/src/audio/transcription/*`
- `frontend/src-tauri/src/parakeet_engine/*`
- `frontend/src-tauri/src/nemotron_engine/*`
- `frontend/src-tauri/src/whisper_engine/*`
- `frontend/scripts/stage-sherpa-runtime.ps1`
- `frontend/src/components/MeetingDetails/SpeakerDiarizationDialog.tsx`
- `frontend/src/components/MeetingDetails/RetranscribeDialog.tsx`

Key behavior to preserve:

- Parakeet is the default fast path.
- Nemotron remains beta.
- Whisper remains the compatibility path.
- Real word timestamps from ASR can drive finer speaker-turn splitting.
- Estimated or approximate timing must not be represented as real word-level
  provenance.
- Failed diarization should be non-destructive. Do not overwrite useful labels
  with collapsed or low-confidence mappings.
- Sherpa runtime DLL staging is part of Windows release readiness.

Suggested next work:

- Re-run long-meeting and short-interjection diarization cases after touching
  transcription, timing-grid, or word timestamp logic.
- Keep diagnostics rich enough to explain why a mapping was rejected.
- Avoid broad cleanup in these paths during release prep; they are high-risk.

## Microsoft Export Area

Canonical docs:

- `docs/integrations/microsoft-graph.md`
- `docs/integrations/onenote-export.md`
- `docs/integrations/planner-export.md`
- `docs/integrations/todo-export.md`

Main files:

- `frontend/src-tauri/src/exports/*`
- `frontend/src/components/MeetingDetails/MeetingExportButtons.tsx`
- `frontend/src/components/MeetingDetails/PlannerExportPreview.tsx`
- `frontend/src/components/MeetingDetails/ToDoExportPreview.tsx`
- `frontend/src/services/microsoftExportService.ts`
- `frontend/src/hooks/useMicrosoftExport.ts`

Key behavior to preserve:

- Microsoft tokens go through the platform credential store.
- Planner and To Do exports use review/edit flows and duplicate protection.
- OneNote export creates fresh dated sections to avoid large-library listing
  limits.
- Graph scopes and tenant behavior should be verified against Microsoft docs
  before changing auth, consent, or export payloads.

Suggested next work:

- Add narrow regression tests around Graph URL/path encoding and duplicate
  ledgers when modifying exports.
- Verify with a real test tenant before claiming a Graph flow works end to end.

## Update System Area

Main files:

- `frontend/src/services/updateService.ts`
- `frontend/src/hooks/useUpdateCheck.ts`
- `frontend/src/components/UpdateCheckProvider.tsx`
- `frontend/src/components/UpdateDialog.tsx`
- `frontend/src/components/UpdateNotification.tsx`
- `frontend/src-tauri/tauri.conf.json`
- `.github/workflows/clawscribe-windows-release.yml`
- `scripts/generate-update-manifest-github.js`

Key behavior to preserve:

- App updater endpoint is the GitHub Release `latest.json` URL.
- `latest.json` version must match the runtime/installable version clients see.
- NSIS setup is the updater download target.
- Prerelease suffixes can be updater-invisible after Windows/Tauri version
  stripping. Bump the numeric version when using the current updater path.

Suggested next work:

- Before the next release, test update discovery from installed `0.5.32` to the
  candidate version, not just fresh install.
- Keep release notes, CHANGELOG, package versions, Tauri version, and
  `latest.json` aligned.

## Security And Privacy Rules

- Do not commit `.env`, API keys, bearer tokens, OAuth codes, refresh tokens,
  private keys, certificates, local auth stores, logs, databases, generated
  installers, or local build tool installers.
- Use placeholder endpoints such as `https://openclaw.example.com` or
  `http://openclaw.local:8765` in docs.
- Redact local usernames, local filesystem paths, endpoint hosts, credentials,
  and transcript content from support payloads and logs unless the user
  explicitly asks for local-only diagnosis.
- Prefer the platform credential store for secrets.
- Keep compatibility names such as `MEETILY_OPENCLAW_*` unless a migration plan
  exists.
- Do not change SQL migration contents that may already be applied in installed
  clients; add a new migration instead.

## Suggested Order Of Operations For The Next Claude Code Session

1. Confirm the live worktree:

   ```powershell
   cd D:\AI\ClawScribe
   git status --short --branch
   git log --oneline -5 --decorate
   ```

2. Read `CLAUDE.md`, then this handoff, then only the canonical docs relevant
   to the requested task.

3. If the task is release-related, verify live GitHub release/latest metadata
   before making claims. Do not rely only on this handoff.

4. If the task is cloud/provider-related, verify the official provider schema
   live before changing code.

5. If the task is installed-app runtime triage, inspect:

   ```powershell
   Get-Item "$env:LOCALAPPDATA\net.rismondo.openclaw.clawscribe\logs\ClawScribe.log"
   Get-Item "$env:APPDATA\net.rismondo.openclaw.clawscribe\meeting_minutes.sqlite"
   ```

   For startup failures, read the app log before guessing from Windows Event
   Viewer. For database repairs, prefer minimal reversible changes and make a
   backup first.

6. Make focused changes in the owning module. Avoid release metadata, lockfile,
   generated asset, and installer churn unless the task requires it.

7. Run the narrowest meaningful checks first. Broaden validation for shared
   audio/transcription/summary/export behavior.

8. For a release, do the full release checklist:
   - Version bump in `frontend/package.json`, `frontend/src-tauri/Cargo.toml`,
     `frontend/src-tauri/tauri.conf.json`, and README/docs where needed.
   - `CHANGELOG.md` section with user-facing notes.
   - `pnpm run test`, `pnpm run typecheck`, relevant Rust tests/checks.
   - Windows release workflow or local build.
   - Verify artifacts, checksums, `latest.json`, and installed app version.
   - Publish descriptive GitHub Release notes.

## High-Value Next Improvements

These are suggestions, not commitments:

- Strengthen hosted transcription live validation with a small private corpus:
  short file, near-25 MB file, over-25 MB file, long meeting, and MAI collapsed
  output case.
- Add more provider error classification fixtures so cloud fallback messages stay
  accurate as provider responses drift.
- Build a repeatable updater smoke from installed `0.5.32` to a staged candidate
  release.
- Improve diagnostics export around cloud retranscription fallback without
  leaking endpoint hosts, keys, or transcript content.
- Keep working toward an RC-quality Windows release path: signed installers,
  update discovery smoke, clean install/upgrade/uninstall smoke, and an operator
  OpenClaw ingest smoke.
- Revisit visible brand consistency only if requested. Prior naming work adopted
  ClawNote as the visible-brand direction, but the current repository, package,
  installer, and release surfaces are ClawScribe. Do not rename package,
  installer, updater, or internal identity surfaces without an explicit
  migration plan.
