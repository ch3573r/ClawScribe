# ClawScribe Productization Sprint Report

Date: 2026-06-15
Coordinator: Nora
Branch: `feat/clawscribe-productization-auth-theme-exports`
Baseline commit: `70628e0fc34dc8032bb42cc016180a312c28f20f`

This report covers the first coordinated productization pass. It does not claim
Windows runtime verification, Microsoft tenant verification, or live OAuth
verification.

Corrective alpha.2 handoff note:

- Current corrective candidate version is `0.5.0-alpha.2`.
- Preserve upstream attribution as "Based on Meetily Community Edition 0.4.0".
- Use `docs/productization/corrective-final-report-checklist.md` for the final
  integration report after provider, theme, Codex UX, icon, and packaging work
  lands.
- Windows artifact bundles must include `SHA256SUMS.txt` and
  `BUILD-METADATA.txt` with build commit, version, upstream base version, and
  build date.

## 1. Summary

Phase 0 inventory is recorded in `docs/productization/phase0-inventory.md`.
The repo is on the feature branch
`feat/clawscribe-productization-auth-theme-exports`.

Implemented in this pass:

- ClawScribe branding and versioning to `0.5.0-alpha.1`.
- About/legal upstream attribution for Meetily Community Edition `0.4.0`.
- Kontron-tokenized light theme and real dark theme support.
- Persisted Light / Dark / System theme selection.
- Teams detection verification guide and code-level test coverage.
- Codex-backed OpenAI login/processing provider with fake-Codex tests and
  Windows verification checklist.
- OpenAI API-key fallback and optional OpenClaw provider remain available.
- Microsoft Graph OneNote/Planner feasibility and implementation design docs.
- Release gate, QA baseline, and Windows final checklist docs.

Not implemented in this pass:

- Generic/private OpenAI OAuth. The supported advanced Codex target is a
  bundled/pinned Codex app-server runtime over stdio JSON-RPC.
- OS credential store migration for API keys and bearer tokens.
- Microsoft MSAL sign-in, live OneNote export, or live Planner task creation.
- Windows installer/runtime verification.
- Corrective alpha.2 final report completion.

## 2. Branch And Commits

- Branch: `feat/clawscribe-productization-auth-theme-exports`
- Current HEAD during this pass: `70628e0fc34dc8032bb42cc016180a312c28f20f`
- Worktree contains uncommitted coordinated productization changes.
- No GitHub Actions Windows installer build was triggered.

## 3. Features Implemented

### Branding, Version, And Legal

- `frontend/package.json`, `frontend/src-tauri/Cargo.toml`,
  `frontend/src-tauri/tauri.conf.json`, and `Cargo.lock` now use
  `0.5.0-alpha.1`.
- Product-visible UI strings in the main app surfaces were rebranded to
  ClawScribe.
- `README.md`, `CHANGELOG.md`, `NOTICE.md`, and `UPSTREAM.md` now describe the
  fork and upstream attribution.
- `frontend/src/components/About.tsx` now shows:
  - ClawScribe
  - product version fallback `0.5.0-alpha.1`
  - "Based on Meetily Community Edition 0.4.0"
  - a license-friendly fork disclaimer
- Remaining Meetily/Zackriya occurrences are classified in
  `docs/productization/branding-search-report.md`.

### Theme

- `frontend/src/app/globals.css` now contains centralized Kontron color tokens
  and dark theme tokens.
- `frontend/tailwind.config.ts` now maps semantic Tailwind colors through CSS
  variables.
- `frontend/src/lib/theme.ts` adds theme preference storage and resolution.
- `frontend/src/components/ThemeSettings.tsx` adds Light / Dark / System UI.
- `frontend/src/components/PreferenceSettings.tsx` mounts theme controls and
  tokenizes nearby OpenClaw/status styling.
- `frontend/src/app/layout.tsx` installs the startup theme initializer.
- Manual visual QA notes are in `docs/productization/theme-qa.md`.

### Teams Detection

- Existing detector code was inspected and code-level tests passed.
- Verification guide added at `docs/verification/teams-detection.md`.
- The current implementation is prompt-only; it does not automatically start or
  stop recording.
- Runtime status remains: implemented, pending Windows Teams runtime
  verification.

### QA And Release Gate

- `docs/productization/release-gate.md` defines Linux-side, Windows-side,
  secret, branding, and OpenClaw checks.
- `docs/productization/qa-baseline-results.md` records command results.
- `docs/verification/windows-final-checklist.md` gives Alex exact Windows
  verification steps.

## 4. Features Evaluated But Blocked

### Codex / ChatGPT Login

Implemented as a Codex-backed provider, not generic OAuth.

Documented at:

- `docs/auth/codex-auth.md`
- `docs/verification/codex-auth-windows.md`
- `docs/verification/alex-windows-codex-checklist.md`

Decision:

- Use Codex as the supported OpenAI/ChatGPT auth and runtime boundary.
- Do not implement private OpenAI OAuth endpoints.
- Do not scrape ChatGPT cookies.
- Do not extract Codex refresh/access tokens and reuse them as generic OpenAI
  API bearer tokens.
