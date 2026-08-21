//! Menu bar presence: the app is otherwise invisible when idle and
//! unquittable without pkill. An NSStatusItem whose icon tracks the phase,
//! with a menu (rebuilt on every open via NSMenuDelegate) showing daemon and
//! model status, opening the settings window, and quitting.

use crate::dictation::{Dictation, Phase};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, Entity};
use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use std::cell::{Cell, OnceCell};

enum MenuAction {
    OpenSettings,
    Quit,
    QuitDaemonToo,
}

struct ControllerIvars {
    actions: UnboundedSender<MenuAction>,
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
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: &NSMenuItem) {
            let _ = self.ivars().actions.unbounded_send(MenuAction::OpenSettings);
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
            status_item: OnceCell::new(),
            phase: Cell::new(Phase::Idle),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn set_phase(&self, phase: Phase) {
        self.ivars().phase.set(phase);
        if let Some(item) = self.ivars().status_item.get()
            && let Some(button) = item.button(self.mtm())
        {
            button.setImage(Some(&orbit_icon(phase)));
        }
    }

    fn rebuild(&self, menu: &NSMenu) {
        let mtm = self.mtm();
        menu.removeAllItems();

        self.add_info(
            menu,
            &format!("Diktafon: {}", phase_label(self.ivars().phase.get())),
        );
        self.add_info(menu, &format!("Daemon: {}", daemon_summary()));
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        self.add_action(menu, "Settings…", sel!(openSettings:));
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

/// The app's mark, drawn instead of shipped: the pill's satellite orbit as a
/// ring of dots, rendered as a template image so the menu bar recolors it for
/// light/dark and for the pressed state. The phase modulates the ring the
/// same way the pill does: a center dot while recording, alternating dot
/// sizes while processing.
fn orbit_icon(phase: Phase) -> Retained<NSImage> {
    const SIZE: f64 = 18.;
    const DOTS: usize = 8;
    const RING_RADIUS: f64 = 6.4;
    let handler = block2::RcBlock::new(move |_rect| {
        objc2_app_kit::NSColor::blackColor().set();
        let dot = |x: f64, y: f64, radius: f64| {
            let rect = objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(x - radius, y - radius),
                objc2_foundation::NSSize::new(radius * 2., radius * 2.),
            );
            objc2_app_kit::NSBezierPath::bezierPathWithOvalInRect(rect).fill();
        };
        let center = SIZE / 2.;
        for i in 0..DOTS {
            let angle = i as f64 * std::f64::consts::TAU / DOTS as f64;
            let radius = match phase {
                // Alternating dot sizes read as "working".
                Phase::Transcribing | Phase::Polishing if i % 2 == 0 => 1.7,
                Phase::Transcribing | Phase::Polishing => 1.1,
                _ => 1.4,
            };
            dot(
                center + angle.cos() * RING_RADIUS,
                center + angle.sin() * RING_RADIUS,
                radius,
            );
        }
        if matches!(phase, Phase::Arming | Phase::Recording) {
            dot(center, center, 2.4);
        }
        objc2::runtime::Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        objc2_foundation::NSSize::new(SIZE, SIZE),
        false,
        &handler,
    );
    image.setTemplate(true);
    image
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

/// Summarizes the daemon from its status.json (see
/// `diktafon_protocol::status_path`); the pid check guards against a stale
/// file left by a crashed daemon. Shared with the settings window.
pub fn daemon_summary() -> String {
    let path = diktafon_protocol::status_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        // A daemon from before the status file exists only in the pidfile.
        let pid = std::fs::read_to_string(diktafon_protocol::pid_path())
            .ok()
            .and_then(|pid| pid.trim().parse().ok())
            .unwrap_or(0);
        return if pid_alive(pid) {
            "running".into()
        } else {
            "not running".into()
        };
    };
    let Ok(status) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return "unknown".into();
    };
    let pid = status["pid"].as_u64().unwrap_or(0);
    if !pid_alive(pid) {
        return "not running".into();
    }
    let asr = status["asr_model"].as_str().unwrap_or("asr");
    let llm = status["llm_model"].as_str().unwrap_or("llm");
    if status["models_loaded"].as_bool().unwrap_or(false) {
        format!("running · {asr} + {llm} loaded")
    } else {
        // Idle unload is by design; loads happen on the next dictation.
        "running · models idle".into()
    }
}

/// Cheap liveness check (no subprocess: this runs while the menu opens). A
/// recycled pid can fool it, unlike the comm check `stop_daemon` uses, but
/// here it only mislabels a dead daemon as running until the next dictation.
fn pid_alive(pid: u64) -> bool {
    pid != 0 && unsafe { libc::kill(pid as i32, 0) } == 0
}

/// Strict check for the destructive path: the pid must still name a
/// diktafond, since pids get recycled.
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
pub fn install(
    cx: &mut App,
    dictation: &Entity<Dictation>,
    settings: std::sync::Arc<std::sync::Mutex<crate::config::SessionSettings>>,
) {
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
        let mut settings_window = None;
        while let Some(action) = actions_rx.next().await {
            match action {
                MenuAction::OpenSettings => {
                    let settings = settings.clone();
                    cx.update(|cx| {
                        settings_window = crate::settings::open(settings_window, settings, cx);
                    });
                }
                MenuAction::QuitDaemonToo => {
                    crate::transport::disable_daemon_spawn();
                    stop_daemon();
                    cx.update(|cx| cx.quit());
                }
                MenuAction::Quit => {
                    cx.update(|cx| cx.quit());
                }
            }
        }
    })
    .detach();
}
