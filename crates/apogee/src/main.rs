mod app_state;
mod bevy_viewport;

use gpui::{App, AppContext, WindowOptions};
use gpui_platform::application;
use theme::ThemeMode;
use workspace::workbench::Workbench;

use crate::app_state::AppState;

fn main() {
    tracing_subscriber::fmt::init();

    application().run(|cx: &mut App| {
        bevy_viewport::init(cx);
        theme::init(cx, ThemeMode::Dark);

        cx.set_global(AppState::new());

        if let Err(e) = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| Workbench::new(window, cx))
        }) {
            tracing::error!("Failed to open main window: {e:?}");
        }
    });
}
