use crate::api::{TranscriptSegment, TranscriptWord, TranscriptWordTimestampSource};
use crate::summary::processor::language_name_from_code;
use anyhow::Result;
use log::{debug, info};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

static ENGINE_LIFECYCLE_LOCK: Lazy<Arc<AsyncMutex<()>>> =
    Lazy::new(|| Arc::new(AsyncMutex::new(())));

pub(crate) fn transcription_source_language_hint(
    provider: Option<&str>,
    language: Option<&str>,
) -> Option<String> {
    if let Some(language) = normalise_fixed_language(language) {
        return Some(language);
    }

    match provider.map(normalise_provider_name).as_deref() {
        Some("parakeet") => Some("en".to_string()),
        _ => None,
    }
}

fn normalise_fixed_language(language: Option<&str>) -> Option<String> {
    let code = language?.trim().to_ascii_lowercase().replace('_', "-");
    if code.is_empty() || matches!(code.as_str(), "auto" | "auto-translate") {
        return None;
    }

    language_name_from_code(&code)?;
    Some(code)
}

fn normalise_provider_name(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "localwhisper" | "whisper" => "localwhisper".to_string(),
        "parakeet" => "parakeet".to_string(),
        "nemotron" => "nemotron".to_string(),
        other => other.to_string(),
    }
}

pub(crate) async fn acquire_engine_lifecycle_lock() -> OwnedMutexGuard<()> {
    ENGINE_LIFECYCLE_LOCK.clone().lock_owned().await
}

