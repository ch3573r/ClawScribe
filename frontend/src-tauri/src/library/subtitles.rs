use std::fmt::Write;

use super::TranscriptPassage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SubtitleFormat {
    Srt,
    Vtt,
}

impl SubtitleFormat {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Srt => "SRT subtitles",
            Self::Vtt => "WebVTT subtitles",
        }
    }
}

fn milliseconds(seconds: f64) -> Option<u64> {
    let rounded = (seconds * 1_000.0).round();
    (seconds >= 0.0 && rounded < u64::MAX as f64).then_some(rounded as u64)
}

fn timing(passage: &TranscriptPassage) -> Option<(u64, u64)> {
    let start = passage.audio_start_time?;
    let end = passage.audio_end_time?;
    if end <= start {
        return None;
    }
    let from = milliseconds(start)?;
    let until = milliseconds(end)?;
    // A valid segment can be shorter than a millisecond. Keep it visible.
    Some((from, until.max(from + 1)))
}

fn write_timestamp(out: &mut String, millis: u64, separator: char) {
    let hours = millis / 3_600_000;
    let minutes = millis / 60_000 % 60;
    let seconds = millis / 1_000 % 60;
    let fraction = millis % 1_000;
    let _ = write!(
        out,
        "{hours:02}:{minutes:02}:{seconds:02}{separator}{fraction:03}"
    );
}

fn clean_text(text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Passages are in saved recording order. Skip ranges without usable audio offsets.
pub(super) fn render(
    passages: &[TranscriptPassage],
    format: SubtitleFormat,
    include_speakers: bool,
) -> Option<String> {
    let mut out = String::new();
    let separator = match format {
        SubtitleFormat::Srt => ',',
        SubtitleFormat::Vtt => {
            out.push_str("WEBVTT\n\n");
            '.'
        }
    };
    let mut number = 0;
    for passage in passages {
        let Some((start, end)) = timing(passage) else {
            continue;
        };
        let mut text = clean_text(&passage.text);
        if text.is_empty() {
            continue;
        }
        if include_speakers {
            let name = clean_text(passage.speaker.as_deref().unwrap_or_default());
            if !name.is_empty() {
                text = format!("{name}: {text}");
            }
        }
        number += 1;
        if format == SubtitleFormat::Srt {
            let _ = writeln!(out, "{number}");
            // Subtitle players can interpret HTML tags and ASS overrides in SRT text.
            text = text.replace('<', "＜").replace("{\\", "｛\\");
        } else {
            text = text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
        }
        write_timestamp(&mut out, start, separator);
        out.push_str(" --> ");
        write_timestamp(&mut out, end, separator);
        let _ = writeln!(out, "\n{text}\n");
    }
    (number > 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_ranges_and_blank_text_do_not_break_cue_numbering() {
        let passage = |start, end, text: &str| TranscriptPassage {
            timestamp: "12:00".into(),
            audio_start_time: Some(start),
            audio_end_time: Some(end),
            speaker: None,
            text: text.into(),
        };
        let rows = [
            passage(-1.0, 1.0, "Negative start"),
            passage(0.0, f64::INFINITY, "Infinite end"),
            passage(0.0, f64::NAN, "Invalid end"),
            passage(0.0, 1.0, " \n\u{0}"),
            passage(1.0, 1.0001, r"<b>Short response</b> {\an8}"),
            passage(2.0, 1.0, "Reversed range"),
            passage(3.0, 4.0, "Next response"),
        ];
        assert_eq!(
            render(&rows, SubtitleFormat::Srt, true).unwrap(),
            "1\n00:00:01,000 --> 00:00:01,001\n＜b>Short response＜/b> ｛\\an8}\n\n2\n00:00:03,000 --> 00:00:04,000\nNext response\n\n"
        );
    }
}
