//! Client error types.

use thiserror::Error;

/// Audio capture, playback, or device errors.
#[derive(Debug, Error)]
pub enum AudioError {
    /// The selected endpoint does not exist. Usually an unplugged device.
    #[error("audio device not found: {device}")]
    DeviceNotFound {
        /// Display name of the missing device.
        device: String,
    },

    /// Capture was refused because the endpoint is the virtual microphone.
    #[error(
        "refusing to capture from {device}: it is the Game Bridge virtual microphone, and \
         feeding it back into translation would create a feedback loop"
    )]
    FeedbackLoopPrevented {
        /// Display name of the offending device.
        device: String,
    },

    /// A render endpoint was used where a capture endpoint was required, or
    /// the reverse.
    #[error("{device} does not support this audio direction")]
    WrongDirection {
        /// Display name of the device.
        device: String,
    },

    /// The virtual audio driver is not installed.
    #[error(
        "Game Bridge Microphone was not found. The Game Bridge audio driver is not installed."
    )]
    VirtualDriverMissing,

    /// WASAPI or the audio engine returned an error.
    #[error("audio backend error: {0}")]
    Backend(String),

    /// The audio format the endpoint delivered is not one this client handles.
    #[error("unsupported audio format: {sample_rate_hz} Hz, {channels} channels")]
    UnsupportedFormat {
        /// Delivered sample rate.
        sample_rate_hz: u32,
        /// Delivered channel count.
        channels: u16,
    },
}

impl AudioError {
    /// Whether the user can fix this by choosing a different device, which the
    /// §40 error screens use to decide between "retry" and "pick a device".
    pub fn is_device_selection_problem(&self) -> bool {
        matches!(
            self,
            AudioError::DeviceNotFound { .. }
                | AudioError::WrongDirection { .. }
                | AudioError::FeedbackLoopPrevented { .. }
        )
    }
}

/// Session lifecycle errors.
#[derive(Debug, Error)]
pub enum SessionError {
    /// No session is currently running.
    #[error("no active session")]
    NotRunning,

    /// A session is already running.
    #[error("a session is already active")]
    AlreadyRunning,

    /// The account cannot afford the requested session.
    #[error("insufficient credit: {balance_minor} minor units remaining")]
    InsufficientCredit {
        /// Remaining balance in minor units.
        balance_minor: i64,
    },

    /// The gateway rejected the session for a reason it explained.
    #[error("gateway rejected the session: {0}")]
    Rejected(String),
}

/// Configuration errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config file could not be read.
    #[error("cannot read config at {path}: {source}")]
    Read {
        /// Path that failed.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The config file was not valid.
    #[error("invalid config: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Errors from the network layer.
#[derive(Debug, Error)]
pub enum NetworkError {
    /// The connection could not be established.
    #[error("cannot connect to the Game Bridge gateway: {0}")]
    Connect(String),

    /// The connection dropped mid-session.
    #[error("connection lost: {0}")]
    Disconnected(String),

    /// Authentication was rejected.
    #[error("authentication failed: {0}")]
    Unauthorized(String),

    /// A protocol-level error.
    #[error(transparent)]
    Protocol(#[from] game_bridge_protocol::ProtocolError),

    /// A text message was not valid JSON for any known event.
    #[error("unexpected message from the gateway: {0}")]
    UnexpectedMessage(String),
}
