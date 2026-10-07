# Meeting Intelligence Implementation Plan

> **For agentic workers:** Use `executing-plans` for sequential implementation,
> or `subagent-driven-development` when the user selects delegation. Read the
> design and repository instructions before starting each phase. Checkboxes
> track implementation work, not capabilities already delivered.

**Goal:** Add semantic meeting search, reference-document context, and manual
live assistance without compromising recording or local data control.

**Architecture:** One Rust knowledge subsystem owns source revisions, local
embeddings, hybrid retrieval, evidence, and request identities. It uses the
existing SQLite database, Tauri commands/events, model downloads, and summary
providers. Deliver search, documents, and live assistance as separate phases.

**Tech stack:** Existing Rust/Tauri 2, SQLx/SQLite, ONNX Runtime through
`ort = 2.0.0-rc.10`, React/Next.js/TypeScript, and pnpm. Add a tokenizer and
document parsers only through the compatibility tasks below.

**Spec:** [Meeting Intelligence Design](meeting-intelligence-design.md).

## Global constraints

- Windows is the supported release target. Keep the Tauri application runtime.
- Recording remains the highest-priority workload; inference runs on blocking workers.
- Local embeddings are opt-in; cloud embeddings are outside the initial scope.
- Chunk size is at most 320 model tokens; overlap is 48 tokens; query limit is 1,024 UTF-8 bytes.
- Retrieval uses 64 candidates per channel, reciprocal rank fusion constant 60, and at most 12 passages.
- One embedding input at a time, at most two inference threads, a 32-entry coalesced notification queue, and 60-second idle unload.
- Reference limits: 25 MiB input, 2 MiB extracted text, 500 PDF pages, 10-second extraction, 512 MiB child memory, 32 MiB DOCX decompression, and 5 MiB per XML entry.
- Live context defaults to 10 minutes; one request in flight; request deadline is 30 seconds.
- Preserve current histories, summary-source identifiers, provider isolation, credential storage, and backup restore compatibility.
- Add migrations; never edit shipped migrations. Commit neither models nor private runtime data.
- Run CI, builds, and native Windows diagnostics only on the trusted runner configured by `CLAWSCRIBE_BUILD_RUNNER`.
- Every implementation commit runs `node scripts/verify-public-repo-safety.mjs` first.
- Version bumps, tags, publishing, and releases require a separate release request.

## Review focus

1. A transcript correction or deletion during indexing must exclude the older generation immediately. Task 2 owns this test.
2. A retry or late provider completion must produce exactly one assistant reply for its owner/request. Task 3 owns this test.
3. A source removed during an answer must not be sent from a queued snapshot or persisted as a current citation. Task 3 owns this test.
4. A renamed extension, oversized DOCX entry, scanned PDF, or stuck parser must fail without publishing an attachment. Task 5 owns these tests.
5. Stop, route changes, and a second recording must not expose an earlier live reply or delay capture finalization. Task 7 owns these tests.

## Existing integration points

| Existing file | Role in the change |
| --- | --- |
| `frontend/src-tauri/src/summary/chat_commands.rs` | Existing meeting chat, history compatibility, question validation. |
| `frontend/src-tauri/src/summary/chat_context.rs` | Current lexical selection; retain as the small-meeting fallback. |
| `frontend/src-tauri/src/summary/llm_client.rs` | Shared provider resolution; add cancellation without duplicating credentials or routing. |
| `frontend/src-tauri/src/summary/context_budget.rs` | Provider input/output budgets. |
| `frontend/src-tauri/src/summary/sources.rs` | Existing source-tag/fingerprint conventions; preserve saved summary links. |
| `frontend/src-tauri/src/database/repositories/transcript.rs` | Existing archive keyword search and saved transcript writes. |
| `frontend/src-tauri/src/database/transcript_edits.rs` | Correction, replacement, undo, restore. |
| `frontend/src-tauri/src/database/repositories/meeting.rs` | Transactional meeting deletion. |
| `frontend/src-tauri/src/library/backup.rs` | Authoritative backup tables and file manifests. |
| `frontend/src-tauri/src/audio/inference.rs` | Job/native permit lifetime and recording preemption. |
| `frontend/src-tauri/src/audio/recording_commands.rs` | Backend transcript snapshots and recording lifecycle. |
| `frontend/src-tauri/src/model_download.rs` | Resumable transfers and cancellation reservations. |
| `frontend/src/app/meetings/page.tsx` | Archive search and project scope. |
| `frontend/src/components/MeetingDetails/MeetingChat.tsx` | Meeting conversation UI. |
| `frontend/src/components/MeetingDetails/SummarySources.tsx` | Source preview/reveal/play behavior. |
| `frontend/src/contexts/RecordingStateContext.tsx` and `TranscriptContext.tsx` | Authoritative frontend state and subscriptions. |

