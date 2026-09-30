//! The subtitle overlay (§32).
//!
//! The overlay is a borderless, always-on-top, **click-through** window that
//! renders the live translation over the game. It must never take focus, or
//! clicking near it would minimise the game mid-fight.
//!
//! # Why the model is portable and the window is not
//!
//! The line buffer, the queue, and the styling rules are pure logic and are
//! tested here. The window itself is a Windows layered window
//! (`WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE`) and lives behind
//! `cfg(target_os = "windows")`.
//!
//! # Line lifetime
//!
//! A subtitle must expire on its own. If lines only vanished when replaced, a
//! single line of dialogue followed by silence would sit on screen for the rest
//! of the match, and a stale callout is worse than none — it is actively
//! misleading in a competitive game. [`OverlayModel::tick`] is what removes it.

pub mod subtitle;
pub mod window;

pub use subtitle::{
    OverlayModel, OverlayStyle, SubtitleLine, TextMode, MAX_DURATION_SECS, MIN_DURATION_SECS,
};
pub use window::OverlayWindow;
