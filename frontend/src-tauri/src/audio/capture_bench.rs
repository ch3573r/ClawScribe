//! Opt-in fixture benchmark. Runs in an isolated temporary recording folder.
use super::*;
use crate::audio::{
    audio_spool, batch_audio,
    device_detection::InputDeviceKind,
    devices::DeviceType as CaptureDeviceType,
    disk_space,
    retranscription::{MAX_TRANSCRIPTION_SEGMENT_SAMPLES, VAD_REDEMPTION_TIME_MS},
    transcription::queue::{recording_audio_queue, transcription_queue},
};
use crate::parakeet_engine::ParakeetModel;
use anyhow::{ensure, Context};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const RATE: usize = 48_000;
const CALLBACK_SAMPLES: usize = RATE / 100;
const MIC: usize = 0;
const SYSTEM: usize = 1;
const DE: &str = "Guten Morgen. Heute planen wir die nächsten Schritte für unser Projekt. Am Mittwoch sprechen wir mit dem Team über die offenen Fragen. Danach schreiben wir die Ergebnisse auf und vereinbaren einen Termin für die nächste Woche.";
const EN: &str = "Good morning. Today we are planning the next steps for our project. On Wednesday we will meet with the team to discuss the open questions. Afterwards we will write down the results and arrange a meeting for next week.";
static CANCELLED: AtomicBool = AtomicBool::new(false);

struct Passage {
    name: &'static str,
    start: usize,
    tracks: [Vec<f32>; 2],
    reference: [&'static str; 2],
    short: bool,
}

impl Passage {
    fn end(&self) -> usize {
        self.start + self.tracks[0].len().max(self.tracks[1].len()) * 3
    }

    fn sample(&self, source: usize, index: usize) -> f32 {
        let offset = index - self.start;
        let track = &self.tracks[source];
        let first = track.get(offset / 3).copied().unwrap_or(0.0);
        let next = track.get(offset / 3 + 1).copied().unwrap_or(0.0);
        first + (next - first) * (offset % 3) as f32 / 3.0
    }
}

fn pcm(bytes: &[u8], gain: f32) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 * gain / 32768.0)
        .collect()
}

fn fixture(directory: &Path, silence_seconds: usize) -> Result<Vec<Passage>> {
    let german = pcm(include_bytes!("pc_test_samples/de.s16le"), 0.15);
    let english = pcm(include_bytes!("pc_test_samples/en.s16le"), 0.45);
    let mut passages = vec![
        Passage {
            name: "German microphone",
            start: RATE * 2,
            tracks: [german.clone(), Vec::new()],
            reference: [DE, ""],
            short: false,
        },
        Passage {
            name: "English system with quiet mic bleed",
            start: 0,
            tracks: [
                english.iter().map(|sample| sample * 0.015).collect(),
                english.clone(),
            ],
            reference: ["", EN],
            short: false,
        },
    ];
    passages[1].start = passages[0].end() + RATE * 2;
    let mut start = passages[1].end() + RATE * silence_seconds;
    for (name, source) in [
        ("ja", MIC),
        ("yes", SYSTEM),
        ("genau", MIC),
        ("right", SYSTEM),
    ] {
        let bytes = std::fs::read(directory.join(format!("{name}.s16le")))
            .with_context(|| format!("Missing synthetic reply fixture {name}.s16le"))?;
        ensure!(
            (3_200..=32_000).contains(&bytes.len()) && bytes.len() % 2 == 0,
            "Use a 0.1–1 s, 16 kHz mono PCM16 fixture for {name}"
        );
        let mut tracks = [Vec::new(), Vec::new()];
        tracks[source] = pcm(&bytes, if source == MIC { 0.12 } else { 0.45 });
        let mut reference = ["", ""];
        reference[source] = name;
        let passage = Passage {
            name,
            start,
            tracks,
            reference,
            short: true,
        };
        start = passage.end() + RATE * 2;
        passages.push(passage);
    }
    passages.push(Passage {
        name: "Overlapping German and English",
        start,
        tracks: [german, english],
        reference: [DE, EN],
        short: false,
    });
    Ok(passages)
}

