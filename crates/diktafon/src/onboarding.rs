//! The first-run window: what diktafon needs from macOS before it can
//! dictate, one permission per screen.
//!
//! It exists because the alternative is worse. A permission raised during
//! startup comes from a windowless menu bar app and can open behind whatever
//! the user is looking at, which reads as never having been asked; here every
//! request is a button the user pressed with the window in front of them.
//! Shown once, and only when something is actually missing, so an install
//! that already works never sees it.

use crate::config::SessionSettings;
use crate::permission_ui::{self, RowStyle};
use crate::permissions::{self, MicrophoneAccess};
use crate::theme;
use gpui::{
    App, AppContext, Bounds, Context, ParentElement, Render, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, point, prelude::*, px, rgba, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Root, StyledExt, h_flex, v_flex};
use std::sync::{Arc, Mutex};

const WINDOW_WIDTH: gpui::Pixels = px(520.);
/// How often the window re-reads the grants, so a switch flipped in System
/// Settings moves the flow along without coming back to press anything.
const POLL: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Welcome,
    Microphone,
    Accessibility,
}

impl Step {
    const ALL: [Step; 3] = [Step::Welcome, Step::Microphone, Step::Accessibility];

    fn title(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome to diktafon",
            Step::Microphone => "Allow the microphone",
            Step::Accessibility => "Allow Accessibility",
        }
    }

    fn body(self) -> &'static str {
        match self {
            Step::Welcome => {
                "Diktafon needs two permissions from macOS. Dictation runs on this Mac and nothing is uploaded."
            }
            Step::Microphone => "Diktafon uses the microphone to record your dictation.",
            Step::Accessibility => "Diktafon uses Accessibility to paste text into other apps.",
        }
    }

    /// Whether this step's permission is already in hand, which is what lets
    /// the flow move on by itself.
    fn satisfied(self, status: permissions::Status) -> bool {
        match self {
            Step::Welcome => false,
            Step::Microphone => status.microphone == MicrophoneAccess::Granted,
            Step::Accessibility => status.accessibility,
        }
    }

    /// The window is resized to each step, so no screen carries a void.
    /// A little slack absorbs a body that wraps one line further than
    /// expected, and it lands between the text and the action rather than
    /// under the dots.
    fn height(self) -> gpui::Pixels {
        match self {
            // Mark, title, two lines of body.
            Step::Welcome => px(324.),
            // Title, one line of body, a status row.
            Step::Microphone | Step::Accessibility => px(348.),
        }
    }

    fn next(self) -> Option<Step> {
        match self {
            Step::Welcome => Some(Step::Microphone),
            Step::Microphone => Some(Step::Accessibility),
            Step::Accessibility => None,
        }
    }
}

/// Whether a grant landing should move the flow along without a press. The
/// last step never does: the window would vanish while the user is still
/// coming back from System Settings.
fn advances_itself(step: Step, status: permissions::Status) -> bool {
    step.next().is_some() && step.satisfied(status)
}

/// What the single button on a screen will do. Pure in (step, status), so
/// the wording and the handling can be checked without a window.
fn action_label(step: Step, status: permissions::Status) -> &'static str {
    match step {
        Step::Welcome => "Get started",
        Step::Microphone => match status.microphone {
            MicrophoneAccess::Granted => "Continue",
            MicrophoneAccess::NotAsked => "Allow microphone",
            MicrophoneAccess::Denied => "Open System Settings",
        },
        Step::Accessibility => {
            if status.accessibility {
                "Start dictating"
            } else {
                "Open System Settings"
            }
        }
    }
}

/// Whether the window should open: never twice, and never for an install
/// that already has everything it needs.
///
/// `DIKTAFON_ONBOARDING=1` forces it open, so the flow can be worked on
/// without revoking a real permission to trigger it.
pub fn needed(settings: &SessionSettings, status: permissions::Status) -> bool {
    if std::env::var_os("DIKTAFON_ONBOARDING").is_some() {
        return true;
    }
    !settings.onboarded && !status.all_granted()
}

