# Meeting Intelligence Design

Status: proposed architecture for implementation planning. These capabilities
are not part of the current product.

ClawScribe should help users find decisions across saved meetings, bring their
own reference documents into meeting questions, and ask for assistance during a
recording. Implement these in that order, using one retrieval and evidence
layer. Recording remains the highest-priority workload.

Across-meeting memory means retrieval from the saved archive: the application
loads a small set of relevant passages into each answer's context. Archive size
does not need to fit the language model's context window. Retrieval can miss
evidence, so citations, scope, and an explicit insufficient-evidence response
are part of the feature rather than an accuracy guarantee.

## Scope and success

| Phase | User outcome | Completion gate |
| --- | --- | --- |
| 1. Meeting knowledge | Search by meaning and ask a question across a deliberately selected set of saved meetings. | Relevant English and German passages are retrieved, answers cite saved evidence, and edits/deletion cannot leave searchable stale content. |
| 2. Reference documents | Attach PDF, DOCX, TXT, or Markdown files to a meeting and include selected files in questions. | Citations open the correct page or paragraph; unsupported documents fail visibly; backup/restore preserves the references. |
| 3. Live assistance | Open a compact panel and explicitly ask about the conversation while recording. | Requests use finalized backend transcript segments, recording remains responsive, and stopping or starting another session invalidates old requests. |

The first live version is manual. Automatic question detection, screenshot
analysis, browser capture, phone mirroring, process disguise, and new calendar
or task integrations are outside this implementation. Each phase can ship
without the next phase.

## Architecture choice

| Approach | Tradeoff | Decision |
| --- | --- | --- |
| Local Rust embedding inference and the existing SQLite database | Requires a model download and careful scheduling; works independently of an external service. | Recommended default. |
| Ollama embeddings | Reuses a familiar provider but requires an available service and a compatible embedding model. | Possible later adapter, not a prerequisite. |
| Cloud embeddings | Avoids local model memory but uploads meeting content and introduces provider costs and availability. | Outside the initial scope. |

Create `frontend/src-tauri/src/knowledge/` for source loading, chunking,
embedding, index lifecycle, retrieval, evidence resolution, and request
orchestration. Register a single `KnowledgeState` through Tauri managed state.
Keep existing recording state authoritative and reuse summary-provider
configuration, context budgeting, and protected credentials.

SQLite stores source revisions, chunks, normalized vectors, indexing jobs, and
conversation request identities. Initially use SQLite FTS5 for lexical
retrieval and a paged Rust cosine scan for vectors. Do not add a SQLite native
vector extension until the acceptance corpus demonstrates that it is needed.
Database access stays in SQLx; model inference and scoring run on blocking
workers with bounded inputs.

## Local model and retrieval

