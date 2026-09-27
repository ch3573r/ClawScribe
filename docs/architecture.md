# ClawScribe Architecture

ClawScribe is a local-first desktop meeting recorder built with Tauri 2, Rust,
Next.js, and local model runtimes. The supported product runtime is the Tauri
desktop app under `frontend/`. The legacy Python/FastAPI backend is no longer
part of the repository.

## Runtime Shape

```text
Next.js / React UI
        |
        | Tauri commands and events
        v
Rust app core
  - recording and import pipeline
  - local transcription engines
  - summary providers
  - Microsoft / Atlassian exports
  - updater, tray, settings, and credential storage
        |
        v
Local files, SQLite/settings, OS credential store, optional external providers
```

The app records from the local user session. It does not join meetings as a bot.
Meeting data is stored locally in a Meetily-compatible folder layout so older
recordings and migration paths continue to work.

## Main Modules

- `frontend/src/`: React UI, settings, meeting detail views, export dialogs,
  update UI, Teams/calendar panels, and client-side state.
- `frontend/src-tauri/src/audio/`: recording, import, audio conversion, device
  handling, and live transcription orchestration.
- `frontend/src-tauri/src/parakeet_engine/`: Parakeet ONNX model catalog,
  downloads, validation, and inference.
- `frontend/src-tauri/src/nemotron_engine/`: Nemotron 3.5 ASR streaming ONNX
  model catalog, feature extraction, DirectML/CPU loading, and RNN-T decoding.
- `frontend/src-tauri/src/summary/`: summary generation providers, including
  built-in/local, API-based, OpenClaw, and Codex app-server paths.
- `frontend/src-tauri/src/exports/`: Microsoft Graph auth, calendar lookup,
  OneNote export, Planner/To Do task export, idempotency, and testable Graph
  transport.
- `frontend/src-tauri/src/exports/confluence.rs`: Confluence direct publish and
  credential handling.
- `frontend/src-tauri/src/teams_detection.rs`: local Windows Teams meeting
  detection using process/window evidence.
- `llama-helper/`: local summary sidecar helper used by built-in/local paths.

## Data Boundaries

- Transcription is local unless a user explicitly selects a cloud transcription
  provider.
- Hosted Whisper uses OpenAI-compatible file transcription. The official
  OpenAI endpoint currently limits uploads to 25 MB; larger uploads are reported
  as a size-limit fallback and transcribed locally.
- MAI-Transcribe uses Azure Speech Fast Transcription with its own Cognitive
  Services key or Azure Speech credential. Microsoft Graph sign-in/export
  scopes are not reused for Azure Speech.
- MAI sentence-level timing is never promoted to word timestamps. If Azure
  returns a combined-only or single-phrase transcript, ClawScribe can remap the
  cloud text onto the local VAD speech grid for readable rows, but those row
  timings are approximate and speaker diarization stays on the conservative
  path.
- Cloud transcription calls upload the whole recording file once. They are not
  split into per-VAD-segment provider calls.
- Summary generation can be local or external depending on the configured
  provider.
- Microsoft Graph is used only after Microsoft sign-in and only for calendar,
  OneNote, Planner, and To Do workflows.
- Confluence direct publish uses the configured server URL and PAT. Browser
  draft export remains available when SSO, proxy, or tenant policy blocks REST.
- OpenClaw handoff is optional and sends completed recording artifacts only to
  the configured operator endpoint.

## Recording And Inference Lifetimes

`audio/inference.rs` serializes recording, batch, diarization and model-changing jobs and holds a
separate native-call permit inside the blocking task. Cancelling the async caller
cannot free a model that native code still uses. `audio/batch_audio.rs` normalizes
imports/retranscription to temporary PCM on disk, uses one continuous VAD state,
and reads one bounded speech segment for inference. Preparation needs temporary
disk capacity; it does not retain a full decoded meeting in RAM.