/// Unload the transcription engine after a batch job (import or retranscription).
/// Skips unloading if a live recording is currently in progress, since recording
/// uses the same global engine instances.
pub(crate) async fn unload_engine_after_batch_for(use_parakeet: bool, use_nemotron: bool) {
    let _engine_lifecycle_guard = acquire_engine_lifecycle_lock().await;

    if crate::audio::recording_commands::is_recording().await {
        log::info!("Skipping model unload after batch: recording in progress");
        return;
    }

    if use_nemotron {
        use crate::nemotron_engine::commands::NEMOTRON_ENGINE;
        let engine = {
            let guard = NEMOTRON_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    } else if use_parakeet {
        use crate::parakeet_engine::commands::PARAKEET_ENGINE;
        let engine = {
            let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    } else {
        use crate::whisper_engine::commands::WHISPER_ENGINE;
        let engine = {
            let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    }
}

/// Create transcript segments from legacy tuple fixtures.
/// Each tuple is (text, start_ms, end_ms) from VAD timestamps.
#[cfg(test)]
pub(crate) fn create_transcript_segments(
    transcripts: &[(String, f64, f64)],
) -> Vec<TranscriptSegment> {
    let transcripts = transcripts
        .iter()
        .map(|(text, start_ms, end_ms)| TranscribedSegment {
            text: text.clone(),
            start_ms: *start_ms,
            end_ms: *end_ms,
            word_timestamps: None,
        })
        .collect::<Vec<_>>();
    create_transcript_segments_with_words(&transcripts)
}

#[derive(Debug, Clone)]
pub(crate) struct TranscribedSegment {
    pub text: String,
    pub start_ms: f64,
    pub end_ms: f64,
    pub word_timestamps: Option<Vec<TranscriptWord>>,
}

/// Create transcript segments from transcription results that may include
/// provider-native word anchors. Each segment carries absolute audio-relative
/// seconds for its words.
pub(crate) fn create_transcript_segments_with_words(
    transcripts: &[TranscribedSegment],
) -> Vec<TranscriptSegment> {
    transcripts
        .iter()
        .map(|segment| {
            let start_seconds = segment.start_ms / 1000.0;
            let end_seconds = segment.end_ms / 1000.0;
            let duration = end_seconds - start_seconds;

            TranscriptSegment {
                id: format!("transcript-{}", Uuid::new_v4()),
                text: segment.text.trim().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                audio_start_time: Some(start_seconds),
                audio_end_time: Some(end_seconds),
                duration: Some(duration),
                speaker: None,
                word_timestamps: segment.word_timestamps.clone(),
            }
        })
        .collect()
}

pub(crate) fn create_readable_transcript_segments_with_words(
    transcripts: &[TranscribedSegment],
) -> Vec<TranscriptSegment> {
    let stitched = stitch_transcript_fragments(transcripts);
    create_transcript_segments_with_words(&stitched)
}

fn stitch_transcript_fragments(transcripts: &[TranscribedSegment]) -> Vec<TranscribedSegment> {
    const MAX_STITCH_GAP_MS: f64 = 1_500.0;
    const MAX_STITCHED_DURATION_MS: f64 = 30_000.0;
    const SHORT_UTTERANCE_WORDS: usize = 7;
    const ORPHAN_FRAGMENT_WORDS: usize = 4;

    let mut stitched: Vec<TranscribedSegment> = Vec::new();

    for segment in transcripts.iter().map(clean_transcribed_segment) {
        if segment.text.trim().is_empty() {
            continue;
        }

        if let Some(previous) = stitched.last_mut() {
            let gap_ms = segment.start_ms - previous.end_ms;
            let combined_duration_ms = segment.end_ms - previous.start_ms;
            let should_stitch = gap_ms >= -250.0
                && gap_ms <= MAX_STITCH_GAP_MS
                && combined_duration_ms <= MAX_STITCHED_DURATION_MS
                && (is_incomplete_fragment(&previous.text)
                    || is_short_utterance(&previous.text, SHORT_UTTERANCE_WORDS)
                    || is_orphan_fragment(&segment.text, ORPHAN_FRAGMENT_WORDS)
                    || starts_like_continuation(&segment.text));

            if should_stitch {
                stitch_segment_into(previous, segment);
                continue;
            }
        }

        stitched.push(segment);
    }

    if stitched.len() != transcripts.len() {
        debug!(
            "Stitched transcript fragments for readability: {} -> {} segments",
            transcripts.len(),
            stitched.len()
        );
    }

    stitched
}

fn clean_transcribed_segment(segment: &TranscribedSegment) -> TranscribedSegment {
    let mut cleaned = segment.clone();
    cleaned.text = collapse_whitespace(&cleaned.text);

    if cleaned
        .text
        .get(..6)
        .map(|prefix| prefix.eq_ignore_ascii_case("th it "))
        .unwrap_or(false)
    {
        let can_fix_words = match cleaned.word_timestamps.as_mut() {
            Some(words) if words.len() >= 2 => {
                let first = words[0].text.trim_matches(is_word_edge_punctuation);
                let second = words[1].text.trim_matches(is_word_edge_punctuation);
                if first.eq_ignore_ascii_case("th") && second.eq_ignore_ascii_case("it") {
                    words[1].text = "It".to_string();
                    words[1].start = words[0].start.min(words[1].start);
                    words.remove(0);
                    true
                } else {
                    false
                }
            }
            Some(_) => false,
            None => true,
        };

        if can_fix_words {
            cleaned.text = format!("It {}", cleaned.text[6..].trim_start());
        }
    }

    cleaned
}

fn stitch_segment_into(previous: &mut TranscribedSegment, mut next: TranscribedSegment) {
    let previous_is_incomplete = is_incomplete_fragment(&previous.text);
    let next_text = if previous_is_incomplete {
        lowercase_continuation_start(&next.text, next.word_timestamps.as_mut())
    } else {
        next.text.clone()
    };

    previous.text = join_transcript_text(&previous.text, &next_text);
    previous.end_ms = previous.end_ms.max(next.end_ms);

    match (&mut previous.word_timestamps, next.word_timestamps) {
        (Some(previous_words), Some(mut next_words)) => {
            previous_words.append(&mut next_words);
        }
        (Some(_), None) => {
            previous.word_timestamps = None;
        }
        (None, _) => {}
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn join_transcript_text(left: &str, right: &str) -> String {
    match (left.trim(), right.trim()) {
        ("", right) => right.to_string(),
        (left, "") => left.to_string(),
        (left, right) => format!("{left} {right}"),
    }
}

fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|word| !word.is_empty())
        .count()
}

fn is_incomplete_fragment(text: &str) -> bool {
    !ends_with_terminal_punctuation(text)
}

fn is_short_utterance(text: &str, max_words: usize) -> bool {
    word_count(text) <= max_words
}

fn is_orphan_fragment(text: &str, max_words: usize) -> bool {
    word_count(text) <= max_words && !ends_with_terminal_punctuation(text)
}

fn starts_like_continuation(text: &str) -> bool {
    let first_word = text.split_whitespace().next().unwrap_or_default();
    if first_word.is_empty() {
        return false;
    }

    let normalized = first_word
        .trim_matches(is_word_edge_punctuation)
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "and"
            | "but"
            | "or"
            | "so"
            | "then"
            | "because"
            | "that"
            | "those"
            | "this"
            | "these"
            | "it"
            | "that's"
            | "they"
            | "where"
            | "who"
            | "which"
            | "when"
    ) || first_word
        .chars()
        .next()
        .map(|ch| ch.is_lowercase())
        .unwrap_or(false)
}

fn ends_with_terminal_punctuation(text: &str) -> bool {
    text.trim_end()
        .chars()
        .rev()
        .find(|ch| !matches!(ch, '"' | '\'' | ')' | ']' | '}'))
        .map(|ch| matches!(ch, '.' | '!' | '?' | ':' | ';'))
        .unwrap_or(false)
}

fn lowercase_continuation_start(text: &str, words: Option<&mut Vec<TranscriptWord>>) -> String {
    let trimmed = text.trim_start();
    let Some(first_word) = trimmed.split_whitespace().next() else {
        return text.to_string();
    };

    let bare_word = first_word.trim_matches(is_word_edge_punctuation);
    if bare_word.len() <= 1 || bare_word.chars().all(|ch| !ch.is_lowercase()) {
        return text.to_string();
    }
    if matches!(bare_word, "I" | "I'm" | "I've" | "I'd" | "I'll") {
        return text.to_string();
    }

    let Some(first_char) = bare_word.chars().next() else {
        return text.to_string();
    };
    if !first_char.is_uppercase() {
        return text.to_string();
    }

    let lower_first = first_char.to_lowercase().collect::<String>();
    let replacement = format!("{}{}", lower_first, &bare_word[first_char.len_utf8()..]);
    let updated = trimmed.replacen(bare_word, &replacement, 1);

    if let Some(words) = words {
        if let Some(word) = words.first_mut() {
            let word_bare = word.text.trim_matches(is_word_edge_punctuation);
            if word_bare == bare_word {
                word.text = word.text.replacen(bare_word, &replacement, 1);
            }
        }
    }

    if text.len() == trimmed.len() {
        updated
    } else {
        format!("{}{}", &text[..text.len() - trimmed.len()], updated)
    }
}

fn is_word_edge_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
}

pub(crate) fn transcript_words_from_token_timestamps(
    text: &str,
    tokens: &[String],
    timestamps: &[f32],
    segment_start_seconds: f64,
    segment_end_seconds: f64,
    confidence: Option<f32>,
    speaker: Option<&str>,
) -> Option<Vec<TranscriptWord>> {
    if text.trim().is_empty()
        || tokens.is_empty()
        || tokens.len() != timestamps.len()
        || !segment_start_seconds.is_finite()
        || !segment_end_seconds.is_finite()
        || segment_end_seconds <= segment_start_seconds
    {
        return None;
    }

    let text_words = text
        .split_whitespace()
        .filter(|word| !word.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if text_words.is_empty() {
        return None;
    }

    let token_word_starts = token_word_start_times(
        tokens,
        timestamps,
        segment_start_seconds,
        segment_end_seconds,
    )?;
    if token_word_starts.len() != text_words.len() {
        return estimate_word_timestamps(
            text,
            segment_start_seconds,
            segment_end_seconds,
            confidence,
            speaker,
        );
    }

    let speaker = speaker.map(str::to_string);
    let mut words = Vec::with_capacity(text_words.len());
    for (index, word) in text_words.iter().enumerate() {
        let start = token_word_starts[index].clamp(segment_start_seconds, segment_end_seconds);
        let next_start = token_word_starts
            .get(index + 1)
            .copied()
            .unwrap_or(segment_end_seconds)
            .clamp(segment_start_seconds, segment_end_seconds);
        let end = next_start.max(start).min(segment_end_seconds);

        words.push(TranscriptWord {
            text: word.clone(),
            start,
            end,
            confidence,
            speaker: speaker.clone(),
            timestamp_source: Some(TranscriptWordTimestampSource::Real),
        });
    }

    Some(words)
}

fn token_word_start_times(
    tokens: &[String],
    timestamps: &[f32],
    segment_start_seconds: f64,
    segment_end_seconds: f64,
) -> Option<Vec<f64>> {
    let mut starts = Vec::<f64>::new();
    let mut in_word = false;

    for (token, timestamp) in tokens.iter().zip(timestamps.iter()) {
        if !timestamp.is_finite() {
            return None;
        }
        if token.trim().is_empty() {
            continue;
        }

        let starts_new_word = !in_word
            || token
                .chars()
                .next()
                .map(|ch| ch.is_whitespace())
                .unwrap_or(false);
        if starts_new_word {
            let absolute = (segment_start_seconds + *timestamp as f64)
                .clamp(segment_start_seconds, segment_end_seconds);
            starts.push(absolute);
            in_word = true;
        }
    }

    (!starts.is_empty()).then_some(starts)
}

pub(crate) fn estimate_word_timestamps(
    text: &str,
    start_time: f64,
    end_time: f64,
    confidence: Option<f32>,
    speaker: Option<&str>,
) -> Option<Vec<TranscriptWord>> {
    if !start_time.is_finite() || !end_time.is_finite() || end_time <= start_time {
        return None;
    }

    let words = text
        .split_whitespace()
        .filter(|word| !word.trim().is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return None;
    }

    let total_weight = words
        .iter()
        .map(|word| word.chars().filter(|ch| !ch.is_whitespace()).count().max(1) as f64)
        .sum::<f64>()
        .max(1.0);
    let duration = end_time - start_time;
    let mut cursor = start_time;
    let speaker = speaker.map(str::to_string);

    Some(
        words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                let weight = word.chars().filter(|ch| !ch.is_whitespace()).count().max(1) as f64;
                let word_start = cursor;
                let word_end = if index + 1 == words.len() {
                    end_time
                } else {
                    (cursor + duration * (weight / total_weight)).min(end_time)
                };
                cursor = word_end;

                TranscriptWord {
                    text: (*word).to_string(),
                    start: word_start,
                    end: word_end.max(word_start),
                    confidence,
                    speaker: speaker.clone(),
                    timestamp_source: Some(TranscriptWordTimestampSource::Estimated),
                }
            })
            .collect(),
    )
}

