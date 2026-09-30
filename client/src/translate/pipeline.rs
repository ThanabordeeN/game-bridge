//! The incoming translation pipeline: transcript in, subtitle out.
//!
//! This is the piece that joins the two halves of the incoming branch — a
//! transcript from STT on one side, a subtitle line for the overlay and the feed
//! on the other — and it is deliberately the only place that decides what
//! happens when translation fails.
//!
//! # Failure policy
//!
//! When a translation fails, the **original text is kept**. A player who reads
//! an untranslated callout is inconvenienced; a player who sees an empty
//! subtitle or a blank line during a firefight has lost information they cannot
//! recover. So [`TranslationOutcome::display_text`] always returns something
//! readable, and the error travels alongside it rather than replacing it.
//!
//! This is also why the outcome carries the error rather than returning a
//! `Result`: a failure here is a *degraded success*, not a failed operation, and
//! modelling it as `Result` would tempt every caller to discard the original.

use std::sync::Arc;
use std::time::{Duration, Instant};

use game_bridge_protocol::Language;

use crate::LanguagePair;

use super::prompt::sanitize_model_output;
use super::{TranslateError, TranslationProvider, TranslationRequest};

/// Running counters for the session's translation activity.
///
/// Surfaced in the UI so a user whose translations are quietly failing can see
/// that they are, rather than concluding the game audio is silent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranslatorStats {
    /// Requests attempted, excluding ones skipped before the provider.
    pub attempts: u32,
    /// Requests that produced a translation.
    pub successes: u32,
    /// Requests that failed.
    pub failures: u32,
    /// Failures that were retryable.
    pub retryable_failures: u32,
    /// Segments skipped because there was nothing to translate.
    pub skipped: u32,
    /// Total time spent waiting on the provider.
    pub total_latency: Duration,
}

impl TranslatorStats {
    /// Fraction of attempts that succeeded, in `0.0..=1.0`.
    ///
    /// Zero attempts reports `1.0`: a session that has not translated anything
    /// yet has not failed at anything either, and showing a 0% success rate
    /// before the first call would read as a fault.
    pub fn success_rate(&self) -> f32 {
        if self.attempts == 0 {
            return 1.0;
        }
        self.successes as f32 / self.attempts as f32
    }

    /// Mean provider latency across attempts, or zero with no attempts.
    pub fn mean_latency(&self) -> Duration {
        if self.attempts == 0 {
            return Duration::ZERO;
        }
        self.total_latency / self.attempts
    }

    /// Whether any translation has been attempted.
    pub fn has_activity(&self) -> bool {
        self.attempts > 0
    }

    /// Whether translation is failing often enough to be worth telling the user.
    ///
    /// A single transient failure during a match is normal and not worth
    /// interrupting anyone over; a sustained rate is a real problem.
    pub fn is_degraded(&self) -> bool {
        self.attempts >= 3 && self.success_rate() < 0.5
    }

    /// A one-line summary for the UI.
    pub fn summary(&self) -> String {
        if !self.has_activity() {
            return "No translations yet".to_string();
        }
        format!(
            "{} of {} translated · {:.0} ms average",
            self.successes,
            self.attempts,
            self.mean_latency().as_secs_f64() * 1000.0
        )
    }
}

/// The result of translating one segment.
///
/// Always carries the original. See the module docs for why this is not a
/// `Result`.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationOutcome {
    /// The segment this came from.
    pub segment_id: u64,
    /// The text as transcribed.
    pub original: String,
    /// The translation, when one was produced.
    pub translation: Option<String>,
    /// The failure, when there was one.
    pub error: Option<TranslateError>,
    /// How long the provider took. Zero when nothing was sent.
    pub latency: Duration,
    /// Tokens the model spent thinking, when the provider reports it.
    ///
    /// Threaded all the way to the UI because "thinking is disabled" is a claim
    /// worth checking: a reasoning model left on adds hundreds of milliseconds
    /// in front of a subtitle, and the only way to know it is off is to read the
    /// number back.
    pub reasoning_tokens: Option<u32>,
}

impl TranslationOutcome {
    /// Whether a translation was produced.
    pub fn is_success(&self) -> bool {
        self.translation.is_some()
    }

    /// Whether the translation failed.
    pub fn is_failure(&self) -> bool {
        self.error.is_some()
    }

    /// Whether this outcome represents an actual attempt.
    ///
    /// False for a segment that was skipped, e.g. because there was nothing to
    /// translate. Skipped segments must not count against the success rate.
    pub fn is_attempt(&self) -> bool {
        self.translation.is_some() || self.error.is_some()
    }

