//! The floating recording pill: a small always-on-top window that appears
//! while a session is active. GPUI's `WindowKind::PopUp` already provides the
//! overlay mechanics (non-activating panel, above-normal level, joins all
//! Spaces, shows over fullscreen apps).
//!
//! Motion follows the platform rules: the pill rises in; on exit its
//! contents fade out in place and then the empty chip sinks away along the
//! entry path. Phase content cross-fades. Under Reduce Motion, gpui snaps animations to their end state,
//! so the pill appears and disappears instantly; only the repeating breath
//! needs an explicit gate.

use crate::capture::LevelBars;
use crate::dictation::{Dictation, Outcome, Phase};
use crate::theme;
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Bounds, BoxShadow, Context, Entity,
    IntoElement, ParentElement, Pixels, Render, Styled, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, ease_out_quint, point,
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
/// Vertical slide distance of the enter/exit transition.
const TRAVEL: f32 = 8.;
const TOP_PAD: f32 = 0.;
/// Gap between the pill's resting position and the bottom of the visible
/// frame (above the Dock).
const BOTTOM_MARGIN: f64 = 15.;

const ENTER: Duration = Duration::from_millis(200);
/// The exit is staged: first the pill's contents fade out in place, then the
/// empty chip sinks away. EXIT is the total; manage() times removal off it.
const EXIT_CONTENT: Duration = Duration::from_millis(150);
const EXIT: Duration = Duration::from_millis(400);
const CONTENT_FADE: Duration = Duration::from_millis(150);
const DOT_BREATH: Duration = Duration::from_millis(1100);
/// Aurora glow floor while recording: the wash never goes fully dark, so
/// quiet moments read as embers rather than the effect blinking off.
const AURORA_BASELINE: f32 = 0.22;
/// How long a failed session's error stays readable before the fade.
const ERROR_HOLD: Duration = Duration::from_millis(2400);
/// Length of the success bloom the orbit plays while the pill fades.
const BLOOM: Duration = Duration::from_millis(350);