pub struct Onboarding {
    step: Step,
    status: permissions::Status,
    focus_handle: gpui::FocusHandle,
}

/// Open the window. Shown at most once per install, so there is nothing to
/// reopen and no handle worth keeping.
pub fn open(settings: Arc<Mutex<SessionSettings>>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(WINDOW_WIDTH, Step::Welcome.height()), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Welcome to diktafon")),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        |window, cx| {
            crate::window_lifecycle::release_view_on_close(window, cx);
            crate::settings::force_dark_titlebar(window);
            let view = cx.new(|cx| Onboarding::new(settings, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        },
    );
    if let Err(e) = opened {
        eprintln!("opening the onboarding window failed: {e:#}");
        return;
    }
    cx.activate(true);
}

impl Onboarding {
    fn new(
        settings: Arc<Mutex<SessionSettings>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let status = cx
                    .background_executor()
                    .spawn(async { permissions::Status::read() })
                    .await;
                let updated = view.update_in(cx, |view: &mut Self, window, cx| {
                    view.observed(status, window, cx)
                });
                if updated.is_err() {
                    return;
                }
            }
        })
        .detach();

        // Written when the window goes away, however it goes away, so a flow
        // abandoned halfway still counts as seen without the flag landing
        // before the user has touched anything.
        cx.on_release({
            let settings = settings.clone();
            move |_, _| Self::remember(&settings)
        })
        .detach();

        Self {
            step: Step::Welcome,
            status: permissions::Status::read(),
            focus_handle,
        }
    }

    fn remember(settings: &Arc<Mutex<SessionSettings>>) {
        let saved = {
            let mut settings = settings.lock().unwrap();
            if settings.onboarded {
                return;
            }
            settings.onboarded = true;
            settings.clone()
        };
        if let Err(e) = saved.save() {
            eprintln!("recording that onboarding was seen failed: {e:#}");
        }
    }

    /// A grant can land while the user is in System Settings, so a step
    /// moves on by itself rather than waiting for a press. The last step is
    /// the exception: closing the window the moment the switch flips would
    /// leave the user coming back to nothing, with no sign it worked.
    fn observed(
        &mut self,
        status: permissions::Status,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.status == status {
            return;
        }
        self.status = status;
        if advances_itself(self.step, status) {
            self.advance(window, cx);
        }
        cx.notify();
    }

    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step.next() {
            Some(step) => self.step = step,
            None => {
                window.remove_window();
                return;
            }
        }
        window.resize(size(WINDOW_WIDTH, self.step.height()));
        cx.notify();
    }

    /// The one action on the screen: move on, ask macOS, or send the user to
    /// the switch only macOS can offer.
    fn act(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Welcome => self.advance(window, cx),
            Step::Microphone => match self.status.microphone {
                MicrophoneAccess::Granted => self.advance(window, cx),
                MicrophoneAccess::NotAsked => permissions::request_microphone(),
                MicrophoneAccess::Denied => {
                    permissions::open_privacy_pane(permissions::PrivacyPane::Microphone)
                }
            },
            Step::Accessibility => {
                if self.status.accessibility {
                    self.advance(window, cx);
                } else {
                    permissions::request_accessibility();
                }
            }
        }
    }

    /// Position, not decoration: the accent stays on the action.
    fn dots(&self) -> impl IntoElement + use<> {
        let current = self.step;
        h_flex()
            .gap_2()
            .justify_center()
            .mt_1()
            .children(Step::ALL.map(|step| {
                let here = step == current;
                let colour = if here {
                    gpui::Hsla::from(rgba(theme::TEXT_PRIMARY | 0xFF))
                } else {
                    gpui::Hsla::from(rgba(theme::RING_IDLE | 0xFF)).opacity(0.4)
                };
                div().size(px(8.)).rounded_full().bg(colour)
            }))
    }
}

