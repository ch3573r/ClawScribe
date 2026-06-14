# ClawScribe Windows Release

This fork is packaged as **ClawScribe** for Windows.

- Tauri product name: `ClawScribe`
- Tauri identifier: `net.rismondo.openclaw.clawscribe`
- Bundle targets: `msi`, `nsis`
- OpenClaw default endpoint: `http://openclaw-host.local:8765/meetings/completed`

Build Windows artifacts on a Windows host with Visual Studio Build Tools,
Windows SDK, Rust, Node.js, pnpm, and LLVM installed.

```powershell
cd frontend
.\scripts\build-windows-release.ps1
```

The default build uses the `vulkan` feature for the Windows meeting recorder
target. Override when needed:

```powershell
.\scripts\build-windows-release.ps1 -Feature cpu
.\scripts\build-windows-release.ps1 -Feature cuda
.\scripts\build-windows-release.ps1 -Feature openblas
```

Run the validation-only path before a release build:

```powershell
.\scripts\build-windows-release.ps1 -CheckOnly
```

Artifacts are written under:

```text
frontend\src-tauri\target\release\bundle\msi\*.msi
frontend\src-tauri\target\release\bundle\nsis\*.exe
```

Authenticode signing is optional. Set `DIGICERT_KEYPAIR_ALIAS` in the build
environment to enable `frontend/src-tauri/scripts/sign-windows.ps1`; leave it
unset for unsigned local artifacts. Updater artifacts are intentionally disabled
until a ClawScribe release feed and signing key are provisioned.

Before handing an installer to a recorder laptop, create or update the
OpenClaw config file from [openclaw-handoff.md](openclaw-handoff.md) and set a
real `MEETILY_OPENCLAW_BEARER_TOKEN` user environment variable on that Windows
machine.
