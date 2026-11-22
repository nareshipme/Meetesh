//! Loads models into memory in the background so nobody waits for them when they need them:
//! at startup, and right after a download finishes. Every step is published as a `model-load`
//! event (and kept in `AppState::runtime`) so the UI can show clear progress.

use meetesh_core::models::{ModelEntry, Role};
use meetesh_core::Result;
use meetesh_core::assist::llama::LlamaAssistant;
use tauri::{AppHandle, Emitter, Manager};

use crate::events::{LoadState, ModelLoadEvent, MODEL_LOAD};
use crate::state::{get_transcriber, load_assistant, AppState, PROFILE};

fn publish(app: &AppHandle, ev: ModelLoadEvent) {
    app.state::<AppState>().runtime.lock().unwrap().insert(ev.id.clone(), ev.clone());
    let _ = app.emit(MODEL_LOAD, ev);
}

fn event(entry: &ModelEntry, state: LoadState) -> ModelLoadEvent {
    ModelLoadEvent { id: entry.id.clone(), role: entry.role, state, progress: None, backend: None, error: None }
}

/// Forget a model's load state (after it is deleted).
pub fn forget(app: &AppHandle, entry: &ModelEntry) {
    app.state::<AppState>().runtime.lock().unwrap().remove(&entry.id);
    let _ = app.emit(MODEL_LOAD, event(entry, LoadState::Idle));
}

/// Load every installed default model (speech first, then live assist), one after the other.
pub fn preload_all(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let entries: Vec<ModelEntry> = [Role::Asr, Role::Assist]
            .into_iter()
            .filter_map(|r| st.manifest.default_for(PROFILE, r).cloned())
            .filter(|m| st.models.is_installed(m))
            .collect();
        for entry in entries {
            load_blocking(&app, &entry);
        }
    });
}

/// Load one model in the background (used after a download finishes, and by "Retry").
pub fn preload_model(app: &AppHandle, id: &str) {
    let app = app.clone();
    let id = id.to_string();
    std::thread::spawn(move || {
        let entry = app.state::<AppState>().manifest.get(&id).cloned();
        if let Some(entry) = entry {
            load_blocking(&app, &entry);
        }
    });
}

fn load_blocking(app: &AppHandle, entry: &ModelEntry) {
    let started = std::time::Instant::now();
    publish(app, event(entry, LoadState::Loading));
    match load_one(app, entry) {
        Ok(backend) => {
            log::info!("model {} ready on {} after {} ms", entry.id, backend, started.elapsed().as_millis());
            publish(app, ModelLoadEvent { backend: Some(backend), ..event(entry, LoadState::Ready) });
        }
        Err(e) => {
            log::warn!("could not load model {}: {e}", entry.id);
            publish(app, ModelLoadEvent { error: Some(e.to_string()), ..event(entry, LoadState::Failed) });
        }
    }
}

/// Shown instead of plain "CPU" when the app was built without a GPU backend, so nobody wonders why
/// a computer with a GPU is not using it.
pub const NO_GPU_BUILD: &str = "CPU (no GPU support in this build)";

/// Returns the backend label ("GPU" or "CPU") once the model is loaded and warmed up.
fn load_one(app: &AppHandle, entry: &ModelEntry) -> Result<String> {
    match entry.role {
        Role::Asr => {
            let t = get_transcriber(app)?;
            publish(app, event(entry, LoadState::Warming));
            // A second of silence: touches the weights and allocates working memory.
            meetesh_core::Transcriber::transcribe(t.as_ref(), &vec![0.0f32; 16_000])?;
            Ok("CPU".into())
        }
        Role::Assist => {
            // Report real progress from llama.cpp, but only when the whole percent changes.
            let (app2, entry2) = (app.clone(), entry.clone());
            let mut last = -1i32;
            let assistant = load_assistant(app, move |p| {
                let pct = (p * 100.0) as i32;
                if pct != last {
                    last = pct;
                    publish(&app2, ModelLoadEvent { progress: Some(p), ..event(&entry2, LoadState::Loading) });
                }
            })?;
            // The first request compiles GPU kernels and settles GPU versus CPU; do it now.
            publish(app, ModelLoadEvent { progress: Some(1.0), ..event(entry, LoadState::Warming) });
            let warm = assistant.warm_up()?;
            if let Some(reason) = &warm.fallback_reason {
                // The GPU could not run the model, so the CPU took over during warm-up.
                log::warn!("GPU failed while warming up {} ({reason}); using the CPU", entry.id);
                return Ok("CPU (GPU failed)".into());
            }
            if assistant.uses_gpu() {
                return Ok("GPU".into());
            }
            // On the CPU. Say why when the cause is that this build has no GPU support at all.
            Ok(if LlamaAssistant::compiled_gpu_backend().is_none() { NO_GPU_BUILD } else { "CPU" }.into())
        }
        Role::Summary => Ok("CPU".into()),
    }
}
