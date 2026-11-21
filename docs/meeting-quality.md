# Meeting Quality And Lower-Power Notebooks

ClawScribe's goal is to preserve what was said, then produce concise notes that
can be checked against it. Transcription, transcript presentation, and summary
generation are separate stages; improvements to one are not a measured accuracy
improvement to every model or provider.

## Capture First

HE-AAC imports use the first decoded buffer's sample rate, which can differ
from the container's rate. Duration and conversion to 16 kHz use that rate.
The VAD's final padded frame is trimmed to the actual input endpoint, including
when a long segment splits during flush. Padding does not add audio or extend
the last transcript segment beyond the source.

Select the microphone and the output device actually used by the meeting app.
Verify both sources in a short saved recording before relying on a long session.
Call detection is optional. With the correct output device selected, manual
recordings can capture audio from any meeting app.

On Windows, recording uses the exact name of each selected device. If a device
disappears, ClawScribe reports the missing source. If two devices have the
same name, rename one in Windows Sound settings before recording.

The Settings microphone test is independent of recording capture. On Windows,
an unavailable preferred microphone falls back to the current default, with the
actual tested device and fallback shown beside its levels. Saved preferences
remain unchanged.

The recorder counts each stretch of at least 15 seconds without audible
system samples while the selected output is connected. Pausing resets the timer.
The saved meeting shows these stretches as quiet periods. Capture warnings
track separate evidence such as device failures, stalls in a previously
responsive microphone stream, and dropped samples. Confirm microphone and
meeting audio in saved playback.

The recorder watches the selected input and output during capture, including
while the main window is hidden or reloading. It tries to restore the same
missing source while the other stream continues. Automatic checks begin after
12 seconds and back off to at most one minute between failed checks. A device
returning can trigger an earlier check, and **Retry** requests an immediate one.
Pause, bookmarks, notes and calendar review remain available during a check.
The live health display shows speech waiting or processing, free space for the
recording and transcription scratch volumes when known, and the backend observed
during inference. Whisper reports its runtime backend. Parakeet reports its
loaded CPU or DirectML/CPU session. Nemotron reports its encoder and decoder
backends. CPU fallback is shown when reported by the engine. Mixer overflow,
capture-queue loss and source disconnects can leave gaps. The saved meeting
flags those gaps even after capture reconnects successfully.

Speak with the microphone at a usable level, avoid clipping, and use the intended
language/model selection. Playback is the reference when a name, number, short
answer, or overlapping utterance needs review. A confidence value is not proof
that a segment is correct or incorrect.

Microphone loudness normalization limits positive gain to about 18 dB so quiet
room noise or far-end bleed cannot drive large amplification. During long quiet
periods, speech detection retains only the recent audio needed to recognize the
next voice and include its pre-roll. During uninterrupted speech, it releases
PCM as each forced segment is emitted and preserves the model's state across
the cut. Recording, import, and retranscription use the same sample clock for
the cut and its remaining audio.

## Testing local transcription speed

In **Settings → Transcription**, select an installed Whisper, Parakeet or Nemotron model
with **Use**, then choose **Test this PC**. With a hosted file-transcription
provider selected, the test targets the local Parakeet Lightning model. The benchmark
shares the recording and transcription job gate. Run it while those jobs are
idle. It uses two synthetic, local speech samples (German and English) and
reports inference time separately from model loading.

The model list marks the fastest tested variant when at least two installed
models of that engine recognized speech in both samples on the same confirmed
backend. Tied results have no winner. A speed comparison does not measure
word accuracy. Whisper's recommended starting model is Large V3 turbo, its
catalog default.

Compare processing time with the sample duration. Taking longer than the
audio means a stream of similar speech would accumulate a backlog. The result
also identifies samples that produced no text. For slow results, try a
smaller installed model or select **Record now, transcribe later**. Short
sample timing measures model speed. Meeting duration, other apps, audio
capture, language and overlapping speakers affect live performance.

For development, the [two-source capture benchmark](local-capture-benchmark.md)
measures capture processing, transcript quality and saved audio on the same
fictional meeting timeline.

