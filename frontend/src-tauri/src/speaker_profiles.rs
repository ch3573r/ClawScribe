//! Voice vectors stay in this module and the local database. IPC only returns
//! names, preferences and the availability of a sample for explicit enrollment.
use crate::audio::diarization::{
    set_transcript_segment_speaker, speaker_label, DiarizationTurn, TranscriptSegment,
};
use crate::state::AppState;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};
use sqlx::{SqliteConnection, SqlitePool};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Runtime, State};

#[cfg(test)]
mod tests;

const SAMPLE_RATE: i32 = 16_000;
const WINDOW_SECONDS: f64 = 3.0;
const MAX_WINDOWS: usize = 3;
const MAX_TURNS_PER_LANE: usize = 64;
const MAX_CANDIDATE_LANES: usize = 16;
const MAX_EMBEDDING_DIM: usize = 2048;
const MIN_SIMILARITY: f32 = 0.85;
const MIN_MATCH_MARGIN: f32 = 0.08;
const MIN_WINDOW_AGREEMENT: f32 = 0.65;
const READ_ERROR: &str = "Could not load speaker profiles.";
const SAVE_ERROR: &str = "Could not save the speaker profile.";
const STALE_SAMPLE: &str = "This voice sample is no longer available. Run speaker detection again.";
const CHANGED: &str = "speaker-profiles-changed";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, sqlx::FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preferences {
    enabled: bool,
    auto_apply: bool,
    #[serde(skip)]
    generation: i64,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct ProfileName {
    key: String,
    name: String,
}

#[derive(Serialize)]
pub(crate) struct ProfileSettings {
    preferences: Preferences,
    profiles: Vec<ProfileName>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpeakerVoiceOption {
    speaker: String,
    can_remember: bool,
    suggested_name: Option<String>,
}

#[cfg_attr(test, derive(Clone))]
pub(crate) struct DetectionSamples {
    generation: i64,
    model: String,
    voices: BTreeMap<String, Vec<f32>>,
}

type Fingerprint = Vec<u8>;

pub(crate) struct DetectionCache {
    meeting: String,
    generation: i64,
    model: String,
    lanes: BTreeMap<String, (Fingerprint, Vec<f32>)>,
}

struct AvailableSamples {
    minimum_generation: i64,
    detection: Option<DetectionCache>,
}

static AVAILABLE: Mutex<AvailableSamples> = Mutex::new(AvailableSamples {
    minimum_generation: 0,
    detection: None,
});

#[cfg_attr(test, derive(Clone))]
struct KnownVoice {
    name_key: String,
    name: String,
    embedding: Vec<f32>,
}

async fn read_preferences(conn: &mut SqliteConnection) -> sqlx::Result<Preferences> {
    sqlx::query_as(
        "SELECT enabled, auto_apply, generation FROM speaker_profile_preferences WHERE id = 1",
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn capture_generation(pool: &SqlitePool) -> Option<i64> {
    let mut conn = pool.acquire().await.ok()?;
    let preferences = read_preferences(&mut conn).await.ok()?;
    preferences.enabled.then_some(preferences.generation)
}

async fn update_preferences(
    conn: &mut SqliteConnection,
    enabled: bool,
    auto_apply: bool,
) -> sqlx::Result<Preferences> {
    let preferences = sqlx::query_as(
        "UPDATE speaker_profile_preferences SET generation = generation + (enabled <> ?),
         enabled = ?, auto_apply = ? WHERE id = 1 RETURNING enabled, auto_apply, generation",
    )
    .bind(enabled)
    .bind(enabled)
    .bind(enabled && auto_apply)
    .fetch_one(&mut *conn)
    .await?;
    if !enabled {
        sqlx::query("DELETE FROM speaker_profile_suggestions")
            .execute(conn)
            .await?;
    }
    Ok(preferences)
}

#[tauri::command]
pub(crate) async fn get_speaker_profile_settings(
    state: State<'_, AppState>,
) -> Result<ProfileSettings, String> {
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|_| READ_ERROR)?;
    let preferences = read_preferences(&mut tx).await.map_err(|_| READ_ERROR)?;
    let profiles = sqlx::query_as(
        "SELECT name_key AS key, MIN(name) AS name FROM speaker_profiles GROUP BY name_key ORDER BY name_key",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| READ_ERROR)?;
    Ok(ProfileSettings {
        preferences,
        profiles,
    })
}

#[tauri::command]
pub(crate) async fn set_speaker_profile_preferences<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    enabled: bool,
    auto_apply: bool,
) -> Result<Preferences, String> {
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|_| SAVE_ERROR)?;
    let preferences = update_preferences(&mut tx, enabled, auto_apply)
        .await
        .map_err(|_| SAVE_ERROR)?;
    tx.commit().await.map_err(|_| SAVE_ERROR)?;
    if !enabled {
        let mut cache = AVAILABLE.lock().unwrap_or_else(|err| err.into_inner());
        cache.minimum_generation = cache.minimum_generation.max(preferences.generation);
        if cache
            .detection
            .as_ref()
            .is_some_and(|run| run.generation < cache.minimum_generation)
        {
            cache.detection = None;
        }
    }
    let _ = app.emit(CHANGED, ());
    Ok(preferences)
}

#[tauri::command]
pub(crate) async fn delete_speaker_profile<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    key: String,
) -> Result<(), String> {
    sqlx::query("DELETE FROM speaker_profiles WHERE name_key = ?")
        .bind(key)
        .execute(state.db_manager.pool())
        .await
        .map_err(|_| "Could not delete the speaker profile.")?;
    let _ = app.emit(CHANGED, ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn get_meeting_speaker_voice_options(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<SpeakerVoiceOption>, String> {
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|_| READ_ERROR)?;
    let preferences = read_preferences(&mut tx).await.map_err(|_| READ_ERROR)?;
    if !preferences.enabled {
        return Ok(Vec::new());
    }

    let cached_lanes: BTreeMap<String, Fingerprint> = {
        let cache = AVAILABLE.lock().unwrap_or_else(|err| err.into_inner());
        cache
            .detection
            .as_ref()
            .filter(|run| run.meeting == meeting_id && run.generation == preferences.generation)
            .map(|run| {
                run.lanes
                    .iter()
                    .map(|(speaker, (fingerprint, _))| (speaker.clone(), fingerprint.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let suggestions = current_suggestions(&mut tx, &meeting_id)
        .await
        .map_err(|_| READ_ERROR)?;
    let mut options: BTreeMap<String, SpeakerVoiceOption> = suggestions
        .into_iter()
        .map(|(speaker, name)| {
            (
                speaker.clone(),
                SpeakerVoiceOption {
                    speaker,
                    suggested_name: Some(name),
                    can_remember: false,
                },
            )
        })
        .collect();
    for (speaker, fingerprint) in cached_lanes {
        if saved_fingerprint(&mut tx, &meeting_id, &speaker)
            .await
            .map_err(|_| READ_ERROR)?
            == Some(fingerprint)
        {
            options
                .entry(speaker.clone())
                .or_insert_with(|| SpeakerVoiceOption {
                    speaker,
                    ..Default::default()
                })
                .can_remember = true;
        }
    }
    Ok(options.into_values().collect())
}

async fn current_suggestions(
    conn: &mut SqliteConnection,
    meeting: &str,
) -> sqlx::Result<BTreeMap<String, String>> {
    let rows: Vec<(String, Fingerprint, String)> = sqlx::query_as(
        "SELECT s.speaker, s.fingerprint, p.name FROM speaker_profile_suggestions s
         JOIN speaker_profiles p ON p.name_key = s.name_key AND p.model = s.model
         WHERE s.meeting_id = ?",
    )
    .bind(meeting)
    .fetch_all(&mut *conn)
    .await?;
    let mut matched = BTreeMap::new();
    for (speaker, fingerprint, name) in rows {
        if saved_fingerprint(conn, meeting, &speaker).await? == Some(fingerprint) {
            matched.insert(speaker, name);
        }
    }
    Ok(matched)
}

/// Run with the rename in the same transaction so a failed or stale rename
/// cannot enroll a voice. Never accept an embedding from IPC.
pub(crate) async fn remember_voice(
    conn: &mut SqliteConnection,
    meeting: &str,
    speaker: Option<&str>,
    name: Option<&str>,
) -> sqlx::Result<()> {
    let invalid = |message: &str| sqlx::Error::Protocol(message.into());
    let speaker = speaker.ok_or_else(|| invalid(STALE_SAMPLE))?;
    let name = name
        .filter(|value| is_person_name(value))
        .ok_or_else(|| invalid("Enter a person's name to remember this voice."))?;
    let preferences = read_preferences(conn).await?;
    if !preferences.enabled {
        return Err(invalid(
            "Enable speaker profiles in Transcription settings first.",
        ));
    }
    let sample = {
        let cache = AVAILABLE.lock().unwrap_or_else(|err| err.into_inner());
        cache
            .detection
            .as_ref()
            .filter(|run| run.meeting == meeting && run.generation == preferences.generation)
            .and_then(|run| {
                run.lanes.get(speaker).map(|(fingerprint, voice)| {
                    (run.model.clone(), fingerprint.clone(), voice.clone())
                })
            })
    };
    let (model, fingerprint, voice) = sample.ok_or_else(|| invalid(STALE_SAMPLE))?;
    if saved_fingerprint(conn, meeting, speaker).await? != Some(fingerprint) {
        return Err(invalid(STALE_SAMPLE));
    }
    let names: Vec<String> = sqlx::query_scalar("SELECT DISTINCT speaker FROM transcripts WHERE meeting_id = ? AND speaker IS NOT NULL AND speaker <> ?")
        .bind(meeting).bind(speaker).fetch_all(&mut *conn).await?;
    if names
        .iter()
        .any(|existing| existing.to_lowercase() == name.to_lowercase())
    {
        return Err(invalid(
            "A merged speaker label cannot be used to remember a voice.",
        ));
    }
    save_voice(conn, name, &model, &voice).await
}

fn is_person_name(name: &str) -> bool {
    let name = name.to_lowercase();
    !name.is_empty()
        && !matches!(name.as_str(), "me" | "participants" | "unlabeled")
        && !name.strip_prefix("speaker ").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

async fn save_voice(
    conn: &mut SqliteConnection,
    name: &str,
    model: &str,
    voice: &[f32],
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO speaker_profiles (name_key, model, name, embedding) VALUES (?, ?, ?, ?) ON CONFLICT(name_key, model) DO NOTHING")
        .bind(name.to_lowercase()).bind(model).bind(name).bind(encode_embedding(voice))
        .execute(conn).await?;
    Ok(())
}

pub(crate) fn offer_detection_sample(cache: Option<DetectionCache>) {
    let mut available = AVAILABLE.lock().unwrap_or_else(|err| err.into_inner());
    available.detection = cache.filter(|run| run.generation >= available.minimum_generation);
}

pub(crate) async fn finish_detection(
    conn: &mut SqliteConnection,
    meeting: &str,
    segments: &mut [TranscriptSegment],
    capture: Option<DetectionSamples>,
) -> sqlx::Result<Option<DetectionCache>> {
    sqlx::query("DELETE FROM speaker_profile_suggestions WHERE meeting_id = ?")
        .bind(meeting)
        .execute(&mut *conn)
        .await?;
    let Some(capture) = capture else {
        return Ok(None);
    };
    let preferences = read_preferences(conn).await?;
    if !preferences.enabled || capture.generation != preferences.generation {
        return Ok(None);
    }

    let mut lanes: BTreeMap<String, Vec<LaneRow>> = BTreeMap::new();
    let mut used_names = BTreeSet::new();
    for segment in segments.iter() {
        if let Some(label) = &segment.speaker {
            used_names.insert(label.to_lowercase());
            if capture.voices.contains_key(label) {
                lanes.entry(label.clone()).or_default().push((
                    segment.id.clone(),
                    segment.audio_start_time,
                    segment.audio_end_time,
                ));
            }
        }
    }
    let profiles: Vec<(String, String, Vec<u8>)> =
        sqlx::query_as("SELECT name_key, name, embedding FROM speaker_profiles WHERE model = ?")
            .bind(&capture.model)
            .fetch_all(&mut *conn)
            .await?;
    let profiles: Vec<_> = profiles
        .into_iter()
        .filter_map(|(name_key, name, bytes)| {
            Some(KnownVoice {
                name_key,
                name,
                embedding: decode_embedding(&bytes)?,
            })
        })
        .collect();
    let matches = unambiguous_matches(&capture.voices, &profiles, &used_names);
    let mut eligible = BTreeMap::new();
    for (speaker, voice) in capture.voices {
        let Some(rows) = lanes.remove(&speaker) else {
            continue;
        };
        let fingerprint = fingerprint(rows);
        match matches.get(&speaker) {
            Some(profile) if preferences.auto_apply => {
                for segment in segments
                    .iter_mut()
                    .filter(|row| row.speaker.as_deref() == Some(speaker.as_str()))
                {
                    set_transcript_segment_speaker(segment, &profile.name);
                }
                continue;
            }
            Some(profile) => {
                sqlx::query("INSERT INTO speaker_profile_suggestions (meeting_id, speaker, fingerprint, name_key, model) VALUES (?, ?, ?, ?, ?)")
                    .bind(meeting).bind(&speaker).bind(&fingerprint).bind(&profile.name_key).bind(&capture.model)
                    .execute(&mut *conn).await?;
            }
            None => {}
        }
        eligible.insert(speaker, (fingerprint, voice));
    }
    Ok(Some(DetectionCache {
        meeting: meeting.into(),
        generation: capture.generation,
        model: capture.model,
        lanes: eligible,
    }))
}

type LaneRow = (String, Option<f64>, Option<f64>);

fn fingerprint(mut rows: Vec<LaneRow>) -> Fingerprint {
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Sha256::new();
    for (id, start, end) in rows {
        hash.update((id.len() as u64).to_le_bytes());
        hash.update(id);
        for boundary in [start, end] {
            hash.update([u8::from(boundary.is_some())]);
            if let Some(value) = boundary {
                hash.update(value.to_le_bytes());
            }
        }
    }
    hash.finalize().to_vec()
}

async fn saved_fingerprint(
    conn: &mut SqliteConnection,
    meeting: &str,
    speaker: &str,
) -> sqlx::Result<Option<Fingerprint>> {
    let rows: Vec<LaneRow> = sqlx::query_as("SELECT id, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ? AND speaker = ?")
        .bind(meeting).bind(speaker).fetch_all(conn).await?;
    Ok((!rows.is_empty()).then(|| fingerprint(rows)))
}

fn unambiguous_matches<'a>(
    voices: &BTreeMap<String, Vec<f32>>,
    profiles: &'a [KnownVoice],
    used_names: &BTreeSet<String>,
) -> BTreeMap<String, &'a KnownVoice> {
    let mut frequencies = BTreeMap::new();
    let matches: Vec<_> = voices
        .iter()
        .filter_map(|(label, voice)| {
            let profile = best_match(voice, profiles)?;
            *frequencies.entry(&profile.name_key).or_insert(0) += 1;
            Some((label, profile))
        })
        .collect();
    matches
        .into_iter()
        .filter(|(_, profile)| {
            frequencies[&profile.name_key] == 1 && !used_names.contains(&profile.name_key)
        })
        .map(|(label, profile)| (label.clone(), profile))
        .collect()
}

fn best_match<'a>(voice: &[f32], profiles: &'a [KnownVoice]) -> Option<&'a KnownVoice> {
    let mut top: Option<(f32, &KnownVoice)> = None;
    let mut runner_up = -1.0_f32;
    for profile in profiles
        .iter()
        .filter(|profile| profile.embedding.len() == voice.len())
    {
        let score = cosine(voice, &profile.embedding);
        if top.is_none_or(|(best, _)| score > best) {
            if let Some((best, _)) = top {
                runner_up = best;
            }
            top = Some((score, profile));
        } else {
            runner_up = runner_up.max(score);
        }
    }
    top.filter(|(score, _)| *score >= MIN_SIMILARITY && *score - runner_up >= MIN_MATCH_MARGIN)
        .map(|(_, profile)| profile)
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn unit_vector(embedding: &[f32]) -> Option<Vec<f32>> {
    if embedding.is_empty() || embedding.len() > MAX_EMBEDDING_DIM {
        return None;
    }
    let norm = embedding
        .iter()
        .map(|&value| f64::from(value).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    Some(
        embedding
            .iter()
            .map(|&value| (f64::from(value) / norm) as f32)
            .collect(),
    )
}

fn centroid(windows: &[Vec<f32>]) -> Option<Vec<f32>> {
    if windows.len() < 2 {
        return None;
    }
    let windows: Vec<_> = windows
        .iter()
        .map(|window| unit_vector(window))
        .collect::<Option<_>>()?;
    let mut sum = vec![0.0; windows[0].len()];
    for (index, window) in windows.iter().enumerate() {
        if window.len() != sum.len()
            || windows[..index]
                .iter()
                .any(|other| cosine(window, other) < MIN_WINDOW_AGREEMENT)
        {
            return None;
        }
        for (value, component) in sum.iter_mut().zip(window) {
            *value += component;
        }
    }
    unit_vector(&sum)
}

fn encode_embedding(voice: &[f32]) -> Vec<u8> {
    voice.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn decode_embedding(bytes: &[u8]) -> Option<Vec<f32>> {
    let (components, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() || components.len() > MAX_EMBEDDING_DIM {
        return None;
    }
    unit_vector(
        &components
            .iter()
            .copied()
            .map(f32::from_le_bytes)
            .collect::<Vec<_>>(),
    )
}

fn sample_windows(
    turns: &[DiarizationTurn],
    speaker: usize,
    sample_count: usize,
) -> Vec<Range<usize>> {
    let sample_rate = f64::from(SAMPLE_RATE);
    let window_len = (WINDOW_SECONDS * sample_rate) as usize;
    let mut windows: Vec<Range<usize>> = Vec::new();
    for turn in turns
        .iter()
        .filter(|turn| turn.speaker == speaker && turn.end_time - turn.start_time >= WINDOW_SECONDS)
        .take(MAX_TURNS_PER_LANE)
    {
        if !turn.start_time.is_finite()
            || !turn.end_time.is_finite()
            || turns[..turns.partition_point(|other| other.start_time < turn.end_time)]
                .iter()
                .any(|other| other.speaker != speaker && other.end_time > turn.start_time)
        {
            continue;
        }
        let start = (turn.start_time * sample_rate).ceil() as usize;
        let end = ((turn.end_time * sample_rate).floor() as usize).min(sample_count);
        let first_free = windows.last().map_or(start, |last| last.end.max(start));
        for offset in (first_free..end.saturating_sub(window_len - 1)).step_by(window_len) {
            windows.push(offset..offset + window_len);
            if windows.len() == MAX_WINDOWS {
                return windows;
            }
        }
    }
    windows
}

/// Collect a few local, single-speaker windows from the winning analysis.
/// Sherpa's diarization API returns turn timings rather than its internal vectors.
pub(crate) fn extract_detection_samples(
    generation: i64,
    model: String,
    model_file: &Path,
    mut turns: Vec<DiarizationTurn>,
    lanes: BTreeMap<usize, usize>,
    samples: &[f32],
    cancelled: &AtomicBool,
) -> Option<DetectionSamples> {
    turns.retain(|turn| {
        turn.start_time.is_finite() && turn.end_time.is_finite() && turn.end_time > turn.start_time
    });
    turns.sort_unstable_by(|a, b| a.start_time.total_cmp(&b.start_time));
    let config = SpeakerEmbeddingExtractorConfig {
        model: Some(model_file.to_str()?.into()),
        num_threads: 2,
        ..Default::default()
    };
    let mut extractor = None;
    let mut voices = BTreeMap::new();
    for (speaker, label) in lanes.into_iter().take(MAX_CANDIDATE_LANES) {
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        let windows = sample_windows(&turns, speaker, samples.len());
        if windows.len() < 2 {
            continue;
        }
        if extractor.is_none() {
            extractor = Some(SpeakerEmbeddingExtractor::create(&config)?);
        }
        let extractor = extractor.as_ref()?;
        let mut embeddings = Vec::new();
        for window in windows {
            if cancelled.load(Ordering::Acquire) {
                return None;
            }
            let stream = extractor.create_stream()?;
            stream.accept_waveform(SAMPLE_RATE, &samples[window]);
            stream.input_finished();
            if extractor.is_ready(&stream) {
                if let Some(embedding) = extractor.compute(&stream) {
                    embeddings.push(embedding);
                }
            }
        }
        if let Some(voice) = centroid(&embeddings) {
            voices.insert(speaker_label(label), voice);
        }
    }
    (!voices.is_empty()).then_some(DetectionSamples {
        generation,
        model,
        voices,
    })
}
