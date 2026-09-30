//! JSON event types exchanged over the text channel (§14).
//!
//! Events are internally tagged on `"type"`, so a reader dispatches on one
//! field without trying every variant. Client and server variants live in
//! separate enums: a client decoding a `transcript.final` is a bug, and the
//! type system should say so rather than a runtime match arm.

use serde::{Deserialize, Serialize};

use crate::language::Language;
use crate::mode::{AudioSource, RoutingMode, VoiceTier};
use crate::usage::{CreditRate, UsageUpdate};

/// Credential and session identity supplied by the gateway at handshake.
///
/// The client never sees a provider key (§15). It receives a scoped session
/// token and an expiry, and presents the token on the audio channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCredentials {
    /// Opaque bearer token for this session. Short-lived and revocable.
    pub session_token: String,
    /// Unix epoch seconds at which the token expires.
    pub expires_at_unix: u64,
    /// Identifier the gateway uses in logs and support requests.
    pub session_id: String,
    /// Routes the client is permitted to use this session.
    pub scopes: Vec<String>,
}

/// Events the client sends to the gateway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    /// Opens a translation session. Must be the first event on a connection.
    #[serde(rename = "session.start")]
    SessionStart {
        /// Protocol version the client speaks.
        protocol_version: u16,
        /// Language the captured speech is in.
        source_language: Language,
        /// Language to translate into.
        target_language: Language,
        /// Which routing mode this session bills and behaves as.
        routing_mode: RoutingMode,
        /// Sample rate of the audio the client will send.
        sample_rate_hz: u32,
        /// Game context hint used by the translation prompt, e.g.
        /// `"competitive_fps"`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        /// Voice tier requested for TTS output.
        #[serde(default)]
        voice_tier: VoiceTier,
    },

    /// Signals that a speech segment is about to begin.
    ///
    /// Sent when local VAD detects speech onset, after the pre-roll buffer has
    /// been attached, so the first phoneme is never clipped.
    #[serde(rename = "audio.start")]
    AudioStart {
        /// Which endpoint this speech came from.
        source: AudioSource,
        /// Monotonic segment counter for this session, starting at 0.
        segment_id: u64,
    },

    /// Marks a chunk boundary in the audio stream.
    ///
    /// Audio itself travels on the binary channel; this event exists so the
    /// gateway can correlate a frame counter with a segment without parsing
    /// binary frames when it only needs timing.
    #[serde(rename = "audio.chunk")]
    AudioChunk {
        /// Segment this chunk belongs to.
        segment_id: u64,
        /// Payload size in bytes, so the gateway can sanity-check framing.
        byte_len: u32,
        /// Milliseconds of audio in this chunk.
        duration_ms: u32,
    },

    /// Speech segment finished; the gateway should flush STT and finalise.
    #[serde(rename = "audio.end")]
    AudioEnd {
        /// Segment that just ended.
        segment_id: u64,
        /// Why the segment ended. Lets the gateway distinguish a natural pause
        /// from a user-initiated stop and treat the tail differently.
        reason: SegmentEndReason,
    },

    /// User asked to stop the session cleanly.
    #[serde(rename = "session.stop")]
    SessionStop {
        /// Optional reason, for telemetry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Push-to-translate state changed (§11).
    #[serde(rename = "control.ptt")]
    PushToTranslate {
        /// Whether the hotkey is currently held.
        engaged: bool,
    },

    /// Bypass toggled: route original voice instead of translated voice (§12).
    #[serde(rename = "control.bypass")]
    Bypass {
        /// Whether bypass is on.
        enabled: bool,
    },

    /// Application-level heartbeat. Keeps NAT bindings alive and lets each
    /// side measure round-trip latency without touching the audio path.
    #[serde(rename = "ping")]
    Ping {
        /// Client send time in milliseconds since the Unix epoch.
        sent_at_unix_ms: u64,
    },
}

/// Why a speech segment ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentEndReason {
    /// Local VAD saw sustained silence past the hangover window.
    Silence,
    /// The user released push-to-translate.
    PushToTranslateReleased,
    /// The user stopped the session mid-utterance.
    UserStopped,
    /// A buffer bound was hit; the segment was force-flushed.
    MaxDuration,
}

