//! The app's icon set: Solar glyphs bundled under `assets/icons/diktafon`,
//! drawn through gpui-component's `Icon` so they take the text colour and
//! size of wherever they sit. Anything not here falls back to the kit's own
//! Lucide bundle (see [`crate::assets`]).

use gpui::SharedString;
use gpui_component::IconNamed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiktafonIcon {
    Settings,
    Bot,
    History,
    Tuning,
    CloseCircle,
    Play,
    Pause,
    Copy,
    Check,
    TrashBin,
    Restart,
    X,
}

impl DiktafonIcon {
    #[cfg(test)]
    pub const ALL: [Self; 12] = [
        Self::Settings,
        Self::Bot,
        Self::History,
        Self::Tuning,
        Self::CloseCircle,
        Self::Play,
        Self::Pause,
        Self::Copy,
        Self::Check,
        Self::TrashBin,
        Self::Restart,
        Self::X,
    ];

    fn file_name(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Bot => "bot",
            Self::History => "history",
            Self::Tuning => "tuning-2",
            Self::CloseCircle => "close-circle",
            Self::Play => "play",
            Self::Pause => "pause",
            Self::Copy => "copy",
            Self::Check => "check",
            Self::TrashBin => "trash-bin",
            Self::Restart => "restart",
            Self::X => "x",
        }
    }
}

impl IconNamed for DiktafonIcon {
    fn path(self) -> SharedString {
        format!("icons/diktafon/{}.svg", self.file_name()).into()
    }
}