impl Render for Onboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step;
        let row = match step {
            Step::Welcome => None,
            Step::Microphone => Some(permission_ui::microphone_row(
                self.status,
                RowStyle::Compact,
            )),
            Step::Accessibility => Some(permission_ui::accessibility_row(
                self.status,
                RowStyle::Compact,
            )),
        };
        v_flex()
            .size_full()
            .justify_between()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            // Clears the traffic lights floating over the content.
            .pt(px(48.))
            .px_8()
            .pb_8()
            .child(
                v_flex()
                    .gap_4()
                    .when(step == Step::Welcome, |el| {
                        el.child(
                            div()
                                .mb_2()
                                .child(gpui::img(crate::assets::MARK_FLAT).w(px(64.)).h(px(36.))),
                        )
                    })
                    .child(
                        div()
                            .font_family(theme::FONT_DISPLAY)
                            .text_2xl()
                            .font_semibold()
                            .child(step.title()),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(step.body()),
                    )
                    .children(row.map(|row| permission_ui::permission_list_of(vec![row], cx))),
            )
            .child(
                v_flex()
                    .gap_4()
                    .child(
                        Button::new("onboarding-action")
                            .label(action_label(self.step, self.status))
                            .primary()
                            .h(px(40.))
                            .w_full()
                            .on_click(cx.listener(|view, _, window, cx| view.act(window, cx))),
                    )
                    .child(self.dots()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(microphone: MicrophoneAccess, accessibility: bool) -> permissions::Status {
        permissions::Status {
            microphone,
            accessibility,
        }
    }

    fn settings(onboarded: bool) -> SessionSettings {
        SessionSettings {
            onboarded,
            ..Default::default()
        }
    }

    #[test]
    fn shown_once_and_only_when_something_is_missing() {
        let missing = status(MicrophoneAccess::NotAsked, false);
        let ready = status(MicrophoneAccess::Granted, true);
        // A fresh install with work to do.
        assert!(needed(&settings(false), missing));
        // An existing install whose permissions broke still gets it once.
        assert!(needed(
            &settings(false),
            status(MicrophoneAccess::Denied, true)
        ));
        // Nothing to fix, and never again once seen.
        assert!(!needed(&settings(false), ready));
        assert!(!needed(&settings(true), missing));
    }

    #[test]
    fn a_step_is_satisfied_only_by_its_own_grant() {
        let mic_only = status(MicrophoneAccess::Granted, false);
        assert!(Step::Microphone.satisfied(mic_only));
        assert!(!Step::Accessibility.satisfied(mic_only));
        // The welcome screen is never satisfied, so it never self-advances.
        assert!(!Step::Welcome.satisfied(status(MicrophoneAccess::Granted, true)));
    }

    #[test]
    fn only_the_last_step_waits_for_a_press() {
        let all = status(MicrophoneAccess::Granted, true);
        // A grant mid-flow carries the user forward.
        assert!(advances_itself(Step::Microphone, all));
        // The welcome screen has nothing to satisfy it.
        assert!(!advances_itself(Step::Welcome, all));
        // The last screen stays up so the user sees that it worked.
        assert!(!advances_itself(Step::Accessibility, all));
    }

    #[test]
    fn the_flow_ends_after_accessibility() {
        assert_eq!(Step::Welcome.next(), Some(Step::Microphone));
        assert_eq!(Step::Microphone.next(), Some(Step::Accessibility));
        assert_eq!(Step::Accessibility.next(), None);
    }

    #[test]
    fn the_button_names_what_it_will_do() {
        let blank = status(MicrophoneAccess::NotAsked, false);
        assert_eq!(action_label(Step::Welcome, blank), "Get started");
        assert_eq!(action_label(Step::Microphone, blank), "Allow microphone");
        // Denied is past asking: only System Settings can undo it.
        assert_eq!(
            action_label(Step::Microphone, status(MicrophoneAccess::Denied, false)),
            "Open System Settings"
        );
        assert_eq!(
            action_label(Step::Microphone, status(MicrophoneAccess::Granted, false)),
            "Continue"
        );
        assert_eq!(
            action_label(Step::Accessibility, status(MicrophoneAccess::Granted, true)),
            "Start dictating"
        );
    }
}
