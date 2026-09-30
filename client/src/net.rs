//! URL parsing and validation shared by the provider adapters.
//!
//! Every provider adapter in this client sends a credential in a header, so
//! every one of them needs the same rule: **plaintext `http://` is allowed only
//! for loopback**. A user who points the client at a remote `http://` endpoint
//! would send their API key across the network in clear text, and would have no
//! way to notice.
//!
//! This lived in the OpenAI adapter until a second adapter needed it. Sharing it
//! is not merely tidiness: a security rule implemented twice is a rule that will
//! eventually be implemented differently in the two places.

/// Why a URL was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlRejection {
    /// The offending URL.
    pub url: String,
    /// What is wrong with it, phrased for a user.
    pub reason: String,
}

/// Validate an endpoint that will receive a credential.
///
/// Returns the reason when the URL must not be used.
pub fn validate_secure_url(base_url: &str) -> Result<(), UrlRejection> {
    let trimmed = base_url.trim();
    let reject = |reason: &str| UrlRejection {
        url: trimmed.to_string(),
        reason: reason.to_string(),
    };

    let (scheme, _rest) = trimmed
        .split_once("://")
        .ok_or_else(|| reject("The URL needs a scheme, for example https://."))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(reject("Only http:// and https:// endpoints are supported."));
    }

    let host = host_of(trimmed);
    if host.is_empty() {
        return Err(reject("The URL has no host."));
    }

    if scheme == "http" && !is_loopback_host(host) {
        return Err(reject(
            "Plaintext http:// is only allowed for localhost, because a remote endpoint would \
             send your API key in clear text.",
        ));
    }

    Ok(())
}

/// Extract the host from a URL, ignoring scheme, port, and path.
///
/// Strips any userinfo component, so `https://user:pass@example.com` reports
/// `example.com` rather than leaking the embedded credentials into a display
/// string.
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // IPv6 literals are bracketed.
    if let Some(inner) = authority.strip_prefix('[') {
        if let Some(close) = inner.find(']') {
            return &inner[..close];
        }
    }
    authority.split(':').next().unwrap_or(authority)
}

/// Whether a host is a loopback address.
///
/// Deliberately not a prefix check on `127.`: `127.0.0.1.evil.com` resolves to a
/// remote host and would pass one.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host == "::1" || host == "0.0.0.0" || host.ends_with(".localhost") {
        return true;
    }
    if host == "127.0.0.1" {
        return true;
    }
    match host.strip_prefix("127.") {
        Some(rest) => {
            // Any 127.0.0.0/8 address is loopback, but only if every octet is a
            // number — which is what rejects 127.0.0.1.evil.com.
            rest.split('.').all(|part| part.parse::<u8>().is_ok())
        }
        None => false,
    }
}

/// Join a base URL and a path, tolerating a trailing slash on either.
pub fn join_url(base: &str, path: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_accepted() {
        assert!(validate_secure_url("https://api.deepgram.com/v1").is_ok());
        assert!(validate_secure_url("https://api.inference.net/v1").is_ok());
    }

    #[test]
    fn plaintext_is_accepted_only_for_loopback() {
        // Local model runners speak http on loopback; refusing them would make
        // offline use impossible.
        for url in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://127.0.0.1:8080",
            "http://[::1]:11434/v1",
        ] {
            assert!(validate_secure_url(url).is_ok(), "{url} should be allowed");
        }

        let rejection = validate_secure_url("http://api.deepgram.com/v1").unwrap_err();
        assert!(rejection.reason.contains("clear text"), "{rejection:?}");
        assert!(
            rejection.reason.ends_with('.'),
            "rejection reasons are shown to users and must read as sentences"
        );
    }

    #[test]
    fn a_hostname_that_merely_starts_with_127_is_not_loopback() {
        // The attack a prefix check would allow: a public hostname that begins
        // with the loopback prefix.
        assert!(!is_loopback_host("127.0.0.1.evil.com"));
        assert!(validate_secure_url("http://127.0.0.1.evil.com/v1").is_err());
        assert!(validate_secure_url("http://127.0.0.1.attacker.net:8080").is_err());
    }

    #[test]
    fn other_loopback_addresses_in_the_127_block_are_allowed() {
        assert!(is_loopback_host("127.0.0.2"));
        assert!(is_loopback_host("127.1.2.3"));
    }

    #[test]
    fn a_url_without_a_scheme_is_refused() {
        assert!(validate_secure_url("api.deepgram.com/v1").is_err());
    }

    #[test]
    fn unsupported_schemes_are_refused() {
        for url in ["ftp://example.com", "file:///etc/passwd", "ws://example.com"] {
            assert!(validate_secure_url(url).is_err(), "{url} should be refused");
        }
    }

    #[test]
    fn a_url_with_no_host_is_refused() {
        assert!(validate_secure_url("https:///v1").is_err());
    }

    #[test]
    fn the_host_is_extracted_without_port_or_path() {
        assert_eq!(host_of("https://api.deepgram.com/v1/listen"), "api.deepgram.com");
        assert_eq!(host_of("http://127.0.0.1:11434/v1"), "127.0.0.1");
        assert_eq!(host_of("https://example.com:8443"), "example.com");
    }

    #[test]
    fn userinfo_is_not_reported_as_the_host() {
        assert_eq!(host_of("https://user:pass@example.com/v1"), "example.com");
    }

    #[test]
    fn ipv6_literals_are_extracted() {
        assert_eq!(host_of("http://[::1]:11434/v1"), "::1");
    }

    #[test]
    fn joining_tolerates_slashes_on_either_side() {
        assert_eq!(
            join_url("https://api.deepgram.com/v1", "listen"),
            "https://api.deepgram.com/v1/listen"
        );
        assert_eq!(
            join_url("https://api.deepgram.com/v1/", "/listen"),
            "https://api.deepgram.com/v1/listen"
        );
        assert_eq!(
            join_url("https://api.deepgram.com/v1", "/listen"),
            "https://api.deepgram.com/v1/listen"
        );
    }
}