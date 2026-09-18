//! Settings window, opened from the menu bar: a sidebar of sections
//! (General / Models / History / Advanced) with a titled content pane,
//! built from gpui-component widgets. Edits persist to config.json; the
//! prompt and language apply to the next dictation, the idle-unload time
//! when the daemon restarts.

use crate::config::SessionSettings;
use crate::control_line;
use crate::statusbar::DaemonStatus;
use crate::updater::{self, UpdateCheck};
use crate::{autostart, statusbar, theme};
use chrono::{Datelike, Local, NaiveDate};
use diktafon_protocol::HistoryEntry;
use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, Div, Entity, ParentElement, Render,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, point,
    prelude::*, px, relative, rems, rgba, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::form::{Form, field, v_form};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::label::Label;
use gpui_component::list::ListItem;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::switch::Switch;
use gpui_component::{
    ActiveTheme, Icon, IconName, IndexPath, Root, Sizable, StyledExt, h_flex, v_flex,
};
use std::sync::{Arc, Mutex};

#[derive(serde::Deserialize)]
struct ModelCatalog {
    models: Vec<CatalogModel>,
}

#[derive(serde::Deserialize)]
struct CatalogModel {
    id: String,
    category: String,
    name: String,
    files: Vec<CatalogFile>,
}

#[derive(serde::Deserialize)]
struct CatalogFile {
    size: u64,
}

fn model_options(
    current: &str,
    category: &str,
    apple_available: bool,
) -> (Vec<String>, Vec<SharedString>, Vec<String>) {
    let catalog: ModelCatalog = serde_json::from_str(diktafon_protocol::MODEL_CATALOG_JSON)
        .expect("bundled model catalog must parse");
    let mut ids = Vec::new();
    let mut labels = Vec::new();
    let mut descriptions = Vec::new();
    for model in catalog.models.into_iter().filter(|model| {
        model.category == category && (model.id != "apple-intelligence" || apple_available)
    }) {
        let size: u64 = model.files.iter().map(|file| file.size).sum();
        let metadata = if model.files.is_empty() {
            "Built into macOS. No download.".to_string()
        } else {
            let size = size as f64 / 1_000_000_000.0;
            if size >= 1.0 {
                format!("Size: {size:.2} GB")
            } else {
                format!("Size: {} MB", (size * 1_000.0).round() as u64)
            }
        };
        labels.push(model.name.clone().into());
        descriptions.push(metadata);
        ids.push(model.id);
    }
    if !ids.iter().any(|id| id == current) && (current != "apple-intelligence" || apple_available) {
        ids.push(current.to_string());
        labels.push(format!("Unknown model ({current})").into());
        descriptions.push("This model is not available. Choose another model.".into());
    }
    (ids, labels, descriptions)
}
fn open_third_party_notices() -> std::io::Result<()> {
    let executable = std::env::current_exe()?;
    let bundled = executable
        .parent()
        .and_then(std::path::Path::parent)
        .map(|contents| contents.join("Resources/THIRD_PARTY_NOTICES.md"));
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../THIRD_PARTY_NOTICES.md");
    let path = bundled.filter(|path| path.exists()).unwrap_or(source);
    std::process::Command::new("open").arg(path).spawn()?;
    Ok(())
}

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(720.), px(500.));
/// How often the open settings window re-reads the daemon's status file.
const DAEMON_POLL: std::time::Duration = std::time::Duration::from_millis(750);
/// One control height for the whole window: the kit's Large inputs and
/// selects are 40px, but its Large button keeps the 32px Medium height, so
/// the button gets the height explicitly to stay coherent.
const CONTROL_HEIGHT: gpui::Pixels = px(40.);
/// ISO 639-1 codes the language dropdown offers; a configured code outside
/// this list is appended so it stays selectable.
const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("el", "Greek"),
    ("ar", "Arabic"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("vi", "Vietnamese"),
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
    Models,
    History,
    Advanced,
}

