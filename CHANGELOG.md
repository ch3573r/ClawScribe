# Changelog

## 0.5.49

- Add memory across saved meetings: keyword search without a model download, optional local multilingual E5 semantic retrieval, and explicit selected-meeting/project/date scopes. Local embeddings run on CPU; recording takes priority over background indexing and active transcript reads.
- Add durable library conversations using the configured summary provider, with bounded selected context and request-specific citations. Expanded meeting chat uses a separate library conversation; returning to meeting-only mode restores its existing history. Answers display the actual provider, model and retrieval mode.
- Preview cited sources, reveal the canonical transcript line and seek saved audio. Corrections, deletion and restore label old evidence stale or historical. Short-answer question context is shown separately from literal model citations. Late replies/navigation cannot replace a newer owner or scope; failed playback and recovered status reads are visible.
- Advance installer/runtime/updater version to 0.5.49 for a Windows GPU preview. Stable remains 0.5.48; enable **Include prereleases** after publication or install manually. Draft candidates are excluded. Both NSIS and MSI installers have Tauri updater signatures; Windows publisher signing is separate.
- Automated feature validation passed frontend typecheck/build and 250 tests, plus native Windows GPU checks, 92 knowledge tests and seven conversation backup tests. The fixed CPU Qwen3.5 4B answer evaluation passed 12 required assertions; the 10,000-meeting retrieval fixture hit 27/30 expected results. These are bounded evidence, not blanket answer quality or installed acceptance. Exact-build offline/provider failure/citation/focus/playback, recording/upgrade/update discovery and endurance checks remain pending. Documents and live assistance remain the follow-up plan. See `docs/releases/0.5.49.md`.

## 0.5.48

- Update the optional Advanced: Codex app-server provider from Codex 0.157.0 to 0.159.2, whose catalog includes GPT-6.1 Sol (`gpt-6.1-sol`). Choose **Check bundled runtime** in Summary settings after upgrading to refresh models. Saved model selections are preserved; model access depends on the signed-in account and workspace.
- Add GPT-6.1 Sol to the OpenAI API fallback model choices. The Codex picker continues to use the live, paginated runtime catalog.
- Windows preflight and installer builds now verify the actual staged Codex executable and its GPT-6.1 Sol catalog using a fresh keyring-only profile, including signed-out account/logout checks. Runtime package and executable hashes, About attribution, and build metadata are synchronized.
- Promote the verified 0.5.48 Windows GPU preview to Stable with its existing installers, signatures, tag, and updater manifest unchanged. Older stable installations can discover 0.5.48; installations already on the preview receive no successive update for the same numeric version. Draft candidates are excluded from updates.
- Automated validation for the runtime update passed 227 frontend tests, 628 native app tests (three exclusions), five helper tests, and the signed-out Codex runtime smoke. The owner confirmed the full installed-build acceptance checklist, including GPT-6.1 summaries/chat, dual-source playback, Stop and a second recording, retained meetings, editing, save recovery, keyboard/scaling/live-follow, factual accuracy, sustained notebook use and update discovery. Updater signatures do not provide Windows publisher signing, and a Vulkan loader remains required. See `docs/releases/0.5.48.md`.

## 0.5.47

- Saved meetings with audio but no transcript now show **No transcript yet** with **Transcribe now** instead of the home screen's welcome text. The dialog says Transcribe and Start Transcription for a first transcript and skips the replace warning; Generate Summary waits until a transcript exists.
- Summaries and Copy transcript work again for meetings with more than 1,000 transcript lines. Transcripts are read in pages, and a failed page stops the action instead of using a partial transcript.
- The playback timeline covers the whole meeting rather than only the transcript lines loaded so far. Every meeting with saved audio gets a playback bar, including audio-only meetings and meetings with speaker attribution turned off.
- Exports: the Timestamps option also removes source times from the summary, and Speaker labels is hidden for summary-only exports. Markdown tables export as real tables to Word and OneDrive DOCX, OneNote, and Confluence. Word export uses a Word icon.
- The Meetings page filters by several project tags at once (match any or all) or shows untagged meetings.
- The sidebar shows each meeting's date instead of "Recent meeting" and can be resized by dragging its edge; the width is remembered. Recording controls and status messages stay centred under the content.
- The recording bar's bookmark control is a round icon button that matches Pause and Stop.
- Advance installer/runtime/updater version to 0.5.47 with Whisper Vulkan and ONNX/sherpa DirectML. The verified draft build was published unchanged to Stable after owner-confirmed real-device acceptance. See `docs/releases/0.5.47.md` for validation and signing caveats.

