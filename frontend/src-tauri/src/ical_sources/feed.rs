use reqwest::{redirect::Policy, Client, Response, StatusCode, Url};
use std::{
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

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
const MAX_REDIRECTS: usize = 3;

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

// The old private property could also be supplied by an imported calendar.
// Publish selected links through the standard event URL instead.
const UNUSED_TEXT: [&str; 5] = [
    "DESCRIPTION",
    "X-ALT-DESC",
    "ATTACH",
    "COMMENT",
    "X-CLAWSCRIBE-TEAMS-URL",
];

fn teams_link(value: &str) -> Option<([u8; 32], Url)> {
    if value.len() > 4096
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let (scheme, rest) = value.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") || rest.starts_with('/') {
        return None;
    }
    let mut url = Url::parse(value).ok()?;
    let room = crate::teams_detection::calls::teams_room(&url)?;
    url.set_fragment(None);
    (url.as_str().len() <= 4096).then_some((room, url))
}

fn teams_links(description: &str) -> impl Iterator<Item = ([u8; 32], Url)> + '_ {
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)\bhttps?://[^\s<>"'\\]+"#).expect("valid URL pattern")
    });
    URL.find_iter(description).filter_map(|entry| {
        let value = entry.as_str().trim_end_matches(['.', ',', ')']);
        if value.len() > 4096 {
            return None;
        }
        teams_link(&value.replace("&amp;", "&"))
    })
}

fn content_value(line: &str) -> Option<&str> {
    let mut quoted = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(&line[index + 1..]),
            _ => {}
        }
    }
    None
}

pub(super) fn without_unused_text(calendar: &str) -> String {
    let mut kept = String::with_capacity(calendar.len());
    let mut lines = calendar.split_inclusive('\n').peekable();
    let mut event_depth = 0u32;
    let mut extracted: Option<([u8; 32], Url)> = None;
    let mut ambiguous = false;
    let mut explicit: Option<([u8; 32], Url)> = None;
    let mut unsupported_url: Option<String> = None;
    while let Some(line) = lines.next() {
        let text = line.trim_end_matches(['\r', '\n']);
        if event_depth == 0 && text.eq_ignore_ascii_case("BEGIN:VEVENT") {
            event_depth = 1;
            extracted = None;
            ambiguous = false;
            explicit = None;
            unsupported_url = None;
        } else if event_depth == 1 && text.eq_ignore_ascii_case("END:VEVENT") {
            let newline = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
            if let Some((_, url)) = explicit
                .take()
                .or_else(|| extracted.take().filter(|_| !ambiguous))
            {
                kept.push_str("URL;VALUE=URI:");
                kept.push_str(url.as_str());
                kept.push_str(newline);
            } else if let Some(original) = unsupported_url.take() {
                kept.push_str(&original);
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
            if event_depth == 1 && name.eq_ignore_ascii_case("URL") {
                let mut property = text.to_owned();
                let mut original = line.to_owned();
                while let Some(folded) = lines.next_if(|next| next.starts_with([' ', '\t'])) {
                    property.push_str(folded[1..].trim_end_matches(['\r', '\n']));
                    original.push_str(folded);
                }
                if explicit.is_none() {
                    explicit = content_value(&property).and_then(teams_link);
                }
                unsupported_url.get_or_insert(original);
                continue;
            }
            if UNUSED_TEXT
                .iter()
                .any(|unused| name.eq_ignore_ascii_case(unused))
            {
                let extract = event_depth == 1
                    && !ambiguous
                    && (name.eq_ignore_ascii_case("DESCRIPTION")
                        || name.eq_ignore_ascii_case("X-ALT-DESC"));
                let mut description = if extract {
                    text.to_owned()
                } else {
                    String::new()
                };
                while let Some(folded) = lines.next_if(|next| next.starts_with([' ', '\t'])) {
                    if extract {
                        description.push_str(folded[1..].trim_end_matches(['\r', '\n']));
                    }
                }
                if let Some(value) = content_value(&description) {
                    let plain = value.replace("\\n", "\n").replace("\\N", "\n");
                    for (room, url) in teams_links(&plain) {
                        match &extracted {
                            Some((previous, _)) if *previous != room => {
                                ambiguous = true;
                                break;
                            }
                            // Same room can appear in both plain text and HTML.
                            // Keep the first query-bearing form if one exists.
                            Some((_, previous))
                                if previous.query().is_some() || url.query().is_none() => {}
                            _ => extracted = Some((room, url)),
                        }
                    }
                }
                continue;
            }
        }
        kept.push_str(line);
    }
    kept
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first != 0
        && first < 224
        && !(first == 100 && (64..=127).contains(&second))
        && !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
}

fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map_or_else(|| ip.segments()[0] & 0xe000 == 0x2000, public_v4),
    }
}

