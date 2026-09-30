//! The credit wallet (§16, §29).
//!
//! The client renders the balance and estimates remaining time. It does not
//! decide it. Every value here is either supplied by the gateway or derived
//! from gateway-supplied rates for display, and [`Wallet::apply`] always
//! overwrites rather than merges.

use game_bridge_protocol::usage::{CreditRate, ServiceTier, UsageUpdate};
use serde::{Deserialize, Serialize};

/// The user's balance and the rates in force.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wallet {
    /// Balance in minor units.
    balance_minor: i64,
    /// ISO-4217 currency code.
    currency: String,
    /// Rates returned by the gateway at `session.started`.
    rates: Vec<CreditRate>,
    /// Threshold below which the low-credit state is raised (§40).
    low_credit_threshold_minor: i64,
}

impl Default for Wallet {
    fn default() -> Self {
        Self {
            balance_minor: 0,
            currency: "THB".into(),
            rates: CreditRate::launch_defaults(),
            // §40's example shows a warning at ฿4.25 remaining.
            low_credit_threshold_minor: 425,
        }
    }
}

impl Wallet {
    /// Balance in minor units.
    pub fn balance_minor(&self) -> i64 {
        self.balance_minor
    }

    /// Currency code.
    pub fn currency(&self) -> &str {
        &self.currency
    }

    /// Format the balance as `"฿84.50"` (§29).
    pub fn balance_display(&self) -> String {
        format_minor(self.balance_minor, &self.currency)
    }

    /// Rates in force.
    pub fn rates(&self) -> &[CreditRate] {
        &self.rates
    }

    /// Take rates and balance from a `session.started`.
    pub fn apply_rates(&mut self, rates: Vec<CreditRate>) {
        if let Some(first) = rates.first() {
            self.currency = first.currency_code.clone();
        }
        self.rates = rates;
    }

    /// Apply a `credit.update`.
    pub fn apply(&mut self, balance_minor: i64, currency: impl Into<String>) {
        self.balance_minor = balance_minor;
        self.currency = currency.into();
    }

    /// Apply a `credit.update` that also carries a threshold.
    pub fn apply_with_threshold(
        &mut self,
        balance_minor: i64,
        currency: impl Into<String>,
        threshold: Option<i64>,
    ) {
        self.apply(balance_minor, currency);
        if let Some(threshold) = threshold {
            self.low_credit_threshold_minor = threshold;
        }
    }

    /// The rate for a tier, if known.
    pub fn rate_for(&self, tier: ServiceTier) -> Option<CreditRate> {
        CreditRate::find(&self.rates, tier)
    }

    /// Whether the balance has crossed the low-credit threshold (§40).
    pub fn is_low(&self) -> bool {
        self.balance_minor <= self.low_credit_threshold_minor
    }

    /// Whether the balance cannot cover any further use.
    pub fn is_exhausted(&self) -> bool {
        self.balance_minor <= 0
    }

    /// Estimated remaining time at a tier before the balance runs out.
    ///
    /// `None` when the rate is unknown or zero, because "infinite" and "unknown"
    /// must not both render as a very large number.
    pub fn remaining_minutes(&self, tier: ServiceTier) -> Option<f64> {
        let rate = self.rate_for(tier)?;
        if rate.minor_per_minute <= 0 || self.balance_minor <= 0 {
            return Some(0.0);
        }
        Some(self.balance_minor as f64 / rate.minor_per_minute as f64)
    }

    /// Estimated remaining time formatted for the wallet screen.
    pub fn remaining_display(&self, tier: ServiceTier) -> String {
        match self.remaining_minutes(tier) {
            None => "—".to_string(),
            Some(minutes) if minutes < 1.0 => {
                format!("{} sec", (minutes * 60.0).round().max(0.0) as u64)
            }
            Some(minutes) if minutes < 60.0 => format!("{minutes:.0} min"),
            Some(minutes) => {
                let hours = (minutes / 60.0).floor();
                let remainder = (minutes - hours * 60.0).round();
                format!("{hours:.0}h {remainder:.0}m")
            }
        }
    }