All new paths below are proposed files. Existing integration paths are the
current implementation, not renamed placeholders.

## Phase 1 Meeting knowledge

### Task 1 Local embedding runtime and recording priority

**Files:** Create `frontend/src-tauri/src/knowledge/{mod.rs,model.rs,embedding.rs,scheduler.rs,types.rs}`
and `embedding-model-pins.json`; modify `frontend/src-tauri/Cargo.toml`,
`Cargo.lock`, `frontend/src-tauri/src/lib.rs`, and `audio/inference.rs`.
Tests live in the new modules; add public synthetic fixtures under
`frontend/src-tauri/tests/fixtures/knowledge/`.

**Interfaces:**

```rust
enum EmbeddingPurpose { Query, Passage }
struct EmbeddingSpace { id: String, dimensions: usize }
trait EmbeddingBackend {
    fn space(&self) -> EmbeddingSpace;
    fn embed(&mut self, text: &str, purpose: EmbeddingPurpose)
        -> Result<Vec<f32>, KnowledgeError>;
}
// KnowledgeError categories include disabled, model unavailable, busy,
// cancelled, invalid input, extraction failure, storage, and provider failure.
```

`KnowledgeState` owns the cancellation registry and scheduler; existing
`AppState` continues to own the database. The scheduler acquires the existing
job/native permits for embeddings and releases the native permit only inside
the completed blocking call. Extend recording preemption to cancel indexing
before trying to claim its job; preserve local-summary preemption behavior.

- [ ] Add tests `rejects_bad_dimensions_and_nonfinite_vectors`, `query_and_passage_prefixes`, `cancel_retains_native_permit`, and `recording_preempts_indexing`. Assert 384 finite normalized values, the correct prefix, and no overlap between native calls.
- [ ] Run `cargo test -p clawscribe --lib knowledge::`; the new behavior tests must fail before implementation.
- [ ] Implement CPU inference, masked mean pooling, normalization, two-thread configuration, one-input batches, idle unload, and the bounded/coalesced scheduler.
- [ ] Pin the candidate model revision from the design, required files and actual SHA-256 values. Reuse `model_download.rs`; add a Rust tokenizer dependency with an exact tested version and compatible license/MSRV. Preserve the existing ORT version.
- [ ] On the designated runner, compare embeddings with independently generated model-card reference outputs. Measure warm single-input completion, model memory, and start-recording preemption. Require a native input/preemption bound of two seconds and incremental worker memory no greater than 1 GiB.
- [ ] If the baseline misses these bounds, stop Phase 1 implementation and revise the model artifact/scheduling design with measured results. Do not silently select an unsupported quantized export.
- [ ] Run targeted tests, `cargo fmt --all -- --check`, `cargo check -p clawscribe --features windows-gpu`, and the safety scan; commit `feat: add local knowledge embedding runtime`.

### Task 2 Source generations, chunking, and hybrid search

**Files:** Create `knowledge/{store.rs,chunking.rs,indexer.rs,retrieval.rs}`
and `frontend/src-tauri/migrations/20261007000000_knowledge_index.sql`.
Extend `knowledge/types.rs` for the shared evidence DTOs below.
Modify database initialization only where needed to start the worker after
migrations; register commands in `lib.rs`. Validate that this migration ID is
unused before creating it.

**Interfaces:**

```rust
enum KnowledgeScope {
    Meeting { meeting_id: String },
    Library { filter: MeetingFilter },
    Live { session_id: String },
}
struct MeetingFilter {
    meeting_ids: Vec<String>, tags: Vec<String>,
    tag_mode: TagMatch, untagged: bool,
    from: Option<String>, to: Option<String>,
}
enum TagMatch { Any, All }
struct SearchRequest { scope: KnowledgeScope, query: String,
    document_ids: Vec<String>, mode: SearchMode }
enum SearchMode { Keyword, Hybrid }
struct EvidenceRef { source_id: String, source_revision: i64,
    chunk_id: String, fingerprint: String, locator: EvidenceLocator }
enum EvidenceLocator {
    Transcript { meeting_id: String, transcript_ids: Vec<String>,
        spans: Vec<TextSpan>, start_seconds: Option<f64> },
    Document { document_id: String, page: Option<u32>, paragraph: u32 },
    Live { session_id: String, sequence_ids: Vec<u64> },
}
struct TextSpan { transcript_id: String, start_byte: usize, end_byte: usize }
async fn retrieve(pool: &SqlitePool, runtime: &KnowledgeState,
    request: &SearchRequest) -> Result<SearchResponse, KnowledgeError>;
```