fn redirect_target(current: &Url, location: &str, redirects: usize) -> Result<Url, &'static str> {
    if redirects >= MAX_REDIRECTS
        || location.len() > 8192
        || location
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(REDIRECTED);
    }
    let target = current.join(location).map_err(|_| REDIRECTED)?;
    if target.scheme() != "https"
        || !target.username().is_empty()
        || target.password().is_some()
        || target.fragment().is_some()
    {
        return Err(REDIRECTED);
    }
    match target.host() {
        Some(url::Host::Domain(host))
            if !host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
                && !host.trim_end_matches('.').ends_with(".localhost") =>
        {
            Ok(target)
        }
        Some(url::Host::Ipv4(ip)) if public_address(ip.into()) => Ok(target),
        Some(url::Host::Ipv6(ip)) if public_address(ip.into()) => Ok(target),
        _ => Err(REDIRECTED),
    }
}

pub(super) fn download(url: Url) -> Result<String, String> {
    tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(15), download_feed(url))
            .await
            .map_err(|_| DOWNLOAD_FAILED)?
    })
}

async fn download_feed(mut target: Url) -> Result<String, String> {
    let builder = || {
        Client::builder()
            .https_only(true)
            .redirect(Policy::none())
            .referer(false)
            .connect_timeout(Duration::from_secs(5))
    };
    let mut client = builder().build().map_err(|_| DOWNLOAD_FAILED)?;
    for redirects in 0..=MAX_REDIRECTS {
        let response = client
            .get(target.clone())
            .header(reqwest::header::ACCEPT, "text/calendar")
            .send()
            .await
            .map_err(|_| DOWNLOAD_FAILED)?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            return read_response(response).await;
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(REDIRECTED)?;
        target = redirect_target(&target, location, redirects)?;

        // Pin the checked addresses so a second DNS lookup or proxy cannot
        // resolve a redirect to a local endpoint. The original subscription
        // remains explicitly configured by the user and can use system proxies.
        let mut next = builder().no_proxy();
        if let Some(url::Host::Domain(host)) = target.host() {
            let port = target.port_or_known_default().ok_or(REDIRECTED)?;
            let addresses = tokio::net::lookup_host((host, port))
                .await
                .map_err(|_| DOWNLOAD_FAILED)?
                .collect::<Vec<_>>();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| !public_address(address.ip()))
            {
                return Err(REDIRECTED.into());
            }
            next = next.resolve_to_addrs(host, &addresses);
        }
        client = next.build().map_err(|_| DOWNLOAD_FAILED)?;
    }
    Err(REDIRECTED.into())
}

fn response_body_allowed(status: StatusCode, length: Option<u64>) -> Result<(), &'static str> {
    if status.is_redirection() {
        return Err(REDIRECTED);
    }
    if !status.is_success() {
        return Err(DOWNLOAD_FAILED);
    }
    if length.is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(TOO_LARGE);
    }
    Ok(())
}

fn append_chunk(bytes: &mut Vec<u8>, chunk: &[u8]) -> Result<(), &'static str> {
    if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
        return Err(TOO_LARGE);
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

async fn read_response(mut response: Response) -> Result<String, String> {
    response_body_allowed(response.status(), response.content_length())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| DOWNLOAD_FAILED)? {
        append_chunk(&mut bytes, &chunk)?;
    }
    calendar_from_bytes(bytes)
}

