//! Mixing of original and translated voice (§12).
//!
//! The user hears their own translated voice in their headphones, optionally
//! blended with their original voice. Two independent gains, both `0..=100%`,
//! with the documented presets.
//!
//! # Why the original is kept at all
//!
//! On a mixed setting the user hears a little of their own voice so they know
//! how loud they are being — the same reason headsets have sidetone. Dropping
//! to pure translation saves nothing and makes the headset feel dead.

use game_bridge_protocol::mode::RoutingMode;
use serde::{Deserialize, Serialize};

/// A gain in the range `0..=100`, stored as an integer percentage.
///
/// Integer percentages rather than a float: the value crosses the wire and
/// appears in a config file, and `20` is unambiguous where `0.2` invites a
/// units bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Gain(u8);

impl Gain {
    /// No signal.
    pub const MUTED: Gain = Gain(0);
    /// Unity gain.
    pub const FULL: Gain = Gain(100);

    /// Clamp a percentage into `0..=100`.
    pub const fn new(percent: u8) -> Self {
        Gain(if percent > 100 { 100 } else { percent })
    }

    /// The percentage.
    pub const fn percent(self) -> u8 {
        self.0
    }

    /// The gain as a linear multiplier in `0.0..=1.0`.
    pub fn factor(self) -> f32 {
        f32::from(self.0) / 100.0
    }

    /// Whether this gain is fully muted.
    pub const fn is_muted(self) -> bool {
        self.0 == 0
    }
}

impl Default for Gain {
    fn default() -> Self {
        Gain::FULL
    }
}

/// How loud each of the two voice paths should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixSettings {
    /// How much of the user's unmodified microphone is passed through.
    pub original: Gain,
    /// How much of the translated voice is played.
    pub translated: Gain,
}

impl Default for MixSettings {
    fn default() -> Self {
        // §12's "Mixed" preset is the sensible default: the user hears
        // themselves quietly and the translation clearly.
        Self::mixed()
    }
}

impl MixSettings {
    /// Translation only: original muted, translated at unity.
    pub const fn translation_only() -> Self {
        Self {
            original: Gain::MUTED,
            translated: Gain::FULL,
        }
    }

    /// Mixed: a little original under a full translated signal.
    pub const fn mixed() -> Self {
        Self {
            original: Gain(20),
            translated: Gain::FULL,
        }
    }

    /// Bypass: the original voice passes through untouched.
    ///
    /// Translated is muted rather than merely reduced. Bypass exists so the user
    /// can talk normally at a moment's notice, and hearing a delayed translated
    /// echo underneath their own voice is exactly what makes that unusable.
    pub const fn bypass() -> Self {
        Self {
            original: Gain::FULL,
            translated: Gain::MUTED,
        }
    }

    /// The preset matching a routing mode and bypass state.
    pub fn for_mode(mode: RoutingMode, bypassed: bool) -> Self {
        if bypassed {
            return Self::bypass();
        }
        match mode {
            RoutingMode::Subtitle => Self::translation_only(),
            RoutingMode::VoiceOut | RoutingMode::FullVoice => Self::mixed(),
        }
    }

    /// Whether the original voice contributes at all.
    pub const fn mixes_original(&self) -> bool {
        !self.original.is_muted()
    }

    /// Whether the translated voice contributes at all.
    pub const fn mixes_translated(&self) -> bool {
        !self.translated.is_muted()
    }
}

/// Mixes two mono `i16` streams into one.
///
/// Both inputs are expected to be the same length; the shorter one is treated
/// as zero-padded, which is what happens naturally when the translation arrives
/// slightly behind the original.
pub struct Mixer {
    settings: MixSettings,
}

impl Mixer {
    /// Create a mixer with the given settings.
    pub fn new(settings: MixSettings) -> Self {
        Self { settings }
    }

    /// Current settings.
    pub fn settings(&self) -> MixSettings {
        self.settings
    }

    /// Replace the settings. Takes effect on the next call.
    pub fn set_settings(&mut self, settings: MixSettings) {
        self.settings = settings;
    }

    /// Mix `original` and `translated` into `out`, returning the number of
    /// samples written.
    ///
    /// The result is clamped, not wrapped: integer overflow on an audio sum
    /// produces a loud crack, whereas clamping produces brief distortion. Only
    /// one of those is acceptable to ship.
    pub fn mix_into(&self, original: &[i16], translated: &[i16], out: &mut [i16]) -> usize {
        let count = original.len().max(translated.len()).min(out.len());
        let original_factor = self.settings.original.factor();
        let translated_factor = self.settings.translated.factor();

        for index in 0..count {
            let a = original.get(index).copied().unwrap_or(0);
            let b = translated.get(index).copied().unwrap_or(0);
            let mixed = f32::from(a) * original_factor + f32::from(b) * translated_factor;
            out[index] = mixed.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        }
        count
    }

    /// Convenience wrapper that allocates the output.
    pub fn mix(&self, original: &[i16], translated: &[i16]) -> Vec<i16> {
        let mut out = vec![0i16; original.len().max(translated.len())];
        let written = self.mix_into(original, translated, &mut out);
        out.truncate(written);
        out
    }

