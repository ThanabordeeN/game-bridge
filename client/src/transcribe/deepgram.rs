//! The Deepgram adapter (§4).
//!
//! Deepgram's prerecorded endpoint accepts either a URL it fetches itself or
//! raw audio bytes. This adapter uses bytes for real work and exposes the URL
//! form for testing.
//!
//! Two details that differ from an OpenAI-shaped API and are easy to get wrong:
//!
//! * The credential goes in `Authorization: Token <key>`, **not** `Bearer`.
//! * Errors come back as `{"err_code": "...", "err_msg": "..."}`, not the
//!   OpenAI `{"error": {...}}` envelope.
//!
//! # Multilingual
//!
//! With [`TranscriptionLanguage::Auto`] the request sends
//! `language=multi&detect_language=true`. `multi` lets the model switch
//! languages inside one clip; `detect_language` makes it report which one it
//! settled on, which is what the translation stage needs in order to translate
//! *from* the right language.

use std::time::{Duration, Instant};

use serde::Deserialize;

use game_bridge_protocol::Language;

use crate::net::{join_url, validate_secure_url};
use crate::secret::Secret;

use super::{
    AudioInput, TranscribeError, Transcription, TranscriptionLanguage, TranscriptionProvider,
    DEFAULT_DEEPGRAM_KEY_ENV,
};

/// Longest provider error message echoed into the UI or logs.
const MAX_ERROR_MESSAGE_CHARS: usize = 200;

/// Configuration for the Deepgram adapter.
#[derive(Debug, Clone, PartialEq)]
pub struct DeepgramConfig {
    /// Base URL.
    pub base_url: String,
    /// Model name, e.g. `"nova-3"`.
    pub model: String,
    /// Which language to expect.
    pub language: TranscriptionLanguage,
    /// Whether to apply smart formatting: punctuation, numerals, and casing.
    ///
    /// On by default. A subtitle reading "push b two guys" is materially worse
    /// to read at a glance than "Push B. Two guys.", and smart formatting is
    /// what closes that gap.
    pub smart_format: bool,
    /// Whether to add punctuation.
    pub punctuate: bool,
    /// Request timeout.
    pub timeout: Duration,
    /// Environment variable the key is read from.
    pub api_key_env: String,
}

impl Default for DeepgramConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepgram.com/v1".to_string(),
            // SPEC §4 names Nova Multilingual as the default.
            model: "nova-3".to_string(),
            language: TranscriptionLanguage::Auto,
            smart_format: true,
            punctuate: true,
            // Uploading and transcribing a few seconds of audio. Longer than
            // this and the callout is stale anyway.
            timeout: Duration::from_secs(30),
            api_key_env: DEFAULT_DEEPGRAM_KEY_ENV.to_string(),
        }
    }
}

impl DeepgramConfig {
    /// The full URL to POST to, including query parameters.
    pub fn listen_url(&self) -> String {
        let mut params: Vec<String> = vec![
            format!("model={}", self.model),
            format!("smart_format={}", self.smart_format),
            format!("punctuate={}", self.punctuate),
        ];

        match self.language {
            // `multi` enables switching languages within a clip; pairing it with
            // `detect_language` is what makes the response report which language
            // it settled on.
            TranscriptionLanguage::Auto => {
                params.push("language=multi".to_string());
                params.push("detect_language=true".to_string());
            }
            TranscriptionLanguage::Fixed(language) => {
                params.push(format!("language={}", language.code()));
            }
        }

        join_url(&self.base_url, &format!("listen?{}", params.join("&")))
    }

    /// The host, for display. Never includes credentials.
    pub fn host(&self) -> String {
        crate::net::host_of(&self.base_url).to_string()
    }

    /// A description safe to show in the UI and to log.
    pub fn describe(&self) -> String {
        format!("{} via {}", self.model, self.host())
    }

