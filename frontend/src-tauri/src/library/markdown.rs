use super::TranscriptPassage;
use crate::exports::metadata::{escape_inline, export_timestamp, markdown_title, Header};
use crate::summary::sources::strip_source_links;
use std::fmt::Write;

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
    metadata: &Header,
    summary: &str,
    passages: &[TranscriptPassage],
    include_speakers: bool,
    include_timestamps: bool,
) -> String {
    let mut output = markdown_title(title);
    let header = metadata.markdown(include_timestamps);
    if !header.is_empty() {
        output.push('\n');
        output.push_str(&header);
    }

    if !summary.trim().is_empty() {
        let summary = strip_source_links(summary)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        // Preserve leading indentation and Markdown hard breaks in the editor's body.
        let _ = writeln!(output, "\n## Summary\n\n{}", summary.trim_matches('\n'));
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
        let metadata = Header {
            date: "2026-11-22 09:00".into(),
            duration: Some("00:02:05".into()),
            tags: vec!["Planning".into()],
            ..Header::default()
        };
        let summary_only = render("# Review", &metadata, summary, &[], true, true);
        assert_eq!(summary_only, "# \\# Review\n\n- Date: 2026-11-22 09:00\n- Duration: 00:02:05\n- Project tags: Planning\n\n## Summary\n\n### Decisions\n\n- **Keep** the plan\n");
        let transcript_only = render("Review", &metadata, "", &passages, true, true);
        assert!(
            transcript_only.ends_with("## Transcript\n\n**[00:01:05] Änne:** Edited passage.\n")
        );
        assert!(!transcript_only.contains("## Summary"));
        let both = render("Review", &metadata, summary, &passages, false, false);
        assert!(both.contains("## Summary\n\n### Decisions"));
        assert!(both.ends_with("## Transcript\n\nEdited passage.\n"));
        assert_eq!(
            render("", &Header::default(), " \t", &[], false, false),
            "# Meeting\n"
        );
    }

    #[test]
    fn escapes_user_text_and_removes_citation_anchors() {
        let metadata = Header {
            attendees: vec!["Dana [guest]".into()],
            bookmarks: vec![(12.0, "- Read <draft>".into())],
            ..Header::default()
        };
        let passages = [TranscriptPassage {
            timestamp: "00:42".into(),
            audio_start_time: None,
            speaker: Some("*Dana* [guest]".into()),
            text: "First line\r\n# still the passage".into(),
        }];
        let output = render(
            "- Draft [A]\n# Next",
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
