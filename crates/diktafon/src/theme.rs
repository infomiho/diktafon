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

// Text is never tinted by phase or accent color: the grille carries the
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
/// The meter at rest; muted, never glows.
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

// ThemeConfigColors keeps its base-color fields private, so it cannot be
// built with struct-literal syntax; assigning onto its Default is the only
// typo-proof construction left.
#[allow(clippy::field_reassign_with_default)]
fn signal_theme_config() -> gpui_component::ThemeConfig {
    use gpui_component::{ThemeConfig, ThemeConfigColors, ThemeMode};
    let color = |c: u32| Some(hex(c).into());
    let alpha = |c: u32, a: u8| Some(hex_a(c, a).into());
    let hairline = || alpha(HAIRLINE, 0x22);
    let mut colors = ThemeConfigColors::default();
    colors.background = color(BACKGROUND);
    colors.foreground = color(TEXT_PRIMARY);
    colors.border = hairline();
    colors.input = hairline();
    colors.window_border = hairline();
    colors.title_bar = color(BACKGROUND);
    colors.title_bar_border = hairline();
    colors.muted = color(SURFACE_RAISED);
    colors.muted_foreground = color(TEXT_DIM);
    colors.accent = color(SURFACE_RAISED);
    colors.accent_foreground = color(TEXT_PRIMARY);
    colors.primary = color(ACCENT);
    colors.primary_hover = color(ACCENT_HOVER);
    colors.primary_active = color(ACCENT_ACTIVE);
    colors.primary_foreground = color(TEXT_PRIMARY);
    colors.secondary = color(SURFACE_RAISED);
    colors.secondary_hover = color(RAISED_HOVER);
    colors.secondary_active = color(RAISED_ACTIVE);
    colors.secondary_foreground = color(TEXT_PRIMARY);
    colors.sidebar = color(SURFACE_SUNKEN);
    colors.sidebar_foreground = color(TEXT_DIM);
    colors.sidebar_accent = color(SURFACE_RAISED);
    colors.sidebar_accent_foreground = color(TEXT_PRIMARY);
    colors.sidebar_border = hairline();
    colors.popover = color(SURFACE_RAISED);
    colors.popover_foreground = color(TEXT_PRIMARY);
    colors.list = color(SURFACE_RAISED);
    colors.list_even = color(SURFACE_RAISED);
    colors.list_hover = color(RAISED_HOVER);
    colors.list_active = alpha(ACCENT, 0x30);
    colors.selection = alpha(ACCENT, 0x40);
    colors.ring = color(ACCENT);
    colors.caret = color(TEXT_PRIMARY);
    colors.link = color(ACCENT);
    colors.switch = color(SWITCH_TRACK);
    colors.scrollbar_thumb = alpha(HAIRLINE, 0x50);
    colors.overlay = alpha(SURFACE_SUNKEN, 0x80);
    colors.danger = color(SIGNAL_RED);
    colors.danger_foreground = Some("#ff6b78".into());
    colors.warning = color(AURORA_EMBER);
    colors.warning_foreground = Some("#ff7e5c".into());
    colors.success = color(SIGNAL_WHITE);
    colors.success_foreground = color(TEXT_PRIMARY);
    ThemeConfig {
        name: "Signal Dark".into(),
        mode: ThemeMode::Dark,
        font_size: Some(15.),
        mono_font_family: Some(FONT_MONO.into()),
        radius: Some(6),
        radius_lg: Some(8),
        shadow: Some(false),
        colors,
        ..Default::default()
    }
}
