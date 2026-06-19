//! Graph calendar: upcoming events and the current/next meeting, including the
//! event's invited attendees ("attendance"). Read-only; uses the existing
//! `Calendars.Read` scope.
//!
//! Times are requested in UTC (`Prefer: outlook.timezone="UTC"`) so current/next
//! selection can compare against `Utc::now()` without timezone ambiguity. The
//! field selection is intentionally minimal (title/time/join URL/organizer/
//! attendees) — never the event body — per the privacy default.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::exports::client::{GraphClient, GraphOutcome, Sleeper};
use crate::exports::transport::{GraphRequest, GraphTransport};

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
const EVENT_SELECT: &str =
    "id,subject,isOnlineMeeting,onlineMeeting,start,end,organizer,attendees";

/// A person invited to a calendar event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAttendee {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// A calendar event flattened to the fields ClawScribe uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub subject: Option<String>,
    /// ISO-8601 UTC (e.g. `2026-06-19T21:00:00Z`).
    pub start: Option<String>,
    pub end: Option<String>,
    pub is_online_meeting: bool,
    pub join_url: Option<String>,
    pub organizer: Option<CalendarAttendee>,
    pub attendees: Vec<CalendarAttendee>,
}

// ── Graph-shaped deserialization (mapped to the flat types above) ──────────

#[derive(Deserialize)]
struct GraphEmailAddress {
    name: Option<String>,
    address: Option<String>,
}

#[derive(Deserialize)]
struct GraphRecipient {
    #[serde(rename = "emailAddress")]
    email_address: Option<GraphEmailAddress>,
}

#[derive(Deserialize)]
struct GraphDateTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
}

#[derive(Deserialize)]
struct GraphOnlineMeeting {
    #[serde(rename = "joinUrl")]
    join_url: Option<String>,
}

#[derive(Deserialize)]
struct GraphEvent {
    id: String,
    subject: Option<String>,
    #[serde(rename = "isOnlineMeeting")]
    is_online_meeting: Option<bool>,
    #[serde(rename = "onlineMeeting")]
    online_meeting: Option<GraphOnlineMeeting>,
    start: Option<GraphDateTime>,
    end: Option<GraphDateTime>,
    organizer: Option<GraphRecipient>,
    #[serde(default)]
    attendees: Vec<GraphRecipient>,
}

#[derive(Deserialize)]
struct GraphListResponse {
    value: Vec<GraphEvent>,
}

fn to_attendee(r: GraphRecipient) -> Option<CalendarAttendee> {
    r.email_address.map(|e| CalendarAttendee {
        name: e.name,
        email: e.address,
    })
}

/// Normalize a Graph UTC dateTime (which may lack a trailing `Z`) to ISO-8601.
fn normalize_utc(raw: Option<String>) -> Option<String> {
    let s = raw?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else if s.ends_with('Z') || s.contains('+') {
        Some(s.to_string())
    } else {
        Some(format!("{s}Z"))
    }
}

fn map_event(e: GraphEvent) -> CalendarEvent {
    CalendarEvent {
        id: e.id,
        subject: e.subject,
        start: normalize_utc(e.start.and_then(|d| d.date_time)),
        end: normalize_utc(e.end.and_then(|d| d.date_time)),
        is_online_meeting: e.is_online_meeting.unwrap_or(false),
        join_url: e.online_meeting.and_then(|m| m.join_url),
        organizer: e.organizer.and_then(to_attendee),
        attendees: e.attendees.into_iter().filter_map(to_attendee).collect(),
    }
}

fn calendar_request(url: String) -> GraphRequest {
    GraphRequest {
        method: "GET".into(),
        url,
        content_type: "application/json".into(),
        body: String::new(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        // Return event times in UTC so current/next comparisons are unambiguous.
        headers: vec![("Prefer".into(), "outlook.timezone=\"UTC\"".into())],
    }
}

fn parse_events(outcome: GraphOutcome) -> Result<Vec<CalendarEvent>, String> {
    match outcome {
        GraphOutcome::Success(resp) => {
            let list: GraphListResponse = serde_json::from_str(&resp.body)
                .map_err(|e| format!("Failed to parse calendar response: {e}"))?;
            Ok(list.value.into_iter().map(map_event).collect())
        }
        GraphOutcome::Failed(kind, detail) => Err(match detail {
            Some(d) => format!("Graph error ({}): {d}", kind.code()),
            None => format!("Graph error: {}", kind.code()),
        }),
        GraphOutcome::Unknown(msg) => Err(format!("Network error: {msg}")),
    }
}

/// Events within `[start_iso, end_iso)` (ISO-8601 UTC), recurrences expanded,
/// ordered by start time. ISO-`Z` strings are query-safe, so no extra encoding.
pub async fn list_calendar_events<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    start_iso: &str,
    end_iso: &str,
) -> Result<Vec<CalendarEvent>, String> {
    let request = calendar_request(format!(
        "{GRAPH_BASE}/me/calendarView?startDateTime={start_iso}&endDateTime={end_iso}\
         &$select={EVENT_SELECT}&$orderby=start/dateTime&$top=50"
    ));
    parse_events(client.execute(&request, token).await)
}

/// The meeting happening now, else the next one starting within ~12h. Returns
/// `None` when nothing is scheduled in the window.
pub async fn current_or_next_meeting<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<Option<CalendarEvent>, String> {
    let now = Utc::now();
    let start_iso = (now - chrono::Duration::minutes(30))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let end_iso = (now + chrono::Duration::hours(12))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let events = list_calendar_events(client, token, &start_iso, &end_iso).await?;

    let parse = |s: &Option<String>| {
        s.as_deref()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|d| d.with_timezone(&Utc))
    };

    // Events are start-ordered. The first one spanning `now` is current; the
    // first one starting after `now` is next.
    let mut next: Option<CalendarEvent> = None;
    for ev in events {
        match (parse(&ev.start), parse(&ev.end)) {
            (Some(s), Some(e)) if s <= now && now <= e => return Ok(Some(ev)),
            (Some(s), _) if s > now && next.is_none() => next = Some(ev),
            _ => {}
        }
    }
    Ok(next)
}
