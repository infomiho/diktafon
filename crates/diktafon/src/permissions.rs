//! The two macOS grants dictation needs, checked at launch so nothing fails
//! mid-first-dictation, and readable at any time for the Permissions pane:
//! the microphone prompt would otherwise fire during (and eat) the first
//! session, and a missing Accessibility grant makes the synthesized Cmd+V
//! silently do nothing.

use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use std::ffi::c_void;

/// The System Settings privacy panes, addressable by URL.
#[derive(Clone, Copy)]
pub enum PrivacyPane {
    Microphone,
    Accessibility,
}

impl PrivacyPane {
    fn anchor(self) -> &'static str {
        match self {
            PrivacyPane::Microphone => "Privacy_Microphone",
            PrivacyPane::Accessibility => "Privacy_Accessibility",
        }
    }
}

/// macOS's answer for the microphone. `NotAsked` means the system prompt has
/// never been shown (or was reset), so asking is what fixes it; `Denied`
/// needs the user to flip the switch in System Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicrophoneAccess {
    Granted,
    NotAsked,
    Denied,
}

/// Both grants at one moment, for the Permissions sheet and its summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub microphone: MicrophoneAccess,
    pub accessibility: bool,
}

impl Status {
    pub fn read() -> Self {
        Self {
            microphone: microphone(),
            accessibility: accessibility_granted(),
        }
    }

    /// One line for the Advanced row. Only what is missing is named, so a
    /// healthy install reads as one calm sentence.
    pub fn summary(&self) -> String {
        let mut missing = Vec::new();
        match self.microphone {
            MicrophoneAccess::Granted => {}
            MicrophoneAccess::NotAsked => missing.push("Microphone not requested"),
            MicrophoneAccess::Denied => missing.push("Microphone denied"),
        }
        if !self.accessibility {
            missing.push("Accessibility not granted");
        }
        if missing.is_empty() {
            return "Microphone and Accessibility granted.".into();
        }
        format!("{}.", missing.join(". "))
    }
}

/// Raises the system microphone prompt the first time, and otherwise only
/// reports. Launching must never open System Settings by itself: that threw
/// the user into a window they never asked for, on every launch. The
/// Permissions UI owns everything past the first ask.
pub fn check_at_launch() -> Status {
    let status = Status::read();
    if status.microphone == MicrophoneAccess::NotAsked {
        request_microphone();
    } else if status.microphone == MicrophoneAccess::Denied {
        eprintln!("microphone access is denied; grant it in Settings > Advanced > Permissions");
    }
    if !status.accessibility {
        eprintln!("Accessibility permission missing; dictated text cannot be pasted");
    }
    status
}

pub fn microphone() -> MicrophoneAccess {
    let media = unsafe { AVMediaTypeAudio }.expect("AVMediaTypeAudio missing");
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) } {
        AVAuthorizationStatus::Authorized => MicrophoneAccess::Granted,
        AVAuthorizationStatus::NotDetermined => MicrophoneAccess::NotAsked,
        _ => MicrophoneAccess::Denied,
    }
}

/// Show the system microphone prompt. macOS shows it only while the status
/// is not determined; afterwards the call just returns the stored answer,
/// which is why a denied microphone gets the System Settings route instead.
///
/// Deliberately no `tccutil reset` first: the reset only takes effect for a
/// newly launched process, so resetting and asking in the same breath would
/// leave the user with no prompt and a cleared grant.
pub fn request_microphone() {
    let media = unsafe { AVMediaTypeAudio }.expect("AVMediaTypeAudio missing");
    let handler = block2::RcBlock::new(|granted: objc2::runtime::Bool| {
        if !granted.as_bool() {
            eprintln!("microphone access denied; dictation cannot hear you");
        }
    });
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler) };
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    // Returns a MacTypes Boolean (unsigned char), so u8 rather than bool.
    fn AXIsProcessTrusted() -> u8;
}

pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    /// The framework's own key; toll-free bridged to `NSString`.
    static kAXTrustedCheckOptionPrompt: &'static NSString;
}

/// Show the system Accessibility dialog, which also lists the app in the
/// Accessibility pane so the switch is there to flip. macOS raises that
/// dialog only for an app with no entry yet, so the pane is opened too:
/// an app already listed and switched off would otherwise get nothing.
pub fn request_accessibility() {
    prompt_accessibility();
    open_privacy_pane(PrivacyPane::Accessibility);
}

fn prompt_accessibility() {
    let options = unsafe {
        NSDictionary::from_slices(
            &[kAXTrustedCheckOptionPrompt],
            &[&*NSNumber::new_bool(true)],
        )
    };
    unsafe { AXIsProcessTrustedWithOptions(&*options as *const _ as *const c_void) };
}

/// Open the pane where the user can grant the permission.
pub fn open_privacy_pane(pane: PrivacyPane) {
    // Spawned, not waited on: launching System Settings cold takes far
    // longer than a frame, and this runs from a click handler.
    let _ = std::process::Command::new("open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{}",
            pane.anchor()
        ))
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_names_what_is_missing() {
        let all = Status {
            microphone: MicrophoneAccess::Granted,
            accessibility: true,
        };
        assert_eq!(all.summary(), "Microphone and Accessibility granted.");
        let stuck = Status {
            microphone: MicrophoneAccess::NotAsked,
            accessibility: false,
        };
        assert_eq!(
            stuck.summary(),
            "Microphone not requested. Accessibility not granted."
        );
        let denied = Status {
            microphone: MicrophoneAccess::Denied,
            accessibility: true,
        };
        // Only the problem is named; a granted permission stays quiet.
        assert_eq!(denied.summary(), "Microphone denied.");
    }
}
