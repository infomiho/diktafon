//! The floating recording pill: a small always-on-top window that appears
//! while a session is active. GPUI's `WindowKind::PopUp` already provides the
//! overlay mechanics (non-activating panel, above-normal level, joins all
//! Spaces, shows over fullscreen apps).
//!
//! The design is settled in docs/mockups/pill.html (variant E): the mark's
//! grille as a 5x3 dot level meter plus the elapsed time while recording,
//! then the status word while processing. The chip is 172px for the whole
//! normal flow and grows only when a text (a long error, the model download)
//! needs more room. Dots run fast-attack/slow-decay ballistics; phase
//! changes lerp the grille color instead of hard-cutting. Session endings:
//! a white wave sweeping up on paste, a quiet dim on cancel, a steady red
//! hold on error. Enter rises; exit fades the contents in place and sinks
//! the chip along the entry path, faster. Under Reduce Motion the ambient
//! animation freezes and the pill cross-fades.

use crate::capture::LevelBars;
use crate::dictation::{Dictation, Outcome, Phase};
use crate::theme;
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Bounds, BoxShadow, Context, Entity,
    IntoElement, ParentElement, Pixels, Render, Styled, TextRun, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div,
    ease_out_quint, point, px, rgba, size,
};
use objc2::MainThreadMarker;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::Duration;

/// The chip's resting width; wide enough for the grille, the time, and the
/// status words.
const CHIP_MIN: f32 = 172.;
/// Growth ceiling for long texts (errors, download progress).
const CHIP_MAX: f32 = 260.;
/// The window carries slack around the widest chip for the slide travel.
const WINDOW_WIDTH: f32 = CHIP_MAX + 4.;
const PILL_HEIGHT: Pixels = px(38.);
const PAD_X: f32 = 14.;
const CONTENT_GAP: f32 = 11.;

// The grille: the mark's 3-row dot pattern, extended to 5 columns as a
// level meter (see docs/design.md "The mark").
const COLS: usize = 5;
const ROWS: usize = 3;
const DOT: f32 = 4.5;
const DOT_GAP: f32 = 3.;
const GRILLE_W: f32 = COLS as f32 * DOT + (COLS as f32 - 1.) * DOT_GAP;
const GRILLE_H: f32 = ROWS as f32 * DOT + (ROWS as f32 - 1.) * DOT_GAP;

/// Vertical slide distance of the enter/exit transition.
const TRAVEL: f32 = 9.;
const TOP_PAD: f32 = 0.;
/// Gap between the pill's resting position and the bottom of the visible
/// frame (above the Dock).
const BOTTOM_MARGIN: f64 = 15.;

/// The pill shows on every dictation, so entry is quick.
const ENTER: Duration = Duration::from_millis(260);
/// The exit is staged: first the pill's contents fade out in place, then the
/// empty chip sinks away. EXIT is the total; manage() times removal off it.
const EXIT_CONTENT: Duration = Duration::from_millis(130);
const EXIT: Duration = Duration::from_millis(330);
const CONTENT_FADE: Duration = Duration::from_millis(150);
/// Aurora glow floor while recording: the wash never goes fully dark, so
/// quiet moments read as embers rather than the effect blinking off.
const AURORA_BASELINE: f32 = 0.22;
/// Below this level an aurora blob's glow reads as a smudge, not a glow.
const AURORA_FLOOR: f32 = 0.08;
/// How long each ending holds before the fade begins.
const ERROR_HOLD: Duration = Duration::from_millis(2400);
const BLOOM_HOLD: Duration = Duration::from_millis(420);
const CANCEL_HOLD: Duration = Duration::from_millis(160);
/// Length of the white wave the grille plays on paste.
const BLOOM_WAVE: Duration = Duration::from_millis(380);

/// Meter ballistics: dots rise fast and fall slow, so speech reads as
/// motion instead of strobing. Per-frame rates at the 30fps repaint cadence.
const ATTACK: f32 = 0.55;
const DECAY: f32 = 0.14;
/// Per-frame convergence of the grille color toward the phase target.
const COLOR_RATE: f32 = 0.22;
/// Per-frame convergence of the chip width toward its fitted target.
const WIDTH_RATE: f32 = 0.35;

