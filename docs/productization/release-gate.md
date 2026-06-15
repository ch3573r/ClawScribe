# ClawScribe Release Gate

Date: 2026-06-15
Branch: `feat/clawscribe-productization-auth-theme-exports`
Baseline: `docs/productization/phase0-inventory.md`

This gate is intentionally conservative. It separates Linux-side build and code
checks from Windows runtime verification, and it does not treat design docs as
implemented behavior.

Do not trigger GitHub Windows installer builds unless the coordinator or Alex
explicitly asks for a release checkpoint build.

## Gate Status Vocabulary

- `pass`: command or manual check completed and matched expected result.
- `fail`: command or manual check completed and exposed a blocker.
- `blocked`: check could not run because required infrastructure, credentials,
  implementation, or platform was unavailable.
- `not applicable`: check does not apply to this release candidate.

## Required Linux-Side Commands

Run from the repo root unless a command includes `cd`.

### Repo State

```bash
git status --short --branch
git rev-parse HEAD
git rev-parse --abbrev-ref HEAD
```

Expected:

- Branch is `feat/clawscribe-productization-auth-theme-exports`.
- Any uncommitted files are understood and owned by the relevant worker.
- QA/release edits are limited to docs.

### Frontend Build

```bash
cd frontend
pnpm build
```

Expected:

- Next production build completes successfully.
- Static routes are generated.
- No Windows runtime claim is made from this result.

### Explicit Typecheck

```bash
cd frontend
pnpm exec tsc --noEmit --pretty false
```

Expected:

- This should eventually pass.
- Current known baseline blocker: `tests/lib/blocknote-markdown.test.ts` imports
  `bun:test` without available typings.

### Lint

```bash
cd frontend
CI=1 pnpm lint
```

Expected:

- This should eventually run non-interactively and pass.
- Current known baseline blocker: `next lint` prompts for ESLint setup and exits
  non-zero.

### Linux Tauri Check

The Linux sidecars must exist first:

```bash
find frontend/src-tauri/binaries -maxdepth 1 -type f -printf '%f %s bytes\n' | sort
```

Then run:

```bash
cargo check --manifest-path frontend/src-tauri/Cargo.toml
```

Expected:

- Linux Tauri crate checks successfully.
- Existing warnings are triaged but are not automatically release blockers.
- This does not verify Windows WASAPI, WebView2, MSI/NSIS, or Teams runtime
  behavior.

### Targeted Rust Tests

Run focused tests that match integrated areas:

```bash
cargo test --manifest-path frontend/src-tauri/Cargo.toml codex_provider --lib
cargo test --manifest-path frontend/src-tauri/Cargo.toml openai::auth --lib
cargo test --manifest-path frontend/src-tauri/Cargo.toml teams_detection --lib
```

Expected:

- Codex tests confirm app-server runtime discovery, missing-runtime behavior,
  WindowsApps rejection, no PATH/global executable discovery, app-server
  account/thread request shapes, secret redaction, and isolated `CODEX_HOME`.
- Auth tests confirm API-key and OpenClaw managed-auth status behavior.
- Teams tests confirm detector invariants at code level.
- These tests do not replace Windows runtime Teams or audio verification.

## Windows Build And Packaging Commands

Run only at an explicit release checkpoint on a Windows host with Visual Studio
Build Tools, Windows SDK, Rust, Node.js, pnpm, and LLVM installed.

Validation-only:

```powershell
cd frontend
.\scripts\build-windows-release.ps1 -CheckOnly
```

Stage the Windows sidecar:

```powershell
cd <repo-root>
cargo build -p llama-helper --release --target x86_64-pc-windows-msvc
Copy-Item .\target\x86_64-pc-windows-msvc\release\llama-helper.exe .\frontend\src-tauri\binaries\llama-helper-x86_64-pc-windows-msvc.exe -Force
```

Build installers:

```powershell
cd frontend
.\scripts\build-windows-release.ps1
```

Optional feature overrides:

```powershell
.\scripts\build-windows-release.ps1 -Feature cpu
.\scripts\build-windows-release.ps1 -Feature cuda
.\scripts\build-windows-release.ps1 -Feature vulkan
.\scripts\build-windows-release.ps1 -Feature openblas
```

Expected:

- MSI and NSIS artifacts are produced only during deliberate release builds.
- Unsigned artifacts are clearly labeled unsigned unless signing secrets are
  configured and signing is verified.
