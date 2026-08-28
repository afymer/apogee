use gpui::{App, Context, Window, WindowOptions, div, prelude::*, px, rgb};
use gpui_platform::application;

#[derive(Default)]
struct ApogeeApp {}

impl Render for ApogeeApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_row().children([
            div().w(px(250.0)).h_full().flex_none().bg(rgb(0x00aa00)),
            div().flex_1().h_full().bg(rgb(0x1e1e2e)),
            div().w(px(250.0)).h_full().flex_none().bg(rgb(0xaa0000)),
        ])
    }
}

pub fn run() {
    application().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| ApogeeApp::default())
        })
        .unwrap();
    });
}
