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
use crate::theme;
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Bounds, BoxShadow, Context, Entity,
    IntoElement, ParentElement, Pixels, Render, Styled, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, ease_in_out, ease_out_quint, point,
    pulsating_between, px, rgba, size,
};
use objc2::MainThreadMarker;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::Duration;

const PILL_WIDTH: Pixels = px(220.);
const PILL_HEIGHT: Pixels = px(38.);
/// The orbital level meter's box; the satellites orbit the phase dot inside.
const METER_BOX: f32 = 28.;
const SATELLITES: usize = 16;
/// Trailing characters of the transcript the marquee shapes per frame; long
/// dictations scroll through this window.
const MARQUEE_CHARS: usize = 80;
/// Base marquee crawl speed in px/s; deliberately slower than speech.
const MARQUEE_SPEED: f32 = 24.;
/// Extra crawl speed per px of backlog (per second), so a long pause's burst
/// of text drains gradually instead of stalling the line forever.
const MARQUEE_CATCHUP: f32 = 0.5;
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
    /// Smoothed marquee scroll offset, eased toward its target every frame.
    marquee_offset: f32,
    /// When this session's recording began, for the elapsed-time readout.
    recording_since: Option<std::time::Instant>,
    /// Drives the comet orbit while transcribing/polishing.
    opened_at: std::time::Instant,
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
            marquee_offset: 0.,
            recording_since: None,
            opened_at: std::time::Instant::now(),
        }
    }

    /// The pill's identity element: an orbit of satellites that IS the phase
    /// indicator, with a neon glow that rides the signal level (glow means
    /// live signal; see docs/design.md). Red and voice-driven while
    /// recording; a soft white highlight slowly circling while transcribing;
    /// a magenta ring breathing (contracting and growing) while polishing; a
    /// quiet muted ring otherwise.
    fn orbital_meter(&self, display: Phase, reduce_motion: bool) -> AnyElement {
        let t = self.opened_at.elapsed().as_secs_f32();
        let (base_color, levels): (u32, [f32; SATELLITES]) = match display {
            Phase::Recording => (theme::SIGNAL_RED, *self.levels.lock().unwrap()),
            Phase::Transcribing if reduce_motion => (theme::SIGNAL_WHITE, [0.3; SATELLITES]),
            Phase::Polishing if reduce_motion => (theme::SIGNAL_MAGENTA, [0.3; SATELLITES]),
            Phase::Transcribing => (
                theme::SIGNAL_WHITE,
                std::array::from_fn(|i| {
                    let angle = i as f32 * std::f32::consts::TAU / SATELLITES as f32;
                    0.18 + 0.32 * (0.5 + 0.5 * (angle - t * 1.6).cos()).powi(2)
                }),
            ),
            Phase::Polishing => (
                theme::SIGNAL_MAGENTA,
                [0.15 + 0.3 * (0.5 + 0.5 * (t * 2.).sin()); SATELLITES],
            ),
            _ => (theme::RING_IDLE, [0.; SATELLITES]),
        };
        let center = METER_BOX / 2.;
        let satellites = (0..SATELLITES).map(move |i| {
            let level = levels[i];
            let angle = i as f32 * std::f32::consts::TAU / SATELLITES as f32;
            let radius = 7.5 + level * 5.;
            let size = 2.5;
            let glow = rgba(base_color | (level * 112.) as u32);
            div()
                .absolute()
                .left(px(center + angle.cos() * radius - size / 2.))
                .top(px(center + angle.sin() * radius - size / 2.))
                .size(px(size))
                .rounded_full()
                .bg(rgba(base_color | (0x60 + (level * 159.) as u32)))
                .shadow(vec![
                    BoxShadow::new(px(0.), px(0.), glow.into()).blur_radius(px(4.)),
                ])
        });
        div()
            .relative()
            .size(px(METER_BOX))
            .flex_none()
            .children(satellites)
            .into_any_element()
    }

    /// A single clipped line where recognized words appear left to right and
    /// scroll left once the line fills. The scroll crawls at a slow constant
    /// speed (plus a gentle catch-up when far behind): it conveys that words
    /// are flowing, not real-time position, so freshly recognized text glides
    /// in from the right instead of snapping into view.
    fn marquee(&mut self, text: &str, width: f32, window: &mut Window) -> AnyElement {
        let total = text.chars().count();
        let shown: String = text
            .chars()
            .skip(total.saturating_sub(MARQUEE_CHARS))
            .collect();
        let style = window.text_style();
        let font_size = px(13.);
        let run = style.to_run(shown.len());
        let measured = window
            .text_system()
            .shape_line(shown.clone().into(), font_size, &[run], None)
            .width;
        let target = (f32::from(measured) - width).max(0.);
        let remaining = target - self.marquee_offset;
        if remaining <= 0. {
            self.marquee_offset = target;
        } else {
            let speed = MARQUEE_SPEED + remaining * MARQUEE_CATCHUP;
            let dt = BAR_FRAME.as_secs_f32();
            self.marquee_offset += remaining.min(speed * dt);
        }
        div()
            .relative()
            .w(px(width))
            // Explicit height: h_full inside the auto-sized flex parent
            // resolves circularly to zero and the text vanishes.
            .h(px(22.))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left(px(-self.marquee_offset))
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(font_size)
                    .text_color(rgba(theme::TEXT_PRIMARY | 0xD9))
                    .whitespace_nowrap()
                    .child(shown),
            )
            .into_any_element()
    }
}

/// Single-line pill text; pulses gently while `busy` to signal activity.
fn label(text: String, busy: bool) -> AnyElement {
    let text_el = div()
        .text_sm()
        .text_color(rgba(theme::TEXT_PRIMARY | 0xD9))
        .whitespace_nowrap()
        .overflow_hidden()
        .max_w(px(170.))
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
        }
        // The exit fade freezes the last active content instead of blanking.
        let display = if phase == Phase::Idle {
            self.last_active
        } else {
            phase
        };
        let partial = self.dictation.read(cx).partial.clone();
        if display == Phase::Recording && self.recording_since.is_none() {
            self.recording_since = Some(std::time::Instant::now());
        }
        let elapsed = self.recording_since.filter(|_| display == Phase::Recording);
        let time_width = if elapsed.is_some() { 34. } else { 0. };
        let text_width = f64::from(PILL_WIDTH) as f32 - METER_BOX - 3. * 12. - time_width;
        let content = match display {
            // Words appear as they are recognized and scroll once the line
            // fills; empty until then.
            Phase::Recording => self.marquee(&partial, text_width, window),
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
            .px_3()
            .gap_2()
            .rounded_full()
            .bg(rgba(theme::SURFACE | 0xE8))
            .border_1()
            .border_color(rgba(theme::HAIRLINE | 0x22))
            .child(self.orbital_meter(display, reduce_motion))
            .child(
                // Cross-fade the content on phase changes and on the empty ->
                // words swap; the changing key restarts the fade.
                div().flex_1().child(content).with_animation(
                    (
                        "content-fade",
                        phase_key(display) * 2 + u64::from(!partial.is_empty()),
                    ),
                    Animation::new(CONTENT_FADE).with_easing(ease_out_quint()),
                    |el, delta| el.opacity(delta),
                ),
            );
        let pill = match elapsed {
            Some(since) => pill.child(time_readout(since)),
            None => pill,
        };

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
