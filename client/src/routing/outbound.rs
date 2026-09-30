//! Outbound routing: the user's own voice, from microphone to virtual mic.
//!
//! This is §10's "Mode B" path and the half of the product a Discord caller
//! actually hears. The router owns the §13 guarantee on this side: it can only
//! be constructed from a device that passed [`crate::audio::assert_capturable`],
//! so there is no way to build a pipeline whose input is the virtual
//! microphone.

use crate::audio::CaptureKind;
use crate::device::detection::AudioDevice;
use crate::error::AudioError;
use game_bridge_protocol::mode::AudioSource;

/// Which of the two outbound behaviours is active (§11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutboundBehaviour {
    /// Translate the user's speech and write the result to the virtual mic.
    #[default]
    Translate,
    /// Pass the user's original voice straight through to the virtual mic.
    ///
    /// This is what push-to-translate does when it is configured as
    /// "Normal → Auto Translate, hold to bypass" (§11).
    Bypass,
}

/// Routes the user's captured speech toward the virtual microphone.
#[derive(Debug)]
pub struct OutboundRouter {
    device: AudioDevice,
    kind: CaptureKind,
    behaviour: OutboundBehaviour,
    /// Whether the virtual microphone is available to write into. When the
    /// driver is missing, translated speech has nowhere to go and the router
    /// must degrade to bypass rather than silently swallowing the user's voice.
    virtual_mic_available: bool,
}

impl OutboundRouter {
    /// Build a router for a capture endpoint.
    ///
    /// Refuses any endpoint that is virtual or is a render device, so the
    /// feedback loop in §13 cannot be constructed even by mistake.
    pub fn new(
        device: AudioDevice,
        kind: CaptureKind,
        virtual_mic_available: bool,
    ) -> Result<Self, AudioError> {
        crate::audio::assert_capturable(&device)?;
        Ok(Self {
            device,
            kind,
            behaviour: OutboundBehaviour::Translate,
            virtual_mic_available,
        })
    }

    /// The endpoint being captured.
    pub fn device(&self) -> &AudioDevice {
        &self.device
    }

    /// The protocol-level source tag for this path.
    pub fn audio_source(&self) -> AudioSource {
        self.kind.audio_source()
    }

    /// The current behaviour.
    pub fn behaviour(&self) -> OutboundBehaviour {
        self.behaviour
    }

    /// Switch between translating and bypassing.
    pub fn set_behaviour(&mut self, behaviour: OutboundBehaviour) {
        self.behaviour = behaviour;
    }

    /// Whether audio currently captured will be translated.
    pub fn is_translating(&self) -> bool {
        self.behaviour == OutboundBehaviour::Translate && self.virtual_mic_available
    }

    /// Whether captured audio is currently passed through unmodified.
    ///
    /// True either because the user engaged bypass, or because the virtual
    /// microphone is missing and there is nothing else honest to do.
    pub fn is_bypassing(&self) -> bool {
        !self.is_translating()
    }

    /// Whether bypass is in effect because the driver is missing rather than
    /// because the user asked for it.
    pub fn is_degraded(&self) -> bool {
        self.behaviour == OutboundBehaviour::Translate && !self.virtual_mic_available
    }

    /// What to do with a chunk of captured speech.
    pub fn decision(&self) -> OutboundDecision {
        if self.is_translating() {
            OutboundDecision::Translate
        } else if self.virtual_mic_available {
            OutboundDecision::PassThrough
        } else {
            // No virtual mic and translation was requested: the user still
            // needs to be heard by their friends. Hand the microphone back
            // rather than dropping their voice.
            OutboundDecision::PassThrough
        }
    }

    /// The user-facing status for the mini player and tray (§30, §31).
    pub fn status_label(&self) -> &'static str {
        match self.decision() {
            OutboundDecision::Translate => "Translating",
            OutboundDecision::PassThrough if self.is_degraded() => "Microphone pass-through",
            OutboundDecision::PassThrough => "Bypassed",
        }
    }

    /// Replace the virtual microphone availability, e.g. after the user
    /// installs the driver mid-session.
    pub fn set_virtual_mic_available(&mut self, available: bool) {
        self.virtual_mic_available = available;
    }
}