/// Open the pill while a session is active; on idle, play the ending beat
/// and the exit animation, then close it.
pub fn manage(cx: &mut App, dictation: Entity<Dictation>, levels: LevelBars) {
    let mut open: Option<WindowHandle<Pill>> = None;
    // A pill lingering through its ending hold; a new session would otherwise
    // open a second pill at the same spot on top of it.
    let held: std::rc::Rc<std::cell::Cell<Option<WindowHandle<Pill>>>> = Default::default();
    cx.observe(&dictation, move |dictation, cx| {
        let idle = dictation.read(cx).phase == Phase::Idle;
        match (&open, idle) {
            (Some(handle), true) => {
                let handle = *handle;
                // Every ending holds briefly so its beat is readable: the
                // bloom wave on paste, the dim on cancel, and the error
                // message longest of all.
                let hold = match dictation.read(cx).outcome {
                    Some(Outcome::Failed(_)) => ERROR_HOLD,
                    Some(Outcome::Pasted) => BLOOM_HOLD,
                    _ => CANCEL_HOLD,
                };
                held.set(Some(handle));
                // A session starting during the fade opens a fresh pill; the
                // fading one is gone within EXIT, so the overlap is brief.
                open = None;
                let held = held.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(hold).await;
                    cx.update(|cx| {
                        // A new session may have superseded (and removed) this
                        // pill during the hold.
                        if held.get() == Some(handle) {
                            held.set(None);
                        }
                        let _ = handle.update(cx, |pill, _, cx| {
                            pill.closing = true;
                            pill.closing_since = Some(std::time::Instant::now());
                            cx.notify();
                        });
                    });
                    cx.background_executor()
                        .timer(EXIT + Duration::from_millis(40))
                        .await;
                    cx.update(|cx| {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    });
                })
                .detach();
            }
            (None, false) => {
                if let Some(superseded) = held.take() {
                    let _ = superseded.update(cx, |_, window, _| window.remove_window());
                }
                open = open_pill(&dictation, levels.clone(), cx);
            }
            _ => {}
        }
    })
    .detach();
}

fn open_pill(
    dictation: &Entity<Dictation>,
    levels: LevelBars,
    cx: &mut App,
) -> Option<WindowHandle<Pill>> {
    let bounds = pill_bounds()?;
    let dictation = dictation.clone();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        |window, cx| {
            configure_overlay_window(window);
            cx.new(|cx| Pill::new(dictation, levels, cx))
        },
    )
    .ok()
}

/// macOS decorates the (mostly transparent) window: the system shadow reads
/// as a ghost rectangle, and the titled style mask gpui uses draws a light
/// top-edge line that shows beside the narrower chip. Strip the window down
/// to a borderless non-activating panel and turn the shadow off. The pill is
/// also a passive overlay, so let clicks pass through to whatever is
/// underneath it.
fn configure_overlay_window(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        let ns_view = appkit.ns_view.as_ptr() as *const objc2_app_kit::NSView;
        let Some(ns_window) = (unsafe { &*ns_view }).window() else {
            return;
        };
        // Borderless is the empty mask; only the panel bit remains.
        ns_window.setStyleMask(objc2_app_kit::NSWindowStyleMask::NonactivatingPanel);
        ns_window.setHasShadow(false);
        ns_window.setIgnoresMouseEvents(true);
    }
}

