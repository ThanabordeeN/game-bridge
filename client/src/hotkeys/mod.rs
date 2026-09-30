//! Push-to-translate hotkeys (§11).
//!
//! Two configurations are supported, and they are opposites:
//!
//! ```text
//! Translate-on-hold:  Normal → original mic;  Hold F8 → translate
//! Bypass-on-hold:     Normal → translate;     Hold F8 → original voice
//! ```
//!
//! Both reduce to the same primitive — a key that is held — so the state is
//! modelled once and interpreted by [`HotkeyAction`]. That avoids the common
//! bug where the two configurations are implemented as separate toggles that
//! drift apart.
//!
//! # Why the manager is portable
//!
//! The key state machine, chord parsing, and repeat suppression are pure logic
//! and are tested here. The actual registration (`RegisterHotKey` on Windows)
//! lives behind `cfg`. What matters for correctness — that a held key produces
//! exactly one press and one release, and that the UI's configured binding is
//! valid — is all portable.

use serde::{Deserialize, Serialize};

pub mod manager;

pub use manager::HotkeyManager;

/// What holding the hotkey does (§11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyBehaviour {
    /// Hold to translate; release to pass the original voice through.
    #[default]
    TranslateWhileHeld,
    /// Hold to bypass translation; release to translate.
    BypassWhileHeld,
}

impl HotkeyBehaviour {
    /// Label for the Settings screen.
    pub const fn label(self) -> &'static str {
        match self {
            HotkeyBehaviour::TranslateWhileHeld => "Hold to translate",
            HotkeyBehaviour::BypassWhileHeld => "Hold to bypass (original voice)",
        }
    }

    /// The §11 example wording shown beside the binding.
    pub const fn description(self) -> &'static str {
        match self {
            HotkeyBehaviour::TranslateWhileHeld => "Normal: original microphone. Hold: translate.",
            HotkeyBehaviour::BypassWhileHeld => {
                "Normal: auto translate. Hold: your original voice."
            }
        }
    }
}

/// A key that can be bound.
///
/// A small closed set rather than raw virtual-key codes: the client only needs
/// to offer keys that are (a) reachable while gaming and (b) not swallowed by
/// every game's own bindings. Mouse buttons are included because they are the
/// most ergonomic and least contested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyKey {
    /// F8, the spec's example key.
    F6,
    /// F7.
    F7,
    /// F8.
    F8,
    /// F9.
    F9,
    /// F10.
    F10,
    /// The left mouse button.
    MouseLeft,
    /// The right mouse button.
    MouseRight,
    /// The middle mouse button.
    MouseMiddle,
    /// Mouse button 4, usually the thumb button.
    Mouse4,
    /// Mouse button 5.
    Mouse5,
    /// The caps lock key, which is otherwise unused in most games.
    CapsLock,
    /// The `\` key.
    Backslash,
    /// The grave/tilde key.
    Grave,
}

impl HotkeyKey {
    /// Label shown in Settings.
    pub const fn label(self) -> &'static str {
        match self {
            HotkeyKey::F6 => "F6",
            HotkeyKey::F7 => "F7",
            HotkeyKey::F8 => "F8",
            HotkeyKey::F9 => "F9",
            HotkeyKey::F10 => "F10",
            HotkeyKey::MouseLeft => "Mouse Left",
            HotkeyKey::MouseRight => "Mouse Right",
            HotkeyKey::MouseMiddle => "Mouse Middle",
            HotkeyKey::Mouse4 => "Mouse 4",
            HotkeyKey::Mouse5 => "Mouse 5",
            HotkeyKey::CapsLock => "Caps Lock",
            HotkeyKey::Backslash => "\\",
            HotkeyKey::Grave => "`",
        }
    }

    /// Windows virtual-key code, when this key has one.
    ///
    /// Mouse buttons are not virtual keys and return `None`; the Windows
    /// backend handles them through a low-level mouse hook instead.
    pub const fn virtual_key(self) -> Option<u32> {
        Some(match self {
            HotkeyKey::F6 => 0x75,
            HotkeyKey::F7 => 0x76,
            HotkeyKey::F8 => 0x77,
            HotkeyKey::F9 => 0x78,
            HotkeyKey::F10 => 0x79,
            HotkeyKey::CapsLock => 0x14,
            HotkeyKey::Backslash => 0xDC,
            HotkeyKey::Grave => 0xC0,
            // Mouse buttons need a hook, not a virtual key.
            HotkeyKey::MouseLeft
            | HotkeyKey::MouseRight
            | HotkeyKey::MouseMiddle
            | HotkeyKey::Mouse4
            | HotkeyKey::Mouse5 => return None,
        })
    }

    /// Whether this key is a mouse button.
    pub const fn is_mouse(self) -> bool {
        matches!(
            self,
            HotkeyKey::MouseLeft
                | HotkeyKey::MouseRight
                | HotkeyKey::MouseMiddle
                | HotkeyKey::Mouse4
                | HotkeyKey::Mouse5
        )
    }

    /// Every bindable key, in the order Settings lists them.
    pub const ALL: [HotkeyKey; 13] = [
        HotkeyKey::F6,
        HotkeyKey::F7,
        HotkeyKey::F8,
        HotkeyKey::F9,
        HotkeyKey::F10,
        HotkeyKey::Mouse4,
        HotkeyKey::Mouse5,
        HotkeyKey::MouseMiddle,
        HotkeyKey::CapsLock,
        HotkeyKey::Backslash,
        HotkeyKey::Grave,
        HotkeyKey::MouseLeft,
        HotkeyKey::MouseRight,
    ];
}

