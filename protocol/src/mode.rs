//! Routing modes and audio source tags.
//!
//! These drive both pricing (§16) and feedback protection (§13): the gateway
//! needs to know which tier a stream is billed at, and the client needs a
//! stable tag for every audio origin so it can never route a virtual
//! microphone back into the translation pipeline.

use serde::{Deserialize, Serialize};

/// Which way audio flows for a session (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    /// Incoming speech becomes subtitles only. Cheapest tier.
    Subtitle,
    /// Incoming speech becomes subtitles; the user's own speech is translated
    /// and voiced into the virtual microphone. The default mode.
    #[default]
    VoiceOut,
    /// Both directions are subtitled and voiced. Premium tier.
    FullVoice,
}

impl RoutingMode {
    /// Whether this mode produces TTS output for the user's own voice.
    pub const fn translates_own_voice(self) -> bool {
        matches!(self, RoutingMode::VoiceOut | RoutingMode::FullVoice)
    }

    /// Whether this mode produces TTS output for other players' speech.
    pub const fn translates_other_voices(self) -> bool {
        matches!(self, RoutingMode::FullVoice)
    }

    /// Whether this mode produces subtitle output.
    ///
    /// Always true today; every mode surfaces text. Kept explicit because a
    /// future audio-only mode would need it.
    pub const fn produces_subtitles(self) -> bool {
        true
    }

    /// Human-readable label for the routing cards (§27).
    pub const fn label(self) -> &'static str {
        match self {
            RoutingMode::Subtitle => "Subtitle",
            RoutingMode::VoiceOut => "Voice Out",
            RoutingMode::FullVoice => "Full Voice",
        }
    }

    /// Supporting copy for the routing cards (§27).
    pub const fn description(self) -> &'static str {
        match self {
            RoutingMode::Subtitle => "Text translation only",
            RoutingMode::VoiceOut => "Your voice translated into the game",
            RoutingMode::FullVoice => "Translate everyone, both directions",
        }
    }

    /// Every mode, in display order.
    pub const ALL: [RoutingMode; 3] = [
        RoutingMode::Subtitle,
        RoutingMode::VoiceOut,
        RoutingMode::FullVoice,
    ];
}

/// Where a piece of audio came from.
///
/// Every endpoint is tagged so the routing layer can assert that no audio
/// originating from [`AudioSource::VirtualMicLoopback`] ever enters the STT
/// path (§13). The client refuses to open a session whose input device
/// resolves to the virtual capture endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSource {
    /// The user's physical microphone. The normal translation input.
    PhysicalMic,
    /// Application loopback capture of a game or voice chat process.
    ApplicationLoopback,
    /// The other direction: TTS audio being written to the virtual microphone
    /// for the game to pick up.
    VirtualMicRender,
    /// The virtual capture endpoint as seen by another process. Must never be
    /// opened as a translation input.
    VirtualMicLoopback,
}

impl AudioSource {
    /// Whether audio from this source may be fed into STT/translation.
    ///
    /// This is the single guard that implements §13's feedback protection.
    pub const fn is_translatable_input(self) -> bool {
        match self {
            AudioSource::PhysicalMic | AudioSource::ApplicationLoopback => true,
            // Deliberately false: accepting either of these would create the
            // Virtual Mic → STT → TTS → Virtual Mic loop the spec forbids.
            AudioSource::VirtualMicRender | AudioSource::VirtualMicLoopback => false,
        }
    }

    /// Whether this source is an output destination rather than an input.
    pub const fn is_output(self) -> bool {
        matches!(self, AudioSource::VirtualMicRender)
    }
}

/// Which voice product a session uses, as shown to the user (§16, §24).
///
/// The user sees these three labels only. The underlying providers (Cartesia,
/// MiniMax) are an advanced-settings detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceTier {
    /// The bundled stock voice. No cloning.
    #[default]
    Standard,
    /// The user's own cloned voice via the primary TTS provider.
    Cloned,
    /// The higher-fidelity premium voice, billed at a higher burn rate.
    Premium,
}

impl VoiceTier {
    /// Label shown in the Voice screen (§24).
    pub const fn label(self) -> &'static str {
        match self {
            VoiceTier::Standard => "Standard Voice",
            VoiceTier::Cloned => "My Cloned Voice",
            VoiceTier::Premium => "Premium Voice",
        }
    }

    /// Whether this tier requires a one-time voice-clone enrollment (§25).
    pub const fn requires_clone_enrollment(self) -> bool {
        matches!(self, VoiceTier::Cloned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_mic_audio_is_never_a_translation_input() {
        // The core guarantee of §13. If this test ever fails, the product has a
        // feedback loop that will howl in a user's headset.
        assert!(!AudioSource::VirtualMicRender.is_translatable_input());
        assert!(!AudioSource::VirtualMicLoopback.is_translatable_input());
    }

    #[test]
    fn physical_sources_are_translatable() {
        assert!(AudioSource::PhysicalMic.is_translatable_input());
        assert!(AudioSource::ApplicationLoopback.is_translatable_input());
    }

    #[test]
    fn virtual_mic_render_is_the_only_output() {
        assert!(AudioSource::VirtualMicRender.is_output());
        assert!(!AudioSource::VirtualMicLoopback.is_output());
        assert!(!AudioSource::PhysicalMic.is_output());
    }

    #[test]
    fn default_routing_mode_is_voice_out() {
        assert_eq!(RoutingMode::default(), RoutingMode::VoiceOut);
    }

    #[test]
    fn routing_modes_agree_with_the_spec_table() {
        assert!(!RoutingMode::Subtitle.translates_own_voice());
        assert!(!RoutingMode::Subtitle.translates_other_voices());

        assert!(RoutingMode::VoiceOut.translates_own_voice());
        assert!(!RoutingMode::VoiceOut.translates_other_voices());

        assert!(RoutingMode::FullVoice.translates_own_voice());
        assert!(RoutingMode::FullVoice.translates_other_voices());
    }

    #[test]
    fn every_mode_is_in_the_all_list_in_card_order() {
        assert_eq!(RoutingMode::ALL.len(), 3);
        assert_eq!(RoutingMode::ALL[0], RoutingMode::Subtitle);
        assert_eq!(RoutingMode::ALL[2], RoutingMode::FullVoice);
    }

    #[test]
    fn routing_mode_serializes_as_snake_case() {
        let json = serde_json::to_string(&RoutingMode::VoiceOut).unwrap();
        assert_eq!(json, "\"voice_out\"");
    }

    #[test]
    fn only_cloned_tier_needs_enrollment() {
        assert!(VoiceTier::Cloned.requires_clone_enrollment());
        assert!(!VoiceTier::Standard.requires_clone_enrollment());
        assert!(!VoiceTier::Premium.requires_clone_enrollment());
    }
}