## 0.5.46

- Codex summaries now enforce the meeting output format; a meeting without a suggested follow-up email no longer fails.
- Project tags can be picked from existing tags or created in a searchable list.
- Transcription settings keep showing a saved Microsoft AI or Cloud Whisper engine and explain that it is used for Import and Enhance, while live recordings transcribe on this device.

- Recover interrupted recordings even without transcripts. The recovery dialog recognizes raw capture audio instead of incorrectly showing "No audio". Keep originals until recovered audio is complete, fall back to WAV if compression fails, preserve capture-gap warnings, and allow retranscription when recording status is damaged.
- Make long-meeting summaries more reliable with provider-aware context and output budgets, bounded reduction, clearer retry and failure states, and support for reasoning models and custom endpoints. Improve source citations and preserve useful partial chat output without duplicate assistant messages; generated notes still require review.
- Save meeting titles and summaries automatically, preserve drafts across navigation, confirm replacement of edited notes, and offer Restore previous summary. Quit waits briefly for pending edits and offers an explicit discard choice if they cannot be saved.
- Restore the previous transcript after retranscription or speaker detection. Preserve correction history when applying speaker labels, block conflicting edits, and keep a successful database save when its file copy needs retry.
- Make speaker detection safer for long recordings with bounded decoding memory, cancellation from the dialog or background toolbar, and a clear wait before recording can start. The current native step may take several minutes to finish after cancellation.
- Verify model downloads against pinned revisions, expected sizes, and SHA-256 hashes before use. Open external links only through validated destinations and supported URL schemes.
- Guard meeting deletion and remove saved transcript recovery copies promptly while retaining unsaved recovery data. Clean abandoned queue folders and bookmarks only when their recordings are no longer recoverable. Store summary context with the meeting so it survives library backup and restore.
- Preserve SQLite WAL data during database transfers and take consistent snapshots before pending migrations. Restored meetings whose recovery files were excluded from the archive now explain that recovery must happen on the original computer.
- Fix the Windows audio-device crash that could follow a short-lived monitoring or enumeration thread. Keep live transcription failure notices to one per recording and display live timestamps in local time.
- Advance installer/runtime/updater version to 0.5.46 with Whisper Vulkan and ONNX/sherpa DirectML. The verified draft build was published unchanged to Stable after real-device acceptance. See `docs/releases/0.5.46.md` for validation and signing caveats.

## 0.5.45

- Preserve recording continuity when live transcription, disk queueing, or device delivery degrades. Keep bounded capture buffers and recoverable raw audio, freeze duration at Stop, and prevent duplicate stop/save operations and duplicate saved meetings.
- Encode final audio once with a duration-scaled deadline, avoiding repeated AAC padding between chunks. Save transcript, metadata, and warning status independently; write stopped metadata before encoding so a crash cannot leave the folder marked as recording.
- Recover readable temporary audio tails. A torn temporary tail records a capture gap without retaining the entire successfully encoded raw spool; unreadable published chunks and failed-save originals remain available. Keep legacy checkpoint recovery while removing the unused checkpoint writer and empty checkpoint folders.
- Allow automatic notes when only capture-gap or recording-file warnings remain. Successful retranscription clears repaired audio/transcription warnings, clears file warnings only after both file rewrites succeed, and preserves permanent capture-gap information. Keep summary status polling active across meeting updates, and surface failed automatic generation.
- Back up libraries containing missing recording folders or unfinished recovery files with explicit incomplete-meeting reports. Skip existing meetings before extracting restore audio, preserve older version-1 archives with missing optional tables, and avoid orphaned restored recordings.
- Use recording offsets for transcript export timestamps, with a legacy timestamp fallback. Unify Word, OneDrive, OneNote, and Confluence export content options; fix local Word dates and native Save As permissions. Stabilize tag loading and bookmark refresh/retry behavior, and dispatch title-bar gestures once.
- Include the project tags, live/saved bookmarks, local Word export, and additive portable backups introduced after stable 0.5.41, plus resumable model downloads, HE-AAC timing fixes, reasoning-tag filtering, and portable Windows CPU build settings.
- Advance installer/runtime/updater version to 0.5.45 with Whisper Vulkan and ONNX/sherpa DirectML. Stage installers as a draft for real-device acceptance; drafts do not change the stable updater. Stable publication makes this version available to older installations without opting into previews.
- Automated validation covers 114 frontend tests and 401 native tests (two ignored), including partial saves, legacy recovery, temporary tails, and stopped metadata. Real-device acceptance and installed-app update discovery must be completed separately; see `docs/releases/0.5.45.md`. Tauri updater signatures do not provide Windows publisher signing, and a Vulkan loader remains required.

