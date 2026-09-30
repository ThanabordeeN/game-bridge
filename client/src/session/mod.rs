//! Session-scoped counters: usage history and the credit wallet.
//!
//! These are the client's *presentation* of billing, not its authority. The
//! gateway is authoritative; everything here exists so the Usage and Billing
//! screens can render without a round trip, and so a session's history survives
//! a restart.

pub mod credits;
pub mod session;
pub mod usage;

pub use credits::Wallet;
pub use session::{Session, SessionConfig, SessionState};
pub use usage::{UsageHistory, UsageRecord, UsageSummary};
