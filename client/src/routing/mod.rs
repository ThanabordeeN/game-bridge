//! Voice routing: which audio goes where (§10, §12, §13).
//!
//! The routing layer is where the spec's promises become invariants:
//!
//! * **§13** — audio never flows from the virtual microphone back into
//!   translation. [`OutboundRouter`] can only be constructed with an endpoint
//!   that passed the capture guard, and [`InboundRouter`] refuses to emit into
//!   a path that would recurse.
//! * **§10** — a routing mode determines which direction is active. The router
//!   exposes that as data ([`RoutingPlan`]) so the UI and the session use the
//!   same source of truth.
//! * **§12** — bypass and mix are evaluated in one place, so a bypass toggled
//!   mid-utterance cannot leave a translated tail playing.

pub mod bypass;
pub mod inbound;
pub mod outbound;

pub use bypass::{BypassState, BypassTrigger};
pub use inbound::InboundRouter;
pub use outbound::OutboundRouter;

use game_bridge_protocol::mode::{AudioSource, RoutingMode};
use serde::{Deserialize, Serialize};

/// Which audio paths a routing mode activates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingPlan {
    /// The mode this plan was derived from.
    pub mode: RoutingMode,
    /// Capture the user's microphone and translate it into the virtual mic.
    pub own_voice_out: bool,
    /// Capture the microphone for subtitles.
    pub own_voice_subtitles: bool,
    /// Capture other players' audio and subtitle it.
    pub other_voices_subtitles: bool,
    /// Translate other players' audio into TTS on the headphones.
    pub other_voices_out: bool,
}

impl RoutingPlan {
    /// Derive the plan for a mode (§10).
    pub const fn for_mode(mode: RoutingMode) -> Self {
        match mode {
            // Mode A: others are subtitled; the user's own voice is untouched.
            RoutingMode::Subtitle => Self {
                mode,
                own_voice_out: false,
                own_voice_subtitles: false,
                other_voices_subtitles: true,
                other_voices_out: false,
            },
            // Mode B: the user's voice is translated out; others are subtitled.
            RoutingMode::VoiceOut => Self {
                mode,
                own_voice_out: true,
                own_voice_subtitles: false,
                other_voices_subtitles: true,
                other_voices_out: false,
            },
            // Mode C: everything, both directions.
            RoutingMode::FullVoice => Self {
                mode,
                own_voice_out: true,
                own_voice_subtitles: true,
                other_voices_subtitles: true,
                other_voices_out: true,
            },
        }
    }

    /// Whether anything needs the physical microphone open.
    pub const fn needs_microphone(&self) -> bool {
        self.own_voice_out || self.own_voice_subtitles
    }

    /// Whether anything needs application loopback open.
    pub const fn needs_application_capture(&self) -> bool {
        self.other_voices_subtitles || self.other_voices_out
    }

    /// Whether anything needs the virtual microphone open for writing.
    pub const fn needs_virtual_mic(&self) -> bool {
        self.own_voice_out
    }

    /// Whether anything needs the headphone render path open.
    pub const fn needs_headphones(&self) -> bool {
        self.other_voices_out
    }
}

/// One audio path through the system, with its source tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPath {
    /// Where this audio originates.
    pub source: AudioSource,
    /// Whether it is subtitled.
    pub to_subtitles: bool,
    /// Whether it is voiced into the virtual microphone.
    pub to_virtual_mic: bool,
    /// Whether it is voiced to the user's headphones.
    pub to_headphones: bool,
}

