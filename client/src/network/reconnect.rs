//! Reconnection with exponential backoff and jitter (§40).
//!
//! When the connection drops the client pauses translation, keeps the user's
//! microphone patched through, and retries. The retry policy matters more than
//! it looks:
//!
//! * **Exponential backoff** so a gateway outage is not hammered into a
//!   self-inflicted denial of service by every client at once.
//! * **Jitter** for the same reason. Without it, every client that dropped at
//!   the same moment reconnects at the same moment, forever.
//! * **A cap** so a long outage does not leave the client retrying every
//!   twenty minutes, which feels broken when service returns.
//! * **A deadline**, after which the client stops and tells the user rather
//!   than silently retrying forever.

use std::time::Duration;

/// Backoff policy for reconnection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackoffConfig {
    /// Delay before the first retry.
    pub initial: Duration,
    /// Upper bound on any single delay.
    pub max_delay: Duration,
    /// Multiplier applied after each failed attempt.
    pub multiplier: f64,
    /// Fraction of the delay that may be randomly subtracted, in `0.0..=1.0`.
    ///
    /// 0.5 means the actual delay lands uniformly in `[d/2, d]`.
    pub jitter: f64,
    /// Total time to keep retrying before giving up.
    pub give_up_after: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
            jitter: 0.5,
            // Two minutes of retrying covers a gateway restart. Beyond that the
            // honest thing is to stop and show the user an error.
            give_up_after: Duration::from_secs(120),
        }
    }
}

/// A deterministic-in-tests reconnection scheduler.
///
/// The random source is injected rather than global, so the backoff curve can
/// be asserted exactly. A test that cannot pin down the delay cannot catch a
/// regression that makes reconnection hammer the gateway.
#[derive(Debug, Clone)]
pub struct Backoff {
    config: BackoffConfig,
    attempt: u32,
    elapsed: Duration,
}

impl Backoff {
    /// Create a scheduler with the given policy.
    pub fn new(config: BackoffConfig) -> Self {
        Self {
            config,
            attempt: 0,
            elapsed: Duration::ZERO,
        }
    }

    /// Create a scheduler with default policy.
    pub fn with_defaults() -> Self {
        Self::new(BackoffConfig::default())
    }

    /// Attempts made since the last reset.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Total time spent retrying since the last reset.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Reset after a successful connection.
    pub fn reset(&mut self) {
        self.attempt = 0;
        self.elapsed = Duration::ZERO;
    }

    /// Ideal delay for the next attempt, before jitter.
    ///
    /// Computed as `initial * multiplier^(attempt - 1)`, clamped to
    /// `max_delay`. Computed in `f64` because `Duration` has no exponentiation
    /// and the value is clamped immediately, so precision is not a concern.
    pub fn base_delay(&self) -> Duration {
        if self.attempt == 0 {
            return Duration::ZERO;
        }
        let exponent = (self.attempt - 1) as i32;
        let scaled = self.config.initial.as_secs_f64() * self.config.multiplier.powi(exponent);
        let clamped = scaled.min(self.config.max_delay.as_secs_f64());
        Duration::from_secs_f64(clamped)
    }

    /// Delay for the next attempt, given a uniform random value in `0.0..1.0`.
    ///
    /// Jitter subtracts up to `jitter` of the base delay, so the delay lands in
    /// `[(1 - jitter) * base, base]`.
    pub fn next_delay(&self, random: f64) -> Duration {
        let base = self.base_delay();
        if base.is_zero() {
            return base;
        }
        let random = random.clamp(0.0, 1.0);
        let factor = 1.0 - self.config.jitter * random;
        Duration::from_secs_f64((base.as_secs_f64() * factor).max(0.0))
    }

    /// Whether the client has retried long enough and should stop.
    pub fn should_give_up(&self) -> bool {
        self.elapsed >= self.config.give_up_after
    }

    /// Record a failed attempt and advance the schedule.
    ///
    /// `actual` is the delay that was really waited, so `elapsed` reflects wall
    /// time rather than the ideal curve.
    pub fn record_failure(&mut self, actual: Duration) {
        self.attempt = self.attempt.saturating_add(1);
        self.elapsed = self.elapsed.saturating_add(actual);
    }

