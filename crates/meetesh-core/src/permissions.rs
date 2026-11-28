//! What Meetesh needs from the computer and from the user, feature by feature.
//!
//! This is the single list the Settings screen shows. It answers, for each thing: which feature needs
//! it, whether it is required or optional, what its state is right now, and what the user can do about
//! it. Two kinds of control exist, and the difference matters:
//!
//! - **System permissions** (microphone, screen and system audio recording on macOS). An app can ask for
//!   these and can read their state, but only the operating system can take them back. The screen
//!   therefore offers "Allow" (ask now) and "Open system settings" (where it is revoked), never a fake
//!   "revoke" button.
//! - **Switches inside Meetesh** (use the network, record the other participants). Turning one off makes
//!   Meetesh itself stop doing that, whatever the system allows.
//!
//! The operating-system probing lives in the app shell; this module only turns what it found into the
//! list, so the wording and the rules are tested without any operating system.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

/// What the operating system reports about a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OsState {
    Granted,
    Denied,
    /// The user has not been asked yet.
    NotAsked,
    /// Not granted: either never asked or refused (macOS does not say which for screen recording).
    NotGranted,
    /// This system gives no way to check.
    Unknown,
}

/// How a row is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Working: allowed (or nothing is needed) and in use when the feature runs.
    Granted,
    /// Blocked by the system.
    Denied,
    /// Not allowed yet.
    NotGranted,
    /// Turned off inside Meetesh.
    Off,
    /// The system cannot tell us.
    Unknown,
    /// This system has no permission for it.
    NotNeeded,
    /// Cannot be done on this system.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Need {
    Required,
    Optional,
}

/// One row of the permissions list.
#[derive(Debug, Clone, Serialize)]
pub struct Permission {
    pub id: &'static str,
    pub name: &'static str,
    /// What Meetesh does with it, in the user's words.
    pub feature: &'static str,
    pub need: Need,
    pub status: Status,
    /// One line on the current state.
    pub detail: String,
    /// Meetesh can ask the system for it now.
    pub can_request: bool,
    /// The system's own settings page for it can be opened from here (that is where it is revoked).
    pub opens_settings: bool,
    /// A switch inside Meetesh: `Some(on)`. `None` means there is none.
    pub switch: Option<bool>,
    /// What the user loses with it off or missing.
    pub without: &'static str,
}

/// Everything the shell found out.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub platform: Platform,
    pub microphone: OsState,
    /// macOS "Screen & System Audio Recording".
    pub screen_audio: OsState,
    /// Meetesh may record the other participants (a switch in Meetesh).
    pub system_audio_on: bool,
    /// Meetesh may use the network (a switch in Meetesh). Off means offline mode.
    pub network_on: bool,
    /// An API key for online notes is stored in the keychain.
    pub has_key: bool,
    /// Where recordings and transcripts live.
    pub data_dir: String,
}

fn os_status(s: OsState) -> Status {
    match s {
        OsState::Granted => Status::Granted,
        OsState::Denied => Status::Denied,
        OsState::NotAsked | OsState::NotGranted => Status::NotGranted,
        OsState::Unknown => Status::Unknown,
    }
}