## 0.5.44

- Fix the missing Tauri Save As permission that caused local Word export and backup creation to fail with an ACL error. File-picker permissions remain limited to the local main window; restore retains its existing Open permission.
- Add a regression check against the shipped desktop capability so mocked browser dialogs cannot hide a missing file-picker permission.
- Fix duplicate title-bar drag and double-click maximize actions. Window controls keep their dedicated actions.
- Advance runtime/updater version to `0.5.44` for a Windows GPU preview. Draft builds are excluded from update discovery; once published, enable **Include prereleases** or install manually. Stable remains `0.5.41`.
- Real-device recording, graphical install/upgrade and file-picker acceptance, Word desktop rendering, and installed-app update discovery remain unverified. See `docs/releases/0.5.44.md`.

## 0.5.43

- Add local Word document export for summaries, full saved transcripts, or both. Optional speaker labels and timestamps, a Save As dialog, and offline DOCX generation require neither Microsoft sign-in nor Word installed. Basic headings, bullets, and paragraphs are preserved; advanced editor formatting is not.
- Add project tags to saved meetings and project/untagged filters to the Meetings archive and transcript search. Tags persist locally and refresh across views.
- Add portable ZIP backup and additive restore for saved meeting data, recordings, notes, summaries, chat, correction history, tags, bookmarks, and recording-folder export history. Existing meeting IDs are skipped. Restore validates archive entries and commits database inserts together. Provider credentials, models, UI preferences, browser-stored meeting context, and unfinished recovery spools are excluded. Archives are not encrypted.
- Add live recording bookmarks and a saved-meeting bookmark panel with labels, playback seeking, renaming, and removal. Live timestamps exclude pauses; markers persist in SQLite and attach to the saved recording folder.
- Add regression coverage for archive round-tripping, duplicate skipping, unsafe paths, rollback, migration preservation, tag filtering, and bookmark cleanup. Frontend and Windows GPU preflight gates run before preview packaging.
- Publish runtime/updater version `0.5.43` as a preview, newer than `0.5.42`. Enable **Include prereleases** or install manually; the stable channel stays on `0.5.41`.
- Real-device dual-source capture, installed-app bookmark timing, Word desktop rendering, large-library restore, graphical install/upgrade, and installed-app update discovery remain unverified. This preview does not assert capture-smoke confirmation or stable readiness. See `docs/releases/0.5.43.md`.

## 0.5.42

- Publish a Windows GPU prerelease with runtime/updater version `0.5.42`, newer than `0.5.41`. Enable **Include prereleases** to discover it; stable remains `0.5.41`.
- Adapt selected Meetily 0.4.1 reliability fixes. This is not a wholesale merge of that release.
- Resume interrupted Whisper and Parakeet downloads from `.partial` files after checking server byte ranges. Keep completed Parakeet files on retry, keep Whisper partial files on cancel, and mark a model available only after validation. Cancellation is per model and waits for the download to close its files.
- Use the decoder's actual sample rate for HE-AAC imports so duration and timestamps survive conversion to 16 kHz.
- Remove complete `<think>`/`<thinking>` blocks, including uppercase and attribute variants, and fail generation on unfinished or stray reasoning tags. Check Codex structured output the same way, reject saving edited summaries with reasoning tags, and hide affected saved notes behind a regeneration message without deleting them.
- Trim the voice-activity detector's padded final frame to the real end of the recording, including when a long segment splits during that flush.
- Build Rust code for `x86-64-v2` and Whisper's CPU code without build-machine tuning or AVX-512; AVX2 stays enabled. Bundling and native tests fail on a host-tuned Whisper build cache, and the release workflow removes such caches left on the build machine before building.
- Pass 74 frontend tests and 325 Windows native tests in the GPU preflight, including new download, HE-AAC decoder, and VAD flush suites. A real resumed download, real HE-AAC recordings, and Whisper CPU behavior on other CPUs remain unverified, along with the real-device capture, install/upgrade, and notebook performance acceptance listed for earlier previews. This preview does not assert capture-smoke confirmation or stable readiness.

