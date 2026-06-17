//! Build exportable meeting content from a summary's **markdown**.
//!
//! ClawScribe persists meeting summaries as markdown, so per-meeting export
//! works from that text rather than a structured `MeetingNotesOutput`:
//!
//! - **OneNote** gets the *whole* summary rendered to sanitized XHTML
//!   (headings, lists, emphasis preserved), so nothing is lost.
//! - **Planner** gets just the discrete action items parsed out of the
//!   "Action items" / "Tasks" / "Next steps" section, one task each.
//!
//! All rendering escapes text first, so no markup from the summary can inject
//! active content into the OneNote page.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::exports::model::{ExportActionItem, MeetingExport};
use crate::exports::onenote::escape_xml;
use crate::exports::planner;

/// Headings whose text marks an action-item / task list.
static ACTION_HEADING: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(action items?|action points?|actions|tasks?|to-?dos?|next steps?|follow[- ]?ups?)\b")
        .expect("valid action heading regex")
});

/// An ISO-8601 date anywhere in a line (used for due-date extraction).
static ISO_DATE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b(\d{4}-\d{2}-\d{2})\b").expect("valid date regex"));

/// `@mention` owner hint.
static MENTION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"@([A-Za-z0-9._-]{2,})").expect("valid mention regex"));

/// Leading bullet / number marker on a list item.
static BULLET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*(?:[-*+]|\d+[.)])\s+").expect("valid bullet regex"));

/// Leading task-list checkbox, e.g. `[ ]` / `[x]`.
static CHECKBOX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*\[[ xX]\]\s*").expect("valid checkbox regex"));

/// A clock timestamp like `12:34`, `01:02:03`, optionally bracketed/parenthesized.
static CLOCK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\s*[\[(]?\b\d{1,2}:\d{2}(?::\d{2})?\b[\])]?\s*").expect("valid clock regex")
});

/// Inline `**bold**`.
static BOLD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*([^*]+)\*\*").expect("valid bold regex"));

/// A whole line that is just a bold label, e.g. `**Action items**` or
/// `**Decisions:**` — summaries often use these instead of `#` headings.
static BOLD_HEADING: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\*\*([^*]+?)\*\*:?$").expect("valid bold heading regex"));

/// Inline `` `code` ``.
static CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`]+)`").expect("valid code regex"));

fn heading_level(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed[hashes..].trim_start();
    Some((hashes as u8, rest))
}

