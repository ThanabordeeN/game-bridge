//! Session lifecycle.
//!
//! The session is the unit the user sees in the mini player, the tray, and the
//! Usage screen, and the unit the gateway bills. It is a small state machine
//! with an explicit state enum rather than a pile of booleans, because the UI
//! has to render every state distinctly (§22, §30, §31, §40) and a bool cannot
//! express "connected but waiting for the driver".
//!
//! # States
//!
//! ```text
//!        ┌──────────┐  start   ┌────────────┐  connected  ┌─────────┐
//!        │  Idle    │ ───────► │ Connecting │ ──────────► │  Live   │
//!        └──────────┘          └────────────┘             └─────────┘
//!             ▲                      │                        │
//!             │ stop                 │ failed                 │ lost
//!             │                      ▼                        ▼
//!             │                ┌──────────┐            ┌─────────────┐
//!             └────────────────│  Failed  │            │ Reconnecting│
//!                              └──────────┘            └─────────────┘
//! ```

use std::time::{Duration, Instant};

use game_bridge_protocol::mode::{RoutingMode, VoiceTier};
use game_bridge_protocol::usage::{CreditRate, ServiceTier, UsageUpdate};
use serde::{Deserialize, Serialize};

use crate::error::SessionError;
use crate::routing::bypass::BypassState;
use crate::routing::RoutingPlan;
use crate::LanguagePair;

/// Where a session is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionState {
    /// No session. The Home screen shows the Start button (§21).
    Idle,
    /// Connecting and waiting for the gateway to accept (§40).
    Connecting,
    /// Running normally (§22).
    Live,
    /// The connection dropped and the client is retrying (§40).
    Reconnecting {
        /// Which attempt this is, starting at 1.
        attempt: u32,
        /// Seconds until the next attempt.
        retry_in_secs: u32,
    },
    /// The session could not start or could not continue.
    Failed {
        /// What went wrong, safe to show the user.
        message: String,
        /// Whether the client may retry automatically.
        retryable: bool,
    },
    /// The user stopped the session; the summary is being shown.
    Stopped,
}

impl SessionState {
    /// Whether audio is currently flowing.
    pub const fn is_live(&self) -> bool {
        matches!(self, SessionState::Live)
    }

    /// Whether the session is in a terminal or idle state.
    pub const fn is_stopped(&self) -> bool {
        matches!(
            self,
            SessionState::Idle | SessionState::Stopped | SessionState::Failed { .. }
        )
    }

    /// Label for the status pill in the hero card and mini player.
    pub const fn label(&self) -> &'static str {
        match self {
            SessionState::Idle => "Ready",
            SessionState::Connecting => "Connecting",
            SessionState::Live => "Live",
            SessionState::Reconnecting { .. } => "Reconnecting",
            SessionState::Failed { .. } => "Error",
            SessionState::Stopped => "Stopped",
        }
    }
}

/// The configuration a session runs with.
///
/// `#[serde(default)]` so a config file written by an older build — which may
/// be missing a field added since — still loads instead of failing to start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Languages, in the direction the user speaks.
    pub pair: LanguagePair,
    /// Routing mode (§10).
    pub mode: RoutingMode,
    /// Voice tier (§24).
    pub voice_tier: VoiceTier,
    /// Game context hint for the translation prompt (§5), e.g.
    /// `"competitive_fps"`.
    pub context: Option<String>,
    /// Stable device ID of the microphone.
    pub microphone_id: String,
    /// Stable device ID of the virtual microphone output.
    pub virtual_mic_id: Option<String>,
    /// Stable device ID of the monitoring output.
    pub headphones_id: Option<String>,
    /// Executable name of the captured application, if any.
    pub application: Option<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            pair: LanguagePair::default(),
            mode: RoutingMode::default(),
            voice_tier: VoiceTier::default(),
            context: None,
            microphone_id: String::new(),
            virtual_mic_id: None,
            headphones_id: None,
            application: None,
        }
    }
}

