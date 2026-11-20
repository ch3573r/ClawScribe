//! Graph calendar: upcoming events and the current/next meeting, including the
//! event's invited attendees. Read-only; uses the existing
//! `Calendars.Read` scope.
//!
//! Times are requested in UTC (`Prefer: outlook.timezone="UTC"`) so current/next
//! selection can compare against `Utc::now()` without timezone ambiguity. The
//! field selection is intentionally minimal (title/time/join URL/organizer/
//! attendees/recurrence identity) — never the event body — per the privacy default.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::exports::client::{GraphClient, GraphOutcome, Sleeper};
use crate::exports::transport::{GraphRequest, GraphTransport};

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
const EVENT_SELECT: &str =
    "id,subject,isOnlineMeeting,onlineMeeting,start,end,organizer,attendees,isAllDay,isCancelled,type,seriesMasterId";
const SERIES_ID_PREFIX: &str = "graph:v1:";

/// Identity from the authenticated Microsoft session. With the shared
/// `organizations` authority, the Graph /me object ID is distinct per tenant.
#[derive(Clone, Copy)]
pub(super) struct CalendarAccount<'a> {
    pub authority: &'a str,
    pub user_id: &'a str,
}

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
    /// Opaque account-scoped identity for a recurring occurrence or exception.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    pub subject: Option<String>,
    /// ISO-8601 UTC (e.g. `2026-06-19T21:00:00Z`).
    pub start: Option<String>,
    pub end: Option<String>,
    pub is_online_meeting: bool,
    pub join_url: Option<String>,
    pub organizer: Option<CalendarAttendee>,
    pub attendees: Vec<CalendarAttendee>,
    pub all_day: bool,
    pub is_cancelled: bool,
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
    #[serde(rename = "type")]
    event_type: Option<String>,
    #[serde(rename = "seriesMasterId")]
    series_master_id: Option<String>,
    subject: Option<String>,
    #[serde(rename = "isOnlineMeeting")]
    is_online_meeting: Option<bool>,
    #[serde(rename = "onlineMeeting")]
    online_meeting: Option<GraphOnlineMeeting>,
    #[serde(rename = "isAllDay")]
    all_day: Option<bool>,
    #[serde(rename = "isCancelled")]
    is_cancelled: Option<bool>,
    start: Option<GraphDateTime>,
    end: Option<GraphDateTime>,
    organizer: Option<GraphRecipient>,
    attendees: Option<Vec<GraphRecipient>>,
}

