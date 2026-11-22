//! Meeting notes (summary, decisions, action items) written by the on-device model.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use meetesh_core::assist::summary::{summarize, SummaryRun};
use meetesh_core::assist::Assistant;
use meetesh_core::{Error, Result};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::{SummaryEvent, SUMMARY_PROGRESS};
use crate::state::{get_assistant, AppState, SummaryJob};

/// Too little was said to be worth a summary (about a dozen words).
const MIN_WORDS_FOR_AUTO: usize = 25;

/// Write the notes for a meeting now (also used to regenerate). Returns at once; progress arrives
/// as `summary-progress`, and the text is saved with the meeting.
#[tauri::command]
pub fn summarize_meeting(app: AppHandle, id: i64) -> Result<()> {
    start_summary(&app, id)
}

#[tauri::command]
pub fn cancel_summary(state: State<AppState>) {
    cancel_running(&state);
}

/// Id of the meeting being summarized, if any (lets the UI resync after a reload).
#[tauri::command]
pub fn active_summary(state: State<AppState>) -> Option<i64> {
    state.summary_job.lock().unwrap().as_ref().map(|j| j.meeting_id)
}

/// Forget a meeting's notes.
#[tauri::command]
pub fn clear_summary(state: State<AppState>, id: i64) -> Result<meetesh_core::Meeting> {
    if state.summary_job.lock().unwrap().as_ref().is_some_and(|j| j.meeting_id == id) {
        return Err(Error::Other("The notes are being written; cancel that first".into()));
    }
    state.db.lock().unwrap().set_summary(id, "", "")
}

pub fn cancel_running(state: &AppState) {
    if let Some(job) = state.summary_job.lock().unwrap().as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

/// After a meeting's transcript is complete: write the notes by themselves if the live-assist model
/// is installed and the meeting has enough in it. Quietly does nothing otherwise.
pub fn maybe_auto_summarize(app: &AppHandle, meeting_id: i64) {
    let st = app.state::<AppState>();
    let installed = st.assist_model().map(|m| st.models.is_installed(m)).unwrap_or(false);
    if !installed {
        return;
    }
    let words = match st.db.lock().unwrap().get_transcript(meeting_id) {
        Ok(t) => t.iter().map(|s| s.text.split_whitespace().count()).sum::<usize>(),
        Err(_) => return,
    };
    let has_notes = st.db.lock().unwrap().get_meeting(meeting_id).map(|m| !m.summary.is_empty()).unwrap_or(true);
    if words < MIN_WORDS_FOR_AUTO || has_notes {
        return;
    }
    if let Err(e) = start_summary(app, meeting_id) {
        log::info!("not summarizing meeting {meeting_id} automatically: {e}");
    }
}

fn start_summary(app: &AppHandle, meeting_id: i64) -> Result<()> {
    let st = app.state::<AppState>();
    let entry = st.assist_model()?.clone();
    if !st.models.is_installed(&entry) {
        return Err(Error::Other("Download the live assist model first (it also writes the meeting notes)".into()));
    }
    let meeting = st.db.lock().unwrap().get_meeting(meeting_id)?;
    if meeting.status == "recording" {
        return Err(Error::Other("Stop the recording first".into()));
    }
    let segments = st.db.lock().unwrap().get_transcript(meeting_id)?;
    if segments.is_empty() {
        return Err(Error::Other("There is no transcript to summarize yet".into()));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut job = st.summary_job.lock().unwrap();
        if job.is_some() {
            return Err(Error::Other("Notes are already being written".into()));
        }
        *job = Some(SummaryJob { meeting_id, cancel: cancel.clone() });
    }

    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let emit = |done, total, finished, cancelled, error| {
            let _ = app.emit(SUMMARY_PROGRESS, SummaryEvent { meeting_id, done, total, finished, cancelled, error });
        };
        emit(0, 1, false, false, None);
        let started = std::time::Instant::now();
        let outcome = (|| -> Result<String> {
            let assistant = get_assistant(&app)?; // may load the model first
            let run = SummaryRun { cancel: cancel.clone(), yield_to: Some(st.asr_pending.clone()) };
            let summary = summarize(assistant.as_ref(), &segments, &meeting.agenda, &run, |d, t| emit(d, t, false, false, None))?;
            let md = summary.to_markdown();
            st.db.lock().unwrap().set_summary(meeting_id, &md, assistant.model_id())?;
            Ok(md)
        })();
        *st.summary_job.lock().unwrap() = None;
        match outcome {
            Ok(md) => {
                // Sizes and timing only: never log the notes themselves.
                log::info!("summary for meeting {meeting_id}: {} chars in {} ms", md.len(), started.elapsed().as_millis());
                emit(1, 1, true, false, None)
            }
            Err(Error::Cancelled) => emit(0, 1, true, true, None),
            Err(e) => {
                log::warn!("summary failed: {e}");
                emit(0, 1, true, false, Some(e.to_string()))
            }
        }
    });
    Ok(())
}
