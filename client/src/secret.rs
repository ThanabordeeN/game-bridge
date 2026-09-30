//! Credentials that must never reach a log line.
//!
//! Every provider credential in this client is wrapped in [`Secret`], whose
//! `Debug` impl redacts the value. That matters more here than in most code:
//! this client is open source, its logs get pasted into issue trackers, and a
//! bearer token printed once is a token leaked permanently.
//!
//! The length is retained in the redacted form because a truncated paste is a
//! real and confusing failure — `Secret(<redacted, 12 bytes>)` immediately tells
//! you the key was cut off, without telling anyone what it was.

use std::fmt;

/// A credential value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a credential, rejecting empty or whitespace-only values.
    ///
    /// Returning `None` rather than storing an empty value means a blank
    /// environment variable is reported as "no credential" instead of producing
    /// a confusing 401 from the provider.
    pub fn new(raw: impl Into<String>) -> Option<Self> {
        let raw = raw.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self(trimmed.to_string()))
    }

    /// Read a credential from an environment variable.
    ///
    /// The only supported source. A credential in a config file would be a
    /// credential in a backup, a screenshot, and a support bundle.
    pub fn from_env(var: &str) -> Option<Self> {
        std::env::var(var).ok().and_then(Self::new)
    }

    /// The credential value. Only an HTTP layer should call this.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Length in bytes, safe to log.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always false; a `Secret` is never empty by construction.
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret(<redacted, {} bytes>)", self.0.len())
    }
}

impl fmt::Display for Secret {
    /// Deliberately redacted too.
    ///
    /// A `Display` impl that printed the value would be a footgun: `{}` is what
    /// people reach for in `format!` when building an error message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_the_value() {
        let secret = Secret::new("sk-super-secret-value").unwrap();
        let debug = format!("{secret:?}");
        assert!(!debug.contains("sk-super-secret-value"), "{debug}");
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn display_redacts_the_value_too() {
        // `{}` is what people reach for inside `format!` when building an error
        // message, so it must not print the credential.
        let secret = Secret::new("sk-super-secret-value").unwrap();
        let displayed = format!("{secret}");
        assert!(!displayed.contains("sk-super-secret-value"), "{displayed}");
        assert_eq!(displayed, "<redacted>");
    }

    #[test]
    fn the_length_is_reported_without_the_value() {
        // A truncated paste is a common and confusing failure; the length tells
        // you it happened.
        let secret = Secret::new("abcdefghij").unwrap();
        assert_eq!(secret.len(), 10);
        assert!(format!("{secret:?}").contains("10 bytes"));
    }

    #[test]
    fn blank_values_are_rejected_rather_than_stored() {
        assert!(Secret::new("").is_none());
        assert!(Secret::new("   ").is_none());
        assert!(Secret::new("\t\n").is_none());
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(Secret::new("  sk-abc  ").unwrap().expose(), "sk-abc");
    }

    #[test]
    fn a_secret_round_trips_its_value_for_the_http_layer() {
        let secret = Secret::new("sk-abc").unwrap();
        assert_eq!(secret.expose(), "sk-abc");
        assert!(!secret.is_empty());
    }
}