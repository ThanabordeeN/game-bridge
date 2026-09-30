//! Errors produced while encoding, decoding, or validating protocol messages.

use thiserror::Error;

/// Anything that can go wrong turning bytes into protocol values.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// The JSON payload was malformed or did not match any known event.
    #[error("invalid event payload: {0}")]
    Json(#[from] serde_json::Error),

    /// A binary message was too short to contain its header.
    #[error("audio frame truncated: need {need} header bytes, got {got}")]
    FrameTruncated {
        /// Bytes required for the header.
        need: usize,
        /// Bytes actually present.
        got: usize,
    },

    /// The binary frame declared a payload length that does not match the
    /// bytes that followed it.
    #[error("audio frame length mismatch: header declares {declared} payload bytes, got {actual}")]
    FrameLengthMismatch {
        /// Payload length declared in the header.
        declared: usize,
        /// Payload bytes actually present.
        actual: usize,
    },

    /// The frame carried a magic value that is not Game Bridge audio.
    #[error("audio frame magic mismatch: expected 0x{MAGIC:08X}, got 0x{got:08X}")]
    FrameMagicMismatch {
        /// The magic value this build expects.
        got: u32,
    },

    /// The frame declared a protocol version this build cannot read.
    #[error("unsupported protocol version {got}, this build speaks {supported}")]
    UnsupportedVersion {
        /// Version found on the wire.
        got: u16,
        /// Version this build supports.
        supported: u16,
    },

    /// The frame used a sample format other than 16-bit signed PCM.
    #[error("unsupported sample format: {bits} bits, {channels} channels")]
    UnsupportedSampleFormat {
        /// Bits per sample declared by the frame.
        bits: u8,
        /// Channel count declared by the frame.
        channels: u8,
    },

    /// The event carried a protocol version the gateway does not speak.
    #[error("protocol version mismatch: client speaks {client}, server speaks {server}")]
    VersionMismatch {
        /// Version the client announced.
        client: u16,
        /// Version the server requires.
        server: u16,
    },
}

/// The 4-byte magic prefix on every binary audio frame: ASCII `"GBR1"` read as
/// a little-endian u32.
pub const MAGIC: u32 = u32::from_le_bytes(*b"GBR1");

impl ProtocolError {
    /// The magic value this build expects, for error formatting.
    pub const EXPECTED_MAGIC: u32 = MAGIC;
}
