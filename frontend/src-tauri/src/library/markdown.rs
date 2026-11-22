use super::{export_timestamp, TranscriptPassage};
use crate::summary::sources::strip_source_links;
use std::fmt::Write;

#[derive(Default)]
pub(super) struct Metadata {
    pub duration: Option<f64>,
    pub attendees: Vec<String>,
    pub tags: Vec<String>,
    pub bookmarks: Vec<(f64, String)>,
}

fn escape_inline(value: &str) -> String {
    let value = strip_source_links(value);
    let mut escaped = String::new();
    // Inline fields stay on one line, even if pasted text contains line breaks.
    let line = value.split_whitespace().collect::<Vec<_>>().join(" ");
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

fn passage_time(passage: &TranscriptPassage) -> String {
    let time = export_timestamp(&passage.timestamp, passage.audio_start_time);
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(&time) {
        return date.format("%H:%M:%S").to_string();
    }
    if let Some((minutes, seconds)) = time.split_once(':') {
        if let (Ok(minutes), Ok(seconds)) = (minutes.parse::<u64>(), seconds.parse::<u64>()) {
            let total = minutes.saturating_mul(60).saturating_add(seconds);
            return export_timestamp("", Some(total as f64));
        }
    }
    time
}

pub(super) fn render(
    title: &str,
    date: &str,
    metadata: &Metadata,
    summary: &str,
    passages: &[TranscriptPassage],
    include_speakers: bool,
    include_timestamps: bool,
) -> String {
    let title = if title.trim().is_empty() {
        "Meeting"
    } else {
        title
    };
    let mut output = format!("# {}\n", escape_inline(title));
    let mut details = String::new();
    if !date.trim().is_empty() {
        let _ = writeln!(details, "- Date: {}", escape_inline(date));
    }
    if let Some(duration) = metadata
        .duration
        .filter(|time| time.is_finite() && *time >= 0.0)
    {
        let _ = writeln!(
            details,
            "- Duration: {}",
            export_timestamp("", Some(duration))
        );
    }
    for (label, values) in [
        ("Attendees", &metadata.attendees),
        ("Project tags", &metadata.tags),
    ] {
        if !values.is_empty() {
            let values = values
                .iter()
                .map(|value| escape_inline(value))
                .collect::<Vec<_>>();
            let _ = writeln!(details, "- {label}: {}", values.join(", "));
        }
    }
    if !details.is_empty() {
        output.push('\n');
        output.push_str(&details);
    }

    if !summary.trim().is_empty() {
        let summary = strip_source_links(summary)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        // Preserve leading indentation and Markdown hard breaks in the editor's body.
        let _ = writeln!(output, "\n## Summary\n\n{}", summary.trim_matches('\n'));
    }
    if !metadata.bookmarks.is_empty() {
        output.push_str("\n## Notes and bookmarks\n\n");
        for (seconds, label) in &metadata.bookmarks {
            let label = escape_inline(label);
            if include_timestamps {
                let _ = writeln!(
                    output,
                    "- **[{}]** {label}",
                    export_timestamp("", Some(*seconds))
                );
            } else {
                let _ = writeln!(output, "- {label}");
            }
        }
    }

    let mut passages = passages
        .iter()
        .filter(|passage| !passage.text.trim().is_empty())
        .peekable();
    if passages.peek().is_some() {
        output.push_str("\n## Transcript\n");
    }
    for passage in passages {
        let mut label = String::new();
        if include_timestamps {
            let time = passage_time(passage);
            if !time.is_empty() {
                let _ = write!(label, "[{}] ", escape_inline(&time));
            }
        }
        if let Some(speaker) = passage
            .speaker
            .as_deref()
            .filter(|speaker| include_speakers && !speaker.trim().is_empty())
        {
            let _ = write!(label, "{}:", escape_inline(speaker));
        }
        output.push('\n');
        if !label.is_empty() {
            let _ = write!(output, "**{}** ", label.trim_end());
        }
        let _ = writeln!(output, "{}", escape_inline(&passage.text));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_only_selected_content_and_omits_empty_sections() {
        let summary = "### Decisions\r\n\r\n- **Keep** the plan";
        let passages = [TranscriptPassage {
            timestamp: "09:00:00".into(),
            audio_start_time: Some(65.9),
            speaker: Some("Änne".into()),
            text: "Edited passage.".into(),
        }];
        let metadata = Metadata {
            duration: Some(125.0),
            tags: vec!["Planning".into()],
            ..Metadata::default()
        };
        let summary_only = render(
            "# Review",
            "2026-11-22 09:00",
            &metadata,
            summary,
            &[],
            true,
            true,
        );
        assert_eq!(summary_only, "# \\# Review\n\n- Date: 2026-11-22 09:00\n- Duration: 00:02:05\n- Project tags: Planning\n\n## Summary\n\n### Decisions\n\n- **Keep** the plan\n");
        let transcript_only = render("Review", "", &metadata, "", &passages, true, true);
        assert!(
            transcript_only.ends_with("## Transcript\n\n**[00:01:05] Änne:** Edited passage.\n")
        );
        assert!(!transcript_only.contains("## Summary"));
        let both = render("Review", "", &metadata, summary, &passages, false, false);
        assert!(both.contains("## Summary\n\n### Decisions"));
        assert!(both.ends_with("## Transcript\n\nEdited passage.\n"));
        assert_eq!(
            render("", "", &Metadata::default(), " \t", &[], false, false),
            "# Meeting\n"
        );
    }

    #[test]
    fn escapes_user_text_and_removes_citation_anchors() {
        let metadata = Metadata {
            attendees: vec!["Dana [guest]".into()],
            bookmarks: vec![(12.0, "- Read <draft>".into())],
            ..Metadata::default()
        };
        let passages = [TranscriptPassage {
            timestamp: "00:42".into(),
            audio_start_time: None,
            speaker: Some("*Dana* [guest]".into()),
            text: "First line\r\n# still the passage".into(),
        }];
        let output = render(
            "- Draft [A]\n# Next",
            "",
            &metadata,
            "See [00:00:12](#clawscribe-source-abc123).",
            &passages,
            true,
            true,
        );
        assert!(output.starts_with("# \\- Draft \\[A\\] # Next\n"));
        assert!(output.contains("- Attendees: Dana \\[guest\\]"));
        assert_eq!(
            escape_inline("Meeting 22_11_26 _draft_ a_"),
            "Meeting 22_11_26 \\_draft\\_ a\\_"
        );
        assert!(output.contains("See (00:00:12)."));
        assert!(!output.contains("clawscribe-source"));
        assert!(output.contains("## Notes and bookmarks\n\n- **[00:00:12]** \\- Read \\<draft>"));
        assert!(output
            .contains("**[00:00:42] \\*Dana\\* \\[guest\\]:** First line # still the passage"));
    }
}