## Live Transcript Behavior

With **Auto** selected, ClawScribe compares each speech segment with the
microphone and system-audio levels captured over the same interval. Only
segments clearly attributable to one source can establish its language.
Overlapping speakers are still transcribed from the mix.

Whisper checks the language acoustically on longer turns. Two credible
observations on the same source establish a language or confirm a switch.
Short replies can then use the recent language for that source. For example,
the microphone can remain German while remote participants switch to English.
The live transcript displays the confirmed languages. Multilingual Parakeet
v3 and Ultra estimate them from finalized text and keep using their normal
decoder. Fixed-language and translation modes retain the user's choice.
Starting a new recording clears both language histories.

The transcript view preserves recognized words, including words that resemble
English fillers in other languages. It does not silently remove German `er` or
`um`. Concision belongs in the notes rather than an unmarked edit of the source
transcript.

Whisper also preserves repeated words and phrases such as "I had had enough"
or an actual mention of applause. Its final text normalization only collapses
whitespace. This avoids deleting legitimate speech; it does not prove a model's
output is correct. Check suspected hallucinations against the saved audio.

Use a timestamp button to play the associated audio. Scroll back to review
previous text without being pulled to the newest segment; **Jump to live
transcript** resumes following during recording. Speaker-name edits report
failures visibly and retain custom input for retry. A manually assigned speaker
label is not independently verified speaker identity.

Refreshing transcript rows does not interrupt the display of an arriving
utterance; stopping shows its complete text. Reloading the interface restores an
active recording's state. Older polling responses cannot reverse a newer pause
or stop event.
Live transcription failures produce one user-facing failure notice per recording;
later failures do not repeat it. Live wall-clock timestamps use local time.

The **Live assistance** preview panel uses finalized backend speech from the
context window, a fixed span of up to ten minutes ending at the finalized-through
time. It is shorter early in a recording or when the segment or size limit
applies. **Summarize the last 10 minutes** summarizes that bounded context, which
may omit earlier discussion and recent untranscribed speech.
Current finalized-through time remains visible while collapsed, and each answer
shows its own timestamp and incomplete status. Assistance/provider errors are
separate from the recording health banner; they do not certify or change capture
health. Review generated claims against the transcript and saved audio.

No answer runs automatically. Live transcript sharing is remembered but off
until explicitly enabled. Selected saved references and per-recording document
sharing are independent controls. Cancel hides late replies, and first Stop
clears temporary messages without waiting for assistance. After saving, use
**Open saved meeting** and its chat or transcription controls. Built-in AI cannot
run a live answer during capture. Ollama runs separately and can contend for
memory and compute; reduce or cancel assistance when transcription falls behind.

Exact-build installed navigation/cancellation, provider failures, keyboard and
theme checks remain pending. A release-like two-hour microphone/system-audio
recording with Live enabled and a comparable disabled baseline is also pending.
Record engine/model/language/backend, memory trend, transcription elapsed time
and backlog, search/answer times, Stop time, saved playback and second-start
behavior. Fixtures do not measure capture gaps, notebook performance or playable
audio. A clearly labeled preview may precede these checks under the Windows
release policy; stable readiness requires the real-device evidence.

Nemotron's **Auto** uses the system locale for its language prompt, including
imports and retranscription. It does not detect the spoken language or translate;
choose the spoken language explicitly when it differs from the system setting.
Parakeet Auto does not record English as the source language without evidence.

With audio saving enabled, captured mixed audio is staged in one-second batches
under the meeting's `.audio-spool` folder. New recordings do not launch periodic
checkpoint encoders or create `.checkpoints` folders. Older checkpoint recordings
can still be recovered. Independent disk workers have bounded memory queues: up to 60 capture
batches and eight speech segments. Temporary queue pressure or spool-write
failures mark the meeting incomplete and keep capture running. Publication
retries transient file locks five times. Samples that never reach disk cannot
be recovered, and a crash can lose pending in-memory buffers.

