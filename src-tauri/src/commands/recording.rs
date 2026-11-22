use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use meetesh_core::asr::live::LiveSettle;
use meetesh_core::audio::sinks::{LevelSink, WavSink};
use meetesh_core::audio::{system_dir, wav, FrameSink, RecordingSession, TARGET_RATE};
use meetesh_core::{Error, Meeting, Result, Source};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::capture;
use crate::commands::transcription::start_transcription;
use crate::events::RECORDING_LEVEL;
use crate::live::{finish_live, live_sink};
use crate::state::{ActiveRecording, AppState, SystemTrack};
use crate::system_audio;

#[tauri::command]
pub fn list_input_devices() -> Result<Vec<capture::InputDevice>> {
    capture::list_input_devices().map_err(Error::Other)
}

/// Id of the meeting being recorded, if any (lets the UI resync after a reload).
#[tauri::command]
pub fn active_recording(state: State<AppState>) -> Option<i64> {
    state.active.lock().unwrap().as_ref().map(|a| a.meeting_id)
}

/// Result of starting a recording. The microphone is the recording; the other participants'
/// audio is best effort, and `system_audio_error` says why it is missing when it was asked for.
#[derive(Serialize)]
pub struct StartedRecording {
    pub meeting: Meeting,
    pub system_audio_error: Option<String>,
}

#[tauri::command]
pub fn system_audio_support() -> system_audio::SystemAudioSupport {
    system_audio::support()
}

#[tauri::command]
pub fn start_recording(
    app: AppHandle,
    state: State<AppState>,
    meeting_id: i64,
    device: Option<String>,
    system_audio: Option<bool>,
) -> Result<StartedRecording> {
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
    let mut live_model: Option<String> = None;
    let live = match state.asr_model() {
        Ok(entry) if state.models.is_installed(entry) => {
            let outcome = Arc::new(Mutex::new(LiveSettle::default()));
            sinks.push(Box::new(live_sink(&app, meeting.id, entry.id.clone(), Source::Mic, outcome.clone())));
            live_model = Some(entry.id.clone());
            Some(outcome)
        }
        _ => None,
    };
    let session = RecordingSession::start(sinks, 256);
    let factory = session.pusher_factory();

    let capture = match capture::start_capture(device, move |rate, ch| factory.make(rate, ch)) {
        Ok(capture) => capture,
        Err(e) => {
            // Could not open the microphone: undo everything so no empty meeting is left behind.
            let _ = session.stop();
            let _ = state.db.lock().unwrap().abort_recording(meeting.id);
            return Err(Error::Other(e));
        }
    };
    // The meeting clock starts now. The other participants' track is padded with silence up to its
    // first sound, so both tracks share this clock.
    let clock_start = Instant::now();

    let mut system_audio_error = None;
    let mut system = None;
    if system_audio.unwrap_or(false) {
        match start_system_track(&app, &meeting, live.as_ref(), live_model, clock_start) {
            Ok(track) => system = Some(track),
            Err(e) => {
                log::warn!("system audio not recorded for meeting {}: {e}", meeting.id);
                system_audio_error = Some(e);
            }
        }
    }
    let meeting = if system.is_some() {
        state.db.lock().unwrap().set_system_audio(meeting.id)?
    } else {
        meeting
    };

    log::info!("recording started for meeting {} (system audio: {})", meeting.id, system.is_some());
    *active = Some(ActiveRecording { meeting_id: meeting.id, session, capture, system, live });
    Ok(StartedRecording { meeting, system_audio_error })
}

/// Record the other participants as a second track in `<audio_dir>/system`, with its own live
/// transcription. On failure nothing is left behind and the microphone recording carries on.
fn start_system_track(
    app: &AppHandle,
    meeting: &Meeting,
    live: Option<&Arc<Mutex<LiveSettle>>>,
    live_model: Option<String>,
    clock_start: Instant,
) -> std::result::Result<SystemTrack, String> {
    let support = system_audio::support();
    if !support.available {
        return Err(support.reason);
    }
    let dir = system_dir(Path::new(&meeting.audio_dir));
    let mut sinks: Vec<Box<dyn FrameSink>> = vec![Box::new(WavSink::new(&dir).map_err(|e| e.to_string())?)];
    if let (Some(outcome), Some(model)) = (live, live_model) {
        outcome.lock().unwrap().expect_another_worker();
        sinks.push(Box::new(live_sink(app, meeting.id, model, Source::System, outcome.clone())));
    }
    let session = RecordingSession::start(sinks, 256);
    let factory = session.pusher_factory();
    match system_audio::start(move |rate, ch| factory.make(rate, ch).fill_gaps_from(clock_start)) {
        Ok(capture) => Ok(SystemTrack { session, capture }),
        Err(e) => {
            let _ = session.stop(); // its live worker reports an empty result, so settling still completes
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
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
    rec.capture.stop(); // stop the capture first, then drain the pipeline
    let report = rec.session.stop();
    if let Some(system) = rec.system {
        system.capture.stop();
        let sys = system.session.stop();
        if sys.dropped_frames > 0 || !sys.errors.is_empty() {
            log::warn!(
                "system audio of recording {} finished with {} dropped frames, errors: {:?}",
                rec.meeting_id,
                sys.dropped_frames,
                sys.errors
            );
        }
    }
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
