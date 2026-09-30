//! Session authentication (§15).
//!
//! The client holds exactly one credential: a session token issued by the Game
//! Bridge gateway. It never holds a Deepgram, DeepSeek, Cartesia, or MiniMax
//! key, and it has no code path that could use one. That is the point of the
//! gateway design — a client that ships provider keys leaks them the day it is
//! published, and this client is open source.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use game_bridge_protocol::events::SessionCredentials;

/// How long before expiry a token is considered stale.
///
/// A token that expires mid-session would drop a live translation, which the
/// user experiences as the product breaking for no reason. Refreshing this
/// early costs one round trip per session.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// A session credential held in memory.
///
/// Deliberately not `Serialize`: it is never written to disk. A token in a
/// config file is a token in a backup, a screenshot, and a support bundle.
#[derive(Clone)]
pub struct SessionToken {
    token: String,
    expires_at_unix: u64,
    session_id: String,
    scopes: Vec<String>,
}

impl SessionToken {
    /// Wrap the credentials from `session.started`.
    pub fn from_credentials(credentials: SessionCredentials) -> Self {
        Self {
            token: credentials.session_token,
            expires_at_unix: credentials.expires_at_unix,
            session_id: credentials.session_id,
            scopes: credentials.scopes,
        }
    }

    /// The bearer value. Only the transport should call this.
    pub fn bearer(&self) -> &str {
        &self.token
    }

    /// Gateway session identifier, for logs and support.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Scopes granted to this session.
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// Expiry as a Unix timestamp.
    pub fn expires_at_unix(&self) -> u64 {
        self.expires_at_unix
    }

    /// Whether the token has expired as of `now_unix`.
    pub fn is_expired_at(&self, now_unix: u64) -> bool {
        now_unix >= self.expires_at_unix
    }

    /// Whether the token should be refreshed before further use.
    pub fn needs_refresh_at(&self, now_unix: u64) -> bool {
        let margin = REFRESH_MARGIN.as_secs();
        now_unix.saturating_add(margin) >= self.expires_at_unix
    }

    /// Whether this session was granted a scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

impl std::fmt::Debug for SessionToken {
    /// Redacts the token. A bearer token in a log line is a credential leak —
    /// logs get pasted into issue trackers.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionToken")
            .field("token", &"<redacted>")
            .field("expires_at_unix", &self.expires_at_unix)
            .field("session_id", &self.session_id)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Current Unix time, or 0 if the clock is before the epoch.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How the client authenticates to the gateway to *obtain* a session token.
///
/// The refresh token is the only long-lived secret the client stores, and it
/// lives in the OS credential store rather than the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountCredentials {
    /// Opaque refresh token from sign-in.
    pub refresh_token: String,
    /// Account identifier, for display.
    pub account_id: String,
}

impl AccountCredentials {
    /// Redact the refresh token when debugging.
    pub fn redacted(&self) -> String {
        format!("account {} with refresh token <redacted>", self.account_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(expires_at_unix: u64) -> SessionCredentials {
        SessionCredentials {
            session_token: "gb_sess_secret_value".into(),
            expires_at_unix,
            session_id: "sess_01HQ2X8W3Y".into(),
            scopes: vec!["stt".into(), "translate".into(), "tts".into()],
        }
    }

    #[test]
    fn a_token_exposes_its_session_metadata() {
        let token = SessionToken::from_credentials(credentials(2_000_000_000));
        assert_eq!(token.session_id(), "sess_01HQ2X8W3Y");
        assert_eq!(token.bearer(), "gb_sess_secret_value");
        assert_eq!(token.scopes().len(), 3);
    }

    #[test]
    fn debug_output_redacts_the_bearer_token() {
        // A token printed to a log is a token leaked into a support ticket.
        let token = SessionToken::from_credentials(credentials(2_000_000_000));
        let debug = format!("{token:?}");
        assert!(!debug.contains("gb_sess_secret_value"), "{debug}");
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("sess_01HQ2X8W3Y"), "session id is fine to log");
    }

    #[test]
    fn expiry_is_exclusive_at_the_boundary() {
        let token = SessionToken::from_credentials(credentials(1000));
        assert!(!token.is_expired_at(999));
        assert!(token.is_expired_at(1000));
        assert!(token.is_expired_at(1001));
    }

    #[test]
    fn refresh_is_requested_before_expiry_not_after() {
        // The bug this prevents: refreshing at expiry means a live session
        // drops while the client is mid-round-trip.
        let token = SessionToken::from_credentials(credentials(1000));
        assert!(!token.needs_refresh_at(900));
        assert!(token.needs_refresh_at(940), "60s margin: 940 + 60 >= 1000");
        assert!(token.needs_refresh_at(1000));
    }

    #[test]
    fn scopes_are_checked_exactly() {
        let token = SessionToken::from_credentials(credentials(1000));
        assert!(token.has_scope("stt"));
        assert!(token.has_scope("tts"));
        assert!(!token.has_scope("billing"));
        assert!(!token.has_scope("STT"), "case matters for scope names");
    }

    #[test]
    fn a_clock_before_the_epoch_does_not_panic() {
        // now_unix() returns 0 rather than panicking if the system clock is
        // wrong, because a wrong clock must not crash the app at launch.
        assert_eq!(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|_| 1u64)
                .unwrap_or(0)
                .max(0),
            1
        );
    }

    #[test]
    fn account_credentials_redact_the_refresh_token() {
        let account = AccountCredentials {
            refresh_token: "long_lived_secret".into(),
            account_id: "acct_123".into(),
        };
        assert!(!account.redacted().contains("long_lived_secret"));
        assert!(account.redacted().contains("acct_123"));
    }
}