pub fn list(i: &Inputs) -> Vec<Permission> {
    let mac = i.platform == Platform::Mac;
    let win = i.platform == Platform::Windows;

    let microphone = Permission {
        id: "microphone",
        name: "Microphone",
        feature: "Recording your voice and live transcription",
        need: Need::Required,
        status: os_status(i.microphone),
        detail: match (i.microphone, i.platform) {
            (OsState::Granted, _) => "Allowed.".into(),
            (OsState::Denied, Platform::Mac) => "Blocked. Allow Meetesh under System Settings > Privacy & Security > Microphone.".into(),
            (OsState::Denied, Platform::Windows) => "Blocked. Turn on microphone access under Settings > Privacy & security > Microphone.".into(),
            (OsState::Denied, _) => "Blocked by the system.".into(),
            (OsState::NotAsked | OsState::NotGranted, _) => "Not allowed yet. Meetesh asks the first time you record, or you can allow it now.".into(),
            (OsState::Unknown, _) => "This system does not say. Meetesh will tell you if recording is blocked.".into(),
        },
        can_request: mac && matches!(i.microphone, OsState::NotAsked | OsState::NotGranted),
        opens_settings: mac || win,
        switch: None,
        without: "Nothing can be recorded. Meetings you already have are still readable.",
    };

    let system_audio = Permission {
        id: "system_audio",
        name: "Other participants' audio",
        feature: "Hearing the other side of a call, so the transcript can say You and Others",
        need: Need::Optional,
        status: if i.platform == Platform::Linux {
            Status::Unsupported
        } else if !i.system_audio_on {
            Status::Off
        } else if mac {
            os_status(i.screen_audio)
        } else {
            Status::NotNeeded
        },
        detail: match i.platform {
            Platform::Linux => "Not available on Linux yet. Only your microphone is recorded.".into(),
            _ if !i.system_audio_on => "Turned off in Meetesh: the other participants are never recorded.".into(),
            Platform::Mac => match i.screen_audio {
                OsState::Granted => "Allowed. macOS calls this \"Screen & System Audio Recording\"; Meetesh keeps only the sound and throws the picture away.".into(),
                _ => "Not allowed yet. macOS calls this \"Screen & System Audio Recording\"; Meetesh keeps only the sound and throws the picture away.".into(),
            },
            Platform::Windows => "Windows needs no permission for this. It records what your speakers play.".into(),
        },
        can_request: mac && i.system_audio_on && i.screen_audio != OsState::Granted,
        opens_settings: mac,
        // Only where it can work at all.
        switch: (i.platform != Platform::Linux).then_some(i.system_audio_on),
        without: "Only your microphone is recorded, so everything is labelled as you.",
    };

    let network = Permission {
        id: "network",
        name: "Internet access",
        feature: "Downloading speech and language models, and (only if you turn it on) writing notes with an online model",
        need: Need::Optional,
        status: if i.network_on { Status::Granted } else { Status::Off },
        detail: if i.network_on {
            "Allowed. Used only when you download a model or use online notes. Recording, transcripts and hints work without it.".into()
        } else {
            "Offline mode: Meetesh makes no network requests at all.".into()
        },
        can_request: false,
        opens_settings: false,
        switch: Some(i.network_on),
        without: "Models cannot be downloaded (ones you already have keep working) and online notes cannot be used.",
    };

    let keychain = Permission {
        id: "keychain",
        name: "Keychain",
        feature: "Keeping the API key for online notes safe (only if you use online notes)",
        need: Need::Optional,
        status: if i.has_key { Status::Granted } else { Status::NotNeeded },
        detail: if i.has_key {
            "An API key is stored in this computer's keychain. Remove it to take it back.".into()
        } else {
            "Not used: no API key is stored.".into()
        },
        can_request: false,
        opens_settings: false,
        switch: None,
        without: "Online notes cannot be used. Everything else is unaffected.",
    };

    let storage = Permission {
        id: "storage",
        name: "Files",
        feature: "Keeping your recordings, transcripts and notes",
        need: Need::Required,
        status: Status::NotNeeded,
        detail: format!("Stored on this computer in {}. No permission is needed for Meetesh's own folder.", i.data_dir),
        can_request: false,
        opens_settings: false,
        switch: None,
        without: "Nothing could be saved.",
    };

    vec![microphone, system_audio, network, keychain, storage]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(platform: Platform) -> Inputs {
        Inputs {
            platform,
            microphone: OsState::Granted,
            screen_audio: OsState::Granted,
            system_audio_on: true,
            network_on: true,
            has_key: false,
            data_dir: "/data".into(),
        }
    }

    fn get(i: &Inputs, id: &str) -> Permission {
        list(i).into_iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn only_the_microphone_and_the_files_are_required() {
        let l = list(&base(Platform::Mac));
        let required: Vec<_> = l.iter().filter(|p| p.need == Need::Required).map(|p| p.id).collect();
        assert_eq!(required, ["microphone", "storage"]);
        assert!(l.iter().filter(|p| p.need == Need::Optional).all(|p| matches!(p.id, "system_audio" | "network" | "keychain")));
    }

    #[test]
    fn every_row_names_its_feature_and_what_is_lost_without_it() {
        for p in list(&base(Platform::Windows)) {
            assert!(!p.feature.is_empty() && !p.without.is_empty() && !p.detail.is_empty(), "{}", p.id);
        }
    }

    #[test]
    fn a_system_permission_is_asked_for_or_opened_never_revoked_from_here() {
        // the microphone: no switch, settings link where the system has one
        let m = get(&base(Platform::Mac), "microphone");
        assert!(m.switch.is_none() && m.opens_settings);
        assert!(get(&base(Platform::Windows), "microphone").opens_settings);
        assert!(!get(&base(Platform::Linux), "microphone").opens_settings, "Linux has no page to open");
    }

    #[test]
    fn microphone_states_read_clearly() {
        let mut i = base(Platform::Mac);
        assert_eq!(get(&i, "microphone").status, Status::Granted);
        assert!(!get(&i, "microphone").can_request);
        i.microphone = OsState::NotAsked;
        let m = get(&i, "microphone");
        assert_eq!(m.status, Status::NotGranted);
        assert!(m.can_request, "macOS can be asked");
        i.microphone = OsState::Denied;
        let m = get(&i, "microphone");
        assert_eq!(m.status, Status::Denied);
        assert!(!m.can_request, "a refusal can only be changed in System Settings");
        assert!(m.detail.contains("System Settings"), "{}", m.detail);
        // Windows cannot be asked from the app, only pointed at the page
        let mut w = base(Platform::Windows);
        w.microphone = OsState::Denied;
        assert!(!get(&w, "microphone").can_request && get(&w, "microphone").detail.contains("Settings > Privacy"));
        // Linux: we cannot tell
        let mut l = base(Platform::Linux);
        l.microphone = OsState::Unknown;
        assert_eq!(get(&l, "microphone").status, Status::Unknown);
    }

    #[test]
    fn other_participants_audio_depends_on_the_system_and_on_the_switch() {
        // Windows: no system permission, only the switch
        let w = base(Platform::Windows);
        let p = get(&w, "system_audio");
        assert_eq!((p.status, p.switch, p.can_request, p.opens_settings), (Status::NotNeeded, Some(true), false, false));
        let mut off = w.clone();
        off.system_audio_on = false;
        assert_eq!(get(&off, "system_audio").status, Status::Off);
        assert_eq!(get(&off, "system_audio").switch, Some(false));
        // macOS: the system permission decides while the switch is on
        let mut m = base(Platform::Mac);
        m.screen_audio = OsState::NotGranted;
        let p = get(&m, "system_audio");
        assert_eq!(p.status, Status::NotGranted);
        assert!(p.can_request && p.opens_settings);
        assert!(p.detail.contains("Screen & System Audio Recording") && p.detail.contains("throws the picture away"), "{}", p.detail);
        m.screen_audio = OsState::Granted;
        assert!(!get(&m, "system_audio").can_request);
        // switched off in Meetesh: never asks the system, whatever it allows
        m.system_audio_on = false;
        let p = get(&m, "system_audio");
        assert_eq!(p.status, Status::Off);
        assert!(!p.can_request);
        // Linux: unsupported, and no switch to offer
        let l = get(&base(Platform::Linux), "system_audio");
        assert_eq!((l.status, l.switch), (Status::Unsupported, None));
    }

    #[test]
    fn offline_mode_is_a_switch_and_says_so() {
        let mut i = base(Platform::Mac);
        let n = get(&i, "network");
        assert_eq!((n.status, n.switch), (Status::Granted, Some(true)));
        assert!(n.detail.contains("work without it"));
        i.network_on = false;
        let n = get(&i, "network");
        assert_eq!((n.status, n.switch), (Status::Off, Some(false)));
        assert!(n.detail.contains("no network requests"), "{}", n.detail);
    }

    #[test]
    fn the_keychain_row_follows_the_stored_key() {
        let mut i = base(Platform::Windows);
        assert_eq!(get(&i, "keychain").status, Status::NotNeeded);
        i.has_key = true;
        let k = get(&i, "keychain");
        assert_eq!(k.status, Status::Granted);
        assert!(k.detail.contains("Remove it"));
        assert!(k.switch.is_none() && !k.can_request, "removed with the key button, not a switch");
    }

    #[test]
    fn the_files_row_shows_where_and_needs_nothing() {
        let f = get(&base(Platform::Mac), "storage");
        assert_eq!(f.status, Status::NotNeeded);
        assert!(f.detail.contains("/data"));
    }
}
