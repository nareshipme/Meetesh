//! Desktop shell: a thin command layer over `meetesh-core`.
//! Business logic belongs in the core crate, not here (NFR-22).

mod capture;

use std::path::PathBuf;
use std::sync::Mutex;

use meetesh_core::audio::sinks::{LevelSink, WavSink};
use meetesh_core::audio::{wav, FrameSink, RecordingSession, TARGET_RATE};
use meetesh_core::{Db, Error, Meeting, Result};
use tauri::{AppHandle, Emitter, Manager, State};

struct ActiveRecording {
    meeting_id: i64,
    session: RecordingSession,
    capture: capture::CaptureHandle,
}

struct AppState {
    db: Mutex<Db>,
    recordings_root: PathBuf,
    active: Mutex<Option<ActiveRecording>>,
}

#[tauri::command]
fn create_meeting(state: State<AppState>, title: Option<String>) -> Result<Meeting> {
    state.db.lock().unwrap().create_meeting(title.as_deref())
}

#[tauri::command]
fn list_meetings(state: State<AppState>) -> Result<Vec<Meeting>> {
    state.db.lock().unwrap().list_meetings()
}

#[tauri::command]
fn rename_meeting(state: State<AppState>, id: i64, title: String) -> Result<Meeting> {
    state.db.lock().unwrap().rename_meeting(id, &title)
}

#[tauri::command]
fn delete_meeting(state: State<AppState>, id: i64) -> Result<()> {
    if state.active.lock().unwrap().as_ref().map(|a| a.meeting_id) == Some(id) {
        return Err(Error::Other("Stop the recording before deleting this meeting".into()));
    }
    state.db.lock().unwrap().delete_meeting(id)
}

#[tauri::command]
fn list_input_devices() -> Result<Vec<capture::InputDevice>> {
    capture::list_input_devices().map_err(Error::Other)
}

/// Id of the meeting being recorded, if any (lets the UI resync after a reload).
#[tauri::command]
fn active_recording(state: State<AppState>) -> Option<i64> {
    state.active.lock().unwrap().as_ref().map(|a| a.meeting_id)
}

#[tauri::command]
fn start_recording(
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
    let sinks: Vec<Box<dyn FrameSink>> = vec![
        Box::new(WavSink::new(&meeting.audio_dir)?),
        Box::new(LevelSink::new(move |l| {
            let _ = level_app.emit("recording-level", l);
        })),
    ];
    let session = RecordingSession::start(sinks, 256);
    let factory = session.pusher_factory();

    match capture::start_capture(device, move |rate, ch| factory.make(rate, ch)) {
        Ok(capture) => {
            log::info!("recording started for meeting {}", meeting.id);
            *active = Some(ActiveRecording { meeting_id: meeting.id, session, capture });
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
fn stop_recording(state: State<AppState>) -> Result<Meeting> {
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
    state.db.lock().unwrap().finish_recording(rec.meeting_id, duration_ms)
}

/// Merge the recording's segments into one WAV file and return its path.
#[tauri::command]
fn export_audio(state: State<AppState>, id: i64) -> Result<String> {
    let m = state.db.lock().unwrap().get_meeting(id)?;
    if m.audio_dir.is_empty() {
        return Err(Error::Other("This meeting has no recording".into()));
    }
    let dir = PathBuf::from(&m.audio_dir);
    let out = dir.join("meeting.wav");
    wav::merge_segments(&dir, &out)?;
    Ok(out.to_string_lossy().into_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Log file in the app log dir, plus stdout (NFR-15). Never log transcript content.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .max_file_size(10_000_000)
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = Db::open(dir.join("meetesh.db"))?;
            log::info!("database ready, schema v{}", db.schema_version()?);
            // A meeting still marked "recording" means the app died mid-recording: keep its audio.
            let recovered = db.recover_interrupted()?;
            if !recovered.is_empty() {
                log::warn!("recovered {} interrupted recording(s)", recovered.len());
            }
            app.manage(AppState {
                db: Mutex::new(db),
                recordings_root: dir.join("recordings"),
                active: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            create_meeting,
            list_meetings,
            rename_meeting,
            delete_meeting,
            list_input_devices,
            active_recording,
            start_recording,
            stop_recording,
            export_audio
        ])
        .run(tauri::generate_context!())
        .expect("error while running Meetesh");
}
