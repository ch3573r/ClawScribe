//! Build exportable meeting content from a summary's **markdown**.
//!
//! ClawScribe persists meeting summaries as markdown, so per-meeting export
//! works from that text rather than a structured `MeetingNotesOutput`:
//!
//! - **OneNote** gets the *whole* summary rendered to sanitized XHTML
//!   (headings, lists, tables, emphasis preserved), so nothing is lost.
//! - **Planner** gets just the discrete action items parsed out of the
//!   "Action items" / "Tasks" / "Next steps" section, one task each.
//!
//! All rendering escapes text first, so no markup from the summary can inject
//! active content into the OneNote page.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::exports::document::{is_table_delimiter, split_table_row};
use crate::exports::model::{ExportActionItem, MeetingExport};
use crate::exports::onenote::escape_xml;

/// Headings whose text marks an action-item / task list.
static ACTION_HEADING: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(action items?|action points?|actions|tasks?|to-?dos?|next steps?|follow[- ]?ups?|aufgaben|aktionspunkte|maßnahmen|nächste schritte|tâches|actions à mener|prochaines étapes|tareas|acciones|próximos pasos|tarefas|próximos passos|azioni|attività|prossimi passi|taken|volgende stappen|zadania|kolejne kroki|задачи|следующие шаги)\b|(?:アクションアイテム|次のステップ|行动项|待办事项|後續行動|다음 단계)")
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

/// Match spans in the original text. A bold span can contain a whole code span.
static INLINE_SPAN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"`([^`]+)`|\*\*((?:[^*`]+|`[^`]+`)+)\*\*").expect("valid inline span regex")
});

/// A whole line that is just a bold label, e.g. `**Action items**` or
/// `**Decisions:**` — summaries often use these instead of `#` headings.
static BOLD_HEADING: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\*\*([^*]+?)\*\*:?$").expect("valid bold heading regex"));

static INLINE_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`]+)`").expect("valid code regex"));

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
    INLINE_SPAN
        .replace_all(escaped, |span: &regex::Captures<'_>| match span.get(1) {
            Some(code) => format!("<code>{}</code>", code.as_str()),
            None => format!(
                "<b>{}</b>",
                INLINE_CODE.replace_all(&span[2], "<code>$1</code>")
            ),
        })
        .into_owned()
}

