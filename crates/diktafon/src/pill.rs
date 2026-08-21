//! The floating recording pill: a small always-on-top window that appears
//! while a session is active. GPUI's `WindowKind::PopUp` already provides the
//! overlay mechanics (non-activating panel, above-normal level, joins all
//! Spaces, shows over fullscreen apps).

use crate::dictation::{Dictation, Phase};
use gpui::{
    div, point, px, rgb, rgba, size, App, AppContext, Bounds, Context, Entity, IntoElement,
    ParentElement, Pixels, Render, Styled, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions,
};
use objc2::MainThreadMarker;

const PILL_WIDTH: Pixels = px(256.);
const PILL_HEIGHT: Pixels = px(46.);
/// Gap between the pill and the bottom of the visible frame (above the Dock).
const BOTTOM_MARGIN: f64 = 15.;

/// Open the pill while a session is active, close it when the pipeline goes
/// idle.
pub fn manage(cx: &mut App, dictation: Entity<Dictation>) {
    let mut open: Option<WindowHandle<Pill>> = None;
    cx.observe(&dictation, move |dictation, cx| {
        let idle = dictation.read(cx).phase == Phase::Idle;
        match (&open, idle) {
            (Some(handle), true) => {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                open = None;
            }
            (None, false) => open = open_pill(&dictation, cx),
            _ => {}
        }
    })
    .detach();
}

fn open_pill(dictation: &Entity<Dictation>, cx: &mut App) -> Option<WindowHandle<Pill>> {
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
        |_, cx| cx.new(|cx| Pill::new(dictation, cx)),
    )
    .ok()
}

/// Bottom-centered on the screen containing the cursor, tracking the Dock via
/// the visible frame. AppKit reports bottom-left-origin coordinates; GPUI
/// wants top-left-origin relative to the primary screen.
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

    let x = visible.origin.x + (visible.size.width - f64::from(PILL_WIDTH)) / 2.;
    let bottom = visible.origin.y + BOTTOM_MARGIN;
    let y = primary_height - (bottom + f64::from(PILL_HEIGHT));
    Some(Bounds {
        origin: point(px(x as f32), px(y as f32)),
        size: size(PILL_WIDTH, PILL_HEIGHT),
    })
}

pub struct Pill {
    dictation: Entity<Dictation>,
}

impl Pill {
    fn new(dictation: Entity<Dictation>, cx: &mut Context<Self>) -> Self {
        cx.observe(&dictation, |_, _, cx| cx.notify()).detach();
        Self { dictation }
    }
}

impl Render for Pill {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = self.dictation.read(cx).phase;
        let (dot_color, label) = match phase {
            Phase::Recording => (rgb(0xE5484D), "recording"),
            Phase::Transcribing => (rgb(0xF0B429), "transcribing"),
            Phase::Polishing => (rgb(0x46A758), "polishing"),
            Phase::Idle => (rgb(0x8F8F94), ""),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded_full()
            .bg(rgba(0x16161AE8))
            .border_1()
            .border_color(rgba(0xFFFFFF14))
            .child(div().size_2().rounded_full().bg(dot_color))
            .child(
                div()
                    .text_sm()
                    .text_color(rgba(0xFFFFFFD9))
                    .child(label),
            )
    }
}
