# AGENTS.md

This file applies to the entire repository. A more specific `AGENTS.md` in a
subdirectory may add or override instructions for that subtree.

## Product And Repository

ClawScribe is a Windows-first, local-first desktop meeting recorder. The
supported application is the Tauri desktop app:

- Next.js, React, and TypeScript UI under `frontend/src`
- Rust/Tauri application core under `frontend/src-tauri/src`
- Local microphone and system-audio capture
- Local Whisper, Parakeet, and Nemotron transcription paths
- Optional cloud transcription, summary providers, and Microsoft Graph exports
- A separately built `llama-helper` sidecar in the root Cargo workspace

The historical Python/FastAPI backend has been removed. Do not recreate a
backend service, Docker runtime, or standalone Whisper server unless the
project owner explicitly changes the architecture.

Read `CLAUDE.md`, `CONTRIBUTING.md`, and the relevant document under `docs/`
before changing a subsystem. Keep this file and those documents consistent.

## Non-Negotiable Safety Rules

This is a public repository.

- Never commit local usernames, machine-specific paths, private hostnames or IP
  addresses, internal infrastructure names, temporary handoff notes, logs,
  databases, recordings, transcripts, screenshots containing private data, or
  generated installers.
- Never commit passwords, API keys, bearer tokens, OAuth codes, refresh tokens,
  session cookies, private keys, certificates, signing material, or populated
  environment files. Use environment variables and explicit placeholders in
  public examples.
- Never print credentials, auth callback URLs, full provider responses, or
  transcript content in diagnostic logs. Log sanitized metadata only.
- Store credentials through the platform credential store. Any file fallback
  must use OS-backed encryption and fail closed; do not introduce or preserve a
  plaintext credential fallback.
- Before every commit, run:

  ```powershell
  node scripts/verify-public-repo-safety.mjs
  ```

- If the safety scan finds something, remove it from the commit. Do not weaken
  the scanner to make a finding disappear.

## Working Agreement

- Inspect `git status` before editing. Preserve unrelated user changes and do
  not reformat or rewrite files outside the task.
- Make the smallest cohesive change that fixes the underlying behavior. Follow
  existing module boundaries and add a focused regression test when practical.
- Use `pnpm` for frontend dependencies and keep `frontend/pnpm-lock.yaml` in
  sync when dependencies intentionally change. Do not use npm or Yarn to mutate
  the dependency graph.
- Do not modify generated assets, lockfiles, vendored binaries, sidecars, model
  files, or installer output unless the task requires that exact change.
- Do not use destructive Git commands, rewrite published history, or discard
  uncommitted work.
- Commit every completed workspace change after validation. Use a short,
  descriptive commit message and leave the worktree clean. Do not amend an
  unrelated existing commit.
- Push when the user requests it or when the requested workflow explicitly
  includes publishing/releasing. Never force-push unless explicitly authorized.
- Do not bump versions, tag, publish, or create a release unless the user asks
  for a release.

## Source Map

- `frontend/src-tauri/src/lib.rs`: Tauri command registration, plugins, and app
  state
- `frontend/src-tauri/src/audio/`: capture, mixing, VAD, recording, import,
  transcription orchestration, and retranscription
- `frontend/src-tauri/src/whisper_engine.rs`: Whisper integration
- `frontend/src-tauri/src/parakeet_engine/`: Parakeet ONNX integration
- `frontend/src-tauri/src/nemotron_engine/`: Nemotron ONNX integration
- `frontend/src-tauri/src/summary/`: summary providers and orchestration,
  including the advanced bundled Codex provider
- `frontend/src-tauri/src/exports/`: Microsoft authentication and Graph export
  flows
- `frontend/src-tauri/src/database/`: local meeting persistence and migrations
- `frontend/src/app/`: routes and page composition
- `frontend/src/components/`: product UI
- `frontend/src/contexts/` and `frontend/src/hooks/`: shared frontend state and
  Tauri event integration
- `frontend/tests/lib/`: frontend helper regression tests
- `frontend/src-tauri/migrations/`: embedded SQLx migrations; preserve canonical
  line endings and never edit a migration already shipped to users
- `frontend/scripts/`: local development, runtime staging, GPU detection, and
  Windows release tooling
- `.github/workflows/clawscribe-windows-release.yml`: authoritative Windows
  release/publish workflow

