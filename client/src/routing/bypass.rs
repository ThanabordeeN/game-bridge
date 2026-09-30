//! Bypass state and push-to-translate arming (§11, §12).
//!
//! Bypass has three independent reasons to be engaged, and conflating them is a
//! real usability bug:
//!
//! * **Latched** — the user toggled bypass on and expects it to stay on.
//! * **Momentary** — the user is holding a key and expects release to restore
//!   translation.
//! * **Forced** — the client engaged it because translated audio cannot exist
//!   right now (a dropped connection, say).
//!
//! They are stored as three independent flags rather than one `active` boolean
//! with a "why", because they can overlap and each must be able to change
//! without clobbering the others. An earlier version stored a single reason, and
//! a dropped connection would erase a latch the user had deliberately set: when
//! the connection came back, translation silently resumed while the user
//! believed they were bypassed.

use serde::{Deserialize, Serialize};

/// Why bypass is engaged, in precedence order (most transient first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BypassTrigger {
    /// Held open by a hotkey; releases when the key is released.
    Momentary,
    /// Toggled on and left on until toggled off.
    Latched,
    /// Forced on by the client, e.g. because the connection dropped.
    Forced,
}

/// Whether translation is currently bypassed, and the independent reasons why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BypassState {
    /// The user's latched preference.
    latched: bool,
    /// Whether a momentary hotkey is currently held.
    momentary: bool,
    /// Whether the client has forced bypass.
    forced: bool,
}

impl BypassState {
    /// Translation active, bypass off.
    pub const fn inactive() -> Self {
        Self {
            latched: false,
            momentary: false,
            forced: false,
        }
    }

    /// Bypass engaged by a momentary hold.
    pub const fn momentary() -> Self {
        Self {
            latched: false,
            momentary: true,
            forced: false,
        }
    }

    /// Bypass engaged by a latched toggle.
    pub const fn latched() -> Self {
        Self {
            latched: true,
            momentary: false,
            forced: false,
        }
    }

    /// Bypass forced by the client.
    pub const fn forced() -> Self {
        Self {
            latched: false,
            momentary: false,
            forced: true,
        }
    }

    /// Whether bypass is active for any reason.
    pub const fn active(&self) -> bool {
        self.latched || self.momentary || self.forced
    }

    /// Whether translation is currently running.
    pub const fn is_translating(&self) -> bool {
        !self.active()
    }

    /// The dominant reason bypass is engaged, for display. `None` when not
    /// active.
    ///
    /// Precedence is forced, then momentary, then latched: the most transient
    /// reason is the most informative one to show, because it is the one that
    /// is about to change on its own.
    pub const fn trigger(&self) -> Option<BypassTrigger> {
        if self.forced {
            Some(BypassTrigger::Forced)
        } else if self.momentary {
            Some(BypassTrigger::Momentary)
        } else if self.latched {
            Some(BypassTrigger::Latched)
        } else {
            None
        }
    }

    /// Whether a momentary hold is active.
    pub const fn is_momentary(&self) -> bool {
        self.momentary
    }

    /// Whether the user's latch is on.
    pub const fn is_latched(&self) -> bool {
        self.latched
    }

    /// Whether bypass was forced by the client.
    pub const fn is_forced(&self) -> bool {
        self.forced
    }

    /// The state after a momentary hotkey is pressed.
    pub const fn on_momentary_press(self) -> Self {
        Self {
            momentary: true,
            ..self
        }
    }

    /// The state after a momentary hotkey is released.
    ///
    /// Only clears the momentary flag. A latch or a forced bypass is left
    /// exactly as it was.
    pub const fn on_momentary_release(self) -> Self {
        Self {
            momentary: false,
            ..self
        }
    }

    /// The state after the latch toggle is flipped.
    pub const fn on_latch_toggle(self) -> Self {
        Self {
            latched: !self.latched,
            ..self
        }
    }

    /// Force bypass on, e.g. on disconnect. Leaves the latch untouched.
    pub const fn on_forced(self) -> Self {
        Self { forced: true, ..self }
    }

    /// Release a forced bypass, e.g. after reconnecting. Leaves the latch and
    /// any held key untouched, so the user's choice survives the outage.
    pub const fn on_forced_release(self) -> Self {
        Self {
            forced: false,
            ..self
        }
    }