Whisper, Parakeet, and Nemotron model construction also runs behind the native
permit on a blocking worker. Model switches release name-read guards before
unloading the previous model. Nemotron resolves language at the engine boundary
so live, import, and retranscription use the same system-locale Auto policy.
Unknown source language remains unset instead of being inferred from the engine
name. Whisper's text normalization collapses whitespace and preserves words.

`RecordingStateContext` subscribes before reading the initial recording snapshot,
restores polling for an active session after a reload, and permits one poll at a
time. Lifecycle events invalidate older polls. Cleanup also removes subscriptions
whose asynchronous registration finishes after the provider unmounts.

`audio/transcription/queue.rs` serves both live recognition and the retained
recording spool. Bounded producer buffers feed independent disk workers so disk
latency does not block the mixer. `audio/audio_spool.rs` encodes final audio from
the available PCM spool in one pass with a duration-scaled deadline and recovers
it into AAC when the encoder is available, with WAV as the fallback if the encoder is missing or fails. Both paths accept a readable temporary tail at the next sequence
position. Torn temporary writes record a gap without preventing spool cleanup;
unreadable published chunks retain their originals. Playback uses scoped asset-protocol byte ranges and an HTML audio
element. Periodic transcript snapshots continue during silence; the library-save
command reads the backend snapshot when saving a finished recording, falling
back to the UI transcript with a warning if that snapshot is unavailable. New
capture creates no checkpoint folder or periodic AAC encodes. The legacy
checkpoint reader and merge fallback remain available for older meetings. The
recovery dialog probes raw chunks (including a readable temporary tail), recovered
files, and legacy checkpoints without encoding the spool. Recovery attempts audio
before checking for recoverable content, so zero transcript rows do not block
audio-only meetings; unsuccessful attempts retain the recovery entry and originals. Final transcript, audio and
metadata writes are independent. Stopped duration and completion time are
persisted before encoding, using the existing error status until finalization
succeeds; capture gaps and artifact-save failures have
separate persistent flags. File warnings reflect final write results. Spool cleanup
requires published audio, complete raw encoding, and a saved outcome; independent
transcript/metadata failures do not retain redundant audio. Retranscription updates repaired flags in the transcript
transaction and mirrors the outcome afterwards; capture gaps remain informational.
Its audio resolver checks retained raw chunks and the saved outcome on a blocking
worker, even when reusing recovered audio, so older partial recoveries also set
the permanent capture-gap flag. Unreadable saved status is treated as unknown;
retained capture and database warnings still preserve gaps, and committed
retranscription rewrites the status file. After library save or committed retranscription,
fully readable recovered spools are released only after audio validation, a
presentation-duration check against all retained samples, and saved outcome status.
The check uses the same preferred recovery file as playback/retranscription; a
longer alternate file cannot authorize cleanup. Publishing a recovered format
removes the superseded recovery format only after the new file is safely written. Frontend release requests require a canonical registered meeting
folder; incomplete chunks retain their originals.
Shared stop ownership suppresses duplicate completion
events, and a SQLite write reservation protects the folder lookup and insert
against concurrent saves.
`audio/outcome.rs` persists recording failures before the completion event;
meeting/transcript/outcome database writes share a transaction. New schema is
added through a migration; shipped migrations remain unchanged.

`credentials.rs` supplies protected, provider-scoped references for legacy
settings columns. Optional providers can fail independently without preventing
local recording. `summary/context_budget.rs` bounds hierarchical reduction, and
`summary/chat_context.rs` selects question-relevant evidence from across the
meeting instead of silently discarding its middle.

## Recording Modes And Meeting Review

`audio/recording_mode.rs` persists the next-session mode through the Tauri store.
Recording startup snapshots it into the manager and meeting metadata. Audio-only
mode always enables the recording saver, omits VAD/model validation and the live
recognition worker, and skips automatic summary/OpenClaw handoff. Capture and the
retained audio spool use the existing paths. Later transcription uses the existing
reviewed retranscription flow and its configured local/cloud preference.