/// A running or completed translation session.
#[derive(Debug)]
pub struct Session {
    config: SessionConfig,
    state: SessionState,
    /// When the session started, for wall-clock accounting.
    started_at: Option<Instant>,
    /// Accumulated wall-clock across reconnects.
    elapsed: Duration,
    /// Billing rates in force, from `session.started`.
    rates: Vec<CreditRate>,
    /// Running usage, authoritative copy from `usage.update`.
    usage: UsageUpdate,
    /// Bypass state.
    bypass: BypassState,
    /// Wallet balance in minor units.
    balance_minor: i64,
    /// Currency code.
    currency: String,
    /// Monotonic segment counter.
    next_segment_id: u64,
    /// Segments currently open, keyed by segment ID.
    open_segments: Vec<u64>,
}

impl Session {
    /// Create an idle session with the given configuration.
    pub fn new(config: SessionConfig) -> Self {
        let tier = ServiceTier::for_routing_mode(config.mode);
        Self {
            config,
            state: SessionState::Idle,
            started_at: None,
            elapsed: Duration::ZERO,
            rates: Vec::new(),
            usage: UsageUpdate {
                tier,
                ..Default::default()
            },
            bypass: BypassState::inactive(),
            balance_minor: 0,
            currency: "THB".into(),
            next_segment_id: 0,
            open_segments: Vec::new(),
        }
    }

    /// Current configuration.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Current state.
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// Running usage counters.
    pub fn usage(&self) -> &UsageUpdate {
        &self.usage
    }

    /// Bypass state.
    pub fn bypass(&self) -> BypassState {
        self.bypass
    }

    /// Wallet balance in minor units.
    pub fn balance_minor(&self) -> i64 {
        self.balance_minor
    }

    /// Currency code.
    pub fn currency(&self) -> &str {
        &self.currency
    }

    /// The routing plan implied by the current mode.
    pub fn routing_plan(&self) -> RoutingPlan {
        RoutingPlan::for_mode(self.config.mode)
    }

    /// Begin the session. Moves `Idle`/`Stopped` → `Connecting`.
    pub fn start(&mut self) -> Result<(), SessionError> {
        match self.state {
            SessionState::Idle | SessionState::Stopped | SessionState::Failed { .. } => {
                self.state = SessionState::Connecting;
                self.started_at = Some(Instant::now());
                self.elapsed = Duration::ZERO;
                self.usage = UsageUpdate {
                    tier: ServiceTier::for_routing_mode(self.config.mode),
                    ..Default::default()
                };
                self.next_segment_id = 0;
                self.open_segments.clear();
                self.bypass = BypassState::inactive();
                Ok(())
            }
            SessionState::Connecting | SessionState::Live | SessionState::Reconnecting { .. } => {
                Err(SessionError::AlreadyRunning)
            }
        }
    }

    /// The gateway accepted the session. Moves `Connecting` → `Live`.
    pub fn on_connected(&mut self, rates: Vec<CreditRate>) -> Result<(), SessionError> {
        if !matches!(self.state, SessionState::Connecting | SessionState::Reconnecting { .. }) {
            return Err(SessionError::NotRunning);
        }
        // If the gateway prices this tier differently, its table wins: billing
        // authority belongs to the server, not to the client's local guess.
        if let Some(rate) = CreditRate::find(&rates, self.usage.tier) {
            self.currency = rate.currency_code.clone();
        }
        self.rates = rates;
        self.state = SessionState::Live;
        self.bypass = self.bypass.on_forced_release();
        Ok(())
    }

    /// The connection dropped. Moves `Live` → `Reconnecting`, and forces
    /// bypass so the user keeps talking while translation is unavailable.
    pub fn on_disconnected(&mut self, attempt: u32, retry_in: Duration) {
        if self.state.is_live() {
            self.bypass = self.bypass.on_forced();
        }
        self.state = SessionState::Reconnecting {
            attempt,
            retry_in_secs: retry_in.as_secs().min(u32::MAX as u64) as u32,
        };
    }

    /// The session failed in a way the user must resolve.
    pub fn on_failed(&mut self, message: impl Into<String>, retryable: bool) {
        self.state = SessionState::Failed {
            message: message.into(),
            retryable,
        };
        self.bypass = self.bypass.on_forced();
    }

    /// The user stopped the session. Finalizes timing and usage.
    pub fn stop(&mut self) -> Result<(), SessionError> {
        if self.state.is_stopped() {
            return Err(SessionError::NotRunning);
        }
        self.commit_elapsed();
        self.state = SessionState::Stopped;
        self.open_segments.clear();
        Ok(())
    }

