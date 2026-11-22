use std::sync::atomic::Ordering;

use meetesh_core::models::store::DownloadProgress;
use meetesh_core::models::selection;
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
    /// "asr" (speech recognition) or "assist" (a language model).
    role: Role,
    name: String,
    description: String,
    size_bytes: u64,
    license: String,
    installed: bool,
    /// Bytes already on disk for an unfinished (paused or failed) download.
    partial_bytes: u64,
    is_default: bool,
    /// Which jobs use this model right now: "asr" (speech), "assist" (hints), "summary" (notes).
    selected_for: Vec<Role>,
}

/// The model chosen for each job.
#[derive(Serialize)]
pub struct ModelSelection {
    asr: String,
    assist: String,
    /// The model that writes notes (the hints model unless a different one was chosen).
    summary: String,
    summary_follows_hints: bool,
}

fn current_selection(state: &AppState) -> ModelSelection {
    let id = |slot| state.model_for(slot).map(|m| m.id.clone()).unwrap_or_default();
    ModelSelection {
        asr: id(Role::Asr),
        assist: id(Role::Assist),
        summary: id(Role::Summary),
        summary_follows_hints: !state.selection.lock().unwrap().contains_key(&Role::Summary),
    }
}

/// Every model this device can use for any job, with what each job currently uses.
#[tauri::command]
pub fn list_models(state: State<AppState>) -> Vec<ModelInfo> {
    let used: Vec<(Role, String)> = [Role::Asr, Role::Assist, Role::Summary]
        .into_iter()
        .filter_map(|slot| state.model_for(slot).ok().map(|m| (slot, m.id.clone())))
        .collect();
    let mut seen = std::collections::HashSet::new();
    [Role::Asr, Role::Assist]
        .into_iter()
        .flat_map(|slot| selection::choices(&state.manifest, PROFILE, slot))
        .filter(|m| seen.insert(m.id.clone()))
        .map(|m| ModelInfo {
            id: m.id.clone(),
            role: m.role,
            name: m.name.clone(),
            description: m.description.clone(),
            size_bytes: m.total_bytes(),
            license: m.license.clone(),
            installed: state.models.is_installed(m),
            partial_bytes: state.models.partial_bytes(m),
            is_default: state.manifest.default_for(PROFILE, m.role).is_some_and(|d| d.id == m.id),
            selected_for: used.iter().filter(|(_, id)| id == &m.id).map(|(slot, _)| *slot).collect(),
        })
        .collect()
}

#[tauri::command]
pub fn model_selection(state: State<AppState>) -> ModelSelection {
    current_selection(&state)
}

/// Choose the model for a job ("asr", "assist" or "summary"). For "summary", an empty id means
/// "same as hints". A chosen model that is installed starts loading right away.
#[tauri::command]
pub fn select_model(app: AppHandle, state: State<AppState>, slot: Role, id: String) -> Result<ModelSelection> {
    state.select_model(slot, &id)?;
    if slot != Role::Summary {
        if let Ok(entry) = state.model_for(slot) {
            if state.models.is_installed(entry) {
                runtime::preload_model(&app, &entry.id);
            }
        }
    }
    // Free language models nobody uses any more (not the one a running request still holds).
    crate::state::release_unused_assistants(&app);
    Ok(current_selection(&state))
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
        let ev = |phase, downloaded, error| DownloadEvent { id: entry.id.clone(), phase, downloaded, total: entry.total_bytes(), error };
        let result = st.models.download(
            &entry,
            |p| {
                let e = match p {
                    DownloadProgress::Bytes { downloaded, .. } => ev(DownloadPhase::Downloading, downloaded, None),
                    DownloadProgress::Verifying => ev(DownloadPhase::Verifying, entry.total_bytes(), None),
                };
                let _ = app.emit(MODEL_DOWNLOAD, e);
            },
            &st.download_cancel,
        );
        *st.downloading.lock().unwrap() = false;
        let partial = st.models.partial_bytes(&entry);
        let last = match result {
            Ok(_) => ev(DownloadPhase::Done, entry.total_bytes(), None),
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
    state.assistants.lock().unwrap().remove(&entry.id);
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
