use std::sync::atomic::Ordering;

use meetesh_core::models::Role;
use meetesh_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::events::{DownloadEvent, MODEL_DOWNLOAD};
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
        let ev = |downloaded, done, error| DownloadEvent { id: entry.id.clone(), downloaded, total: entry.size_bytes, done, error };
        let result = st.models.download(
            &entry,
            |d, _| {
                let _ = app.emit(MODEL_DOWNLOAD, ev(d, false, None));
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
        let _ = app.emit(MODEL_DOWNLOAD, last);
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_download(state: State<AppState>) {
    state.download_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub fn delete_model(state: State<AppState>, id: String) -> Result<()> {
    let entry = state.manifest.get(&id).ok_or_else(|| Error::Other(format!("Unknown model {id}")))?;
    // Release the loaded models so their files are not held open.
    *state.transcriber.lock().unwrap() = None;
    *state.assistant.lock().unwrap() = None;
    state.models.delete(entry)
}
