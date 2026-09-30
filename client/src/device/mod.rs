//! Device discovery and the virtual microphone (§8, §9).
//!
//! [`detection`] is portable and tested everywhere. [`virtual_mic`] describes
//! the driver contract; the Windows implementation lives behind `cfg`.

pub mod detection;
pub mod virtual_mic;

pub use detection::{
    AudioDevice, CapturableApplication, DataFlow, DeviceInventory, DeviceRole, SelectionError,
    VIRTUAL_DEVICE_NAME,
};
pub use virtual_mic::{VirtualMicDriver, VirtualMicStatus};
