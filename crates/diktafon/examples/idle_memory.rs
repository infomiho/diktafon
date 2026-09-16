//! Checks native view teardown after repeated normal and popup window closure.
//! Run with `cargo run -p diktafon --example idle_memory` on macOS.
//! Add `-- --keep-alive` to inspect idle memory or `-- --without-cleanup`
//! to verify the regression fails without the workaround.

#[path = "../src/window_lifecycle.rs"]
mod window_lifecycle;

use gpui::{AppContext, Context, IntoElement, Render, Window, WindowKind, WindowOptions, div};
use objc2::rc::Weak;
use objc2_app_kit::NSView;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::Duration;

struct Probe;

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn native_view(window: &Window) -> &NSView {
    let handle = HasWindowHandle::window_handle(window).unwrap();
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        unreachable!("macOS probe");
    };
    // The window owns the view for the duration of this borrow.
    unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() }
}

fn main() {
    let without_cleanup = std::env::args().any(|arg| arg == "--without-cleanup");
    let keep_alive = std::env::args().any(|arg| arg == "--keep-alive");
    gpui_platform::application()
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx| {
            cx.spawn(async move |cx| {
                for cycle in 0..20 {
                    let popup = cycle % 2 == 0;
                    let mut view = None;
                    let handle = cx.update(|cx| {
                        cx.open_window(
                            WindowOptions {
                                kind: if popup {
                                    WindowKind::PopUp
                                } else {
                                    WindowKind::Normal
                                },
                                ..Default::default()
                            },
                            |window, cx| {
                                if !without_cleanup {
                                    window_lifecycle::release_view_on_close(window, cx);
                                }
                                view = Some(Weak::new(native_view(window)));
                                cx.new(|_| Probe)
                            },
                        )
                        .unwrap()
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(300))
                        .await;
                    if popup {
                        handle
                            .update(cx, |_, window, _| window.remove_window())
                            .unwrap();
                    } else {
                        let native_window = handle
                            .update(cx, |_, window, _| native_view(window).window().unwrap())
                            .unwrap();
                        native_window.performClose(None);
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(300))
                        .await;
                    assert!(
                        view.unwrap().load().is_none(),
                        "native view survived closure at cycle {}",
                        cycle + 1
                    );
                }
                println!(
                    "PASS: all 20 native views released, pid {}",
                    std::process::id()
                );
                if !keep_alive {
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
        });
}
