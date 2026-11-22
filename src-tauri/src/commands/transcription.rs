use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use meetesh_core::asr::chunker::ChunkerConfig;
use meetesh_core::asr::run::transcribe_track;
use meetesh_core::audio::{system_dir, wav};
use meetesh_core::{Error, Result, Source, TranscriptSegment};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::{TranscribeEvent, TRANSCRIBE_PROGRESS};
use crate::state::{get_transcriber, AppState, TranscriptionJob};

#[tauri::command]
pub fn get_transcript(state: State<AppState>, id: i64) -> Result<Vec<TranscriptSegment>> {
    state.db.lock().unwrap().get_transcript(id)
}

#[tauri::command]
pub fn transcribe_meeting(app: AppHandle, id: i64) -> Result<()> {
    start_transcription(&app, id)
}

/// Id of the meeting being transcribed, if any (lets the UI resync after a reload).
#[tauri::command]
pub fn active_transcription(state: State<AppState>) -> Option<i64> {
    state.job.lock().unwrap().as_ref().map(|j| j.meeting_id)
}

#[tauri::command]
pub fn cancel_transcription(state: State<AppState>) {
    if let Some(job) = state.job.lock().unwrap().as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

/// Transcribe a finished recording on a background thread. Emits `transcribe-progress`.
pub fn start_transcription(app: &AppHandle, meeting_id: i64) -> Result<()> {
    let st = app.state::<AppState>();
    let meeting = st.db.lock().unwrap().get_meeting(meeting_id)?;
    if meeting.audio_dir.is_empty() || meeting.duration_ms == 0 {
        return Err(Error::Other("This meeting has no recording to transcribe".into()));
    }
    if meeting.status == "recording" {
        return Err(Error::Other("Stop the recording first".into()));
    }
    let entry = st.asr_model()?.clone();
    if !st.models.is_installed(&entry) {
        return Err(Error::Other("Download the speech model first".into()));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut job = st.job.lock().unwrap();
        if job.is_some() {
            return Err(Error::Other("A transcription is already running".into()));
        }
        *job = Some(TranscriptionJob { meeting_id, cancel: cancel.clone() });
    }

    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let emit = |progress, done, cancelled, error| {
            let _ = app.emit(TRANSCRIBE_PROGRESS, TranscribeEvent { meeting_id, progress, done, cancelled, error });
        };
        let outcome = (|| -> Result<()> {
            let transcriber = get_transcriber(&app)?;
            // The microphone, then (if it was recorded) the other participants' track. Progress is
            // shared out by how much audio each has.
            let dir = std::path::Path::new(&meeting.audio_dir);
            let sys_dir = system_dir(dir);
            let mic_ms = wav::duration_ms(dir)? as f32;
            let sys_ms = if meeting.has_system_audio { wav::duration_ms(&sys_dir).unwrap_or(0) as f32 } else { 0.0 };
            let total = (mic_ms + sys_ms).max(1.0);
            let cfg = ChunkerConfig::default;
            let mut segments = transcribe_track(
                dir,
                Source::Mic,
                transcriber.as_ref(),
                cfg(),
                &cancel,
                |p| emit(p * mic_ms / total, false, false, None),
                |_| {},
            )?;
            if sys_ms > 0.0 {
                segments.extend(transcribe_track(
                    &sys_dir,
                    Source::System,
                    transcriber.as_ref(),
                    cfg(),
                    &cancel,
                    |p| emit((mic_ms + p * sys_ms) / total, false, false, None),
                    |_| {},
                )?);
                segments.sort_by_key(|s| s.start_ms);
            }
            st.db.lock().unwrap().replace_transcript(meeting_id, &entry.id, &segments)?;
            log::info!("transcribed meeting {meeting_id}: {} segments", segments.len()); // never log text
            Ok(())
        })();
        *st.job.lock().unwrap() = None;
        match outcome {
            Ok(()) => emit(1.0, true, false, None),
            Err(Error::Cancelled) => emit(0.0, true, true, None),
            Err(e) => {
                log::warn!("transcription failed: {e}");
                emit(0.0, true, false, Some(e.to_string()))
            }
        }
    });
    Ok(())
}