impl Section {
    const ALL: [Section; 4] = [
        Section::General,
        Section::Models,
        Section::History,
        Section::Advanced,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Models => "Models",
            Section::History => "History",
            Section::Advanced => "Advanced",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::General => IconName::Settings,
            Section::Models => IconName::Bot,
            Section::History => IconName::Calendar,
            Section::Advanced => IconName::Cpu,
        }
    }
}

/// The pane is a "what did I just say" surface, not an archive; only the
/// freshest entries are shown. The full file stays intact for recovery.
const HISTORY_CAP: usize = 20;

/// The recorded dictations, newest first. Entries whose polish came out empty
/// have nothing to show or copy, so they never count toward the cap.
fn load_history() -> Vec<HistoryEntry> {
    diktafon_protocol::history::recent_matching(HISTORY_CAP, |entry| {
        !entry.polished.trim().is_empty()
    })
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

fn search_placeholder(count: usize) -> String {
    match count {
        1 => "Search 1 dictation".to_string(),
        n => format!("Search {n} dictations"),
    }
}

fn local_time(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| "--:--".into())
}

/// The History pane's state: dictations grouped by day, filtered by the
/// search well. Each row's copy button puts its polished text on the
/// clipboard.
struct History {
    /// Everything shown, newest first.
    entries: Vec<HistoryEntry>,
    /// The filtered view: one `(day label, entry indices)` per group.
    days: Vec<(String, Vec<usize>)>,
    query: String,
    /// Entry whose text was just copied; drives the brief check-mark flash.
    copied: Option<usize>,
}

impl History {
    fn new() -> Self {
        let mut history = Self {
            entries: load_history(),
            days: Vec::new(),
            query: String::new(),
            copied: None,
        };
        history.regroup();
        history
    }

    fn reload(&mut self) {
        self.entries = load_history();
        // An index into the old entries would flash the wrong row.
        self.copied = None;
        self.regroup();
    }

    fn regroup(&mut self) {
        self.days.clear();
        for (ix, entry) in self.entries.iter().enumerate() {
            if !self.query.is_empty()
                && !entry.polished.to_lowercase().contains(&self.query)
                && !entry.raw.to_lowercase().contains(&self.query)
            {
                continue;
            }
            let label = day_label(local_day(&entry.at));
            match self.days.last_mut() {
                Some((day, rows)) if *day == label => rows.push(ix),
                _ => self.days.push((label, vec![ix])),
            }
        }
    }
}