The raw spool uses approximately 700 MB per hour in addition to final audio. Prefer a local recording folder with enough free
space; syncing that folder with OneDrive can add disk contention. Originals
remain until all readable raw chunks are encoded, final audio is published, and
warning status is saved. Transcript or metadata failures remain visible but do not
retain raw audio that cannot repair those files. A transient stopped-metadata
write failure leaves no file warning if the final write repairs it. Capture gaps have their own persistent warning and do not keep
an otherwise fully encoded spool forever. A readable temporary chunk at the next sequence position is included in final
audio and recovery. A torn temporary tail records a gap without retaining the
spool; unreadable published chunks and originals needed after a failed save
are retained.

Stopped duration and completion time are saved before the final encode. A crash
during encoding leaves a stopped, unfinished meeting available to recovery.
Final audio is encoded once from available raw chunks, avoiding repeated AAC
priming at checkpoint joins. Missing or unreadable chunks do not prevent saving
the rest. The encode deadline scales with recording duration (one fifth of the
duration plus one minute, bounded between four minutes and one hour); the outer
save deadline allows two additional minutes for publication and fallback work.
If raw encoding fails, existing legacy checkpoints are tried and originals are
retained. New sessions rely on their raw spool for recovery.

Older unsaved transcripts remain available in **Interrupted recordings**, including
transcripts held only in the app's local recovery cache. A recording already
saved to the library or currently being captured is excluded from cache recovery.

Recovery accepts recordings without a transcript: recover the saved audio first,
then use Transcribe in the saved meeting. If audio recovery fails and there is no
transcript, the recovery entry and originals remain available for retry. Recovered
live transcripts are marked incomplete and offer Retranscribe, since queued speech
may not have been transcribed before the crash. Audio-only recoveries keep the
normal Transcribe flow.
Recovery includes an unconsumed tail. After library save or successful retranscription,
fully recovered raw audio is released once the recovered file is validated and
capture warnings are saved. Unreadable or missing chunks keep their originals. Opening a recovered
meeting reuses its completed audio; an explicit recovery action can rebuild it.
Retranscription checks retained chunks and saved warnings for gaps, including
when reusing a recovered file, and preserves that information in the library.
Recovery uses compact AAC when FFmpeg is available; WAV remains the fallback
if the encoder is missing or encoding fails, within its 32-bit size limit. Existing recovered WAV files
remain supported.
Temporary legacy checkpoints must decode successfully before recovery uses them.
Playback uses the browser's streaming audio element through scoped local-file
access rather than decoding the whole recording into webview memory.

Stop has one shared owner across the app, tray and shortcut. Recording duration
is frozen before transcription drain, and device-error limits apply to a recent
30-second window. A background worker writes transcript snapshots during silence
and once more before audio finalization. The core saves the finalized transcript
and outcome directly to the library. If the library write fails after the files
are saved, open **Interrupted recordings** to finish saving the meeting.
Stop gives the capture pipeline up to 45 seconds to flush, then allows up to
five more seconds for stream release. If the pipeline flush completes, a slow
driver release is recorded in diagnostics. A driver busy reconnecting has a
separate 20-second wait when Stop is requested. Live transcription gets up to
two minutes to drain and five
seconds to cancel. Saved audio remains available for manual retranscription
when speech segments are unfinished. Live capture and Stop use local
transcription even when a hosted file provider is configured.

An empty live transcript reports whether the speech detector produced no
segments, the engine returned no text, or the session was incomplete. The
audio-only recording flow still offers **Transcribe now**.
Transcript, audio and metadata writes each get an independent attempt; failures
are reported together. Each recording has a stable library ID. Retries reuse it,
and library deletion leaves retained recording files out of recovery. A failed
microphone or system-audio stream can be retried while the other source keeps
recording. The capture-gap warning stays with the saved meeting. Failed starts
stop their background workers; empty owned folders are removed, while possible
recovery data is retained with an error status.

