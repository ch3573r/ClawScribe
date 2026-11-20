# ClawScribe Windows Releases

The primary release target is the Windows x64 Tauri desktop app. Product name:
`ClawScribe`; identifier: `net.rismondo.openclaw.clawscribe`; installer publisher:
`OpenClaw`; MSI upgrade code: `8b6aff03-4baa-5d80-9239-e65d85c288d3`.
Installer targets are NSIS setup and MSI.

## Build Prerequisites

### CPU Portability Gate

The release and native-test scripts source `configure-windows-portability.ps1`.
Rust uses `x86-64-v2`; Whisper's native build keeps AVX2 but disables host-native
tuning and every AVX-512 variant through `portable-ggml.cmake`. Conflicting Rust
CPU overrides fail before compilation. Vulkan and DirectML remain separate
acceleration paths under `windows-gpu`.

The Windows Tauri `beforeBundleCommand` and native-test helper inspect Whisper
CMake caches with `verify-windows-portability.mjs`. Missing caches or any stale
incompatible cache fail the gate. Because the designated runner keeps its target
directory, the release workflow first runs the script with `--remove-stale`,
which deletes only Whisper build outputs whose cache fails the gate, together
with their Cargo fingerprints, so those build scripts rerun. Locally, run the
same command from the repository root or use a fresh Cargo target directory; do
not bypass the check to reuse a host-tuned cache. This gate verifies build
configuration and does not replace testing on supported CPUs or the
dual-source recording smoke.

### Toolchain

Use Windows with Visual Studio Build Tools 2022 (Desktop development with C++),
a Windows SDK, WebView2, Rust stable/MSVC, Node.js 24.21.0, pnpm 10, PowerShell 7, and
CMake. The workflow pins LLVM 20.1.8 for bindgen compatibility and Vulkan SDK
1.4.309.0 for the `windows-gpu` and `vulkan` paths. Node is pinned to include the
isolated test-runner transport fix; keep process isolation and assertions enabled.
Set `LIBCLANG_PATH` to the
LLVM `bin` directory when needed. Use the versions declared by the checked-out
workflow rather than silently changing the native toolchain during a release.

The persistent runner keeps its toolchain. **Native library tests** therefore
verifies it with `frontend/scripts/ensure-native-toolchain.ps1` (the Rust channel
from `rust-toolchain.toml` with the MSVC target, LLVM 20.1.8, the Vulkan SDK 1.4.309.0
and a loadable `vulkan-1.dll`) and fails with what is missing. The SDK is
recognised by what it contains, never by its folder's name: the headers
(`vulkan_core.h`) must be 1.4.309, `Lib/vulkan-1.lib` must exist and, when
`Bin/vulkaninfoSDK.exe` is present, its file version must be 1.4.309.0. Matching
content is accepted in any folder, and wrong content is rejected even in a folder
named 1.4.309.0. The search covers per-version tool folders
(`CLAWSCRIBE_TOOLCHAIN_ROOT`, the runner tool cache and ProgramData), `VULKAN_SDK`
(process, machine and user) and the default SDK folder (`C:\VulkanSDK`, or
`CLAWSCRIBE_VULKAN_SDK_ROOT`); every candidate and its verdict is printed. Pull
requests and pushes never install, replace or downgrade anything. A runner whose
inherited `VULKAN_SDK` names a different SDK is fine: that SDK is rejected by its
content and the search continues.

Every workflow that needs the SDK (release builds and, through them, the
candidate workflow, plus the knowledge validation and loader diagnostics) uses the
local `.github/actions/vulkan-sdk` action. It reuses a matching SDK, and otherwise
stages the pinned SDK in its own per-version folder,
`<toolchain root>/vulkan-sdk/<version>` (`CLAWSCRIBE_TOOLCHAIN_ROOT`, else the
runner tool cache, else ProgramData), where later runs find it by content. The
extraction action unpacks into whatever `VULKAN_SDK` names, so
`stage-vulkan-sdk.ps1` first points `VULKAN_SDK` (and `VULKAN_SDK_VERSION`, which
the action prefers over its input) at a fresh `.staging-*` folder, then requires
that the action really used it, that the content is the pinned SDK and that the
inherited SDK's identifying files are unchanged before renaming the folder into
place. An existing per-version folder with other content stops the run and is
never overwritten or deleted; remove it manually. Only the staging folder is ever
deleted, after checking its name, its parent and that it holds no link. The
runner's own SDK is never written.

