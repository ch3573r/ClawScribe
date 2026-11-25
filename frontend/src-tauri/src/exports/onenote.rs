//! OneNote page payload builder.
//!
//! Produces well-formed, UTF-8 XHTML page bodies suitable for
//! `POST /me/onenote/.../pages`. Text-first by design: no scripts, forms,
//! included CSS, audio attachments, or `data-render-src` snapshots. Long
//! transcripts are split across a deterministic page series so each page stays
//! under the Microsoft Graph REST request size limit. See
//! `docs/integrations/onenote-export.md`.

use crate::exports::model::MeetingExport;

/// Microsoft Graph REST request size limit (4 MB). We split before reaching it.
pub const GRAPH_REST_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Default per-page budget, kept below the hard limit to leave envelope margin.
pub const DEFAULT_PAGE_BUDGET_BYTES: usize = 3 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneNotePage {
    /// Page title, also used as the `<title>` element.
    pub title: String,
    /// Kind discriminator used in the per-page dedupe key.
    pub kind: OneNotePageKind,
    /// Zero-based index within its kind (transcript pages are 0,1,2,...).
    pub index: usize,
    /// Well-formed XHTML body.
    pub xhtml: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneNotePageKind {
    Notes,
    Transcript,
}

impl OneNotePageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OneNotePageKind::Notes => "notes",
            OneNotePageKind::Transcript => "transcript",
        }
    }
}

/// Escape text for inclusion in XHTML element/attribute content.
pub fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Build the OneNote page series for a meeting.
///
/// Always emits one Notes page (summary, decisions, action items) followed by
/// zero or more Transcript pages when a transcript excerpt is present and large.
pub fn build_pages(meeting: &MeetingExport, page_budget_bytes: usize) -> Vec<OneNotePage> {
    let mut pages = vec![OneNotePage {
        title: meeting.title.clone(),
        kind: OneNotePageKind::Notes,
        index: 0,
        xhtml: build_notes_xhtml(meeting),
    }];

    if let Some(transcript) = meeting.transcript_excerpt.as_deref() {
        let trimmed = transcript.trim();
        if !trimmed.is_empty() {
            for (i, body) in build_transcript_pages(meeting, trimmed, page_budget_bytes)
                .into_iter()
                .enumerate()
            {
                pages.push(OneNotePage {
                    title: format!("{} - Transcript {}", meeting.title, i + 1),
                    kind: OneNotePageKind::Transcript,
                    index: i,
                    xhtml: body,
                });
            }
        }
    }

    pages
}

fn build_notes_xhtml(meeting: &MeetingExport) -> String {
    let title = escape_xml(&meeting.title);
    let created = meeting.created_at.as_deref().unwrap_or("");
    let mut body = String::new();

    body.push_str(&format!("<h1>{title}</h1>"));
    body.push_str(&meeting.metadata.html);
    body.push_str("<p><b>Recorded by:</b> ClawScribe</p>");

    body.push_str("<h2>Summary</h2>");
    match &meeting.summary_html {
        // Pre-rendered, already-sanitized XHTML (e.g. a full markdown summary).
        Some(html) if !html.trim().is_empty() => body.push_str(html),
        _ => body.push_str(&format!(
            "<p>{}</p>",
            escape_xml(&meeting.executive_summary)
        )),
    }

    if !meeting.decisions.is_empty() {
        body.push_str("<h2>Decisions</h2><ul>");
        for d in &meeting.decisions {
            let owner = d
                .owner
                .as_deref()
                .map(|o| format!(" ({})", escape_xml(o)))
                .unwrap_or_default();
            body.push_str(&format!("<li>{}{}</li>", escape_xml(&d.decision), owner));
        }
        body.push_str("</ul>");
    }

    if !meeting.action_items.is_empty() {
        body.push_str("<h2>Action Items</h2><ul>");
        for a in &meeting.action_items {
            let owner = a.owner.as_deref().unwrap_or("Unassigned");
            let due = a
                .due_date
                .as_deref()
                .map(|d| format!(" — due {}", escape_xml(d)))
                .unwrap_or_default();
            body.push_str(&format!(
                "<li>{} - {}{}</li>",
                escape_xml(owner),
                escape_xml(&a.task),
                due
            ));
        }
        body.push_str("</ul>");
    }

    let marker = format!(
        "{}{}",
        notes_marker_prefix(&meeting.meeting_id),
        notes_revision(meeting)
    );
    wrap_page(
        &title,
        created,
        &format!("<div><div data-id=\"{marker}\">{body}</div></div>"),
    )
}

