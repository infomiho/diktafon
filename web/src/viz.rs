//! Hand-drawn SVG illustrations for the home page's bento tiles. Each is
//! decorative (`aria-hidden`); the motion lives in `style.css` under a
//! `prefers-reduced-motion: no-preference` guard, so every rest state reads on
//! its own.
//!
//! SVG shapes need an explicit empty body (`rect {}`, not `rect;`): maud only
//! omits the closing tag for known HTML void elements, and an unclosed `<rect>`
//! makes the HTML parser nest the rest of the drawing inside it.

use maud::{Markup, html};

const VIEW_WIDE: &str = "0 0 600 96";
const VIEW_NARROW: &str = "0 0 280 72";
const VIEW_SILENCE: &str = "0 0 280 110";
const VIEW_STACK: &str = "0 0 280 152";

/// Speech, on device: the compact pill shown while the daemon is transcribing.
/// Its grille and capsule proportions follow the production overlay.
pub fn speech() -> Markup {
    const LEVELS: [usize; 5] = [3, 0, 0, 0, 0];
    html! {
        svg .viz .viz-speech viewBox=(VIEW_WIDE) aria-hidden="true" {
            rect .pill x="82" y="12" width="436" height="72" rx="36" {}
            @for col in 0..5usize {
                @for row in 0..3usize {
                    (grille_dot(col, row, LEVELS[col]))
                }
            }
            text .status x="300" y="57" { "Transcribing" }
        }
    }
}

fn grille_dot(col: usize, row: usize, level: usize) -> Markup {
    let lit = (2 - row) < level;
    let cx = 150.0 + col as f32 * 15.0;
    let cy = 34.0 + row as f32 * 14.0;
    let delay = col * 3 + (2 - row);
    html! {
        circle class=(if lit { "dot lit" } else { "dot" })
                cx=(cx) cy=(cy) r="4.5" style=(format!("--i:{delay}")) {}
    }
}

/// Cleanup on every take: a filler word gets struck out on hover.
pub fn cleanup() -> Markup {
    html! {
        svg .viz .viz-cleanup viewBox=(VIEW_NARROW) aria-hidden="true" {
            rect .line x="8" y="15" width="200" height="9" rx="4.5" {}
            rect .line x="8" y="32" width="84" height="9" rx="4.5" {}
            rect .filler x="100" y="32" width="54" height="9" rx="4.5" {}
            rect .line x="162" y="32" width="64" height="9" rx="4.5" {}
            rect .line x="8" y="49" width="150" height="9" rx="4.5" {}
            line .strike x1="100" y1="36.5" x2="154" y2="36.5" {}
        }
    }
}

/// Split at silence: two chunks part around a marked cut on hover.
pub fn silence() -> Markup {
    html! {
        svg .viz .viz-silence viewBox=(VIEW_SILENCE) aria-hidden="true" {
            g .chunk-left {
                @for i in 0..10 {
                    rect .bar
                        x=(8.0 + i as f32 * 12.0)
                        y=(55.0 - wave(i, 0.0) / 2.0)
                        width="5" height=(wave(i, 0.0)) rx="2.5" {}
                }
            }
            g .chunk-right {
                @for i in 0..10 {
                    rect .bar
                        x=(160.0 + i as f32 * 12.0)
                        y=(55.0 - wave(i, 2.4) / 2.0)
                        width="5" height=(wave(i, 2.4)) rx="2.5" {}
                }
            }
            line .cut x1="140" y1="16" x2="140" y2="94" {}
        }
    }
}

/// Local history: the real pane's shape. A search well, a day label, and rows
/// with a time, two wrapped lines, and a copy button that lights up in turn.
pub fn history() -> Markup {
    let rows = [
        (54.0_f32, (150.0_f32, 104.0_f32)),
        (84.0, (128.0, 168.0)),
        (114.0, (170.0, 96.0)),
    ];
    html! {
        svg .viz .viz-history viewBox=(VIEW_STACK) aria-hidden="true" {
            rect .field x="8" y="6" width="264" height="22" rx="11" {}
            circle .lens cx="26" cy="17" r="5.5" {}
            line .handle x1="30" y1="21" x2="35" y2="26" {}
            rect .label x="8" y="38" width="46" height="8" rx="4" {}
            @for (i, (y, (first, second))) in rows.iter().enumerate() {
                (history_row(*y, *first, *second, i))
            }
        }
    }
}

fn history_row(y: f32, first: f32, second: f32, delay: usize) -> Markup {
    html! {
        g .entry style=(format!("--i:{delay}")) {
            rect .card x="8" y=(y) width="264" height="26" rx="8" {}
            rect .time x="20" y=(y + 9.0) width="28" height="7" rx="3.5" {}
            rect .text x="58" y=(y + 6.0) width=(first) height="6" rx="3" {}
            rect .text x="58" y=(y + 15.0) width=(second) height="6" rx="3" {}
            rect .back x="236" y=(y + 7.0) width="10" height="10" rx="2" {}
            rect .front x="243" y=(y + 13.0) width="10" height="10" rx="2" {}
        }
    }
}

/// Unloads when idle: the daemon's lifecycle rather than another content list.
/// It shows the two model roles, the five-minute timeout, and the transition
/// from loaded to unloaded on hover.
pub fn idle() -> Markup {
    html! {
        svg .viz .viz-idle viewBox=(VIEW_STACK) aria-hidden="true" {
            circle .live cx="18" cy="17" r="5" {}
            rect .running x="30" y="11" width="52" height="8" rx="4" {}
            rect .loaded x="92" y="11" width="82" height="8" rx="4" {}
            rect .model-tag x="8" y="38" width="116" height="24" rx="12" {}
            rect .model-tag x="132" y="38" width="116" height="24" rx="12" {}
            rect .tag-text x="22" y="47" width="55" height="6" rx="3" {}
            rect .tag-text x="146" y="47" width="48" height="6" rx="3" {}
            line .timeline x1="22" y1="100" x2="258" y2="100" {}
            circle .state-active cx="22" cy="100" r="6" {}
            circle .state-idle cx="140" cy="100" r="6" {}
            circle .state-off cx="258" cy="100" r="6" {}
            rect .state-label x="8" y="116" width="48" height="6" rx="3" {}
            rect .state-label x="116" y="116" width="48" height="6" rx="3" {}
            rect .state-label x="236" y="116" width="36" height="6" rx="3" {}
            rect .footer x="8" y="140" width="122" height="6" rx="3" {}
        }
    }
}

/// A repeatable pseudo-waveform height in the 8..52 range.
fn wave(i: usize, seed: f32) -> f32 {
    let t = i as f32;
    let a = (t * 0.62 + seed).sin().abs();
    let b = (t * 0.23 + 1.1 + seed).sin().abs();
    (8.0 + 44.0 * a * (0.45 + 0.55 * b)).round()
}
