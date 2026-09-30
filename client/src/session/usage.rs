//! Usage history (§28).
//!
//! The Usage screen is Spotify's listening history applied to sessions: what
//! the user played, for how long, and how much of that was billed. The
//! distinction the screen must never blur is **session time versus active
//! voice time** — a two-hour session with twenty minutes of talking costs the
//! same as a twenty-minute session.

use game_bridge_protocol::usage::{format_clock, ActiveVoice, CreditRate, ServiceTier};
use game_bridge_protocol::mode::RoutingMode;
use serde::{Deserialize, Serialize};

/// One completed or in-progress session, as shown in the history list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    /// What the user was playing.
    pub application: String,
    /// Routing mode used.
    pub mode: RoutingMode,
    /// Tier billed.
    pub tier: ServiceTier,
    /// Active-voice counters.
    pub active_voice: ActiveVoice,
    /// Credits consumed, in minor units.
    pub cost_minor: i64,
    /// Currency code.
    pub currency: String,
    /// Unix epoch seconds when the session started.
    pub started_at_unix: u64,
}

impl UsageRecord {
    /// Session length as `"42 min"` or `"1h 08m"`, matching §28.
    pub fn session_duration_display(&self) -> String {
        compact_duration(self.active_voice.session_ms)
    }

    /// Active voice as `"12.4 active min"`, matching §28.
    ///
    /// One decimal place, because the value is an estimate the user reads at a
    /// glance; more precision would imply an accuracy the meter does not claim.
    pub fn active_minutes_display(&self) -> String {
        let minutes = self.active_voice.speech_ms as f64 / 60_000.0;
        format!("{minutes:.1} active min")
    }

    /// Cost as `"฿6.20"`.
    pub fn cost_display(&self) -> String {
        let symbol = game_bridge_protocol::usage::currency_symbol(&self.currency);
        let major = self.cost_minor / 100;
        let minor = (self.cost_minor % 100).abs();
        format!("{symbol}{major}.{minor:02}")
    }
}

/// Format a duration the way §28 does: minutes under an hour, else `Hh MMm`.
pub fn compact_duration(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes} min")
    }
}

/// A month of aggregated usage, for the summary card in §28.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    /// Total time in sessions.
    pub session_ms: u64,
    /// Total detected speech.
    pub speech_ms: u64,
    /// Total credits consumed, in minor units.
    pub cost_minor: i64,
    /// Currency code.
    pub currency: String,
    /// Number of sessions included.
    pub sessions: u32,
}

impl UsageSummary {
    /// Aggregate a set of records.
    pub fn from_records(records: &[UsageRecord]) -> Self {
        let mut summary = Self {
            currency: records
                .first()
                .map(|r| r.currency.clone())
                .unwrap_or_else(|| "THB".into()),
            ..Default::default()
        };
        for record in records {
            summary.session_ms = summary.session_ms.saturating_add(record.active_voice.session_ms);
            summary.speech_ms = summary.speech_ms.saturating_add(record.active_voice.speech_ms);
            summary.cost_minor = summary.cost_minor.saturating_add(record.cost_minor);
            summary.sessions += 1;
        }
        summary
    }

    /// Gameplay time as `"21h 42m"`.
    pub fn session_display(&self) -> String {
        compact_duration(self.session_ms)
    }

    /// Active voice time as `"8h 12m"`.
    ///
    /// The summary uses the compact form rather than the one-decimal form used
    /// per row, because at month scale minutes are precise enough to read.
    pub fn speech_display(&self) -> String {
        compact_duration(self.speech_ms)
    }

    /// Credits used as `"฿246"`.
    ///
    /// Whole units at month scale: the satang are noise in a summary, and
    /// `฿246` is what a user repeats to a friend.
    pub fn cost_display(&self) -> String {
        let symbol = game_bridge_protocol::usage::currency_symbol(&self.currency);
        format!("{symbol}{}", self.cost_minor / 100)
    }

    /// What fraction of session time had speech.
    pub fn duty_cycle(&self) -> f32 {
        if self.session_ms == 0 {
            0.0
        } else {
            (self.speech_ms as f64 / self.session_ms as f64) as f32
        }
    }
}