## 0.5.41

- Publish a Windows GPU prerelease with runtime/updater version `0.5.41`, newer than `0.5.40`. Enable **Include prereleases** to discover it; stable remains `0.5.38`.
- Promote the unchanged, verified 0.5.41 installers to the stable channel at the repository owner's request. This is not a claim that real-device acceptance passed.
- Fix HTTP 400 errors with current OpenAI models: GPT-5+ and o-series requests use `max_completion_tokens` and omit temperature/top-p. Other compatible endpoints keep `max_tokens` and configured sampling.
- Raise the Claude output cap from 2,048 to 16,000 tokens because current Claude models think by default and thinking counts toward the cap.
- Replace retired or shut-down model suggestions for Claude, OpenAI and Groq with current models, and skip image, transcription, live-voice and text-to-speech models in live model lists.
- Show the live Codex model catalog, following every page, instead of a built-in list that hid newer models. Bundle Codex app-server `0.157.0`, which adds GPT-6 Astra, Sol and Luna; the optional Codex voice host is not bundled.
- Pass 70 frontend tests and 296 Windows native tests in the GPU preflight. Live OpenAI/Anthropic requests, a signed-in Codex turn on the new runtime, and summary quality with the newly listed models remain unverified, along with the real-device capture, install/upgrade, and notebook performance acceptance listed for 0.5.40. This preview does not assert capture-smoke confirmation or stable readiness.

## 0.5.40

- Publish a Windows GPU prerelease with runtime/updater version `0.5.40`, newer than `0.5.39`. Enable **Include prereleases** to discover it; stable remains `0.5.38`.
- Preserve legitimate Whisper words, repetitions, and short German answers. Fix Whisper/Parakeet model-switch deadlocks, move speech-model loading off async workers, and avoid redundant batch PCM copies.
- Cancel hosted import/retranscription requests without starting local fallback. Resolve Nemotron Auto consistently from the system locale and avoid falsely labeling Parakeet Auto as English.
- Discard stale meeting, summary, source-page, and recording-poll results. Finish transcript animations, preserve edits made during saves, and avoid rewriting notes for title-only changes. Automatic summaries wait for saved notes and a configured provider.
- Share configured summary requests across chat and reviewed task polishing. Bound API response memory, keep cancellation active through response bodies, reject explicitly truncated/refused output, and preserve all Claude text blocks.
- Prevent duplicate summary jobs and stale cancellation updates. Serialize local sidecar request/reply exchanges and scan only appended output plus stop-marker overlap, retaining Unicode boundaries and discarding text after the earliest marker.
- Add a transcript paging index and consistent database snapshots. A synthetic 150,000-row count/page benchmark improved from median 12.813 ms to 0.342 ms; this is database paging, not transcription speed.
- Checkpoint each Microsoft export attempt and result. Block exports when history is invalid/unavailable, require review for unknown submissions, and preserve OneNote sections when a page may have been created. Remove sensitive provider diagnostics from reviewed paths.
- Pass 70 frontend tests and 293 Windows native tests in the GPU preflight, and restore full-workspace Rust formatting checks. Installer publication additionally requires native build/tests and downloaded signature/checksum verification.
- Real dual-source recording, graphical install/upgrade and installed-app update discovery, multilingual model quality, accessibility, and sustained notebook performance remain unconfirmed. MAI conversion and diarization can still use full-file memory; running native/conversion work may finish after cancellation. This preview does not assert capture-smoke confirmation or stable readiness. Updater signatures are separate from Windows publisher signing; the Vulkan loader remains a startup prerequisite.

