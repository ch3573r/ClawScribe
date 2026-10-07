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

The backend contains no analytics client or analytics commands. Frontend analytics
calls remain inert compatibility stubs; recording stop does not read provider
credentials or copy transcripts for telemetry.
Recording, summary status and notification logs omit meeting names and
notification contents. Summary diagnostics use the meeting ID instead.

SQLite connections explicitly enable WAL, normal synchronous mode, foreign keys
and a five-second busy timeout. Legacy database transfers checkpoint first and
use a consistent SQLite snapshot so committed WAL data is retained.
Before pending migrations, startup writes a consistent database snapshot under
the app-data backups folder and retains the newest two. Fresh databases with no
user tables do not create an empty snapshot. After successful migrations, each
startup removes snapshots older than 14 days. Deleted meeting data can remain
longer while the app is closed or cleanup fails. A backup failure warns without
blocking startup; already-applied migrations do not create another copy.

Background Teams detection returns matching candidates only. Unmatched browser
window titles are returned only when the settings diagnostics panel requests them.

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

On Windows, `audio/com_anchor.rs` initializes CPAL’s device enumerator on a
process-lifetime MTA thread before device access. This keeps its COM apartment
alive when microphone-monitor or reconnect worker threads exit.

`audio/inference.rs` serializes recording, batch, diarization and model-changing jobs and holds a
separate native-call permit inside the blocking task. Cancelling the async caller
cannot free a model that native code still uses. `audio/batch_audio.rs` normalizes
imports/retranscription to temporary PCM on disk, uses one continuous VAD state,
and reads one bounded speech segment for inference. Preparation needs temporary
disk capacity; it does not retain a full decoded meeting in RAM.

Imports own their newly created folder until the meeting database transaction
commits. Failed or cancelled imports remove that copy; the source file is kept.
Cancellation reports the same status during decoding, cloud and local inference.
Batch inference retries each failed segment once. Imports retain successful text
and mark persistent failures as incomplete with a segment-count warning.
Retranscription replaces the transcript only when every segment succeeds.

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
recording spool. Startup removes abandoned UUID-named temporary transcription
queues without following links; queues owned by a running session are retained. Bounded producer buffers feed independent disk workers so disk
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

Retranscription warns when it will replace corrections. Restore previous
transcript swaps the current rows with the latest archived revision, preserving
every original ID and field. It archives the replaced version, retires old edit
batches, and queues the file mirror in one transaction. Active summaries block
restoration; regenerated notes are needed to refresh source links.

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
Reported `insufficient_quota` and Retry-After values above 60 seconds are not retried.
Provider error messages are bounded and redact credentials and input echoes.

Built-in summary models use pinned Hugging Face revisions with exact byte sizes
and SHA-256 hashes. Transfers resume from `.partial` files and verify integrity
before promotion to the final filename. Readiness caches successful verification
for an unchanged file; a complete verified model needs no network request.

Speech and speaker-detection downloads use the immutable revisions, exact sizes
and SHA-256 hashes in `speech-model-pins.json`. The shared transfer verifies before
publishing a file. Existing files are hashed on a blocking worker; verification
receipts cache unchanged size, modification time and expected hash across starts.
Changed or mismatched files require verification or re-download before loading.
The default Parakeet v3 Hugging Face mirror is byte-identical to the previous
four-file download. SmoothQuant retains the original export filenames from the
last pinned revision before upstream reorganized those files.

Codex output documents and their processing log live in the meeting folder, or
app data `meeting-outputs/<id>` when no recording folder is set.
Temporary prompt/transcript run files are removed on success, failure or
cancellation. Meeting deletion removes legacy run files, and startup retries
orphan cleanup without blocking local recording.

Meeting deletion defaults to removing recording files after the database commit.
File deletion requires an unshared, marked ClawScribe folder within configured
or supported default recording locations, or app-data restored-recordings, with
no links or junctions in its path.
Users can retain recording files. Export ledgers and temporary Codex runs are
removed either way; any retained folder is reported without undoing database deletion.

Codex meeting turns send `outputSchema`; raw prompts do not. Summary and chat
threads use an ephemeral scratch working directory,
read-only sandbox, no approval escalation, and disabled shell tools. Every turn
reapplies the sandbox and approval policy; unexpected server approval requests
are declined. These overrides also cover legacy home-mode configurations.

Generated headings replace only untouched recording titles or “New Meeting”.
Regeneration preserves manual names and ignores template title placeholders.

Saving a generated summary retries three times with short delays. A persistent
save failure marks the run failed with a regeneration message.

