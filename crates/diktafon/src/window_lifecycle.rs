//! Workaround for native views retained after GPUI window closure on macOS.
//! https://github.com/longbridge/gpui-kit/issues/3052

use gpui::{App, Global, Window, WindowId};
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::HashMap;

#[derive(Default)]
struct WindowViews(HashMap<WindowId, Retained<NSView>>);

impl Global for WindowViews {}

pub fn release_view_on_close(window: &Window, cx: &mut App) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // The live GPUI window owns this AppKit view. Keep it alive until GPUI's
    // window teardown has stopped display callbacks and dropped its renderer handle.
    let view = unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }
        .expect("live window has a native view");
    if !cx.has_global::<WindowViews>() {
        cx.set_global(WindowViews::default());
        cx.on_window_closed(|cx, id| {
            if let Some(view) = cx.global_mut::<WindowViews>().0.remove(&id) {
                cx.defer(move |_| {
                    // AppKit can retain closed views through notification callbacks.
                    // Detaching them releases those callbacks and their Metal resources.
                    view.removeFromSuperview();
                });
            }
        })
        .detach();
    }
    cx.global_mut::<WindowViews>()
        .0
        .insert(Window::window_handle(window).window_id(), view);
}