## 0.5.39

- Publish a Windows GPU prerelease for manual evaluation and the opt-in **Include prereleases** update channel. Runtime version `0.5.39` advances beyond `0.5.38`; stable users remain on `0.5.38`.
- Add **Record now, transcribe later**: audio-only recording always saves audio, skips live recognition, and offers **Transcribe** on the saved meeting.
- Edit transcript passages, preview literal find-and-replace across a meeting, and undo corrections while retaining original recognition and segment timing. Failed recording-file synchronization remains visible and retryable.
- Find and replace summary text while preserving formatting and source links. Save corrected notes without a later title change restoring an older summary.
- Request summary links to supporting transcript passages. Inspect the source, reveal it in the transcript, or play its audio; flag references made stale by corrections. Citation coverage and factual support still require review.
- Create and edit summary templates and persist a default, with optional per-meeting selection. Structured summary providers retain export fields while rendering the selected template.
- Require native regressions for audio-only preservation, transcript edit/undo persistence, source references, and template storage/rendering. Publish detached updater signatures for both installers.
- Real microphone/system-audio recording, graphical installation/upgrade, model-generated citation quality, and sustained notebook performance remain unconfirmed. This preview does not assert capture-smoke confirmation or stable readiness. Updater signatures are separate from Windows publisher signing; the Vulkan loader remains a startup prerequisite.

## 0.5.38

- Publish version `0.5.38` to the stable update channel so installed `0.5.36` and `0.5.37` builds can discover it. The new prerelease option becomes available after upgrading.
- Add a persistent **Include prereleases** switch in Preferences and About. Stable remains the default; opting in applies to startup, manual, and tray checks without automatically installing anything.
- Select the newest eligible numeric Windows version across stable and preview releases. Ignore drafts and incomplete uploads, preserve signature verification, and never downgrade when leaving previews.
- Install the exact checked release, share concurrent checks, invalidate stale results when switching channels, and allow immediate retries after network or GitHub rate-limit errors.
- Show clear no-update results, preview labels, accessible controls, and download progress. Check recording state before download and installation, and release downloaded resources after failures.
- Include the recording recovery, bounded import/retranscription, transcript preservation, credential protection, and meeting-notes improvements from the 0.5.37 preview.
- Windows security policy can still block installer execution; updater signatures are separate from Authenticode signing. Real dual-source capture, graphical installation/upgrade, native UI scaling, multilingual accuracy, and sustained i5/8 GB performance remain unconfirmed. Long-recording diarization is still memory intensive.

## 0.5.37

- Publish a Windows GPU preview for manual installation and real-device testing. Runtime version `0.5.37` advances beyond `0.5.36`; this prerelease does not advance the stable automatic-update channel.
- Preserve captured audio in a recovery spool while encoding, drain accepted saver work at stop, and retain recoverable audio after encoder or finalization failure.
- Bound capture buffering, move native inference off the async executor, and make cancellation and incomplete-transcription outcomes visible and persistent. Automatic notes wait when recovery is needed.
- Prepare long imports and retranscription on disk, processing bounded speech segments instead of retaining the entire decoded recording in memory. Prevent competing local jobs from changing active models.
- Preserve existing transcripts when retranscription returns empty or incomplete text, and retain the prior transcript revision after a successful replacement. Let SQLite recover committed WAL data without deleting its transaction history.
- Migrate provider credentials to provider-isolated OS credential storage with an encrypted Windows fallback; remove sensitive content from reviewed diagnostic paths.
- Budget long-meeting summaries across providers, reject failed or incomplete reductions, and retrieve question-relevant chat excerpts from throughout the transcript.
- Show unavailable confidence honestly, preserve short utterances, and improve recording warnings, smaller-window layouts, onboarding labels, and progress accessibility.
- Respect Microsoft Graph `Retry-After` delays and preserve the designated local Windows build/storage policy.
- Real microphone/system-audio capture, graphical installation/upgrade, multilingual quality, native UI scaling, and sustained i5/8 GB performance still need acceptance testing. Long-recording diarization remains memory intensive; an active Parakeet native call may finish after cancellation. Updater signatures are separate from Windows publisher signing.

