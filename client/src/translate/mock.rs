//! Deterministic translation providers for tests and offline use.
//!
//! Two implementations, for two different jobs:
//!
//! * [`ScriptedTranslator`] returns a pre-programmed sequence of results. Tests
//!   use it to drive the pipeline through success, failure, and retry without a
//!   network.
//! * [`DemoTranslator`] is a tiny fixed dictionary that lets the UI and the
//!   whole incoming pipeline run with no API key at all. It is **not** a
//!   translation engine, and the UI labels it as such — see the type's docs for
//!   why that labelling is load-bearing.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use game_bridge_protocol::Language;

use super::{TranslateError, TranslationProvider, TranslationRequest, TranslationResponse};

/// A provider that returns pre-programmed results in order.
///
/// Records every request it receives so tests can assert what the pipeline
/// actually asked for — which is how the "context is forwarded" and "empty
/// input is never sent" properties are checked.
#[derive(Debug, Default)]
pub struct ScriptedTranslator {
    /// Results to return, consumed in order.
    responses: Mutex<VecDeque<Result<String, TranslateError>>>,
    /// Requests received, in order.
    requests: Mutex<Vec<TranslationRequest>>,
    /// Latency reported *and simulated* for each response.
    latency: Duration,
}

impl ScriptedTranslator {
    /// A provider with no scripted responses.
    ///
    /// Any call returns an error, which is the right default: a test that
    /// forgot to script a response should fail loudly rather than silently
    /// receive a plausible-looking translation.
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            // Zero by default so the suite stays fast; `with_latency` makes the
            // provider genuinely take the time.
            latency: Duration::ZERO,
        }
    }

    /// A provider that always succeeds, returning `text` for every request.
    pub fn always(text: impl Into<String>) -> Self {
        let translator = Self::new();
        {
            let mut responses = translator.responses.lock().expect("lock");
            // Unbounded: the same answer for any number of calls.
            responses.push_back(Ok(text.into()));
        }
        translator
    }

    /// Queue a successful response.
    pub fn push_ok(&self, text: impl Into<String>) {
        self.responses
            .lock()
            .expect("lock")
            .push_back(Ok(text.into()));
    }

    /// Queue a failure.
    pub fn push_err(&self, error: TranslateError) {
        self.responses.lock().expect("lock").push_back(Err(error));
    }

    /// Set the latency this provider simulates and reports.
    ///
    /// It genuinely sleeps for the duration. The pipeline measures wall-clock
    /// time rather than trusting a provider's self-reported figure, so a
    /// provider that returned instantly while *claiming* latency would let a
    /// latency test pass without testing anything.
    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Every request received, in order.
    pub fn requests(&self) -> Vec<TranslationRequest> {
        self.requests.lock().expect("lock").clone()
    }

    /// How many requests have been received.
    pub fn call_count(&self) -> usize {
        self.requests.lock().expect("lock").len()
    }

    /// The most recent request.
    pub fn last_request(&self) -> Option<TranslationRequest> {
        self.requests.lock().expect("lock").last().cloned()
    }
}

impl TranslationProvider for ScriptedTranslator {
    fn translate(
        &self,
        request: &TranslationRequest,
    ) -> Result<TranslationResponse, TranslateError> {
        self.requests.lock().expect("lock").push(request.clone());

        // Reuse the last scripted response when the queue runs dry, so a test
        // that calls repeatedly with one scripted answer behaves predictably.
        // Simulate the wait so the caller's measured latency is real.
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency);
        }

        let mut responses = self.responses.lock().expect("lock");
        let result = if responses.len() > 1 {
            responses.pop_front()
        } else {
            responses.front().cloned()
        };

        match result {
            Some(Ok(text)) => Ok(TranslationResponse {
                text,
                latency: self.latency,
                model: Some("scripted".to_string()),
                reasoning_tokens: None,
            }),
            Some(Err(error)) => Err(error),
            None => Err(TranslateError::BadResponse {
                detail: "ScriptedTranslator was not given a response".to_string(),
            }),
        }
    }

    fn describe(&self) -> String {
        "scripted (tests)".to_string()
    }
}

