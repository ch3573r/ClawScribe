use super::*;
use sqlx::migrate::Migrator;
use std::borrow::Cow;

#[test]
fn rejects_weak_ambiguous_and_duplicate_voice_matches() {
    let known = |name: &str, embedding: Vec<f32>| KnownVoice {
        name_key: name.to_lowercase(),
        name: name.into(),
        embedding: unit_vector(&embedding).unwrap(),
    };
    let ada = known("Ada", vec![1.0, 0.0]);
    let grace = known("Grace", vec![0.0, 1.0]);
    let similar_to_ada = known("Other", vec![0.97, 0.24]);
    let query = unit_vector(&[0.99, 0.1]).unwrap();
    assert_eq!(
        best_match(&query, &[ada.clone(), grace.clone()])
            .unwrap()
            .name,
        "Ada"
    );
    assert!(best_match(&query, &[ada.clone(), similar_to_ada]).is_none());
    assert!(best_match(&unit_vector(&[0.7, 0.7]).unwrap(), &[ada.clone(), grace]).is_none());
    assert!(best_match(&[1.0], &[ada.clone()]).is_none());
    let voices = BTreeMap::from([
        ("Speaker 1".into(), query.clone()),
        ("Speaker 2".into(), query),
    ]);
    assert!(unambiguous_matches(&voices, &[ada], &BTreeSet::new()).is_empty());
}

#[test]
fn requires_two_consistent_finite_windows_for_a_centroid() {
    assert!(centroid(&[vec![1.0, 0.0]]).is_none());
    assert!(centroid(&[vec![1.0, 0.0], vec![0.0, 1.0]]).is_none());
    assert!(unit_vector(&[f32::NAN, 1.0]).is_none());
    assert!(unit_vector(&[0.0, 0.0]).is_none());
    let value = centroid(&[vec![1.0, 0.0], vec![1.0, 0.1]]).unwrap();
    assert_eq!(
        decode_embedding(&encode_embedding(&value)).unwrap().len(),
        2
    );
}

#[test]
fn sample_selection_excludes_overlap_and_is_bounded() {
    let turns = [
        DiarizationTurn {
            speaker: 5,
            start_time: 0.0,
            end_time: 100.0,
        },
        DiarizationTurn {
            speaker: 2,
            start_time: 1.0,
            end_time: 10.0,
        },
        DiarizationTurn {
            speaker: 5,
            start_time: 101.0,
            end_time: 150.0,
        },
    ];
    let windows = sample_windows(&turns, 5, 160 * SAMPLE_RATE as usize);
    assert_eq!(windows.len(), MAX_WINDOWS);
    assert!(windows
        .iter()
        .all(|range| range.start >= 101 * SAMPLE_RATE as usize));
    assert!(sample_windows(&turns, 2, 160 * SAMPLE_RATE as usize).is_empty());
}

#[tokio::test]
async fn migration_preserves_existing_meetings_and_starts_disabled() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let all = sqlx::migrate!("./migrations");
    let previous = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|m| m.version < 20261127000100)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    previous.run(&pool).await.unwrap();
    sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('existing', 'Meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
        .execute(&pool).await.unwrap();
    all.run(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        read_preferences(&mut conn).await.unwrap(),
        Preferences::default()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT title FROM meetings WHERE id = 'existing'")
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        "Meeting"
    );
}

#[tokio::test]
async fn remembering_requires_current_sample_and_renames_in_the_same_transaction() {
    use crate::database::repositories::transcript::TranscriptsRepository;

    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('enroll-meeting', 'Meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, speaker) VALUES ('sample-row', 'enroll-meeting', 'Test', CURRENT_TIMESTAMP, 'Speaker 1')")
        .execute(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let preferences = update_preferences(&mut conn, true, false).await.unwrap();
    let fingerprint = saved_fingerprint(&mut conn, "enroll-meeting", "Speaker 1")
        .await
        .unwrap()
        .unwrap();
    offer_detection_sample(Some(DetectionCache {
        meeting: "enroll-meeting".into(),
        generation: preferences.generation,
        model: "model".into(),
        lanes: BTreeMap::from([("Speaker 1".into(), (fingerprint, vec![1.0, 0.0]))]),
    }));
    sqlx::query("UPDATE transcripts SET audio_start_time = 1 WHERE id = 'sample-row'")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let rename = || {
        TranscriptsRepository::update_transcript_speakers_matching(
            &pool,
            "enroll-meeting",
            Some("Speaker 1"),
            Some("Ada"),
            true,
        )
    };
    assert!(rename()
        .await
        .unwrap_err()
        .to_string()
        .contains(STALE_SAMPLE));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM speaker_profiles")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT speaker FROM transcripts WHERE id = 'sample-row'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "Speaker 1"
    );

    sqlx::query("UPDATE transcripts SET audio_start_time = NULL WHERE id = 'sample-row'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER refuse_rename BEFORE UPDATE OF speaker ON transcripts BEGIN SELECT RAISE(ABORT, 'synthetic refusal'); END")
        .execute(&pool).await.unwrap();
    assert!(rename().await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM speaker_profiles")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER refuse_rename")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rename().await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT name FROM speaker_profiles")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "Ada"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT speaker FROM transcripts WHERE id = 'sample-row'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "Ada"
    );
    offer_detection_sample(None);
}

