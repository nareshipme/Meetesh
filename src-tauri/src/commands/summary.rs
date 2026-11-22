//! Meeting notes (summary, decisions, action items) written by the on-device model, in the
//! background: asked for from anywhere, written one meeting at a time in the order asked, and never
//! cancelled just because something more urgent came along (see `ModelGate`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use meetesh_core::assist::gate::PauseReason;
use meetesh_core::assist::summary::{summarize, SummaryRun};
use meetesh_core::assist::title::generate_title;
use meetesh_core::assist::Assistant;
use meetesh_core::db::title_source;
use meetesh_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::{SummaryEvent, SUMMARY_PROGRESS};
use crate::state::{get_notes_assistant, release_unused_assistants, AppState, SummaryJob};

/// Too little was said to be worth a summary (about a dozen words).
const MIN_WORDS_FOR_AUTO: usize = 25;

/// Ask for a meeting's notes (also used to regenerate). Returns at once; the notes join the line,
/// progress arrives as `summary-progress`, and the text is saved with the meeting.
#[tauri::command]
pub fn summarize_meeting(app: AppHandle, id: i64) -> Result<()> {
    enqueue(&app, id)
}

/// Stop writing this meeting's notes, or take them out of the line.
#[tauri::command]
pub fn cancel_summary(app: AppHandle, id: i64) {
    let st = app.state::<AppState>();
    let mut q = st.notes.lock().unwrap();
    if let Some(job) = q.running.as_ref().filter(|j| j.meeting_id == id) {
        job.cancel.store(true, Ordering::Relaxed);
    } else if let Some(i) = q.waiting.iter().position(|m| *m == id) {
        q.waiting.remove(i);
        drop(q);
        emit(&app, id, "finished", 0, None, 0, 1, true, None);
    }
}

#[derive(Serialize)]
pub struct SummaryStatus {
    /// "idle", "queued" or "running"; "paused" while it waits for a hint or a recording.
    state: &'static str,
    position: usize,
    paused_for: Option<&'static str>,
}

/// Where a meeting's notes are right now (lets the UI sync after a reload).
#[tauri::command]
pub fn summary_status(state: State<AppState>, id: i64) -> SummaryStatus {
    let q = state.notes.lock().unwrap();
    if q.running.as_ref().is_some_and(|j| j.meeting_id == id) {
        let paused_for = state.gate.pause_reason(true).map(reason_name);
        return SummaryStatus { state: if paused_for.is_some() { "paused" } else { "running" }, position: 0, paused_for };
    }
    match q.waiting.iter().position(|m| *m == id) {
        Some(p) => SummaryStatus { state: "queued", position: p + usize::from(q.running.is_some()), paused_for: None },
        None => SummaryStatus { state: "idle", position: 0, paused_for: None },
    }
}

/// Forget a meeting's notes.
#[tauri::command]
pub fn clear_summary(state: State<AppState>, id: i64) -> Result<meetesh_core::Meeting> {
    let q = state.notes.lock().unwrap();
    if q.running.as_ref().is_some_and(|j| j.meeting_id == id) || q.waiting.contains(&id) {
        return Err(Error::Other("The notes are being written; cancel that first".into()));
    }
    drop(q);
    state.db.lock().unwrap().set_summary(id, "", "")
}

fn reason_name(r: PauseReason) -> &'static str {
    match r {
        PauseReason::Hint => "hint",
        PauseReason::Recording => "recording",
    }
}

#[allow(clippy::too_many_arguments)]
fn emit(app: &AppHandle, meeting_id: i64, state: &'static str, position: usize, paused_for: Option<&'static str>, done: usize, total: usize, cancelled: bool, error: Option<String>) {
    let _ = app.emit(
        SUMMARY_PROGRESS,
        SummaryEvent { meeting_id, state, position, paused_for, done, total, finished: state == "finished", cancelled, error },
    );
}

/// After a meeting's transcript is complete: line up the notes by themselves if a notes model is
/// installed and the meeting has enough in it. Quietly does nothing otherwise.
pub fn maybe_auto_summarize(app: &AppHandle, meeting_id: i64) {
    let st = app.state::<AppState>();
    let online = crate::commands::online::load_settings(&st);
    if online.enabled {
        // Meeting text only leaves the device when asked for: unless "write notes automatically" was
        // chosen, online notes are written when the user clicks the button.
        if !online.auto {
            return;
        }
    } else if !st.summary_model().map(|m| st.models.is_installed(m)).unwrap_or(false) {
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
    if let Err(e) = enqueue(app, meeting_id) {
        log::info!("not summarizing meeting {meeting_id} automatically: {e}");
    }
}

/// Validate, join the line, and make sure a worker is running.
fn enqueue(app: &AppHandle, meeting_id: i64) -> Result<()> {
    let st = app.state::<AppState>();
    if !crate::commands::online::load_settings(&st).enabled {
        let entry = st.summary_model()?.clone();
        if !st.models.is_installed(&entry) {
            return Err(Error::Other(format!("Download {} first (it writes the meeting notes; you can pick another model in Settings)", entry.name)));
        }
    }
    let meeting = st.db.lock().unwrap().get_meeting(meeting_id)?;
    if meeting.status == "recording" {
        return Err(Error::Other("Stop the recording first".into()));
    }
    if st.db.lock().unwrap().get_transcript(meeting_id)?.is_empty() {
        return Err(Error::Other("There is no transcript to summarize yet".into()));
    }

    let (position, spawn) = {
        let mut q = st.notes.lock().unwrap();
        if q.running.as_ref().is_some_and(|j| j.meeting_id == meeting_id) || q.waiting.contains(&meeting_id) {
            return Err(Error::Other("Notes for this meeting are already being written or waiting their turn".into()));
        }
        let position = q.waiting.len() + usize::from(q.running.is_some());
        q.waiting.push_back(meeting_id);
        let spawn = !q.worker_alive;
        q.worker_alive = true;
        (position, spawn)
    };
    emit(app, meeting_id, "queued", position, None, 0, 1, false, None);
    if spawn {
        let app = app.clone();
        std::thread::Builder::new().name("meetesh-notes".into()).spawn(move || worker(app)).expect("spawn notes worker");
    }
    Ok(())
}

/// Writes the notes of each waiting meeting in turn, then ends (a new one starts when needed).
fn worker(app: AppHandle) {
    loop {
        let (meeting_id, cancel) = {
            let st = app.state::<AppState>();
            let mut q = st.notes.lock().unwrap();
            match q.waiting.pop_front() {
                Some(id) => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    q.running = Some(SummaryJob { meeting_id: id, cancel: cancel.clone() });
                    (id, cancel)
                }
                None => {
                    q.worker_alive = false;
                    return;
                }
            }
        };
        run_one(&app, meeting_id, &cancel);
        app.state::<AppState>().notes.lock().unwrap().running = None;
        release_unused_assistants(&app); // a notes model that differs from the hints model is not kept in memory
    }
}

