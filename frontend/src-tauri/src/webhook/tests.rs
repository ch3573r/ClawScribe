use std::{collections::VecDeque, sync::Mutex, time::Duration};

use chrono::Utc;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::watch,
};

use super::{delivery::*, history, meetings, payload::*, settings::Switches};

#[test]
fn destination_policy_requires_consent_for_plain_http() {
    assert!(destination("https://hooks.example.com/example", false).is_ok());
    assert!(destination("http://hooks.example.com/example", true).is_err());
    assert!(destination("http://hooks.local/example", false).is_err());
    assert!(destination("http://hooks.local/example", true).is_ok());
    assert!(destination("http://localhost/example", false).is_err());
    assert!(destination("http://localhost/example", true).is_ok());
    assert!(destination("https://user@hooks.example.com/example", false).is_err());
}

#[test]
fn signature_covers_timestamp_and_exact_json_bytes() {
    assert_eq!(
        signature(b"example-key", "1700000000", br#"{"sample":true}"#),
        "sha256=1f32619d2f28ee0bded2e636266d3d1f807c45e8d54f60592c4ea575cde3f684"
    );
}

fn example_content() -> Content {
    let mut content = sample();
    content.summary = Some(
        "## Overview\nNotes [S1] [00:12](#clawscribe-source-aabbcc)\n\n## Action items\n- Publish the agenda"
            .into(),
    );
    content.transcript = Transcript::Passages(vec![Passage {
        text: "Edited words".into(),
        speaker: Some("Alex".into()),
        start_seconds: Some(1.5),
        end_seconds: Some(3.0),
    }]);
    content
}

fn body(content: Content, summary: bool, transcript: bool) -> Value {
    serde_json::from_slice(
        &build(
            Event::SummaryReady,
            "example-delivery",
            Utc::now(),
            content,
            summary,
            transcript,
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn payload_respects_each_switch_and_omits_absent_optional_data() {
    for include_summary in [false, true] {
        for include_transcript in [false, true] {
            let output = body(example_content(), include_summary, include_transcript);
            assert_eq!(output["schema"], "clawscribe.webhook.v1");
            assert_eq!(output["event"], "summary.ready");
            assert_eq!(output["delivery_id"], "example-delivery");
            assert_eq!(output["meeting"]["id"], "sample-meeting");
            assert!(output["sent_at"].as_str().unwrap().ends_with('Z'));
            assert_eq!(output.get("summary_markdown").is_some(), include_summary);
            assert_eq!(output.get("action_items").is_some(), include_summary);
            assert_eq!(output.get("transcript").is_some(), include_transcript);
            if include_summary {
                assert_eq!(
                    output["action_items"],
                    json!([{"text":"Publish the agenda"}])
                );
                assert!(!output.to_string().contains("clawscribe-source"));
                assert!(!output.to_string().contains("[S1]"));
            }
            if include_transcript {
                assert_eq!(
                    output["transcript"][0],
                    json!({
                        "start_seconds":1.5,"end_seconds":3.0,"speaker":"Alex","text":"Edited words"
                    })
                );
            }
        }
    }
    let output = body(sample(), true, true);
    for field in [
        "summary_markdown",
        "action_items",
        "transcript",
        "transcript_truncated",
    ] {
        assert!(output.get(field).is_none());
    }
}

#[test]
fn oversized_serialized_transcript_is_removed_as_a_whole() {
    let mut content = example_content();
    let Transcript::Passages(rows) = &mut content.transcript else {
        panic!("Expected passages")
    };
    rows[0].text = "\"".repeat(MAX_BODY_BYTES / 2);
    let output = body(content, true, true);
    assert!(output.get("transcript").is_none());
    assert_eq!(output["transcript_truncated"], true);
    assert_eq!(output["action_items"][0]["text"], "Publish the agenda");
}

#[tokio::test]
async fn reviewed_actions_replace_suggestions_and_follow_summary_consent() {
    let pool = crate::database::transcript_edits::tests::fixture().await;
    let reviewed = json!([
        {"id":"00000000-0000-4000-8000-000000000001","title":"Revised task","owner":"Dana",
         "dueDate":"2026-12-01","done":false,"details":"Use the reviewed draft"},
        {"id":"00000000-0000-4000-8000-000000000002","title":"Completed task","done":true}
    ]);
    let summary = "## Action items\n- Summary suggestion".to_owned();
    for (stored, expected) in [
        (Some(reviewed.to_string()), Some("Revised task")),
        (Some("[]".to_owned()), None),
        (None, Some("Summary suggestion")),
    ] {
        sqlx::query("UPDATE meetings SET reviewed_actions = ? WHERE id = 'review-test'")
            .bind(&stored)
            .execute(&pool)
            .await
            .unwrap();
        let content = meetings::load(
            &pool,
            "review-test".into(),
            Some(summary.clone()),
            Switches::default(),
        )
        .await
        .unwrap();
        let output = body(content, true, false);
        assert_eq!(output["action_items"][0]["text"].as_str(), expected);
        if expected == Some("Revised task") {
            assert_eq!(output["action_items"].as_array().unwrap().len(), 1);
            assert_eq!(
                output["reviewed_actions"][0],
                json!({
                    "text":"Revised task","owner":"Dana","due":"2026-12-01","done":false,
                    "details":"Use the reviewed draft"
                })
            );
            assert_eq!(output["reviewed_actions"][1]["done"], true);
        }
        let off = Switches {
            include_summary: false,
            ..Switches::default()
        };
        let content = meetings::load(&pool, "review-test".into(), Some(summary.clone()), off)
            .await
            .unwrap();
        let output = body(content, false, false);
        assert!(output.get("reviewed_actions").is_none());
        assert!(output.get("action_items").is_none());
    }
}

#[tokio::test]
async fn meeting_history_is_scoped_newest_first_and_omits_delivery_destinations() {
    let pool = crate::database::transcript_edits::tests::fixture().await;
    sqlx::query(
        "INSERT INTO webhook_deliveries (id,event,meeting_id,completion_key,started_at,outcome,http_status)
         VALUES ('older','recording.saved','review-test','saved','2026-11-20','failed',503),
                ('newer','summary.ready','review-test','generation','2026-11-21','succeeded',204),
                ('other','test',NULL,NULL,'2026-11-22','succeeded',200)",
    ).execute(&pool).await.unwrap();
    let history =
        serde_json::to_value(history::for_meeting(&pool, "review-test").await.unwrap()).unwrap();
    assert_eq!(history.as_array().unwrap().len(), 2);
    assert_eq!(history[0]["event"], "summary.ready");
    assert_eq!(history[0]["httpStatus"], 204);
    assert_eq!(history[1]["outcome"], "failed");
    assert!(history[0].get("completionKey").is_none());
    assert!(history[0].get("destination").is_none());
}

#[tokio::test]
async fn reads_only_current_passages_and_reviewed_meeting_metadata() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let calendar = json!({
        "eventId":"internal-event-id","reviewed":true,
        "attendees":[
            {"name":"Alex","email":"alex@example.com","attendance":"present"},
            {"name":"Unreviewed person","email":"guest@example.com"}
        ]
    });
    let summary = json!({
        "markdown":"## Overview\nPublic notes.",
        "generation":{"transcript_fingerprint":"internal-fingerprint"},
        "codex":{"notes_markdown_path":"/example/private/notes.md"}
    });
    sqlx::query("INSERT INTO meetings (id,title,created_at,updated_at,folder_path,calendar_link,recording_duration_s)
                 VALUES ('example','Example meeting',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,'/example/private/audio',?,43.5)")
        .bind(calendar.to_string()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO meeting_tags (meeting_id, tag) VALUES ('example','Project example')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO transcripts (id,meeting_id,transcript,timestamp,original_transcript,speaker,audio_start_time,audio_end_time)
                 VALUES ('internal-segment-id','example','Edited words','example','Original words','Alex',1.0,2.0)")
        .execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO summary_processes (meeting_id,status,created_at,updated_at,result)
                 VALUES ('example','completed',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,?)",
    )
    .bind(summary.to_string())
    .execute(&pool)
    .await
    .unwrap();
    let switches = Switches {
        include_transcript: true,
        ..Switches::default()
    };
    let content = meetings::load(&pool, "example".into(), None, switches)
        .await
        .unwrap();
    let output = body(content, true, true);
    assert_eq!(output["meeting"]["attendees"], json!(["Alex"]));
    assert_eq!(
        output["meeting"]["project_tags"],
        json!(["Project example"])
    );
    assert_eq!(output["meeting"]["duration_seconds"], 43.5);
    assert_eq!(output["transcript"][0]["text"], "Edited words");
    assert_eq!(output["transcript"][0]["speaker"], "Alex");
    for hidden in [
        "Original words",
        "internal-event-id",
        "internal-segment-id",
        "internal-fingerprint",
        "example.com",
        "/example/private",
        "Unreviewed person",
    ] {
        assert!(!output.to_string().contains(hidden));
    }
}

type SentRequest = (String, bytes::Bytes, String, Option<String>);

struct Fake {
    responses: Mutex<VecDeque<Result<u16, Failure>>>,
    seen: Mutex<Vec<SentRequest>>,
}

#[async_trait::async_trait]
impl Transport for Fake {
    async fn post(&self, request: &Request) -> Result<u16, Failure> {
        self.seen.lock().unwrap().push((
            request.id.clone(),
            request.body.clone(),
            request.timestamp.clone(),
            request.signature.clone(),
        ));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("Unexpected attempt")
    }
}

fn request(url: url::Url) -> Request {
    let body = bytes::Bytes::from_static(br#"{"sample":true}"#);
    let timestamp = String::from("1700000000");
    Request {
        url,
        allow_unencrypted: true,
        event: Event::Test,
        id: "example-delivery".into(),
        signature: Some(signature(b"example-key", &timestamp, &body)),
        timestamp,
        body,
    }
}

#[tokio::test]
async fn retry_policy_reuses_the_same_delivery_and_bounds_attempts() {
    let request = request(url::Url::parse("https://hooks.example.com/example").unwrap());
    let (_sender, mut changed) = watch::channel(());
    for (responses, status, failure) in [
        (vec![Ok(204)], Some(204), None),
        (vec![Ok(429), Ok(200)], Some(200), None),
        (
            vec![Err(Failure::Network), Ok(408), Ok(200)],
            Some(200),
            None,
        ),
        (vec![Ok(404)], Some(404), Some(Failure::HttpStatus)),
        (vec![Ok(302)], Some(302), Some(Failure::Redirect)),
        (
            vec![Ok(503), Ok(500), Ok(502)],
            Some(502),
            Some(Failure::HttpStatus),
        ),
    ] {
        let expected_attempts = responses.len() as u8;
        let fake = Fake {
            responses: Mutex::new(responses.into()),
            seen: Mutex::new(Vec::new()),
        };
        assert_eq!(
            deliver(&fake, &request, &mut changed).await,
            Outcome {
                attempts: expected_attempts,
                status,
                failure,
            }
        );
        let seen = fake.seen.into_inner().unwrap();
        assert_eq!(seen.len(), usize::from(expected_attempts));
        assert!(seen.iter().all(|sent| sent == &seen[0]));
    }
}

#[tokio::test]
async fn settings_change_stops_a_queued_delivery_before_transport() {
    let request = request(url::Url::parse("https://hooks.example.com/example").unwrap());
    let (sender, mut changed) = watch::channel(());
    let fake = Fake {
        responses: Mutex::new(VecDeque::new()),
        seen: Mutex::new(Vec::new()),
    };
    sender.send_replace(());
    assert_eq!(
        deliver(&fake, &request, &mut changed).await,
        Outcome::rejected(Failure::SettingsChanged)
    );
    assert!(fake.seen.into_inner().unwrap().is_empty());
}

#[tokio::test]
async fn real_transport_posts_expected_headers_and_never_follows_redirects() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let request = request(url::Url::parse(&format!("http://127.0.0.1:{port}/example")).unwrap());
    let server = async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        while !received.ends_with(br#"{"sample":true}"#) {
            let mut chunk = [0; 512];
            let read = socket.read(&mut chunk).await.unwrap();
            assert!(read > 0 && received.len() < 2048);
            received.extend_from_slice(&chunk[..read]);
        }
        let received = String::from_utf8(received).unwrap();
        let headers = received
            .lines()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        for value in [
            "post /example http/1.1",
            "content-type: application/json",
            "x-clawscribe-event: test",
            "x-clawscribe-delivery: example-delivery",
            "x-clawscribe-timestamp: 1700000000",
            concat!("user-agent: clawscribe/", env!("CARGO_PKG_VERSION")),
            "x-clawscribe-signature: sha256=1f32619d2f28ee0bded2e636266d3d1f807c45e8d54f60592c4ea575cde3f684",
        ] {
            assert!(headers.iter().any(|line| line == value), "Missing expected test header");
        }
        socket.write_all(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{port}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ).as_bytes()).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.accept())
                .await
                .is_err()
        );
    };
    let (status, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(Http.post(&request), server)
    })
    .await
    .unwrap();
    assert_eq!(status.unwrap(), 307);
}

