//! Binary audio framing.
//!
//! Audio travels on the WebSocket binary channel as a fixed 16-byte header
//! followed by raw PCM. The header is little-endian throughout.
//!
//! ```text
//! offset  size  field
//!      0     4  magic          b"GBR1"
//!      4     2  version        protocol version
//!      6     1  flags          see FrameFlags
//!      7     1  channels       interleaved channel count
//!      8     4  sample_rate_hz
//!     12     4  payload_len    bytes of PCM that follow
//!     16     N  payload        signed 16-bit LE PCM
//! ```
//!
//! A fixed header keeps parsing branch-free and lets the gateway reject a
//! malformed frame on the first two bytes.

use crate::error::{ProtocolError, MAGIC};
use crate::{PROTOCOL_VERSION, WIRE_BITS_PER_SAMPLE};

/// Size of the fixed audio frame header, in bytes.
pub const AUDIO_FRAME_HEADER_LEN: usize = 16;

bitflags_lite! {
    /// Per-frame flags.
    ///
    /// Hand-rolled rather than pulling in `bitflags`, so the protocol crate
    /// stays dependency-light enough to compile in seconds and to vendor into
    /// the gateway's own language bindings.
    pub struct FrameFlags: u8 {
        /// This frame is the last of its segment.
        const FINAL = 0b0000_0001;
        /// The segment this frame belongs to was cut short rather than ending
        /// on a natural pause.
        const TRUNCATED = 0b0000_0010;
        /// Frame carries TTS audio flowing gateway → client rather than
        /// capture audio flowing client → gateway.
        const TTS = 0b0000_0100;
    }
}

/// The parsed header of a binary audio frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFrameHeader {
    /// Protocol version.
    pub version: u16,
    /// Per-frame flags.
    pub flags: FrameFlags,
    /// Interleaved channel count.
    pub channels: u8,
    /// Sample rate in Hz.
    pub sample_rate_hz: u32,
    /// Number of payload bytes that follow the header.
    pub payload_len: u32,
}

impl AudioFrameHeader {
    /// Build a header for a payload of `payload_len` bytes.
    pub fn new(flags: FrameFlags, sample_rate_hz: u32, payload_len: u32) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            flags,
            channels: crate::WIRE_CHANNELS,
            sample_rate_hz,
            payload_len,
        }
    }

    /// Encode the header to its 16 wire bytes.
    pub fn to_bytes(self) -> [u8; AUDIO_FRAME_HEADER_LEN] {
        let mut out = [0u8; AUDIO_FRAME_HEADER_LEN];
        out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6] = self.flags.bits();
        out[7] = self.channels;
        out[8..12].copy_from_slice(&self.sample_rate_hz.to_le_bytes());
        out[12..16].copy_from_slice(&self.payload_len.to_le_bytes());
        out
    }

    /// Parse a header from the first 16 bytes of a binary message.
    ///
    /// Validates magic, version, and sample format. It does not validate that
    /// the payload is actually present — [`AudioFrame::decode`] does that.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < AUDIO_FRAME_HEADER_LEN {
            return Err(ProtocolError::FrameTruncated {
                need: AUDIO_FRAME_HEADER_LEN,
                got: bytes.len(),
            });
        }

        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != MAGIC {
            return Err(ProtocolError::FrameMagicMismatch { got: magic });
        }

        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion {
                got: version,
                supported: PROTOCOL_VERSION,
            });
        }

        let flags = FrameFlags::from_bits_truncate(bytes[6]);
        let channels = bytes[7];
        let sample_rate_hz = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let payload_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);

        // Channels are not in the header as a field we can reject on their own,
        // but mono and stereo are the only layouts any provider emits.
        if channels == 0 || channels > 2 {
            return Err(ProtocolError::UnsupportedSampleFormat {
                bits: WIRE_BITS_PER_SAMPLE,
                channels,
            });
        }

        Ok(Self {
            version,
            flags,
            channels,
            sample_rate_hz,
            payload_len,
        })
    }

    /// Duration of the payload in milliseconds, from the sample rate and the
    /// declared length. Zero-length payloads report zero.
    pub fn duration_ms(&self) -> u32 {
        if self.sample_rate_hz == 0 {
            return 0;
        }
        let channels = self.channels.max(1) as u32;
        let bytes_per_sample = u32::from(WIRE_BITS_PER_SAMPLE / 8);
        let frames = self.payload_len / (bytes_per_sample * channels);
        frames.saturating_mul(1000) / self.sample_rate_hz
    }
}

