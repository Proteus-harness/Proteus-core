use std::net::IpAddr;

use url::{Host, Url};

pub mod pending;
pub mod response;
pub mod session_selection;
pub mod sync;

/// Parses and canonicalizes the local HTTP(S) origin accepted by the
/// client. Paths, queries, fragments, credentials and non-loopback hosts are
/// rejected so an app-server or sibling-client endpoint cannot silently turn
/// into an arbitrary credential recipient.
pub fn normalize_local_origin(value: &str) -> Result<String, String> {
    let url = Url::parse(value.trim()).map_err(|error| format!("invalid origin: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("origin scheme must be http or https".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("origin must not contain user info".to_owned());
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err("origin must not contain a path, query, or fragment".to_owned());
    }
    let is_loopback = match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    };
    if !is_loopback {
        return Err("origin host must be localhost or a loopback IP address".to_owned());
    }

    Ok(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_origin_parser_rejects_non_origin_and_remote_urls() {
        assert_eq!(
            normalize_local_origin(" http://localhost:8787/ ").unwrap(),
            "http://localhost:8787"
        );
        assert_eq!(
            normalize_local_origin("https://[::1]:9443").unwrap(),
            "https://[::1]:9443"
        );

        for invalid in [
            "javascript:alert(1)",
            "http://example.com:8787",
            "http://127.0.0.1:8787/api",
            "http://user:secret@127.0.0.1:8787",
            "http://127.0.0.1:8787/?token=secret",
        ] {
            assert!(
                normalize_local_origin(invalid).is_err(),
                "unexpected valid origin: {invalid}"
            );
        }
    }
}