/// Open the pill while a session is active; on idle, play the exit animation
/// and then close it.
pub fn manage(cx: &mut App, dictation: Entity<Dictation>, levels: LevelBars) {
    let mut open: Option<WindowHandle<Pill>> = None;
    // A pill lingering through its error hold; a new session would otherwise
    // open a second pill at the same spot on top of it.
    let held: std::rc::Rc<std::cell::Cell<Option<WindowHandle<Pill>>>> = Default::default();
    cx.observe(&dictation, move |dictation, cx| {
        let idle = dictation.read(cx).phase == Phase::Idle;
        match (&open, idle) {
            (Some(handle), true) => {
                let handle = *handle;
                // A failed session lingers so the error is readable; anything
                // else fades right away.
                let failed = matches!(dictation.read(cx).outcome, Some(Outcome::Failed(_)));
                let hold = if failed { ERROR_HOLD } else { Duration::ZERO };
                if failed {
                    held.set(Some(handle));
                }
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

/// macOS outlines the whole (mostly transparent) window rect with its system
/// shadow, which reads as a ghost rectangle around the pill; turn it off. The
/// pill is also a passive overlay, so let clicks pass through to whatever is
/// underneath it.
fn configure_overlay_window(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        let ns_view = appkit.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
        unsafe {
            let ns_window: *mut objc2::runtime::AnyObject = objc2::msg_send![&*ns_view, window];
            let _: () = objc2::msg_send![&*ns_window, setHasShadow: false];
            let _: () = objc2::msg_send![&*ns_window, setIgnoresMouseEvents: true];
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

/// What the orbit should show this frame; ending beats outrank the phase.
enum OrbitView {
    Phase(Phase),
    Error,
    Bloom,
    Quiet,
}

pub struct Pill {
    dictation: Entity<Dictation>,
    levels: LevelBars,
    closing: bool,
    /// When the exit fade began; times the success bloom.
    closing_since: Option<std::time::Instant>,
    /// What the fading pill keeps showing after the phase already went Idle.
    last_active: Phase,
    /// When this session's recording began, for the elapsed-time readout.
    recording_since: Option<std::time::Instant>,
    /// Drives the comet orbit while transcribing/polishing.
    opened_at: std::time::Instant,
    /// Per-band smoothed aurora intensity (fast attack, slow decay), so the
    /// glow breathes with speech instead of flickering with the raw meter.
    aurora_smooth: [f32; 3],
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
            closing_since: None,
            last_active: Phase::Recording,
            recording_since: None,
            opened_at: std::time::Instant::now(),
            aurora_smooth: [0.; 3],
        }
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
    /// Siri-style. Three hue-shifted blobs
    /// (ember, red, rose; lows left, highs right) drift inside the pill over
    /// a constant baseline glow, so the wash breathes with speech instead of
    /// flashing from black.
    fn aurora_wash(&self, bands: [f32; 3], reduce_motion: bool) -> AnyElement {
        let t = self.opened_at.elapsed().as_secs_f32();
        let blobs = [
            (40., bands[0], theme::AURORA_EMBER, 0.),
            (110., bands[1], theme::SIGNAL_RED, 2.1),
            (180., bands[2], theme::AURORA_ROSE, 4.2),
        ];
        let mid = f64::from(PILL_HEIGHT) as f32 / 2.;
        div()
            .absolute()
            .inset_0()
            .rounded_full()
            .overflow_hidden()
            .children(blobs.into_iter().map(|(x, level, hue, seed)| {
                let drift = if reduce_motion {
                    0.
                } else {
                    (t * 0.4 + seed).sin() * 16.
                };
                let intensity = (AURORA_BASELINE + level).min(1.);
                let glow = rgba(hue | (intensity * 170.) as u32);
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
                    ])
            }))
            .into_any_element()
    }

    /// The Siri move: layered inset glows hugging the capsule's edge, in two
    /// hues whose dominance slowly trades places so the colors read as moving
    /// around the border. Brightness rides the overall voice level.
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

    /// The pill's identity element: an orbit of satellites that IS the phase
    /// indicator, with a neon glow that rides the signal level (glow means
    /// live signal; see docs/design.md). Red and voice-driven while
    /// recording; a soft white highlight slowly circling while transcribing;
    /// a magenta ring breathing (contracting and growing) while polishing; a
    /// quiet muted ring otherwise. Session endings get their own beats: a
    /// steady red ring for an error, a white bloom for a paste, quiet for a
    /// cancel.
    fn orbital_meter(&self, view: OrbitView, reduce_motion: bool) -> AnyElement {
        let t = self.opened_at.elapsed().as_secs_f32();
        let display = match view {
            OrbitView::Error => {
                return self.satellite_ring(theme::SIGNAL_RED, [0.35; SATELLITES]);
            }
            OrbitView::Quiet => {
                return self.satellite_ring(theme::RING_IDLE, [0.; SATELLITES]);
            }
            OrbitView::Bloom => {
                let fade = self
                    .closing_since
                    .map(|since| since.elapsed().as_secs_f32() / BLOOM.as_secs_f32())
                    .unwrap_or(1.);
                let level = (1. - fade).max(0.) * 0.7;
                return self.satellite_ring(theme::SIGNAL_WHITE, [level; SATELLITES]);
            }
            OrbitView::Phase(phase) => phase,
        };
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
        self.satellite_ring(base_color, levels)
    }

    fn satellite_ring(&self, base_color: u32, levels: [f32; SATELLITES]) -> AnyElement {
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
}

/// Single-line pill text; pulses gently while `busy` to signal activity.
fn label(text: String, busy: bool) -> AnyElement {
    let text_el = div()
        .font_family(theme::FONT_UI)
        .text_sm()
        .text_color(rgba(theme::TEXT_PRIMARY | 0xF5))
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
        .font_family(theme::FONT_UI)
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
        if display == Phase::Recording && self.recording_since.is_none() {
            self.recording_since = Some(std::time::Instant::now());
        }
        let aurora_bands =
            (display == Phase::Recording && !self.closing).then(|| self.aurora_bands());
        let elapsed = self.recording_since.filter(|_| display == Phase::Recording);
        let outcome = self.dictation.read(cx).outcome.clone();
        let error = match (&outcome, phase) {
            (Some(Outcome::Failed(message)), Phase::Idle) => Some(message.clone()),
            _ => None,
        };
        let orbit = if error.is_some() {
            OrbitView::Error
        } else if self.closing {
            match outcome {
                Some(Outcome::Pasted) => OrbitView::Bloom,
                Some(Outcome::Cancelled) => OrbitView::Quiet,
                _ => OrbitView::Phase(display),
            }
        } else {
            OrbitView::Phase(display)
        };
        let download = self.dictation.read(cx).download.clone();
        let content = match &error {
            Some(message) => label(message.clone(), false),
            // A first-run model download outranks the session content: the
            // session is stalled on it and would otherwise look like a hang.
            None if download.is_some() => {
                let percent = download.as_ref().expect("guarded above").percent;
                label(format!("Downloading models {percent}%"), !reduce_motion)
            }
            None => match display {
                // The orbit carries the liveness while recording; the label
                // just states what the pill is doing.
                Phase::Recording => label("Listening".into(), false),
                Phase::Arming => label("Starting".into(), false),
                // The words themselves are about to be pasted; the label just
                // has to feel alive, so it pulses.
                Phase::Transcribing => label("Transcribing".into(), !reduce_motion),
                Phase::Polishing => label("Polishing".into(), !reduce_motion),
                Phase::Idle => label(String::new(), false),
            },
        };

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

        let contents = div()
            .size_full()
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .opacity(content_alpha)
            .child(self.orbital_meter(orbit, reduce_motion))
            .child(
                // Cross-fade the content on phase changes; the changing key
                // restarts the fade.
                div().flex_1().child(content).with_animation(
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
        let contents = match elapsed {
            Some(since) => contents.child(time_readout(since)),
            None => contents,
        };

        // The chip fades via explicit color alphas rather than element
        // opacity: gpui composites opacity per primitive and the hairline
        // border can survive it as a ghost line.
        let chip = 1. - sink;
        let fade = |alpha: u32| (alpha as f32 * chip) as u32;
        let pill = div()
            .absolute()
            .left_0()
            .w(PILL_WIDTH)
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
            .child(contents);

        // Enter rises in; the staged exit above sinks out along the same path.
        let root = div().size_full().relative();
        if self.closing {
            root.child(pill.top(px(TOP_PAD + TRAVEL * sink)))
        } else {
            root.child(pill.with_animation(
                "enter",
                Animation::new(ENTER).with_easing(ease_out_quint()),
                move |el, delta| el.opacity(delta).top(px(TOP_PAD + TRAVEL * (1.0 - delta))),
            ))
        }
    }
}
