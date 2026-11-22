//! The optional online model for meeting notes and titles (see `meetesh_core::online`).
//!
//! Nothing here runs unless the user turned it on. The API key is kept in the operating system's
//! keychain: it is never stored in the database or a file, never sent to the window after it is saved
//! (the window only learns that a key exists), and never logged.

use meetesh_core::online::{presets, Preset, RemoteAssistant, Settings};
use meetesh_core::{Error, Result};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

const SETTING_KEY: &str = "online.notes";
const KEYCHAIN_SERVICE: &str = "app.meetesh";
const KEYCHAIN_ACCOUNT: &str = "online-notes-api-key";

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|e| Error::Other(format!("Could not use the system keychain: {e}")))
}

/// The saved key, if any.
pub fn load_key() -> Result<Option<String>> {
    match entry()?.get_password() {
        Ok(k) if !k.trim().is_empty() => Ok(Some(k)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::Other(format!("Could not read the API key from the system keychain: {e}"))),
    }
}

pub fn load_settings(st: &AppState) -> Settings {
    st.db.lock().unwrap().get_setting(SETTING_KEY).ok().flatten().map(|j| Settings::parse(&j)).unwrap_or_default()
}

/// The online model to write notes with, when the user turned it on and it is complete; otherwise
/// `None` and the on-device model is used. A broken setting never falls back silently to sending
/// text anywhere: it simply means "not online".
pub fn active(st: &AppState) -> Result<Option<(RemoteAssistant, Settings)>> {
    let s = load_settings(st);
    if !s.enabled {
        return Ok(None);
    }
    let provider = s.provider()?;
    let Some(key) = load_key()? else {
        return Err(Error::Other("Online notes are on but there is no API key. Add it in Settings, or turn online notes off.".into()));
    };
    Ok(Some((RemoteAssistant::new(provider, key)?, s)))
}

#[derive(Serialize)]
pub struct OnlineStatus {
    #[serde(flatten)]
    settings: Settings,
    has_key: bool,
    /// Why the key could not be checked (no keychain on this system, for example).
    keychain_error: Option<String>,
    presets: Vec<Preset>,
}

fn status(st: &AppState) -> OnlineStatus {
    let (has_key, keychain_error) = match load_key() {
        Ok(k) => (k.is_some(), None),
        Err(e) => (false, Some(e.to_string())),
    };
    OnlineStatus { settings: load_settings(st), has_key, keychain_error, presets: presets() }
}

#[tauri::command]
pub fn online_status(state: State<AppState>) -> OnlineStatus {
    status(&state)
}

/// Save the choices. Turning online notes on needs `consent`: the user has been told that the text of
/// their meetings is sent to the provider they chose.
#[tauri::command]
pub fn save_online(state: State<AppState>, settings: Settings, consent: bool) -> Result<OnlineStatus> {
    if settings.enabled {
        settings.provider()?; // address, header and model must be valid before it can be switched on
        if !consent {
            return Err(Error::Other("Tick the box to confirm that your meeting text will be sent to this provider".into()));
        }
        if load_key()?.is_none() {
            return Err(Error::Other("Add the API key first".into()));
        }
    }
    state.db.lock().unwrap().set_setting(SETTING_KEY, &settings.to_json())?;
    log::info!("online notes settings saved: enabled={} provider={}", settings.enabled, settings.preset);
    Ok(status(&state))
}

#[tauri::command]
pub fn set_online_key(state: State<AppState>, key: String) -> Result<OnlineStatus> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("Enter an API key".into()));
    }
    entry()?.set_password(key).map_err(|e| Error::Other(format!("Could not save the API key in the system keychain: {e}")))?;
    log::info!("online notes API key saved to the system keychain");
    Ok(status(&state))
}

/// Forget the key. Online notes are switched off with it.
#[tauri::command]
pub fn clear_online_key(state: State<AppState>) -> Result<OnlineStatus> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(Error::Other(format!("Could not remove the API key from the system keychain: {e}"))),
    }
    let mut s = load_settings(&state);
    s.enabled = false;
    state.db.lock().unwrap().set_setting(SETTING_KEY, &s.to_json())?;
    log::info!("online notes API key removed");
    Ok(status(&state))
}

/// Build the client for the settings the window shows right now (which may not be saved yet).
fn client_for(settings: &Settings) -> Result<RemoteAssistant> {
    let Some(key) = load_key()? else { return Err(Error::Other("Add the API key first".into())) };
    RemoteAssistant::new(settings.provider()?, key)
}

/// A tiny test request that contains no meeting text.
#[tauri::command]
pub async fn test_online(settings: Settings) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || client_for(&settings)?.check())
        .await
        .map_err(|e| Error::Other(format!("The test stopped: {e}")))?
}

/// The models the provider offers (OpenRouter: only the free ones).
#[tauri::command]
pub async fn list_online_models(mut settings: Settings) -> Result<Vec<String>> {
    // Only the address and the key matter for listing; any model name will do.
    if settings.model.trim().is_empty() {
        settings.model = "list".into();
    }
    let free_only = settings.preset().is_some_and(|p| p.free_only);
    tauri::async_runtime::spawn_blocking(move || client_for(&settings)?.list_models(free_only))
        .await
        .map_err(|e| Error::Other(format!("The request stopped: {e}")))?
}