pub struct SettingsWindow {
    settings: Arc<Mutex<SessionSettings>>,
    section: Section,
    /// Chosen value per axis of S1-mini's control line, in the order they
    /// appear in it.
    control: [usize; 3],
    language_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// Codes parallel to the dropdown items. Parallel indexing is only valid
    /// while the selects stay non-searchable: with `.searchable(true)` the
    /// selected index would point into the filtered list.
    language_codes: Vec<String>,
    transcription_select: Entity<SelectState<SearchableVec<SharedString>>>,
    transcription_ids: Vec<String>,
    transcription_descriptions: Vec<String>,
    polishing_select: Entity<SelectState<SearchableVec<SharedString>>>,
    polishing_ids: Vec<String>,
    polishing_descriptions: Vec<String>,
    apple_prompt_input: Entity<TextareaState>,
    idle_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// Seconds parallel to the idle dropdown items.
    idle_values: Vec<u64>,
    /// Loaded asynchronously: the SMAppService query is a blocking XPC call.
    autostart: bool,
    sound_cues: bool,
    /// The push-to-talk chord, in global-hotkey syntax.
    hotkey: String,
    /// True while the hotkey control waits for the user to press a new chord.
    capturing_hotkey: bool,
    hotkey_focus: gpui::FocusHandle,
    /// Cached at open: reading it does file IO and must not run per render.
    daemon_status: DaemonStatus,
    /// Reloaded when the History section is entered.
    history: History,
    /// The design's search well; drives the history filter.
    history_search: Entity<InputState>,
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
                crate::window_lifecycle::release_view_on_close(window, cx);
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
        let mut current = settings.lock().unwrap().clone();
        let mut daemon_status = statusbar::daemon_status();
        if daemon_status.polishing_availability.is_none() {
            daemon_status.polishing_availability =
                crate::apple_intelligence_available().then(|| "Available".to_string());
        }
        let apple_available = daemon_status.polishing_availability.as_deref() == Some("Available");
        let resolved_models = current.models();
        if current.transcription_model != resolved_models.transcription {
            current.transcription_model = resolved_models.transcription;
        }
        if current.polishing_model == "apple-intelligence" && !apple_available {
            current.polishing_model = diktafon_protocol::DEFAULT_POLISHING_MODEL.into();
            if let Err(error) = current.save() {
                eprintln!("saving Apple Intelligence fallback failed: {error:#}");
            }
            *settings.lock().unwrap() = current.clone();
            cx.global::<crate::AppServices>()
                .models
                .set(current.models());
        }

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let hotkey_focus = cx.focus_handle();
        cx.on_focus_out(&hotkey_focus, window, |view, _, _, cx| {
            if view.capturing_hotkey {
                view.capturing_hotkey = false;
                cx.global::<crate::AppServices>().hotkey.resume();
                cx.notify();
            }
        })
        .detach();
        // Closing the window mid-capture must not leave the hotkey suspended.
        cx.on_release(|view: &mut Self, cx| {
            if view.capturing_hotkey {
                cx.global::<crate::AppServices>().hotkey.resume();
            }
        })
        .detach();