For the native tests, start the workflow manually with `provision-toolchain` to
install only what is missing: Rust with rustup, LLVM with Chocolatey (replacing a
different version), the SDK in its own folder as above, and the SDK's signed
runtime only when the loader cannot be loaded. Node.js and pnpm come from the
runner's tool cache through `actions/setup-node` and `pnpm/action-setup`.

To check a runner's toolchain without changing it, start **Windows Vulkan runtime
validation** manually with `verify-only` (any branch or commit; it needs no pull
request). It runs on the designated runner with read-only repository access and
only verifies: `ensure-native-toolchain.ps1`, the SDK search without installing
(the verdict for every candidate is printed and added to the job summary) and the
loader probe. It installs, extracts, stages, builds and tests nothing.

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

The validation path runs locked Rust checking, frontend typechecking, frontend
tests, and the native release-profile regressions described below. Packaging
checks the frontend, builds installers, requires both
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
individual DLLs from third-party sites. The 0.5.42 installer does not install
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

The app build must first stage `frontend/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe`.
The test helper fails immediately if that file is missing, then copies it beside
the test executable as `ffmpeg.exe`. Recording tests use this build-staged encoder
without relying on PATH or downloading an encoder at test time.

The helper compiles the actual release-profile library test executable and loads
the same staged sherpa/ONNX DLL set used by the installer. It rejects missing
DLLs, zero matched tests, and failing test results. Required suites cover summary
providers and sidecar exchanges, meeting memory and source invalidation,
audio cancellation and model switching,
transcript preservation and paging, credentials, Microsoft export persistence,
and updater selection. It also runs the local summary helper protocol, sampling,
and stop-sequence tests. Recording suites cover partial audio finalization,
checkpoint fallback, capture gaps, transcript snapshot failures, duplicate saves,
outcome migrations and acceleration selection. Tests run serially where they
share native resources.
It does not perform live capture, GUI interaction, or model-quality benchmarking.
The unfiltered library suite can also run in the runner's service session.
Tests that require a real audio output device are explicitly ignored; run those
with `--ignored --exact` in an interactive session with the required hardware.

Trusted pull requests and pushes to `main` that touch native code, scripts or
the native workflow run these groups automatically in **Native library tests**
(`native-library-tests.yml`). The job uses the release preflight's own staging
and test entry point, `build-windows-release.ps1 -Feature windows-gpu -CheckOnly
-NativeOnly`: locked cargo check, sidecar and runtime staging, then every
required group above. `-NativeOnly` skips the frontend typecheck and tests,
which **Validation Check** already covers. It still installs the frontend
dependencies from the lockfile and rebuilds the exported frontend assets
(`frontend/out`, which the native build embeds) from the checked-out commit,
after deleting the old folder, so a stale build left on the runner is never
compiled in. A reused Codex runtime is re-verified against its pinned hash
instead of being downloaded again (`stage-codex-runtime.ps1 -SkipIfCurrent`).

`self_test::tests` is part of the required groups. Besides unit tests of the
command line, the profile isolation checks, the bundled-template list and the
package-layout checks, it runs the same code as the packaged self-test on a
temporary profile, with no encoder and no packaged executable: it builds a
database from the older migrations (once from the legacy schema and once from a
recent schema seeded with non-secret rows), upgrades it through the production
open path, imports a synthetic WAV and transcript, produces a summary through the
provider path against a loopback stub, closes the database and reopens it. The
package layout, packaged encoder and interrupted-recording recovery need the
packaged executable and are covered only by the release-time acceptance below.

The runner keeps Cargo output between runs, so the job also bounds it.
`limit-native-test-cache.ps1` keeps the two newest `app_lib-*` test executables
in the release dependency folder, deletes only older ones (and their `.pdb` and
`.d` files) after checking that no link is involved, and fails early with a plain
message when the volume that holds the Cargo target folder, or the checkout's
volume, has less than 12 GiB free (network and removable volumes are refused).
Libraries, models, sidecars, installers and source are not touched. Nothing is uploaded to Actions
storage or cache. Manual dispatch requires an immutable commit SHA, like the
other manual validation.

## Packaged-Binary Acceptance

Unit tests run the library test executable, not the shipped program. Release
builds therefore also run `frontend/scripts/test-packaged-acceptance.ps1` after
the installers and native tests, before build metadata is written. It runs only
on the designated runner (it checks `CLAWSCRIBE_BUILD_RUNNER` first) and:

