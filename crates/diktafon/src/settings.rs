//! Settings window, opened from the menu bar: a sidebar of sections
//! (General / Models / History / Advanced) with a titled content pane,
//! built from gpui-component widgets. Edits persist to config.json; the
//! prompt and language apply to the next dictation, the idle-unload time
//! when the daemon restarts.

use crate::config::{HotkeyBehavior, SessionSettings, catalog, transcription_languages};
use crate::control_line;
use crate::icons::DiktafonIcon;
use crate::permissions;
use crate::statusbar::DaemonStatus;
use crate::updater::{self, UpdateCheck};
use crate::{autostart, statusbar, theme};
use chrono::{Datelike, Local, NaiveDate};
use diktafon_protocol::HistoryEntry;
use gpui::{
    Animation, AnimationExt, App, AppContext, Bounds, ClipboardItem, Context, Div, Entity,
    ParentElement, Render, SharedString, Stateful, TitlebarOptions, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, relative, rems, rgba, size,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::form::{Form, field, v_form};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::label::Label;
use gpui_component::list::ListItem;
use gpui_component::searchable_list::SearchableVec;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme, Icon, IndexPath, Root, Sizable, StyledExt, h_flex, v_flex};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Dropdown rows for a model picker, with each row's id, help line, and
/// catalog languages at the same index.
struct ModelOptions {
    ids: Vec<String>,
    labels: Vec<SharedString>,
    descriptions: Vec<String>,
    languages: Vec<Vec<String>>,
}

fn model_options(current: &str, category: &str, apple_available: bool) -> ModelOptions {
    let catalog = catalog();
    let mut ids = Vec::new();
    let mut labels = Vec::new();
    let mut descriptions = Vec::new();
    let mut languages = Vec::new();
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
        languages.push(model.languages);
        ids.push(model.id);
    }
    if !ids.iter().any(|id| id == current) && (current != "apple-intelligence" || apple_available) {
        ids.push(current.to_string());
        labels.push(format!("Unknown model ({current})").into());
        descriptions.push("This model is not available. Choose another model.".into());
        languages.push(Vec::new());
    }
    ModelOptions {
        ids,
        labels,
        descriptions,
        languages,
    }
}

/// Whether polishing is skipped for `language`, so the transcript is pasted
/// raw. A polisher listing no languages is an unknown model, whose own help
/// line already says it is unavailable.
fn skips_polishing(polisher_languages: &[String], language: &str) -> bool {
    let is_unknown = polisher_languages.is_empty();
    !is_unknown && !polisher_languages.iter().any(|code| code == language)
}

fn language_name(code: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|(known, _)| *known == code)
        .map_or(code, |(_, name)| name)
}

