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
pub const SUMMARY_PROGRESS: &str = "summary-progress";
pub const LIVE_POINTS: &str = "live-points";
pub const MODEL_LOAD: &str = "model-load";

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadPhase {
    Downloading,
    /// Checking the file (also when resuming: the hash continues over what is already on disk).
    Verifying,
    /// Stopped by the user; the partial file is kept and can be resumed.
    Paused,
    Failed,
    Done,
}

#[derive(Serialize, Clone)]
pub struct DownloadEvent {
    pub id: String,
    pub phase: DownloadPhase,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoadState {
    /// Installed but not loaded into memory.
    Idle,
    Loading,
    /// Loaded; running a tiny request so the first real one is fast.
    Warming,
    Ready,
    Failed,
}

/// Where a model is on its way into memory. Sent on every change and available via `model_runtime`.
#[derive(Serialize, Clone)]
pub struct ModelLoadEvent {
    pub id: String,
    pub role: meetesh_core::models::Role,
    pub state: LoadState,
    /// 0.0 to 1.0 when the loader reports real progress (llama.cpp); `None` means indeterminate.
    pub progress: Option<f32>,
    /// "GPU" or "CPU" once ready.
    pub backend: Option<String>,
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
    /// Why an automatic hint was offered ("question" or "pause"); `None` for a hint the user asked for.
    pub trigger: Option<&'static str>,
    pub meeting_id: i64,
    pub hint_id: u64,
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct HintDoneEvent {
    /// Earlier meetings whose notes were shown to the model for this hint.
    pub sources: Vec<HintSource>,
    /// Why an automatic hint was offered ("question" or "pause"); `None` for a hint the user asked for.
    pub trigger: Option<&'static str>,
    pub meeting_id: i64,
    pub hint_id: u64,
    /// Cleaned hint, or `None` when the model had nothing useful to say.
    pub text: Option<String>,
    pub cancelled: bool,
    pub error: Option<String>,
    pub first_token_ms: u64,
    pub total_ms: u64,
    pub tokens: usize,
    /// Which processor produced the hint, and whether the GPU failed and the CPU took over.
    pub on_gpu: bool,
    pub cpu_fallback: bool,
}

/// Where a meeting's notes are: waiting in line, being written, paused for something more
/// important, or finished. The finished text is saved with the meeting; the UI reloads the meeting
/// when `finished` is set.
#[derive(Serialize, Clone)]
pub struct SummaryEvent {
    pub meeting_id: i64,
    /// "queued", "running", "paused" or "finished".
    pub state: &'static str,
    /// How many notes jobs are ahead of this one (queued only).
    pub position: usize,
    /// Why it is paused: "hint" (a hint is being written) or "recording".
    pub paused_for: Option<&'static str>,
    pub done: usize,
    pub total: usize,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

/// An earlier meeting that a hint had in front of it.
#[derive(Serialize, Clone)]
pub struct HintSource {
    pub meeting_id: i64,
    pub title: String,
    /// "Mon 16 Nov"
    pub day: String,
}

/// The live list of what is being discussed. Also sent when the setting is switched.
#[derive(Serialize, Clone, Default)]
pub struct LivePointsEvent {
    pub meeting_id: i64,
    /// Whether live points are switched on.
    pub enabled: bool,
    /// What is being discussed right now.
    pub now: String,
    pub points: Vec<String>,
    /// An update is being written.
    pub updating: bool,
}
