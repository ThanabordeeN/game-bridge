//! Active-voice metering and credit rates (§16, §29).
//!
//! The product promise is narrow and must be exact: **only speech that is
//! detected is charged; silence is not.** That means the meter counts
//! *speech* milliseconds, not session wall-clock, and the cost function is
//! pro-rata at millisecond resolution rather than rounding up to whole minutes.
//!
//! All money is integer minor units (satang for THB). Floating-point currency
//! accrues rounding drift that shows up as a one-satang discrepancy a user will
//! screenshot, so it is not used anywhere in this module.

use serde::{Deserialize, Serialize};

use crate::mode::RoutingMode;

/// What a user is charged for, in the words the UI uses (§16).
///
/// Deliberately not the provider names. A user buys *Subtitle* or *Voice*, not
/// a Cartesia minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// Text translation only.
    Subtitle,
    /// Translated voice output at the standard or cloned tier.
    Voice,
    /// The higher-fidelity voice product.
    PremiumVoice,
}

impl ServiceTier {
    /// Label shown in the wallet screen (§29).
    pub const fn label(self) -> &'static str {
        match self {
            ServiceTier::Subtitle => "Subtitle",
            ServiceTier::Voice => "Voice",
            ServiceTier::PremiumVoice => "Premium Voice",
        }
    }

    /// The tier a routing mode bills at, absent an explicit premium upgrade.
    ///
    /// Subtitle mode bills as subtitle regardless of voice settings, because it
    /// emits no TTS at all (§10).
    pub const fn for_routing_mode(mode: RoutingMode) -> Self {
        match mode {
            RoutingMode::Subtitle => ServiceTier::Subtitle,
            RoutingMode::VoiceOut | RoutingMode::FullVoice => ServiceTier::Voice,
        }
    }
}

/// A price: minor units per active-voice minute.
///
/// Not `Copy`: the currency code is a `String`. Rates are cloned rather than
/// copied, which is fine — they are read once per session, not per audio frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditRate {
    /// Which product this rate applies to.
    pub tier: ServiceTier,
    /// Cost in minor units (satang) per minute of active voice.
    pub minor_per_minute: i64,
    /// ISO-4217 currency code.
    pub currency_code: String,
}

impl CreditRate {
    /// The launch rates from §16 and §29.
    ///
    /// * Subtitle: ฿1 per 2 minutes, i.e. ฿0.50 per minute.
    /// * Voice: ฿2.50 per minute, the top of the documented 2.0–2.5 range.
    /// * Premium Voice: higher burn, set at ฿4.00 per minute.
    pub fn launch_defaults() -> Vec<Self> {
        vec![
            Self {
                tier: ServiceTier::Subtitle,
                minor_per_minute: 50,
                currency_code: "THB".into(),
            },
            Self {
                tier: ServiceTier::Voice,
                minor_per_minute: 250,
                currency_code: "THB".into(),
            },
            Self {
                tier: ServiceTier::PremiumVoice,
                minor_per_minute: 400,
                currency_code: "THB".into(),
            },
        ]
    }

    /// Find the rate for `tier` in a rate table.
    pub fn find(rates: &[Self], tier: ServiceTier) -> Option<Self> {
        rates.iter().find(|r| r.tier == tier).cloned()
    }

    /// Cost in minor units for `speech_ms` of detected speech.
    ///
    /// Pro-rata at millisecond resolution, rounded half-up to the nearest
    /// satang. Zero speech costs zero — the §29 promise, enforced here rather
    /// than in the UI.
    pub fn cost_minor_for(&self, speech_ms: u64) -> i64 {
        if speech_ms == 0 || self.minor_per_minute == 0 {
            return 0;
        }
        // minor = rate * ms / 60000, rounded half-up, using integer math.
        let numerator = (self.minor_per_minute as i128) * (speech_ms as i128);
        let denominator = 60_000i128;
        let rounded = (numerator + denominator / 2) / denominator;
        rounded as i64
    }

    /// Human-readable rate string, e.g. `"฿0.50 / minute"`.
    ///
    /// The symbol is derived from the currency code so the UI does not hardcode
    /// a baht sign.
    pub fn display(&self) -> String {
        let symbol = currency_symbol(&self.currency_code);
        let negative = self.minor_per_minute < 0;
        let magnitude = self.minor_per_minute.unsigned_abs();
        let sign = if negative { "-" } else { "" };
        format!("{symbol}{sign}{}.{:02} / minute", magnitude / 100, magnitude % 100)
    }
}

/// The symbol for a currency code, falling back to the code itself.
pub fn currency_symbol(code: &str) -> &str {
    match code {
        "THB" => "฿",
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        other => other,
    }
}

/// Accumulated active-voice counters for a session or a day (§28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActiveVoice {
    /// Milliseconds during which speech was detected. This is the billed
    /// quantity.
    pub speech_ms: u64,
    /// Milliseconds of session wall-clock, whether or not anyone spoke. Shown
    /// in history but never billed.
    pub session_ms: u64,
    /// Number of speech segments detected.
    pub segments: u32,
}

