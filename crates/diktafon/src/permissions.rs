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

/// `activate` brings the app forward before a prompt is raised: diktafon
/// launches as a background agent with no window, and a TCC prompt from one
/// can open behind whatever the user is looking at, which reads as no
/// prompt at all.
pub fn check_at_launch(activate: impl FnOnce()) {
    let microphone = microphone();
    if microphone != MicrophoneAccess::Granted || !accessibility_granted() {
        activate();
    }
    match microphone {
        MicrophoneAccess::NotAsked => request_microphone(),
        MicrophoneAccess::Denied => {
            eprintln!(
                "microphone access is denied; enable it in System Settings > Privacy & Security > Microphone"
            );
            open_privacy_pane_from_bundle(PrivacyPane::Microphone);
        }
        MicrophoneAccess::Granted => {}
    }
    if !accessibility_granted() {
        eprintln!(
            "Accessibility permission missing; grant it in System Settings > Privacy & Security > Accessibility so dictated text can be pasted"
        );
        open_privacy_pane_from_bundle(PrivacyPane::Accessibility);
    }
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

/// Only from the app bundle: that is where first-run happens, and terminal
/// dev runs should not have System Settings popping up on every start.
fn open_privacy_pane_from_bundle(pane: PrivacyPane) {
    let in_bundle = std::env::current_exe()
        .is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS"));
    if in_bundle {
        open_privacy_pane(pane);
    }
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
