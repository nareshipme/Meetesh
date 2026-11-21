use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use meetesh_core::asr::live::LiveSettle;
use meetesh_core::audio::sinks::{LevelSink, WavSink};
use meetesh_core::audio::{wav, FrameSink, RecordingSession, TARGET_RATE};
use meetesh_core::{Error, Meeting, Result};
use tauri::{AppHandle, Emitter, State};

use crate::capture;
use crate::commands::transcription::start_transcription;
use crate::events::RECORDING_LEVEL;
use crate::live::{finish_live, live_sink};
use crate::state::{ActiveRecording, AppState};

#[tauri::command]
pub fn list_input_devices() -> Result<Vec<capture::InputDevice>> {
    capture::list_input_devices().map_err(Error::Other)
}

/// Id of the meeting being recorded, if any (lets the UI resync after a reload).
#[tauri::command]
pub fn active_recording(state: State<AppState>) -> Option<i64> {
    state.active.lock().unwrap().as_ref().map(|a| a.meeting_id)
}

#[tauri::command]
pub fn start_recording(
    app: AppHandle,
    state: State<AppState>,
    meeting_id: i64,
    device: Option<String>,
) -> Result<Meeting> {
    let mut active = state.active.lock().unwrap();
    if active.is_some() {
        return Err(Error::Other("A recording is already in progress".into()));
    }
    let meeting = state.db.lock().unwrap().begin_recording(meeting_id, &state.recordings_root)?;

    let level_app = app.clone();
    let mut sinks: Vec<Box<dyn FrameSink>> = vec![
        Box::new(WavSink::new(&meeting.audio_dir)?),
        Box::new(LevelSink::new(move |l| {
            let _ = level_app.emit(RECORDING_LEVEL, l);
        })),
    ];

    // Live transcription, if the speech model is installed. Never required for recording.
    let live = match state.asr_model() {
        Ok(entry) if state.models.is_installed(entry) => {
            let outcome = Arc::new(Mutex::new(LiveSettle::default()));
            sinks.push(Box::new(live_sink(&app, meeting.id, entry.id.clone(), outcome.clone())));
            Some(outcome)
        }
        _ => None,
    };
    let session = RecordingSession::start(sinks, 256);
    let factory = session.pusher_factory();

    match capture::start_capture(device, move |rate, ch| factory.make(rate, ch)) {
        Ok(capture) => {
            log::info!("recording started for meeting {}", meeting.id);
            *active = Some(ActiveRecording { meeting_id: meeting.id, session, capture, live });
            Ok(meeting)
        }
        Err(e) => {
            // Could not open the microphone: undo everything so no empty meeting is left behind.
            let _ = session.stop();
            let _ = state.db.lock().unwrap().abort_recording(meeting.id);
            Err(Error::Other(e))
        }
    }
}

#[tauri::command]
pub fn stop_recording(app: AppHandle, state: State<AppState>) -> Result<Meeting> {
    let rec = state
        .active
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| Error::Other("No recording in progress".into()))?;
    rec.capture.stop(); // stop the microphone first, then drain the pipeline
    let report = rec.session.stop();
    if report.dropped_frames > 0 || !report.errors.is_empty() {
        log::warn!(
            "recording {} finished with {} dropped frames, errors: {:?}",
            rec.meeting_id,
            report.dropped_frames,
            report.errors
        );
    }
    let duration_ms = (report.samples * 1000 / TARGET_RATE as u64) as i64;
    log::info!("recording stopped for meeting {} ({} ms)", rec.meeting_id, duration_ms);
    let meeting = state.db.lock().unwrap().finish_recording(rec.meeting_id, duration_ms)?;
    match rec.live {
        // Live transcription is finishing in the background; it decides whether a full pass is needed.
        Some(outcome) => {
            let settled = outcome.lock().unwrap().recording_stopped();
            if let Some(report) = settled {
                finish_live(&app, meeting.id, report);
            }
        }
        // No live transcript (model not installed at start): transcribe now if possible.
        None => {
            if let Err(e) = start_transcription(&app, meeting.id) {
                log::info!("not transcribing meeting {} automatically: {e}", meeting.id);
            }
        }
    }
    Ok(meeting)
}

/// Merge the recording's segments into one WAV file and return its path.
#[tauri::command]
pub fn export_audio(state: State<AppState>, id: i64) -> Result<String> {
    let m = state.db.lock().unwrap().get_meeting(id)?;
    if m.audio_dir.is_empty() {
        return Err(Error::Other("This meeting has no recording".into()));
    }
    let dir = PathBuf::from(&m.audio_dir);
    let out = dir.join("meeting.wav");
    wav::merge_segments(&dir, &out)?;
    Ok(out.to_string_lossy().into_owned())
}
