//! Measures how live segmentation affects Parakeet accuracy on one clip.
//!
//! Feeds a 16 kHz mono WAV through the same VAD the recording pipeline uses,
//! transcribes every segment, and reports the word error rate against a
//! reference text. Run it for several caps to compare cut policies:
//!
//! ```text
//! cargo run --example live_cut_bench -- <model_dir> <wav> <reference.txt> \
//!     <redemption_ms>:<cap_ms>[:<lead_ms>:<trail_ms>] [...]
//! ```
//!
//! `400:12000` is the live Parakeet setting. Optional third and fourth fields
//! add silence before and after every segment. A cap of `0` disables the cap.
//! Set `LIVE_CUT_BENCH_RETRY` to retry blank segment decodes with leading silence.
//! The switch does not affect `probe_empty`.
//! `LIVE_CUT_BENCH_WORKER` uses supervised inference (including empty retries).
//! `LIVE_CUT_BENCH_KILL_AFTER=<n>` kills that worker once after n successful
//! segments to exercise recovery. Worker mode does not support the raw probe.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use app_lib::audio::vad::ContinuousVadProcessor;
use app_lib::parakeet_engine::worker::{transcription_timeout, LoadSpec, TranscriptionWorker};
use app_lib::parakeet_engine::ParakeetModel;

const USAGE: &str =
    "usage: live_cut_bench <model_dir> <wav_16k_mono_pcm> <reference.txt> <redemption_ms>:<cap_ms>[:<lead_ms>:<trail_ms>]...";
/// The live pipeline drops segments shorter than this before transcription.
const MIN_SEGMENT_SAMPLES: usize = 800;
/// Roughly the block size the live mixer hands to the VAD.
const FEED_SAMPLES: usize = 1_600;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(code) = app_lib::parakeet_engine::worker::dispatch() {
        std::process::exit(code);
    }
    let mut args = env::args().skip(1);
    let model_dir = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let wav = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let reference_path = args.next().map(PathBuf::from).ok_or(USAGE)?;
    let settings: Vec<String> = args.collect();
    if settings.is_empty() {
        return Err(USAGE.into());
    }

    let mut samples = read_wav_16k_mono_pcm(&wav)?;
    // Synthetic clips contain exact digital silence, which a microphone never
    // produces. LIVE_CUT_BENCH_NOISE adds a noise floor of that amplitude.
    if let Some(level) = env::var("LIVE_CUT_BENCH_NOISE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        let mut seed = 0x2545_F491u32;
        for sample in &mut samples {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *sample += level * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0);
        }
    }
    let reference = words(&fs::read_to_string(&reference_path)?);
    println!(
        "clip: {:.1}s, reference: {} words",
        samples.len() as f64 / 16_000.0,
        reference.len()
    );

    let use_worker = env::var_os("LIVE_CUT_BENCH_WORKER").is_some();
    let kill_after = env::var("LIVE_CUT_BENCH_KILL_AFTER")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?;
    if kill_after.is_some() && !use_worker {
        return Err("LIVE_CUT_BENCH_KILL_AFTER requires LIVE_CUT_BENCH_WORKER".into());
    }
    if use_worker && env::var_os("LIVE_CUT_BENCH_PROBE").is_some() {
        return Err("LIVE_CUT_BENCH_PROBE requires in-process inference".into());
    }
    let mut worker = if use_worker {
        let mut worker = TranscriptionWorker::new()?;
        worker.load(
            LoadSpec {
                model_dir: model_dir.clone(),
                precision_suffix: Some("int8".into()),
                use_directml: false,
            },
            &|| false,
        )?;
        Some(worker)
    } else {
        None
    };
    let mut model = if use_worker {
        None
    } else {
        Some(ParakeetModel::new(&model_dir, Some("int8"), false)?)
    };
    let mut successful_segments = 0usize;
    let mut killed = false;
    if kill_after == Some(0) {
        if let Some(worker) = worker.as_mut() {
            worker.kill_child()?;
            killed = true;
            println!("worker: forced termination before the first segment");
        }
    }
    let retry_empty = env::var_os("LIVE_CUT_BENCH_RETRY").is_some();

    for setting in settings {
        let fields: Vec<u32> = setting
            .split(':')
            .map(|field| field.parse::<u32>())
            .collect::<Result<_, _>>()
            .map_err(|_| USAGE)?;
        let (redemption_ms, cap_ms) = match fields[..] {
            [redemption_ms, cap_ms]
            | [redemption_ms, cap_ms, _]
            | [redemption_ms, cap_ms, _, _] => (redemption_ms, cap_ms),
            _ => return Err(USAGE.into()),
        };
        // Optional third and fourth fields: silence added before and after
        // every segment, in milliseconds.
        let lead = vec![0.0f32; fields.get(2).copied().unwrap_or(0) as usize * 16];
        let trail = vec![0.0f32; fields.get(3).copied().unwrap_or(0) as usize * 16];
        let mut vad = ContinuousVadProcessor::new_with_max_segment_duration(
            16_000,
            redemption_ms,
            (cap_ms > 0).then_some(cap_ms),
        )?;
        let mut segments = Vec::new();
        for block in samples.chunks(FEED_SAMPLES) {
            segments.extend(vad.process_audio(block)?);
        }
        segments.extend(vad.flush()?);
        segments.retain(|segment| segment.samples.len() >= MIN_SEGMENT_SAMPLES);

        let mut durations = Vec::new();
        let mut pieces = Vec::new();
        let mut empty = 0usize;
        for segment in segments {
            durations.push(segment.samples.len() as f64 / 16_000.0);
            let padded = [lead.as_slice(), &segment.samples, trail.as_slice()].concat();
            let text = if let Some(worker) = worker.as_mut() {
                let timeout = transcription_timeout(padded.len());
                worker.transcribe(padded, timeout, &|| false)?
            } else {
                let model = model.as_mut().ok_or("No in-process model loaded")?;
                if retry_empty {
                    model.transcribe_samples_with_retry(padded, &|| false)?
                } else {
                    model.transcribe_samples(padded)?
                }
            }
            .text;
            successful_segments += 1;
            if !killed && kill_after == Some(successful_segments) {
                if let Some(worker) = worker.as_mut() {
                    worker.kill_child()?;
                    killed = true;
                    println!(
                        "worker: forced termination after {successful_segments} successful segments"
                    );
                }
            }
            empty += usize::from(text.trim().is_empty());
            if text.trim().is_empty() && env::var_os("LIVE_CUT_BENCH_PROBE").is_some() {
                probe_empty(
                    model.as_mut().ok_or("No in-process model loaded")?,
                    &segment.samples,
                    segment.start_timestamp_ms,
                )?;
            }
            pieces.push(text);
        }
        let hypothesis = words(&pieces.join(" "));
        let errors = edit_distance(&reference, &hypothesis);
        let longest = durations.iter().cloned().fold(0.0, f64::max);
        let mean = durations.iter().sum::<f64>() / durations.len().max(1) as f64;
        println!(
            "{setting}: segments={} empty={empty} mean={mean:.1}s longest={longest:.1}s errors={errors} wer={:.2}%",
            durations.len(),
            100.0 * errors as f64 / reference.len().max(1) as f64
        );
        if env::var_os("LIVE_CUT_BENCH_TEXT").is_some() {
            for piece in &pieces {
                println!("  | {}", piece.trim());
            }
        }
    }
    Ok(())
}