## 0.5.36

- Fix native Windows test execution by explicitly loading the same staged sherpa/ONNX runtime DLLs as the installer; keep tests mandatory and run them against release-profile code.
- Preserve all transcript content at summary chunk boundaries, including Unicode.
- Reject failed or empty summary chunks rather than silently publishing incomplete notes.
- Ground summaries in stated facts, distinguish proposals from decisions, and retain uncertainty.
- Preserve meaningful multilingual words in the transcript display.
- Improve live-follow scrolling, user interruption, cleanup, and reduced-motion navigation.
- Add a visible jump-to-live control and accessible timestamp playback buttons.
- Preserve custom speaker input on failed saves, show an actionable error, and prevent duplicate submissions.
- Detect physical RAM instead of assuming 8 GB; cap Windows Whisper threads conservatively on 8 GB notebooks.
- Bound diagnostic logging and flush quiet batches without dropping audio.
- Include the previously unreleased token-storage encryption and short-utterance preservation fixes.
- Make Windows build failures fatal, require both current-version installers, and run native regression tests before staging release assets.
- Support draft Windows releases without changing the stable updater or bypassing the real audio-capture publication gate.

## 0.5.35

- Reworked Home into a focused capture workspace with a clearer recording
  console, an explicit microphone-to-transcription signal path, recent
  meetings, and faster access to import and configuration actions.
- Fixed preferred microphone and system-audio devices staying stale on Home
  after they were changed in Settings. Home and recording startup now consume
  the same shared device state immediately after the backend accepts a change.
- Made long recordings more resilient under transcription load. Live speech
  segments are staged in a memory-bounded, disk-backed queue, queue progress is
  reported during stop, and the app preserves the recording and warns clearly
  if queued transcription cannot be written or fully drained.
- Reduced peak memory and steady-state overhead by removing redundant audio
  buffer copies during import, retranscription, and hosted-provider retries;
  limiting decoder preallocation from untrusted media metadata; and removing
  high-frequency no-change logging/state updates.
- Updated the bundled Codex app-server to `0.144.1`, made `gpt-5.6-sol` the
  default Advanced Codex summary model, and populated the summary model picker
  from the runtime's live model catalog with reasoning-effort choices.
- Fixed meeting chat occasionally showing a Codex answer twice when streamed
  deltas and the completed response snapshot both carried the same output.
- Improved transcription language selection with system-locale defaults and
  clearer explicit-language behavior, including German.
- Upgraded the UI runtime to supported Next.js 15 / React 19 releases and added
  release guards for public-repository safety, immutable version tags, and
  repeatable pnpm 10 builds.
- `latest.json` advertises runtime version `0.5.35`, so installed `0.5.34`
  clients can discover this update.

## 0.5.34

- Fixed `0.5.33` failing to open the meeting database on machines whose
  database was stamped by builds with different migration-file line endings
  ("migration was previously applied but has been modified"). The app now
  reconciles stored migration checksums that match the same SQL under either
  line-ending convention, while genuinely modified migrations still fail.
- The release workflow now force-refreshes migration files after checkout so
  every build embeds the canonical LF migration bytes regardless of runner
  workspace history.
- `latest.json` advertises runtime version `0.5.34`, so installed `0.5.33`
  clients can discover this update.

## 0.5.33

- Convert audio formats Azure MAI-Transcribe does not accept (for example
  M4A/MP4 recordings) to 16 kHz mono WAV locally before cloud upload, so
  retranscription of voice files no longer fails with a misleading
  API-key error.
- Preflight MAI-Transcribe uploads against Azure's 300 MB limit and classify
  provider rejections honestly: size and duration limits report as
  `upload_too_large`, unsupported formats use a new `unsupported_media`
  fallback toast, and the provider's error detail is kept in logs for
  diagnosis instead of being discarded.
- Fixed meeting-screen playback jank on slower systems: the playback clock no
  longer re-renders the whole meeting page 60 times per second, transcript
  rows only re-render when their highlight changes, and the speaker timeline
  reconciles only the playhead per tick instead of every segment bar.
- Load meeting audio over binary IPC instead of a JSON number array, removing
  a multi-second UI stall when opening meetings with large recordings.
