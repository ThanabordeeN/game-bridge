//! Speech-to-text for the incoming path (§4, §7).
//!
//! This is the first stage: captured game audio becomes a transcript, which the
//! translation pipeline then turns into a subtitle.
//!
//! ```text
//! audio ──► TranscriptionProvider ──► transcript ──► IncomingTranslator ──► subtitle
//!           (this module)                             (translate module)
//! ```
//!
//! # Multilingual by default
//!
//! Teammates do not all speak one language. [`TranscriptionLanguage::Auto`]
//! asks the provider to detect the language and switch between them within a
//! single clip, and the detected language is carried out in
//! [`Transcription::detected_language`] so the translation stage translates
//! *from the language that was actually spoken* rather than from a guess.
//!
//! That distinction matters: assuming the source language means a teammate who
//! code-switches, or who speaks a language nobody expected, gets translated as
//! if they had spoken the assumed one.
//!
//! # Why batch rather than streaming
//!
//! This adapter posts one request per utterance. That is the right first shape
//! because it reuses the segmentation the local VAD already does, and because it
//! keeps the whole path synchronous and testable.
//!
//! It has a measurable cost, and it is worth stating plainly: a batch request
//! pays connection and queueing overhead on **every** utterance, where a
//! streaming session pays it once. Measured against Deepgram with a reused TLS
//! connection, a one-second clip transcribes in roughly 0.3–0.5 s; with a fresh
//! connection each time, the same clip costs 1.6–1.7 s. The client keeps one
//! pooled connection per provider, which is what makes the batch shape viable —
//! see `documentation/LATENCY.md`.

pub mod deepgram;

pub use deepgram::{Deepgram, DeepgramConfig};

use std::time::Duration;

use game_bridge_protocol::Language;
use thiserror::Error;

/// The default environment variable the Deepgram key is read from.
pub const DEFAULT_DEEPGRAM_KEY_ENV: &str = "DEEPGRAM_API_KEY";

/// Which language the audio is expected to be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionLanguage {
    /// Detect the language, and allow switching within a clip.
    ///
    /// The default. Teammates in a mixed lobby do not all speak one language,
    /// and a fixed guess turns a language nobody expected into confident
    /// nonsense.
    #[default]
    Auto,
    /// Pin one language.
    ///
    /// More accurate when the language really is known — but only then.
    Fixed(Language),
}

impl TranscriptionLanguage {
    /// Label for the settings screen.
    pub fn label(self) -> String {
        match self {
            TranscriptionLanguage::Auto => "Auto-detect (multilingual)".to_string(),
            TranscriptionLanguage::Fixed(language) => {
                format!("Always {}", language.display_name())
            }
        }
    }

    /// The language to send, if one is pinned.
    pub fn pinned(self) -> Option<Language> {
        match self {
            TranscriptionLanguage::Auto => None,
            TranscriptionLanguage::Fixed(language) => Some(language),
        }
    }
}

/// Audio to transcribe.
#[derive(Debug)]
pub enum AudioInput<'a> {
    /// Raw encoded audio, with its MIME type.
    ///
    /// This is the real path: the client captures audio, encodes it, and posts
    /// the bytes.
    Bytes {
        /// The encoded audio.
        data: &'a [u8],
        /// MIME type, e.g. `"audio/wav"` or `"audio/l16"`.
        content_type: &'a str,
    },
    /// A URL the provider fetches itself.
    ///
    /// Only useful for testing and for transcribing something already hosted.
    Url(&'a str),
}

impl<'a> AudioInput<'a> {
    /// Wrap raw audio bytes.
    pub fn bytes(data: &'a [u8], content_type: &'a str) -> Self {
        AudioInput::Bytes { data, content_type }
    }

    /// How many bytes of audio are being sent. Zero for a URL.
    pub fn len(&self) -> usize {
        match self {
            AudioInput::Bytes { data, .. } => data.len(),
            AudioInput::Url(_) => 0,
        }
    }

    /// Whether there is no audio to send.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A successful transcription.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcription {
    /// The recognised text, already punctuated and formatted if requested.
    pub text: String,
    /// Provider confidence in `0.0..=1.0`.
    pub confidence: f32,
    /// The language the provider believed it heard.
    ///
    /// `None` when the provider was not asked to detect one. This is what the
    /// translation stage should translate *from*.
    pub detected_language: Option<Language>,
    /// Confidence in the detected language, when reported.
    pub language_confidence: Option<f32>,
    /// Duration of the audio that was transcribed.
    pub audio_duration: Option<Duration>,
    /// Wall-clock time for the request, including upload.
    pub latency: Duration,
}

impl Transcription {
    /// Whether the provider returned any words.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Whether the detected language differs from what was expected.
    ///
    /// Useful for the UI: a teammate who suddenly speaks a different language
    /// is worth surfacing, because it changes which language the subtitle is in.
    pub fn differs_from(&self, expected: Language) -> bool {
        self.detected_language
            .is_some_and(|detected| detected != expected)
    }
}

