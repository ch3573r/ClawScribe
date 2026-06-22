# Changelog

## 0.5.12

- Added Microsoft To Do export for reviewed personal action items, including
  To Do list selection, editable task titles and notes, and duplicate
  protection.
- Improved Planner export review so task notes are editable and export stays
  disabled until candidate tasks are loaded.
- Added speaker-label review for saved meeting transcripts, with per-row
  edits, custom labels, and apply-to-matching-row updates that feed copied
  transcripts and regenerated summaries.
- Preserved speaker attribution as structured metadata in new recording
  artifacts and OpenClaw transcript markdown instead of baking labels into
  transcript text.
- Polished the Meetings archive with saved timestamps and newest, oldest, and
  title sorting.
- Removed stale beta-page copy now that Import Audio and Retranscribe are
  production workflows.
- `latest.json` advertises runtime version `0.5.12`, so installed `0.5.11`
  clients can discover this update.

## 0.5.11

- Refined the Windows chrome and app shell with a thinner custom top bar,
  sharper corners, and a less rounded desktop layout.
- Overhauled integration iconography so add-ons such as Microsoft 365,
  Confluence, Jira, OpenClaw, OneNote, and Planner present as distinct
  product destinations instead of generic placeholders.
- Updated recording storage defaults and labels to use ClawScribe paths while
  continuing to honor the configured recording location.
- Cleaned out the unused legacy Python/FastAPI backend and removed stale
  frontend filesystem access.
- Added runtime cleanup guardrails, expanded frontend helper validation, and
  kept the Windows Vulkan + DirectML artifact path working.
- Resolved small meeting-workflow TODOs by adding system-audio capture state,
  enabling microphone testing, and wiring summary search from the meeting
  summary overflow menu.
- `latest.json` advertises runtime version `0.5.11`, so installed `0.5.10`
  clients can discover this update.

## 0.5.10

- Reworked the custom Windows titlebar into a quiet app-shell drag region:
  branding now lives in the sidebar, while only the native window controls stay
  in the top-right corner.
- Added a Meetings overview page and renamed the old Meeting Notes navigation
  entry to Meetings so saved recordings have a clearer home.
- Rebalanced the collapsed icon rail with primary navigation at the top,
  recording/import actions at the bottom, and a compact status/version footer.
- Polished the Home dashboard, Settings surfaces, Add-ons readiness cards, and
  Meeting details layout for denser desktop use.
- Graduated Import Audio and Retranscribe from beta into the production meeting
  workflow, including cancel/error handling and safer retranscribe warnings.
- Improved recording start feedback so the app acknowledges capture initiation
  immediately while the backend finishes setup.
- Preserved the `latest.json` updater path with runtime version `0.5.10`, so
  installed `0.5.9` clients can discover this update.

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
