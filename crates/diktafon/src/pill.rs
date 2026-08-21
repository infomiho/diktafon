//! The floating recording pill: a small always-on-top window that appears
//! while a session is active. GPUI's `WindowKind::PopUp` already provides the
//! overlay mechanics (non-activating panel, above-normal level, joins all
//! Spaces, shows over fullscreen apps).
//!
//! Motion follows the platform rules: the pill enters and exits along the same
//! path (rises in, sinks out, exits softer than enters), the recording dot
//! breathes only while audio is actually being captured, and phase content
//! cross-fades. Under Reduce Motion, gpui snaps animations to their end state,
//! so the pill appears and disappears instantly; only the repeating breath
//! needs an explicit gate.

use crate::capture::LevelBars;
use crate::dictation::{Dictation, Phase};
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Bounds, Context, Entity, IntoElement,
    ParentElement, Pixels, Render, Styled, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, ease_in_out, ease_out_quint, point,
    pulsating_between, px, rgb, rgba, size,
};
use objc2::MainThreadMarker;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::Duration;

const PILL_WIDTH: Pixels = px(256.);
const PILL_HEIGHT: Pixels = px(46.);
/// Vertical slide distance of the enter/exit transition.
const TRAVEL: f32 = 8.;
const TOP_PAD: f32 = 0.;
/// Gap between the pill's resting position and the bottom of the visible
/// frame (above the Dock).
const BOTTOM_MARGIN: f64 = 15.;

const ENTER: Duration = Duration::from_millis(200);
/// Exits are softer than enters.
const EXIT: Duration = Duration::from_millis(300);
const CONTENT_FADE: Duration = Duration::from_millis(150);
const DOT_BREATH: Duration = Duration::from_millis(1100);

/// Open the pill while a session is active; on idle, play the exit animation
/// and then close it.
pub fn manage(cx: &mut App, dictation: Entity<Dictation>, levels: LevelBars) {
    let mut open: Option<WindowHandle<Pill>> = None;
    cx.observe(&dictation, move |dictation, cx| {
        let idle = dictation.read(cx).phase == Phase::Idle;
        match (&open, idle) {
            (Some(handle), true) => {
                let handle = *handle;
                let _ = handle.update(cx, |pill, _, cx| {
                    pill.closing = true;
                    cx.notify();
                });
                // A session starting during the fade opens a fresh pill; the
                // fading one is gone within EXIT, so the overlap is brief.
                open = None;
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(EXIT + Duration::from_millis(40))
                        .await;
                    cx.update(|cx| {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    });
                })
                .detach();
            }
            (None, false) => open = open_pill(&dictation, levels.clone(), cx),
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
            disable_window_shadow(window);
            cx.new(|cx| Pill::new(dictation, levels, cx))
        },
    )
    .ok()
}

/// macOS outlines the whole (mostly transparent) window rect with its system
/// shadow, which reads as a ghost rectangle around the pill; turn it off.
fn disable_window_shadow(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        let ns_view = appkit.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
        unsafe {
            let ns_window: *mut objc2::runtime::AnyObject = objc2::msg_send![&*ns_view, window];
            let _: () = objc2::msg_send![&*ns_window, setHasShadow: false];
        }
    }
}

/// Bottom-centered on the screen containing the cursor, tracking the Dock via
/// the visible frame. AppKit reports bottom-left-origin coordinates; GPUI
/// wants top-left-origin relative to the primary screen. The window carries
/// transparent padding around the pill for its slide travel and shadow.
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
    let window_width = f64::from(PILL_WIDTH);
    let window_height = f64::from(TOP_PAD) + f64::from(PILL_HEIGHT) + bottom_pad;
    let x = visible.origin.x + (visible.size.width - window_width) / 2.;
    let window_bottom = visible.origin.y + BOTTOM_MARGIN - bottom_pad;
    let y = primary_height - (window_bottom + window_height);
    Some(Bounds {
        origin: point(px(x as f32), px(y as f32)),
        size: size(px(window_width as f32), px(window_height as f32)),
    })
}

/// Repaint cadence for the live level bars; matches the meter's update rate.
const BAR_FRAME: Duration = Duration::from_millis(33);

pub struct Pill {
    dictation: Entity<Dictation>,
    levels: LevelBars,
    closing: bool,
    /// What the fading pill keeps showing after the phase already went Idle.
    last_active: Phase,
}

