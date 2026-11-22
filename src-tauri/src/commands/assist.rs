use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use meetesh_core::assist::memory::{keywords, plain_excerpt, Related};
use meetesh_core::assist::prompt::{build_hint_messages, clean_hint, recent_window, Budget, HintContext};
use meetesh_core::db::MatchKind;
use meetesh_core::assist::trigger::{AutoHintMode, Trigger};
use meetesh_core::assist::{Assistant, GenOptions};
use meetesh_core::{Error, Result};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::{HintDoneEvent, HintPieceEvent, HintSource, HINT_DONE, HINT_PIECE};
use crate::state::{get_assistant, AppState, HintJob};

/// How much of the recent conversation a hint is based on.
const CONTEXT_WINDOW_MS: i64 = 3 * 60 * 1000;

static NEXT_HINT_ID: AtomicU64 = AtomicU64::new(1);

/// Up to three earlier meetings that talk about what was just said, each with one short excerpt (its
/// notes if they match, else a transcript line). Best effort: a search problem means no memory, not
/// no hint.
fn earlier_meetings(st: &AppState, meeting_id: i64, recent: &[meetesh_core::TranscriptSegment]) -> Vec<(Related, i64)> {
    let words = keywords(recent, 4);
    let db = st.db.lock().unwrap();
    let Ok(found) = db.related(meeting_id, &words, 3) else { return vec![] };
    found
        .into_iter()
        .filter_map(|r| {
            let pick = r
                .matches
                .iter()
                .find(|m| m.kind == MatchKind::Notes)
                .or_else(|| r.matches.iter().find(|m| m.kind == MatchKind::Transcript))
                .or_else(|| r.matches.first())?;
            let related = Related {
                meeting_id: r.meeting_id,
                title: r.title.clone(),
                day: db.day_label(r.created_at).unwrap_or_default(),
                excerpt: plain_excerpt(&pick.snippet, 240),
            };
            Some((related, r.created_at))
        })
        .collect()
}

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
    // Someone is waiting for this: background work (meeting notes) steps aside until it is done.
    let interactive = st.gate.begin_interactive();

    std::thread::spawn(move || {
        let _interactive = interactive;
        let st = app.state::<AppState>();
        let mut sources: Vec<HintSource> = Vec::new();
        let outcome = (|| -> Result<meetesh_core::assist::Completion> {
            let assistant = get_assistant(&app)?; // first use loads the model (a few seconds)
            let segments = st.db.lock().unwrap().get_transcript(meeting_id)?;
            let recent = recent_window(&segments, CONTEXT_WINDOW_MS);
            if recent.is_empty() {
                return Err(Error::Other("Nothing has been said yet".into()));
            }
            // What earlier meetings said about the same things (keyword search in the local index).
            let related = earlier_meetings(&st, meeting_id, &recent);
            sources = related.iter().map(|(r, _)| HintSource { meeting_id: r.meeting_id, title: r.title.clone(), day: r.day.clone() }).collect();
            let related: Vec<Related> = related.into_iter().map(|(r, _)| r).collect();
            let messages = build_hint_messages(&HintContext { agenda: meeting.agenda, recent, related }, Budget::default());
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
                    sources,
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
                HintDoneEvent { sources: vec![], trigger, meeting_id, hint_id, text: None, cancelled: false, error: Some(e.to_string()), first_token_ms: 0, total_ms: 0, tokens: 0, on_gpu: false, cpu_fallback: false }
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

/// The live list of what is being discussed (and whether it is switched on).
#[tauri::command]
pub fn get_live_points(state: State<AppState>) -> crate::events::LivePointsEvent {
    state.live_points.view()
}

#[tauri::command]
pub fn set_live_points(app: AppHandle, state: State<AppState>, enabled: bool) -> Result<()> {
    state.db.lock().unwrap().set_setting(crate::live_points::SETTING_KEY, if enabled { "on" } else { "off" })?;
    state.live_points.set_enabled(&app, enabled);
    Ok(())
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