Summary cancellation follows the job token rather than provider error wording.
Startup marks unfinished summary rows failed with an interruption message and
restores the previous saved result, allowing transcript corrections again.

Starting a recording cancels Built-in AI summary jobs before stopping the helper,
with a recording-specific message and the previous summary retained. Other
providers continue independently.
Import, retranscription and speaker detection use the same cancellation path
with an operation-specific explanation before reclaiming the local helper.

The local helper serializes model switches and requests under one exchange lock.
Cancelling a queued request leaves the current generation intact; cancelling the
owner stops the helper before another request can acquire the lock.

Release builds resolve the local summary helper only by exact packaged filenames
beside the executable or in its resource directory. Environment overrides and
workspace lookup are available only in debug builds.

### Summary context budgets

`summary/context_budget.rs` resolves summary and chat budgets together. These
are ClawScribe's request-planning limits, not guarantees about a provider account:

| Provider or model family | Context tokens |
| --- | --- |
| GPT-4o; GPT-5/6 and o1/o3/o4 reasoning names | 128,000 |
| GPT-4.1 | 1,047,576 |
| Llama 3.1/3.3 | 131,072 |
| Claude | 200,000 |
| Unknown cloud model | 32,768 |
| Custom/OpenAI-compatible/OpenClaw endpoint | Configured context; otherwise 128,000 for reasoning names, 8,192 for other names |
| Ollama | Model metadata, capped at 16,384; 8,192 fallback |
| Built-in AI | Model registry, capped at 16,384 |
| Codex | 32,768 |

Family matching also applies to namespaced OpenRouter IDs. GPT-5/6 and o1/o3/o4
names, and the Claude provider, default to 16,000 output tokens and up to 8,000
extraction tokens. Other names default to 4,096 output tokens for Built-in AI
and Codex, or 2,048 elsewhere. Default output is capped at `max(context / 4, 1024)`;
thus it never exceeds a quarter of context when context is at least 4,096.
Explicit output settings override the default. Codex reasoning names still use
the Codex context budget, so their default output is capped at 8,192.
The local helper receives the resolved local context.
After reserving output tokens and 256 tokens, planning converts the remainder
to three UTF-8 bytes per token and subtracts prompt overhead in bytes. Extraction
pieces use their short prompt and at most 1,024 output tokens for non-reasoning
models, or 8,000 for reasoning models (never more than the resolved output limit).
The final report reserves its full template. Structured OpenAI requests carry
the schema once, with an inline schema only for strict-JSON fallback.

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

### Summary edits and previous version

Meeting title and summary edits autosave after two seconds of inactivity. Pending
revisions remain owned by a shared draft queue across navigation; leaving a meeting
flushes that queue. Closing the window flushes edits, and quitting waits for pending
writes for up to five seconds. A failed save or timeout opens one native dialog
with Quit (discard pending edits) and Keep ClawScribe open. Choosing Keep preserves
the drafts; stale callbacks cannot complete a later quit attempt. Failed writes
retain the draft and offer retry. Summary generation blocks
edit writes, and regeneration asks before replacing a result marked `user_edited_at`.
A generated replacement removes that marker and retains one `previous_result` with
its timestamp. Restore atomically swaps current and previous results when generation
is idle. These columns are independent of `result_backup`, whose failure and
interruption recovery behavior is unchanged.

Per-meeting summary context is stored in `meetings.summary_context`. Startup
migrates legacy browser-storage values without replacing existing database
context and removes each legacy value only after persistence succeeds. Library
archives include the column; older archives omit it and restore with empty
context. Deleting the meeting removes its context.

Live-transcript IndexedDB copies are removed after SQLite save and library
deletion. Startup cleanup runs once from the root layout and removes only aged
saved copies; unsaved recovery data never expires automatically.

Startup removes unattached live bookmarks only when their recording folder is
gone and no saved meeting owns that folder. Existing recovery folders and
attached bookmarks are retained, and cleanup skips active recordings.

Restored recording outcomes retain whether recovery files were excluded from the
archive. Those meetings direct recovery to the original computer; they do not
recommend retranscribing unavailable audio. Older archives default this flag to false.

## Local Knowledge Embedding Foundation