    /// Current end-to-end latency in milliseconds, as last reported.
    pub fn latency_ms(&self) -> u32 {
        self.usage.latency_ms
    }

    /// Active-voice duration so far, including the currently open interval.
    ///
    /// The caller passes the current instant so the session stays testable
    /// without a real clock: nothing here reads `Instant::now()` on its own
    /// except [`Session::start`].
    pub fn speech_clock(&self) -> String {
        self.usage.active_voice.format_speech_clock()
    }

    /// Cost so far, formatted for the session card.
    pub fn formatted_cost(&self) -> String {
        self.usage.format_cost()
    }

    /// Allocate the next segment ID.
    pub fn next_segment(&mut self) -> u64 {
        let id = self.next_segment_id;
        self.next_segment_id += 1;
        self.open_segments.push(id);
        id
    }

    /// Close a segment, returning whether it was open.
    pub fn close_segment(&mut self, segment_id: u64) -> bool {
        let before = self.open_segments.len();
        self.open_segments.retain(|&id| id != segment_id);
        self.open_segments.len() != before
    }

    /// Whether any segment is awaiting its final result.
    pub fn has_open_segments(&self) -> bool {
        !self.open_segments.is_empty()
    }

    /// Record a completed speech segment and bill it.
    ///
    /// `speech_ms` is the **detected speech** duration, not the wall-clock
    /// duration of the segment. That distinction is the §29 promise.
    pub fn record_speech(&mut self, speech_ms: u64) {
        self.usage.active_voice.record_speech(speech_ms);
        self.usage.recompute_cost(&self.rates);
    }

    /// Advance the wall-clock session timer.
    pub fn tick(&mut self, now: Instant) {
        if let Some(started) = self.started_at {
            self.elapsed = now.saturating_duration_since(started);
            self.usage.active_voice.session_ms = self.elapsed.as_millis() as u64;
        }
    }

    fn commit_elapsed(&mut self) {
        self.usage.active_voice.session_ms = self.elapsed.as_millis() as u64;
    }

    /// Apply a `usage.update` from the gateway.
    ///
    /// The server's numbers replace the local ones outright rather than being
    /// merged. The client's counters exist only so the UI is not blank between
    /// updates; making them authoritative would mean a patched client could
    /// bill itself less.
    pub fn apply_usage(&mut self, usage: UsageUpdate) {
        self.usage = usage;
    }

    /// Apply a rate table from `session.started`, recomputing the local cost
    /// estimate against the gateway's real prices.
    pub fn apply_rates(&mut self, rates: Vec<CreditRate>) {
        if let Some(rate) = CreditRate::find(&rates, self.usage.tier) {
            self.currency = rate.currency_code.clone();
        }
        self.rates = rates;
        self.usage.recompute_cost(&self.rates);
    }

    /// Apply a `credit.update` from the gateway.
    pub fn apply_balance(&mut self, balance_minor: i64, currency: impl Into<String>) {
        self.balance_minor = balance_minor;
        self.currency = currency.into();
    }

    /// Toggle bypass via the latch.
    pub fn toggle_bypass(&mut self) -> BypassState {
        self.bypass = self.bypass.on_latch_toggle();
        self.bypass
    }

    /// Set bypass from a momentary hotkey press or release.
    pub fn set_momentary_bypass(&mut self, held: bool) -> BypassState {
        self.bypass = if held {
            self.bypass.on_momentary_press()
        } else {
            self.bypass.on_momentary_release()
        };
        self.bypass
    }