/// Render a markdown summary into a sanitized XHTML fragment suitable for a
/// OneNote page body. Supports headings, unordered/ordered lists, bold, inline
/// code, tables, and paragraphs. Everything else is treated as paragraph text.
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

    let mut lines = markdown.lines().peekable();
    while let Some(raw) = lines.next() {
        let line = raw.trim_end();

        if line.trim().is_empty() {
            flush_paragraph(&mut out, &mut paragraph);
            close_list(&mut out, &mut in_list);
            continue;
        }

        if line.trim().starts_with('|') {
            let header = split_table_row(line);
            if lines
                .peek()
                .is_some_and(|next| is_table_delimiter(next, header.len()))
            {
                flush_paragraph(&mut out, &mut paragraph);
                close_list(&mut out, &mut in_list);
                lines.next();
                out.push_str("<table><tbody><tr>");
                for cell in &header {
                    out.push_str(&format!("<th>{}</th>", inline_xhtml(&escape_xml(cell))));
                }
                out.push_str("</tr>");
                while lines
                    .peek()
                    .is_some_and(|next| next.trim().starts_with('|'))
                {
                    let mut row = split_table_row(lines.next().unwrap());
                    row.resize(header.len(), String::new());
                    out.push_str("<tr>");
                    for cell in row {
                        out.push_str(&format!("<td>{}</td>", inline_xhtml(&escape_xml(&cell))));
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</tbody></table>");
                continue;
            }
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
            out.push_str(&format!(
                "<{tag}>{}</{tag}>",
                inline_xhtml(&escape_xml(text))
            ));
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

/// An action found in a summary, awaiting local review.
pub(crate) struct ActionSuggestion {
    pub action: ExportActionItem,
    pub done: bool,
    pub source_key: Option<String>,
}

static SOURCE_LINK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[[^\]\n]*\]\(#clawscribe-source-([0-9a-f]{1,64})\)").unwrap());

pub fn parse_action_items(markdown: &str) -> Vec<ExportActionItem> {
    parse_action_suggestions(markdown)
        .into_iter()
        .filter(|item| !item.done)
        .map(|item| item.action)
        .collect()
}

pub(crate) fn parse_action_suggestions(markdown: &str) -> Vec<ActionSuggestion> {
    let mut items = Vec::new();
    let mut in_action_section = false;
    let mut lines = markdown.lines().peekable();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if let Some((_, text)) = heading_level(line) {
            in_action_section = ACTION_HEADING.is_match(text);
            continue;
        }
        if let Some(caps) = BOLD_HEADING.captures(line) {
            in_action_section = ACTION_HEADING.is_match(&caps[1]);
            continue;
        }
        if !in_action_section || line.is_empty() {
            continue;
        }
        if let Some(text) = bullet_text(line) {
            if let Some(item) = action_suggestion(&text, None, None, None) {
                items.push(item);
            }
            continue;
        }
        if line.contains('|') {
            let cells = split_table_row(line);
            if lines
                .peek()
                .is_some_and(|next| is_table_delimiter(next, cells.len()))
            {
                lines.next();
                let headers: Vec<_> = cells
                    .iter()
                    .map(|cell| cell.trim_matches('*').trim().to_lowercase())
                    .collect();
                let column = |names: &[&str]| {
                    headers
                        .iter()
                        .position(|header| names.contains(&header.as_str()))
                };
                let task = column(&[
                    "task",
                    "action",
                    "action item",
                    "next step",
                    "aufgabe",
                    "aktion",
                    "maßnahme",
                    "tâche",
                    "tarea",
                    "acción",
                    "tarefa",
                    "attività",
                ]);
                let owner = column(&[
                    "owner",
                    "assignee",
                    "assigned to",
                    "verantwortlich",
                    "zuständig",
                    "responsable",
                    "responsável",
                    "responsabile",
                ]);
                let due = column(&[
                    "due",
                    "due date",
                    "deadline",
                    "fällig",
                    "fällig am",
                    "frist",
                    "échéance",
                    "fecha límite",
                    "prazo",
                    "scadenza",
                ]);
                let status = column(&["status", "state", "estado", "statut", "stato"]);
                while lines.peek().is_some_and(|next| {
                    !next.trim().is_empty()
                        && next.contains('|')
                        && heading_level(next).is_none()
                        && bullet_text(next).is_none()
                }) {
                    let row = split_table_row(lines.next().unwrap());
                    let cell = |index: Option<usize>| {
                        index.and_then(|index| row.get(index)).map(String::as_str)
                    };
                    if let Some(text) = cell(task) {
                        if let Some(mut item) =
                            action_suggestion(text, cell(owner), cell(due), cell(status))
                        {
                            if item.source_key.is_none() {
                                item.source_key = row.iter().find_map(|text| source_key(text));
                            }
                            items.push(item);
                        }
                    }
                }
                continue;
            }
        }
        // Prose ends the action list.
        in_action_section = false;
    }

    for (index, item) in items.iter_mut().enumerate() {
        item.action.local_action_id = format!("action-{}", index + 1);
    }
    items
}

pub(super) fn source_key(text: &str) -> Option<String> {
    SOURCE_LINK
        .captures(text)
        .map(|captures| captures[1].to_owned())
}

pub(super) fn action_suggestion(
    text: &str,
    owner: Option<&str>,
    due: Option<&str>,
    status: Option<&str>,
) -> Option<ActionSuggestion> {
    let without_sources = SOURCE_LINK.replace_all(text, "");
    let task = clean_task_text(&without_sources);
    if task.is_empty() || matches!(task.as_str(), "-" | "—") {
        return None;
    }
    let owner = owner
        .map(str::trim)
        .filter(|name| {
            !matches!(
                name.to_lowercase().as_str(),
                "" | "-" | "—" | "unassigned" | "nicht zugewiesen"
            )
        })
        .map(str::to_owned)
        .or_else(|| MENTION.captures(&without_sources).map(|c| c[1].to_owned()))
        .or_else(|| extract_labeled_owner(&without_sources));
    let due_date = ISO_DATE
        .captures(due.unwrap_or(&without_sources))
        .map(|c| c[1].to_owned());
    let done = matches!(text.get(..3), Some("[x]" | "[X]"))
        || status.is_some_and(|value| {
            matches!(
                value.trim().to_lowercase().as_str(),
                "done"
                    | "completed"
                    | "erledigt"
                    | "abgeschlossen"
                    | "terminé"
                    | "completado"
                    | "concluído"
                    | "completato"
            )
        });
    Some(ActionSuggestion {
        action: ExportActionItem {
            local_action_id: String::new(),
            task,
            owner,
            due_date,
            details: None,
            legacy_action: None,
        },
        done,
        source_key: source_key(text),
    })
}

static OWNER_LABEL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:owner|assigned to|verantwortlich|zuständig|responsable|responsável|responsabile)\s*:").unwrap()
});

