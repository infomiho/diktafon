use anyhow::{Result, bail};
use std::ffi::{CStr, CString, c_char, c_int};

const MAX_CONTEXT_BUDGET: usize = 4_000;

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
        prompt: *const c_char,
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
    /// The stub bridge: this binary was built without the Foundation
    /// Models framework (Command Line Tools instead of full Xcode).
    NotInThisBuild,
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
            -5 => Self::NotInThisBuild,
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
            Self::NotInThisBuild => "Not included in this build",
            Self::Unavailable => "Unavailable",
        }
    }
}

pub fn availability() -> Availability {
    Availability::from_code(unsafe { apple_intelligence_availability() })
}

fn instructions() -> &'static str {
    "Clean speech-to-text transcripts for direct insertion into another application. \
     Preserve meaning, entities, numbers, and every intended sentence. Remove filler and \
     false starts only when the speaker clearly corrected them. Return only the cleaned \
     transcript, with no explanation, label, or quotes."
}

fn request(transcript: &str, user_prompt: &str) -> String {
    let user_prompt = user_prompt.trim();
    if user_prompt.is_empty() {
        format!("Transcript:\n{transcript}")
    } else {
        format!("{user_prompt}\n\nTranscript:\n{transcript}")
    }
}

fn checked_request(transcript: &str, user_prompt: &str, response_tokens: usize) -> Result<String> {
    let request = request(transcript, user_prompt);
    if instructions().len() + request.len() + response_tokens > MAX_CONTEXT_BUDGET {
        bail!("Apple Intelligence request is too long");
    }
    Ok(request)
}

pub fn polish(transcript: &str, user_prompt: &str) -> Result<String> {
    let state = availability();
    if state != Availability::Available {
        bail!("Apple Intelligence is unavailable: {state:?}");
    }
    let instructions = CString::new(instructions())?;
    let word_count = transcript
        .as_bytes()
        .split(|byte| byte.is_ascii_whitespace())
        .count();
    let max_tokens = (word_count.saturating_mul(2) + 64).min(2_048) as c_int;
    let request = CString::new(checked_request(
        transcript,
        user_prompt,
        max_tokens as usize,
    )?)?;
    let response =
        unsafe { apple_intelligence_polish(instructions.as_ptr(), request.as_ptr(), max_tokens) };
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
        assert_eq!(Availability::from_code(-5), Availability::NotInThisBuild);
    }

    #[test]
    fn instructions_constrain_output_and_preserve_intent() {
        let prompt = instructions();
        assert!(prompt.contains("Preserve meaning, entities, numbers"));
        assert!(prompt.contains("Return only the cleaned transcript"));
    }

    #[test]
    fn request_labels_the_transcript() {
        assert_eq!(
            request("Hello world", "Use a formal tone."),
            "Use a formal tone.\n\nTranscript:\nHello world"
        );
        assert_eq!(request("Hello world", ""), "Transcript:\nHello world");
    }

    #[test]
    fn oversized_request_is_rejected_before_inference() {
        let error = checked_request("Hello", &"x".repeat(MAX_CONTEXT_BUDGET), 64)
            .unwrap_err()
            .to_string();
        assert!(error.contains("too long"));
    }

    #[test]
    #[ignore = "requires Apple Intelligence"]
    fn eligible_system_model_polishes_text() {
        assert_eq!(availability(), Availability::Available);
        let output = polish("hello comma world", diktafon_protocol::DEFAULT_APPLE_PROMPT).unwrap();
        assert!(output.to_lowercase().contains("hello"), "{output}");
    }
}
