//! OpenAI-compatible translation adapter.
//!
//! The endpoint shape is `/chat/completions` with a bearer token, which is not
//! one vendor's API but a de-facto standard. DeepSeek, OpenRouter, Groq,
//! Together, and local runners such as Ollama and LM Studio all implement it,
//! so one adapter covers cloud and offline use.
//!
//! # Credentials
//!
//! See the module-level note in [`super`] for why this prototype holds a key at
//! all. Three rules are enforced here in code:
//!
//! 1. The key is read from an environment variable, never from the config file.
//!    [`crate::config::ClientConfig`] has no field that could store one.
//! 2. [`ApiKey`]'s `Debug` impl redacts it, so it cannot reach a log or a crash
//!    report.
//! 3. A remote endpoint must be `https://`. Sending a key in clear text is
//!    refused outright rather than warned about.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::prompt::{build_system_prompt, sanitize_model_output};
use super::{TranslateError, TranslationProvider, TranslationRequest, TranslationResponse};
use super::DEFAULT_API_KEY_ENV;

/// Longest input accepted, in characters.
///
/// A voice callout is a sentence or two. A request far beyond that is either a
/// mis-segmented stream or a misconfigured client, and truncating it silently
/// would produce a translation of something the user never said. Rejecting it
/// is honest.
pub const MAX_INPUT_CHARS: usize = 2_000;

/// Longest provider error message echoed into the UI or logs.
const MAX_ERROR_MESSAGE_CHARS: usize = 200;

/// An API key.
///
/// An alias for [`crate::secret::Secret`], which redacts its value from `Debug`
/// and `Display`. Named `ApiKey` here because that is what this adapter's
/// documentation calls it.
pub use crate::secret::Secret as ApiKey;

/// Configuration for an OpenAI-compatible endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiConfig {
    /// Base URL, with or without a trailing `/v1`.
    pub base_url: String,
    /// Model name, e.g. `"deepseek-chat"`.
    pub model: String,
    /// Request timeout.
    pub timeout: Duration,
    /// Cap on the generated translation.
    pub max_tokens: u32,
    /// Sampling temperature. Low, because translation is not a creative task
    /// and variance in a subtitle is a defect.
    pub temperature: f32,
    /// Reasoning effort to request, when the model exposes the setting.
    ///
    /// Real-time translation gains nothing from a model thinking before it
    /// answers: the thinking tokens are pure added latency in front of a
    /// subtitle someone is waiting to read. `"none"` disables it.
    ///
    /// Not every OpenAI-compatible endpoint accepts this parameter, so a
    /// rejection is detected and the request retried without it — see
    /// [`OpenAiCompatible::reasoning_unsupported`].
    pub reasoning_effort: Option<String>,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            // DeepSeek is SPEC §5's named default and speaks this shape.
            base_url: "https://api.deepseek.com/v1".to_string(),
            model: "deepseek-chat".to_string(),
            // A translation is a short request; 20s is generous. Waiting longer
            // than the user's patience and then showing a subtitle is worse
            // than showing the original immediately.
            timeout: Duration::from_secs(20),
            max_tokens: 256,
            temperature: 0.2,
            // Off by default: a subtitle is not a reasoning task.
            reasoning_effort: Some("none".to_string()),
        }
    }
}

impl OpenAiConfig {
    /// Configuration for a local OpenAI-compatible server, e.g. Ollama.
    ///
    /// `http://` is permitted here because [`validate_base_url`] allows
    /// plaintext only for loopback addresses.
    pub fn local(port: u16, model: impl Into<String>) -> Self {
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: model.into(),
            ..Default::default()
        }
    }

    /// The full URL to POST to.
    ///
    /// Accepts a base URL with or without `/v1`, and an already-complete
    /// `/chat/completions` URL, because every OpenAI-compatible product
    /// documents it differently and users paste what their provider told them.
    pub fn endpoint_url(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }

    /// The host, for display. Never includes credentials or the path.
    pub fn host(&self) -> String {
        host_of(&self.base_url).to_string()
    }

    /// A description safe to show in the UI and to log.
    pub fn describe(&self) -> String {
        format!("{} via {}", self.model, self.host())
    }

    /// Reject an endpoint that cannot be used safely.
    pub fn validate(&self) -> Result<(), TranslateError> {
        validate_base_url(&self.base_url)
    }
}

