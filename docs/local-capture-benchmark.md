# Local capture benchmark

Run this fixture benchmark on the designated self-hosted Windows runner for
Windows validation. Use a local, release-profile build, FFmpeg, an installed
Parakeet int8 model, Python 3, and eSpeak NG 1.52.0 to generate the four short
replies. The two longer fictional excerpts are bundled with the transcription
speed test. Use a fresh fixture directory.

~~~powershell
$fixtures = Join-Path ([System.IO.Path]::GetTempPath()) ("clawscribe-fixtures-" + [guid]::NewGuid())
python frontend/scripts/make-capture-bench-fixtures.py $fixtures
$env:CLAWSCRIBE_AUDIO_BENCH_FIXTURES = $fixtures
$env:CLAWSCRIBE_AUDIO_BENCH_MODEL = "<installed Parakeet int8 model directory>"
cargo test --release -p clawscribe --lib two_source_capture_quality -- --ignored --nocapture
~~~

Use Parakeet v3 or Ultra for German and English. Compact (v2 int8) is English
only. The benchmark runs CPU inference and reports the model variant and load
time. It runs offline in a temporary folder, checks free space before capture,
and prints aggregate metrics. Use the generated fixtures or other fictional
synthetic replies.

## What it measures

The fixture feeds paired 10 ms blocks into the production capture callbacks.
It uses the microphone high-pass filter and EBU R128 normalizer, the real
mixer and disk queues, the live Silero VAD, and the same Parakeet decoder and
blank retry used for local transcription. Direct decoder calls exclude startup
and IPC overhead from the production transcription worker. It runs a second decode from the
captured AAC using the saved-meeting audio preparation path.

The fixture covers quiet background and simulated bleed, isolated German and
English passages, two minutes of digital silence, four short replies, and
overlapping speech. It reports word error rate and per-passage errors, blank
segments, exact short replies, contiguous boundaries, and each fixture
bookmark's distance from the corresponding transcript interval. The mixed
overlap row reports word count because the two speakers have no single reference
word order. Capture metrics include callback time, sampled resident memory, raw
spool bytes and duration, sampled queued audio, and stop-to-last-decode time.
The saved pass reports AAC bytes and separate encode, preparation and inference
times.

Use the ratio of captured to input duration only to associate recognizer output
with the known fixture passages when regular mixer padding shifts the timeline.
Bookmark distance still uses the original fixture clock. Check the reported
clock drift directly and use saved playback to check word-level timing.

## Comparisons

Set only the options under comparison. Remove environment overrides before the
next baseline run. Each run creates fresh temporary outputs.

| Environment variable | Effect |
| --- | --- |
| CLAWSCRIBE_AUDIO_BENCH_PACED | Deliver the fixture in real time. Fast replay is useful for deterministic quality and stress tests. Use paced results for queue and Stop timing. |
| CLAWSCRIBE_AUDIO_BENCH_SILENCE_SECONDS | Set the silent interval from 2 to 7,200 seconds. Use paced input and enough temporary storage for sustained performance checks. |
| CLAWSCRIBE_AUDIO_BENCH_UNCAPPED | Compare gain against the earlier uncapped normalizer. |
| CLAWSCRIBE_AUDIO_BENCH_CAP_MS | Force shorter live cuts, for example 2,000 ms. The Parakeet live default is 12,000 ms. |
| CLAWSCRIBE_AUDIO_BENCH_MIN_SPEECH_MS | Test a shorter live speech confirmation duration, for example 120 ms. |
| CLAWSCRIBE_AUDIO_BENCH_SOURCE | Run with only mic or system audio. Compare both against the same mixed fixture to measure separate-track decoding. |
| CLAWSCRIBE_AUDIO_BENCH_TARGETED | Replay each bounded short-reply window from saved audio with and without an extra silence lead. Reports exact matches without editing a transcript. |
| CLAWSCRIBE_AUDIO_BENCH_AAC_KBPS | Encode the original spool at 64, 96, 128, or 192 kb/s. The recording default is 192 kb/s. |

## Portable fixture observations

The following observations come from a single unpaced Linux CPU comparison
on 21 November 2026 with the audio code in `628b67f7`, Parakeet v3 int8
and the built-in fictional passages. It used
synthetic callback injection with desktop/device services substituted.

- Capping gain reduced startup background RMS from 0.07074 to 0.01122.
  Both capped live narration passages had zero word errors. The uncapped
  German passage had two word errors out of 36.
- All four short replies reached the decoder with non-empty text. The live
  pass had zero exact matches and the saved pass matched one. The 192 kb/s
  targeted pilot matched two of the replies with the silence lead. Shortening
  speech confirmation in the forced-cut run left the live exact count at zero.
  The 2 s cap produced 23 contiguous boundaries.
- The mixer produced 180.890 s of PCM from 179.377 s of paired input.
  A ready source can trigger a window while the other source is one callback
  behind, so repeated zero padding can accumulate on this schedule. The short
  reply landmarks arrived roughly 0.8–0.95 s after their original bookmarks.
- The 96 kb/s file used 724,001 bytes versus 1,145,015 at 192 kb/s. Both
  preserved the long passages, but the 96 kb/s saved pass matched zero short
  replies and the padded targeted pass matched one.
- Decoding the two sources separately identified more of the overlapping
  passage. The final mic pass had one word error out of 36, and the system
  pass had zero out of 39. The mic-only pass also transcribed words from the
  simulated far-end bleed. A source track identifies which input carried the
  sound and can contain both speakers.

The comparison switches apply to the fixture benchmark. Live Parakeet uses a
12 s cap and 250 ms speech confirmation, and the default AAC bitrate is 192 kb/s.
The [installed capture smoke](installed-capture-smoke.md) covers actual
microphone and system audio, saved playback, Stop, and a second recording.
For quiet-microphone acceptance, record a known passage at a low input level
with Whisper and Nemotron. Check that speech triggers detection, the saved mic
track is audible in the mix, and the transcript contains the quiet replies.
Report the input level and engine with each result.

## Pilot guardrails

- Separate-track capture needs an explicit storage budget and recovery plan.
  Two raw 48 kHz f32 mono tracks use about 1.38 GB per hour before encoding.
  Keeping the mixed spool as well brings the total to about 2.07 GB per hour.
  The source comparison measures isolated passes. Validate concurrent writes
  and device-clock synchronization on the installed app.
- Assess targeted retranscription on a user-selected window, keep timestamps
  in meeting time, and preserve the original text for review. Include fixtures
  with silence, overlap, and complete short answers when comparing candidates.
- Whisper's native decoder exposes an initial prompt. A vocabulary pilot can
  compare bounded lists of user-approved terms against the same audio without
  a prompt. Measure both name recovery and insertions of terms that were never
  spoken, including silence and unrelated passages. Keep vocabulary and output
  local to that experiment.
- A lower bitrate should meet the same word, timing, and audible playback
  criteria across quiet replies and overlap. Compare bytes alongside quality.
  The raw capture spool uses the same 48 kHz PCM at every AAC bitrate.
