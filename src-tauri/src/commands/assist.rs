use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use meetesh_core::assist::prompt::{build_hint_messages, clean_hint, recent_window, Budget, HintContext};
use meetesh_core::assist::trigger::{AutoHintMode, Trigger};
use meetesh_core::assist::{Assistant, GenOptions};
use meetesh_core::{Error, Result};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::{HintDoneEvent, HintPieceEvent, HINT_DONE, HINT_PIECE};
use crate::state::{get_assistant, AppState, HintJob};

/// How much of the recent conversation a hint is based on.
const CONTEXT_WINDOW_MS: i64 = 3 * 60 * 1000;

static NEXT_HINT_ID: AtomicU64 = AtomicU64::new(1);

/// Ask the on-device model for a short hint based on the last few minutes of the meeting.
/// Returns immediately with a hint id; text streams via `hint-piece`, then `hint-done`.
#[tauri::command]
pub fn request_hint(app: AppHandle, meeting_id: i64) -> Result<u64> {
    start_hint(&app, meeting_id, None)
}

/// Start a hint. `trigger` is set when the app offers it by itself (see `auto_hints`).
pub fn start_hint(app: &AppHandle, meeting_id: i64, trigger: Option<Trigger>) -> Result<u64> {
    let trigger = trigger.map(Trigger::as_str);
    let app = app.clone();
    let st = app.state::<AppState>();
    let entry = st.assist_model()?.clone();
    if !st.models.is_installed(&entry) {
        return Err(Error::Other("Download the live assist model first".into()));
    }
    let meeting = st.db.lock().unwrap().get_meeting(meeting_id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut job = st.hint_job.lock().unwrap();
        if job.is_some() {
            return Err(Error::Other("A hint is already being generated".into()));
        }
        *job = Some(HintJob { meeting_id, cancel: cancel.clone() });
    }
    let hint_id = NEXT_HINT_ID.fetch_add(1, Ordering::Relaxed);

    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let outcome = (|| -> Result<meetesh_core::assist::Completion> {
            let assistant = get_assistant(&app)?; // first use loads the model (a few seconds)
            let segments = st.db.lock().unwrap().get_transcript(meeting_id)?;
            let recent = recent_window(&segments, CONTEXT_WINDOW_MS);
            if recent.is_empty() {
                return Err(Error::Other("Nothing has been said yet".into()));
            }
            let messages = build_hint_messages(&HintContext { agenda: meeting.agenda, recent }, Budget::default());
            let mut so_far = String::new();
            // Let live transcription go first: the hint pauses while transcription work is pending.
            let opts = GenOptions::hint(cancel.clone()).yielding_to(st.asr_pending.clone());
            assistant.complete(&messages, &opts, &mut |piece| {
                so_far.push_str(piece);
                let _ = app.emit(HINT_PIECE, HintPieceEvent { trigger, meeting_id, hint_id, text: so_far.clone() });
                true
            })
        })();
        *st.hint_job.lock().unwrap() = None;

        let done = match outcome {
            Ok(c) => {
                // Timings and counts only: never log the hint or transcript text.
                log::info!(
                    "hint for meeting {meeting_id}: {} prompt tokens, {} generated, first token {} ms, total {} ms ({} ms held back for transcription), {}{}",
                    c.stats.prompt_tokens,
                    c.stats.generated_tokens,
                    c.stats.first_token_ms,
                    c.stats.total_ms,
                    c.stats.yielded_ms,
                    if c.stats.on_gpu { "GPU" } else { "CPU" },
                    if c.stats.cpu_fallback { " (GPU failed, fell back to CPU)" } else { "" }
                );
                if let Some(reason) = &c.stats.fallback_reason {
                    log::warn!("GPU failure reason: {reason}");
                }
                HintDoneEvent {
                    trigger,
                    meeting_id,
                    hint_id,
                    text: if c.stats.cancelled { None } else { clean_hint(&c.text) },
                    cancelled: c.stats.cancelled,
                    error: None,
                    first_token_ms: c.stats.first_token_ms,
                    total_ms: c.stats.total_ms,
                    tokens: c.stats.generated_tokens,
                    on_gpu: c.stats.on_gpu,
                    cpu_fallback: c.stats.cpu_fallback,
                }
            }
            Err(e) => {
                log::warn!("hint failed: {e}");
                HintDoneEvent { trigger, meeting_id, hint_id, text: None, cancelled: false, error: Some(e.to_string()), first_token_ms: 0, total_ms: 0, tokens: 0, on_gpu: false, cpu_fallback: false }
            }
        };
        let _ = app.emit(HINT_DONE, done);
    });
    Ok(hint_id)
}

#[tauri::command]
pub fn cancel_hint(state: State<AppState>) {
    if let Some(job) = state.hint_job.lock().unwrap().as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

/// Is the live-assist model downloaded and ready?
#[tauri::command]
pub fn assist_ready(state: State<AppState>) -> bool {
    state.assist_model().map(|m| state.models.is_installed(m)).unwrap_or(false)
}

/// Id of the meeting a hint is being generated for, if any (lets the UI resync after a reload).
#[tauri::command]
pub fn active_hint(state: State<AppState>) -> Option<i64> {
    state.hint_job.lock().unwrap().as_ref().map(|j| j.meeting_id)
}

/// How the app offers hints by itself: "off", "questions" or "questions_and_pauses".
#[tauri::command]
pub fn get_auto_hints(state: State<AppState>) -> String {
    state.auto_hints.mode().as_str().to_string()
}

#[tauri::command]
pub fn set_auto_hints(state: State<AppState>, mode: String) -> Result<()> {
    let mode = AutoHintMode::parse(&mode);
    state.db.lock().unwrap().set_setting(AUTO_HINTS_KEY, mode.as_str())?;
    state.auto_hints.set_mode(mode);
    Ok(())
}

pub const AUTO_HINTS_KEY: &str = "auto_hints";
