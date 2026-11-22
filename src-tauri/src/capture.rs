//! Microphone capture with cpal (also the base for Windows system-audio loopback). Platform-specific, so it lives in the shell, not the core.
//! The cpal stream is built and owned by one dedicated thread (streams are not Send everywhere).

use std::sync::mpsc::channel;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
use meetesh_core::audio::Pusher;
use serde::Serialize;

#[derive(Serialize)]
pub struct InputDevice {
    pub name: String,
    pub is_default: bool,
}

pub fn list_input_devices() -> Result<Vec<InputDevice>, String> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    let devices = host.input_devices().map_err(|e| e.to_string())?;
    Ok(devices
        .filter_map(|d| d.name().ok())
        .map(|name| InputDevice { is_default: default.as_deref() == Some(name.as_str()), name })
        .collect())
}

fn pick_device(name: Option<&str>) -> Result<Device, String> {
    let host = cpal::default_host();
    match name {
        Some(n) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().ok().as_deref() == Some(n))
            .ok_or_else(|| format!("Microphone \"{n}\" not found")),
        None => host.default_input_device().ok_or_else(|| "No microphone found".to_string()),
    }
}

/// A running capture. Dropping it without calling `stop` leaves the capture running.
pub struct CaptureHandle {
    stop: Box<dyn FnOnce() + Send>,
}

impl CaptureHandle {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        CaptureHandle { stop: Box::new(stop) }
    }

    /// Stops the stream and waits for the capture to wind down.
    pub fn stop(self) {
        (self.stop)()
    }
}

/// Start capturing from the microphone `device_name` (or the default input). `make_pusher` is
/// called with the device's native sample rate and channel count. Returns once the stream is
/// running, or the reason it could not start (no device, permission denied, ...).
pub fn start_capture<F>(device_name: Option<String>, make_pusher: F) -> Result<CaptureHandle, String>
where
    F: FnOnce(u32, u16) -> Pusher + Send + 'static,
{
    start_cpal(
        "microphone",
        move || {
            let device = pick_device(device_name.as_deref())?;
            let config = device.default_input_config().map_err(|e| format!("Microphone unavailable: {e}"))?;
            Ok((device, config))
        },
        make_pusher,
    )
}

/// Capture through cpal from whatever device `select` returns. The stream is built and owned by
/// one dedicated thread (streams are not Send everywhere). `what` names the source in errors.
pub fn start_cpal<S, F>(what: &'static str, select: S, make_pusher: F) -> Result<CaptureHandle, String>
where
    S: FnOnce() -> Result<(Device, SupportedStreamConfig), String> + Send + 'static,
    F: FnOnce(u32, u16) -> Pusher + Send + 'static,
{
    let (stop_tx, stop_rx) = channel::<()>();
    let (ready_tx, ready_rx) = channel::<Result<(), String>>();

    let join = std::thread::Builder::new()
        .name(format!("meetesh-capture-{what}"))
        .spawn(move || {
            let built = (|| -> Result<cpal::Stream, String> {
                let (device, config) = select()?;
                let (rate, channels, format) = (config.sample_rate().0, config.channels(), config.sample_format());
                let mut pusher = make_pusher(rate, channels);
                let err_fn = move |e| log::error!("{what} stream error: {e}");
                let cfg = config.into();
                let stream = match format {
                    SampleFormat::F32 => device.build_input_stream(
                        &cfg,
                        move |data: &[f32], _| pusher.push_interleaved(data),
                        err_fn,
                        None,
                    ),
                    SampleFormat::I16 => device.build_input_stream(
                        &cfg,
                        move |data: &[i16], _| {
                            let v: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                            pusher.push_interleaved(&v)
                        },
                        err_fn,
                        None,
                    ),
                    SampleFormat::U16 => device.build_input_stream(
                        &cfg,
                        move |data: &[u16], _| {
                            let v: Vec<f32> = data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0).collect();
                            pusher.push_interleaved(&v)
                        },
                        err_fn,
                        None,
                    ),
                    other => return Err(format!("Unsupported sample format {other:?}")),
                }
                .map_err(|e| format!("Could not open the {what}: {e}"))?;
                stream.play().map_err(|e| format!("Could not start the {what}: {e}"))?;
                Ok(stream)
            })();

            match built {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stop_rx.recv(); // keep the stream alive until stop
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| e.to_string())?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(CaptureHandle::new(move || {
            let _ = stop_tx.send(());
            let _ = join.join();
        })),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => Err("Audio thread failed to start".into()),
    }
}