/// A complete audio frame: header plus PCM payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame<'a> {
    /// The parsed header.
    pub header: AudioFrameHeader,
    /// Raw signed 16-bit little-endian PCM.
    pub payload: &'a [u8],
}

impl<'a> AudioFrame<'a> {
    /// Wrap a payload with a freshly built header.
    pub fn new(flags: FrameFlags, sample_rate_hz: u32, payload: &'a [u8]) -> Self {
        let header = AudioFrameHeader::new(flags, sample_rate_hz, payload.len() as u32);
        Self { header, payload }
    }

    /// Decode a complete binary message.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, ProtocolError> {
        let header = AudioFrameHeader::from_bytes(bytes)?;
        let payload = &bytes[AUDIO_FRAME_HEADER_LEN..];
        let declared = header.payload_len as usize;
        if payload.len() != declared {
            return Err(ProtocolError::FrameLengthMismatch {
                declared,
                actual: payload.len(),
            });
        }
        Ok(Self { header, payload })
    }

    /// Encode header and payload into one contiguous message.
    ///
    /// Allocates once. The capture path uses this rather than sending header
    /// and payload as separate WebSocket messages, which would double the
    /// syscall count on the hottest path in the client.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(AUDIO_FRAME_HEADER_LEN + self.payload.len());
        out.extend_from_slice(&self.header.to_bytes());
        out.extend_from_slice(self.payload);
        out
    }

    /// Payload interpreted as signed 16-bit samples.
    ///
    /// A trailing odd byte, which a truncated frame could produce, is dropped.
    pub fn samples_i16(&self) -> Vec<i16> {
        self.payload
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    }
}