1. unpacks the NSIS installer just built into a new temporary folder with
   7-Zip. Nothing is installed: no registry entry, shortcut or existing
   installation is touched;
2. runs the packaged executable twice, in two separate processes, as
   `--self-test run <profile>` and `--self-test verify <profile>`. The profile is
   a new folder inside the temporary folder. The run phase refuses a profile that
   is not new or empty (so a stale marker or planted file is never reused), is a
   link, is not absolute, or overlaps the application's own data folders, and
   marks the folder with a random run id. The verify phase refuses a folder
   without that exact marker, a folder that contains any link, and saved state
   with another run id or a path outside the profile; it never creates a missing
   database. Reports and state are written only as new files. The database is
   opened through an isolated path that skips the credential-store step;
3. runs the executable against unsafe input and requires the exact exit code and
   an untouched folder: a command line without a phase or profile, an unknown
   phase, an extra argument (all exit code 2), a relative path, a non-empty
   folder, a folder with only a stale marker, an unmarked folder for verify, a
   forged marker and a profile that contains a junction (all exit code 4, with
   nothing written through the junction). Exit codes are 0 passed, 1 a check
   failed, 2 unusable command line, 3 runtime or report setup failed, 4 profile
   folder refused;
4. deletes everything it created, only after checking the folder name and that it
   holds no links. `-KeepWork` keeps it for inspection. A cleanup failure is
   reported after, and never replaces, the first failure.

The Actions log and summary hold only fixed check names, pass or fail, exit codes
(a crash such as a missing runtime library shows its hexadecimal code, even with
no report) and a few counts. Error text, paths and the application's own output
are never printed. The work folder must be on a fixed local drive with 3 GiB free.

The self-test is a headless mode of the shipped executable (`self_test.rs`). It
starts before any window, plugin, single-instance guard or logger, so it never
opens the interface, forwards to a running copy, or reads the user's own
profile, credential store, settings, templates or providers. It needs no
network. A failed check fails the release build; each check is printed and
recorded in `self-test-report-<phase>.json` inside the profile.

What it checks (all against synthetic data):