/// Bottom-centered on the screen containing the cursor, tracking the Dock via
/// the visible frame. AppKit reports bottom-left-origin coordinates; GPUI
/// wants top-left-origin relative to the primary screen. The window carries
/// transparent padding around the pill for its slide travel and width growth.
fn pill_bounds() -> Option<Bounds<Pixels>> {
    let mtm = MainThreadMarker::new().expect("not on the main thread");
    let mouse = objc2_app_kit::NSEvent::mouseLocation();
    let screens = objc2_app_kit::NSScreen::screens(mtm);
    let primary_height = screens.iter().next()?.frame().size.height;
    // A cursor pinned to a screen's very top edge reports y == maxY and falls
    // through to the first-screen fallback; cosmetic and self-healing.
    let screen = screens
        .iter()
        .find(|s| {
            let f = s.frame();
            mouse.x >= f.origin.x
                && mouse.x < f.origin.x + f.size.width
                && mouse.y >= f.origin.y
                && mouse.y < f.origin.y + f.size.height
        })
        .or_else(|| screens.iter().next())?;
    let visible = screen.visibleFrame();

    let bottom_pad = f64::from(TRAVEL);
    let window_width = f64::from(WINDOW_WIDTH);
    let window_height = f64::from(TOP_PAD) + f64::from(PILL_HEIGHT) + bottom_pad;
    let x = visible.origin.x + (visible.size.width - window_width) / 2.;
    let window_bottom = visible.origin.y + BOTTOM_MARGIN - bottom_pad;
    let y = primary_height - (window_bottom + window_height);
    Some(Bounds {
        origin: point(px(x as f32), px(y as f32)),
        size: size(px(window_width as f32), px(window_height as f32)),
    })
}

/// Repaint cadence for the live meter; every animation in the pill advances
/// on this clock.
const BAR_FRAME: Duration = Duration::from_millis(33);

/// What the grille should show this frame; ending beats outrank the phase.
enum GrilleView {
    Phase(Phase),
    Error,
    Bloom,
    Cancel,
}

pub struct Pill {
    dictation: Entity<Dictation>,
    levels: LevelBars,
    closing: bool,
    /// When the exit fade began; times the staged sink.
    closing_since: Option<std::time::Instant>,
    /// What the fading pill keeps showing after the phase already went Idle.
    last_active: Phase,
    /// When this session's recording began, for the elapsed-time readout.
    recording_since: Option<std::time::Instant>,
    /// When the session ended; times the ending beat (the bloom wave).
    ended_at: Option<std::time::Instant>,
    /// Drives the ambient animations (scan, twinkle, aurora drift).
    opened_at: std::time::Instant,
    /// Per-dot smoothed lit level (fast attack, slow decay).
    dot_smooth: [f32; COLS * ROWS],
    /// The grille color, lerping toward the phase target (no hard cuts).
    live_color: [f32; 3],
    /// The chip width, lerping toward its content-fitted target.
    chip_width: f32,
    /// Per-band smoothed aurora intensity (fast attack, slow decay), so the
    /// glow breathes with speech instead of flickering with the raw meter.
    aurora_smooth: [f32; 3],
}

