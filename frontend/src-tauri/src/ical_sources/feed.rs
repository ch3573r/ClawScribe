use reqwest::{blocking::Client, redirect::Policy, Url};
use std::{io::Read, time::Duration};

pub(super) const MAX_BYTES: usize = 2 * 1024 * 1024;
pub(super) const INVALID_URL: &str =
    "Use an HTTPS or webcal calendar URL without a username, password, or fragment.";
pub(super) const INVALID_CALENDAR: &str = "Choose an iCalendar file or feed in UTF-8 format.";
pub(super) const TOO_LARGE: &str = "Calendar files and feeds must be 2 MiB or smaller.";
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
    Ok(calendar)
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
            sample
        );
    }
}