/// A partial or final transcript from STT.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// The recognised text.
    pub text: String,
    /// Language the recogniser believed it heard.
    pub language: Language,
    /// Provider confidence in `0.0..=1.0`, rounded to three decimals on the
    /// wire. See [`crate::confidence`] for why it is not a raw `f32`.
    #[serde(with = "crate::confidence")]
    pub confidence: f32,
    /// Milliseconds of audio consumed to produce this transcript, measured
    /// from segment start. Lets the client display latency honestly.
    pub audio_ms: u32,
    /// Duration from segment start to this result, in milliseconds. This is the
    /// STT leg of the end-to-end latency budget.
    pub latency_ms: u32,
}

/// A partial or final translation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Translation {
    /// The translated text.
    pub text: String,
    /// Model confidence in `0.0..=1.0`, rounded to three decimals on the wire.
    #[serde(with = "crate::confidence")]
    pub confidence: f32,
    /// Language translated into.
    pub language: Language,
    /// Milliseconds from segment start to this result. This is the translation
    /// leg of the latency budget.
    pub latency_ms: u32,
}

/// Metadata describing a chunk of TTS audio that follows on the binary channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtsChunk {
    /// Segment this audio belongs to, matching the originating `audio.start`.
    pub segment_id: u64,
    /// Sample rate of the returned audio, in Hz.
    ///
    /// Stated explicitly rather than assumed: providers do not all emit 16 kHz,
    /// and the client resamples before writing to the virtual microphone.
    pub sample_rate_hz: u32,
    /// Channel count of the returned audio.
    pub channels: u8,
    /// Bits per sample.
    pub bits_per_sample: u8,
    /// Milliseconds of audio in this chunk.
    pub duration_ms: u32,
    /// Whether this is the last chunk of the segment.
    pub is_final: bool,
}

/// An error reported by the gateway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteError {
    /// Stable machine-readable code, e.g. `"low_credit"`, `"provider_timeout"`.
    pub code: String,
    /// Human-readable message. Safe to show to the user.
    pub message: String,
    /// Whether the client should retry automatically.
    pub retryable: bool,
}

/// Events the gateway sends to the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Session accepted. Carries credentials and the agreed configuration.
    #[serde(rename = "session.started")]
    SessionStarted {
        /// Credentials the client presents on the audio channel.
        credentials: SessionCredentials,
        /// Protocol version the gateway speaks. Must equal the client's.
        protocol_version: u16,
        /// Billing rates in force for this session.
        rates: Vec<CreditRate>,
    },

    /// Local or server-side VAD confirms speech started.
    #[serde(rename = "speech.start")]
    SpeechStart {
        /// Segment that started.
        segment_id: u64,
    },

    /// Speech ended server-side; final results follow.
    #[serde(rename = "speech.end")]
    SpeechEnd {
        /// Segment that ended.
        segment_id: u64,
    },

    /// Interim transcript. Rendered in the feed as greyed, replaceable text.
    #[serde(rename = "transcript.partial")]
    TranscriptPartial {
        /// Segment this partial belongs to.
        segment_id: u64,
        /// The interim transcript.
        transcript: Transcript,
    },

    /// Final transcript for a segment.
    #[serde(rename = "transcript.final")]
    TranscriptFinal {
        /// Segment this final belongs to.
        segment_id: u64,
        /// The final transcript.
        transcript: Transcript,
    },

    /// Interim translation.
    #[serde(rename = "translation.partial")]
    TranslationPartial {
        /// Segment this partial belongs to.
        segment_id: u64,
        /// The interim translation.
        translation: Translation,
    },

    /// Final translation for a segment.
    #[serde(rename = "translation.final")]
    TranslationFinal {
        /// Segment this final belongs to.
        segment_id: u64,
        /// The final translation.
        translation: Translation,
    },

    /// TTS audio for a segment is about to arrive on the binary channel.
    #[serde(rename = "tts.audio")]
    TtsAudio {
        /// Segment this audio belongs to.
        segment_id: u64,
        /// Format and framing metadata for the binary frames that follow.
        chunk: TtsChunk,
    },

    /// Running usage and cost for the session.
    #[serde(rename = "usage.update")]
    UsageUpdate {
        /// The updated counters.
        usage: UsageUpdate,
    },

    /// Wallet balance changed.
    #[serde(rename = "credit.update")]
    CreditUpdate {
        /// Remaining balance in the account's minor unit (satang).
        balance_minor: i64,
        /// ISO-4217 code, e.g. `"THB"`.
        currency: String,
        /// Set when the balance crosses the low-credit threshold, so the client
        /// can raise the §40 low-credit state without polling.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        low_credit_threshold_minor: Option<i64>,
    },

    /// The gateway is about to close; the client should reconnect.
    #[serde(rename = "server.draining")]
    ServerDraining {
        /// Suggested delay before the client reconnects, in milliseconds.
        retry_after_ms: u32,
    },

    /// Pong for a client [`ClientEvent::Ping`].
    #[serde(rename = "pong")]
    Pong {
        /// The `sent_at_unix_ms` value echoed back, for RTT computation.
        sent_at_unix_ms: u64,
        /// Server receive time in milliseconds since the Unix epoch.
        received_at_unix_ms: u64,
    },

    /// Something went wrong. May or may not be fatal; see `retryable`.
    #[serde(rename = "error")]
    Error {
        /// The error detail.
        error: RemoteError,
        /// Segment the error relates to, when it is segment-scoped rather than
        /// session-scoped.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        segment_id: Option<u64>,
    },
}