- `latest.json` advertises runtime version `0.5.33`, so installed `0.5.32`
  clients can discover this update.

## 0.5.32

- Preflight OpenAI Hosted Whisper retranscription uploads against the 25 MB
  request limit, so oversized recordings skip the doomed cloud call and switch
  to local transcription with the `upload_too_large` category.
- Classify provider error bodies that mention maximum file or content size as
  `upload_too_large` even when the HTTP status is not 413.
- Reworded cloud fallback toasts to say ClawScribe is switching to local
  transcription, avoiding a premature success claim before fallback completes.
- `latest.json` advertises runtime version `0.5.32`, so installed `0.5.31`
  clients can discover this update.

## 0.5.31

- Added an in-app hosted transcription provider smoke test in Settings ->
  Transcription, so Hosted Whisper and MAI credentials can be checked against a
  selected audio file before running a full retranscription.
- Fixed the Test button by granting the desktop file-picker permission and
  routing picker/provider failures through visible toast errors instead of
  failing silently before the backend command runs.
- Documented the hosted transcription smoke-test flow, including the CLI
  live-smoke environment variables and the in-app audio-file picker.
- `latest.json` advertises runtime version `0.5.31`, so installed `0.5.30`
  clients can discover this update.

## 0.5.30

- Clarified the auto speaker-diarization significant-speaker threshold as a
  capped threshold rather than a minimum, matching the behavior shipped in
  `0.5.29`.
- Added a long-meeting regression test so brief but real speakers are preserved
  as meaningful diarization lanes instead of being treated as micro-lanes when
  a dominant speaker lasts several minutes.
- `latest.json` advertises runtime version `0.5.30`, so installed `0.5.29`
  clients can discover this update.

## 0.5.29

- Added beta-gated cloud transcription providers for whole-file retranscription:
  Hosted Whisper through OpenAI-compatible file transcription and
  MAI-Transcribe 1.5 through Azure Speech Fast Transcription.
- Kept cloud transcription opt-in with explicit consent, separate provider
  credentials, and local fallback notifications when cloud requests fail.
- Preserved the diarization provenance invariant: Hosted Whisper word timings
  are treated as real when returned by the provider, while MAI sentence-level
  timing never emits fabricated word timestamps.
- Added the OpenAI-compatible 25 MB upload-size fallback so oversized cloud
  uploads report a size-limit fallback instead of misleading users toward API
  key troubleshooting.
- Remapped collapsed MAI output onto the local VAD speech grid when Azure
  returns combined-only or single-phrase transcripts, while keeping row timing
  approximate and speaker diarization on the conservative path.
- Stabilized the diarization release gate so unreliable speaker mappings are
  rejected before they overwrite useful labels.
- `latest.json` advertises runtime version `0.5.29`, so installed `0.5.28`
  clients can discover this update.

## 0.5.28

- Fixed interrupted recording recovery so checkpoint-temp audio can still be
  found and restored.
- Centralized audio-recovery lookup so meeting and recording flows use the same
  recovered-audio path.
- Restored meeting audio actions after recording completes.
- Gated source attribution on saved audio so transcripts do not claim an audio
  source before one exists.
- Made toast notifications and the recording CTA theme-aware.
- Cleared stale diarization labels when speaker mapping is rerun in overwrite
  mode and a segment no longer has diarization coverage.
- Recovered compact diarization speaker hints that land in word-timestamp gaps
  by anchoring them to the nearest word.
- Flushed checkpoint-temp audio through a writable handle on Windows before
  rename so interrupted-recording recovery files are durable.
- `latest.json` advertises runtime version `0.5.28`, so installed `0.5.27`
  clients can discover this update.

## 0.5.27

- Kept retranscription and speaker-diarization benchmark dialogs mounted while
  transcripts refresh after processing, so completion stats stay visible instead
  of disappearing behind a full-page reload.
- Preserved existing transcript rows during post-processing refreshes to avoid
  blank flashes while the updated transcript is reloaded.
- Restored readable contrast for the retranscribe warning callout in dark mode.
- Stitched short batch-transcription fragments back into readable transcript
  rows while keeping word timestamps non-mutating for speaker-diarization
  splitting.