The root `Cargo.toml` is the workspace manifest. Its `cpal` patch is active for
all members; changing that revision is an audio-stack change, not routine
dependency cleanup.

## Development Commands

Install and run from `frontend`:

```powershell
pnpm install --frozen-lockfile
pnpm run dev
pnpm run tauri:dev
```

Useful validation commands:

```powershell
# Repository root
node scripts/verify-public-repo-safety.mjs
cargo fmt --all -- --check
cargo check
cargo test --lib

# frontend
pnpm run typecheck
pnpm run test
pnpm run build
pnpm run verify:icons
```

Run the smallest relevant set during iteration, then the complete relevant set
before committing:

| Change | Minimum validation |
| --- | --- |
| Documentation or agent guidance | Public-repository safety scan |
| Frontend helper or state logic | `pnpm run typecheck` and `pnpm run test` |
| UI component or route | Typecheck, frontend tests, and `pnpm run build` |
| Rust logic | `cargo fmt --all -- --check`, `cargo check`, and targeted Rust tests |
| Microsoft exports/auth | Targeted `cargo test --lib exports::` plus sanitized-log review |
| Audio/transcription | Targeted Rust tests, relevant feature build, and a real recording smoke when behavior changes |
| Release metadata or packaging | Windows release `-CheckOnly`, version consistency, icons, and release checklist |

If an existing unrelated failure prevents a command from passing, report the
exact command and failure. Do not hide it or claim the repository is validated.

## Recording And Performance Guardrails

Recording is the highest-risk path. A two-hour meeting must not cause
unbounded memory growth, starve the async runtime, or delay shutdown by the
length of the transcription backlog.

- Preserve captured audio even if live transcription, queueing, checkpointing,
  or a provider fails. Surface degraded live transcription to the user and
  make recovery/retranscription possible.
- Keep long-running inference, encoding, and blocking disk/process work off
  latency-sensitive async and capture paths. Use bounded queues and explicit
  backpressure or degradation behavior.
- Make cancellation and stop behavior explicit. Stop capture promptly, drain
  only bounded essential work, persist recoverable state, and never silently
  leave the UI waiting on an unlimited backlog.
- Do not discard a non-empty transcript solely because it is short or has low
  confidence. Confidence may be displayed or logged as sanitized metrics, but
  it must not erase legitimate words such as short German responses.
- Keep microphone and system-audio selection synchronized across Settings,
  Home, and recording startup. Device-change events must update every consumer
  without requiring an application restart.
- Any change to `cpal`, capture formats, mixing, device selection, recording
  save, or stop flow requires a Windows smoke test with both microphone and
  system audio. Confirm the saved audio is playable, stop completes, and a
  second recording can start.
- For performance work, test a release-like build and report elapsed
  transcription time, backlog behavior, memory trend, model, language, engine,
  and acceleration backend. A short debug run is not evidence for long-meeting
  stability.

## Transcription And Acceleration

- Keep local transcription local by default. Cloud transcription is opt-in and
  must never upload audio without explicit user enablement.
- Model and language choices must flow through the Tauri state/persistence
  path. The UI must show the engine/model actually used, including fallbacks.
- `windows-gpu` is the release acceleration profile: Whisper uses Vulkan while
  ONNX/sherpa paths use DirectML. These are separate backends; do not describe
  a DirectML run as Vulkan or assume a visible Vulkan adapter proves the active
  engine is accelerated.
- A Vulkan SDK enables compilation. Runtime acceleration must be confirmed from
  feature metadata and startup/inference logs in the built app.
- Hardware/provider initialization failures must use a deliberate supported
  fallback and a user-visible explanation. Never silently turn a GPU-labeled
  configuration into a slow CPU session.
- Preserve live and import/retranscription differences unless a change is
  intentionally designed and tested for both.
- Treat real German speech, short utterances, overlapping speakers, silence,
  and a long mixed-source meeting as required quality cases for major
  transcription changes.

## Summary Providers And Meeting Chat

- Keep provider credentials and connection states isolated. Microsoft,
  OpenAI-compatible, OpenClaw, Codex, Ollama, and other providers must fail
  independently.
- A meeting-chat submission must produce exactly one assistant message. When a
  provider exposes streaming events, completed items, and a final RPC snapshot,
  reconcile them by stable identity/content rather than rendering every path.
- Preserve useful partial output on recoverable streaming failures, but do not
  duplicate it in the final response.