/// Write transcripts.json to a meeting folder (atomic write with temp file)
pub(crate) fn write_transcripts_json(folder: &Path, segments: &[TranscriptSegment]) -> Result<()> {
    let transcript_path = folder.join("transcripts.json");
    let temp_path = folder.join(".transcripts.json.tmp");

    let json = serde_json::json!({
        "version": "1.0",
        "last_updated": chrono::Utc::now().to_rfc3339(),
        "total_segments": segments.len(),
        "segments": segments.iter().enumerate().map(|(i, s)| {
            serde_json::json!({
                "id": s.id,
                "text": s.text,
                "speaker": s.speaker,
                "timestamp": s.timestamp,
                "audio_start_time": s.audio_start_time,
                "audio_end_time": s.audio_end_time,
                "duration": s.duration,
                "sequence_id": i,
                "word_timestamps": s.word_timestamps.clone()
            })
        }).collect::<Vec<_>>()
    });

    let json_string = serde_json::to_string_pretty(&json)?;
    std::fs::write(&temp_path, &json_string)?;
    std::fs::rename(&temp_path, &transcript_path)?;

    info!(
        "Wrote transcripts.json with {} segments to {}",
        segments.len(),
        transcript_path.display()
    );
    Ok(())
}

/// Split a long speech segment at the lowest-energy (silence) point near the target size.
///
/// Scans for 100ms windows with minimal RMS energy within +/-3 seconds of each target
/// split point. If no clear silence is found, falls back to a 1-second overlap split
/// to avoid cutting words at boundaries.
pub(crate) fn split_segment_at_silence(
    segment: &crate::audio::vad::SpeechSegment,
    max_samples: usize,
) -> Vec<crate::audio::vad::SpeechSegment> {
    const SAMPLE_RATE: usize = 16000;
    // 100ms window for energy measurement (1600 samples at 16kHz)
    const ENERGY_WINDOW: usize = SAMPLE_RATE / 10;
    // Search +/-3 seconds around the target split point
    const SEARCH_RADIUS: usize = SAMPLE_RATE * 3;
    // RMS threshold below which we consider a window "silent"
    const SILENCE_RMS_THRESHOLD: f32 = 0.02;
    // Overlap to use when no silence boundary is found (1 second)
    const FALLBACK_OVERLAP: usize = SAMPLE_RATE;

    let total = segment.samples.len();
    if total <= max_samples {
        return vec![segment.clone()];
    }

    let ms_per_sample =
        (segment.end_timestamp_ms - segment.start_timestamp_ms) / segment.samples.len() as f64;
    let mut result = Vec::new();
    let mut pos = 0usize;

    while pos < total {
        let remaining = total - pos;
        if remaining <= max_samples {
            // Last chunk - take everything remaining
            let chunk_samples = segment.samples[pos..].to_vec();
            let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
            let chunk_end_ms = segment.end_timestamp_ms;
            result.push(crate::audio::vad::SpeechSegment {
                samples: chunk_samples,
                start_timestamp_ms: chunk_start_ms,
                end_timestamp_ms: chunk_end_ms,
                confidence: segment.confidence,
            });
            break;
        }

        // Target split point
        let target = pos + max_samples;

        // Search window: [target - SEARCH_RADIUS, target + SEARCH_RADIUS]
        let search_start = target.saturating_sub(SEARCH_RADIUS).max(pos + SAMPLE_RATE);
        let search_end = (target + SEARCH_RADIUS).min(total.saturating_sub(ENERGY_WINDOW));

        // Find the lowest-energy 100ms window in the search range
        let mut best_split = target.min(total); // fallback: exact target
        let mut best_rms = f32::MAX;

        if search_start + ENERGY_WINDOW <= search_end {
            let mut idx = search_start;
            while idx + ENERGY_WINDOW <= search_end {
                let window = &segment.samples[idx..idx + ENERGY_WINDOW];
                let rms = (window.iter().map(|s| s * s).sum::<f32>() / ENERGY_WINDOW as f32).sqrt();
                if rms < best_rms {
                    best_rms = rms;
                    best_split = idx + ENERGY_WINDOW / 2; // split at center of quiet window
                }
                // Step by 10ms (160 samples) for efficiency
                idx += SAMPLE_RATE / 100;
            }
        }

        let split_at = best_split;
        if best_rms <= SILENCE_RMS_THRESHOLD {
            debug!(
                "Splitting at silence boundary: sample {} (RMS={:.4})",
                split_at, best_rms
            );
        } else {
            debug!(
                "No silence found near target (best RMS={:.4}), splitting with overlap at sample {}",
                best_rms, split_at
            );
        }

        // Determine the actual end of this chunk (with overlap if no silence)
        let chunk_end = if best_rms > SILENCE_RMS_THRESHOLD {
            (split_at + FALLBACK_OVERLAP).min(total)
        } else {
            split_at
        };

        let chunk_samples = segment.samples[pos..chunk_end].to_vec();
        let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
        let chunk_end_ms = segment.start_timestamp_ms + (chunk_end as f64 * ms_per_sample);

        result.push(crate::audio::vad::SpeechSegment {
            samples: chunk_samples,
            start_timestamp_ms: chunk_start_ms,
            end_timestamp_ms: chunk_end_ms,
            confidence: segment.confidence,
        });

        // Advance position to where the current chunk actually ends
        // to avoid transcribing the overlap region twice
        pos = chunk_end;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_language_hint_uses_fixed_language_for_multilingual_engines() {
        assert_eq!(
            transcription_source_language_hint(Some("localWhisper"), Some("de")),
            Some("de".to_string())
        );
        assert_eq!(
            transcription_source_language_hint(Some("nemotron"), Some("zh-cn")),
            Some("zh-cn".to_string())
        );
        assert_eq!(
            transcription_source_language_hint(Some("parakeet"), Some("fr")),
            Some("fr".to_string())
        );
    }

    #[test]
    fn source_language_hint_handles_auto_modes_by_provider() {
        assert_eq!(
            transcription_source_language_hint(Some("parakeet"), Some("auto-translate")),
            Some("en".to_string())
        );
        assert_eq!(
            transcription_source_language_hint(Some("parakeet"), None),
            Some("en".to_string())
        );
        assert_eq!(
            transcription_source_language_hint(Some("nemotron"), Some("auto")),
            None
        );
        assert_eq!(
            transcription_source_language_hint(Some("localWhisper"), Some("auto-translate")),
            None
        );
    }

    #[test]
    fn source_language_hint_rejects_unknown_language_codes() {
        assert_eq!(
            transcription_source_language_hint(Some("nemotron"), Some("not-a-language")),
            None
        );
    }

    #[test]
    fn token_timestamps_create_absolute_word_anchors() {
        let tokens = vec![
            "It".to_string(),
            " seems".to_string(),
            " like".to_string(),
            " drones".to_string(),
        ];
        let timestamps = vec![0.0, 0.4, 0.8, 1.6];

        let words = transcript_words_from_token_timestamps(
            "It seems like drones",
            &tokens,
            &timestamps,
            10.0,
            13.0,
            None,
            Some("Speaker 1"),
        )
        .unwrap();

        assert_eq!(words.len(), 4);
        assert_eq!(words[0].text, "It");
        assert_eq!(words[0].start, 10.0);
        assert!((words[1].start - 10.4).abs() < 0.001);
        assert_eq!(words[3].end, 13.0);
        assert!(words
            .iter()
            .all(|word| word.speaker.as_deref() == Some("Speaker 1")));
    }

    #[test]
    fn token_timestamp_mismatch_falls_back_to_word_count_safe_estimate() {
        let tokens = vec!["Citizens".to_string()];
        let timestamps = vec![0.0];

        let words = transcript_words_from_token_timestamps(
            "Citizens United ruling",
            &tokens,
            &timestamps,
            2.0,
            5.0,
            Some(0.9),
            None,
        )
        .unwrap();

        assert_eq!(words.len(), 3);
        assert_eq!(words[0].text, "Citizens");
        assert_eq!(words[2].end, 5.0);
    }

    fn test_word(text: &str, start: f64, end: f64) -> TranscriptWord {
        TranscriptWord {
            text: text.to_string(),
            start,
            end,
            confidence: None,
            speaker: None,
            timestamp_source: Some(TranscriptWordTimestampSource::Real),
        }
    }

    fn test_transcribed_segment(
        text: &str,
        start_ms: f64,
        end_ms: f64,
        words: &[&str],
    ) -> TranscribedSegment {
        let duration = ((end_ms - start_ms) / 1000.0).max(0.001);
        let start = start_ms / 1000.0;
        let step = duration / words.len().max(1) as f64;
        TranscribedSegment {
            text: text.to_string(),
            start_ms,
            end_ms,
            word_timestamps: Some(
                words
                    .iter()
                    .enumerate()
                    .map(|(index, word)| {
                        let word_start = start + step * index as f64;
                        test_word(word, word_start, word_start + step)
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn readable_segments_stitch_vad_fragments_without_losing_word_anchors() {
        let transcripts = vec![
            test_transcribed_segment(
                "Th it seems like there is a pretty clear point.",
                0.0,
                13_000.0,
                &[
                    "Th", "it", "seems", "like", "there", "is", "a", "pretty", "clear", "point.",
                ],
            ),
            test_transcribed_segment(
                "What about the military drones?",
                13_000.0,
                15_000.0,
                &["What", "about", "the", "military", "drones?"],
            ),
            test_transcribed_segment(
                "Those contracts when this war started",
                15_000.0,
                18_000.0,
                &["Those", "contracts", "when", "this", "war", "started"],
            ),
            test_transcribed_segment("That's", 18_000.0, 20_000.0, &["That's"]),
            test_transcribed_segment(
                "You can look into who made money.",
                20_000.0,
                24_000.0,
                &["You", "can", "look", "into", "who", "made", "money."],
            ),
        ];

        let segments = create_readable_transcript_segments_with_words(&transcripts);

        assert_eq!(segments.len(), 2);
        assert_eq!(
            segments[0].text,
            "It seems like there is a pretty clear point."
        );
        assert_eq!(
            segments[1].text,
            "What about the military drones? Those contracts when this war started that's you can look into who made money."
        );
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(13.0));
        assert_eq!(segments[1].audio_start_time, Some(13.0));
        assert_eq!(segments[1].audio_end_time, Some(24.0));

        let first_words = segments[0].word_timestamps.as_ref().unwrap();
        assert_eq!(first_words[0].text, "It");
        assert_eq!(first_words.len(), 9);

        let second_words = segments[1].word_timestamps.as_ref().unwrap();
        assert_eq!(second_words.len(), 19);
        assert_eq!(second_words[0].text, "What");
        assert_eq!(second_words[11].text, "that's");
        assert_eq!(second_words[12].text, "you");
        assert!(second_words
            .iter()
            .all(|word| word.timestamp_source == Some(TranscriptWordTimestampSource::Real)));
    }

    #[test]
    fn readable_segments_do_not_stitch_complete_rows_across_long_gap() {
        let transcripts = vec![
            test_transcribed_segment("First complete sentence.", 0.0, 1_000.0, &["First"]),
            test_transcribed_segment("Second complete sentence.", 4_000.0, 5_000.0, &["Second"]),
        ];

        let segments = create_readable_transcript_segments_with_words(&transcripts);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "First complete sentence.");
        assert_eq!(segments[1].text, "Second complete sentence.");
    }

    #[tokio::test]
    async fn test_engine_lifecycle_lock_serializes_acquirers() {
        let guard = acquire_engine_lifecycle_lock().await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async {
            started_tx.send(()).unwrap();
            let _guard = acquire_engine_lifecycle_lock().await;
            acquired_tx.send(()).unwrap();
        });

        started_rx.await.unwrap();
        assert!(acquired_rx.try_recv().is_err());
        drop(guard);

        acquired_rx.await.unwrap();
        waiter.await.unwrap();
    }
}