The optional knowledge state is disabled on startup and does not load/download
models implicitly. Its CPU ONNX candidate and SHA-256 manifest live under
`knowledge/`; explicit downloads use the existing resumable transfer registry.
The embedding-space identity includes the model revision, both artifact hashes,
tokenizer version, prefixes, pooling version, and CPU loading policy. The revised
candidate disables graph optimizations and weight prepacking while keeping the
exact float32 artifact. It constructs the session before the tokenizer to avoid
overlapping retained tokenizer allocations with transient model loading. ORT
defines prepacking control as `session.disable_prepacking=1` in its
[session configuration reference](https://github.com/microsoft/onnxruntime/blob/v1.22.0/include/onnxruntime/core/session/onnxruntime_session_options_config_keys.h).
The model-space identity distinguishes this loading policy from the earlier
optimized candidate. Inference handles one input
at a time with two intra-op threads and sequential graph execution. Inputs are
bounded to 1,024 UTF-8 bytes for questions and 512 model tokens including
prefix/special tokens for both queries and passages; oversized inputs fail
rather than truncate. The indexing layer separately limits chunk bodies to
320 tokenizer tokens with 48 body tokens of overlap.

Knowledge calls own the shared job and native permits until the blocking call
returns. Foreground recording/import claims cancel knowledge first and prevent
new knowledge claims while waiting up to two seconds; local-summary preemption
retains its existing behavior. Index notifications coalesce by source revision
in a 32-entry queue; a full queue reports busy so persistent indexing can retry.
The resident model unloads after 60 seconds idle. Provider/inference errors
carry categories only, without source text or raw native responses.

The manual knowledge input of the summary regression workflow runs on the
configured trusted runner, using an exact reviewed commit. Ordinary regression
runs never download embedding models. Its acceptance option independently
computes PyTorch/model-card reference vectors from pinned artifacts, then tests
ONNX parity, peak incremental process memory and cold/warm recording preemption.
Synthetic fixtures and reference scripts are public; models, reference vectors,
Python environments, and benchmark output stay on the runner. The acceptance
gate must pass on the designated runner before dependent indexing work proceeds.
This foundation does not expose semantic search to users.

The tokenizer is pinned to Apache-2.0 `tokenizers = 0.21.4` with default/network
features disabled and the Rust regex implementation selected. ORT stays at the
existing locked `2.0.0-rc.10`. Dependency validation uses the supported stable
runner toolchain: the repository's declared Rust 1.77 floor is already older
than ORT's Rust 1.81 and existing `time`/`serde_with` Rust 1.88 requirements.
This change does not claim compatibility with Rust 1.77.

### Canonical knowledge indexing and retrieval

The knowledge backend uses an additive SQLite migration to seed a stable source
for every saved meeting, including empty meetings. Canonical transcript changes
advance the source revision, immediately invalidate semantic publication and
coalesce one durable job. Speaker, timing, row identity/order and moves between
meetings participate in invalidation. SQLite FTS5 projects only the changed row
inside the existing transaction; model tokenization and inference run in the
background. FTS deletion is explicit because virtual tables do not inherit
ordinary foreign-key cascades.
The FTS physical row ID matches the canonical SQLite transcript row ID, so
per-row updates and deletions use point lookups rather than scanning the full
FTS table. Evidence identities continue to use canonical transcript IDs and
spans, independently of that physical projection.

FTS indexes current speaker labels in their own column. Speaker-only hits return
real transcript spans with visible speaker metadata; semantic bodies remain
canonical transcript text. Speaker renames invalidate both lexical and semantic
generations. The source schema reserves document kind and nullable meeting
ownership for later migrations, while the current worker accepts meeting sources
only and all document inputs remain rejected.

One worker starts after the installed database pool is available, across normal,
fresh and legacy-import initialization. It polls durable jobs even if a bounded
notification was dropped. Publication and cleanup compare both source revision
and indexing generation under a write transaction. Reindexing the same revision
therefore supersedes older work. Recording contention, disabled indexing,
cancellation and superseded jobs do not consume the three real-failure attempts.
The worker borrows the existing scheduler tokenizer inside bounded calls; it
does not retain a second tokenizer or a handle across recording backoff.

Keyword evidence uses exact canonical UTF-8 windows of at most 2,048 bytes with
at most 128 bytes of overlap. Semantic bodies use at most 320 model tokens and
48-token overlap, with prefixes/special tokens separately checked against 512.
Evidence identities hash source/revision, canonical row/span and a fingerprint
including current speaker and time metadata. They do not depend on derived
cache rows. Titles and meeting dates are joined from current metadata.
FTS selection carries the original source, revision and generation into
materialization; a moved or replaced row is rejected as superseded. Canonical
row pages contain IDs only. Bodies and complete speaker/timing metadata use
16 KiB incremental SQLite reads, with hashing and keyword matching on one
bounded blocking reader. Each reader owns its connection, read transaction,
locked handle and BLOBs until completion or acknowledged cancellation, and
releases them before model inference or writes. Connection acquisition, identity
lookup and locked-handle setup check cancellation/foreground priority at
five-millisecond intervals while pending. Shared inference admission is acquired
only for synchronous native work after setup, and is released before transaction
completion awaits. Setup cancellation follows SQLx ownership/rollback; an active
BLOB still closes in its owning blocking worker. Recording prevents background
reads and preempts them between windows; keyword reads remain available.

The reusable canonical reader is `knowledge::store`: carry a `SelectedRow`
(source, owner, revision and generation) from scoped selection, then use
`locate` for a keyword span or `materialize` for a known canonical span. Each
call rechecks that identity within its own read transaction; callers must
still recheck their frozen scope before persisting an answer. Background
indexing uses `row_ids_page` (32 IDs) and `body_window` (16,384 bytes).
No caller receives a database handle or retains one across provider work.

A keyword scan retains one 16,384-byte UTF-8 input, a folded-string capacity
of at most 49,152 bytes and an offset-map capacity of at most 49,152 pairs
(786,432 bytes on the supported 64-bit runtime), independently of row length.
Its 4,096-byte scan overlap covers the bounded query across read boundaries.
Hashing retains one 16,384-byte metadata input plus the passage; JSON escaping
streams directly into SHA-256. Display strings add at most 3,072 bytes per
passage. Indexing can temporarily retain both a passage and its inference
input copy (at most 32,768 bytes combined); tokenizer/model allocations are
separate existing runtime costs. These bounds cover canonical content and
metadata buffers, not SQLite's cache, canonical IDs, frozen scope lists or
the already bounded candidate/vector collections.

The native boundary uses the existing resolved SQLx 0.8.6 and libsqlite3-sys
0.30.1, now pinned exactly as required by
[SQLx's locked-handle API](https://docs.rs/sqlx/0.8.6/sqlx/sqlite/struct.LockedSqliteHandle.html#method.as_raw_handle).
[SQLite incremental BLOB reads](https://www.sqlite.org/c3ref/blob_open.html)
also support TEXT values and avoid whole-value copies from SQL substring/cast
expressions. BLOB handles close before their SQLx guards and transactions.
No SQLite or model runtime version changes accompany this dependency edge.

Display speaker, title and date strings are each capped at 1,024 UTF-8 bytes,
with `metadata_truncated` explicitly marking incomplete previews. Complete
canonical metadata is streamed into the existing evidence fingerprint format;
internal ordering and scope checks use authoritative values. Answer prompts
must label incomplete metadata rather than infer facts from clipped labels.

Library scope requires selected meeting IDs, project/date/untagged constraints,
or explicit `all_meetings`. Unicode-lowercase tag equality and Any/All/Untagged
semantics match the library. Scope resolves before ranking and can be frozen and
rechecked on a caller-owned SQLite transaction. A frozen scope never acquires
new meetings. Live and document inputs remain rejected at this phase.

`knowledge_search` accepts a scope, query, document IDs and `keyword`/`hybrid`
mode. It returns actual mode, index status and canonical passages. Questions are
limited to 1,024 UTF-8 bytes. Quoted lexical queries and a local cosine scan each
retain at most 64 candidates; vector pages hold at most 512 rows and scoring
runs on blocking workers. RRF uses constant 60 and returns at most 12 passages.
When multiple current indexed sources are eligible, the semantic heap keeps
at most three candidates per source. A sole eligible source retains all 64
slots. This limits single-source crowding without merging or deleting stored
passages; lexical selection retains its full budget. Actual hybrid mode requires
current vectors inside the selected scope.
Lexical rank transfers to a published semantic chunk only when that chunk
covers the complete match. Containment deduplication stays within one source
revision and preserves distinct dated meetings. Missing models or recording
contention return keyword results with a visible fallback reason.
The saved semantic opt-in is distinct from runtime readiness: an enabled
preference with missing model files reports `model_unavailable`, not `disabled`.

The command surface also includes `knowledge_index_status`, `knowledge_reindex`,
`knowledge_cancel_index`, `knowledge_model_enable`, `knowledge_model_status`,
`knowledge_model_download` and `knowledge_model_cancel_download`. Semantic
enablement is an explicit persisted preference, initially off; a missing model
never triggers an automatic download. These backend commands precede the
meeting-intelligence UI and do not change the existing archive search UI.

The guarded manual workflow has a separate retrieval-acceptance switch. It runs
production indexing and end-to-end searches over 10,000 distinct synthetic
passages and 30 fixed English/German queries, including nonlexical paraphrases,
cross-language questions, identifiers and project/date conflicts. It reports
bulk insertion/coalescing, indexing time, process-memory growth, top-five recall
and warm p95. Models and measurements remain local to the designated runner.

### Saved conversations and canonical citations

`knowledge_ask` accepts a globally unique request UUID, a durable meeting or
library owner, and the same explicit scope used by search. The backend resolves
the configured provider/model, freezes the selected meeting IDs, and saves one
user turn before generation. A completed retry with identical inputs returns
the same assistant message; reusing an identity with different inputs fails.
Cancelled or invalidated requests cannot accept late output. Startup marks
interrupted requests without automatically resubmitting them.

Prompts contain bounded JSON sections for the question, eligible prior turns,
and transcript evidence. Document and live inputs remain rejected. Small
selections can use complete canonical rows to retain short replies and dated
contradictions: at most eight meetings, fewer than 32 rows per meeting, 64 rows
overall, and 2,048 bytes per complete row. Larger selections retain bounded
search results. Context limits can still omit whole rows; answers must qualify
incomplete evidence. Source text and metadata are untrusted data. Clipped
metadata is explicitly marked incomplete.

Each request owns its ordered `[K1]` evidence map. Unknown tags never become
links. `knowledge_resolve_evidence` reads the current bounded canonical passage
and checks source revision, content/metadata fingerprint, transcript ID, exact
UTF-8 span and audio offset. Only a current result supplies a navigation target.
Derived chunk IDs and SQLite rowids are never citation identities. Saved title,
date and speaker labels describe the original answer snapshot. Editing a title
or meeting date advances the source revision, invalidates pending answers and
requeues semantic indexing; keyword retrieval remains available during rebuild.

Prior turns enter a new prompt only when their frozen scope is contained in the
current selection and every source dependency is current. Historical tags are
neutralized so they cannot refer to the new request's map. Normalized dependency
rows cover inherited context as well as newly selected evidence. Source checks
run immediately before dispatch, during generation, and within the final write
transaction. Deleting a source redacts dependent turns before removing its
associations. Clearing a meeting removes legacy and new history together and
signals its pending requests to stop.

Configured text generation uses one absolute deadline across setup, queueing,
retries and reads: HTTP 300 seconds or the compatible endpoint's configured
duration, bundled Codex's configured duration with its 30-second minimum, and
Built-in AI 900 seconds. The existing text helper delegates to the cancellable
path. Reply metadata names the backend-normalized dispatched provider and model;
it does not claim to observe a gateway's internal model routing.

A bounded supervisor retains each operation when its caller disappears. Stop
signals cancellation without requiring the UI to await cleanup. Cleanup has a
separate five-second reporting allowance; expiry reports quarantine and does
not release a live child's exchange or inference admission. The local helper
rejects reuse while quarantined. Codex cleanup owns only that request's child
and bounded readers. Ownership ends after actual reap; runtime pins, executable
discovery and credential/profile boundaries remain unchanged.

Version 1 portable archives include authoritative owner, request, message,
evidence and dependency rows in dependency order. They omit semantic caches.
Restored citations preserve their canonical identities but are explicitly
historical, even if newly created source revision counters match their old
values. Readable restored history is excluded from future prompts until fresh
evidence is established. Source counters are never rewritten to make a
historical reference appear current.

These native conversation commands precede the meeting-intelligence UI. The
guarded answer-acceptance workflow uses a fixed invented corpus, an isolated
validation profile and the catalog-pinned Built-in AI model. Its bounded public
synthetic step summary contains actual first-attempt answers and prompt/evidence
records for factual review. Ordinary diagnostics contain outcome metadata only;
model binaries and generated evaluation files are not uploaded or committed.

The manual synthetic answer review job has narrowly scoped Check Run write
permission. It publishes bounded batches of the invented fixture, exact prompts,
actual responses and canonical resolutions for independent factual review, and
keeps the same report in its step summary. Ordinary validation jobs remain
read-only. Reports are neutral until factual review; they are never diagnostic
logs, repository files, Actions artifacts or cache entries.

Completed identical requests validate the saved immutable inputs and return their
original answer before resolving current provider settings or credentials.
Cancellation signals the active operation immediately, before database access.
Terminal status persistence shares the existing overall cleanup allowance; if
storage remains busy, the caller receives a cleanup error and the bounded
request registry retains ownership until that write finishes. Caller drop uses
the same retained cancellation ownership.

Answer instructions distinguish an established action from an unassigned owner
or missing deadline, require original and replacement source citations when
comparing dated changes, and pair short answers with their question anchors.
A later incomplete reopening cannot establish suspension or a replacement
outcome. Actual-provider factual review remains separate from structural
citation and persistence checks.
