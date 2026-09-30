//! Text translation for the incoming path (§5).
//!
//! This is the module that turns a transcript into the language the user chose.
//! It sits on the *incoming* branch of the product: other players' speech is
//! captured, transcribed, translated, and shown as subtitles. It has no
//! knowledge of TTS or the virtual microphone, which are the outgoing branch
//! and are not implemented.
//!
//! # Provider model
//!
//! [`TranslationProvider`] is the seam. The shipped adapter speaks the
//! **OpenAI-compatible** `/chat/completions` shape, which is not one vendor's
//! API but a de-facto standard implemented by DeepSeek, OpenRouter, Groq,
//! Together, and local runners such as Ollama and LM Studio. One adapter
//! therefore covers both cloud and offline use.
//!
//! # The §15 tension, stated plainly
//!
//! SPEC §15 says the client must never hold a provider API key, because this
//! client is open source and a shipped key is a leaked key. An
//! OpenAI-compatible adapter that the client calls directly does hold one.
//!
//! This is a deliberate, bounded prototype decision, and the boundaries are
//! enforced in code rather than trusted to discipline:
//!
//! * The key is read from an **environment variable only**. There is no config
//!   field for it, so [`crate::config::ClientConfig`] cannot persist one.
//! * It is wrapped in [`openai::ApiKey`], whose `Debug` impl redacts it, so it
//!   cannot reach a log line or a crash report.
//! * A non-local endpoint must be `https://`. Sending a key in clear text to a
//!   remote host is refused, not warned about.
//!
//! The production path remains the gateway from SPEC §15, where the client
//! holds a short-lived session token instead. Swapping to it means implementing
//! [`TranslationProvider`] once; nothing else in the pipeline changes.

pub mod mock;
pub mod openai;
pub mod pipeline;
pub mod prompt;
pub mod worker;

pub use crate::secret::Secret;
pub use mock::{DemoTranslator, ScriptedTranslator};
pub use openai::{ApiKey, OpenAiCompatible, OpenAiConfig};
pub use pipeline::{
    incoming_source, incoming_target, IncomingTranslator, TranslationOutcome, TranslatorStats,
};
pub use prompt::{build_system_prompt, sanitize_model_output};
pub use worker::{TranslationWorker, WorkResult};

use std::time::Duration;

use game_bridge_protocol::Language;
use thiserror::Error;

/// The default environment variable the API key is read from.
pub const DEFAULT_API_KEY_ENV: &str = "GAME_BRIDGE_API_KEY";

/// A translation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationRequest {
    /// The text to translate.
    pub text: String,
    /// Language the text is in.
    pub source: Language,
    /// Language to translate into. The user's selection.
    pub target: Language,
    /// Game context hint, e.g. `"competitive_fps"` (SPEC §5).
    pub context: Option<String>,
}

impl TranslationRequest {
    /// Build a request.
    pub fn new(text: impl Into<String>, source: Language, target: Language) -> Self {
        Self {
            text: text.into(),
            source,
            target,
            context: None,
        }
    }

    /// Attach a game context hint.
    pub fn with_context(mut self, context: Option<String>) -> Self {
        self.context = context;
        self
    }
}

/// A successful translation.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslationResponse {
    /// The translated text.
    pub text: String,
    /// How long the provider took.
    pub latency: Duration,
    /// The model that served it, when the provider reports one.
    pub model: Option<String>,
    /// Tokens the model spent thinking before answering, when reported.
    ///
    /// Zero means thinking is genuinely off. This is a measurement, not a
    /// claim: a reasoning model left on will add hundreds of milliseconds of
    /// latency in front of a subtitle, and the only way to know is to read it
    /// back from the response.
    pub reasoning_tokens: Option<u32>,
}