/// What the outbound router decided for the current behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundDecision {
    /// Send the audio up for STT and translation, then voice the result.
    Translate,
    /// Send the user's original voice to the virtual microphone untouched.
    PassThrough,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::detection::DataFlow;

    fn mic() -> AudioDevice {
        AudioDevice::physical("{mic}", "HyperX QuadCast", DataFlow::Capture)
    }

    fn router(virtual_mic_available: bool) -> OutboundRouter {
        OutboundRouter::new(mic(), CaptureKind::Microphone, virtual_mic_available).unwrap()
    }

    #[test]
    fn a_virtual_endpoint_cannot_be_used_as_the_outbound_source() {
        let device = AudioDevice::virtual_mic("{virtual}");
        let error = OutboundRouter::new(device, CaptureKind::Microphone, true).unwrap_err();
        assert!(matches!(error, AudioError::FeedbackLoopPrevented { .. }));
    }

    #[test]
    fn a_render_endpoint_cannot_be_used_as_the_outbound_source() {
        let device = AudioDevice::physical("{out}", "Headphones", DataFlow::Render);
        let error = OutboundRouter::new(device, CaptureKind::Microphone, true).unwrap_err();
        assert!(matches!(error, AudioError::WrongDirection { .. }));
    }

    #[test]
    fn default_behaviour_is_to_translate() {
        let router = router(true);
        assert!(router.is_translating());
        assert!(!router.is_bypassing());
        assert_eq!(router.decision(), OutboundDecision::Translate);
        assert_eq!(router.status_label(), "Translating");
    }

    #[test]
    fn bypass_passes_the_original_voice_through() {
        let mut router = router(true);
        router.set_behaviour(OutboundBehaviour::Bypass);
        assert!(!router.is_translating());
        assert!(router.is_bypassing());
        assert_eq!(router.decision(), OutboundDecision::PassThrough);
        assert_eq!(router.status_label(), "Bypassed");
    }

    #[test]
    fn a_missing_driver_degrades_to_pass_through_instead_of_dropping_voice() {
        // The honest failure: without the virtual microphone the user cannot be
        // translated, but they must still be audible. Silently swallowing their
        // microphone would be far worse than passing it through.
        let router = router(false);
        assert!(router.is_degraded());
        assert!(!router.is_translating());
        assert_eq!(router.decision(), OutboundDecision::PassThrough);
        assert_eq!(router.status_label(), "Microphone pass-through");
    }

    #[test]
    fn installing_the_driver_mid_session_enables_translation() {
        let mut router = router(false);
        assert!(router.is_degraded());
        router.set_virtual_mic_available(true);
        assert!(!router.is_degraded());
        assert!(router.is_translating());
    }

    #[test]
    fn an_explicit_bypass_is_not_reported_as_degraded() {
        let mut router = router(false);
        router.set_behaviour(OutboundBehaviour::Bypass);
        assert!(!router.is_degraded(), "the user asked for this");
        assert_eq!(router.status_label(), "Bypassed");
    }

    #[test]
    fn the_source_tag_reflects_the_capture_kind() {
        let mic_router = router(true);
        assert_eq!(mic_router.audio_source(), AudioSource::PhysicalMic);

        let app_router =
            OutboundRouter::new(mic(), CaptureKind::ApplicationLoopback, true).unwrap();
        assert_eq!(app_router.audio_source(), AudioSource::ApplicationLoopback);
    }

    #[test]
    fn the_source_tag_is_always_translatable() {
        // Restating §13 for this type: whatever the router is built from, the
        // tag it reports must be an allowed translation input.
        for available in [true, false] {
            assert!(router(available).audio_source().is_translatable_input());
        }
    }
}