/// Anything that can go wrong transcribing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TranscribeError {
    /// No credential was found in the environment.
    #[error("no transcription key found; set the {env_var} environment variable")]
    MissingKey {
        /// The variable that was consulted.
        env_var: String,
    },

    /// The provider rejected the credential.
    #[error("the transcription provider rejected the key")]
    Unauthorized,

    /// The provider is rate limiting.
    #[error("rate limited by the transcription provider")]
    RateLimited,

    /// The audio was rejected as unusable.
    #[error("the transcription provider rejected the audio: {message}")]
    BadAudio {
        /// Provider explanation, truncated.
        message: String,
    },

    /// The provider returned a server error.
    #[error("transcription provider error (HTTP {status}): {message}")]
    Server {
        /// HTTP status.
        status: u16,
        /// Message from the provider, truncated.
        message: String,
    },

    /// The response was not the shape the adapter expects.
    #[error("could not read the transcription response: {detail}")]
    BadResponse {
        /// What was wrong.
        detail: String,
    },

    /// The request timed out.
    #[error("transcription timed out after {after:?}")]
    Timeout {
        /// The configured timeout.
        after: Duration,
    },

    /// The endpoint could not be reached.
    #[error("cannot reach the transcription endpoint: {detail}")]
    Connect {
        /// Underlying reason.
        detail: String,
    },

    /// The endpoint is not usable, e.g. plaintext to a remote host.
    #[error("transcription endpoint {url} is not usable: {reason}")]
    UnsupportedEndpoint {
        /// The offending URL.
        url: String,
        /// Why it was refused.
        reason: String,
    },

    /// There was no audio to send.
    #[error("no audio to transcribe")]
    EmptyAudio,
}

impl TranscribeError {
    /// Whether retrying the same request could succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TranscribeError::RateLimited
                | TranscribeError::Server { .. }
                | TranscribeError::Timeout { .. }
                | TranscribeError::Connect { .. }
        )
    }

    /// Whether the user must change something for this to work.
    pub fn needs_user_action(&self) -> bool {
        matches!(
            self,
            TranscribeError::MissingKey { .. }
                | TranscribeError::Unauthorized
                | TranscribeError::UnsupportedEndpoint { .. }
        )
    }

    /// Short, safe message for the UI.
    pub fn user_message(&self) -> String {
        match self {
            TranscribeError::MissingKey { env_var } => format!(
                "No speech-to-text key. Set {env_var} and restart to subtitle game audio."
            ),
            TranscribeError::Unauthorized => {
                "The speech-to-text provider rejected the key. Check it and restart.".to_string()
            }
            TranscribeError::RateLimited => {
                "Speech-to-text is rate limited. Retrying shortly.".to_string()
            }
            TranscribeError::BadAudio { message } => {
                // The provider's own wording is interpolated, and it does not
                // reliably end with punctuation.
                format!("The audio could not be transcribed: {}.", message.trim_end_matches('.'))
            }
            TranscribeError::Server { status, .. } => {
                format!("Speech-to-text service error ({status}). Retrying.")
            }
            TranscribeError::BadResponse { .. } => {
                "The speech-to-text service returned an unexpected response.".to_string()
            }
            TranscribeError::Timeout { .. } => "Speech-to-text timed out.".to_string(),
            TranscribeError::Connect { .. } => {
                "Cannot reach the speech-to-text service. Check the endpoint in Advanced \
                 settings."
                    .to_string()
            }
            TranscribeError::UnsupportedEndpoint { reason, .. } => reason.clone(),
            TranscribeError::EmptyAudio => "There was no audio to transcribe.".to_string(),
        }
    }
}

/// Something that can turn audio into text.
pub trait TranscriptionProvider: Send + Sync {
    /// Transcribe a piece of audio.
    fn transcribe(&self, audio: &AudioInput<'_>) -> Result<Transcription, TranscribeError>;

    /// A short description for the settings screen.
    ///
    /// Must never contain credentials.
    fn describe(&self) -> String;
}

impl std::fmt::Debug for dyn TranscriptionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscriptionProvider")
            .field("description", &self.describe())
            .finish()
    }
}