/// Anything that can go wrong translating.
///
/// `Clone` and `PartialEq` because a [`pipeline::TranslationOutcome`] carries
/// one and outcomes are cloned and compared in the UI and in tests.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TranslateError {
    /// No API key was found in the environment.
    #[error("no API key found; set the {env_var} environment variable")]
    MissingApiKey {
        /// The variable that was consulted.
        env_var: String,
    },

    /// The provider rejected the credentials.
    #[error("the translation provider rejected the API key (HTTP 401/403)")]
    Unauthorized,

    /// The provider is rate limiting.
    #[error("rate limited by the translation provider")]
    RateLimited {
        /// Value of a `Retry-After` header, when the provider sent one.
        retry_after: Option<Duration>,
    },

    /// The provider returned a server error.
    #[error("translation provider error (HTTP {status}): {message}")]
    Server {
        /// HTTP status.
        status: u16,
        /// Message from the provider, truncated.
        message: String,
    },

    /// The response was not the shape the adapter expects.
    #[error("could not read the translation response: {detail}")]
    BadResponse {
        /// What was wrong.
        detail: String,
    },

    /// The request timed out.
    #[error("translation timed out after {after:?}")]
    Timeout {
        /// The configured timeout.
        after: Duration,
    },

    /// The endpoint could not be reached.
    #[error("cannot reach the translation endpoint: {detail}")]
    Connect {
        /// Underlying reason.
        detail: String,
    },

    /// The endpoint is not usable, e.g. plaintext to a remote host.
    #[error("translation endpoint {url} is not usable: {reason}")]
    UnsupportedEndpoint {
        /// The offending URL.
        url: String,
        /// Why it was refused.
        reason: String,
    },

    /// The demo dictionary has no entry for this phrase.
    ///
    /// Distinct from a provider failure: nothing went wrong, the offline demo
    /// simply does not know the phrase. Reporting it as a success with the
    /// input echoed back would be a false claim — the user would see identical
    /// text on both lines and be told it was translated.
    #[error("the demo dictionary has no entry for this phrase")]
    DemoDictionaryMiss,

    /// There was nothing to translate.
    #[error("nothing to translate")]
    EmptyInput,

    /// The input exceeded the configured limit.
    #[error("input is {len} characters, over the {max} character limit")]
    TooLong {
        /// Length of the input.
        len: usize,
        /// The configured limit.
        max: usize,
    },
}

impl TranslateError {
    /// Whether retrying the same request could succeed.
    ///
    /// The pipeline uses this to decide whether to keep the segment queued.
    /// Retrying an auth failure or an over-length input would just fail again.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TranslateError::RateLimited { .. }
                | TranslateError::Server { .. }
                | TranslateError::Timeout { .. }
                | TranslateError::Connect { .. }
        )
    }

    /// Whether the user must change something for this to work.
    ///
    /// These are the failures worth interrupting the user about (§40); the rest
    /// are transient and the client should simply keep showing the original.
    pub fn needs_user_action(&self) -> bool {
        matches!(
            self,
            TranslateError::MissingApiKey { .. }
                | TranslateError::Unauthorized
                | TranslateError::UnsupportedEndpoint { .. }
                | TranslateError::TooLong { .. }
        )
    }

    /// Short, safe message for the UI.
    pub fn user_message(&self) -> String {
        match self {
            TranslateError::MissingApiKey { env_var } => format!(
                "No translation API key. Set {env_var} and restart, or use demo mode."
            ),
            TranslateError::Unauthorized => {
                "The translation provider rejected the API key. Check it and restart.".to_string()
            }
            TranslateError::RateLimited { .. } => {
                "Translation is rate limited. Retrying shortly.".to_string()
            }
            TranslateError::Server { status, .. } => {
                format!("Translation service error ({status}). Retrying.")
            }
            TranslateError::BadResponse { .. } => {
                "The translation service returned an unexpected response.".to_string()
            }
            TranslateError::Timeout { .. } => {
                "Translation timed out. The original text is shown instead.".to_string()
            }
            TranslateError::Connect { .. } => {
                "Cannot reach the translation service. Check the endpoint in Advanced settings."
                    .to_string()
            }
            TranslateError::UnsupportedEndpoint { reason, .. } => reason.clone(),
            TranslateError::DemoDictionaryMiss => {
                "The demo dictionary has no entry for this phrase. It knows a handful of \
                 callouts only; use a real provider for anything else."
                    .to_string()
            }
            TranslateError::EmptyInput => "Nothing to translate.".to_string(),
            TranslateError::TooLong { len, max } => {
                format!("That segment is too long to translate ({len} of {max} characters).")
            }
        }
    }
}

/// Something that can translate text.
///
/// Implementations must be `Send + Sync` so a translation can run on a worker
/// thread while the UI keeps painting.
pub trait TranslationProvider: Send + Sync {
    /// Translate a request.
    fn translate(&self, request: &TranslationRequest)
        -> Result<TranslationResponse, TranslateError>;

    /// A short description for the Advanced settings screen, e.g.
    /// `"deepseek-chat via api.deepseek.com"`.
    ///
    /// Must never contain credentials.
    fn describe(&self) -> String;
}

