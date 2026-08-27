use gpui::{App, Application, Context, Window, WindowOptions, div, prelude::*, rgb};

#[derive(Default)]
struct ApogeeApp {}

impl Render for ApogeeApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().bg(rgb(0x888888))
    }
}

pub fn run() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| ApogeeApp::default())
        })
        .unwrap();
    });
}
