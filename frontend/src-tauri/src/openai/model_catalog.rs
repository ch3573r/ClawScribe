//! Credential-scoped discovery for the fixed hosted-provider catalog endpoints.
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;

fn status_error(status: StatusCode) -> Option<&'static str> {
    match status {
        StatusCode::UNAUTHORIZED => Some("invalid_api_key"),
        StatusCode::FORBIDDEN => Some("catalog_forbidden"),
        status if status.is_success() => None,
        _ => Some("catalog_unavailable"),
    }
}

pub(crate) fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "catalog_unavailable".into())
}

pub(crate) async fn read<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, String> {
    let response = request.send().await.map_err(|_| "catalog_unavailable")?;
    if let Some(error) = status_error(response.status()) {
        return Err(error.into());
    }
    response
        .json()
        .await
        .map_err(|_| "catalog_unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_denied_catalog_is_distinct_from_a_rejected_key_or_an_outage() {
        assert_eq!(
            status_error(StatusCode::UNAUTHORIZED),
            Some("invalid_api_key")
        );
        assert_eq!(
            status_error(StatusCode::FORBIDDEN),
            Some("catalog_forbidden")
        );
        assert_eq!(
            status_error(StatusCode::TOO_MANY_REQUESTS),
            Some("catalog_unavailable")
        );
        assert_eq!(status_error(StatusCode::OK), None);
    }
}
