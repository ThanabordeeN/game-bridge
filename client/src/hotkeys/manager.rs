//! Hotkey state tracking.
//!
//! The portable half of §11: turning raw key-down/key-up events into exactly one
//! press and one release, no matter how the operating system repeats them.
//!
//! # Key repeat
//!
//! Holding a key on Windows generates a stream of repeated key-down events.
//! Without suppression, a bound key that the user holds for two seconds would
//! produce a dozen "engage" transitions, each one a protocol message and a
//! state change. [`HotkeyManager`] tracks which keys are already down and
//! reports a transition only when the state actually changes.

use std::collections::HashSet;

use super::{action_for, HotkeyAction, HotkeyBinding, HotkeyKey};

/// Tracks key state and emits actions on transitions.
#[derive(Debug)]
pub struct HotkeyManager {
    binding: HotkeyBinding,
    /// Keys currently held down.
    held: HashSet<HotkeyKey>,
    /// Number of transitions emitted, for the Settings screen's "test" readout.
    transitions: u64,
}

impl HotkeyManager {
    /// Create a manager with a binding.
    pub fn new(binding: HotkeyBinding) -> Self {
        Self {
            binding,
            held: HashSet::new(),
            transitions: 0,
        }
    }

    /// The current binding.
    pub fn binding(&self) -> HotkeyBinding {
        self.binding
    }

    /// Replace the binding, releasing any held state.
    ///
    /// Releasing is deliberate: rebinding while a key is held would otherwise
    /// leave the old key's action engaged forever, with no key that can turn it
    /// off.
    pub fn set_binding(&mut self, binding: HotkeyBinding) -> HotkeyAction {
        // Capture the old binding before overwriting it. An earlier version
        // checked `binding.key` — the *new* key — so rebinding away from a held
        // key never emitted the release, leaving translation engaged with no
        // way to disengage it.
        let was_held = self.is_held();
        let old_binding = self.binding;
        self.binding = binding;
        self.held.clear();
        if was_held {
            // Emit the release against the binding that was actually held.
            self.transitions += 1;
            return action_for(&old_binding, false);
        }
        HotkeyAction::None
    }

    /// Whether the bound key is currently held.
    pub fn is_held(&self) -> bool {
        self.held.contains(&self.binding.key)
    }

    /// How many transitions have been emitted.
    pub fn transitions(&self) -> u64 {
        self.transitions
    }

    /// Report a key-down event. Returns an action only on a real transition.
    pub fn on_key_down(&mut self, key: HotkeyKey) -> HotkeyAction {
        if key != self.binding.key {
            return HotkeyAction::None;
        }
        // Suppress OS key repeat: only the first down counts.
        if !self.held.insert(key) {
            return HotkeyAction::None;
        }
        self.transitions += 1;
        action_for(&self.binding, true)
    }

    /// Report a key-up event. Returns an action only if the key was down.
    pub fn on_key_up(&mut self, key: HotkeyKey) -> HotkeyAction {
        if key != self.binding.key {
            return HotkeyAction::None;
        }
        if !self.held.remove(&key) {
            return HotkeyAction::None;
        }
        self.transitions += 1;
        action_for(&self.binding, false)
    }

    /// Release everything, e.g. when the session stops or the window loses
    /// focus. Returns the action needed to return to normal.
    pub fn release_all(&mut self) -> HotkeyAction {
        if self.held.remove(&self.binding.key) {
            self.transitions += 1;
            action_for(&self.binding, false)
        } else {
            HotkeyAction::None
        }
    }

