use std::sync::atomic::Ordering;

use meetesh_core::models::store::DownloadProgress;
use meetesh_core::models::Role;
use meetesh_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::{DownloadEvent, DownloadPhase, ModelLoadEvent, MODEL_DOWNLOAD};
use crate::runtime;
use crate::state::{AppState, PROFILE};

#[derive(Serialize)]
pub struct ModelInfo {
    id: String,
    /// "asr" (speech recognition) or "assist" (live hints).
    role: Role,
    name: String,
    description: String,
    size_bytes: u64,
    license: String,
    installed: bool,
    /// Bytes already on disk for an unfinished (paused or failed) download.
    partial_bytes: u64,
    is_default: bool,
}

#[tauri::command]
pub fn list_models(state: State<AppState>) -> Vec<ModelInfo> {
    // Every model of a role this device has a default for. Roles with no default (e.g. assist on
    // mobile) are simply not offered.
    let defaults: Vec<(Role, String)> = [Role::Asr, Role::Assist]
        .into_iter()
        .filter_map(|r| state.manifest.default_for(PROFILE, r).map(|m| (r, m.id.clone())))
        .collect();
    defaults
        .iter()
        .flat_map(|(role, _)| state.manifest.by_role(*role))
        .map(|m| ModelInfo {
            id: m.id.clone(),
            role: m.role,
            name: m.name.clone(),
            description: m.description.clone(),
            size_bytes: m.size_bytes,
            license: m.license.clone(),
            installed: state.models.is_installed(m),
            partial_bytes: state.models.partial_bytes(m),
            is_default: defaults.iter().any(|(_, id)| id == &m.id),
        })
        .collect()
}

#[tauri::command]
pub fn download_model(app: AppHandle, state: State<AppState>, id: String) -> Result<()> {
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
        let ev = |phase, downloaded, error| DownloadEvent { id: entry.id.clone(), phase, downloaded, total: entry.size_bytes, error };
        let result = st.models.download(
            &entry,
            |p| {
                let e = match p {
                    DownloadProgress::Bytes { downloaded, .. } => ev(DownloadPhase::Downloading, downloaded, None),
                    DownloadProgress::Verifying => ev(DownloadPhase::Verifying, entry.size_bytes, None),
                };
                let _ = app.emit(MODEL_DOWNLOAD, e);
            },
            &st.download_cancel,
        );
        *st.downloading.lock().unwrap() = false;
        let partial = st.models.partial_bytes(&entry);
        let last = match result {
            Ok(_) => ev(DownloadPhase::Done, entry.size_bytes, None),
            // "Pause": the partial file is kept so the download can continue later.
            Err(Error::Cancelled) => ev(DownloadPhase::Paused, partial, None),
            Err(e) => {
                log::warn!("model download failed: {e}");
                ev(DownloadPhase::Failed, partial, Some(e.to_string()))
            }
        };
        let finished = last.phase == DownloadPhase::Done;
        let _ = app.emit(MODEL_DOWNLOAD, last);
        if finished {
            runtime::preload_model(&app, &entry.id); // load it now so nobody waits later
        }
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<AppState>) {
    state.download_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub fn delete_model(app: AppHandle, state: State<AppState>, id: String) -> Result<()> {
    let entry = state.manifest.get(&id).ok_or_else(|| Error::Other(format!("Unknown model {id}")))?;
    // Release the loaded models so their files are not held open.
    *state.transcriber.lock().unwrap() = None;
    *state.assistant.lock().unwrap() = None;
    runtime::forget(&app, entry);
    state.models.delete(entry)
}

/// Throw away a paused or failed download's partial file.
#[tauri::command]
pub fn discard_download(state: State<AppState>, id: String) -> Result<()> {
    if *state.downloading.lock().unwrap() {
        return Err(Error::Other("A download is in progress; pause it first".into()));
    }
    let entry = state.manifest.get(&id).ok_or_else(|| Error::Other(format!("Unknown model {id}")))?;
    state.models.discard_partial(entry)
}

/// Where every installed model is on its way into memory (the UI syncs from this after a reload).
#[tauri::command]
pub fn model_runtime(state: State<AppState>) -> Vec<ModelLoadEvent> {
    state.runtime.lock().unwrap().values().cloned().collect()
}

/// Try loading a model again after a failure.
#[tauri::command]
pub fn retry_model_load(app: AppHandle, id: String) {
    runtime::preload_model(&app, &id);
}

/// Folder with the app's log file, so the UI can point people at it when something fails.
#[tauri::command]
pub fn log_dir(app: AppHandle) -> Option<String> {
    app.path().app_log_dir().ok().map(|p| p.to_string_lossy().into_owned())
}
