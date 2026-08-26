use anyhow::{Result, bail};
use std::ffi::{CStr, CString, c_char, c_int};

const MAX_INPUT_CHARS: usize = 12_000;

#[repr(C)]
struct AppleIntelligenceResponse {
    response: *mut c_char,
    success: c_int,
    error_message: *mut c_char,
}

unsafe extern "C" {
    fn apple_intelligence_availability() -> c_int;
    fn apple_intelligence_polish(
        instructions: *const c_char,
        transcript: *const c_char,
        max_response_tokens: c_int,
    ) -> *mut AppleIntelligenceResponse;
    fn apple_intelligence_response_free(response: *mut AppleIntelligenceResponse);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    UnsupportedBuildOrOs,
    DeviceNotEligible,
    NotEnabled,
    ModelNotReady,
    Unavailable,
}

impl Availability {
    fn from_code(code: c_int) -> Self {
        match code {
            1 => Self::Available,
            -1 => Self::UnsupportedBuildOrOs,
            -2 => Self::DeviceNotEligible,
            -3 => Self::NotEnabled,
            -4 => Self::ModelNotReady,
            _ => Self::Unavailable,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::UnsupportedBuildOrOs => "Requires macOS 26 and a supported build",
            Self::DeviceNotEligible => "This device is not eligible",
            Self::NotEnabled => "Apple Intelligence is not enabled",
            Self::ModelNotReady => "System model is not ready",
            Self::Unavailable => "Unavailable",
        }
    }
}

pub fn availability() -> Availability {
    Availability::from_code(unsafe { apple_intelligence_availability() })
}

fn instructions(control_line: &str) -> String {
    format!(
        "Clean speech-to-text transcripts for direct insertion into another application. \
         Preserve meaning, entities, numbers, and every intended sentence. Remove filler and \
         false starts only when the speaker clearly corrected them. Apply this user preference: \
         {control_line}. Return only the cleaned transcript, with no explanation, label, or quotes."
    )
}

pub fn polish(transcript: &str, control_line: &str) -> Result<String> {
    let state = availability();
    if state != Availability::Available {
        bail!("Apple Intelligence is unavailable: {state:?}");
    }
    if transcript.chars().count() > MAX_INPUT_CHARS {
        bail!("transcript exceeds Apple Intelligence input limit");
    }
    let instructions = CString::new(instructions(control_line))?;
    let transcript = CString::new(transcript)?;
    let word_count = transcript
        .as_bytes()
        .split(|byte| byte.is_ascii_whitespace())
        .count();
    let max_tokens = (word_count.saturating_mul(2) + 64).min(2_048) as c_int;
    let response = unsafe {
        apple_intelligence_polish(instructions.as_ptr(), transcript.as_ptr(), max_tokens)
    };
    if response.is_null() {
        bail!("Apple Intelligence returned a null response");
    }
    let result = unsafe {
        let response_ref = &*response;
        if response_ref.success == 1 && !response_ref.response.is_null() {
            let output = CStr::from_ptr(response_ref.response)
                .to_string_lossy()
                .trim()
                .to_string();
            if output.is_empty() {
                Err(anyhow::anyhow!("Apple Intelligence returned empty output"))
            } else {
                Ok(output)
            }
        } else if !response_ref.error_message.is_null() {
            Err(anyhow::anyhow!(
                "{}",
                CStr::from_ptr(response_ref.error_message).to_string_lossy()
            ))
        } else {
            Err(anyhow::anyhow!(
                "Apple Intelligence failed without a reason"
            ))
        }
    };
    unsafe { apple_intelligence_response_free(response) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_codes_are_specific() {
        assert_eq!(Availability::from_code(1), Availability::Available);
        assert_eq!(Availability::from_code(-2), Availability::DeviceNotEligible);
        assert_eq!(Availability::from_code(-3), Availability::NotEnabled);
        assert_eq!(Availability::from_code(-4), Availability::ModelNotReady);
    }

    #[test]
    fn instructions_constrain_output_and_preserve_intent() {
        let prompt = instructions("[Styling: formal]");
        assert!(prompt.contains("Preserve meaning, entities, numbers"));
        assert!(prompt.contains("Return only the cleaned transcript"));
        assert!(prompt.contains("[Styling: formal]"));
    }

    #[test]
    #[ignore = "requires Apple Intelligence"]
    fn eligible_system_model_polishes_text() {
        assert_eq!(availability(), Availability::Available);
        let output = polish("hello comma world", "[Styling: semi-formal]").unwrap();
        assert!(!output.is_empty());
    }
}
