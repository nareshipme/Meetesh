//! Events pushed to the frontend. Names must match `src/api.ts`.

use meetesh_core::TranscriptSegment;
use serde::Serialize;

pub const RECORDING_LEVEL: &str = "recording-level";
pub const MODEL_DOWNLOAD: &str = "model-download";
pub const TRANSCRIBE_PROGRESS: &str = "transcribe-progress";
pub const LIVE_SEGMENTS: &str = "live-segments";
pub const LIVE_DONE: &str = "live-done";
pub const HINT_PIECE: &str = "hint-piece";
pub const HINT_DONE: &str = "hint-done";

#[derive(Serialize, Clone)]
pub struct DownloadEvent {
    pub id: String,
    pub downloaded: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct LiveSegmentsEvent {
    pub meeting_id: i64,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Serialize, Clone)]
pub struct LiveDoneEvent {
    pub meeting_id: i64,
    /// True when the live transcript may have gaps and a full pass was started to fill them.
    pub full_pass_started: bool,
}

#[derive(Serialize, Clone)]
pub struct TranscribeEvent {
    pub meeting_id: i64,
    pub progress: f32,
    pub done: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

/// Streamed text while a hint is being generated (raw, may include a label that the final text drops).
#[derive(Serialize, Clone)]
pub struct HintPieceEvent {
    pub meeting_id: i64,
    pub hint_id: u64,
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct HintDoneEvent {
    pub meeting_id: i64,
    pub hint_id: u64,
    /// Cleaned hint, or `None` when the model had nothing useful to say.
    pub text: Option<String>,
    pub cancelled: bool,
    pub error: Option<String>,
    pub first_token_ms: u64,
    pub total_ms: u64,
    pub tokens: usize,
}
