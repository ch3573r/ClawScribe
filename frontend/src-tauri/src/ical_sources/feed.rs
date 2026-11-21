use reqwest::{blocking::Client, redirect::Policy, Url};
use std::{io::Read, time::Duration};

// A published work calendar with full details exceeds 2 MiB within a year,
// mostly through HTML meeting descriptions.
pub(super) const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const INVALID_URL: &str =
    "Use an HTTPS or webcal calendar URL without a username, password, or fragment.";
pub(super) const INVALID_CALENDAR: &str = "Choose an iCalendar file or feed in UTF-8 format.";
pub(super) const TOO_LARGE: &str = "Calendar files and feeds must be 16 MiB or smaller.";
const DOWNLOAD_FAILED: &str =
    "Could not download the calendar. Check the subscription link and try again.";
const REDIRECTED: &str =
    "The calendar redirected to another address. Use the final HTTPS subscription URL.";

pub(super) fn subscription_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    let (scheme, rest) = input.split_once("://").ok_or(INVALID_URL)?;
    if input.len() > 8192
        || input
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        || !(scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("webcal"))
        || rest.starts_with('/')
        || rest.is_empty()
    {
        return Err(INVALID_URL.into());
    }
    let url = Url::parse(&format!("https://{rest}")).map_err(|_| INVALID_URL)?;
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(INVALID_URL.into());
    }
    Ok(url)
}

pub(super) fn validate_calendar(content: &str) -> Result<(), String> {
    if content.len() > MAX_BYTES {
        return Err(TOO_LARGE.into());
    }
    let mut lines = content.trim_start_matches('\u{feff}').trim().lines();
    if !lines
        .next()
        .is_some_and(|s| s.eq_ignore_ascii_case("BEGIN:VCALENDAR"))
        || !lines
            .next_back()
            .is_some_and(|s| s.eq_ignore_ascii_case("END:VCALENDAR"))
        || content.contains('\0')
    {
        return Err(INVALID_CALENDAR.into());
    }
    Ok(())
}

// Strip free-form text before storage or IPC. Extract a single eligible Teams
// link from event descriptions without keeping the surrounding content.
const UNUSED_TEXT: [&str; 4] = ["DESCRIPTION", "X-ALT-DESC", "ATTACH", "COMMENT"];
const EXTRACTED_TEAMS_URL: &str = "X-CLAWSCRIBE-TEAMS-URL;VALUE=URI:";

fn teams_links(description: &str) -> impl Iterator<Item = Url> + '_ {
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)\bhttps?://[^\s<>"'\\]+"#).expect("valid URL pattern")
    });
    URL.find_iter(description).filter_map(|entry| {
        let value = entry.as_str().trim_end_matches(['.', ',', ')']);
        if value.len() > 4096 {
            return None;
        }
        let mut url = Url::parse(&value.replace("&amp;", "&")).ok()?;
        crate::teams_detection::calls::teams_room(&url)?;
        url.set_fragment(None);
        Some(url)
    })
}

pub(super) fn without_unused_text(calendar: &str) -> String {
    let mut kept = String::with_capacity(calendar.len());
    let mut lines = calendar.split_inclusive('\n').peekable();
    let mut event_depth = 0u32;
    let mut extracted = None;
    let mut ambiguous = false;
    while let Some(line) = lines.next() {
        let text = line.trim_end_matches(['\r', '\n']);
        if text.eq_ignore_ascii_case("BEGIN:VEVENT") {
            event_depth = 1;
            extracted = None;
            ambiguous = false;
        } else if event_depth == 1 && text.eq_ignore_ascii_case("END:VEVENT") {
            if let Some(url) = extracted.take().filter(|_| !ambiguous) {
                kept.push_str(EXTRACTED_TEAMS_URL);
                kept.push_str(url::Url::as_str(&url));
                kept.push_str(if line.ends_with("\r\n") { "\r\n" } else { "\n" });
            }
            event_depth = 0;
        } else if event_depth > 0 {
            let boundary = text.split_once(':').map(|(name, _)| name);
            if boundary.is_some_and(|name| name.eq_ignore_ascii_case("BEGIN")) {
                event_depth = event_depth.saturating_add(1);
            } else if boundary.is_some_and(|name| name.eq_ignore_ascii_case("END")) {
                event_depth -= 1;
            }
        }
        if !line.starts_with([' ', '\t']) {
            let name = line.split([';', ':']).next().unwrap_or_default().trim_end();
            if UNUSED_TEXT
                .iter()
                .any(|unused| name.eq_ignore_ascii_case(unused))
            {
                let extract = event_depth == 1
                    && !ambiguous
                    && (name.eq_ignore_ascii_case("DESCRIPTION")
                        || name.eq_ignore_ascii_case("X-ALT-DESC"));
                let mut description = if extract {
                    text.split_once(':')
                        .map_or(String::new(), |(_, value)| value.to_owned())
                } else {
                    String::new()
                };
                while let Some(folded) = lines.next_if(|next| next.starts_with([' ', '\t'])) {
                    if extract {
                        description.push_str(folded[1..].trim_end_matches(['\r', '\n']));
                    }
                }
                if extract {
                    let plain = description.replace("\\n", "\n").replace("\\N", "\n");
                    for url in teams_links(&plain) {
                        if extracted.as_ref().is_some_and(|previous| previous != &url) {
                            ambiguous = true;
                            break;
                        }
                        extracted = Some(url);
                    }
                }
                continue;
            }
        }
        kept.push_str(line);
    }
    kept
}