        let history = History::new();
        let history_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(search_placeholder(history.entries.len()))
        });

        let control = control_line::AXES.map(|axis| axis.index_in(&current.control_line));

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

        let (transcription_ids, transcription_items, transcription_descriptions) = model_options(
            &current.transcription_model,
            "transcription",
            apple_available,
        );
        let transcription_index = transcription_ids
            .iter()
            .position(|id| id == &current.transcription_model)
            .unwrap();
        let transcription_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(transcription_items),
                Some(IndexPath::new(transcription_index)),
                window,
                cx,
            )
        });
        let (polishing_ids, polishing_items, polishing_descriptions) =
            model_options(&current.polishing_model, "polishing", apple_available);
        let polishing_index = polishing_ids
            .iter()
            .position(|id| id == &current.polishing_model)
            .or_else(|| {
                polishing_ids
                    .iter()
                    .position(|id| id == diktafon_protocol::DEFAULT_POLISHING_MODEL)
            })
            .unwrap();
        let polishing_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(polishing_items),
                Some(IndexPath::new(polishing_index)),
                window,
                cx,
            )
        });
        let apple_prompt_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 8)
                .placeholder("Describe how the transcript should be polished.")
                .default_value(current.apple_prompt.clone())
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

        // The daemon starts, loads its models and exits on idle while this
        // window is open, and it reports that through a file rather than to
        // us, so the card is polled. Reading it is a small file plus a pid
        // check; the loop ends with the window, when the update fails.
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(DAEMON_POLL).await;
                let status = cx
                    .background_executor()
                    .spawn(async { statusbar::daemon_status() })
                    .await;
                let updated = view.update(cx, |view: &mut Self, cx| {
                    if view.daemon_status != status {
                        view.daemon_status = status;
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
        })
        .detach();

        // Sparkle reports its findings whenever a check finishes, including
        // the automatic one at launch, so the Updates card re-renders on it.
        if let Some(status) = updater::status(cx) {
            cx.observe(&status, |_, _, cx| cx.notify()).detach();
        }

        // Settings apply as they change, macOS-style; there is no Save button.

        cx.subscribe(&history_search, |view, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.history.query = input.read(cx).value().trim().to_lowercase();
                view.history.regroup();
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(
            &language_select,
            window,
            |view, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                let SelectEvent::Confirm(_) = event;
                view.save(window, cx);
            },
        )
        .detach();
        cx.subscribe_in(
            &idle_select,
            window,
            |view, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                let SelectEvent::Confirm(_) = event;
                view.save(window, cx);
            },
        )
        .detach();
        for select in [&transcription_select, &polishing_select] {
            cx.subscribe_in(
                select,
                window,
                |view, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(_) = event;
                    view.save(window, cx);
                    cx.notify();
                },
            )
            .detach();
        }
        cx.subscribe_in(
            &apple_prompt_input,
            window,
            |view, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    view.save(window, cx);
                }
            },
        )
        .detach();

        Self {
            settings,
            section: Section::General,
            control,
            language_select,
            language_codes,
            transcription_select,
            transcription_ids,
            transcription_descriptions,
            polishing_select,
            polishing_ids,
            polishing_descriptions,
            apple_prompt_input,
            idle_select,
            idle_values,
            autostart: false,
            sound_cues: current.sound_cues,
            hotkey: current.hotkey.clone(),
            capturing_hotkey: false,
            hotkey_focus,
            daemon_status,
            history,
            history_search,
            focus_handle,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = SessionSettings::default();
        let control_line = control_line::compose(self.control[0], self.control[1], self.control[2]);
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
        let transcription_model = self
            .transcription_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.transcription_ids.get(index.row).cloned())
            .unwrap_or(defaults.transcription_model);
        let polishing_model = self
            .polishing_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.polishing_ids.get(index.row).cloned())
            .unwrap_or(defaults.polishing_model);
        let mut updated = SessionSettings {
            language,
            control_line,
            apple_prompt: self.apple_prompt_input.read(cx).value().to_string(),
            idle_unload_secs,
            sound_cues: self.sound_cues,
            hotkey: self.hotkey.clone(),
            transcription_model: transcription_model.clone(),
            polishing_model,
        };
        let models = updated.models();
        updated.transcription_model = models.transcription.clone();
        if updated.transcription_model != transcription_model
            && let Some(index) = self
                .transcription_ids
                .iter()
                .position(|id| id == &updated.transcription_model)
        {
            self.transcription_select.update(cx, |select, cx| {
                select.set_selected_index(Some(IndexPath::new(index)), window, cx);
                cx.notify();
            });
        }
        if let Err(e) = updated.save() {
            eprintln!("saving settings failed: {e:#}");
            return;
        }
        *self.settings.lock().unwrap() = updated;
        cx.global::<crate::AppServices>().models.set(models);
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
            .on_click(cx.listener(move |view, _, window, cx| {
                // Dictations land while the window is open; entering the
                // pane rereads them.
                if entry == Section::History {
                    view.history.reload();
                    let count = view.history.entries.len();
                    view.history_search.update(cx, |input, cx| {
                        input.set_placeholder(search_placeholder(count), window, cx);
                    });
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
                v_flex()
                    .gap_1p5()
                    .child(Label::new(label).font_medium())
                    .when(!description.is_empty(), |el| {
                        el.child(
                            Label::new(description)
                                .text_sm()
                                .text_color(cx.theme().muted_foreground),
                        )
                    }),
            )
            .child(control)
    }

    /// The hotkey as keycap chips.
    fn keycaps(keys: Vec<String>) -> impl IntoElement {
        h_flex().gap_1p5().children(keys.into_iter().map(|key| {
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
                .child(key)
        }))
    }

    fn general_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(Self::control_row(
                "Start at login",
                "Open Diktafon when you sign in.",
                Switch::new("autostart")
                    .large()
                    .checked(self.autostart)
                    .on_click(
                        cx.listener(|view, checked: &bool, _, cx| view.set_autostart(*checked, cx)),
                    ),
                cx,
            ))
            .child(Self::control_row(
                "Sound cues",
                "Play a sound when recording starts, is canceled, or fails.",
                Switch::new("sound-cues")
                    .large()
                    .checked(self.sound_cues)
                    .on_click(cx.listener(|view, checked: &bool, window, cx| {
                        view.sound_cues = *checked;
                        view.save(window, cx);
                        cx.notify();
                    })),
                cx,
            ))
            .child(Self::control_row(
                "Hotkey",
                "Hold to record. Release to paste.",
                self.hotkey_control(cx),
                cx,
            ))
    }

    /// One keycap label per chord token: modifier symbols, title-cased keys.
    fn hotkey_caps(hotkey: &str) -> Vec<String> {
        hotkey
            .split('+')
            .map(|token| match token.to_lowercase().as_str() {
                "alt" | "option" => "⌥".to_string(),
                "cmd" | "command" | "super" => "⌘".to_string(),
                "ctrl" | "control" => "⌃".to_string(),
                "shift" => "⇧".to_string(),
                key => match crate::keymap::char_for_positional_token(key) {
                    // Positional names render as the character the user's
                    // layout prints on that key.
                    Some(c) => c.to_uppercase().to_string(),
                    None => {
                        let mut chars = key.chars();
                        match chars.next() {
                            Some(first) => first.to_uppercase().chain(chars).collect(),
                            None => String::new(),
                        }
                    }
                },
            })
            .collect()
    }

    fn capture_hotkey(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mods = keystroke.modifiers;
        if keystroke.key == "escape" && !mods.modified() {
            self.capturing_hotkey = false;
            cx.global::<crate::AppServices>().hotkey.resume();
            self.focus_handle.focus(window, cx);
            cx.notify();
            return;
        }
        let mut parts: Vec<&str> = Vec::new();
        if mods.control {
            parts.push("ctrl");
        }
        if mods.alt {
            parts.push("alt");
        }
        if mods.shift {
            parts.push("shift");
        }
        if mods.platform {
            parts.push("cmd");
        }
        if parts.is_empty() {
            // A bare key would fire on normal typing; keep waiting.
            return;
        }
        // Global hotkeys register by physical position while gpui reports the
        // layout's character; a character key must resolve through the layout
        // (e.g. "z" on QWERTZ is the KeyY position) or Z would fire on the
        // wrong key. Named keys (space, f5) type the same everywhere.
        let key = keystroke.key.as_str();
        let token = if key.chars().count() == 1 {
            match crate::keymap::positional_token_for_char(key.chars().next().unwrap()) {
                Some(token) => token.to_string(),
                None => return,
            }
        } else {
            key.to_string()
        };
        let candidate = format!("{}+{}", parts.join("+"), token);
        let Some(hotkey) = crate::config::parse_hotkey(&candidate) else {
            return;
        };
        match cx.global::<crate::AppServices>().hotkey.commit(hotkey) {
            Ok(()) => {
                self.hotkey = candidate;
                self.capturing_hotkey = false;
                self.save(window, cx);
                self.focus_handle.focus(window, cx);
                cx.notify();
            }
            Err(e) => eprintln!("hotkey rebind failed: {e:#}"),
        }
    }

    fn hotkey_control(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let capturing = self.capturing_hotkey;
        div()
            .id("hotkey-capture")
            .track_focus(&self.hotkey_focus)
            .rounded_md()
            .when(!capturing, |el| {
                el.on_click(cx.listener(|view, _, window, cx| {
                    let services = cx.global::<crate::AppServices>();
                    // Unregistering a chord that is currently held would
                    // swallow its release and strand the session.
                    if services.dictation.read(cx).phase != crate::dictation::Phase::Idle {
                        return;
                    }
                    services.hotkey.suspend();
                    view.capturing_hotkey = true;
                    view.hotkey_focus.focus(window, cx);
                    cx.notify();
                }))
            })
            .when(capturing, |el| {
                el.on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                    // The chord is being recorded, not typed: without this a
                    // captured Cmd+W would also close the window.
                    cx.stop_propagation();
                    view.capture_hotkey(&event.keystroke, window, cx);
                }))
            })
            .child(if capturing {
                div()
                    .h(px(28.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_dashed()
                    .border_color(rgba(theme::HAIRLINE | 0x44))
                    .text_size(px(13.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Press new keys")
                    .into_any_element()
            } else {
                Self::keycaps(Self::hotkey_caps(&self.hotkey)).into_any_element()
            })
    }

    /// The one form recipe every pane shares, so the kit's field labels
    /// match the hand-rolled `control_row` labels (15px medium): the kit's
    /// default label is text_sm, a step too small next to 40px controls.
    fn form() -> Form {
        v_form().large().label_text_size(rems(1.))
    }

    fn polishing_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Self::form()
            .child(field().label("Tone").child(self.segmented(0, cx)))
            .child(
                field().child(
                    h_flex()
                        .gap_6()
                        .items_start()
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_2()
                                .child(Self::field_label("Shape"))
                                .child(self.segmented(1, cx)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_2()
                                .child(Self::field_label("Written for"))
                                .child(self.segmented(2, cx)),
                        ),
                ),
            )
    }

    fn apple_prompt(&self) -> impl IntoElement {
        Self::form().child(field().label("Instructions").child(
            Textarea::new(&self.apple_prompt_input).aria_label("Apple Intelligence instructions"),
        ))
    }

    /// A row of choices with the current one raised, for axes with a handful
    /// of values: a dropdown hides two options behind a click and reads busy
    /// next to the rest of the pane.
    fn segmented(&self, slot: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let axis = &control_line::AXES[slot];
        let selected = self.control[slot];
        let theme = cx.theme();
        let (raised, muted, chosen) = (
            rgba(theme::SURFACE_RAISED | 0xFF),
            theme.muted_foreground,
            theme.foreground,
        );
        h_flex()
            .w_full()
            // Concentric: the inner radius plus this padding is the outer one.
            .p(px(3.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(rgba(theme::SURFACE_SUNKEN | 0xFF))
            .border_1()
            .border_color(rgba(theme::HAIRLINE | 0x22))
            .children((0..axis.values.len()).map(|index| {
                let active = index == selected;
                div()
                    .id(("segment", slot * 16 + index))
                    .flex_1()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .text_size(px(13.))
                    .when(active, |el| el.bg(raised).text_color(chosen).font_medium())
                    .when(!active, |el| {
                        el.text_color(muted)
                            .hover(|el| el.bg(rgba(theme::HAIRLINE | 0x14)))
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if view.control[slot] != index {
                            view.control[slot] = index;
                            view.save(window, cx);
                            cx.notify();
                        }
                    }))
                    .child(axis.label(index))
            }))
    }

    /// The kit's field label and help text, for rows built by hand.
    fn field_label(text: &'static str) -> impl IntoElement {
        div().text_size(px(15.)).font_medium().child(text)
    }

    /// A pane subsection as a bordered card whose first row is the section
    /// name in eyebrow style (small uppercase, muted): the size/case/color
    /// step between the pane title and the 15px field labels, and the card
    /// edge keeps the section's fields visibly one unit while scrolling.
    /// Settled in docs/mockups/settings.html.
    fn section_card(title: &'static str, cx: &App) -> Div {
        v_flex()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(rgba(theme::SURFACE | 0xFF))
            .p_5()
            .gap_4()
            .child(
                div()
                    .text_size(px(12.))
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.to_uppercase()),
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
        .when_some(status.asr_backend.clone(), |card, backend| {
            let runtime = match &status.asr_device {
                Some(device) => format!("{backend} on {device}"),
                None => backend,
            };
            card.child(Self::daemon_row("Transcription runtime", runtime, cx))
        })
        .when_some(
            status.polishing_availability.clone(),
            |card, availability| {
                card.child(Self::daemon_row("Apple Intelligence", availability, cx))
            },
        )
    }

    /// The running version with the outcome of Sparkle's last check, a button
    /// that opens Sparkle's window, and the automatic-check switch. A build
    /// that does not update itself shows only the version.
    fn updates_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let check = updater::status(cx).map(|status| status.read(cx).check.clone());
        let (detail, emphasized) = match &check {
            None => ("This build does not update itself.".to_string(), false),
            Some(UpdateCheck::Unknown) => ("Not checked yet.".to_string(), false),
            Some(UpdateCheck::UpToDate) => ("Up to date.".to_string(), false),
            Some(UpdateCheck::Available(version)) => {
                (format!("Diktafon {version} is ready to install."), true)
            }
            Some(UpdateCheck::Skipped(version)) => (format!("Version {version} skipped."), false),
        };
        let action = match &check {
            Some(UpdateCheck::Available(_)) => "Install update",
            _ => "Check for updates",
        };
        let detail_color = if emphasized {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        };
        let version_row = h_flex()
            .gap_6()
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).font_medium(),
                    )
                    .child(Label::new(detail).text_sm().text_color(detail_color)),
            )
            .when(check.is_some(), |row| {
                row.child(
                    Button::new("check-for-updates")
                        .label(action)
                        .outline()
                        .on_click(|_, _, cx| updater::check_for_updates(cx)),
                )
            });
        v_flex()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(rgba(theme::SURFACE | 0xFF))
            .p_4()
            .gap_4()
            .child(version_row)
            .when_some(updater::automatic_checks(cx), |card, enabled| {
                card.child(
                    div()
                        .pt_4()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(Self::control_row(
                            "Check for updates automatically",
                            "Once a day. You choose when to install.",
                            Switch::new("automatic-updates")
                                .large()
                                .checked(enabled)
                                .on_click(cx.listener(|_, checked: &bool, _, cx| {
                                    updater::set_automatic_checks(*checked, cx);
                                    cx.notify();
                                })),
                            cx,
                        )),
                )
            })
    }

    fn models_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let transcription_description = self
            .transcription_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.transcription_descriptions.get(index.row))
            .cloned()
            .unwrap_or_default();
        let polishing_description = self
            .polishing_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.polishing_descriptions.get(index.row))
            .cloned()
            .unwrap_or_default();
        let selected_polisher = self
            .polishing_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.polishing_ids.get(index.row));
        let uses_s1_prompt = selected_polisher.is_some_and(|id| id == "s1-mini-q4-k-m");
        let uses_apple_prompt = selected_polisher.is_some_and(|id| id == "apple-intelligence");
        let transcription = Self::section_card("Transcription", cx).child(
            Self::form()
                .child(
                    field()
                        .label("Transcription model")
                        .description(transcription_description)
                        .child(Select::new(&self.transcription_select).large()),
                )
                .child(
                    field()
                        .label("Language")
                        .description("Choose the language you speak.")
                        .child(Select::new(&self.language_select).large()),
                ),
        );
        let polishing = Self::section_card("Polishing", cx)
            .child(
                Self::form().child(
                    field()
                        .label("Polishing model")
                        .description(polishing_description)
                        .child(Select::new(&self.polishing_select).large()),
                ),
            )
            .when(uses_s1_prompt, |card| card.child(self.polishing_prompt(cx)))
            .when(uses_apple_prompt, |card| card.child(self.apple_prompt()));
        v_flex().gap_8().child(transcription).child(polishing)
    }

    fn advanced_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(
                Self::form().child(
                    field()
                        .label("Unload models when idle")
                        .description(
                            "Free memory when Diktafon is idle. Models reload with the next dictation.",
                        )
                        .child(Select::new(&self.idle_select).large()),
                ),
            )
            .child(Self::form().child(field().label("Daemon").child(self.daemon_card(cx))))
            .child(Self::form().child(field().label("Updates").child(self.updates_card(cx))))
            .child(
                h_flex().child(
                    Button::new("open-third-party-notices")
                        .label("Third-party notices")
                        .link()
                        .small()
                        .on_click(|_, _, _| {
                            if let Err(error) = open_third_party_notices() {
                                eprintln!("opening third-party notices failed: {error}");
                            }
                        }),
                ),
            )
    }

    fn day_header(label: &str, first: bool, cx: &App) -> impl IntoElement {
        div()
            .when(!first, |el| el.pt(px(20.)))
            .pb(px(4.))
            .px(px(10.))
            .text_size(px(13.))
            .font_medium()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(label.to_string()))
    }

    fn history_row(&self, entry_ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = &self.history.entries[entry_ix];
        let copied = self.history.copied == Some(entry_ix);
        let copy_text = entry.polished.clone();
        ListItem::new(("dictation", entry_ix))
            .px(px(10.))
            .py(px(10.))
            .rounded(px(8.))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(14.))
                    .w_full()
                    .child(
                        div()
                            .w(px(40.))
                            .flex_none()
                            .text_size(px(13.))
                            .text_color(rgba(theme::TEXT_FAINT | 0xFF))
                            .child(local_time(&entry.at)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(15.))
                            .line_height(relative(1.5))
                            .line_clamp(2)
                            .child(SharedString::from(entry.polished.clone())),
                    )
                    .child(
                        div()
                            .id(("copy", entry_ix))
                            .size(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
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
                                view.history.copied = Some(entry_ix);
                                cx.notify();
                                cx.spawn(async move |view, cx| {
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(1500))
                                        .await;
                                    let _ = view.update(cx, |view: &mut Self, cx| {
                                        if view.history.copied == Some(entry_ix) {
                                            view.history.copied = None;
                                            cx.notify();
                                        }
                                    });
                                })
                                .detach();
                            }))
                            .child(
                                Icon::new(if copied {
                                    IconName::Check
                                } else {
                                    IconName::Copy
                                })
                                .small(),
                            ),
                    ),
            )
    }

    fn history_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut items: Vec<gpui::AnyElement> = Vec::new();
        for (label, rows) in self.history.days.clone() {
            let first = items.is_empty();
            items.push(Self::day_header(&label, first, cx).into_any_element());
            for entry_ix in rows {
                items.push(self.history_row(entry_ix, cx).into_any_element());
            }
        }

        let body: gpui::AnyElement = if items.is_empty() {
            let text = if self.history.entries.is_empty() {
                "No dictations yet"
            } else {
                "No matches"
            };
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_size(px(15.))
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element()
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
            Section::Models => self.models_pane(cx).into_any_element(),
            Section::History => self.history_pane(cx).into_any_element(),
            Section::Advanced => self.advanced_pane(cx).into_any_element(),
        };

        h_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .on_action(cx.listener(|view, _: &crate::CloseWindow, window, cx| {
                view.save(window, cx);
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
                    // Panes are taller than the fixed window; without this the
                    // last field is simply cut off.
                    .child(
                        div()
                            .id("pane-scroll")
                            .mt_8()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(pane),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_intelligence_is_only_offered_when_available() {
        let (unavailable, _, _) = model_options(
            diktafon_protocol::DEFAULT_POLISHING_MODEL,
            "polishing",
            false,
        );
        assert!(!unavailable.iter().any(|id| id == "apple-intelligence"));

        let (available, _, _) = model_options(
            diktafon_protocol::DEFAULT_POLISHING_MODEL,
            "polishing",
            true,
        );
        assert!(available.iter().any(|id| id == "apple-intelligence"));
    }

    #[test]
    fn model_descriptions_are_brief() {
        let (_, _, descriptions) = model_options(
            diktafon_protocol::DEFAULT_TRANSCRIPTION_MODEL,
            "transcription",
            false,
        );
        assert!(
            descriptions
                .iter()
                .all(|description| !description.contains(';')
                    && description.split_whitespace().count() <= 6)
        );

        let (ids, _, descriptions) = model_options("apple-intelligence", "polishing", true);
        let apple = ids
            .iter()
            .position(|id| id == "apple-intelligence")
            .unwrap();
        assert_eq!(descriptions[apple], "Built into macOS. No download.");
    }
}