/// Build the complete set of active paths for a plan.
///
/// The user's own voice is tagged [`AudioSource::PhysicalMic`]; other players'
/// audio is [`AudioSource::ApplicationLoopback`]. Neither is ever a virtual
/// endpoint, which is what makes the §13 guarantee structural rather than
/// merely conventional.
pub fn paths_for(plan: RoutingPlan) -> Vec<AudioPath> {
    let mut paths = Vec::new();

    if plan.own_voice_out || plan.own_voice_subtitles {
        paths.push(AudioPath {
            source: AudioSource::PhysicalMic,
            to_subtitles: plan.own_voice_subtitles,
            to_virtual_mic: plan.own_voice_out,
            to_headphones: false,
        });
    }

    if plan.other_voices_subtitles || plan.other_voices_out {
        paths.push(AudioPath {
            source: AudioSource::ApplicationLoopback,
            to_subtitles: plan.other_voices_subtitles,
            to_virtual_mic: false,
            to_headphones: plan.other_voices_out,
        });
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_a_subtitles_others_and_leaves_the_user_alone() {
        let plan = RoutingPlan::for_mode(RoutingMode::Subtitle);
        assert!(!plan.own_voice_out);
        assert!(!plan.own_voice_subtitles);
        assert!(plan.other_voices_subtitles);
        assert!(!plan.other_voices_out);
        assert!(!plan.needs_microphone(), "Mode A needs no microphone");
        assert!(plan.needs_application_capture());
        assert!(!plan.needs_virtual_mic());
        assert!(!plan.needs_headphones());
    }

    #[test]
    fn mode_b_is_the_documented_default_shape() {
        // §10: "Me → STT → Translate → Voice Clone TTS → Virtual Mic" and
        // "Others → Subtitle".
        let plan = RoutingPlan::for_mode(RoutingMode::VoiceOut);
        assert!(plan.own_voice_out);
        assert!(!plan.own_voice_subtitles);
        assert!(plan.other_voices_subtitles);
        assert!(!plan.other_voices_out);
        assert!(plan.needs_microphone());
        assert!(plan.needs_virtual_mic());
        assert!(!plan.needs_headphones());
    }

    #[test]
    fn mode_c_activates_every_path() {
        let plan = RoutingPlan::for_mode(RoutingMode::FullVoice);
        assert!(plan.own_voice_out);
        assert!(plan.own_voice_subtitles);
        assert!(plan.other_voices_subtitles);
        assert!(plan.other_voices_out);
        assert!(plan.needs_microphone());
        assert!(plan.needs_application_capture());
        assert!(plan.needs_virtual_mic());
        assert!(plan.needs_headphones());
    }

    #[test]
    fn no_plan_ever_routes_audio_out_of_the_virtual_microphone() {
        // The §13 guarantee expressed over the whole mode space: every path
        // built for every mode draws from a real capture source.
        for mode in RoutingMode::ALL {
            for path in paths_for(RoutingPlan::for_mode(mode)) {
                assert!(
                    path.source.is_translatable_input(),
                    "{mode:?} produced a path from {:?}",
                    path.source
                );
                assert_ne!(path.source, AudioSource::VirtualMicLoopback);
                assert_ne!(path.source, AudioSource::VirtualMicRender);
            }
        }
    }

    #[test]
    fn only_the_own_voice_path_feeds_the_virtual_microphone() {
        // Other players' speech must never be re-voiced into the user's mic,
        // which would make the client talk over the user.
        for mode in RoutingMode::ALL {
            for path in paths_for(RoutingPlan::for_mode(mode)) {
                if path.to_virtual_mic {
                    assert_eq!(path.source, AudioSource::PhysicalMic, "{mode:?}");
                }
            }
        }
    }

    #[test]
    fn only_the_other_voice_path_feeds_the_headphones() {
        for mode in RoutingMode::ALL {
            for path in paths_for(RoutingPlan::for_mode(mode)) {
                if path.to_headphones {
                    assert_eq!(path.source, AudioSource::ApplicationLoopback, "{mode:?}");
                }
            }
        }
    }

    #[test]
    fn subtitle_mode_opens_a_single_path() {
        let paths = paths_for(RoutingPlan::for_mode(RoutingMode::Subtitle));
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].source, AudioSource::ApplicationLoopback);
    }

    #[test]
    fn plan_matches_the_mode_helpers() {
        // RoutingPlan and RoutingMode must not disagree about what a mode does.
        for mode in RoutingMode::ALL {
            let plan = RoutingPlan::for_mode(mode);
            assert_eq!(plan.own_voice_out, mode.translates_own_voice(), "{mode:?}");
            assert_eq!(
                plan.other_voices_out,
                mode.translates_other_voices(),
                "{mode:?}"
            );
        }
    }
}
