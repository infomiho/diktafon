//! Every user-tunable setting in one place. Still compile-time constants; a
//! config file can replace this later without touching the consumers.

use diktafon_protocol::{
    DEFAULT_APPLE_PROMPT, DEFAULT_POLISHING_MODEL, DEFAULT_TRANSCRIPTION_MODEL, ModelSelection,
    SessionConfig,
};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};

const TEMPLATE_DEFAULT_APPLE_PROMPT: &str = "Touch up the raw transcript slightly so it looks a bit more like written communication.\n\nReturn only the cleaned transcript.\n\nTranscript:\n${output}";

pub struct Config {
    /// Push-to-talk hotkey.
    pub hotkey_modifiers: Modifiers,
    pub hotkey_code: Code,
    /// ISO 639-1 language hint for the ASR model.
    pub language: &'static str,
    /// S1-mini control line selecting styling, structure, and context.
    pub control_line: &'static str,
    /// Silero speech probability threshold.
    pub speech_threshold: f32,
    /// Consecutive speech frames (30ms each) before speech onset.
    pub onset_frames: usize,
    /// Pre-onset audio kept, in frames.
    pub prefill_frames: usize,
    /// Non-speech frames before a speech segment is declared over.
    pub hangover_frames: usize,
    /// Speech segments shorter than this are merged with the next one instead
    /// of paying a per-chunk ASR roundtrip.
    pub min_chunk_secs: f32,
}

/// Handy's tuned Silero values; language and control line match the daemon's
/// own defaults.
pub const CONFIG: Config = Config {
    hotkey_modifiers: Modifiers::ALT,
    hotkey_code: Code::Space,
    language: "en",
    control_line: "[Styling: semi-formal] [Structure: prose] [Context: general]",
    speech_threshold: 0.3,
    onset_frames: 2,
    prefill_frames: 15,
    hangover_frames: 15,
    min_chunk_secs: 1.5,
};

impl Config {
    pub fn hotkey(&self) -> HotKey {
        HotKey::new(Some(self.hotkey_modifiers), self.hotkey_code)
    }
}

/// `None` unless the string is a valid chord with at least one modifier: a
/// bare key would fire on normal typing.
pub fn parse_hotkey(s: &str) -> Option<HotKey> {
    let hotkey = HotKey::try_from(s).ok()?;
    if hotkey.mods.is_empty() {
        return None;
    }
    Some(hotkey)
}

/// The user-editable subset, persisted as `config.json` in the data dir and
/// edited live from the settings window; the compile-time [`CONFIG`] provides
/// the defaults.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SessionSettings {
    pub language: String,
    pub control_line: String,
    pub apple_prompt: String,
    /// Seconds of daemon idleness before the models are unloaded; passed as
    /// DIKTAFOND_IDLE_SECS when the client spawns the daemon.
    pub idle_unload_secs: u64,
    /// Audible cues: mic live, cancel, error.
    pub sound_cues: bool,
    /// Push-to-talk chord in global-hotkey syntax, e.g. "alt+space".
    pub hotkey: String,
    pub transcription_model: String,
    pub polishing_model: String,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            language: CONFIG.language.into(),
            control_line: CONFIG.control_line.into(),
            apple_prompt: DEFAULT_APPLE_PROMPT.into(),
            idle_unload_secs: 300,
            sound_cues: true,
            hotkey: "alt+space".into(),
            transcription_model: DEFAULT_TRANSCRIPTION_MODEL.into(),
            polishing_model: DEFAULT_POLISHING_MODEL.into(),
        }
    }
}

fn settings_path() -> std::path::PathBuf {
    diktafon_protocol::data_dir().join("config.json")
}