impl ActiveVoice {
    /// Average speech-segment length in milliseconds, or 0 with no segments.
    pub fn mean_segment_ms(&self) -> u64 {
        if self.segments == 0 {
            0
        } else {
            self.speech_ms / u64::from(self.segments)
        }
    }

    /// What fraction of the session had speech, in `0.0..=1.0`.
    ///
    /// The Usage screen shows this as the difference between "42 min session"
    /// and "12.4 active min" (§28).
    pub fn duty_cycle(&self) -> f32 {
        if self.session_ms == 0 {
            0.0
        } else {
            (self.speech_ms as f64 / self.session_ms as f64) as f32
        }
    }

    /// Add a detected speech segment.
    pub fn record_speech(&mut self, speech_ms: u64) {
        self.speech_ms = self.speech_ms.saturating_add(speech_ms);
        self.segments = self.segments.saturating_add(1);
    }

    /// Add elapsed wall-clock time to the session.
    pub fn record_elapsed(&mut self, elapsed_ms: u64) {
        self.session_ms = self.session_ms.saturating_add(elapsed_ms);
    }

    /// Format `speech_ms` the way the session card does: `"12:48 active voice"`.
    ///
    /// Hours are shown only when nonzero, so a short session reads as `08:42`
    /// rather than `00:08:42`.
    pub fn format_speech_clock(&self) -> String {
        format_clock(self.speech_ms)
    }
}

/// Format milliseconds as `MM:SS`, or `H:MM:SS` past an hour.
pub fn format_clock(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// A running usage/cost snapshot, sent as `usage.update`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageUpdate {
    /// Active-voice counters so far this session.
    pub active_voice: ActiveVoice,
    /// Credits consumed so far, in minor units.
    pub cost_minor: i64,
    /// ISO-4217 currency code.
    pub currency: String,
    /// Tier this session is billing at.
    pub tier: ServiceTier,
    /// Most recent end-to-end latency measurement in milliseconds, measured
    /// speech-end to first translated audio. Shown live on the session card
    /// (§45).
    pub latency_ms: u32,
}

impl Default for UsageUpdate {
    fn default() -> Self {
        Self {
            active_voice: ActiveVoice::default(),
            cost_minor: 0,
            currency: "THB".into(),
            tier: ServiceTier::Subtitle,
            latency_ms: 0,
        }
    }
}

impl UsageUpdate {
    /// Format the cost the way the session card does: `"฿6.40 used"`.
    pub fn format_cost(&self) -> String {
        let symbol = currency_symbol(&self.currency);
        // Decide the sign once. Rust's integer division truncates toward zero,
        // so formatting a negative amount the naive way yields the wrong sign
        // and the wrong magnitude across the zero boundary.
        let negative = self.cost_minor < 0;
        let magnitude = self.cost_minor.unsigned_abs();
        let sign = if negative { "-" } else { "" };
        format!("{symbol}{sign}{}.{:02}", magnitude / 100, magnitude % 100)
    }

