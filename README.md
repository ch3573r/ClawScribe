# ClawScribe

![ClawScribe README hero](docs/brand/clawscribe-readme-hero.png)

ClawScribe is a local-first Windows desktop companion for meetings, calls,
interviews, and recorded audio. It captures microphone and system audio from
your own session, transcribes speech locally, and turns transcripts into
reviewable meeting notes and action items. No meeting bot is required.

Source version: **0.5.40 Preview**. The [0.5.40 Windows prerelease](https://github.com/ch3573r/ClawScribe/releases/tag/v0.5.40)
adds audio-only recording, transcript corrections, summary source links, and
editable summary templates. Enable **Include prereleases** in 0.5.38 or install
manually; the stable channel remains on 0.5.38.
Read the [release validation limits](docs/releases/0.5.40.md): real-device capture,
install/upgrade acceptance, and sustained notebook performance remain unconfirmed.

ClawScribe is based on Meetily Community Edition **0.4.0**. Attribution and
license details are in [UPSTREAM.md](UPSTREAM.md), [NOTICE.md](NOTICE.md),
and [LICENSE.md](LICENSE.md).

## Install And Start A Meeting

Use the NSIS setup installer from the selected GitHub Release; an MSI is also
provided for deployment scenarios. Read that release's validation and signing
notes before installing. Draft builds are unpublished, and prereleases do not
advance the stable automatic-update channel.

The Windows GPU build needs the Vulkan loader (`vulkan-1.dll`) at startup, even
when selecting a non-Vulkan model. Use a supported GPU driver or the official
Vulkan runtime. The CI runtime fix does not install a driver or runtime on your
PC. See [Windows runtime prerequisites](docs/windows-release.md#windows-runtime-prerequisite)
for diagnosis; never download individual DLLs from third-party sites.

Choose a microphone and the output device used by Teams, Webex, or the other
meeting application. Make a short test recording and verify that both your
voice and the remote participants are audible in playback. Start recording,
review the live transcript, then stop and allow queued transcription to finish
before generating notes. Automatic meeting detection is a separate Teams
feature; general system-audio capture does not require it.

Review names, numbers, decisions, owners, and deadlines before sharing notes or
exporting tasks. Obtain the recording permissions required for your meeting.

## Capture And Transcription

- Microphone and system-audio capture from the local Windows session.
- Live transcription, audio/video import, and retranscription with a different
  model or language selection.
- **Record now, transcribe later:** choose audio-only mode on Home or in
  Recording settings. It always saves audio, skips the speech model and live
  transcription, and leaves a **Transcribe** action on the saved meeting.
- Correct transcript passages or preview literal find-and-replace across the
  entire meeting. Original recognition and segment timing are retained; undo
  restores previous corrections. Regenerate notes after correcting a transcript.
- Import support for MP4, M4A, WAV, MP3, FLAC, OGG, AAC, MKV, WebM, and WMA.
- Disk-backed live recognition queue and interrupted-recording recovery paths.
- Timestamp playback controls, speaker-label editing, and a **Jump to live
  transcript** control when reading earlier text during a recording.

| Engine | Role |
| --- | --- |
| Parakeet | Default local fast path, with stock v3 int8, SmoothQuant int8, and v2 int8 options. Supported GPU builds can use DirectML. |
| Whisper | Local whisper.cpp/whisper-rs engine with selectable models and Vulkan support in the Windows GPU build. |
| Nemotron | Beta multilingual Nemotron 3.5 ASR path. fp16 is CPU-capable; int8 is intended for DirectML-capable builds. |
| Hosted Whisper | Opt-in beta cloud retranscription through OpenAI-compatible file transcription. |
| MAI-Transcribe | Opt-in beta Azure Speech Fast Transcription, using credentials separate from Microsoft Graph sign-in. |

Models are downloaded in the app. Downloads are checked against expected file
sizes to reject incomplete downloads and LFS pointer files.

Whisper preserves recognized repetitions and short answers instead of removing
them through phrase matching. Nemotron's **Auto** language follows the system
locale; select the spoken language explicitly when it differs. Parakeet's Auto
selection does not label an unknown language as English.

Cloud transcription requires explicit opt-in. Hosted Whisper can provide word
timestamps; MAI has sentence-level timing and may use approximate local VAD-row
alignment, not fabricated word timestamps. The implemented OpenAI-hosted upload
limit is 25 MB; the implemented MAI limit is 300 MB. MAI uploads use WAV, MP3, or
FLAC, with other formats converted locally to 16 kHz mono WAV. Rejected cloud
requests can fall back to local transcription with a notification explaining
why. See [hosted transcription verification](docs/hosted-transcription-smoke.md).

## Meeting Notes And Exports

Generate template-based meeting summaries from the transcript and optional
context, regenerate notes, and chat about the selected meeting. Configurable
providers include Built-in AI, Ollama, OpenAI, OpenAI-compatible endpoints,
OpenRouter, Anthropic/Claude, Groq, OpenClaw managed processing, and the advanced
bundled Codex app-server path.

In **Settings → Summary**, create or edit templates and choose a persistent
default. Each meeting can use a different template when generating notes.
Summary find-and-replace preserves formatting and source links; review the
changes, then select Save. Use the editor's undo command to revert changes.

Saving a title leaves unchanged notes alone. Failed saves retain drafts, and
edits made during a save remain available to save again. Switching meetings
discards late loading results from the previous meeting. Automatic summaries
wait for saved notes to load and use the provider you have configured.

New summaries request links to supporting transcript passages. Open a source
link to inspect the text, reveal it in the transcript, or play its saved audio.
Changed or replaced passages are flagged and require regenerated references.
Citation coverage depends on the model; inspect the passage to check the claim.

Microsoft sign-in supports calendar context and exports. Teams detection can
prompt or auto-start recording according to your setting. Invited attendees
can be included as a reviewable attendance checklist; an invitation does not
prove attendance.

- **OneNote:** choose or create a notebook and export notes/transcripts into a
  fresh dated section, avoiding large-library section-listing problems.
- **Planner and Microsoft To Do:** review and edit action items before exporting;
  saved export history protects supported re-export paths against duplicates.
- **Confluence:** copy rich text into a browser draft, or publish through a
  configured self-hosted Server/Data Center REST endpoint.
- **OpenClaw:** optional handoff of completed Meetily-compatible recording folders.

Microsoft exports stop if duplicate-protection history cannot be read or saved.
An interrupted export with an unknown outcome is not automatically repeated;
check the destination before creating another page or task. See
[Microsoft export recovery](docs/integrations/microsoft-graph.md#export-history-and-recovery).

OpenClaw is optional. A standalone installation does not require an OpenClaw
endpoint, token, or separate server.

## Update Preferences

ClawScribe checks **stable releases** by default. Enable **Include prereleases**
under **Settings > Preferences > Updates** or **About** to receive preview builds
as well. The choice persists across restarts and applies to manual and startup
checks. You still choose when to install; checking at launch is a separate option.

Preview builds may contain unfinished features. Turning previews off waits for a
newer stable version and never downgrades your installation. Update downloads
retain Tauri signature verification and remain subject to Windows security policy.

## What Changed In 0.5.40 Preview

- Preserve legitimate Whisper words and repetitions; fix model-switch deadlocks
  and move speech-model initialization off async workers.
- Reduce batch audio copies and cancel hosted transcription requests without
  starting local fallback. Keep Nemotron Auto behavior consistent across paths.
- Prevent stale meeting/recording responses and incomplete transcript animations.
  Preserve edits during saves and avoid rewriting notes for title-only changes.
- Share summary provider configuration, reject explicitly truncated answers,
  serialize local sidecar exchanges, and scan stop markers incrementally.
- Index transcript paging. A synthetic 150,000-row count/page query improved from
  12.813 ms to 0.342 ms; this measures database paging only.
- Persist Microsoft export attempts and results before proceeding. Stop blind
  retries when a submission has an unknown outcome.

The transcript corrections, summary source links, editable templates, and deferred
transcription introduced in 0.5.39 remain available. See the
[0.5.40 preview release notes](docs/releases/0.5.40.md),
[meeting-quality guide](docs/meeting-quality.md), and [changelog](CHANGELOG.md)
for validation results and remaining device/model acceptance.

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

Microsoft refresh tokens use the platform credential store when possible. On
Windows, the file fallback is DPAPI-encrypted for the current user. A legacy
plaintext file is removed only after encrypted migration succeeds; if migration
fails, that existing file may remain. Non-Windows platforms do not create a new
plaintext fallback. Access tokens are not persisted by this token
store. This protects that credential path; it does **not** mean all recordings,
transcripts, or the meeting database are encrypted at rest.

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

Windows installers are built on the designated local self-hosted machine, with
no GitHub-hosted fallback. Test installers and build caches stay on that machine;
GitHub distribution uses explicitly requested draft or published releases.

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