impl Pill {
    fn new(dictation: Entity<Dictation>, levels: LevelBars, cx: &mut Context<Self>) -> Self {
        cx.observe(&dictation, |_, _, cx| cx.notify()).detach();
        // Drive repaints for the pill's whole (short) life: this non-activating
        // panel gets no frames on its own, so every animation advances only
        // when we notify. Ends when the window removes the entity.
        cx.spawn(async move |pill, cx| {
            loop {
                cx.background_executor().timer(BAR_FRAME).await;
                if pill.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        })
        .detach();
        Self {
            dictation,
            levels,
            closing: false,
            closing_since: None,
            last_active: Phase::Recording,
            recording_since: None,
            ended_at: None,
            opened_at: std::time::Instant::now(),
            dot_smooth: [0.; COLS * ROWS],
            live_color: color_components(theme::SIGNAL_RED),
            chip_width: CHIP_MIN,
            aurora_smooth: [0.; 3],
        }
    }

    /// The meter's five bands from the 16-band capture spectrum.
    fn meter_bands(&self) -> [f32; COLS] {
        let levels = *self.levels.lock().unwrap();
        let band = |range: std::ops::Range<usize>| {
            let len = range.len() as f32;
            levels[range].iter().sum::<f32>() / len
        };
        [
            band(0..3),
            band(3..6),
            band(6..10),
            band(10..13),
            band(13..16),
        ]
    }

    /// Spectrum thirds (lows, mids, highs) smoothed with fast attack and slow
    /// decay; called once per frame while the aurora is active.
    fn aurora_bands(&mut self) -> [f32; 3] {
        let levels = *self.levels.lock().unwrap();
        let band = |range: std::ops::Range<usize>| {
            let len = range.len() as f32;
            levels[range].iter().sum::<f32>() / len
        };
        let raw = [band(0..5), band(5..11), band(11..16)];
        for (smooth, value) in self.aurora_smooth.iter_mut().zip(raw) {
            let rate = if value > *smooth { 0.55 } else { 0.08 };
            *smooth += (value - *smooth) * rate;
        }
        self.aurora_smooth
    }

    /// A voice-driven glow wash behind the pill content while recording,
    /// Siri-style. Three hue-shifted blobs (ember, red, rose; lows left,
    /// highs right) drift inside the pill over a constant baseline glow.
    fn aurora_wash(&self, bands: [f32; 3], reduce_motion: bool) -> AnyElement {
        let t = self.opened_at.elapsed().as_secs_f32();
        // Positions sit inside the 172px resting chip, drift included.
        let blobs = [
            (30., bands[0], theme::AURORA_EMBER, 0.),
            (85., bands[1], theme::SIGNAL_RED, 2.1),
            (140., bands[2], theme::AURORA_ROSE, 4.2),
        ];
        let mid = f32::from(PILL_HEIGHT) / 2.;
        div()
            .absolute()
            .inset_0()
            .rounded_full()
            .overflow_hidden()
            .children(blobs.into_iter().filter_map(|(x, level, hue, seed)| {
                // Below the floor the blob's glow reads as a smudge.
                if level < AURORA_FLOOR {
                    return None;
                }
                let drift = if reduce_motion {
                    0.
                } else {
                    (t * 0.4 + seed).sin() * 16.
                };
                let intensity = (AURORA_BASELINE + level).min(1.);
                let glow = rgba(hue | (intensity * 170.) as u32);
                Some(
                    div()
                        .absolute()
                        .left(px(x + drift - 4.))
                        .top(px(mid - 4.))
                        .size(px(8.))
                        .rounded_full()
                        .shadow(vec![
                            BoxShadow::new(px(0.), px(0.), glow.into())
                                .blur_radius(px(22.))
                                .spread_radius(px(10.)),
                        ]),
                )
            }))
            .into_any_element()
    }

    /// Layered inset glows hugging the capsule's edge, in two hues whose
    /// dominance slowly trades places. Brightness rides the voice level.
    fn aurora_edge(&self, bands: [f32; 3], reduce_motion: bool) -> Vec<BoxShadow> {
        let t = self.opened_at.elapsed().as_secs_f32();
        let overall = (bands.iter().sum::<f32>() / 3. + AURORA_BASELINE).min(1.);
        let trade = if reduce_motion {
            0.5
        } else {
            0.5 + 0.5 * (t * 0.7).sin()
        };
        let edge = |hue: u32, weight: f32| {
            let alpha = (overall * weight * 135.) as u32;
            BoxShadow::new(px(0.), px(0.), rgba(hue | alpha).into())
                .blur_radius(px(12.))
                .spread_radius(px(2.))
                .inset()
        };
        vec![
            edge(theme::SIGNAL_RED, 0.6 + 0.4 * trade),
            edge(theme::AURORA_ROSE, 0.6 + 0.4 * (1. - trade)),
        ]
    }

    /// The target lit level for one dot this frame, before ballistics.
    fn dot_target(&self, col: usize, row: usize, view: &GrilleView, reduce_motion: bool) -> f32 {
        let t = self.opened_at.elapsed().as_secs_f32();
        match view {
            GrilleView::Bloom => {
                // A white wave sweeping bottom-to-top, then holding bright.
                if reduce_motion {
                    return 0.8;
                }
                let p = self
                    .ended_at
                    .map(|at| at.elapsed().as_secs_f32() / BLOOM_WAVE.as_secs_f32())
                    .unwrap_or(1.)
                    .min(1.);
                let wave = p * (ROWS as f32 + 1.4) - (ROWS - 1 - row) as f32;
                wave.clamp(0., 1.)
            }
            GrilleView::Cancel => 0.,
            GrilleView::Error => 0.5,
            GrilleView::Phase(Phase::Recording) => {
                let level = self.meter_bands()[col];
                (level * 3.6 - (ROWS - 1 - row) as f32).clamp(0., 1.)
            }
            GrilleView::Phase(Phase::Transcribing) => {
                if reduce_motion {
                    return 0.35;
                }
                let scan = (t * 4.) % (COLS as f32 + 2.) - 1.;
                (1. - (col as f32 - scan).abs() / 1.5).max(0.) * 0.9
            }
            GrilleView::Phase(Phase::Polishing) => {
                if reduce_motion {
                    return 0.35;
                }
                // Random dots twinkling: a new constellation every beat,
                // from a position hash so it ports as pure math.
                let step = t * 2.5;
                let i0 = step.floor();
                let f = step - i0;
                let rnd = |k: f32| {
                    let x = ((col as f32 * 7.3 + row as f32 * 13.7) * 127.1 + k * 311.7).sin()
                        * 43758.547;
                    x - x.floor()
                };
                let ease = if f < 0.5 { f * 2. } else { (1. - f) * 2. };
                let a0 = if rnd(i0) > 0.7 { 1. } else { 0. };
                let a1 = if rnd(i0 + 1.) > 0.7 { 1. } else { 0. };
                (a0 * (1. - f)).max(a1 * f) * (0.55 + 0.45 * ease)
            }
            GrilleView::Phase(_) => 0.,
        }
    }

    /// The grille as a level meter: the mark's dot pattern, lit by voice or
    /// by the phase animation, with ballistics and color applied.
    fn grille(&mut self, view: &GrilleView, reduce_motion: bool) -> AnyElement {
        let target_color = color_components(match view {
            GrilleView::Bloom => theme::SIGNAL_WHITE,
            GrilleView::Cancel => theme::RING_IDLE,
            GrilleView::Error => theme::SIGNAL_RED,
            GrilleView::Phase(Phase::Recording) => theme::SIGNAL_RED,
            GrilleView::Phase(Phase::Transcribing) => theme::SIGNAL_WHITE,
            GrilleView::Phase(Phase::Polishing) => theme::SIGNAL_MAGENTA,
            GrilleView::Phase(_) => theme::RING_IDLE,
        });
        for (live, target) in self.live_color.iter_mut().zip(target_color) {
            *live += (target - *live) * COLOR_RATE;
        }
        let color = pack_color(self.live_color);

        let mut targets = [0.; COLS * ROWS];
        for (i, target) in targets.iter_mut().enumerate() {
            *target = self.dot_target(i % COLS, i / COLS, view, reduce_motion);
        }
        for (smooth, target) in self.dot_smooth.iter_mut().zip(targets) {
            let rate = if target > *smooth { ATTACK } else { DECAY };
            *smooth += (target - *smooth) * rate;
        }

        let smooth = self.dot_smooth;
        div()
            .relative()
            .w(px(GRILLE_W))
            .h(px(GRILLE_H))
            .flex_none()
            .children((0..COLS * ROWS).map(move |i| {
                let (col, row) = (i % COLS, i / COLS);
                let lit = smooth[i];
                let alpha = ((0.24 + lit * 0.76) * 255.) as u32;
                let dot = div()
                    .absolute()
                    .left(px(col as f32 * (DOT + DOT_GAP)))
                    .top(px(row as f32 * (DOT + DOT_GAP)))
                    .size(px(DOT))
                    .rounded_full()
                    .bg(rgba(color | alpha));
                if lit > 0.3 {
                    dot.shadow(vec![
                        BoxShadow::new(px(0.), px(0.), rgba(color | (lit * 242.) as u32).into())
                            .blur_radius(px(4. + lit * 6.)),
                    ])
                } else {
                    dot
                }
            }))
            .into_any_element()
    }

    /// The chip width fitted to the text it must show; CHIP_MIN covers the
    /// whole normal flow.
    fn fit_width(&mut self, text: &str, window: &Window, reduce_motion: bool) -> f32 {
        let target = if text.is_empty() {
            CHIP_MIN
        } else {
            let style = window.text_style();
            let run = TextRun {
                len: text.len(),
                font: style.font(),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let width = window
                .text_system()
                // shape_line rejects newlines; error text may one day carry
                // a raw daemon message.
                .shape_line(text.replace('\n', " ").into(), px(13.), &[run], None)
                .width;
            (PAD_X * 2. + GRILLE_W + CONTENT_GAP + f32::from(width) + 2.).clamp(CHIP_MIN, CHIP_MAX)
        };
        if reduce_motion {
            self.chip_width = target;
        } else {
            self.chip_width += (target - self.chip_width) * WIDTH_RATE;
        }
        self.chip_width
    }
}

/// `0xRRGGBB00` token to float components.
fn color_components(color: u32) -> [f32; 3] {
    [
        ((color >> 24) & 0xFF) as f32,
        ((color >> 16) & 0xFF) as f32,
        ((color >> 8) & 0xFF) as f32,
    ]
}

/// Float components back to a `0xRRGGBB00` value for `rgba`.
fn pack_color(c: [f32; 3]) -> u32 {
    ((c[0] as u32) << 24) | ((c[1] as u32) << 16) | ((c[2] as u32) << 8)
}

/// The status/error text, right-aligned and muted: the grille carries the
/// liveness, the word only names the phase.
fn status_label(text: String) -> AnyElement {
    div()
        .flex_1()
        .text_size(px(13.))
        .text_color(rgba(theme::TEXT_DIM | 0x8C))
        .text_right()
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text)
        .into_any_element()
}

/// Elapsed time with every glyph in its own fixed-width cell: proportional
/// digits vary in width, so even a right-aligned plain string shifts as the
/// digits change. Cells pin each glyph in place.
fn time_readout(since: std::time::Instant) -> AnyElement {
    let secs = since.elapsed().as_secs();
    let text = format!("{}:{:02}", secs / 60, secs % 60);
    div()
        .w(px(34.))
        .flex_none()
        .flex()
        .justify_end()
        .text_size(px(11.))
        .text_color(rgba(theme::TEXT_DIM | 0x8C))
        .children(text.chars().map(|c| {
            let cell = if c == ':' { 4. } else { 7. };
            div()
                .w(px(cell))
                .flex_none()
                .text_center()
                .child(c.to_string())
        }))
        .into_any_element()
}

/// Deliberately not `phase as u64`: these keys are animation element ids and
/// must stay stable even if the enum is ever reordered.
fn phase_key(phase: Phase) -> u64 {
    match phase {
        Phase::Idle => 0,
        Phase::Recording => 1,
        Phase::Transcribing => 2,
        Phase::Polishing => 3,
        Phase::Arming => 4,
    }
}

impl Render for Pill {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reduce_motion = cx.reduce_motion();
        let phase = self.dictation.read(cx).phase;
        if phase != Phase::Idle {
            self.last_active = phase;
        } else if self.ended_at.is_none() {
            self.ended_at = Some(std::time::Instant::now());
        }
        // The ending and exit keep showing the last active content.
        let display = if phase == Phase::Idle {
            self.last_active
        } else {
            phase
        };
        if display == Phase::Recording && self.recording_since.is_none() {
            self.recording_since = Some(std::time::Instant::now());
        }
        let outcome = self.dictation.read(cx).outcome.clone();
        let error = match (&outcome, phase) {
            (Some(Outcome::Failed(message)), Phase::Idle) => Some(message.clone()),
            _ => None,
        };
        let view = if error.is_some() {
            GrilleView::Error
        } else if phase == Phase::Idle {
            match outcome {
                Some(Outcome::Pasted) => GrilleView::Bloom,
                Some(Outcome::Cancelled) => GrilleView::Cancel,
                _ => GrilleView::Phase(display),
            }
        } else {
            GrilleView::Phase(display)
        };