`SearchResponse` contains passages, actual retrieval mode, and index status.
Each passage contains `EvidenceRef`, display metadata, text, and rank;
define `EvidenceRef` in this task so Task 3 can implement its resolver.
Keep scope checks in the backend and use
the existing tag semantics. Live is rejected until Task 7 supplies its source.

- [ ] Write clean/upgrade database tests and retrieval tests: `edited_source_cannot_publish_old_generation`, `delete_cascades_index`, `restore_requeues_index`, `tag_scope_applies_before_ranking`, `hybrid_preserves_identifier_hits`, `unicode_chunk_spans_roundtrip`, and `model_space_change_requires_reindex`.
- [ ] Pin assertions: 320-token maximum, 48-token overlap, vector pages of at most 512 rows, 64 candidates per channel, and at most 12 final passages. Verify sources outside the selected meetings/tags never appear.
- [ ] Run the new tests and confirm the absent behavior fails.
- [ ] Add the source/chunk/vector/job tables and FTS5 table. Test FTS5 availability in the supported bundled SQLite runtime. Add transcript mutation triggers and source deletion cascades in the new migration.
- [ ] Implement streaming chunk construction with transcript IDs/byte spans, durable coalesced jobs, three-attempt retry limit, and conditional generation publication. Restarted jobs remain recoverable.
- [ ] Implement quoted/sanitized lexical queries and normalized cosine search with a bounded heap; combine ranks using constant 60. Return keyword-only status when semantic work cannot run.
- [ ] Register `knowledge_search`, `knowledge_index_status`, `knowledge_reindex`, `knowledge_cancel_index`, and model enable/download/status commands. Commands never accept a client-provided SQL filter or embedding-space identity.
- [ ] Run targeted tests and the 10,000-passage/30-question English-German acceptance corpus on the designated runner: at least 27 expected top-five hits and warm p95 no greater than two seconds. Commit after Rust checks and the safety scan: `feat: add hybrid meeting retrieval`.

### Task 3 Evidence, answer requests, and conversation persistence

**Files:** Create `knowledge/{evidence.rs,answers.rs,conversations.rs,commands.rs}`
and `frontend/src-tauri/migrations/20261007000001_knowledge_conversations.sql`.
Modify `summary/llm_client.rs`, `summary/chat_commands.rs`, `lib.rs`,
`database/repositories/meeting.rs`, and `library/backup.rs` as needed.

**Interfaces:**

```rust
// Consumes SearchRequest and EvidenceRef from Task 2.
enum ConversationOwner { Meeting(String), Library(String), Live(String) }
struct AskRequest { request_id: String, owner: ConversationOwner,
    search: SearchRequest }
struct AssistantReply { request_id: String, message_id: String,
    content: String, evidence: Vec<EvidenceRef>, retrieval_mode: SearchMode,
    provider: String, model: String }
struct ConfiguredTextReply { text: String, provider: String, model: String }
```

`knowledge_ask(request) -> AssistantReply`, `knowledge_cancel_request(request_id)`,
`knowledge_history(owner)`, and `knowledge_resolve_evidence(reference)` are the
frontend boundary. Resolve configured provider/model server-side at request
start; do not accept credentials from these commands. Live owners remain
in-memory when enabled in Task 7.

