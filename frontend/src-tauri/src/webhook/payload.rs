use std::io::{self, Write};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use crate::exports::actions::ReviewedAction;

pub(super) const MAX_BODY_BYTES: usize = 3 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    SummaryReady,
    RecordingSaved,
    Test,
}

impl Event {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::SummaryReady => "summary.ready",
            Self::RecordingSaved => "recording.saved",
            Self::Test => "test",
        }
    }
}

#[derive(Default, Serialize)]
pub(super) struct Meeting {
    pub id: String,
    pub title: String,
    pub started_at: String,
    pub duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub project_tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(super) struct Passage {
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Clone, Serialize)]
struct Action {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due: Option<String>,
}

#[derive(Serialize)]
struct Reviewed {
    #[serde(flatten)]
    action: Action,
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

#[derive(Default)]
pub(super) enum Transcript {
    #[default]
    Absent,
    Passages(Vec<Passage>),
    TooLarge,
}

pub(super) struct Content {
    pub meeting: Meeting,
    pub summary: Option<String>,
    pub reviewed_actions: Option<Vec<ReviewedAction>>,
    pub transcript: Transcript,
}

#[derive(Serialize)]
struct Payload<'a> {
    schema: &'static str,
    event: &'static str,
    delivery_id: &'a str,
    sent_at: String,
    app_version: &'static str,
    meeting: Meeting,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary_markdown: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    action_items: Vec<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewed_actions: Option<Vec<Reviewed>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<Vec<Passage>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    transcript_truncated: bool,
}

pub(super) fn utc(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(super) fn sample() -> Content {
    Content {
        meeting: Meeting {
            id: "sample-meeting".into(),
            title: "Example meeting".into(),
            started_at: "2026-01-01T12:00:00Z".into(),
            duration_seconds: Some(60.0),
            ..Meeting::default()
        },
        summary: None,
        reviewed_actions: None,
        transcript: Transcript::Absent,
    }
}

pub(super) fn build(
    event: Event,
    id: &str,
    now: DateTime<Utc>,
    content: Content,
    include_summary: bool,
    include_transcript: bool,
) -> Option<Vec<u8>> {
    let summary_markdown = content
        .summary
        .filter(|_| include_summary)
        .map(|text| {
            let text = crate::summary::sources::strip_source_links(&text);
            crate::summary::sources::expand_tags(&text, &[])
        })
        .filter(|text| !text.trim().is_empty());
    let reviewed_actions = content
        .reviewed_actions
        .filter(|_| include_summary)
        .map(|actions| {
            actions
                .into_iter()
                .map(|item| Reviewed {
                    action: Action {
                        text: export_text(&item.title),
                        owner: item.owner.as_deref().map(export_text),
                        due: item.due_date,
                    },
                    done: item.done,
                    details: item.details.as_deref().map(export_text),
                })
                .collect::<Vec<_>>()
        });
    let action_items = match &reviewed_actions {
        Some(reviewed) => reviewed
            .iter()
            .filter(|item| !item.done)
            .map(|item| item.action.clone())
            .collect(),
        None => summary_markdown
            .as_deref()
            .map(crate::exports::markdown_notes::parse_action_items)
            .unwrap_or_default()
            .into_iter()
            .map(|item| Action {
                text: item.task,
                owner: item.owner,
                due: item.due_date,
            })
            .collect(),
    };
    let (transcript, transcript_truncated) = match (include_transcript, content.transcript) {
        (true, Transcript::Passages(rows)) if !rows.is_empty() => (Some(rows), false),
        (true, Transcript::TooLarge) => (None, true),
        _ => (None, false),
    };
    let mut payload = Payload {
        schema: "clawscribe.webhook.v1",
        event: event.name(),
        delivery_id: id,
        sent_at: utc(now),
        app_version: env!("CARGO_PKG_VERSION"),
        meeting: content.meeting,
        summary_markdown,
        action_items,
        reviewed_actions,
        transcript,
        transcript_truncated,
    };
    let first = encode(&payload);
    if first.is_some() || payload.transcript.is_none() {
        return first;
    }
    payload.transcript = None;
    payload.transcript_truncated = true;
    encode(&payload)
}

fn export_text(text: &str) -> String {
    let text = crate::summary::sources::strip_source_links(text);
    crate::summary::sources::expand_tags(&text, &[])
}

struct BoundedBody(Vec<u8>);

impl Write for BoundedBody {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BODY_BYTES - self.0.len() {
            return Err(io::Error::other("Webhook body exceeds its size limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode(payload: &Payload<'_>) -> Option<Vec<u8>> {
    let mut bytes = BoundedBody(Vec::new());
    serde_json::to_writer(&mut bytes, payload).ok()?;
    Some(bytes.0)
}
