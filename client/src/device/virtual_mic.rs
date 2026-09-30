//! The virtual microphone driver contract (§9, §40).
//!
//! The driver is a separate deliverable in `virtual-audio-driver/`, signed
//! separately and installed by the user. The client's job is to detect whether
//! it is present, tell the user honestly when it is not, and offer the install
//! path — not to silently pretend translation is working when there is nowhere
//! for the translated voice to go.
//!
//! # Why the driver cannot ship inside the client binary
//!
//! A Windows audio driver is a kernel-mode component that must be installed
//! with administrator rights and pass Microsoft's signing process. It cannot be
//! an ordinary dependency of a user-mode application.

use serde::{Deserialize, Serialize};

use crate::device::detection::VIRTUAL_DEVICE_NAME;

/// Whether the virtual microphone is usable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VirtualMicStatus {
    /// The driver is installed and the endpoint is present.
    Ready,
    /// The driver is not installed. The §40 error state, with an install path.
    NotInstalled,
    /// The driver is installed but the endpoint is not currently present, e.g.
    /// the driver was disabled in Device Manager or is mid-update.
    EndpointMissing,
    /// The driver is present but the client cannot write to it.
    Unavailable {
        /// Reason, safe to show the user.
        reason: String,
    },
}

impl VirtualMicStatus {
    /// Whether translated voice has somewhere to go.
    pub const fn is_usable(&self) -> bool {
        matches!(self, VirtualMicStatus::Ready)
    }

    /// The headline shown in the §40 error card.
    pub const fn headline(&self) -> &'static str {
        match self {
            VirtualMicStatus::Ready => "Ready",
            VirtualMicStatus::NotInstalled => "Virtual Microphone Not Found",
            VirtualMicStatus::EndpointMissing => "Virtual Microphone Unavailable",
            VirtualMicStatus::Unavailable { .. } => "Virtual Microphone Unavailable",
        }
    }

    /// The body copy shown in the §40 error card.
    pub fn detail(&self) -> String {
        match self {
            VirtualMicStatus::Ready => {
                format!("{VIRTUAL_DEVICE_NAME} is installed and ready to use.")
            }
            VirtualMicStatus::NotInstalled => {
                "Game Bridge Audio Driver is not installed. Translated voice has nowhere to go \
                 until it is."
                    .to_string()
            }
            VirtualMicStatus::EndpointMissing => {
                "The Game Bridge audio driver is installed, but its microphone endpoint is not \
                 available. It may be disabled in Device Manager, or Windows may still be \
                 finishing the installation."
                    .to_string()
            }
            VirtualMicStatus::Unavailable { reason } => reason.clone(),
        }
    }

    /// Whether the UI should offer an install action.
    pub const fn offers_install(&self) -> bool {
        matches!(self, VirtualMicStatus::NotInstalled)
    }

    /// Whether translation can still run in a degraded mode.
    ///
    /// It always can: subtitles do not need the driver, and the outbound router
    /// falls back to microphone pass-through so the user stays audible. The
    /// client degrades rather than refusing to start.
    pub const fn allows_subtitles(&self) -> bool {
        true
    }
}

/// The client's handle on the virtual microphone.
///
/// Implemented by the Windows backend; the trait exists so the UI and routing
/// layers never depend on driver details.
pub trait VirtualMicDriver: Send {
    /// Probe the current status.
    fn status(&self) -> VirtualMicStatus;

    /// Write PCM to the virtual microphone.
    ///
    /// `sample_rate_hz` and `channels` describe `samples`; the driver resamples
    /// if the endpoint format differs.
    fn write(&mut self, samples: &[i16], sample_rate_hz: u32, channels: u16) -> Result<(), String>;

    /// The endpoint's native sample rate, after negotiation.
    fn native_sample_rate_hz(&self) -> u32;

    /// Samples still queued in the driver, for the latency readout.
    fn queued_samples(&self) -> usize;
}

/// A driver handle used when the Windows feature is not compiled in.
///
/// Reports [`VirtualMicStatus::NotInstalled`] rather than pretending to work.
/// This exists so the UI can be developed and screenshotted off Windows without
/// a fake "ready" state that would hide the §40 screen entirely.
#[derive(Debug, Default)]
pub struct UnavailableVirtualMic;

impl VirtualMicDriver for UnavailableVirtualMic {
    fn status(&self) -> VirtualMicStatus {
        VirtualMicStatus::NotInstalled
    }

    fn write(&mut self, _samples: &[i16], _sample_rate_hz: u32, _channels: u16) -> Result<(), String> {
        Err("the Game Bridge virtual audio driver is not available on this platform".to_string())
    }

    fn native_sample_rate_hz(&self) -> u32 {
        48_000
    }

    fn queued_samples(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_status_is_usable() {
        assert!(VirtualMicStatus::Ready.is_usable());
        assert_eq!(VirtualMicStatus::Ready.headline(), "Ready");
        assert!(!VirtualMicStatus::Ready.offers_install());
    }

    #[test]
    fn missing_driver_matches_the_error_state_copy() {
        // §40 specifies this headline exactly.
        let status = VirtualMicStatus::NotInstalled;
        assert_eq!(status.headline(), "Virtual Microphone Not Found");
        assert!(status.detail().contains("Game Bridge Audio Driver is not installed"));
        assert!(status.offers_install());
        assert!(!status.is_usable());
    }

    #[test]
    fn missing_driver_still_allows_subtitles() {
        // Degrading is the point: a user without the driver can still read
        // their teammates, and their own voice still reaches the game.
        assert!(VirtualMicStatus::NotInstalled.allows_subtitles());
        assert!(VirtualMicStatus::EndpointMissing.allows_subtitles());
    }

    #[test]
    fn endpoint_missing_is_distinguished_from_not_installed() {
        let status = VirtualMicStatus::EndpointMissing;
        assert!(!status.offers_install(), "the driver is already installed");
        assert!(!status.is_usable());
        assert!(status.detail().contains("Device Manager"));
    }

    #[test]
    fn unavailable_carries_its_reason_into_the_ui_copy() {
        let status = VirtualMicStatus::Unavailable {
            reason: "The endpoint is in use by another application.".into(),
        };
        assert_eq!(status.detail(), "The endpoint is in use by another application.");
        assert!(!status.offers_install());
    }

    #[test]
    fn status_serializes_for_the_ui() {
        let json = serde_json::to_string(&VirtualMicStatus::NotInstalled).unwrap();
        assert_eq!(json, r#"{"status":"not_installed"}"#);
    }

    #[test]
    fn the_placeholder_driver_reports_honestly() {
        // It must not report Ready: that would hide the §40 screen and let a
        // developer believe translation worked.
        let mut driver = UnavailableVirtualMic;
        assert_eq!(driver.status(), VirtualMicStatus::NotInstalled);
        assert!(driver.write(&[0i16; 8], 48_000, 1).is_err());
        assert_eq!(driver.queued_samples(), 0);
    }
}
