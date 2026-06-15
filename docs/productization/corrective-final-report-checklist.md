# ClawScribe Corrective Productization Final Report Checklist

Date:
Coordinator:
Branch:
Build commit:
Version:
Upstream base version: Meetily Community Edition `0.4.0`
Build date:

Use this checklist for the final integration report after provider, theme,
Codex UX, icon, and packaging work lands. Do not mark Windows runtime items
pass until a Windows build has been installed and tested.

## Artifact Metadata

- [ ] `BUILD-METADATA.txt` is attached to the Windows artifact bundle.
- [ ] Metadata records `build_commit`.
- [ ] Metadata records `version`.
- [ ] Metadata records `upstream_base_version=0.4.0`.
- [ ] Metadata records `build_date_utc`.
- [ ] `SHA256SUMS.txt` verifies from the artifact root and uses paths such as
  `msi/<installer>.msi` and `nsis/<installer>.exe`.

Result:

```text
Artifact link:
SHA256SUMS link:
MSI checksum:
NSIS checksum:
BUILD-METADATA contents:
Notes:
```

## Provider Behavior

- [ ] Built-in/local provider remains selectable where expected.
- [ ] OpenAI API-key provider works without exposing full keys.
- [ ] OpenClaw managed provider reports endpoint/token status without exposing
  the bearer token.
- [ ] Provider error states are actionable and sanitized.
- [ ] Summary/action extraction works for a short Windows recording.

Result:

```text
Providers tested:
Pass/Fail:
Secrets observed:
Notes:
```

## Codex App-Server Runtime

- [ ] Codex remains third provider: `Advanced: Codex app-server`.
- [ ] OpenAI/OpenAI-compatible processing works without Codex runtime.
- [ ] OpenClaw processing works without Codex runtime.
- [ ] Missing Codex app-server runtime warns only when Codex provider is
  selected.
- [ ] WindowsApps, PATH discovery, browsed `codex.exe`, and standalone CLI auth
  are rejected or absent from UI.
- [ ] The report states the pinned bundled app-server runtime version, SHA256,
  source package, and Windows verification result.

Result:

```text
Codex app-server runtime version:
Bundled runtime SHA256:
Runtime path:
Summary generated:
Limitations:
Notes:
```

## Theme And Contrast

- [ ] Theme token table is included or linked.
- [ ] Light, dark, and system modes are tested.
- [ ] Contrast summary covers text, controls, focus rings, disabled states,
  dialogs, settings, transcript, summary, and action item surfaces.
- [ ] Remaining hardcoded colors are classified as tokenized, third-party,
  compatibility/deferred, or blocker.
- [ ] No unreadable mixed-theme panels remain in tested flows.

Result:

```text
Theme token table:
Contrast summary:
Hardcoded colors classification:
Pass/Fail:
Notes:
```

## Icon Assets

- [ ] Icon asset list is included or linked.
- [ ] Windows `.ico`, Tauri app icons, About/sidebar icons, installer icons,
  and any tray/notification icons are accounted for.
- [ ] Icons render correctly in Start menu, installed-apps surface, window,
  tray/notifications, and About.

Result:

```text
Icon asset list:
Missing/legacy assets:
Pass/Fail:
Notes:
```

## Tests

- [ ] Linux frontend build.
- [ ] Typecheck status, including known blockers if any.
- [ ] Lint status, including known blockers if any.
- [ ] Linux Tauri check.
- [ ] Targeted Rust tests for provider/auth/Teams detection.
- [ ] Secret scan.
- [ ] Branding/upstream-string classification scan.
- [ ] Windows installer validation and runtime smoke.

Result:

```text
Commands run:
Failures/blockers:
Waivers:
Notes:
```

## Windows Runtime And Alex Retest Steps

- [ ] Installer launch/install/uninstall behavior tested.
- [ ] About/version/upstream attribution tested.
- [ ] Provider settings and Codex path tested.
- [ ] Teams prompt-only discovery behavior tested.
- [ ] Recording with microphone and system audio tested.
- [ ] OpenClaw handoff tested.
- [ ] Windows artifact link and checksums recorded.
- [ ] Limitations are explicit and do not overclaim Microsoft/Codex/Windows
  behavior.

Alex retest steps:

```text
1.
2.
3.
4.
5.
```
