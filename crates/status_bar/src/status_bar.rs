use gpui::{div, prelude::*, px, rems, rgb, text};

pub struct StatusBar {}

impl StatusBar {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {}
    }
}

impl Render for StatusBar {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::prelude::Context<Self>,
    ) -> impl gpui::prelude::IntoElement {
        div()
            .flex() // Make it into a component
            .flex_row()
            .items_center()
            .w_full()
            .gap(rems(0.8))
            .p(rems(0.4))
            .bg(rgb(0xffffff))
            .child(div().line_height(px(12.)).child(text!("Status...")))
    }
}