struct Decoded {
    start: f64,
    end: f64,
    words: Vec<String>,
}

fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn word_errors(expected: &[String], actual: &[String]) -> usize {
    let mut previous: Vec<_> = (0..=actual.len()).collect();
    for (index, expected_word) in expected.iter().enumerate() {
        let mut next = vec![index + 1];
        for (column, actual_word) in actual.iter().enumerate() {
            let substitution = previous[column] + usize::from(expected_word != actual_word);
            next.push(
                substitution
                    .min(previous[column + 1] + 1)
                    .min(next[column] + 1),
            );
        }
        previous = next;
    }
    previous[actual.len()]
}

fn score(passages: &[Passage], decoded: &[Decoded], isolated_source: Option<usize>) {
    // Exclude adjacent speech and VAD context at the ends of the silent interval.
    let silence_start = passages[1].end() as f64 / RATE as f64 + 0.5;
    let silence_end = passages[2].start as f64 / RATE as f64 - 0.5;
    let silence_words: usize = decoded
        .iter()
        .filter(|row| row.start >= silence_start && row.end <= silence_end)
        .map(|row| row.words.len())
        .sum();
    println!(
        "  interior_digital_silence_s={:.3} false_words={silence_words}",
        silence_end - silence_start
    );

    let mut by_passage = vec![Vec::new(); passages.len()];
    let mut unassigned = 0;
    for row in decoded {
        let closest = passages
            .iter()
            .enumerate()
            .map(|(index, passage)| {
                let start = passage.start as f64 / RATE as f64;
                let end = passage.end() as f64 / RATE as f64;
                (index, row.end.min(end) - row.start.max(start))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1));
        if let Some((index, _)) = closest.filter(|(_, overlap)| *overlap > 0.0) {
            by_passage[index].push(row);
        } else {
            unassigned += row.words.len();
        }
    }

    let mut errors_total = unassigned;
    let mut reference_total = 0;
    for (passage, rows) in passages.iter().zip(by_passage) {
        let start = passage.start as f64 / RATE as f64;
        let end = passage.end() as f64 / RATE as f64;
        let bookmark = (start + end) / 2.0;
        let bookmark_gap = rows
            .iter()
            .map(|row| (row.start - bookmark).max(bookmark - row.end).max(0.0))
            .min_by(f64::total_cmp);
        let actual: Vec<_> = rows
            .iter()
            .flat_map(|row| row.words.iter().cloned())
            .collect();
        let expected: Vec<_> = passage
            .reference
            .iter()
            .enumerate()
            .filter(|(source, _)| isolated_source.is_none_or(|active| active == *source))
            .flat_map(|(_, text)| words(text))
            .collect();
        let overlap =
            isolated_source.is_none() && passage.reference.iter().all(|text| !text.is_empty());
        if overlap || expected.is_empty() {
            if !overlap {
                errors_total += actual.len();
            }
            println!(
                "  {}: segments={} words={} bookmark_s={bookmark:.3} bookmark_gap_s={bookmark_gap:?}",
                passage.name,
                rows.len(),
                actual.len()
            );
        } else {
            let errors = word_errors(&expected, &actual);
            errors_total += errors;
            reference_total += expected.len();
            let cue_coverage_pct = passage.short.then(|| {
                let mut covered = 0.0;
                let mut through = start;
                for row in &rows {
                    covered += (row.end.min(end) - row.start.max(through)).max(0.0);
                    through = row.end.max(through);
                }
                (100.0 * covered / (end - start)).round() as u32
            });
            println!(
                "  {}: segments={} blank={} errors={errors}/{} short_reply_match={:?} cue_coverage_pct={cue_coverage_pct:?} bookmark_s={bookmark:.3} bookmark_gap_s={bookmark_gap:?}",
                passage.name,
                rows.len(),
                rows.iter().filter(|row| row.words.is_empty()).count(),
                expected.len(),
                passage.short.then(|| actual == expected)
            );
        }
    }
    let wer = 100.0 * errors_total as f64 / reference_total as f64;
    println!("  word_error_rate={wer:.2}% errors={errors_total}/{reference_total} unassigned_words={unassigned}");
}

