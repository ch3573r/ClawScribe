//! Shared transport policy for operator-configured credential destinations.
use std::net::{IpAddr, SocketAddr};
use url::{Host, Url};

fn local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || (a == 100 && (64..=127).contains(&b))
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || (ip.segments()[0] & 0xfe00 == 0xfc00)
                || (ip.segments()[0] & 0xffc0 == 0xfe80)
        }
    }
}

pub(crate) fn needs_http_opt_in(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(Host::Ipv4(ip)) => local_ip(ip.into()) && !ip.is_loopback(),
        Some(Host::Ipv6(ip)) => local_ip(ip.into()) && !ip.is_loopback(),
        Some(Host::Domain(host)) => {
            host != "localhost"
                && (!host.contains('.')
                    || [".local", ".lan", ".internal", ".home.arpa", ".ts.net"]
                        .iter()
                        .any(|suffix| host.ends_with(suffix)))
        }
        None => false,
    }
}

pub(crate) fn validate_secret_destination(
    value: &str,
    allow_unencrypted: bool,
) -> Result<(), String> {
    let url = Url::parse(value).map_err(|_| "Enter a valid endpoint URL")?;
    if url.host().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err("Use an endpoint with a host and without credentials in its URL".into());
    }
    let loopback = match url.host() {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        "http" if needs_http_opt_in(value) && allow_unencrypted => Ok(()),
        "http" if needs_http_opt_in(value) => Err("Enable ‘Allow unencrypted HTTP to this local-network server’ or use HTTPS. The token/key would be sent without encryption.".into()),
        "http" => Err("Public HTTP endpoints are not allowed. Change the endpoint to HTTPS.".into()),
        _ => Err("The endpoint must use HTTP or HTTPS".into()),
    }
}

/// Only absent flags migrate. Explicit false remains false on subsequent loads.
/// Loading must remain possible so users can repair rejected destinations.
/// Validate the destination when saving or sending, never during migration.
pub(crate) fn migrate_http_opt_in(
    config: &mut serde_json::Value,
    endpoints: &[&str],
) -> Result<bool, String> {
    let allow = config
        .get("allow_unencrypted")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| {
            endpoints
                .iter()
                .any(|key| config[*key].as_str().is_some_and(needs_http_opt_in))
        });
    let missing = config.get("allow_unencrypted").is_none();
    if missing {
        config["allow_unencrypted"] = allow.into();
    }
    Ok(missing)
}

fn validate_addresses(addresses: &[SocketAddr]) -> Result<(), String> {
    if addresses.is_empty() || addresses.iter().any(|addr| !local_ip(addr.ip())) {
        return Err(
            "HTTP destination did not resolve exclusively to local-network addresses. Use HTTPS."
                .into(),
        );
    }
    Ok(())
}

/// Resolve immediately before sending, pin that resolution, and prohibit redirects
/// and proxies for plaintext requests so a second resolution cannot bypass policy.
pub(crate) async fn secret_client(
    mut builder: reqwest::ClientBuilder,
    value: &str,
    allow_unencrypted: bool,
) -> Result<reqwest::Client, String> {
    validate_secret_destination(value, allow_unencrypted)?;
    let url = Url::parse(value).map_err(|_| "Invalid endpoint")?;
    builder = builder.redirect(reqwest::redirect::Policy::none());
    if url.scheme() == "http" {
        let host = url
            .host_str()
            .ok_or("Missing endpoint host")?
            .trim_matches(['[', ']']);
        let addresses: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::net::lookup_host((
                host,
                url.port_or_known_default().ok_or("Missing endpoint port")?,
            )),
        )
        .await
        .map_err(|_| "Endpoint lookup timed out")?
        .map_err(|_| "Could not resolve the local-network endpoint")?
        .collect();
        validate_addresses(&addresses)?;
        builder = builder.no_proxy().resolve_to_addrs(host, &addresses);
    }
    builder
        .build()
        .map_err(|_| "Could not create integration HTTP client".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destination_policy_and_legacy_migration() {
        assert!(validate_secret_destination("http://public.example.com", true).is_err());
        assert!(validate_secret_destination("http://openclaw.local", false).is_err());
        assert!(validate_secret_destination("http://openclaw.local", true).is_ok());
        for url in ["http://localhost", "http://127.0.0.1", "http://[::1]"] {
            assert!(validate_secret_destination(url, false).is_ok());
        }
        let mut old = serde_json::json!({"endpoint":"http://openclaw.local"});
        assert!(migrate_http_opt_in(&mut old, &["endpoint"]).unwrap());
        assert_eq!(old["allow_unencrypted"], true);
        old["allow_unencrypted"] = false.into();
        assert!(!migrate_http_opt_in(&mut old, &["endpoint"]).unwrap());
        assert_eq!(old["allow_unencrypted"], false);
        assert!(migrate_http_opt_in(
            &mut serde_json::json!({"endpoint":"http://public.example.com"}),
            &["endpoint"]
        )
        .unwrap());
    }
    #[test]
    fn tailscale_policy_checks_range_boundaries_and_dns() {
        for url in [
            "http://100.64.0.0",
            "http://100.127.255.255",
            "http://host.example.ts.net",
        ] {
            assert!(needs_http_opt_in(url));
            assert!(validate_secret_destination(url, false).is_err());
            assert!(validate_secret_destination(url, true).is_ok());
            let mut old = serde_json::json!({"endpoint": url});
            migrate_http_opt_in(&mut old, &["endpoint"]).unwrap();
            assert_eq!(old["allow_unencrypted"], true);
        }
        for url in [
            "http://100.63.255.255",
            "http://100.128.0.0",
            "http://ts.net",
            "http://host.ts.net.example.com",
        ] {
            assert!(validate_secret_destination(url, true).is_err());
        }
        assert!(validate_addresses(&["100.64.0.1:80".parse().unwrap()]).is_ok());
        assert!(validate_addresses(&[
            "100.64.0.1:80".parse().unwrap(),
            "203.0.113.1:80".parse().unwrap()
        ])
        .is_err());
    }
    #[test]
    fn private_name_cannot_resolve_to_public_address() {
        assert!(validate_addresses(&[
            "127.0.0.1:80".parse().unwrap(),
            "203.0.113.1:80".parse().unwrap()
        ])
        .is_err());
        assert!(validate_addresses(&[]).is_err());
        assert!(validate_addresses(&[
            "127.0.0.1:80".parse().unwrap(),
            "[::1]:80".parse().unwrap()
        ])
        .is_ok());
    }
}