    /// The rate rows shown in the wallet screen (§29).
    pub fn rate_rows(&self) -> Vec<(ServiceTier, String)> {
        let mut rows: Vec<(ServiceTier, String)> = self
            .rates
            .iter()
            .map(|rate| (rate.tier, rate.display()))
            .collect();
        rows.sort_by_key(|(tier, _)| match tier {
            ServiceTier::Subtitle => 0,
            ServiceTier::Voice => 1,
            ServiceTier::PremiumVoice => 2,
        });
        rows
    }

    /// The cost of a usage snapshot at the current rates, for cross-checking
    /// the gateway's figure in logs.
    pub fn cost_of(&self, usage: &UsageUpdate) -> Option<i64> {
        self.rate_for(usage.tier)
            .map(|rate| rate.cost_minor_for(usage.active_voice.speech_ms))
    }
}

/// Format a minor-unit amount with its currency symbol.
///
/// Handles the `.00`-to-`.99` boundary across zero explicitly. Rust's integer
/// division truncates toward zero, so a balance of `-5` yields a major part of
/// `0` and a fraction of `5`, and naive formatting produces `฿0.05` for a
/// negative balance — wrong by both magnitude and sign. The sign is therefore
/// decided once, from the original value.
pub fn format_minor(minor: i64, currency: &str) -> String {
    let symbol = game_bridge_protocol::usage::currency_symbol(currency);
    let negative = minor < 0;
    let magnitude = minor.unsigned_abs();
    let major = magnitude / 100;
    let fraction = magnitude % 100;
    let sign = if negative { "-" } else { "" };
    format!("{symbol}{sign}{major}.{fraction:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_bridge_protocol::usage::ActiveVoice;

    #[test]
    fn balance_formats_like_the_wallet_screen() {
        // §29: "฿84.50".
        let mut wallet = Wallet::default();
        wallet.apply(8450, "THB");
        assert_eq!(wallet.balance_display(), "฿84.50");
    }

    #[test]
    fn negative_balances_format_without_a_double_sign() {
        // A balance can go momentarily negative while a final usage update
        // settles; the display must not read "฿-1.-50".
        assert_eq!(format_minor(-150, "THB"), "฿-1.50");
        assert_eq!(format_minor(-5, "THB"), "฿-0.05");
    }

    #[test]
    fn low_credit_matches_the_error_state_example() {
        // §40: "Low Credit — ฿4.25 remaining".
        let mut wallet = Wallet::default();
        wallet.apply(425, "THB");
        assert!(wallet.is_low(), "฿4.25 should be the warning threshold");
        wallet.apply(426, "THB");
        assert!(!wallet.is_low());
    }

    #[test]
    fn exhausted_covers_zero_and_negative() {
        let mut wallet = Wallet::default();
        wallet.apply(0, "THB");
        assert!(wallet.is_exhausted());
        wallet.apply(-100, "THB");
        assert!(wallet.is_exhausted());
        wallet.apply(1, "THB");
        assert!(!wallet.is_exhausted());
    }

    #[test]
    fn gateway_rates_replace_local_defaults() {
        let mut wallet = Wallet::default();
        wallet.apply_rates(vec![CreditRate {
            tier: ServiceTier::Subtitle,
            minor_per_minute: 75,
            currency_code: "USD".into(),
        }]);
        assert_eq!(wallet.currency(), "USD");
        assert_eq!(wallet.rate_for(ServiceTier::Subtitle).unwrap().minor_per_minute, 75);
    }

    #[test]
    fn a_gateway_update_overwrites_rather_than_merges() {
        let mut wallet = Wallet::default();
        wallet.apply(10_000, "THB");
        wallet.apply(50, "THB");
        assert_eq!(wallet.balance_minor(), 50, "the latest update wins");
    }

    #[test]
    fn threshold_can_be_supplied_by_the_gateway() {
        let mut wallet = Wallet::default();
        wallet.apply_with_threshold(900, "THB", Some(1000));
        assert!(wallet.is_low());
    }

    #[test]
    fn remaining_time_is_derived_from_the_voice_rate() {
        let mut wallet = Wallet::default();
        wallet.apply(8450, "THB");
        // ฿84.50 at ฿2.50/min is 33.8 minutes.
        let minutes = wallet.remaining_minutes(ServiceTier::Voice).unwrap();
        assert!((minutes - 33.8).abs() < 0.05, "got {minutes}");
    }

    #[test]
    fn remaining_time_is_zero_when_the_balance_is_gone() {
        let mut wallet = Wallet::default();
        wallet.apply(0, "THB");
        assert_eq!(wallet.remaining_minutes(ServiceTier::Voice), Some(0.0));
    }

    #[test]
    fn remaining_time_is_unknown_without_a_rate() {
        let mut wallet = Wallet::default();
        wallet.apply_rates(vec![]);
        assert_eq!(wallet.remaining_minutes(ServiceTier::Voice), None);
        assert_eq!(wallet.remaining_display(ServiceTier::Voice), "—");
    }

    #[test]
    fn remaining_display_scales_from_seconds_to_hours() {
        let mut wallet = Wallet::default();

        wallet.apply(50, "THB"); // ฿0.50 at ฿2.50/min = 12 seconds
        assert_eq!(wallet.remaining_display(ServiceTier::Voice), "12 sec");

        wallet.apply(2500, "THB"); // 10 minutes
        assert_eq!(wallet.remaining_display(ServiceTier::Voice), "10 min");

        wallet.apply(25_000, "THB"); // 100 minutes = 1h 40m
        assert_eq!(wallet.remaining_display(ServiceTier::Voice), "1h 40m");
    }

    #[test]
    fn rate_rows_are_ordered_as_the_ui_shows_them() {
        let wallet = Wallet::default();
        let rows = wallet.rate_rows();
        assert_eq!(rows[0].0, ServiceTier::Subtitle);
        assert_eq!(rows[1].0, ServiceTier::Voice);
        assert_eq!(rows[2].0, ServiceTier::PremiumVoice);
    }

    #[test]
    fn rate_rows_use_the_documented_prices() {
        let wallet = Wallet::default();
        let rows = wallet.rate_rows();
        assert_eq!(rows[0].1, "฿0.50 / minute");
        assert_eq!(rows[1].1, "฿2.50 / minute");
    }

    #[test]
    fn cost_of_a_usage_snapshot_uses_the_current_rate() {
        let wallet = Wallet::default();
        let mut usage = UsageUpdate {
            tier: ServiceTier::Voice,
            ..Default::default()
        };
        usage.active_voice.record_speech(60_000);
        assert_eq!(wallet.cost_of(&usage), Some(250));
    }

    #[test]
    fn wallet_round_trips_through_json() {
        let mut wallet = Wallet::default();
        wallet.apply(8450, "THB");
        let json = serde_json::to_string(&wallet).unwrap();
        let back: Wallet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, wallet);
    }

    #[test]
    fn untouched_wallet_defaults_are_the_launch_rates() {
        let wallet = Wallet::default();
        assert_eq!(
            wallet.rate_for(ServiceTier::Subtitle).unwrap().minor_per_minute,
            50
        );
        assert_eq!(
            wallet.rate_for(ServiceTier::Voice).unwrap().minor_per_minute,
            250
        );
    }

    #[test]
    fn active_voice_type_is_used_for_cost_math() {
        let mut av = ActiveVoice::default();
        av.record_speech(120_000);
        let usage = UsageUpdate {
            active_voice: av,
            tier: ServiceTier::Voice,
            ..Default::default()
        };
        assert_eq!(Wallet::default().cost_of(&usage), Some(500));
    }
}
