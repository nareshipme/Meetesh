use meetesh_core::{Error, Meeting, Result};
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn create_meeting(state: State<AppState>, title: Option<String>) -> Result<Meeting> {
    state.db.lock().unwrap().create_meeting(title.as_deref())
}

#[tauri::command]
pub fn list_meetings(state: State<AppState>) -> Result<Vec<Meeting>> {
    state.db.lock().unwrap().list_meetings()
}

#[tauri::command]
pub fn rename_meeting(state: State<AppState>, id: i64, title: String) -> Result<Meeting> {
    state.db.lock().unwrap().rename_meeting(id, &title)
}

#[tauri::command]
pub fn delete_meeting(state: State<AppState>, id: i64) -> Result<()> {
    if state.active.lock().unwrap().as_ref().map(|a| a.meeting_id) == Some(id) {
        return Err(Error::Other("Stop the recording before deleting this meeting".into()));
    }
    state.db.lock().unwrap().delete_meeting(id)
}
