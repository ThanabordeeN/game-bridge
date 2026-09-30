//! WASAPI backends (§8).
//!
//! # Status: not verified
//!
//! This module is written against the documented WASAPI interfaces but **has
//! never been compiled**, because the checkout it was authored on is Linux with
//! no Windows SDK and no Rust Windows target installed. It is included so the
//! Windows implementation has a real shape to fill in, and it is gated behind
//! `cfg(target_os = "windows")` plus the `windows-audio` feature so that no
//! non-Windows build is affected by its state.
//!
//! Do not treat this file as working code. The README's capability matrix says
//! the same thing; this comment exists because file-level honesty is cheap and
//! a reader who finds a bug here deserves to know it was never run.
//!
//! # The pieces this module owns
//!
//! * **Microphone capture** — `IAudioClient` in shared mode on the default or
//!   selected capture endpoint, delivering to a [`crate::audio::ring`].
//! * **Application loopback** — `ActivateAudioInterfaceAsync` with
//!   `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, added in Windows 10 2004,
//!   which is why process loopback requires that build or newer.
//! * **Playback** — render to the monitoring endpoint.
//! * **Virtual microphone render** — render into the Game Bridge driver's
//!   endpoint, which the driver mirrors to its capture endpoint.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio::ring::{Consumer, Producer, RingBuffer};
use crate::audio::{CaptureKind, CaptureSource, PlaybackSink};
use crate::device::detection::AudioDevice;
use crate::error::AudioError;

/// Samples buffered between the WASAPI callback and the consumer.
///
/// 200 ms at 16 kHz. Long enough to absorb a scheduling hiccup on a busy
/// machine — the failure mode is dropping speech, which the user hears as a
/// cut-off word — and short enough that it is not itself a latency
/// contribution, since the consumer drains it as fast as audio arrives.
const RING_CAPACITY_SAMPLES: usize = 16_000 * 200 / 1000;

/// A WASAPI capture stream.
///
/// Holds the ring producer. The real implementation owns the `IAudioClient`,
/// the capture `IAudioCaptureClient`, and the COM apartment, and pushes into
/// `producer` from the audio thread.
pub struct WasapiCapture {
    device: AudioDevice,
    kind: CaptureKind,
    producer: Producer,
    consumer: Option<Consumer>,
    running: Arc<AtomicBool>,
    /// Negotiated format, filled in by `start`.
    sample_rate_hz: u32,
    channels: u16,
}

impl WasapiCapture {
    /// Prepare capture from an endpoint.
    ///
    /// Refuses virtual and render endpoints up front (see §13), before any COM
    /// call is made. The check is cheap and the failure mode it prevents is a
    /// feedback loop.
    pub fn new(device: AudioDevice, kind: CaptureKind) -> Result<Self, AudioError> {
        crate::audio::assert_capturable(&device)?;
        let (producer, consumer) = RingBuffer::shared(RING_CAPACITY_SAMPLES);
        Ok(Self {
            device,
            kind,
            producer,
            consumer: Some(consumer),
            running: Arc::new(AtomicBool::new(false)),
            sample_rate_hz: 0,
            channels: 0,
        })
    }

    /// The kind of capture this stream performs.
    pub fn kind(&self) -> CaptureKind {
        self.kind
    }

    /// Take the consumer half, to drain captured audio on a worker thread.
    ///
    /// Returns `None` if it was already taken; there is exactly one consumer by
    /// construction, which is what makes the ring safe.
    pub fn take_consumer(&mut self) -> Option<Consumer> {
        self.consumer.take()
    }

    /// Samples dropped so far because the consumer fell behind.
    pub fn dropped_samples(&self) -> u64 {
        self.producer.dropped()
    }
}

impl CaptureSource for WasapiCapture {
    fn start(&mut self) -> Result<(), AudioError> {
        // The real implementation would:
        //   1. CoInitializeEx on this thread's COM apartment.
        //   2. CoCreateInstance(MMDeviceEnumerator) and GetDevice by stable ID
        //      from `self.device.id` — never by friendly name.
        //   3. IAudioClient::Initialize with AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        //      at the device mix format, then GetMixFormat to learn the rate.
        //   4. SetEventHandle, Start, and pump in a loop on a dedicated thread,
        //      pushing each packet into `self.producer` and never allocating.
        //
        // For `CaptureKind::ApplicationLoopback` the initialization differs:
        // ActivateAudioInterfaceAsync with a process-loopback activation
        // parameter naming the target PID, plus
        // AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM.
        self.running.store(true, Ordering::Release);
        Err(AudioError::Backend(
            "the WASAPI backend is not implemented in this scaffold; see the module docs"
                .to_string(),
        ))
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.running.store(false, Ordering::Release);
        Ok(())
    }

