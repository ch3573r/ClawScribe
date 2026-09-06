# ClawScribe Windows Releases

The primary release target is the Windows x64 Tauri desktop app. Product name:
`ClawScribe`; identifier: `net.rismondo.openclaw.clawscribe`; installer publisher:
`OpenClaw`; MSI upgrade code: `8b6aff03-4baa-5d80-9239-e65d85c288d3`.
Installer targets are NSIS setup and MSI.

## Build Prerequisites

Use Windows with Visual Studio Build Tools 2022 (Desktop development with C++),
a Windows SDK, WebView2, Rust stable/MSVC, Node.js 24, pnpm 10, PowerShell 7, and
CMake. The workflow pins LLVM 20.1.8 for bindgen compatibility and Vulkan SDK
1.4.309.0 for the `windows-gpu` and `vulkan` paths. Set `LIBCLANG_PATH` to the
LLVM `bin` directory when needed. Use the versions declared by the checked-out
workflow rather than silently changing the native toolchain during a release.

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build -p llama-helper --release --locked --target x86_64-pc-windows-msvc
New-Item -ItemType Directory -Force frontend/src-tauri/binaries | Out-Null
Copy-Item target/x86_64-pc-windows-msvc/release/llama-helper.exe frontend/src-tauri/binaries/llama-helper-x86_64-pc-windows-msvc.exe -Force
```

Run those commands from the repository root. The local release script requires
this prebuilt helper; it does not compile it for you. It stages the sherpa
runtime and, for installer builds, the pinned Codex runtime. See
[codex-runtime.md](codex-runtime.md). The native build stages FFmpeg.

## Local Validation And Packaging

From `frontend/`:

```powershell
.\scripts\build-windows-release.ps1 -CheckOnly
.\scripts\build-windows-release.ps1
```

The default feature set is `windows-gpu`: Whisper Vulkan plus DirectML for the
supported ONNX/sherpa paths. `-Feature cpu`, `directml`, `vulkan`, `cuda`, and
`openblas` are explicit alternatives and need their own prerequisite checks.

The validation path runs locked Rust checking, frontend typechecking, and
frontend tests. Packaging checks the frontend, builds installers, requires both
current-version formats, and writes build identity and SHA-256 checksums. A
failed native command must stop the script. These checks do not exercise a real
microphone, meeting application, or graphical installer interaction.

The local script resolves the Cargo target directory through `cargo metadata`;
its bundle root is `<cargo-target-directory>/release/bundle`. Do not assume the
member crate has its own target directory. The GitHub workflow normalizes its
artifacts into `frontend/src-tauri/target/release/bundle` before upload.

## Windows Runtime Prerequisite

The `windows-gpu` and `vulkan` builds have a load-time dependency on
`vulkan-1.dll`. A machine without the loader can fail before the app opens or
native tests are discovered, even if no GPU inference is requested. Use a
supported GPU driver or the official LunarG Vulkan runtime. Do not download
individual DLLs from third-party sites. The 0.5.36 installer does not install
a GPU driver or the Vulkan runtime for you.

Extracting the Vulkan SDK supplies headers, libraries, and build tools; it does
not run its bundled `Helpers/VulkanRT.exe`. The release workflow explicitly
runs the signed runtime installer and verifies the loader before building.
This supplies the build machine's loader prerequisite; it does not establish
GPU availability or performance.

From the repository root in **64-bit PowerShell 7**, probe without installing:

```powershell
.\frontend\scripts\ensure-windows-vulkan-runtime.ps1
```

On a development/build machine with the pinned SDK staged and `VULKAN_SDK` set,
opt in to installation (administrative permission may be required):

```powershell
.\frontend\scripts\ensure-windows-vulkan-runtime.ps1 -InstallFromSdk
```

The helper requires a valid LunarG Authenticode signature, checks the installer's
exit code, and verifies the loader export. Native-test invocation itself only
probes and never installs prerequisites implicitly. Installing a loader does
not establish a compatible GPU, successful inference, or live audio capture.

## Native Windows Unit Tests

From the repository root, after staging the required sidecars:

```powershell
.\frontend\scripts\stage-sherpa-runtime.ps1 -TauriRoot frontend/src-tauri -Runtime directml
.\frontend\scripts\test-windows-native.ps1 -Features windows-gpu
```

The helper compiles the actual release-profile library test executable and loads
the same staged sherpa/ONNX DLL set used by the installer. It rejects missing
DLLs, zero matched tests, and failing test results. It does not perform live
capture, GUI interaction, or model-quality benchmarking.

## GitHub Actions

**ClawScribe Windows Release** is the manual/reusable build workflow. Installer
builds and native Windows diagnostics run exclusively on the designated local
Windows x64 machine. There is no GitHub-hosted fallback: if the runner is
offline, jobs wait for it.

Assign the `clawscribe` label only to that machine's repository runner. Set the
repository Actions variable `CLAWSCRIBE_BUILD_RUNNER` to both its registered
runner name and Windows computer name. Before checking out any source, the
workflow requires a self-hosted environment and verifies both names against
that variable. Missing or mismatched configuration stops the job. Keep actual
machine names in repository settings, never in committed configuration.

Only trusted maintainer code may run on this persistent machine. Native Windows
diagnostics use manual dispatch and trusted branch pushes; pull requests run
the separate syntax and regression checks on standard GitHub-hosted Ubuntu
runners. Those validation jobs are free while the repository is public and do
not produce application installers. Reassess this policy before changing
repository visibility or using larger runners.

Important inputs:

| Input | Meaning |
| --- | --- |
| `build-ref` | Exact commit, branch, or existing tag to build. Prefer an immutable commit SHA. |
| `feature` | `windows-gpu` for the normal Windows GPU build. |
| `check-only` | Validate without producing installers. |
| `publish` | Publish to the stable release channel; requires real capture confirmation. |
| `draft-release` | Stage unpublished release assets for verification. Mutually exclusive with `publish`. |
| `capture-smoke-confirmed` | Assert only after the actual microphone/system-audio smoke test described below. |

Normal test builds leave both `publish` and `draft-release` false. Installers
and verification metadata remain in the local runner checkout's
`frontend/src-tauri/target/release/bundle`. They are not uploaded to Actions;
copy needed outputs locally before a later build replaces them. Dependency,
SDK, and build caches also remain local, with Actions caching disabled.
Draft builds upload to an explicitly requested draft GitHub Release instead.
Stable builds publish release assets and advance the `latest` update channel.
The workflow builds sidecars, verifies icons, runs frontend checks, creates both
installers, and then runs the selected summary, chunker, logger, and hardware
tests in the native Windows crate before staging release assets.

**Stage Windows release candidate** runs on `release/**` pushes and can be run
manually. It validates frontend code, uses the designated local Windows GPU build, and
requests a draft. A successful job named `validate` alone is not a successful
Windows build; the native `stage` job must also succeed. Avoid pushing unrelated
changes to a building release branch because concurrency cancels its prior run.

The legacy cross-platform installer workflows have been retired. Disable their
entries in GitHub Actions as well as historical automation workflows that can
still build from old branches. Keep the release, candidate, and native diagnostic
workflows disabled while deploying a change to runner policy; enable them only
after the local-only configuration and runner variable are present on the
intended ref. Do not dispatch or rerun historical refs that select hosted builds.

### GitHub Storage And Spending

The current workflows do not upload Actions artifacts or caches. Source archives
are available from GitHub by commit, so readiness checks do not upload duplicate
source archives. Existing artifacts and caches from earlier runs remain until
expiration or deliberate cleanup; changing a workflow does not delete them.
Release assets remain the explicitly requested distribution path.

Check the account and organization billing pages separately. Set metered-product
budgets to zero with usage blocking when no overage is intended, and inspect
Actions storage, Codespaces (including stopped instances and prebuilds), Packages,
Git LFS, larger runners, AI-credit budgets, and paid subscriptions. An online local
runner does not establish that unrelated GitHub products are free. Account billing
and some resource inventories require permissions beyond repository access.

See [GitHub Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)
and [budgets and alerts](https://docs.github.com/en/billing/concepts/budgets-and-alerts)
for current allowances and blocking behavior.

## Release Identity And Notes

Land implementation changes before metadata-only release preparation. Keep the
application version aligned in `frontend/package.json`,
`frontend/src-tauri/Cargo.toml`, `frontend/src-tauri/tauri.conf.json`, and the
application entry in the workspace `Cargo.lock`. Do not upgrade dependencies as
an incidental release step.

Use a `## <source-version>` section in `CHANGELOG.md`. The workflow extracts that
section into release notes. Notes must describe changes since the previous
published tag, not everything in the product. Distinguish prompt changes from
measured model quality, helper tests from native integration tests, and updater
signatures from Authenticode signing. Record remaining validation explicitly.

An existing version tag on a different commit is a stop condition. Never move
an already published tag or replace its installers with a different build under
the same version. Keep `BUILD-METADATA.txt` tied to the actual build commit. If
subsequent documentation changes are not rebuilt, identify the documentation
commit separately instead of relabeling the binaries.

## Artifacts And Signing

For source version 0.5.36, installer filenames are:

```text
ClawScribe_0.5.36_x64-setup.exe
ClawScribe_0.5.36_x64_en-US.msi
```

Release bundles also include `SHA256SUMS.txt`, `BUILD-METADATA.txt`, and
`BUILD-METRICS.json`; updater-capable releases include `latest.json` containing
the NSIS URL and its Tauri signature. Verify that the assets exist and are
nonempty, their checksums match, and metadata identifies the expected commit
and version before publication. A workflow status is not a substitute for
checking the uploaded assets.

Both the local script and workflow write bundle-relative checksum paths,
including `nsis/` and `msi/` directory prefixes. For a flat download from GitHub,
place the installers in the subdirectories named by the checksum entries, or
verify each downloaded asset against its corresponding hash explicitly. Run
the following from the directory matching those entries:

```powershell
Get-Content .\SHA256SUMS.txt | ForEach-Object {
    $parts = $_ -split '\s+', 2
    if ($parts.Count -ne 2) { throw 'Malformed checksum entry' }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $parts[1]).Hash.ToLowerInvariant()
    if ($actual -ne $parts[0]) { throw "Checksum mismatch: $($parts[1])" }
}
```

Authenticode and updater signing are separate. `DIGICERT_KEYPAIR_ALIAS` enables
the optional DigiCert signing script. Without it, installers are not
Authenticode-signed and Windows may display an unknown-publisher/SmartScreen
warning. Do not describe an updater-signed installer as Authenticode-signed.
Never bypass an unexpected signature or checksum mismatch.

Tauri updater signatures require `TAURI_SIGNING_PRIVATE_KEY`; draft/stable
staging that generates updater metadata fails when the expected signature is
missing. Do not publish an empty signature to work around that failure.

## Stable Update Channel

The app checks the latest non-draft, non-prerelease release's `latest.json`.
Drafts are unpublished. A public prerelease is for manual testing and is not a
stable update. Do not promote a candidate to stable solely because it compiled.

Windows installer/runtime versions cannot distinguish source prerelease suffixes
in this workflow. A candidate and final release sharing the same numeric runtime
version are not successive in-app updates. Reuse the verified binaries only
when they are genuinely unchanged; otherwise increment the numeric version.

## Required Real-Device Acceptance

These checks are required before stable-channel promotion. A clearly labeled
public prerelease may be distributed for manual evaluation after the native
build/tests and artifact integrity checks pass, without advancing the stable
updater. Its notes must state the missing real-device acceptance. Publishing a
preview must not assert `capture-smoke-confirmed` or claim stable readiness.

Use a separate test profile or a backed-up installation; do not delete meeting
data to test installation. Record the application commit, engine/model, Windows
version, audio devices, and result without publishing private meeting content.

1. Install the candidate. Start a recording that captures microphone and remote
   system audio. Verify both sources in the saved playback, stop cleanly, and
   start a second recording. The publication confirmation must cover the active
   `cpal` revision in `Cargo.lock`; never check it based on a mock or unit test.
2. Verify retained meeting files, transcript playback, speaker-label editing,
   visible save failure/retry behavior, keyboard focus, scaling, and live-follow
   interruption/recovery. Test upgrade without losing existing meetings.
3. Generate notes from the recording using the intended provider. Check factual
   decisions/actions against the transcript, including names, negation, numbers,
   uncertain points, and missing owners/deadlines. Do not require invented action
   items when none were agreed.
4. Run a sustained Teams/Webex session on the target notebook with its normal
   workload. Observe RAM, transcription backlog, CPU contention, and stop time;
   include device changes, sleep/wake, and failure recovery in acceptance testing.

Optional hosted-provider verification is documented in
[hosted-transcription-smoke.md](hosted-transcription-smoke.md). OpenClaw handoff
verification is in [openclaw-handoff.md](openclaw-handoff.md); it is not required
for standalone local recording.
