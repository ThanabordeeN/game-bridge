//! Inbound routing: other players' speech, from loopback to subtitles and
//! optionally to the user's headphones.
//!
//! Inbound audio is the mirror of [`super::outbound`]: it never touches the
//! virtual microphone, because re-voicing another player's speech into the
//! user's microphone would make the client talk over the user in their own
//! voice. That constraint is enforced by [`InboundDestination`] having no
//! virtual-microphone variant at all.

use crate::audio::mixer::MixSettings;
use crate::device::detection::AudioDevice;
use crate::error::AudioError;
use game_bridge_protocol::mode::AudioSource;

/// Where inbound translated audio may be delivered.
///
/// Deliberately does **not** include the virtual microphone. See the module
/// docs: this omission is the enforcement mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundDestination {
    /// The subtitle overlay only.
    SubtitlesOnly,
    /// Subtitles plus translated audio in the user's headphones.
    SubtitlesAndHeadphones,
}

/// Routes other players' audio into the subtitle overlay and headphones.
#[derive(Debug)]
pub struct InboundRouter {
    device: AudioDevice,
    destination: InboundDestination,
    /// Whether the user wants the original voice audible under the translation.
    mix: MixSettings,
}

impl InboundRouter {
    /// Build a router for an application-loopback capture endpoint.
    ///
    /// The endpoint must be a real capture source: a virtual endpoint here
    /// would mean transcribing the client's own output as if another player had
    /// said it.
    pub fn new(device: AudioDevice, destination: InboundDestination) -> Result<Self, AudioError> {
        crate::audio::assert_capturable(&device)?;
        Ok(Self {
            device,
            destination,
            mix: match destination {
                InboundDestination::SubtitlesOnly => MixSettings::translation_only(),
                InboundDestination::SubtitlesAndHeadphones => MixSettings::mixed(),
            },
        })
    }

    /// The endpoint being captured.
    pub fn device(&self) -> &AudioDevice {
        &self.device
    }

    /// The protocol-level source tag for this path.
    pub const fn audio_source(&self) -> AudioSource {
        AudioSource::ApplicationLoopback
    }

    /// Where this router delivers audio.
    pub fn destination(&self) -> InboundDestination {
        self.destination
    }

    /// Change the destination, updating the mix preset to match.
    pub fn set_destination(&mut self, destination: InboundDestination) {
        self.destination = destination;
        self.mix = match destination {
            InboundDestination::SubtitlesOnly => MixSettings::translation_only(),
            InboundDestination::SubtitlesAndHeadphones => MixSettings::mixed(),
        };
    }

    /// Current mix settings.
    pub fn mix(&self) -> MixSettings {
        self.mix
    }

    /// Override the mix settings (§12's manual sliders).
    pub fn set_mix(&mut self, mix: MixSettings) {
        self.mix = mix;
    }

    /// Whether translated speech should be played to the user's headphones.
    pub fn plays_to_headphones(&self) -> bool {
        self.destination == InboundDestination::SubtitlesAndHeadphones
    }

    /// Whether the overlay should render subtitles.
    ///
    /// Always true: every destination in this version produces text. It is a
    /// method rather than a constant so a future audio-only mode is a change in
    /// one place.
    pub const fn shows_subtitles(&self) -> bool {
        true
    }

    /// Whether this router can ever write to the virtual microphone. Always
    /// false, by construction.
    pub const fn can_write_to_virtual_mic(&self) -> bool {
        false
    }

    /// The user-facing status for the mini player and tray.
    pub fn status_label(&self) -> &'static str {
        match self.destination {
            InboundDestination::SubtitlesOnly => "Subtitles",
            InboundDestination::SubtitlesAndHeadphones => "Subtitles + voice",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::detection::DataFlow;

    fn game() -> AudioDevice {
        // Application loopback presents as a capture endpoint for the process.
        AudioDevice::physical("{game}", "Valorant.exe", DataFlow::Capture)
    }

    #[test]
    fn inbound_router_has_no_virtual_microphone_destination() {
        // §13, expressed as an exhaustiveness property: the enum simply has no
        // variant that writes to the virtual mic.
        let router = InboundRouter::new(game(), InboundDestination::SubtitlesAndHeadphones).unwrap();
        assert!(!router.can_write_to_virtual_mic());
    }

    #[test]
    fn a_virtual_endpoint_cannot_be_an_inbound_source() {
        let device = AudioDevice::virtual_mic("{virtual}");
        let error = InboundRouter::new(device, InboundDestination::SubtitlesOnly).unwrap_err();
        assert!(matches!(error, AudioError::FeedbackLoopPrevented { .. }));
    }

    #[test]
    fn subtitles_only_does_not_play_audio() {
        let router = InboundRouter::new(game(), InboundDestination::SubtitlesOnly).unwrap();
        assert!(!router.plays_to_headphones());
        assert!(router.shows_subtitles());
        assert_eq!(router.status_label(), "Subtitles");
        assert_eq!(router.mix(), MixSettings::translation_only());
    }

    #[test]
    fn subtitles_and_headphones_plays_translated_audio() {
        let router =
            InboundRouter::new(game(), InboundDestination::SubtitlesAndHeadphones).unwrap();
        assert!(router.plays_to_headphones());
        assert!(router.shows_subtitles());
        assert_eq!(router.status_label(), "Subtitles + voice");
        assert_eq!(router.mix(), MixSettings::mixed());
    }

    #[test]
    fn changing_destination_updates_the_mix_preset() {
        let mut router = InboundRouter::new(game(), InboundDestination::SubtitlesOnly).unwrap();
        assert_eq!(router.mix(), MixSettings::translation_only());
        router.set_destination(InboundDestination::SubtitlesAndHeadphones);
        assert_eq!(router.mix(), MixSettings::mixed());
        assert!(router.plays_to_headphones());
    }

    #[test]
    fn manual_mix_override_is_preserved_and_applied() {
        let mut router = InboundRouter::new(game(), InboundDestination::SubtitlesOnly).unwrap();
        let custom = MixSettings::bypass();
        router.set_mix(custom);
        assert_eq!(router.mix(), custom);
    }

    #[test]
    fn inbound_source_is_always_application_loopback() {
        let router = InboundRouter::new(game(), InboundDestination::SubtitlesOnly).unwrap();
        assert_eq!(router.audio_source(), AudioSource::ApplicationLoopback);
        assert!(router.audio_source().is_translatable_input());
        assert!(!router.audio_source().is_output());
    }
}