    /// Reject an endpoint that cannot be used safely.
    pub fn validate(&self) -> Result<(), TranscribeError> {
        validate_secure_url(&self.base_url).map_err(|rejection| {
            TranscribeError::UnsupportedEndpoint {
                url: rejection.url,
                reason: rejection.reason,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListenResponse {
    #[serde(default)]
    results: Option<ListenResults>,
    #[serde(default)]
    metadata: Option<ListenMetadata>,
}

#[derive(Debug, Deserialize)]
struct ListenResults {
    #[serde(default)]
    channels: Vec<ListenChannel>,
}

#[derive(Debug, Deserialize)]
struct ListenChannel {
    #[serde(default)]
    alternatives: Vec<ListenAlternative>,
    /// Present only when `detect_language=true`.
    #[serde(default)]
    detected_language: Option<String>,
    #[serde(default)]
    language_confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct ListenAlternative {
    #[serde(default)]
    transcript: String,
    #[serde(default)]
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct ListenMetadata {
    #[serde(default)]
    duration: Option<f32>,
}

/// Deepgram's error envelope, which is not the OpenAI one.
#[derive(Debug, Deserialize)]
struct DeepgramError {
    #[serde(default)]
    err_code: Option<String>,
    #[serde(default)]
    err_msg: Option<String>,
}

impl DeepgramError {
    fn message(&self) -> String {
        self.err_msg
            .clone()
            .or_else(|| self.err_code.clone())
            .unwrap_or_else(|| "no message".to_string())
    }

    /// Whether this is a credential problem rather than a malformed request.
    fn is_auth(&self) -> bool {
        matches!(
            self.err_code.as_deref(),
            Some("INVALID_AUTH") | Some("UNAUTHORIZED") | Some("FORBIDDEN")
        )
    }
}

// ---------------------------------------------------------------------------
// The provider
// ---------------------------------------------------------------------------

/// A Deepgram transcription provider.
pub struct Deepgram {
    config: DeepgramConfig,
    api_key: Option<Secret>,
    agent: ureq::Agent,
}

impl Deepgram {
    /// Build an adapter, validating the endpoint up front.
    pub fn new(config: DeepgramConfig, api_key: Option<Secret>) -> Result<Self, TranscribeError> {
        config.validate()?;
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(config.timeout))
                // Read the status ourselves so the provider's own error message
                // can be surfaced instead of a bare "HTTP 400".
                .http_status_as_error(false)
                // Keeps the TLS session alive between utterances. Measured
                // against Deepgram, a reused connection turns a ~1.7 s
                // transcription of a one-second clip into ~0.35 s, because the
                // handshake dominates the fixed cost.
                .user_agent("GameBridge/0.1")
                .build(),
        );
        Ok(Self {
            config,
            api_key,
            agent,
        })
    }

    /// Build from the environment, reading the configured variable.
    pub fn from_env(config: DeepgramConfig) -> Result<Self, TranscribeError> {
        let key = Secret::from_env(&config.api_key_env);
        Self::new(config, key)
    }

    /// The configuration in use.
    pub fn config(&self) -> &DeepgramConfig {
        &self.config
    }

    /// Whether a key is configured.
    pub fn has_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Map a transport-level failure.
    fn map_transport_error(&self, error: ureq::Error) -> TranscribeError {
        match error {
            ureq::Error::Timeout(_) => TranscribeError::Timeout {
                after: self.config.timeout,
            },
            ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => TranscribeError::Connect {
                detail: error.to_string(),
            },
            ureq::Error::Io(io) => TranscribeError::Connect {
                detail: io.to_string(),
            },
            other => TranscribeError::Connect {
                detail: other.to_string(),
            },
        }
    }

    /// Parse a successful response body.
    fn parse_response(
        &self,
        body: &str,
        started: Instant,
    ) -> Result<Transcription, TranscribeError> {
        let parsed: ListenResponse =
            serde_json::from_str(body).map_err(|error| TranscribeError::BadResponse {
                detail: format!("not a Deepgram listen response: {error}"),
            })?;

        let channel = parsed
            .results
            .as_ref()
            .and_then(|results| results.channels.first())
            .ok_or_else(|| TranscribeError::BadResponse {
                detail: "the response contained no channel".to_string(),
            })?;

        let alternative = channel
            .alternatives
            .first()
            .ok_or_else(|| TranscribeError::BadResponse {
                detail: "the response contained no transcript alternative".to_string(),
            })?;

        // Deepgram reports the language as a BCP-47 code. An unknown code is
        // reported as no detection rather than guessed at, so the caller falls
        // back to its configured source language instead of translating from a
        // language that was invented.
        let detected_language = channel
            .detected_language
            .as_deref()
            .and_then(Language::from_code);

        Ok(Transcription {
            text: alternative.transcript.trim().to_string(),
            confidence: alternative.confidence,
            detected_language,
            language_confidence: channel.language_confidence,
            audio_duration: parsed
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.duration)
                .map(|seconds| Duration::from_secs_f32(seconds.max(0.0))),
            latency: started.elapsed(),
        })
    }
}

impl std::fmt::Debug for Deepgram {
    /// Redacts the key.
    ///
    /// Hand-written rather than derived: this struct is exactly the kind of
    /// thing that ends up in a log line when a request fails.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deepgram")
            .field("endpoint", &self.config.listen_url())
            .field("model", &self.config.model)
            .field("language", &self.config.language)
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

impl TranscriptionProvider for Deepgram {
    fn transcribe(&self, audio: &AudioInput<'_>) -> Result<Transcription, TranscribeError> {
        if audio.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }

        let url = self.config.listen_url();
        let started = Instant::now();

        let mut builder = self.agent.post(&url);

        // Deepgram uses `Token`, not `Bearer`. Getting this wrong produces a
        // 401 that looks like a bad key.
        if let Some(key) = &self.api_key {
            builder = builder.header("Authorization", &format!("Token {}", key.expose()));
        }

        let response = match audio {
            AudioInput::Bytes { data, content_type } => builder
                .header("Content-Type", *content_type)
                .send(*data)
                .map_err(|error| self.map_transport_error(error))?,
            AudioInput::Url(source) => builder
                .header("Content-Type", "application/json")
                .send_json(serde_json::json!({ "url": source }))
                .map_err(|error| self.map_transport_error(error))?,
        };

        let status = response.status().as_u16();
        let body = response
            .into_body()
            .read_to_string()
            .map_err(|error| self.map_transport_error(error))?;

        match status {
            200..=299 => self.parse_response(&body, started),
            401 | 403 => Err(TranscribeError::Unauthorized),
            429 => Err(TranscribeError::RateLimited),
            400 | 415 | 422 => {
                // A malformed or unusable audio payload. Deepgram explains
                // which, and the explanation is the useful part.
                let message = serde_json::from_str::<DeepgramError>(&body)
                    .map(|error| error.message())
                    .unwrap_or_else(|_| body.clone());

                // A 400 can also be a rejected credential in some gateways, so
                // the code is checked rather than assuming it is the audio.
                if serde_json::from_str::<DeepgramError>(&body)
                    .map(|error| error.is_auth())
                    .unwrap_or(false)
                {
                    return Err(TranscribeError::Unauthorized);
                }

                Err(TranscribeError::BadAudio {
                    message: truncate(&message, MAX_ERROR_MESSAGE_CHARS),
                })
            }
            _ => {
                let message = serde_json::from_str::<DeepgramError>(&body)
                    .map(|error| error.message())
                    .unwrap_or_else(|_| body.clone());
                Err(TranscribeError::Server {
                    status,
                    message: truncate(&message, MAX_ERROR_MESSAGE_CHARS),
                })
            }
        }
    }

    fn describe(&self) -> String {
        self.config.describe()
    }
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

/// A provider that always reports itself unavailable.
///
/// Used when no transcription is configured, so the UI can say so plainly
/// instead of silently producing no subtitles.
#[derive(Debug, Default)]
pub struct StubTranscriber;

impl TranscriptionProvider for StubTranscriber {
    fn transcribe(&self, _audio: &AudioInput<'_>) -> Result<Transcription, TranscribeError> {
        Err(TranscribeError::MissingKey {
            env_var: DEFAULT_DEEPGRAM_KEY_ENV.to_string(),
        })
    }

    fn describe(&self) -> String {
        "not configured".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> DeepgramConfig {
        DeepgramConfig::default()
    }

    // -- URL construction ---------------------------------------------------

    #[test]
    fn the_auto_language_request_asks_for_multilingual_detection() {
        // The behaviour the product needs: teammates do not all speak one
        // language, and the response must say which was heard.
        let url = config().listen_url();
        assert!(url.contains("language=multi"), "{url}");
        assert!(url.contains("detect_language=true"), "{url}");
    }

    #[test]
    fn a_pinned_language_replaces_the_multilingual_parameters() {
        let config = DeepgramConfig {
            language: TranscriptionLanguage::Fixed(Language::Thai),
            ..Default::default()
        };
        let url = config.listen_url();
        assert!(url.contains("language=th"), "{url}");
        assert!(!url.contains("language=multi"), "{url}");
        assert!(!url.contains("detect_language"), "{url}");
    }

    #[test]
    fn the_listen_url_targets_the_listen_endpoint() {
        let url = config().listen_url();
        assert!(
            url.starts_with("https://api.deepgram.com/v1/listen?"),
            "{url}"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_base_does_not_double_up() {
        let config = DeepgramConfig {
            base_url: "https://api.deepgram.com/v1/".into(),
            ..Default::default()
        };
        assert!(
            config.listen_url().starts_with("https://api.deepgram.com/v1/listen?"),
            "{}",
            config.listen_url()
        );
    }

    #[test]
    fn the_default_model_is_nova_3() {
        assert_eq!(config().model, "nova-3");
    }

    #[test]
    fn smart_format_is_on_by_default() {
        // "push b two guys" is materially worse to read at a glance than
        // "Push B. Two guys."
        let config = config();
        assert!(config.smart_format);
        assert!(config.punctuate);
        assert!(config.listen_url().contains("smart_format=true"));
    }

    #[test]
    fn the_endpoint_is_validated() {
        let bad = DeepgramConfig {
            base_url: "http://remote.example.com/v1".into(),
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        assert!(config().validate().is_ok());
    }

    #[test]
    fn constructing_with_a_bad_endpoint_fails_immediately() {
        let bad = DeepgramConfig {
            base_url: "http://remote.example.com/v1".into(),
            ..Default::default()
        };
        assert!(Deepgram::new(bad, None).is_err());
    }

    // -- credentials ---------------------------------------------------------

    #[test]
    fn the_key_never_appears_in_debug_output() {
        let provider = Deepgram::new(config(), Secret::new("dg-super-secret")).unwrap();
        let debug = format!("{provider:?}");
        assert!(!debug.contains("dg-super-secret"), "{debug}");
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn the_description_identifies_model_and_host() {
        let provider = Deepgram::new(config(), Secret::new("k")).unwrap();
        let described = provider.describe();
        assert!(described.contains("nova-3"), "{described}");
        assert!(described.contains("api.deepgram.com"), "{described}");
        assert!(!described.contains("k\n"));
    }

    #[test]
    fn a_provider_without_a_key_reports_that() {
        let provider = Deepgram::new(config(), None).unwrap();
        assert!(!provider.has_key());
    }

    // -- input validation ----------------------------------------------------

    #[test]
    fn empty_audio_is_refused_before_a_request() {
        let provider = Deepgram::new(config(), None).unwrap();
        let empty = AudioInput::bytes(&[], "audio/wav");
        assert_eq!(
            provider.transcribe(&empty).unwrap_err(),
            TranscribeError::EmptyAudio
        );
    }

    // -- response parsing ----------------------------------------------------

    /// A response shaped like the one Deepgram returned for the English sample
    /// with `language=multi&detect_language=true`.
    fn real_shaped_response() -> String {
        r#"{
            "metadata": {"request_id": "01a0f1f8-39e1-7622-9ee2-1f9e57faf43b", "duration": 17.566313},
            "results": {
                "channels": [{
                    "alternatives": [{
                        "transcript": "Yep. I said it before, and I'll say it again.",
                        "confidence": 0.99902344,
                        "words": []
                    }],
                    "detected_language": "en",
                    "language_confidence": 0.9881898
                }]
            }
        }"#
        .to_string()
    }

    #[test]
    fn a_real_shaped_response_parses() {
        let provider = Deepgram::new(config(), None).unwrap();
        let result = provider
            .parse_response(&real_shaped_response(), Instant::now())
            .expect("should parse");

        assert!(result.text.starts_with("Yep. I said it before"));
        assert!((result.confidence - 0.999).abs() < 0.001);
        assert_eq!(result.detected_language, Some(Language::English));
        assert!(result.language_confidence.unwrap() > 0.98);
        assert!((result.audio_duration.unwrap().as_secs_f32() - 17.566).abs() < 0.01);
    }

    #[test]
    fn a_detected_language_is_mapped_to_the_protocol_enum() {
        let provider = Deepgram::new(config(), None).unwrap();

        for (code, expected) in [
            ("en", Language::English),
            ("th", Language::Thai),
            ("ja", Language::Japanese),
            ("ko", Language::Korean),
            ("zh", Language::Chinese),
        ] {
            let body = format!(
                r#"{{"results":{{"channels":[{{"alternatives":[{{"transcript":"x","confidence":0.9}}],"detected_language":"{code}"}}]}}}}"#
            );
            let result = provider.parse_response(&body, Instant::now()).unwrap();
            assert_eq!(result.detected_language, Some(expected), "code {code}");
        }
    }

    #[test]
    fn an_unknown_language_code_is_reported_as_no_detection() {
        // Better to fall back to the configured source language than to
        // translate from a language that was invented.
        let provider = Deepgram::new(config(), None).unwrap();
        let body = r#"{"results":{"channels":[{"alternatives":[{"transcript":"x","confidence":0.9}],"detected_language":"xx-unknown"}]}}"#;
        let result = provider.parse_response(body, Instant::now()).unwrap();
        assert_eq!(result.detected_language, None);
    }

    #[test]
    fn a_response_without_detection_reports_none() {
        let provider = Deepgram::new(config(), None).unwrap();
        let body = r#"{"results":{"channels":[{"alternatives":[{"transcript":"x","confidence":0.9}]}]}}"#;
        let result = provider.parse_response(body, Instant::now()).unwrap();
        assert_eq!(result.detected_language, None);
        assert_eq!(result.language_confidence, None);
    }

    #[test]
    fn an_empty_transcript_parses_as_empty_rather_than_failing() {
        // Deepgram returns an empty transcript for silence. That is a valid
        // answer, not an error; the pipeline decides not to translate it.
        let provider = Deepgram::new(config(), None).unwrap();
        let body = r#"{"results":{"channels":[{"alternatives":[{"transcript":"","confidence":0.0}]}]}}"#;
        let result = provider.parse_response(body, Instant::now()).unwrap();
        assert!(result.is_empty());
        assert!(!super::super::is_translatable(&result));
    }

    #[test]
    fn whitespace_around_a_transcript_is_trimmed() {
        let provider = Deepgram::new(config(), None).unwrap();
        let body = r#"{"results":{"channels":[{"alternatives":[{"transcript":"  Push B.  ","confidence":0.9}]}]}}"#;
        let result = provider.parse_response(body, Instant::now()).unwrap();
        assert_eq!(result.text, "Push B.");
    }

    #[test]
    fn a_response_with_no_channels_is_a_bad_response() {
        let provider = Deepgram::new(config(), None).unwrap();
        let error = provider
            .parse_response(r#"{"results":{"channels":[]}}"#, Instant::now())
            .unwrap_err();
        assert!(matches!(error, TranscribeError::BadResponse { .. }));
    }

    #[test]
    fn a_response_with_no_alternatives_is_a_bad_response() {
        let provider = Deepgram::new(config(), None).unwrap();
        let error = provider
            .parse_response(r#"{"results":{"channels":[{"alternatives":[]}]}}"#, Instant::now())
            .unwrap_err();
        assert!(matches!(error, TranscribeError::BadResponse { .. }));
    }

    #[test]
    fn a_non_json_body_is_a_bad_response() {
        let provider = Deepgram::new(config(), None).unwrap();
        assert!(matches!(
            provider
                .parse_response("<html>502</html>", Instant::now())
                .unwrap_err(),
            TranscribeError::BadResponse { .. }
        ));
    }

    #[test]
    fn a_response_missing_optional_fields_still_parses() {
        let provider = Deepgram::new(config(), None).unwrap();
        let body = r#"{"results":{"channels":[{"alternatives":[{"transcript":"hi"}]}]}}"#;
        let result = provider.parse_response(body, Instant::now()).unwrap();
        assert_eq!(result.text, "hi");
        assert_eq!(result.confidence, 0.0);
        assert_eq!(result.audio_duration, None);
    }

    // -- error bodies --------------------------------------------------------

    #[test]
    fn deepgrams_error_envelope_is_read() {
        // This is not the OpenAI shape: err_code/err_msg rather than error.message.
        let body = r#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials.","request_id":"01a0"}"#;
        let parsed: DeepgramError = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.message(), "Invalid credentials.");
        assert!(parsed.is_auth());
    }

    #[test]
    fn a_non_auth_error_code_is_not_treated_as_auth() {
        let body = r#"{"err_code":"BAD_REQUEST","err_msg":"Audio too short"}"#;
        let parsed: DeepgramError = serde_json::from_str(body).unwrap();
        assert!(!parsed.is_auth());
        assert_eq!(parsed.message(), "Audio too short");
    }

    #[test]
    fn an_error_without_a_message_falls_back_to_the_code() {
        let parsed: DeepgramError =
            serde_json::from_str(r#"{"err_code":"SOMETHING"}"#).unwrap();
        assert_eq!(parsed.message(), "SOMETHING");
    }

    #[test]
    fn error_messages_are_truncated_for_display() {
        let long = "x".repeat(500);
        assert_eq!(truncate(&long, 200).chars().count(), 201);
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        // Thai is three bytes per character; byte slicing would panic.
        let thai = "ก".repeat(300);
        assert_eq!(truncate(&thai, 10).chars().count(), 11);
    }

    // -- stub ----------------------------------------------------------------

    #[test]
    fn the_stub_reports_itself_as_unconfigured() {
        let stub = StubTranscriber;
        assert!(stub.transcribe(&AudioInput::bytes(&[0u8; 8], "audio/wav")).is_err());
        assert_eq!(stub.describe(), "not configured");
    }
}