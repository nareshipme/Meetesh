//! Shared application state and the pieces of it that several modules need.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use meetesh_core::asr::live::LiveSettle;
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::assist::llama::LlamaAssistant;
use meetesh_core::models::store::ModelStore;
use meetesh_core::models::selection;
use meetesh_core::asr::Transcriber;
use meetesh_core::models::{Engine, Manifest, ModelEntry, Profile, Role};
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

/// Meeting notes are written one meeting at a time, in the order they were asked for, by one
/// background worker. Asking for more just adds to the line.
#[derive(Default)]
pub struct NotesQueue {
    pub waiting: std::collections::VecDeque<i64>,
    pub running: Option<SummaryJob>,
    pub worker_alive: bool,
}

pub struct AppState {
    /// The "what is being discussed" list while recording.
    pub live_points: Arc<crate::live_points::LivePoints>,
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
    pub transcriber: Mutex<Option<Arc<dyn Transcriber>>>,
    pub job: Mutex<Option<TranscriptionJob>>,
    /// Loaded language models by manifest id (a few GB each). The hints model stays loaded; a
    /// different notes model is loaded when it is needed and released afterwards.
    pub assistants: Mutex<HashMap<String, Arc<Mutex<Option<Arc<LlamaAssistant>>>>>>,
    /// Explicit model choices per slot (see `models::selection`), mirrored in the settings table.
    pub selection: Mutex<HashMap<Role, String>>,
    pub hint_job: Mutex<Option<HintJob>>,
    pub notes: Mutex<NotesQueue>,
    /// Who gets the processor: hints before notes, and notes wait while a recording runs (on a CPU).
    pub gate: Arc<meetesh_core::assist::gate::ModelGate>,
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
        let stored_selection = [Role::Asr, Role::Assist, Role::Summary]
            .into_iter()
            .filter_map(|r| db.get_setting(&selection_key(r)).ok().flatten().filter(|v| !v.is_empty()).map(|v| (r, v)))
            .collect::<HashMap<_, _>>();
        Ok(AppState {
            live_points: Arc::new(crate::live_points::LivePoints::new(crate::live_points::load_enabled(&db))),
            auto_hints: Arc::new(crate::auto_hints::AutoHints::new(auto_mode)),
            db: Mutex::new(db),
            recordings_root: data_dir.join("recordings"),
            exports_root: data_dir.join("exports"),
            active: Mutex::new(None),
            manifest: Manifest::bundled(),
            models: ModelStore::new(data_dir.join("models")),
            transcriber: Mutex::new(None),
            job: Mutex::new(None),
            assistants: Mutex::new(HashMap::new()),
            selection: Mutex::new(stored_selection),
            hint_job: Mutex::new(None),
            notes: Mutex::new(NotesQueue::default()),
            gate: meetesh_core::assist::gate::ModelGate::new(),
            asr_pending: Arc::new(AtomicUsize::new(0)),
            runtime: Mutex::new(HashMap::new()),
            download_cancel: Arc::new(AtomicBool::new(false)),
            downloading: Mutex::new(false),
        })
    }

    /// The model a slot uses (see `models::selection`): the user's choice, else the default.
    pub fn model_for(&self, slot: Role) -> Result<&ModelEntry> {
        let chosen = self.selection.lock().unwrap().clone();
        selection::resolve(&self.manifest, PROFILE, slot, |r| chosen.get(&r).cloned()).ok_or_else(|| {
            Error::Other(match slot {
                Role::Asr => "No speech model configured".to_string(),
                _ => "Live assist is not available on this device".to_string(),
            })
        })
    }

    /// The speech model for transcription.
    pub fn asr_model(&self) -> Result<&ModelEntry> {
        self.model_for(Role::Asr)
    }

    /// The language model for hints, automatic hints and live points, if this device has one.
    pub fn assist_model(&self) -> Result<&ModelEntry> {
        self.model_for(Role::Assist)
    }

    /// The language model that writes meeting notes and titles (the hints model unless chosen otherwise).
    pub fn summary_model(&self) -> Result<&ModelEntry> {
        self.model_for(Role::Summary)
    }

    /// Choose a model for a slot. An empty id clears the choice (notes then follow the hints model).
    pub fn select_model(&self, slot: Role, id: &str) -> Result<()> {
        if id.is_empty() {
            if slot == Role::Summary {
                self.db.lock().unwrap().set_setting(&selection_key(slot), "")?;
                self.selection.lock().unwrap().remove(&slot);
                return Ok(());
            }
            return Err(Error::Other("Choose a model".into()));
        }
        let entry = self.manifest.get(id).ok_or_else(|| Error::Other(format!("Unknown model {id}")))?;
        if entry.role != selection::kind_of(slot) {
            return Err(Error::Other(format!("{} cannot be used for this job", entry.name)));
        }
        self.db.lock().unwrap().set_setting(&selection_key(slot), id)?;
        self.selection.lock().unwrap().insert(slot, id.to_string());
        Ok(())
    }
}