fn extract_labeled_owner(text: &str) -> Option<String> {
    let after = &text[OWNER_LABEL.find(text)?.start()..];
    let value = after.split(':').nth(1)?.trim();
    // Stop at common separators.
    let value = value
        .split(['|', '—', '(', '[', ';'])
        .next()
        .unwrap_or(value)
        .trim();
    if value.is_empty() || value.eq_ignore_ascii_case("unassigned") {
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
    let mut t = without_clock
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Drop a trailing "(...)" or "[...]" annotation if it looks like metadata.
    // Besides owner/due hints we also strip transcript provenance like
    // "(timestamp: …; confidence: low)" that can leak into the summary text,
    // so it never ends up in a Planner task title.
    if let Some(pos) = t.rfind(" (") {
        let tail = &t[pos..].to_lowercase();
        if tail.contains("owner")
            || tail.contains("due")
            || tail.contains('@')
            || tail.contains("timestamp")
            || tail.contains("confidence")
            || ISO_DATE.is_match(tail)
        {
            t.truncate(pos);
        }
    }
    // Drop an inline "Owner: …; Due: …" tail, the form the standard template
    // asks for, so only the task itself becomes the title.
    if let Some(label) = OWNER_LABEL.find(&t).filter(|label| label.start() > 0) {
        t.truncate(label.start());
    }
    t.trim()
        .trim_end_matches(['-', '—', ':', '('])
        .trim()
        .to_string()
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
    let markdown = crate::summary::sources::strip_source_links(markdown);
    let markdown = markdown.as_str();
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
        metadata: Default::default(),
    }
}