/// Minimal `bitflags`-style newtype, so the protocol crate needs no extra
/// dependency for four flags.
#[macro_export]
macro_rules! bitflags_lite {
    (
        $(#[$meta:meta])*
        pub struct $name:ident: $ty:ty {
            $(
                $(#[$fmeta:meta])*
                const $flag:ident = $value:expr;
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $name($ty);

        impl $name {
            $(
                $(#[$fmeta])*
                pub const $flag: Self = Self($value);
            )*

            /// The empty flag set.
            pub const NONE: Self = Self(0);

            /// The raw bit pattern.
            pub const fn bits(self) -> $ty {
                self.0
            }

            /// Build from a raw bit pattern, keeping only defined bits.
            pub const fn from_bits_truncate(bits: $ty) -> Self {
                let mut known: $ty = 0;
                $( known |= $value; )*
                Self(bits & known)
            }

            /// Whether every bit in `other` is set.
            pub const fn contains(self, other: Self) -> bool {
                (self.0 & other.0) == other.0
            }

            /// Whether any bit in `other` is set.
            pub const fn intersects(self, other: Self) -> bool {
                (self.0 & other.0) != 0
            }

            /// Whether no bits are set.
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }
        }

        impl core::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self {
                Self(self.0 | rhs.0)
            }
        }

        impl core::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }
    };
}

pub use bitflags_lite;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WIRE_SAMPLE_RATE_HZ;

    #[test]
    fn header_is_sixteen_bytes_with_the_documented_layout() {
        let header = AudioFrameHeader::new(FrameFlags::FINAL, 16_000, 640);
        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), AUDIO_FRAME_HEADER_LEN);
        assert_eq!(&bytes[0..4], b"GBR1");
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), PROTOCOL_VERSION);
        assert_eq!(bytes[6], FrameFlags::FINAL.bits());
        assert_eq!(bytes[7], 1, "wire audio is mono");
        assert_eq!(u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]), 16_000);
        assert_eq!(u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]), 640);
    }

    #[test]
    fn header_roundtrips() {
        let header = AudioFrameHeader::new(FrameFlags::TTS, 24_000, 960);
        assert_eq!(AudioFrameHeader::from_bytes(&header.to_bytes()).unwrap(), header);
    }

    #[test]
    fn truncated_header_is_rejected() {
        let err = AudioFrameHeader::from_bytes(&[0u8; 8]).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::FrameTruncated { need: 16, got: 8 }
        ));
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut bytes = AudioFrameHeader::new(FrameFlags::NONE, 16_000, 0).to_bytes();
        bytes[0] = b'X';
        assert!(matches!(
            AudioFrameHeader::from_bytes(&bytes).unwrap_err(),
            ProtocolError::FrameMagicMismatch { .. }
        ));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut bytes = AudioFrameHeader::new(FrameFlags::NONE, 16_000, 0).to_bytes();
        bytes[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert!(matches!(
            AudioFrameHeader::from_bytes(&bytes).unwrap_err(),
            ProtocolError::UnsupportedVersion { got: 99, .. }
        ));
    }

    #[test]
    fn zero_and_three_channel_frames_are_rejected() {
        let mut bytes = AudioFrameHeader::new(FrameFlags::NONE, 16_000, 0).to_bytes();
        bytes[7] = 0;
        assert!(AudioFrameHeader::from_bytes(&bytes).is_err());
        bytes[7] = 3;
        assert!(AudioFrameHeader::from_bytes(&bytes).is_err());
    }

    #[test]
    fn frame_roundtrips_with_payload() {
        let pcm: Vec<u8> = (0..640u16).map(|i| (i % 251) as u8).collect();
        let frame = AudioFrame::new(FrameFlags::NONE, WIRE_SAMPLE_RATE_HZ, &pcm);
        let encoded = frame.encode();
        assert_eq!(encoded.len(), AUDIO_FRAME_HEADER_LEN + 640);

        let decoded = AudioFrame::decode(&encoded).unwrap();
        assert_eq!(decoded.payload, &pcm[..]);
        assert_eq!(decoded.header.sample_rate_hz, WIRE_SAMPLE_RATE_HZ);
    }

    #[test]
    fn lying_length_field_is_rejected() {
        let pcm = [0u8; 320];
        let mut encoded = AudioFrame::new(FrameFlags::NONE, 16_000, &pcm).encode();
        // Declare 640 payload bytes while only 320 follow.
        encoded[12..16].copy_from_slice(&640u32.to_le_bytes());
        assert!(matches!(
            AudioFrame::decode(&encoded).unwrap_err(),
            ProtocolError::FrameLengthMismatch {
                declared: 640,
                actual: 320
            }
        ));
    }

    #[test]
    fn duration_is_computed_from_rate() {
        // 20 ms of 16 kHz mono 16-bit audio is 320 samples = 640 bytes.
        let header = AudioFrameHeader::new(FrameFlags::NONE, 16_000, 640);
        assert_eq!(header.duration_ms(), 20);

        // Same duration at 48 kHz is three times the bytes.
        let header = AudioFrameHeader::new(FrameFlags::NONE, 48_000, 1920);
        assert_eq!(header.duration_ms(), 20);

        // Stereo doubles the byte count for the same duration.
        let mut stereo = AudioFrameHeader::new(FrameFlags::NONE, 16_000, 1280);
        stereo.channels = 2;
        assert_eq!(stereo.duration_ms(), 20);
    }

    #[test]
    fn zero_rate_reports_zero_duration_instead_of_dividing_by_zero() {
        let mut header = AudioFrameHeader::new(FrameFlags::NONE, 16_000, 640);
        header.sample_rate_hz = 0;
        assert_eq!(header.duration_ms(), 0);
    }

    #[test]
    fn samples_decode_little_endian() {
        let payload = [0x01u8, 0x00, 0xFF, 0xFF, 0x00, 0x80];
        let frame = AudioFrame::new(FrameFlags::NONE, 16_000, &payload);
        assert_eq!(frame.samples_i16(), vec![1i16, -1i16, -32768i16]);
    }

    #[test]
    fn flags_compose_and_test() {
        let flags = FrameFlags::FINAL | FrameFlags::TRUNCATED;
        assert!(flags.contains(FrameFlags::FINAL));
        assert!(flags.contains(FrameFlags::TRUNCATED));
        assert!(!flags.contains(FrameFlags::TTS));
        assert!(flags.intersects(FrameFlags::FINAL));
        assert!(!flags.is_empty());
        assert!(FrameFlags::NONE.is_empty());
    }

    #[test]
    fn unknown_flag_bits_are_dropped_not_rejected() {
        // Forward compatibility: a future version may add flags, and an older
        // client should ignore what it does not know rather than drop audio.
        let flags = FrameFlags::from_bits_truncate(0b1111_0001);
        assert!(flags.contains(FrameFlags::FINAL));
        assert_eq!(flags.bits(), 0b0000_0001);
    }
}
