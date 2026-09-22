//! Muting playback while a dictation records, so music or call audio does
//! not fight the microphone. The default output device is silenced when the
//! session goes live and restored exactly when it ends.
//!
//! The device's master mute flag is flipped, so an output the user had
//! already muted stays muted: only a flag we turned on is turned back off.
//! A device that exposes no software mute (some USB interfaces and HDMI
//! outputs) is left alone and reported, rather than pretending it was
//! silenced.

use core::ptr::{self, NonNull};

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertyElement, AudioObjectPropertyScope, AudioObjectPropertySelector,
    AudioObjectSetPropertyData, kAudioDevicePropertyMute,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    kAudioObjectUnknown,
};

/// The muted output of one dictation. Dropping it restores the master mute
/// flag to what it was; the session owns it, so every end path lets go of
/// playback with the session.
pub struct Muted {
    device: AudioObjectID,
}

impl Muted {
    /// Silence the default output device, or `None` when playback was left
    /// alone: no default device, already muted, or a device exposing no
    /// software mute. `None` also means the caller's own start cue stays
    /// audible.
    pub fn engage() -> Option<Self> {
        let device = default_output_device()?;
        if is_master_muted(device)? {
            eprintln!("muting playback: the default output is already muted");
            return None;
        }
        if !set_master_muted(device, true) {
            eprintln!("muting playback: the default output refuses the mute flag");
            return None;
        }
        Some(Self { device })
    }
}

impl Drop for Muted {
    fn drop(&mut self) {
        set_master_muted(self.device, false);
    }
}

/// Whether the device's master mute flag is on, or `None` when it has none.
fn is_master_muted(device: AudioObjectID) -> Option<bool> {
    property_u32(device, kAudioDevicePropertyMute).map(|muted| muted != 0)
}

fn set_master_muted(device: AudioObjectID, muted: bool) -> bool {
    set_property_u32(device, kAudioDevicePropertyMute, muted.into())
}

fn default_output_device() -> Option<AudioObjectID> {
    let mut device = 0u32;
    let mut size = size_of::<AudioObjectID>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&address(
                kAudioHardwarePropertyDefaultOutputDevice,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            )),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device).cast(),
        )
    };
    (status == 0 && device != kAudioObjectUnknown).then_some(device)
}

fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    element: AudioObjectPropertyElement,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: element,
    }
}

/// The master mute flag's value, or `None` when the device has no such
/// property.
fn property_u32(device: AudioObjectID, selector: AudioObjectPropertySelector) -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address(
                selector,
                kAudioObjectPropertyScopeOutput,
                kAudioObjectPropertyElementMain,
            )),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    (status == 0).then_some(value)
}

fn set_property_u32(
    device: AudioObjectID,
    selector: AudioObjectPropertySelector,
    value: u32,
) -> bool {
    let mut value = value;
    let status = unsafe {
        AudioObjectSetPropertyData(
            device,
            NonNull::from(&address(
                selector,
                kAudioObjectPropertyScopeOutput,
                kAudioObjectPropertyElementMain,
            )),
            0,
            ptr::null(),
            size_of::<u32>() as u32,
            NonNull::from(&mut value).cast(),
        )
    };
    status == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "touches the real default output; run manually with --ignored"]
    fn engaging_mutes_playback_and_dropping_restores_it() {
        let Some(device) = default_output_device() else {
            eprintln!("no default output device; nothing to verify");
            return;
        };
        let Some(before) = is_master_muted(device) else {
            eprintln!("default output has no master mute; skipping state assertions");
            return;
        };
        if before {
            eprintln!("default output is already muted; nothing to verify");
            return;
        }
        let Some(muted) = Muted::engage() else {
            panic!("engage() declined a mutable output");
        };
        assert!(
            is_master_muted(device).is_some_and(|muted| muted),
            "output is still audible after engage()"
        );
        drop(muted);
        assert_eq!(
            is_master_muted(device),
            Some(false),
            "master mute was not restored"
        );
    }
}
