# GitHub Actions Workflows

ClawScribe uses one designated local Windows x64 runner for every CI validation
job, application installer, and native Windows diagnostic. There is no hosted
fallback. Its `clawscribe` label and `CLAWSCRIBE_BUILD_RUNNER` repository variable
must identify that machine. The workflow verifies both the registered runner
name and Windows computer name before checking out code. Keep real machine names
in repository settings, never in committed files.

| Workflow | Trigger | Execution and output |
| --- | --- | --- |
| `clawscribe-windows-release.yml` | Manual or reusable call | Local Windows runner; checks or installers; explicit draft/publish options |
| `windows-candidate.yml` | `release/**` push or manual | Local Windows frontend checks, then the Windows GPU build and a draft release |
| `windows-native-loader-diagnostics.yml` | Selected trusted branch pushes or manual | Local Windows runner; native Vulkan loader fixture |
| `pr-main-check.yml` | Trusted pull request, `main` push, or manual | Local Windows safety, version, frontend typecheck and helper tests |
| `release-readiness.yml` | Selected pushes, trusted pull-request paths, or manual | Local Windows isolated Rust module regressions |
| `summary-chunking-tests.yml` | Relevant trusted pull-request/`main` paths or manual | Local Windows isolated chunker tests |
| `windows-script-validation.yml` | Relevant trusted pull-request paths or release branch pushes | Local Windows PowerShell syntax and repository safety checks |

Hosted validation is disabled as a cost policy, including for this public
repository. PR jobs are scheduled only for same-repository branches authored by
an owner, member, or collaborator. Forks and untrusted authors are skipped before
runner allocation. Maintainers must review external changes and transfer the
approved code to a trusted repository branch before local validation. Do not
use `pull_request_target` to execute PR code on the persistent runner.
Require repository-level approval for all external contributors' fork workflows
and do not approve those runs locally; a fork can edit its own job conditions.

The workflow policy tests reject hosted or unresolved runner selections and
require the identity guard independently in every local job before checkout.
Jobs wait when the designated runner is offline; there is no fallback.

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
