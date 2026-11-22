use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::{sync::watch, time::Instant};
use url::Url;

use super::payload::Event;
use crate::openai::secret_destination::{
    build_secret_client, validate_secret_destination, SecretClientError,
};

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(9);
const MAX_ATTEMPTS: u8 = 3;

pub(super) fn destination(value: &str, allow_unencrypted: bool) -> Result<Url, String> {
    validate_secret_destination(value, allow_unencrypted)?;
    let url = Url::parse(value).map_err(|_| "Enter a valid endpoint URL")?;
    // The shared credential policy permits loopback HTTP. Sending meeting
    // content requires the opt-in even when the receiver is on this PC.
    if url.scheme() == "http" && !allow_unencrypted {
        return Err("Allow unencrypted HTTP for this destination or use HTTPS.".into());
    }
    Ok(url)
}

pub(super) fn signature(secret: &[u8], timestamp: &str, body: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let mut header = String::from("sha256=");
    for byte in mac.finalize().into_bytes() {
        let _ = write!(header, "{byte:02x}");
    }
    header
}

pub(super) struct Request {
    pub url: Url,
    pub allow_unencrypted: bool,
    pub event: Event,
    pub id: String,
    pub timestamp: String,
    pub signature: Option<String>,
    pub body: Bytes,
}

// Only these closed values, never underlying HTTP errors or response text,
// can reach status events, errors, logs or the delivery history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    CredentialsUnavailable,
    Destination,
    MeetingUnavailable,
    PayloadTooLarge,
    Network,
    Timeout,
    Redirect,
    HttpStatus,
    SettingsChanged,
}

impl Failure {
    pub(super) fn class(self) -> &'static str {
        match self {
            Self::CredentialsUnavailable => "credentials_unavailable",
            Self::Destination => "destination",
            Self::MeetingUnavailable => "meeting_unavailable",
            Self::PayloadTooLarge => "payload_too_large",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Redirect => "redirect",
            Self::HttpStatus => "http_status",
            Self::SettingsChanged => "settings_changed",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Outcome {
    pub attempts: u8,
    pub status: Option<u16>,
    pub failure: Option<Failure>,
}

impl Outcome {
    pub(super) fn rejected(failure: Failure) -> Self {
        Self {
            attempts: 0,
            status: None,
            failure: Some(failure),
        }
    }
}

#[async_trait]
pub(super) trait Transport: Sync {
    async fn post(&self, request: &Request) -> Result<u16, Failure>;
}

pub(super) struct Http;

#[async_trait]
impl Transport for Http {
    async fn post(&self, request: &Request) -> Result<u16, Failure> {
        // Recheck and pin plaintext DNS for every attempt. The shared client
        // also disables redirects for HTTPS.
        if request.url.scheme() == "http" && !request.allow_unencrypted {
            return Err(Failure::Destination);
        }
        let client = build_secret_client(
            reqwest::Client::builder().timeout(ATTEMPT_TIMEOUT),
            request.url.as_str(),
            request.allow_unencrypted,
        )
        .await
        .map_err(|error| match error {
            SecretClientError::LookupTimeout => Failure::Timeout,
            SecretClientError::LookupFailed => Failure::Network,
            _ => Failure::Destination,
        })?;
        let mut post = client
            .post(request.url.clone())
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                concat!("ClawScribe/", env!("CARGO_PKG_VERSION")),
            )
            .header("X-ClawScribe-Event", request.event.name())
            .header("X-ClawScribe-Delivery", &request.id)
            .body(request.body.clone());
        if let Some(signature) = &request.signature {
            post = post
                .header("X-ClawScribe-Timestamp", &request.timestamp)
                .header("X-ClawScribe-Signature", signature);
        }
        // Drop the response immediately after reading its status. No body
        // bytes are consumed or exposed, even on errors or redirects.
        post.send()
            .await
            .map(|response| response.status().as_u16())
            .map_err(|error| {
                if error.is_timeout() {
                    Failure::Timeout
                } else {
                    Failure::Network
                }
            })
    }
}

pub(super) async fn deliver(
    transport: &impl Transport,
    request: &Request,
    settings_changed: &mut watch::Receiver<()>,
) -> Outcome {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    let mut outcome = Outcome::rejected(Failure::SettingsChanged);
    for attempt in 1..=MAX_ATTEMPTS {
        if settings_changed.has_changed().unwrap_or(true) {
            outcome.failure = Some(Failure::SettingsChanged);
            break;
        }
        let attempt_deadline = (Instant::now() + ATTEMPT_TIMEOUT).min(deadline);
        outcome.attempts = attempt;
        let response = tokio::select! {
            biased;
            _ = settings_changed.changed() => Err(Failure::SettingsChanged),
            result = tokio::time::timeout_at(attempt_deadline, transport.post(request)) => {
                result.unwrap_or(Err(Failure::Timeout))
            }
        };
        let (status, failure, retry) = match response {
            Ok(status @ 200..=299) => (Some(status), None, false),
            Ok(status @ 300..=399) => (Some(status), Some(Failure::Redirect), false),
            Ok(status) => (
                Some(status),
                Some(Failure::HttpStatus),
                matches!(status, 408 | 429 | 500..=599),
            ),
            Err(failure) => (
                None,
                Some(failure),
                matches!(failure, Failure::Network | Failure::Timeout),
            ),
        };
        outcome.status = status;
        outcome.failure = failure;
        if !retry || attempt == MAX_ATTEMPTS {
            break;
        }
        let next_attempt = Instant::now() + Duration::from_millis(250 * u64::from(attempt));
        if next_attempt >= deadline {
            break;
        }
        tokio::select! {
            biased;
            _ = settings_changed.changed() => {
                outcome.failure = Some(Failure::SettingsChanged);
                break;
            }
            _ = tokio::time::sleep_until(next_attempt) => {}
        }
    }
    outcome
}