fn bullet_text(line: &str) -> Option<String> {
    BULLET
        .find(line)
        .map(|m| line[m.end()..].trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Apply inline emphasis to an already XML-escaped string.
fn inline_xhtml(escaped: &str) -> String {
    // `**bold**` and `` `code` `` — markers aren't XML-special, so it's safe to
    // run these after escaping.
    let bolded = BOLD.replace_all(escaped, "<b>$1</b>");
    CODE.replace_all(&bolded, "<code>$1</code>").into_owned()
}

/// Render a markdown summary into a sanitized XHTML fragment suitable for a
/// OneNote page body. Supports headings, unordered/ordered lists, bold, inline
/// code, and paragraphs. Everything else is treated as paragraph text.
pub fn markdown_to_xhtml(markdown: &str) -> String {
    let mut out = String::new();
    let mut in_list = false;
    let mut paragraph: Vec<String> = Vec::new();

    let flush_paragraph = |out: &mut String, paragraph: &mut Vec<String>| {
        if !paragraph.is_empty() {
            out.push_str("<p>");
            out.push_str(&paragraph.join("<br/>"));
            out.push_str("</p>");
            paragraph.clear();
        }
    };
    let close_list = |out: &mut String, in_list: &mut bool| {
        if *in_list {
            out.push_str("</ul>");
            *in_list = false;
        }
    };

    for raw in markdown.lines() {
        let line = raw.trim_end();

        if line.trim().is_empty() {
            flush_paragraph(&mut out, &mut paragraph);
            close_list(&mut out, &mut in_list);
            continue;
        }

        if let Some((level, text)) = heading_level(line) {
            flush_paragraph(&mut out, &mut paragraph);
            close_list(&mut out, &mut in_list);
            // Page title is <h1>; map summary headings to h2..h4.
            let tag = match level {
                1 => "h2",
                2 => "h3",
                _ => "h4",
            };
            out.push_str(&format!("<{tag}>{}</{tag}>", inline_xhtml(&escape_xml(text))));
            continue;
        }

        if let Some(item) = bullet_text(line) {
            flush_paragraph(&mut out, &mut paragraph);
            if !in_list {
                out.push_str("<ul>");
                in_list = true;
            }
            out.push_str(&format!("<li>{}</li>", inline_xhtml(&escape_xml(&item))));
            continue;
        }

        close_list(&mut out, &mut in_list);
        paragraph.push(inline_xhtml(&escape_xml(line.trim())));
    }

    flush_paragraph(&mut out, &mut paragraph);
    close_list(&mut out, &mut in_list);
    out
}

/// Extract discrete action items from the summary markdown. Looks for an
/// "Action items"/"Tasks"/"Next steps" heading and collects the bullet list
/// that follows it (until the next heading). Owner (`@mention` / `Owner: X`)
/// and an ISO due date are extracted best-effort; nothing is auto-mapped to a
/// directory user.
pub fn parse_action_items(markdown: &str) -> Vec<ExportActionItem> {
    let mut items: Vec<ExportActionItem> = Vec::new();
    let mut in_action_section = false;

    for raw in markdown.lines() {
        let line = raw.trim_end();

        // A markdown heading switches the section on/off.
        if let Some((_, text)) = heading_level(line) {
            in_action_section = ACTION_HEADING.is_match(text);
            continue;
        }
        // A bold-only line (e.g. `**Decisions**`) acts as a pseudo-heading, so a
        // section divider that isn't a `#` heading still bounds the action list.
        if let Some(caps) = BOLD_HEADING.captures(line.trim()) {
            in_action_section = ACTION_HEADING.is_match(&caps[1]);
            continue;
        }
        if !in_action_section {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some(item_text) = bullet_text(line) else {
            // Non-bullet prose ends the list, so we don't keep collecting items
            // from whatever follows it under the same (or no) heading.
            in_action_section = false;
            continue;
        };

        let due_date = ISO_DATE
            .captures(&item_text)
            .map(|c| c[1].to_string());

        let owner = MENTION
            .captures(&item_text)
            .map(|c| c[1].to_string())
            .or_else(|| extract_labeled_owner(&item_text));

        let task = clean_task_text(&item_text);
        if task.is_empty() {
            continue;
        }

        items.push(ExportActionItem {
            local_action_id: String::new(),
            task,
            owner,
            due_date,
        });
    }

    planner::ensure_local_action_ids(&mut items);
    items
}

fn extract_labeled_owner(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let idx = lower.find("owner:").or_else(|| lower.find("assigned to:"))?;
    let after = &text[idx..];
    let value = after.split(':').nth(1)?.trim();
    // Stop at common separators.
    let value = value
        .split(['|', '—', '(', '[', ';'])
        .next()
        .unwrap_or(value)
        .trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Strip trailing owner/due annotations so the task title reads cleanly.
fn clean_task_text(text: &str) -> String {
    // Drop a leading task-list checkbox ("[ ]"/"[x]") and any clock timestamps
    // so they don't end up in the Planner task title.
    let without_checkbox = CHECKBOX.replace(text, "");
    let without_clock = CLOCK.replace_all(&without_checkbox, " ");
    let mut t = without_clock.split_whitespace().collect::<Vec<_>>().join(" ");
    // Drop a trailing "(...)" or "[...]" annotation if it looks like metadata.
    if let Some(pos) = t.rfind(" (") {
        let tail = &t[pos..].to_lowercase();
        if tail.contains("owner")
            || tail.contains("due")
            || tail.contains('@')
            || ISO_DATE.is_match(tail)
        {
            t.truncate(pos);
        }
    }
    t.trim().trim_end_matches(['-', '—', ':']).trim().to_string()
}

/// Whether the summary contains at least one parsed action item.
pub fn has_action_items(markdown: &str) -> bool {
    !parse_action_items(markdown).is_empty()
}

/// A [`MeetingExport`] for OneNote: the whole summary as rendered XHTML, with no
/// duplicated structured sections (the rendered summary already contains them).
pub fn meeting_export_for_onenote(
    meeting_id: impl Into<String>,
    title: impl Into<String>,
    created_at: Option<String>,
    markdown: &str,
) -> MeetingExport {
    let html = markdown_to_xhtml(markdown);
    MeetingExport {
        meeting_id: meeting_id.into(),
        title: title.into(),
        created_at,
        executive_summary: plain_excerpt(markdown),
        decisions: Vec::new(),
        action_items: Vec::new(),
        transcript_excerpt: None,
        summary_html: Some(html),
    }
}

/// A [`MeetingExport`] for Planner: parsed action items only.
pub fn meeting_export_for_planner(
    meeting_id: impl Into<String>,
    title: impl Into<String>,
    created_at: Option<String>,
    markdown: &str,
) -> MeetingExport {
    MeetingExport {
        meeting_id: meeting_id.into(),
        title: title.into(),
        created_at,
        executive_summary: plain_excerpt(markdown),
        decisions: Vec::new(),
        action_items: parse_action_items(markdown),
        transcript_excerpt: None,
        summary_html: None,
    }
}

/// A short plain-text excerpt (markers stripped) for fallback contexts.
fn plain_excerpt(markdown: &str) -> String {
    let text: String = markdown
        .lines()
        .map(|l| {
            let l = l.trim();
            let l = l.trim_start_matches('#').trim_start();
            BULLET.replace(l, "").into_owned()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    text.chars().take(800).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Meeting Summary

We aligned on the **Q3 roadmap** and cleared two blockers.

## Decisions
- Adopt plan A
- Ship the beta on 2026-07-01

## Action Items
- Send the proposal to Contoso (due 2026-06-20) @alex
- Owner: Sam — Draft the migration plan
- Review designs

## Notes
Some closing thoughts.";

    #[test]
    fn renders_headings_lists_and_bold_escaped() {
        let html = markdown_to_xhtml(SAMPLE);
        assert!(html.contains("<h2>Meeting Summary</h2>"));
        assert!(html.contains("<h3>Decisions</h3>"));
        assert!(html.contains("<b>Q3 roadmap</b>"));
        assert!(html.contains("<ul><li>Adopt plan A</li>"));
        // No raw markdown markers leak through as headings.
        assert!(!html.contains("## Decisions"));
    }

    #[test]
    fn xhtml_escapes_dangerous_text() {
        let html = markdown_to_xhtml("- <script>alert(1)</script>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn parses_action_items_with_owner_and_due() {
        let items = parse_action_items(SAMPLE);
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| !i.local_action_id.is_empty()));

        let proposal = &items[0];
        assert!(proposal.task.starts_with("Send the proposal to Contoso"));
        assert_eq!(proposal.owner.as_deref(), Some("alex"));
        assert_eq!(proposal.due_date.as_deref(), Some("2026-06-20"));

        assert_eq!(items[1].owner.as_deref(), Some("Sam"));
        assert!(items[2].owner.is_none());
        assert!(items[2].due_date.is_none());
    }

    #[test]
    fn decisions_section_is_not_treated_as_actions() {
        // "Ship the beta on 2026-07-01" lives under Decisions, not Action Items.
        let items = parse_action_items(SAMPLE);
        assert!(items.iter().all(|i| !i.task.contains("Ship the beta")));
    }

    #[test]
    fn bold_pseudo_heading_bounds_the_action_list() {
        // Summaries that use **bold** labels instead of `#` headings must still
        // stop the action list at the next (non-action) bold label.
        let md = "\
**Action Items**
- Do the thing @bob
- Email the client

**Decisions**
- Adopt plan B
- Hire two engineers";
        let items = parse_action_items(md);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|i| i.task.starts_with("Do the thing")));
        assert!(items.iter().all(|i| !i.task.contains("Adopt plan B")));
        assert!(items.iter().all(|i| !i.task.contains("Hire two engineers")));
    }

    #[test]
    fn prose_after_the_list_ends_the_section() {
        let md = "\
## Next Steps
- File the report

Separately, the team also reviewed the budget and noted:
- This is commentary, not a task";
        let items = parse_action_items(md);
        assert_eq!(items.len(), 1);
        assert!(items[0].task.starts_with("File the report"));
    }

    #[test]
    fn has_action_items_detects_presence_and_absence() {
        assert!(has_action_items(SAMPLE));
        assert!(!has_action_items("# Summary\n\nNo tasks here, just prose."));
    }
}
