//! "Signal" design tokens; see docs/design.md for the system and rationale.
//! Colors are `0xRRGGBB00` so call sites append the alpha byte: `rgba(SURFACE | 0xE8)`.

/// Pill ground: deep indigo glass.
pub const SURFACE: u32 = 0x14163A00;
/// Borders: violet-tinted hairline.
pub const HAIRLINE: u32 = 0x9B9DFF00;
/// Words and labels: cool white.
pub const TEXT_PRIMARY: u32 = 0xF1F2FF00;
/// Secondary readouts (elapsed time).
pub const TEXT_DIM: u32 = 0xAAACD600;
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
