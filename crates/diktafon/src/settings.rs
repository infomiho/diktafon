//! Settings window, opened from the menu bar: a sidebar of sections
//! (General / Dictation / History / Advanced) with a titled content pane,
//! built from gpui-component widgets. Edits persist to config.json; the
//! prompt and language apply to the next dictation, the idle-unload time
//! when the daemon restarts.

use crate::config::SessionSettings;
use crate::statusbar::DaemonStatus;
use crate::{autostart, statusbar, theme};
use chrono::{Datelike, Local, NaiveDate};
use diktafon_protocol::HistoryEntry;
use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, ParentElement, Render, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, point, prelude::*, px,
    relative, rems, rgba, size,
};
use gpui_component::form::{Form, field, v_form};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::label::Label;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::switch::Switch;
use gpui_component::{
    ActiveTheme, Icon, IconName, IndexPath, Root, Sizable, StyledExt, h_flex, v_flex,
};
use std::sync::{Arc, Mutex};

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(720.), px(500.));
/// One control height for the whole window: the kit's Large inputs and
/// selects are 40px, but its Large button keeps the 32px Medium height, so
/// the button gets the height explicitly to stay coherent.
const CONTROL_HEIGHT: gpui::Pixels = px(40.);
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
    History,
    Advanced,
}

impl Section {
    const ALL: [Section; 4] = [
        Section::General,
        Section::Dictation,
        Section::History,
        Section::Advanced,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Dictation => "Dictation",
            Section::History => "History",
            Section::Advanced => "Advanced",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::General => IconName::Settings,
            Section::Dictation => IconName::ALargeSmall,
            Section::History => IconName::Calendar,
            Section::Advanced => IconName::Cpu,
        }
    }
}

/// The pane is a "what did I just say" surface, not an archive; only the
/// freshest entries are shown. The full file stays intact for recovery.
const HISTORY_CAP: usize = 20;

/// The recorded dictations, newest first. Unparseable lines and entries
/// whose polish came out empty (nothing to show or copy) are skipped rather
/// than failing the whole pane.
fn load_history() -> Vec<HistoryEntry> {
    let Ok(content) = std::fs::read_to_string(diktafon_protocol::history_path()) else {
        return Vec::new();
    };
    let mut entries: Vec<HistoryEntry> = content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|entry: &HistoryEntry| !entry.polished.trim().is_empty())
        .collect();
    entries.reverse();
    entries.truncate(HISTORY_CAP);
    entries
}

/// The local calendar day an entry belongs to, for grouping under one label.
fn local_day(at: &str) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.with_timezone(&Local).date_naive())
}

fn day_label(day: Option<NaiveDate>) -> String {
    let Some(day) = day else {
        return "Earlier".into();
    };
    let today = Local::now().date_naive();
    if day == today {
        "Today".into()
    } else if Some(day) == today.pred_opt() {
        "Yesterday".into()
    } else if day.year() == today.year() {
        day.format("%B %-d").to_string()
    } else {
        day.format("%B %-d, %Y").to_string()
    }
}

fn local_time(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| "--:--".into())
}

pub struct SettingsWindow {
    settings: Arc<Mutex<SessionSettings>>,
    section: Section,
    control_input: Entity<TextareaState>,
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
    daemon_status: DaemonStatus,
    /// Read at open and again when the History section is entered; newest
    /// first.
    history: Vec<HistoryEntry>,
    history_search: Entity<InputState>,
    /// Index into `history` whose copy button just fired; drives the brief
    /// check-mark flash.
    copied: Option<usize>,
    /// Keeps the window on the action dispatch path, so the global Cmd+W
    /// binding reaches the CloseWindow handler even with no control focused.
    focus_handle: gpui::FocusHandle,
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
                // The titlebar is transparent and full-size-content: the pane
                // extends to the window's top edge and the traffic lights
                // float over the sidebar.
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Diktafon Settings")),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            |window, cx| {
                force_dark_titlebar(window);
                let view = cx.new(|cx| SettingsWindow::new(settings, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .ok()?;
    cx.activate(true);
    Some(handle)
}

/// The window content is Signal-dark regardless of system appearance (see
/// theme::apply_settings_theme), so the native titlebar must match or it
/// renders as a light strip over the dark pane.
fn force_dark_titlebar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        let ns_view = appkit.ns_view.as_ptr() as *const objc2_app_kit::NSView;
        let appearance = objc2_app_kit::NSAppearance::appearanceNamed(unsafe {
            objc2_app_kit::NSAppearanceNameDarkAqua
        });
        if let Some(ns_window) = (unsafe { &*ns_view }).window() {
            use objc2_app_kit::NSAppearanceCustomization;
            ns_window.setAppearance(appearance.as_deref());
        }
    }
}

