//! Audio endpoint identity and discovery.
//!
//! # Stable device IDs
//!
//! §13 requires every endpoint to be identified by a stable ID, because a
//! device *name* is not an identity. Windows lets a user rename an endpoint to
//! anything, and two identical headsets of the same model produce two entries
//! named "Speakers (USB Audio Device)". If the client remembered "the one
//! called Speakers", a user with two would silently start translating their own
//! translated output back into the pipeline.
//!
//! The stable ID is the WASAPI endpoint ID string, which is derived from the
//! device's container ID and is stable across replug, rename, and reboots.
//!
//! # Feedback protection
//!
//! [`AudioDevice::is_translation_input_allowed`] refuses any endpoint belonging
//! to the virtual audio device. This is enforced here, at the type level, and
//! again in the routing layer, so a UI bug cannot open the loop described in
//! §13.

use serde::{Deserialize, Serialize};

/// Which role an endpoint plays in the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    /// Physical microphone capture.
    Microphone,
    /// A game or voice application's audio, via loopback.
    ApplicationAudio,
    /// Where monitoring audio is played for the user.
    Headphones,
    /// The virtual microphone the client writes translated speech into.
    VirtualMicrophone,
}

impl DeviceRole {
    /// Label used in the Audio screen (§26).
    pub const fn label(self) -> &'static str {
        match self {
            DeviceRole::Microphone => "Microphone",
            DeviceRole::ApplicationAudio => "Game Audio",
            DeviceRole::Headphones => "Headphones",
            DeviceRole::VirtualMicrophone => "Virtual Output",
        }
    }

    /// The section heading this role appears under.
    pub const fn section(self) -> &'static str {
        match self {
            DeviceRole::Microphone | DeviceRole::VirtualMicrophone => "MICROPHONE",
            DeviceRole::ApplicationAudio => "GAME AUDIO",
            DeviceRole::Headphones => "MONITORING",
        }
    }
}

/// The audio data flow direction of an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataFlow {
    /// An input the client captures from.
    Capture,
    /// An output the client renders to.
    Render,
}

/// The name reported for the virtual audio device.
///
/// Exported so the Windows backend, the driver INF, and the UI all agree on one
/// string. A mismatch here is the classic cause of "driver installed but the
/// app cannot find it".
pub const VIRTUAL_DEVICE_NAME: &str = "Game Bridge Microphone";

/// A discovered audio endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    /// Stable WASAPI endpoint ID. Never a display name.
    pub id: String,
    /// Human-readable name, shown in the UI and freely changeable by the user.
    pub name: String,
    /// Whether this endpoint is an input or an output.
    pub flow: DataFlow,
    /// Whether this is the Game Bridge virtual device.
    pub is_virtual: bool,
    /// Endpoint mix format sample rate in Hz, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    /// Endpoint channel count, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    /// Whether this is the system default for its flow.
    #[serde(default)]
    pub is_default: bool,
}

impl AudioDevice {
    /// Create a physical (non-virtual) endpoint.
    pub fn physical(id: impl Into<String>, name: impl Into<String>, flow: DataFlow) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            flow,
            is_virtual: false,
            sample_rate_hz: None,
            channels: None,
            is_default: false,
        }
    }

    /// Create the virtual microphone endpoint.
    pub fn virtual_mic(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: VIRTUAL_DEVICE_NAME.to_string(),
            flow: DataFlow::Capture,
            is_virtual: true,
            sample_rate_hz: Some(48_000),
            channels: Some(1),
            is_default: false,
        }
    }

    /// Whether this endpoint may be used as a translation input.
    ///
    /// The single type-level guard for §13. A virtual endpoint is never a valid
    /// translation input, whichever direction it claims to flow in — capturing
    /// it would transcribe the client's own translated output and echo it back.
    pub fn is_translation_input_allowed(&self) -> bool {
        !self.is_virtual && self.flow == DataFlow::Capture
    }

    /// Whether this endpoint can be written to as the virtual microphone.
    pub fn is_virtual_output(&self) -> bool {
        self.is_virtual
    }
}

/// Everything the client needs to know about the machine's audio.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceInventory {
    /// All discovered endpoints.
    pub devices: Vec<AudioDevice>,
    /// Whether the virtual audio driver was found at all. Drives the §40
    /// "Virtual Microphone Not Found" state.
    pub virtual_driver_present: bool,
}

impl DeviceInventory {
    /// Build an inventory from a list of endpoints.
    pub fn new(devices: Vec<AudioDevice>) -> Self {
        let virtual_driver_present = devices.iter().any(|d| d.is_virtual);
        Self {
            devices,
            virtual_driver_present,
        }
    }

