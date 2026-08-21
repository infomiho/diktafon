//! Minimal gpui text-rendering lab: a normal focused window with three rows -
//! bare text, text styled like the pill label, and the full pill structure.
//! `cargo run -p diktafon --example pill_lab`

use gpui::{
    App, AppContext, Application, Bounds, Context, IntoElement, ParentElement, Render, Styled,
    Window, WindowBounds, WindowOptions, div, point, px, rgb, rgba, size,
};

struct Lab;

impl Render for Lab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0x202025))
            .child("row 1: bare text child")
            .child(
                div()
                    .text_sm()
                    .text_color(rgba(0xFFFFFFD9))
                    .child("row 2: styled like the pill label"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .h(px(46.))
                    .rounded_full()
                    .bg(rgba(0x16161AE8))
                    .border_1()
                    .border_color(rgba(0xFFFFFF14))
                    .child(div().size_2().rounded_full().bg(rgb(0xF0B429)))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgba(0xFFFFFFD9))
                            .max_w(px(210.))
                            .overflow_hidden()
                            .child("row 3: full pill structure"),
                    ),
            )
    }
}

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .init();
    Application::with_platform(gpui_platform::current_platform(false)).run(|cx: &mut App| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(200.), px(200.)),
                    size: size(px(420.), px(220.)),
                })),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Lab),
        )
        .unwrap();
        cx.activate(true);
    });
}