- Keep API-key mode and optional OpenClaw mode available.

Status:

- Codex provider implemented and fake-tested.
- Linux Codex binary and invocation smoke tested; real model execution is
  blocked on this host by Codex auth returning 401 Unauthorized.
- Windows login/runtime verification pending Alex after Windows build.

Next implementation task:

- Run the Alex Windows Codex checklist after the next Windows build.
- Add a backend credential-store abstraction and migrate OpenAI API keys,
  custom endpoint bearer tokens, and OpenClaw bearer tokens into OS credential
  storage while preserving backward-compatible reads.

### Microsoft Graph Exports

Documented at:

- `docs/integrations/microsoft-graph-evaluation.md`
- `docs/integrations/onenote-export.md`
- `docs/integrations/planner-export.md`
- `docs/productization/microsoft-export-test-plan.md`

Decision:

- Feasible design is delegated Microsoft Graph auth with separate Microsoft
  login for exports.
- Do not use app-only auth, Teams bot APIs, Copilot, or admin-consent-only
  design.

Blockers:

- No Microsoft credentials or test tenant were available.
- No existing MSAL/native Microsoft auth pattern exists in the repo.
- Live OneNote/Planner verification cannot be claimed.

Feasible next work without credentials:

- Mock Graph transport.
- Sanitized error mapper.
- OneNote HTML builder.
- Planner task request builder.
- `exports.json` ledger and idempotency tests.

## 5. Tests Run

All commands below were run after worker integration unless noted.

```bash
git diff --check
```

Result: pass.

```bash
cd frontend
pnpm build
```

Result: pass at `clawscribe@0.5.0-alpha.1`; Next.js compiled and generated 11
static pages.

```bash
python3 -m json.tool frontend/package.json >/dev/null
python3 -m json.tool frontend/src-tauri/tauri.conf.json >/dev/null
cargo metadata --manifest-path frontend/src-tauri/Cargo.toml --no-deps --format-version 1
```

Result: pass; Cargo metadata reports `clawscribe` version
`0.5.0-alpha.1`.

```bash
cargo test --manifest-path frontend/src-tauri/Cargo.toml openai::auth --lib
```

Result: pass; 9 passed, 0 failed.

```bash
cargo test --manifest-path frontend/src-tauri/Cargo.toml codex_provider --lib
```

Result: pass; 11 passed, 0 failed.

```bash
cargo test --manifest-path frontend/src-tauri/Cargo.toml --lib
```

Result: pass; 215 passed, 0 failed, 2 ignored. One existing precision issue in
`audio::device_detection::calculate_buffer_timeout` was fixed by using
`Duration::mul_f64(2.0)` instead of `mul_f32(2.0)`.

```bash
cargo test -p llama-helper
```

Result: pass; 2 passed, 0 failed.

```bash
cargo test --manifest-path frontend/src-tauri/Cargo.toml teams_detection --lib
```

Result: pass; 8 passed, 0 failed.

```bash
cargo test --manifest-path frontend/src-tauri/Cargo.toml codex_provider --lib
```

Result: pass for Codex app-server direction tests. The app no longer treats a
global CLI or `codex exec` smoke as the product integration path.

```bash
cd frontend
pnpm exec tsc --noEmit --pretty false
```

Result: fail on existing repo setup:

```text
tests/lib/blocknote-markdown.test.ts(1,57): error TS2307: Cannot find module 'bun:test' or its corresponding type declarations.
```

```bash
cd frontend
CI=1 pnpm lint
```

Result: fail because `next lint` prompts for initial ESLint setup.

```bash
rg -n --hidden --pcre2 -S "(sk-proj-[A-Za-z0-9_-]{20,}|sk-[A-Za-z0-9_-]{32,}|sk-ant-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{20,}|AIza[0-9A-Za-z_-]{30,})" --glob '!**/node_modules/**' --glob '!**/target/**' --glob '!frontend/out/**' --glob '!frontend/vs_buildtools.exe' --glob '!**/*.png' --glob '!**/*.jpg' --glob '!**/*.gif' .
```

Result: no real credential matches. The scan reported only documentation
placeholders, Codex redaction-test strings, and existing PostHog project keys
that still need a product/security review before stronger secret-hygiene claims.

```bash
python3 - <<'PY'
import json, pathlib, re
root=pathlib.Path('.')
package=json.loads((root/'frontend/package.json').read_text())
tauri=json.loads((root/'frontend/src-tauri/tauri.conf.json').read_text())
cargo=(root/'frontend/src-tauri/Cargo.toml').read_text()
print(package['name'], package['version'])
print(tauri['productName'], tauri['version'], tauri['identifier'])
print(re.search(r'^name\\s*=\\s*"([^"]+)"', cargo, re.M).group(1))
print(re.search(r'^version\\s*=\\s*"([^"]+)"', cargo, re.M).group(1))
PY
```

Result: pass; `clawscribe` / `ClawScribe` is version
`0.5.0-alpha.1` in package, Tauri, and Cargo metadata.

OpenClaw provider presence check:

```bash
rg -n "openclaw|OpenClaw|OpenClawProvider|Meeting Handoff|provider.*openclaw" \
  frontend/src-tauri/src frontend/src/components frontend/src/contexts docs
```

