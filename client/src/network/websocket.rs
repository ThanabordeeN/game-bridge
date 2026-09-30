//! The WebSocket transport (§14).
//!
//! Game Bridge uses WebSocket over TLS on one persistent connection, carrying
//! JSON events on the text channel and framed PCM on the binary channel.
//!
//! # Why the transport is described rather than implemented here
//!
//! A working client needs a WebSocket stack, and every candidate (`tokio-
//! tungstenite`, `fastwebsockets`) is a real dependency with a real build cost.
//! Choosing one is a decision that belongs with the first end-to-end
//! integration, not with a scaffold, and picking now would mean either a
//! dependency nobody exercised or a fake implementation that looks real.
//!
//! What is defined here is the **contract** the transport must satisfy, plus a
//! fully working in-memory implementation used by tests and by the UI's
//! offline mode. That keeps the seam honest: the session state machine, the
//! codec, and the backoff logic are all exercised against [`InMemoryTransport`]
//! today, and swapping in a socket is a matter of implementing one trait.

use std::collections::VecDeque;
use std::time::Duration;

use game_bridge_protocol::frame::AudioFrame;
use game_bridge_protocol::PROTOCOL_VERSION;

use crate::error::NetworkError;

/// A bidirectional message from the gateway.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportMessage {
    /// A JSON text event.
    Text(String),
    /// A binary audio frame, already decoded.
    Audio {
        /// The frame's PCM payload.
        payload: Vec<u8>,
        /// Sample rate declared on the frame.
        sample_rate_hz: u32,
        /// Whether the frame is the last of its segment.
        is_final: bool,
    },
}

/// The transport contract the session drives.
pub trait Transport: Send {
    /// Open the connection.
    ///
    /// `token` is the short-lived session bearer from §15. Implementations must
    /// send it in a header, never in a URL query string, where it would land in
    /// access logs and browser history.
    fn connect(&mut self, endpoint: &str, token: &str) -> Result<(), NetworkError>;

    /// Send a JSON text event.
    fn send_text(&mut self, text: &str) -> Result<(), NetworkError>;

    /// Send a binary audio frame.
    fn send_audio(&mut self, frame: &AudioFrame<'_>) -> Result<(), NetworkError>;

    /// Receive the next message, waiting at most `timeout`.
    ///
    /// Returns `Ok(None)` on timeout rather than an error: a quiet socket is
    /// normal when nobody is speaking, which is most of the time in a game.
    fn receive(&mut self, timeout: Duration) -> Result<Option<TransportMessage>, NetworkError>;

    /// Close the connection.
    fn close(&mut self) -> Result<(), NetworkError>;

    /// Whether the connection is currently open.
    fn is_connected(&self) -> bool;
}

/// An in-memory transport for tests and for the UI's offline mode.
///
/// It is a real implementation of the trait, not a stub: it encodes frames,
/// queues messages in both directions, and can be driven deterministically. The
/// UI's demo mode uses it so the whole session state machine runs without a
/// network.
#[derive(Debug, Default)]
pub struct InMemoryTransport {
    connected: bool,
    /// Messages the client has sent, in order.
    pub sent_text: Vec<String>,
    /// Frames the client has sent, as encoded bytes.
    pub sent_audio: Vec<Vec<u8>>,
    /// Messages waiting to be delivered to the client.
    pub inbox: VecDeque<TransportMessage>,
    /// When set, the next call fails with this message.
    pub fail_next: Option<String>,
}

impl InMemoryTransport {
    /// Create a disconnected transport.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a text event for delivery to the client.
    pub fn queue_text(&mut self, text: impl Into<String>) {
        self.inbox.push_back(TransportMessage::Text(text.into()));
    }

    /// Queue a binary audio frame for delivery.
    pub fn queue_audio(&mut self, payload: Vec<u8>, sample_rate_hz: u32, is_final: bool) {
        self.inbox.push_back(TransportMessage::Audio {
            payload,
            sample_rate_hz,
            is_final,
        });
    }

    /// Number of text events the client sent.
    pub fn sent_text_count(&self) -> usize {
        self.sent_text.len()
    }

