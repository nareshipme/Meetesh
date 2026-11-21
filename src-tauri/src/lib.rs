//! Desktop shell: a thin command layer over `meetesh-core`.
//! Business logic belongs in the core crate, not here (NFR-22).

use std::sync::Mutex;

use meetesh_core::{Db, Meeting, Result};
use tauri::{Manager, State};

struct AppState {
    db: Mutex<Db>,
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
    state.db.lock().unwrap().delete_meeting(id)
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
            app.manage(AppState { db: Mutex::new(db) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            create_meeting,
            list_meetings,
            rename_meeting,
            delete_meeting
        ])
        .run(tauri::generate_context!())
        .expect("error while running Meetesh");
}