    /// Which physical key the backend should register, if any.
    pub fn registration_target(&self) -> Option<HotkeyKey> {
        if self.binding.is_active() {
            Some(self.binding.key)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkeys::HotkeyBehaviour;

    fn manager() -> HotkeyManager {
        HotkeyManager::new(HotkeyBinding::default())
    }

    #[test]
    fn a_key_down_produces_one_engage_action() {
        let mut manager = manager();
        assert_eq!(
            manager.on_key_down(HotkeyKey::F8),
            HotkeyAction::EngageTranslation
        );
        assert!(manager.is_held());
    }

    #[test]
    fn os_key_repeat_does_not_produce_repeated_actions() {
        // The bug this prevents: holding F8 for a second floods the gateway
        // with duplicate control events and re-triggers session state.
        let mut manager = manager();
        assert_eq!(
            manager.on_key_down(HotkeyKey::F8),
            HotkeyAction::EngageTranslation
        );
        for _ in 0..50 {
            assert_eq!(
                manager.on_key_down(HotkeyKey::F8),
                HotkeyAction::None,
                "repeat should be suppressed"
            );
        }
        assert_eq!(manager.transitions(), 1);
    }

    #[test]
    fn key_up_produces_exactly_one_release() {
        let mut manager = manager();
        manager.on_key_down(HotkeyKey::F8);
        assert_eq!(
            manager.on_key_up(HotkeyKey::F8),
            HotkeyAction::ReleaseTranslation
        );
        assert!(!manager.is_held());
        // A duplicate up is a no-op.
        assert_eq!(manager.on_key_up(HotkeyKey::F8), HotkeyAction::None);
        assert_eq!(manager.transitions(), 2);
    }

    #[test]
    fn a_full_press_and_release_cycle_is_symmetric() {
        let mut manager = manager();
        let press = manager.on_key_down(HotkeyKey::F8);
        let release = manager.on_key_up(HotkeyKey::F8);
        assert_eq!(press, HotkeyAction::EngageTranslation);
        assert_eq!(release, HotkeyAction::ReleaseTranslation);
        assert!(!manager.is_held());
    }

    #[test]
    fn unbound_keys_are_ignored() {
        let mut manager = manager();
        assert_eq!(manager.on_key_down(HotkeyKey::F6), HotkeyAction::None);
        assert_eq!(manager.on_key_up(HotkeyKey::F6), HotkeyAction::None);
        assert!(!manager.is_held());
        assert_eq!(manager.transitions(), 0);
    }

    #[test]
    fn a_mouse_binding_works_the_same_way() {
        let mut manager = HotkeyManager::new(HotkeyBinding {
            key: HotkeyKey::Mouse4,
            ..Default::default()
        });
        assert_eq!(
            manager.on_key_down(HotkeyKey::Mouse4),
            HotkeyAction::EngageTranslation
        );
        assert_eq!(
            manager.on_key_up(HotkeyKey::Mouse4),
            HotkeyAction::ReleaseTranslation
        );
    }

    #[test]
    fn rebinding_while_held_releases_the_old_action() {
        // Otherwise the engage action stays on forever with no key left to
        // release it.
        let mut manager = manager();
        manager.on_key_down(HotkeyKey::F8);
        assert!(manager.is_held());

        let action = manager.set_binding(HotkeyBinding {
            key: HotkeyKey::F6,
            ..Default::default()
        });
        assert_eq!(action, HotkeyAction::ReleaseTranslation);
        assert!(!manager.is_held(), "the old key must be released");
    }

    #[test]
    fn rebinding_while_idle_produces_no_action() {
        let mut manager = manager();
        let action = manager.set_binding(HotkeyBinding {
            key: HotkeyKey::F6,
            ..Default::default()
        });
        assert_eq!(action, HotkeyAction::None);
    }

    #[test]
    fn release_all_returns_to_normal() {
        let mut manager = manager();
        manager.on_key_down(HotkeyKey::F8);
        assert_eq!(
            manager.release_all(),
            HotkeyAction::ReleaseTranslation
        );
        assert!(!manager.is_held());

        // Idempotent when nothing is held.
        assert_eq!(manager.release_all(), HotkeyAction::None);
    }

    #[test]
    fn a_disabled_binding_registers_nothing() {
        let mut manager = HotkeyManager::new(HotkeyBinding {
            enabled: false,
            ..Default::default()
        });
        assert_eq!(manager.registration_target(), None);
        // And even a synthetic key event changes nothing.
        assert_eq!(manager.on_key_down(HotkeyKey::F8), HotkeyAction::None);
    }

    #[test]
    fn an_enabled_binding_reports_its_registration_target() {
        let manager = manager();
        assert_eq!(manager.registration_target(), Some(HotkeyKey::F8));
    }

    #[test]
    fn bypass_held_binding_engages_and_releases_bypass() {
        let mut manager = HotkeyManager::new(HotkeyBinding {
            behaviour: HotkeyBehaviour::BypassWhileHeld,
            ..Default::default()
        });
        assert_eq!(
            manager.on_key_down(HotkeyKey::F8),
            HotkeyAction::EngageBypass
        );
        assert_eq!(
            manager.on_key_up(HotkeyKey::F8),
            HotkeyAction::ReleaseBypass
        );
    }

    #[test]
    fn transitions_are_counted_for_the_settings_readout() {
        let mut manager = manager();
        manager.on_key_down(HotkeyKey::F8);
        manager.on_key_down(HotkeyKey::F8);
        manager.on_key_up(HotkeyKey::F8);
        manager.on_key_up(HotkeyKey::F8);
        assert_eq!(manager.transitions(), 2, "one press, one release");
    }
}