    /// Endpoints usable for a role.
    ///
    /// Filters out the virtual device for every role except
    /// [`DeviceRole::VirtualMicrophone`], which is the other half of the §13
    /// guard: even if the UI offered it, it would not be listed.
    pub fn for_role(&self, role: DeviceRole) -> Vec<&AudioDevice> {
        self.devices
            .iter()
            .filter(|device| match role {
                DeviceRole::Microphone | DeviceRole::ApplicationAudio => {
                    device.is_translation_input_allowed()
                }
                DeviceRole::Headphones => device.flow == DataFlow::Render && !device.is_virtual,
                DeviceRole::VirtualMicrophone => device.is_virtual_output(),
            })
            .collect()
    }

    /// Find a device by its stable ID.
    pub fn by_id(&self, id: &str) -> Option<&AudioDevice> {
        self.devices.iter().find(|device| device.id == id)
    }

    /// The system default capture endpoint, excluding the virtual device.
    pub fn default_microphone(&self) -> Option<&AudioDevice> {
        self.for_role(DeviceRole::Microphone)
            .into_iter()
            .find(|d| d.is_default)
            .or_else(|| self.for_role(DeviceRole::Microphone).into_iter().next())
    }

    /// The system default render endpoint, excluding the virtual device.
    pub fn default_headphones(&self) -> Option<&AudioDevice> {
        self.for_role(DeviceRole::Headphones)
            .into_iter()
            .find(|d| d.is_default)
            .or_else(|| self.for_role(DeviceRole::Headphones).into_iter().next())
    }

    /// The virtual microphone endpoint, if the driver is installed.
    pub fn virtual_microphone(&self) -> Option<&AudioDevice> {
        self.devices.iter().find(|device| device.is_virtual)
    }

    /// Whether the selected microphone ID is safe to use as a translation
    /// input. Returns the reason when it is not.
    pub fn validate_microphone_selection(&self, id: &str) -> Result<&AudioDevice, SelectionError> {
        let device = self.by_id(id).ok_or(SelectionError::NotFound)?;
        if device.is_virtual {
            return Err(SelectionError::VirtualDeviceAsInput);
        }
        if device.flow != DataFlow::Capture {
            return Err(SelectionError::WrongDirection);
        }
        Ok(device)
    }
}

/// Why a device selection was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    /// No endpoint has that ID. Usually means it was unplugged.
    NotFound,
    /// The chosen endpoint is the virtual microphone, which would create a
    /// feedback loop (§13).
    VirtualDeviceAsInput,
    /// A capture endpoint was chosen where a render endpoint is required, or
    /// the reverse.
    WrongDirection,
}

impl SelectionError {
    /// Message shown to the user (§40).
    pub const fn message(self) -> &'static str {
        match self {
            SelectionError::NotFound => {
                "That audio device is no longer connected. Pick another in Audio settings."
            }
            SelectionError::VirtualDeviceAsInput => {
                "Game Bridge Microphone cannot be used as an input. It carries translated \
                 speech into your game, and listening to it would create an echo loop."
            }
            SelectionError::WrongDirection => {
                "That device does not support this direction of audio."
            }
        }
    }
}

/// A running application that can be captured for game audio (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturableApplication {
    /// Process ID, valid only until the process exits.
    pub pid: u32,
    /// Executable name, e.g. `Valorant.exe`.
    pub executable: String,
    /// Window title, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    /// Whether the process is currently playing audio. Only these can be
    /// loopback-captured, so offering a silent process would be a dead end.
    pub has_audio: bool,
}

