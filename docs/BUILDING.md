# Building ClawScribe

The Windows x64 Tauri desktop app under `frontend/` is the primary release
target. No standalone Python/FastAPI service, Docker component, or manually
started whisper-server is required for local use.

## Toolchain

Use Git, Node.js 24, pnpm 10, Rust stable with the MSVC target, PowerShell 7,
Visual Studio Build Tools 2022 with C++/Windows SDK, WebView2, and CMake. Windows
release automation pins LLVM 20.1.8 and Vulkan SDK 1.4.309.0 for relevant GPU
builds. See [Windows releases](windows-release.md) for sidecar staging and full
prerequisites; use the toolchain in the checked-out workflow for parity.

## Development

From `frontend/`:

```powershell
pnpm install --frozen-lockfile
pnpm run tauri:dev
```

Use `pnpm run dev` for the web UI only. It does not run native capture or validate
Windows desktop integration. Use `pnpm run tauri:build` for a desktop build.
The default scripts call `scripts/tauri-auto.js`; explicit scripts are available
when validating an acceleration path:

```powershell
pnpm run tauri:dev:cpu
pnpm run tauri:dev:windows-gpu
pnpm run tauri:build:cpu
pnpm run tauri:build:windows-gpu
```

Equivalent `vulkan`, `directml`, `cuda`, and `openblas` suffixes select those
features. A feature in the binary is not proof that a specific machine or model
will successfully use its GPU; validate the actual backend.

## Frontend Checks

```powershell
pnpm typecheck
pnpm test
pnpm build
pnpm run verify:icons
```

Run these from `frontend/`; icon verification uses PowerShell. The configured
helper/control suite includes transcript display, scrolling, and speaker-save
regressions. Those deterministic tests do not replace rendered desktop checks.

`pnpm run lint` still invokes the legacy `next lint` command. Lint is not an
enforced, validated release gate in this branch; Next builds explicitly skip it.
Do not report a lint pass from successful typechecking or bundling.

## Native Checks

From the repository root, with the native prerequisites and sidecars installed:

```powershell
cargo check --locked --manifest-path frontend/src-tauri/Cargo.toml --features windows-gpu
cargo test --locked --manifest-path frontend/src-tauri/Cargo.toml --features windows-gpu --lib summary::processor::tests
cargo test --locked --manifest-path frontend/src-tauri/Cargo.toml --features windows-gpu --lib summary::chunking::tests
cargo test --locked --manifest-path frontend/src-tauri/Cargo.toml --features windows-gpu --lib audio::async_logger::tests
cargo test --locked --manifest-path frontend/src-tauri/Cargo.toml --features windows-gpu --lib audio::hardware_detector::tests
```

Omit GPU features only for a deliberate CPU-path test. The production chunker
can also be tested without desktop dependencies:

```powershell
rustc --edition=2021 --test frontend/src-tauri/src/summary/chunking.rs -o summary-chunking-tests.exe
.\summary-chunking-tests.exe
Remove-Item .\summary-chunking-tests.exe
```

The isolated core CI harness is useful for rapid regressions, but it does not
link the complete native application or exercise model inference. Preserve this
distinction when reporting results.

## Windows Installers

Build and stage the `llama-helper` sidecar first, following
[Windows releases](windows-release.md#build-prerequisites). Then from `frontend/`:

```powershell
.\scripts\build-windows-release.ps1 -CheckOnly
.\scripts\build-windows-release.ps1
```

The local script uses the Cargo metadata target directory for its
`release/bundle` output and requires both current-version MSI and NSIS installers.
The GitHub workflow normalizes upload paths separately. Do not infer a successful
build from old files left in a reused workspace.

## Repository Safety

From the repository root:

```powershell
node scripts/verify-public-repo-safety.mjs
git diff --check
```

Do not commit generated installers, models, logs, databases, keys, or private
workspace paths. Release metadata and notes must match the exact source that
produced the artifacts. See [meeting quality](meeting-quality.md) and
[real-device acceptance](windows-release.md#required-real-device-acceptance)
before treating a build as suitable for everyday meetings.
