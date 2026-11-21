use super::common::estimate_word_timestamps;
use super::vocabulary::{normalize_custom_words, Vocabulary, DEFAULT_THRESHOLD};
use crate::api::{TranscriptSegment, TranscriptWord, TranscriptWordTimestampSource};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};

static PREFERENCE_LOCK: Mutex<()> = Mutex::new(());
const STORE: &str = "custom_vocabulary.json";
pub(crate) const MAX_STORE_BYTES: u64 = 256 * 1024;

pub(crate) fn portable_path(app_data: &Path) -> PathBuf {
    app_data.join(STORE)
}

fn preference_guard(lock: &Mutex<()>) -> std::sync::MutexGuard<'_, ()> {
    // The mutex protects file operations, not in-memory preference data.
    // An earlier panic must not permanently disable reading or saving.
    lock.lock().unwrap_or_else(|error| error.into_inner())
}

// Keep the existing Tauri-store JSON shape, but read without registering an
// exit-time auto-save. Recovery must never rewrite the file merely by reading.
pub(crate) fn read_saved_words(path: &Path) -> (Vec<String>, bool) {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), false),
        Err(_) => return (Vec::new(), true),
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_STORE_BYTES
    {
        return (Vec::new(), true);
    }
    let value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(serde_json::Value::Object(value)) => value,
        _ => return (Vec::new(), true),
    };
    let entries = match value.get("words") {
        None => return (Vec::new(), false),
        Some(serde_json::Value::Array(entries)) => entries,
        _ => return (Vec::new(), true),
    };
    let mut repaired = entries.len() > 500;
    let mut words = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        if words.len() == 500 {
            break;
        }
        let Some(entry) = entry.as_str() else {
            repaired = true;
            continue;
        };
        match normalize_custom_words(vec![entry.to_string()]) {
            Ok(valid) => {
                for word in valid {
                    if seen.insert(word.to_lowercase()) && words.len() < 500 {
                        words.push(word);
                    }
                }
            }
            Err(_) => repaired = true,
        }
    }
    (words, repaired)
}

fn save_words(path: &Path, words: &[String]) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let persist = || -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(&serde_json::json!({ "words": words }))?;
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    };
    if persist().is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err("Could not save custom vocabulary. Retry the action.".into());
    }
    Ok(())
}

#[tauri::command]
pub fn get_custom_vocabulary<R: Runtime>(app: AppHandle<R>) -> Result<Vec<String>, String> {
    Ok(read_words(&app))
}

fn read_words<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    let _guard = preference_guard(&PREFERENCE_LOCK);
    let (words, repaired) = match app.path().app_data_dir() {
        Ok(directory) => read_saved_words(&directory.join(STORE)),
        Err(_) => (Vec::new(), true),
    };
    if repaired {
        log::warn!("Some saved custom vocabulary could not be loaded; using valid entries only");
    }
    words
}

#[tauri::command]
pub fn set_custom_vocabulary<R: Runtime>(
    app: AppHandle<R>,
    words: Vec<String>,
) -> Result<Vec<String>, String> {
    let _archive = crate::library::backup::PORTABLE_SETTINGS_LOCK
        .try_lock()
        .map_err(|_| "Wait for the library restore to finish.")?;
    let words = normalize_custom_words(words)?;
    let _guard = preference_guard(&PREFERENCE_LOCK);
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "Could not open custom vocabulary. Retry in transcription settings.")?;
    save_words(&directory.join(STORE), &words)?;
    Ok(words)
}

pub(super) fn load_vocabulary<R: Runtime>(app: &AppHandle<R>) -> Vocabulary {
    Vocabulary::new(read_words(app))
}

pub(super) fn correct_segments(vocabulary: &Vocabulary, segments: &mut [TranscriptSegment]) {
    if vocabulary.is_empty() {
        return;
    }
    for segment in segments {
        correct_transcript(
            vocabulary,
            &mut segment.text,
            &mut segment.word_timestamps,
            segment.audio_start_time.unwrap_or(0.0),
            segment.audio_end_time.unwrap_or(0.0),
        );
    }
}

