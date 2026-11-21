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
The final callback carries only the remaining samples.
It uses the microphone high-pass filter and EBU R128 normalizer, the real
mixer and disk queues, the live Silero VAD, and the same Parakeet decoder and
blank retry used for local transcription. It runs a second decode from the
captured AAC using the saved-meeting audio preparation path.

The fixture covers quiet background and simulated bleed, isolated German and
English passages, two minutes of digital silence, four short replies, and
overlapping speech. It reports word error rate and per-passage errors, blank
segments, exact short replies, contiguous boundaries, and each fixture
bookmark's distance from the corresponding transcript interval. It also reports
how much of each short cue reached the decoder and counts words in segments
wholly within digital silence, excluding 0.5 s at each end. The mixed overlap row
reports word count because the two speakers have no single reference word order.
Capture metrics include callback time, sampled resident memory, raw spool bytes
and duration, sampled queued audio, and stop-to-last-decode time.
The saved pass reports AAC bytes and separate encode, preparation and inference
times.

Passage scores and bookmark distances use the original fixture clock. Check
the reported clock drift and use saved playback to check word-level timing.

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

Release-profile Linux runs used the ClawScribe audio sources, Parakeet v3 int8
on CPU and synthetic callback injection with desktop and device services
substituted. The original mixer produced 180.890 s of PCM in paced and unpaced
replays. The fixture timeline was 179.377 s, rounded up to 179.380 s by full
callbacks at the end. The other 1.510 s came from the mixer padding one source
when its callback arrived just after the other source at a window boundary.

With up to 100 ms of sample lookahead for the second source, the mixer produced
179.380 s from the original callbacks and 179.377 s with the exact final
callback. The mismatched-rate regression feeds 44.1 and 48 kHz sources in
317- and 1,024-frame callbacks. Stop now saves all 240,528 frames on the 48 kHz
clock, including the final marker. The original mixer returned 241,296 frames.
When one source stalls, the other still drives output after the lookahead.
This fixture does not measure physical device-clock drift or reconnect gaps.

In the paced replay, sampled peak queued audio went from 10.78 s to 10.57 s.
Stop-to-last-decode went from 398 ms to 50 ms on these runs. The final AAC
decoded to 179.392 s, within 15 ms of the mixed PCM duration.

| Measure | Original mixer | Paired windows and VAD tail flush |
| --- | ---: | ---: |
| Exact replies, live | 0/4 | 1/4 |
| Exact replies, saved AAC | 1/4 | 1/4 |
| Live reply cues reaching the decoder | 4/4 | 4/4 |
| Saved reply cues reaching the decoder | 4/4 | 3/4 |
| Live word errors, excluding overlap | 4/79 | 3/79 |
| Saved word errors, excluding overlap | 3/79 | 3/79 |

All four original live replies returned nonempty text. With the corrected mix,
all four live segments cover their full fixture cues and contain their
bookmarks. Three still decode to different words. The saved pass misses one
cue during VAD, although a direct window from the AAC decodes it correctly.
Both corrected passes produced zero false words in 119 s of guarded digital
silence, and both narrated passages retained zero word errors.

The VAD resampler now drains the remaining input at Stop and trims the result
to the capture endpoint. In a voiced cutoff regression it recovers 228
samples at 16 kHz (14.25 ms). A separate sweep of 275 near-Stop cutoffs across
six synthetic short replies yielded 83 VAD segments, up from 77. Parakeet
returned text for all six newly delivered segments and two were exact matches.
An identically cut digital-silence control yielded no segment. Short or
ambiguous speech can still be misrecognized, and silence results do not
characterize noisy rooms.

### Earlier fixture comparisons

- Capping gain reduced startup background RMS from 0.07074 to 0.01122.
  Both capped live narration passages had zero word errors. The uncapped
  German passage had two word errors out of 36.
- The 192 kb/s targeted pilot matched two of the replies with the silence lead.
  It used the original mixer and a duration-ratio adjustment to locate cues.
  Shortening speech confirmation in the forced-cut run left the live exact
  count at zero. The 2 s cap produced 23 contiguous boundaries and increased
  narration errors.
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
