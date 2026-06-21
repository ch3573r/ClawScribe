# ClawScribe Windows Release

This fork is packaged as **ClawScribe** for Windows.

- Tauri product name: `ClawScribe`
- Tauri identifier: `net.rismondo.openclaw.clawscribe`
- Windows publisher/manufacturer: `OpenClaw`
- MSI upgrade code: `8b6aff03-4baa-5d80-9239-e65d85c288d3`
- Bundle targets: `msi`, `nsis`
- Optional OpenClaw endpoint example: `http://openclaw.local:8765/meetings/completed`

Build Windows artifacts on a Windows host with Visual Studio Build Tools,
Windows SDK, Rust, Node.js, pnpm, and LLVM installed. The release script must
be run from `frontend`.

## Prerequisites

Install these on the Windows build host:

- Windows 10/11 or Windows Server with WebView2 Runtime.
- Visual Studio Build Tools 2022 with **Desktop development with C++**.
- Windows 10/11 SDK from Visual Studio Installer.
- Node.js 20.
- pnpm 10.
- Rust stable with the MSVC target:

```powershell
rustup target add x86_64-pc-windows-msvc
```

- LLVM/Clang, with `LIBCLANG_PATH` pointing at the LLVM `bin` directory if
  bindgen cannot find it automatically.
- CMake if any native dependency in the selected feature path requires it.

Stage the Windows `llama-helper` sidecar before running the Tauri bundle:

```powershell
cd <repo-root>
cargo build -p llama-helper --release --target x86_64-pc-windows-msvc
Copy-Item .\target\x86_64-pc-windows-msvc\release\llama-helper.exe .\frontend\src-tauri\binaries\llama-helper-x86_64-pc-windows-msvc.exe -Force
```

The release script stages the pinned Codex app-server sidecar automatically for
the Advanced Codex provider:

```powershell
cd frontend
.\scripts\stage-codex-runtime.ps1
```

The pinned runtime metadata is in [codex-runtime.md](codex-runtime.md). The
Windows build verifies both the source package SHA256 and runtime executable
SHA256 before bundling.

FFmpeg is downloaded and cached by `frontend/src-tauri/build.rs` during the
Tauri build as `frontend/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe`.

Run the validation-only path before a release build:

```powershell
cd frontend
.\scripts\build-windows-release.ps1 -CheckOnly
```

Build both installer formats:

```powershell
cd frontend
.\scripts\build-windows-release.ps1
```

If you do not have a Windows build machine, use GitHub Actions instead:

1. Push this branch to GitHub.
2. Open **Actions**.
3. Run **ClawScribe Windows Release** manually.
4. Use `cpu` for the first build unless you explicitly need GPU acceleration.
5. Download the `clawscribe-windows-<feature>` artifact from the completed run.

The workflow builds on GitHub's `windows-latest` runner, stages the
`llama-helper-x86_64-pc-windows-msvc.exe` sidecar, stages the pinned Codex
app-server runtime, runs `frontend\scripts\build-windows-release.ps1`, and
uploads the generated MSI and NSIS installers. The artifacts are unsigned unless
Windows signing secrets are added in a future release workflow.

The default build uses the `vulkan` feature for the Windows meeting recorder
target. Override when needed:

```powershell
.\scripts\build-windows-release.ps1 -Feature cpu
.\scripts\build-windows-release.ps1 -Feature cuda
.\scripts\build-windows-release.ps1 -Feature openblas
```

Artifacts are written under:

```text
frontend\src-tauri\target\release\bundle\msi\*.msi
frontend\src-tauri\target\release\bundle\nsis\*.exe
frontend\src-tauri\target\release\bundle\SHA256SUMS.txt
frontend\src-tauri\target\release\bundle\BUILD-METADATA.txt
```

Expected artifact names currently look like:

```text
ClawScribe_0.5.7-alpha.1_x64_en-US.msi
ClawScribe_0.5.7-alpha.1_x64-setup.exe
SHA256SUMS.txt
BUILD-METADATA.txt
```