/// A decoded text-channel message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A client-originated event.
    Client(Box<ClientEvent>),
    /// A server-originated event.
    Server(Box<ServerEvent>),
}

impl ClientEvent {
    /// Encode to the JSON text of the wire format.
    pub fn to_json(&self) -> Result<String, crate::ProtocolError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Encode to pretty JSON. Used by tests and the protocol docs generator.
    pub fn to_json_pretty(&self) -> Result<String, crate::ProtocolError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Decode a client event from JSON.
    pub fn from_json(s: &str) -> Result<Self, crate::ProtocolError> {
        Ok(serde_json::from_str(s)?)
    }
}

impl ServerEvent {
    /// Encode to the JSON text of the wire format.
    pub fn to_json(&self) -> Result<String, crate::ProtocolError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Encode to pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, crate::ProtocolError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Decode a server event from JSON.
    pub fn from_json(s: &str) -> Result<Self, crate::ProtocolError> {
        Ok(serde_json::from_str(s)?)
    }
}

impl Message {
    /// Decode a text-channel payload without knowing in advance which
    /// direction sent it. Useful for proxies, recorders, and the conformance
    /// tests that assert both sides agree.
    pub fn from_json(s: &str) -> Result<Self, crate::ProtocolError> {
        // Try client first: the two enums share the `type` tag namespace, so a
        // payload only ever parses as one of them.
        match serde_json::from_str::<ClientEvent>(s) {
            Ok(ev) => Ok(Message::Client(Box::new(ev))),
            Err(client_err) => match serde_json::from_str::<ServerEvent>(s) {
                Ok(ev) => Ok(Message::Server(Box::new(ev))),
                // Report the client-side error: for an unknown tag it names the
                // tag, which is the more useful diagnostic.
                Err(_) => Err(client_err.into()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROTOCOL_VERSION;

    fn thai_to_english_start() -> ClientEvent {
        ClientEvent::SessionStart {
            protocol_version: PROTOCOL_VERSION,
            source_language: Language::Thai,
            target_language: Language::English,
            routing_mode: RoutingMode::VoiceOut,
            sample_rate_hz: crate::WIRE_SAMPLE_RATE_HZ,
            context: Some("competitive_fps".into()),
            voice_tier: VoiceTier::Cloned,
        }
    }

    #[test]
    fn client_event_roundtrips() {
        let ev = thai_to_english_start();
        let json = ev.to_json().unwrap();
        assert_eq!(ClientEvent::from_json(&json).unwrap(), ev);
    }

    #[test]
    fn session_start_uses_the_documented_tag_and_field_names() {
        let json = thai_to_english_start().to_json().unwrap();
        assert!(json.contains(r#""type":"session.start""#), "{json}");
        assert!(json.contains(r#""source_language":"thai""#), "{json}");
        assert!(json.contains(r#""target_language":"english""#), "{json}");
        assert!(json.contains(r#""routing_mode":"voice_out""#), "{json}");
        // §5 names this field `context`; keep it that way.
        assert!(json.contains(r#""context":"competitive_fps""#), "{json}");
    }

    #[test]
    fn server_event_roundtrips() {
        let ev = ServerEvent::TranslationFinal {
            segment_id: 7,
            translation: Translation {
                text: "Two are pushing B.".into(),
                confidence: 0.94,
                language: Language::English,
                latency_ms: 310,
            },
        };
        let json = ev.to_json().unwrap();
        assert!(json.contains(r#""type":"translation.final""#));
        assert_eq!(ServerEvent::from_json(&json).unwrap(), ev);
    }

    #[test]
    fn message_can_be_decoded_from_either_direction() {
        let client_json = thai_to_english_start().to_json().unwrap();
        match Message::from_json(&client_json).unwrap() {
            Message::Client(ev) => assert!(matches!(*ev, ClientEvent::SessionStart { .. })),
            Message::Server(_) => panic!("client event decoded as a server event"),
        }

        let server_json = ServerEvent::TranscriptFinal {
            segment_id: 1,
            transcript: Transcript {
                text: "ศัตรูอยู่ข้างหลัง".into(),
                language: Language::Thai,
                confidence: 0.97,
                audio_ms: 900,
                latency_ms: 240,
            },
        }
        .to_json()
        .unwrap();
        match Message::from_json(&server_json).unwrap() {
            Message::Server(ev) => assert!(matches!(*ev, ServerEvent::TranscriptFinal { .. })),
            Message::Client(_) => panic!("server event decoded as a client event"),
        }
    }

    #[test]
    fn every_documented_client_tag_parses() {
        // Guards against a rename silently breaking the wire contract in §14.
        let cases: Vec<(ClientEvent, &str)> = vec![
            (thai_to_english_start(), "session.start"),
            (
                ClientEvent::AudioStart {
                    source: AudioSource::PhysicalMic,
                    segment_id: 0,
                },
                "audio.start",
            ),
            (
                ClientEvent::AudioChunk {
                    segment_id: 0,
                    byte_len: 640,
                    duration_ms: 20,
                },
                "audio.chunk",
            ),
            (
                ClientEvent::AudioEnd {
                    segment_id: 0,
                    reason: SegmentEndReason::Silence,
                },
                "audio.end",
            ),
            (
                ClientEvent::SessionStop { reason: None },
                "session.stop",
            ),
            (
                ClientEvent::PushToTranslate { engaged: true },
                "control.ptt",
            ),
            (ClientEvent::Bypass { enabled: true }, "control.bypass"),
        ];
        for (ev, tag) in cases {
            let json = ev.to_json().unwrap();
            assert!(json.contains(&format!(r#""type":"{tag}""#)), "{json}");
            assert_eq!(ClientEvent::from_json(&json).unwrap(), ev, "tag {tag}");
        }
    }

    #[test]
    fn every_documented_server_tag_parses() {
        let cases: Vec<(ServerEvent, &str)> = vec![
            (
                ServerEvent::SpeechStart { segment_id: 3 },
                "speech.start",
            ),
            (ServerEvent::SpeechEnd { segment_id: 3 }, "speech.end"),
            (
                ServerEvent::UsageUpdate {
                    usage: UsageUpdate::default(),
                },
                "usage.update",
            ),
            (
                ServerEvent::CreditUpdate {
                    balance_minor: 8450,
                    currency: "THB".into(),
                    low_credit_threshold_minor: Some(1000),
                },
                "credit.update",
            ),
            (
                ServerEvent::ServerDraining {
                    retry_after_ms: 5000,
                },
                "server.draining",
            ),
            (
                ServerEvent::Pong {
                    sent_at_unix_ms: 1_700_000_000_000,
                    received_at_unix_ms: 1_700_000_000_042,
                },
                "pong",
            ),
        ];
        for (ev, tag) in cases {
            let json = ev.to_json().unwrap();
            assert!(json.contains(&format!(r#""type":"{tag}""#)), "{json}");
            assert_eq!(ServerEvent::from_json(&json).unwrap(), ev, "tag {tag}");
        }
    }

    #[test]
    fn unknown_event_type_is_an_error_not_a_panic() {
        let err = Message::from_json(r#"{"type":"does.not.exist"}"#);
        assert!(err.is_err());
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(ClientEvent::from_json("{not json").is_err());
    }

    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let json = ClientEvent::SessionStop { reason: None }.to_json().unwrap();
        assert!(!json.contains("reason"), "{json}");
    }
}