impl CapturableApplication {
    /// Display label for the application dropdown (§26).
    pub fn display(&self) -> &str {
        &self.executable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyperx() -> AudioDevice {
        AudioDevice {
            id: "{0.0.1.00000000}.{hyperx-quadcast}".into(),
            name: "HyperX QuadCast".into(),
            flow: DataFlow::Capture,
            is_virtual: false,
            sample_rate_hz: Some(48_000),
            channels: Some(1),
            is_default: true,
        }
    }

    fn arctis() -> AudioDevice {
        AudioDevice {
            id: "{0.0.0.00000000}.{steelseries-arctis}".into(),
            name: "SteelSeries Arctis".into(),
            flow: DataFlow::Render,
            is_virtual: false,
            sample_rate_hz: Some(48_000),
            channels: Some(2),
            is_default: true,
        }
    }

    fn virtual_mic() -> AudioDevice {
        AudioDevice::virtual_mic("{0.0.1.00000000}.{game-bridge-virtual}")
    }

    fn inventory() -> DeviceInventory {
        DeviceInventory::new(vec![hyperx(), arctis(), virtual_mic()])
    }

    #[test]
    fn virtual_device_is_never_offered_as_a_microphone() {
        // §13, enforced at the point where the UI would list devices.
        let inv = inventory();
        let microphones = inv.for_role(DeviceRole::Microphone);
        assert_eq!(microphones.len(), 1);
        assert_eq!(microphones[0].name, "HyperX QuadCast");
        assert!(microphones.iter().all(|d| !d.is_virtual));
    }

    #[test]
    fn virtual_device_is_offered_only_as_the_virtual_output() {
        let inv = inventory();
        let outputs = inv.for_role(DeviceRole::VirtualMicrophone);
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].is_virtual);
    }

    #[test]
    fn headphones_are_render_endpoints_and_not_virtual() {
        let inv = inventory();
        let headphones = inv.for_role(DeviceRole::Headphones);
        assert_eq!(headphones.len(), 1);
        assert_eq!(headphones[0].name, "SteelSeries Arctis");
    }

    #[test]
    fn selecting_the_virtual_mic_as_input_is_rejected_with_an_explanation() {
        let inv = inventory();
        let error = inv
            .validate_microphone_selection(&virtual_mic().id)
            .unwrap_err();
        assert_eq!(error, SelectionError::VirtualDeviceAsInput);
        assert!(error.message().contains("echo loop"));
    }

    #[test]
    fn selecting_a_render_device_as_input_is_rejected() {
        let inv = inventory();
        assert_eq!(
            inv.validate_microphone_selection(&arctis().id).unwrap_err(),
            SelectionError::WrongDirection
        );
    }

    #[test]
    fn a_missing_device_reports_not_found() {
        let inv = inventory();
        assert_eq!(
            inv.validate_microphone_selection("{unplugged}").unwrap_err(),
            SelectionError::NotFound
        );
    }

    #[test]
    fn a_valid_microphone_selection_succeeds() {
        let inv = inventory();
        let device = inv.validate_microphone_selection(&hyperx().id).unwrap();
        assert_eq!(device.name, "HyperX QuadCast");
    }

    #[test]
    fn lookup_is_by_stable_id_not_by_name() {
        // Two identical headsets of the same model share a display name but not
        // an endpoint ID. ID lookup must distinguish them.
        let inv = DeviceInventory::new(vec![
            AudioDevice::physical(
                "{id-a}",
                "Speakers (USB Audio Device)",
                DataFlow::Capture,
            ),
            AudioDevice::physical(
                "{id-b}",
                "Speakers (USB Audio Device)",
                DataFlow::Capture,
            ),
        ]);
        assert_eq!(inv.by_id("{id-a}").unwrap().id, "{id-a}");
        assert_eq!(inv.by_id("{id-b}").unwrap().id, "{id-b}");
        assert_ne!(
            inv.by_id("{id-a}").unwrap().id,
            inv.by_id("{id-b}").unwrap().id
        );
    }

    #[test]
    fn virtual_driver_presence_is_detected_from_the_inventory() {
        assert!(inventory().virtual_driver_present);
        assert!(!DeviceInventory::new(vec![hyperx(), arctis()]).virtual_driver_present);
    }

    #[test]
    fn default_microphone_falls_back_when_nothing_is_flagged_default() {
        let mut device = hyperx();
        device.is_default = false;
        let inv = DeviceInventory::new(vec![device, virtual_mic()]);
        assert_eq!(
            inv.default_microphone().map(|d| d.name.as_str()),
            Some("HyperX QuadCast")
        );
    }

    #[test]
    fn default_microphone_is_none_when_only_the_virtual_device_exists() {
        let inv = DeviceInventory::new(vec![virtual_mic()]);
        assert!(inv.default_microphone().is_none());
    }

    #[test]
    fn virtual_device_name_matches_the_ui_label() {
        // §9 specifies the exact device name users must see and select.
        assert_eq!(VIRTUAL_DEVICE_NAME, "Game Bridge Microphone");
        assert_eq!(virtual_mic().name, VIRTUAL_DEVICE_NAME);
    }

    #[test]
    fn app_capture_lists_expose_the_executable() {
        let app = CapturableApplication {
            pid: 1234,
            executable: "Valorant.exe".into(),
            window_title: Some("VALORANT".into()),
            has_audio: true,
        };
        assert_eq!(app.display(), "Valorant.exe");
    }
}