/// Retries a segment that decoded to nothing with small changes to its
/// audio, to show what the empty result depends on.
fn probe_empty(
    model: &mut ParakeetModel,
    samples: &[f32],
    start_ms: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    println!(
        "  EMPTY segment at {start_ms:.0} ms: {} samples ({:.2}s), peak {peak:.3}",
        samples.len(),
        samples.len() as f64 / 16_000.0
    );
    let silence = |ms: usize| vec![0.0f32; ms * 16];
    let mut variants: Vec<(String, Vec<f32>)> = Vec::new();
    for ms in [100usize, 300, 1000] {
        variants.push((
            format!("lead {ms} ms silence"),
            [silence(ms), samples.to_vec()].concat(),
        ));
        variants.push((
            format!("trail {ms} ms silence"),
            [samples.to_vec(), silence(ms)].concat(),
        ));
    }
    for ms in [10usize, 50, 200]
        .into_iter()
        .filter(|ms| samples.len() > ms * 32)
    {
        variants.push((format!("drop first {ms} ms"), samples[ms * 16..].to_vec()));
        variants.push((
            format!("drop last {ms} ms"),
            samples[..samples.len() - ms * 16].to_vec(),
        ));
    }
    for gain in [0.5f32, 0.1] {
        variants.push((
            format!("gain {gain}"),
            samples.iter().map(|s| s * gain).collect(),
        ));
    }
    variants.push(("same again".to_string(), samples.to_vec()));
    for (name, audio) in variants {
        let text = model.transcribe_samples(audio)?.text;
        let shown: String = text.trim().chars().take(60).collect();
        println!(
            "    {name:<22} -> {} chars: {shown}",
            text.trim().chars().count()
        );
    }
    Ok(())
}

/// Lower-cased words with punctuation removed, so only wording is scored.
fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Word-level Levenshtein distance: substitutions, insertions and deletions.
fn edit_distance(reference: &[String], hypothesis: &[String]) -> usize {
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, expected) in reference.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, actual) in hypothesis.iter().enumerate() {
            let substitution = previous[j] + usize::from(expected != actual);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[hypothesis.len()]
}

fn read_wav_16k_mono_pcm(path: &Path) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let data = fs::read(path)?;
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(format!("{} is not a RIFF/WAVE file", path.display()).into());
    }
    let mut pos = 12usize;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut pcm: Option<&[u8]> = None;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let len = u32::from_le_bytes(data[pos + 4..pos + 8].try_into()?) as usize;
        pos += 8;
        let end = (pos + len).min(data.len());
        match id {
            b"fmt " if len >= 16 => {
                format = Some((
                    u16::from_le_bytes(data[pos..pos + 2].try_into()?),
                    u16::from_le_bytes(data[pos + 2..pos + 4].try_into()?),
                    u32::from_le_bytes(data[pos + 4..pos + 8].try_into()?),
                    u16::from_le_bytes(data[pos + 14..pos + 16].try_into()?),
                ));
            }
            b"data" => pcm = Some(&data[pos..end]),
            _ => {}
        }
        pos = end + (len % 2);
    }
    if format != Some((1, 1, 16_000, 16)) {
        return Err(format!(
            "{} must be 16 kHz mono signed 16-bit PCM, found {format:?}",
            path.display()
        )
        .into());
    }
    let pcm = pcm.ok_or_else(|| format!("{} has no data chunk", path.display()))?;
    Ok(pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect())
}