`database/transcript_edits.rs` applies corrections with optimistic text checks in
one transaction. The first recognized text is retained in `original_transcript`;
a batch journal supports undo across restarts. Segment timing stays unchanged,
and edited word alignment is cleared until an undo restores it or transcription
rebuilds it. Active summary generation and native recording/processing jobs block
corrections. Retranscription archives old correction batches instead of applying
undo to replacement segments. SQLite is authoritative. A durable pending marker
and visible retry preserve corrections if the atomic `transcripts.json` mirror
cannot be written. Existing migrations are immutable; the correction schema is
added by a new migration.

`summary/sources.rs` prepares summaries from the complete saved transcript and
annotates passages with stable content-derived source links. Reduction prompts
retain links alongside their facts. Completed results store the cited source
identities and fingerprints. Resolving a link is scoped to the meeting and checks
the current text, timing and speaker. Stale or missing segments cannot silently
seek another passage. The UI loads the cited page without skipping sequential
pagination. Source identity checks establish which passage was cited, not whether
the passage entails the model's claim. Legacy summaries remain readable without
references; regeneration requests sources.

Generation groups consecutive rows into passages of roughly 45 seconds or 800
characters and labels each with a short source tag. Before persistence, known
tags become the existing source links using server-owned timestamps; unknown
tags are removed. Passage references retain all transcript IDs and a combined
fingerprint. Resolution still supports older single-row references and marks a
passage stale if any included row changes.

Transcript pages use indexed `(meeting_id, audio_start_time, id)` ordering and
read the count and rows in one database snapshot. The UI fetches metadata and the
first page concurrently; navigation and refetches invalidate earlier responses,
including source-page requests. A synchronous loading guard coalesces duplicate
page requests. Transcript animation tracks the last segment's identity and text,
so refreshing the surrounding array cannot leave an utterance partly hidden.

User template edits are validated and atomically stored as personal JSON files
in the existing template directory. Overrides take precedence over bundled
versions. The default template uses `summary_preferences.json`; a missing saved
default falls back to `standard_meeting`. Shared template events refresh Settings
and meeting selectors. Structured providers return a template-shaped
`notes_markdown` alongside their existing export fields; old stored outputs use
the standard renderer. Manual summary saves retain source identities and
invalidate the English generation cache. Summary replacement edits inline text,
preserves block IDs/formatting/link destinations, checks its preview snapshot,
and uses the editor's normal undo and save flow.

Meeting saves write only dirty content, preserve later edits while a write is in
flight, and report title failures without claiming success. Summary loads are
scoped to the current meeting. Automatic generation waits for the saved summary
lookup and requires a configured provider; it never installs a default cloud
configuration. API summary response readers enforce an 8 MiB limit, reject
reported output truncation, and keep cancellation active while reading the body.
Chat and reviewed task polishing share provider configuration resolution.

Summary HTTP providers make at most three attempts for connection failures and
HTTP 408, 429, 500, 502, 503, 504 or 529. Backoff is cancellable, honors
Retry-After up to 60 seconds, and never restarts a full response timeout.
Provider error messages are bounded and redact credentials and input echoes.

Codex summary and chat threads use an ephemeral scratch working directory,
read-only sandbox, no approval escalation, and disabled shell tools. Every turn
reapplies the sandbox and approval policy; unexpected server approval requests
are declined. These overrides also cover legacy home-mode configurations.

