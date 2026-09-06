# Local Test Builds

The historical DevTest workflow has been retired. Use **ClawScribe Windows
Release** on the designated local Windows runner for test installers.

- Select `windows-gpu` for the normal Windows build.
- Use `check-only=true` for validation without installers.
- Leave `publish` and `draft-release` false for ordinary local test builds.
- Installers and verification metadata stay in the runner checkout's
  `frontend/src-tauri/target/release/bundle`; copy outputs locally before a later
  build replaces them.
- Request a draft or published release explicitly when GitHub distribution is
  needed. Test builds do not upload Actions artifacts or caches.

See [Windows releases](../../docs/windows-release.md) for prerequisites, runner
setup, signing, and capture acceptance, or [the workflow overview](WORKFLOWS_OVERVIEW.md)
for automatic validation triggers.
