//! Menu bar presence: the app is otherwise invisible when idle and
//! unquittable without pkill. An NSStatusItem whose icon tracks the phase,
//! with a menu (rebuilt on every open via NSMenuDelegate) showing daemon and
//! model status, recent dictations (click to copy), the autostart toggle, and
//! quit.

use crate::autostart;
use crate::dictation::{Dictation, Phase};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, Entity};
use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use std::cell::{Cell, OnceCell, RefCell};

/// How many recent dictations the menu offers to copy.
const HISTORY_SHOWN: usize = 5;
/// Menu titles stay skimmable; the full text goes in the tooltip.
const HISTORY_TITLE_CHARS: usize = 44;
/// More than enough bytes for the last [`HISTORY_SHOWN`] entries.
const HISTORY_TAIL_BYTES: u64 = 256 * 1024;

enum MenuAction {
    Quit,
    QuitDaemonToo,
}

struct ControllerIvars {
    actions: UnboundedSender<MenuAction>,
    /// Full texts behind the currently shown history items, indexed by tag.
    history: RefCell<Vec<String>>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    phase: Cell<Phase>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ControllerIvars]
    struct MenuController;

    unsafe impl NSObjectProtocol for MenuController {}

    unsafe impl NSMenuDelegate for MenuController {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.rebuild(menu);
        }
    }

    impl MenuController {
        #[unsafe(method(copyHistoryItem:))]
        fn copy_history_item(&self, sender: &NSMenuItem) {
            let index = sender.tag() as usize;
            let Some(text) = self.ivars().history.borrow().get(index).cloned() else {
                return;
            };
            let copied = arboard::Clipboard::new().and_then(|mut c| c.set_text(text));
            if let Err(e) = copied {
                eprintln!("copying dictation failed: {e}");
            }
        }

        #[unsafe(method(toggleAutostart:))]
        fn toggle_autostart(&self, _sender: &NSMenuItem) {
            if let Err(e) = autostart::set(!autostart::is_enabled()) {
                eprintln!("autostart change failed: {e:#}");
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &NSMenuItem) {
            let _ = self.ivars().actions.unbounded_send(MenuAction::Quit);
        }

        #[unsafe(method(quitAll:))]
        fn quit_all(&self, _sender: &NSMenuItem) {
            let _ = self.ivars().actions.unbounded_send(MenuAction::QuitDaemonToo);
        }
    }
);

impl MenuController {
    fn new(mtm: MainThreadMarker, actions: UnboundedSender<MenuAction>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ControllerIvars {
            actions,
            history: RefCell::new(Vec::new()),
            status_item: OnceCell::new(),
            phase: Cell::new(Phase::Idle),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn set_phase(&self, phase: Phase) {
        self.ivars().phase.set(phase);
        // Abstract marks echoing the pill's orbit; deliberately not the mic
        // glyphs, which read as macOS's own mic-in-use indicator. Fallbacks
        // cover older SF Symbols sets.
        let candidates: &[&str] = match phase {
            Phase::Idle => &["circle.dotted", "circle"],
            Phase::Arming | Phase::Recording => &["record.circle", "largecircle.fill.circle"],
            Phase::Transcribing | Phase::Polishing => &["waveform.circle", "ellipsis.circle"],
        };
        if let Some(item) = self.ivars().status_item.get()
            && let Some(button) = item.button(self.mtm())
        {
            let image = candidates.iter().find_map(|symbol| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    None,
                )
            });
            button.setImage(image.as_deref());
        }
    }

    fn rebuild(&self, menu: &NSMenu) {
        let mtm = self.mtm();
        menu.removeAllItems();

        self.add_info(
            menu,
            &format!("Diktafon: {}", phase_label(self.ivars().phase.get())),
        );
        self.add_info(menu, &daemon_status_line());
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let history = recent_history(HISTORY_SHOWN);
        if history.is_empty() {
            self.add_info(menu, "No dictations yet");
        }
        for (index, text) in history.iter().enumerate() {
            let item = self.add_action(menu, &menu_title(text), sel!(copyHistoryItem:));
            item.setTag(index as isize);
            item.setToolTip(Some(&NSString::from_str(text)));
        }
        self.ivars().history.replace(history);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let auto = self.add_action(menu, "Start at Login", sel!(toggleAutostart:));
        if autostart::is_enabled() {
            auto.setState(NSControlStateValueOn);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        self.add_action(menu, "Quit Diktafon", sel!(quit:));
        self.add_action(menu, "Quit Diktafon and Daemon", sel!(quitAll:));
    }

    /// A disabled line of information (no action, so AppKit disables it).
    fn add_info(&self, menu: &NSMenu, title: &str) {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                None,
                &NSString::from_str(""),
            )
        };
        menu.addItem(&item);
    }

    fn add_action(&self, menu: &NSMenu, title: &str, action: Sel) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(""),
            )
        };
        unsafe { item.setTarget(Some(self.as_ref())) };
        menu.addItem(&item);
        item
    }
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "Idle",
        Phase::Arming => "Starting",
        Phase::Recording => "Listening",
        Phase::Transcribing => "Transcribing",
        Phase::Polishing => "Polishing",
    }
}

