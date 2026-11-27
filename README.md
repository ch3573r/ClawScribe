# ClawScribe

![ClawScribe README hero](docs/brand/clawscribe-readme-hero.png)

ClawScribe is a local-first Windows desktop companion for meetings, calls,
interviews, and recorded audio. It captures microphone and system audio from
your own session, transcribes speech locally, and turns transcripts into
reviewable meeting notes and action items. No meeting bot is required.

**0.67.0** adds Markdown export, live conversation metrics, an optional webhook
and compact floating recording controls. The OpenClaw integration is removed.
See the [release notes](docs/releases/0.67.0.md) and
[download the latest stable Windows release](https://github.com/ch3573r/ClawScribe/releases/latest).

**0.68.2-preview.1** builds on 0.68.1-preview.1: action items catch short
commitments such as "I will send the draft" and link back to their transcript
passage, and release builds are faster. It is a prerelease for manual
evaluation; the stable update stays on 0.67.0. See the
[preview notes](docs/releases/0.68.2-preview.1.md). Its installer filenames use
numeric version `0.68.2`, as required by existing preview updaters.

ClawScribe is based on Meetily Community Edition **0.4.0**. Attribution and
license details are in [UPSTREAM.md](UPSTREAM.md), [NOTICE.md](NOTICE.md),
and [LICENSE.md](LICENSE.md).

## Install And Start A Meeting

Use the NSIS setup installer from the selected GitHub Release; an MSI is also
provided for deployment scenarios. Read that release's validation and signing
notes before installing. Draft builds are unpublished, and prereleases do not
advance the stable automatic-update channel.

Starting with 0.67.0, both Windows installers include the required Microsoft
Visual C++ runtime DLLs. Releases through 0.66.1 require the Microsoft Visual
C++ 2015–2022 x64 Redistributable to be installed.

The Windows GPU build needs the Vulkan loader (`vulkan-1.dll`) at startup, even
when selecting a non-Vulkan model. Use a supported GPU driver or the official
Vulkan runtime. The CI runtime fix does not install a driver or runtime on your
PC. See [Windows runtime prerequisites](docs/windows-release.md#windows-runtime-prerequisite)
for diagnosis; never download individual DLLs from third-party sites.

Choose a microphone and the output device used by Teams, Webex, or the other
meeting application. Make a short test recording and verify that both your
voice and the remote participants are audible in playback. Start recording,
review the live transcript, then stop and allow queued transcription to finish
before generating notes. You can also enable call detection for selected
meeting apps under **Settings → Recording → Call detection**. Call prompts
appear above the meeting app without taking focus. Timed Teams invitations
from Microsoft 365 and iCalendar can use **Join and record**.

**Test mic** in Recording settings shows the microphone actually tested. If a
preferred Windows microphone is unavailable, the test identifies its fallback
to the current default. The saved meeting records quiet system-audio periods as
diagnostics. A warning appears if a previously responsive microphone stops
delivering samples for 15 seconds. Verify both sources in saved playback before
relying on the recording.
The recording bar shows capture warnings, queued speech, available disk space,
and the observed recognition backend when available. The compact floating
controls show capture warnings and let you pause, resume, bookmark, and add notes.
During Stop, the window shows what is being saved. Choose **Start after saving**
to request the next recording once the previous one is in the library.

Review names, numbers, decisions, owners, and deadlines before sharing notes or
exporting tasks. Obtain the recording permissions required for your meeting.

## Command-line control

While ClawScribe is running, launch it again with a flag to control the running
instance from a Stream Deck, AutoHotkey, or a script:

- `--toggle-recording` starts recording, or stops it if one is running.
- `--start-recording` starts recording unless one is already running.
- `--stop-recording` stops the current recording if there is one.

Example: `"C:\Path\To\ClawScribe.exe" --toggle-recording`. The flags only work
on an instance that is already running; launching the app with a flag does not
start a recording. A start from the command line, the tray or a global shortcut
runs in the background: the window keeps its page and any unsaved text, and it
is not brought to the front. See
[Recording from the tray, shortcuts and command line](docs/tray-and-recording-control.md).

Closing the main window keeps ClawScribe running in the system tray by default.
Under **Settings → General → Close button**, choose **Quit** to exit when
closing the window. If a recording is active, the window stays open so you can save it.

## Capture And Transcription

- Microphone and system-audio capture from the local Windows session.
- Floating controls open when recording starts. The window shows elapsed time,
  both audio levels, timestamped notes, and Stop. The window closes after a
  successful save and offers to open ClawScribe if recovery is needed. You can
  also close it while recording continues. To keep it closed on future starts,
  turn off **Open floating controls when recording starts** in **Settings → Recording → Capture**.
  **Open floating controls** on the recording bar or tray can reopen it at any time during capture.
- Customize saved-meeting playback keys in **Settings → Shortcuts**. The
  player's shortcut help shows your current bindings.
- Live transcription, audio/video import, and retranscription with a different
  model or language selection.
- **Record now, transcribe later:** choose **Record audio now** on first launch
  to open Home in audio-only mode without a model download. Audio-only mode
  always saves audio and leaves a **Transcribe** action on the saved meeting.
  Live transcription and the local notes model can be set up independently.
  If a speech model fails after capture opens, a session with audio saving
  enabled continues in audio-only mode.
- **Check recording readiness:** Choose the microphone and system output under
  **Signal path** on Home. These choices also apply in Recording settings and
  when starting from the tray. **Recheck devices** refreshes the list after a change.
  Home shows the on-device live engine and any hosted file-transcription preference separately.
- Recording continues after temporary queue pressure or spool-write failures,
  with incomplete audio reported visibly. Playback streams saved recordings.
  Stop saves available audio even if transcript backup writes fail, and capture
  gaps are reported even after a source reconnects. Retry a missing source
  without interrupting the other source. Stop saves one library meeting and
  leaves the recording available in **Interrupted recordings** if the library
  write fails. Recovery originals use about 700 MB per hour while recording. Choose a local
  folder with enough space. See [recording and recovery](docs/meeting-quality.md).
- On Windows, a sleep notification asks ClawScribe to save the current recording.
  Sign-out and shutdown use the recording recovery files if Windows closes the
  app before it can finish. See [Windows sleep and sign-out](docs/tray-and-recording-control.md#windows-sleep-sign-out-and-shutdown).
- Correct transcript passages or preview literal find-and-replace across the
  entire meeting. Original recognition and segment timing are retained; undo
  restores previous corrections. Regenerate notes after correcting a transcript.
- **Restore previous transcript** after retranscription or speaker detection.
- Detect speakers in a saved recording and cancel from the dialog or meeting
  toolbar. Cancellation waits for the current native step before another recording
  can start.
- Optionally remember a speaker’s voice on this device after naming them. Later
  speaker detection can suggest their name, or apply it if you enable automatic
  matching in **Settings → Transcription**.
- Import support for MP4, M4A, WAV, MP3, FLAC, OGG, AAC, MKV, WebM, and WMA.
  Review and queue multiple files from the picker or by dropping them on the
  app. Cancel or retry an individual import from the queue. Open a saved meeting
  to repair missing folder files without transcribing the audio again.
- Apply a meeting date to the selected files, use each file's modified time,
  or keep each import time. You can also queue summaries after transcription.
- HE-AAC imports use the decoder's actual sample rate to preserve timing.
- Interrupted Whisper and Parakeet downloads retain partial files for retry;
  cancellation finishes the active transfer before another attempt can start.
- Disk-backed live recognition queue and interrupted-recording recovery paths.
- Timestamp playback controls, speaker-label editing, and a **Jump to live
  transcript** control when reading earlier text during a recording.

| Engine | Role |
| --- | --- |
| Parakeet | Default local fast path, with multilingual stock v3 int8, Parakeet Ultra (fine-tuned v3, int8 or fp16), and SmoothQuant int8. Compact (v2 int8) is English only. Supported GPU builds can use DirectML. |
| Whisper | Local whisper.cpp/whisper-rs engine with selectable models and Vulkan support in the Windows GPU build. |
| Nemotron | Beta multilingual Nemotron 3.5 ASR path. fp16 is CPU-capable; int8 is intended for DirectML-capable builds. |
| Hosted Whisper | Import and Enhance only (beta), through an opt-in OpenAI-compatible file-transcription API. |
| Azure Speech | Import and Enhance only (beta), through opt-in standard Azure Speech Fast Transcription; credentials are separate from Microsoft Graph sign-in. |

**Run transcription in a separate process (beta).** With this switch on in the beta
settings, Parakeet loads and transcribes in a separate process. If the speech
model crashes or stops responding, ClawScribe restarts it and retries that
segment once, so the recording keeps running. After a failure with GPU
acceleration on, the rest of the session uses the CPU and the settings page says
so. The switch is off by default and applies to Parakeet only; Whisper and
Nemotron run inside the app as before.

Speech, speaker-detection, and Built-in AI summary models downloaded in the app
are pinned to specific revisions and SHA-256-verified before use. Damaged or
incomplete files require re-download; unchanged verified files reuse their
integrity checks.

In **Settings → Transcription**, **Browse engines** shows the available models.
Choose **Use** to switch to an installed model. **Test this PC** times the selected
Whisper, Parakeet or Nemotron model on two built-in speech samples in German and English.
The result shows the model, runtime backend, and processing time for each
sample. When two or more installed models of an engine recognized both samples
on the same backend, the fastest measured variant is marked. If the model falls
behind, you can switch to **Record now, transcribe later**. The test runs locally
and leaves the microphone free.

Whisper preserves recognized repetitions and short answers instead of removing
them through phrase matching. Nemotron's **Auto** language follows the system
locale; select the spoken language explicitly when it differs. Parakeet's Auto
selection does not label an unknown language as English.

In **Auto** language mode, the live transcript can show a separate language for
the microphone and system audio. Whisper uses confirmed languages to help with
short replies and continues checking longer turns for language switches.
Multilingual Parakeet estimates the languages from finalized text. When
the two sources overlap, ClawScribe keeps the transcript and leaves each
source's language unchanged. See [live transcript behavior](docs/meeting-quality.md#live-transcript-behavior).

**Settings → Transcription → Custom vocabulary** stores up to 500 names or
terms (64 characters each) and corrects similar spellings in new recordings,
imports, and retranscription. A recording uses the list loaded when its live
transcription starts for spelling correction. Paste several terms at once by
separating them with commas or line breaks. Move important terms to the front
to prioritize them in Whisper. Matching supports Unicode names and terms, including
German umlauts and ß. Short terms require an exact match apart from case and
punctuation; single-letter keys such as C# and C++ are left unchanged.

Cloud transcription requires explicit opt-in. Saved cloud settings remain visible
when the opt-in is off. Settings explain which local engine and model live
recordings use. Cloud APIs apply to Import and Enhance (whole-file), while live
recordings stay on-device.
Hosted Whisper can provide word timestamps. Text-only responses use local
speech boundaries to estimate passage timing. For OpenAI, ClawScribe converts
large files to 16 kHz mono MP3 and splits them to fit the 25 MB upload limit.
Custom Whisper endpoints receive the original file up to 500 MB. Larger files
are compressed and split using the same process.
Azure Speech uploads are limited to 300 MB and use WAV, MP3, or FLAC, with
other formats converted locally to 16 kHz mono WAV. Rejected cloud
requests can fall back to local transcription with a notification explaining
why. See [hosted transcription verification](docs/hosted-transcription-smoke.md).

## Meeting Notes And Exports

Press **Ctrl+K** to open the command palette. Search meetings, transcript
passages, and open actions, or jump to a Settings page. The palette also
offers recording controls, audio import, and Meeting memory.

Search saved meeting titles, current transcripts, summaries, personal notes
and bookmarks from the sidebar or **Meetings → Archive**. Search for several
terms in any order or put an exact phrase in quotes. Results are grouped by
meeting and highlight matches in the saved text. Open a source in its meeting
or play a timed transcript result. The Archive keeps the current search,
project, date and attention filters when you return. Choose **Hybrid** to add
semantic transcript matches when the local semantic index is ready. Use
**Select meetings** for bulk tagging, local export, deletion, or to scope a
Meeting memory conversation.

Read and correct the transcript while the speaker timeline loads. The text
stays available when the recording is missing. Use **Find and replace** to
preview name corrections across the meeting, with a **Whole words only** option.

**Ask about these results** opens Meeting memory with the displayed matching
meetings selected and the search text in an editable draft. Review the sources,
provider and question before choosing **Ask**. With a provider that supports
reviewed excerpts, check any personal note or bookmark you want to include.
**Review and ask** shows the exact excerpts, question, provider,
model and endpoint. **Confirm and send** includes those excerpts once.

The meeting-memory preview in this source branch adds **Meetings → Meeting
memory** in a separate tab: use the searchable picker to select saved meetings
explicitly, or choose all saved meetings, then
search transcript passages and ask cited questions. Project Any/All/Untagged
filters and inclusive dates constrain that same selection. Keyword retrieval
works without downloading a model; optional local semantic retrieval uses the
embedding model under **Settings → Meeting memory**. Installed files are
verified when first needed after startup or enabling semantic search. Results show the mode
actually used, and answers show the configured and executed summary provider.
The first question starts a saved conversation automatically. Conversations
restore their last submitted meeting scope, and answers can be copied. Model settings
show download bytes and integrity verification separately from indexing. The
pinned multilingual E5 model and tokenizer use ONNX Runtime locally on CPU;
healthy files can be verified without downloading them again.
Open citations to inspect evidence, reveal its current transcript position, or
play saved audio at a verified recording offset. Historical, changed, missing,
and invalid sources remain explicitly identified and cannot navigate as current
evidence. Existing meeting chat is preserved and defaults to this meeting only.
Expanding to other meetings uses a separate saved library conversation; switching
back restores the meeting's history.
Saved meetings also accept PDF, DOCX, TXT and Markdown reference documents.
Use **Attach file** directly in **Meetings → Meeting memory**; a single selected
meeting is the destination, otherwise choose a saved meeting there. The compact
corner chat on a meeting's details has a paperclip beside the question field.
Its **Context** control holds the meeting picker, project filters, and reference
files; selected files appear as removable chips above the question. **Search mode**
is in **More options**. Attach and preview files locally, then explicitly select which references a
question may use. Citations open the extracted page or paragraph, keeping
reference material distinct from the recorded discussion. Sending excerpts to
an external answer provider requires **Use selected references for answers** for that
conversation, off by default.

The **Conversation** readout estimates talk time, pace, questions and uninterrupted
speaking stretches during live transcription. It runs on this PC and can be hidden
in **Settings → Recording**. See [conversation metrics](docs/meeting-quality.md#conversation-metrics).

During a recording with live transcription, choose **Live assistance** on Home
to open a panel alongside the transcript. Explicitly choose **Ask**, **Summarize the last 10 minutes**, or **List open
questions** using the configured summary provider; no provider request runs
automatically. **Share live transcript with the provider** is off by default
and remembered for future recordings. Switch it on in the panel or in Summary settings before asking.
Selected saved meetings can supplement the current recording. Document selection
is explicit, and document sharing is independent and resets for each recording.
The panel and each answer show finalized-through time and incomplete status;
recent speech may still be catching up. Context is a fixed window of up to ten
minutes of finalized speech, ending at the finalized-through time. It is shorter
early in a recording or when the segment or size limit applies, and the answer
says so.
Cancel abandons assistance without waiting for the provider. Stop immediately
clears temporary Live messages; navigation preserves the current request without
starting another listener. Audio-only recordings and Built-in AI offer saved
meeting/transcription guidance instead of live generation. Ollama may compete
for this PC's resources; concurrent recording performance remains unmeasured.

Installed offline, provider-failure, focus, citation,
and playback acceptance remains pending; see the
[local library guide](docs/local-library.md#meeting-memory) and
[preview acceptance policy](docs/windows-release.md#required-real-device-acceptance).
Exact-build dual-source recording and comparable two-hour enabled/disabled
Live performance acceptance remain pending. Stable **0.5.58** was published
without these checks.

- **Local document export:** save a summary, full transcript, or both as Word (`.docx`) or Markdown (`.md`),
  including tables, with optional speaker labels and recording-relative timestamps. Works offline without Microsoft
  sign-in or Word installed.
- **Project tags:** choose existing labels from a searchable list or create new
  ones, then filter the archive and transcript search by one or more projects
  (matching any or all), or show untagged meetings.
- **Meeting bookmarks and notes:** mark a moment from the recording bar, tray, or
  keyboard. Add timestamped notes while recording or during playback, then revisit
  them in the saved transcript. Choose whether to include them in meeting summaries.

Generate template-based meeting summaries from the transcript and optional
context, regenerate notes, and chat about the selected meeting. Per-meeting summary
context is saved in the local library and included in backups. Configurable
providers include Built-in AI, Ollama, OpenAI, OpenAI-compatible endpoints,
OpenRouter, Anthropic/Claude, Groq, and the advanced
bundled Codex app-server path, plus **Claude Code** using your own installation.

Enable **Summarize automatically** in Summary settings to generate local notes
with Built-in AI or loopback Ollama after saving a recording. External providers
with a known destination require a review in Summary settings. A changed provider,
model, destination, or expansion of the meeting data sent requires fresh approval.
Review existing external approvals again to include the local meeting date and
weekday, duration when known, and title when permitted. If a recording has a capture gap,
notes can still use its saved transcript. Transcribing an audio-only recording
also starts approved automatic notes once the transcript is saved. For audio
imports, you can review and use the selected provider for that file. Reopen the
meeting to check progress, see why notes were skipped, or recover excerpt
notes after a failed attempt.
Windows notifications can tell you when automatic notes are ready. See
[Automatic notes](docs/automatic-notes.md) for supported providers and recovery.

Setup offers a cancelable local notes model download and a separate choice to
generate local notes automatically. Regenerating a summary keeps the saved
notes readable. If translation fails, review the saved English draft and use
**Retry translation** to translate that draft.

**Claude Code**: Uses the Claude Code you installed and signed in to. Your Claude
plan's limits apply. Install it yourself, run `claude auth login` in a terminal,
then select **Claude Code** in Summary settings. ClawScribe does not install
Claude Code or handle its sign-in or credentials. It supports text summaries and
meeting chat with tools disabled. Choose Sonnet (default), Opus, or Haiku. The
CLI aliases use a bounded context budget. Prompts travel through stdin
and a temporary UTF-8 system file, keeping prompt text off the command line.
See [Claude Code setup and limitations](docs/auth/claude-code.md), including
Windows command shims and the optional Program path setting.

The advanced Codex provider bundles app-server **0.169.0**, with GPT-6.1 Sol
(`gpt-6.1-sol`) in its model catalog. Choose **Check bundled runtime** in Summary
settings after upgrading to refresh models. Availability depends on the signed-in account and workspace;
your saved model selection is preserved. OpenAI's API model fallback list also
includes GPT-6.1 Sol.

In **Settings → Summary**, create, edit or delete personal templates, reset
customized built-in templates, and choose a default. Each meeting can use a
different template when generating notes.
Summary find-and-replace preserves formatting and source links; review the
changes, which save automatically. Use the editor's undo command to revert changes.

Meeting titles and summary edits save automatically. Regenerating edited notes
asks for confirmation, and **Restore previous summary** lets you return to the
previous version after a successful generation. Newly generated notes show the
model and template used, and flag later changes to the saved transcript. When
the provider produces a follow-up email, open **Follow-up email** to read or
copy the draft.

Saving a title leaves unchanged notes alone. Failed saves retain drafts, and
edits made during a save remain available to save again. Switching meetings
discards late loading results from the previous meeting.

New summaries request links to supporting transcript passages. Open a source
link to inspect the text, reveal it in the transcript, or play its saved audio.
Changed or replaced passages are flagged and require regenerated references.
Citation coverage depends on the model; inspect the passage to check the claim.

Open **Meeting actions** from a saved meeting to review suggestions from the
summary and its structured action list, or add follow-ups manually. Saved
actions keep their owner, due date, completion status, and available transcript
evidence when the summary is edited or regenerated. This works offline.

**Settings → Calendars** lets you import an `.ics` file or add a read-only
subscription from a calendar provider. Upcoming meetings appear on Home, and
the sidebar shows the next one. On Windows, **Join and record** opens a supported
Teams invitation and watches for that meeting for up to five minutes. If Teams
doesn't expose its meeting link, confirm the event before capture starts.
Choose **Use for next recording** to select an event for a manual start or let
ClawScribe link a single timed event in progress. For early joins, choose an event
starting in the next five minutes. Review attendance while
recording or from the saved meeting. Summaries receive only calendar names you
confirm. In a saved recurring meeting, open **Attendance** to choose a summary
template for the series. Open **Recurring meeting** in the meeting menu to set a
series language, select a preferred follow-up, or approve notes for later
preparation briefs. **Upcoming meetings** on Home can show a recurring meeting's
brief from earlier reviewed occurrences. This works with Microsoft 365 and
iCalendar. [Calendar sources and attendance](docs/integrations/icalendar.md)

Microsoft sign-in supports calendar context and exports. Windows call detection
can prompt or start recording for the Teams, Zoom, Webex and Google Meet
providers you enable. Optional automatic stop applies to the recording started
for that call after the detector observes its end. Existing Teams preferences
remain Teams-only until you select other providers.
See [call detection](docs/teams-detection.md).

- **OneNote** exports notes or transcripts to a new section. Use a saved section
  to resume an export or review and update an exported page without listing
  sections.
- **Planner and Microsoft To Do** export selected, reviewed actions. Repeat
  the export to repair missing notes or due dates on tasks already created.
- **Follow-up email** lets you edit a draft, select confirmed attendees, and
  open the draft in your default email client.
- **OneDrive or SharePoint** saves Word and optional PDF files. Repeating a
  partial export reuses confirmed files and retries unfinished steps. Edited
  drafts get dated filenames.
- **Confluence** copies nested lists, code, quotes, and checklist status into a
  browser draft, or publishes through a configured self-hosted REST endpoint.
  If the page is already published, use **Open** to apply reviewed edits in
  Confluence.

**Export history** shows Microsoft and Confluence results from the meeting and
travels with portable backups. Inspect the original destination and choose
**Resolve outcome** for an interrupted export or **Review link** for a saved
item. ClawScribe checks the item and destination before saving the link.
See [export recovery](docs/integrations/microsoft-graph.md#export-history-and-recovery).

An optional outbound webhook sends saved meeting details, notes and selected
transcript text to your automation endpoint. Configure it in **Settings →
Webhook**. [Webhook setup and request format](docs/integrations/webhook.md)

Private-network HTTP for custom OpenAI endpoints and Confluence requires
an explicit unencrypted-HTTP opt-in in Settings. Existing private HTTP setups
retain access on upgrade. Tailscale addresses and MagicDNS names are supported.
Public HTTP destinations are rejected on save/send; existing settings remain
editable and explain what needs correction. Confluence remembers its HTTP choice
for the saved server even while that server is offline.

## Library And Data

**Meetings → Archive** shows recording source and measured duration when available.
Filter by recurring series or narrow to meetings that need attention to find
incomplete recordings, missing transcripts, and failed summaries. Use
**Storage** to measure the library or select meetings to review which saved
audio copies can be removed while keeping their transcripts and notes.
See [Archive review and storage](docs/local-library.md#archive-review-and-storage).

**Meetings → Open actions** lists unfinished reviewed actions across meetings.
Filter by owner, due date, or recurring series, then open the source meeting
to complete the action or review an export.

During recording, choose **Save answer to meeting notes** on a live answer to
keep it and its cited finalized speech. Find it under **Saved live answers** in
the saved meeting. Other live answers clear at Stop.

**Settings → Assistant access** offers separate permissions for an MCP client.
Follow-through access reads open reviewed actions and basic metadata for a
meeting reference you provide. Library access supports filtered meeting and
passage searches, cross-meeting open actions, notes, bookmarks, attachment
metadata, transcripts, and recording paths. Both permissions use local standard
input and output and start off. [Local assistant access](docs/local-library.md#local-assistant-access)

**Backup and restore** saves a portable archive of meeting data, transcript history,
recordings, custom templates, vocabulary, approved series notes, and linked
calendar series preferences. It includes explicitly saved live answers when their
recording supplied all the source material. Restore adds missing meetings and
offers optional audio repair for matching meetings. Existing audio, notes, and
preferences are preserved. Backups continue when recording
folders are missing and report affected meetings,
excluded recovery files, unavailable audio, skipped custom template items, and
excluded saved live answers.
Archives exclude credentials and models. Manual backups can be encrypted with
a passphrase. Optional daily backups keep three unencrypted copies in a chosen
local folder.
Recordings take priority over scheduled backups, which retry when the library is idle.
If the database fails to open, a separate recovery library can import one of
these archives while preserving the original files. From recovery, select
**Return to original library**, then quit and reopen ClawScribe to try it again.
Saved knowledge conversations, attached reference originals and extracted blocks,
and their original citation labels are included. Reference sharing permissions
are excluded and remain off after restore.
Restored references remain historical until a new question establishes fresh
evidence; interrupted answers are never resubmitted automatically.

Meeting deletion can also remove its recording folder, including audio,
transcript copies, metadata, recovery originals, and generated documents. The
option is on by default. Folders outside supported storage locations, shared
with another meeting, or failing ownership checks are kept. Review retained
files and retry cleanup in **Meetings → Archive → Storage**. Staged cleanup
stays pending while its storage location is unavailable.
Pre-migration database snapshots and separate backup archives can retain deleted
meetings. See [local library tools](docs/local-library.md#deleting-meetings).

The recordings folder picker saves the verified target of a link or junction.
For an older linked setting, choose the folder again in **Settings → Recording**.
If a recovery scan cannot verify a recording location, reconnect it and choose
**Retry scan**. You can preview an unverified draft and save its transcript
while the folder is unavailable. ClawScribe can add verified audio to the same
library meeting when it returns. Older drafts can be saved as a text file.

## Update Preferences

ClawScribe checks **stable releases** by default. Enable **Include prereleases**
under **Settings > General > Updates** or **About** to receive preview builds
as well. The choice persists across restarts and applies to all checks. Turn on
**Check for updates automatically** to check at startup and periodically while
ClawScribe stays open in the tray. You choose when to install.

Preview builds may contain unfinished features. Turning previews off waits for a
newer stable version and never downgrades your installation. Update downloads
retain Tauri signature verification and remain subject to Windows security policy.
ClawScribe saves pending meeting title and summary edits before installation and
keeps the app open if a save fails.

## What changed in the 0.64.0 preview

- Start recording without downloading a speech model. Stop saves the meeting
  before reporting success, and interrupted saves can be recovered.
- Find a passage from the sidebar or Archive, jump to its audio, and bookmark
  moments for review. Sharing those notes with a summary is optional.
- Confirm who attended, remember a recurring meeting’s summary template, and
  edit follow-up tasks before exporting them to Planner or Microsoft To Do.
- Review meetings that need attention and see how much space saved audio uses
  before choosing which copies to remove.

See the [0.64.0 preview notes](docs/releases/0.64.0.md) for the updater behavior,
automated coverage, and pending device tests.

## What changed in the 0.63.0 preview

- A device reconnect no longer blocks the app, and a failed Stop is shown.
- A recording does not start on a nearly full disk, and a notice appears when
  space runs low.
- Home shows real levels for the microphone and the meeting audio.
- Recovery also finds interrupted recordings in the recording folders.
- Stored keys stay in the core and are only sent to the address they were saved
  for. Files and folders are used when you chose them in a dialog.
- Engine, cloud and language settings are stored by the core.

See the [preview notes](docs/releases/0.63.0.md) for limits and pending
real-device checks.

## What changed in the 0.62.0 preview

- A manually started recording uses the title and the devices you chose.
- Starting a recording stops a running retranscription, import, speaker
  detection or PC test instead of failing.
- Stop in the sidebar works from every page, and a stop no longer pulls you to
  the new meeting unless Home is open.
- Quit during a recording saves the meeting first. A capture failure is shown.
- Notices follow the theme and appear at the bottom right.
- Settings → Transcription has a language setting. Settings → Appearance has
  more fonts, a separate transcript and summary font, and a text size setting.
- Translated summaries keep English working terms.

See the [preview notes](docs/releases/0.62.0.md) for limits and pending
real-device checks.

## What changed in the 0.61.1 preview

- With live transcription, audio capture starts while the speech model loads. The
  first seconds after pressing Record are no longer lost.
- Quiet speech is raised to a normal level before Parakeet transcribes it, in live
  recording, retranscription and import.

See the [preview notes](docs/releases/0.61.1.md) for a known issue and pending
real-device checks.

## What changed in the 0.61.0 preview

- Settings has one page per topic. Add-ons is split into Microsoft 365, Calendars,
  Confluence and Assistant access. The Beta switches are on the Transcription page.
- **Settings → Privacy** lists what stays on this PC and what leaves it, based on
  your current settings.
- Call detection is under **Settings → Recording**, with a live microphone level.
  It recognises Teams calls whose controls read Leave and Mute mic.
- Parakeet models are listed in a table that keeps the measured speed per model.
- Summary settings save as you change them. A different provider is used only
  after you choose **Use**.
- The sidebar shows the next calendar event and groups meetings by date.
- Calendar files and subscriptions can be up to 16 MiB.

See the [preview notes](docs/releases/0.61.0.md) for pending real-device checks.

## What changed in the 0.5.60 preview

- Import an iCalendar file or add a read-only subscription in **Settings → Add-ons**.
  Choose an upcoming event to prefill the next recording title and review invitees.
- Enable call detection for Teams, Zoom, Webex or Google Meet. Automatic stop can
  finish the recording that the detector started when the call ends.
- See language estimates for the microphone and system audio when source
  attribution is enabled. **Test this PC** times an installed speech model.
- Connect an MCP client to the local meeting library with read-only access.
  The Live assistance panel appears when live transcript sharing is enabled.

See the [preview notes](docs/releases/0.5.60.md) for updater settings and pending
real-device checks.

## What changed in 0.5.59 preview

- Speaker detection uses a different voice model and grouping threshold. On a test
  set of four-person meetings it found 3 to 9 speakers where the previous default
  found 10 to 114. The model (about 40 MB) downloads on the next detection.
- Detect speakers no longer needs the Beta source attribution setting, and speaker
  labels show whenever a meeting has them.
- Relabeling is two actions: "Who said this?" on a transcript row, and renaming or
  merging a speaker from the timeline.
- Summary citations point at a passage of at most 10 seconds, not 45.
- Action items are a list with owner and due date that Planner and To Do export can
  read. Unanswered questions get their own Open questions section.
- The playback strip and the active transcript row follow the accent colour.
- Check these on an installed build with real meeting audio. These checks remain
  pending before stable promotion.

See the [preview notes](docs/releases/0.5.59.md) and the [changelog](CHANGELOG.md).

## What changed in 0.5.58

- Meeting details has the title in a header across the page, a playback strip that
  shows the transcript segments, and one-row toolbars with menus for the less
  common actions.
- The summary renders on the pane surface with section labels on their own line.
  Chat opens from a button under the summary and docks beside it in wide windows.
- The Meetings list, its empty states and Meeting memory are simpler to scan.
- Add-ons has one Microsoft 365 card and a Change folder dialog for OneDrive.
  Microsoft product icons use their own colours.
- Claude Code is an optional summary and meeting chat provider that uses your own
  installed and signed-in Claude Code.
- Error messages are app notifications, not browser alerts. Error text in the dark
  theme and keyboard focus rings are easier to see.
- These changes were published to stable without installed acceptance. Installed
  checks in light and dark themes and recording acceptance remain open.

See the [release notes](docs/releases/0.5.58.md), the
[previous transcription preview](docs/releases/0.5.57.md), and the [changelog](CHANGELOG.md).

## Notebook Performance And Product Status

Windows is the primary release target; the supported runtime is the Tauri
desktop app. macOS/Linux source paths are not a claim of equivalent release
validation. Nemotron and cloud transcription remain beta.

On Windows, Whisper's adaptive policy caps inference at four threads when the
detected memory value is at most 8 GiB, and at eight otherwise. Where possible
it leaves one logical thread outside that limit. This is a Whisper policy,
not a process-wide CPU or RAM cap, and does not automatically govern Parakeet,
Nemotron, or a summary provider. A valid `MEMORY_GB` environment override is
retained for diagnostics.

Performance depends on the selected model, audio, available memory, GPU/driver,
and the concurrent meeting application. The i5-1235U/8 GB target has not been
certified by the source tests. Do not interpret queued transcription as lost
audio or claim it is live when processing is still catching up.

## Privacy And Credentials

Recording and transcription remain local unless you explicitly enable cloud
transcription or configure external summary/export processing. Data sent to an
external provider is subject to that provider's configuration and policies.

Provider keys and Microsoft refresh tokens use protected OS storage,
with current-user DPAPI fallbacks on Windows. Codex sign-in and Confluence use
the keyring without file fallback. Meeting files and the full database are not
encrypted by this protection; see the [privacy policy](PRIVACY_POLICY.md).

Never commit credentials, `.env` files, private logs, recordings, databases,
generated installers, or personal workspace paths. Use explicit placeholders
in examples and keep diagnostics redacted. Contributor guidance is in
[CLAUDE.md](CLAUDE.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## Development

Use Node.js 24, pnpm 10, and the native prerequisites in
[Building ClawScribe](docs/BUILDING.md). From `frontend/`:

```powershell
pnpm install --frozen-lockfile
pnpm run tauri:dev
```

For the web UI only, use `pnpm run dev`. It is not a replacement for native
recording tests. Run frontend checks with `pnpm typecheck` and `pnpm test`.
Windows packaging and required sidecar staging are documented in
[Windows releases](docs/windows-release.md).

All CI checks and Windows installers run on the designated local self-hosted
machine, with no GitHub-hosted fallback. Fork PRs require maintainer review and
validation from a trusted repository branch. Test installers and build caches
stay local; GitHub distribution uses explicitly requested draft or published
releases.

```text
frontend/src/             React UI, hooks, services, and routes
frontend/src-tauri/src/   Rust/Tauri audio, transcription, summary, and exports
llama-helper/             Local summary sidecar
scripts/                  Repository utilities
docs/                     Product and build documentation
```

The historical Python/FastAPI service has been removed; no standalone service,
Docker component, or manually started whisper-server is needed for local use.

## Documentation And Support

In **Settings → Diagnostics**, choose **Save report** to save a JSON file for
troubleshooting. It contains app and platform information, device counts,
engine status, and failure categories. You choose where to save it and whether
to share it. See the [diagnostics report contents](docs/architecture.md#diagnostics-report).

- [Documentation index](docs/README.md)
- [Building from source](docs/BUILDING.md)
- [Windows release and verification](docs/windows-release.md)
- [GPU acceleration](docs/GPU_ACCELERATION.md)
- [Frontend developer guide](frontend/README.md)

[Support development](https://buymeacoffee.com/ch3573r).

## License

ClawScribe is free to use, modify, and redistribute under the
[MIT License](LICENSE.md). Upstream Meetily code is copyright Zackriya Solutions
and contributors. ClawScribe changes are copyright OpenClaw contributors unless
otherwise noted.
