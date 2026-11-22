//! Speech recognition: a backend-agnostic `Transcriber` trait, a voice-activity chunker, and
//! helpers that turn recordings (finished or live) into timestamped transcript segments.
//!
//! Whisper (whisper.cpp, `whisper` feature) and Parakeet (ONNX Runtime, `parakeet` feature) are backends. Models are chosen by id
//! from the manifest, never named in logic (NFR-21).

pub mod chunker;
pub mod live;
pub mod run;
#[cfg(feature = "parakeet-engine")]
pub mod parakeet;
#[cfg(feature = "whisper")]
pub mod whisper;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Which audio track a piece of speech came from.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The user's microphone ("You").
    #[default]
    Mic,
    /// Audio played by the computer: the other participants on a call ("Others").
    System,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Mic => "mic",
            Source::System => "system",
        }
    }

    pub fn parse(s: &str) -> Source {
        if s == "system" {
            Source::System
        } else {
            Source::Mic
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Older data and transcribers that do not know about tracks mean the microphone.
    #[serde(default)]
    pub source: Source,
}

impl TranscriptSegment {
    pub fn new(start_ms: i64, end_ms: i64, text: impl Into<String>) -> Self {
        TranscriptSegment { start_ms, end_ms, text: text.into(), source: Source::Mic }
    }

    pub fn from_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }
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