/// The polishing model's help line when it skips the selected language.
fn polishing_skipped_notice(language: &str) -> String {
    let language = language_name(language);
    format!("Does not polish {language}. Dictation is pasted as transcribed.")
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
/// How often the Advanced pane re-reads the macOS grants, so a switch
/// flipped in System Settings shows up while the sheet is open.
const PERMISSIONS_POLL: std::time::Duration = std::time::Duration::from_secs(1);
/// Display names for every ISO 639-1 code a transcription model may list,
/// in picker order.
const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("ar", "Arabic"),
    ("zh", "Chinese"),
    ("hr", "Croatian"),
    ("nl", "Dutch"),
    ("fr", "French"),
    ("de", "German"),
    ("el", "Greek"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("es", "Spanish"),
    ("vi", "Vietnamese"),
];

/// The first dropdown row, standing for "no preference".
const SYSTEM_DEFAULT_INPUT: &str = "System default";
/// How often the open settings window re-enumerates microphones, so a
/// device plugged in while the window is open shows up without a button.
const MICROPHONE_RESCAN: std::time::Duration = std::time::Duration::from_secs(2);

/// Dropdown rows for the microphone picker: the system default first, then
/// every connected input, then the saved choice if it is not connected so
/// the preference stays visible and selectable. Returns the device names
/// parallel to the labels (empty for the default) and the selected row.
fn microphone_options(
    connected: &[String],
    current: &str,
) -> (Vec<String>, Vec<SharedString>, usize) {
    let mut names = vec![String::new()];
    let mut labels = vec![SharedString::from(SYSTEM_DEFAULT_INPUT)];
    for name in connected {
        names.push(name.clone());
        labels.push(name.clone().into());
    }
    if !current.is_empty() && !connected.iter().any(|name| name == current) {
        names.push(current.to_string());
        labels.push(format!("{current} (not connected)").into());
    }
    let selected = names.iter().position(|name| name == current).unwrap_or(0);
    (names, labels, selected)
}

/// Dropdown rows for the language picker: the languages the transcription
/// model supports, in [`LANGUAGES`] order. Returns the codes parallel to the
/// labels and the selected row, the first one when `current` is not listed.
fn language_options(
    supported: &[String],
    current: &str,
) -> (Vec<String>, Vec<SharedString>, usize) {
    let (codes, labels): (Vec<String>, Vec<SharedString>) = LANGUAGES
        .iter()
        .filter(|(code, _)| supported.iter().any(|language| language == code))
        .map(|(code, name)| (code.to_string(), format!("{name} ({code})").into()))
        .unzip();
    let selected = codes.iter().position(|code| code == current).unwrap_or(0);
    (codes, labels, selected)
}

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

    fn icon(self) -> DiktafonIcon {
        match self {
            Section::General => DiktafonIcon::Settings,
            Section::Models => DiktafonIcon::Bot,
            Section::History => DiktafonIcon::History,
            Section::Advanced => DiktafonIcon::Tuning,
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
/// clipboard; rows with retained audio also play and delete the dictation.
struct History {
    /// Everything shown, newest first.
    entries: Vec<HistoryEntry>,
    /// The filtered view: one `(day label, entry indices)` per group.
    days: Vec<(String, Vec<usize>)>,
    query: String,
    /// Entry whose text was just copied; drives the brief check-mark flash.
    copied: Option<usize>,
    /// Recording name playing, if any. Keyed by name, not row: deleting one
    /// row must not silence another.
    playing: Option<String>,
    /// Transient feedback under the list ("Dictation deleted.", failures);
    /// cleared on a timer like the copy flash.
    notice: Option<String>,
    player: crate::playback::RecordingPlayer,
    /// Finished reruns by recording name. Names are stable across history
    /// reloads; entry indices are not.
    reruns: HashMap<String, Rerun>,
    /// Recording name of the rerun in flight, if any. Single-flight: one
    /// rerun at a time, so results cannot be misattributed.
    working: Option<String>,
}

/// One retranscription, shown alongside the entry it reran. The original
/// history result is never touched.
#[derive(Clone)]
struct Rerun {
    text: String,
    raw: String,
    asr_model: String,
    polishing_model: Option<String>,
}

impl History {
    fn new() -> Self {
        let mut history = Self {
            entries: load_history(),
            days: Vec::new(),
            query: String::new(),
            copied: None,
            playing: None,
            notice: None,
            player: crate::playback::RecordingPlayer::new(),
            reruns: HashMap::new(),
            working: None,
        };
        history.regroup();
        history
    }

    fn reload(&mut self) {
        self.entries = load_history();
        // An index into the old entries would flash the wrong row.
        // Playback is stopped only when its own entry is deleted; other rows
        // must keep playing across a reload.
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
    polishing_languages: Vec<Vec<String>>,
    apple_prompt_input: Entity<TextareaState>,
    idle_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// Seconds parallel to the idle dropdown items.
    idle_values: Vec<u64>,
    microphone_select: Entity<SelectState<SearchableVec<SharedString>>>,
    /// Device names parallel to the microphone dropdown; empty is the
    /// system default.
    microphone_names: Vec<String>,
    /// Loaded asynchronously: the SMAppService query is a blocking XPC call.
    autostart: bool,
    sound_cues: bool,
    mute_while_recording: bool,
    retain_recordings: bool,
    /// The dictation chord, in global-hotkey syntax.
    hotkey: String,
    hotkey_behavior: HotkeyBehavior,
    /// True while the hotkey control waits for the user to press a new chord.
    capturing_hotkey: bool,
    hotkey_focus: gpui::FocusHandle,
    /// Cached at open: reading it does file IO and must not run per render.
    daemon_status: DaemonStatus,
    /// Polled while the Advanced pane shows; the sheet reads it per render.
    permissions: permissions::Status,
    permissions_sheet_open: bool,
    /// Focused while the sheet is open, so Escape reaches it and closing
    /// hands focus back to the pane.
    permissions_focus: gpui::FocusHandle,
    /// Reloaded when the History section is entered.
    history: History,
    /// Sends retranscription requests to the transport; owned by the control
    /// thread's client, so the window never touches the daemon directly.
    reprocess: crate::transport::ReprocessHandle,
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
    reprocess: crate::transport::ReprocessHandle,
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
                let view = cx.new(|cx| SettingsWindow::new(settings, reprocess, window, cx));
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
pub fn force_dark_titlebar(window: &Window) {
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
        reprocess: crate::transport::ReprocessHandle,
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
        let permissions_focus = cx.focus_handle();

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

        let (language_codes, language_items, language_index) = language_options(
            &transcription_languages(&current.transcription_model),
            &current.language,
        );
        let language_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(language_items),
                Some(IndexPath::new(language_index)),
                window,
                cx,
            )
        });

        let ModelOptions {
            ids: transcription_ids,
            labels: transcription_items,
            descriptions: transcription_descriptions,
            ..
        } = model_options(
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
        let ModelOptions {
            ids: polishing_ids,
            labels: polishing_items,
            descriptions: polishing_descriptions,
            languages: polishing_languages,
        } = model_options(&current.polishing_model, "polishing", apple_available);
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
        // A hand-edited value outside the presets stays selectable.
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

        let (microphone_names, microphone_items, microphone_index) =
            microphone_options(&crate::capture::input_device_names(), &current.input_device);
        let microphone_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(microphone_items),
                Some(IndexPath::new(microphone_index)),
                window,
                cx,
            )
        });
        cx.spawn_in(window, async move |view, cx| {
            loop {
                cx.background_executor().timer(MICROPHONE_RESCAN).await;
                let showing = view.read_with(cx, |view: &Self, _| view.section == Section::General);
                match showing {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => return,
                }
                let connected = cx
                    .background_executor()
                    .spawn(async { crate::capture::input_device_names() })
                    .await;
                let updated = view.update_in(cx, |view: &mut Self, window, cx| {
                    view.refresh_microphones(&connected, window, cx);
                });
                if updated.is_err() {
                    return;
                }
            }
        })
        .detach();

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

        cx.spawn_in(window, async move |view, cx| {
            loop {
                cx.background_executor().timer(PERMISSIONS_POLL).await;
                let showing =
                    view.read_with(cx, |view: &Self, _| view.section == Section::Advanced);
                match showing {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => return,
                }
                let status = cx
                    .background_executor()
                    .spawn(async { permissions::Status::read() })
                    .await;
                let updated = view.update(cx, |view: &mut Self, cx| {
                    if view.permissions != status {
                        view.permissions = status;
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
        cx.subscribe_in(
            &microphone_select,
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

        let view = Self {
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
            polishing_languages,
            apple_prompt_input,
            idle_select,
            idle_values,
            microphone_select,
            microphone_names,
            autostart: false,
            sound_cues: current.sound_cues,
            mute_while_recording: current.mute_while_recording,
            retain_recordings: current.retain_recordings,
            hotkey: current.hotkey.clone(),
            hotkey_behavior: current.hotkey_behavior,
            capturing_hotkey: false,
            hotkey_focus,
            daemon_status,
            permissions: permissions::Status::read(),
            permissions_sheet_open: false,
            permissions_focus: permissions_focus.clone(),
            history,
            reprocess,
            history_search,
            focus_handle,
        };
        // A finished clip reports itself through `is_playing`; poll it so the
        // row drops its playing state with no task per click. Ends with the
        // window: a closed view fails the update and breaks the loop.
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let gone = view
                    .update(cx, |view: &mut Self, cx| {
                        if view.history.playing.is_some() && !view.history.player.is_playing() {
                            view.history.playing = None;
                            cx.notify();
                        }
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        })
        .detach();
        view
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = SessionSettings::default();
        let control_line = control_line::compose(self.control[0], self.control[1], self.control[2]);
        let language = self.selected_language(cx).unwrap_or(defaults.language);
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
            mute_while_recording: self.mute_while_recording,
            retain_recordings: self.retain_recordings,
            hotkey: self.hotkey.clone(),
            hotkey_behavior: self.hotkey_behavior,
            input_device: self.selected_microphone(cx),
            transcription_model,
            onboarded: self.settings.lock().unwrap().onboarded,
            polishing_model,
        };
        let models = updated.models();
        updated.transcription_model = models.transcription.clone();
        updated.normalize_language();
        self.refresh_languages(&updated, window, cx);
        if let Err(e) = updated.save() {
            eprintln!("saving settings failed: {e:#}");
            return;
        }
        *self.settings.lock().unwrap() = updated;
        cx.global::<crate::AppServices>().models.set(models);
    }

    fn selected_language(&self, cx: &App) -> Option<String> {
        self.language_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.language_codes.get(index.row).cloned())
    }

    /// The notice replacing a polishing row's help line when that polisher
    /// skips the selected language.
    fn polishing_skipped_notice(&self, row: usize, cx: &App) -> Option<String> {
        let language = self.selected_language(cx)?;
        let polisher_languages = self.polishing_languages.get(row)?;
        if !skips_polishing(polisher_languages, &language) {
            return None;
        }
        Some(polishing_skipped_notice(&language))
    }

    /// Rebuild the language rows for the settings' transcription model and
    /// select their language.
    fn refresh_languages(
        &mut self,
        settings: &SessionSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (codes, items, index) = language_options(
            &transcription_languages(&settings.transcription_model),
            &settings.language,
        );
        self.language_codes = codes;
        self.language_select.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(items), window, cx);
            select.set_selected_index(Some(IndexPath::new(index)), window, cx);
        });
        cx.notify();
    }

    /// The device name behind the dropdown's selection; empty for the
    /// system default.
    fn selected_microphone(&self, cx: &App) -> String {
        self.microphone_select
            .read(cx)
            .selected_index(cx)
            .and_then(|index| self.microphone_names.get(index.row).cloned())
            .unwrap_or_default()
    }

    /// Rebuild the microphone rows around the current choice when the set
    /// of connected devices changed.
    fn refresh_microphones(
        &mut self,
        connected: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.selected_microphone(cx);
        let (names, items, index) = microphone_options(connected, &current);
        if names == self.microphone_names {
            return;
        }
        self.microphone_names = names;
        self.microphone_select.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(items), window, cx);
            select.set_selected_index(Some(IndexPath::new(index)), window, cx);
        });
        cx.notify();
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
            .h(theme::CONTROL_HEIGHT)
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
        description: impl Into<SharedString>,
        control: impl IntoElement,
        cx: &App,
    ) -> impl IntoElement {
        let description: SharedString = description.into();
        h_flex()
            .justify_between()
            .items_center()
            .gap_6()
            .py_1()
            .child(
                // The text column yields to the control: without a min width
                // a long help line pushes the control off the pane instead of
                // wrapping.
                v_flex()
                    .flex_1()
                    .min_w_0()
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
            .child(div().flex_shrink_0().child(control))
    }

    /// The hotkey as keycap chips.
    fn keycaps(keys: Vec<String>) -> impl IntoElement {
        h_flex().gap_1p5().children(keys.into_iter().map(|key| {
            div()
                .hover(|el| el.border_color(rgba(theme::HAIRLINE | 0x59)))
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

    /// The hotkey leads: it is what the app is about, and the behavior rows
    /// under it are the first-visit explainer. Then the other input setting,
    /// then the two switches.
    fn general_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(
                v_flex()
                    .gap(px(12.))
                    .child(Self::control_row(
                        "Hotkey",
                        "Click the keys to record a new hotkey.",
                        self.hotkey_control(cx),
                        cx,
                    ))
                    .child(self.hotkey_behavior_options(cx)),
            )
            .child(Self::control_row(
                "Microphone",
                "Follows System Settings unless you pick one.",
                Select::new(&self.microphone_select).large().w(px(220.)),
                cx,
            ))
            .child(Self::control_row(
                "Sound cues",
                "Play a sound when a dictation starts, is canceled, or fails.",
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
                "Mute while recording",
                "Silence playback until the dictation ends. Replaces the start sound.",
                Switch::new("mute-while-recording")
                    .large()
                    .checked(self.mute_while_recording)
                    .on_click(cx.listener(|view, checked: &bool, window, cx| {
                        view.mute_while_recording = *checked;
                        view.save(window, cx);
                        cx.notify();
                    })),
                cx,
            ))
            .child(Self::control_row(
                "Keep recordings",
                "Save dictation audio on this Mac so it can be replayed. Latest 20 kept.",
                Switch::new("keep-recordings")
                    .large()
                    .checked(self.retain_recordings)
                    .on_click(cx.listener(|view, checked: &bool, window, cx| {
                        view.retain_recordings = *checked;
                        view.save(window, cx);
                        cx.notify();
                    })),
                cx,
            ))
            .child(Self::control_row(
                "Open at login",
                "Open Diktafon when you log in.",
                Switch::new("autostart")
                    .large()
                    .checked(self.autostart)
                    .on_click(
                        cx.listener(|view, checked: &bool, _, cx| view.set_autostart(*checked, cx)),
                    ),
                cx,
            ))
    }

    /// The two ways the hotkey can drive a dictation, as radio rows that
    /// name the gesture and its result with the current chord inline, so the
    /// choice explains itself (docs/mockups/settings.html).
    fn hotkey_behavior_options(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let caps = Self::hotkey_caps(&self.hotkey);
        let options = [
            (
                HotkeyBehavior::Hold,
                "Hold to talk",
                "Hold",
                "while you speak. Release to paste.",
            ),
            (
                HotkeyBehavior::Toggle,
                "Press to toggle",
                "Press",
                "to start. Press again to paste.",
            ),
        ];
        v_flex()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(rgba(theme::SURFACE | 0xFF))
            .overflow_hidden()
            .children(options.into_iter().enumerate().map(
                |(index, (behavior, name, verb, rest))| {
                    let selected = self.hotkey_behavior == behavior;
                    h_flex()
                        .id(("hotkey-behavior", index))
                        .items_start()
                        .gap_3()
                        .px(px(14.))
                        .py(px(12.))
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(cx.theme().border)
                        })
                        .when(selected, |row| row.bg(rgba(theme::SURFACE_RAISED | 0xFF)))
                        .when(!selected, |row| {
                            row.hover(|row| row.bg(rgba(theme::SURFACE_RAISED | 0x99)))
                        })
                        .child(Self::radio(selected))
                        .child(
                            v_flex()
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .line_height(px(22.))
                                        .font_medium()
                                        .child(name),
                                )
                                .child(
                                    h_flex()
                                        .mt(px(2.))
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(5.))
                                        .line_height(px(20.))
                                        .text_size(px(13.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(verb)
                                        .children(Self::mini_keycaps(caps.clone(), selected))
                                        .child(rest),
                                ),
                        )
                        .on_click(cx.listener(move |view, _, window, cx| {
                            if view.hotkey_behavior != behavior {
                                view.hotkey_behavior = behavior;
                                view.save(window, cx);
                                cx.notify();
                            }
                        }))
                },
            ))
    }

    /// A radio indicator: an outlined ring, filled with the accent and a
    /// white dot when selected.
    fn radio(selected: bool) -> impl IntoElement {
        div()
            .size(px(16.))
            .mt(px(3.))
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .when(selected, |el| {
                el.border_color(rgba(theme::ACCENT | 0xFF))
                    .bg(rgba(theme::ACCENT | 0xFF))
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(rgba(theme::TEXT_PRIMARY | 0xFF)),
                    )
            })
            .when(!selected, |el| {
                el.border_color(rgba(theme::RING_IDLE | 0xFF))
            })
    }

    /// Keycaps sized for inline use in a description line. On a raised row
    /// they sit on the window background so they still read as keys.
    fn mini_keycaps(keys: Vec<String>, on_raised: bool) -> impl Iterator<Item = impl IntoElement> {
        let background = if on_raised {
            rgba(theme::BACKGROUND | 0xFF)
        } else {
            rgba(theme::SURFACE_RAISED | 0xFF)
        };
        keys.into_iter().map(move |key| {
            div()
                .h(px(20.))
                .min_w(px(20.))
                .px(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .bg(background)
                .border_1()
                .border_color(rgba(theme::HAIRLINE | 0x22))
                .text_size(px(12.))
                .font_medium()
                .text_color(rgba(theme::TEXT_PRIMARY | 0xFF))
                .child(key)
        })
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
        // Magenta = alive, dim idle = not; the dot never glows (glow means live
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

    fn close_permissions_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.permissions_sheet_open = false;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// The Permissions sheet: a backdrop over the whole window with a
    /// centered card holding both grants, their state, and the one action
    /// that fixes each. It reads the polled status, so it updates live.
    fn permissions_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let card = v_flex()
            .id("permissions-sheet")
            // Clicks on the card stay on the card; only the backdrop closes.
            .occlude()
            .track_focus(&self.permissions_focus)
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    view.close_permissions_sheet(window, cx);
                }
            }))
            .w(px(600.))
            .p_6()
            .gap_4()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(rgba(theme::SURFACE | 0xFF))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family(theme::FONT_DISPLAY)
                            .text_2xl()
                            .font_semibold()
                            .child("Permissions"),
                    )
                    .child(
                        // A 28px square target; the kit's small ghost button
                        // is only as big as its glyph.
                        div()
                            .id("close-permissions")
                            .size(px(28.))
                            .mr(px(-6.))
                            .rounded_md()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgba(theme::TEXT_FAINT | 0xFF))
                            .hover(|el| {
                                el.bg(rgba(theme::SURFACE_RAISED | 0xFF))
                                    .text_color(rgba(theme::TEXT_PRIMARY | 0xFF))
                            })
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.close_permissions_sheet(window, cx)
                            }))
                            .child(Icon::new(DiktafonIcon::CloseCircle).size_4()),
                    ),
            )
            .child(crate::permission_ui::permission_list(self.permissions, cx));
        let backdrop = div()
            .id("permissions-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().overlay)
            .on_click(cx.listener(|view, _, window, cx| view.close_permissions_sheet(window, cx)))
            .child(card);
        if cx.reduce_motion() {
            backdrop.into_any_element()
        } else {
            backdrop
                .with_animation(
                    "permissions-sheet-in",
                    Animation::new(std::time::Duration::from_millis(150))
                        .with_easing(gpui::ease_out_quint()),
                    |layer, delta| layer.opacity(delta),
                )
                .into_any_element()
        }
    }

    /// The running version with the outcome of Sparkle's last check, a button
    /// that opens Sparkle's window, and the automatic-check switch. A build
    /// that does not update itself shows only the version.
    fn updates_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let check = updater::status(cx).map(|status| status.read(cx).clone());
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
                        .h(theme::CONTROL_HEIGHT)
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
        let polishing_row = self
            .polishing_select
            .read(cx)
            .selected_index(cx)
            .map(|index| index.row);
        let skipped_notice = polishing_row.and_then(|row| self.polishing_skipped_notice(row, cx));
        let polishes = skipped_notice.is_none();
        let polishing_field = field().label("Polishing model");
        let polishing_field = match skipped_notice {
            Some(notice) => polishing_field.description_fn(move |_, cx| {
                div().text_color(cx.theme().warning).child(notice.clone())
            }),
            None => {
                let description = polishing_row
                    .and_then(|row| self.polishing_descriptions.get(row).cloned())
                    .unwrap_or_default();
                polishing_field.description(description)
            }
        };
        let selected_polisher = polishing_row.and_then(|row| self.polishing_ids.get(row));
        let uses_s1_prompt = polishes && selected_polisher.is_some_and(|id| id == "s1-mini-q4-k-m");
        let uses_apple_prompt =
            polishes && selected_polisher.is_some_and(|id| id == "apple-intelligence");
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
                Self::form()
                    .child(polishing_field.child(Select::new(&self.polishing_select).large())),
            )
            .when(uses_s1_prompt, |card| card.child(self.polishing_prompt(cx)))
            .when(uses_apple_prompt, |card| card.child(self.apple_prompt()));
        v_flex().gap_8().child(transcription).child(polishing)
    }

    fn advanced_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_8()
            .child(Self::control_row(
                "Permissions",
                self.permissions.summary(),
                Button::new("review-permissions")
                    .label("Review…")
                    .outline()
                    .h(theme::CONTROL_HEIGHT)
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.permissions_sheet_open = true;
                        view.permissions_focus.focus(window, cx);
                        cx.notify();
                    })),
                cx,
            ))
            .child(
                Self::form().child(
                    field()
                        .label("Unload models when idle")
                        .description(
                            "Frees memory between dictations. The next one starts a moment slower while models reload.",
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

    /// A 28px History row icon button. `active` tints it the polishing
    /// magenta, `danger` turns the hover red, `accent` paints it the action
    /// accent for the primary row decision.
    fn history_button(
        id: (&'static str, usize),
        icon: impl Into<Icon>,
        active: bool,
        danger: bool,
        accent: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .text_color(if active {
                rgba(theme::SIGNAL_MAGENTA | 0xFF)
            } else if accent {
                rgba(theme::ACCENT_TEXT | 0xFF)
            } else {
                rgba(theme::TEXT_FAINT | 0xFF)
            })
            .hover(|el| {
                let hovered = if accent {
                    el.bg(rgba(theme::ACCENT | 0x2E))
                        .text_color(rgba(theme::ACCENT_TEXT | 0xFF))
                } else {
                    el.bg(rgba(theme::HAIRLINE | 0x22))
                        .text_color(rgba(theme::TEXT_PRIMARY | 0xFF))
                };
                if danger {
                    hovered.text_color(rgba(theme::SIGNAL_RED | 0xFF))
                } else {
                    hovered
                }
            })
            .child(Icon::new(icon).small())
    }

    /// Display name for a model id ("Canary 1B Flash"), falling back to the
    /// id when the catalog does not know it.
    fn model_display_name(id: &str) -> String {
        catalog()
            .models
            .into_iter()
            .find(|model| model.id == id)
            .map(|model| model.name)
            .unwrap_or_else(|| id.to_string())
    }

    fn rerun_caption(rerun: &Rerun) -> String {
        let asr = Self::model_display_name(&rerun.asr_model);
        match &rerun.polishing_model {
            Some(polishing_model) => {
                format!("{asr} + {}", Self::model_display_name(polishing_model))
            }
            None => asr,
        }
    }

    /// Word-level diff of the rerun against the original: rerun-side words
    /// the original does not have are flagged, so differences pop without
    /// reading both texts twice.
    fn diff_tokens(original: &str, rerun: &str) -> Vec<(String, bool)> {
        let a: Vec<&str> = original.split_whitespace().collect();
        let b: Vec<&str> = rerun.split_whitespace().collect();
        let mut lcs = vec![vec![0; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                lcs[i][j] = if a[i] == b[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            if a[i] == b[j] {
                out.push((b[j].to_string(), false));
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                i += 1;
            } else {
                out.push((b[j].to_string(), true));
                j += 1;
            }
        }
        while j < b.len() {
            out.push((b[j].to_string(), true));
            j += 1;
        }
        out
    }

    /// Play `name`, or stop it when it is the one playing. Failures report in
    /// the notice; the row keeps whatever state shows the truth.
    fn toggle_playback(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.history.playing.as_deref() == Some(name) {
            self.history.player.stop();
            self.history.playing = None;
            return;
        }
        let Some(path) = crate::recordings::path(name) else {
            self.set_notice("Audio file is missing.".into(), cx);
            return;
        };
        match self.history.player.play(&path) {
            Ok(()) => {
                self.history.playing = Some(name.to_string());
                self.history.notice = None;
            }
            Err(e) => self.set_notice(format!("Could not play the recording: {e:#}"), cx),
        }
    }

    /// Rerun the entry's retained clip through the currently selected models.
    /// Single-flight; the original result is never touched and neither
    /// history nor audio is written. Every outcome reports in place.
    fn retranscribe(&mut self, entry_ix: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.history.entries.get(entry_ix).cloned() else {
            return;
        };
        let Some(name) = entry.recording.clone() else {
            return;
        };
        if self.history.working.is_some() {
            self.set_notice("A rerun is already in progress.".into(), cx);
            return;
        }
        if self.history.playing.as_deref() == Some(&name) {
            self.history.player.stop();
            self.history.playing = None;
        }
        let Some(path) = crate::recordings::path(&name) else {
            self.set_notice("Audio file is missing.".into(), cx);
            return;
        };
        let samples = match crate::recordings::samples(&path) {
            Ok(samples) => samples,
            Err(e) => {
                self.set_notice(format!("Could not read the recording: {e:#}"), cx);
                return;
            }
        };
        let (config, asr_model, polishing_model) = {
            let settings = self.settings.lock().unwrap();
            let models = settings.models();
            let mut config = settings.session();
            config.no_history = true;
            config.recording = String::new();
            (config, models.transcription, models.polishing)
        };
        let Some(reply_rx) = self.reprocess.request(samples, config) else {
            self.set_notice("Dictation client is gone.".into(), cx);
            return;
        };
        self.history.working = Some(name.clone());
        cx.notify();
        cx.spawn(async move |view, cx| {
            let answer = cx
                .background_executor()
                .spawn(async move { reply_rx.recv() })
                .await;
            let _ = view.update(cx, |view: &mut Self, cx| {
                // A delete or reload may have cleared the working state;
                // only the matching rerun may claim the answer.
                if view.history.working.as_deref() != Some(&name) {
                    return;
                }
                view.history.working = None;
                match answer {
                    Ok(Ok(result)) if !result.polished.trim().is_empty() => {
                        view.history.reruns.insert(
                            name.clone(),
                            Rerun {
                                text: result.polished,
                                raw: result.raw,
                                asr_model: asr_model.clone(),
                                polishing_model: result
                                    .polish_ms
                                    .is_some()
                                    .then(|| polishing_model.clone()),
                            },
                        );
                    }
                    Ok(Ok(_)) => {
                        view.set_notice("The rerun came back empty.".into(), cx);
                    }
                    Ok(Err(reason)) => {
                        view.set_notice(reason, cx);
                    }
                    Err(_) => {
                        view.set_notice("Retranscription was interrupted.".into(), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Accept the rerun: copy its text for use elsewhere and dismiss the
    /// card. The original history entry is left untouched.
    fn accept_rerun(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(entry) = self
            .history
            .entries
            .iter()
            .find(|entry| entry.recording.as_deref() == Some(name))
            .cloned()
        else {
            return;
        };
        let Some(rerun) = self.history.reruns.remove(name) else {
            return;
        };
        let mut adopted = entry.clone();
        adopted.raw = rerun.raw.clone();
        adopted.polished = rerun.text.clone();
        adopted.transcription_model = Some(rerun.asr_model.clone());
        adopted.polishing_model = rerun.polishing_model.clone();
        match diktafon_protocol::history::replace_matching(
            &diktafon_protocol::history::path(),
            &entry,
            adopted,
        ) {
            Ok(true) => {
                self.history.reload();
                cx.write_to_clipboard(ClipboardItem::new_string(rerun.text));
                self.set_notice("Rerun accepted. Copied to clipboard.".into(), cx);
            }
            Ok(false) => {
                self.set_notice("Dictation is already gone.".into(), cx);
            }
            Err(e) => {
                self.set_notice(format!("Could not update history: {e:#}"), cx);
            }
        }
    }

    /// Dismiss the rerun without using it. Silent: the disappearing card is
    /// the feedback.
    fn reject_rerun(&mut self, name: &str) {
        self.history.reruns.remove(name);
    }

    /// Delete the whole dictation: its retained audio, if any, then its
    /// history line. Audio goes first, so a half-failure leaves a transcript
    /// with a gracefully handled dangling reference rather than an orphaned
    /// clip. Either failure keeps the row and reports in the notice.
    fn delete_entry(&mut self, entry_ix: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.history.entries.get(entry_ix).cloned() else {
            return;
        };
        if self.history.working.is_some() {
            self.set_notice("Wait for the rerun to finish.".into(), cx);
            return;
        }
        if self.history.playing == entry.recording {
            self.history.player.stop();
            self.history.playing = None;
        }
        if let Some(name) = entry.recording.as_deref()
            && let Err(e) = crate::recordings::delete(name)
        {
            self.set_notice(format!("Could not delete the recording: {e:#}"), cx);
            return;
        }
        match diktafon_protocol::history::remove_matching(
            &diktafon_protocol::history::path(),
            &entry,
        ) {
            Ok(true) => {
                self.history.reload();
                self.set_notice("Dictation deleted.".into(), cx);
            }
            Ok(false) => {
                self.history.reload();
                self.set_notice("Dictation is already gone.".into(), cx);
            }
            Err(e) => self.set_notice(format!("Could not delete the dictation: {e:#}"), cx),
        }
    }

    /// Show transient feedback under the list, cleared after a few seconds
    /// like the copy flash. Only the latest notice survives: a stale timer
    /// for an older one must not clear its replacement.
    fn set_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.history.notice = Some(text.clone());
        cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2500))
                .await;
            let _ = view.update(cx, |view: &mut Self, cx| {
                if view.history.notice.as_deref() == Some(&text) {
                    view.history.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn history_row(&self, entry_ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let entry = &self.history.entries[entry_ix];
        let copied = self.history.copied == Some(entry_ix);
        let copy_text = entry.polished.clone();
        // Play pairs with the clip; delete removes the whole dictation, so
        // every row gets one — transcript-only entries are deletable too,
        // and a missing file leaves copy and delete.
        let play_name = entry
            .recording
            .clone()
            .filter(|name| crate::recordings::exists(name));
        let playing = play_name
            .as_deref()
            .is_some_and(|name| self.history.playing.as_deref() == Some(name));
        let rerun = entry
            .recording
            .as_deref()
            .and_then(|name| self.history.reruns.get(name))
            .cloned();
        // A rerun in flight replaces the row's result display until it lands;
        // the stale text would read as the answer meanwhile.
        let working_here = entry
            .recording
            .as_deref()
            .is_some_and(|name| self.history.working.as_deref() == Some(name));
        let rerun = rerun.filter(|_| !working_here);
        ListItem::new(("dictation", entry_ix))
            .px(px(10.))
            .py(px(10.))
            .rounded(px(8.))
            .child(
                h_flex()
                    .items_start()
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
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .line_height(relative(1.5))
                                    .line_clamp(2)
                                    .child(SharedString::from(entry.polished.clone())),
                            )
                            .child(
                                h_flex()
                                    .gap(px(2.))
                                    .items_center()
                                    .when_some(play_name.clone(), |el, name| {
                                        el.child(
                                            Self::history_button(
                                                ("play", entry_ix),
                                                if playing {
                                                    DiktafonIcon::Pause
                                                } else {
                                                    DiktafonIcon::Play
                                                },
                                                playing,
                                                false,
                                                false,
                                            )
                                            .on_click(
                                                cx.listener(move |view, _, _, cx| {
                                                    view.toggle_playback(&name, cx);
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                    })
                                    .when_some(play_name, |el, name| {
                                        let working =
                                            self.history.working.as_deref() == Some(&name);
                                        el.child(
                                            Self::history_button(
                                                ("retranscribe", entry_ix),
                                                DiktafonIcon::Restart,
                                                working,
                                                false,
                                                false,
                                            )
                                            .on_click(
                                                cx.listener(move |view, _, _, cx| {
                                                    view.retranscribe(entry_ix, cx);
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                    })
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
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy_text.clone(),
                                                ));
                                                view.history.copied = Some(entry_ix);
                                                cx.notify();
                                                cx.spawn(async move |view, cx| {
                                                    cx.background_executor()
                                                        .timer(std::time::Duration::from_millis(
                                                            1500,
                                                        ))
                                                        .await;
                                                    let _ =
                                                        view.update(cx, |view: &mut Self, cx| {
                                                            if view.history.copied == Some(entry_ix)
                                                            {
                                                                view.history.copied = None;
                                                                cx.notify();
                                                            }
                                                        });
                                                })
                                                .detach();
                                            }))
                                            .child(
                                                Icon::new(if copied {
                                                    DiktafonIcon::Check
                                                } else {
                                                    DiktafonIcon::Copy
                                                })
                                                .small(),
                                            ),
                                    )
                                    .child(
                                        Self::history_button(
                                            ("delete", entry_ix),
                                            DiktafonIcon::TrashBin,
                                            false,
                                            true,
                                            false,
                                        )
                                        .on_click(
                                            cx.listener(move |view, _, _, cx| {
                                                view.delete_entry(entry_ix, cx);
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .when(working_here, |el| {
                                el.child(
                                    div()
                                        .mt(px(8.))
                                        .py(px(12.))
                                        .px(px(14.))
                                        .rounded(px(8.))
                                        .bg(rgba(theme::SURFACE | 0xFF))
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Retranscribing…"),
                                        )
                                        .child(
                                            v_flex()
                                                .mt(px(8.))
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .h(px(14.))
                                                        .rounded(px(4.))
                                                        .bg(rgba(theme::SURFACE_RAISED | 0xFF)),
                                                )
                                                .child(
                                                    div()
                                                        .h(px(14.))
                                                        .rounded(px(4.))
                                                        .bg(rgba(theme::SURFACE_RAISED | 0xFF)),
                                                ),
                                        ),
                                )
                            })
                            .when_some(rerun, |el, rerun| {
                                let caption = SharedString::from(Self::rerun_caption(&rerun));
                                let words = Self::diff_tokens(&entry.polished, &rerun.text);
                                el.child(
                                    div()
                                        .mt(px(8.))
                                        .py(px(12.))
                                        .px(px(14.))
                                        .rounded(px(8.))
                                        .bg(rgba(theme::SURFACE | 0xFF))
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .child(
                                            div()
                                                .flex()
                                                .flex_wrap()
                                                .text_size(px(15.))
                                                .line_height(relative(1.5))
                                                .children(words.into_iter().map(
                                                    |(word, changed)| {
                                                        div()
                                                            .when(changed, |el| {
                                                                el.bg(rgba(
                                                                    theme::SIGNAL_GREEN | 0x38,
                                                                ))
                                                                .rounded(px(3.))
                                                                .px(px(1.))
                                                            })
                                                            .child(SharedString::from(format!(
                                                                "{word}\u{a0}"
                                                            )))
                                                    },
                                                )),
                                        )
                                        .child(
                                            h_flex()
                                                .mt(px(8.))
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(caption),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap(px(2.))
                                                        .items_center()
                                                        .child(
                                                            Self::history_button(
                                                                ("accept-rerun", entry_ix),
                                                                DiktafonIcon::Check,
                                                                false,
                                                                false,
                                                                true,
                                                            )
                                                            .on_click(cx.listener(
                                                                move |view, _, _, cx| {
                                                                    // Reborrowed per click: the map
                                                                    // may have changed since render.
                                                                    if let Some(name) = view
                                                                        .history
                                                                        .entries
                                                                        .get(entry_ix)
                                                                        .and_then(|entry| {
                                                                            entry.recording.clone()
                                                                        })
                                                                    {
                                                                        view.accept_rerun(
                                                                            &name, cx,
                                                                        );
                                                                    }
                                                                    cx.notify();
                                                                },
                                                            )),
                                                        )
                                                        .child(
                                                            Self::history_button(
                                                                ("reject-rerun", entry_ix),
                                                                DiktafonIcon::X,
                                                                false,
                                                                true,
                                                                false,
                                                            )
                                                            .on_click(cx.listener(
                                                                move |view, _, _, cx| {
                                                                    if let Some(name) = view
                                                                        .history
                                                                        .entries
                                                                        .get(entry_ix)
                                                                        .and_then(|entry| {
                                                                            entry.recording.clone()
                                                                        })
                                                                    {
                                                                        view.reject_rerun(&name);
                                                                    }
                                                                    cx.notify();
                                                                },
                                                            )),
                                                        ),
                                                ),
                                        ),
                                )
                            }),
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
            .when_some(self.history.notice.clone(), |el, text| {
                el.child(
                    div()
                        .px(px(2.))
                        .text_size(px(13.))
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(text)),
                )
            })
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
            .relative()
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
            .when(self.permissions_sheet_open, |el| {
                el.child(self.permissions_sheet(cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_flags_only_rerun_side_changes() {
        let unchanged = SettingsWindow::diff_tokens("a b c", "a b c");
        assert!(unchanged.iter().all(|(_, changed)| !changed));

        // "anodamone" splits into two words; both flag, neighbours do not.
        let fixed = SettingsWindow::diff_tokens("keep anodamone removal", "keep a daemon removal");
        let changed: Vec<&str> = fixed
            .iter()
            .filter(|(_, changed)| *changed)
            .map(|(word, _)| word.as_str())
            .collect();
        assert_eq!(changed, vec!["a", "daemon"]);

        // Trailing additions flag; empty reruns flag nothing.
        let appended = SettingsWindow::diff_tokens("a b", "a b c d");
        assert_eq!(
            appended.iter().filter(|(_, c)| *c).count(),
            2,
            "{appended:?}"
        );
        assert!(
            SettingsWindow::diff_tokens("a b", "")
                .iter()
                .all(|(_, c)| !c)
        );
    }

    #[test]
    fn model_names_fall_back_to_ids() {
        assert_eq!(
            SettingsWindow::model_display_name("canary-1b-flash-q5-k-m"),
            "Canary 1B Flash"
        );
        assert_eq!(
            SettingsWindow::model_display_name("no-such-model"),
            "no-such-model"
        );
    }

    #[test]
    fn every_catalog_transcription_language_has_a_name() {
        let transcription_models = catalog()
            .models
            .into_iter()
            .filter(|model| model.category == "transcription");
        for model in transcription_models {
            for language in &model.languages {
                assert!(
                    LANGUAGES.iter().any(|(code, _)| code == language),
                    "{} lists {language} without a display name",
                    model.id
                );
            }
        }
    }

    #[test]
    fn every_polisher_lists_named_languages() {
        let polishers = catalog()
            .models
            .into_iter()
            .filter(|model| model.category == "polishing");
        for model in polishers {
            assert!(
                !model.languages.is_empty(),
                "{} lists no languages",
                model.id
            );
            for language in &model.languages {
                assert!(
                    LANGUAGES.iter().any(|(code, _)| code == language),
                    "{} lists {language} without a display name",
                    model.id
                );
            }
        }
    }

    #[test]
    fn polishing_is_skipped_only_for_unlisted_languages() {
        let english_only = vec!["en".to_string()];
        assert!(!skips_polishing(&english_only, "en"));
        assert!(skips_polishing(&english_only, "hr"));
        assert!(
            !skips_polishing(&[], "hr"),
            "an unknown model keeps its own help"
        );
    }

    #[test]
    fn catalog_polishers_skip_croatian() {
        let ModelOptions { ids, languages, .. } = model_options(
            diktafon_protocol::DEFAULT_POLISHING_MODEL,
            "polishing",
            true,
        );
        let languages_of = |id: &str| {
            let row = ids.iter().position(|known| known == id).unwrap();
            &languages[row]
        };
        assert!(!skips_polishing(languages_of("s1-mini-q4-k-m"), "en"));
        assert!(skips_polishing(languages_of("s1-mini-q4-k-m"), "hr"));
        assert!(skips_polishing(languages_of("apple-intelligence"), "hr"));
    }

    #[test]
    fn unknown_polisher_row_lists_no_languages() {
        let ModelOptions { ids, languages, .. } =
            model_options("retired-polisher", "polishing", false);
        let row = ids.iter().position(|id| id == "retired-polisher").unwrap();
        assert!(languages[row].is_empty());
    }

    #[test]
    fn skipped_notice_names_the_language() {
        assert_eq!(
            polishing_skipped_notice("hr"),
            "Does not polish Croatian. Dictation is pasted as transcribed."
        );
    }

    #[test]
    fn language_options_filter_and_order() {
        let supported = vec!["fr".to_string(), "en".to_string(), "de".to_string()];
        let (codes, labels, selected) = language_options(&supported, "de");
        assert_eq!(codes, vec!["en", "fr", "de"]);
        let labels: Vec<&str> = labels.iter().map(|label| label.as_ref()).collect();
        assert_eq!(labels, vec!["English (en)", "French (fr)", "German (de)"]);
        assert_eq!(selected, 2);
    }

    #[test]
    fn microphone_rows_keep_a_disconnected_choice_selectable() {
        let connected = vec!["MacBook Pro Microphone".to_string()];
        let (names, labels, selected) = microphone_options(&connected, "");
        assert_eq!(names, vec!["", "MacBook Pro Microphone"]);
        assert_eq!(labels[0].as_ref(), SYSTEM_DEFAULT_INPUT);
        assert_eq!(selected, 0);

        let (names, labels, selected) = microphone_options(&connected, "AirPods");
        assert_eq!(names[2], "AirPods");
        assert_eq!(labels[2].as_ref(), "AirPods (not connected)");
        assert_eq!(selected, 2);

        let (_, labels, selected) = microphone_options(&connected, "MacBook Pro Microphone");
        assert_eq!(labels.len(), 2);
        assert_eq!(selected, 1);
    }

    #[test]
    fn apple_intelligence_is_only_offered_when_available() {
        let unavailable = model_options(
            diktafon_protocol::DEFAULT_POLISHING_MODEL,
            "polishing",
            false,
        )
        .ids;
        assert!(!unavailable.iter().any(|id| id == "apple-intelligence"));

        let available = model_options(
            diktafon_protocol::DEFAULT_POLISHING_MODEL,
            "polishing",
            true,
        )
        .ids;
        assert!(available.iter().any(|id| id == "apple-intelligence"));
    }

    #[test]
    fn model_descriptions_are_brief() {
        let descriptions = model_options(
            diktafon_protocol::DEFAULT_TRANSCRIPTION_MODEL,
            "transcription",
            false,
        )
        .descriptions;
        assert!(
            descriptions
                .iter()
                .all(|description| !description.contains(';')
                    && description.split_whitespace().count() <= 6)
        );

        let ModelOptions {
            ids, descriptions, ..
        } = model_options("apple-intelligence", "polishing", true);
        let apple = ids
            .iter()
            .position(|id| id == "apple-intelligence")
            .unwrap();
        assert_eq!(descriptions[apple], "Built into macOS. No download.");
    }
}
