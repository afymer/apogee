mod bevy_viewport;

use gpui::{App, Context, Entity, Window, WindowOptions, div, prelude::*, px, rgb};
use gpui_component::button::{Button, ButtonVariants};
use gpui_platform::application;

use crate::bevy_viewport::BevyViewportView;

struct ApogeeApp {
    viewport: Entity<BevyViewportView>,
}

impl Render for ApogeeApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = self.viewport.clone();

        div().size_full().flex().flex_row().children([
            div()
                .w(px(200.))
                .h_full()
                .bg(rgb(0xaa2222))
                .child(
                    Button::new("refresh_rendering")
                        .primary()
                        .label("Refresh rendering")
                        .on_click(move |_event, _window, cx| {
                            viewport.update(cx, |view, _cx| {
                                view.request_redraw();
                            });
                        }),
                )
                .into_any_element(),
            div()
                .h_full()
                .flex_1()
                .bg(rgb(0x111111))
                .child(self.viewport.clone())
                .into_any_element(),
            div()
                .w(px(200.))
                .h_full()
                .bg(rgb(0xaa2222))
                .into_any_element(),
        ])
    }
}

impl ApogeeApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let viewport = BevyViewportView::build(window, cx, 64, 64);

        cx.observe(&viewport, |_this, _viewport, cx| {
            cx.notify();
        })
        .detach();

        Self { viewport }
    }
}

pub fn run() {
    application().run(|cx: &mut App| {
        gpui_component::init(cx);

        if let Err(e) = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| ApogeeApp::new(window, cx))
        }) {
            tracing::error!("Failed to open main window: {e:?}");
        }
    });
}