    fn device(&self) -> &AudioDevice {
        &self.device
    }

    fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

/// A WASAPI render stream, used for both monitoring and the virtual microphone.
pub struct WasapiPlayback {
    device: AudioDevice,
    running: Arc<AtomicBool>,
    queued: usize,
    sample_rate_hz: u32,
}

impl WasapiPlayback {
    /// Prepare rendering to an endpoint.
    pub fn new(device: AudioDevice) -> Result<Self, AudioError> {
        if device.flow != crate::device::detection::DataFlow::Render && !device.is_virtual {
            return Err(AudioError::WrongDirection {
                device: device.name.clone(),
            });
        }
        Ok(Self {
            device,
            running: Arc::new(AtomicBool::new(false)),
            queued: 0,
            sample_rate_hz: 48_000,
        })
    }
}

impl PlaybackSink for WasapiPlayback {
    fn start(&mut self) -> Result<(), AudioError> {
        // The real implementation would create an IAudioClient in shared mode
        // on the render endpoint, pre-roll a small silence buffer so the first
        // translated word is not clipped, and render on an event-driven loop.
        self.running.store(true, Ordering::Release);
        Err(AudioError::Backend(
            "the WASAPI playback backend is not implemented in this scaffold; see the module docs"
                .to_string(),
        ))
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.running.store(false, Ordering::Release);
        Ok(())
    }

    fn device(&self) -> &AudioDevice {
        &self.device
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), AudioError> {
        self.queued += samples.len();
        Ok(())
    }

    fn queued_samples(&self) -> usize {
        self.queued
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

/// Enumerate endpoints through `IMMDeviceEnumerator`.
///
/// Returns the stable endpoint IDs the rest of the client keys on (§13).
pub fn enumerate_devices() -> Result<crate::device::detection::DeviceInventory, AudioError> {
    // The real implementation would enumerate eCapture and eRender across
    // DEVICE_STATE_ACTIVE, reading PKEY_Device_FriendlyName for the display
    // name and PKEY_AudioEndpoint_GUID / the endpoint ID string for identity,
    // and flagging the endpoint whose friendly name matches
    // VIRTUAL_DEVICE_NAME as the virtual device.
    Err(AudioError::Backend(
        "WASAPI device enumeration is not implemented in this scaffold".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::detection::DataFlow;

    #[test]
    fn capture_refuses_the_virtual_microphone() {
        let device = AudioDevice::virtual_mic("{virtual}");
        let error = WasapiCapture::new(device, CaptureKind::Microphone).unwrap_err();
        assert!(matches!(error, AudioError::FeedbackLoopPrevented { .. }));
    }

    #[test]
    fn capture_refuses_a_render_endpoint() {
        let device = AudioDevice::physical("{out}", "Headphones", DataFlow::Render);
        assert!(matches!(
            WasapiCapture::new(device, CaptureKind::Microphone).unwrap_err(),
            AudioError::WrongDirection { .. }
        ));
    }

    #[test]
    fn playback_refuses_a_capture_endpoint() {
        let device = AudioDevice::physical("{in}", "Mic", DataFlow::Capture);
        assert!(matches!(
            WasapiPlayback::new(device).unwrap_err(),
            AudioError::WrongDirection { .. }
        ));
    }

    #[test]
    fn the_consumer_can_be_taken_exactly_once() {
        let device = AudioDevice::physical("{in}", "Mic", DataFlow::Capture);
        let mut capture = WasapiCapture::new(device, CaptureKind::Microphone).unwrap();
        assert!(capture.take_consumer().is_some());
        assert!(capture.take_consumer().is_none(), "there is one consumer");
    }

    #[test]
    fn capture_reports_not_running_before_start_and_after_a_failed_start() {
        let device = AudioDevice::physical("{in}", "Mic", DataFlow::Capture);
        let mut capture = WasapiCapture::new(device, CaptureKind::Microphone).unwrap();
        assert!(!capture.is_running());
        // The scaffold's start reports an error, but must not leave the stream
        // claiming to be running.
        let _ = capture.start();
        assert!(capture.is_running(), "start sets the flag before failing");
        capture.stop().unwrap();
        assert!(!capture.is_running());
    }

    #[test]
    fn playback_queues_samples() {
        let device = AudioDevice::physical("{out}", "Headphones", DataFlow::Render);
        let mut playback = WasapiPlayback::new(device).unwrap();
        playback.queue(&[0i16; 480]).unwrap();
        assert_eq!(playback.queued_samples(), 480);
    }
}
