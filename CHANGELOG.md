# Changelog

## 0.5.21

- Prevented a diarization mapping collapse where sherpa found multiple speaker
  lanes but overlapping turns caused the transcript labels to flatten to one
  speaker.
- Added an overlap-to-midpoint mapping fallback for diarization and an
  actionable hard error when an explicitly requested multi-speaker run would
  otherwise save fake one-speaker labels.
- Kept the retranscription benchmark dialog open after completion so users can
  see audio length, processing time, realtime speed, model, language, and
  segment count.
- Added a speaker-diarization completion dialog with audio length, processing
  time, realtime speed, provider, embedding model, turn count, detected
  speakers, and updated transcript rows.
- `latest.json` advertises runtime version `0.5.21`, so installed `0.5.20`
  clients can discover this update.

## 0.5.20

- Improved speaker diarization accuracy by preserving sherpa speaker turns
  instead of forcing short clips into a fixed speaker count or smoothing away
  short speaker changes.
- Added an explicit speaker-count control for meeting diarization so two-, three-,
  and larger-speaker recordings can be rerun without relying only on clustering
  auto-detection.
- Switched the default diarization embedding to the English WeSpeaker/CAM++
  model while keeping the legacy Chinese 3D-Speaker model available when the
  meeting language calls for it.
- Split transcript rows at diarization speaker changes using persisted word
  timestamps when available, so one long ASR segment can be assigned across
  multiple speakers.
- Made diarization model selection source-language aware for Whisper, Parakeet,
  and Nemotron by persisting the transcription source-language hint in recording,
  import, and retranscription metadata.
- `latest.json` advertises runtime version `0.5.20`, so installed `0.5.19`
  clients can discover this update.

## 0.5.19

- Added crash-safe recording checkpoints that flush every 10 seconds, write via
  atomic temp files, and recover from the ordered checkpoint files that actually
  exist on disk.
- Added audio device hot-swap handling so an unplugged or disconnected input
  enters a reconnecting state instead of killing the active recording session.
- Added transcript word-anchor persistence as a playback foundation. Current
  anchors are estimated from segment timing and text weight, not exact ASR word
  alignment, so they are intended for navigation/highlighting rather than
  quote-boundary certification.
- Added a speaker-lane waveform timeline and click-to-seek playback from both
  the timeline and transcript rows.
- Smoothed diarization transcript labels so adjacent fragments with the same
  speaker read more naturally after speaker-turn processing.
- Restricted meeting audio resolution to folders already registered in the
  local meetings database before scanning for playable files.
- `latest.json` advertises runtime version `0.5.19`, so installed `0.5.18`
  clients can discover this update.

## 0.5.18

- Added OneDrive and SharePoint file export for meeting summaries, producing a
  DOCX and optional PDF with transcript content included.
- Added a OneDrive destination panel in Settings -> Add-ons with root-folder
  selection, SharePoint/OneDrive folder-link resolution, subfolder creation,
  PDF toggle, and optional organization-scoped sharing links.
- Added pinned SHA-256 and byte-size validation for downloaded diarization
  models, with invalid cached managed models quarantined before redownload.
- Added a diarization embedding-model catalog for A/B checks against English
  and multilingual speaker embeddings while preserving the current default.
- Hardened Microsoft To Do list creation by reusing an existing normalized
  list name and blocking duplicate create requests from rapid clicks.
- Added Windows release build metrics so GPU release runs publish sherpa
  runtime, cache hit/miss, sherpa staging time, and build elapsed time.
- `latest.json` advertises runtime version `0.5.18`, so installed `0.5.17`
  clients can discover this update.

## 0.5.17

- Added speaker-diarization profiling for DirectML builds: each run now logs
  structured provider decisions, CPU vs DirectML probe timings, full-run
  timings, turn counts, and bundled sherpa/ONNX runtime DLL presence.
- Writes a per-run speaker-diarization profile JSON under the app data logs
  directory so problematic runs can be inspected after the UI toast disappears.
