use std::fmt::Write;

use sqlx::SqlitePool;
use tauri::State;

use crate::{library::calendar, state::AppState};

#[derive(Debug, Default)]
pub(crate) struct Header {
    pub(crate) date: String,
    pub(crate) duration: Option<String>,
    pub(crate) attendees: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) bookmarks: Vec<(f64, String)>,
}

#[derive(Default)]
pub(super) struct DocumentHeader {
    pub markdown: String,
    pub bookmark_revision: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OneNoteHeader {
    pub(super) html: String,
    pub(super) bookmark_revision: Option<String>,
    pub(super) revision: Option<String>,
}

impl Header {
    pub(crate) async fn load(
        pool: &SqlitePool,
        meeting_id: &str,
        include_bookmarks: bool,
    ) -> Result<Self, String> {
        if include_bookmarks {
            crate::library::attach_bookmarks(pool)
                .await
                .map_err(|_| "Could not load bookmarks.")?;
        }
        let mut tx = pool
            .begin()
            .await
            .map_err(|_| "Could not read meeting details.")?;
        let (created, duration, calendar_link): (String, Option<f64>, Option<String>) =
            sqlx::query_as(
                "SELECT created_at, recording_duration_s, calendar_link FROM active_meetings WHERE id = ?",
            )
            .bind(meeting_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| "Could not read meeting details.")?;
        let tags =
            sqlx::query_scalar("SELECT tag FROM meeting_tags WHERE meeting_id = ? ORDER BY tag")
                .bind(meeting_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(|_| "Could not read project tags.")?;
        let bookmarks = if include_bookmarks {
            sqlx::query_as("SELECT seconds, label FROM meeting_bookmarks WHERE meeting_id = ? ORDER BY seconds, id")
                .bind(meeting_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(|_| "Could not load bookmarks.")?
        } else {
            Vec::new()
        };
        tx.commit()
            .await
            .map_err(|_| "Could not read meeting details.")?;
        let attendees = calendar_link
            .as_deref()
            .and_then(|raw| calendar::decode(raw).ok().flatten())
            .filter(calendar::CalendarLink::is_reviewed)
            .map(|link| {
                link.confirmed_names()
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            date: export_meeting_date(&created),
            duration: duration
                .filter(|time| time.is_finite() && *time >= 0.0)
                .map(|time| export_timestamp("", Some(time))),
            attendees,
            tags,
            bookmarks,
        })
    }

    fn fields(&self) -> impl Iterator<Item = (&'static str, String)> + '_ {
        [
            ("Date", self.date.clone()),
            ("Duration", self.duration.clone().unwrap_or_default()),
            ("Attendees", self.attendees.join(", ")),
            ("Project tags", self.tags.join(", ")),
        ]
        .into_iter()
        .filter(|(_, value)| !value.trim().is_empty())
    }

    fn bookmark_rows(&self, timestamps: bool) -> impl Iterator<Item = (Option<String>, &str)> {
        self.bookmarks.iter().map(move |(seconds, label)| {
            let time = export_timestamp("", timestamps.then_some(*seconds));
            ((!time.is_empty()).then_some(time), label.as_str())
        })
    }

    fn revision(&self) -> Option<String> {
        let fields: Vec<_> = self.fields().collect();
        (!fields.is_empty()).then(|| {
            let parts: Vec<_> = fields
                .iter()
                .flat_map(|(label, value)| [*label, value.as_str()])
                .collect();
            super::ledger::artifact_key_hash(&parts)
        })
    }

    // Bookmarks are a reviewed content selection. Keep them in artifact
    // identity while retaining the legacy keys when none are included.
    fn bookmark_revision(&self, timestamps: bool) -> Option<String> {
        let fields: Vec<_> = self
            .bookmark_rows(timestamps)
            .flat_map(|(time, label)| [time.unwrap_or_default(), inline_text(label)])
            .collect();
        (!fields.is_empty()).then(|| {
            super::ledger::artifact_key_hash(&fields.iter().map(String::as_str).collect::<Vec<_>>())
        })
    }

    pub(super) fn document(&self, timestamps: bool) -> DocumentHeader {
        DocumentHeader {
            markdown: self.markdown(timestamps),
            bookmark_revision: self.bookmark_revision(timestamps),
            revision: self.revision(),
        }
    }

    pub(super) fn onenote(&self, timestamps: bool) -> OneNoteHeader {
        OneNoteHeader {
            html: self.xhtml(timestamps),
            bookmark_revision: self.bookmark_revision(timestamps),
            revision: self.revision(),
        }
    }

    pub(crate) fn markdown(&self, timestamps: bool) -> String {
        let mut output = String::new();
        for (label, value) in self.fields() {
            let _ = writeln!(output, "- {label}: {}", escape_inline(&value));
        }
        if !self.bookmarks.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            let _ = writeln!(output, "## {BOOKMARK_HEADING}\n");
            for (time, label) in self.bookmark_rows(timestamps) {
                let time = time
                    .map(|time| format!("**[{time}]** "))
                    .unwrap_or_default();
                let _ = writeln!(output, "- {time}{}", escape_inline(label));
            }
        }
        output
    }

    pub(super) fn xhtml(&self, timestamps: bool) -> String {
        use super::onenote::escape_xml;

        let mut output = String::new();
        for (label, value) in self.fields() {
            let _ = write!(
                output,
                "<li>{label}: {}</li>",
                escape_xml(&inline_text(&value))
            );
        }
        if !output.is_empty() {
            output = format!("<ul>{output}</ul>");
        }
        if !self.bookmarks.is_empty() {
            let _ = write!(output, "<h2>{BOOKMARK_HEADING}</h2><ul>");
            for (time, label) in self.bookmark_rows(timestamps) {
                let time = time
                    .map(|time| format!("<strong>[{time}]</strong> "))
                    .unwrap_or_default();
                let _ = write!(output, "<li>{time}{}</li>", escape_xml(&inline_text(label)));
            }
            output.push_str("</ul>");
        }
        output
    }
}

const BOOKMARK_HEADING: &str = "Notes and bookmarks";

pub(crate) fn markdown_title(title: &str) -> String {
    let title = if title.trim().is_empty() {
        "Meeting"
    } else {
        title
    };
    format!("# {}\n", escape_inline(title))
}

fn inline_text(value: &str) -> String {
    crate::summary::sources::strip_source_links(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn escape_inline(value: &str) -> String {
    let mut escaped = String::new();
    // Inline fields stay on one line, even if pasted text contains line breaks.
    let line = inline_text(value);
    let list_marker = line.bytes().take_while(u8::is_ascii_digit).count();
    let mut previous = None;
    let mut rest = line.chars().skip(1);
    for (index, ch) in line.char_indices() {
        let next = rest.next();
        // An underscore between letters or digits is never emphasis, as in a
        // default title such as Meeting 22_11_26.
        let inside_word = ch == '_'
            && previous.is_some_and(char::is_alphanumeric)
            && next.is_some_and(char::is_alphanumeric);
        previous = Some(ch);
        // Escape only what changes meaning here, so the file stays readable as text:
        // inline markup anywhere, block markers at the start of the line.
        if (matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '<') && !inside_word)
            || (index == 0 && matches!(ch, '#' | '-' | '+' | '>'))
            || (index == list_marker && list_marker > 0 && matches!(ch, '.' | ')'))
        {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

pub(crate) fn export_timestamp(timestamp: &str, audio_start_time: Option<f64>) -> String {
    match audio_start_time.filter(|time| time.is_finite() && *time >= 0.0) {
        Some(time) => {
            let seconds = time.floor() as u64;
            format!(
                "{:02}:{:02}:{:02}",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            )
        }
        None => timestamp.trim().to_string(),
    }
}

pub(crate) fn export_meeting_date(created: &str) -> String {
    let date = chrono::DateTime::parse_from_rfc3339(created)
        .map(|date| date.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(created, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|date| date.and_utc())
        });
    date.map(|date| {
        date.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
    .unwrap_or_else(|| created.to_string())
}

#[tauri::command]
pub(crate) async fn get_meeting_export_header(
    state: State<'_, AppState>,
    meeting_id: String,
    title: String,
    include_bookmarks: bool,
    timestamps: bool,
) -> Result<ExportHeader, String> {
    let header = Header::load(state.db_manager.pool(), &meeting_id, include_bookmarks).await?;
    let html_title = super::onenote::escape_xml(&inline_text(&title));
    let html_title = if html_title.is_empty() {
        "Meeting"
    } else {
        &html_title
    };
    Ok(ExportHeader {
        markdown: format!(
            "{}\n{}",
            markdown_title(&title),
            header.markdown(timestamps)
        ),
        html: format!("<h1>{html_title}</h1>{}", header.xhtml(timestamps)),
    })
}

#[derive(serde::Serialize)]
pub(crate) struct ExportHeader {
    markdown: String,
    html: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canonical_header_uses_reviewed_attendance_and_only_selected_bookmarks() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let calendar = serde_json::json!({
            "eventId": "example", "reviewed": true, "attendees": [
                {"name":"Alex & Dana","attendance":"present"},
                {"name":"Unknown guest"}
            ]
        });
        sqlx::query("UPDATE meetings SET created_at = '2026-11-22', recording_duration_s = 125, calendar_link = ? WHERE id = 'review-test'")
            .bind(calendar.to_string()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meeting_tags VALUES ('review-test','Review *draft*')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meeting_bookmarks (id, meeting_id, seconds, label) VALUES ('example', 'review-test', 12, '<Decision>')")
            .execute(&pool).await.unwrap();
        let header = Header::load(&pool, "review-test", true).await.unwrap();
        assert_eq!(
            header.markdown(true),
            "- Date: 2026-11-22\n- Duration: 00:02:05\n- Attendees: Alex & Dana\n- Project tags: Review \\*draft\\*\n\n## Notes and bookmarks\n\n- **[00:00:12]** \\<Decision>\n",
        );
        let html = header.xhtml(false);
        assert!(html.contains("<li>Attendees: Alex &amp; Dana</li>"));
        assert!(html.contains("<li>&lt;Decision&gt;</li>"));
        assert!(!html.contains("00:00:12"));
        assert!(!html.contains("Unknown guest"));
        let without = Header::load(&pool, "review-test", false).await.unwrap();
        assert!(!without.markdown(true).contains("Decision"));
    }
}
