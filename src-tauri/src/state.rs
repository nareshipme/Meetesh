//! Shared application state and the pieces of it that several modules need.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use meetesh_core::asr::live::LiveSettle;
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::models::store::ModelStore;
use meetesh_core::models::{Manifest, ModelEntry, Profile, Role};
use meetesh_core::{Db, Error, Result};
use tauri::{AppHandle, Manager};

use crate::capture::CaptureHandle;
use crate::events::ModelLoadEvent;
use meetesh_core::audio::RecordingSession;

/// Device class for picking default models from the manifest (NFR-24).
pub const PROFILE: Profile = Profile::Desktop;

/// The other participants' audio, recorded next to the microphone.
pub struct SystemTrack {
    pub session: RecordingSession,
    pub capture: CaptureHandle,
}

pub struct ActiveRecording {
    pub meeting_id: i64,
    pub session: RecordingSession,
    pub capture: CaptureHandle,
    pub system: Option<SystemTrack>,
    /// Present when live transcription was running for this recording.
    pub live: Option<Arc<Mutex<LiveSettle>>>,
}

pub struct TranscriptionJob {
    pub meeting_id: i64,
    pub cancel: Arc<AtomicBool>,
}

pub struct HintJob {
    pub meeting_id: i64,
    pub cancel: Arc<AtomicBool>,
}

pub struct SummaryJob {
    pub meeting_id: i64,
    pub cancel: Arc<AtomicBool>,
}

pub struct AppState {
    /// Hints offered by themselves while recording (opt-in).
    pub auto_hints: Arc<crate::auto_hints::AutoHints>,
    pub db: Mutex<Db>,
    pub recordings_root: PathBuf,
    /// Where "Export Markdown" writes its files.
    pub exports_root: PathBuf,
    pub active: Mutex<Option<ActiveRecording>>,
    pub manifest: Manifest,
    pub models: ModelStore,
    /// Loaded model, reused between transcriptions. Reloaded when the active model changes.
    pub transcriber: Mutex<Option<Arc<WhisperTranscriber>>>,
    pub job: Mutex<Option<TranscriptionJob>>,
    /// Loaded assist model (about 4 GB in memory). Loaded on the first hint, kept afterwards.
    pub assistant: Mutex<Option<Arc<LlamaAssistant>>>,
    pub hint_job: Mutex<Option<HintJob>>,
    pub summary_job: Mutex<Option<SummaryJob>>,
    /// Live transcription chunks queued or in progress. Hint generation yields while this is above
    /// zero, so a hint never makes the live transcript lag.
    pub asr_pending: Arc<AtomicUsize>,
    /// Latest load state per model id, so the UI can sync after a reload.
    pub runtime: Mutex<HashMap<String, ModelLoadEvent>>,
    pub download_cancel: Arc<AtomicBool>,
    pub downloading: Mutex<bool>,
}

impl AppState {
    /// Open the database in `data_dir`, recover interrupted recordings, and set up the model store.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db = Db::open(data_dir.join("meetesh.db"))?;
        log::info!("database ready, schema v{}", db.schema_version()?);
        // A meeting still marked "recording" means the app died mid-recording: keep its audio.
        let recovered = db.recover_interrupted()?;
        if !recovered.is_empty() {
            log::warn!("recovered {} interrupted recording(s)", recovered.len());
        }
        let auto_mode = db
            .get_setting(crate::commands::assist::AUTO_HINTS_KEY)?
            .map(|s| meetesh_core::assist::trigger::AutoHintMode::parse(&s))
            .unwrap_or_default();
        Ok(AppState {
            auto_hints: Arc::new(crate::auto_hints::AutoHints::new(auto_mode)),
            db: Mutex::new(db),
            recordings_root: data_dir.join("recordings"),
            exports_root: data_dir.join("exports"),
            active: Mutex::new(None),
            manifest: Manifest::bundled(),
            models: ModelStore::new(data_dir.join("models")),
            transcriber: Mutex::new(None),
            job: Mutex::new(None),
            assistant: Mutex::new(None),
            hint_job: Mutex::new(None),
            summary_job: Mutex::new(None),
            asr_pending: Arc::new(AtomicUsize::new(0)),
            runtime: Mutex::new(HashMap::new()),
            download_cancel: Arc::new(AtomicBool::new(false)),
            downloading: Mutex::new(false),
        })
    }

    /// The speech model the app is configured to use for this device.
    pub fn asr_model(&self) -> Result<&ModelEntry> {
        self.manifest
            .default_for(PROFILE, Role::Asr)
            .ok_or_else(|| Error::Other("No speech model configured".into()))
    }
}

impl AppState {
    /// The live-assist model the app is configured to use for this device, if there is one.
    pub fn assist_model(&self) -> Result<&ModelEntry> {
        self.manifest
            .default_for(PROFILE, Role::Assist)
            .ok_or_else(|| Error::Other("Live assist is not available on this device".into()))
    }
}

/// Context window for hints: about 1,900 tokens of prompt plus the reply fit comfortably.
const ASSIST_CONTEXT_TOKENS: u32 = 4096;

/// Load (or reuse) the assist model. Takes a couple of seconds if it is not loaded yet.
pub fn get_assistant(app: &AppHandle) -> Result<Arc<LlamaAssistant>> {
    load_assistant(app, |_| {})
}

/// Like `get_assistant`, reporting real loading progress (0.0 to 1.0) while it loads.
pub fn load_assistant(app: &AppHandle, on_progress: impl FnMut(f32) + 'static) -> Result<Arc<LlamaAssistant>> {
    let st = app.state::<AppState>();
    let entry = st.assist_model()?.clone();
    let mut cached = st.assistant.lock().unwrap();
    if let Some(a) = cached.as_ref() {
        if meetesh_core::assist::Assistant::model_id(a.as_ref()) == entry.id {
            return Ok(a.clone());
        }
    }
    let a = Arc::new(LlamaAssistant::load_with_progress(&entry.id, &st.models.path(&entry), ASSIST_CONTEXT_TOKENS, on_progress)?);
    *cached = Some(a.clone());
    Ok(a)
}

/// Load (or reuse) the speech model. Shared by live and full-recording transcription.
pub fn get_transcriber(app: &AppHandle) -> Result<Arc<WhisperTranscriber>> {
    let st = app.state::<AppState>();
    let entry = st.asr_model()?.clone();
    let mut cached = st.transcriber.lock().unwrap();
    if let Some(t) = cached.as_ref() {
        if meetesh_core::Transcriber::model_id(t.as_ref()) == entry.id {
            return Ok(t.clone());
        }
    }
    let lang = if entry.language.is_empty() { None } else { Some(entry.language.as_str()) };
    let t = Arc::new(WhisperTranscriber::load(&entry.id, &st.models.path(&entry), lang)?);
    *cached = Some(t.clone());
    Ok(t)
}