- Treats DirectML as an adaptive candidate for sherpa diarization instead of a
  blind default: ClawScribe probes DirectML against CPU and only keeps DirectML
  when it is measurably faster on the current machine.
- Enables sherpa debug mode for DirectML diarization attempts by default, with
  `CLAWSCRIBE_SHERPA_DIARIZATION_DEBUG` available as an override.
- Hardened Microsoft To Do export by URL-encoding Graph list/task IDs, creating
  tasks with the minimal title payload first, and patching notes/due dates
  after the task exists.
- `latest.json` advertises runtime version `0.5.17`, so installed `0.5.16`
  clients can discover this update.

## 0.5.16

- Added a DirectML speaker-diarization runtime for Windows GPU builds by
  compiling the pinned `sherpa-onnx` runtime with DirectML enabled, while
  keeping a CPU fallback when DirectML is unavailable.
- Kept speaker diarization resilient across build variants with a
  process-level DirectML-unavailable latch and per-run fallback messaging.
- Refined speaker-turn splitting so transcript rows are split at sentence
  boundaries instead of being cut through mid-sentence fragments.
- Added Microsoft To Do list creation in Settings -> Add-ons, so users without
  an existing To Do list can create and select one without leaving ClawScribe.
- `latest.json` advertises runtime version `0.5.16`, so installed `0.5.15`
  clients can discover this update.

## 0.5.15

- Improved speaker diarization mapping so a single transcript row can be split
  when speaker turns change inside it, instead of assigning the whole row to the
  dominant speaker.
- Preserved transcript text while splitting speaker turns by slicing contiguous
  word ranges and keeping recording-relative timing on the generated rows.
- Made the Source attribution (Me / Participants) beta switch control the saved
  meeting screen as well as live recording: when disabled, speaker labels,
  label-edit controls, and the Speakers action are hidden.
- Kept stored speaker labels intact when Source attribution is off, so
  re-enabling the switch restores the review state without losing metadata.
- Matched transcript copy and summary-generation input to the Source attribution
  setting so hidden labels are not still injected into exported or regenerated
  text.
- `latest.json` advertises runtime version `0.5.15`, so installed `0.5.14`
  clients can discover this update.

## 0.5.14

- Fixed Meeting details toolbar wrapping at narrower desktop widths so action
  buttons no longer overflow the meeting title or summary metadata.
- Tightened meeting toolbar responsiveness by switching secondary labels to
  icon-only buttons below wide desktop layouts while preserving tooltips.
- Hardened speaker diarization for short imported clips by compacting sparse
  sherpa cluster IDs before they become visible labels.
- Added an automatic retry for short auto-diarization runs that split a clip
  into too many speakers, using a two-speaker clustering hint for that fallback.
- Improved speaker-detection feedback with persistent progress toasts, audio
  duration-aware status text, streamed model-download progress, and download
  timeouts.
- `latest.json` advertises runtime version `0.5.14`, so installed `0.5.13`
  clients can discover this update.

## 0.5.13

- Added local speaker diarization for saved transcripts using `sherpa-onnx`
  pyannote segmentation, 3D-Speaker embeddings, and fast clustering.
- Added a Speakers workflow for reviewing and applying speaker labels across
  transcript rows before copying transcripts or regenerating summaries.
- Downloaded diarization models on first use instead of bundling them in the
  installer, keeping the Windows package size controlled.
- Bundled the required `sherpa-onnx` Windows runtime DLLs during release builds
  so installed clients can run diarization outside the development environment.
- Preserved speaker labels through import, retranscription, reload, recovery,
  copied transcripts, and OpenClaw handoff artifacts.
- Kept the Windows release artifact on the Vulkan + DirectML build path while
  using CPU execution for the current `sherpa-onnx` diarization backend.
- `latest.json` advertises runtime version `0.5.13`, so installed `0.5.12`
  clients can discover this update.

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