The capture channel is bounded. Live recognition has a separate disk queue,
and native inference runs outside the async executor. Stop ends capture before
waiting for a bounded recognition drain. If work remains, the meeting is marked
incomplete and the saved audio can be retranscribed. Whisper cancellation uses
its native abort callback; Nemotron checks between streaming windows. A Parakeet
native call may finish after cancellation, retaining its model/permit and
blocking another job until it returns.

Audio-save failures, capture gaps, incomplete recording files and incomplete
transcription are saved with the meeting and in its recording folder.
Automatic notes pause after a failed audio save or incomplete transcription.
Capture gaps stay visible when notes use the available transcript.
Successful retranscription clears audio-save and
transcript failure flags. Recording-file warnings clear after the transcript,
metadata, and recording status are written. If a retranscription folder write
fails, the new transcript is available in the library and **Retry file update**
stays available after restarting. A successful file update lets approved notes
start from that transcript. Empty retranscription is rejected before replacement.
A successful replacement retains the prior transcript as a database revision.
Permanent capture gaps remain visible after retranscribing the available audio.

## Reviewing a saved meeting

Select a transcript timestamp to hear that passage. Dragging the playback bar
changes the position and keeps the current play or pause state.

- **Speed** (0.5× to 2×, pitch preserved). The chosen speed is remembered in this
  app's browser storage and applies to the next recording you open.
- **Follow playback** keeps the passage being played in view. It scrolls only when
  the spoken passage changes and turns itself off when you scroll the transcript by
  hand, so it never fights the reader. It is on by default and applies while audio plays.
- Bookmarks are drawn on the timeline and seek when chosen.

In **Settings → Shortcuts → Saved meeting player**, select an action and press
a key or key combination. Each seek distance and speed direction has its own
binding. The player remembers changes on this device and lists the current keys
under **Playback shortcuts**. **Reset player shortcuts** restores the defaults
below. Global recording shortcuts have their own reset.

Player shortcuts work while the saved meeting is open. Text fields, the summary
editor, dialogs and menus keep their keyboard controls. Space still activates a
focused button, and the arrows still move a focused slider.

| Default key | Action |
| --- | --- |
| Space | Play or pause |
| Left, Right | Back or forward 5 seconds (hold Shift for 15) |
| `<`, `>` | Slower or faster |
| F | Follow playback |
| B | Add a bookmark at the playback position |
| Ctrl+F | Find in the transcript |

**Find in transcript** searches the complete saved transcript, not only the loaded
pages, ignores case, and treats the text literally. Matches are marked in the text;
Enter and Shift+Enter move to the next and previous passage (wrapping at the ends)
and Escape closes the bar. The count is of passages, not occurrences.

**Copy** for the summary and the transcript puts plain, readable text and an HTML
version on the clipboard. Neither contains Markdown markup, source-link anchors,
the internal meeting id or the time of copying; the summary starts with the title
and date, the transcript with the title, date and one line per passage with its
time and speaker. The HTML uses a fixed set of formatting tags, escapes all text
and links only to http and https addresses. Where the platform refuses rich copy,
the plain text is still copied. The Confluence export keeps its own format.

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

Native speech-model loading runs outside the async executor. Import and
retranscription pass each bounded speech buffer into inference without making
another full copy. MAI uses a 64 KiB file-read buffer for multipart audio.
Conversion uses bounded copies into a temporary WAV, which is limited to
300 MB before upload. For two hours at 16 kHz mono PCM16, allow about 230 MB
of temporary disk space. These are buffer and file-size bounds. FFmpeg, HTTP
transport, and transcript responses have their own memory needs. Cancellation
stops conversion and can drop an in-flight upload or retry delay.

The temporary WAV is an unnamed file the operating system deletes when the
process closes it or dies. A manual, ignored test on the designated runner
measures the conversion and upload stream with synthetic audio only
(`CLAWSCRIBE_MAI_PERF_SECONDS`, 10 to 3600, default 600): it prints elapsed time,
peak memory delta of the app process (FFmpeg is separate), source seconds,
output bytes and cancellation latency for the current path, the earlier
named-file path and the in-memory decode baseline.
Select `measure-mai` when manually dispatching Windows validation to run the
ten-minute synthetic comparison with the release-profile test binary and staged
encoder. The fixture makes no provider requests.

