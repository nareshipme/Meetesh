//! Capturing what the computer plays (the other participants on a call), as a second track.
//!
//! - Windows: WASAPI loopback of the default output device, through cpal.
//! - macOS 13+: ScreenCaptureKit (`macos`), audio only.
//! - Linux: not supported yet (PulseAudio/PipeWire monitor capture is planned).
//!
//! Everything here is platform-specific, so it lives in the shell. The core only sees a `Pusher`.

#[cfg(target_os = "macos")]
mod macos;

use meetesh_core::audio::Pusher;
use serde::Serialize;

use crate::capture::CaptureHandle;

#[derive(Serialize)]
pub struct SystemAudioSupport {
    pub available: bool,
    /// Why it is not available (empty when it is), written for the user.
    pub reason: String,
}

pub fn support() -> SystemAudioSupport {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        SystemAudioSupport { available: true, reason: String::new() }
    } else {
        SystemAudioSupport {
            available: false,
            reason: "Capturing the other participants is not available on Linux yet. Only your microphone is recorded.".into(),
        }
    }
}

/// Start capturing system audio. `make_pusher` gets the native sample rate and channel count.
/// Returns once capture is running, or the reason it could not start (permission, no device, ...).
#[cfg(target_os = "windows")]
pub fn start<F>(make_pusher: F) -> Result<CaptureHandle, String>
where
    F: FnOnce(u32, u16) -> Pusher + Send + 'static,
{
    use cpal::traits::{DeviceTrait, HostTrait};
    // Building an *input* stream on the default *output* device is how cpal asks WASAPI for loopback.
    crate::capture::start_cpal(
        "system audio",
        || {
            let device = cpal::default_host()
                .default_output_device()
                .ok_or_else(|| "No speakers or headphones found to capture".to_string())?;
            let config = device
                .default_output_config()
                .map_err(|e| format!("System audio unavailable: {e}"))?;
            Ok((device, config))
        },
        make_pusher,
    )
}

#[cfg(target_os = "macos")]
pub fn start<F>(make_pusher: F) -> Result<CaptureHandle, String>
where
    F: FnOnce(u32, u16) -> Pusher + Send + 'static,
{
    // The audio format is only known once the first buffer arrives, so the pusher is made then.
    let mut make = Some(make_pusher);
    let mut pusher: Option<Pusher> = None;
    let stop = macos::start(Box::new(move |samples, rate, channels| {
        if pusher.is_none() {
            if let Some(make) = make.take() {
                pusher = Some(make(rate, channels));
            }
        }
        if let Some(p) = pusher.as_mut() {
            p.push_interleaved(samples);
        }
    }))?;
    Ok(CaptureHandle::new(stop))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn start<F>(_make_pusher: F) -> Result<CaptureHandle, String>
where
    F: FnOnce(u32, u16) -> Pusher + Send + 'static,
{
    Err(support().reason)
}