- `BUILD-METADATA.txt` records product, version, upstream base version
  `0.4.0`, build commit, short commit, and UTC build date.
- `SHA256SUMS.txt` uses artifact-root-relative paths such as
  `msi/<installer>.msi` and `nsis/<installer>.exe`.
- Generated installer artifacts are not treated as Linux-side validation.

Checksum verification from the downloaded artifact root:

```powershell
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $parts = $_ -split '\s+', 2
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $parts[1]).Hash.ToLowerInvariant() -ne $parts[0]) {
        throw "Checksum mismatch: $($parts[1])"
    }
}
Get-Content .\BUILD-METADATA.txt
```

## Secret Search

High-entropy token search:

```bash
rg -n --hidden --pcre2 -S "(sk-proj-[A-Za-z0-9_-]{20,}|sk-[A-Za-z0-9_-]{32,}|sk-ant-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{20,}|AIza[0-9A-Za-z_-]{30,})" --glob '!**/node_modules/**' --glob '!**/target/**' --glob '!frontend/out/**' --glob '!frontend/vs_buildtools.exe' --glob '!**/*.png' --glob '!**/*.jpg' --glob '!**/*.gif' .
```

Expected:

- No matches for committed OpenAI, Anthropic, GitHub, Slack, or Google API keys.

Broader token/storage audit:

```bash
rg -n --hidden --pcre2 -S "(?i)(api[_-]?key|bearer|token|secret|password|client[_-]?secret|authorization)" --glob '!**/node_modules/**' --glob '!**/target/**' --glob '!frontend/out/**' --glob '!frontend/vs_buildtools.exe' --glob '!**/*.png' --glob '!**/*.jpg' --glob '!**/*.gif' .
```

Expected:

- Workflow `${{ secrets.* }}` references are allowed.
- Placeholder docs are allowed.
- Code paths that store or log keys require explicit review before claiming
  production-grade secret hygiene.
- Full tokens must not be logged. Truncated-token logging is still a release
  risk unless reviewed and accepted.

## Branding Search

Current product-name scan:

```bash
rg -n --hidden -S "ClawScribe|clawscribe|OpenClaw" --glob '!**/node_modules/**' --glob '!**/target/**' --glob '!frontend/out/**' --glob '!frontend/vs_buildtools.exe' --glob '!**/*.png' --glob '!**/*.jpg' --glob '!**/*.gif' .
```

Legacy/upstream-name scan:

```bash
rg -n --hidden -S "Meetily|meetily|MEETILY|Zackriya" --glob '!**/node_modules/**' --glob '!**/target/**' --glob '!frontend/out/**' --glob '!frontend/vs_buildtools.exe' --glob '!**/*.png' --glob '!**/*.jpg' --glob '!**/*.gif' .
```

Expected:

- Product-visible surfaces use ClawScribe unless an exception is documented.
- Remaining Meetily/Zackriya references are classified as attribution,
  compatibility, previous-install migration, archived code, or non-customer
  developer docs.
- Use `docs/productization/branding-search-report.md` as the current
  classification source.

## OpenClaw Handoff Verification

OpenClaw host readiness:

```bash
cd /path/to/openclaw-ingest
curl -sS http://127.0.0.1:8765/readyz
sudo scripts/smoke-openclaw-bridge.py
```

Windows recorder reachability:

```powershell
Invoke-RestMethod http://openclaw-host.local:8765/readyz
```

Windows app-level handoff:

1. Configure `openclaw.json` and `MEETILY_OPENCLAW_BEARER_TOKEN` according to
   `docs/openclaw-handoff.md`.
2. Launch ClawScribe.
3. Refresh OpenClaw handoff status in settings.
4. Record a short meeting with non-empty transcript content.
5. Stop recording and wait for handoff.
6. Confirm `.openclaw-submitted.json` appears in the recording folder.
7. Confirm OpenClaw ingest receives non-empty transcript content.
8. Intentionally test bad endpoint or bad token and confirm
   `.openclaw-failed.json` contains only sanitized failure details.

Expected:

- Handoff remains optional.
- Existing `openclaw.json` and `.openclaw-*` marker semantics are preserved.
- Bearer token presence may be reported; token value must not be displayed or
  logged.

## Codex Provider Verification

Linux-side implementation gate:

```bash
cargo test --manifest-path frontend/src-tauri/Cargo.toml codex_provider --lib
```