- The bundled Codex app-server is an optional advanced summary provider, not a
  transcription engine and not a required app runtime. Model choices come from
  its `model/list` response.
- Do not use a globally installed Codex executable or the user's normal Codex
  profile. Launch only the pinned sidecar from the app resource path with the
  isolated app-owned state described in `docs/codex-runtime.md`.
- A Codex runtime upgrade is an explicit dependency/security change. Update the
  staging script, runtime manifest, Rust constants, About text, documentation,
  release metadata, source hash, and executable hash together, then exercise
  authentication, `model/list`, summary generation, chat streaming, and
  fallback behavior.

## Microsoft 365 Guardrails

- Microsoft sign-in is optional. Local recording, transcription, summaries,
  and non-Microsoft exports must continue to work while disconnected.
- Request only Graph scopes used by implemented endpoints. Any new or broader
  scope needs an explicit review and user-facing rationale.
- Keep calendar access read-only unless the product explicitly adds a reviewed
  write flow.
- Planner and Microsoft To Do exports are review-first. Never create tasks
  directly from AI output without letting the user inspect, edit, and select
  them.
- Preserve duplicate protection and idempotency across retries.
- Respect Graph throttling and typed auth states. Surface consent, tenant, and
  destination failures without deleting local meeting artifacts.
- Keep the OneNote create-new/saved-ID flow working without requiring section
  enumeration; large OneDrive or SharePoint libraries can make enumeration
  fail.

## Frontend And UX

- Respect dark and light themes, the selected accent color, keyboard use, focus
  visibility, and narrow as well as wide desktop layouts.
- Use shared contexts/hooks for cross-page settings rather than page-local
  snapshots that become stale.
- Prefer one authoritative state owner for recording, selected devices,
  providers, and models. Subscribe/unsubscribe to Tauri events in effect
  cleanup and avoid duplicate listeners after navigation.
- Disable or make submit actions idempotent while work is in flight. Errors and
  degraded states must be visible and actionable, not log-only.
- Use `ClawScribe` in product-facing copy. Retain `Meetily` only for upstream
  attribution or compatibility storage/schema identifiers.

## Database And Persistence

- Treat shipped SQLx migrations as immutable. Add a new migration for schema
  changes and test both a clean database and upgrade from an existing one.
- Persist before emitting success to the UI. Use atomic/transactional writes
  where partial state could corrupt a meeting or cause duplicate exports.
- Maintain backward compatibility for existing meeting folders, settings, and
  Meetily-compatible storage unless a documented migration is part of the
  change.
- Never solve corruption or auth migration failures by silently deleting user
  recordings or accepting plaintext secrets.

## Release Rules

Only perform a release when explicitly requested.

1. Land and commit feature/fix work first.
2. Create a separate metadata-only release commit. It may contain synchronized
   version fields, `Cargo.lock`, `CHANGELOG.md`, release notes, and updater
   metadata, but no feature or refactor work.
3. Keep the version synchronized in `frontend/package.json`,
   `frontend/src-tauri/Cargo.toml`, `frontend/src-tauri/tauri.conf.json`, the
   workspace lockfile, and current-version documentation.
4. Ensure the numeric runtime/updater version is greater than the published
   version. Never replace binaries under an existing version or tag.
5. From `frontend`, run:

   ```powershell
   .\scripts\build-windows-release.ps1 -CheckOnly
   ```

6. The normal Windows release uses `windows-gpu`. A CPU-only or single-backend
   build must be explicitly requested and clearly labeled.
7. Complete the Windows dual-source capture smoke on the exact code and active
   `cpal` revision before confirming the publish gate.
8. Publish from the intended annotated tag/build ref, include descriptive
   release notes, and verify the release contains both installers, signatures
   when configured, checksums, build metadata, and `latest.json`.
9. Verify `latest.json` advertises the intended runtime version and points to
   the published updater artifact. Confirm an installed older build discovers
   the update.

Do not commit staged sidecar executables, installers, model downloads, or local
build output. The release workflow produces those artifacts outside Git.

## Documentation

- Keep committed documentation evergreen, public-safe, and product-facing.
- Update `README.md` for user-visible capabilities, providers, model choices,
  installation, or updater changes. Update the focused document under `docs/`
  for implementation and operational details.
- Remove stale or contradictory guidance. Do not add dated personal handoff
  documents to the tracked tree.
- Release notes must describe user-visible changes, fixes, updater behavior,
  and known caveats; artifact-only notes are not sufficient.