For an i5-1235U / 8 GB notebook, choose models based on a measured session with
Teams/Webex and the normal concurrent workload. Compare sustained backlog,
recognition quality, responsiveness, memory pressure, and finalization time.
No target-notebook benchmark is implied by unit tests. Cloud processing is an
optional, explicitly configured privacy trade-off, not an automatic performance
fix that should be enabled without the user's consent.

## Speaker Detection

Speaker detection shares the exclusive job/engine gate, so it cannot compete
with capture or transcription. It decodes and resamples through the streaming
batch decoder. Only the 16 kHz mono float buffer is held in full: about 230 MB
per hour of audio, plus a temporary file of similar size during preparation.
The original-rate audio is not held in memory. Models and native analysis need
additional memory; long-recording detection still needs measurement on an 8 GiB
notebook.

Detection temporarily blocks transcript corrections, undo, restore, and speaker
renames for that meeting. Before saving labels it checks that the transcript has
not changed; otherwise it preserves existing edits and asks you to run detection
again. The previous transcript and speaker labels remain available through
**Restore previous transcript**.

Cancel from the detection dialog or meeting toolbar, including after continuing
in the background. Cancellation discards pending labels and waits for the current
native step before another recording can start. When detection blocks recording,
the error offers **Cancel speaker detection and record**.

If labels save to the database but the recording-folder transcript copy cannot
be updated, the app reports a pending file warning. **Retry file update** writes
the saved database transcript when that folder is available again.

### Measuring speaker detection

Keep evaluation audio, models and generated results outside the repository.
Arrange clips in one directory per speaker, using anonymous directory names
without spaces; each directory must contain 16 kHz mono PCM16 WAV files.
Use speech-only clips: every excerpt is labeled as speech by that speaker.
Convert other formats separately with
`ffmpeg -i in.mp3 -ac 1 -ar 16000 -sample_fmt s16 out.wav`.
From the repository root, build repeatable mixtures and reference RTTM labels:

```powershell
node scripts/build-diarization-eval-set.mjs --clips "<speaker-clips-directory>" --out "<evaluation-directory>" --count 5 --speakers 4 --minutes 5 --seed 1 --overlap 0.05 --language de
```

`--gap-min`/`--gap-max` default to 0.2/1.5 seconds and
`--turn-min`/`--turn-max` to 2/12 seconds; every clip must be at least the minimum
turn length. Turn boundaries use whole milliseconds (16 samples), so the
three-decimal RTTM labels are sample-exact. Overlaps are summed with a shared
gain reduction when needed to prevent clipping. The last turn may be truncated.
Reusing the clips, options and seed reproduces the set; language defaults to
`und` if omitted.

Run the ignored Rust benchmark from the repository root with existing models:

```powershell
$env:CLAWSCRIBE_DIARIZATION_EVAL_DIR = "<evaluation-directory>"
$env:CLAWSCRIBE_DIARIZATION_SEGMENTATION_MODEL = "<segmentation-model.onnx>"
$env:CLAWSCRIBE_DIARIZATION_EMBEDDING_MODELS = "nemo-titanet-small-en=<embedding-model.onnx>"
$env:CLAWSCRIBE_DIARIZATION_EVAL_PROVIDER = "cpu"
$env:CLAWSCRIBE_DIARIZATION_EVAL_OUT = "<results-directory>"
cargo test --lib diarization::eval::diarization_accuracy_benchmark -- --ignored --nocapture
```

Embedding models are comma-separated `catalog-id=path` pairs; paths may be
absolute or relative to the working directory and must not contain commas.
IDs and default clustering thresholds come from `EMBEDDING_MODEL_CATALOG` in
`diarization.rs`. CPU is the default provider. Output is printed; the optional
output directory also receives `results.json` and `results.md`. With no evaluation
directory configured, the benchmark prints a skip notice and returns.

