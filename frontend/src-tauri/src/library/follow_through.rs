//! Local follow-through over stable, user-reviewed meeting records.
use super::calendar::{self, CalendarLink};
use crate::{exports::actions, state::AppState};
use chrono::{Days, Local, NaiveDate};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, SqlitePool};
use std::collections::{BTreeMap, HashSet};

const READ_ERROR: &str = "Could not load meeting follow-ups.";
const WRITE_ERROR: &str = "Could not save changes for this series.";
const SERIES_REQUIRED: &str = "Review the linked recurring event first.";
const PAGE_SIZE: usize = 40;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenAction {
    id: String,
    title: String,
    owner: Option<String>,
    due_date: Option<String>,
    source_key: Option<String>,
    done: bool,
}

impl From<actions::ReviewedAction> for OpenAction {
    fn from(action: actions::ReviewedAction) -> Self {
        Self {
            id: action.id,
            title: action.title,
            owner: action.owner,
            due_date: action.due_date,
            source_key: action.source_key,
            done: action.done,
        }
    }
}

fn open_actions(raw: Option<&str>) -> Result<impl Iterator<Item = OpenAction>, String> {
    let actions = raw.map(actions::decode).transpose()?.unwrap_or_default();
    Ok(actions
        .into_iter()
        .filter(|action| !action.done)
        .map(Into::into))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActionFilters {
    search: Option<String>,
    #[serde(default)]
    status: ActionStatus,
    owner: Option<String>,
    due_from: Option<String>,
    due_to: Option<String>,
    series: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ActionStatus {
    #[default]
    Open,
    Overdue,
    DueSoon,
    Completed,
}

impl ActionStatus {
    fn matches(&self, done: bool, due: Option<NaiveDate>, today: NaiveDate) -> bool {
        match self {
            Self::Completed => done,
            Self::Open => !done,
            Self::Overdue => !done && due.is_some_and(|due| due < today),
            Self::DueSoon => {
                !done
                    && due.is_some_and(|due| {
                        due >= today
                            && today
                                .checked_add_days(Days::new(7))
                                .is_none_or(|end| due <= end)
                    })
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionRow {
    meeting_id: String,
    meeting_title: String,
    created_at: String,
    action: OpenAction,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SeriesChoice {
    id: String,
    label: String,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenActionsPage {
    rows: Vec<ActionRow>,
    series: Vec<SeriesChoice>,
    has_more: bool,
    skipped_meetings: usize,
}

#[derive(sqlx::FromRow)]
struct MeetingRow {
    id: String,
    title: String,
    created_at: String,
    calendar_link: Option<String>,
    reviewed_actions: Option<String>,
}

impl MeetingRow {
    fn calendar(&self) -> Option<CalendarLink> {
        calendar::decode(self.calendar_link.as_deref()?)
            .ok()
            .flatten()
    }
}

fn parse_date(raw: Option<&str>) -> Result<Option<NaiveDate>, String> {
    raw.map(|date| {
        if date.len() == 10 {
            if let Ok(date) = NaiveDate::parse_from_str(date, "%Y-%m-%d") {
                return Ok(date);
            }
        }
        Err("Use a valid due date.".into())
    })
    .transpose()
}

async fn list(
    pool: &SqlitePool,
    filters: ActionFilters,
    page: u32,
    today: NaiveDate,
) -> Result<OpenActionsPage, String> {
    let from = parse_date(filters.due_from.as_deref())?;
    let to = parse_date(filters.due_to.as_deref())?;
    if matches!((from, to), (Some(from), Some(to)) if from > to) {
        return Err("Choose an end date on or after the start date.".into());
    }
    let owner = filters.owner.as_deref().unwrap_or("").trim().to_lowercase();
    let search = filters.search.as_deref().unwrap_or("").trim();
    if owner.chars().count() > 160
        || search.chars().count() > 255
        || filters.series.as_ref().is_some_and(|id| id.len() > 512)
    {
        return Err("Invalid action filter.".into());
    }
    let search = search.to_lowercase();
    let mut meetings = sqlx::query_as::<_, MeetingRow>(
        "SELECT id, title, created_at, calendar_link, reviewed_actions FROM active_meetings
         WHERE reviewed_actions IS NOT NULL AND reviewed_actions != '[]'
         ORDER BY julianday(created_at) DESC, id",
    )
    .fetch(pool);
    let mut series = BTreeMap::new();
    let mut rows = Vec::new();
    let mut skipped_meetings = 0;
    while let Some(meeting) = meetings.try_next().await.map_err(|_| READ_ERROR)? {
        let Ok(actions) = actions::decode(meeting.reviewed_actions.as_deref().unwrap_or("[]"))
        else {
            skipped_meetings += 1;
            continue;
        };
        if actions.is_empty() {
            continue;
        }
        let calendar = meeting.calendar();
        let key = calendar
            .as_ref()
            .and_then(CalendarLink::reviewed_series_key);
        let series_id = key.map(|key| serde_json::json!(key).to_string());
        if let Some(id) = series_id.as_ref() {
            series.entry(id.clone()).or_insert_with(|| SeriesChoice {
                id: id.clone(),
                label: calendar
                    .as_ref()
                    .and_then(CalendarLink::subject)
                    .unwrap_or(&meeting.title)
                    .to_owned(),
                created_at: meeting.created_at.clone(),
            });
        }
        if filters.series.is_some() && filters.series != series_id {
            continue;
        }
        let meeting_matches = search.is_empty() || meeting.title.to_lowercase().contains(&search);
        for action in actions {
            let due = parse_date(action.due_date.as_deref())?;
            if !filters.status.matches(action.done, due, today)
                || (!meeting_matches
                    && !action.title.to_lowercase().contains(&search)
                    && !action
                        .owner
                        .as_deref()
                        .is_some_and(|owner| owner.to_lowercase().contains(&search)))
                || (!owner.is_empty()
                    && !action
                        .owner
                        .as_ref()
                        .is_some_and(|s| s.to_lowercase().contains(&owner)))
                || from.is_some_and(|from| due.is_none_or(|due| due < from))
                || to.is_some_and(|to| due.is_none_or(|due| due > to))
            {
                continue;
            }
            rows.push((
                due,
                ActionRow {
                    meeting_id: meeting.id.clone(),
                    meeting_title: meeting.title.clone(),
                    created_at: meeting.created_at.clone(),
                    action: action.into(),
                },
            ));
        }
    }
    // Preserve meeting recency and stored action order for equal dates.
    rows.sort_by_key(|(due, _)| (due.is_none(), *due));
    let offset = (page as usize).saturating_mul(PAGE_SIZE).min(rows.len());
    let has_more = rows.len() - offset > PAGE_SIZE;
    let rows = rows
        .into_iter()
        .skip(offset)
        .take(PAGE_SIZE)
        .map(|(_, row)| row)
        .collect();
    let mut series: Vec<_> = series.into_values().collect();
    series.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then(a.created_at.cmp(&b.created_at))
            .then(a.id.cmp(&b.id))
    });
    Ok(OpenActionsPage {
        rows,
        series,
        has_more,
        skipped_meetings,
    })
}

#[tauri::command]
pub(crate) async fn list_open_reviewed_actions(
    state: tauri::State<'_, AppState>,
    filters: ActionFilters,
    page: u32,
) -> Result<OpenActionsPage, String> {
    list(
        state.db_manager.pool(),
        filters,
        page,
        Local::now().date_naive(),
    )
    .await
}

#[derive(Clone, Copy, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
enum Destination {
    Planner,
    Todo,
    Email,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SeriesPreferences {
    language: Option<String>,
    include_approved_notes: bool,
    destination: Option<Destination>,
}

impl Default for SeriesPreferences {
    fn default() -> Self {
        Self {
            language: None,
            include_approved_notes: true,
            destination: None,
        }
    }
}

async fn preferences(
    conn: &mut SqliteConnection,
    key: (&str, &str),
) -> Result<SeriesPreferences, String> {
    sqlx::query_as(
        "SELECT summary_language AS language, include_approved_notes, destination
         FROM recurring_meeting_preferences WHERE source_id = ? AND series_id = ?",
    )
    .bind(key.0)
    .bind(key.1)
    .fetch_optional(conn)
    .await
    .map(|row| row.unwrap_or_default())
    .map_err(|_| READ_ERROR.into())
}

pub(crate) async fn summary_language(
    pool: &SqlitePool,
    link: &CalendarLink,
) -> Result<Option<String>, String> {
    let Some(key) = link.reviewed_series_key() else {
        return Ok(None);
    };
    let mut conn = pool.acquire().await.map_err(|_| READ_ERROR)?;
    Ok(preferences(&mut conn, key).await?.language)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BriefOccurrence {
    meeting_id: String,
    title: String,
    starts_at: String,
    approved_notes: Option<String>,
    actions: Vec<OpenAction>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeriesReview {
    subject: Option<String>,
    review_token: String,
    preferences: SeriesPreferences,
    template_id: Option<String>,
    approved_notes: String,
    prior_occurrences: Vec<BriefOccurrence>,
}

async fn meeting_row(conn: &mut SqliteConnection, meeting_id: &str) -> Result<MeetingRow, String> {
    sqlx::query_as(
        "SELECT id, title, created_at, calendar_link, reviewed_actions FROM active_meetings WHERE id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(conn)
    .await
    .map_err(|_| READ_ERROR)?
    .ok_or_else(|| "Meeting no longer exists.".into())
}

async fn series_review(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Option<SeriesReview>, String> {
    let mut tx = pool.begin().await.map_err(|_| READ_ERROR)?;
    let target = meeting_row(&mut tx, meeting_id).await?;
    let Some(link) = target.calendar() else {
        return Ok(None);
    };
    let Some(key) = link.reviewed_series_key() else {
        return Ok(None);
    };
    let preferences = preferences(&mut tx, key).await?;
    let template_id = calendar::series_template_for(&mut tx, key).await?;
    let approved_notes: Option<String> = sqlx::query_scalar(
        "SELECT approved_notes FROM approved_series_notes
         WHERE meeting_id = ? AND source_id = ? AND series_id = ? AND event_id = ?",
    )
    .bind(meeting_id)
    .bind(key.0)
    .bind(key.1)
    .bind(link.event_id())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| READ_ERROR)?;
    let prior_occurrences = prior_occurrences(
        &mut tx,
        &link,
        key,
        meeting_id,
        preferences.include_approved_notes,
    )
    .await?;
    tx.commit().await.map_err(|_| READ_ERROR)?;
    Ok(Some(SeriesReview {
        subject: link.subject().map(str::to_owned),
        review_token: review_token(&link, key),
        preferences,
        template_id,
        approved_notes: approved_notes.unwrap_or_default(),
        prior_occurrences,
    }))
}

async fn prior_occurrences(
    conn: &mut SqliteConnection,
    link: &CalendarLink,
    key: (&str, &str),
    excluded_id: &str,
    include_approved_notes: bool,
) -> Result<Vec<BriefOccurrence>, String> {
    let mut prior_occurrences = Vec::new();
    if let Some(before) = link.start_time() {
        // Filter the account/source and series before collecting evidence, then
        // validate each CalendarLink and compare real occurrence timestamps.
        let mut candidates = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>)>(
            "WITH candidates AS (
               SELECT id, title, reviewed_actions, CASE WHEN json_valid(calendar_link)
                   THEN calendar_link ELSE NULL END AS link
               FROM active_meetings WHERE id != ?
             )
             SELECT m.id, m.title, m.link, m.reviewed_actions, n.approved_notes
             FROM candidates m LEFT JOIN approved_series_notes n ON n.meeting_id = m.id
                 AND n.source_id = ? AND n.series_id = ? AND n.event_id = json_extract(m.link, '$.eventId')
             WHERE json_extract(m.link, '$.seriesId') = ?
                 AND CASE WHEN json_extract(m.link, '$.source') = 'ical'
                    THEN json_extract(m.link, '$.sourceId') ELSE 'microsoft' END = ?
             ORDER BY julianday(json_extract(m.link, '$.start')) DESC, m.id",
        )
        .bind(excluded_id).bind(key.0).bind(key.1).bind(key.1).bind(key.0)
        .fetch(conn);
        let mut events = HashSet::from([link.event_id().to_owned()]);
        while let Some((id, title, stored, raw_actions, note)) =
            candidates.try_next().await.map_err(|_| READ_ERROR)?
        {
            let Some(prior) = calendar::decode(&stored).ok().flatten() else {
                continue;
            };
            let Some(starts) = prior.start_time().filter(|starts| *starts < before) else {
                continue;
            };
            if prior.reviewed_series_key() != Some(key) {
                continue;
            }
            let actions: Vec<_> = open_actions(raw_actions.as_deref())
                .ok()
                .into_iter()
                .flatten()
                .collect();
            let approved_notes = include_approved_notes.then_some(note).flatten();
            if (actions.is_empty() && approved_notes.is_none())
                || !events.insert(prior.event_id().to_owned())
            {
                continue;
            }
            prior_occurrences.push(BriefOccurrence {
                meeting_id: id,
                title,
                starts_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(starts)
                    .ok_or(READ_ERROR)?
                    .to_rfc3339(),
                approved_notes,
                actions,
            });
            if prior_occurrences.len() == 8 {
                break;
            }
        }
    }
    Ok(prior_occurrences)
}

#[tauri::command]
pub(crate) async fn get_upcoming_series_brief(
    state: tauri::State<'_, AppState>,
    event: CalendarLink,
) -> Result<Vec<BriefOccurrence>, String> {
    let link = event.normalized()?;
    let Some(key) = link.series_key() else {
        return Ok(Vec::new());
    };
    let mut tx = state
        .db_manager
        .pool()
        .begin()
        .await
        .map_err(|_| READ_ERROR)?;
    let settings = preferences(&mut tx, key).await?;
    prior_occurrences(&mut tx, &link, key, "", settings.include_approved_notes).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DraftRecipient {
    email: String,
    name: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct FollowUpDraft {
    recipients: Vec<DraftRecipient>,
    subject: String,
    body: String,
}

fn is_mailbox(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    email.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && local.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'+'))
        })
        && domain.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        && domain.contains('.')
}

fn draft_recipients(link: Option<&CalendarLink>) -> Vec<DraftRecipient> {
    let mut seen = HashSet::new();
    link.into_iter()
        .flat_map(CalendarLink::confirmed_emails)
        .filter(|(email, _)| is_mailbox(email) && seen.insert(email.to_ascii_lowercase()))
        .map(|(email, name)| DraftRecipient {
            email: email.into(),
            name: name.map(str::to_owned),
        })
        .collect()
}

#[tauri::command]
pub(crate) async fn get_reviewed_follow_up_draft(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<FollowUpDraft, String> {
    let mut conn = state
        .db_manager
        .pool()
        .acquire()
        .await
        .map_err(|_| READ_ERROR)?;
    let meeting = meeting_row(&mut conn, &meeting_id).await?;
    let recipients = draft_recipients(meeting.calendar().as_ref());
    let title: String = meeting
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(190)
        .collect();
    let mut body = String::new();
    for action in open_actions(meeting.reviewed_actions.as_deref())? {
        let line = format!(
            "- {}{}{}\n",
            action.title,
            action
                .owner
                .map(|owner| format!(" — {owner}"))
                .unwrap_or_default(),
            action
                .due_date
                .map(|date| format!(" (due {date})"))
                .unwrap_or_default(),
        );
        if body.len() + line.len() > 3_000 {
            break;
        }
        body.push_str(&line);
    }
    Ok(FollowUpDraft {
        recipients,
        subject: format!("Follow-up: {title}"),
        body,
    })
}

/// A typed address limited to characters that cannot add mailto headers or
/// further recipients (no `?`, `&`, `,`, `;`, whitespace or percent escapes).
fn is_plain_email(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    let label_ok = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    address.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+'-".contains(c))
        && domain.contains('.')
        && domain.split('.').all(label_ok)
}

fn compose_uri(
    allowed: &[DraftRecipient],
    recipients: &[String],
    manual: &[String],
    subject: &str,
    body: &str,
) -> Result<String, String> {
    let mut unique = HashSet::new();
    if recipients.len() + manual.len() > 25
        || !recipients.iter().all(|email| {
            unique.insert(email.to_ascii_lowercase())
                && allowed.iter().any(|person| person.email == *email)
        })
    {
        return Err("Select up to 25 currently confirmed attendee addresses.".into());
    }
    if !manual.iter().all(|email| is_plain_email(email)) {
        return Err("Enter valid email addresses separated by commas or semicolons.".into());
    }
    let recipients: Vec<String> = recipients
        .iter()
        .cloned()
        .chain(
            manual
                .iter()
                .filter(|email| unique.insert(email.to_ascii_lowercase()))
                .cloned(),
        )
        .collect();
    if subject.trim().is_empty()
        || subject.chars().count() > 220
        || subject.chars().any(char::is_control)
        || body.trim().is_empty()
        || body.len() > 8_000
        || body.contains('\0')
    {
        return Err("Review the subject and body before opening your email client.".into());
    }
    let to = recipients
        .iter()
        .map(|address| address.replace('+', "%2B"))
        .collect::<Vec<_>>()
        .join(",");
    let body = body
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    let headers = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("subject", subject)
        .append_pair("body", &body)
        .finish()
        .replace('+', "%20");
    let uri = format!("mailto:{to}?{headers}");
    if uri.len() > 8_000 {
        return Err(
            "This draft is too long for the email client handoff. Copy the draft or shorten it."
                .into(),
        );
    }
    Ok(uri)
}

#[tauri::command]
pub(crate) async fn open_reviewed_follow_up_draft(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    recipients: Vec<String>,
    manual_recipients: Option<Vec<String>>,
    subject: String,
    body: String,
) -> Result<(), String> {
    let link = calendar::read(state.db_manager.pool(), &meeting_id).await?;
    let uri = compose_uri(
        &draft_recipients(link.as_ref()),
        &recipients,
        manual_recipients.as_deref().unwrap_or_default(),
        &subject,
        &body,
    )?;
    tauri::async_runtime::spawn_blocking(move || crate::external_url::open_mail_draft(&uri))
        .await
        .map_err(|_| "Could not open your default email client.")?
}

#[tauri::command]
pub(crate) async fn get_meeting_series_review(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Option<SeriesReview>, String> {
    series_review(state.db_manager.pool(), &meeting_id).await
}

#[tauri::command]
pub(crate) async fn get_meeting_series_preferences(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Option<SeriesPreferences>, String> {
    let mut conn = state
        .db_manager
        .pool()
        .acquire()
        .await
        .map_err(|_| READ_ERROR)?;
    let meeting = meeting_row(&mut conn, &meeting_id).await?;
    let Some(link) = meeting.calendar() else {
        return Ok(None);
    };
    let Some(key) = link.reviewed_series_key() else {
        return Ok(None);
    };
    preferences(&mut conn, key).await.map(Some)
}

#[tauri::command]
pub(crate) async fn save_meeting_series_review(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    preferences: SeriesPreferences,
    approved_notes: String,
    review_token: String,
    template_id: Option<String>,
) -> Result<(), String> {
    save_series_review(
        state.db_manager.pool(),
        &meeting_id,
        preferences,
        &approved_notes,
        &review_token,
        template_id.as_deref(),
    )
    .await
}

fn review_token(link: &CalendarLink, key: (&str, &str)) -> String {
    use sha2::{Digest, Sha256};
    let identity = serde_json::json!([key.0, key.1, link.event_id(), link.start_time()]);
    format!("{:x}", Sha256::digest(identity.to_string()))
}

async fn save_series_review(
    pool: &SqlitePool,
    meeting_id: &str,
    preferences: SeriesPreferences,
    approved_notes: &str,
    token: &str,
    template_id: Option<&str>,
) -> Result<(), String> {
    if preferences
        .language
        .as_deref()
        .is_some_and(|code| crate::summary::processor::language_name_from_code(code).is_none())
    {
        return Err("Choose a supported summary language.".into());
    }
    let approved_notes = approved_notes.trim();
    if approved_notes.chars().count() > 2_000 || approved_notes.contains('\0') {
        return Err("Keep approved notes within 2,000 characters.".into());
    }
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| WRITE_ERROR)?;
    let link = meeting_row(&mut tx, meeting_id)
        .await?
        .calendar()
        .ok_or(SERIES_REQUIRED)?;
    let key = link.reviewed_series_key().ok_or(SERIES_REQUIRED)?;
    if review_token(&link, key) != token {
        return Err("The linked event changed. Reopen the series to review it.".into());
    }
    // A review saved by an older caller omitted the template. An empty ID
    // represents an explicit choice to use the default template.
    if let Some(template_id) = template_id {
        let selected = (!template_id.is_empty()).then_some(template_id);
        calendar::save_series_template_for(&mut tx, key, selected).await?;
    }
    sqlx::query("INSERT INTO recurring_meeting_preferences VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(source_id, series_id) DO UPDATE SET
            summary_language = excluded.summary_language,
            include_approved_notes = excluded.include_approved_notes, destination = excluded.destination")
        .bind(key.0).bind(key.1).bind(preferences.language)
        .bind(preferences.include_approved_notes).bind(preferences.destination)
        .execute(&mut *tx).await.map_err(|_| WRITE_ERROR)?;
    if approved_notes.is_empty() {
        sqlx::query("DELETE FROM approved_series_notes WHERE meeting_id = ?")
            .bind(meeting_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| WRITE_ERROR)?;
    } else {
        sqlx::query("INSERT INTO approved_series_notes VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(meeting_id) DO UPDATE SET source_id = excluded.source_id,
                series_id = excluded.series_id, event_id = excluded.event_id, approved_notes = excluded.approved_notes")
            .bind(meeting_id).bind(key.0).bind(key.1).bind(link.event_id()).bind(approved_notes)
            .execute(&mut *tx).await.map_err(|_| WRITE_ERROR)?;
    }
    tx.commit().await.map_err(|_| WRITE_ERROR.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn action_search_limits_input_characters_before_case_expansion() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let filters = |length| ActionFilters {
            search: Some("İ".repeat(length)),
            ..Default::default()
        };
        assert!(list(&pool, filters(255), 1, today).await.is_ok());
        assert_eq!(
            list(&pool, filters(256), 1, today).await.err().as_deref(),
            Some("Invalid action filter.")
        );
    }

    fn action(id: u128, title: &str, done: bool) -> serde_json::Value {
        json!({
            "id": uuid::Uuid::from_u128(id).to_string(), "title": title, "owner": "Änne",
            "dueDate": "2026-12-01", "done": done, "sourceKey": "12ab"
        })
    }

    async fn occurrence(pool: &SqlitePool, id: &str, scope: &str, start: &str, reviewed: bool) {
        let event = json!({
            "source": "microsoft", "seriesId": format!("graph:v1:{}", scope.repeat(64)),
            "eventId": id, "subject":"Same subject", "start": start, "reviewed": reviewed,
        });
        let actions = json!([action(1, id, false), action(2, "Completed", true)]).to_string();
        sqlx::query(
            "INSERT INTO meetings(id,title,created_at,updated_at,calendar_link,reviewed_actions)
            VALUES (?1,'Synthetic meeting',?2,?2,?3,?4)",
        )
        .bind(id)
        .bind(start)
        .bind(event.to_string())
        .bind(actions)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_brief_includes_only_earlier_reviewed_occurrences_in_its_account_series() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        for (id, scope, starts, reviewed) in [
            ("target", "a", "2026-11-20T10:00:00+01:00", true),
            ("prior", "a", "2026-11-20T08:00:00Z", true),
            ("unreviewed", "a", "2026-11-19T10:00:00Z", false),
            ("other-account", "b", "2026-11-19T10:00:00Z", true),
            ("equal", "a", "2026-11-20T09:00:00Z", true),
            ("future", "a", "2026-11-21T09:00:00Z", true),
        ] {
            occurrence(&pool, id, scope, starts, reviewed).await;
        }
        let prior = series_review(&pool, "prior").await.unwrap().unwrap();
        save_series_review(
            &pool,
            "prior",
            SeriesPreferences {
                language: Some("de".into()),
                include_approved_notes: true,
                destination: Some(Destination::Todo),
            },
            "Reviewed decision",
            &prior.review_token,
            Some("standard_meeting"),
        )
        .await
        .unwrap();

        let result = series_review(&pool, "target").await.unwrap().unwrap();
        assert_eq!(result.preferences.language.as_deref(), Some("de"));
        assert_eq!(result.template_id.as_deref(), Some("standard_meeting"));
        assert_eq!(result.prior_occurrences.len(), 1);
        let evidence = &result.prior_occurrences[0];
        assert_eq!(evidence.meeting_id, "prior");
        assert_eq!(
            evidence.approved_notes.as_deref(),
            Some("Reviewed decision")
        );
        assert_eq!(evidence.actions.len(), 1);
        assert_eq!(evidence.actions[0].source_key.as_deref(), Some("12ab"));
        assert!(series_review(&pool, "unreviewed").await.unwrap().is_none());
        for (id, expected) in [("target", Some("de")), ("unreviewed", None)] {
            let link = calendar::read(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                summary_language(&pool, &link).await.unwrap().as_deref(),
                expected
            );
        }
        assert!(series_review(&pool, "other-account")
            .await
            .unwrap()
            .unwrap()
            .preferences
            .language
            .is_none());
        let other = calendar::read(&pool, "other-account")
            .await
            .unwrap()
            .unwrap()
            .encoded()
            .unwrap();
        sqlx::query("UPDATE meetings SET calendar_link = ? WHERE id = 'target'")
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
        assert!(save_series_review(
            &pool,
            "target",
            result.preferences,
            "Old review",
            &result.review_token,
            None,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn series_review_preserves_an_omitted_template_and_clears_an_explicit_default() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        occurrence(&pool, "saved", "a", "2026-11-20T10:00:00Z", true).await;
        for (change, expected) in [
            (Some("daily_standup"), Some("daily_standup")),
            (None, Some("daily_standup")),
            (Some(""), None),
        ] {
            let review = series_review(&pool, "saved").await.unwrap().unwrap();
            save_series_review(
                &pool,
                "saved",
                review.preferences,
                "",
                &review.review_token,
                change,
            )
            .await
            .unwrap();
            let saved = series_review(&pool, "saved").await.unwrap().unwrap();
            assert_eq!(saved.template_id.as_deref(), expected);
        }
    }

    #[tokio::test]
    async fn open_actions_match_unicode_owner_and_inclusive_due_dates_in_the_selected_series() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        occurrence(&pool, "included", "a", "2026-11-20T10:00:00Z", true).await;
        occurrence(&pool, "other-account", "b", "2026-11-20T10:00:00Z", true).await;
        occurrence(&pool, "unreviewed", "a", "2026-11-20T10:00:00Z", false).await;
        let page = list(
            &pool,
            ActionFilters {
                owner: Some("äNn".into()),
                due_from: Some("2026-12-01".into()),
                due_to: Some("2026-12-01".into()),
                series: Some(
                    json!(["microsoft", format!("graph:v1:{}", "a".repeat(64))]).to_string(),
                ),
                ..Default::default()
            },
            0,
            NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].meeting_id, "included");
        assert_eq!(page.series.len(), 2);
    }

    #[tokio::test]
    async fn filters_and_due_order_apply_before_pagination() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        occurrence(&pool, "first", "a", "2026-11-20T10:00:00Z", true).await;
        occurrence(&pool, "older", "a", "2026-11-19T10:00:00Z", true).await;
        let mut actions: Vec<_> = (3..=43).map(|id| action(id, "Due soon", false)).collect();
        let mut undated = action(44, "Undated", false);
        undated["dueDate"] = serde_json::Value::Null;
        actions.push(undated);
        let mut overdue = action(45, "Überfällig", false);
        overdue["dueDate"] = json!("2026-11-24");
        let mut late = action(46, "Later", false);
        late["dueDate"] = json!("2026-12-03");
        for (meeting, actions) in [
            ("first", actions),
            ("older", vec![overdue, late, action(2, "Completed", true)]),
        ] {
            sqlx::query("UPDATE meetings SET reviewed_actions = ? WHERE id = ?")
                .bind(json!(actions).to_string())
                .bind(meeting)
                .execute(&pool)
                .await
                .unwrap();
        }
        let first = list(&pool, ActionFilters::default(), 0, today)
            .await
            .unwrap();
        assert_eq!(first.rows.first().unwrap().action.title, "Überfällig");
        assert!(first.has_more);
        let second = list(&pool, ActionFilters::default(), 1, today)
            .await
            .unwrap();
        assert_eq!(second.rows.last().unwrap().action.title, "Undated");
        assert!(!second.has_more);
        for (status, search, title) in [
            (ActionStatus::Overdue, "ÜBER", "Überfällig"),
            (ActionStatus::DueSoon, "änne", "Due soon"),
            (ActionStatus::Completed, "synthetic meeting", "Completed"),
        ] {
            let result = list(
                &pool,
                ActionFilters {
                    status,
                    search: Some(search.into()),
                    ..Default::default()
                },
                0,
                today,
            )
            .await
            .unwrap();
            assert!(!result.rows.is_empty());
            assert!(result.rows.iter().all(|row| row.action.title == title));
        }
    }

    #[test]
    fn draft_uses_only_confirmed_addresses_and_encodes_headers_as_data() {
        let link = calendar::decode(
            &json!({
                "eventId":"meeting-draft", "attendees":[
                    {"email":"present+project@example.com","attendance":"present"},
                    {"email":"absent@example.com","attendance":"absent"},
                    {"email":"unknown@example.com","attendance":"unreviewed"},
                    {"email":"injection@example.com?bcc=evil@example.com","attendance":"present"}
                ],
            })
            .to_string(),
        )
        .unwrap()
        .unwrap();
        let allowed = draft_recipients(Some(&link));
        let selected = vec!["present+project@example.com".to_string()];
        assert_eq!(allowed.len(), 1);
        // Ad-hoc meetings may open a draft with no recipients.
        assert!(compose_uri(&allowed, &[], &[], "Subject", "Body")
            .unwrap()
            .starts_with("mailto:?subject="));
        let manual = compose_uri(
            &allowed,
            &[],
            &["guest.name+team@example.org".into()],
            "Subject",
            "Body",
        )
        .unwrap();
        assert!(manual.starts_with("mailto:guest.name%2Bteam@example.org?"));
        for injected in [
            "a@example.com?bcc=x@example.com",
            "a@example.com,b@example.com",
            "a b@example.com",
            "a%40@example.com",
            "a@example",
            "a@-example.com",
            "",
        ] {
            assert!(
                compose_uri(&allowed, &[], &[injected.into()], "Subject", "Body").is_err(),
                "{injected}"
            );
        }
        assert!(compose_uri(
            &allowed,
            &["absent@example.com".into()],
            &[],
            "Subject",
            "Body"
        )
        .is_err());
        assert!(compose_uri(&allowed, &selected, &[], "Subject\r\nBcc: injected", "Body").is_err());
        let url = compose_uri(
            &allowed,
            &selected,
            &[],
            "Review & decide",
            "Line one\nBcc: harmless",
        )
        .unwrap();
        assert_eq!(url, "mailto:present%2Bproject@example.com?subject=Review%20%26%20decide&body=Line%20one%0D%0ABcc%3A%20harmless");
    }
}