| Check | What is exercised |
| --- | --- |
| `package_layout`, `packaged_encoder` | Sidecars, runtime libraries (DirectML in DirectML and Windows GPU builds), every bundled summary template (equal to this build's copy and parseable), the Codex runtime manifest and runtime folder sit where the installer puts them, and the packaged encoder runs. The check never falls back to a PATH or downloaded encoder. The resource locations follow `tauri.conf.json`; the first acceptance run on the runner confirms them against a real unpacked installer. |
| `migration_upgrade` | A database built from the older migrations with synthetic meetings, one migration stamped with the other line-ending checksum, then opened through the production path without the credential-store step: pre-migration backup written, drift repaired, every embedded migration applied, the newest tables and columns present, legacy rows intact, integrity check passes. |
| `recording_recovery` | A synthetic interrupted recording (capture chunks, marker, snapshot) is found by the recovery scan and recovered to compressed audio by the packaged encoder, then decoded and checked for length and level. |
| `import_fixture` | A synthetic WAV and the recovered compressed audio are validated and decoded; an imported meeting with transcript lines is saved. |
| `summary_stub` | The production summary path (provider call, prompt assembly, output checks) runs against an offline loopback stub, and the result is saved. |
| `state_saved` | The state the second process reopens was written as a new file. |
| `persistence_reopen`, `database_integrity`, `recovered_audio_persisted` | In a second process: the legacy meeting, imported transcript and summary survived, migrations are complete, the integrity check passes and the recovered audio (a path inside the profile) still decodes. |

What it does not check, and what therefore remains manual (see the acceptance
list below): microphone and system-audio capture, device changes and sleep/wake,
speech models and transcription quality (no model is installed on the runner),
import through a transcription engine, real summary providers and note quality,
the graphical interface and keyboard behaviour, running the installer
(interaction, upgrade over an existing installation, Windows security prompts),
the updater, and long sessions on the target notebook. Passing it does not
establish stable readiness.

To rerun it against an existing installer on the runner (set
`EXPECTED_BUILD_RUNNER` to the configured name first):

```powershell
.\frontend\scripts\test-packaged-acceptance.ps1 -Installer <path to ClawScribe_<version>_x64-setup.exe>
```

## GitHub Actions

**Native library tests** runs the real Rust library suite for trusted pull
requests and `main` pushes (see Native Windows Unit Tests above).

**ClawScribe Windows Release** is the manual/reusable build workflow. All CI
validation, installer builds, and native Windows diagnostics run exclusively on
the designated local Windows x64 machine. GitHub-hosted runners are disabled as
a cost policy, including for this public repository. If the runner is offline,
jobs wait for it.

Assign the `clawscribe` label only to that machine's repository runner. Set the
repository Actions variable `CLAWSCRIBE_BUILD_RUNNER` to both its registered
runner name and Windows computer name. Before checking out any source, the
job requires a self-hosted environment and verifies both names against
that variable. Missing or mismatched configuration stops the job. Keep actual
machine names in repository settings, never in committed configuration.

Only trusted code may run on this persistent machine. PR validation jobs use a
job-level condition that requires a branch in this repository and an author
association of `OWNER`, `MEMBER`, or `COLLABORATOR`. Fork PRs and untrusted authors
are skipped before runner allocation. For an external contribution, a maintainer
must review the exact code and transfer the approved changes to a trusted
repository branch before running validation. Manual dispatch must likewise use
reviewed code. Never use `pull_request_target` to run untrusted PR code locally.

In repository Settings > Actions > General, require approval for all external
contributors' fork workflows (`all_external_contributors`). PRs can modify their
own workflow definitions, so the job condition alone cannot protect against a
fork removing that condition. Do not approve fork workflow runs on this runner;
transfer reviewed code to a trusted repository branch instead.

Every job uses `[self-hosted, Windows, X64, clawscribe]`, checks the configured
machine before checkout, and retains local build caches. Validation uses
PowerShell 7; isolated Rust checks use the Windows MSVC toolchain. The workflow
policy tests reject hosted runners, unresolved runner expressions, missing
per-job identity guards, and unguarded PR execution.

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
For disk pressure, manually dispatch **Summary chunking regression tests** with
`inspect-runner-storage` enabled to report generated build sizes without tests.
Its optional `prune-generated-caches` input clears only the workspace's Rust
debug incremental cache and Next.js cache after rejecting paths or contents
that contain links. Source, models, credentials, installers, and other build
outputs remain intact; subsequent checks recreate these caches locally.
If more space is needed, `prune-debug-build` removes the generated Rust debug
tree using the same path checks. The next debug check recompiles it; release
build output and installers remain intact.
An alternative `compress-release-dependencies` input applies native filesystem
compression only to regular Rust `.rlib`/`.rmeta` files in the release dependency
directory after checking every parent for links. It preserves those files and
does not compress models, executables, installers, or source. Use another local
volume for large evaluation model caches when available.
Stable builds publish release assets and advance the `latest` update channel.
The workflow builds sidecars, verifies icons, runs frontend checks, creates both
installers, and then runs the required native regression suites and the
packaged-binary acceptance before staging release assets. A check-only run exercises those suites before packaging is
attempted, so compile or behavioral failures can be resolved first.

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

For source version 0.63.0, installer filenames are:

```text
ClawScribe_0.63.0_x64-setup.exe
ClawScribe_0.63.0_x64_en-US.msi
```

Published installers include detached Tauri updater signatures (`.sig`).
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

To check a staged draft end to end, download its assets into one directory and
run the release verifier from the repository root. Find the draft's numeric
release id with `gh api repos/<owner>/<repo>/releases`; the tag endpoint hides
drafts.

```powershell
node scripts/verify-release-assets.mjs --dir <assets> --version <x.y.z> --commit <full commit sha> --repo <owner>/<repo> --release-id <id>
```

It confirms that the expected assets are present, that `SHA256SUMS.txt` and the
digests GitHub reports match the files, that `BUILD-METADATA.txt` and
`latest.json` name the version and commit, and that both installer signatures
verify against the updater public key pinned in `tauri.conf.json`. It prints
every check and exits nonzero if any fails. Omit `--release-id` to verify the
local files only. Passing the verifier does not replace the acceptance checks
below.

Use the full source version for `--version`, including any prerelease suffix.
Asset filenames, the release tag and download URL retain that suffix; the
verifier derives the numeric installer/runtime version for build metadata and
`latest.json`, matching the release workflow. The supplied GitHub release must
use the corresponding `v<source-version>` tag. Signature verification also
requires the Minisign trusted-comment prefix accepted by installed updaters.

Authenticode and updater signing are separate. `DIGICERT_KEYPAIR_ALIAS` enables
the optional DigiCert signing script. Without it, installers are not
Authenticode-signed and Windows may display an unknown-publisher/SmartScreen
warning. Do not describe an updater-signed installer as Authenticode-signed.
Never bypass an unexpected signature or checksum mismatch.

Tauri updater signatures require `TAURI_SIGNING_PRIVATE_KEY`; draft/stable
staging that generates updater metadata fails when the expected signature is
missing. Do not publish an empty signature to work around that failure.

## Update Channels

Stable releases are the default. The app checks the latest non-draft,
non-prerelease release's `latest.json`. Users can enable **Include prereleases**
in Settings > Preferences > Updates or About. This choice persists locally in
the Tauri-owned `updater.json` store and applies to startup checks, manual checks,
and the tray. Checking at launch is a separate preference; opting into previews
does not turn on automatic checks or install anything automatically.

The preview channel considers the stable update and the newest eligible Windows
x64 release in GitHub's 100 most recent release entries. It compares numeric
runtime versions, rather than publication order or version strings. Drafts,
incomplete asset uploads, empty assets, and releases without the Windows setup
and updater manifest are excluded. Discovery uses GitHub's public API without
credentials, with bounded response size and timeouts. Rate limits and network
failures are shown for manual checks and remain retryable.

The Rust checker owns endpoint selection. It validates preview manifest identity
and installer location, then returns a Tauri updater resource to the shared UI
service. The dialog installs that exact checked release without a second stable
check. Both channels use the existing pinned updater public key and Tauri's
signature verification. Changing channels invalidates cached and in-flight
results; leaving previews never downgrades an installed build. The native
application downloads and signature-checks the installer itself (at most 1 GiB,
held in memory, never handed to the page) and keeps the verified bytes until the
install step, so a page reload cannot change what is installed. An update can
install once recording, transcription, speaker detection, indexing, summaries,
saved answers and Microsoft 365 or Confluence exports finish. Work can start
during the download. If it does, the install step stops without losing that work;
try the update again after it finishes. The exclusion is held by the native
application for the whole installer hand-off and released if the installer
fails, so it cannot be stranded by a reload. Device security policy still applies
to the installer; selecting a channel does not bypass Windows security blocks.

To distribute a preview, stage a draft, verify its assets, then publish it as a
GitHub prerelease without marking it latest. The workflow's direct `publish`
path remains the stable channel.

Public prereleases remain outside the stable channel. Apps released before the
opt-in feature need a stable update or manual installation to gain it. Do not
promote a candidate to stable solely because it compiled.

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

None of the items below is covered by the scripted packaged-binary acceptance or
the native library tests; they need a person, real devices and the target
notebook. Record each as passed, failed or not run for the exact build. Item 4
and the two-hour comparison in item 6 are separate, long checks.

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
5. For the meeting-memory preview, use an isolated profile with invented English
   and German meetings. Check keyword retrieval offline before any model
   download, semantic fallback and downloaded-model retrieval, and visible
   provider-failure/retry behavior. Verify explicit scope and legacy history,
   grouped/range and preceding-question citations, a source beyond the first
   transcript page, same-meeting repeat navigation, missing/stale/historical
   sources, keyboard focus, dark/light/accent and narrow layouts, and saved-audio
   playback at the verified offset. These installed checks remain pending when
   only helper and native resolver evidence is available; they do not block a
   clearly labeled preview under the policy above or establish stable readiness.
6. For the Live assistance preview, verify off-default remembered transcript
   sharing and independent document consent/selection, audio-only and Built-in
   AI guidance, finalized timestamps and incomplete status, provider failure,
   Cancel, navigation, repeated Stop and a second recording. Confirm temporary
   replies disappear at first Stop and cannot enter the next session. Compare
   a release-like two-hour microphone/system-audio recording with assistance
   enabled to a comparable disabled baseline, recording engine/model/language/
   backend, memory trend, transcription elapsed time/backlog, search/answer
   times, Stop timing, saved playback and second start. Accept only with no
   assistance-induced capture gaps or unbounded memory and no Stop wait on
   assistance backlog. Exact-build installed and two-hour checks remain pending
   when only fixture/helper/native tests exist. Preview notes must disclose
   those missing checks. Stable is 0.5.58, which the owner published on 2026-11-15
   without real-device acceptance; that exception does not change this policy.

Optional hosted-provider verification is documented in
[hosted-transcription-smoke.md](hosted-transcription-smoke.md). OpenClaw handoff
verification is in [openclaw-handoff.md](openclaw-handoff.md); it is not required
for standalone local recording.