- [ ] Write tests `duplicate_request_returns_same_reply`, `unknown_tag_has_no_link`, `changed_passage_resolves_stale`, `deleted_source_aborts_queued_dispatch`, `deleted_source_discards_completed_answer`, `cancelled_request_cannot_commit`, `prompt_injection_stays_inside_evidence`, and `changed_decision_retains_dates_and_both_sources`.
- [ ] Assert one assistant message per owner/request, zero navigable links for invented tags, no provider dispatch after detected source deletion, and an insufficient-evidence answer for unsupported questions. Run tests and observe failure before implementation.
- [ ] Implement backend-owned `[K1]` maps and fingerprint resolution. Preserve saved summary-source links. Recheck source revisions immediately before dispatch and transactionally before persisting a reply; redact invalidated dependent conversation turns on source deletion.
- [ ] Add request/message tables with a unique owner/request identity and normalized evidence associations. Existing `ai_chat_messages` remain readable; merge legacy history for a meeting deterministically while writing new knowledge turns only once.
- [ ] Add `generate_configured_text_cancellable` beside the current helper with the existing app/state/provider/model/system/user inputs plus `&CancellationToken`, returning `Result<ConfiguredTextReply, String>`. Keep the existing helper as a compatibility wrapper returning only text. Report the actual resolved provider/model and propagate cancellation/deadlines through every existing provider, including bundled Codex, without creating a second credential path.
- [ ] Build bounded prompts using `summary/context_budget.rs`, meeting titles/dates, distinct transcript/document sections, and only selected evidence. A latest-decision question must retain conflicting dated evidence and qualify incomplete retrieval. Persist user turns before generation; failed requests keep a retryable status and their request identity.
- [ ] Extend version-1 backup optional tables for authoritative knowledge conversations and exclude derived indexes; restore requeues sources. Add round-trip tests with pre-feature archives.
- [ ] On the designated runner, review the design's 12-case synthetic answer set with the configured provider/model: supported facts, absent evidence, changed decisions, project collisions, and short German responses. Require correct source navigation and no unsupported claims; record sanitized outcome metadata only.
- [ ] Run relevant `knowledge::`, summary-provider cancellation/reconciliation tests, Rust checks, and the safety scan; commit `feat: add cited knowledge conversations`.

### Task 4 Archive search and scope UI

**Files:** Create `frontend/src/{types/knowledge.ts,services/knowledgeService.ts,hooks/useKnowledgeSearch.ts,lib/knowledge-state.ts}`,
`components/Knowledge/{SearchResults.tsx,KnowledgeChat.tsx,EvidencePreview.tsx}`,
and `components/KnowledgeSettings.tsx`. Modify `app/meetings/page.tsx`,
`app/_components/SettingsModal.tsx`, and `components/MeetingDetails/MeetingChat.tsx`.
Add `frontend/tests/lib/knowledge-state.test.mjs` and `knowledge-scope.test.mjs`.

**Interfaces:** TypeScript mirrors Task 2/3 DTOs. `knowledgeService` is the only
invoke wrapper. `useKnowledgeSearch` owns request sequence, loading state,
actual retrieval mode, and scope; components render that state. Expose index
progress, Pause/Retry/Rebuild, and model download in Settings.

- [ ] Add helper tests `old_query_cannot_replace_new_results`, `meeting_navigation_discards_late_reply`, `scope_and_project_filter_stay_synchronized`, and `repeated_submit_reuses_request_id`. Assert only the newest response is visible and a single request is sent while pending.
- [ ] Run `pnpm run test` from `frontend` and confirm the new assertions fail before implementation.
- [ ] Add Keyword/Semantic search modes, selected-meeting/project scope, ranked passages, and an Ask about these meetings action. Keep keyword mode usable before model download.
- [ ] Add citation previews with transcript reveal/playback navigation and stale/missing-source messages. Preserve existing meeting history and default scope. Show the configured answer provider and the actual retrieval mode.
- [ ] Implement cancellation, empty/error states, accessible focus/keyboard behavior, dark/light/accent styling, and narrow-layout behavior using existing shared components.
- [ ] Run `pnpm run typecheck`, `pnpm run test`, and `pnpm run build`; complete installed-app offline retrieval and provider-failure smoke on the designated runner. Update `README.md`, `docs/architecture.md`, and `docs/local-library.md` to describe only delivered behavior.
- [ ] Run the safety scan and commit `feat: add meeting knowledge interface`.

**Phase 1 gate:** Search and cited library questions work independently of
documents/live assistance. Pass fresh/upgrade database, mutation, deletion,
backup, resource-preemption, offline, and English/German acceptance cases before
starting Phase 2.

## Phase 2 Reference documents

### Task 5 Bounded document extraction and attachment persistence

**Files:** Create `knowledge/documents/{mod.rs,extract.rs,import.rs,store.rs}`
and `frontend/src-tauri/migrations/20261007000002_knowledge_documents.sql`.
Modify `frontend/src-tauri/src/main.rs` for an early extraction-worker mode,
`Cargo.toml`/`Cargo.lock` for tested parser dependencies and Windows Job Object
APIs, and `knowledge/commands.rs` for document commands.

**Interfaces:**

