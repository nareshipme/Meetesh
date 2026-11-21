//! Desktop shell: a thin command layer over `meetesh-core`.
//! Business logic belongs in the core crate, not here (NFR-22).

mod capture;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use meetesh_core::asr::run::transcribe_recording;
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::audio::sinks::{LevelSink, WavSink};
use meetesh_core::audio::{wav, FrameSink, RecordingSession, TARGET_RATE};
use meetesh_core::models::store::ModelStore;
use meetesh_core::models::{Manifest, ModelEntry, Profile, Role};
use meetesh_core::{Db, Error, Meeting, Result, TranscriptSegment};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Device class for picking default models from the manifest (NFR-24).
const PROFILE: Profile = Profile::Desktop;

struct ActiveRecording {
    meeting_id: i64,
    session: RecordingSession,
    capture: capture::CaptureHandle,
}

struct TranscriptionJob {
    meeting_id: i64,
    cancel: Arc<AtomicBool>,
}

struct AppState {
    db: Mutex<Db>,
    recordings_root: PathBuf,
    active: Mutex<Option<ActiveRecording>>,
    manifest: Manifest,
    models: ModelStore,
    /// Loaded model, reused between transcriptions. Reloaded when the active model changes.
    transcriber: Mutex<Option<Arc<WhisperTranscriber>>>,
    job: Mutex<Option<TranscriptionJob>>,
    download_cancel: Arc<AtomicBool>,
    downloading: Mutex<bool>,
}

impl AppState {
    fn asr_model(&self) -> Result<&ModelEntry> {
        self.manifest
            .default_for(PROFILE, Role::Asr)
            .ok_or_else(|| Error::Other("No speech model configured".into()))
    }
}

#[derive(Serialize)]
struct ModelInfo {
    id: String,
    name: String,
    description: String,
    size_bytes: u64,
    license: String,
    installed: bool,
    is_default: bool,
}

#[derive(Serialize, Clone)]
struct DownloadEvent {
    id: String,
    downloaded: u64,
    total: u64,
    done: bool,
    error: Option<String>,
}

#[derive(Serialize, Clone)]
struct TranscribeEvent {
    meeting_id: i64,
    progress: f32,
    done: bool,
    cancelled: bool,
    error: Option<String>,
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
fn stop_recording(app: AppHandle, state: State<AppState>) -> Result<Meeting> {
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
    // Transcribe automatically when the speech model is installed.
    if let Err(e) = start_transcription(&app, meeting.id) {
        log::info!("not transcribing meeting {} automatically: {e}", meeting.id);
    }
    Ok(meeting)
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


#[tauri::command]
fn list_models(state: State<AppState>) -> Vec<ModelInfo> {
    let default_id = state.manifest.default_for(PROFILE, Role::Asr).map(|m| m.id.clone());
    state
        .manifest
        .by_role(Role::Asr)
        .map(|m| ModelInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
            size_bytes: m.size_bytes,
            license: m.license.clone(),
            installed: state.models.is_installed(m),
            is_default: default_id.as_deref() == Some(m.id.as_str()),
        })
        .collect()
}

#[tauri::command]
fn download_model(app: AppHandle, state: State<AppState>, id: String) -> Result<()> {
    let entry = state.manifest.get(&id).cloned().ok_or_else(|| Error::Other(format!("Unknown model {id}")))?;
    {
        let mut busy = state.downloading.lock().unwrap();
        if *busy {
            return Err(Error::Other("A download is already in progress".into()));
        }
        *busy = true;
    }
    state.download_cancel.store(false, Ordering::Relaxed);
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let ev = |downloaded, done, error| DownloadEvent { id: entry.id.clone(), downloaded, total: entry.size_bytes, done, error };
        let result = st.models.download(
            &entry,
            |d, _| {
                let _ = app.emit("model-download", ev(d, false, None));
            },
            &st.download_cancel,
        );
        *st.downloading.lock().unwrap() = false;
        let last = match result {
            Ok(_) => ev(entry.size_bytes, true, None),
            Err(Error::Cancelled) => ev(0, true, Some("cancelled".into())),
            Err(e) => {
                log::warn!("model download failed: {e}");
                ev(0, true, Some(e.to_string()))
            }
        };
        let _ = app.emit("model-download", last);
    });
    Ok(())
}

#[tauri::command]
fn cancel_download(state: State<AppState>) {
    state.download_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn delete_model(state: State<AppState>, id: String) -> Result<()> {
    let entry = state.manifest.get(&id).ok_or_else(|| Error::Other(format!("Unknown model {id}")))?;
    *state.transcriber.lock().unwrap() = None; // release the file
    state.models.delete(entry)
}

#[tauri::command]
fn get_transcript(state: State<AppState>, id: i64) -> Result<Vec<TranscriptSegment>> {
    state.db.lock().unwrap().get_transcript(id)
}

#[tauri::command]
fn transcribe_meeting(app: AppHandle, id: i64) -> Result<()> {
    start_transcription(&app, id)
}

/// Id of the meeting being transcribed, if any (lets the UI resync after a reload).
#[tauri::command]
fn active_transcription(state: State<AppState>) -> Option<i64> {
    state.job.lock().unwrap().as_ref().map(|j| j.meeting_id)
}

#[tauri::command]
fn cancel_transcription(state: State<AppState>) {
    if let Some(job) = state.job.lock().unwrap().as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

/// Transcribe a finished recording on a background thread. Emits `transcribe-progress`.
fn start_transcription(app: &AppHandle, meeting_id: i64) -> Result<()> {
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
            let _ = app.emit("transcribe-progress", TranscribeEvent { meeting_id, progress, done, cancelled, error });
        };
        let outcome = (|| -> Result<()> {
            let transcriber = {
                let mut cached = st.transcriber.lock().unwrap();
                match cached.as_ref() {
                    Some(t) if meetesh_core::Transcriber::model_id(t.as_ref()) == entry.id => t.clone(),
                    _ => {
                        let lang = if entry.language.is_empty() { None } else { Some(entry.language.as_str()) };
                        let t = Arc::new(WhisperTranscriber::load(&entry.id, &st.models.path(&entry), lang)?);
                        *cached = Some(t.clone());
                        t
                    }
                }
            };
            let segments = transcribe_recording(
                std::path::Path::new(&meeting.audio_dir),
                transcriber.as_ref(),
                &cancel,
                |p| emit(p, false, false, None),
                |_| {},
            )?;
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
                manifest: Manifest::bundled(),
                models: ModelStore::new(dir.join("models")),
                transcriber: Mutex::new(None),
                job: Mutex::new(None),
                download_cancel: Arc::new(AtomicBool::new(false)),
                downloading: Mutex::new(false),
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
            export_audio,
            list_models,
            download_model,
            cancel_download,
            delete_model,
            get_transcript,
            transcribe_meeting,
            cancel_transcription,
            active_transcription
        ])
        .run(tauri::generate_context!())
        .expect("error while running Meetesh");
}
