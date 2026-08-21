//! Settings window, opened from the menu bar: edit the S1 control line and
//! ASR language (persisted to config.json and applied to the next session),
//! see the hotkey and daemon status. Styled with the Signal theme.

use crate::config::SessionSettings;
use crate::text_input::TextInput;
use crate::{statusbar, theme};
use gpui::{
    App, AppContext, Bounds, Context, Entity, Focusable, MouseButton, ParentElement, Render,
    SharedString, Styled, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div,
    prelude::*, px, rgba, size,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(460.), px(300.));
/// How long the "Saved" confirmation stays visible.
const SAVED_FLASH: Duration = Duration::from_secs(2);

pub struct SettingsWindow {
    settings: Arc<Mutex<SessionSettings>>,
    control_input: Entity<TextInput>,
    language_input: Entity<TextInput>,
    saved_at: Option<Instant>,
}

/// Open the settings window, or surface the existing one.
pub fn open(
    existing: Option<WindowHandle<SettingsWindow>>,
    settings: Arc<Mutex<SessionSettings>>,
    cx: &mut App,
) -> Option<WindowHandle<SettingsWindow>> {
    if let Some(handle) = existing
        && handle.update(cx, |_, _, cx| cx.notify()).is_ok()
    {
        cx.activate(true);
        return Some(handle);
    }
    let bounds = Bounds::centered(None, WINDOW_SIZE, cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Diktafon Settings")),
                    ..Default::default()
                }),
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| SettingsWindow::new(settings, cx));
                let focus = view.read(cx).control_input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                view
            },
        )
        .ok()?;
    cx.activate(true);
    Some(handle)
}

impl SettingsWindow {
    fn new(settings: Arc<Mutex<SessionSettings>>, cx: &mut Context<Self>) -> Self {
        let current = settings.lock().unwrap().clone();
        let control_input = cx.new(|cx| {
            TextInput::new(
                cx,
                "[Styling: ...] [Structure: ...] [Context: ...]",
                &current.control_line,
            )
        });
        let language_input = cx.new(|cx| TextInput::new(cx, "en", &current.language));
        Self {
            settings,
            control_input,
            language_input,
            saved_at: None,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let updated = SessionSettings {
            language: self.language_input.read(cx).text().trim().to_string(),
            control_line: self.control_input.read(cx).text().trim().to_string(),
        };
        if let Err(e) = updated.save() {
            eprintln!("saving settings failed: {e:#}");
            return;
        }
        *self.settings.lock().unwrap() = updated;
        self.saved_at = Some(Instant::now());
        cx.notify();
        // Let the confirmation fade back out.
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(SAVED_FLASH).await;
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn field(label: &str, input: Entity<TextInput>) -> impl gpui::IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(theme::TEXT_DIM | 0xB0))
                    .child(label.to_string()),
            )
            .child(input)
    }

    fn info_row(label: &str, value: String) -> impl gpui::IntoElement {
        div()
            .flex()
            .justify_between()
            .text_size(px(12.))
            .child(
                div()
                    .text_color(rgba(theme::TEXT_DIM | 0xB0))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_color(rgba(theme::TEXT_PRIMARY | 0xD9))
                    .child(value),
            )
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let saved = self.saved_at.is_some_and(|at| at.elapsed() < SAVED_FLASH);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgba(theme::SURFACE | 0xFF))
            .text_color(rgba(theme::TEXT_PRIMARY | 0xF5))
            .child(Self::field(
                "Post-processing prompt (S1 control line)",
                self.control_input.clone(),
            ))
            .child(Self::field("Language", self.language_input.clone()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Self::info_row("Hotkey", "⌥ Space".into()))
                    .child(Self::info_row("Daemon", statusbar::daemon_summary())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .mt_auto()
                    .child(
                        div()
                            .id("save")
                            .px_4()
                            .py_1p5()
                            .rounded_md()
                            .bg(rgba(theme::SIGNAL_RED | 0xCC))
                            .hover(|style| style.bg(rgba(theme::SIGNAL_RED | 0xFF)))
                            .active(|style| style.bg(rgba(theme::SIGNAL_RED | 0xAA)))
                            .cursor_pointer()
                            .text_size(px(13.))
                            .child("Save")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|view, _, _, cx| view.save(cx)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgba(theme::TEXT_DIM | 0xB0))
                            .child(if saved { "Saved" } else { "" }),
                    ),
            )
    }
}