```rust
struct DocumentBlock { page: Option<u32>, paragraph: u32, text: String }
struct ExtractedDocument { format: DocumentFormat, blocks: Vec<DocumentBlock> }
enum DocumentFormat { Pdf, Docx, Text, Markdown }
async fn import_document(pool: &SqlitePool, runtime: &KnowledgeState,
    meeting_id: &str, selected_path: &Path,
    cancel: CancellationToken) -> Result<DocumentAttachment, KnowledgeError>;
```

`DocumentAttachment` exposes ID, display name, format, size, hash, extraction
status, and indexing status; it never exposes an original filesystem path.
Commands: `knowledge_import_document`, `knowledge_list_documents`,
`knowledge_remove_attachment`, `knowledge_delete_document`, and
`knowledge_get_document_blocks`. Read blocks by document ID and anchor with
backend ownership checks.

- [ ] Add public synthetic PDF/DOCX/TXT/Markdown fixtures and tests `format_mismatch_rejected`, `scanned_pdf_reports_no_text`, `encrypted_pdf_reports_unsupported`, `zip_expansion_limit`, `xml_external_relationship_ignored`, `invalid_utf8_rejected`, `cancelled_import_not_published`, and `shared_document_survives_detach`.
- [ ] Assert every exact file/text/page/decompression limit from the design. Use a deliberately stalled test child to assert timeout/cancellation kills and reaps it and leaves no published attachment. Run and verify failure first.
- [ ] Select and pin a Rust PDF extractor and `quick-xml` plus the existing ZIP implementation after Windows, Rust compatibility, and license checks. Extract actual PDF pages and DOCX paragraphs; do not use a whole-file PDF string with invented page numbers.
- [ ] Implement the early child mode before Tauri single-instance/plugin initialization. Parent validates and copies inputs, applies the hidden-process/512 MiB Job Object boundary, bounds IPC output to 2 MiB, and terminates the child after 10 seconds.
- [ ] Add document metadata, blocks, and meeting-attachment relations. Publish all database rows only after extraction succeeds; queue indexing through Task 2. Removing a relation preserves content attached elsewhere; explicit document deletion invalidates its evidence and dependent turns.
- [ ] Run targeted Rust/parser tests and native cancellation/limit smoke on the designated runner, Rust checks, and the safety scan; commit `feat: add local reference document ingestion`.

### Task 6 Document selection, evidence UI, and portable archives

**Files:** Create `frontend/src/components/MeetingDetails/ReferenceDocuments.tsx`
and `frontend/tests/lib/knowledge-documents.test.mjs`; modify knowledge types,
service, chat, evidence preview, meeting detail composition, Rust retrieval,
answer scope validation, `library/backup.rs`, and the focused documentation.

**Interfaces:** Task 5 supplies attachment IDs and document blocks; Task 2
accepts selected IDs; Task 3 resolves their citations. Add owner-specific
context-sharing settings rather than a new provider credential/configuration.

- [ ] Add tests `unselected_document_not_retrieved`, `foreign_attachment_rejected`, `external_context_requires_enablement`, `replacement_marks_citation_stale`, `backup_v2_document_roundtrip`, and `restore_v1_without_documents`. Assert no provider receives a reference excerpt before the owner enables sharing.
- [ ] Run failing backend/helper tests, then add Attach/Preview/Select/Detach/Retry controls and page/paragraph citation previews. Keep errors and actual indexed status visible.
- [ ] Extend hybrid retrieval to documents after backend attachment-scope validation. Label transcript evidence separately from reference material in prompts and UI.
- [ ] Add version-2 archive attachment-file metadata and safe relative storage references. Preserve version-1 restore, validate hashes/IDs, restore blocks and originals transactionally, and rebuild derived indexes. Never include model files or credentials.
- [ ] Verify offline use, mixed meeting/document answers, malicious document text, replacement/detach behavior, and portable restoration on the designated runner. Run Rust checks and frontend typecheck/tests/build.
- [ ] Update user/architecture/library/backup documentation, run the safety scan, and commit `feat: add reference document context and backup`.

**Phase 2 gate:** Local extraction, explicitly selected context, real anchors,
external-sharing controls, and old/new archive restoration all pass before
starting Phase 3.

## Phase 3 Manual live assistance

### Task 7 Backend live-session requests and cancellation

**Files:** Create `knowledge/{live.rs,live_context.rs}`; modify
`audio/recording_manager.rs`, `audio/recording_commands.rs`, knowledge commands,
answer orchestration, and `lib.rs`. Tests live in `knowledge/live.rs` and
existing recording command test modules.

**Interfaces:**

