# Changelog

## 0.5.0-alpha.2

- Prepared the corrective productization QA/build metadata pass.
- Preserved upstream Meetily Community Edition `0.4.0` attribution.
- Replaced the generated ClawScribe icon set with Alex's supplied app icon.
- Routed summary regeneration through the same user-provided context field as
  first-time summary generation.
- Routed that regeneration context through OpenAI-compatible, OpenClaw, and
  Codex app-server providers, not only the built-in summary path.
- Switched ChatGPT sign-in URL opening away from `cmd /C start`, opened
  device-code verification URLs automatically on Windows, and regenerated icon
  assets with transparent corners.
- Added a Settings → Add-ons tab that exposes Teams detection status, OpenClaw
  handoff, OneNote export, Planner export, and Advanced Codex app-server state.

## 0.5.0-alpha.1

- Productized the fork as ClawScribe.
- Preserved upstream Meetily Community Edition `0.4.0` attribution in About, NOTICE, and UPSTREAM docs.
- Updated package, Tauri, and Cargo product versions to `0.5.0-alpha.1`.
- Updated product-visible UI strings, window metadata, tray tooltip, and notification titles to ClawScribe.
- Documented intentional remaining Meetily references for compatibility and provenance.
