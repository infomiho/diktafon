//! Settings window, opened from the menu bar: edit the S1 control line and
//! ASR language (persisted to config.json and applied to the next session),
//! see the hotkey and daemon status. Built from gpui-component's stock
//! widgets so it follows the system look in light and dark.

use crate::config::SessionSettings;
use crate::statusbar;
use gpui::{
    App, AppContext, Bounds, Context, Entity, ParentElement, Render, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowHandle, WindowOptions, prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::form::{field, v_form};
use gpui_component::group_box::GroupBox;
use gpui_component::input::{Input, InputState};
use gpui_component::label::Label;
use gpui_component::{ActiveTheme, Root, h_flex, v_flex};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(520.), px(420.));
/// How long the "Saved" confirmation stays visible.
const SAVED_FLASH: Duration = Duration::from_secs(2);

pub struct SettingsWindow {
    settings: Arc<Mutex<SessionSettings>>,
    control_input: Entity<InputState>,
    language_input: Entity<InputState>,
    /// Cached at open: reading it does file IO and must not run per render.
    daemon_summary: SharedString,
    saved_at: Option<Instant>,
}

/// Open the settings window, or bring the existing one to the front.
pub fn open(
    existing: Option<WindowHandle<Root>>,
    settings: Arc<Mutex<SessionSettings>>,
    cx: &mut App,
) -> Option<WindowHandle<Root>> {
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
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
                let view = cx.new(|cx| SettingsWindow::new(settings, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .ok()?;
    cx.activate(true);
    Some(handle)
}

impl SettingsWindow {
    fn new(
        settings: Arc<Mutex<SessionSettings>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = settings.lock().unwrap().clone();
        let control_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("[Styling: ...] [Structure: ...] [Context: ...]")
                .default_value(current.control_line.clone())
        });
        let language_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("en")
                .default_value(current.language.clone())
        });
        Self {
            settings,
            control_input,
            language_input,
            daemon_summary: statusbar::daemon_summary().into(),
            saved_at: None,
        }
    }

    /// An emptied field falls back to its default: an empty control line or
    /// language would silently degrade the models (S1-mini needs its exact
    /// control-line format).
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = SessionSettings::default();
        let updated = SessionSettings {
            language: non_empty_or(
                self.language_input.read(cx).value().trim(),
                defaults.language,
            ),
            control_line: non_empty_or(
                self.control_input.read(cx).value().trim(),
                defaults.control_line,
            ),
        };
        if let Err(e) = updated.save() {
            eprintln!("saving settings failed: {e:#}");
            return;
        }
        self.language_input.update(cx, |state, cx| {
            state.set_value(updated.language.clone(), window, cx)
        });
        self.control_input.update(cx, |state, cx| {
            state.set_value(updated.control_line.clone(), window, cx)
        });
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

    fn info_row(&self, label: &'static str, value: SharedString, cx: &App) -> impl IntoElement {
        h_flex()
            .justify_between()
            .text_sm()
            .child(Label::new(label).text_color(cx.theme().muted_foreground))
            .child(Label::new(value))
    }
}

fn non_empty_or(value: &str, fallback: String) -> String {
    if value.is_empty() {
        fallback
    } else {
        value.to_string()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let saved = self.saved_at.is_some_and(|at| at.elapsed() < SAVED_FLASH);
        v_flex()
            .size_full()
            .p_6()
            .gap_5()
            .bg(cx.theme().background)
            .child(
                GroupBox::new().title("Dictation").child(
                    v_form()
                        .child(
                            field()
                                .label("Post-processing prompt")
                                .description("S1-mini control line; applies to the next dictation")
                                .child(Input::new(&self.control_input)),
                        )
                        .child(
                            field()
                                .label("Language")
                                .description("ISO 639-1 hint for the speech recognizer")
                                .child(Input::new(&self.language_input)),
                        ),
                ),
            )
            .child(
                GroupBox::new().title("Status").child(
                    v_flex()
                        .gap_2()
                        .child(self.info_row("Hotkey", "⌥ Space".into(), cx))
                        .child(self.info_row("Daemon", self.daemon_summary.clone(), cx)),
                ),
            )
            .child(
                h_flex()
                    .mt_auto()
                    .justify_end()
                    .items_center()
                    .gap_3()
                    .child(
                        Label::new(if saved { "Saved" } else { "" })
                            .text_sm()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        Button::new("save")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|view, _, window, cx| view.save(window, cx))),
                    ),
            )
    }
}
