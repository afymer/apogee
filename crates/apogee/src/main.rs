mod app_state;
mod bevy_viewport;

use gpui::{App, AppContext, WindowOptions};
use gpui_platform::application;
use workspace::workbench::Workbench;

use crate::app_state::AppState;

fn main() {
    tracing_subscriber::fmt::init();

    application().run(|cx: &mut App| {
        gpui_component::init(cx);
        bevy_viewport::init(cx);

        cx.set_global(AppState::new());

        if let Err(e) = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| Workbench::new(window, cx))
        }) {
            tracing::error!("Failed to open main window: {e:?}");
        }
    });
}