#[tokio::test]
async fn migration_upgrades_and_history_deduplicates_automatic_completions() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    let migrations = sqlx::migrate!("./migrations");
    let prior = sqlx::migrate::Migrator {
        migrations: migrations
            .iter()
            .filter(|entry| entry.version != 20261122004500)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    prior.run(&pool).await.unwrap();
    sqlx::query("INSERT INTO meetings (id,title,created_at,updated_at) VALUES ('example','Example meeting',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")
        .execute(&pool).await.unwrap();
    migrations.run(&pool).await.unwrap();
    let claim =
        |id, event, completion| history::claim(&pool, id, event, Some("example"), completion);
    assert!(claim("first", Event::RecordingSaved, Some("saved"))
        .await
        .unwrap());
    assert!(!claim("duplicate", Event::RecordingSaved, Some("saved"))
        .await
        .unwrap());
    assert!(claim("generation", Event::SummaryReady, Some("generation"))
        .await
        .unwrap());
    assert!(!claim(
        "duplicate-generation",
        Event::SummaryReady,
        Some("generation")
    )
    .await
    .unwrap());
    assert!(claim("regenerate", Event::SummaryReady, Some("regenerate"))
        .await
        .unwrap());
    assert!(claim("manual", Event::SummaryReady, None).await.unwrap());
    assert!(claim("manual-again", Event::SummaryReady, None)
        .await
        .unwrap());
    let delivery = history::finish(
        &pool,
        "manual-again",
        Outcome {
            attempts: 1,
            status: Some(307),
            failure: Some(Failure::Redirect),
        },
    )
    .await
    .unwrap();
    let status = serde_json::to_value(delivery).unwrap();
    assert_eq!(status["errorClass"], "redirect");
    assert_eq!(status["httpStatus"], 307);
    assert_eq!(status["attempts"], 1);
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('webhook_deliveries') ORDER BY cid")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        columns,
        [
            "id",
            "event",
            "meeting_id",
            "completion_key",
            "started_at",
            "finished_at",
            "attempts",
            "outcome",
            "http_status",
            "error_class"
        ]
    );
}