Existing datasets can supply `manifest.json` with an `items` array; each item
requires `name`, `audio`, `rttm`, `language` and `num_speakers` (1–12 or `null`).
The optional string `recording` defaults to `name` and selects the recording id
in the RTTM file, including combined files containing multiple recordings.
Audio and RTTM paths must be relative files inside the evaluation directory.
The benchmark uses the production decoder and prepared turn pipeline. A known
speaker count runs both auto and known-count modes; `null` runs auto only.
Models and their catalog thresholds are compared on identical data, with
unweighted mean DER per model and per language, separately for each mode.
Timing includes model loading, inference and turn preparation, excluding decode
and scoring; it is an accuracy benchmark, not a long-meeting performance claim.

DER is missed speech plus false alarm plus speaker confusion, divided by scored
reference speaker-seconds after optimal one-to-one speaker matching.
Miss measures reference speaker time with too few detected speakers.
False alarm measures detected speaker time in excess of the reference count.
Confusion measures detected speech attributed to the wrong mapped speaker.
The scorer excludes ±0.25 seconds around every reference boundary by default,
counts overlapped speech per speaker, and supports excluding reference overlap
in its unit-tested options; zero scored speech with non-zero errors is rejected.

Synthetic mixtures of studio-quality clips are an optimistic, relative
comparison: they do not capture room acoustics, codecs or crosstalk. Real German
speech and a long mixed-source meeting remain required quality cases.

## Summary Reliability

Completed `<think>` and `<thinking>` blocks are removed from model output,
including case and attribute variants. Unfinished or orphaned tags fail with
an actionable retry error. Generated structured notes receive the same checks.
Saving edited summaries rejects reasoning markup, and loading older affected
summaries hides them with a regeneration message without deleting their stored
copy. Source evidence remains unchanged.

Automatic notes use an already-configured provider and wait for the saved
summary lookup before deciding whether generation is needed. Switching meetings
discards old summary and transcript loading results. Title-only saves leave notes
unchanged, and save failures retain drafts for retry; edits made during a write
remain available for the next save.

Manual edits keep structured follow-ups supplied by the summary provider and
remove any corresponding actions deleted from the notes. **Add summary
suggestions** brings the remaining follow-ups into **Meeting actions** for review.
Removing a suggestion and saving the actions keeps it dismissed. Saving an
action with an available transcript reference keeps that passage reference with
the action, including after summary regeneration. The
app flags passages that changed and reports when their transcript was replaced.

The shared summary processor now advances chunks from their actual emitted
boundaries, rejects failed or empty chunk results, and stops rather than presenting
an incomplete multi-chunk result as successful. It rejects empty transcript input.
Extraction/combination/report instructions preserve uncertainty and negation,
distinguish proposals from decisions, and forbid invented owners and deadlines.

The shared, compatible-API, and bundled Codex routes all reduce long inputs
before their final prompt and reject incomplete reductions. Budgets reserve
output and prompt overhead, use UTF-8 bytes conservatively, and limit reduction
to eight passes. Built-in AI and Ollama context is capped at 16,384 tokens.
Direct cloud providers use model-specific budgets. Unknown cloud models default
to 32,768 tokens. OpenClaw and compatible endpoints default to 8,192 and honor
a configured context size. The bundled Codex route uses 500,000 tokens for
GPT-6.1 Sol and 32,768 for other models. The architecture guide lists default
output budgets and how much input each stage can use. [Summary context budgets](architecture.md#summary-context-budgets).
These limits may increase reduction requests; they do not prove factual accuracy.

Summary HTTP providers make at most three attempts for transient connection or
server errors. They do not retry reported quota exhaustion, a response timeout,
or a Retry-After longer than 60 seconds. Backoff can be cancelled. Codex meeting
turns send an output schema; missing or null follow-up emails render as no email
suggested rather than failing the summary.

Provider responses that report reaching an output or context limit are rejected
as incomplete. Reduce the requested report or increase a supported output/context
limit before retrying. API responses are capped at 8 MiB, and cancellation remains
active while their response body is arriving. Claude text responses retain all
returned text blocks rather than only the first.

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