        let recording = matches!(view, GrilleView::Phase(Phase::Recording));
        let aurora_bands = (recording && !self.closing).then(|| self.aurora_bands());
        let download = self.dictation.read(cx).download.clone();
        let text = if let Some(message) = &error {
            message.clone()
        } else if let Some(download) = &download {
            // A first-run model download outranks the session content: the
            // session is stalled on it and would otherwise look like a hang.
            format!("Downloading models {}%", download.percent)
        } else {
            // The last status word stays through the ending beat; only the
            // exit fade takes it away.
            match display {
                Phase::Transcribing => "Transcribing".into(),
                Phase::Polishing => "Polishing".into(),
                _ => String::new(),
            }
        };
        let chip_w = self.fit_width(&text, window, reduce_motion);
        let grille = self.grille(&view, reduce_motion);

        // Staged exit, driven off closing_since by the 30fps repaint loop:
        // contents fade out in place first, then the empty chip sinks away.
        // Under Reduce Motion the pill just disappears.
        let (content_alpha, sink) = if self.closing {
            if reduce_motion {
                (0., 1.)
            } else {
                let t = self
                    .closing_since
                    .map(|since| since.elapsed().as_secs_f32())
                    .unwrap_or(0.);
                let content_secs = EXIT_CONTENT.as_secs_f32();
                let sink_secs = (EXIT - EXIT_CONTENT).as_secs_f32();
                let sink = ((t - content_secs) / sink_secs).clamp(0., 1.);
                // Smoothstep so the sink starts and lands gently.
                let sink = sink * sink * (3. - 2. * sink);
                (1. - (t / content_secs).clamp(0., 1.), sink)
            }
        } else {
            (1., 0.)
        };
        // A session cancelled within the enter window would otherwise snap
        // the half-risen pill to rest: the exit continues from the enter's
        // presentation value instead.
        let entered = {
            let t = (self.opened_at.elapsed().as_secs_f32() / ENTER.as_secs_f32()).clamp(0., 1.);
            1. - (1. - t).powi(5)
        };
        let (content_alpha, chip_alpha) = if self.closing && !reduce_motion {
            (content_alpha * entered, (1. - sink) * entered)
        } else {
            (content_alpha, 1. - sink)
        };

