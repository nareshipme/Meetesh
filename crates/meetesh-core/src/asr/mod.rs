//! Speech recognition: a backend-agnostic `Transcriber` trait, a voice-activity chunker, and
//! helpers that turn recordings (finished or live) into timestamped transcript segments.
//!
//! Whisper (whisper.cpp) is one backend behind the `whisper` feature. Models are chosen by id
//! from the manifest, never named in logic (NFR-21).

pub mod chunker;
pub mod run;
#[cfg(feature = "whisper")]
pub mod whisper;

use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Turns 16 kHz mono audio into text. Timestamps are relative to the start of `samples`.
pub trait Transcriber: Send + Sync {
    /// Manifest id of the model, recorded with every transcript.
    fn model_id(&self) -> &str;
    fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptSegment>>;
}

/// Whisper emits markers like `[BLANK_AUDIO]` or `(music)` for non-speech; drop them.
pub fn is_noise_marker(text: &str) -> bool {
    let t = text.trim();
    t.is_empty()
        || (t.starts_with('[') && t.ends_with(']'))
        || (t.starts_with('(') && t.ends_with(')'))
        || (t.starts_with('*') && t.ends_with('*'))
}