/// A hotkey binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    /// The key to hold.
    pub key: HotkeyKey,
    /// What holding it does.
    pub behaviour: HotkeyBehaviour,
    /// Whether the binding is active.
    pub enabled: bool,
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        // §11's example: F8, hold to translate.
        Self {
            key: HotkeyKey::F8,
            behaviour: HotkeyBehaviour::TranslateWhileHeld,
            enabled: true,
        }
    }
}

impl HotkeyBinding {
    /// Display string, e.g. `"F8 — Hold to translate"`.
    pub fn display(&self) -> String {
        format!("{} — {}", self.key.label(), self.behaviour.label())
    }

    /// Whether any binding is active at all.
    pub fn is_active(&self) -> bool {
        self.enabled
    }
}

/// What the session should do in response to hotkey state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// No change.
    None,
    /// Hold-to-translate engaged: start translating.
    EngageTranslation,
    /// Hold-to-translate released: return to pass-through.
    ReleaseTranslation,
    /// Hold-to-bypass engaged: pass the original voice through.
    EngageBypass,
    /// Hold-to-bypass released: return to translating.
    ReleaseBypass,
}

/// Translate a key transition into a session action.
pub fn action_for(binding: &HotkeyBinding, held: bool) -> HotkeyAction {
    if !binding.is_active() {
        return HotkeyAction::None;
    }
    match (binding.behaviour, held) {
        (HotkeyBehaviour::TranslateWhileHeld, true) => HotkeyAction::EngageTranslation,
        (HotkeyBehaviour::TranslateWhileHeld, false) => HotkeyAction::ReleaseTranslation,
        (HotkeyBehaviour::BypassWhileHeld, true) => HotkeyAction::EngageBypass,
        (HotkeyBehaviour::BypassWhileHeld, false) => HotkeyAction::ReleaseBypass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_binding_is_the_spec_example() {
        // §11: "Hold F8 → Translate".
        let binding = HotkeyBinding::default();
        assert_eq!(binding.key, HotkeyKey::F8);
        assert_eq!(binding.behaviour, HotkeyBehaviour::TranslateWhileHeld);
        assert!(binding.enabled);
        assert_eq!(binding.display(), "F8 — Hold to translate");
    }

    #[test]
    fn f_keys_map_to_their_windows_virtual_keys() {
        assert_eq!(HotkeyKey::F6.virtual_key(), Some(0x75));
        assert_eq!(HotkeyKey::F8.virtual_key(), Some(0x77));
        assert_eq!(HotkeyKey::F10.virtual_key(), Some(0x79));
    }

    #[test]
    fn mouse_buttons_have_no_virtual_key_and_say_so() {
        // They need a low-level mouse hook, so the binding code must be able to
        // tell the backend which path to take rather than registering a bogus
        // virtual key.
        for key in [
            HotkeyKey::MouseLeft,
            HotkeyKey::MouseRight,
            HotkeyKey::MouseMiddle,
            HotkeyKey::Mouse4,
            HotkeyKey::Mouse5,
        ] {
            assert!(key.is_mouse(), "{} should be a mouse key", key.label());
            assert_eq!(key.virtual_key(), None, "{} must not claim a VK", key.label());
        }
    }

    #[test]
    fn keyboard_keys_are_not_mouse_keys() {
        assert!(!HotkeyKey::F8.is_mouse());
        assert!(!HotkeyKey::CapsLock.is_mouse());
        assert!(!HotkeyKey::Grave.is_mouse());
    }

    #[test]
    fn every_bindable_key_has_a_label_and_a_home_in_the_list() {
        for key in HotkeyKey::ALL {
            assert!(!key.label().is_empty());
            assert!(
                key.virtual_key().is_some() || key.is_mouse(),
                "{} is neither a virtual key nor a mouse key",
                key.label()
            );
        }
    }

    #[test]
    fn translate_while_held_engages_and_releases_translation() {
        let binding = HotkeyBinding::default();
        assert_eq!(action_for(&binding, true), HotkeyAction::EngageTranslation);
        assert_eq!(
            action_for(&binding, false),
            HotkeyAction::ReleaseTranslation
        );
    }

    #[test]
    fn bypass_while_held_is_the_mirror_image() {
        // §11's second configuration: normal is auto-translate, holding gives
        // the user their own voice back.
        let binding = HotkeyBinding {
            behaviour: HotkeyBehaviour::BypassWhileHeld,
            ..Default::default()
        };
        assert_eq!(action_for(&binding, true), HotkeyAction::EngageBypass);
        assert_eq!(action_for(&binding, false), HotkeyAction::ReleaseBypass);
    }

    #[test]
    fn a_disabled_binding_produces_no_action() {
        let binding = HotkeyBinding {
            enabled: false,
            ..Default::default()
        };
        assert_eq!(action_for(&binding, true), HotkeyAction::None);
        assert_eq!(action_for(&binding, false), HotkeyAction::None);
        assert!(!binding.is_active());
    }

    #[test]
    fn behaviour_description_matches_the_spec_wording() {
        assert!(HotkeyBehaviour::TranslateWhileHeld
            .description()
            .contains("Hold: translate"));
        assert!(HotkeyBehaviour::BypassWhileHeld
            .description()
            .contains("Hold: your original voice"));
    }

    #[test]
    fn binding_round_trips_through_json() {
        let binding = HotkeyBinding {
            key: HotkeyKey::Mouse4,
            behaviour: HotkeyBehaviour::BypassWhileHeld,
            enabled: false,
        };
        let json = serde_json::to_string(&binding).unwrap();
        let back: HotkeyBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(back, binding);
    }

    #[test]
    fn keys_serialize_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&HotkeyKey::Mouse4).unwrap(),
            r#""mouse4""#
        );
        assert_eq!(
            serde_json::to_string(&HotkeyKey::CapsLock).unwrap(),
            r#""caps_lock""#
        );
    }
}