/// A fixed-dictionary translator for offline demonstration.
///
/// # This is not translation
///
/// It looks up a short phrase in a hardcoded table and returns the entry, or
/// echoes the input unchanged. It cannot generalise, it knows a handful of
/// phrases, and it will be wrong for anything else.
///
/// It exists so the incoming pipeline — segmentation, the feed, the overlay,
/// timing, error paths — can be exercised end to end with no API key and no
/// network. Its [`TranslationProvider::describe`] says `"demo dictionary (not
/// real translation)"` and the UI is expected to show that string, because a
/// user who mistakes this for working translation would draw exactly the wrong
/// conclusion about the product.
#[derive(Debug, Clone)]
pub struct DemoTranslator {
    source: Language,
    target: Language,
}

impl DemoTranslator {
    /// Create a demo translator for a language pair.
    pub fn new(source: Language, target: Language) -> Self {
        Self { source, target }
    }

    /// Look up a phrase, case- and whitespace-insensitively.
    fn lookup(&self, text: &str) -> Option<&'static str> {
        let normalized = normalize(text);
        let thai_to_english = self.source == Language::Thai && self.target == Language::English;
        let english_to_thai = self.source == Language::English && self.target == Language::Thai;

        for (thai, english) in PHRASES {
            if thai_to_english && normalize(thai) == normalized {
                return Some(english);
            }
            if english_to_thai && normalize(english) == normalized {
                return Some(thai);
            }
        }
        None
    }
}

/// Lowercase, collapse whitespace, and strip trailing punctuation so a lookup
/// is not defeated by a stray full stop.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', '!', '?', '。', '！', '？'])
        .to_lowercase()
}

/// The demo phrase table: common competitive callouts, Thai and English.
const PHRASES: &[(&str, &str)] = &[
    ("ศัตรูอยู่ข้างหลัง", "Enemy is behind us."),
    ("มีสองคนอยู่หลังกำแพง", "Two guys behind the wall."),
    ("ดันบี", "Push B."),
    ("มันดันบีมาแล้วสอง", "Two are pushing B."),
    ("ระวังสไนเปอร์", "Watch the sniper."),
    ("ฉันตายแล้ว", "I'm down."),
    ("ช่วยด้วย", "Need help."),
    ("ไปเอ", "Go A."),
    ("ไปบี", "Go B."),
    ("รีโหลด", "Reloading."),
    ("ข้างหลังเรา", "Behind us."),
    ("ถอย", "Fall back."),
    ("รอตรงนี้", "Hold here."),
    ("มีคนอยู่บนหลังคา", "Someone's on the roof."),
    ("ใช้ระเบิด", "Use the grenade."),
];

impl TranslationProvider for DemoTranslator {
    fn translate(
        &self,
        request: &TranslationRequest,
    ) -> Result<TranslationResponse, TranslateError> {
        let text = request.text.trim();
        if text.is_empty() {
            return Err(TranslateError::EmptyInput);
        }

        // A miss is reported as a miss. Returning the input unchanged with a
        // success status would tell the caller "translated" about text that was
        // not translated, and the user would see the same line twice with no
        // indication that nothing happened.
        let translated = self
            .lookup(text)
            .ok_or(TranslateError::DemoDictionaryMiss)?;

        Ok(TranslationResponse {
            text: translated.to_string(),
            latency: Duration::from_millis(2),
            model: Some("demo".to_string()),
            reasoning_tokens: None,
        })
    }