Use `intfloat/multilingual-e5-small` as the first model candidate. Its
[model card](https://huggingface.co/intfloat/multilingual-e5-small) documents
384-dimensional embeddings, a 512-token input limit, query/passage prefixes,
masked mean pooling, and normalization. The model is MIT-licensed. The initial
candidate revision is `614241f622f53c4eeff9890bdc4f31cfecc418b3`.

Validate the official `onnx/model.onnx` CPU baseline with the existing
`ort = 2.0.0-rc.10` dependency. Do not assume the AVX-512 VNNI-specific
quantized export runs on the supported notebook class. Task 1 must establish
compatible tokenizer dependencies, artifact SHA-256 values, tensor names,
reference-output parity, and the resource gate before this candidate becomes
the product default. Download only the required files, using the existing
resumable transfer and integrity-check patterns. Never commit model binaries.

Validate a conservative CPU loading policy with graph optimizations and
weight prepacking disabled, while retaining the same pinned float32 artifact,
two-thread limit, and independent reference comparison. The initial optimized
loading candidate exceeded the worker memory budget despite passing timing and
numerical parity. Neither a larger memory allowance nor an alternative quantized
artifact is an automatic fallback; accept the revised policy only after the
unchanged resource gate passes.

Retrieval rules:

- Semantic indexing is disabled until the user enables it and downloads the
  model. Keyword search remains available without it.
- Semantic chunks contain at most 320 body tokens with a 48-token overlap. Split oversized
  rows without losing their transcript IDs and byte spans. Account for special
  tokens and prefixes within the 512-token input limit.
- Before the tokenizer download, keyword evidence uses canonical UTF-8 windows
  of at most 2,048 bytes with at most 128 bytes of overlap, split on character
  boundaries. These are byte limits, not estimated model-token counts. Preserve
  row IDs, exact byte spans, speakers, and recording times. After semantic
  publication, transfer a lexical hit to a real current model chunk only when
  its spans cover the complete match; otherwise keep the lexical evidence.
- Index current corrected transcript text and speaker labels. Generated
  summaries and assistant messages are not primary evidence.
- Speaker labels have their own lexical index field. Keep semantic embeddings
  limited to canonical transcript bodies; return speaker-name matches with
  actual row spans and current speaker metadata, including after label edits.
- Apply meeting, date, and project-tag scope in the backend before ranking.
  Document context is limited to explicitly selected attachments in that scope.
- A library request selects meeting IDs or project/date/untagged constraints,
  or explicitly opts into all meetings. An empty selection never silently
  broadens an answer. Meeting-owned requests keep that meeting as their scope.
- Retrieve up to 64 candidates from each of lexical and semantic search, merge
  with reciprocal rank fusion using constant 60, and return at most 12 passages.
  Vector scans read at most 512 rows per page and retain only the best candidates.
- When at least two currently indexed saved sources are eligible in the frozen
  scope, retain at most three semantic candidates per source within the overall
  64-candidate budget. A sole eligible source retains the full budget. This
  prevents repetitive passages from one large source from crowding out other
  meetings. Lexical candidates keep their existing budget and exact identifiers.
  This is candidate selection, not deduplication: preserve every stored passage
  and its distinct source, date, span, and fingerprint. A question needing more
  semantic passages from one source can select that source alone.
- Questions retain the existing 1,024 UTF-8-byte limit. Evidence uses the
  selected provider's context budget; never silently exceed it.
- Missing models, recording contention, or indexing failures produce a visible
  keyword-only status. Never automatically send text to a cloud embedding API.
- Relevance ranks evidence; it does not prove that evidence answers a question.
  The answer prompt must state insufficient support instead of guessing.
- Supply each saved passage's meeting date and title. A question about the
  latest decision must preserve conflicting earlier/later evidence, identify
  when a decision changed, and qualify gaps instead of treating the closest
  semantic match as the current decision. Project scope prevents unrelated
  meetings with similar terminology from being silently combined.
- Cap each passage display string (speaker, title and date) at 1,024 UTF-8
  bytes on a character boundary and set `metadata_truncated` when any is
  incomplete. Canonical metadata is retained and fully fingerprinted. Answer
  prompts must label incomplete metadata and must not treat a clipped name or
  date as a complete factual value; source previews must show the indication.

The initial workload gate is 10,000 passages from synthetic meetings and at
least 30 English/German retrieval questions. At least 27 questions must return
the expected passage in the first five results. Warm search p95 should be at
most two seconds on the designated runner. These are acceptance targets, not
current performance claims.

Before enabling generated library answers, review a 12-case synthetic answer
set covering supported facts, absent evidence, changed decisions, project-name
collisions, and short German responses. Every expected answer must have correct
source navigation and no unsupported factual claim. A model/provider change
requires rerunning this answer set; passing retrieval alone is insufficient.

## Persistence and invalidation

Add new immutable SQLx migrations; do not alter shipped migrations.

- `knowledge_sources`: source ID, kind, optional meeting ID, current revision,
  indexing state, and content fingerprint.
- `knowledge_chunks`: source ID/revision, ordinal, current text, transcript
  row/byte spans or document anchors, and content fingerprint.
- `knowledge_vectors`: chunk ID, embedding-space identity, dimension, and
  normalized vector bytes. Space identity includes model revision, tokenizer,
  pooling, and preprocessing version.
- `knowledge_index_jobs`: one coalesced job per source, requested revision,
  attempt count, and sanitized failure category.
- `knowledge_requests` and `knowledge_messages`: conversation owner, request
  ID, state, answer, evidence map, and provider/model metadata.

New source generations are published atomically only if the source revision
still matches the snapshot that was embedded. Retrieval excludes older
generations immediately. Transcript insert/update/delete triggers mark a
meeting source dirty inside the same transaction, covering corrections, undo,
retranscription, speaker updates, import, restore, and legacy command paths.
Source removal cascades to chunks, vectors, and jobs.

Request evidence has normalized source associations. Deleting a source
invalidates pending requests and removes stored question/answer turns that
depended on it; other library conversations survive. Revalidate evidence before
dispatch and before saving a reply. Changed evidence is visibly stale and must
be retrieved again. Retry interrupted index jobs on startup; stop automatic
retries after three failed attempts and offer Retry.

Portable backups include authoritative request/message data, attachment
metadata, extracted document blocks, and original attachment files. Exclude
derived vectors, FTS rows, jobs, credentials, and model files. Restore queues a
rebuild. Phase 2 adds an archive format revision for attachment files while
preserving restoration of existing version-1 archives. Backups remain
unencrypted, consistent with the current product.

## Evidence and conversation behavior

Every retrieved passage carries a backend-owned reference:

`EvidenceRef { source_id, source_revision, chunk_id, fingerprint, locator }`

`locator` identifies saved transcript IDs and recording-relative time,
document page/paragraph, or live session ID and segment sequence. The model
receives short tags such as `[K1]`. Resolve tags only through the supplied
evidence map; unknown tags cannot become navigation links. Citations open a
transcript passage/playback or an extracted-document preview. Document
evidence must remain distinguishable from what someone said in a meeting.

Use the existing summary source UI behavior as a reference, but introduce an
evidence resolver that handles more than one meeting and documents. Do not
reinterpret existing saved summary links or change their identifiers.

Evidence identities are backend-owned canonical units. Resolving a reference
uses its locator and fingerprint independently of the derived chunk/vector
cache, so rebuilding the index does not erase conversation history. A changed
source revision remains stale. Library threads have durable owner rows;
request UUIDs are globally unique and an input fingerprint prevents changed
inputs from replaying an unrelated result. Deletion dependencies include any
prior-turn evidence actually sent in a later prompt. Clearing history cancels
pending requests before deleting both legacy and new turns.

The current request's scope also constrains inherited prompt history. Omit
prior turns whose scope or evidence cannot be validated within the frozen
allowed meeting and selected-document set; changing a project or date filter
must not resend older information from outside that scope. Saved history
remains readable under its original conversation owner.

Portable conversation restore follows owner/request/message/evidence dependency
order with table-specific validation. Library turns do not acquire a fictitious
meeting ID. Old version-1 archives remain readable; that does not promise that
older applications can read newly added tables. Index caches are rebuilt and
historical references may remain stale after restoration.

Restored conversation evidence is explicitly historical and remains non-current,
even if its saved revision counter happens to equal a newly recreated source
counter. Restore preserves the original canonical IDs, spans, fingerprints and
display metadata; it does not invent cache rows or rewrite source counters.
Readable restored history is excluded from future prompts until fresh evidence
is established. This conservative policy avoids upgrading references that were
already stale when the backup was created.

Changing a meeting title or start date advances its source revision. The existing
source-generation trigger invalidates and requeues semantic indexing, so a
metadata edit can temporarily leave Keyword mode as the available channel.
This ensures a dated prompt snapshot is rechecked alongside transcript content
before dispatch and persistence; historical citation metadata is not overwritten.

Conversation owners are a saved meeting, a library thread, or a live session.
Existing `ai_chat_messages` history remains readable. New knowledge requests
use UUID request identities and a unique owner/request constraint. A retry
returns the existing result; it does not generate another assistant message.
The first implementation returns a completed reply using the existing provider
path. Cancellation and timeout are required; streaming is a later improvement.

Saved answers use one absolute request deadline across configuration, startup,
queueing, writes, retries, and reads. Preserve the existing provider budgets:
HTTP 300 seconds or the compatible provider's configured timeout; Codex's
configured duration, default 600 seconds and existing 30-second minimum; and
Built-in AI 900 seconds. Allow at most five additional seconds for cleanup
before surfacing a cleanup error. Cancellation never proves a child exited:
retain resource ownership until actual termination and reaping, or quarantine
the resource explicitly. Keep native admission and shared helper locks owned
through cleanup. Live assistance uses its separate 30-second deadline; stopping
a recording never awaits assistance cleanup.

The configured-text supervisor owns each operation independently of its waiting
caller. Dropping the caller signals cancellation; it does not drop the native
operation. If cleanup exceeds five seconds, report the cleanup error while the
supervised owner retains the child, exchange lock and native admission until
actual reaping. A helper awaiting cleanup is quarantined from reuse. Bound the
number of supervised owners so failed cleanup cannot accumulate unbounded work.

Scope is visible beside the input: This meeting, Selected meetings, or Current
recording, plus selected reference documents. A saved meeting remains the
default. A library-wide scope requires a deliberate selection. Project filters
must behave like the existing Any/All/Untagged controls.

## Reference document boundary

Copy explicitly selected files into app-owned storage using generated IDs,
then extract locally. The original path is not a persisted identifier. Keep a
separate attachment relation so removing an attachment from one meeting does
not delete a document still attached elsewhere.

- Accept `.pdf`, `.docx`, `.txt`, and `.md`; verify format as well as extension.
- Limit input to 25 MiB, extracted UTF-8 text to 2 MiB, PDF pages to 500, and
  extraction to 10 seconds per file. Limit DOCX data actually decompressed to
  32 MiB and a single XML entry to 5 MiB.
- Run extraction in a child mode of the existing application executable,
  dispatched before Tauri startup. This is a short-lived parser, not a service.
  Use a hidden Windows process and a Job Object with a 512 MiB process-memory
  limit; terminate and reap it on cancellation or timeout.
- Use a Rust PDF text extractor and a bounded ZIP/XML DOCX reader. Task 5 pins
  parser versions after Windows/MSRV and license checks. Never invoke Office,
  fetch external relationships, execute macros, or resolve XML external entities.
- Scanned or encrypted PDFs, malformed documents, invalid UTF-8, and empty
  extraction receive actionable errors. OCR is outside the first version.
- Citations refer to actual extracted pages/paragraphs. Replacing a document
  creates a new revision; a repeated file hash can reuse its existing content.
- Commit metadata and blocks only after extraction succeeds. Failed or cancelled
  imports remove their unpublished copy and leave existing attachments intact.

Treat extracted text as evidence, not instructions. An attachment can be used
locally without permitting upload. Before an external-provider request includes
reference excerpts, show the provider and require the corresponding context
sharing option to be enabled. Persist that choice per conversation owner.

## Resource scheduling and live assistance

The existing Built-in AI path claims the recording/transcription job gate.
Preserve that exclusion. Do not bypass it to make a local answer appear live.

Index with one worker, one embedding input at a time, and at most two CPU
inference threads. An embedding operation retains its native permit until the
call actually returns. New recordings preempt indexing between inputs; Task 1
must prove the current call completes within two seconds. Target incremental
embedding-worker memory at no more than 1 GiB and unload the session after
60 seconds idle. Source jobs stay durable; the in-memory notification queue is
bounded to 32 entries and coalesces by source ID.

During recording, pause model loading, embedding, document extraction, and
local knowledge-answer generation. Search can use lexical evidence. External
answers use the configured provider only after the user enables live text
sharing. Ollama is treated as a separately running provider, with visible
resource guidance; enabling it does not certify concurrent notebook performance.

The live panel uses a backend snapshot of finalized transcript segments.
Default context is the last 10 minutes; selected saved references may supplement
it. Audio-only mode explains that no live transcript is available. Provide
Ask, Summarize so far, and List open questions actions. Each action requires a
user submission, allows one request in flight, and offers Cancel. The request
deadline is 30 seconds.

Assign a fresh backend session UUID at recording startup. Bind every live
request and event to that UUID and a request UUID. Stopping signals cancellation
without awaiting provider completion; late replies never appear in another
session. Keep live conversation state in memory initially, and clear it on
stop rather than introducing an implicit new retention policy. Navigation and
panel visibility changes cannot create duplicate transcript subscriptions.

Transcript backlog and incomplete-capture state remain visible beside live
assistance. Answers identify the latest finalized transcript time; they must
not imply that delayed transcription describes the current moment.

## Validation and rollout

Each phase updates its product documentation only when implemented. Run all CI
jobs, installer builds, and native Windows diagnostics on the trusted local
runner selected by `CLAWSCRIBE_BUILD_RUNNER`. Do not schedule forks or untrusted
authors on that machine, add hosted runners, or commit local benchmark output.

Exercise fresh and upgraded databases; stale evidence; changed model spaces;
interrupted jobs; cancellation; repeated submissions; provider failures; and
document limits. Verify dark/light themes, accent colors, keyboard operation,
focus, and narrow desktop layouts.

Before accepting Phase 3, run a release-like two-hour mixed microphone/system
audio session. Record model, language, engine, acceleration backend, memory
trend, backlog, request timing, stop timing, saved-audio playback, and a second
recording. Compare against the same build with knowledge features disabled.
Require no capture gaps attributable to assistance and no unbounded memory
trend; stop must not wait for an assistance request or index backlog. Native
call/model failures preserve captured audio and expose degraded functionality.

Implementation must use independently developed code and appropriately licensed
dependencies. Do not copy source, prompts, or UI assets from a project whose
license is incompatible with ClawScribe's MIT distribution.
