//! Shared application state and the pieces of it that several modules need.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use meetesh_core::asr::live::LiveSettle;
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::models::store::ModelStore;
use meetesh_core::models::{Manifest, ModelEntry, Profile, Role};
use meetesh_core::{Db, Error, Result};
use tauri::{AppHandle, Manager};

use crate::capture::CaptureHandle;
use meetesh_core::audio::RecordingSession;

/// Device class for picking default models from the manifest (NFR-24).
pub const PROFILE: Profile = Profile::Desktop;

pub struct ActiveRecording {
    pub meeting_id: i64,
    pub session: RecordingSession,
    pub capture: CaptureHandle,
    /// Present when live transcription was running for this recording.
    pub live: Option<Arc<Mutex<LiveSettle>>>,
}

pub struct TranscriptionJob {
    pub meeting_id: i64,
    pub cancel: Arc<AtomicBool>,
}

pub struct AppState {
    pub db: Mutex<Db>,
    pub recordings_root: PathBuf,
    pub active: Mutex<Option<ActiveRecording>>,
    pub manifest: Manifest,
    pub models: ModelStore,
    /// Loaded model, reused between transcriptions. Reloaded when the active model changes.
    pub transcriber: Mutex<Option<Arc<WhisperTranscriber>>>,
    pub job: Mutex<Option<TranscriptionJob>>,
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
        Ok(AppState {
            db: Mutex::new(db),
            recordings_root: data_dir.join("recordings"),
            active: Mutex::new(None),
            manifest: Manifest::bundled(),
            models: ModelStore::new(data_dir.join("models")),
            transcriber: Mutex::new(None),
            job: Mutex::new(None),
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
