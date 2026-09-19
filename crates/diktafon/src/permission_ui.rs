//! The permission rows shared by the onboarding window and the Permissions
//! sheet, so the two can never describe the same grant differently.

use crate::permissions::{self, MicrophoneAccess, PrivacyPane};
use crate::theme;
use gpui::{App, ParentElement, Styled, div, prelude::*, px, rgba};
use gpui_component::button::Button;
use gpui_component::label::Label;
use gpui_component::{ActiveTheme, StyledExt, h_flex, v_flex};

/// One height for every control in a window; mirrors settings.
const CONTROL_HEIGHT: gpui::Pixels = px(40.);

/// One permission as the UI shows it.
pub struct PermissionRow {
    id: &'static str,
    name: &'static str,
    purpose: &'static str,
    state: &'static str,
    dot: Dot,
    action: Option<(&'static str, Action)>,
    style: RowStyle,
}

#[derive(Clone, Copy)]
enum Dot {
    Granted,
    Pending,
    Missing,
}

/// What a permission row's button does.
#[derive(Clone, Copy)]
enum Action {
    RequestMicrophone,
    OpenMicrophonePane,
    RequestAccessibility,
}

impl Action {
    fn run(self) {
        match self {
            Action::RequestMicrophone => permissions::request_microphone(),
            Action::OpenMicrophonePane => permissions::open_privacy_pane(PrivacyPane::Microphone),
            Action::RequestAccessibility => permissions::request_accessibility(),
        }
    }
}

/// How much of a row to show. Onboarding's screen already names the
/// permission and carries the only button, so its rows report status and
/// nothing else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowStyle {
    /// Name, badge, purpose, and the action that fixes it.
    Full,
    /// Name and badge.
    Compact,
}

impl RowStyle {
    fn full(self) -> bool {
        self == RowStyle::Full
    }
}

pub fn microphone_row(status: permissions::Status, style: RowStyle) -> PermissionRow {
    let (state, dot, action) = match status.microphone {
        MicrophoneAccess::Granted => ("Granted", Dot::Granted, None),
        MicrophoneAccess::NotAsked => (
            "Not asked",
            Dot::Pending,
            Some(("Request access", Action::RequestMicrophone)),
        ),
        MicrophoneAccess::Denied => (
            "Denied",
            Dot::Missing,
            Some(("Open System Settings", Action::OpenMicrophonePane)),
        ),
    };
    PermissionRow {
        id: "permission-microphone",
        name: "Microphone",
        purpose: "Records your voice while you dictate.",
        state,
        dot,
        action: action.filter(|_| style.full()),
        style,
    }
}

pub fn accessibility_row(status: permissions::Status, style: RowStyle) -> PermissionRow {
    PermissionRow {
        id: "permission-accessibility",
        name: "Accessibility",
        purpose: "Pastes the text where you are typing.",
        state: if status.accessibility {
            "Granted"
        } else {
            "Not granted"
        },
        dot: if status.accessibility {
            Dot::Granted
        } else {
            Dot::Missing
        },
        action: (!status.accessibility && style.full())
            .then_some(("Request access", Action::RequestAccessibility)),
        style,
    }
}

/// Both permissions, as the Settings sheet shows them.
pub fn permission_list(status: permissions::Status, cx: &App) -> impl IntoElement {
    permission_list_of(
        vec![
            microphone_row(status, RowStyle::Full),
            accessibility_row(status, RowStyle::Full),
        ],
        cx,
    )
}

/// One bordered group; rows after the first carry the divider.
pub fn permission_list_of(rows: Vec<PermissionRow>, cx: &App) -> impl IntoElement {
    v_flex()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .overflow_hidden()
        .children(
            rows.into_iter()
                .enumerate()
                .map(|(index, row)| permission_row(row, index > 0, cx)),
        )
}

fn permission_row(row: PermissionRow, divided: bool, cx: &App) -> impl IntoElement {
    let color = match row.dot {
        Dot::Granted => theme::SIGNAL_GREEN,
        Dot::Pending => theme::RING_IDLE,
        Dot::Missing => theme::SIGNAL_RED,
    };
    let badge = h_flex()
        .h(px(24.))
        .px_2()
        .gap_2()
        .items_center()
        .rounded_full()
        .bg(rgba(theme::SURFACE_RAISED | 0xFF))
        .border_1()
        .border_color(cx.theme().border)
        .text_sm()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(div().size(px(8.)).rounded_full().bg(rgba(color | 0xFF)))
        .child(row.state);
    h_flex()
        .items_center()
        .justify_between()
        .gap_6()
        .px_4()
        .py_3()
        .when(divided, |el| {
            el.border_t_1().border_color(cx.theme().border)
        })
        .child(
            v_flex()
                .gap_1()
                .min_w_0()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(Label::new(row.name).font_medium())
                        .child(badge),
                )
                .when(row.style.full(), |text| {
                    text.child(
                        Label::new(row.purpose)
                            .text_sm()
                            .text_color(cx.theme().muted_foreground),
                    )
                }),
        )
        .when_some(row.action, |el, (label, action)| {
            el.child(
                div().flex_shrink_0().child(
                    Button::new(row.id)
                        .label(label)
                        .outline()
                        .h(CONTROL_HEIGHT)
                        .on_click(move |_, _, _| action.run()),
                ),
            )
        })
}