impl SettingsWindow {
    fn new(
        settings: Arc<Mutex<SessionSettings>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = settings.lock().unwrap().clone();

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let history = load_history();
        let history_search = cx.new(|cx| {
            let placeholder = match history.len() {
                0 => "Search".to_string(),
                1 => "Search 1 dictation".to_string(),
                n => format!("Search {n} dictations"),
            };
            InputState::new(window, cx).placeholder(placeholder)
        });

        let control_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 6)
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

        // Settings apply as they change, macOS-style; there is no Save
        // button. Enter inserts a newline in the textarea, so only Blur
        // (and closing the window) commits the prompt.
        cx.subscribe(&control_input, |view, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Blur) {
                view.save(cx);
            }
        })
        .detach();
        cx.subscribe(&history_search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(
            &language_select,
            |view, _, event: &SelectEvent<SearchableVec<SharedString>>, cx| {
                let SelectEvent::Confirm(_) = event;
                view.save(cx);
            },
        )
        .detach();
        cx.subscribe(
            &idle_select,
            |view, _, event: &SelectEvent<SearchableVec<SharedString>>, cx| {
                let SelectEvent::Confirm(_) = event;
                view.save(cx);
            },
        )
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
            daemon_status: statusbar::daemon_status(),
            history,
            history_search,
            copied: None,
            focus_handle,
        }
    }

    /// An emptied prompt falls back to its default: an empty control line
    /// would silently degrade S1-mini, which needs its exact format.
    fn save(&mut self, cx: &mut Context<Self>) {
        let defaults = SessionSettings::default();
        // S1-mini's control line is strictly one line; the textarea only
        // wraps for editing comfort.
        let control_line = self
            .control_input
            .read(cx)
            .value()
            .replace('\n', " ")
            .trim()
            .to_string();
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

    fn brand_row(cx: &App) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap(px(9.))
            .px_3()
            .pb(px(14.))
            // The canonical mark SVG, served by the app's asset source.
            .child(gpui::img(crate::assets::MARK_FLAT).w(px(32.5)).h(px(18.)))
            .child(
                div()
                    .font_family(theme::FONT_DISPLAY)
                    .text_size(px(16.))
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child("diktafon"),
            )
    }

    /// Hand-rolled nav row on the shared control height: the kit's
    /// SidebarMenuItem hardcodes a 28px compact height that would sit
    /// undersized next to the 40px controls.
    fn nav_item(&self, entry: Section, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.section == entry;
        let theme = cx.theme();
        h_flex()
            .id(entry.title())
            .h(CONTROL_HEIGHT)
            .px_3()
            .gap_3()
            .rounded_lg()
            .cursor_pointer()
            .text_color(if active {
                theme.sidebar_accent_foreground
            } else {
                theme.sidebar_foreground
            })
            .when(active, |el| el.bg(theme.sidebar_accent))
            .when(!active, {
                let hover_bg = theme.sidebar_accent.opacity(0.5);
                move |el| el.hover(move |el| el.bg(hover_bg))
            })
            .on_click(cx.listener(move |view, _, _, cx| {
                // Dictations land while the window is open; entering the
                // pane rereads them.
                if entry == Section::History {
                    view.history = load_history();
                }
                view.section = entry;
                cx.notify();
            }))
            .child(Icon::new(entry.icon()).small())
            .child(entry.title())
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
                v_flex().gap_1p5().child(Label::new(label).font_medium()).child(
                    Label::new(description)
                        .text_sm()
                        .text_color(cx.theme().muted_foreground),
                ),
            )
            .child(control)
    }

    /// The hotkey as keycap chips.
    fn keycaps(keys: &'static [&'static str]) -> impl IntoElement {
        h_flex().gap_1p5().children(keys.iter().map(|key| {
            div()
                .h(px(28.))
                .min_w(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(rgba(theme::SURFACE_RAISED | 0xFF))
                .border_1()
                .border_b_2()
                .border_color(rgba(theme::HAIRLINE | 0x22))
                .text_size(px(13.))
                .font_medium()
                .child(*key)
        }))
    }

    fn general_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(Self::control_row(
                "Start at login",
                "Launch diktafon when you log in",
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
                Self::keycaps(&["⌥", "Space"]),
                cx,
            ))
    }

    /// The one form recipe every pane shares, so the kit's field labels
    /// match the hand-rolled `control_row` labels (15px medium): the kit's
    /// default label is text_sm, a step too small next to 40px controls.
    fn form() -> Form {
        v_form().large().label_text_size(rems(1.))
    }

    fn dictation_pane(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        Self::form()
            .child(
                field()
                    .label("Post-processing prompt")
                    .description(
                        "Shapes how your words are polished; applies to the next dictation",
                    )
                    .child(Textarea::new(&self.control_input)),
            )
            .child(
                field()
                    .label("Language")
                    .description("The language you dictate in")
                    .child(Select::new(&self.language_select).large()),
            )
    }

    /// A muted label on the left, a truncating mono value on the right; the
    /// row layout inside the daemon card.
    fn daemon_row(label: &'static str, value: String, cx: &App) -> impl IntoElement {
        h_flex()
            .gap_6()
            .items_center()
            .justify_between()
            .child(
                Label::new(label)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .font_family(theme::FONT_MONO)
                    .text_size(px(13.))
                    .truncate()
                    .child(value),
            )
    }

    /// The daemon as a status card: a liveness dot and word, model residency,
    /// and the model names as label/value rows. Replaces the raw one-line
    /// summary that overflowed the pane.
    fn daemon_card(&self, cx: &App) -> impl IntoElement {
        let status = &self.daemon_status;
        // White = alive, muted = not; the dot never glows (glow means live
        // signal, and a resident daemon is not one).
        let (dot, word) = if status.running {
            (rgba(theme::SIGNAL_MAGENTA | 0xFF), "Running")
        } else {
            (rgba(theme::RING_IDLE | 0x80), "Not running")
        };
        let residency = match (status.running, status.models_loaded) {
            (false, _) => None,
            (true, true) => Some("Models loaded"),
            (true, false) => Some("Loading models"),
        };
        let card = v_flex()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(rgba(theme::SURFACE | 0xFF))
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .mb_1()
                    .items_center()
                    .child(div().size(px(8.)).rounded_full().bg(dot))
                    .child(Label::new(word).font_medium())
                    .when_some(residency, |el, residency| {
                        el.child(div().flex_1()).child(
                            Label::new(residency)
                                .text_sm()
                                .text_color(cx.theme().muted_foreground),
                        )
                    }),
            );
        card.when_some(status.asr.clone(), |card, asr| {
            card.child(Self::daemon_row("Transcription model", asr, cx))
        })
        .when_some(status.llm.clone(), |card, llm| {
            card.child(Self::daemon_row("Polishing model", llm, cx))
        })
    }

    fn advanced_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(
                Self::form().child(
                    field()
                        .label("Unload models when idle")
                        .description(
                            "Frees a few GB of RAM; models reload on the next dictation. \
                             Applies when the daemon restarts.",
                        )
                        .child(Select::new(&self.idle_select).large()),
                ),
            )
            .child(Self::form().child(field().label("Daemon").child(self.daemon_card(cx))))
    }

    fn day_header(text: String, first: bool, cx: &App) -> impl IntoElement {
        div()
            .when(!first, |el| el.mt(px(20.)))
            .mb(px(4.))
            .px(px(14.))
            .text_size(px(13.))
            .font_medium()
            .text_color(cx.theme().muted_foreground)
            .child(text)
    }

    fn history_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = &self.history[ix];
        let time = local_time(&entry.at);
        let text = SharedString::from(entry.polished.clone());
        let copy_text = entry.polished.clone();
        let copied = self.copied == Some(ix);
        h_flex()
            .items_start()
            .gap(px(14.))
            .px(px(14.))
            .py(px(13.))
            .rounded(px(8.))
            .hover(|el| el.bg(rgba(theme::SURFACE_RAISED | 0xFF)))
            .child(
                div()
                    .w(px(40.))
                    .flex_none()
                    .pt(px(1.))
                    .text_size(px(13.))
                    .text_color(rgba(theme::TEXT_FAINT | 0xFF))
                    .child(time),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(15.))
                    .line_height(relative(1.5))
                    .line_clamp(2)
                    .child(text),
            )
            .child(
                div()
                    .id(("copy", ix))
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(if copied {
                        rgba(theme::SIGNAL_MAGENTA | 0xFF)
                    } else {
                        rgba(theme::TEXT_FAINT | 0xFF)
                    })
                    .hover(|el| {
                        el.bg(rgba(theme::HAIRLINE | 0x22))
                            .text_color(rgba(theme::TEXT_PRIMARY | 0xFF))
                    })
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                        view.copied = Some(ix);
                        cx.notify();
                        cx.spawn(async move |view, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(1500))
                                .await;
                            let _ = view.update(cx, |view: &mut Self, cx| {
                                if view.copied == Some(ix) {
                                    view.copied = None;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }))
                    .child(Icon::new(if copied { IconName::Check } else { IconName::Copy }).small()),
            )
    }

    fn history_empty(text: &'static str) -> gpui::AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .text_size(px(15.))
            .text_color(rgba(theme::TEXT_DIM | 0xFF))
            .child(text)
            .into_any_element()
    }

    fn history_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self
            .history_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let visible: Vec<usize> = (0..self.history.len())
            .filter(|&ix| {
                let entry = &self.history[ix];
                query.is_empty()
                    || entry.polished.to_lowercase().contains(&query)
                    || entry.raw.to_lowercase().contains(&query)
            })
            .collect();

        let mut items: Vec<gpui::AnyElement> = Vec::new();
        let mut last_day: Option<Option<NaiveDate>> = None;
        for ix in visible {
            let day = local_day(&self.history[ix].at);
            if last_day != Some(day) {
                let first = items.is_empty();
                items.push(Self::day_header(day_label(day), first, cx).into_any_element());
                last_day = Some(day);
            }
            items.push(self.history_row(ix, cx).into_any_element());
        }

        let body = if self.history.is_empty() {
            Self::history_empty("No dictations yet")
        } else if items.is_empty() {
            Self::history_empty("No matches")
        } else {
            v_flex().gap(px(6.)).children(items).into_any_element()
        };

        v_flex()
            .size_full()
            .gap(px(20.))
            .child(Input::new(&self.history_search).large().cleanable(true))
            .child(
                div()
                    .id("history-list")
                    .flex_1()
                    .min_h_0()
                    // Rows pad 14px past the text column so the hover pill
                    // has bleed, mirroring the pane edge alignment of the
                    // search well above.
                    .mx(px(-14.))
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.section;
        let theme = cx.theme();
        let sidebar = v_flex()
            .w(px(200.))
            .h_full()
            .flex_shrink_0()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            // Clears the traffic lights floating over the sidebar's top-left.
            .pt(px(48.))
            .px_3()
            .gap_1p5()
            .child(Self::brand_row(cx))
            .children({
                let mut items = Vec::new();
                for entry in Section::ALL {
                    items.push(self.nav_item(entry, cx).into_any_element());
                }
                items
            });

        let pane: gpui::AnyElement = match section {
            Section::General => self.general_pane(cx).into_any_element(),
            Section::Dictation => self.dictation_pane(cx).into_any_element(),
            Section::History => self.history_pane(cx).into_any_element(),
            Section::Advanced => self.advanced_pane(cx).into_any_element(),
        };

        h_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            // A pending prompt edit would be lost on Cmd+W: Blur never fires
            // for a closing window.
            .on_action(cx.listener(|view, _: &crate::CloseWindow, window, cx| {
                view.save(cx);
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
                    .child(
                        div()
                            .font_family(theme::FONT_DISPLAY)
                            .text_2xl()
                            .font_semibold()
                            .child(section.title()),
                    )
                    .child(v_flex().mt_8().flex_1().min_h_0().child(pane)),
            )
    }
}
