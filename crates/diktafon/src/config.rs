//! Every user-tunable setting in one place. Still compile-time constants; a
//! config file can replace this later without touching the consumers.

use diktafon_protocol::SessionConfig;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};

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
    /// Seconds of daemon idleness before the models are unloaded; passed as
    /// DIKTAFOND_IDLE_SECS when the client spawns the daemon.
    pub idle_unload_secs: u64,
    /// Audible cues: mic live, cancel, error.
    pub sound_cues: bool,
    /// Push-to-talk chord in global-hotkey syntax, e.g. "alt+space".
    pub hotkey: String,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            language: CONFIG.language.into(),
            control_line: CONFIG.control_line.into(),
            idle_unload_secs: 300,
            sound_cues: true,
            hotkey: "alt+space".into(),
        }
    }
}

fn settings_path() -> std::path::PathBuf {
    diktafon_protocol::data_dir().join("config.json")
}

impl SessionSettings {
    /// Missing or unparseable file falls back to the defaults.
    pub fn load() -> Self {
        std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
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
        let mut settings = SessionSettings::default();
        settings.hotkey = "garbage".into();
        assert_eq!(settings.hotkey().id(), CONFIG.hotkey().id());
    }
}
