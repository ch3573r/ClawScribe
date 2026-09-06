# GitHub Actions Workflows

ClawScribe uses one designated local Windows x64 runner for application
installers and native Windows diagnostics. The release workflow has no hosted
fallback. Its `clawscribe` label and `CLAWSCRIBE_BUILD_RUNNER` repository variable
must identify that machine. The workflow verifies both the registered runner
name and Windows computer name before checking out code. Keep real machine names
in repository settings, never in committed files.

| Workflow | Trigger | Execution and output |
| --- | --- | --- |
| `clawscribe-windows-release.yml` | Manual or reusable call | Local Windows runner; checks or installers; explicit draft/publish options |
| `windows-candidate.yml` | `release/**` push or manual | Standard Ubuntu frontend checks, then the local Windows GPU build and a draft release |
| `windows-native-loader-diagnostics.yml` | Selected trusted branch pushes or manual | Local Windows runner; native Vulkan loader fixture |
| `pr-main-check.yml` | Pull request, `main` push, or manual | Standard Ubuntu safety, version, frontend typecheck and helper tests |
| `release-readiness.yml` | Selected pushes, pull-request paths, or manual | Standard Ubuntu isolated Rust module regressions |
| `summary-chunking-tests.yml` | Relevant pull-request/`main` paths or manual | Standard Ubuntu isolated chunker tests |
| `windows-script-validation.yml` | Relevant pull-request paths or release branch pushes | Standard Ubuntu PowerShell syntax and repository safety checks |

Standard hosted validation is free for this public repository and does not
produce application installers. Do not run untrusted pull-request code on the
persistent local runner. Reassess hosted validation before making the repository
private or selecting larger runners.

The legacy DevTest, cross-platform, and standalone hosted installer workflows
have been retired. Disable their GitHub workflow entries and historical branch
automations that can still launch hosted builds. Do not rerun old hosted refs.

Test builds keep installers and verification metadata in the local checkout's
`frontend/src-tauri/target/release/bundle`. Copy outputs locally before another
build replaces them. Current workflows upload no Actions artifacts or caches.
Draft and published GitHub Release assets require an explicit release request.
Existing artifacts/caches from older workflows require expiration or deliberate
cleanup; removing an uploader does not delete stored resources.

Use **ClawScribe Windows Release** with `check-only=true` for native preflight,
or leave `publish` and `draft-release` false for local test installers. The normal
acceleration feature is `windows-gpu`. Keep build workflows disabled until the
local-only workflow revision and runner variable are configured, then enable only
the supported workflows. A queued job waits for the local runner to come online.

Follow [Windows releases](../../docs/windows-release.md) for prerequisites,
runner configuration, metered spending checks, release identity, updater signing,
and the required real-device capture acceptance before stable publication.
