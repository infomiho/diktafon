//! Settings window, opened from the menu bar: a sidebar of sections
//! (General / Dictation / Advanced) with a titled content pane, built from
//! gpui-component widgets. Edits persist to config.json; the prompt and
//! language apply to the next dictation, the idle-unload time when the
//! daemon restarts.

use crate::config::SessionSettings;
use crate::{autostart, statusbar};
use gpui::{
    App, AppContext, Bounds, Context, Entity, ParentElement, Render, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState};
use gpui_component::label::Label;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::select::{Select, SelectState};
use gpui_component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme, IconName, IndexPath, Root, Sizable, StyledExt, h_flex, v_flex};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(720.), px(500.));
/// One control height for the whole window: the kit's Large inputs and
/// selects are 40px, but its Large button keeps the 32px Medium height, so
/// the button gets the height explicitly to stay coherent.
const CONTROL_HEIGHT: gpui::Pixels = px(40.);
/// How long the "Saved" confirmation stays visible.
const SAVED_FLASH: Duration = Duration::from_secs(2);

/// ISO 639-1 codes the language dropdown offers; a configured code outside
/// this list is appended so it stays selectable.
const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("hr", "Croatian"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("nb", "Norwegian"),
    ("fi", "Finnish"),
    ("hu", "Hungarian"),
    ("ro", "Romanian"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ru", "Russian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
];

const IDLE_OPTIONS: &[(u64, &str)] = &[
    (60, "After 1 minute"),
    (300, "After 5 minutes"),
    (900, "After 15 minutes"),
    (3600, "After 1 hour"),
];

#[derive(Clone, Copy, PartialEq)]
enum Section {
    General,
    Dictation,
    Advanced,
}

impl Section {
    const ALL: [Section; 3] = [Section::General, Section::Dictation, Section::Advanced];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Dictation => "Dictation",
            Section::Advanced => "Advanced",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Section::General => "How diktafon starts and how you talk to it.",
            Section::Dictation => "What happens to your words after you stop speaking.",
            Section::Advanced => "Model residency and daemon state.",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::General => IconName::Settings,
            Section::Dictation => IconName::ALargeSmall,
            Section::Advanced => IconName::Cpu,
        }
    }
}

pub struct SettingsWindow {
    settings: Arc<Mutex<SessionSettings>>,
    section: Section,
    control_input: Entity<InputState>,
    language_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// Codes parallel to the dropdown items. Parallel indexing is only valid
    /// while the selects stay non-searchable: with `.searchable(true)` the
    /// selected index would point into the filtered list.
    language_codes: Vec<String>,
    idle_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// Seconds parallel to the idle dropdown items.
    idle_values: Vec<u64>,
    /// Loaded asynchronously: the SMAppService query is a blocking XPC call.
    autostart: bool,
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