fn calendar_from_bytes(bytes: Vec<u8>) -> Result<String, String> {
    let calendar = String::from_utf8(bytes).map_err(|_| INVALID_CALENDAR)?;
    validate_calendar(&calendar)?;
    Ok(without_unused_text(&calendar))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_allow_bounded_https_hops_and_reject_local_or_unsafe_targets() {
        let start = subscription_url("https://example.com/calendar.ics").unwrap();
        assert_eq!(
            redirect_target(&start, "/new.ics", 0).unwrap().as_str(),
            "https://example.com/new.ics"
        );
        assert_eq!(
            redirect_target(&start, "https://example.org/feed.ics", 2)
                .unwrap()
                .host_str(),
            Some("example.org")
        );
        assert!(redirect_target(&start, "/new.ics", MAX_REDIRECTS).is_err());
        for target in [
            "http://example.org/feed.ics",
            "https://user@example.org/feed.ics",
            "https://example.org/feed.ics#secret",
            "https://localhost./feed.ics",
            "https://127.1/feed.ics",
            "https://169.254.1.1/feed.ics",
            "https://[::ffff:127.0.0.1]/feed.ics",
            "https://[fc00::1]/feed.ics",
            "https://[fe80::1]/feed.ics",
        ] {
            assert_eq!(redirect_target(&start, target, 0).unwrap_err(), REDIRECTED);
        }
    }

    #[test]
    fn rejects_redirects_and_failed_status_without_disclosing_response() {
        assert_eq!(
            response_body_allowed(StatusCode::TEMPORARY_REDIRECT, None).unwrap_err(),
            REDIRECTED
        );
        assert_eq!(
            response_body_allowed(StatusCode::UNAUTHORIZED, None).unwrap_err(),
            DOWNLOAD_FAILED
        );
    }

    #[test]
    fn streamed_and_advertised_sizes_are_bounded_and_invalid_utf8_is_rejected() {
        let mut bytes = vec![b'x'; MAX_BYTES - 1];
        assert!(append_chunk(&mut bytes, b"x").is_ok());
        assert_eq!(append_chunk(&mut bytes, b"y"), Err(TOO_LARGE));
        assert_eq!(bytes.len(), MAX_BYTES);
        assert_eq!(
            response_body_allowed(StatusCode::OK, Some(MAX_BYTES as u64 + 1)).unwrap_err(),
            TOO_LARGE
        );
        assert_eq!(
            calendar_from_bytes(vec![0xff]).unwrap_err(),
            INVALID_CALENDAR
        );
        let sample = "BEGIN:VCALENDAR\nATTACH:https://example.com/attachment\nEND:VCALENDAR";
        assert_eq!(
            calendar_from_bytes(sample.as_bytes().to_vec()).unwrap(),
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
            " 123456\\nhttps://example.org/other\r\n",
            "X-ALT-DESC;FMTTYPE=\"text/html:synthetic\":<a href=\"https://teams.microsoft.com/meet/123456/?passcode=example#ignored\">Private</a>\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let sanitized = without_unused_text(source);
        assert!(sanitized.contains(
            "URL;VALUE=URI:https://teams.microsoft.com/meet/123456/?passcode=example\r\n"
        ));
        assert!(!sanitized.contains("Private"));
        assert!(!sanitized.contains("example.org"));
        assert_eq!(sanitized, without_unused_text(&sanitized));
        let ambiguous = source.replace("Private agenda", "https://teams.live.com/meet/456");
        assert!(!without_unused_text(&ambiguous).contains("URL;VALUE=URI"));
    }

    #[test]
    fn native_sanitizer_strips_a_supplied_private_marker_and_prefers_the_explicit_teams_url() {
        let source = concat!(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\n",
            "x-clawscribe-teams-url;VALUE=URI:https://teams.microsoft.com/meet/23\n",
            " 45\n",
            "URL:https://teams.cloud.microsoft/meet/123#fragment\n",
            "DESCRIPTION:https://teams.microsoft.com/meet/456\n",
            "END:VEVENT\nEND:VCALENDAR\n",
        );
        assert_eq!(
            without_unused_text(source),
            concat!(
                "BEGIN:VCALENDAR\nBEGIN:VEVENT\n",
                "URL;VALUE=URI:https://teams.cloud.microsoft/meet/123\n",
                "END:VEVENT\nEND:VCALENDAR\n",
            )
        );
    }
}