fn menu_title(text: &str) -> String {
    let mut title: String = text.chars().take(HISTORY_TITLE_CHARS).collect();
    if text.chars().count() > HISTORY_TITLE_CHARS {
        title.push('…');
    }
    title
}

/// One line summarizing the daemon from its status.json (see
/// `diktafon_protocol::status_path`); the pid check guards against a stale
/// file left by a crashed daemon.
fn daemon_status_line() -> String {
    let path = diktafon_protocol::status_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        // A daemon from before the status file exists only in the pidfile.
        let pid = std::fs::read_to_string(diktafon_protocol::pid_path())
            .ok()
            .and_then(|pid| pid.trim().parse().ok())
            .unwrap_or(0);
        return if pid_is_diktafond(pid) {
            "Daemon: running".into()
        } else {
            "Daemon: not running".into()
        };
    };
    let Ok(status) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return "Daemon: unknown".into();
    };
    let pid = status["pid"].as_u64().unwrap_or(0);
    if !pid_is_diktafond(pid) {
        return "Daemon: not running".into();
    }
    let asr = status["asr_model"].as_str().unwrap_or("asr");
    let llm = status["llm_model"].as_str().unwrap_or("llm");
    if status["models_loaded"].as_bool().unwrap_or(false) {
        format!("Daemon: {asr} + {llm} loaded")
    } else {
        "Daemon: running, models unloaded".into()
    }
}

fn pid_is_diktafond(pid: u64) -> bool {
    if pid == 0 {
        return false;
    }
    std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .ends_with("diktafond")
        })
        .unwrap_or(false)
}

/// Newest first. Prefers the polished text; falls back to the raw transcript.
/// Reads only the file's tail: this runs synchronously while the menu opens,
/// and the history grows without bound.
fn recent_history(count: usize) -> Vec<String> {
    let path = diktafon_protocol::data_dir().join("history.jsonl");
    let Ok(raw) = read_tail(&path, HISTORY_TAIL_BYTES) else {
        return Vec::new();
    };
    raw.lines()
        .rev()
        .filter_map(|line| {
            let entry = serde_json::from_str::<serde_json::Value>(line).ok()?;
            let text = match entry["polished"].as_str() {
                Some(polished) if !polished.is_empty() => polished,
                _ => entry["raw"].as_str()?,
            };
            (!text.is_empty()).then(|| text.to_string())
        })
        .take(count)
        .collect()
}

/// The file's last `max_bytes`, starting at the first complete line (the
/// seek can land mid-line and even mid-UTF-8-character, so trim to the byte
/// after the first newline before decoding).
fn read_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut tail = Vec::new();
    if len > max_bytes {
        file.seek(SeekFrom::Start(len - max_bytes))?;
        file.read_to_end(&mut tail)?;
        let first_line_end = tail.iter().position(|&b| b == b'\n').map(|i| i + 1);
        tail.drain(..first_line_end.unwrap_or(tail.len()));
    } else {
        file.read_to_end(&mut tail)?;
    }
    Ok(String::from_utf8_lossy(&tail).into_owned())
}

/// SIGTERM the daemon named by its pidfile; quiet no-op if none is running.
fn stop_daemon() {
    let Ok(pid) = std::fs::read_to_string(diktafon_protocol::pid_path()) else {
        return;
    };
    let pid = pid.trim();
    if !pid_is_diktafond(pid.parse().unwrap_or(0)) {
        return;
    }
    let _ = std::process::Command::new("kill")
        .args(["-TERM", pid])
        .status();
}

/// Create the status item and keep it (and its controller) alive for the
/// app's lifetime via the phase observer's clone.
pub fn install(cx: &mut App, dictation: &Entity<Dictation>) {
    let mtm = MainThreadMarker::new().expect("not on the main thread");
    let (actions_tx, mut actions_rx) = unbounded();
    let controller = MenuController::new(mtm, actions_tx);

    let status_bar = NSStatusBar::systemStatusBar();
    let item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
    let menu = NSMenu::new(mtm);
    // NSMenu holds its delegate weakly; the controller is owned by the phase
    // observer below.
    menu.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    item.setMenu(Some(&menu));
    let _ = controller.ivars().status_item.set(item);
    controller.set_phase(Phase::Idle);

    cx.observe(dictation, {
        let controller = controller.clone();
        move |dictation, cx| controller.set_phase(dictation.read(cx).phase)
    })
    .detach();

    cx.spawn(async move |cx| {
        while let Some(action) = actions_rx.next().await {
            if let MenuAction::QuitDaemonToo = action {
                crate::transport::disable_daemon_spawn();
                stop_daemon();
            }
            cx.update(|cx| cx.quit());
        }
    })
    .detach();
}