/// Whether a transcript is worth sending to translation.
///
/// Providers emit filler for silence or noise — a bare period, a stray "you".
/// Translating that costs a request and puts a meaningless subtitle on screen.
pub fn is_translatable(transcription: &Transcription) -> bool {
    let text = transcription.text.trim();
    if text.is_empty() {
        return false;
    }
    // Strip punctuation and see whether any word survived.
    let meaningful = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>();
    if meaningful.is_empty() {
        return false;
    }
    // A very short result at low confidence is noise, not speech.
    let confidence_ok = transcription.confidence >= 0.30;
    meaningful.chars().count() >= 2 && confidence_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcription(text: &str, confidence: f32) -> Transcription {
        Transcription {
            text: text.to_string(),
            confidence,
            detected_language: None,
            language_confidence: None,
            audio_duration: None,
            latency: Duration::from_millis(300),
        }
    }

    #[test]
    fn the_default_language_mode_is_auto_detect() {
        // A mixed lobby is the normal case, and a fixed guess turns a language
        // nobody expected into confident nonsense.
        assert_eq!(
            TranscriptionLanguage::default(),
            TranscriptionLanguage::Auto
        );
        assert_eq!(TranscriptionLanguage::Auto.pinned(), None);
    }

    #[test]
    fn a_pinned_language_reports_itself() {
        let pinned = TranscriptionLanguage::Fixed(Language::Thai);
        assert_eq!(pinned.pinned(), Some(Language::Thai));
        assert!(pinned.label().contains("Thai"));
    }

    #[test]
    fn language_mode_labels_are_distinguishable() {
        assert!(TranscriptionLanguage::Auto.label().contains("Auto"));
        assert!(TranscriptionLanguage::Auto.label().contains("multilingual"));
    }

    #[test]
    fn a_detected_language_that_differs_is_reported() {
        let mut result = transcription("สวัสดี", 0.9);
        result.detected_language = Some(Language::Thai);
        assert!(result.differs_from(Language::English));
        assert!(!result.differs_from(Language::Thai));
    }

    #[test]
    fn an_undetected_language_never_reports_a_difference() {
        // "Unknown" must not be treated as "different", or every transcription
        // would flag a language change.
        let result = transcription("hello", 0.9);
        assert!(!result.differs_from(Language::English));
    }

    #[test]
    fn empty_transcripts_are_not_translatable() {
        assert!(!is_translatable(&transcription("", 0.9)));
        assert!(!is_translatable(&transcription("   ", 0.9)));
        assert!(!is_translatable(&transcription(".", 0.9)));
        assert!(!is_translatable(&transcription("...", 0.9)));
    }

    #[test]
    fn single_character_noise_is_not_translatable() {
        // Providers emit a stray "a" or "嗯" for noise.
        assert!(!is_translatable(&transcription("a", 0.9)));
        assert!(!is_translatable(&transcription("-", 0.9)));
    }

    #[test]
    fn low_confidence_noise_is_not_translatable() {
        // A short low-confidence result is a hallucination, not speech.
        assert!(!is_translatable(&transcription("hmm", 0.10)));
        assert!(!is_translatable(&transcription("you", 0.25)));
    }

    #[test]
    fn a_real_callout_is_translatable() {
        assert!(is_translatable(&transcription("Push B.", 0.95)));
        assert!(is_translatable(&transcription("Enemy is behind us.", 0.99)));
        assert!(is_translatable(&transcription("ดันบี", 0.88)));
    }

    #[test]
    fn a_thai_callout_is_not_rejected_for_being_short() {
        // Thai packs meaning into few characters; a length rule tuned for
        // English would throw away legitimate callouts.
        assert!(is_translatable(&transcription("ถอย", 0.9)));
        assert!(is_translatable(&transcription("ไปเอ", 0.9)));
    }

    #[test]
    fn empty_audio_input_is_detected() {
        let empty = AudioInput::bytes(&[], "audio/wav");
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let data = [0u8; 64];
        let audio = AudioInput::bytes(&data, "audio/wav");
        assert!(!audio.is_empty());
        assert_eq!(audio.len(), 64);

        assert!(AudioInput::Url("https://example.com/a.wav").is_empty());
    }

    #[test]
    fn transient_failures_are_retryable() {
        assert!(TranscribeError::RateLimited.is_retryable());
        assert!(TranscribeError::Timeout {
            after: Duration::from_secs(30)
        }
        .is_retryable());
        assert!(TranscribeError::Connect {
            detail: "refused".into()
        }
        .is_retryable());
    }

    #[test]
    fn credential_failures_need_the_user() {
        assert!(TranscribeError::Unauthorized.needs_user_action());
        assert!(TranscribeError::MissingKey {
            env_var: DEFAULT_DEEPGRAM_KEY_ENV.into()
        }
        .needs_user_action());
        assert!(!TranscribeError::Timeout {
            after: Duration::from_secs(30)
        }
        .needs_user_action());
    }

    #[test]
    fn the_missing_key_message_names_the_variable() {
        let error = TranscribeError::MissingKey {
            env_var: "MY_STT_KEY".into(),
        };
        assert!(error.user_message().contains("MY_STT_KEY"));
    }

    #[test]
    fn user_messages_are_short_enough_for_a_banner() {
        let errors = [
            TranscribeError::Unauthorized,
            TranscribeError::RateLimited,
            TranscribeError::BadAudio {
                message: "too short".into(),
            },
            TranscribeError::Server {
                status: 500,
                message: "internal".into(),
            },
            TranscribeError::BadResponse {
                detail: "x".into(),
            },
            TranscribeError::Timeout {
                after: Duration::from_secs(30),
            },
            TranscribeError::Connect {
                detail: "refused".into(),
            },
            TranscribeError::EmptyAudio,
        ];
        for error in errors {
            let message = error.user_message();
            assert!(message.len() < 140, "{error:?}: {message}");
            assert!(message.ends_with('.'), "{error:?}: {message}");
        }
    }

    #[test]
    fn the_trait_object_has_a_safe_debug() {
        let provider: Box<dyn TranscriptionProvider> = Box::new(deepgram::StubTranscriber);
        let debug = format!("{provider:?}");
        assert!(debug.contains("TranscriptionProvider"));
        assert!(!debug.contains("Token "));
    }
}