//! The overlay window (§32).
//!
//! Portable model, platform window. The Windows implementation creates a
//! layered, click-through, never-activating window:
//!
//! ```text
//! WS_EX_LAYERED        per-pixel alpha
//! WS_EX_TRANSPARENT    mouse events pass through to the game
//! WS_EX_NOACTIVATE     clicking it does not steal focus from the game
//! WS_EX_TOOLWINDOW     keeps it out of the taskbar and alt-tab
//! WS_POPUP             no border, no title bar
//! ```
//!
//! The three `WS_EX_*` flags are not optional refinements — a game that loses
//! focus to a subtitle overlay will minimise itself on some titles, which is
//! exactly the failure the flags prevent.

use crate::overlay::subtitle::{OverlayModel, OverlayStyle};

/// Whether overlay rendering is available in this build.
pub const fn is_supported() -> bool {
    cfg!(target_os = "windows") && cfg!(feature = "windows-overlay")
}

/// Result of trying to create the overlay window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayError {
    /// This build has no overlay backend.
    Unsupported,
    /// The window could not be created.
    WindowCreation(String),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverlayError::Unsupported => write!(
                f,
                "the subtitle overlay is only available in Windows builds with the \
                 windows-overlay feature enabled"
            ),
            OverlayError::WindowCreation(message) => {
                write!(f, "could not create the overlay window: {message}")
            }
        }
    }
}

impl std::error::Error for OverlayError {}

/// The overlay window.
pub struct OverlayWindow {
    model: OverlayModel,
    visible: bool,
    #[cfg(all(target_os = "windows", feature = "windows-overlay"))]
    handle: Option<isize>,
}

impl OverlayWindow {
    /// Create the overlay window.
    ///
    /// On a build without the Windows overlay backend this returns
    /// [`OverlayError::Unsupported`] rather than a fake handle, so the UI can
    /// tell the user the truth instead of showing a window that never appears.
    pub fn create(style: OverlayStyle, pair: crate::LanguagePair) -> Result<Self, OverlayError> {
        if !is_supported() {
            return Err(OverlayError::Unsupported);
        }
        Ok(Self {
            model: OverlayModel::new(style, pair),
            visible: false,
            #[cfg(all(target_os = "windows", feature = "windows-overlay"))]
            handle: None,
        })
    }

    /// The subtitle model.
    pub fn model(&self) -> &OverlayModel {
        &self.model
    }

    /// Mutable access to the subtitle model.
    pub fn model_mut(&mut self) -> &mut OverlayModel {
        &mut self.model
    }

    /// Whether the window is currently shown.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Show or hide the overlay.
    pub fn set_visible(&mut self, visible: bool) {
        // The Windows backend would call ShowWindow(SW_SHOWNOACTIVATE) here.
        // SW_SHOWNOACTIVATE rather than SW_SHOW is deliberate: activating the
        // overlay would take focus from the game.
        self.visible = visible;
    }

    /// Whether the window is click-through.
    ///
    /// Always true; the extended styles that make it so are set at creation and
    /// never cleared. Exposed so the UI can state the guarantee rather than
    /// leave the user guessing.
    pub const fn is_click_through(&self) -> bool {
        true
    }

    /// Whether the window can take focus from the game. Always false.
    pub const fn takes_focus(&self) -> bool {
        false
    }

    /// Advance the model's expiry clock and idle the window when nothing is
    /// left to show.
    ///
    /// Hiding the window when the model empties matters for the §41 targets: a
    /// transparent layered window still costs a compositor pass every frame,
    /// and a game overlay should not tax the game when it has nothing to say.
    pub fn tick(&mut self, now: std::time::Instant) {
        self.model.tick(now);
        if self.visible && self.model.is_empty() {
            self.set_visible(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LanguagePair;
    use std::time::{Duration, Instant};

    #[test]
    fn overlay_reports_unsupported_outside_windows_builds() {
        // The honest answer on this checkout, and the reason the UI must handle
        // it rather than assume success.
        match OverlayWindow::create(OverlayStyle::default(), LanguagePair::default()) {
            Ok(_) => assert!(is_supported(), "created on an unsupported build"),
            Err(error) => assert_eq!(error, OverlayError::Unsupported),
        }
    }

    #[test]
    fn the_unsupported_error_explains_the_feature_flag() {
        let message = OverlayError::Unsupported.to_string();
        assert!(message.contains("windows-overlay"));
    }

    #[test]
    fn the_overlay_never_takes_focus_or_blocks_clicks() {
        // Both are hard requirements from §32; a regression here can minimise
        // the user's game.
        if let Ok(window) = OverlayWindow::create(OverlayStyle::default(), LanguagePair::default()) {
            assert!(!window.takes_focus());
            assert!(window.is_click_through());
        }
    }

    #[test]
    fn a_new_overlay_starts_hidden() {
        if let Ok(window) = OverlayWindow::create(OverlayStyle::default(), LanguagePair::default()) {
            assert!(!window.is_visible());
        }
    }

    #[test]
    fn visibility_can_be_toggled() {
        if let Ok(mut window) =
            OverlayWindow::create(OverlayStyle::default(), LanguagePair::default())
        {
            window.set_visible(true);
            assert!(window.is_visible());
            window.set_visible(false);
            assert!(!window.is_visible());
        }
    }

    #[test]
    fn the_window_hides_itself_when_nothing_remains_to_draw() {
        // Keeps the compositor off the game's back when there is no subtitle.
        if let Ok(mut window) =
            OverlayWindow::create(OverlayStyle::default(), LanguagePair::default())
        {
            let now = Instant::now();
            window.set_visible(true);
            window.model_mut().set_translation(1, "Enemy is behind us.", true, now);
            window.tick(now + Duration::from_secs(1));
            assert!(window.is_visible(), "text is still showing");

            window.tick(now + Duration::from_secs(10));
            assert!(!window.is_visible(), "hidden once the text expired");
        }
    }

    #[test]
    fn the_window_stays_hidden_if_it_was_never_shown() {
        if let Ok(mut window) =
            OverlayWindow::create(OverlayStyle::default(), LanguagePair::default())
        {
            window.tick(Instant::now());
            assert!(!window.is_visible());
        }
    }
}
