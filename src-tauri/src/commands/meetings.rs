use meetesh_core::db::SearchResult;
use meetesh_core::{export, Error, Meeting, Result};
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

#[tauri::command]
pub fn set_agenda(state: State<AppState>, id: i64, agenda: String) -> Result<Meeting> {
    state.db.lock().unwrap().set_agenda(id, &agenda)
}

/// Search titles, notes and transcripts of all meetings.
#[tauri::command]
pub fn search_meetings(state: State<AppState>, query: String) -> Result<Vec<SearchResult>> {
    state.db.lock().unwrap().search(&query)
}

fn markdown_of(state: &AppState, id: i64) -> Result<(meetesh_core::Meeting, String)> {
    let db = state.db.lock().unwrap();
    let meeting = db.get_meeting(id)?;
    let segments = db.get_transcript(id)?;
    let when = db.local_time(meeting.created_at)?;
    let md = export::markdown(&meeting, &segments, &when);
    Ok((meeting, md))
}

/// The meeting (notes and transcript) as Markdown text, for the clipboard.
#[tauri::command]
pub fn meeting_markdown(state: State<AppState>, id: i64) -> Result<String> {
    Ok(markdown_of(&state, id)?.1)
}

/// Write the meeting as a Markdown file in the app's exports folder and return its path.
#[tauri::command]
pub fn export_markdown(state: State<AppState>, id: i64) -> Result<String> {
    let (meeting, md) = markdown_of(&state, id)?;
    std::fs::create_dir_all(&state.exports_root)?;
    let path = state.exports_root.join(export::file_name(&meeting));
    std::fs::write(&path, md)?;
    Ok(path.to_string_lossy().into_owned())
}