- Added the final local diarization recovery attempts for compact interjections,
  short speaker islands, weak mappings, and dominant-host explicit counts.
- `latest.json` advertises runtime version `0.5.27`, so installed `0.5.26`
  clients can discover this update.

## 0.5.26

- Added diarization mapping diagnostics to speaker-detection profiles so support
  payloads can show timestamp eligibility, prepared turn counts, and split-block
  reasons when speaker labels look wrong.
- Removed the redundant top-level Add-ons sidebar item now that Add-ons lives
  under Settings.
- Kept Settings in the sidebar footer, including the collapsed icon rail, so it
  stays with the recording/import controls instead of duplicating primary
  navigation.
- Neutralized dark-mode shell colors from blue-black to charcoal while keeping
  the Claw cyan/blue accent for active states, recording controls, and primary
  actions.
- `latest.json` advertises runtime version `0.5.26`, so installed `0.5.25`
  clients can discover this update.

## 0.5.25

- Redacted copied diagnostics so exported support payloads no longer include
  local account names, email addresses, home-folder paths, or endpoint hosts.
- Recovered Auto speaker diarization when clustering over-fragments into
  micro-lanes by rerunning with the meaningful speaker count instead of saving
  unreliable labels or failing immediately.
- Improved short speaker-turn splitting for Parakeet transcripts by using real
  word timestamps to split brief interjections that were previously swallowed by
  longer transcript rows.
- Tracked word-timestamp provenance so real ASR anchors can use fine-grained
  speaker splits while estimated timestamps stay on the conservative path.
- Clarified Settings -> Add-ons copy and labels, including Confluence/Calendar
  discoverability and clearer Teams auto-record/status wording.
- `latest.json` advertises runtime version `0.5.25`, so installed `0.5.24`
  clients can discover this update.

## 0.5.24

- Added a Diagnostics health snapshot with model, audio, storage, runtime, and
  speaker-diarization status so troubleshooting no longer depends on scattered
  logs.
- Let the speaker diarization progress dialog continue in the background while
  the run remains active, then return to the result/error state when processing
  finishes.
- Prevented English speaker-diarization retries from falling through to the
  Mandarin `zh-cn` embedding model, so English meetings stay on English
  speaker embeddings.
- Made failed diarization non-destructive: collapsed, low-confidence, or
  zero-speaker mappings now return an error without clearing existing speaker
  labels.
- Treat zero mapped transcript speakers as a failure instead of a successful
  completion with `0` speakers.
- Made the speaker diarization benchmark window reliable by returning the
  completion payload from the Tauri command as well as emitting the existing
  completion event.
- `latest.json` advertises runtime version `0.5.24`, so installed `0.5.23`
  clients can discover this update.

## 0.5.23

- Preserved Parakeet token timestamps through live transcription, import, and
  retranscription paths so speaker-turn splitting can use real word anchors when
  available.
- Tightened import and retranscription VAD redemption so transcript rows are
  less likely to bridge speaker handoffs before diarization runs.
- Blocked unreliable Auto diarization from saving over-fragmented generated
  speaker labels, while preserving user-renamed speaker labels.
- Moved speaker diarization progress into the same dialog-style workflow used
  for import and retranscription, including percent, stage, and status details
  while speaker detection runs.
- Kept the speaker diarization completion window open with benchmark stats for
  audio length, processing time, realtime speed, provider, embedding model,
  detected speakers, speaker turns, and updated transcript rows.
- Removed the outdated "Current workflow" callout from About.
- `latest.json` advertises runtime version `0.5.23`, so installed `0.5.22`
  clients can discover this update.

## 0.5.22

- Stopped explicit multi-speaker diarization from silently saving collapsed
  labels when most transcript time maps back to one speaker.
- Added a mapping-quality gate for explicit speaker counts, including dominant
  speaker share diagnostics in diarization profiles.
- Added automatic fallback across alternate speaker embedding models when the
  requested speaker count collapses with the default embedding.
- Return a clear error instead of writing bad labels if every embedding still
  collapses, so users can retry with different speaker settings without
  corrupting the transcript.
- `latest.json` advertises runtime version `0.5.22`, so installed `0.5.21`
  clients can discover this update.

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