/// Refuse an endpoint that would leak the key or cannot work.
///
/// Delegates to [`crate::net::validate_secure_url`], which is shared with the
/// transcription adapter. A security rule implemented twice is a rule that will
/// eventually be implemented differently in the two places.
pub fn validate_base_url(base_url: &str) -> Result<(), TranslateError> {
    crate::net::validate_secure_url(base_url).map_err(|rejection| {
        TranslateError::UnsupportedEndpoint {
            url: rejection.url,
            reason: rejection.reason,
        }
    })
}

/// Extract the host from a URL. See [`crate::net::host_of`].
fn host_of(url: &str) -> &str {
    crate::net::host_of(url)
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    max_tokens: u32,
    stream: bool,
    /// Omitted entirely when unset, so an endpoint that does not know the
    /// parameter never sees it.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokenDetails>,
}

#[derive(Debug, Deserialize)]
struct CompletionTokenDetails {
    /// Tokens the model spent thinking before answering.
    ///
    /// Surfaced so "thinking is disabled" is a measurement rather than a claim:
    /// the UI can show it, and the test asserts it is zero.
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: Option<ChatResponseMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    /// `None` when a provider returns a refusal or a tool call instead of text.
    #[serde(default)]
    content: Option<String>,
}

/// Provider error bodies come in two shapes: an object with a `message`, or a
/// bare string. Both appear in the wild.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ApiErrorBody {
    Structured { error: ApiErrorDetail },
    Plain { error: String },
    Bare(String),
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

