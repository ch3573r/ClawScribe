# CLAUDE.md

This file gives coding-agent context for working in the ClawScribe repository.
Keep it evergreen and safe to commit. Do not add private infrastructure names,
internal IP addresses, credentials, local usernames, personal workspace paths,
or temporary handoff notes.

Before committing, run `node scripts/verify-public-repo-safety.mjs`. CI and the
Windows release preflight run the same tracked-file scan and reject personal
home/workspace paths, private keys, provider tokens, and literal credential
assignments. Use environment-variable references and explicit placeholders in
public examples.

## Product Context

ClawScribe is a local-first desktop meeting recorder and summarizer. It is based
on Meetily Community Edition and is currently focused on the Tauri desktop app:

- Next.js and React UI in `frontend/src`
- Rust/Tauri core in `frontend/src-tauri/src`
- Local recording, audio processing, transcription, storage, and summarization
  through Tauri commands and events
- Optional Microsoft Graph exports and optional OpenClaw/OpenAI-compatible
  integrations configured by the user

The historical Python/FastAPI backend has been removed. Do not reintroduce a
separate backend runtime unless the project explicitly chooses that direction.

## UI conventions

Existing violations are debt, not precedent. Use shared controls from
`frontend/src/components/ui/` (Button, Switch, Select, Textarea, Input,
Progress, Tabs, Popover, DropdownMenu, Tooltip, Dialog, ScrollArea, Separator,
and Checkbox). Add missing primitives in the existing shadcn style without
new UI dependencies. Native select, textarea, progress, checkbox inputs, and
details belong only in that directory.

Use `globals.css` theme tokens through Tailwind: background, card, border,
muted, muted-foreground, primary, accent, destructive, success, warning,
error, and info. Do not use hex colors or raw palette utilities. Use sentence
case throughout the UI, retaining proper nouns such as ClawScribe, Microsoft
365, OneNote, and Codex. Keep wording plain and short; internal identifiers,
hashes, citation tags, and model repository IDs belong in a Tooltip or Details
when needed, outside the normal view.

Use one primary action per panel, outline/ghost secondary actions, and an
overflow menu for rare actions. Avoid empty boxes and duplicate headings.
Reuse the shared PageSection card and spacing pattern. Keep one scrollbar per
view; support light/dark themes at 1000 px and wider. Provide loading, empty,
error, and disabled states for asynchronous controls, and show actions only
when they apply. Keep keyboard focus visible and label icon buttons. Enter
submits; Shift+Enter adds a new line.

Run `node scripts/verify-ui-conventions.mjs`. Its committed baseline records
existing debt; do not raise counts for new violations. Remove a file from the
baseline when all its counts reach zero.

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

Run all CI validation, installer builds, and native Windows diagnostics on the
designated local self-hosted machine configured by the `CLAWSCRIBE_BUILD_RUNNER`
Actions variable. No workflow may use a hosted runner. PR jobs must exclude
forks and untrusted authors before scheduling on the persistent machine. Keep
caches and test installers local. Use the public-safe runner setup in
`docs/windows-release.md`; never commit the actual machine name or revive
legacy hosted workflows.

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