```rust
struct LiveSnapshot { session_id: String, finalized_through_seconds: f64,
    segments: Vec<LiveEvidenceSegment>, transcription_incomplete: bool }
struct LiveEvidenceSegment { sequence_id: u64, text: String,
    start_seconds: Option<f64>, end_seconds: Option<f64> }
// knowledge_live_snapshot() -> LiveSnapshot
// knowledge_ask(AskRequest with Live owner/scope) -> AssistantReply
```

Assign a new UUID at recording start; clear it at stop. Read finalized segments
from the backend recording manager, never a UI-submitted transcript. Do not
embed live text or add a second capture/transcription pipeline.

- [ ] Add tests `partial_segments_excluded`, `context_uses_last_ten_minutes`, `audio_only_has_no_live_context`, `local_builtin_rejected_during_capture`, `old_session_reply_discarded`, `stop_does_not_wait_for_provider`, and `pending_index_cannot_delay_stop`.
- [ ] Assert a new session UUID on each recording, one request in flight, a 30-second request deadline, and no persisted live history after stop. Fake a never-ending provider to prove stop only signals cancellation and never awaits its completion. Run and observe failures.
- [ ] Implement snapshot scoping, current-session validation, bounded lexical live retrieval, and optional explicitly selected saved references. Report the latest finalized time and incomplete-transcription status in replies.
- [ ] Require owner-specific live text-sharing enablement before external provider dispatch. Built-in AI offers a post-recording action rather than bypassing the job gate. Pause document extraction and index/model work when capture starts.
- [ ] Invalidate session/request tokens on stop and recheck them before delivery. Retain native inference permits until calls finish; never make recording finalization depend on assistance completion.
- [ ] Run recording stop/start, job arbitration, provider cancellation/reconciliation, and knowledge tests plus Rust checks and the safety scan; commit `feat: add recording-scoped live assistance`.

### Task 8 Live panel and recording acceptance

**Files:** Create `frontend/src/{components/LiveAssistancePanel.tsx,contexts/LiveAssistanceContext.tsx,hooks/useLiveAssistance.ts,lib/live-assistance-state.ts}`
and `frontend/tests/lib/live-assistance-state.test.mjs`. Modify Home route
composition, settings, knowledge service/types, and existing shared context
integration only where needed.

**Interfaces:** One provider/context owns live requests across route/panel
changes. Use Task 7 session IDs and snapshot/reply DTOs; reuse existing
recording/transcript context state and cleanup patterns.

- [ ] Add tests `route_change_does_not_duplicate_listener`, `stop_clears_live_messages`, `second_session_cannot_receive_first_reply`, `repeated_action_sends_once`, and `backlog_timestamp_stays_visible`. Run the new tests and confirm failure before implementation.
- [ ] Add a collapsible panel with Ask, Summarize so far, List open questions, Cancel, selected references, provider, and finalized-transcript time. Make live sharing opt-in and distinguish provider/resource errors from recording health.
- [ ] Add unavailable states for audio-only mode and Built-in AI while recording. Keep focus, keyboard, dark/light/accent, and narrow-layout behavior consistent with existing recording controls.
- [ ] Run frontend typecheck/tests/build and relevant Rust tests on the designated runner. Exercise provider failure, cancellation, navigation, repeated Stop, and a second recording.
- [ ] Perform a release-like two-hour dual-source smoke with features enabled and a comparable disabled baseline. Record engine/model/language/backend, memory trend, transcription elapsed time/backlog, search/answer times, stop timing, saved audio playback, and second-start behavior. Accept only if assistance creates no capture gaps/unbounded memory and Stop never waits on its backlog.
- [ ] Update `README.md`, architecture/meeting-quality documentation and the acceptance checklist with delivered behavior and measured limitations. Run the safety scan; commit `feat: add manual live assistance panel`.

## Implementation and release boundaries

Recommended execution is sequential, one task and validation cycle at a time.
Phase 1 must clear its resource gate before document or live work starts.
Finish each phase with an independent behavior review; interfaces and recording
lifecycle need particular attention.

Use the smallest relevant tests while iterating. Before each Rust commit, run
format checks, compilation, and targeted tests; before UI commits, also run
frontend typecheck/tests/build. Keep native diagnostics and installer checks on
the designated trusted runner. Run the public-repository safety scanner before
every commit and preserve a clean worktree.

This plan does not schedule a release or version change. A separately requested
release must follow `docs/windows-release.md`, including the exact-code capture
smoke, metadata-only release commit, `-CheckOnly`, and updater acceptance.
