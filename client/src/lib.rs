//! The Game Bridge Windows desktop client.
//!
//! # Layout
//!
//! ```text
//! audio/     capture, loopback, playback, resampling, mixing, VAD
//! network/   websocket, protocol codec, reconnect, auth
//! session/   session lifecycle, usage, credits
//! routing/   inbound/outbound voice paths, bypass
//! overlay/   click-through subtitle window
//! device/    endpoint discovery and the virtual microphone
//! hotkeys/   push-to-translate hotkey manager
//! ui/        egui application shell
//! ```
//!
//! # Platform strategy
//!
//! The portable modules — [`audio::vad`], [`audio::mixer`],
//! [`audio::resampler`], [`audio::ring`], [`routing`], [`session`] — carry the
//! product's logic and compile with no system dependencies, so they are unit
//! tested on any platform. The Windows-specific modules are feature- and
//! `cfg`-gated and are **not verified on this checkout**; see the README's
//! capability matrix.

pub mod audio;
pub mod config;
pub mod device;
pub mod error;
pub mod hotkeys;
pub mod incoming;
pub mod net;
pub mod network;
pub mod overlay;
pub mod routing;
pub mod secret;
pub mod session;
pub mod transcribe;
pub mod translate;

#[cfg(feature = "gui")]
pub mod ui;

pub use error::{AudioError, ConfigError, NetworkError, SessionError};

/// The language pair a session translates between, in the direction the user
/// speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguagePair {
    /// The user's own language, spoken into the microphone.
    pub source: game_bridge_protocol::Language,
    /// The language other players hear and read.
    pub target: game_bridge_protocol::Language,
}

impl Default for LanguagePair {
    fn default() -> Self {
        Self {
            source: game_bridge_protocol::Language::Thai,
            target: game_bridge_protocol::Language::English,
        }
    }
}

impl LanguagePair {
    /// Create a pair.
    pub const fn new(
        source: game_bridge_protocol::Language,
        target: game_bridge_protocol::Language,
    ) -> Self {
        Self { source, target }
    }

    /// Render as the UI does: `"TH → EN"`.
    pub fn short_display(&self) -> String {
        format!(
            "{} → {}",
            self.source.short_tag(),
            self.target.short_tag()
        )
    }

    /// Swap the direction.
    pub const fn reversed(self) -> Self {
        Self {
            source: self.target,
            target: self.source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_bridge_protocol::Language;

    #[test]
    fn default_pair_is_thai_to_english() {
        let pair = LanguagePair::default();
        assert_eq!(pair.source, Language::Thai);
        assert_eq!(pair.target, Language::English);
    }

    #[test]
    fn short_display_matches_the_hero_card() {
        // §19 and §21 show "TH → EN".
        assert_eq!(LanguagePair::default().short_display(), "TH → EN");
    }

    #[test]
    fn reversing_swaps_both_sides() {
        let pair = LanguagePair::new(Language::English, Language::Japanese);
        let reversed = pair.reversed();
        assert_eq!(reversed.source, Language::Japanese);
        assert_eq!(reversed.target, Language::English);
        assert_eq!(reversed.reversed(), pair);
    }
}