Result: pass for presence/backward-compatibility check. Existing OpenClaw
provider and handoff docs/config paths are still present. Windows OpenClaw
handoff runtime verification was not rerun in this pass.

## 5.1 Windows Build Readiness

This Linux OpenClaw host cannot directly produce trusted Windows Tauri
MSI/NSIS artifacts. The repository has two reproducible Windows build paths:

- Local Windows build script: `frontend/scripts/build-windows-release.ps1`
- GitHub Actions workflow: `.github/workflows/clawscribe-windows-release.yml`

Both paths document prerequisites, build commands, expected artifact names, and
checksum generation in `docs/windows-release.md`. The release script now writes
`target/release/bundle/SHA256SUMS.txt` next to the MSI/NSIS artifacts and the
workflow uploads that checksum file with the installer bundle.

No new Windows installer build was triggered in this pass. Trigger one
deliberately after Alex is ready to run
`docs/verification/alex-windows-codex-checklist.md`.

## 6. Windows Verification Status

Not run.

No claims are made for:

- Windows app launch.
- MSI/NSIS installer behavior.
- WebView2 behavior.
- Codex app-server browser login, device-code login, and Windows
  app-server runtime.
- Teams desktop/browser runtime detection.
- WASAPI microphone/system audio capture.
- Real short recording.
- Windows OpenClaw handoff from the recorder.
- Microsoft login.
- OneNote page creation.
- Planner task creation.

Use `docs/verification/windows-final-checklist.md` before any release claim.

## 7. Remaining Risks

- Secrets are still stored through existing local settings/config paths. This
  must be fixed before claiming production-grade credential handling.
- `frontend/src-tauri/src/api/api.rs` contains truncated-token logging. It is
  not a full token leak, but should be removed or explicitly accepted.
- A hardcoded PostHog project key exists in analytics code. It may be a public
  analytics key, but it should be reviewed before stronger secret-hygiene
  claims.
- Full `tsc --noEmit` is blocked by missing `bun:test` typings.
- `pnpm lint` is blocked by interactive ESLint setup.
- Dark mode is functional and tokenized, but some older screens still contain
  hard-coded one-off classes. Visual QA is required on Windows.
- Microsoft Graph integration is design-only in this pass.
- The recording folder remains `meetily-recordings` for compatibility.

## 8. Exact Verification Instructions For Alex

### Teams Detection

Follow `docs/verification/teams-detection.md`.

Core command in DevTools:

```js
const status = await window.__clawscribeTeamsDetection.printStatus()
copy(JSON.stringify(status, null, 2))
status
```

Expected: positive Teams meeting cases return `detected=true` and
`nextRecommendedAction="promptToRecord"` only. No automatic recording starts.

### OpenAI Login

Follow:

- `docs/auth/codex-auth.md`
- `docs/verification/alex-windows-codex-checklist.md`

Expected in this pass:

- Advanced: Codex app-server uses a bundled/pinned app-server runtime as the
  auth and runtime boundary.
- Direct OpenAI uses an API key.
- OpenClaw Gateway uses the OpenClaw model bridge and bearer token.
- Status wording until Alex verifies Windows:

```text
Codex app-server provider direction implemented and fake-tested. Bundled runtime packaging implemented; Windows app-server auth/runtime verification pending.
```

### Microsoft Login

Not implemented. Use
`docs/integrations/microsoft-graph-evaluation.md` to review the intended
delegated-auth design.

### OneNote Export

Not implemented. Use `docs/integrations/onenote-export.md` for the intended
Graph page format and failure behavior.

### Planner Task Export

Not implemented. Use `docs/integrations/planner-export.md` for the intended
plan/bucket/idempotency behavior.

### OpenClaw Handoff

Use `docs/productization/release-gate.md` and `docs/openclaw-handoff.md`.

Minimum Windows recorder check:

```powershell
Invoke-RestMethod http://openclaw.local:8765/readyz
```

Then record a short meeting with real spoken content, wait for handoff, and
confirm:

- `.openclaw-submitted.json` appears in the recording folder.
- OpenClaw ingest receives non-empty transcript content.
- Bad endpoint/bad token failure markers are sanitized.

## 9. Known Limitations

- This is not a release candidate yet.
- It is a coordinated productization pass with docs, branding, theme, and
  verification scaffolding.
- Live OpenAI OAuth, Microsoft auth, OneNote export, Planner export, and Windows
  runtime checks remain pending.

## 10. Next Recommended Sprint

1. Implement OS credential storage and migrate existing provider secrets.
2. Remove or harden truncated-token logging.
3. Fix `bun:test` typings or isolate Bun tests from `tsc --noEmit`.
4. Add non-interactive ESLint config or replace the lint command.
5. Implement Microsoft Graph mock transport, OneNote HTML builder, Planner task
   builder, `exports.json`, and idempotency tests.
6. Add a Teams detection status panel or menu item backed by the existing
   prompt-only detector.
7. Run the Windows final checklist on the recorder.
8. Only after that, trigger one deliberate Windows installer build.