    /// The text to show a reader.
    ///
    /// The translation when there is one, otherwise the original. Never empty
    /// when the original was not.
    pub fn display_text(&self) -> &str {
        match &self.translation {
            Some(translated) if !translated.is_empty() => translated,
            _ => &self.original,
        }
    }

    /// Whether the displayed text is a fallback to the original.
    ///
    /// The UI uses this to mark the line, so a reader can tell an untranslated
    /// callout from a translated one.
    pub fn is_showing_original(&self) -> bool {
        !self.is_success()
    }

    /// The failure message, when there is one.
    pub fn error_message(&self) -> Option<String> {
        self.error.as_ref().map(|error| error.user_message())
    }

    /// Whether the failure is worth interrupting the user about.
    pub fn needs_user_action(&self) -> bool {
        self.error
            .as_ref()
            .is_some_and(|error| error.needs_user_action())
    }
}

/// Translates transcripts on the incoming path.
///
/// Holds the provider and the language pair, so a caller hands it text and gets
/// back a displayable line. The provider is behind an `Arc` so a translation can
/// run on a worker thread while the UI keeps painting.
pub struct IncomingTranslator {
    provider: Arc<dyn TranslationProvider>,
    pair: LanguagePair,
    context: Option<String>,
    stats: TranslatorStats,
}

impl IncomingTranslator {
    /// Build a translator for a language pair.
    pub fn new(provider: Arc<dyn TranslationProvider>, pair: LanguagePair) -> Self {
        Self {
            provider,
            pair,
            context: None,
            stats: TranslatorStats::default(),
        }
    }

    /// Attach the game context hint forwarded to the provider (SPEC §5).
    pub fn with_context(mut self, context: Option<String>) -> Self {
        self.context = context.filter(|c| !c.trim().is_empty());
        self
    }

    /// The provider's description, for the Advanced settings screen.
    pub fn provider_description(&self) -> String {
        self.provider.describe()
    }

    /// The language pair in use.
    pub fn pair(&self) -> LanguagePair {
        self.pair
    }

    /// Change the target language. Takes effect on the next translation.
    pub fn set_pair(&mut self, pair: LanguagePair) {
        self.pair = pair;
    }

    /// Change the game context hint.
    pub fn set_context(&mut self, context: Option<String>) {
        self.context = context.filter(|c| !c.trim().is_empty());
    }

    /// Running counters.
    pub fn stats(&self) -> TranslatorStats {
        self.stats
    }

    /// Reset the counters, e.g. when a session starts.
    pub fn reset_stats(&mut self) {
        self.stats = TranslatorStats::default();
    }