impl Pill {
    fn new(dictation: Entity<Dictation>, levels: LevelBars, cx: &mut Context<Self>) -> Self {
        cx.observe(&dictation, |_, _, cx| cx.notify()).detach();
        // Drive repaints for the pill's whole (short) life: this non-activating
        // panel gets no frames on its own, so every animation - bars, fades,
        // the dot breath - advances only when we notify. Ends when the window
        // removes the entity.
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
            last_active: Phase::Recording,
        }
    }

    fn bars(&self) -> AnyElement {
        let levels = *self.levels.lock().unwrap();
        let (width, gap) = (5., 4.);
        div()
            .flex()
            .items_center()
            .gap(px(gap))
            .h(px(30.))
            .children(levels.into_iter().map(|level| {
                div()
                    .w(px(width))
                    .h(px(4. + level * 26.))
                    .rounded_full()
                    .bg(rgba(0xFFFFFFCC))
            }))
            .into_any_element()
    }
}

/// Single-line pill text; pulses gently while `busy` to signal activity.
fn label(text: String, busy: bool) -> AnyElement {
    let text_el = div()
        .text_sm()
        .text_color(rgba(0xFFFFFFD9))
        .whitespace_nowrap()
        .overflow_hidden()
        .max_w(px(200.))
        .child(text);
    if busy {
        text_el
            .with_animation(
                "label-pulse",
                Animation::new(DOT_BREATH)
                    .repeat()
                    .with_easing(pulsating_between(0.5, 1.0)),
                |el, level| el.opacity(level),
            )
            .into_any_element()
    } else {
        text_el.into_any_element()
    }
}

fn dot(phase: Phase, breathing: bool) -> AnyElement {
    let color = match phase {
        Phase::Arming => rgb(0x8F8F94),
        Phase::Recording => rgb(0xE5484D),
        Phase::Transcribing => rgb(0xF0B429),
        Phase::Polishing => rgb(0x46A758),
        Phase::Idle => rgb(0x8F8F94),
    };
    let dot = div().size_2().rounded_full().bg(color);
    // The breath means "live microphone"; processing phases hold steady.
    if breathing {
        dot.with_animation(
            "dot-breath",
            Animation::new(DOT_BREATH)
                .repeat()
                .with_easing(pulsating_between(0.4, 1.0)),
            |dot, level| dot.opacity(level),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

/// Deliberately not `phase as u64`: these keys are animation element ids and
/// must stay stable even if the enum is ever reordered.
/// The last `chars` characters, front-ellipsized.
fn tail(text: &str, chars: usize) -> String {
    let total = text.chars().count();
    if total <= chars {
        return text.to_string();
    }
    let tail: String = text.chars().skip(total - chars).collect();
    format!("…{tail}")
}

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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reduce_motion = cx.reduce_motion();
        let phase = self.dictation.read(cx).phase;
        if phase != Phase::Idle {
            self.last_active = phase;
        }
        // The exit fade freezes the last active content instead of blanking.
        let display = if phase == Phase::Idle {
            self.last_active
        } else {
            phase
        };
        let partial = self.dictation.read(cx).partial.clone();
        let content = match display {
            // Bars until the first words arrive, then the words rolling in.
            Phase::Recording if partial.is_empty() => self.bars(),
            Phase::Recording => label(tail(&partial, 26), false),
            Phase::Arming => label("starting".into(), false),
            // The words themselves are about to be pasted; the label just has
            // to feel alive, so it pulses.
            Phase::Transcribing => label("transcribing".into(), !reduce_motion),
            Phase::Polishing => label("polishing".into(), !reduce_motion),
            Phase::Idle => label(String::new(), false),
        };

        let pill = div()
            .absolute()
            .left_0()
            .w(PILL_WIDTH)
            .h(PILL_HEIGHT)
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded_full()
            .bg(rgba(0x16161AE8))
            .border_1()
            .border_color(rgba(0xFFFFFF14))
            .child(dot(display, phase == Phase::Recording && !reduce_motion))
            .child(
                // Cross-fade the content on phase changes and on the bars ->
                // words swap; the changing key restarts the fade.
                div().child(content).with_animation(
                    (
                        "content-fade",
                        phase_key(display) * 2 + u64::from(!partial.is_empty()),
                    ),
                    Animation::new(CONTENT_FADE).with_easing(ease_out_quint()),
                    |el, delta| el.opacity(delta),
                ),
            );

        // Enter rises in; exit sinks out along the same path, softer.
        let root = div().size_full().relative();
        if self.closing {
            root.child(pill.with_animation(
                "exit",
                Animation::new(EXIT).with_easing(ease_in_out),
                move |el, delta| el.opacity(1.0 - delta).top(px(TOP_PAD + TRAVEL * delta)),
            ))
        } else {
            root.child(pill.with_animation(
                "enter",
                Animation::new(ENTER).with_easing(ease_out_quint()),
                move |el, delta| el.opacity(delta).top(px(TOP_PAD + TRAVEL * (1.0 - delta))),
            ))
        }
    }
}