/// Apply one edit plan to text and anchors. Never run matching on the anchors
/// independently: n-gram decisions must be identical to the displayed text.
pub(super) fn correct_transcript(
    vocabulary: &Vocabulary,
    text: &mut String,
    timestamps: &mut Option<Vec<TranscriptWord>>,
    start: f64,
    end: f64,
) {
    if vocabulary.is_empty() {
        return;
    }
    let (corrected, replacements) = vocabulary.correct(text, DEFAULT_THRESHOLD);
    if replacements.is_empty() {
        return;
    }
    if let Some(words) = timestamps.as_ref() {
        let speaker = words.first().and_then(|first| {
            words
                .iter()
                .all(|w| w.speaker == first.speaker)
                .then_some(first.speaker.as_deref())
                .flatten()
        });
        let tokens = text.split_whitespace().collect::<Vec<_>>();
        let aligned = words.len() == tokens.len()
            && words.iter().zip(&tokens).all(|(word, token)| {
                word.text.trim_matches(|c: char| !c.is_alphanumeric())
                    == token.trim_matches(|c: char| !c.is_alphanumeric())
            });
        if aligned {
            let mut output = Vec::new();
            let mut cursor = 0;
            for replacement in replacements {
                // Also copy punctuation into otherwise unchanged provider anchors.
                for index in cursor..replacement.start {
                    let mut word = words[index].clone();
                    word.text = tokens[index].to_string();
                    output.push(word);
                }
                let source = &words[replacement.start..replacement.end];
                let mut merged = source[0].clone();
                merged.start = source.iter().map(|w| w.start).fold(f64::INFINITY, f64::min);
                merged.end = source
                    .iter()
                    .map(|w| w.end)
                    .fold(f64::NEG_INFINITY, f64::max);
                merged.confidence = source
                    .iter()
                    .try_fold(1.0_f32, |value, w| w.confidence.map(|c| value.min(c)));
                if source.iter().any(|w| w.speaker != merged.speaker) {
                    merged.speaker = None;
                }
                if source
                    .iter()
                    .any(|w| w.timestamp_source != Some(TranscriptWordTimestampSource::Real))
                {
                    merged.timestamp_source = Some(TranscriptWordTimestampSource::Estimated);
                }
                if replacement.text.split_whitespace().count() == 1 {
                    merged.text = replacement.text;
                    output.push(merged);
                } else if let Some(split) = estimate_word_timestamps(
                    &replacement.text,
                    merged.start,
                    merged.end,
                    merged.confidence,
                    merged.speaker.as_deref(),
                ) {
                    // A replacement phrase has no native internal word boundaries.
                    output.extend(split);
                } else {
                    // Native tokens can have equal start/end timestamps. A
                    // phrase cannot be split inside that zero-length span.
                    *timestamps = estimate_word_timestamps(&corrected, start, end, None, speaker);
                    *text = corrected;
                    return;
                }
                cursor = replacement.end;
            }
            for index in cursor..words.len() {
                let mut word = words[index].clone();
                word.text = tokens[index].to_string();
                output.push(word);
            }
            *timestamps = Some(output);
        } else {
            // Provider tokens can disagree with sentence text. Do not retain
            // stale anchors or label newly distributed positions as native.
            *timestamps = estimate_word_timestamps(&corrected, start, end, None, speaker);
        }
    }
    *text = corrected;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_preference_lock_does_not_block_further_operations() {
        let lock = Mutex::new(());
        let _ = std::panic::catch_unwind(|| {
            let _guard = lock.lock().unwrap();
            panic!("simulate interrupted preference operation");
        });
        assert!(lock.is_poisoned());
        drop(preference_guard(&lock));
        drop(preference_guard(&lock));
    }

    fn word(text: &str, start: f64, end: f64) -> TranscriptWord {
        TranscriptWord {
            text: text.into(),
            start,
            end,
            confidence: Some(0.9),
            speaker: Some("Speaker 1".into()),
            timestamp_source: Some(TranscriptWordTimestampSource::Real),
        }
    }

    #[test]
    fn joined_words_merge_native_ranges_and_keep_unaffected_anchors() {
        let vocabulary = Vocabulary::new(vec!["ChargeBee".into()]);
        let mut text = "Use Charge B, today.".to_string();
        let mut words = Some(vec![
            word("Use", 1.0, 1.5),
            word("Charge", 1.5, 2.0),
            word("B", 2.0, 2.4),
            word("today", 2.4, 3.0),
        ]);
        correct_transcript(&vocabulary, &mut text, &mut words, 1.0, 3.0);
        assert_eq!(text, "Use ChargeBee, today.");
        let words = words.unwrap();
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            vec!["Use", "ChargeBee,", "today."]
        );
        assert_eq!((words[1].start, words[1].end), (1.5, 2.4));
        assert_eq!(
            words[1].timestamp_source,
            Some(TranscriptWordTimestampSource::Real)
        );
        assert_eq!((words[2].start, words[2].end), (2.4, 3.0));
    }

    #[test]
    fn phrase_replacement_estimates_only_its_internal_boundaries() {
        let vocabulary = Vocabulary::new(vec!["MacBook Pro".into()]);
        let mut text = "Use Macbookpro today".to_string();
        let mut words = Some(vec![
            word("Use", 1.0, 1.5),
            word("Macbookpro", 1.5, 2.5),
            word("today", 2.5, 3.0),
        ]);
        correct_transcript(&vocabulary, &mut text, &mut words, 1.0, 3.0);
        assert_eq!(text, "Use MacBook Pro today");
        let words = words.unwrap();
        assert_eq!(words.len(), 4);
        assert_eq!(words[1].text, "MacBook");
        assert_eq!(words[2].text, "Pro");
        assert_eq!((words[1].start, words[2].end), (1.5, 2.5));
        assert_eq!(
            words[1].timestamp_source,
            Some(TranscriptWordTimestampSource::Estimated)
        );
        assert_eq!(
            words[3].timestamp_source,
            Some(TranscriptWordTimestampSource::Real)
        );
    }

    #[test]
    fn mismatched_anchors_are_replaced_by_explicitly_estimated_words() {
        let vocabulary = Vocabulary::new(vec!["ChargeBee".into()]);
        let mut text = "Charge B works".to_string();
        let mut words = Some(vec![word("different", 0.0, 2.0)]);
        correct_transcript(&vocabulary, &mut text, &mut words, 0.0, 3.0);
        let words = words.unwrap();
        assert_eq!(text, "ChargeBee works");
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            vec!["ChargeBee", "works"]
        );
        assert!(words
            .iter()
            .all(|w| w.timestamp_source == Some(TranscriptWordTimestampSource::Estimated)));
        assert_eq!((words[0].start, words[1].end), (0.0, 3.0));
    }

    #[test]
    fn empty_vocabulary_does_not_touch_text_or_anchors() {
        let mut text = "  Charge B\n".to_string();
        let original = Some(vec![word("Charge B", 0.0, 1.0)]);
        let mut words = original.clone();
        correct_transcript(&Vocabulary::new(vec![]), &mut text, &mut words, 0.0, 1.0);
        assert_eq!(text, "  Charge B\n");
        assert_eq!(words, original);
    }

    #[test]
    fn phrase_expansion_from_zero_duration_uses_estimated_segment_anchors() {
        let vocabulary = Vocabulary::new(vec!["MacBook Pro".into()]);
        let mut text = "Macbookpro works".to_string();
        let mut words = Some(vec![word("Macbookpro", 1.0, 1.0), word("works", 1.0, 2.0)]);
        correct_transcript(&vocabulary, &mut text, &mut words, 1.0, 2.0);
        let words = words.unwrap();
        assert_eq!(text, "MacBook Pro works");
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            vec!["MacBook", "Pro", "works"]
        );
        assert_eq!((words[0].start, words[2].end), (1.0, 2.0));
        assert!(words
            .iter()
            .all(|w| w.timestamp_source == Some(TranscriptWordTimestampSource::Estimated)));
        assert!(words
            .iter()
            .all(|w| w.speaker.as_deref() == Some("Speaker 1")));
    }

    #[test]
    fn unchanged_canonical_suffixes_leave_native_anchors_untouched() {
        let vocabulary = Vocabulary::new(vec!["Robert".into(), "ChargeBee".into()]);
        let mut text = "Robert's ChargeBees".to_string();
        let original = Some(vec![
            word("Robert's", 0.0, 1.0),
            word("ChargeBees", 1.0, 2.0),
        ]);
        let mut words = original.clone();
        correct_transcript(&vocabulary, &mut text, &mut words, 0.0, 2.0);
        assert_eq!(text, "Robert's ChargeBees");
        assert_eq!(words, original);
    }

    #[test]
    fn zero_duration_fallback_does_not_assign_a_mixed_speaker() {
        let vocabulary = Vocabulary::new(vec!["MacBook Pro".into()]);
        let mut text = "Macbookpro works".to_string();
        let mut other_speaker = word("works", 1.0, 2.0);
        other_speaker.speaker = Some("Speaker 2".into());
        let mut words = Some(vec![word("Macbookpro", 1.0, 1.0), other_speaker]);
        correct_transcript(&vocabulary, &mut text, &mut words, 1.0, 2.0);
        assert!(words.unwrap().iter().all(|w| w.speaker.is_none()));
    }

    fn temporary_store() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "clawscribe-vocabulary-test-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn missing_store_is_an_empty_list_without_repair() {
        let path = temporary_store();
        assert_eq!(read_saved_words(&path), (Vec::<String>::new(), false));
        assert!(!path.exists());
    }

    #[test]
    fn unreadable_store_returns_an_empty_list() {
        let path = temporary_store();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(read_saved_words(&path), (Vec::<String>::new(), true));
        std::fs::remove_dir(path).unwrap();
    }

    #[test]
    fn failed_save_keeps_the_previous_file() {
        let path = temporary_store();
        let temporary = path.with_extension("json.tmp");
        let words = vec!["Müller".into()];
        save_words(&path, &words).unwrap();
        let previous = std::fs::read(&path).unwrap();
        std::fs::create_dir(&temporary).unwrap();
        assert!(save_words(&path, &["Zoë".into()]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        std::fs::remove_dir(temporary).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_and_non_array_stores_are_empty_and_not_overwritten_by_reading() {
        let path = temporary_store();
        for content in ["not json", "[]", r#"{"words":"invalid"}"#] {
            std::fs::write(&path, content).unwrap();
            assert_eq!(read_saved_words(&path), (Vec::<String>::new(), true));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stored_invalid_entries_are_dropped_and_valid_entries_retained() {
        let path = temporary_store();
        let value = serde_json::json!({ "words": [" Müller ", "ü".repeat(65), "+++", 42, "two\nlines", "Zoë", "MÜLLER"] });
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            read_saved_words(&path),
            (vec!["Müller".into(), "Zoë".into()], true)
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stored_lists_over_the_limit_are_truncated() {
        let path = temporary_store();
        let words = (0..505).map(|i| format!("word{i}")).collect::<Vec<_>>();
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({ "words": words })).unwrap(),
        )
        .unwrap();
        let (loaded, repaired) = read_saved_words(&path);
        assert_eq!(loaded, words[..500]);
        assert!(repaired);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn save_overwrites_corrupt_file_and_invalid_words_value() {
        let path = temporary_store();
        let words = vec!["Müller".into(), "ChargeBee".into()];
        for content in [
            "not json",
            r#"{"words":42}"#,
            r#"{"words":["+++","stale"]}"#,
        ] {
            std::fs::write(&path, content).unwrap();
            save_words(&path, &words).unwrap();
            assert_eq!(read_saved_words(&path), (words.clone(), false));
        }
        std::fs::remove_file(path).unwrap();
    }
}