    /// Translate one transcript segment.
    ///
    /// Blocking. Callers on the UI thread must run this on a worker; the
    /// pipeline is deliberately synchronous so it stays testable without an
    /// async runtime, and so the worker's ownership of the provider is obvious.
    pub fn translate(&mut self, segment_id: u64, text: &str) -> TranslationOutcome {
        let original = text.trim().to_string();

        // Nothing to translate: do not spend a request, and do not count it as
        // an attempt. Counting it would make the success rate misleading on a
        // session with many empty segments.
        if original.is_empty() {
            self.stats.skipped += 1;
            return TranslationOutcome {
                segment_id,
                original,
                translation: None,
                error: None,
                latency: Duration::ZERO,
                reasoning_tokens: None,
            };
        }

        // Same language on both sides: the answer is the input. Spending a
        // request on it would be pure waste, and a model asked to "translate"
        // Thai into Thai occasionally rewords the callout instead of leaving it
        // alone.
        if self.pair.source == self.pair.target {
            self.stats.skipped += 1;
            return TranslationOutcome {
                segment_id,
                original: original.clone(),
                translation: Some(original),
                error: None,
                latency: Duration::ZERO,
                reasoning_tokens: None,
            };
        }

        // Note the reversal. `LanguagePair` is stated from the user's own point
        // of view: `source` is what they speak and `target` is what teammates
        // hear. Incoming audio is the opposite direction — teammates speak
        // `target`, and the user reads `source`. Getting this backwards is
        // silent: the provider happily translates English into English and the
        // user sees untranslated callouts, which looks like a provider fault
        // rather than a wiring one.
        let request = TranslationRequest::new(
            original.clone(),
            incoming_source(self.pair),
            incoming_target(self.pair),
        )
        .with_context(self.context.clone());

        self.stats.attempts += 1;
        // Wall-clock, not the provider's self-reported figure: what the user
        // waits for is the round trip, including the network, and a provider
        // reporting its own internal timing would understate it.
        let started = Instant::now();
        let result = self.provider.translate(&request);
        let elapsed = started.elapsed();

        match result {
            Ok(response) => {
                // The provider is responsible for its own cleanup, but a
                // provider that forgets would put quotes or a "Translation:"
                // label in the overlay. Sanitizing here is the single guarantee
                // every provider inherits; it is idempotent, so a provider that
                // already cleaned its output is unaffected.
                let cleaned = sanitize_model_output(&response.text);

                if cleaned.is_empty() {
                    // The model had nothing translatable to say (SPEC §5 rule
                    // 6). Fall back to the original rather than rendering a
                    // blank subtitle.
                    self.stats.failures += 1;
                    return TranslationOutcome {
                        segment_id,
                        original,
                        translation: None,
                        error: Some(TranslateError::BadResponse {
                            detail: "the provider returned nothing translatable".to_string(),
                        }),
                        latency: elapsed,
                        reasoning_tokens: response.reasoning_tokens,
                    };
                }

                self.stats.successes += 1;
                self.stats.total_latency += elapsed;
                TranslationOutcome {
                    segment_id,
                    original,
                    translation: Some(cleaned),
                    error: None,
                    latency: elapsed,
                    reasoning_tokens: response.reasoning_tokens,
                }
            }
            Err(error) => {
                self.stats.failures += 1;
                if error.is_retryable() {
                    self.stats.retryable_failures += 1;
                }
                self.stats.total_latency += elapsed;
                // The original is preserved deliberately: see the module docs.
                TranslationOutcome {
                    segment_id,
                    original,
                    translation: None,
                    error: Some(error),
                    latency: elapsed,
                    reasoning_tokens: None,
                }
            }
        }
    }
}

impl std::fmt::Debug for IncomingTranslator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingTranslator")
            .field("provider", &self.provider.describe())
            .field("source", &self.pair.source)
            .field("target", &self.pair.target)
            .field("context", &self.context)
            .field("stats", &self.stats)
            .finish()
    }
}

/// The language a transcript is expected to be in.
///
/// The incoming branch assumes other players speak the pair's *target*
/// language — the one the user does not — and translates into the user's own
/// language. Exposed as a named function because the two directions are easy to
/// transpose and the resulting bug is subtle: subtitles in the wrong language
/// look like a provider problem.
pub fn incoming_source(pair: LanguagePair) -> Language {
    pair.target
}

