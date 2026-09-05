# ClawScribe Frontend And Desktop Shell

The Next.js/React UI and Rust/Tauri desktop shell for source version **0.5.36**.
Windows x64 is the primary release target. See the root [README](../README.md)
for product features and [GitHub Releases](https://github.com/ch3573r/ClawScribe/releases)
for actual installer availability and validation status.

## Prerequisites

Use Node.js 24, pnpm 10, Rust stable, PowerShell 7, Visual Studio Build Tools
with C++/Windows SDK, and WebView2 on Windows. Native builds also require the
LLVM/CMake and acceleration prerequisites described in
[Building ClawScribe](../docs/BUILDING.md). Stage `llama-helper` before using the
local Windows release script as documented in
[Windows releases](../docs/windows-release.md).

macOS source development additionally needs Xcode command-line tools; macOS and
Linux code paths do not imply the same release validation as Windows. Use the
package scripts rather than historical cleanup scripts for normal development.

## Commands

From this directory:

```powershell
pnpm install --frozen-lockfile
pnpm run tauri:dev
```

Web UI only, without native recording:

```powershell
pnpm run dev
```

Frontend checks and static production output:

```powershell
pnpm typecheck
pnpm test
pnpm build
```

Desktop build:

```powershell
pnpm run tauri:build
```

The default Tauri scripts use `scripts/tauri-auto.js`. For an explicit Windows
GPU path use `pnpm run tauri:dev:windows-gpu` or
`pnpm run tauri:build:windows-gpu`. CPU, Vulkan, DirectML, CUDA, and OpenBLAS
variants also exist and require the matching native dependencies.

For reproducible installer validation and packaging:

```powershell
.\scripts\build-windows-release.ps1 -CheckOnly
.\scripts\build-windows-release.ps1
```

The release script checks command exit codes and requires both current-version
installer formats. Tauri's `beforeBuildCommand` builds the frontend; a green
TypeScript check alone is not a successful native release.

## Source Layout

```text
src/           React components, routes, hooks, and services
src-tauri/     Native capture, transcription, persistence, summary, and exports
public/        Static assets
scripts/       Development, staging, and packaging utilities
tests/lib/     Frontend helper/control regression tests
```

There is no separately started FastAPI service or whisper-server. Native
operations flow through Tauri commands and events. The web-only UI cannot
validate microphone capture, Windows permissions, or installed-app behavior.

## Meeting Review

The transcript view preserves recognized words, supports timestamp playback,
and offers **Jump to live transcript** after scrolling back during recording.
Speaker edits retain custom input on failure and prevent duplicate submissions.
See [meeting quality](../docs/meeting-quality.md) for summary and notebook limits.

Frontend control tests use deterministic hook/event mocks; they are not a browser
or Windows accessibility certification. The shared Rust summary processor has
its own unit tests, and release staging runs selected tests in the native crate.

## Cloud Verification

Hosted transcription is beta and opt-in. For an actual provider check:

```powershell
pnpm run test:cloud-live
```

See [hosted transcription smoke](../docs/hosted-transcription-smoke.md) for the
required audio file and local credential configuration. Never commit credentials
or private meeting recordings as fixtures.

## Troubleshooting

For build failures, first check the failing command and the matching native
prerequisites. For capture failures, check Windows microphone privacy settings
and the meeting application's actual output device. Verify both sources in saved
playback. Run the application as the normal user; elevation is not a general
fix for audio or startup problems and may change the user's storage/credential
context. Preserve existing meeting data when testing repairs.

See [architecture](../docs/architecture.md), [GPU acceleration](../docs/GPU_ACCELERATION.md),
and [contributing](../CONTRIBUTING.md). License: [MIT](../LICENSE.md).