    fn describe(&self) -> String {
        "demo dictionary (not real translation)".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scripted_provider_returns_its_responses_in_order() {
        let provider = ScriptedTranslator::new();
        provider.push_ok("first");
        provider.push_ok("second");

        let request = TranslationRequest::new("x", Language::Thai, Language::English);
        assert_eq!(provider.translate(&request).unwrap().text, "first");
        assert_eq!(provider.translate(&request).unwrap().text, "second");
        // The queue is exhausted, so the last answer repeats.
        assert_eq!(provider.translate(&request).unwrap().text, "second");
    }

    #[test]
    fn an_unscripted_provider_fails_loudly() {
        // A test that forgot to script a response must not receive a plausible
        // translation that hides the omission.
        let provider = ScriptedTranslator::new();
        let request = TranslationRequest::new("x", Language::Thai, Language::English);
        let error = provider.translate(&request).unwrap_err();
        assert!(matches!(error, TranslateError::BadResponse { .. }));
        assert!(error.to_string().contains("ScriptedTranslator"));
    }

    #[test]
    fn a_scripted_failure_is_returned_as_is() {
        let provider = ScriptedTranslator::new();
        provider.push_err(TranslateError::RateLimited { retry_after: None });
        let request = TranslationRequest::new("x", Language::Thai, Language::English);
        assert!(matches!(
            provider.translate(&request).unwrap_err(),
            TranslateError::RateLimited { .. }
        ));
    }

    #[test]
    fn requests_are_recorded_for_assertion() {
        let provider = ScriptedTranslator::always("ok");
        let request = TranslationRequest::new("hello", Language::Thai, Language::English)
            .with_context(Some("competitive_fps".into()));
        provider.translate(&request).unwrap();

        assert_eq!(provider.call_count(), 1);
        let recorded = provider.last_request().unwrap();
        assert_eq!(recorded.text, "hello");
        assert_eq!(recorded.context.as_deref(), Some("competitive_fps"));
    }

    #[test]
    fn a_scripted_provider_reports_its_configured_latency() {
        let provider =
            ScriptedTranslator::always("ok").with_latency(Duration::from_millis(250));
        let request = TranslationRequest::new("x", Language::Thai, Language::English);
        assert_eq!(
            provider.translate(&request).unwrap().latency,
            Duration::from_millis(250)
        );
    }

    #[test]
    fn the_demo_translator_handles_a_known_thai_callout() {
        let provider = DemoTranslator::new(Language::Thai, Language::English);
        let request = TranslationRequest::new("ศัตรูอยู่ข้างหลัง", Language::Thai, Language::English);
        assert_eq!(
            provider.translate(&request).unwrap().text,
            "Enemy is behind us."
        );
    }

    #[test]
    fn the_demo_translator_works_in_both_directions() {
        let to_thai = DemoTranslator::new(Language::English, Language::Thai);
        let request = TranslationRequest::new("Push B.", Language::English, Language::Thai);
        assert_eq!(to_thai.translate(&request).unwrap().text, "ดันบี");
    }

    #[test]
    fn the_demo_translator_ignores_case_and_trailing_punctuation() {
        let provider = DemoTranslator::new(Language::English, Language::Thai);
        for variant in ["push b", "Push B.", "PUSH B!", "  Push   B.  "] {
            let request = TranslationRequest::new(variant, Language::English, Language::Thai);
            assert_eq!(
                provider.translate(&request).unwrap().text,
                "ดันบี",
                "variant {variant:?}"
            );
        }
    }

    #[test]
    fn the_demo_translator_reports_a_miss_rather_than_echoing() {
        // The honesty property: a phrase it does not know must not come back
        // with a success status, because that claims a translation happened.
        let provider = DemoTranslator::new(Language::Thai, Language::English);
        let request = TranslationRequest::new(
            "ข้อความที่ไม่มีในตาราง",
            Language::Thai,
            Language::English,
        );
        assert_eq!(
            provider.translate(&request).unwrap_err(),
            TranslateError::DemoDictionaryMiss
        );
    }

    #[test]
    fn the_demo_translator_refuses_empty_input() {
        let provider = DemoTranslator::new(Language::Thai, Language::English);
        let request = TranslationRequest::new("   ", Language::Thai, Language::English);
        assert!(matches!(
            provider.translate(&request).unwrap_err(),
            TranslateError::EmptyInput
        ));
    }

    #[test]
    fn the_demo_translator_says_it_is_not_real_translation() {
        // The label is load-bearing: a user who mistakes the dictionary for
        // working translation draws exactly the wrong conclusion.
        let provider = DemoTranslator::new(Language::Thai, Language::English);
        let described = provider.describe();
        assert!(described.contains("demo"), "{described}");
        assert!(described.contains("not real translation"), "{described}");
    }

    #[test]
    fn every_demo_phrase_pair_is_distinct() {
        // A duplicated source phrase would silently shadow an earlier entry.
        let mut sources: Vec<String> = PHRASES.iter().map(|(th, _)| normalize(th)).collect();
        let count = sources.len();
        sources.sort();
        sources.dedup();
        assert_eq!(sources.len(), count, "a Thai phrase is listed twice");
    }

    #[test]
    fn normalize_strips_punctuation_and_case() {
        assert_eq!(normalize("Push B."), "push b");
        assert_eq!(normalize("  PUSH   B  "), "push b");
        assert_eq!(normalize("Enemy!"), "enemy");
        assert_eq!(normalize("敵。"), "敵");
    }
}