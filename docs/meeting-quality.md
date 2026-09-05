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

Live recognition uses a disk-backed queue so slow inference does not retain the
entire recognition backlog in RAM. This does not make inference faster than real
time, cap every audio/persistence queue, or eliminate disk-full risk. Allow
queued transcription to finish before treating the transcript as complete.

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

## Summary Reliability

The shared summary processor now advances chunks from their actual emitted
boundaries, rejects failed or empty chunk results, and stops rather than presenting
an incomplete multi-chunk result as successful. It rejects empty transcript input.
Extraction/combination/report instructions preserve uncertainty and negation,
distinguish proposals from decisions, and forbid invented owners and deadlines.

These changes apply to the shared processor; dedicated OpenAI-compatible and
Codex processing routes have separate implementations. They do not prove that
all providers have identical validation or context-budget behavior. Provider-wide
context budgeting, resumable reduction, and evidence-linked actions remain
separate engineering work.

Before exporting, check decisions, dates, names, quantities, owners, and deadlines
against source audio/transcript. Missing information should remain unspecified.
Suggested next steps must not be presented as agreed commitments. Prompt rules
reduce avoidable mistakes but cannot certify hallucination-free output.

## Credential Scope

Microsoft refresh-token file fallback on Windows is protected with current-user
DPAPI when the platform credential store cannot persist it. Access tokens are
not stored by this persistence path. This is not encryption of all recordings,
transcripts, logs, or databases; protect the Windows account and storage as well.

See [Windows release acceptance](windows-release.md#required-real-device-acceptance)
for the checks needed before recommending a build for everyday meetings.