    /// Whether the balance is low enough to warn (§40).
    ///
    /// The threshold is the larger of one minute of the current tier and the
    /// gateway-provided value, so a user with two minutes left is told before
    /// they run out mid-sentence rather than after.
    pub fn is_low_credit(&self) -> bool {
        let one_minute = CreditRate::find(&self.rates, self.usage.tier)
            .map(|rate| rate.minor_per_minute)
            .unwrap_or(100);
        self.balance_minor < one_minute.max(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_bridge_protocol::Language;

    fn config() -> SessionConfig {
        SessionConfig {
            pair: LanguagePair::new(Language::Thai, Language::English),
            mode: RoutingMode::VoiceOut,
            ..Default::default()
        }
    }

    fn live_session() -> Session {
        let mut session = Session::new(config());
        session.start().unwrap();
        session.on_connected(CreditRate::launch_defaults()).unwrap();
        session
    }

    #[test]
    fn a_new_session_is_idle() {
        let session = Session::new(config());
        assert_eq!(*session.state(), SessionState::Idle);
        assert_eq!(session.state().label(), "Ready");
        assert!(!session.state().is_live());
    }

    #[test]
    fn starting_moves_to_connecting_then_live() {
        let mut session = Session::new(config());
        session.start().unwrap();
        assert_eq!(*session.state(), SessionState::Connecting);
        assert_eq!(session.state().label(), "Connecting");

        session.on_connected(CreditRate::launch_defaults()).unwrap();
        assert!(session.state().is_live());
        assert_eq!(session.state().label(), "Live");
    }

    #[test]
    fn starting_twice_is_rejected() {
        let mut session = Session::new(config());
        session.start().unwrap();
        assert!(matches!(
            session.start().unwrap_err(),
            SessionError::AlreadyRunning
        ));
    }

    #[test]
    fn stopping_an_idle_session_is_rejected() {
        let mut session = Session::new(config());
        assert!(matches!(
            session.stop().unwrap_err(),
            SessionError::NotRunning
        ));
    }

    #[test]
    fn a_stopped_session_can_be_started_again() {
        let mut session = live_session();
        session.stop().unwrap();
        assert_eq!(*session.state(), SessionState::Stopped);
        session.start().unwrap();
        assert_eq!(*session.state(), SessionState::Connecting);
    }

    #[test]
    fn reconnecting_forces_bypass_so_the_user_keeps_talking() {
        let mut session = live_session();
        session.on_disconnected(1, Duration::from_secs(2));
        assert!(matches!(
            session.state(),
            SessionState::Reconnecting { attempt: 1, .. }
        ));
        assert!(session.bypass().active());
        assert!(session.bypass().is_forced());
    }

    #[test]
    fn reconnecting_restores_the_previous_bypass_state() {
        let mut session = live_session();
        // The user had deliberately bypassed before the drop.
        session.toggle_bypass();
        assert!(session.bypass().is_latched());

        session.on_disconnected(1, Duration::from_secs(1));
        assert!(session.bypass().is_forced());

        session.on_connected(CreditRate::launch_defaults()).unwrap();
        // Forced bypass clears; the user's latch is remembered.
        assert!(session.bypass().is_latched());
    }

    #[test]
    fn connecting_mid_session_out_of_order_is_rejected() {
        let mut session = Session::new(config());
        assert!(matches!(
            session.on_connected(vec![]).unwrap_err(),
            SessionError::NotRunning
        ));
    }

    #[test]
    fn a_failed_session_records_a_retryable_flag() {
        let mut session = Session::new(config());
        session.start().unwrap();
        session.on_failed("Gateway unavailable", true);
        match session.state() {
            SessionState::Failed {
                message, retryable, ..
            } => {
                assert_eq!(message, "Gateway unavailable");
                assert!(*retryable);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(session.state().label(), "Error");
        assert!(session.state().is_stopped());
    }

    #[test]
    fn billing_tier_follows_the_routing_mode() {
        let mut session = Session::new(SessionConfig {
            mode: RoutingMode::Subtitle,
            ..config()
        });
        session.start().unwrap();
        assert_eq!(session.usage().tier, ServiceTier::Subtitle);

        let mut session = Session::new(SessionConfig {
            mode: RoutingMode::FullVoice,
            ..config()
        });
        session.start().unwrap();
        assert_eq!(session.usage().tier, ServiceTier::Voice);
    }

    #[test]
    fn only_detected_speech_is_billed() {
        // The §29 promise, exercised through the session rather than the rate
        // table alone.
        let mut session = live_session();
        session.record_speech(0);
        assert_eq!(session.usage().cost_minor, 0);

        session.record_speech(60_000);
        assert_eq!(session.usage().cost_minor, 250);
        assert_eq!(session.formatted_cost(), "฿2.50");
    }

    #[test]
    fn speech_accumulates_across_segments() {
        let mut session = live_session();
        session.record_speech(30_000);
        session.record_speech(30_000);
        assert_eq!(session.usage().active_voice.speech_ms, 60_000);
        assert_eq!(session.usage().active_voice.segments, 2);
        assert_eq!(session.usage().cost_minor, 250);
    }

    #[test]
    fn wall_clock_is_tracked_but_not_billed() {
        let mut session = live_session();
        let start = Instant::now();
        session.tick(start + Duration::from_secs(600));
        // Ten minutes of session, no speech: no cost.
        assert_eq!(session.usage().active_voice.session_ms, 600_000);
        assert_eq!(session.usage().cost_minor, 0);
    }

    #[test]
    fn server_usage_overrides_local_counters() {
        let mut session = live_session();
        session.record_speech(60_000);
        assert_eq!(session.usage().cost_minor, 250);

        // The gateway says otherwise; the gateway wins.
        let mut authoritative = UsageUpdate {
            tier: ServiceTier::Voice,
            cost_minor: 100,
            ..Default::default()
        };
        authoritative.active_voice.record_speech(24_000);
        session.apply_usage(authoritative);

        assert_eq!(session.usage().cost_minor, 100);
        assert_eq!(session.usage().active_voice.speech_ms, 24_000);
    }

    #[test]
    fn balance_updates_from_the_gateway() {
        let mut session = live_session();
        session.apply_balance(8450, "THB");
        assert_eq!(session.balance_minor(), 8450);
        assert_eq!(session.currency(), "THB");
    }

    #[test]
    fn low_credit_is_detected_before_the_user_runs_out() {
        let mut session = live_session();
        // Voice tier is ฿2.50/min, so ฿1.00 remaining is low.
        session.apply_balance(100, "THB");
        assert!(session.is_low_credit());

        session.apply_balance(8450, "THB");
        assert!(!session.is_low_credit());
    }

    #[test]
    fn segment_ids_are_unique_and_monotonic() {
        let mut session = live_session();
        assert_eq!(session.next_segment(), 0);
        assert_eq!(session.next_segment(), 1);
        assert_eq!(session.next_segment(), 2);
        assert!(session.has_open_segments());
    }

    #[test]
    fn closing_a_segment_removes_it_and_reports_whether_it_was_open() {
        let mut session = live_session();
        let id = session.next_segment();
        assert!(session.close_segment(id));
        assert!(!session.has_open_segments());
        assert!(!session.close_segment(id), "closing twice is a no-op");
        assert!(!session.close_segment(999), "unknown segment");
    }

    #[test]
    fn starting_a_new_session_resets_counters_and_segments() {
        let mut session = live_session();
        session.next_segment();
        session.record_speech(60_000);
        session.stop().unwrap();
        session.start().unwrap();

        assert_eq!(session.usage().cost_minor, 0);
        assert_eq!(session.usage().active_voice.speech_ms, 0);
        assert!(!session.has_open_segments());
        assert_eq!(session.next_segment(), 0);
    }

    #[test]
    fn bypass_latch_survives_a_reconnect_and_toggles_correctly() {
        let mut session = live_session();
        assert!(!session.bypass().active());
        let latched = session.toggle_bypass();
        assert!(latched.is_latched());
        assert!(session.bypass().active());
        let off = session.toggle_bypass();
        assert!(!off.active());
    }

    #[test]
    fn momentary_bypass_does_not_disturb_a_latch() {
        let mut session = live_session();
        session.toggle_bypass(); // latch on
        session.set_momentary_bypass(true);
        assert!(session.bypass().is_latched());
        session.set_momentary_bypass(false);
        assert!(session.bypass().is_latched(), "release must not clear the latch");
    }

    #[test]
    fn the_routing_plan_follows_the_configured_mode() {
        let session = Session::new(SessionConfig {
            mode: RoutingMode::Subtitle,
            ..config()
        });
        let plan = session.routing_plan();
        assert!(!plan.needs_microphone());
        assert!(plan.needs_application_capture());
    }

    #[test]
    fn state_serializes_for_the_ui() {
        let json = serde_json::to_string(&SessionState::Reconnecting {
            attempt: 2,
            retry_in_secs: 5,
        })
        .unwrap();
        assert!(json.contains(r#""state":"reconnecting""#));
        assert!(json.contains(r#""attempt":2"#));
    }
}
