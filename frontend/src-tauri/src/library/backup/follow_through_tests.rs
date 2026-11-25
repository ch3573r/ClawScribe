use super::{tests::empty_conversation_destination, *};
use serde_json::json;

fn series(scope: &str) -> String {
    format!("graph:v1:{}", scope.repeat(64))
}

async fn meeting(pool: &SqlitePool, id: &str, scope: &str, reviewed: bool) {
    let link = json!({"eventId": id, "seriesId": series(scope), "reviewed": reviewed});
    sqlx::query(
        "INSERT INTO meetings (id, title, created_at, updated_at, calendar_link)
         VALUES (?, 'Example meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)",
    )
    .bind(id)
    .bind(link.to_string())
    .execute(pool)
    .await
    .unwrap();
}

async fn write_and_unpack(
    source: &SqlitePool,
    existing: HashSet<String>,
    stage_root: &Path,
) -> (BackupReport, Manifest, tempfile::TempDir) {
    let records = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let path = archive.path().join("library.zip");
    let (manifest, folders) = snapshot_current(source, records.path()).await.unwrap();
    let report = write_archive_impl(
        &path,
        manifest,
        folders,
        Some(archive.path()),
        Some(records.path()),
        false,
    )
    .unwrap();
    let (manifest, stage) = unpack(&path, stage_root, &existing).unwrap();
    (report, manifest, stage)
}

#[tokio::test]
async fn v3_restores_current_approved_notes_and_preserves_destination_series_preferences() {
    let source = empty_conversation_destination().await;
    let target = empty_conversation_destination().await;
    for (id, scope, reviewed) in [
        ("new", "a", true),
        ("stale", "a", true),
        ("unreviewed", "a", false),
        ("new-series", "b", true),
        ("existing", "c", true),
    ] {
        meeting(&source, id, scope, reviewed).await;
        sqlx::query("INSERT INTO approved_series_notes VALUES (?, 'microsoft', ?, ?, ?)")
            .bind(id)
            .bind(series(scope))
            .bind(if id == "stale" { "old-event" } else { id })
            .bind(format!("Approved note for {id}"))
            .execute(&source)
            .await
            .unwrap();
    }
    for scope in ["a", "b", "c", "d"] {
        sqlx::query(
            "INSERT INTO recurring_meeting_preferences VALUES ('microsoft', ?, 'de', 0, 'todo')",
        )
        .bind(series(scope))
        .execute(&source)
        .await
        .unwrap();
    }
    meeting(&target, "existing", "c", true).await;
    sqlx::query("INSERT INTO approved_series_notes VALUES ('existing', 'microsoft', ?, 'existing', 'Destination note')")
        .bind(series("c")).execute(&target).await.unwrap();
    sqlx::query(
        "INSERT INTO recurring_meeting_preferences VALUES ('microsoft', ?, NULL, 1, 'email')",
    )
    .bind(series("a"))
    .execute(&target)
    .await
    .unwrap();

    let stage_root = tempfile::tempdir().unwrap();
    let (_, manifest, stage) = write_and_unpack(
        &source,
        HashSet::from(["existing".into()]),
        stage_root.path(),
    )
    .await;
    assert_eq!(manifest.version, 3);
    import_manifest(&target, manifest, stage).await.unwrap();
    let notes: Vec<(String, String)> = sqlx::query_as(
        "SELECT meeting_id, approved_notes FROM approved_series_notes ORDER BY meeting_id",
    )
    .fetch_all(&target)
    .await
    .unwrap();
    assert_eq!(
        notes,
        [
            ("existing".into(), "Destination note".into()),
            ("new".into(), "Approved note for new".into()),
            ("new-series".into(), "Approved note for new-series".into()),
        ]
    );
    let preferences: Vec<(String, Option<String>, bool, String)> = sqlx::query_as(
        "SELECT series_id, summary_language, include_approved_notes, destination
         FROM recurring_meeting_preferences ORDER BY series_id",
    )
    .fetch_all(&target)
    .await
    .unwrap();
    assert_eq!(
        preferences,
        [
            (series("a"), None, true, "email".into()),
            (series("b"), Some("de".into()), false, "todo".into()),
        ]
    );
}

fn answer_id(n: u128) -> String {
    uuid::Uuid::from_u128(n).to_string()
}

fn live_answer(id: &str) -> Value {
    let session = "4e5452a6-94f3-4ed0-ab33-c5837e87e5d3";
    json!({
        "id": id,
        "content": "The example decision stands [K1].",
        "provider": "openai",
        "model": "example-model",
        "context": {"session_id": session, "finalized_through_seconds": 31.0, "transcription_incomplete": true},
        "cited_tags": [1],
        "question_tags": [],
        "citations": [{
            "tag": 1,
            "locator": {
                "historical": false,
                "source_id": format!("live:{session}"), "source_revision": 1,
                "chunk_id": format!("live:{session}:19"), "fingerprint": "a".repeat(64),
                "locator": {"kind": "live", "session_id": session, "sequence_ids": [19]},
            },
            "live_excerpt": {"sequence_id": 19, "text": "The example decision stands.", "start_seconds": 28.0, "end_seconds": 31.0},
            "attachment_meeting": null,
        }],
    })
}

async fn save_answer(pool: &SqlitePool, row: (u128, Option<&str>, &str, bool), answer: Value) {
    let (id, meeting, recording, complete) = row;
    sqlx::query(
        "INSERT INTO saved_live_answers (id, meeting_id, recording_id, source_dependencies_complete, answer)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(answer_id(id))
    .bind(meeting)
    .bind(recording)
    .bind(complete)
    .bind(answer.to_string())
    .execute(pool)
    .await
    .unwrap();
}

#[test]
fn portable_live_recap_keeps_citations_beyond_the_search_result_limit() {
    let id = answer_id(1);
    let mut answer = live_answer(&id);
    answer["content"] = json!("The example decision stands [K15].");
    answer["cited_tags"] = json!([15]);
    answer["citations"][0]["tag"] = json!(15);
    assert!(crate::knowledge::live_notes::is_portable_live_answer(
        &id, &answer
    ));
}

#[tokio::test]
async fn v3_ports_only_proven_live_only_answers_and_reports_omissions() {
    let source = empty_conversation_destination().await;
    let target = empty_conversation_destination().await;
    for id in ["recording", "earlier", "existing"] {
        meeting(&source, id, "a", true).await;
    }
    meeting(&target, "existing", "a", true).await;
    for row in [
        (1, Some("recording"), "recording", true),
        (2, Some("recording"), "recording", false), // Old or incomplete dependency proof.
        (3, Some("recording"), "recording", true),  // Uncited external dependency below.
        (4, Some("recording"), "wrong-recording", true),
        (5, None, "recording", true), // Pending even though the recording ID matches.
        (6, Some("existing"), "existing", true),
        (7, Some("existing"), "existing", false),
        (8, Some("recording"), "recording", true), // Destination answer ID collision.
        (9, Some("recording"), "recording", true), // Unexpected provider metadata.
        (10, Some("existing"), "existing", false),
    ] {
        let mut answer = live_answer(&answer_id(row.0));
        if row.0 == 9 {
            answer["context"]["endpoint"] = json!("profile-local endpoint");
        }
        save_answer(&source, row, answer).await;
    }
    sqlx::query(
        "INSERT INTO saved_live_answer_dependencies (answer_id, ordinal, source_id, reference_json)
         VALUES (?, 0, 'meeting:earlier', '{}')",
    )
    .bind(answer_id(3))
    .execute(&source)
    .await
    .unwrap();
    for row in [
        (6, Some("existing"), "existing", true),
        (8, None, "pending", true),
    ] {
        let mut answer = live_answer(&answer_id(row.0));
        answer["content"] = json!("Destination answer [K1].");
        save_answer(&target, row, answer).await;
    }

    let root = tempfile::tempdir().unwrap();
    let (report, manifest, stage) =
        write_and_unpack(&source, HashSet::from(["existing".into()]), root.path()).await;
    assert_eq!(report.omitted_saved_live_answers, 6);
    let wire = serde_json::to_value(&manifest).unwrap();
    let answers = wire["follow_through"]["answers"].as_array().unwrap();
    assert_eq!(answers.len(), 3);
    assert_eq!(answers[0]["id"], answer_id(6));
    assert_eq!(answers[1]["id"], answer_id(1));
    assert_eq!(answers[2]["id"], answer_id(8));
    assert!(!wire.to_string().contains("profile-local endpoint"));
    let restored = import_manifest(&target, manifest, stage).await.unwrap();
    assert_eq!(restored.omitted_saved_live_answers, 5);
    let rows: Vec<(String, Option<String>, String, bool, String)> = sqlx::query_as(
        "SELECT id, meeting_id, recording_id, source_dependencies_complete, answer
         FROM saved_live_answers ORDER BY id",
    )
    .fetch_all(&target)
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        (
            answer_id(1),
            Some("recording".into()),
            "recording".into(),
            true,
            live_answer(&answer_id(1)).to_string()
        )
    );
    assert_eq!(rows[1].1.as_deref(), Some("existing"));
    assert!(rows[1].4.contains("Destination answer"));
    assert_eq!(rows[2].1, None);
    assert_eq!(rows[2].2, "pending");
    assert!(rows[2].4.contains("Destination answer"));
}

#[tokio::test]
async fn restore_rejects_inconsistent_follow_through_before_importing_any_meeting() {
    let source = empty_conversation_destination().await;
    let target = empty_conversation_destination().await;
    meeting(&source, "recording", "a", true).await;
    let records = tempfile::tempdir().unwrap();
    let (manifest, _) = snapshot_current(&source, records.path()).await.unwrap();
    let baseline = serde_json::to_value(&manifest).unwrap();
    let id = answer_id(1);
    let valid_answer = json!({
        "id": id, "meeting_id": "recording", "recording_id": "recording",
        "saved_at": "2026-11-22 08:02:00", "answer": live_answer(&id),
    });
    let mut wrong_recording = valid_answer.clone();
    wrong_recording["recording_id"] = json!("other-recording");
    let mut wrong_session = valid_answer.clone();
    wrong_session["answer"]["citations"][0]["locator"]["locator"]["session_id"] =
        json!(answer_id(2));
    for follow_through in [
        json!({"answers": [wrong_recording]}),
        json!({"answers": [wrong_session]}),
        json!({"answers": [valid_answer.clone(), valid_answer]}),
        json!({"notes": [{"meeting_id": "recording", "source_id": "microsoft", "series_id": series("a"), "event_id": "old-event", "approved_notes": "Changed link"}]}),
        json!({"series": [{"source_id": "microsoft", "series_id": series("b"), "include_approved_notes": true, "destination": "planner"}]}),
        json!({"omitted_answers": {"unknown-meeting": 1}}),
    ] {
        let mut wire = baseline.clone();
        wire["follow_through"] = follow_through;
        let manifest = serde_json::from_value(wire).unwrap();
        let error = import_manifest(&target, manifest, tempfile::tempdir().unwrap())
            .await
            .err()
            .unwrap();
        assert!(error.contains("Invalid meeting follow-through data"));
    }
    for (old, _) in [
        snapshot(&source).await.unwrap(),
        snapshot_with_documents(&source).await.unwrap(),
    ] {
        let mut wire = serde_json::to_value(old).unwrap();
        assert!(wire.get("follow_through").is_none());
        let legacy = serde_json::from_value(wire.clone()).unwrap();
        let legacy_target = empty_conversation_destination().await;
        let report = import_manifest(&legacy_target, legacy, tempfile::tempdir().unwrap())
            .await
            .unwrap();
        assert_eq!((report.meetings, report.omitted_saved_live_answers), (1, 0));
        wire["follow_through"] = json!({"omitted_answers": {"recording": 1}});
        let manifest = serde_json::from_value(wire).unwrap();
        assert!(
            import_manifest(&target, manifest, tempfile::tempdir().unwrap())
                .await
                .is_err()
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM meetings")
        .fetch_one(&target)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