impl ApiErrorBody {
    fn message(&self) -> String {
        match self {
            ApiErrorBody::Structured { error } => error
                .message
                .clone()
                .or_else(|| error.code.clone())
                .unwrap_or_else(|| "no message".to_string()),
            ApiErrorBody::Plain { error } => error.clone(),
            ApiErrorBody::Bare(text) => text.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// The provider
// ---------------------------------------------------------------------------

/// A translation provider speaking the OpenAI-compatible chat API.
pub struct OpenAiCompatible {
    config: OpenAiConfig,
    api_key: Option<ApiKey>,
    agent: ureq::Agent,
    /// Set once an endpoint has rejected `reasoning_effort`.
    ///
    /// Without this, a provider that does not implement the parameter would
    /// cost a failed round trip on every single utterance — the request is
    /// retried without it once, and the answer is remembered for the life of
    /// the process.
    reasoning_unsupported: std::sync::atomic::AtomicBool,
}

impl OpenAiCompatible {
    /// Build an adapter, validating the endpoint up front.
    ///
    /// Validating at construction rather than per request means a misconfigured
    /// endpoint is reported once, when the user sets it, instead of on every
    /// utterance.
    pub fn new(config: OpenAiConfig, api_key: Option<ApiKey>) -> Result<Self, TranslateError> {
        config.validate()?;
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(config.timeout))
                // Read the status ourselves so a provider error body can be
                // surfaced instead of a bare "HTTP 400".
                .http_status_as_error(false)
                .user_agent("GameBridge/0.1")
                .build(),
        );
        Ok(Self {
            config,
            api_key,
            agent,
            reasoning_unsupported: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Build from the environment, reading the default variable.
    pub fn from_env(config: OpenAiConfig) -> Result<Self, TranslateError> {
        Self::from_env_var(config, DEFAULT_API_KEY_ENV)
    }

    /// Build from the environment, reading a named variable.
    pub fn from_env_var(config: OpenAiConfig, var: &str) -> Result<Self, TranslateError> {
        let key = ApiKey::from_env(var);
        Self::new(config, key)
    }

    /// The configuration in use.
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// Whether a key is configured.
    pub fn has_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Whether this endpoint has rejected `reasoning_effort`.
    pub fn reasoning_unsupported(&self) -> bool {
        self.reasoning_unsupported
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Send a request and return its status and body.
    ///
    /// Reading the body even for an error response is deliberate: providers put
    /// the actionable detail there, and discarding it produces the useless
    /// "HTTP 400" the user cannot act on.
    fn post(&self, body: &ChatRequest) -> Result<(u16, String), TranslateError> {
        let url = self.config.endpoint_url();
        let mut builder = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");

        if let Some(key) = &self.api_key {
            builder = builder.header("Authorization", &format!("Bearer {}", key.expose()));
        }

        let response = builder
            .send_json(body)
            .map_err(|error| self.map_transport_error(error))?;

        let status = response.status().as_u16();
        let text = response
            .into_body()
            .read_to_string()
            .map_err(|error| self.map_transport_error(error))?;
        Ok((status, text))
    }

    /// Build the request body for a translation.
    ///
    /// Exposed to tests so the wire shape can be asserted without a server.
    fn build_body(&self, request: &TranslationRequest, text: &str) -> ChatRequest {
        let system_prompt =
            build_system_prompt(request.source, request.target, request.context.as_deref());
        ChatRequest {
            model: self.config.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system_prompt,
                },
                ChatMessage {
                    role: "user",
                    content: text.to_string(),
                },
            ],
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            stream: false,
            reasoning_effort: if self.reasoning_unsupported() {
                None
            } else {
                self.config.reasoning_effort.clone()
            },
        }
    }

    /// Map a transport-level failure.
    fn map_transport_error(&self, error: ureq::Error) -> TranslateError {
        match error {
            ureq::Error::Timeout(_) => TranslateError::Timeout {
                after: self.config.timeout,
            },
            ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => TranslateError::Connect {
                detail: error.to_string(),
            },
            ureq::Error::Io(io) => TranslateError::Connect {
                detail: io.to_string(),
            },
            other => TranslateError::Connect {
                detail: other.to_string(),
            },
        }
    }
}

impl std::fmt::Debug for OpenAiCompatible {
    /// Redacts the key.
    ///
    /// Written by hand rather than derived: a derived `Debug` would print the
    /// `ApiKey`, and this struct is exactly the kind of thing that ends up in a
    /// log line when a request fails. `ApiKey`'s own `Debug` also redacts, so
    /// this is defence in depth rather than the only guard.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatible")
            .field("endpoint", &self.config.endpoint_url())
            .field("model", &self.config.model)
            .field("timeout", &self.config.timeout)
            .field(
                "api_key",
                &match &self.api_key {
                    Some(key) => format!("<redacted, {} bytes>", key.len()),
                    None => "none".to_string(),
                },
            )
            .finish()
    }
}

impl TranslationProvider for OpenAiCompatible {
    fn translate(
        &self,
        request: &TranslationRequest,
    ) -> Result<TranslationResponse, TranslateError> {
        let text = request.text.trim();
        if text.is_empty() {
            return Err(TranslateError::EmptyInput);
        }
        let char_count = text.chars().count();
        if char_count > MAX_INPUT_CHARS {
            return Err(TranslateError::TooLong {
                len: char_count,
                max: MAX_INPUT_CHARS,
            });
        }

        // Some local runners accept an empty key; a remote provider does not.
        // The request is still attempted so the provider's own error message is
        // what the user sees, which is more specific than a guess.
        let body = self.build_body(request, text);
        let started = Instant::now();

        let (mut status, mut body_text) = self.post(&body)?;

        // Some OpenAI-compatible endpoints do not implement `reasoning_effort`
        // and reject the whole request. Retry once without it and remember, so
        // the cost is one failed round trip per process rather than one per
        // utterance.
        if status == 400 && body.reasoning_effort.is_some() && mentions_reasoning_effort(&body_text)
        {
            self.reasoning_unsupported
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let retry = self.build_body(request, text);
            let (retry_status, retry_body) = self.post(&retry)?;
            status = retry_status;
            body_text = retry_body;
        }

        let latency = started.elapsed();

        match status {
            200..=299 => {}
            401 | 403 => return Err(TranslateError::Unauthorized),
            429 => {
                return Err(TranslateError::RateLimited {
                    retry_after: None,
                })
            }
            _ => {
                let message = serde_json::from_str::<ApiErrorBody>(&body_text)
                    .map(|body| body.message())
                    .unwrap_or_else(|_| body_text.clone());
                return Err(TranslateError::Server {
                    status,
                    message: truncate(&message, MAX_ERROR_MESSAGE_CHARS),
                });
            }
        }

        let parsed: ChatResponse =
            serde_json::from_str(&body_text).map_err(|error| TranslateError::BadResponse {
                detail: format!("not a chat completion: {error}"),
            })?;

        let raw = parsed
            .choices
            .first()
            .and_then(|choice| choice.message.as_ref())
            .and_then(|message| message.content.as_deref())
            .ok_or_else(|| TranslateError::BadResponse {
                detail: "the response contained no message content".to_string(),
            })?;

        let reasoning_tokens = parsed
            .usage
            .as_ref()
            .and_then(|usage| usage.completion_tokens_details.as_ref())
            .and_then(|details| details.reasoning_tokens);

        Ok(TranslationResponse {
            text: sanitize_model_output(raw),
            latency,
            model: parsed.model,
            reasoning_tokens,
        })
    }

    fn describe(&self) -> String {
        self.config.describe()
    }
}

/// Whether an error body is complaining about `reasoning_effort`.
///
/// Matched on the parameter name appearing anywhere in the message, because
/// providers word the complaint differently ("unsupported_value",
/// "unknown parameter", "not supported by this model") but all name the field.
fn mentions_reasoning_effort(body: &str) -> bool {
    body.to_lowercase().contains("reasoning_effort")
}

/// Truncate a string to `max` characters, appending an ellipsis when cut.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_bridge_protocol::Language;

    // -- endpoint URL normalisation -----------------------------------------

    #[test]
    fn a_bare_host_gets_the_full_path() {
        let config = OpenAiConfig {
            base_url: "https://api.deepseek.com".into(),
            ..Default::default()
        };
        assert_eq!(
            config.endpoint_url(),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn a_v1_base_gets_only_the_operation_appended() {
        let config = OpenAiConfig {
            base_url: "https://api.deepseek.com/v1".into(),
            ..Default::default()
        };
        assert_eq!(
            config.endpoint_url(),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        for base in [
            "https://api.deepseek.com/",
            "https://api.deepseek.com/v1/",
        ] {
            let config = OpenAiConfig {
                base_url: base.into(),
                ..Default::default()
            };
            assert_eq!(
                config.endpoint_url(),
                "https://api.deepseek.com/v1/chat/completions",
                "base {base}"
            );
        }
    }

    #[test]
    fn an_already_complete_url_is_left_alone() {
        // Users paste whatever their provider's docs showed them.
        let config = OpenAiConfig {
            base_url: "https://example.com/v1/chat/completions".into(),
            ..Default::default()
        };
        assert_eq!(
            config.endpoint_url(),
            "https://example.com/v1/chat/completions"
        );
    }

    #[test]
    fn a_local_ollama_endpoint_is_built_correctly() {
        let config = OpenAiConfig::local(11434, "llama3.1");
        assert_eq!(
            config.endpoint_url(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(config.model, "llama3.1");
    }

    #[test]
    fn a_provider_path_prefix_is_preserved() {
        // OpenRouter's base is /api/v1, not /v1.
        let config = OpenAiConfig {
            base_url: "https://openrouter.ai/api/v1".into(),
            ..Default::default()
        };
        assert_eq!(
            config.endpoint_url(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    // -- host extraction -----------------------------------------------------

    #[test]
    fn the_host_is_extracted_without_port_or_path() {
        assert_eq!(host_of("https://api.deepseek.com/v1"), "api.deepseek.com");
        assert_eq!(host_of("http://127.0.0.1:11434/v1"), "127.0.0.1");
        assert_eq!(host_of("https://example.com:8443"), "example.com");
        assert_eq!(host_of("https://example.com"), "example.com");
    }

    #[test]
    fn ipv6_literals_are_extracted() {
        assert_eq!(host_of("http://[::1]:11434/v1"), "::1");
    }

    #[test]
    fn userinfo_is_not_treated_as_the_host() {
        // A URL with embedded credentials must not have them treated as a host,
        // and must not have them echoed in the UI.
        assert_eq!(host_of("https://user:pass@example.com/v1"), "example.com");
    }

    // -- endpoint validation -------------------------------------------------

    #[test]
    fn https_is_accepted() {
        assert!(validate_base_url("https://api.deepseek.com/v1").is_ok());
    }

    #[test]
    fn plaintext_http_is_accepted_for_loopback() {
        // Ollama and LM Studio run locally over http; refusing these would make
        // offline use impossible.
        for url in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://127.0.0.1:8080",
            "http://[::1]:11434/v1",
        ] {
            assert!(validate_base_url(url).is_ok(), "{url} should be allowed");
        }
    }

    #[test]
    fn plaintext_http_is_refused_for_a_remote_host() {
        // The property that matters: an API key must never cross the network in
        // clear text, and the user has no way to notice if it does.
        let error = validate_base_url("http://api.deepseek.com/v1").unwrap_err();
        match error {
            TranslateError::UnsupportedEndpoint { reason, .. } => {
                assert!(reason.contains("clear text"), "{reason}");
            }
            other => panic!("expected UnsupportedEndpoint, got {other:?}"),
        }
    }

    #[test]
    fn a_hostname_that_merely_starts_with_127_is_not_loopback() {
        // "127.0.0.1.evil.com" resolves to a remote host. A prefix check would
        // wrongly allow it.
        assert!(!crate::net::is_loopback_host("127.0.0.1.evil.com"));
        assert!(validate_base_url("http://127.0.0.1.evil.com/v1").is_err());
    }

    #[test]
    fn a_url_without_a_scheme_is_refused() {
        assert!(validate_base_url("api.deepseek.com/v1").is_err());
    }

    #[test]
    fn an_unsupported_scheme_is_refused() {
        for url in ["ftp://example.com", "file:///etc/passwd", "ws://example.com"] {
            assert!(validate_base_url(url).is_err(), "{url} should be refused");
        }
    }

    #[test]
    fn a_url_with_no_host_is_refused() {
        assert!(validate_base_url("https:///v1").is_err());
    }

    #[test]
    fn the_config_validates_its_own_endpoint() {
        let bad = OpenAiConfig {
            base_url: "http://remote.example.com".into(),
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        let good = OpenAiConfig::default();
        assert!(good.validate().is_ok());
    }

    #[test]
    fn constructing_with_a_bad_endpoint_fails_immediately() {
        // Reported once when the user configures it, not on every utterance.
        let bad = OpenAiConfig {
            base_url: "http://remote.example.com".into(),
            ..Default::default()
        };
        assert!(OpenAiCompatible::new(bad, None).is_err());
    }

    // -- credentials ---------------------------------------------------------

    #[test]
    fn an_api_key_never_appears_in_debug_output() {
        let key = ApiKey::new("sk-super-secret-value").unwrap();
        let debug = format!("{key:?}");
        assert!(!debug.contains("sk-super-secret-value"), "{debug}");
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn an_api_key_reports_its_length_without_its_value() {
        let key = ApiKey::new("abcdefghij").unwrap();
        assert_eq!(key.len(), 10);
        assert!(format!("{key:?}").contains("10 bytes"));
    }

    #[test]
    fn blank_keys_are_rejected_rather_than_stored() {
        // An empty variable should read as "no key", not as a 401 from the
        // provider with no explanation.
        assert!(ApiKey::new("").is_none());
        assert!(ApiKey::new("   ").is_none());
        assert!(ApiKey::new("\t\n").is_none());
    }

    #[test]
    fn a_key_is_trimmed_of_surrounding_whitespace() {
        let key = ApiKey::new("  sk-abc  ").unwrap();
        assert_eq!(key.expose(), "sk-abc");
    }

    #[test]
    fn the_provider_description_never_contains_the_key() {
        let config = OpenAiConfig::default();
        let provider =
            OpenAiCompatible::new(config, ApiKey::new("sk-secret")).expect("valid endpoint");
        let described = provider.describe();
        assert!(!described.contains("sk-secret"), "{described}");
        assert!(described.contains("deepseek-chat"));
        assert!(described.contains("api.deepseek.com"));
    }

    #[test]
    fn a_provider_without_a_key_reports_that() {
        let provider = OpenAiCompatible::new(OpenAiConfig::default(), None).unwrap();
        assert!(!provider.has_key());
    }

    // -- input validation ----------------------------------------------------

    #[test]
    fn empty_input_is_refused_before_a_request_is_made() {
        let provider = OpenAiCompatible::new(OpenAiConfig::default(), None).unwrap();
        let request = TranslationRequest::new("   ", Language::Thai, Language::English);
        assert!(matches!(
            provider.translate(&request).unwrap_err(),
            TranslateError::EmptyInput
        ));
    }

    #[test]
    fn over_length_input_is_refused_with_both_numbers() {
        let provider = OpenAiCompatible::new(OpenAiConfig::default(), None).unwrap();
        let long = "a".repeat(MAX_INPUT_CHARS + 1);
        let request = TranslationRequest::new(long, Language::Thai, Language::English);
        match provider.translate(&request).unwrap_err() {
            TranslateError::TooLong { len, max } => {
                assert_eq!(len, MAX_INPUT_CHARS + 1);
                assert_eq!(max, MAX_INPUT_CHARS);
            }
            other => panic!("expected TooLong, got {other:?}"),
        }
    }

    #[test]
    fn the_length_limit_counts_characters_not_bytes() {
        // Thai is three bytes per character in UTF-8. A byte-based limit would
        // reject a legitimate Thai sentence at roughly a third of the stated
        // length.
        let provider = OpenAiCompatible::new(OpenAiConfig::default(), None).unwrap();
        let thai = "ก".repeat(MAX_INPUT_CHARS);
        assert_eq!(thai.len(), MAX_INPUT_CHARS * 3, "sanity: 3 bytes per char");
        let request = TranslationRequest::new(thai, Language::Thai, Language::English);
        // Exactly at the limit, so it must pass the length check and fail later
        // at the network layer instead.
        match provider.translate(&request).unwrap_err() {
            TranslateError::TooLong { .. } => panic!("character limit was counted in bytes"),
            _ => {}
        }
    }

    // -- error body handling -------------------------------------------------

    #[test]
    fn a_structured_provider_error_is_read() {
        let body = r#"{"error":{"message":"Model not found","type":"invalid_request_error"}}"#;
        let parsed: ApiErrorBody = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.message(), "Model not found");
    }

    #[test]
    fn a_string_provider_error_is_read() {
        let body = r#"{"error":"Rate limit exceeded"}"#;
        let parsed: ApiErrorBody = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.message(), "Rate limit exceeded");
    }

    #[test]
    fn an_unstructured_error_body_does_not_panic() {
        // A provider behind a proxy can return HTML.
        let body = "<html><body>502 Bad Gateway</body></html>";
        assert!(serde_json::from_str::<ApiErrorBody>(body).is_err());
    }

    #[test]
    fn error_messages_are_truncated_for_display() {
        let long = "x".repeat(500);
        let truncated = truncate(&long, MAX_ERROR_MESSAGE_CHARS);
        assert_eq!(truncated.chars().count(), MAX_ERROR_MESSAGE_CHARS + 1);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn a_short_error_message_is_not_truncated() {
        assert_eq!(truncate("short", 200), "short");
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        // Slicing by bytes would panic here.
        let thai = "ก".repeat(300);
        let truncated = truncate(&thai, 10);
        assert_eq!(truncated.chars().count(), 11);
    }

    // -- response parsing ----------------------------------------------------

    #[test]
    fn a_normal_chat_completion_parses() {
        let body = r#"{
            "model": "deepseek-chat",
            "choices": [
                {"message": {"role": "assistant", "content": "Enemy is behind us."}}
            ]
        }"#;
        let parsed: ChatResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.model.as_deref(), Some("deepseek-chat"));
        let content = parsed.choices[0].message.as_ref().unwrap().content.as_deref();
        assert_eq!(content, Some("Enemy is behind us."));
    }

    #[test]
    fn a_null_content_is_treated_as_no_content_not_a_panic() {
        // A refusal or a tool call returns null content.
        let body = r#"{"choices":[{"message":{"role":"assistant","content":null}}]}"#;
        let parsed: ChatResponse = serde_json::from_str(body).unwrap();
        let content = parsed.choices[0].message.as_ref().unwrap().content.as_deref();
        assert_eq!(content, None);
    }

    #[test]
    fn an_empty_choices_array_is_handled() {
        let body = r#"{"choices":[]}"#;
        let parsed: ChatResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.choices.first().is_none());
    }

    #[test]
    fn a_response_without_choices_is_handled() {
        let body = r#"{"model":"x"}"#;
        let parsed: ChatResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.choices.is_empty());
    }

    #[test]
    fn a_message_without_a_content_field_is_handled() {
        let body = r#"{"choices":[{"message":{}}]}"#;
        let parsed: ChatResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.choices[0]
            .message
            .as_ref()
            .unwrap()
            .content
            .is_none());
    }

    #[test]
    fn the_request_body_has_the_documented_shape() {
        let provider = OpenAiCompatible::new(OpenAiConfig::default(), None).unwrap();
        let request = TranslationRequest::new("ศัตรูอยู่ข้างหลัง", Language::Thai, Language::English)
            .with_context(Some("competitive_fps".into()));
        let body = provider.build_body(&request, "ศัตรูอยู่ข้างหลัง");
        let json = serde_json::to_value(&body).unwrap();

        assert_eq!(json["model"], "deepseek-chat");
        assert_eq!(json["stream"], false);
        assert_eq!(json["max_tokens"], 256);
        assert!(json["messages"].is_array());
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        assert_eq!(json["messages"][1]["role"], "user");
        assert_eq!(json["messages"][1]["content"], "ศัตรูอยู่ข้างหลัง");
    }

    #[test]
    fn the_default_temperature_is_low() {
        // Translation is not a creative task; variance in a subtitle is a
        // defect, so the default must stay near-deterministic.
        let config = OpenAiConfig::default();
        assert!(config.temperature <= 0.3, "{}", config.temperature);
    }

    #[test]
    fn the_default_timeout_is_bounded() {
        // Waiting longer than a user's patience and then showing a subtitle is
        // worse than showing the original immediately.
        let config = OpenAiConfig::default();
        assert!(config.timeout <= Duration::from_secs(30));
    }
}