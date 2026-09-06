# ClawScribe Documentation

Documentation for source version **0.5.37**. Installer availability and stable,
prerelease, or draft status are tracked in
[GitHub Releases](https://github.com/ch3573r/ClawScribe/releases), not inferred
from the source version.

## Product And Build Guides

- [Meeting quality and notebook performance](meeting-quality.md)
- [Architecture](architecture.md)
- [Building from source](BUILDING.md)
- [Windows release, signing, and acceptance checks](windows-release.md)
- [0.5.37 preview release notes](releases/0.5.37.md)
- [0.5.36 stable release notes](releases/0.5.36.md)
- [Hosted transcription smoke test](hosted-transcription-smoke.md)
- [GPU acceleration](GPU_ACCELERATION.md)
- [Teams detection](teams-detection.md)
- [Optional OpenClaw handoff](openclaw-handoff.md)
- [Integration backlog](integrations-backlog.md)

## Authentication And Providers

- [Codex authentication](auth/codex-auth.md)
- [OpenAI authentication modes](openai-oauth.md)
- [OpenAI login background](auth/openai-login.md)
- [Bundled Codex runtime](codex-runtime.md)

## Microsoft 365 Exports

- [Microsoft Graph integration](integrations/microsoft-graph.md)
- [OneNote export](integrations/onenote-export.md)
- [Planner export](integrations/planner-export.md)
- [Microsoft To Do export](integrations/todo-export.md)

## Brand

- [Theme notes](brand/theme.md)
- [Icon notes](brand/clawscribe-icon.md)

Brand assets include `brand/clawscribe-logo.png` and
`brand/clawscribe-readme-hero.png`.

## Documentation Policy

Keep guides product-facing and current. Keep detailed run-by-run build status
in the associated PR or workflow. Release notes must distinguish implemented
behavior, executed tests, and remaining limitations. Do not publish private
meeting content, attendee data, tenant endpoints, credentials, or local machine
paths in examples or screenshots.