Summary and chat budgets resolve provider/model context and output tokens together.
GPT-4o uses 128,000 context tokens, GPT-4.1 uses 1,047,576, Llama 3.1/3.3 uses
131,072, and Claude uses a conservative 200,000; unknown cloud models use 32,768.
These families also apply to namespaced OpenRouter IDs. See the
[OpenAI model specifications](https://developers.openai.com/api/docs/models/gpt-4.1)
and [Groq Llama specifications](https://console.groq.com/docs/model/llama-3.3-70b-versatile).
Operator endpoints use their configured context or 8,192. Ollama metadata and
the Built-in AI registry are capped at 16,384, with an 8,192 Ollama fallback;
the local helper receives that same context. Codex retains 32,768.
After reserving output tokens and 256 tokens, planning converts the remainder
to three UTF-8 bytes per token and subtracts prompt overhead in bytes. Extraction
pieces use their short prompt and at most 1,024 output tokens; the final report
reserves its full template. Structured OpenAI requests carry the schema once,
with an inline schema only for strict-JSON fallback.

Summary status polling has one shared timer owner across meetings. Starting or
finishing one poll keeps other meetings' polls alive; stopped or replaced requests
cannot publish late responses. Missing jobs and completed jobs without saved
content surface retry errors. Status checks do not overlap, and the polling limit
reports an unconfirmed status without claiming that provider work was cancelled.

## Microsoft Export Persistence

`exports/commands.rs` serializes export owners so separate dialogs cannot race
on the same local history. `exports/ledger.rs` validates the meeting/schema and
atomically checkpoints pending attempts before remote page/task creation, then
checkpoints each result. File writes run on blocking workers. A history read or
write failure stops export instead of disabling duplicate protection.

An interrupted pending attempt becomes `unknown_after_submit` on reload and
cannot be automatically replayed. A failed checkpoint after submission also
requires destination review. This prevents blind retries; it cannot establish
whether Microsoft accepted a request whose response was lost. OneNote cleanup
retains a new section when a page is confirmed or may have been created. See
[Microsoft export recovery](integrations/microsoft-graph.md#export-history-and-recovery).

## Compatibility Boundaries

Some internal names, folders, and environment variables still use Meetily names.
That is intentional compatibility debt for existing recordings and deployments,
not product branding.

Windows is the primary release target. Linux/macOS build paths may exist because
of the upstream Tauri app and model libraries, but release validation currently
focuses on Windows installers and GPU paths.

Microsoft sign-in keeps partially granted sessions connected. Missing permissions
and unavailable exports are shown in Settings; only “Request missing permissions”
opens a consent flow. A declined request retains the session. Without offline
access, sign-in lasts only for the current app run. Export commands check their
required permissions before Graph calls. If a permission is missing, one token
refresh checks for newly approved scopes before reporting it unavailable. Missing
permissions trigger at most one forced refresh per scope per hour; signing in or
requesting missing permissions resets that limit. Calendar views skip calendar
requests when connection status reports Calendar lookup unavailable.

Credential-bearing integration endpoints require HTTPS, loopback HTTP, or an explicit
private-network HTTP opt-in. Legacy private HTTP settings migrate on load. HTTP
requests validate and pin DNS results and disable redirects and proxies. Public
HTTP destinations are rejected on save/send even with the opt-in. Rejected saved
settings remain editable and show an attention message. Tailscale's shared address
range and MagicDNS names are accepted as local, with the same DNS pinning checks.
Settings responses carry `destination_problem` (camelCase for Confluence); this
diagnostic is recomputed on load and is not saved as configuration.
Confluence credentials include their saved origin and cannot be reused against a
different origin; older unbound PATs must be saved again before use.
Confluence status includes its saved base URL and HTTP opt-in, so settings can
restore both without reaching the server or overwriting an edited destination.

Microsoft sign-in has one active flow. Cancel stops its loopback listener and
prevents late completion from restoring a session. Listener polling and bounded
connection reads release the port after cancellation or timeout.

Microsoft refresh results are committed under the session lock only when the
sign-in generation is unchanged and the connection is still active. Sign-out
invalidates that generation and clears memory and persisted credentials under
the same lock, so an in-flight refresh cannot restore a signed-out account.

Microsoft authentication HTTP calls time out after 30 seconds. OpenClaw handoffs
time out after 60 seconds and retain at most 64 KiB of response text in submission
markers.

A Microsoft token fallback save removes the older keychain entry so the next
load cannot prefer stale credentials over the encrypted fallback.

Microsoft refreshes persist only changes to refresh tokens, account metadata,
tenant, or granted scopes; access-token-only changes stay in memory.
