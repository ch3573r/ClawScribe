# Meeting Quality And Lower-Power Notebooks

ClawScribe's goal is to preserve what was said, then produce concise notes that
can be checked against it. Transcription, transcript presentation, and summary
generation are separate stages; improvements to one are not a measured accuracy
improvement to every model or provider.

## Capture First

Select the microphone and the output device actually used by the meeting app.
Verify both sources in a short saved recording before relying on a long session.
Teams detection is optional and distinct from audio capture; Webex and other
applications can be recorded through system audio without a dedicated meeting bot.

Speak with the microphone at a usable level, avoid clipping, and use the intended
language/model selection. Playback is the reference when a name, number, short
answer, or overlapping utterance needs review. A confidence value is not proof
that a segment is correct or incorrect.

## Live Transcript Behavior

The transcript view preserves recognized words, including words that resemble
English fillers in other languages. It does not silently remove German `er` or
`um`. Concision belongs in the notes rather than an unmarked edit of the source
transcript.

Use a timestamp button to play the associated audio. Scroll back to review
previous text without being pulled to the newest segment; **Jump to live
transcript** resumes following during recording. Speaker-name edits report
failures visibly and retain custom input for retry. A manually assigned speaker
label is not independently verified speaker identity.

With audio saving enabled, captured mixed audio is staged in one-second batches under the meeting's
`.audio-spool` folder while AAC checkpoints are encoded separately. Accepted
spool chunks remain until final audio and metadata are saved successfully.
Recovery reads the spool in sequence, includes an unconsumed tail, and retains
originals. A process crash can still lose the in-memory fraction of the current
second. Full disks or capture overrun stop capture with a visible warning; they
cannot guarantee recovery of samples that never reached disk.

The capture channel is bounded. Live recognition has a separate disk queue,
and native inference runs outside the async executor. Stop ends capture before
waiting for a bounded recognition drain. If work remains, the meeting is marked
incomplete and the saved audio can be retranscribed. Whisper cancellation uses
its native abort callback; Nemotron checks between streaming windows. A Parakeet
native call may finish after cancellation, retaining its model/permit and
blocking another job until it returns.

Audio-save failures and incomplete transcription are saved with the meeting and
in its recording folder. Automatic notes are withheld for these meetings. Empty
retranscription is rejected before replacement, and a successful replacement
retains the prior transcript as a database revision. Capture-loss warnings remain
conservative: retranscribing available audio cannot restore missing samples.

## Notebook Resource Policy

The hardware helper measures total physical RAM, not current free RAM. A valid
positive `MEMORY_GB` override remains supported for diagnostics; invalid values
fall back to measured memory. Detection is cached with the hardware profile.

On Windows, adaptive Whisper inference uses at most four threads when the
reported memory value is at most 8 GiB, otherwise at most eight. It also limits
threads to one fewer than available logical parallelism where possible, with a
minimum of one. This is not a process-wide limit, a CPU reservation, or a budget
for Parakeet, Nemotron, or external summary providers. Do not claim universal
real-time performance from this setting alone.

For an i5-1235U / 8 GB notebook, choose models based on a measured session with
Teams/Webex and the normal concurrent workload. Compare sustained backlog,
recognition quality, responsiveness, memory pressure, and finalization time.
No target-notebook benchmark is implied by unit tests. Cloud processing is an
optional, explicitly configured privacy trade-off, not an automatic performance
fix that should be enabled without the user's consent.

Speaker diarization shares the exclusive job/engine gate, so it cannot compete
with capture or transcription. Its current offline analysis still materializes
the decoded recording and needs more memory than the bounded ASR preparation
path. Long-recording diarization on an 8 GiB notebook needs separate measurement
and a future bounded analysis path before it can be treated as low-memory work.

## Summary Reliability

The shared summary processor now advances chunks from their actual emitted
boundaries, rejects failed or empty chunk results, and stops rather than presenting
an incomplete multi-chunk result as successful. It rejects empty transcript input.
Extraction/combination/report instructions preserve uncertainty and negation,
distinguish proposals from decisions, and forbid invented owners and deadlines.

The shared, compatible-API, and bundled Codex routes all reduce long inputs
before their final prompt and reject incomplete reductions. Budgets reserve
output and prompt overhead, use UTF-8 bytes conservatively, and limit reduction
to eight passes. Unknown API limits default to 8,192 tokens; compatible provider
settings can specify the actual context. Built-in and Ollama inference cap the
requested context at 8,192 tokens to limit local memory use. These limits may
increase the number of reduction requests; they do not prove factual accuracy.

Meeting chat ranks excerpts across the entire transcript using the question,
includes nearby context and timeline coverage, and labels when only selected
excerpts fit. Lexical retrieval can miss synonyms or dispersed evidence. The
assistant is instructed to cite timestamps and acknowledge insufficient evidence;
review broad conclusions against the full transcript.

Before exporting, check decisions, dates, names, quantities, owners, and deadlines
against source audio/transcript. Missing information should remain unspecified.
Suggested next steps must not be presented as agreed commitments. Prompt rules
reduce avoidable mistakes but cannot certify hallucination-free output.

## Credential Scope

Summary and transcription API keys use provider-specific OS credential-store
references. On Windows, current-user DPAPI protects fallback bytes. Existing
plaintext settings are migrated before use; a failed migration disables that
provider until protected storage is available. Microsoft authentication remains
independent. SQLite owns WAL recovery and checkpointing; ClawScribe does not
delete WAL/SHM files to repair a failed open. Secure deletion is enabled for
updated SQLite values, but old backups and storage snapshots are outside this
migration. Recordings and transcripts remain local files, not encrypted vaults.

See [Windows release acceptance](windows-release.md#required-real-device-acceptance)
for the checks needed before recommending a build for everyday meetings.