        let mut language_codes: Vec<String> =
            LANGUAGES.iter().map(|(c, _)| c.to_string()).collect();
        let mut language_items: Vec<SharedString> = LANGUAGES
            .iter()
            .map(|(code, name)| SharedString::from(format!("{name} ({code})")))
            .collect();
        let language_index = match language_codes.iter().position(|c| *c == current.language) {
            Some(index) => index,
            None => {
                language_codes.push(current.language.clone());
                language_items.push(current.language.clone().into());
                language_items.len() - 1
            }
        };
        let language_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(language_items),
                Some(IndexPath::new(language_index)),
                window,
                cx,
            )
        });

        let mut idle_values: Vec<u64> = IDLE_OPTIONS.iter().map(|(secs, _)| *secs).collect();
        let mut idle_items: Vec<SharedString> = IDLE_OPTIONS
            .iter()
            .map(|(_, label)| (*label).into())
            .collect();
        // A hand-edited value outside the presets stays selectable; the
        // language dropdown gets the same treatment.
        let idle_index = match idle_values
            .iter()
            .position(|secs| *secs == current.idle_unload_secs)
        {
            Some(index) => index,
            None => {
                idle_values.push(current.idle_unload_secs);
                idle_items.push(format!("After {} seconds", current.idle_unload_secs).into());
                idle_items.len() - 1
            }
        };
        let idle_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(idle_items),
                Some(IndexPath::new(idle_index)),
                window,
                cx,
            )
        });

        cx.spawn(async move |view, cx| {
            let enabled = cx
                .background_executor()
                .spawn(async { autostart::is_enabled() })
                .await;
            let _ = view.update(cx, |view: &mut Self, cx| {
                view.autostart = enabled;
                cx.notify();
            });
        })
        .detach();

        Self {
            settings,
            section: Section::General,
            control_input,
            language_select,
            language_codes,
            idle_select,
            idle_values,
            autostart: false,
            daemon_summary: statusbar::daemon_summary().into(),
            saved_at: None,
        }
    }

    /// An emptied prompt falls back to its default: an empty control line
    /// would silently degrade S1-mini, which needs its exact format.
    fn save(&mut self, cx: &mut Context<Self>) {
        let defaults = SessionSettings::default();
        let control_line = self.control_input.read(cx).value().trim().to_string();
        let language = self
            .language_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.language_codes.get(index.row).cloned())
            .unwrap_or(defaults.language);
        let idle_unload_secs = self
            .idle_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.idle_values.get(index.row).copied())
            .unwrap_or(defaults.idle_unload_secs);
        let updated = SessionSettings {
            language,
            control_line: if control_line.is_empty() {
                defaults.control_line
            } else {
                control_line
            },
            idle_unload_secs,
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

    /// Optimistic flip, reverted if the change fails; failure is the normal
    /// case outside the app bundle.
    fn set_autostart(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.autostart = enabled;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { autostart::set(enabled) })
                .await;
            if let Err(e) = result {
                eprintln!("autostart change failed: {e:#}");
                let _ = view.update(cx, |view: &mut Self, cx| {
                    view.autostart = !enabled;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// A titled row with a muted description on the left and a control on
    /// the right; the layout for switches and static values.
    fn control_row(
        label: &'static str,
        description: &'static str,
        control: impl IntoElement,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .justify_between()
            .items_center()
            .gap_6()
            .py_1()
            .child(
                v_flex().gap_1p5().child(Label::new(label)).child(
                    Label::new(description)
                        .text_sm()
                        .text_color(cx.theme().muted_foreground),
                ),
            )
            .child(control)
    }

    fn general_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(Self::control_row(
                "Start at Login",
                "Launch diktafon when you log in (needs the app bundle)",
                Switch::new("autostart")
                    .large()
                    .checked(self.autostart)
                    .on_click(
                        cx.listener(|view, checked: &bool, _, cx| view.set_autostart(*checked, cx)),
                    ),
                cx,
            ))
            .child(Self::control_row(
                "Hotkey",
                "Hold to dictate, release to paste",
                Label::new("⌥ Space"),
                cx,
            ))
    }

    fn dictation_pane(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        v_form()
            .large()
            .child(
                field()
                    .label("Post-processing prompt")
                    .description("S1-mini control line; applies to the next dictation")
                    .child(Input::new(&self.control_input).large()),
            )
            .child(
                field()
                    .label("Language")
                    .description("Hint for the speech recognizer")
                    .child(Select::new(&self.language_select).large()),
            )
    }

    fn advanced_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(
                field()
                    .label("Unload models when idle")
                    .description(
                        "Frees a few GB of RAM; models reload on the next dictation. \
                         Applies when the daemon restarts.",
                    )
                    .child(Select::new(&self.idle_select).large()),
            )
            .child(Self::control_row(
                "Daemon",
                "The resident inference process",
                Label::new(self.daemon_summary.clone()),
                cx,
            ))
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let saved = self.saved_at.is_some_and(|at| at.elapsed() < SAVED_FLASH);
        let section = self.section;
        let sidebar =
            Sidebar::new("settings-sidebar")
                .w(px(184.))
                .child(SidebarMenu::new().children(Section::ALL.map(|entry| {
                    SidebarMenuItem::new(entry.title())
                        .icon(entry.icon())
                        .active(section == entry)
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.section = entry;
                            cx.notify();
                        }))
                })));

        let pane: gpui::AnyElement = match section {
            Section::General => self.general_pane(cx).into_any_element(),
            Section::Dictation => self.dictation_pane(cx).into_any_element(),
            Section::Advanced => self.advanced_pane(cx).into_any_element(),
        };

        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(|_, _: &crate::CloseWindow, window, _| {
                window.remove_window();
            }))
            .child(sidebar)
            .child(
                v_flex()
                    .flex_1()
                    // Without a min width the long descriptions push the pane
                    // past the window's right edge.
                    .min_w_0()
                    .h_full()
                    .p_8()
                    .gap_2()
                    .child(div().text_2xl().font_semibold().child(section.title()))
                    .child(
                        Label::new(section.description())
                            .text_sm()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().mt_6().child(pane))
                    .child(
                        h_flex()
                            .mt_auto()
                            .pt_5()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .justify_end()
                            .items_center()
                            .gap_4()
                            .child(
                                Label::new(if saved { "Saved" } else { "" })
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                Button::new("save")
                                    .primary()
                                    .large()
                                    .h(CONTROL_HEIGHT)
                                    .px_5()
                                    .label("Save")
                                    .on_click(cx.listener(|view, _, _, cx| view.save(cx))),
                            ),
                    ),
            )
    }
}