#[derive(Deserialize)]
struct GraphListResponse {
    value: Vec<GraphEvent>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
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

fn bounded_id(id: &str, max: usize) -> bool {
    !id.is_empty()
        && id.len() <= max
        && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

pub(crate) fn valid_series_id(id: &str) -> bool {
    id.strip_prefix(SERIES_ID_PREFIX).is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn series_id(e: &GraphEvent, account: CalendarAccount<'_>) -> Option<String> {
    if !matches!(e.event_type.as_deref(), Some("occurrence" | "exception")) {
        return None;
    }
    let master = e.series_master_id.as_deref()?;
    if !bounded_id(account.authority, 128)
        || !bounded_id(account.user_id, 256)
        || !bounded_id(master, 2048)
        || !bounded_id(&e.id, 2048)
    {
        return None;
    }
    // Keep the account and master IDs out of the link and portable metadata.
    // Length prefixes preserve the boundaries of the case-sensitive IDs.
    let mut hash = Sha256::new();
    hash.update(b"clawscribe:graph-series:v1");
    for id in [account.authority, account.user_id, master] {
        hash.update((id.len() as u32).to_be_bytes());
        hash.update(id.as_bytes());
    }
    Some(format!("{SERIES_ID_PREFIX}{:x}", hash.finalize()))
}

fn map_event(e: GraphEvent, account: CalendarAccount<'_>) -> CalendarEvent {
    CalendarEvent {
        series_id: series_id(&e, account),
        id: e.id,
        subject: e.subject,
        start: normalize_utc(e.start.and_then(|d| d.date_time)),
        end: normalize_utc(e.end.and_then(|d| d.date_time)),
        is_online_meeting: e.is_online_meeting.unwrap_or(false),
        join_url: e.online_meeting.and_then(|m| m.join_url),
        organizer: e.organizer.and_then(to_attendee),
        attendees: e
            .attendees
            .unwrap_or_default()
            .into_iter()
            .filter_map(to_attendee)
            .collect(),
        all_day: e.all_day.unwrap_or(false),
        is_cancelled: e.is_cancelled.unwrap_or(false),
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

fn parse_events(
    outcome: GraphOutcome,
    account: CalendarAccount<'_>,
    require_complete: bool,
) -> Result<Vec<CalendarEvent>, String> {
    match outcome {
        GraphOutcome::Success(resp) => {
            let list: GraphListResponse = serde_json::from_str(&resp.body)
                .map_err(|e| format!("Failed to parse calendar response: {e}"))?;
            if require_complete && list.next_link.is_some() {
                return Err("The calendar returned an incomplete list of events.".into());
            }
            Ok(list
                .value
                .into_iter()
                .map(|event| map_event(event, account))
                .filter(|event| !event.is_cancelled)
                .collect())
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
pub(super) async fn list_calendar_events<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    account: CalendarAccount<'_>,
    start_iso: &str,
    end_iso: &str,
    require_complete: bool,
) -> Result<Vec<CalendarEvent>, String> {
    let request = calendar_request(format!(
        "{GRAPH_BASE}/me/calendarView?startDateTime={start_iso}&endDateTime={end_iso}\
         &$select={EVENT_SELECT}&$orderby=start/dateTime&$top=50"
    ));
    parse_events(client.execute(&request, token).await, account, require_complete)
}

/// The meeting happening now, else the next one starting within ~12h. Returns
/// `None` when nothing is scheduled in the window.
pub(super) async fn current_or_next_meeting<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    account: CalendarAccount<'_>,
) -> Result<Option<CalendarEvent>, String> {
    let now = Utc::now();
    let start_iso = (now - chrono::Duration::minutes(30))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let end_iso = (now + chrono::Duration::hours(12))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let events = list_calendar_events(client, token, account, &start_iso, &end_iso, false).await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{RetryPolicy, TokioSleeper};
    use crate::exports::transport::{GraphResponse, MockGraphTransport};

    const FIXTURE: &str = include_str!("../../../tests/fixtures/microsoft-calendar-series.json");
    const EXPECTED_SERIES: &str =
        "graph:v1:5b910e68a596c61027e658aaf8baa621706d748d0af2b1e2ea52d49a13c6f49f";

    #[tokio::test]
    async fn calendar_view_uses_one_read_to_identify_occurrences_and_exceptions() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::success(200, FIXTURE)]);
        let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
        let events = list_calendar_events(
            &client,
            "fixture",
            CalendarAccount {
                authority: "organizations",
                user_id: "fixture-account-a",
            },
            "2030-04-15T00:00:00Z",
            "2030-05-01T00:00:00Z",
            true,
        )
        .await
        .unwrap();

        let requests = client.transport().recorded();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        let url = url::Url::parse(&requests[0].url).unwrap();
        assert_eq!(url.path(), "/v1.0/me/calendarView");
        let select = url
            .query_pairs()
            .find(|(key, _)| key == "$select")
            .unwrap()
            .1;
        for field in ["seriesMasterId", "type"] {
            assert!(select.split(',').any(|selected| selected == field));
        }
        assert!(!select.split(',').any(|field| field.starts_with("body")));
        assert_eq!(events.len(), 6);
        for (event, id) in events.iter().zip([
            "fixture-occurrence-one",
            "fixture-occurrence-two",
            "fixture-exception",
        ]) {
            assert_eq!(event.id, id);
            assert_eq!(event.series_id.as_deref(), Some(EXPECTED_SERIES));
        }
        assert!(events[3..].iter().all(|event| event.series_id.is_none()));
        assert_eq!(events[0].start.as_deref(), Some("2030-04-15T10:00:00Z"));
        assert_eq!(
            events[0].attendees[0].name.as_deref(),
            Some("Invited person")
        );
        let portable = serde_json::to_string(&events[0]).unwrap();
        for raw in ["FixtureMaster/Ab=", "fixture-account-a", "organizations"] {
            assert!(!portable.contains(raw));
        }
    }

    #[test]
    fn account_scope_and_opaque_master_are_required() {
        let make_event = |kind: &str, master: &str| {
            serde_json::from_value(serde_json::json!({
                "id": "occurrence-id", "type": kind, "seriesMasterId": master
            }))
            .unwrap()
        };
        let account = CalendarAccount {
            authority: "organizations",
            user_id: "fixture-account-a",
        };
        let master = "FixtureMaster/Ab=";
        for different_scope in [
            CalendarAccount {
                user_id: "fixture-account-b",
                ..account
            },
            CalendarAccount {
                authority: "another-tenant",
                ..account
            },
        ] {
            let mapped = map_event(make_event("occurrence", master), different_scope);
            assert!(mapped
                .series_id
                .as_deref()
                .is_some_and(|id| id != EXPECTED_SERIES));
        }
        for incomplete_scope in [
            CalendarAccount {
                user_id: "",
                ..account
            },
            CalendarAccount {
                authority: "",
                ..account
            },
        ] {
            assert!(map_event(make_event("occurrence", master), incomplete_scope)
                .series_id
                .is_none());
        }
        let overlong_master = "x".repeat(2049);
        for (kind, invalid_master) in [
            ("occurrence", "invalid id"),
            ("occurrence", "invalid\n"),
            ("occurrence", overlong_master.as_str()),
            ("singleInstance", master),
            ("seriesMaster", master),
        ] {
            assert!(map_event(make_event(kind, invalid_master), account)
                .series_id
                .is_none());
        }
        let different_case = map_event(make_event("occurrence", "fixtureMaster/Ab="), account);
        assert!(different_case
            .series_id
            .as_deref()
            .is_some_and(|id| id != EXPECTED_SERIES));
    }
}
