//! "Signal" design tokens; see docs/design.md for the system and rationale.
//! Colors are `0xRRGGBB00` so call sites append the alpha byte: `rgba(SURFACE | 0xE8)`.
//! The settings window consumes the same tokens through
//! [`apply_settings_theme`], so the pill and the windows cannot drift apart.

// Surfaces: one indigo hue stepped by lightness; depth is a step plus a
// hairline, never a shadow.
/// Window ground; the deep void.
pub const BACKGROUND: u32 = 0x0E102C00;
/// Pill glass, input and card ground.
pub const SURFACE: u32 = 0x14163A00;
/// Hover fills, active nav, popovers.
pub const SURFACE_RAISED: u32 = 0x1C1F5200;
/// Sidebar; the deepest step.
pub const SURFACE_SUNKEN: u32 = 0x0B0D2400;
/// Borders: violet-tinted hairline, ~13% alpha at the call site.
pub const HAIRLINE: u32 = 0x9B9DFF00;

// Text is never tinted by phase or accent color: the orbit carries the
// color, text carries the words.
/// Words and labels: cool white.
pub const TEXT_PRIMARY: u32 = 0xF1F2FF00;
/// Descriptions, secondary readouts.
pub const TEXT_DIM: u32 = 0xAAACD600;

// The phase language. Status colors derive from it: danger is SIGNAL_RED,
// warning is AURORA_EMBER, success/completion is white (Signal has no green).
/// Recording signal.
pub const SIGNAL_RED: u32 = 0xFF3B4D00;
/// Transcribing signal.
pub const SIGNAL_WHITE: u32 = 0xFFFFFF00;
/// Polishing signal.
pub const SIGNAL_MAGENTA: u32 = 0xCE5CFF00;
/// Orbit at rest; muted, never glows.
pub const RING_IDLE: u32 = 0x8E90BE00;
/// Aurora wash companions to SIGNAL_RED: a warm ember and a red-magenta
/// rose, so the recording glow spans a hot-red family instead of one flat hue.
pub const AURORA_EMBER: u32 = 0xFF5A3600;
pub const AURORA_ROSE: u32 = 0xE0459E00;

// The single interactive accent; deliberately the polishing magenta, so
// buttons and focus rings speak the same "diktafon is acting" color.
pub const ACCENT: u32 = SIGNAL_MAGENTA;
const ACCENT_HOVER: u32 = 0xD878FF00;
const ACCENT_ACTIVE: u32 = 0xBB3EF200;
const RAISED_HOVER: u32 = 0x23265E00;
const RAISED_ACTIVE: u32 = 0x171A4500;
/// Off-state switch track: the raised surface is invisible against the
/// window ground at track size, so it gets its own clearly lighter step.
const SWITCH_TRACK: u32 = 0x3A3E7C00;

/// The UI face; embedded via [`install_fonts`] so the bundle needs no
/// installed fonts. Menlo (always present on macOS) covers mono readouts.
pub const FONT_UI: &str = "Inter";
pub const FONT_MONO: &str = "Menlo";

/// Register the embedded Inter faces with the text system; must run before
/// any window renders.
pub fn install_fonts(cx: &gpui::App) {
    let fonts = vec![
        std::borrow::Cow::Borrowed(&include_bytes!("../assets/fonts/Inter-Regular.ttf")[..]),
        std::borrow::Cow::Borrowed(&include_bytes!("../assets/fonts/Inter-Medium.ttf")[..]),
        std::borrow::Cow::Borrowed(&include_bytes!("../assets/fonts/Inter-SemiBold.ttf")[..]),
    ];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("embedding Inter failed, falling back to the system font: {e:#}");
    }
}

fn hex(color: u32) -> String {
    format!("#{:06x}", color >> 8)
}

fn hex_a(color: u32, alpha: u8) -> String {
    format!("#{:06x}{alpha:02x}", color >> 8)
}

/// Project the Signal tokens onto gpui-component's theme so kit widgets
/// render Signal instead of kit defaults. The windows are Signal-dark
/// regardless of system appearance: the pill and the menu bar are
/// dark-native, and one language beats two.
pub fn apply_settings_theme(cx: &mut gpui::App) {
    let config = std::rc::Rc::new(signal_theme_config());
    gpui_component::Theme::global_mut(cx).apply_config(&config);
    gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
}

fn signal_theme_config() -> gpui_component::ThemeConfig {
    let hairline = hex_a(HAIRLINE, 0x22);
    let colors: serde_json::Map<String, serde_json::Value> = [
        ("background", hex(BACKGROUND)),
        ("foreground", hex(TEXT_PRIMARY)),
        ("border", hairline.clone()),
        ("input.border", hairline.clone()),
        ("window.border", hairline.clone()),
        ("title_bar.background", hex(BACKGROUND)),
        ("title_bar.border", hairline.clone()),
        ("muted.background", hex(SURFACE_RAISED)),
        ("muted.foreground", hex(TEXT_DIM)),
        ("accent.background", hex(SURFACE_RAISED)),
        ("accent.foreground", hex(TEXT_PRIMARY)),
        ("primary.background", hex(ACCENT)),
        ("primary.hover.background", hex(ACCENT_HOVER)),
        ("primary.active.background", hex(ACCENT_ACTIVE)),
        ("primary.foreground", hex(BACKGROUND)),
        ("secondary.background", hex(SURFACE_RAISED)),
        ("secondary.hover.background", hex(RAISED_HOVER)),
        ("secondary.active.background", hex(RAISED_ACTIVE)),
        ("secondary.foreground", hex(TEXT_PRIMARY)),
        ("sidebar.background", hex(SURFACE_SUNKEN)),
        ("sidebar.foreground", hex(TEXT_DIM)),
        ("sidebar.accent.background", hex(SURFACE_RAISED)),
        ("sidebar.accent.foreground", hex(TEXT_PRIMARY)),
        ("sidebar.border", hairline),
        ("popover.background", hex(SURFACE_RAISED)),
        ("popover.foreground", hex(TEXT_PRIMARY)),
        ("list.background", hex(SURFACE_RAISED)),
        ("list.even.background", hex(SURFACE_RAISED)),
        ("list.hover.background", hex(RAISED_HOVER)),
        ("list.active.background", hex_a(ACCENT, 0x30)),
        ("selection.background", hex_a(ACCENT, 0x40)),
        ("ring", hex(ACCENT)),
        ("caret", hex(TEXT_PRIMARY)),
        ("link.foreground", hex(ACCENT)),
        ("switch.background", hex(SWITCH_TRACK)),
        ("scrollbar.thumb.background", hex_a(HAIRLINE, 0x50)),
        ("overlay", hex_a(SURFACE_SUNKEN, 0x80)),
        ("danger.background", hex(SIGNAL_RED)),
        ("danger.foreground", "#ff6b78".into()),
        ("warning.background", hex(AURORA_EMBER)),
        ("warning.foreground", "#ff7e5c".into()),
        ("success.background", hex(SIGNAL_WHITE)),
        ("success.foreground", hex(TEXT_PRIMARY)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.into()))
    .collect();
    let config = serde_json::json!({
        "name": "Signal Dark",
        "mode": "dark",
        "font.family": FONT_UI,
        "mono_font.family": FONT_MONO,
        "radius": 6,
        "radius_lg": 8,
        "shadow": false,
        "colors": colors,
    });
    serde_json::from_value(config).expect("signal theme config is valid")
}

#[cfg(test)]
mod tests {
    #[test]
    fn signal_theme_config_parses() {
        let config = super::signal_theme_config();
        assert!(config.mode.is_dark());
    }
}
