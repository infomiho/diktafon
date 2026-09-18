//! In-app updates through the Sparkle framework that `package-app.sh` embeds.
//!
//! Sparkle only runs in a release build whose bundle carries the framework
//! and a feed URL. Debug builds, `bundle.sh` bundles and ad-hoc packaging
//! runs stay inert, so a development build never offers to replace itself
//! with a release. The update windows are Sparkle's own. A small delegate
//! mirrors what Sparkle found into [`UpdateStatus`] so the settings window
//! can show it.
//!
//! Sparkle's classes exist only after its framework loads, so the controller
//! is reached by name through the runtime and the delegate is a plain
//! NSObject subclass: Sparkle asks `respondsToSelector:` rather than
//! checking protocol conformance.

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, AppContext, Entity, Global};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_foundation::{MainThreadMarker, NSBundle, NSObject, NSObjectProtocol, NSString};
use std::sync::OnceLock;

/// The choice in Sparkle's window that hides this version for good.
const USER_UPDATE_CHOICE_SKIP: isize = 0;

/// What Sparkle last reported, watched by the settings window.
#[derive(Default)]
pub struct UpdateStatus {
    pub check: UpdateCheck,
}

/// The outcome of Sparkle's most recent check, by display version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateCheck {
    #[default]
    Unknown,
    UpToDate,
    Available(String),
    Skipped(String),
}

/// Sparkle's standard updater controller and its delegate, kept alive for
/// the whole process.
pub struct Updater {
    controller: Retained<AnyObject>,
    _delegate: Retained<UpdaterDelegate>,
    status: Entity<UpdateStatus>,
}

impl Global for Updater {}

enum UpdateEvent {
    Found(String),
    NotFound,
    Skipped(String),
}

static EVENTS: OnceLock<UnboundedSender<UpdateEvent>> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "DiktafonUpdaterDelegate"]
    struct UpdaterDelegate;

    unsafe impl NSObjectProtocol for UpdaterDelegate {}

    impl UpdaterDelegate {
        #[unsafe(method(updater:didFindValidUpdate:))]
        fn did_find_valid_update(&self, _updater: Option<&AnyObject>, item: Option<&AnyObject>) {
            if let Some(version) = display_version(item) {
                report(UpdateEvent::Found(version));
            }
        }

        #[unsafe(method(updaterDidNotFindUpdate:))]
        fn did_not_find_update(&self, _updater: Option<&AnyObject>) {
            report(UpdateEvent::NotFound);
        }

        #[unsafe(method(updater:userDidMakeChoice:forUpdate:state:))]
        fn user_did_make_choice(
            &self,
            _updater: Option<&AnyObject>,
            choice: isize,
            item: Option<&AnyObject>,
            _state: Option<&AnyObject>,
        ) {
            if choice != USER_UPDATE_CHOICE_SKIP {
                return;
            }
            if let Some(version) = display_version(item) {
                report(UpdateEvent::Skipped(version));
            }
        }
    }
);

impl UpdaterDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Starts Sparkle when this bundle is meant to update itself and records
/// the controller so the menu and settings can reach it.
pub fn start(cx: &mut App) -> bool {
    if !should_start(cfg!(debug_assertions), feed_url().is_some()) {
        return false;
    }
    if !load_framework() {
        eprintln!("updater: Sparkle framework missing from the bundle");
        return false;
    }
    let Some(controller_class) = AnyClass::get(c"SPUStandardUpdaterController") else {
        eprintln!("updater: Sparkle framework loaded without its controller");
        return false;
    };
    let (sender, mut receiver) = unbounded();
    if EVENTS.set(sender).is_err() {
        eprintln!("updater: already started");
        return false;
    }
    let status = cx.new(|_| UpdateStatus::default());
    cx.spawn({
        let status = status.clone();
        async move |cx| {
            while let Some(event) = receiver.next().await {
                cx.update(|cx| {
                    status.update(cx, |status, cx| {
                        status.check = match event {
                            UpdateEvent::Found(version) => UpdateCheck::Available(version),
                            UpdateEvent::NotFound => UpdateCheck::UpToDate,
                            UpdateEvent::Skipped(version) => UpdateCheck::Skipped(version),
                        };
                        cx.notify();
                    })
                });
            }
        }
    })
    .detach();
    let mtm = MainThreadMarker::new().expect("not on the main thread");
    let delegate = UpdaterDelegate::new(mtm);
    let controller: Option<Retained<AnyObject>> = unsafe {
        let allocated: Allocated<AnyObject> = msg_send![controller_class, alloc];
        msg_send![
            allocated,
            initWithStartingUpdater: true,
            updaterDelegate: &*delegate,
            userDriverDelegate: None::<&AnyObject>,
        ]
    };
    let Some(controller) = controller else {
        eprintln!("updater: Sparkle refused to start");
        return false;
    };
    cx.set_global(Updater {
        controller,
        _delegate: delegate,
        status,
    });
    println!("[updater] started");
    true
}

/// Runs a user-initiated update check with Sparkle's own progress windows.
pub fn check_for_updates(cx: &mut App) {
    let Some(updater) = cx.try_global::<Updater>() else {
        return;
    };
    let _: () = unsafe { msg_send![&*updater.controller, checkForUpdates: None::<&AnyObject>] };
}

/// What Sparkle found, or `None` when this build does not update itself.
pub fn status(cx: &App) -> Option<Entity<UpdateStatus>> {
    cx.try_global::<Updater>()
        .map(|updater| updater.status.clone())
}

/// Whether Sparkle looks for updates on its own, or `None` when this build
/// does not update itself. Sparkle persists the choice.
pub fn automatic_checks(cx: &App) -> Option<bool> {
    let updater = cx.try_global::<Updater>()?;
    let enabled: bool =
        unsafe { msg_send![&*sparkle_updater(updater), automaticallyChecksForUpdates] };
    Some(enabled)
}

pub fn set_automatic_checks(enabled: bool, cx: &mut App) {
    let Some(updater) = cx.try_global::<Updater>() else {
        return;
    };
    let _: () =
        unsafe { msg_send![&*sparkle_updater(updater), setAutomaticallyChecksForUpdates: enabled] };
}

fn sparkle_updater(updater: &Updater) -> Retained<AnyObject> {
    unsafe { msg_send![&*updater.controller, updater] }
}

fn should_start(debug_build: bool, has_feed: bool) -> bool {
    !debug_build && has_feed
}

fn feed_url() -> Option<String> {
    let value =
        NSBundle::mainBundle().objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"))?;
    let url = value.downcast_ref::<NSString>()?.to_string();
    (!url.is_empty()).then_some(url)
}

fn load_framework() -> bool {
    let Some(frameworks) = NSBundle::mainBundle().privateFrameworksPath() else {
        return false;
    };
    let path = format!("{frameworks}/Sparkle.framework");
    let Some(bundle) = NSBundle::bundleWithPath(&NSString::from_str(&path)) else {
        return false;
    };
    unsafe { bundle.load() }
}

fn display_version(item: Option<&AnyObject>) -> Option<String> {
    let version: Option<Retained<NSString>> = unsafe { msg_send![item?, displayVersionString] };
    Some(version?.to_string())
}

fn report(event: UpdateEvent) {
    if let Some(events) = EVENTS.get() {
        let _ = events.unbounded_send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::should_start;

    #[test]
    fn only_release_builds_with_a_feed_update_themselves() {
        assert!(should_start(false, true));
        assert!(!should_start(true, true));
        assert!(!should_start(false, false));
        assert!(!should_start(true, false));
    }
}