impl std::fmt::Debug for dyn TranslationProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranslationProvider")
            .field("description", &self.describe())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_can_carry_context() {
        let request = TranslationRequest::new("ศัตรูอยู่ข้างหลัง", Language::Thai, Language::English)
            .with_context(Some("competitive_fps".into()));
        assert_eq!(request.context.as_deref(), Some("competitive_fps"));
        assert_eq!(request.target, Language::English);
    }

    #[test]
    fn a_request_defaults_to_no_context() {
        let request = TranslationRequest::new("test", Language::Thai, Language::English);
        assert_eq!(request.context, None);
    }

    #[test]
    fn transient_failures_are_retryable() {
        assert!(TranslateError::RateLimited { retry_after: None }.is_retryable());
        assert!(TranslateError::Timeout {
            after: Duration::from_secs(20)
        }
        .is_retryable());
        assert!(TranslateError::Connect {
            detail: "connection refused".into()
        }
        .is_retryable());
        assert!(TranslateError::Server {
            status: 503,
            message: "unavailable".into()
        }
        .is_retryable());
    }

    #[test]
    fn permanent_failures_are_not_retryable() {
        // Retrying these would fail identically and burn the user's time.
        assert!(!TranslateError::Unauthorized.is_retryable());
        assert!(!TranslateError::MissingApiKey {
            env_var: DEFAULT_API_KEY_ENV.into()
        }
        .is_retryable());
        assert!(!TranslateError::EmptyInput.is_retryable());
        assert!(
            !TranslateError::DemoDictionaryMiss.is_retryable(),
            "retrying an unknown phrase cannot help"
        );
        assert!(!TranslateError::TooLong { len: 10, max: 5 }.is_retryable());
        assert!(!TranslateError::BadResponse {
            detail: "no choices".into()
        }
        .is_retryable());
    }

    #[test]
    fn only_actionable_failures_ask_the_user_to_do_something() {
        // A transient timeout must not raise a dialog; a missing key must.
        assert!(TranslateError::MissingApiKey {
            env_var: DEFAULT_API_KEY_ENV.into()
        }
        .needs_user_action());
        assert!(TranslateError::Unauthorized.needs_user_action());
        assert!(TranslateError::UnsupportedEndpoint {
            url: "http://example.com".into(),
            reason: "plaintext".into()
        }
        .needs_user_action());

        assert!(!TranslateError::Timeout {
            after: Duration::from_secs(20)
        }
        .needs_user_action());
        assert!(!TranslateError::RateLimited { retry_after: None }.needs_user_action());
    }

    #[test]
    fn the_missing_key_message_names_the_variable_to_set() {
        let error = TranslateError::MissingApiKey {
            env_var: "MY_KEY".into(),
        };
        assert!(error.user_message().contains("MY_KEY"));
        assert!(error.to_string().contains("MY_KEY"));
    }

    #[test]
    fn user_messages_are_short_enough_for_a_banner() {
        // These render inline in the UI, not in a scrollable log.
        let errors = [
            TranslateError::Unauthorized,
            TranslateError::RateLimited { retry_after: None },
            TranslateError::Server {
                status: 500,
                message: "internal".into(),
            },
            TranslateError::BadResponse {
                detail: "x".into(),
            },
            TranslateError::Timeout {
                after: Duration::from_secs(20),
            },
            TranslateError::Connect {
                detail: "refused".into(),
            },
            TranslateError::EmptyInput,
            TranslateError::DemoDictionaryMiss,
            TranslateError::TooLong { len: 9, max: 5 },
        ];
        for error in errors {
            let message = error.user_message();
            assert!(!message.is_empty(), "{error:?}");
            assert!(message.len() < 140, "{error:?} message is too long: {message}");
            // A user-facing string should read as a sentence.
            assert!(message.ends_with('.'), "{error:?}: {message}");
        }
    }

    #[test]
    fn the_provider_trait_object_has_a_safe_debug() {
        // A provider that logged its key would be a credential leak; the trait's
        // Debug goes through `describe`, which is documented to exclude secrets.
        let provider: Box<dyn TranslationProvider> = Box::new(DemoTranslator::new(
            Language::Thai,
            Language::English,
        ));
        let debug = format!("{provider:?}");
        assert!(debug.contains("TranslationProvider"));
        assert!(!debug.contains("Bearer"));
    }
}