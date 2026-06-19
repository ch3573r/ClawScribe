# CLAUDE.md

This file gives coding-agent context for working in the ClawScribe repository.
Keep it evergreen and safe to commit. Do not add private infrastructure names,
internal IP addresses, credentials, local usernames, personal workspace paths,
or temporary handoff notes.

## Product Context

ClawScribe is a local-first desktop meeting recorder and summarizer. It is based
on Meetily Community Edition and is currently focused on the Tauri desktop app:

- Next.js and React UI in `frontend/src`
- Rust/Tauri core in `frontend/src-tauri/src`
- Local recording, audio processing, transcription, storage, and summarization
  through Tauri commands and events
- Optional Microsoft Graph exports and optional OpenClaw/OpenAI-compatible
  integrations configured by the user

The historical Python/FastAPI backend under `backend/` is legacy reference
material. Do not add new supported runtime behavior there unless the project
explicitly reintroduces that backend.

## Development Commands

From `frontend/`:

```bash
pnpm install
pnpm run dev
pnpm run tauri:dev
pnpm run tauri:build
```

Windows release validation and bundling lives in:

```powershell
cd frontend
.\scripts\build-windows-release.ps1 -CheckOnly
.\scripts\build-windows-release.ps1
```

Use GPU feature scripts only when the task is specifically about acceleration:

```bash
./dev-gpu.sh
./build-gpu.sh
```

or the matching `.ps1` / `.bat` scripts on Windows.

## Architecture Notes

- `frontend/src-tauri/src/lib.rs` registers the Tauri commands and app state.
- `frontend/src-tauri/src/audio/` owns capture, mixing, VAD, recording, import,
  and retranscription paths.
- `frontend/src-tauri/src/parakeet_engine/` and
  `frontend/src-tauri/src/nemotron_engine/` own ONNX transcription paths.
- `frontend/src-tauri/src/summary/` owns summary generation and provider
  orchestration.
- `frontend/src-tauri/src/exports/` owns Microsoft Graph export flows.
- `frontend/src/components/` and `frontend/src/app/` own the UI.

Meeting persistence, model selection, and summarization should flow through the
Tauri app, not through a separate web backend.

## Security Rules

- Never commit real API keys, bearer tokens, OAuth codes, refresh tokens,
  private keys, certificates, local auth stores, logs, databases, or generated
  installer artifacts.
- Keep `.env` files local. Commit only `.env.example` placeholders.
- Use placeholders such as `https://openclaw.example.com` or
  `http://openclaw.local:8765` in docs instead of private network addresses.
- Redact credentials in logs, analytics, errors, tests, fixtures, screenshots,
  and documentation.
- Store user credentials through the app's credential-storage path where
  available; do not add new plaintext credential files.

## Working Conventions

- Prefer small, focused changes that match existing module boundaries.
- Avoid touching generated assets, lockfiles, or vendored binaries unless the
  task requires it.
- Remove dead backup files rather than preserving `.old`, `.backup`, or copied
  source files in the tracked tree.
- Keep README and docs product-facing. Move transient implementation notes to
  issues or pull requests.