    /// A human-readable status line for the §40 reconnecting state.
    pub fn status_text(&self) -> String {
        if self.attempt == 0 {
            return "Connecting…".to_string();
        }
        format!("Reconnecting… (attempt {})", self.attempt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deterministic() -> Backoff {
        Backoff::new(BackoffConfig {
            jitter: 0.0,
            ..Default::default()
        })
    }

    #[test]
    fn first_delay_uses_the_initial_value() {
        let mut backoff = deterministic();
        backoff.record_failure(Duration::ZERO);
        assert_eq!(backoff.base_delay(), Duration::from_millis(500));
    }

    #[test]
    fn delays_grow_exponentially_then_cap() {
        let mut backoff = deterministic();
        let mut delays = Vec::new();
        for _ in 0..8 {
            backoff.record_failure(Duration::ZERO);
            delays.push(backoff.base_delay());
        }
        // 0.5, 1, 2, 4, 8, 16, 30 (capped), 30 (capped)
        assert_eq!(delays[0], Duration::from_millis(500));
        assert_eq!(delays[1], Duration::from_secs(1));
        assert_eq!(delays[2], Duration::from_secs(2));
        assert_eq!(delays[3], Duration::from_secs(4));
        assert_eq!(delays[4], Duration::from_secs(8));
        assert_eq!(delays[5], Duration::from_secs(16));
        assert_eq!(delays[6], Duration::from_secs(30), "must cap at max_delay");
        assert_eq!(delays[7], Duration::from_secs(30));
    }

    #[test]
    fn every_delay_is_within_the_maximum() {
        let mut backoff = deterministic();
        for _ in 0..50 {
            backoff.record_failure(Duration::ZERO);
            assert!(backoff.base_delay() <= backoff.config.max_delay);
        }
    }

    #[test]
    fn jitter_spreads_delays_within_the_documented_band() {
        // The property that matters under load: clients that dropped together
        // must not all retry together.
        let mut backoff = Backoff::new(BackoffConfig {
            jitter: 0.5,
            ..Default::default()
        });
        backoff.record_failure(Duration::ZERO);

        let base = backoff.base_delay();
        let lowest = backoff.next_delay(1.0);
        let highest = backoff.next_delay(0.0);

        assert_eq!(highest, base, "random=0 means no subtraction");
        assert_eq!(
            lowest,
            Duration::from_secs_f64(base.as_secs_f64() * 0.5),
            "random=1 means the full jitter fraction"
        );

        // And everything in between stays inside the band.
        for step in 0..=10 {
            let random = step as f64 / 10.0;
            let delay = backoff.next_delay(random);
            assert!(delay <= base);
            assert!(delay >= Duration::from_secs_f64(base.as_secs_f64() * 0.5));
        }
    }

    #[test]
    fn jitter_output_is_monotonic_in_the_random_input() {
        let mut backoff = Backoff::new(BackoffConfig::default());
        backoff.record_failure(Duration::ZERO);
        let mut previous = backoff.next_delay(0.0);
        for step in 1..=20 {
            let delay = backoff.next_delay(step as f64 / 20.0);
            assert!(delay <= previous, "jitter should decrease with random");
            previous = delay;
        }
    }

    #[test]
    fn out_of_range_random_values_are_clamped() {
        let mut backoff = Backoff::new(BackoffConfig::default());
        backoff.record_failure(Duration::ZERO);
        let base = backoff.base_delay();
        assert!(backoff.next_delay(-5.0) <= base);
        assert!(backoff.next_delay(99.0) <= base);
        assert!(backoff.next_delay(f64::NAN).as_secs_f64().is_finite());
    }

    #[test]
    fn giving_up_after_the_deadline() {
        let mut backoff = deterministic();
        assert!(!backoff.should_give_up());
        backoff.record_failure(Duration::from_secs(60));
        assert!(!backoff.should_give_up());
        backoff.record_failure(Duration::from_secs(60));
        assert!(
            backoff.should_give_up(),
            "two minutes of retrying is the limit"
        );
    }

    #[test]
    fn a_successful_connection_resets_the_schedule() {
        let mut backoff = deterministic();
        backoff.record_failure(Duration::from_secs(30));
        backoff.record_failure(Duration::from_secs(60));
        assert_eq!(backoff.attempt(), 2);

        backoff.reset();
        assert_eq!(backoff.attempt(), 0);
        assert_eq!(backoff.elapsed(), Duration::ZERO);
        assert_eq!(backoff.base_delay(), Duration::ZERO);
        assert!(!backoff.should_give_up());
    }

    #[test]
    fn attempt_counter_saturates_instead_of_overflowing() {
        let mut backoff = deterministic();
        for _ in 0..10 {
            backoff.record_failure(Duration::ZERO);
        }
        // Drive the counter to its maximum directly, then one more.
        backoff.attempt = u32::MAX;
        backoff.record_failure(Duration::ZERO);
        assert_eq!(backoff.attempt(), u32::MAX);
    }

    #[test]
    fn status_text_matches_the_reconnecting_ui() {
        let mut backoff = deterministic();
        assert_eq!(backoff.status_text(), "Connecting…");
        backoff.record_failure(Duration::ZERO);
        assert_eq!(backoff.status_text(), "Reconnecting… (attempt 1)");
    }

    #[test]
    fn elapsed_accumulates_actual_waits_not_ideal_ones() {
        let mut backoff = deterministic();
        backoff.record_failure(Duration::from_millis(700));
        backoff.record_failure(Duration::from_millis(1300));
        assert_eq!(backoff.elapsed(), Duration::from_secs(2));
    }
}
