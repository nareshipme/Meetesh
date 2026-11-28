//! The permissions screen: what each feature needs, what the system says, and the controls.
//! The wording and rules are in `meetesh_core::permissions`; this only gathers the facts and applies
//! the switches that live inside Meetesh.

use meetesh_core::permissions::{self, Inputs, Permission};
use meetesh_core::{Error, Result};
use tauri::State;

use crate::os_permissions;
use crate::state::AppState;

const NETWORK_KEY: &str = "perm.network";
const SYSTEM_AUDIO_KEY: &str = "perm.system_audio";

/// A switch inside Meetesh is on unless the user turned it off.
fn switch_on(st: &AppState, key: &str) -> bool {
    st.db.lock().unwrap().get_setting(key).ok().flatten().as_deref() != Some("off")
}

/// May Meetesh use the network? Off means offline mode.
pub fn network_allowed(st: &AppState) -> bool {
    switch_on(st, NETWORK_KEY)
}

/// May Meetesh record the other participants?
pub fn system_audio_allowed(st: &AppState) -> bool {
    switch_on(st, SYSTEM_AUDIO_KEY)
}

/// The refusal used wherever the network would be touched in offline mode.
pub fn require_network(st: &AppState, what: &str) -> Result<()> {
    if network_allowed(st) {
        Ok(())
    } else {
        Err(Error::Other(format!("Offline mode is on, so Meetesh cannot {what}. Turn on Internet access in Settings > Permissions.")))
    }
}

fn build(st: &AppState) -> Vec<Permission> {
    let has_key = crate::commands::online::load_key().ok().flatten().is_some();
    permissions::list(&Inputs {
        platform: os_permissions::platform(),
        microphone: os_permissions::microphone(),
        screen_audio: os_permissions::screen_audio(),
        system_audio_on: system_audio_allowed(st),
        network_on: network_allowed(st),
        has_key,
        data_dir: st.data_dir.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn list_permissions(state: State<AppState>) -> Vec<Permission> {
    build(&state)
}

/// Ask the system to show its own prompt (macOS only). The window re-reads the list when it regains focus.
#[tauri::command]
pub fn request_permission(id: String) -> Result<()> {
    os_permissions::request(&id).map_err(Error::Other)
}

#[tauri::command]
pub fn open_system_settings(id: String) -> Result<()> {
    os_permissions::open_settings(&id).map_err(Error::Other)
}

/// Turn one of Meetesh's own switches on or off ("network" or "system_audio").
#[tauri::command]
pub fn set_permission_switch(state: State<AppState>, id: String, on: bool) -> Result<Vec<Permission>> {
    let key = match id.as_str() {
        "network" => NETWORK_KEY,
        "system_audio" => SYSTEM_AUDIO_KEY,
        _ => return Err(Error::Other("That permission has no switch inside Meetesh".into())),
    };
    state.db.lock().unwrap().set_setting(key, if on { "on" } else { "off" })?;
    log::info!("permission switch {id} set to {}", if on { "on" } else { "off" });
    Ok(build(&state))
}
