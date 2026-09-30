//! Wire protocol for Game Bridge.
//!
//! The protocol has two channels over one persistent TLS connection (§14):
//!
//! * A **text channel** carrying JSON events. Client and server events are
//!   internally tagged so a reader can dispatch on a single field.
//! * A **binary channel** carrying audio. Each binary message is a
//!   [`AudioFrame`]: a fixed 16-byte header followed by raw PCM payload.
//!
//! Keeping audio out of JSON matters: base64 would inflate the stream by ~33%
//! and force the server to decode it before it can hand bytes to the STT
//! provider, both of which cost latency the product cannot spare.
//!
//! # Example
//!
//! ```
//! use game_bridge_protocol::events::ClientEvent;
//! use game_bridge_protocol::language::Language;
//! use game_bridge_protocol::mode::{RoutingMode, VoiceTier};
//! use game_bridge_protocol::PROTOCOL_VERSION;
//!
//! let start = ClientEvent::SessionStart {
//!     protocol_version: PROTOCOL_VERSION,
//!     source_language: Language::Thai,
//!     target_language: Language::English,
//!     routing_mode: RoutingMode::VoiceOut,
//!     sample_rate_hz: game_bridge_protocol::WIRE_SAMPLE_RATE_HZ,
//!     context: Some("competitive_fps".to_string()),
//!     voice_tier: VoiceTier::Standard,
//! };
//! let json = serde_json::to_string(&start).unwrap();
//! assert!(json.contains("\"type\":\"session.start\""));
//! assert!(json.contains("\"source_language\":\"thai\""));
//! ```

pub mod confidence;
pub mod error;
pub mod events;
pub mod frame;
pub mod language;
pub mod mode;
pub mod usage;

pub use error::ProtocolError;
pub use frame::{AudioFrame, AudioFrameHeader, FrameFlags, AUDIO_FRAME_HEADER_LEN};
pub use language::Language;
pub use mode::{AudioSource, RoutingMode, VoiceTier};
pub use usage::{ActiveVoice, CreditRate, UsageUpdate};

/// Wire protocol version. The client sends this in `session.start`; the gateway
/// rejects a mismatch rather than guessing at compatibility.
pub const PROTOCOL_VERSION: u16 = 1;

/// Audio sample rate used on the wire, in Hz.
///
/// 16 kHz mono is the native input rate for every STT provider we target and is
/// sufficient for speech; sending 48 kHz would triple bandwidth for no
/// recognition benefit.
pub const WIRE_SAMPLE_RATE_HZ: u32 = 16_000;

/// Number of interleaved channels on the wire. Always mono.
pub const WIRE_CHANNELS: u8 = 1;

/// Audio sample format on the wire.
///
/// 16-bit signed little-endian PCM. Chosen over f32 to halve bandwidth; STT
/// providers accept it directly.
pub const WIRE_BITS_PER_SAMPLE: u8 = 16;

/// Default client audio chunk duration in milliseconds.
///
/// 20 ms balances latency against per-message overhead: it is long enough that
/// header and WebSocket framing cost is negligible, short enough that VAD and
/// streaming STT stay responsive.
pub const DEFAULT_CHUNK_MS: u32 = 20;
