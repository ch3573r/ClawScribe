//! Test-only accuracy measurement; no app state, downloads or transcript access.

use super::{
    diarization_config_for_provider, model_descriptor_by_id, prepare_diarization_turns_for_mapping,
    DiarizationModelDescriptor, DiarizationModelPaths, DiarizationTurn, SherpaDiarizationConfig,
    SherpaOfflineDiarizer, EMBEDDING_MODEL_CATALOG,
};
use anyhow::{anyhow, bail, ensure, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ReferenceSegment {
    pub recording: String,
    pub channel: String,
    pub start: f64,
    pub end: f64,
    pub speaker: String,
}

pub fn parse_rttm(text: &str) -> Result<Vec<ReferenceSegment>> {
    let mut segments = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(";;") {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields[0] != "SPEAKER" {
            continue;
        }
        let invalid = || anyhow!("Invalid RTTM SPEAKER record on line {}", index + 1);
        if fields.len() < 10 {
            return Err(invalid());
        }
        let start: f64 = fields[3].parse().map_err(|_| invalid())?;
        let duration: f64 = fields[4].parse().map_err(|_| invalid())?;
        let end = start + duration;
        if !start.is_finite()
            || start < 0.0
            || !duration.is_finite()
            || duration <= 0.0
            || !end.is_finite()
            || end <= start
        {
            return Err(invalid());
        }
        segments.push(ReferenceSegment {
            recording: fields[1].into(),
            channel: fields[2].into(),
            start,
            end,
            speaker: fields[7].into(),
        });
    }
    Ok(segments)
}

pub fn select_recording(
    segments: &[ReferenceSegment],
    recording: &str,
) -> Result<Vec<ReferenceSegment>> {
    let selected: Vec<_> = segments
        .iter()
        .filter(|segment| segment.recording == recording)
        .cloned()
        .collect();
    ensure!(
        !selected.is_empty(),
        "No RTTM segments match recording '{recording}'"
    );
    let channel = &selected[0].channel;
    ensure!(
        selected.iter().all(|segment| &segment.channel == channel),
        "RTTM recording '{recording}' has multiple channels; multi-channel files are not supported"
    );
    Ok(selected)
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct DerOptions {
    pub collar_seconds: f64,
    pub skip_overlap: bool,
}

impl Default for DerOptions {
    fn default() -> Self {
        Self {
            collar_seconds: 0.25,
            skip_overlap: false,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DerReport {
    pub scored_speech: f64,
    pub missed: f64,
    pub false_alarm: f64,
    pub speaker_confusion: f64,
    pub der: f64,
    pub reference_speakers: usize,
    pub hypothesis_speakers: usize,
    pub speaker_count_error: i64,
    pub mapping: Vec<(String, usize)>,
}

// Each reference row gets at most one hypothesis column and the reverse. State
// = set of reference rows already matched; columns are taken one at a time, so
// the hypothesis may have any number of speakers. O(columns * rows * 2^rows),
// rows <= 12. An unmatched row maps to `usize::MAX`.
fn optimal_assignment(weights: &[Vec<f64>], rows: usize, columns: usize) -> Vec<usize> {
    const UNMATCHED: u8 = u8::MAX;
    let states = 1usize << rows;
    let mut best = vec![0.0f64; states];
    let mut reachable = vec![false; states];
    reachable[0] = true;
    let mut choices = Vec::with_capacity(columns);
    for column in 0..columns {
        let mut next = best.clone();
        let mut next_reachable = reachable.clone();
        let mut choice = vec![UNMATCHED; states];
        for mask in 0..states {
            if !reachable[mask] {
                continue;
            }
            for row in 0..rows {
                if mask & (1 << row) != 0 {
                    continue;
                }
                let target = mask | (1 << row);
                let total = best[mask] + weights[row][column];
                if !next_reachable[target] || total > next[target] {
                    next[target] = total;
                    next_reachable[target] = true;
                    choice[target] = row as u8;
                }
            }
        }
        best = next;
        reachable = next_reachable;
        choices.push(choice);
    }
    let mut mask = (0..states)
        .filter(|&mask| reachable[mask])
        .max_by(|&a, &b| best[a].total_cmp(&best[b]))
        .unwrap_or(0);
    let mut assignment = vec![usize::MAX; rows];
    for column in (0..columns).rev() {
        let row = choices[column][mask];
        if row != UNMATCHED {
            assignment[row as usize] = column;
            mask ^= 1 << row;
        }
    }
    assignment
}

pub fn score_der(
    reference: &[ReferenceSegment],
    hypothesis: &[DiarizationTurn],
    options: DerOptions,
) -> Result<DerReport> {
    ensure!(
        options.collar_seconds.is_finite() && options.collar_seconds >= 0.0,
        "Collar must be finite and non-negative"
    );
    let mut boundaries = Vec::new();
    for (start, end) in reference
        .iter()
        .map(|s| (s.start, s.end))
        .chain(hypothesis.iter().map(|s| (s.start_time, s.end_time)))
    {
        ensure!(
            start.is_finite() && end.is_finite() && start >= 0.0 && end > start,
            "Segments require finite, non-negative start and end > start"
        );
        boundaries.extend([start, end]);
    }
    let refs: Vec<_> = reference
        .iter()
        .map(|s| s.speaker.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let hyps: Vec<_> = hypothesis
        .iter()
        .map(|s| s.speaker)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ensure!(
        refs.len() <= 12,
        "DER supports at most 12 reference speakers"
    );
    let mut collars = Vec::new();
    if options.collar_seconds > 0.0 {
        for segment in reference {
            for boundary in [segment.start, segment.end] {
                let start = (boundary - options.collar_seconds).max(0.0);
                let end = boundary + options.collar_seconds;
                ensure!(end.is_finite(), "Collar endpoint must be finite");
                collars.push((start, end));
                boundaries.extend([start, end]);
            }
        }
    }
    boundaries.sort_by(f64::total_cmp);
    boundaries.dedup();
    let mut intervals = Vec::new();
    let mut weights = vec![vec![0.0; hyps.len()]; refs.len()];
    let mut scored_speech = 0.0;
    let mut missed = 0.0;
    let mut false_alarm = 0.0;
    for pair in boundaries.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if collars.iter().any(|&(a, b)| a <= start && end <= b) {
            continue;
        }
        // Speaker sets prevent duplicate segments of the same speaker counting twice.
        let active_ref: BTreeSet<_> = reference
            .iter()
            .filter(|s| s.start <= start && end <= s.end)
            .map(|s| refs.binary_search(&s.speaker).unwrap())
            .collect();
        if options.skip_overlap && active_ref.len() > 1 {
            continue;
        }
        let active_hyp: BTreeSet<_> = hypothesis
            .iter()
            .filter(|s| s.start_time <= start && end <= s.end_time)
            .map(|s| hyps.binary_search(&s.speaker).unwrap())
            .collect();
        let duration = end - start;
        scored_speech += duration * active_ref.len() as f64;
        missed += duration * active_ref.len().saturating_sub(active_hyp.len()) as f64;
        false_alarm += duration * active_hyp.len().saturating_sub(active_ref.len()) as f64;
        for &r in &active_ref {
            for &h in &active_hyp {
                weights[r][h] += duration;
            }
        }
        intervals.push((duration, active_ref, active_hyp));
    }
    ensure!(
        scored_speech.is_finite()
            && missed.is_finite()
            && false_alarm.is_finite()
            && weights.iter().flatten().all(|v| v.is_finite()),
        "DER time totals overflowed"
    );
    let assignment = optimal_assignment(&weights, refs.len(), hyps.len());
    let mut speaker_confusion = 0.0;
    for (duration, active_ref, active_hyp) in intervals {
        let correct = active_ref
            .iter()
            .filter(|&&r| active_hyp.contains(&assignment[r]))
            .count();
        speaker_confusion += duration * (active_ref.len().min(active_hyp.len()) - correct) as f64;
    }
    let errors = missed + false_alarm + speaker_confusion;
    ensure!(errors.is_finite(), "DER error totals overflowed");
    ensure!(
        scored_speech > 0.0 || errors == 0.0,
        "DER is undefined: no scored reference speech but non-zero errors"
    );
    let der = if scored_speech == 0.0 {
        0.0
    } else {
        errors / scored_speech
    };
    ensure!(der.is_finite(), "DER ratio overflowed");
    let mapping = refs
        .iter()
        .enumerate()
        .filter_map(|(r, name)| {
            let h = assignment[r];
            (h < hyps.len() && weights[r][h] > 0.0).then(|| (name.clone(), hyps[h]))
        })
        .collect();
    Ok(DerReport {
        scored_speech,
        missed,
        false_alarm,
        speaker_confusion,
        der,
        reference_speakers: refs.len(),
        hypothesis_speakers: hyps.len(),
        speaker_count_error: hyps.len() as i64 - refs.len() as i64,
        mapping,
    })
}

#[derive(serde::Deserialize)]
struct Manifest {
    items: Vec<ManifestItem>,
}

#[derive(serde::Deserialize)]
struct ManifestItem {
    name: String,
    recording: Option<String>,
    audio: String,
    rttm: String,
    language: String,
    // A Value keeps this field required while allowing explicit JSON null.
    num_speakers: serde_json::Value,
}

impl ManifestItem {
    fn recording_id(&self) -> &str {
        self.recording.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct BenchmarkRow {
    item: String,
    language: String,
    model: String,
    mode: String,
    clustering_threshold: f32,
    seconds: f64,
    #[serde(flatten)]
    report: DerReport,
}

#[derive(serde::Serialize)]
struct MeanDer {
    group: String,
    name: String,
    mode: String,
    runs: usize,
    der: f64,
}

#[derive(serde::Serialize)]
struct BenchmarkResults {
    provider: String,
    scoring: DerOptions,
    rows: Vec<BenchmarkRow>,
    means: Vec<MeanDer>,
}

fn required_env(name: &str) -> Result<String> {
    let value = std::env::var(name).map_err(|_| anyhow!("Set {name}"))?;
    ensure!(!value.trim().is_empty(), "Set {name} to a non-empty value");
    Ok(value)
}

fn benchmark_diarization_config(
    segmentation: &Path,
    embedding: &Path,
    descriptor: &DiarizationModelDescriptor,
    explicit_num_speakers: Option<i32>,
    provider: &str,
) -> SherpaDiarizationConfig {
    // Explicit model files have no download descriptors in production either.
    let model_paths = DiarizationModelPaths {
        segmentation_model: segmentation.to_path_buf(),
        embedding_model: embedding.to_path_buf(),
        segmentation_descriptor: None,
        embedding_descriptor: None,
        can_download_segmentation: false,
        can_download_embedding: false,
    };
    diarization_config_for_provider(
        &model_paths,
        explicit_num_speakers,
        provider,
        descriptor.default_clustering_threshold,
    )
}

fn manifest_path(directory: &Path, relative: &str, field: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    ensure!(
        !relative.trim().is_empty()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "Manifest {field} must be a relative path inside the evaluation directory"
    );
    let resolved = directory
        .join(path)
        .canonicalize()
        .map_err(|_| anyhow!("Manifest {field} file is missing or inaccessible"))?;
    ensure!(
        resolved.starts_with(directory) && resolved.is_file(),
        "Manifest {field} must name a file inside the evaluation directory"
    );
    Ok(resolved)
}

fn markdown_cell(value: &str) -> String {
    value.replace(['\r', '\n'], " ").replace('|', "\\|")
}

fn results_markdown(results: &BenchmarkResults) -> String {
    let mut text = format!(
        "Provider: {}. Collar: {:.2} s; skip overlap: {}. Time includes model initialization, inference and turn preparation; excludes decoding and scoring.\n\n\
         | item | language | model | mode | ref spk | hyp spk | DER | miss | FA | confusion | seconds |\n\
         | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
        markdown_cell(&results.provider), results.scoring.collar_seconds, results.scoring.skip_overlap
    );
    for row in &results.rows {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {:.2}% | {:.3} | {:.3} | {:.3} | {:.3} |\n",
            markdown_cell(&row.item),
            markdown_cell(&row.language),
            markdown_cell(&row.model),
            row.mode,
            row.report.reference_speakers,
            row.report.hypothesis_speakers,
            row.report.der * 100.0,
            row.report.missed,
            row.report.false_alarm,
            row.report.speaker_confusion,
            row.seconds
        ));
    }
    text.push_str("\nUnweighted mean DER per model and per language (modes kept separate):\n\n| group | name | mode | runs | mean DER |\n| --- | --- | --- | ---: | ---: |\n");
    for mean in &results.means {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {:.2}% |\n",
            mean.group,
            markdown_cell(&mean.name),
            mean.mode,
            mean.runs,
            mean.der * 100.0
        ));
    }
    text
}

#[test]
#[ignore = "needs audio, models and CLAWSCRIBE_DIARIZATION_EVAL_DIR"]
fn diarization_accuracy_benchmark() -> Result<()> {
    let directory = match std::env::var_os("CLAWSCRIBE_DIARIZATION_EVAL_DIR") {
        Some(value) => PathBuf::from(value),
        None => {
            println!(
                "Skipping diarization accuracy benchmark: CLAWSCRIBE_DIARIZATION_EVAL_DIR is unset"
            );
            return Ok(());
        }
    };
    ensure!(
        !directory.as_os_str().is_empty(),
        "CLAWSCRIBE_DIARIZATION_EVAL_DIR must name a directory"
    );
    let directory = directory
        .canonicalize()
        .map_err(|_| anyhow!("CLAWSCRIBE_DIARIZATION_EVAL_DIR is missing or inaccessible"))?;
    ensure!(
        directory.is_dir(),
        "CLAWSCRIBE_DIARIZATION_EVAL_DIR must name a directory"
    );
    let manifest = std::fs::read_to_string(directory.join("manifest.json")).map_err(|_| {
        anyhow!("manifest.json is missing or unreadable in CLAWSCRIBE_DIARIZATION_EVAL_DIR")
    })?;
    let manifest: Manifest = serde_json::from_str(&manifest).map_err(|_| anyhow!(
        "manifest.json requires items with name, audio, rttm, language and num_speakers (number or null)"
    ))?;
    ensure!(
        !manifest.items.is_empty(),
        "manifest.json items must not be empty"
    );
    let segmentation = PathBuf::from(required_env("CLAWSCRIBE_DIARIZATION_SEGMENTATION_MODEL")?);
    ensure!(
        segmentation.is_file(),
        "CLAWSCRIBE_DIARIZATION_SEGMENTATION_MODEL must name an existing ONNX file"
    );
    let embedding_specs = required_env("CLAWSCRIBE_DIARIZATION_EMBEDDING_MODELS")?;
    let mut embeddings = Vec::new();
    let mut ids = BTreeSet::new();
    for spec in embedding_specs.split(',') {
        let (id, path) = spec.trim().split_once('=').ok_or_else(|| {
            anyhow!(
            "CLAWSCRIBE_DIARIZATION_EMBEDDING_MODELS requires comma-separated catalog-id=path pairs"
        )
        })?;
        let descriptor = model_descriptor_by_id(EMBEDDING_MODEL_CATALOG, id)
            .filter(|d| d.id == id.trim())
            .ok_or_else(|| {
                anyhow!("Embedding model ID must be an id in EMBEDDING_MODEL_CATALOG")
            })?;
        ensure!(
            ids.insert(descriptor.id),
            "Embedding model IDs must be distinct"
        );
        let path = PathBuf::from(path.trim());
        ensure!(
            path.is_file(),
            "Each embedding model path must name an existing ONNX file"
        );
        embeddings.push((descriptor, path));
    }
    let provider = match std::env::var("CLAWSCRIBE_DIARIZATION_EVAL_PROVIDER") {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        Ok(_) => bail!("CLAWSCRIBE_DIARIZATION_EVAL_PROVIDER must not be empty"),
        Err(std::env::VarError::NotPresent) => "cpu".into(),
        Err(_) => bail!("CLAWSCRIBE_DIARIZATION_EVAL_PROVIDER must be valid text"),
    };
    let thresholds = match std::env::var("CLAWSCRIBE_DIARIZATION_EVAL_THRESHOLDS") {
        Ok(value) => Some(
            value
                .split(',')
                .map(|t| t.trim().parse::<f32>().ok().filter(|t| (0.0..=2.0).contains(t)))
                .collect::<Option<Vec<_>>>()
                .filter(|values| !values.is_empty())
                .ok_or_else(|| {
                    anyhow!("CLAWSCRIBE_DIARIZATION_EVAL_THRESHOLDS requires comma-separated numbers in 0..2")
                })?,
        ),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => bail!("CLAWSCRIBE_DIARIZATION_EVAL_THRESHOLDS must be valid text"),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| anyhow!("Could not create the evaluation Tokio runtime"))?;
    let mut results = BenchmarkResults {
        provider: provider.clone(),
        scoring: DerOptions::default(),
        rows: Vec::new(),
        means: Vec::new(),
    };
    let mut names = BTreeSet::new();
    for item in manifest.items {
        ensure!(
            !item.name.trim().is_empty() && !item.language.trim().is_empty(),
            "Manifest name and language must not be empty"
        );
        ensure!(
            names.insert(item.name.clone()),
            "Manifest item names must be distinct"
        );
        let explicit = if item.num_speakers.is_null() {
            None
        } else {
            let count = item
                .num_speakers
                .as_u64()
                .filter(|&n| (1..=12).contains(&n))
                .ok_or_else(|| {
                    anyhow!("Manifest num_speakers must be null or an integer in 1..12")
                })?;
            Some(count as i32)
        };
        let audio = manifest_path(&directory, &item.audio, "audio")?;
        let rttm = manifest_path(&directory, &item.rttm, "rttm")?;
        let reference = std::fs::read_to_string(rttm)
            .map_err(|_| anyhow!("Could not read manifest RTTM file"))?;
        let reference = parse_rttm(&reference)?;
        let reference = select_recording(&reference, item.recording_id())?;
        let samples = runtime
            .block_on(super::super::batch_audio::prepare_diarization_samples(
                &audio,
                Arc::new(AtomicBool::new(false)),
            ))
            .map_err(|_| {
                anyhow!(
                    "Audio preparation failed for item {}",
                    markdown_cell(&item.name)
                )
            })?;
        ensure!(
            !samples.is_empty(),
            "Evaluation audio has no decodable samples"
        );
        for (descriptor, embedding) in &embeddings {
            // A sweep only varies Auto; a fixed count ignores the threshold.
            let runs = match &thresholds {
                Some(values) => values.iter().map(|&t| (None, Some(t))).collect::<Vec<_>>(),
                None => std::iter::once(None)
                    .chain(explicit.map(Some))
                    .map(|count| (count, None))
                    .collect(),
            };
            for (count, threshold) in runs {
                let mut config = benchmark_diarization_config(
                    &segmentation,
                    embedding,
                    descriptor,
                    count,
                    &provider,
                );
                if let Some(threshold) = threshold {
                    config.clustering_threshold = threshold;
                }
                let clustering_threshold = config.clustering_threshold;
                let started = Instant::now();
                let diarizer = SherpaOfflineDiarizer::new(config).map_err(|_| {
                    anyhow!("Could not initialize diarization model {}", descriptor.id)
                })?;
                let turns = diarizer
                    .diarize(&samples)
                    .map_err(|_| anyhow!("Diarization failed for model {}", descriptor.id))?;
                let turns = prepare_diarization_turns_for_mapping(samples.len(), &turns, count);
                let seconds = started.elapsed().as_secs_f64();
                let report = score_der(&reference, &turns, results.scoring)?;
                results.rows.push(BenchmarkRow {
                    item: item.name.clone(),
                    language: item.language.clone(),
                    model: descriptor.id.into(),
                    mode: match (count, threshold) {
                        (Some(_), _) => "known count".into(),
                        (None, Some(t)) => format!("auto {t:.2}"),
                        (None, None) => "auto".into(),
                    },
                    clustering_threshold,
                    seconds,
                    report,
                });
            }
        }
    }
    let mut means: BTreeMap<(&str, &str, &str), (f64, usize)> = BTreeMap::new();
    for row in &results.rows {
        for key in [
            ("model", row.model.as_str(), row.mode.as_str()),
            ("language", row.language.as_str(), row.mode.as_str()),
        ] {
            let total = means.entry(key).or_default();
            total.0 += row.report.der;
            total.1 += 1;
        }
    }
    results.means = means
        .into_iter()
        .map(|((group, name, mode), (sum, runs))| MeanDer {
            group: group.into(),
            name: name.into(),
            mode: mode.into(),
            runs,
            der: sum / runs as f64,
        })
        .collect();
    let markdown = results_markdown(&results);
    println!("{markdown}");
    if let Some(out) = std::env::var_os("CLAWSCRIBE_DIARIZATION_EVAL_OUT") {
        ensure!(
            !out.is_empty(),
            "CLAWSCRIBE_DIARIZATION_EVAL_OUT must name a directory"
        );
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out)
            .map_err(|_| anyhow!("Could not create evaluation output directory"))?;
        let json = serde_json::to_vec_pretty(&results)
            .map_err(|_| anyhow!("Could not serialize evaluation results"))?;
        std::fs::write(out.join("results.json"), json)
            .map_err(|_| anyhow!("Could not write results.json"))?;
        std::fs::write(out.join("results.md"), markdown)
            .map_err(|_| anyhow!("Could not write results.md"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(start: f64, end: f64, speaker: &str) -> ReferenceSegment {
        ReferenceSegment {
            recording: "mix".into(),
            channel: "1".into(),
            start,
            end,
            speaker: speaker.into(),
        }
    }

    fn turn(start_time: f64, end_time: f64, speaker: usize) -> DiarizationTurn {
        DiarizationTurn {
            start_time,
            end_time,
            speaker,
        }
    }

    fn options() -> DerOptions {
        DerOptions {
            collar_seconds: 0.0,
            skip_overlap: false,
        }
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    fn metrics(report: &DerReport, speech: f64, miss: f64, fa: f64, confusion: f64, der: f64) {
        close(report.scored_speech, speech);
        close(report.missed, miss);
        close(report.false_alarm, fa);
        close(report.speaker_confusion, confusion);
        close(report.der, der);
    }

    #[test]
    fn perfect_and_permuted_labels() {
        let refs = [reference(0.0, 2.0, "a"), reference(2.0, 4.0, "b")];
        for labels in [[0, 1], [1, 0]] {
            let report = score_der(
                &refs,
                &[turn(0.0, 2.0, labels[0]), turn(2.0, 4.0, labels[1])],
                options(),
            )
            .unwrap();
            // Four reference seconds, all correctly mapped, even under a full swap.
            metrics(&report, 4.0, 0.0, 0.0, 0.0, 0.0);
            assert_eq!(
                report.mapping,
                vec![("a".into(), labels[0]), ("b".into(), labels[1])]
            );
        }
    }

    #[test]
    fn pure_confusion() {
        let report = score_der(
            &[reference(0.0, 2.0, "a"), reference(2.0, 4.0, "b")],
            &[
                turn(0.0, 1.0, 0),
                turn(1.0, 2.0, 1),
                turn(2.0, 3.0, 0),
                turn(3.0, 4.0, 1),
            ],
            options(),
        )
        .unwrap();
        // Each mapping gets only one second per speaker: confusion 2 / speech 4.
        metrics(&report, 4.0, 0.0, 0.0, 2.0, 0.5);
    }

    #[test]
    fn pure_miss_and_false_alarm() {
        // Reference 4 s, hypothesis 3 s: miss = 1, DER = 1/4.
        metrics(
            &score_der(&[reference(0.0, 4.0, "a")], &[turn(0.0, 3.0, 0)], options()).unwrap(),
            4.0,
            1.0,
            0.0,
            0.0,
            0.25,
        );
        // Reference 2 s, hypothesis extends 1 s on both sides: FA = 2, DER = 1.
        metrics(
            &score_der(&[reference(1.0, 3.0, "a")], &[turn(0.0, 4.0, 0)], options()).unwrap(),
            2.0,
            0.0,
            2.0,
            0.0,
            1.0,
        );
        // Empty hypothesis misses all 4 s: DER = 4/4 = 1.
        metrics(
            &score_der(&[reference(0.0, 4.0, "a")], &[], options()).unwrap(),
            4.0,
            4.0,
            0.0,
            0.0,
            1.0,
        );
    }

    #[test]
    fn collar_forgives_shifted_boundary() {
        let refs = [reference(0.0, 2.0, "a"), reference(2.0, 4.0, "b")];
        let hyp = [turn(0.0, 2.2, 0), turn(2.2, 4.0, 1)];
        // No collar: 0.2 s confused / 4 s = 0.05.
        metrics(
            &score_der(&refs, &hyp, options()).unwrap(),
            4.0,
            0.0,
            0.0,
            0.2,
            0.05,
        );
        // +/-0.25 at 0, 2, 4 leaves [0.25,1.75] + [2.25,3.75] = 3 s.
        metrics(
            &score_der(&refs, &hyp, DerOptions::default()).unwrap(),
            3.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
    }

    #[test]
    fn overlap_is_counted_per_reference_speaker_or_excluded() {
        let refs = [reference(0.0, 4.0, "a"), reference(2.0, 6.0, "b")];
        let hyp = [turn(0.0, 4.0, 0), turn(4.0, 6.0, 1)];
        // 8 speaker-seconds, 2 missed during [2,4]: DER = 2/8.
        metrics(
            &score_der(&refs, &hyp, options()).unwrap(),
            8.0,
            2.0,
            0.0,
            0.0,
            0.25,
        );
        // Dropping [2,4] leaves four correctly attributed speaker-seconds.
        metrics(
            &score_der(
                &refs,
                &hyp,
                DerOptions {
                    skip_overlap: true,
                    ..options()
                },
            )
            .unwrap(),
            4.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        // Both overlapping speakers detected: all eight speaker-seconds correct.
        metrics(
            &score_der(&refs, &[turn(0.0, 4.0, 0), turn(2.0, 6.0, 1)], options()).unwrap(),
            8.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
    }

    #[test]
    fn unequal_counts_split_and_merge() {
        let split = score_der(
            &[reference(0.0, 4.0, "a")],
            &[turn(0.0, 2.0, 0), turn(2.0, 4.0, 1)],
            options(),
        )
        .unwrap();
        // One of two hypothesis labels must remain unmapped: 2/4 confused.
        metrics(&split, 4.0, 0.0, 0.0, 2.0, 0.5);
        assert_eq!(
            (
                split.reference_speakers,
                split.hypothesis_speakers,
                split.speaker_count_error
            ),
            (1, 2, 1)
        );
        let merged = score_der(
            &[reference(0.0, 2.0, "a"), reference(2.0, 4.0, "b")],
            &[turn(0.0, 4.0, 0)],
            options(),
        )
        .unwrap();
        // One reference speaker cannot map to the merged label: 2/4 confused.
        metrics(&merged, 4.0, 0.0, 0.0, 2.0, 0.5);
        assert_eq!(merged.speaker_count_error, -1);
        // Extra simultaneous unmapped hypothesis: 4 FA / 4 reference = 1.
        metrics(
            &score_der(
                &[reference(0.0, 4.0, "a")],
                &[turn(0.0, 4.0, 0), turn(0.0, 4.0, 1)],
                options(),
            )
            .unwrap(),
            4.0,
            0.0,
            4.0,
            0.0,
            1.0,
        );
    }

    #[test]
    fn optimal_assignment_beats_greedy() {
        // Weights a->0=9, a->1=8, b->0=8, b->1=0. Greedy gets 9;
        // optimum a->1,b->0 gets 16 of 25 seconds: confusion 9, DER 9/25.
        let report = score_der(
            &[reference(0.0, 17.0, "a"), reference(17.0, 25.0, "b")],
            &[turn(0.0, 9.0, 0), turn(9.0, 17.0, 1), turn(17.0, 25.0, 0)],
            options(),
        )
        .unwrap();
        metrics(&report, 25.0, 0.0, 0.0, 9.0, 9.0 / 25.0);
        assert_eq!(report.mapping, vec![("a".into(), 1), ("b".into(), 0)]);
    }

    #[test]
    fn sub_frame_boundaries_and_sparse_speaker_ids() {
        // Reference 1.3 ms; hypothesis [0.2,1.1] ms misses 0.4 ms.
        // DER = 0.4/1.3 = 4/13; no time is rounded to an analysis frame.
        let report = score_der(
            &[reference(0.0, 0.0013, "a")],
            &[turn(0.0002, 0.0011, 42)],
            options(),
        )
        .unwrap();
        metrics(&report, 0.0013, 0.0004, 0.0, 0.0, 4.0 / 13.0);
        assert_eq!(report.mapping, vec![("a".into(), 42)]);
    }

    #[test]
    fn benchmark_config_matches_production_for_auto_and_known_counts() {
        let model_paths = super::super::DiarizationModelPaths {
            segmentation_model: PathBuf::from("segmentation.onnx"),
            embedding_model: PathBuf::from("embedding.onnx"),
            segmentation_descriptor: None,
            embedding_descriptor: None,
            can_download_segmentation: false,
            can_download_embedding: false,
        };
        for descriptor in EMBEDDING_MODEL_CATALOG {
            for provider in ["cpu", "directml"] {
                for (count, min_on, min_off) in
                    [(None, 0.3, 0.5), (Some(4), 0.25, 0.25), (Some(1), 0.3, 0.5)]
                {
                    let benchmark = benchmark_diarization_config(
                        &model_paths.segmentation_model,
                        &model_paths.embedding_model,
                        descriptor,
                        count,
                        provider,
                    );
                    let production = super::super::diarization_config_for_provider(
                        &model_paths,
                        count,
                        provider,
                        descriptor.default_clustering_threshold,
                    );
                    assert_eq!(benchmark.min_duration_on, production.min_duration_on);
                    assert_eq!(benchmark.min_duration_off, production.min_duration_off);
                    assert_eq!(benchmark.num_clusters, production.num_clusters);
                    assert_eq!(
                        benchmark.clustering_threshold,
                        production.clustering_threshold
                    );
                    assert_eq!(benchmark.provider, production.provider);
                    assert_eq!(
                        benchmark.segmentation_model_path,
                        production.segmentation_model_path
                    );
                    assert_eq!(
                        benchmark.embedding_model_path,
                        production.embedding_model_path
                    );
                    assert_eq!(benchmark.min_duration_on, min_on);
                    assert_eq!(benchmark.min_duration_off, min_off);
                }
            }
        }
    }

    #[test]
    fn select_recording_keeps_perfect_hypothesis_at_zero_der() {
        let segments = parse_rttm(
            "SPEAKER first 1 0 2 <NA> <NA> speaker-a <NA> <NA>\n\
             SPEAKER second 2 0 2 <NA> <NA> speaker-b <NA> <NA>\n",
        )
        .unwrap();
        let hypothesis = [turn(0.0, 2.0, 0)];
        // Merging the unrelated recordings would miss 2 of 4 speaker-seconds.
        close(
            score_der(&segments, &hypothesis, options()).unwrap().der,
            0.5,
        );
        let selected = select_recording(&segments, "first").unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].recording, "first");
        assert_eq!(selected[0].channel, "1");
        assert_eq!(selected[0].speaker, "speaker-a");
        metrics(
            &score_der(&selected, &hypothesis, options()).unwrap(),
            2.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
    }

    #[test]
    fn select_recording_rejects_unknown_recording() {
        let segments = parse_rttm("SPEAKER mix 1 0 2 <NA> <NA> a <NA> <NA>\n").unwrap();
        let error = select_recording(&segments, "missing")
            .unwrap_err()
            .to_string();
        assert!(error.contains("No RTTM segments"), "{error}");
        assert!(error.contains("missing"), "{error}");
    }

    #[test]
    fn select_recording_rejects_multiple_channels() {
        let segments = parse_rttm(
            "SPEAKER mix 1 0 2 <NA> <NA> a <NA> <NA>\n\
             SPEAKER mix 2 0 2 <NA> <NA> b <NA> <NA>\n",
        )
        .unwrap();
        let error = select_recording(&segments, "mix").unwrap_err().to_string();
        assert!(
            error.contains("multi-channel files are not supported"),
            "{error}"
        );
    }

    #[test]
    fn manifest_recording_defaults_to_name_and_accepts_override() {
        for (recording_field, expected) in [("", "mix"), (r#", "recording": "source""#, "source")] {
            let item: ManifestItem = serde_json::from_str(&format!(
                r#"{{"name":"mix","audio":"mix.wav","rttm":"combined.rttm","language":"de","num_speakers":null{recording_field}}}"#
            ))
            .unwrap();
            assert_eq!(item.recording_id(), expected);
        }
    }

    #[test]
    fn rttm_success_and_line_numbered_failures() {
        let segments = parse_rttm(
            "\n;; comment\nLEXEME ignored\nSPEAKER mix 1 1.25 2.5 <NA> <NA> speaker-a <NA> <NA>\n",
        )
        .unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].recording, "mix");
        assert_eq!(segments[0].channel, "1");
        assert_eq!(segments[0].speaker, "speaker-a");
        close(segments[0].start, 1.25);
        close(segments[0].end, 3.75);
        for line in [
            "SPEAKER mix 1 0",
            "SPEAKER mix 1 -1 2 x x a x x",
            "SPEAKER mix 1 NaN 2 x x a x x",
            "SPEAKER mix 1 inf 2 x x a x x",
            "SPEAKER mix 1 nope 2 x x a x x",
            "SPEAKER mix 1 0 0 x x a x x",
            "SPEAKER mix 1 0 -1 x x a x x",
            "SPEAKER mix 1 0 NaN x x a x x",
            "SPEAKER mix 1 0 inf x x a x x",
            "SPEAKER mix 1 0 nope x x a x x",
            "SPEAKER mix 1 1e308 1e308 x x a x x",
        ] {
            let error = parse_rttm(&format!(";; ignored\n{line}"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("line 2"), "{error}");
            assert!(!error.contains(line));
        }
    }

    #[test]
    fn speaker_limit_and_invalid_inputs() {
        let refs: Vec<_> = (0..13)
            .map(|s| reference(s as f64, s as f64 + 1.0, &format!("s{s}")))
            .collect();
        let hyp: Vec<_> = (0..13).map(|s| turn(s as f64, s as f64 + 1.0, s)).collect();
        assert!(score_der(&refs, &[], options()).is_err());
        // Any number of hypothesis speakers is scored: the thirteenth turn is a
        // one-second false alarm beside twelve correct seconds.
        metrics(
            &score_der(&refs[..12], &hyp, options()).unwrap(),
            12.0,
            0.0,
            1.0,
            0.0,
            1.0 / 12.0,
        );
        // Twelve is supported exactly; 12/12 seconds correct.
        metrics(
            &score_der(&refs[..12], &hyp[..12], options()).unwrap(),
            12.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        assert!(score_der(&[reference(-1.0, 1.0, "a")], &[], options()).is_err());
        assert!(score_der(&[], &[turn(0.0, f64::NAN, 0)], options()).is_err());
        assert!(score_der(
            &[],
            &[],
            DerOptions {
                collar_seconds: -1.0,
                ..options()
            }
        )
        .is_err());
        assert!(score_der(
            &[],
            &[],
            DerOptions {
                collar_seconds: f64::NAN,
                ..options()
            }
        )
        .is_err());
        // No speech and no errors: 0; no speech with 1 s FA: undefined, rejected.
        metrics(
            &score_der(&[], &[], options()).unwrap(),
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        assert!(score_der(&[], &[turn(0.0, 1.0, 0)], options()).is_err());
    }
}