fn selection_key(slot: Role) -> String {
    format!(
        "model.{}",
        match slot {
            Role::Asr => "asr",
            Role::Assist => "assist",
            Role::Summary => "summary",
        }
    )
}

/// Context window for hints: about 1,900 tokens of prompt plus the reply fit comfortably.
const ASSIST_CONTEXT_TOKENS: u32 = 4096;

/// Load (or reuse) the hints model. Takes a couple of seconds if it is not loaded yet.
pub fn get_assistant(app: &AppHandle) -> Result<Arc<LlamaAssistant>> {
    let entry = app.state::<AppState>().assist_model()?.clone();
    load_assistant(app, &entry, |_| {})
}

/// Load (or reuse) the model that writes meeting notes (the hints model unless chosen otherwise).
pub fn get_notes_assistant(app: &AppHandle) -> Result<Arc<LlamaAssistant>> {
    let entry = app.state::<AppState>().summary_model()?.clone();
    load_assistant(app, &entry, |_| {})
}

/// Load (or reuse) a language model, reporting real loading progress (0.0 to 1.0) while it loads.
/// Models load independently: loading the notes model never blocks a hint on the hints model.
pub fn load_assistant(app: &AppHandle, entry: &ModelEntry, on_progress: impl FnMut(f32) + 'static) -> Result<Arc<LlamaAssistant>> {
    let st = app.state::<AppState>();
    let slot = st.assistants.lock().unwrap().entry(entry.id.clone()).or_default().clone();
    let mut loaded = slot.lock().unwrap();
    if let Some(a) = loaded.as_ref() {
        return Ok(a.clone());
    }
    let a = Arc::new(LlamaAssistant::load_with_progress(&entry.id, &st.models.path(entry), ASSIST_CONTEXT_TOKENS, on_progress)?);
    *loaded = Some(a.clone());
    Ok(a)
}

/// Free language models nobody needs any more (a notes model that differs from the hints model, or
/// one that was switched away from). Models still in use by a running request stay alive until it ends.
pub fn release_unused_assistants(app: &AppHandle) {
    let st = app.state::<AppState>();
    let keep: Vec<String> = [st.assist_model().ok(), st.summary_model().ok()].into_iter().flatten().map(|m| m.id.clone()).collect();
    st.assistants.lock().unwrap().retain(|id, _| keep.contains(id));
}

/// Load (or reuse) the speech model. Shared by live and full-recording transcription.
pub fn get_transcriber(app: &AppHandle) -> Result<Arc<dyn Transcriber>> {
    let st = app.state::<AppState>();
    let entry = st.asr_model()?.clone();
    let mut cached = st.transcriber.lock().unwrap();
    if let Some(t) = cached.as_ref() {
        if t.model_id() == entry.id {
            return Ok(t.clone());
        }
    }
    let t: Arc<dyn Transcriber> = match entry.engine {
        Engine::Whisper => {
            let lang = if entry.language.is_empty() { None } else { Some(entry.language.as_str()) };
            Arc::new(WhisperTranscriber::load(&entry.id, &st.models.path(&entry), lang)?)
        }
        #[cfg(feature = "parakeet-engine")]
        Engine::Parakeet => Arc::new(meetesh_core::asr::parakeet::ParakeetTranscriber::load(&entry.id, &st.models.folder(&entry))?),
        #[cfg(not(feature = "parakeet-engine"))]
        Engine::Parakeet => return Err(Error::Other("This build of Meetesh does not include the Parakeet speech engine".into())),
    };
    *cached = Some(t.clone());
    Ok(t)
}
