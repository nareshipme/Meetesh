//! Asking the operating system about permissions, and asking it to prompt.
//!
//! Only reading a state, showing the system's own prompt and opening the system's own settings page
//! are done here. Nothing is granted or revoked by Meetesh itself: that is the operating system's job.
//! macOS code is compile-checked in CI and could not be run where this was written.

use meetesh_core::permissions::{OsState, Platform};

pub fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Mac
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::OsState;
    use block2::RcBlock;
    use objc2::class;
    use objc2::msg_send;
    use objc2::runtime::Bool;
    use objc2_foundation::NSString;

    // The classes below live in these frameworks; linking them makes sure the classes are there.
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    /// `AVMediaTypeAudio`
    fn audio() -> objc2::rc::Retained<NSString> {
        NSString::from_str("soun")
    }

    pub fn microphone() -> OsState {
        // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized.
        let status: isize = unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*audio()] };
        match status {
            0 => OsState::NotAsked,
            1 | 2 => OsState::Denied,
            3 => OsState::Granted,
            _ => OsState::Unknown,
        }
    }

    /// Shows the system prompt if the user has not been asked yet. The answer arrives later; the
    /// window re-reads the state when it gets focus back.
    pub fn request_microphone() {
        let done = RcBlock::new(|_granted: Bool| {});
        unsafe {
            let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: &*audio(), completionHandler: &*done];
        }
    }

    pub fn screen_audio() -> OsState {
        // macOS does not say whether it was refused or never asked.
        if unsafe { CGPreflightScreenCaptureAccess() } {
            OsState::Granted
        } else {
            OsState::NotGranted
        }
    }

    pub fn request_screen_audio() {
        unsafe {
            CGRequestScreenCaptureAccess();
        }
    }
}

pub fn microphone() -> OsState {
    #[cfg(target_os = "macos")]
    return mac::microphone();
    #[cfg(target_os = "windows")]
    return windows_microphone();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    OsState::Unknown
}

pub fn screen_audio() -> OsState {
    #[cfg(target_os = "macos")]
    return mac::screen_audio();
    #[cfg(not(target_os = "macos"))]
    OsState::Unknown
}

/// Windows keeps the "let apps use your microphone" choice in the registry. `Deny` means blocked.
#[cfg(target_os = "windows")]
fn windows_microphone() -> OsState {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone", "/v", "Value"])
        .creation_flags(NO_WINDOW)
        .output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_lowercase();
            if text.contains("deny") {
                OsState::Denied
            } else if text.contains("allow") {
                OsState::Granted
            } else {
                OsState::Unknown
            }
        }
        Err(_) => OsState::Unknown,
    }
}

/// Ask the system to show its prompt for `id`. Only macOS can be asked from an app.
pub fn request(id: &str) -> Result<(), String> {
    match id {
        #[cfg(target_os = "macos")]
        "microphone" => {
            mac::request_microphone();
            Ok(())
        }
        #[cfg(target_os = "macos")]
        "system_audio" => {
            mac::request_screen_audio();
            Ok(())
        }
        _ => Err("This system does not let an app ask for that. Use \"Open system settings\".".into()),
    }
}

/// Open the system's own page where `id` is allowed or taken back.
pub fn open_settings(id: &str) -> Result<(), String> {
    let run = |program: &str, arg: &str| -> Result<(), String> {
        let mut c = std::process::Command::new(program);
        c.arg(arg);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c.spawn().map(|_| ()).map_err(|e| format!("Could not open the system settings: {e}"))
    };
    match (platform(), id) {
        (Platform::Mac, "microphone") => run("open", "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"),
        (Platform::Mac, "system_audio") => run("open", "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"),
        (Platform::Windows, "microphone") => run("explorer.exe", "ms-settings:privacy-microphone"),
        _ => Err("There is no system settings page to open for this.".into()),
    }
}
