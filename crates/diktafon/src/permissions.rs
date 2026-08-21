//! First-run permission guidance, checked at launch so nothing fails
//! mid-first-dictation: the microphone prompt would otherwise fire during
//! (and eat) the first session, and a missing Accessibility grant makes the
//! synthesized Cmd+V silently do nothing.

use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

pub fn check_at_launch() {
    prewarm_microphone();
    check_accessibility();
}

/// Ask for microphone access now if it was never decided; the prompt then
/// appears at launch instead of eating the first dictation.
fn prewarm_microphone() {
    let media = unsafe { AVMediaTypeAudio }.expect("AVMediaTypeAudio missing");
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    match status {
        AVAuthorizationStatus::NotDetermined => {
            let handler = block2::RcBlock::new(|granted: objc2::runtime::Bool| {
                if !granted.as_bool() {
                    eprintln!("microphone access denied; dictation cannot hear you");
                }
            });
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler)
            };
        }
        AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => {
            eprintln!(
                "microphone access is denied; enable it in System Settings > Privacy & Security > Microphone"
            );
            open_privacy_pane("Privacy_Microphone");
        }
        _ => {}
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    // Returns a MacTypes Boolean (unsigned char), so u8 rather than bool.
    fn AXIsProcessTrusted() -> u8;
}

/// The paste keystroke needs Accessibility; surface that at launch instead of
/// letting the first paste silently do nothing.
fn check_accessibility() {
    if unsafe { AXIsProcessTrusted() } != 0 {
        return;
    }
    eprintln!(
        "Accessibility permission missing; grant it in System Settings > Privacy & Security > Accessibility so dictated text can be pasted"
    );
    open_privacy_pane("Privacy_Accessibility");
}

/// Only from the app bundle: that is where first-run happens, and terminal
/// dev runs should not have System Settings popping up on every start.
fn open_privacy_pane(pane: &str) {
    let in_bundle = std::env::current_exe()
        .is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS"));
    if !in_bundle {
        return;
    }
    let _ = std::process::Command::new("open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{pane}"
        ))
        .status();
}
