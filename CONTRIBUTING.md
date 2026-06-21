# Contributing To ClawScribe

ClawScribe is a Windows-first, local-first meeting recorder and transcription
app built with Tauri, Rust, Next.js, and local/optional AI providers. It is based
on Meetily Community Edition, but this repository is no longer the upstream
community app. Keep new contribution docs, issues, branches, and PRs oriented
around ClawScribe.

## Branches And Pull Requests

- `main` is the active development and release branch.
- Create feature branches from the latest `main`.
- Open pull requests back into `main` unless a maintainer explicitly asks for a
  different target.
- Keep PRs focused. Avoid bundling unrelated UI, export, model, release, and
  cleanup work into one commit.
- Rebase or merge latest `main` before asking for review when the branch has
  drifted.

## Local Setup

Required tools for normal Windows development:

- Git
- Rust stable, MSVC toolchain
- Node.js
- pnpm
- PowerShell
- WebView2 runtime
- Windows SDK / build tools when producing installers

Install dependencies:

```powershell
cd frontend
pnpm install
```

Run the Tauri desktop app:

```powershell
cd frontend
pnpm run tauri:dev
```

Run the web UI only:

```powershell
cd frontend
pnpm run dev
```

## Validation

Run the smallest validation set that matches your change. For broad changes,
run more than one layer.

Frontend:

```powershell
cd frontend
pnpm run lint
pnpm run build
```

Rust workspace:

```powershell
cargo check
```

Tauri app:

```powershell
cd frontend
pnpm run tauri:build
```

Windows release smoke checks:

```powershell
cd frontend
.\scripts\build-windows-release.ps1 -CheckOnly
pnpm run verify:icons
```

Targeted Rust tests are acceptable for focused export/model changes, for
example:

```powershell
cargo test --lib exports::
```

## Areas That Need Extra Care

### Recording And Transcription

- Preserve the difference between live recording and import workflows unless a
  change explicitly intends to alter both.
- Keep local transcription local by default.
- For Parakeet and Nemotron changes, document model variant behavior, execution
  provider behavior, and fallback behavior in logs and user-facing text.
- Do not assume DirectML correctness without a self-test or measured hardware
  run.

### Microsoft 365

- Keep Microsoft auth separate from OpenAI, OpenClaw, Codex, and other summary
  provider auth.
- Request only the Graph scopes used by the code.
- OneNote section listing can fail on large OneDrive/SharePoint libraries due
  to Graph's 5,000-item limit. The reliable export path is creating a fresh
  dated section or using a previously known section ID.
- Planner exports should stay review-first. Never create tasks silently from an
  AI summary without user review.

### Atlassian And Proxied Services

- Direct Confluence/Jira API access depends on tenant reachability and auth. A
  browser session, PAT, SSO cookie, and Entra App Proxy token are not
  interchangeable.
- Keep the browser-draft Confluence path available for hosted/proxied instances
  where direct REST calls are blocked.

### UI And Product

- Respect both dark and light themes.
- Respect the user's selected accent color.
- Keep text and controls responsive across narrow and wide app layouts.
- Avoid reintroducing stale Meetily branding except where compatibility paths or
  upstream attribution require it.

## Documentation

- Update `README.md` when user-facing features, supported providers, exports,
  model variants, install behavior, or release/update behavior changes.
- Update targeted docs under `docs/` when implementation details change.
- Remove or archive stale docs instead of leaving contradictory guidance.
- Use `ClawScribe` for product-facing language. Use `Meetily` only for upstream
  attribution or compatibility storage paths.

## Release Notes

Every GitHub Release must include descriptive release notes. Do not publish a
release that only contains binaries or generic build metadata.

Release notes should cover:

- User-facing features
- Fixed bugs
- Export/model/update behavior changes
- Known caveats
- Whether the updater metadata points to the intended installer

Keep updater-facing notes and release notes aligned.

## Security And Secrets

Never commit:

- `.env` files
- API keys, bearer tokens, PATs, refresh tokens, session cookies, certificates,
  or private keys
- Local auth stores
- App logs
- Local databases
- Generated installers or build artifacts
- Machine-specific paths unless they are clearly examples

Use placeholders such as `example.com`, `openclaw.local`, or
`<redacted-token>` in docs and tests. Store real secrets in the OS credential
store, environment variables, or local ignored config only.

## Commit Messages

Use short, descriptive commit messages. Conventional prefixes are helpful but
not mandatory:

```text
feat(scope): add calendar attendance checklist
fix(exports): avoid OneNote section listing on large libraries
docs(readme): refresh feature set
chore(release): update Windows signing metadata
```

## Pull Request Checklist

Use this as the minimum review checklist:

```markdown
## Summary
- Summary of the change

## Testing
- [ ] Frontend validation
- [ ] Rust validation
- [ ] Manual app check, if relevant
- [ ] Release/update check, if relevant

## Risk
- [ ] Recording/transcription behavior considered
- [ ] Export/auth scopes considered
- [ ] Dark/light theme checked for UI changes
- [ ] README/docs updated or not needed
- [ ] No secrets, logs, installers, or local machine artifacts committed
```

## License

By contributing, you agree that your contributions are licensed under the MIT
License used by this repository. Upstream Meetily attribution is maintained in
`UPSTREAM.md`, `NOTICE.md`, and `LICENSE.md`.
