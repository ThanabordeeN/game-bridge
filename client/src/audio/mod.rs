//! Audio capture and playback backends.
//!
//! The Windows implementation uses WASAPI (§8). Every backend is behind
//! `#[cfg(target_os = "windows")]` with a stub elsewhere, so the portable audio
//! logic — VAD, mixer, resampler, ring buffer — compiles and tests on any
//! platform. See the README's capability matrix: this is a development
//! convenience, not a supported product configuration.

use crate::device::detection::AudioDevice;
use crate::error::AudioError;

pub mod mixer;
pub mod resampler;
pub mod segmenter;
pub mod ring;
pub mod vad;
pub mod wav;

#[cfg(target_os = "windows")]
pub mod wasapi;

/// A source of captured audio samples.
///
/// Implementations push into a [`ring::Producer`] from a real-time callback and
/// never allocate or block there. Everything expensive happens on the consumer
/// side, on a normal thread.
pub trait CaptureSource: Send {
    /// Open the endpoint and begin capturing.
    fn start(&mut self) -> Result<(), AudioError>;

    /// Stop capturing and release the endpoint.
    fn stop(&mut self) -> Result<(), AudioError>;

    /// The endpoint being captured, for display and for the §13 audit.
    fn device(&self) -> &AudioDevice;

    /// Sample rate the endpoint is delivering, after any format negotiation.
    fn sample_rate_hz(&self) -> u32;

    /// Channel count the endpoint is delivering.
    fn channels(&self) -> u16;

    /// Whether capture is currently running.
    fn is_running(&self) -> bool;
}

/// A sink audio can be rendered to.
pub trait PlaybackSink: Send {
    /// Open the endpoint and begin rendering.
    fn start(&mut self) -> Result<(), AudioError>;

    /// Stop rendering and release the endpoint.
    fn stop(&mut self) -> Result<(), AudioError>;

    /// The endpoint being rendered to.
    fn device(&self) -> &AudioDevice;

    /// Queue samples for playback. Must not block the caller indefinitely.
    fn queue(&mut self, samples: &[i16]) -> Result<(), AudioError>;

    /// Samples still waiting to be played.
    fn queued_samples(&self) -> usize;

    /// Whether rendering is currently running.
    fn is_running(&self) -> bool;
}

/// The direction a [`CaptureSource`] represents, used for logging and for the
/// §13 audit that no virtual endpoint is ever opened for capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// Physical microphone.
    Microphone,
    /// Application loopback.
    ApplicationLoopback,
}

impl CaptureKind {
    /// The protocol-level source tag for this capture kind.
    pub fn audio_source(self) -> game_bridge_protocol::mode::AudioSource {
        match self {
            CaptureKind::Microphone => game_bridge_protocol::mode::AudioSource::PhysicalMic,
            CaptureKind::ApplicationLoopback => {
                game_bridge_protocol::mode::AudioSource::ApplicationLoopback
            }
        }
    }
}

/// Refuse to open a virtual endpoint for capture.
///
/// Called by both capture backends before they touch the device. The device
/// inventory already filters these out, and the routing layer checks again;
/// this third check exists because the consequence of getting it wrong is a
/// howling feedback loop in the user's headset, which is worth three cheap
/// guards.
pub fn assert_capturable(device: &AudioDevice) -> Result<(), AudioError> {
    if device.is_virtual {
        return Err(AudioError::FeedbackLoopPrevented {
            device: device.name.clone(),
        });
    }
    if device.flow != crate::device::detection::DataFlow::Capture {
        return Err(AudioError::WrongDirection {
            device: device.name.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::detection::{DataFlow, VIRTUAL_DEVICE_NAME};

    #[test]
    fn virtual_endpoints_are_refused_for_capture() {
        let device = AudioDevice::virtual_mic("{virtual}");
        let error = assert_capturable(&device).unwrap_err();
        assert!(matches!(error, AudioError::FeedbackLoopPrevented { .. }));
    }

    #[test]
    fn render_endpoints_are_refused_for_capture() {
        let device = AudioDevice::physical("{out}", "Headphones", DataFlow::Render);
        assert!(matches!(
            assert_capturable(&device).unwrap_err(),
            AudioError::WrongDirection { .. }
        ));
    }

    #[test]
    fn a_normal_microphone_is_accepted() {
        let device = AudioDevice::physical("{in}", "HyperX QuadCast", DataFlow::Capture);
        assert!(assert_capturable(&device).is_ok());
    }

    #[test]
    fn capture_kinds_map_to_the_protocol_sources() {
        use game_bridge_protocol::mode::AudioSource;
        assert_eq!(
            CaptureKind::Microphone.audio_source(),
            AudioSource::PhysicalMic
        );
        assert_eq!(
            CaptureKind::ApplicationLoopback.audio_source(),
            AudioSource::ApplicationLoopback
        );
    }

    #[test]
    fn the_virtual_device_name_is_referenced_consistently() {
        let device = AudioDevice::virtual_mic("{virtual}");
        assert_eq!(device.name, VIRTUAL_DEVICE_NAME);
    }
}
