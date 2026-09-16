//! Decorative illustrations for the home page bento tiles.
//!
//! SVG shapes need an explicit empty body (`rect {}`, not `rect;`): maud only
//! omits the closing tag for known HTML void elements, and an unclosed `<rect>`
//! makes the HTML parser nest the rest of the drawing inside it.

use maud::{Markup, html};

const VIEW_NARROW: &str = "0 0 280 72";
const VIEW_SILENCE: &str = "0 0 280 110";
const VIEW_STACK: &str = "0 0 280 152";

pub fn speech() -> Markup {
    html! {
        div .viz .viz-speech aria-hidden="true" {
            div .speech-pill {
                div .speech-wash {
                    span {} span {} span {}
                }
                div .speech-dots {
                    @for _ in 0..15 {
                        span {}
                    }
                }
                span .speech-label { "Transcribing" }
            }
        }
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

/// Local history: the real pane's shape, with believable transcript snippets
/// instead of placeholder skeleton lines.
pub fn history() -> Markup {
    html! {
        svg .viz .viz-history viewBox=(VIEW_STACK) aria-hidden="true" {
            rect .field x="8" y="6" width="264" height="22" rx="11" {}
            circle .lens cx="26" cy="17" r="5.5" {}
            line .handle x1="30" y1="21" x2="35" y2="26" {}
            rect .label x="8" y="38" width="46" height="8" rx="4" {}
            (history_row(54.0, "09:41", "Send the invoice", "tomorrow morning"))
            (history_row(84.0, "09:18", "Call me after lunch", "about the new build"))
            (history_row(114.0, "08:52", "Move the meeting to", "Thursday afternoon"))
        }
    }
}

fn history_row(y: f32, time: &str, first: &str, second: &str) -> Markup {
    html! {
        g .entry {
            rect .card x="8" y=(y) width="264" height="26" rx="8" {}
            text .time x="18" y=(y + 15.0) { (time) }
            text .snippet x="58" y=(y + 11.0) { (first) }
            text .snippet .secondary x="58" y=(y + 20.0) { (second) }
            rect .back x="236" y=(y + 7.0) width="10" height="10" rx="2" {}
            rect .front x="243" y=(y + 13.0) width="10" height="10" rx="2" {}
        }
    }
}

/// Unloads when idle: a simplified Activity Monitor row. The resident-memory
/// figure and bar drop when the models unload, which is the whole point.
pub fn idle() -> Markup {
    html! {
        svg .viz .viz-idle viewBox=(VIEW_STACK) aria-hidden="true" {
            rect .monitor x="8" y="7" width="264" height="58" rx="9" {}
            circle .monitor-light cx="20" cy="19" r="3" {}
            text .monitor-title x="30" y="23" { "Activity Monitor" }
            text .monitor-memory x="238" y="23" { "Memory" }
            line .monitor-rule x1="8" y1="31" x2="272" y2="31" {}
            rect .process-selected x="12" y="37" width="256" height="20" rx="5" {}
            text .process-name x="22" y="51" { "diktafon" }
            text .process-memory .memory-loaded x="256" y="51" { "353 MB" }
            text .process-memory .memory-released x="256" y="51" { "24 MB" }
            text .idle-note x="8" y="82" { "5 min idle" }
            text .memory-caption x="8" y="132" { "Illustrative memory usage" }
            rect .memory-track x="8" y="96" width="264" height="13" rx="6.5" {}
            rect .memory-fill x="8" y="96" width="148" height="13" rx="6.5" {}
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