    /// Whether the mix is currently silent on both paths.
    pub fn is_silent(&self) -> bool {
        self.settings.original.is_muted() && self.settings.translated.is_muted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_scale(value: i16) -> Vec<i16> {
        vec![value; 8]
    }

    #[test]
    fn gain_clamps_above_one_hundred() {
        assert_eq!(Gain::new(255).percent(), 100);
        assert_eq!(Gain::new(101).percent(), 100);
        assert_eq!(Gain::new(0).percent(), 0);
    }

    #[test]
    fn gain_factor_maps_to_unit_range() {
        assert_eq!(Gain::new(0).factor(), 0.0);
        assert_eq!(Gain::new(50).factor(), 0.5);
        assert_eq!(Gain::new(100).factor(), 1.0);
    }

    #[test]
    fn documented_presets_match_the_spec() {
        // §12 gives exact numbers; these are contract, not preference.
        let only = MixSettings::translation_only();
        assert_eq!(only.original.percent(), 0);
        assert_eq!(only.translated.percent(), 100);

        let mixed = MixSettings::mixed();
        assert_eq!(mixed.original.percent(), 20);
        assert_eq!(mixed.translated.percent(), 100);

        let bypass = MixSettings::bypass();
        assert_eq!(bypass.original.percent(), 100);
        assert_eq!(bypass.translated.percent(), 0);
    }

    #[test]
    fn bypass_silences_translation_entirely() {
        let mixer = Mixer::new(MixSettings::bypass());
        let original = full_scale(10_000);
        let translated = full_scale(20_000);
        let out = mixer.mix(&original, &translated);
        assert_eq!(out, original, "bypass must pass the original untouched");
    }

    #[test]
    fn translation_only_silences_the_original() {
        let mixer = Mixer::new(MixSettings::translation_only());
        let original = full_scale(30_000);
        let translated = full_scale(1000);
        let out = mixer.mix(&original, &translated);
        assert_eq!(out, translated);
    }

    #[test]
    fn mixed_preset_weights_both_paths() {
        let mixer = Mixer::new(MixSettings::mixed());
        let out = mixer.mix(&full_scale(10_000), &full_scale(1_000));
        // 10000 * 0.20 + 1000 * 1.00 = 3000
        assert_eq!(out[0], 3000);
    }

    #[test]
    fn summing_past_full_scale_clamps_instead_of_wrapping() {
        // The bug this guards against: i16 overflow turning a loud mix into a
        // full-scale crackle of the opposite sign.
        let mixer = Mixer::new(MixSettings {
            original: Gain::FULL,
            translated: Gain::FULL,
        });
        let out = mixer.mix(&full_scale(i16::MAX), &full_scale(i16::MAX));
        assert_eq!(out[0], i16::MAX, "positive sum must clamp high");

        let out = mixer.mix(&full_scale(i16::MIN), &full_scale(i16::MIN));
        assert_eq!(out[0], i16::MIN, "negative sum must clamp low");
    }

    #[test]
    fn mismatched_lengths_are_zero_padded() {
        let mixer = Mixer::new(MixSettings::translation_only());
        let original = vec![5000i16; 2];
        let translated = vec![100i16; 5];
        let out = mixer.mix(&original, &translated);
        assert_eq!(out.len(), 5, "output spans the longer input");
        assert_eq!(out, vec![100i16; 5]);
    }

    #[test]
    fn mix_into_respects_a_short_output_buffer() {
        let mixer = Mixer::new(MixSettings::translation_only());
        let original = vec![1i16; 10];
        let translated = vec![2i16; 10];
        let mut out = [0i16; 3];
        let written = mixer.mix_into(&original, &translated, &mut out);
        assert_eq!(written, 3);
        assert_eq!(out, [2i16; 3]);
    }

    #[test]
    fn both_muted_reports_silent() {
        let mixer = Mixer::new(MixSettings {
            original: Gain::MUTED,
            translated: Gain::MUTED,
        });
        assert!(mixer.is_silent());
        assert_eq!(mixer.mix(&full_scale(1000), &full_scale(1000)), vec![0i16; 8]);
    }

    #[test]
    fn preset_follows_mode_and_bypass() {
        assert_eq!(
            MixSettings::for_mode(RoutingMode::Subtitle, false),
            MixSettings::translation_only()
        );
        assert_eq!(
            MixSettings::for_mode(RoutingMode::VoiceOut, false),
            MixSettings::mixed()
        );
        // Bypass wins regardless of mode.
        assert_eq!(
            MixSettings::for_mode(RoutingMode::FullVoice, true),
            MixSettings::bypass()
        );
    }

    #[test]
    fn settings_round_trip_through_json_as_percentages() {
        let json = serde_json::to_string(&MixSettings::mixed()).unwrap();
        assert_eq!(json, r#"{"original":20,"translated":100}"#);
        let back: MixSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MixSettings::mixed());
    }
}