pub(super) fn notes_marker_prefix(meeting_id: &str) -> String {
    format!(
        "clawscribe-{}-",
        super::ledger::artifact_key_hash(&["onenote-notes", meeting_id])
    )
}

pub(super) fn notes_revision(meeting: &MeetingExport) -> String {
    let artifact = meeting.artifact_hash();
    let mut parts = vec![
        artifact.as_str(),
        meeting.summary_html.as_deref().unwrap_or(""),
    ];
    parts.extend(meeting.metadata.bookmark_revision.as_deref());
    super::ledger::artifact_key_hash(&parts)
}

/// Split the transcript into page bodies that each stay under the byte budget.
/// Splitting is line-oriented and deterministic.
fn build_transcript_pages(
    meeting: &MeetingExport,
    transcript: &str,
    page_budget_bytes: usize,
) -> Vec<String> {
    let title = escape_xml(&meeting.title);
    let created = meeting.created_at.as_deref().unwrap_or("");

    // Reserve room for the exact wrapping envelope (an empty-body transcript
    // page) so the finished page fits the budget. Floor the content budget so a
    // tiny budget still makes progress.
    let envelope = render_transcript_page(&title, created, "").len();
    let content_budget = page_budget_bytes.saturating_sub(envelope).max(1024);

    let mut pages = Vec::new();
    let mut current = String::new();

    for line in transcript.lines() {
        let escaped = escape_xml(line);
        // +1 for the newline we re-add between lines.
        if !current.is_empty() && current.len() + escaped.len() + 1 > content_budget {
            pages.push(render_transcript_page(&title, created, &current));
            current.clear();
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&escaped);
    }
    if !current.is_empty() || pages.is_empty() {
        pages.push(render_transcript_page(&title, created, &current));
    }
    pages
}

fn render_transcript_page(title: &str, created: &str, escaped_pre_body: &str) -> String {
    let body = format!("<h1>{title}</h1><h2>Transcript</h2><pre>{escaped_pre_body}</pre>");
    wrap_page(title, created, &format!("<div>{body}</div>"))
}

fn wrap_page(escaped_title: &str, created: &str, body: &str) -> String {
    let meta = if created.is_empty() {
        String::new()
    } else {
        format!(
            "<meta name=\"created\" content=\"{}\" />",
            escape_xml(created)
        )
    };
    format!(
        "<!DOCTYPE html><html><head><title>{escaped_title}</title>{meta}</head><body>{body}</body></html>"
    )
}