impl SessionSettings {
    /// Missing or unparseable file falls back to the defaults. A hotkey
    /// string that does not parse is reset in place so every surface (keycaps,
    /// startup line) shows the chord that is actually registered.
    pub fn load() -> Self {
        let mut settings: Self = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        if parse_hotkey(&settings.hotkey).is_none() {
            settings.hotkey = Self::default().hotkey;
        }
        if settings.apple_prompt == TEMPLATE_DEFAULT_APPLE_PROMPT {
            settings.apple_prompt = DEFAULT_APPLE_PROMPT.into();
        }
        settings
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// The push-to-talk chord; an unparseable or modifier-less string falls
    /// back to the compile-time default.
    pub fn hotkey(&self) -> HotKey {
        parse_hotkey(&self.hotkey).unwrap_or_else(|| CONFIG.hotkey())
    }

    pub fn session(&self) -> SessionConfig {
        SessionConfig {
            language: self.language.clone(),
            control_line: self.control_line.clone(),
            apple_prompt: self.apple_prompt.clone(),
        }
    }

    pub fn models(&self) -> ModelSelection {
        let catalog: serde_json::Value =
            serde_json::from_str(diktafon_protocol::MODEL_CATALOG_JSON)
                .expect("bundled model catalog must parse");
        let valid = |id: &str, category: &str| {
            catalog["models"].as_array().is_some_and(|models| {
                models
                    .iter()
                    .any(|model| model["id"] == id && model["category"].as_str() == Some(category))
            })
        };
        let supports_language = |model: &serde_json::Value| {
            model["languages"]
                .as_array()
                .is_some_and(|languages| languages.iter().any(|code| code == &self.language))
        };
        let transcription = catalog["models"]
            .as_array()
            .and_then(|models| {
                models
                    .iter()
                    .find(|model| {
                        model["id"] == self.transcription_model
                            && model["category"] == "transcription"
                            && supports_language(model)
                    })
                    .or_else(|| {
                        models.iter().find(|model| {
                            model["category"] == "transcription" && supports_language(model)
                        })
                    })
            })
            .and_then(|model| model["id"].as_str())
            .unwrap_or(DEFAULT_TRANSCRIPTION_MODEL)
            .to_string();
        ModelSelection {
            transcription,
            polishing: if valid(&self.polishing_model, "polishing") {
                self.polishing_model.clone()
            } else {
                DEFAULT_POLISHING_MODEL.into()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_strings_parse_and_bare_keys_are_rejected() {
        assert_eq!(
            parse_hotkey("alt+space").unwrap().id(),
            CONFIG.hotkey().id()
        );
        assert!(parse_hotkey("ctrl+shift+a").is_some());
        assert!(parse_hotkey("cmd+f5").is_some());
        assert!(parse_hotkey("space").is_none());
        assert!(parse_hotkey("alt+nonsense").is_none());
        assert!(parse_hotkey("").is_none());
    }

    #[test]
    fn settings_hotkey_falls_back_to_default() {
        let settings = SessionSettings {
            hotkey: "garbage".into(),
            ..Default::default()
        };
        assert_eq!(settings.hotkey().id(), CONFIG.hotkey().id());
    }

    #[test]
    fn unknown_or_wrong_category_models_fall_back_independently() {
        let settings = SessionSettings {
            transcription_model: "missing".into(),
            polishing_model: DEFAULT_TRANSCRIPTION_MODEL.into(),
            ..Default::default()
        };
        assert_eq!(settings.models(), ModelSelection::default());
    }

    #[test]
    fn configs_without_model_fields_use_catalog_defaults() {
        let settings: SessionSettings = serde_json::from_str(
            r#"{"language":"de","control_line":"custom","idle_unload_secs":60,"sound_cues":false,"hotkey":"alt+d"}"#,
        )
        .unwrap();
        assert_eq!(settings.transcription_model, DEFAULT_TRANSCRIPTION_MODEL);
        assert_eq!(settings.polishing_model, DEFAULT_POLISHING_MODEL);
        assert_eq!(settings.language, "de");
        assert_eq!(settings.apple_prompt, DEFAULT_APPLE_PROMPT);
    }

    #[test]
    fn an_explicitly_empty_apple_prompt_stays_empty() {
        let settings: SessionSettings = serde_json::from_str(r#"{"apple_prompt":""}"#).unwrap();
        assert!(settings.apple_prompt.is_empty());
    }

    #[test]
    fn unsupported_transcription_model_falls_back_to_one_for_the_language() {
        let settings = SessionSettings {
            language: "it".into(),
            transcription_model: "canary-1b-flash-q5-k-m".into(),
            ..Default::default()
        };
        assert_eq!(settings.models().transcription, "cohere-transcribe-q5-k-m");
    }
}