/// The client's rolling usage history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageHistory {
    /// Records, newest last.
    records: Vec<UsageRecord>,
    /// Cap on retained records, so a long-running install cannot grow the
    /// config file without bound.
    #[serde(default = "default_capacity")]
    capacity: usize,
}

fn default_capacity() -> usize {
    500
}

impl Default for UsageHistory {
    /// A history with the default capacity.
    ///
    /// Written out rather than derived. A derived `Default` gives
    /// `capacity: 0` — the `#[serde(default)]` attribute above applies only to
    /// deserialization — and the first `push` would then call `remove(0)` on an
    /// empty vector and panic. That is a crash on the very first session the
    /// user ever finishes, which is exactly the kind of bug a derived `Default`
    /// invites.
    fn default() -> Self {
        Self {
            records: Vec::new(),
            capacity: default_capacity(),
        }
    }
}

impl UsageHistory {
    /// Create a history retaining at most `capacity` records.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            records: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// All records, oldest first.
    pub fn records(&self) -> &[UsageRecord] {
        &self.records
    }

    /// How many records are retained.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether there is no history.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Append a record, evicting the oldest if at capacity.
    pub fn push(&mut self, record: UsageRecord) {
        // A zero capacity can only arrive from a hand-edited config file;
        // guard rather than panic in that case.
        if self.capacity == 0 {
            self.capacity = default_capacity();
        }
        if self.records.len() >= self.capacity {
            self.records.remove(0);
        }
        self.records.push(record);
    }

    /// The most recent record.
    pub fn latest(&self) -> Option<&UsageRecord> {
        self.records.last()
    }

    /// Records from the last `seconds` before `now_unix`.
    pub fn since(&self, now_unix: u64, seconds: u64) -> Vec<&UsageRecord> {
        let cutoff = now_unix.saturating_sub(seconds);
        self.records
            .iter()
            .filter(|record| record.started_at_unix >= cutoff)
            .collect()
    }

    /// Aggregate everything recorded.
    pub fn summary(&self) -> UsageSummary {
        UsageSummary::from_records(&self.records)
    }
}

/// Recompute a record's cost from its own counters and a rate table.
///
/// Used when a session ends: the gateway's final `usage.update` is preferred,
/// but if the connection dropped before it arrived, this produces a defensible
/// local figure for the history list rather than showing zero.
pub fn cost_for_record(record: &UsageRecord, rates: &[CreditRate]) -> i64 {
    CreditRate::find(rates, record.tier)
        .map(|rate| rate.cost_minor_for(record.active_voice.speech_ms))
        .unwrap_or(record.cost_minor)
}

