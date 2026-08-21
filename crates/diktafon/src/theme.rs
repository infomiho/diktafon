//! "Signal" design tokens; see docs/design.md for the system and rationale.
//! Colors are `0xRRGGBB00` so call sites append the alpha byte: `rgba(SURFACE | 0xE8)`.
//! The settings window consumes the same tokens through
//! [`apply_settings_theme`], so the pill and the windows cannot drift apart.

// Surfaces: one indigo hue stepped by lightness; depth is a step plus a
// hairline, never a shadow.
/// Window ground; the deep void.
pub const BACKGROUND: u32 = 0x10122900;
/// Pill glass, input and card ground.
pub const SURFACE: u32 = 0x17193400;
/// Hover fills, active nav, popovers.
pub const SURFACE_RAISED: u32 = 0x24284900;
/// Sidebar; the deepest step.
pub const SURFACE_SUNKEN: u32 = 0x0B0D2000;
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

// The single interactive accent: the polishing magenta deepened for large
// fills (white text passes AA on it), so buttons and focus rings speak the
// same "diktafon is acting" hue as the pill without going neon.
pub const ACCENT: u32 = 0x9843C000;
const ACCENT_HOVER: u32 = 0xA853D100;
const ACCENT_ACTIVE: u32 = 0x8835AE00;
const RAISED_HOVER: u32 = 0x2B2F5100;
const RAISED_ACTIVE: u32 = 0x1D213F00;
/// Off-state switch track: the raised surface is invisible against the
/// window ground at track size, so it gets its own clearly lighter step.
const SWITCH_TRACK: u32 = 0x5A608600;

/// The display face for titles, embedded via [`install_fonts`] so the
/// bundle needs no installed fonts. Body text is the system font; Menlo
/// (always present on macOS) covers mono readouts.
pub const FONT_DISPLAY: &str = "Chakra Petch";
pub const FONT_MONO: &str = "Menlo";

/// Register the embedded display face with the text system; must run before
/// any window renders.
pub fn install_fonts(cx: &gpui::App) {
    let fonts = vec![std::borrow::Cow::Borrowed(
        &include_bytes!("../assets/fonts/ChakraPetch-SemiBold.ttf")[..],
    )];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("embedding Chakra Petch failed, titles fall back to the system font: {e:#}");
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
        ("primary.foreground", hex(TEXT_PRIMARY)),
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
        "font.size": 15,
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