fn run_one(app: &AppHandle, meeting_id: i64, cancel: &Arc<AtomicBool>) {
    let st = app.state::<AppState>();
    let progress = Arc::new(Mutex::new((0usize, 1usize)));
    let send = {
        let (app, progress) = (app.clone(), progress.clone());
        move |state: &'static str, paused_for: Option<&'static str>| {
            let (d, t) = *progress.lock().unwrap();
            emit(&app, meeting_id, state, 0, paused_for, d, t, false, None);
        }
    };
    send("running", None);
    let started = std::time::Instant::now();
    // Tell the UI when the job starts waiting for a hint or a recording, and when it carries on.
    let on_pause: Arc<dyn Fn(Option<PauseReason>) + Send + Sync> = {
        let (app, progress) = (app.clone(), progress.clone());
        Arc::new(move |r| {
            let (d, t) = *progress.lock().unwrap();
            match r {
                Some(r) => emit(&app, meeting_id, "paused", 0, Some(reason_name(r)), d, t, false, None),
                None => emit(&app, meeting_id, "running", 0, None, d, t, false, None),
            }
        })
    };

    let outcome = (|| -> Result<String> {
        let meeting = st.db.lock().unwrap().get_meeting(meeting_id)?; // gone if it was deleted while waiting
        let segments = st.db.lock().unwrap().get_transcript(meeting_id)?;
        // The online model (if the user chose it) runs on someone else's computer, so it neither waits for
        // hints or a recording nor slows them down. The on-device one does not even get loaded while a
        // hint is being written or a recording runs.
        let remote = crate::commands::online::active(&st)?;
        let local: Arc<meetesh_core::assist::llama::LlamaAssistant>;
        let (assistant, run): (&dyn Assistant, SummaryRun) = match &remote {
            Some((r, _)) => (r, SummaryRun { cancel: cancel.clone(), yield_to: None, gate: None, cpu_bound: false, on_pause: None }),
            None => {
                if !st.gate.wait_for_turn(true, cancel, |r| on_pause(r)) {
                    return Err(Error::Cancelled);
                }
                local = get_notes_assistant(app)?; // may load the model first
                let run = SummaryRun {
                    cancel: cancel.clone(),
                    yield_to: Some(st.asr_pending.clone()),
                    gate: Some(st.gate.clone()),
                    cpu_bound: !local.uses_gpu(),
                    on_pause: Some(on_pause.clone()),
                };
                (local.as_ref(), run)
            }
        };
        let summary = summarize(assistant, &segments, &meeting.agenda, &run, |d, t| {
            *progress.lock().unwrap() = (d, t);
            send("running", None);
        })?;
        let md = summary.to_markdown();
        st.db.lock().unwrap().set_summary(meeting_id, &md, assistant.model_id())?;
        // A short title from the notes, unless the user has named the meeting themselves.
        // Best effort: the notes are already saved, so a failure here changes nothing else.
        let user_named = st.db.lock().unwrap().get_meeting(meeting_id)?.title_source == title_source::USER;
        if !user_named {
            match generate_title(assistant, &md, &run) {
                Ok(Some(t)) => match st.db.lock().unwrap().set_model_title(meeting_id, &t) {
                    Ok(Some(_)) => log::info!("title for meeting {meeting_id} written by the model ({} chars)", t.chars().count()),
                    Ok(None) => {} // the user renamed it in the meantime
                    Err(e) => log::warn!("could not save the title: {e}"),
                },
                Ok(None) => log::info!("the model gave no usable title for meeting {meeting_id}"),
                Err(e) => log::info!("no title for meeting {meeting_id}: {e}"),
            }
        }
        Ok(md)
    })();

    match outcome {
        Ok(md) => {
            // Sizes and timing only: never log the notes themselves.
            log::info!("summary for meeting {meeting_id}: {} chars in {} ms", md.len(), started.elapsed().as_millis());
            emit(app, meeting_id, "finished", 0, None, 1, 1, false, None)
        }
        Err(Error::Cancelled) => emit(app, meeting_id, "finished", 0, None, 0, 1, true, None),
        Err(e) => {
            log::warn!("summary failed: {e}");
            emit(app, meeting_id, "finished", 0, None, 0, 1, false, Some(e.to_string()))
        }
    }
}