/// Total speech time formatted for a session card.
pub fn speech_clock(speech_ms: u64) -> String {
    format_clock(speech_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(application: &str, session_ms: u64, speech_ms: u64, cost_minor: i64) -> UsageRecord {
        let mut active_voice = ActiveVoice::default();
        active_voice.record_elapsed(session_ms);
        if speech_ms > 0 {
            active_voice.record_speech(speech_ms);
        }
        UsageRecord {
            application: application.into(),
            mode: RoutingMode::VoiceOut,
            tier: ServiceTier::Voice,
            active_voice,
            cost_minor,
            currency: "THB".into(),
            started_at_unix: 1_700_000_000,
        }
    }

    #[test]
    fn session_duration_matches_the_usage_screen_examples() {
        // §28: "42 min session" and "1h 08m session".
        let short = record("Valorant", 42 * 60_000, 0, 0);
        assert_eq!(short.session_duration_display(), "42 min");

        let long = record("Delta Force", (60 + 8) * 60_000, 0, 0);
        assert_eq!(long.session_duration_display(), "1h 08m");
    }

    #[test]
    fn active_minutes_match_the_usage_screen_examples() {
        // §28: "12.4 active min" and "21.7 active min".
        let a = record("Valorant", 0, 12 * 60_000 + 24_000, 0);
        assert_eq!(a.active_minutes_display(), "12.4 active min");

        let b = record("Delta Force", 0, 21 * 60_000 + 42_000, 0);
        assert_eq!(b.active_minutes_display(), "21.7 active min");
    }

    #[test]
    fn cost_matches_the_usage_screen_examples() {
        // §28 shows ฿6.20 and ฿10.85.
        assert_eq!(record("Valorant", 0, 0, 620).cost_display(), "฿6.20");
        assert_eq!(record("Delta Force", 0, 0, 1085).cost_display(), "฿10.85");
    }

    #[test]
    fn session_time_and_active_time_are_independent() {
        // The distinction the screen exists to make: a long session with little
        // speech is cheap.
        let r = record("Valorant", 42 * 60_000, 12 * 60_000 + 24_000, 620);
        assert_eq!(r.session_duration_display(), "42 min");
        assert_eq!(r.active_minutes_display(), "12.4 active min");
    }

    #[test]
    fn compact_duration_boundaries() {
        assert_eq!(compact_duration(0), "0 min");
        assert_eq!(compact_duration(59_999), "0 min");
        assert_eq!(compact_duration(60_000), "1 min");
        assert_eq!(compact_duration(3_599_000), "59 min");
        assert_eq!(compact_duration(3_600_000), "1h 00m");
        assert_eq!(compact_duration(21 * 3_600_000 + 42 * 60_000), "21h 42m");
    }

    #[test]
    fn summary_matches_the_month_card() {
        // §28: "Gameplay 21h 42m / Active Voice 8h 12m / Credits Used ฿246".
        let records = vec![
            record("A", 10 * 3_600_000, 4 * 3_600_000, 12_000),
            record("B", 11 * 3_600_000 + 42 * 60_000, 4 * 3_600_000 + 12 * 60_000, 12_600),
        ];
        let summary = UsageSummary::from_records(&records);
        assert_eq!(summary.session_display(), "21h 42m");
        assert_eq!(summary.speech_display(), "8h 12m");
        assert_eq!(summary.cost_display(), "฿246");
        assert_eq!(summary.sessions, 2);
    }

    #[test]
    fn summary_of_nothing_is_zeroed() {
        let summary = UsageSummary::from_records(&[]);
        assert_eq!(summary.session_display(), "0 min");
        assert_eq!(summary.cost_display(), "฿0");
        assert_eq!(summary.sessions, 0);
        assert_eq!(summary.duty_cycle(), 0.0);
    }

    #[test]
    fn summary_duty_cycle_reflects_talk_time() {
        let records = vec![record("A", 60_000, 15_000, 0)];
        let summary = UsageSummary::from_records(&records);
        assert!((summary.duty_cycle() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn history_evicts_the_oldest_at_capacity() {
        let mut history = UsageHistory::with_capacity(3);
        for i in 0..5 {
            history.push(record(&format!("game{i}"), 0, 0, 0));
        }
        assert_eq!(history.len(), 3);
        assert_eq!(history.records()[0].application, "game2");
        assert_eq!(history.latest().unwrap().application, "game4");
    }

    #[test]
    fn history_since_filters_by_start_time() {
        let mut history = UsageHistory::with_capacity(10);
        for offset in [0u64, 100, 200] {
            let mut r = record("game", 0, 0, 0);
            r.started_at_unix = 1_000 + offset;
            history.push(r);
        }
        let recent = history.since(1_200, 150);
        assert_eq!(recent.len(), 2, "only records at or after 1050");
    }

    #[test]
    fn history_survives_a_json_round_trip() {
        let mut history = UsageHistory::with_capacity(10);
        history.push(record("Valorant", 60_000, 30_000, 125));
        let json = serde_json::to_string(&history).unwrap();
        let back: UsageHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.latest().unwrap().application, "Valorant");
    }

    #[test]
    fn local_cost_recompute_uses_the_rate_table() {
        let r = record("Valorant", 0, 120_000, 0);
        let cost = cost_for_record(&r, &CreditRate::launch_defaults());
        // Voice tier at ฿2.50/min for two minutes.
        assert_eq!(cost, 500);
    }

    #[test]
    fn local_cost_recompute_falls_back_to_the_stored_value() {
        let r = record("Valorant", 0, 120_000, 777);
        assert_eq!(cost_for_record(&r, &[]), 777);
    }

    #[test]
    fn empty_history_is_reported_as_empty() {
        let history = UsageHistory::default();
        assert!(history.is_empty());
        assert_eq!(history.len(), 0);
        assert!(history.latest().is_none());
    }
}