        // `display` (not the view) keeps the timer up through a cancel beat.
        let elapsed = self
            .recording_since
            .filter(|_| display == Phase::Recording && error.is_none() && download.is_none());
        let mut content = div()
            .size_full()
            .flex()
            .items_center()
            .px(px(PAD_X))
            .gap(px(CONTENT_GAP))
            .opacity(content_alpha)
            .child(grille)
            .child(
                // Cross-fade the text on phase changes; the changing key
                // restarts the fade.
                div().flex_1().child(status_label(text)).with_animation(
                    (
                        "content-fade",
                        phase_key(display) * 4
                            + u64::from(error.is_some()) * 2
                            + u64::from(download.is_some()),
                    ),
                    Animation::new(CONTENT_FADE).with_easing(ease_out_quint()),
                    |el, delta| el.opacity(delta),
                ),
            );
        if let Some(since) = elapsed {
            content = content.child(time_readout(since));
        }

        // The chip fades via explicit color alphas rather than element
        // opacity: gpui composites opacity per primitive and the hairline
        // border can survive it as a ghost line.
        let fade = |alpha: u32| (alpha as f32 * chip_alpha) as u32;
        let pill = div()
            .absolute()
            .left(px((WINDOW_WIDTH - chip_w) / 2.))
            .w(px(chip_w))
            .h(PILL_HEIGHT)
            .rounded_full()
            .bg(rgba(theme::SURFACE | fade(0xE8)))
            .border_1()
            .border_color(rgba(theme::HAIRLINE | fade(0x22)))
            .shadow(
                aurora_bands
                    .map(|bands| self.aurora_edge(bands, reduce_motion))
                    .unwrap_or_default(),
            )
            .children(aurora_bands.map(|bands| self.aurora_wash(bands, reduce_motion)))
            .child(content);

        // Enter rises in; the staged exit above sinks out along the same path.
        let root = div().size_full().relative();
        if self.closing {
            root.child(pill.top(px(TOP_PAD + TRAVEL * (1. - entered).max(sink))))
        } else {
            root.child(pill.with_animation(
                "enter",
                Animation::new(ENTER).with_easing(ease_out_quint()),
                move |el, delta| el.opacity(delta).top(px(TOP_PAD + TRAVEL * (1.0 - delta))),
            ))
        }
    }
}