Expected:

- App-server direction tests pass.
- No global Codex CLI, PATH, WindowsApps, or `codex exec` path is required.
- Do not claim Windows Codex app-server verification until a bundled/pinned
  runtime or controlled runtime installer is tested on Windows.

Windows runtime gate:

1. Build/install a Windows ClawScribe candidate.
2. Run `docs/verification/alex-windows-codex-checklist.md`.
3. Record the commit, installer artifact name, Windows version, Codex
   app-server runtime version/path, and result.

Required status wording until Alex completes the checklist:

> Codex app-server provider direction implemented and fake-tested. Bundled
> runtime packaging and Windows app-server auth/runtime verification pending.

## Manual Windows Runtime Gate

Run `docs/verification/windows-final-checklist.md` on Windows. Minimum required
areas:

- launch and installed-app branding
- About dialog and legal attribution
- light/dark theme
- Advanced: Codex app-server provider fake-tested and awaiting Windows runtime
  verification
- OpenAI API-key fallback
- OpenClaw managed auth provider
- Microsoft login, OneNote, and Planner marked not implemented or verified
  only when actual implementation and tests exist
- Teams dry-run with prompt-only detection
- manual recording with microphone and system audio
- artifact inspection
- OpenClaw handoff

Expected:

- No automatic Teams recording side effects.
- Transcript and summary are non-empty for the smoke recording.
- Windows-only claims are backed by Windows evidence.

## Release Blockers

Treat these as blockers until resolved or explicitly waived:

- Windows app cannot launch, install, uninstall, or upgrade.
- App starts/stops recording automatically from Teams detection.
- Transcript or summary is empty in the Windows smoke path.
- Full API keys, bearer tokens, OAuth codes, refresh tokens, or Microsoft access
  tokens appear in logs, docs, screenshots, artifacts, or committed files.
- OpenClaw handoff breaks existing config or marker compatibility.
- Product-visible surfaces show unclassified upstream Meetily branding.
- Microsoft export is advertised as working before login/export verification
  passes.

## No-Overclaiming Language

Use precise statements:

- "Linux-side `pnpm build` passed" instead of "release build verified".
- "Linux Tauri `cargo check` passed" instead of "Windows runtime verified".
- "Windows checklist pending" until a Windows tester completes it.
- "Microsoft Graph export not implemented" or "pending verification" until code
  and tests exist.
- "OpenAI API-key auth supported" instead of "OpenAI login supported" unless an
  official supported login path is implemented.
- "Codex provider implemented and fake-tested. Linux Codex smoke tested if
  available. Windows login/runtime verification pending Alex after Windows
  build." until `docs/verification/alex-windows-codex-checklist.md` passes on
  Windows.
- "OpenClaw handoff verified from Windows" only after a Windows recording
  produces a submitted marker and OpenClaw receives non-empty content.

## Coordinator Rerun Checklist After Integrations Land

After auth, theme, Microsoft export, Teams detection, branding, and release docs
are integrated, rerun:

```bash
git status --short --branch
git rev-parse HEAD
python3 -m json.tool frontend/package.json >/dev/null
python3 -m json.tool frontend/src-tauri/tauri.conf.json >/dev/null
cargo metadata --manifest-path frontend/src-tauri/Cargo.toml --no-deps --format-version 1
cd frontend && pnpm build
cd frontend && pnpm exec tsc --noEmit --pretty false
cd frontend && CI=1 pnpm lint
cargo check --manifest-path frontend/src-tauri/Cargo.toml
cargo test --manifest-path frontend/src-tauri/Cargo.toml codex_provider --lib
cargo test --manifest-path frontend/src-tauri/Cargo.toml openai::auth --lib
cargo test --manifest-path frontend/src-tauri/Cargo.toml teams_detection --lib
cargo test -p llama-helper
rg -n "0\\.5\\.0-alpha\\.1|0\\.5\\.0-alpha\\.2|Based on Meetily Community Edition 0\\.4\\.0|upstream_base_version" frontend/package.json frontend/src-tauri/Cargo.toml frontend/src-tauri/tauri.conf.json frontend/src/components README.md CHANGELOG.md NOTICE.md UPSTREAM.md docs/windows-release.md .github/workflows/clawscribe-windows-release.yml frontend/scripts/build-windows-release.ps1
```

Then run the secret and branding searches in this document, followed by the full
Windows checklist on a Windows recorder.