fn read_rss(system: &mut sysinfo::System, pid: sysinfo::Pid) -> u64 {
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map_or(0, sysinfo::Process::memory)
}

async fn probe_reply_windows(
    model: &mut ParakeetModel,
    saved_audio: &Path,
    passages: &[Passage],
    isolated_source: Option<usize>,
) -> Result<()> {
    for passage in passages.iter().filter(|passage| passage.short) {
        if isolated_source.is_some_and(|source| passage.reference[source].is_empty()) {
            continue;
        }
        let start = passage.start as f64 / RATE as f64 - 0.15;
        let end = passage.end() as f64 / RATE as f64 + 0.20;
        let mut decoder = batch_audio::audio_decoder_command(saved_audio)?;
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            decoder
                .args([
                    "-ss",
                    &format!("{start:.3}"),
                    "-t",
                    &format!("{:.3}", end - start),
                ])
                .args(["-f", "f32le", "pipe:1"])
                .stdout(std::process::Stdio::piped())
                .output(),
        )
        .await??;
        ensure!(
            output.status.success() && !output.stdout.is_empty(),
            "Reply window decoding failed"
        );
        let audio: Vec<_> = output
            .stdout
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        let expected = words(passage.name);
        let started = Instant::now();
        let original = model.transcribe_samples_with_retry(audio.clone(), &|| false)?;
        let padded = model.transcribe_samples([vec![0.0; 16_000], audio].concat())?;
        println!(
            "  targeted {}: window_s={:.3} exact={} exact_with_silence_lead={} decode_s={:.3}",
            passage.name,
            end - start,
            words(&original.text) == expected,
            words(&padded.text) == expected,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

struct CaptureResult {
    stopped: Instant,
    wall_seconds: f64,
    media_seconds: f64,
    callback_total: Duration,
    callback_peak: Duration,
    rss: (u64, u64, u64),
}

struct CaptureOptions {
    vad_cap_ms: u32,
    min_speech_ms: Option<u64>,
    uncapped: bool,
    paced: bool,
    isolated_source: Option<usize>,
}

#[tokio::test(flavor = "multi_thread")]
async fn mixed_capture_clock_survives_mismatched_callbacks_and_stop() -> Result<()> {
    const MIC_RATE: usize = 44_100;
    const MIC_FRAMES: usize = 220_985;
    const SYSTEM_FRAMES: usize = 240_528; // 5.011 seconds at 48 kHz.
    const MARKERS: [usize; 2] = [96_000, SYSTEM_FRAMES - 1];
    let folder = tempfile::tempdir()?;
    let (capture_writer, mut saved, _) = recording_audio_queue(folder.path())?;
    let (speech_writer, _speech, _) = transcription_queue()?;
    let (sender, receiver) = mpsc::channel(128);
    let state = RecordingState::new();
    let mic = AudioCapture::new(
        Arc::new(AudioDevice::new(
            "Fixture mic".into(),
            CaptureDeviceType::Input,
        )),
        state.clone(),
        MIC_RATE as u32,
        1,
        DeviceType::Microphone,
    );
    let system = AudioCapture::new(
        Arc::new(AudioDevice::new(
            "Fixture system".into(),
            CaptureDeviceType::Output,
        )),
        state.clone(),
        RATE as u32,
        1,
        DeviceType::System,
    );
    let mut pipeline = AudioPipeline::new(
        receiver,
        speech_writer,
        state.clone(),
        0,
        RATE as u32,
        "Fixture mic".into(),
        InputDeviceKind::Unknown,
        "Fixture system".into(),
        InputDeviceKind::Unknown,
        false,
    );
    pipeline.recording_sender_for_mixed =
        Some(BufferedSpool::new(capture_writer, state.clone(), true));
    let pipeline_task = tokio::spawn(pipeline.run());
    state.set_audio_sender(sender.clone());
    state.start_recording()?;

    let (mut mic_cursor, mut system_cursor) = (0, 0);
    while mic_cursor < MIC_FRAMES || system_cursor < SYSTEM_FRAMES {
        let mic_end = (mic_cursor + 317).min(MIC_FRAMES);
        let system_end = (system_cursor + 1_024).min(SYSTEM_FRAMES);
        while sender.capacity() == 0 {
            ensure!(!sender.is_closed(), "Fixture capture pipeline exited early");
            tokio::task::yield_now().await;
        }
        // Order each callback by the time its last sample became available.
        // No sleeps are needed to reproduce the same interleaving.
        if mic_cursor < MIC_FRAMES
            && (system_cursor == SYSTEM_FRAMES
                || mic_end as u64 * RATE as u64 <= system_end as u64 * MIC_RATE as u64)
        {
            let samples: Vec<_> = (mic_cursor..mic_end)
                .map(|index| {
                    if (MIC_RATE..MIC_RATE + 1_323).contains(&index) {
                        0.02 * (std::f32::consts::TAU * (index % 147) as f32 / 147.0).sin()
                    } else {
                        0.0
                    }
                })
                .collect();
            mic.process_audio_data(&samples);
            mic_cursor = mic_end;
        } else {
            let samples: Vec<_> = (system_cursor..system_end)
                .map(|index| if MARKERS.contains(&index) { 0.5 } else { 0.0 })
                .collect();
            system.process_audio_data(&samples);
            system_cursor = system_end;
        }
    }

    state.stop_recording();
    drop(sender);
    pipeline_task.await??;
    let mut pcm = Vec::new();
    while let Some(item) = saved.recv().await? {
        pcm.extend(item.chunk.data);
    }
    assert!(!state.capture_incomplete());
    assert_eq!(
        pcm.len(),
        SYSTEM_FRAMES,
        "mixed PCM changed the sample clock"
    );
    let actual_markers: Vec<_> = pcm
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| (*sample > 0.49).then_some(index))
        .collect();
    assert_eq!(actual_markers, MARKERS);
    // The 30 ms mic tone starts at 1 s, allowing for the 10 ms limiter delay.
    assert!(pcm[RATE..RATE + 2_400]
        .iter()
        .any(|sample| sample.abs() > 0.01));
    Ok(())
}

async fn capture_fixture(
    folder: &Path,
    passages: &[Passage],
    options: &CaptureOptions,
    speech_writer: TranscriptionQueueSender,
) -> Result<CaptureResult> {
    let (capture_writer, mut saved, capture_metrics) = recording_audio_queue(folder)?;
    let (sender, receiver) = mpsc::channel(128);
    let state = RecordingState::new();
    let mic = Arc::new(AudioDevice::new(
        "Fixture mic".into(),
        CaptureDeviceType::Input,
    ));
    let system = Arc::new(AudioDevice::new(
        "Fixture system".into(),
        CaptureDeviceType::Output,
    ));
    state.set_microphone_device(mic.clone());
    state.set_system_device(system.clone());
    let mic = AudioCapture::new(mic, state.clone(), RATE as u32, 1, DeviceType::Microphone);
    let system = AudioCapture::new(system, state.clone(), RATE as u32, 1, DeviceType::System);
    if options.uncapped {
        mic.normalizer
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .use_uncapped_gain_for_benchmark();
    }
    let mut pipeline = AudioPipeline::new(
        receiver,
        speech_writer,
        state.clone(),
        0,
        RATE as u32,
        "Fixture mic".into(),
        InputDeviceKind::Unknown,
        "Fixture system".into(),
        InputDeviceKind::Unknown,
        false,
    );
    let mut vad = ContinuousVadProcessor::new_with_max_segment_duration(
        RATE as u32,
        LIVE_VAD_REDEMPTION_MS,
        Some(options.vad_cap_ms),
    )?;
    if let Some(min_speech_ms) = options.min_speech_ms {
        vad.set_min_speech_for_benchmark(min_speech_ms);
    }
    pipeline.vad_processor = Some(vad);
    pipeline.recording_sender_for_mixed =
        Some(BufferedSpool::new(capture_writer, state.clone(), true));
    let pipeline_task = tokio::spawn(pipeline.run());

    let pid = sysinfo::get_current_pid().map_err(anyhow::Error::msg)?;
    let mut process = sysinfo::System::new();
    let initial_rss = read_rss(&mut process, pid);
    let mut peak_rss = initial_rss;
    let end = passages.last().unwrap().end() + RATE;
    let mut passage_index = 0;
    let mut callback_total = Duration::ZERO;
    let mut callback_peak = Duration::ZERO;
    let begin = tokio::time::Instant::now();
    state.set_audio_sender(sender.clone());
    state.start_recording()?;
    for tick in (0..end).step_by(CALLBACK_SAMPLES) {
        if options.paced {
            tokio::time::sleep_until(begin + Duration::from_secs_f64(tick as f64 / RATE as f64))
                .await;
        }
        while sender.capacity() < 2 {
            ensure!(!sender.is_closed(), "Fixture capture pipeline exited early");
            tokio::task::yield_now().await;
        }
        let mut mic_input = [0.0; CALLBACK_SAMPLES];
        let mut system_input = [0.0; CALLBACK_SAMPLES];
        let callback_samples = (end - tick).min(CALLBACK_SAMPLES);
        for offset in 0..callback_samples {
            let index = tick + offset;
            while passage_index < passages.len() && index >= passages[passage_index].end() {
                passage_index += 1;
            }
            if let Some(passage) = passages
                .get(passage_index)
                .filter(|passage| index >= passage.start)
            {
                mic_input[offset] = passage.sample(MIC, index);
                system_input[offset] = passage.sample(SYSTEM, index);
            }
            if index < passages[0].start {
                // Deterministic quiet background to exercise integrated gain on startup.
                mic_input[offset] +=
                    0.002 * (std::f32::consts::TAU * (index % 48) as f32 / 48.0).sin();
            }
        }
        if options.isolated_source == Some(SYSTEM) {
            mic_input.fill(0.0);
        } else if options.isolated_source == Some(MIC) {
            system_input.fill(0.0);
        }
        let before = Instant::now();
        mic.process_audio_data(&mic_input[..callback_samples]);
        system.process_audio_data(&system_input[..callback_samples]);
        let elapsed = before.elapsed();
        callback_total += elapsed;
        callback_peak = callback_peak.max(elapsed);
        if tick % RATE == 0 {
            peak_rss = peak_rss.max(read_rss(&mut process, pid));
        }
    }
    let stopped = Instant::now();
    state.stop_recording();
    drop(sender);
    pipeline_task.await??;
    let snapshot = capture_metrics.snapshot();
    ensure!(
        snapshot.failed_chunks == 0 && !state.capture_incomplete(),
        "Capture spool has gaps"
    );
    let wall_seconds = begin.elapsed().as_secs_f64();
    let first = saved.recv().await?.context("No audio was saved")?.chunk;
    let background = first
        .data
        .get(RATE * 3 / 5..RATE)
        .context("Captured opening was shorter than the background window")?;
    let background_rms = (background.iter().map(|sample| sample * sample).sum::<f32>()
        / background.len() as f32)
        .sqrt();
    let mut total_samples = first.data.len();
    while let Some(queued) = saved.recv().await? {
        total_samples += queued.chunk.data.len();
    }
    println!(
        "capture spool_bytes={} startup_background_rms={background_rms:.5}",
        snapshot.spool_bytes
    );
    Ok(CaptureResult {
        stopped,
        wall_seconds,
        media_seconds: total_samples as f64 / RATE as f64,
        callback_total,
        callback_peak,
        rss: (initial_rss, peak_rss, read_rss(&mut process, pid)),
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a local Parakeet model, FFmpeg, and synthetic short reply fixtures"]
async fn two_source_capture_quality() -> Result<()> {
    let directory = |name| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .with_context(|| format!("Set {name}"))
    };
    let model_dir = directory("CLAWSCRIBE_AUDIO_BENCH_MODEL")?;
    let fixtures = directory("CLAWSCRIBE_AUDIO_BENCH_FIXTURES")?;
    let silence = std::env::var("CLAWSCRIBE_AUDIO_BENCH_SILENCE_SECONDS")
        .map_or(Ok(120), |value| value.parse::<usize>())?;
    let cap = std::env::var("CLAWSCRIBE_AUDIO_BENCH_CAP_MS")
        .map_or(Ok(live_max_segment_ms_for_provider("parakeet")), |value| {
            value.parse::<u32>()
        })?;
    ensure!(
        (2..=7_200).contains(&silence) && (1_000..=60_000).contains(&cap),
        "Invalid fixture silence or cap"
    );
    let uncapped = std::env::var_os("CLAWSCRIBE_AUDIO_BENCH_UNCAPPED").is_some();
    let paced = std::env::var_os("CLAWSCRIBE_AUDIO_BENCH_PACED").is_some();
    let bitrate = std::env::var("CLAWSCRIBE_AUDIO_BENCH_AAC_KBPS")
        .map_or(Ok(crate::audio::encode::RECORDING_AAC_KBPS), |value| {
            value.parse::<u32>()
        })?;
    ensure!(
        [64, 96, 128, crate::audio::encode::RECORDING_AAC_KBPS].contains(&bitrate),
        "Use 64, 96, 128 or 192 kb/s for AAC"
    );
    let min_speech_ms = std::env::var("CLAWSCRIBE_AUDIO_BENCH_MIN_SPEECH_MS")
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()?;
    ensure!(
        min_speech_ms.is_none_or(|value| (30..=250).contains(&value)),
        "Invalid minimum speech time"
    );
    let isolated_source = match std::env::var("CLAWSCRIBE_AUDIO_BENCH_SOURCE").as_deref() {
        Ok("mic") => Some(MIC),
        Ok("system") => Some(SYSTEM),
        Ok(_) => anyhow::bail!("Set source to mic or system"),
        Err(_) => None,
    };
    let passages = fixture(&fixtures, silence)?;
    let folder = tempfile::tempdir()?;
    let expected_duration = (passages.last().unwrap().end() + RATE) as f64 / RATE as f64;
    // Capture + AAC + decoded PCM + staged speech, including some overhead.
    let scratch_bytes = expected_duration.ceil() as u64 * 384_000 + 64 * 1024 * 1024;
    ensure!(
        disk_space::free_space_bounded(folder.path())
            .await
            .is_some_and(|bytes| bytes >= scratch_bytes),
        "The local temporary volume needs at least {scratch_bytes} free bytes for this run"
    );
    let model_variant = model_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            matches!(
                *name,
                "parakeet-tdt-0.6b-v3-int8"
                    | "parakeet-tdt-0.6b-ultra-int8"
                    | "parakeet-tdt-0.6b-v3-smoothquant-int8"
                    | "parakeet-tdt-0.6b-v2-int8"
            )
        })
        .unwrap_or("custom int8");
    let model_start = Instant::now();
    let model = ParakeetModel::new(&model_dir, Some("int8"), false)?;
    println!(
        "Parakeet CPU de/en model={model_variant} load_s={:.3} cap_ms={cap} min_speech_ms={min_speech_ms:?} uncapped={uncapped} paced={paced} source={isolated_source:?}",
        model_start.elapsed().as_secs_f64(),
    );

    let (speech_writer, mut speech, metrics) = transcription_queue()?;
    let live_metrics = metrics.clone();
    let live_task = tokio::spawn(async move {
        let mut model = model;
        let mut decoded = Vec::new();
        let mut max_queued_seconds = 0.0f64;
        live_metrics.set_worker_active(true);
        while let Some(queued) = speech.recv().await? {
            let before = Instant::now();
            live_metrics.mark_processing_started();
            max_queued_seconds =
                max_queued_seconds.max(live_metrics.snapshot().queued_audio_seconds);
            let audio_ms = queued.audio_ms();
            let spool_bytes = queued.spool_bytes();
            let chunk = queued.chunk;
            let finish = chunk.timestamp + chunk.data.len() as f64 / 16_000.0;
            let (returned_model, text) = tokio::task::spawn_blocking(move || {
                let text = model
                    .transcribe_samples_with_retry(chunk.data, &|| false)?
                    .text;
                Ok::<_, anyhow::Error>((model, words(&text)))
            })
            .await??;
            model = returned_model;
            max_queued_seconds =
                max_queued_seconds.max(live_metrics.snapshot().queued_audio_seconds);
            live_metrics.mark_completed(audio_ms, spool_bytes, before.elapsed());
            decoded.push(Decoded {
                start: chunk.timestamp,
                end: finish,
                words: text,
            });
        }
        live_metrics.set_worker_active(false);
        Ok::<_, anyhow::Error>((model, decoded, max_queued_seconds, Instant::now()))
    });
    let capture = capture_fixture(
        folder.path(),
        &passages,
        &CaptureOptions {
            vad_cap_ms: cap,
            min_speech_ms,
            uncapped,
            paced,
            isolated_source,
        },
        speech_writer,
    )
    .await?;
    let (mut model, live, queued_seconds, live_end) = live_task.await??;
    let snapshot = metrics.snapshot();
    ensure!(
        snapshot.failed_chunks == 0 && snapshot.chunks_in_queue == 0,
        "Live speech spool lost segments"
    );
    println!(
        "capture pipeline_s={:.3} input_s={expected_duration:.3} saved_pcm_s={:.3} clock_drift_s={:.3} callback_total_s={:.3} callback_max_ms={:.3} rss_bytes_start_peak_end={:?}",
        capture.wall_seconds, capture.media_seconds, capture.media_seconds - expected_duration,
        capture.callback_total.as_secs_f64(), capture.callback_peak.as_secs_f64() * 1_000.0, capture.rss
    );
    let adjacent = live
        .windows(2)
        .filter(|pair| (pair[0].end - pair[1].start).abs() < 0.000_001)
        .count();
    println!(
        "live segments={} contiguous_cuts={adjacent} decode_rtf={:?} sampled_peak_queued_audio_s={queued_seconds:.3} stop_to_last_decode_s={:.3}",
        live.len(), snapshot.processing_realtime_factor, live_end.saturating_duration_since(capture.stopped).as_secs_f64()
    );
    score(&passages, &live, isolated_source);

    let output = folder.path().join("audio.mp4");
    let encode_started = Instant::now();
    let (has_gaps, complete) =
        audio_spool::encode_capture_at_bitrate(folder.path(), &output, bitrate)?;
    ensure!(!has_gaps && complete, "Saved audio has capture gaps");
    let encode_seconds = encode_started.elapsed().as_secs_f64();
    let prepare_started = Instant::now();
    let prepared = batch_audio::prepare(
        &output,
        VAD_REDEMPTION_TIME_MS,
        MAX_TRANSCRIPTION_SEGMENT_SAMPLES,
        &CANCELLED,
    )
    .await?;
    let prepare_seconds = prepare_started.elapsed().as_secs_f64();
    let duration = prepared.duration_seconds;
    ensure!(
        (duration - capture.media_seconds).abs() < 0.05,
        "Encoded duration {duration:.3} s differs from captured PCM clock {:.3} s",
        capture.media_seconds
    );
    let decode_started = Instant::now();
    let mut final_rows = Vec::new();
    for segment in &prepared.segments {
        if segment.sample_count < 1_600 {
            continue;
        }
        let segment = segment.load().await?;
        let result = model.transcribe_samples_with_retry(segment.samples, &|| false)?;
        final_rows.push(Decoded {
            start: segment.start_timestamp_ms / 1_000.0,
            end: segment.end_timestamp_ms / 1_000.0,
            words: words(&result.text),
        });
    }
    println!(
        "final media_s={duration:.3} aac_kbps={bitrate} aac_bytes={} encode_s={encode_seconds:.3} prepare_s={prepare_seconds:.3} decode_s={:.3} segments={}",
        std::fs::metadata(&output)?.len(), decode_started.elapsed().as_secs_f64(), final_rows.len(),
    );
    score(&passages, &final_rows, isolated_source);
    if std::env::var_os("CLAWSCRIBE_AUDIO_BENCH_TARGETED").is_some() {
        probe_reply_windows(&mut model, &output, &passages, isolated_source).await?;
    }
    Ok(())
}