/// Guard: reject page bodies that contain unsupported active content elements.
///
/// Detects raw active-element openings (`<script>`, `<form>`, `<iframe>`, …).
/// Generated pages never contain these because all dynamic text is escaped — so
/// escaped transcript text that merely *mentions* `onload=` or `javascript:` is
/// correctly treated as inert. This validates externally-supplied XHTML before
/// it is sent to Graph.
pub fn contains_active_content(xhtml: &str) -> bool {
    let lower = xhtml.to_ascii_lowercase();
    [
        "<script", "<form", "<iframe", "<object", "<embed", "<applet", "<base", "<link",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::model::{ExportActionItem, ExportDecision};

    fn sample(transcript: Option<String>) -> MeetingExport {
        MeetingExport {
            meeting_id: "2026-06-15-weekly-sync".into(),
            title: "Weekly <sync> & \"review\"".into(),
            created_at: Some("2026-06-15T10:00:00Z".into()),
            executive_summary: "We shipped a thing & decided <stuff>.".into(),
            decisions: vec![ExportDecision {
                decision: "Adopt plan A".into(),
                owner: Some("Alex".into()),
            }],
            action_items: vec![ExportActionItem {
                local_action_id: "action-1".into(),
                task: "Send proposal to <Contoso>".into(),
                owner: None,
                due_date: Some("2026-06-20".into()),
                details: None,
                legacy_action: None,
            }],
            transcript_excerpt: transcript,
            summary_html: None,
            metadata: Default::default(),
        }
    }

    #[test]
    fn notes_page_escapes_all_dynamic_text() {
        let pages = build_pages(&sample(None), DEFAULT_PAGE_BUDGET_BYTES);
        assert_eq!(pages.len(), 1);
        let xhtml = &pages[0].xhtml;
        // Raw angle brackets from content must not survive.
        assert!(!xhtml.contains("<sync>"));
        assert!(!xhtml.contains("<stuff>"));
        assert!(!xhtml.contains("<Contoso>"));
        assert!(xhtml.contains("&lt;sync&gt;"));
        assert!(xhtml.contains("&amp;"));
        assert!(xhtml.contains("Unassigned"));
        assert!(xhtml.contains("due 2026-06-20"));
        assert!(!contains_active_content(xhtml));
    }

    #[test]
    fn notes_header_renders_saved_metadata_and_versions_the_bookmark_selection() {
        let mut meeting = sample(None);
        let revision = notes_revision(&meeting);
        let header = super::super::metadata::Header {
            date: "2026-06-15 10:00".into(),
            duration: Some("00:30:00".into()),
            attendees: vec!["Alex & Dana".into()],
            tags: vec!["Roadmap".into()],
            bookmarks: vec![(42.0, "<Decision>".into())],
        };
        meeting.metadata = header.onenote(true);
        let pages = build_pages(&meeting, DEFAULT_PAGE_BUDGET_BYTES);
        let body = &pages[0].xhtml;
        assert!(body.contains("<li>Date: 2026-06-15 10:00</li><li>Duration: 00:30:00</li>"));
        assert!(body.contains("<li>Attendees: Alex &amp; Dana</li><li>Project tags: Roadmap</li>"));
        assert!(body.contains("<li><strong>[00:00:42]</strong> &lt;Decision&gt;</li>"));
        assert!(body.find("Notes and bookmarks").unwrap() < body.find("<h2>Summary").unwrap());
        assert_ne!(notes_revision(&meeting), revision);
        meeting.metadata = super::super::metadata::Header {
            bookmarks: Vec::new(),
            ..header
        }
        .onenote(true);
        assert_eq!(notes_revision(&meeting), revision);
    }

    #[test]
    fn generated_pages_have_no_active_content() {
        let nasty = MeetingExport {
            executive_summary: "<script>alert(1)</script>".into(),
            ..sample(Some("onload=evil <iframe>".into()))
        };
        for page in build_pages(&nasty, DEFAULT_PAGE_BUDGET_BYTES) {
            assert!(
                !contains_active_content(&page.xhtml),
                "escaped content must not be detected as active: {}",
                page.title
            );
        }
    }

    #[test]
    fn transcript_splits_under_budget() {
        // Build a transcript far larger than a tiny budget to force splitting.
        let lines: Vec<String> = (0..2000)
            .map(|i| format!("speaker{i}: hello there"))
            .collect();
        let transcript = lines.join("\n");
        let budget = 8 * 1024; // small budget to force multiple pages
        let pages = build_pages(&sample(Some(transcript)), budget);

        // 1 notes page + N transcript pages.
        assert!(pages.len() >= 3, "expected splitting, got {}", pages.len());
        let transcript_pages: Vec<_> = pages
            .iter()
            .filter(|p| p.kind == OneNotePageKind::Transcript)
            .collect();
        assert!(transcript_pages.len() >= 2);
        for (i, p) in transcript_pages.iter().enumerate() {
            assert_eq!(p.index, i);
            assert!(p.title.ends_with(&format!("Transcript {}", i + 1)));
            assert!(p.xhtml.len() <= budget, "page {} exceeded budget", i);
        }
    }

    #[test]
    fn single_transcript_page_when_small() {
        let pages = build_pages(&sample(Some("one line".into())), DEFAULT_PAGE_BUDGET_BYTES);
        let transcript_pages: Vec<_> = pages
            .iter()
            .filter(|p| p.kind == OneNotePageKind::Transcript)
            .collect();
        assert_eq!(transcript_pages.len(), 1);
    }

    #[test]
    fn empty_transcript_yields_no_transcript_page() {
        let pages = build_pages(&sample(Some("   \n  ".into())), DEFAULT_PAGE_BUDGET_BYTES);
        assert!(pages.iter().all(|p| p.kind == OneNotePageKind::Notes));
    }

    #[test]
    fn active_content_detector_flags_raw_elements() {
        assert!(contains_active_content("<script>x</script>"));
        assert!(contains_active_content("<FORM action=...>"));
        assert!(contains_active_content("<iframe src=...>"));
        assert!(contains_active_content("<object data=...>"));
        // Plain text and escaped markup are inert.
        assert!(!contains_active_content("<p>plain text</p>"));
        assert!(!contains_active_content(
            "the speaker said onload=now and javascript:void"
        ));
        assert!(!contains_active_content(
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        ));
    }
}
