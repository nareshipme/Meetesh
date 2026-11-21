//! Desktop shell: a thin command layer over `meetesh-core`.
//! Business logic belongs in the core crate, not here (NFR-22).
//!
//! - `state`: shared application state
//! - `commands`: Tauri commands grouped by area (meetings, recording, models, transcription)
//! - `live`: glue for live transcription
//! - `events`: events pushed to the UI
//! - `capture`: microphone capture (platform specific)

mod capture;
mod commands;
mod events;
mod live;
mod state;

use commands::{assist, meetings, models, recording, transcription};
use state::AppState;
use tauri::Manager;

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
            let state = AppState::open(&app.path().app_data_dir()?)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            meetings::create_meeting,
            meetings::list_meetings,
            meetings::rename_meeting,
            meetings::delete_meeting,
            meetings::set_agenda,
            recording::list_input_devices,
            recording::active_recording,
            recording::start_recording,
            recording::stop_recording,
            recording::export_audio,
            models::list_models,
            models::download_model,
            models::cancel_download,
            models::delete_model,
            transcription::get_transcript,
            transcription::transcribe_meeting,
            transcription::cancel_transcription,
            transcription::active_transcription,
            assist::request_hint,
            assist::cancel_hint,
            assist::assist_ready,
            assist::active_hint
        ])
        .run(tauri::generate_context!())
        .expect("error while running Meetesh");
}