#[tokio::test]
async fn suggestions_use_current_lane_and_disappear_with_profile_deletion() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('meeting', 'Meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, speaker) VALUES ('row', 'meeting', 'Test', CURRENT_TIMESTAMP, 'Speaker 1')")
        .execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    update_preferences(&mut tx, true, false).await.unwrap();
    save_voice(&mut tx, "Ada", "model", &[1.0, 0.0])
        .await
        .unwrap();
    save_voice(&mut tx, "Other", "different-model", &[1.0, 0.0])
        .await
        .unwrap();
    let mut segments = vec![TranscriptSegment {
        id: "row".into(),
        text: "Test".into(),
        timestamp: None,
        speaker: Some("Speaker 1".into()),
        audio_start_time: None,
        audio_end_time: None,
        duration: None,
        word_timestamps: None,
    }];
    let capture = DetectionSamples {
        generation: 1,
        model: "model".into(),
        voices: BTreeMap::from([("Speaker 1".into(), vec![1.0, 0.0])]),
    };
    finish_detection(&mut tx, "meeting", &mut segments, Some(capture))
        .await
        .unwrap();
    assert_eq!(segments[0].speaker.as_deref(), Some("Speaker 1"));
    tx.commit().await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        current_suggestions(&mut conn, "meeting")
            .await
            .unwrap()
            .get("Speaker 1")
            .map(String::as_str),
        Some("Ada")
    );
    sqlx::query("UPDATE transcripts SET audio_start_time = 1 WHERE id = 'row'")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(current_suggestions(&mut conn, "meeting")
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE transcripts SET audio_start_time = NULL WHERE id = 'row'")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DELETE FROM speaker_profiles WHERE name_key = 'ada'")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM speaker_profile_suggestions")
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn auto_apply_requires_current_consent_and_does_not_replace_other_names() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    update_preferences(&mut tx, true, true).await.unwrap();
    save_voice(&mut tx, "Ada", "model", &[1.0, 0.0])
        .await
        .unwrap();
    let mut segments = vec![TranscriptSegment {
        id: "row".into(),
        text: "Test".into(),
        timestamp: None,
        speaker: Some("Speaker 1".into()),
        audio_start_time: None,
        audio_end_time: None,
        duration: None,
        word_timestamps: None,
    }];
    let capture = DetectionSamples {
        generation: 1,
        model: "model".into(),
        voices: BTreeMap::from([("Speaker 1".into(), vec![1.0, 0.0])]),
    };
    let mut existing = segments[0].clone();
    existing.speaker = Some("Ada".into());
    segments.push(existing);
    let cache = finish_detection(&mut tx, "meeting", &mut segments, Some(capture.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(segments[0].speaker.as_deref(), Some("Speaker 1"));
    assert!(cache.lanes.contains_key("Speaker 1"));
    segments.pop();
    finish_detection(&mut tx, "meeting", &mut segments, Some(capture.clone()))
        .await
        .unwrap();
    assert_eq!(segments[0].speaker.as_deref(), Some("Ada"));
    update_preferences(&mut tx, false, true).await.unwrap();
    update_preferences(&mut tx, true, true).await.unwrap();
    segments[0].speaker = Some("Speaker 1".into());
    assert!(
        finish_detection(&mut tx, "meeting", &mut segments, Some(capture))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(segments[0].speaker.as_deref(), Some("Speaker 1"));
}
