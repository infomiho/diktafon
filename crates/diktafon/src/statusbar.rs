//! Menu bar presence: the app is otherwise invisible when idle and
//! unquittable without pkill. An NSStatusItem whose icon tracks the phase,
//! with a menu (rebuilt on every open via NSMenuDelegate) showing daemon and
//! model status, opening the settings window, and quitting.

use crate::dictation::{Dictation, Phase};
use crate::mark;
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
            button.setImage(Some(&mark_icon(phase)));
        }
    }

    fn rebuild(&self, menu: &NSMenu) {
        let mtm = self.mtm();
        menu.removeAllItems();

        self.add_info(
            menu,
            &format!("Diktafon: {}", phase_label(self.ivars().phase.get())),
        );
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        self.add_action(menu, "Settings…", sel!(openSettings:));
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        self.add_action(menu, "Quit Diktafon", sel!(quit:));
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

    fn add_action(&self, menu: &NSMenu, title: &str, action: Sel) {
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
    }
}

/// The app's mark, rendered as a template image so the menu bar recolors it
/// for light/dark and for the pressed state. Geometry comes from `mark` (the
/// single source shared with the app icon and the settings brand row); the
/// phase modulates the face: a hub dot in the dial while recording,
/// alternating grille dot sizes while processing.
fn mark_icon(phase: Phase) -> Retained<NSImage> {
    const WIDTH: f64 = 20.;
    const HEIGHT: f64 = 18.;
    const SCALE: f64 = 0.475;
    // Menu-bar pixels need more meat than a straight scale gives: the holes
    // and the hub get a small optical boost.
    const HOLE_BOOST: f64 = 1.15;
    const HUB_BOOST: f64 = 1.3;
    let handler = block2::RcBlock::new(move |_rect| {
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let origin = NSPoint::new(
            (WIDTH - f64::from(mark::BODY_W) * SCALE) / 2.,
            (HEIGHT - f64::from(mark::BODY_H) * SCALE) / 2.,
        );
        let oval = |path: &objc2_app_kit::NSBezierPath, x: f32, y: f32, radius: f64| {
            let x = origin.x + f64::from(x - mark::BODY_X) * SCALE;
            let y = origin.y + f64::from(y - mark::BODY_Y) * SCALE;
            let rect = NSRect::new(
                NSPoint::new(x - radius, y - radius),
                NSSize::new(radius * 2., radius * 2.),
            );
            path.appendBezierPathWithOvalInRect(rect);
        };
        objc2_app_kit::NSColor::blackColor().set();

        // Face with the dial and grille punched out via even-odd.
        let body = NSRect::new(
            origin,
            NSSize::new(
                f64::from(mark::BODY_W) * SCALE,
                f64::from(mark::BODY_H) * SCALE,
            ),
        );
        let radius = f64::from(mark::BODY_R) * SCALE;
        let face = objc2_app_kit::NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            body, radius, radius,
        );
        face.setWindingRule(objc2_app_kit::NSWindingRule::EvenOdd);
        oval(
            &face,
            mark::DIAL_X,
            mark::DIAL_Y,
            f64::from(mark::DIAL_R) * SCALE,
        );
        for (i, (gx, gy)) in mark::grille().enumerate() {
            let dot = f64::from(mark::GRILLE_R) * SCALE * HOLE_BOOST;
            let dot = match phase {
                // Alternating dot sizes read as "working".
                Phase::Transcribing | Phase::Polishing if i % 2 == 0 => dot * 1.3,
                Phase::Transcribing | Phase::Polishing => dot * 0.75,
                _ => dot,
            };
            oval(&face, gx, gy, dot);
        }
        face.fill();

        // The dial hub is the REC light.
        if matches!(phase, Phase::Arming | Phase::Recording) {
            let hub = objc2_app_kit::NSBezierPath::new();
            oval(
                &hub,
                mark::DIAL_X,
                mark::DIAL_Y,
                f64::from(mark::HUB_R) * SCALE * HUB_BOOST,
            );
            hub.fill();
        }
        objc2::runtime::Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        objc2_foundation::NSSize::new(WIDTH, HEIGHT),
        false,
        &handler,
    );
    image.setTemplate(true);
    image
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "Ready",
        Phase::Arming => "Starting",
        Phase::Recording => "Listening",
        Phase::Transcribing => "Transcribing",
        Phase::Polishing => "Polishing",
    }
}

/// The daemon's state as read from its status.json, for structured display
/// in the settings window.
pub struct DaemonStatus {
    pub running: bool,
    pub models_loaded: bool,
    pub asr: Option<String>,
    pub llm: Option<String>,
}

/// Reads the daemon state from its status.json (see
/// `diktafon_protocol::status_path`); the pid check guards against a stale
/// file left by a crashed daemon.
pub fn daemon_status() -> DaemonStatus {
    let not_running = DaemonStatus {
        running: false,
        models_loaded: false,
        asr: None,
        llm: None,
    };
    let Ok(raw) = std::fs::read_to_string(diktafon_protocol::status_path()) else {
        // A daemon from before the status file exists only in the pidfile.
        let pid = std::fs::read_to_string(diktafon_protocol::pid_path())
            .ok()
            .and_then(|pid| pid.trim().parse().ok())
            .unwrap_or(0);
        return DaemonStatus {
            running: pid_alive(pid),
            ..not_running
        };
    };
    let Ok(status) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return not_running;
    };
    if !pid_alive(status["pid"].as_u64().unwrap_or(0)) {
        return not_running;
    }
    DaemonStatus {
        running: true,
        // Idle unload is by design; loads happen on the next dictation.
        models_loaded: status["models_loaded"].as_bool().unwrap_or(false),
        asr: status["asr_model"].as_str().map(str::to_string),
        llm: status["llm_model"].as_str().map(str::to_string),
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
                // Quit takes the daemon down too: with exit-on-idle there
                // is nothing worth keeping warm after the client leaves.
                MenuAction::Quit => {
                    crate::transport::disable_daemon_spawn();
                    stop_daemon();
                    cx.update(|cx| cx.quit());
                }
            }
        }
    })
    .detach();
}