    /// Recompute `cost_minor` from the active-voice counters and a rate table.
    ///
    /// The client uses this for optimistic local display between
    /// `usage.update` messages; the gateway remains the authority for billing.
    /// Treating the server value as authoritative is deliberate — a client that
    /// can set its own bill is a client that can be edited to set its own bill.
    pub fn recompute_cost(&mut self, rates: &[CreditRate]) {
        if let Some(rate) = CreditRate::find(rates, self.tier) {
            self.cost_minor = rate.cost_minor_for(self.active_voice.speech_ms);
            self.currency = rate.currency_code.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subtitle_rate() -> CreditRate {
        CreditRate {
            tier: ServiceTier::Subtitle,
            minor_per_minute: 50,
            currency_code: "THB".into(),
        }
    }

    fn voice_rate() -> CreditRate {
        CreditRate {
            tier: ServiceTier::Voice,
            minor_per_minute: 250,
            currency_code: "THB".into(),
        }
    }

    #[test]
    fn silence_is_never_billed() {
        // The central billing promise (§29). Zero speech, zero cost, for every
        // rate.
        for rate in CreditRate::launch_defaults() {
            assert_eq!(rate.cost_minor_for(0), 0, "{}", rate.tier.label());
        }
    }

    #[test]
    fn subtitle_costs_one_baht_for_two_minutes() {
        // §16: ฿1 = 2 active voice minutes.
        assert_eq!(subtitle_rate().cost_minor_for(120_000), 100);
    }

    #[test]
    fn subtitle_costs_half_a_baht_for_one_minute() {
        assert_eq!(subtitle_rate().cost_minor_for(60_000), 50);
    }

    #[test]
    fn voice_costs_two_fifty_per_minute() {
        assert_eq!(voice_rate().cost_minor_for(60_000), 250);
        assert_eq!(voice_rate().cost_minor_for(120_000), 500);
    }

    #[test]
    fn premium_voice_burns_faster_than_voice() {
        let rates = CreditRate::launch_defaults();
        let voice = CreditRate::find(&rates, ServiceTier::Voice).unwrap();
        let premium = CreditRate::find(&rates, ServiceTier::PremiumVoice).unwrap();
        assert!(premium.minor_per_minute > voice.minor_per_minute);
    }

    #[test]
    fn part_minutes_are_pro_rata_not_rounded_up() {
        // 30 seconds at ฿2.50/min is ฿1.25, not a full minute.
        assert_eq!(voice_rate().cost_minor_for(30_000), 125);
        // 6 seconds is ฿0.25.
        assert_eq!(voice_rate().cost_minor_for(6_000), 25);
    }

    #[test]
    fn sub_satang_fractions_round_half_up_and_stay_integral() {
        let rate = subtitle_rate();
        // One second at ฿0.50/min is 0.833 satang -> 1 satang.
        assert_eq!(rate.cost_minor_for(1_000), 1);
        // Ten milliseconds is 0.0083 satang -> 0.
        assert_eq!(rate.cost_minor_for(10), 0);
        // Cost is monotonic as speech accumulates.
        let mut previous = 0i64;
        for second in 1..=120u64 {
            let cost = rate.cost_minor_for(second * 1000);
            assert!(cost >= previous, "cost went backwards at {second}s");
            previous = cost;
        }
    }

    #[test]
    fn two_minutes_of_speech_spread_over_an_hour_costs_the_same() {
        // Billing follows speech, not wall-clock: this is the whole point of
        // local VAD (§7).
        let rate = voice_rate();
        assert_eq!(rate.cost_minor_for(120_000), 500);
    }

    #[test]
    fn display_formats_baht_with_two_decimals() {
        assert_eq!(subtitle_rate().display(), "฿0.50 / minute");
        assert_eq!(voice_rate().display(), "฿2.50 / minute");
    }

    #[test]
    fn display_uses_the_currency_symbol_for_each_code() {
        let usd = CreditRate {
            tier: ServiceTier::Voice,
            minor_per_minute: 350,
            currency_code: "USD".into(),
        };
        assert_eq!(usd.display(), "$3.50 / minute");
    }

    #[test]
    fn tier_follows_routing_mode() {
        assert_eq!(
            ServiceTier::for_routing_mode(RoutingMode::Subtitle),
            ServiceTier::Subtitle
        );
        assert_eq!(
            ServiceTier::for_routing_mode(RoutingMode::VoiceOut),
            ServiceTier::Voice
        );
        assert_eq!(
            ServiceTier::for_routing_mode(RoutingMode::FullVoice),
            ServiceTier::Voice
        );
    }

    #[test]
    fn active_voice_tracks_speech_and_session_separately() {
        let mut av = ActiveVoice::default();
        av.record_elapsed(60_000);
        av.record_speech(10_000);
        av.record_speech(5_000);

        assert_eq!(av.session_ms, 60_000);
        assert_eq!(av.speech_ms, 15_000);
        assert_eq!(av.segments, 2);
        assert_eq!(av.mean_segment_ms(), 7_500);
        assert!((av.duty_cycle() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn duty_cycle_handles_empty_session() {
        let av = ActiveVoice::default();
        assert_eq!(av.duty_cycle(), 0.0);
        assert_eq!(av.mean_segment_ms(), 0);
    }

    #[test]
    fn speech_clock_matches_the_session_card_format() {
        let mut av = ActiveVoice::default();
        av.record_speech(12 * 60_000 + 48_000); // §22 shows "12:48 active voice"
        assert_eq!(av.format_speech_clock(), "12:48");
    }

    #[test]
    fn clock_grows_to_hours_past_sixty_minutes() {
        assert_eq!(format_clock(8 * 60_000 + 42_000), "08:42");
        assert_eq!(format_clock(3_600_000), "1:00:00");
        assert_eq!(format_clock(21 * 3_600_000 + 42 * 60_000), "21:42:00");
    }

    #[test]
    fn cost_formats_like_the_session_card() {
        let usage = UsageUpdate {
            cost_minor: 640,
            ..Default::default()
        };
        assert_eq!(usage.format_cost(), "฿6.40");
    }

    #[test]
    fn recompute_matches_the_rate_table() {
        let rates = CreditRate::launch_defaults();
        let mut usage = UsageUpdate {
            tier: ServiceTier::Voice,
            ..Default::default()
        };
        usage.active_voice.record_speech(120_000);
        usage.recompute_cost(&rates);
        assert_eq!(usage.cost_minor, 500);
        assert_eq!(usage.currency, "THB");
    }

    #[test]
    fn recompute_leaves_cost_untouched_when_the_tier_has_no_rate() {
        let mut usage = UsageUpdate {
            tier: ServiceTier::PremiumVoice,
            cost_minor: 123,
            ..Default::default()
        };
        usage.recompute_cost(&[]);
        assert_eq!(usage.cost_minor, 123, "no rate means no local guess");
    }

    #[test]
    fn saturating_counters_do_not_wrap() {
        let mut av = ActiveVoice {
            speech_ms: u64::MAX - 5,
            ..Default::default()
        };
        av.record_speech(1_000);
        assert_eq!(av.speech_ms, u64::MAX);
    }
}