/// The language an incoming transcript is translated into.
pub fn incoming_target(pair: LanguagePair) -> Language {
    pair.source
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::mock::{DemoTranslator, ScriptedTranslator};
    use crate::translate::TranslationResponse;

    fn pair() -> LanguagePair {
        // The user speaks Thai; teammates speak English.
        LanguagePair::new(Language::Thai, Language::English)
    }

    fn build(provider: ScriptedTranslator) -> IncomingTranslator {
        IncomingTranslator::new(Arc::new(provider), pair())
    }

    #[test]
    fn a_successful_translation_is_returned_and_counted() {
        let provider = ScriptedTranslator::always("Enemy is behind us.");
        let mut translator = build(provider);

        let outcome = translator.translate(7, "ศัตรูอยู่ข้างหลัง");
        assert_eq!(outcome.segment_id, 7);
        assert_eq!(outcome.original, "ศัตรูอยู่ข้างหลัง");
        assert_eq!(outcome.translation.as_deref(), Some("Enemy is behind us."));
        assert!(outcome.is_success());
        assert!(!outcome.is_failure());
        assert_eq!(outcome.display_text(), "Enemy is behind us.");
        assert!(!outcome.is_showing_original());

        let stats = translator.stats();
        assert_eq!(stats.attempts, 1);
        assert_eq!(stats.successes, 1);
        assert_eq!(stats.failures, 0);
    }

    #[test]
    fn a_failure_keeps_the_original_text_for_display() {
        // The central design decision of this module: a failed translation is a
        // degraded success, not a lost line.
        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::Timeout {
            after: Duration::from_secs(20),
        });
        let mut translator = build(provider);

        let outcome = translator.translate(3, "ศัตรูอยู่ข้างหลัง");
        assert!(outcome.is_failure());
        assert!(!outcome.is_success());
        assert_eq!(outcome.translation, None);
        assert_eq!(outcome.original, "ศัตรูอยู่ข้างหลัง");
        assert_eq!(
            outcome.display_text(),
            "ศัตรูอยู่ข้างหลัง",
            "a reader must still see the callout"
        );
        assert!(outcome.is_showing_original());
        assert!(outcome.error_message().is_some());
    }

    #[test]
    fn reasoning_tokens_are_reported_through_the_pipeline() {
        // The number that proves thinking is off. A provider that reports zero
        // must surface zero, not None, or the UI cannot distinguish "no
        // thinking" from "provider did not say".
        struct ThinkingProvider;
        impl TranslationProvider for ThinkingProvider {
            fn translate(
                &self,
                _request: &TranslationRequest,
            ) -> Result<TranslationResponse, TranslateError> {
                Ok(TranslationResponse {
                    text: "ok".into(),
                    latency: Duration::from_millis(1),
                    model: Some("thinking".into()),
                    reasoning_tokens: Some(0),
                })
            }
            fn describe(&self) -> String {
                "thinking stub".into()
            }
        }

        let mut translator =
            IncomingTranslator::new(Arc::new(ThinkingProvider), pair());
        let outcome = translator.translate(1, "text");
        assert_eq!(outcome.reasoning_tokens, Some(0));
    }

    #[test]
    fn a_skipped_segment_reports_no_reasoning_tokens() {
        let provider = ScriptedTranslator::always("ok");
        let mut translator = build(provider);
        let outcome = translator.translate(1, "   ");
        assert_eq!(outcome.reasoning_tokens, None);
    }

    #[test]
    fn a_failure_is_counted_against_the_success_rate() {
        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::Unauthorized);
        let mut translator = build(provider);

        translator.translate(1, "text");
        let stats = translator.stats();
        assert_eq!(stats.attempts, 1);
        assert_eq!(stats.failures, 1);
        assert_eq!(stats.successes, 0);
        assert_eq!(stats.success_rate(), 0.0);
    }

    #[test]
    fn retryable_failures_are_counted_separately() {
        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::RateLimited { retry_after: None });
        provider.push_err(TranslateError::Unauthorized);
        let mut translator = build(provider);

        translator.translate(1, "a");
        translator.translate(2, "b");
        assert_eq!(translator.stats().retryable_failures, 1);
        assert_eq!(translator.stats().failures, 2);
    }

    #[test]
    fn only_actionable_failures_ask_the_user_to_act() {
        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::Unauthorized);
        let mut translator = build(provider);
        assert!(translator.translate(1, "x").needs_user_action());

        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::Timeout {
            after: Duration::from_secs(20),
        });
        let mut translator = build(provider);
        assert!(
            !translator.translate(1, "x").needs_user_action(),
            "a transient timeout must not raise a dialog"
        );
    }

    #[test]
    fn empty_input_is_skipped_without_a_request() {
        let provider = ScriptedTranslator::always("should not be used");
        let mut translator = build(provider);

        let outcome = translator.translate(1, "   ");
        assert!(!outcome.is_attempt(), "a skip is not an attempt");
        assert_eq!(outcome.translation, None);
        assert_eq!(outcome.error, None);
        assert_eq!(translator.stats().skipped, 1);
        assert_eq!(translator.stats().attempts, 0);
    }

    #[test]
    fn a_skipped_segment_does_not_lower_the_success_rate() {
        // Otherwise a session with many silent segments would look like a
        // failing translation service.
        let provider = ScriptedTranslator::always("ok");
        let mut translator = build(provider);
        translator.translate(1, "");
        translator.translate(2, "   ");
        translator.translate(3, "real text");

        let stats = translator.stats();
        assert_eq!(stats.skipped, 2);
        assert_eq!(stats.attempts, 1);
        assert_eq!(stats.success_rate(), 1.0);
    }

    #[test]
    fn the_provider_is_never_called_for_empty_input() {
        let provider = ScriptedTranslator::always("ok");
        let mut translator = build(provider);
        translator.translate(1, "   ");
        // The provider was moved into an Arc, so assert via the translator's
        // own stats rather than reaching back into it.
        assert_eq!(translator.stats().attempts, 0);
    }

    #[test]
    fn the_context_hint_is_forwarded_to_the_provider() {
        let provider = ScriptedTranslator::always("ok");
        let mut translator =
            build(provider).with_context(Some("competitive_fps".into()));
        translator.translate(1, "text");
        // Stats confirm the request went out; the recorded request is asserted
        // in the provider-level test below via a shared handle.
        assert_eq!(translator.stats().attempts, 1);
    }

    #[test]
    fn the_context_hint_reaches_the_provider_request() {
        let provider = Arc::new(ScriptedTranslator::always("ok"));
        let mut translator =
            IncomingTranslator::new(provider.clone(), pair()).with_context(Some("moba".into()));
        translator.translate(1, "text");

        let recorded = provider.last_request().expect("a request was sent");
        assert_eq!(recorded.context.as_deref(), Some("moba"));
        assert_eq!(recorded.text, "text");
    }

    #[test]
    fn a_blank_context_hint_is_dropped() {
        let provider = Arc::new(ScriptedTranslator::always("ok"));
        let mut translator = IncomingTranslator::new(provider.clone(), pair())
            .with_context(Some("   ".into()));
        translator.translate(1, "text");
        assert_eq!(provider.last_request().unwrap().context, None);
    }

    #[test]
    fn the_request_carries_the_incoming_direction_not_the_pair_direction() {
        // The regression this guards: using the pair's own direction, which
        // asks the provider to translate English into English and silently
        // shows untranslated callouts.
        let provider = Arc::new(ScriptedTranslator::always("ok"));
        let mut translator = IncomingTranslator::new(provider.clone(), pair());
        translator.translate(1, "text");

        let recorded = provider.last_request().unwrap();
        assert_eq!(
            recorded.source,
            Language::English,
            "teammates speak the pair's target language"
        );
        assert_eq!(
            recorded.target,
            Language::Thai,
            "the user reads their own language"
        );
    }

    #[test]
    fn a_same_language_pair_echoes_without_a_request() {
        let provider = Arc::new(ScriptedTranslator::always("should not be used"));
        let same = LanguagePair::new(Language::Thai, Language::Thai);
        let mut translator = IncomingTranslator::new(provider.clone(), same);

        let outcome = translator.translate(1, "ศัตรูอยู่ข้างหลัง");
        assert_eq!(outcome.translation.as_deref(), Some("ศัตรูอยู่ข้างหลัง"));
        assert_eq!(provider.call_count(), 0, "no request should be sent");
        assert_eq!(translator.stats().skipped, 1);
    }

    #[test]
    fn provider_output_is_sanitized_even_if_the_provider_forgot() {
        // A provider that returns a labelled, quoted answer must not put
        // "Translation: " into the overlay.
        let provider = ScriptedTranslator::always("Translation: \"Two are pushing B.\"");
        let mut translator = build(provider);
        let outcome = translator.translate(1, "มันดันบีมาแล้วสอง");
        assert_eq!(outcome.translation.as_deref(), Some("Two are pushing B."));
    }

    #[test]
    fn an_empty_model_answer_falls_back_to_the_original() {
        // SPEC §5 rule 6: the model returns nothing when there is nothing
        // translatable. A blank subtitle is worse than the original.
        let provider = ScriptedTranslator::always("...");
        let mut translator = build(provider);
        let outcome = translator.translate(1, "ศัตรูอยู่ข้างหลัง");

        assert!(outcome.is_failure());
        assert_eq!(outcome.display_text(), "ศัตรูอยู่ข้างหลัง");
        assert_eq!(translator.stats().failures, 1);
    }

    #[test]
    fn the_original_is_trimmed_but_not_altered() {
        let provider = ScriptedTranslator::always("ok");
        let mut translator = build(provider);
        let outcome = translator.translate(1, "  ศัตรูอยู่ข้างหลัง  ");
        assert_eq!(outcome.original, "ศัตรูอยู่ข้างหลัง");
    }

    #[test]
    fn latency_is_accumulated_across_attempts() {
        let provider =
            ScriptedTranslator::always("ok").with_latency(Duration::from_millis(100));
        let mut translator = build(provider);
        translator.translate(1, "a");
        translator.translate(2, "b");

        let stats = translator.stats();
        assert_eq!(stats.attempts, 2);
        assert!(stats.total_latency >= Duration::from_millis(200));
        assert!(stats.mean_latency() >= Duration::from_millis(100));
    }

    #[test]
    fn stats_report_no_activity_before_the_first_translation() {
        let translator = build(ScriptedTranslator::always("ok"));
        let stats = translator.stats();
        assert!(!stats.has_activity());
        assert_eq!(stats.success_rate(), 1.0, "0% before any attempt reads as a fault");
        assert_eq!(stats.mean_latency(), Duration::ZERO);
        assert_eq!(stats.summary(), "No translations yet");
    }

    #[test]
    fn degraded_is_only_reported_after_several_failures() {
        // One transient failure during a match is normal and not worth
        // interrupting anyone over.
        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::Timeout {
            after: Duration::from_secs(20),
        });
        let mut translator = build(provider);
        translator.translate(1, "a");
        translator.translate(2, "b");
        assert!(!translator.stats().is_degraded(), "two failures is too few");

        translator.translate(3, "c");
        assert!(translator.stats().is_degraded());
    }

    #[test]
    fn a_healthy_session_is_not_degraded() {
        let provider = ScriptedTranslator::always("ok");
        let mut translator = build(provider);
        for i in 0..10 {
            translator.translate(i, "text");
        }
        assert!(!translator.stats().is_degraded());
        assert_eq!(translator.stats().success_rate(), 1.0);
    }

    #[test]
    fn the_summary_reads_as_a_sentence() {
        let provider =
            ScriptedTranslator::always("ok").with_latency(Duration::from_millis(200));
        let mut translator = build(provider);
        translator.translate(1, "a");
        translator.translate(2, "b");
        let summary = translator.stats().summary();
        assert!(summary.contains("2 of 2 translated"), "{summary}");
        assert!(summary.contains("ms average"), "{summary}");
    }

    #[test]
    fn resetting_stats_clears_every_counter() {
        let provider = ScriptedTranslator::always("ok");
        let mut translator = build(provider);
        translator.translate(1, "a");
        translator.reset_stats();
        assert_eq!(translator.stats(), TranslatorStats::default());
        assert!(!translator.stats().has_activity());
    }

    #[test]
    fn changing_the_target_language_takes_effect_on_the_next_call() {
        let provider = Arc::new(ScriptedTranslator::always("ok"));
        let mut translator = IncomingTranslator::new(provider.clone(), pair());
        translator.translate(1, "a");
        assert_eq!(
            provider.last_request().unwrap().target,
            Language::Thai,
            "the user reads Thai"
        );

        // Switching the user's own language switches the subtitle language.
        translator.set_pair(LanguagePair::new(Language::Japanese, Language::English));
        translator.translate(2, "b");
        assert_eq!(provider.last_request().unwrap().target, Language::Japanese);
    }

    #[test]
    fn the_pair_can_be_read_back() {
        let translator = build(ScriptedTranslator::always("ok"));
        assert_eq!(translator.pair(), pair());
    }

    #[test]
    fn the_provider_description_is_exposed_for_settings() {
        let translator = IncomingTranslator::new(
            Arc::new(DemoTranslator::new(Language::Thai, Language::English)),
            pair(),
        );
        assert!(translator.provider_description().contains("demo"));
    }

    #[test]
    fn debug_output_does_not_leak_credentials() {
        let translator = build(ScriptedTranslator::always("ok"));
        let debug = format!("{translator:?}");
        assert!(debug.contains("IncomingTranslator"));
        assert!(!debug.contains("Bearer"));
    }

    #[test]
    fn a_full_session_of_callouts_behaves_as_expected() {
        // An end-to-end pass through the pipeline with the demo provider, which
        // is what a user sees in offline mode.
        let provider = Arc::new(DemoTranslator::new(Language::Thai, Language::English));
        let mut translator = IncomingTranslator::new(provider, pair());

        let outcome = translator.translate(0, "ศัตรูอยู่ข้างหลัง");
        assert_eq!(outcome.display_text(), "Enemy is behind us.");

        let outcome = translator.translate(1, "มันดันบีมาแล้วสอง");
        assert_eq!(outcome.display_text(), "Two are pushing B.");

        // An unknown phrase is reported as a miss, and the reader keeps the
        // original rather than being told it was translated.
        let outcome = translator.translate(2, "ข้อความแปลก");
        assert!(outcome.is_failure());
        assert_eq!(outcome.display_text(), "ข้อความแปลก");
        assert_eq!(
            outcome.error,
            Some(TranslateError::DemoDictionaryMiss),
            "a miss is not a provider fault"
        );

        let stats = translator.stats();
        assert_eq!(stats.attempts, 3);
        assert_eq!(stats.successes, 2);
        assert_eq!(stats.failures, 1);
    }

    #[test]
    fn the_incoming_direction_is_the_reverse_of_the_pair() {
        // The user speaks Thai and hears English; teammates speak English and
        // are subtitled in Thai. Transposing these is a subtle bug that looks
        // like a provider fault.
        let pair = pair();
        assert_eq!(incoming_source(pair), Language::English);
        assert_eq!(incoming_target(pair), Language::Thai);
    }
}