// Only the explicitly subscribed URL is requested. Neither redirects nor
// URIs embedded in calendar properties trigger follow-up requests.
pub(super) fn download(url: Url) -> Result<String, String> {
    let client = Client::builder()
        .https_only(true)
        .redirect(Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| DOWNLOAD_FAILED)?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/calendar")
        .timeout(Duration::from_secs(15))
        .send()
        .map_err(|_| DOWNLOAD_FAILED)?;
    read_response(response.status(), response.content_length(), response)
}

fn read_response(
    status: reqwest::StatusCode,
    length: Option<u64>,
    body: impl Read,
) -> Result<String, String> {
    if status.is_redirection() {
        return Err(REDIRECTED.into());
    }
    if !status.is_success() {
        return Err(DOWNLOAD_FAILED.into());
    }
    if length.is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(TOO_LARGE.into());
    }
    let mut bytes = Vec::new();
    body.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DOWNLOAD_FAILED)?;
    if bytes.len() > MAX_BYTES {
        return Err(TOO_LARGE.into());
    }
    let calendar = String::from_utf8(bytes).map_err(|_| INVALID_CALENDAR)?;
    validate_calendar(&calendar)?;
    Ok(without_unused_text(&calendar))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn rejects_redirects_and_failed_status_without_disclosing_response() {
        let body = b"https://example.com/private-calendar".as_slice();
        assert_eq!(
            read_response(StatusCode::TEMPORARY_REDIRECT, None, body).unwrap_err(),
            REDIRECTED
        );
        assert_eq!(
            read_response(StatusCode::UNAUTHORIZED, None, body).unwrap_err(),
            DOWNLOAD_FAILED
        );
    }

    #[test]
    fn streamed_and_advertised_sizes_are_bounded_and_invalid_utf8_is_rejected() {
        let bytes = vec![b'x'; MAX_BYTES + 1];
        assert_eq!(
            read_response(StatusCode::OK, None, bytes.as_slice()).unwrap_err(),
            TOO_LARGE
        );
        assert_eq!(
            read_response(StatusCode::OK, Some(MAX_BYTES as u64 + 1), &b""[..]).unwrap_err(),
            TOO_LARGE
        );
        assert_eq!(
            read_response(StatusCode::OK, None, &[0xff][..]).unwrap_err(),
            INVALID_CALENDAR
        );
        let sample = "BEGIN:VCALENDAR\nATTACH:https://example.com/attachment\nEND:VCALENDAR";
        assert_eq!(
            read_response(StatusCode::OK, None, sample.as_bytes()).unwrap(),
            "BEGIN:VCALENDAR\nEND:VCALENDAR"
        );
    }

    #[test]
    fn unused_text_is_dropped_with_its_folded_lines_and_nothing_else() {
        let calendar = concat!(
            "BEGIN:VCALENDAR\r\n",
            "BEGIN:VEVENT\r\n",
            "DESCRIPTION;LANGUAGE=en:First line\r\n",
            " folded line\r\n",
            "\tSUMMARY:still part of the description\r\n",
            "SUMMARY:Planning\r\n",
            " review\r\n",
            "x-alt-desc;FMTTYPE=text/html:<p>Body</p>\r\n",
            "DESCRIPTION-NOTE:kept\r\n",
            "comment:dropped\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );
        assert_eq!(
            without_unused_text(calendar),
            concat!(
                "BEGIN:VCALENDAR\r\n",
                "BEGIN:VEVENT\r\n",
                "SUMMARY:Planning\r\n",
                " review\r\n",
                "DESCRIPTION-NOTE:kept\r\n",
                "END:VEVENT\r\n",
                "END:VCALENDAR\r\n",
            )
        );
        let untouched = "\u{feff}BEGIN:VCALENDAR\nSUMMARY:Description: none\nEND:VCALENDAR";
        assert_eq!(without_unused_text(untouched), untouched);
    }

    #[test]
    fn only_one_eligible_teams_link_survives_a_folded_description() {
        let source = concat!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n",
            "DESCRIPTION:Private agenda\\nhttps://teams.microsoft.com/meet/\r\n",
            " 123456?passcode=example\\nhttps://example.org/other\r\n",
            "X-ALT-DESC:<a href=\"https://teams.microsoft.com/meet/123456?passcode=example\">Private</a>\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let sanitized = without_unused_text(source);
        assert!(sanitized.contains("X-CLAWSCRIBE-TEAMS-URL;VALUE=URI:https://teams.microsoft.com/meet/123456?passcode=example"));
        assert!(!sanitized.contains("Private"));
        assert!(!sanitized.contains("example.org"));
        assert_eq!(sanitized, without_unused_text(&sanitized));
        let ambiguous = source.replace("Private agenda", "https://teams.live.com/meet/456");
        assert!(!without_unused_text(&ambiguous).contains("X-CLAWSCRIBE-TEAMS-URL"));
    }
}