/// A [`MeetingExport`] for Planner: parsed action items only.
pub fn meeting_export_for_planner(
    meeting_id: impl Into<String>,
    title: impl Into<String>,
    created_at: Option<String>,
    markdown: &str,
) -> MeetingExport {
    let actions = parse_action_items(markdown);
    let markdown = crate::summary::sources::strip_source_links(markdown);
    let markdown = markdown.as_str();
    MeetingExport {
        meeting_id: meeting_id.into(),
        title: title.into(),
        created_at,
        executive_summary: plain_excerpt(markdown),
        decisions: Vec::new(),
        action_items: actions,
        transcript_excerpt: None,
        summary_html: None,
        metadata: Default::default(),
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

    #[test]
    fn fallback_export_excludes_completed_tasks_with_their_original_positions_reserved() {
        let markdown = "## Action items\n- [x] Already sent\n- [ ] Send the revised draft";
        let open = parse_action_items(markdown);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].task, "Send the revised draft");
        assert_eq!(open[0].local_action_id, "action-2");
    }

    #[test]
    fn parses_localized_lists_and_project_sync_tables() {
        let markdown = "## Aufgaben\n- [x] Bericht senden [00:01:18](#clawscribe-source-abc123)\n\n## Entscheidungen\n- Kein Auftrag\n\n## Próximos pasos\n| **Responsable** | **Tarea** | **Fecha límite** | **Estado** |\n| --- | --- | --- | --- |\n| Éva | Enviar informe | 2026-11-20 | Completado |\n\n## Action Items\n| **Owner** | **Task** | **Due Date** | **Priority** | **Status** |\n| --- | --- | --- | --- | --- |\n| Dana | Send draft | 2026-11-22 | High | Open |";
        let suggestions = parse_action_suggestions(markdown);
        assert_eq!(
            suggestions
                .iter()
                .map(|item| (item.action.task.as_str(), item.done))
                .collect::<Vec<_>>(),
            [
                ("Bericht senden", true),
                ("Enviar informe", true),
                ("Send draft", false)
            ]
        );
        assert_eq!(suggestions[1].action.owner.as_deref(), Some("Éva"));
        assert_eq!(
            suggestions[2].action.due_date.as_deref(),
            Some("2026-11-22")
        );
        let open = parse_action_items(markdown);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].local_action_id, "action-3");
    }

    #[test]
    fn xhtml_tables_escape_cells_and_close_surrounding_blocks() {
        let table = "| **Name** | Note |\n| :--- | ---: |\n| Ana | <script>alert(1)</script> |\n| Ben | left \\| right | extra |\n| Sam |";
        let expected = "<table><tbody><tr><th><b>Name</b></th><th>Note</th></tr><tr><td>Ana</td><td>&lt;script&gt;alert(1)&lt;/script&gt;</td></tr><tr><td>Ben</td><td>left | right</td></tr><tr><td>Sam</td><td></td></tr></tbody></table>";
        assert_eq!(markdown_to_xhtml(table), expected);
        assert_eq!(
            markdown_to_xhtml(&format!("Before\n{table}\n\nAfter")),
            format!("<p>Before</p>{expected}<p>After</p>")
        );
        assert_eq!(
            markdown_to_xhtml(&format!("- Before\n{table}")),
            format!("<ul><li>Before</li></ul>{expected}")
        );
        assert_eq!(
            markdown_to_xhtml("| not a table |"),
            "<p>| not a table |</p>"
        );
        assert!(!markdown_to_xhtml("| Name |\n| -- |").contains("<table>"));
    }

    #[test]
    fn export_sources_keep_readable_times_without_internal_links() {
        let markdown =
            "## Action items\n- Send draft to finance [00:12:34](#clawscribe-source-abc123)";
        let planner = meeting_export_for_planner("meeting", "Notes", None, markdown);
        assert_eq!(planner.action_items[0].task, "Send draft to finance");
        assert!(has_action_items(markdown));
        let onenote = meeting_export_for_onenote("meeting", "Notes", None, markdown);
        let html = onenote.summary_html.unwrap();
        assert!(html.contains("(00:12:34)"));
        assert!(!html.contains("clawscribe-source"));
        assert!(!onenote.executive_summary.contains("clawscribe-source"));
    }

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
    fn parses_the_standard_template_action_list() {
        let items = parse_action_items(
            "**Action Items**\n- Send the budget to finance. Owner: Dana; Due: 2026-06-20 [00:01:18](#clawscribe-source-abc123)\n- Find out who approves the contract. Owner: Unassigned; Due: Not specified\n\n**Open Questions**\n- Who signs?",
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task, "Send the budget to finance.");
        assert_eq!(items[0].owner.as_deref(), Some("Dana"));
        assert_eq!(items[0].due_date.as_deref(), Some("2026-06-20"));
        assert_eq!(items[1].task, "Find out who approves the contract.");
        assert!(items[1].owner.is_none() && items[1].due_date.is_none());
        assert!(parse_action_items("## Action Items\nNone noted.\n").is_empty());
    }

    #[test]
    fn code_spans_keep_literal_emphasis_markers() {
        assert_eq!(
            inline_xhtml("Check `**kwargs` and `**options` with **care**."),
            "Check <code>**kwargs</code> and <code>**options</code> with <b>care</b>."
        );
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
    fn strips_transcript_provenance_from_task_title() {
        let md = "\
## Action Items
- Follow up with the vendor (timestamp: ; confidence: low)
- Send notes (timestamp: 12:03; confidence: high)";
        let items = parse_action_items(md);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].task, "Follow up with the vendor");
        assert_eq!(items[1].task, "Send notes");
    }

    #[test]
    fn has_action_items_detects_presence_and_absence() {
        assert!(has_action_items(SAMPLE));
        assert!(!has_action_items("# Summary\n\nNo tasks here, just prose."));
    }
}