    /// The label shown in the mini player and tray menu (§30, §31).
    pub const fn label(&self) -> &'static str {
        match self.trigger() {
            Some(BypassTrigger::Momentary) => "Bypass (held)",
            Some(BypassTrigger::Latched) => "Bypass",
            Some(BypassTrigger::Forced) => "Bypass (reconnecting)",
            None => "Translate",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_translates() {
        let state = BypassState::default();
        assert!(!state.active());
        assert!(state.is_translating());
        assert_eq!(state.trigger(), None);
        assert_eq!(state.label(), "Translate");
    }

    #[test]
    fn holding_and_releasing_a_momentary_key_round_trips() {
        let held = BypassState::inactive().on_momentary_press();
        assert!(held.active());
        assert!(held.is_momentary());
        let released = held.on_momentary_release();
        assert_eq!(released, BypassState::inactive());
        assert!(released.is_translating());
    }

    #[test]
    fn releasing_a_momentary_key_does_not_clear_a_latch() {
        // The bug this prevents: the user latches bypass on, brushes the
        // push-to-talk key, and translation silently comes back.
        let latched = BypassState::inactive().on_latch_toggle();
        assert!(latched.is_latched());

        let after_press = latched.on_momentary_press();
        assert!(after_press.is_latched(), "a press must not downgrade a latch");
        assert!(after_press.active());

        let after_release = after_press.on_momentary_release();
        assert!(after_release.is_latched(), "a release must not clear a latch");
        assert!(after_release.active(), "the latch keeps bypass on");
    }

    #[test]
    fn pressing_while_already_momentary_stays_momentary() {
        let held = BypassState::momentary();
        assert_eq!(held.on_momentary_press(), held);
    }

    #[test]
    fn latch_toggles_both_ways() {
        let on = BypassState::inactive().on_latch_toggle();
        assert!(on.active() && on.is_latched());
        let off = on.on_latch_toggle();
        assert!(!off.active());
        assert!(off.is_translating());
    }

    #[test]
    fn a_forced_bypass_survives_momentary_and_latch_input() {
        // While the client has forced bypass — the connection is down and no
        // translated audio can exist — the state stays bypassed whatever the
        // user does.
        let forced = BypassState::inactive().on_forced();
        assert!(forced.is_forced());
        assert!(forced.on_momentary_press().active());
        assert!(forced.on_momentary_release().active());
        assert!(forced.on_latch_toggle().active());
    }

    #[test]
    fn toggling_the_latch_during_a_forced_bypass_is_remembered() {
        // The user presses their bypass toggle while the connection is down.
        // That intent must survive the reconnect.
        let forced = BypassState::inactive().on_forced();
        let toggled = forced.on_latch_toggle();
        assert!(toggled.is_latched(), "the user's intent is recorded");
        assert!(toggled.is_forced());

        let reconnected = toggled.on_forced_release();
        assert!(reconnected.is_latched(), "and it survives the reconnect");
        assert!(reconnected.active());
    }

    #[test]
    fn a_forced_bypass_releases_without_touching_the_latch() {
        // The regression this guards: a disconnect erasing a deliberate latch,
        // so translation silently resumes mid-game.
        let latched = BypassState::latched();
        let during_outage = latched.on_forced();
        assert!(during_outage.is_forced() && during_outage.is_latched());

        let after_outage = during_outage.on_forced_release();
        assert!(!after_outage.is_forced(), "the force is released");
        assert!(after_outage.is_latched(), "the latch is not");
        assert!(after_outage.active(), "still bypassed, as the user chose");
    }

    #[test]
    fn releasing_a_forced_bypass_that_was_not_forced_is_a_no_op() {
        let latched = BypassState::latched();
        assert_eq!(latched.on_forced_release(), latched);
    }

    #[test]
    fn trigger_precedence_prefers_the_most_transient_reason() {
        // Forced is reported first because it is the one that changes on its
        // own, so it is the most useful thing to tell the user.
        let all = BypassState {
            latched: true,
            momentary: true,
            forced: true,
        };
        assert_eq!(all.trigger(), Some(BypassTrigger::Forced));

        let momentary_and_latched = BypassState {
            latched: true,
            momentary: true,
            forced: false,
        };
        assert_eq!(momentary_and_latched.trigger(), Some(BypassTrigger::Momentary));

        assert_eq!(BypassState::latched().trigger(), Some(BypassTrigger::Latched));
    }

    #[test]
    fn labels_distinguish_the_three_reasons() {
        assert_eq!(BypassState::inactive().label(), "Translate");
        assert_eq!(BypassState::latched().label(), "Bypass");
        assert_eq!(BypassState::momentary().label(), "Bypass (held)");
        assert_eq!(BypassState::forced().label(), "Bypass (reconnecting)");
    }

    #[test]
    fn state_round_trips_through_json() {
        let json = serde_json::to_string(&BypassState::latched()).unwrap();
        let back: BypassState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, BypassState::latched());
        assert!(back.is_latched());
    }

    #[test]
    fn the_three_flags_are_independent() {
        // Exhaustive over all eight combinations: `active` is exactly the
        // disjunction, and clearing one flag never clears another.
        for bits in 0..8u8 {
            let state = BypassState {
                latched: bits & 1 != 0,
                momentary: bits & 2 != 0,
                forced: bits & 4 != 0,
            };
            let expected_active = bits != 0;
            assert_eq!(state.active(), expected_active, "bits {bits:03b}");
            assert_eq!(state.is_translating(), !expected_active);

            // Releasing momentary clears bit 1 and nothing else.
            let released = state.on_momentary_release();
            assert_eq!(released.latched, state.latched);
            assert_eq!(released.forced, state.forced);
            assert!(!released.momentary);

            // Releasing a forced bypass clears bit 4 and nothing else.
            let unf = state.on_forced_release();
            assert_eq!(unf.latched, state.latched);
            assert_eq!(unf.momentary, state.momentary);
            assert!(!unf.forced);
        }
    }
}