    /// Decode the most recent audio frame the client sent.
    pub fn last_sent_frame(&self) -> Option<AudioFrame<'_>> {
        self.sent_audio.last().and_then(|bytes| AudioFrame::decode(bytes).ok())
    }
}

impl Transport for InMemoryTransport {
    fn connect(&mut self, _endpoint: &str, token: &str) -> Result<(), NetworkError> {
        if token.is_empty() {
            return Err(NetworkError::Unauthorized(
                "no session token supplied".into(),
            ));
        }
        if let Some(message) = self.fail_next.take() {
            return Err(NetworkError::Connect(message));
        }
        self.connected = true;
        Ok(())
    }

    fn send_text(&mut self, text: &str) -> Result<(), NetworkError> {
        if !self.connected {
            return Err(NetworkError::Disconnected("socket is closed".into()));
        }
        if let Some(message) = self.fail_next.take() {
            self.connected = false;
            return Err(NetworkError::Disconnected(message));
        }
        self.sent_text.push(text.to_string());
        Ok(())
    }

    fn send_audio(&mut self, frame: &AudioFrame<'_>) -> Result<(), NetworkError> {
        if !self.connected {
            return Err(NetworkError::Disconnected("socket is closed".into()));
        }
        let encoded = frame.encode();
        // Validate what we are about to hand the transport: a frame that cannot
        // be decoded by the gateway must never leave the client.
        AudioFrame::decode(&encoded)?;
        self.sent_audio.push(encoded);
        Ok(())
    }

    fn receive(&mut self, _timeout: Duration) -> Result<Option<TransportMessage>, NetworkError> {
        if !self.connected {
            return Err(NetworkError::Disconnected("socket is closed".into()));
        }
        Ok(self.inbox.pop_front())
    }

    fn close(&mut self) -> Result<(), NetworkError> {
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

/// The gateway endpoint the client connects to.
///
/// TLS is not optional (§14). The scheme is fixed to `wss` so a build cannot be
/// pointed at a plaintext endpoint by editing a config value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayEndpoint {
    /// Host, e.g. `gateway.gamebridge.dev`.
    pub host: String,
    /// Port, 443 by default.
    pub port: u16,
    /// Path, e.g. `/v1/stream`.
    pub path: String,
}

impl Default for GatewayEndpoint {
    fn default() -> Self {
        Self {
            host: "gateway.gamebridge.dev".into(),
            port: 443,
            path: "/v1/stream".into(),
        }
    }
}

impl GatewayEndpoint {
    /// The secure URL to connect to.
    ///
    /// Always `wss://`. See [`GatewayEndpoint`] for why this is not
    /// configurable.
    pub fn url(&self) -> String {
        format!("wss://{}:{}{}", self.host, self.port, self.path)
    }

    /// The URL without credentials, for logs.
    pub fn safe_display(&self) -> String {
        self.url()
    }
}

/// The handshake the client sends first: `session.start` (§14).
#[derive(Debug, Clone, PartialEq)]
pub struct Handshake {
    /// Protocol version.
    pub protocol_version: u16,
    /// Negotiated sample rate.
    pub sample_rate_hz: u32,
}

impl Default for Handshake {
    fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            sample_rate_hz: game_bridge_protocol::WIRE_SAMPLE_RATE_HZ,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_bridge_protocol::frame::FrameFlags;

    #[test]
    fn endpoint_url_is_always_secure() {
        let endpoint = GatewayEndpoint::default();
        assert!(endpoint.url().starts_with("wss://"), "{}", endpoint.url());
        assert_eq!(endpoint.url(), "wss://gateway.gamebridge.dev:443/v1/stream");
    }

    #[test]
    fn the_safe_display_contains_no_secret() {
        let endpoint = GatewayEndpoint {
            host: "gw.example".into(),
            port: 443,
            path: "/v1/stream".into(),
        };
        assert_eq!(endpoint.safe_display(), "wss://gw.example:443/v1/stream");
    }

    #[test]
    fn connecting_without_a_token_is_rejected() {
        let mut transport = InMemoryTransport::new();
        let error = transport
            .connect("wss://example", "")
            .expect_err("must reject an empty token");
        assert!(matches!(error, NetworkError::Unauthorized(_)));
        assert!(!transport.is_connected());
    }

