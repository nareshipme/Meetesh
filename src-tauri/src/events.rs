//! Events pushed to the frontend. Names must match `src/api.ts`.

use meetesh_core::TranscriptSegment;
use serde::Serialize;

pub const RECORDING_LEVEL: &str = "recording-level";
pub const MODEL_DOWNLOAD: &str = "model-download";
pub const TRANSCRIBE_PROGRESS: &str = "transcribe-progress";
pub const LIVE_SEGMENTS: &str = "live-segments";
pub const LIVE_DONE: &str = "live-done";

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
