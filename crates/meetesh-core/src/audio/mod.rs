//! Audio pipeline: capture source -> resample -> fan-out to sinks (disk, level meter, VAD/ASR).
//!
//! The capture side (cpal, system audio, mobile APIs) lives in the platform shells and only
//! needs a `Pusher`. Everything here is pure Rust and testable without hardware.

pub mod pipeline;
pub mod resample;
pub mod sinks;
pub mod wav;

pub use pipeline::{FrameSink, Pusher, PusherFactory, RecordingSession, SessionReport};
pub use resample::{Resampler, TARGET_RATE};