`BUILD-METADATA.txt` records the ClawScribe version, build commit, upstream
Meetily base version, Codex runtime version, Codex runtime SHA256, source
package, source URL, and license.

The release script generates `SHA256SUMS.txt` after a successful installer
build. Checksum entries are relative to the bundle root, for example
`msi/ClawScribe_0.5.7-alpha.1_x64_en-US.msi`, so this command verifies cleanly
from `frontend\src-tauri\target\release\bundle`:

```powershell
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $parts = $_ -split '\s+', 2
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $parts[1]).Hash.ToLowerInvariant() -ne $parts[0]) {
        throw "Checksum mismatch: $($parts[1])"
    }
}
```

For a local ad-hoc checksum, run:

```powershell
Get-FileHash -Algorithm SHA256 .\src-tauri\target\release\bundle\msi\*.msi
Get-FileHash -Algorithm SHA256 .\src-tauri\target\release\bundle\nsis\*.exe
```

Portable/no-install execution is not the normal release path. For a developer
smoke without installing, run the Tauri dev app:

```powershell
cd frontend
pnpm install --frozen-lockfile
pnpm tauri dev
```

or run a built app executable directly from the Tauri release output if the
bundle step produced one:

```powershell
.\src-tauri\target\release\ClawScribe.exe
```

Authenticode signing is optional. Set `DIGICERT_KEYPAIR_ALIAS` in the build
environment to enable `frontend/src-tauri/scripts/sign-windows.ps1`; leave it
unset for unsigned local artifacts. Updater artifacts are intentionally disabled
until a ClawScribe release feed and signing key are provisioned.

Unsigned artifacts will show an unknown-publisher / SmartScreen warning on
Windows. That is expected for private test builds. Public-friendly installs
require an Authenticode code-signing certificate and a release signing pipeline.

Before handing an installer to a recorder laptop, create or update the
OpenClaw config file from [openclaw-handoff.md](openclaw-handoff.md) and set a
real `MEETILY_OPENCLAW_BEARER_TOKEN` user environment variable on that Windows
machine.

## Smoke Checklist

Use a clean Windows user profile when practical, or uninstall the previous
ClawScribe build first.

1. Install either `frontend\src-tauri\target\release\bundle\msi\*.msi` or
   `frontend\src-tauri\target\release\bundle\nsis\*.exe`.
2. Confirm Windows lists the app as `ClawScribe` and publisher/manufacturer as
   `OpenClaw` where the installer surface exposes it.
3. Launch `ClawScribe`, select the local transcription model, and start a short
   recording that captures both microphone and system audio.
4. Stop the recording and confirm a new folder appears under
   `%USERPROFILE%\Music\meetily-recordings` with `metadata.json`,
   `transcripts.json`, and audio artifacts.
5. In model settings, select a standalone summary provider first. Recommended
   release smoke path: `Custom OpenAI` with an OpenAI-compatible endpoint,
   model, and API key, or `Built-in AI` if the bundled local summary model is
   already downloaded.
6. Generate a summary for the smoke recording and confirm the meeting detail
   page shows a non-empty summary plus action items.
7. Optional OpenClaw provider smoke: configure [openclaw-handoff.md](openclaw-handoff.md),
   set `MEETILY_OPENCLAW_BEARER_TOKEN` as a user environment variable, restart
   `ClawScribe`, select `OpenClaw managed auth`, refresh status, then generate
   a summary and confirm `.openclaw-submitted.json` appears in the recording
   folder.

## Windows-Only Blockers

These items cannot be fully validated from Linux:

- Visual Studio Build Tools and Windows SDK availability.
- `cargo check` and `tauri build` against `x86_64-pc-windows-msvc`.
- Windows `llama-helper-x86_64-pc-windows-msvc.exe` sidecar build/staging.
- WiX/MSI and NSIS installer generation, installation, uninstall, and upgrade
  behavior.
- WASAPI microphone/system-audio recording and WebView2 runtime behavior.
- Authenticode signing through DigiCert `smctl`, if signing is enabled.