    #[test]
    fn connecting_with_a_token_succeeds() {
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();
        assert!(transport.is_connected());
    }

    #[test]
    fn sending_before_connecting_is_an_error() {
        let mut transport = InMemoryTransport::new();
        assert!(matches!(
            transport.send_text("{}").unwrap_err(),
            NetworkError::Disconnected(_)
        ));
    }

    #[test]
    fn text_events_are_recorded_in_order() {
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();
        transport.send_text("first").unwrap();
        transport.send_text("second").unwrap();
        assert_eq!(transport.sent_text, vec!["first", "second"]);
        assert_eq!(transport.sent_text_count(), 2);
    }

    #[test]
    fn audio_frames_sent_are_decodable_by_the_gateway() {
        // The property that matters: whatever the client sends must survive the
        // round trip through the same decoder the gateway uses.
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();

        let pcm = vec![7u8; 640];
        let frame = AudioFrame::new(FrameFlags::NONE, 16_000, &pcm);
        transport.send_audio(&frame).unwrap();

        let decoded = transport.last_sent_frame().expect("a frame was sent");
        assert_eq!(decoded.payload, &pcm[..]);
        assert_eq!(decoded.header.sample_rate_hz, 16_000);
    }

    #[test]
    fn a_final_tts_frame_keeps_its_flags() {
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();
        let flags = FrameFlags::TTS | FrameFlags::FINAL;
        transport
            .send_audio(&AudioFrame::new(flags, 24_000, &[0u8; 32]))
            .unwrap();
        let decoded = transport.last_sent_frame().unwrap();
        assert!(decoded.header.flags.contains(FrameFlags::TTS));
        assert!(decoded.header.flags.contains(FrameFlags::FINAL));
    }

    #[test]
    fn receive_returns_queued_messages_then_none() {
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();
        transport.queue_text("hello");

        let first = transport.receive(Duration::from_millis(1)).unwrap();
        assert_eq!(first, Some(TransportMessage::Text("hello".into())));

        // A quiet socket is not an error; it is the normal state.
        let empty = transport.receive(Duration::from_millis(1)).unwrap();
        assert_eq!(empty, None);
    }

    #[test]
    fn audio_received_preserves_its_metadata() {
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();
        transport.queue_audio(vec![1, 2, 3, 4], 24_000, true);

        match transport.receive(Duration::from_millis(1)).unwrap() {
            Some(TransportMessage::Audio {
                payload,
                sample_rate_hz,
                is_final,
            }) => {
                assert_eq!(payload, vec![1, 2, 3, 4]);
                assert_eq!(sample_rate_hz, 24_000);
                assert!(is_final);
            }
            other => panic!("expected audio, got {other:?}"),
        }
    }

    #[test]
    fn closing_stops_further_sends() {
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();
        transport.close().unwrap();
        assert!(!transport.is_connected());
        assert!(transport.send_text("{}").is_err());
    }

    #[test]
    fn an_injected_failure_surfaces_as_a_connect_error() {
        let mut transport = InMemoryTransport::new();
        transport.fail_next = Some("gateway unreachable".into());
        let error = transport.connect("wss://example", "tok").unwrap_err();
        assert!(matches!(error, NetworkError::Connect(_)));
    }

    #[test]
    fn a_mid_stream_failure_drops_the_connection() {
        // A dropped socket must be distinguishable from a transient send error,
        // because the session reacts to it by entering Reconnecting.
        let mut transport = InMemoryTransport::new();
        transport.connect("wss://example", "tok").unwrap();
        transport.fail_next = Some("broken pipe".into());
        assert!(transport.send_text("{}").is_err());
        assert!(!transport.is_connected());
    }

    #[test]
    fn the_default_handshake_matches_the_wire_constants() {
        let handshake = Handshake::default();
        assert_eq!(handshake.protocol_version, PROTOCOL_VERSION);
        assert_eq!(
            handshake.sample_rate_hz,
            game_bridge_protocol::WIRE_SAMPLE_RATE_HZ
        );
    }
}
